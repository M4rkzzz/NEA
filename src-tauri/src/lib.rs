use base64::{engine::general_purpose, Engine};
use chrono::Utc;
use futures::{future::pending, AsyncWriteExt};
use keyring::Entry;
use magic_wormhole::{transfer, transit, Code, MailboxConnection, Wormhole};
use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{HashMap, HashSet},
    fs,
    io::{ErrorKind, Read, Write},
    path::{Component, Path, PathBuf},
    process::Command,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc, Arc, Mutex,
    },
    thread,
    time::{Duration, Instant},
};
use sysinfo::{Pid, ProcessRefreshKind, Signal, System, UpdateKind};
use tauri::{
    menu::{Menu, MenuItem, PredefinedMenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    AppHandle, Emitter, LogicalSize, Manager, PhysicalPosition, State, WebviewUrl, WebviewWindow,
    WebviewWindowBuilder, WindowEvent,
};
use uuid::Uuid;
use webview2_com::Microsoft::Web::WebView2::Win32::{
    ICoreWebView2_19, COREWEBVIEW2_MEMORY_USAGE_TARGET_LEVEL_LOW,
    COREWEBVIEW2_MEMORY_USAGE_TARGET_LEVEL_NORMAL,
};
use windows::core::w;
use windows::Win32::Foundation::{CloseHandle, GetLastError, BOOL, HANDLE, HWND, LPARAM, RECT};
use windows::Win32::System::Threading::CreateMutexW;
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_LBUTTON};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetAncestor, GetForegroundWindow, GetWindowRect, GetWindowThreadProcessId,
    IsIconic, IsWindow, IsWindowVisible, SetWindowLongPtrW, GA_ROOTOWNER, GWLP_HWNDPARENT,
};
use windows_core::Interface;
use winreg::{enums::HKEY_CURRENT_USER, RegKey};
use zip::{write::SimpleFileOptions, CompressionMethod, ZipArchive, ZipWriter};

mod adapters;
mod steam;
use adapters::AppAdapter;

const APP_DIR_NAME: &str = "NEA";
const LEGACY_APP_DIR_NAME: &str = "OOPZ+";
const CREDENTIAL_SERVICE: &str = "NEA";
const LEGACY_CREDENTIAL_SERVICE: &str = "OOPZ+";
const WATCHER_FILE_NAME: &str = "oopz-plus-watcher.exe";
const RUN_KEY_PATH: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Run";

fn process_refresh_kind() -> ProcessRefreshKind {
    ProcessRefreshKind::new()
        .with_cmd(UpdateKind::Always)
        .with_exe(UpdateKind::Always)
}

fn process_system() -> System {
    let mut system = System::new();
    system.refresh_processes_specifics(process_refresh_kind());
    system
}

fn refresh_process_system(system: &mut System) {
    system.refresh_processes_specifics(process_refresh_kind());
}
const RUN_KEY_NAME: &str = "OOPZ+ Watcher";
const LEGACY_EXPORT_FORMAT: &str = "oopz-plus-account-v1";
const EXPORT_FORMAT: &str = "oopz-plus-package-v2";
const EXPORT_FORMAT_V3: &str = "oopz-plus-package-v3";
const NEA_EXPORT_FORMAT_V1: &str = "nea-package-v1";
const MAX_EXPORT_PACKAGE_BYTES: u64 = 512 * 1024 * 1024;
const MAX_V3_ARCHIVE_BYTES: u64 = 528 * 1024 * 1024;
const MAX_LEGACY_EXPORT_PACKAGE_BYTES: u64 = 128 * 1024 * 1024;
const MAX_EXPORT_ACCOUNTS: usize = 100;
const MAX_EXPORT_FILES: usize = 100_000;
const WORMHOLE_TIMEOUT_SECONDS: u64 = 10 * 60;
const WORMHOLE_CODE_WORDS: usize = 4;
const QUICK_SHARE_CANCELLED: &str = "快捷分享已取消";
const MAX_AVATAR_BYTES: u64 = 2 * 1024 * 1024;
const GITHUB_LATEST_RELEASE_URL: &str =
    "https://api.github.com/repos/M4rkzzz/oopz-plus/releases/latest";
const GITHUB_DOWNLOAD_PROXY_PREFIX: &str = "https://gh-proxy.com/";
const MAX_UPDATE_BYTES: u64 = 150 * 1024 * 1024;
const UPDATE_CHECK_INTERVAL_MINUTES: i64 = 30;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct AppConfig {
    oopz_install_dir: Option<String>,
    oopz_exe_path: Option<String>,
    roaming_data_dir: Option<String>,
    local_sandbox_dir: Option<String>,
    #[serde(default)]
    plugin_mode_enabled: bool,
    #[serde(default)]
    plugin_autostart_enabled: bool,
    #[serde(default)]
    overlay_offset_x: i32,
    #[serde(default)]
    overlay_offset_y: i32,
    #[serde(default)]
    overlay_vertical: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    last_update_check_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SavedAccount {
    id: String,
    display_name: String,
    uid: Option<String>,
    pid: Option<String>,
    user_common_id: Option<String>,
    masked_phone: Option<String>,
    avatar_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    avatar_source_url: Option<String>,
    login_name: Option<String>,
    note: Option<String>,
    has_session_snapshot: bool,
    has_credential: bool,
    #[serde(default)]
    has_login_state: bool,
    created_at: String,
    updated_at: String,
    last_used_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AccountExportPackage {
    format: String,
    exported_at: String,
    accounts: Vec<ExportedAccountEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct V3AccountManifest {
    directory: String,
    account: ExportedAccount,
    oopz_login: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct V3ExportManifest {
    format: String,
    #[serde(default = "default_oopz_app_id")]
    app_id: String,
    exported_at: String,
    accounts: Vec<V3AccountManifest>,
}

fn default_oopz_app_id() -> String {
    "oopz".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ImportJournalEntry {
    account_id: String,
    had_snapshot: bool,
    credential_backup_id: String,
    phase: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ImportJournal {
    id: String,
    status: String,
    config_existed: bool,
    entries: Vec<ImportJournalEntry>,
}

struct PreparedImportAccount {
    account: SavedAccount,
    oopz_login: String,
    staged_snapshot: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LegacyAccountExportPackage {
    format: String,
    exported_at: String,
    account: ExportedAccount,
    oopz_login: String,
    files: Vec<ExportedFile>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ExportedAccountEntry {
    account: ExportedAccount,
    oopz_login: String,
    files: Vec<ExportedFile>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ExportedAccount {
    display_name: String,
    uid: Option<String>,
    pid: Option<String>,
    user_common_id: Option<String>,
    masked_phone: Option<String>,
    avatar_url: Option<String>,
    note: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ExportedFile {
    path: String,
    data_base64: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct AppData {
    #[serde(default)]
    schema_version: u32,
    config: AppConfig,
    accounts: Vec<SavedAccount>,
    #[serde(default)]
    steam: steam::SteamWorkspace,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    current_login_uid: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct OopzPaths {
    oopz_install_dir: String,
    oopz_exe_path: String,
    roaming_data_dir: String,
    local_sandbox_dir: String,
    source: String,
    valid: bool,
    message: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ImportedCandidate {
    uid: String,
    display_name: String,
    pid: Option<String>,
    user_common_id: Option<String>,
    masked_phone: Option<String>,
    avatar_url: Option<String>,
    has_roaming_state: bool,
    has_local_state: bool,
    last_write_time: Option<String>,
    has_current_login: bool,
    can_switch: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct SecretPayload {
    login_name: Option<String>,
    password: Option<String>,
    oopz_login: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SwitchResult {
    ok: bool,
    message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PluginStatus {
    plugin_mode_enabled: bool,
    watcher_installed: bool,
    watcher_running: bool,
    plugin_runtime_running: bool,
    oopz_running: bool,
    overlay_visible: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct UpdateStatus {
    state: String,
    current_version: String,
    available_version: Option<String>,
    message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    transferred: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    total: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    percent: Option<u64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct WormholeStatus {
    state: String,
    direction: String,
    message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    transferred: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    total: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct GitHubRelease {
    tag_name: String,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
    #[serde(default)]
    assets: Vec<GitHubAsset>,
}

#[derive(Debug, Deserialize)]
struct GitHubAsset {
    name: String,
    browser_download_url: String,
    size: u64,
    digest: Option<String>,
}

struct AppState {
    data: Mutex<AppData>,
    account_operation: Mutex<()>,
    switch_operation: Mutex<()>,
    discovery_cancelled: AtomicBool,
    auto_import_running: AtomicBool,
    plugin_operation: Mutex<()>,
    plugin_environment_running: AtomicBool,
    overlay_rebind_requested: AtomicBool,
    overlay_dragging: AtomicBool,
    update_running: AtomicBool,
    update_status: Mutex<UpdateStatus>,
    wormhole_running: AtomicBool,
    wormhole_cancelled: AtomicBool,
    main_webview_low_memory: AtomicBool,
}

struct NamedMutex(HANDLE);

impl Drop for NamedMutex {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

fn acquire_plugin_runtime_mutex() -> Result<Option<NamedMutex>, String> {
    unsafe {
        let handle = CreateMutexW(None, false, w!("Local\\OOPZPlus.PluginRuntime"))
            .map_err(|e| format!("创建插件单实例锁失败: {}", e))?;
        if GetLastError().is_err() {
            let _ = CloseHandle(handle);
            Ok(None)
        } else {
            Ok(Some(NamedMutex(handle)))
        }
    }
}

fn now() -> String {
    Utc::now().to_rfc3339()
}

fn home_env(name: &str) -> Result<PathBuf, String> {
    std::env::var_os(name)
        .map(PathBuf::from)
        .ok_or_else(|| format!("环境变量 {} 不存在", name))
}

fn storage_dir() -> Result<PathBuf, String> {
    let base = home_env("APPDATA")?;
    let current = base.join(APP_DIR_NAME);
    let legacy = base.join(LEGACY_APP_DIR_NAME);
    if !current.exists() && legacy.exists() {
        fs::create_dir_all(&current).map_err(|error| error.to_string())?;
        for entry in fs::read_dir(&legacy).map_err(|error| error.to_string())? {
            let entry = entry.map_err(|error| error.to_string())?;
            let destination = current.join(entry.file_name());
            if entry
                .file_type()
                .map_err(|error| error.to_string())?
                .is_dir()
            {
                copy_directory(&entry.path(), &destination)?;
            } else {
                fs::copy(entry.path(), destination).map_err(|error| error.to_string())?;
            }
        }
        let marker = current.join("migrated-from-oopz-plus.txt");
        let _ = fs::write(marker, now());
    }
    let oopz_workspace = current.join("workspaces").join("oopz");
    for folder in ["accounts", "backups"] {
        let legacy_folder = current.join(folder);
        let workspace_folder = oopz_workspace.join(folder);
        if legacy_folder.exists() && !workspace_folder.exists() {
            copy_directory(&legacy_folder, &workspace_folder)?;
        }
    }
    Ok(current)
}

fn copy_directory(source: &Path, destination: &Path) -> Result<(), String> {
    fs::create_dir_all(destination).map_err(|error| error.to_string())?;
    for entry in fs::read_dir(source).map_err(|error| error.to_string())? {
        let entry = entry.map_err(|error| error.to_string())?;
        let target = destination.join(entry.file_name());
        if entry
            .file_type()
            .map_err(|error| error.to_string())?
            .is_dir()
        {
            copy_directory(&entry.path(), &target)?;
        } else {
            fs::copy(entry.path(), target).map_err(|error| error.to_string())?;
        }
    }
    Ok(())
}

fn config_path() -> Result<PathBuf, String> {
    Ok(storage_dir()?.join("config.json"))
}

fn accounts_dir() -> Result<PathBuf, String> {
    Ok(storage_dir()?
        .join("workspaces")
        .join("oopz")
        .join("accounts"))
}

fn backups_dir() -> Result<PathBuf, String> {
    Ok(storage_dir()?
        .join("workspaces")
        .join("oopz")
        .join("backups"))
}

fn update_marker_path() -> Result<PathBuf, String> {
    Ok(storage_dir()?.join("update-completed.txt"))
}

fn update_error_path() -> Result<PathBuf, String> {
    Ok(storage_dir()?.join("update-error.txt"))
}

fn initial_update_status() -> UpdateStatus {
    UpdateStatus {
        state: "idle".to_string(),
        current_version: env!("CARGO_PKG_VERSION").to_string(),
        available_version: None,
        message: "自动更新已启用".to_string(),
        transferred: None,
        total: None,
        percent: None,
    }
}

fn set_update_status(
    app: &AppHandle,
    state_name: &str,
    available_version: Option<String>,
    message: impl Into<String>,
) -> UpdateStatus {
    let status = UpdateStatus {
        state: state_name.to_string(),
        current_version: env!("CARGO_PKG_VERSION").to_string(),
        available_version,
        message: message.into(),
        transferred: None,
        total: None,
        percent: None,
    };
    if let Ok(mut current) = app.state::<AppState>().update_status.lock() {
        *current = status.clone();
    }
    let _ = app.emit("update-status", status.clone());
    status
}

fn download_percent(transferred: u64, total: u64) -> u64 {
    if total == 0 {
        return 0;
    }
    ((u128::from(transferred) * 100) / u128::from(total)).min(100) as u64
}

fn set_update_progress(
    app: &AppHandle,
    version: &str,
    transferred: u64,
    total: u64,
    message: impl Into<String>,
) {
    let status = UpdateStatus {
        state: "downloading".to_string(),
        current_version: env!("CARGO_PKG_VERSION").to_string(),
        available_version: Some(version.to_string()),
        message: message.into(),
        transferred: Some(transferred),
        total: Some(total),
        percent: Some(download_percent(transferred, total)),
    };
    if let Ok(mut current) = app.state::<AppState>().update_status.lock() {
        *current = status.clone();
    }
    let _ = app.emit("update-status", status);
}

fn parse_release_version(value: &str) -> Option<([u64; 3], String)> {
    let value = value.trim().trim_start_matches(['v', 'V']);
    let parts: Vec<_> = value.split('.').collect();
    if parts.len() != 3 {
        return None;
    }
    let version = [
        parts[0].parse().ok()?,
        parts[1].parse().ok()?,
        parts[2].parse().ok()?,
    ];
    Some((
        version,
        format!("{}.{}.{}", version[0], version[1], version[2]),
    ))
}

fn update_check_due(config: &AppConfig) -> bool {
    let Some(last_check) = config.last_update_check_at.as_deref() else {
        return true;
    };
    let Ok(last_check) = chrono::DateTime::parse_from_rfc3339(last_check) else {
        return true;
    };
    Utc::now().signed_duration_since(last_check.with_timezone(&Utc))
        >= chrono::Duration::minutes(UPDATE_CHECK_INTERVAL_MINUTES)
}

fn record_update_check(app: &AppHandle) -> Result<(), String> {
    let state = app.state::<AppState>();
    let mut data = state.data.lock().map_err(|e| e.to_string())?;
    data.config.last_update_check_at = Some(now());
    save_data(&data)
}

fn validate_update_asset<'a>(asset: &'a GitHubAsset, version: &str) -> Result<&'a str, String> {
    let nea_name = format!("NEA_{}_x64_en-US.msi", version);
    let legacy_name = format!("OOPZ+_{}_x64_en-US.msi", version);
    if !asset.name.eq_ignore_ascii_case(&nea_name) && !asset.name.eq_ignore_ascii_case(&legacy_name)
    {
        return Err(format!(
            "Release 缺少安装包 {} 或 {}",
            nea_name, legacy_name
        ));
    }
    if asset.size == 0 || asset.size > MAX_UPDATE_BYTES {
        return Err("更新安装包大小异常".to_string());
    }
    if !asset
        .browser_download_url
        .starts_with("https://github.com/M4rkzzz/oopz-plus/releases/download/")
    {
        return Err("更新下载地址不可信".to_string());
    }
    asset
        .digest
        .as_deref()
        .and_then(|value| value.strip_prefix("sha256:"))
        .filter(|value| {
            value.len() == 64 && value.chars().all(|character| character.is_ascii_hexdigit())
        })
        .ok_or_else(|| "Release 安装包缺少 SHA-256 摘要，已拒绝自动安装".to_string())
}

fn github_proxy_url(asset: &GitHubAsset) -> String {
    format!(
        "{}{}",
        GITHUB_DOWNLOAD_PROXY_PREFIX, asset.browser_download_url
    )
}

fn download_update_asset_from_url(
    app: &AppHandle,
    asset: &GitHubAsset,
    download_url: &str,
    version: &str,
    expected_digest: &str,
) -> Result<PathBuf, String> {
    let expected_name = asset.name.clone();

    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(600))
        .build()
        .map_err(|e| e.to_string())?;
    let mut response = client
        .get(download_url)
        .header(reqwest::header::USER_AGENT, "NEA-Updater")
        .send()
        .map_err(|e| format!("下载更新失败: {}", e))?
        .error_for_status()
        .map_err(|e| format!("下载更新失败: {}", e))?;
    if response
        .content_length()
        .is_some_and(|size| size > MAX_UPDATE_BYTES)
    {
        return Err("更新下载内容过大".to_string());
    }

    let temp_dir = std::env::temp_dir();
    let partial = temp_dir.join(format!("oopz-plus-{}.msi.part", version));
    let target = temp_dir.join(&expected_name);
    let mut file = fs::File::create(&partial).map_err(|e| format!("创建更新文件失败: {}", e))?;
    let mut hasher = Sha256::new();
    let mut total = 0u64;
    let mut last_percent = 0u64;
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let count = response
            .read(&mut buffer)
            .map_err(|e| format!("读取更新失败: {}", e))?;
        if count == 0 {
            break;
        }
        total += count as u64;
        if total > MAX_UPDATE_BYTES {
            let _ = fs::remove_file(&partial);
            return Err("更新下载内容过大".to_string());
        }
        file.write_all(&buffer[..count])
            .map_err(|e| format!("保存更新失败: {}", e))?;
        hasher.update(&buffer[..count]);
        let percent = download_percent(total, asset.size).min(99);
        if percent > last_percent {
            last_percent = percent;
            set_update_progress(
                app,
                version,
                total.min(asset.size),
                asset.size,
                format!("正在下载 {}... {}%", version, percent),
            );
        }
    }
    file.sync_all().map_err(|e| e.to_string())?;
    drop(file);
    if total != asset.size {
        let _ = fs::remove_file(&partial);
        return Err("更新安装包大小校验失败".to_string());
    }
    let actual_digest = format!("{:x}", hasher.finalize());
    if !actual_digest.eq_ignore_ascii_case(expected_digest) {
        let _ = fs::remove_file(&partial);
        return Err("更新安装包 SHA-256 校验失败".to_string());
    }
    if target.exists() {
        fs::remove_file(&target).map_err(|e| e.to_string())?;
    }
    fs::rename(&partial, &target).map_err(|e| e.to_string())?;
    set_update_progress(
        app,
        version,
        asset.size,
        asset.size,
        format!("更新安装包 {} 下载并校验完成 100%", version),
    );
    Ok(target)
}

fn download_update_asset(
    app: &AppHandle,
    asset: &GitHubAsset,
    version: &str,
) -> Result<PathBuf, String> {
    let expected_digest = validate_update_asset(asset, version)?;
    let proxy_url = github_proxy_url(asset);
    set_update_progress(
        app,
        version,
        0,
        asset.size,
        format!("正在通过加速线路下载 {}... 0%", version),
    );
    match download_update_asset_from_url(app, asset, &proxy_url, version, expected_digest) {
        Ok(path) => Ok(path),
        Err(proxy_error) => {
            set_update_progress(
                app,
                version,
                0,
                asset.size,
                format!("加速线路不可用，正在通过 GitHub 下载 {}... 0%", version),
            );
            download_update_asset_from_url(
                app,
                asset,
                &asset.browser_download_url,
                version,
                expected_digest,
            )
            .map_err(|github_error| {
                format!(
                    "下载加速线路失败: {}; GitHub 直链失败: {}",
                    proxy_error, github_error
                )
            })
        }
    }
}

fn preferred_installed_exe(original_exe: &Path) -> PathBuf {
    let program_files_exes = std::env::var_os("ProgramFiles")
        .map(PathBuf::from)
        .map(|path| {
            vec![
                path.join("NEA").join("oopz-plus.exe"),
                path.join("OOPZ+").join("oopz-plus.exe"),
            ]
        });
    if original_exe
        .to_string_lossy()
        .to_ascii_lowercase()
        .contains("program files")
        && original_exe.exists()
    {
        original_exe.to_path_buf()
    } else if let Some(path) =
        program_files_exes.and_then(|paths| paths.into_iter().find(|path| path.exists()))
    {
        path
    } else {
        original_exe.to_path_buf()
    }
}

fn apply_update_helper(args: &[String]) {
    let Some(msi_path) = args.get(2).map(PathBuf::from) else {
        return;
    };
    let Some(parent_pid) = args.get(3).and_then(|value| value.parse::<u32>().ok()) else {
        return;
    };
    let Some(original_exe) = args.get(4).map(PathBuf::from) else {
        return;
    };
    let Some(version) = args.get(5) else {
        return;
    };

    let mut parent_exited = false;
    for _ in 0..120 {
        let system = process_system();
        if system.process(Pid::from_u32(parent_pid)).is_none() {
            parent_exited = true;
            break;
        }
        thread::sleep(Duration::from_millis(500));
    }

    if !parent_exited {
        if let Ok(error_path) = update_error_path() {
            let _ = fs::write(error_path, "自动安装失败：主程序未能及时退出");
        }
        return;
    }
    stop_watcher();
    stop_plugin_runtime();
    thread::sleep(Duration::from_millis(500));

    let msiexec = std::env::var_os("SystemRoot")
        .map(PathBuf::from)
        .map(|path| path.join("System32").join("msiexec.exe"))
        .unwrap_or_else(|| PathBuf::from("C:\\Windows\\System32\\msiexec.exe"));
    let status = Command::new(msiexec)
        .arg("/i")
        .arg(&msi_path)
        .arg("/passive")
        .arg("/norestart")
        .status();
    let exit_code = status.ok().and_then(|status| status.code()).unwrap_or(-1);
    let install_succeeded = matches!(exit_code, 0 | 1641 | 3010);
    let launch_exe = preferred_installed_exe(&original_exe);
    if install_succeeded {
        if let Ok(marker) = update_marker_path() {
            let _ = fs::write(marker, version);
        }
    } else if let Ok(error_path) = update_error_path() {
        let _ = fs::write(error_path, format!("自动安装失败，错误码 {}", exit_code));
    }
    let helper_path = std::env::current_exe().ok();
    let mut command = Command::new(launch_exe);
    if let Some(helper_path) = helper_path {
        command.arg("--cleanup-updater").arg(helper_path);
    }
    let _ = command.spawn();
    let _ = fs::remove_file(msi_path);
}

fn launch_update_installer(app: &AppHandle, msi_path: &Path, version: &str) -> Result<(), String> {
    let current_exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let helper = std::env::temp_dir().join(format!("oopz-plus-updater-{}.exe", Uuid::new_v4()));
    fs::copy(&current_exe, &helper).map_err(|e| format!("准备更新程序失败: {}", e))?;
    if let Err(error) = Command::new(&helper)
        .arg("--apply-update")
        .arg(msi_path)
        .arg(std::process::id().to_string())
        .arg(&current_exe)
        .arg(version)
        .spawn()
    {
        let _ = fs::remove_file(&helper);
        return Err(format!("启动更新安装失败: {}", error));
    }
    app.exit(0);
    Ok(())
}

fn perform_update_check(app: &AppHandle, force: bool) -> Result<(), String> {
    if !force {
        let state = app.state::<AppState>();
        let data = state.data.lock().map_err(|e| e.to_string())?;
        if !update_check_due(&data.config) {
            return Ok(());
        }
    }
    set_update_status(app, "checking", None, "正在检查 GitHub 更新...");
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(20))
        .build()
        .map_err(|e| e.to_string())?;
    let response = client
        .get(GITHUB_LATEST_RELEASE_URL)
        .header(reqwest::header::USER_AGENT, "NEA-Updater")
        .header(reqwest::header::ACCEPT, "application/vnd.github+json")
        .header("X-GitHub-Api-Version", "2022-11-28")
        .send()
        .map_err(|e| format!("检查更新失败: {}", e))?;
    if response.status() == reqwest::StatusCode::NOT_FOUND {
        record_update_check(app)?;
        set_update_status(app, "current", None, "GitHub 暂无可用 Release");
        return Ok(());
    }
    if response.status() == reqwest::StatusCode::FORBIDDEN {
        return Err("GitHub 更新检查暂时受限，请稍后重试".to_string());
    }
    let response = response
        .error_for_status()
        .map_err(|e| format!("检查更新失败: {}", e))?;
    let mut raw = String::new();
    response
        .take(2 * 1024 * 1024)
        .read_to_string(&mut raw)
        .map_err(|e| format!("读取更新信息失败: {}", e))?;
    let release: GitHubRelease =
        serde_json::from_str(&raw).map_err(|e| format!("更新信息格式错误: {}", e))?;
    if release.draft || release.prerelease {
        return Err("GitHub 最新版本不是正式 Release".to_string());
    }
    let (available, version) = parse_release_version(&release.tag_name)
        .ok_or_else(|| "GitHub Release 版本号格式不正确".to_string())?;
    let (current, _) = parse_release_version(env!("CARGO_PKG_VERSION"))
        .ok_or_else(|| "当前版本号格式不正确".to_string())?;
    record_update_check(app)?;
    if available <= current {
        set_update_status(app, "current", None, "当前已是最新版本");
        return Ok(());
    }
    let nea_name = format!("NEA_{}_x64_en-US.msi", version);
    let legacy_name = format!("OOPZ+_{}_x64_en-US.msi", version);
    let asset = release
        .assets
        .iter()
        .find(|asset| asset.name.eq_ignore_ascii_case(&nea_name))
        .or_else(|| {
            release
                .assets
                .iter()
                .find(|asset| asset.name.eq_ignore_ascii_case(&legacy_name))
        })
        .ok_or_else(|| format!("Release 缺少安装包 {} 或 {}", nea_name, legacy_name))?;
    set_update_status(
        app,
        "downloading",
        Some(version.clone()),
        format!("发现新版本 {}，正在通过加速线路下载...", version),
    );
    let msi_path = download_update_asset(app, asset, &version)?;
    set_update_status(
        app,
        "installing",
        Some(version.clone()),
        format!("正在安装 {}，程序即将重启...", version),
    );
    launch_update_installer(app, &msi_path, &version)
}

fn schedule_update_check(app: AppHandle, force: bool, delay: bool) {
    let state = app.state::<AppState>();
    if state.update_running.swap(true, Ordering::SeqCst) {
        return;
    }
    thread::spawn(move || {
        if delay {
            thread::sleep(Duration::from_secs(3));
        }
        if let Err(error) = perform_update_check(&app, force) {
            set_update_status(&app, "error", None, error);
        }
        app.state::<AppState>()
            .update_running
            .store(false, Ordering::SeqCst);
    });
}

fn start_auto_update_checks(app: AppHandle) {
    schedule_update_check(app.clone(), true, true);
    thread::spawn(move || loop {
        thread::sleep(Duration::from_secs(
            UPDATE_CHECK_INTERVAL_MINUTES as u64 * 60,
        ));
        schedule_update_check(app.clone(), false, false);
    });
}

#[tauri::command]
fn get_update_status(state: State<AppState>) -> Result<UpdateStatus, String> {
    state
        .update_status
        .lock()
        .map(|status| status.clone())
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn check_for_updates(app: AppHandle) -> UpdateStatus {
    schedule_update_check(app.clone(), true, false);
    app.state::<AppState>()
        .update_status
        .lock()
        .map(|status| status.clone())
        .unwrap_or_else(|_| initial_update_status())
}

fn process_update_result(app: &AppHandle) {
    if let Ok(error_path) = update_error_path() {
        if let Ok(error) = fs::read_to_string(&error_path) {
            let _ = fs::remove_file(error_path);
            set_update_status(app, "error", None, error);
            return;
        }
    }
    let Ok(marker_path) = update_marker_path() else {
        return;
    };
    let Ok(version) = fs::read_to_string(&marker_path) else {
        return;
    };
    let Some(marker_version) = parse_release_version(version.trim()).map(|value| value.0) else {
        let _ = fs::remove_file(marker_path);
        return;
    };
    let Some(current_version) =
        parse_release_version(env!("CARGO_PKG_VERSION")).map(|value| value.0)
    else {
        return;
    };
    if marker_version > current_version {
        return;
    }
    let _ = fs::remove_file(marker_path);
    let plugin_enabled = app
        .state::<AppState>()
        .data
        .lock()
        .map(|data| data.config.plugin_mode_enabled)
        .unwrap_or(false);
    schedule_plugin_environment(app.clone(), plugin_enabled, true);
    set_update_status(
        app,
        "updated",
        None,
        format!("已更新到 {}，插件环境正在修复", env!("CARGO_PKG_VERSION")),
    );
}

fn schedule_updater_cleanup(path: PathBuf) {
    thread::spawn(move || {
        for _ in 0..10 {
            thread::sleep(Duration::from_secs(1));
            if !path.exists() || fs::remove_file(&path).is_ok() {
                break;
            }
        }
    });
}

fn ensure_storage() -> Result<(), String> {
    fs::create_dir_all(accounts_dir()?).map_err(|e| e.to_string())?;
    fs::create_dir_all(backups_dir()?).map_err(|e| e.to_string())?;
    Ok(())
}

fn load_data() -> AppData {
    if ensure_storage().is_err() {
        return AppData::default();
    }
    let Ok(path) = config_path() else {
        return AppData::default();
    };
    let Ok(raw) = fs::read_to_string(&path) else {
        return AppData::default();
    };
    let mut data: AppData = serde_json::from_str(&raw).unwrap_or_default();
    if data.schema_version == 0 {
        data.schema_version = 2;
    }
    migrate_current_login_state(&mut data);
    migrate_avatar_sources(&mut data);
    reconcile_account_readiness(&mut data);
    if let Ok(next_raw) = serde_json::to_string_pretty(&data) {
        if next_raw != raw {
            let _ = fs::write(&path, next_raw);
        }
    }
    data
}

fn migrate_avatar_sources(data: &mut AppData) {
    for account in &mut data.accounts {
        if account.avatar_source_url.is_none()
            && account
                .avatar_url
                .as_deref()
                .is_some_and(|url| url.starts_with("http://") || url.starts_with("https://"))
        {
            account.avatar_source_url = account.avatar_url.clone();
        }
    }
}

fn save_data(data: &AppData) -> Result<(), String> {
    ensure_storage()?;
    let raw = serde_json::to_string_pretty(data).map_err(|e| e.to_string())?;
    let path = config_path()?;
    let temp = path.with_extension("json.tmp");
    let backup = path.with_extension("json.bak");
    fs::write(&temp, raw).map_err(|e| format!("写入配置失败: {}", e))?;

    if backup.exists() {
        let _ = fs::remove_file(&backup);
    }
    if path.exists() {
        fs::rename(&path, &backup).map_err(|e| format!("备份原配置失败: {}", e))?;
    }
    if let Err(error) = fs::rename(&temp, &path) {
        if backup.exists() {
            let _ = fs::rename(&backup, &path);
        }
        let _ = fs::remove_file(&temp);
        return Err(format!("保存配置失败: {}", error));
    }
    if backup.exists() {
        let _ = fs::remove_file(backup);
    }
    Ok(())
}

fn migrate_current_login_state(data: &mut AppData) {
    let Some(login) = current_registry_login() else {
        return;
    };
    let Some(uid) = uid_from_registry_login(&login) else {
        return;
    };
    for account in &mut data.accounts {
        if account.uid.as_deref() == Some(uid.as_str())
            && store_oopz_login(&account.id, &login).is_ok()
        {
            account.has_login_state = true;
            account.has_session_snapshot = true;
        }
    }
}

fn saved_snapshot_exists(account: &SavedAccount) -> bool {
    let Some(uid) = account.uid.as_deref() else {
        return false;
    };
    account_snapshot_dir(&account.id)
        .map(|snapshot| {
            snapshot.join("roaming").join(uid).exists()
                || snapshot.join("local_sandbox").join(uid).exists()
        })
        .unwrap_or(false)
}

fn reconcile_account_readiness(data: &mut AppData) {
    for account in &mut data.accounts {
        let has_snapshot = saved_snapshot_exists(account);
        account.has_session_snapshot = has_snapshot;
        account.has_login_state = has_snapshot && read_oopz_login(&account.id).is_some();
    }
}

fn current_registry_login() -> Option<String> {
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let key = hkcu.open_subkey("Software\\Oopz\\OopzData").ok()?;
    key.get_value::<String, _>("login")
        .ok()
        .filter(|s| !s.is_empty())
}

fn write_registry_login(login: &str) -> Result<(), String> {
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let (key, _) = hkcu
        .create_subkey("Software\\Oopz\\OopzData")
        .map_err(|e| format!("打开 OOPZ 注册表失败: {}", e))?;
    key.set_value("login", &login)
        .map_err(|e| format!("保存当前 OOPZ 登录状态失败: {}", e))
}

fn clear_registry_login() -> Result<(), String> {
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let (key, _) = hkcu
        .create_subkey("Software\\Oopz\\OopzData")
        .map_err(|e| format!("打开 OOPZ 注册表失败: {}", e))?;
    match key.delete_value("login") {
        Ok(_) => Ok(()),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!("清理 OOPZ 自动登录失败: {}", error)),
    }
}

fn uid_from_registry_login(login: &str) -> Option<String> {
    let decoded = general_purpose::STANDARD.decode(login).ok()?;
    let value: serde_json::Value = serde_json::from_slice(&decoded).ok()?;
    value.get("uid")?.as_str().map(str::to_string)
}

fn verify_registry_login_uid(expected_uid: &str) -> Result<(), String> {
    let written =
        current_registry_login().ok_or_else(|| "保存后未读到 OOPZ 登录状态".to_string())?;
    let written_uid = uid_from_registry_login(&written)
        .ok_or_else(|| "保存后的 OOPZ 登录状态无法识别账号".to_string())?;
    if written_uid != expected_uid {
        return Err(format!(
            "账号保存校验失败：目标 UID {}，当前 UID {}",
            expected_uid, written_uid
        ));
    }
    Ok(())
}

fn is_oopz_process_name(name: &str) -> bool {
    name.eq_ignore_ascii_case("oopz.exe") || name.eq_ignore_ascii_case("oopz")
}

fn running_oopz_exe_in(system: &System) -> Option<PathBuf> {
    system
        .processes()
        .values()
        .find(|process| is_oopz_process_name(process.name()))
        .and_then(|process| {
            process
                .exe()
                .filter(|path| path.is_file())
                .map(Path::to_path_buf)
        })
}

fn running_oopz_exe() -> Option<PathBuf> {
    running_oopz_exe_in(&process_system())
}

fn is_plugin_runtime_running_in(system: &System) -> bool {
    system.processes().values().any(|process| {
        (process.name().eq_ignore_ascii_case("oopz-plus.exe")
            || process.name().eq_ignore_ascii_case(WATCHER_FILE_NAME))
            && process.cmd().iter().any(|arg| arg == "--plugin-runtime")
    })
}

fn is_plugin_runtime_running() -> bool {
    is_plugin_runtime_running_in(&process_system())
}

fn is_watcher_running_in(system: &System) -> bool {
    system.processes().values().any(|process| {
        (process.name().eq_ignore_ascii_case("oopz-plus.exe")
            || process.name().eq_ignore_ascii_case(WATCHER_FILE_NAME))
            && process.cmd().iter().any(|arg| arg == "--watcher")
    })
}

fn is_watcher_running() -> bool {
    is_watcher_running_in(&process_system())
}

fn stop_watcher() {
    let system = process_system();
    for process in system.processes().values() {
        if (process.name().eq_ignore_ascii_case("oopz-plus.exe")
            || process.name().eq_ignore_ascii_case(WATCHER_FILE_NAME))
            && process.cmd().iter().any(|arg| arg == "--watcher")
        {
            let _ = process
                .kill_with(Signal::Term)
                .unwrap_or_else(|| process.kill());
        }
    }
    thread::sleep(Duration::from_millis(500));
}

fn stop_plugin_runtime() {
    let system = process_system();
    for process in system.processes().values() {
        if (process.name().eq_ignore_ascii_case("oopz-plus.exe")
            || process.name().eq_ignore_ascii_case(WATCHER_FILE_NAME))
            && process.cmd().iter().any(|arg| arg == "--plugin-runtime")
        {
            let _ = process
                .kill_with(Signal::Term)
                .unwrap_or_else(|| process.kill());
        }
    }
}

fn watcher_path() -> Result<PathBuf, String> {
    Ok(storage_dir()?.join(WATCHER_FILE_NAME))
}

fn remove_file_with_retries(path: &Path) -> Result<(), String> {
    let mut last_error = None;
    for _ in 0..8 {
        match fs::remove_file(path) {
            Ok(_) => return Ok(()),
            Err(error) if error.kind() == ErrorKind::NotFound => return Ok(()),
            Err(error) => {
                last_error = Some(error);
                thread::sleep(Duration::from_millis(250));
            }
        }
    }
    Err(format!(
        "删除文件失败: {}: {}",
        path.display(),
        last_error
            .map(|error| error.to_string())
            .unwrap_or_else(|| "未知错误".to_string())
    ))
}

fn install_watcher() -> Result<(), String> {
    stop_watcher();
    let current_exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let watcher = watcher_path()?;
    let mut copy_error = None;
    for _ in 0..8 {
        match fs::copy(&current_exe, &watcher) {
            Ok(_) => {
                copy_error = None;
                break;
            }
            Err(error) => {
                copy_error = Some(error);
                thread::sleep(Duration::from_millis(250));
            }
        }
    }
    if let Some(error) = copy_error {
        return Err(format!("安装守护进程失败: {}", error));
    }
    let command = format!("\"{}\" --watcher", watcher.display());
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let (key, _) = hkcu
        .create_subkey(RUN_KEY_PATH)
        .map_err(|e| e.to_string())?;
    key.set_value(RUN_KEY_NAME, &command)
        .map_err(|e| format!("注册守护自启动失败: {}", e))
}

fn uninstall_watcher() -> Result<(), String> {
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    if let Ok(key) = hkcu.open_subkey_with_flags(RUN_KEY_PATH, winreg::enums::KEY_SET_VALUE) {
        match key.delete_value(RUN_KEY_NAME) {
            Ok(_) => {}
            Err(error) if error.kind() == ErrorKind::NotFound => {}
            Err(error) => return Err(format!("取消守护自启动失败: {}", error)),
        }
    }
    Ok(())
}

fn watcher_installed() -> bool {
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    hkcu.open_subkey(RUN_KEY_PATH)
        .and_then(|key| key.get_value::<String, _>(RUN_KEY_NAME))
        .is_ok()
}

fn spawn_plugin_runtime() -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    Command::new(exe)
        .arg("--plugin-runtime")
        .spawn()
        .map_err(|e| format!("启动插件运行态失败: {}", e))?;
    Ok(())
}

fn ensure_plugin_runtime_for_oopz(config: &AppConfig) {
    if config.plugin_mode_enabled && running_oopz_exe().is_some() && !is_plugin_runtime_running() {
        let _ = spawn_plugin_runtime();
    }
}

fn ensure_plugin_runtime_after_oopz_start(config: AppConfig) {
    ensure_plugin_runtime_for_oopz(&config);
    if !config.plugin_mode_enabled {
        return;
    }
    thread::spawn(move || {
        thread::sleep(Duration::from_millis(1500));
        ensure_plugin_runtime_for_oopz(&config);
        thread::sleep(Duration::from_millis(2500));
        ensure_plugin_runtime_for_oopz(&config);
    });
}

fn watcher_loop() {
    let mut system = process_system();
    let mut config_modified = None;
    let mut plugin_enabled = false;
    loop {
        let modified = config_path()
            .ok()
            .and_then(|path| fs::metadata(path).ok())
            .and_then(|metadata| metadata.modified().ok());
        if modified != config_modified {
            config_modified = modified;
            plugin_enabled = load_data().config.plugin_mode_enabled;
        }
        if !plugin_enabled {
            thread::sleep(Duration::from_secs(3));
            continue;
        }
        refresh_process_system(&mut system);
        if running_oopz_exe_in(&system).is_some() && !is_plugin_runtime_running_in(&system) {
            let _ = spawn_plugin_runtime();
        }
        thread::sleep(Duration::from_secs(3));
    }
}

struct WindowSearch {
    pids: Vec<u32>,
    hwnd: Option<HWND>,
    rect: Option<RECT>,
}

unsafe extern "system" fn enum_windows_proc(hwnd: HWND, lparam: LPARAM) -> BOOL {
    let search = &mut *(lparam.0 as *mut WindowSearch);
    if !IsWindowVisible(hwnd).as_bool() {
        return BOOL(1);
    }
    if GetAncestor(hwnd, GA_ROOTOWNER) != hwnd {
        return BOOL(1);
    }
    let mut pid = 0u32;
    GetWindowThreadProcessId(hwnd, Some(&mut pid));
    if search.pids.contains(&pid) {
        let mut rect = RECT::default();
        if GetWindowRect(hwnd, &mut rect).is_ok() {
            search.rect = Some(rect);
            search.hwnd = Some(hwnd);
            return BOOL(0);
        }
    }
    BOOL(1)
}

fn oopz_process_ids_in(system: &System) -> Vec<u32> {
    system
        .processes()
        .iter()
        .filter_map(|(pid, process)| is_oopz_process_name(process.name()).then_some(pid.as_u32()))
        .collect()
}

fn oopz_process_ids() -> Vec<u32> {
    oopz_process_ids_in(&process_system())
}

fn oopz_window_info_for_pids(pids: Vec<u32>) -> Option<(HWND, RECT)> {
    if pids.is_empty() {
        return None;
    }
    let mut search = WindowSearch {
        pids,
        hwnd: None,
        rect: None,
    };
    unsafe {
        let _ = EnumWindows(
            Some(enum_windows_proc),
            LPARAM(&mut search as *mut _ as isize),
        );
    }
    search.hwnd.zip(search.rect)
}

fn oopz_window_info() -> Option<(HWND, RECT)> {
    oopz_window_info_for_pids(oopz_process_ids())
}

fn overlay_dimensions(account_count: usize, vertical: bool) -> (u32, u32) {
    if account_count == 0 {
        return if vertical { (54, 52) } else { (52, 52) };
    }
    let count = account_count as u32;
    if vertical {
        (
            54,
            (18 + count * 32 + count.saturating_sub(1) * 6 + 8 + 4).max(52),
        )
    } else {
        (
            (8 + count * 32 + count.saturating_sub(1) * 6 + 18 + 4).max(52),
            52,
        )
    }
}

fn scaled_pixels(value: i32, scale_factor: f64) -> i32 {
    (f64::from(value) * scale_factor).round() as i32
}

fn window_scale_factor(hwnd: HWND) -> f64 {
    let dpi = unsafe { GetDpiForWindow(hwnd) };
    if dpi == 0 {
        1.0
    } else {
        f64::from(dpi) / 96.0
    }
}

fn overlay_geometry(
    rect: RECT,
    config: &AppConfig,
    account_count: usize,
    scale_factor: f64,
) -> (i32, i32, u32, u32) {
    let logical_width = f64::from(rect.right - rect.left) / scale_factor;
    let (overlay_width, overlay_height) =
        overlay_dimensions(account_count, config.overlay_vertical);
    if logical_width < 1000.0 {
        (
            rect.left + scaled_pixels(70 + config.overlay_offset_x, scale_factor),
            rect.top + scaled_pixels(275 + config.overlay_offset_y, scale_factor),
            overlay_width,
            overlay_height,
        )
    } else {
        (
            rect.left + scaled_pixels(720 + config.overlay_offset_x, scale_factor),
            rect.top + scaled_pixels(15 + config.overlay_offset_y, scale_factor),
            overlay_width,
            overlay_height,
        )
    }
}

fn overlay_offset_for_position(
    rect: RECT,
    config: &AppConfig,
    account_count: usize,
    position: PhysicalPosition<i32>,
    scale_factor: f64,
) -> (i32, i32) {
    let (base_x, base_y, _, _) = overlay_geometry(
        rect,
        &AppConfig {
            overlay_offset_x: 0,
            overlay_offset_y: 0,
            ..config.clone()
        },
        account_count,
        scale_factor,
    );
    (
        ((f64::from(position.x - base_x) / scale_factor).round() as i32).clamp(-4000, 4000),
        ((f64::from(position.y - base_y) / scale_factor).round() as i32).clamp(-4000, 4000),
    )
}

#[tauri::command]
fn drag_overlay(app: AppHandle) -> Result<(), String> {
    let state = app.state::<AppState>();
    if state.overlay_dragging.swap(true, Ordering::SeqCst) {
        return Ok(());
    }
    let Some(window) = app.get_webview_window("plugin-overlay") else {
        state.overlay_dragging.store(false, Ordering::SeqCst);
        return Err("未找到插件浮层".to_string());
    };
    if let Err(error) = window.start_dragging() {
        state.overlay_dragging.store(false, Ordering::SeqCst);
        return Err(error.to_string());
    }

    thread::spawn(move || {
        let started = Instant::now();
        while started.elapsed() < Duration::from_secs(30) {
            let left_button_down = unsafe { GetAsyncKeyState(VK_LBUTTON.0 as i32) < 0 };
            if started.elapsed() >= Duration::from_millis(100) && !left_button_down {
                break;
            }
            thread::sleep(Duration::from_millis(25));
        }
        let result = persist_overlay_position(&app);
        app.state::<AppState>()
            .overlay_dragging
            .store(false, Ordering::SeqCst);
        if let Err(error) = result {
            let _ = app.emit("overlay-drag-error", error);
        }
    });
    Ok(())
}

fn persist_overlay_position(app: &AppHandle) -> Result<(), String> {
    let (oopz_hwnd, rect) = oopz_window_info().ok_or_else(|| "未找到 OOPZ 窗口".to_string())?;
    let window = app
        .get_webview_window("plugin-overlay")
        .ok_or_else(|| "未找到插件浮层".to_string())?;
    let position = window.outer_position().map_err(|e| e.to_string())?;
    let state = app.state::<AppState>();
    let mut data = state.data.lock().map_err(|e| e.to_string())?;
    let (offset_x, offset_y) = overlay_offset_for_position(
        rect,
        &data.config,
        data.accounts.len(),
        position,
        window_scale_factor(oopz_hwnd),
    );
    data.config.overlay_offset_x = offset_x;
    data.config.overlay_offset_y = offset_y;
    save_data(&data)
}

fn reset_overlay_position_inner(app: AppHandle, state: State<AppState>) -> Result<(), String> {
    let mut data = state.data.lock().map_err(|e| e.to_string())?;
    data.config.overlay_offset_x = 0;
    data.config.overlay_offset_y = 0;
    save_data(&data)?;
    drop(data);
    state.overlay_dragging.store(false, Ordering::SeqCst);
    state.overlay_rebind_requested.store(true, Ordering::SeqCst);
    let _ = app.emit("app-data-changed", ());
    Ok(())
}

#[tauri::command]
async fn reset_overlay_position(app: AppHandle) -> Result<(), String> {
    let app_for_task = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let state = app_for_task.state::<AppState>();
        reset_overlay_position_inner(app_for_task.clone(), state)
    })
    .await
    .map_err(|e| e.to_string())?
}

fn set_overlay_layout_inner(
    app: AppHandle,
    state: State<AppState>,
    vertical: bool,
) -> Result<(), String> {
    let mut data = state.data.lock().map_err(|e| e.to_string())?;
    data.config.overlay_vertical = vertical;
    save_data(&data)?;
    drop(data);
    state.overlay_rebind_requested.store(true, Ordering::SeqCst);
    let _ = app.emit("app-data-changed", ());
    Ok(())
}

#[tauri::command]
async fn set_overlay_layout(app: AppHandle, vertical: bool) -> Result<(), String> {
    let app_for_task = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let state = app_for_task.state::<AppState>();
        set_overlay_layout_inner(app_for_task.clone(), state, vertical)
    })
    .await
    .map_err(|e| e.to_string())?
}

fn visible_window_rect(hwnd: HWND) -> Option<RECT> {
    unsafe {
        if !IsWindow(hwnd).as_bool() || !IsWindowVisible(hwnd).as_bool() || IsIconic(hwnd).as_bool()
        {
            return None;
        }
        let mut rect = RECT::default();
        GetWindowRect(hwnd, &mut rect).ok().map(|_| rect)
    }
}

fn hide_overlay_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("plugin-overlay") {
        let _ = window.hide();
        let _ = window.set_always_on_top(false);
        detach_overlay_window(&window);
    }
}

fn detach_overlay_window(window: &WebviewWindow) {
    if let Ok(handle) = window.hwnd() {
        unsafe {
            let _ = SetWindowLongPtrW(HWND(handle.0 as isize), GWLP_HWNDPARENT, 0);
        }
    }
}

fn detach_plugin_overlay(app: &AppHandle) {
    let state = app.state::<AppState>();
    state.overlay_rebind_requested.store(true, Ordering::SeqCst);
    if let Some(window) = app.get_webview_window("plugin-overlay") {
        let _ = window.hide();
        detach_overlay_window(&window);
    }
}

fn sync_overlay_loop(app: AppHandle) {
    thread::spawn(move || {
        let mut last_config_refresh = Instant::now() - Duration::from_secs(2);
        let mut last_window_search = Instant::now() - Duration::from_secs(2);
        let mut owner: Option<HWND> = None;
        let mut last_geometry: Option<(i32, i32, u32, u32)> = None;
        let mut overlay_visible = false;
        let mut overlay_window = app.get_webview_window("plugin-overlay");

        loop {
            if last_config_refresh.elapsed() >= Duration::from_millis(500) {
                last_config_refresh = Instant::now();
                let plugin_enabled = app
                    .state::<AppState>()
                    .data
                    .lock()
                    .map(|data| data.config.plugin_mode_enabled)
                    .unwrap_or(false);
                if !plugin_enabled {
                    hide_overlay_window(&app);
                    app.exit(0);
                    break;
                }
            }

            if overlay_window.is_none() {
                overlay_window = app.get_webview_window("plugin-overlay");
            }
            let Some(window) = overlay_window.as_ref() else {
                thread::sleep(Duration::from_millis(500));
                continue;
            };

            let rebind_requested = app
                .state::<AppState>()
                .overlay_rebind_requested
                .swap(false, Ordering::SeqCst);
            if rebind_requested {
                owner = None;
                last_geometry = None;
                overlay_visible = false;
                last_window_search = Instant::now() - Duration::from_secs(2);
                detach_overlay_window(window);
            }

            let mut current =
                owner.and_then(|hwnd| visible_window_rect(hwnd).map(|rect| (hwnd, rect)));

            if current.is_none() && last_window_search.elapsed() >= Duration::from_secs(1) {
                last_window_search = Instant::now();
                current = oopz_window_info();
            }

            if let Some((next_owner, rect)) = current {
                owner = Some(next_owner);
                let foreground = unsafe { GetForegroundWindow() };
                let oopz_is_foreground = foreground == next_owner
                    || unsafe { GetAncestor(foreground, GA_ROOTOWNER) } == next_owner;
                let overlay_is_foreground = window
                    .hwnd()
                    .is_ok_and(|handle| foreground.0 == handle.0 as isize);
                if !oopz_is_foreground && !overlay_is_foreground {
                    if overlay_visible {
                        let _ = window.hide();
                        let _ = window.set_always_on_top(false);
                        overlay_visible = false;
                    }
                    thread::sleep(Duration::from_millis(100));
                    continue;
                }

                if app
                    .state::<AppState>()
                    .overlay_dragging
                    .load(Ordering::SeqCst)
                {
                    last_geometry = None;
                    thread::sleep(Duration::from_millis(33));
                    continue;
                }
                let (overlay_vertical, overlay_offset_x, overlay_offset_y, account_count) = app
                    .state::<AppState>()
                    .data
                    .lock()
                    .map(|data| {
                        (
                            data.config.overlay_vertical,
                            data.config.overlay_offset_x,
                            data.config.overlay_offset_y,
                            data.accounts.len(),
                        )
                    })
                    .unwrap_or_default();
                let config = AppConfig {
                    overlay_vertical,
                    overlay_offset_x,
                    overlay_offset_y,
                    ..AppConfig::default()
                };
                let geometry = overlay_geometry(
                    rect,
                    &config,
                    account_count,
                    window_scale_factor(next_owner),
                );
                let (x, y, w, h) = geometry;
                if last_geometry.map(|value| (value.0, value.1)) != Some((x, y)) {
                    let _ = window.set_position(PhysicalPosition::new(x, y));
                }
                if last_geometry.map(|value| (value.2, value.3)) != Some((w, h)) {
                    let _ = window.set_size(LogicalSize::new(w as f64, h as f64));
                }
                if !overlay_visible {
                    let _ = window.set_always_on_top(true);
                    let _ = window.show();
                    overlay_visible = true;
                }
                last_geometry = Some(geometry);
                thread::sleep(Duration::from_millis(33));
            } else {
                last_geometry = None;
                if overlay_visible {
                    let _ = window.hide();
                    let _ = window.set_always_on_top(false);
                    overlay_visible = false;
                }
                thread::sleep(Duration::from_millis(100));
            }
        }
    });
}

fn build_paths_from_exe(exe: &Path, source: &str) -> Result<OopzPaths, String> {
    if !exe.is_file() {
        return Err(format!("未找到 oopz.exe: {}", exe.display()));
    }
    let install_dir = exe
        .parent()
        .ok_or_else(|| "无法识别 OOPZ 安装目录".to_string())?
        .to_path_buf();
    let appdata = home_env("APPDATA")?;
    let localappdata = home_env("LOCALAPPDATA")?;
    let roaming_data_dir = appdata.join("oopz");
    let local_sandbox_dir = localappdata.join("oopz").join("sandbox");
    Ok(OopzPaths {
        oopz_install_dir: install_dir.to_string_lossy().to_string(),
        oopz_exe_path: exe.to_string_lossy().to_string(),
        roaming_data_dir: roaming_data_dir.to_string_lossy().to_string(),
        local_sandbox_dir: local_sandbox_dir.to_string_lossy().to_string(),
        source: source.to_string(),
        valid: true,
        message: None,
    })
}

fn discovery_cancelled(cancelled: &AtomicBool) -> Result<(), String> {
    if cancelled.load(Ordering::SeqCst) {
        Err("已停止搜索，可手动选择目录".to_string())
    } else {
        Ok(())
    }
}

fn emit_discovery_progress(app: &AppHandle, path: &Path) {
    let _ = app.emit(
        "oopz-discovery-progress",
        path.to_string_lossy().to_string(),
    );
}

fn discover_paths_with_progress(
    app: &AppHandle,
    cancelled: &AtomicBool,
) -> Result<OopzPaths, String> {
    discovery_cancelled(cancelled)?;
    if let Some(exe) = running_oopz_exe() {
        emit_discovery_progress(app, &exe);
        discovery_cancelled(cancelled)?;
        if let Ok(paths) = build_paths_from_exe(&exe, "running-process") {
            return Ok(paths);
        }
    }

    let localappdata = home_env("LOCALAPPDATA")?;
    let appdata = home_env("APPDATA")?;
    let candidates = [
        localappdata.join("oopz").join("oopz.exe"),
        appdata.join("oopz").join("oopz.exe"),
        appdata.join("oopz.cn").join("oopz").join("oopz.exe"),
    ];

    for exe in candidates {
        emit_discovery_progress(app, &exe);
        discovery_cancelled(cancelled)?;
        if exe.is_file() {
            return build_paths_from_exe(&exe, "auto-search");
        }
    }

    Err("未自动找到 OOPZ，请手动选择包含 oopz.exe 的目录".to_string())
}

fn discover_paths_inner() -> Result<OopzPaths, String> {
    if let Some(exe) = running_oopz_exe() {
        if let Ok(paths) = build_paths_from_exe(&exe, "running-process") {
            return Ok(paths);
        }
    }

    let localappdata = home_env("LOCALAPPDATA")?;
    let appdata = home_env("APPDATA")?;
    let candidates = [
        localappdata.join("oopz").join("oopz.exe"),
        appdata.join("oopz").join("oopz.exe"),
        appdata.join("oopz.cn").join("oopz").join("oopz.exe"),
    ];

    for exe in candidates {
        if exe.is_file() {
            return build_paths_from_exe(&exe, "auto-search");
        }
    }

    Err("未自动找到 OOPZ，请手动选择包含 oopz.exe 的目录".to_string())
}

fn paths_from_config(config: &AppConfig) -> Result<OopzPaths, String> {
    let Some(exe) = &config.oopz_exe_path else {
        return discover_paths_inner();
    };
    let mut paths = build_paths_from_exe(Path::new(exe), "configured")?;
    if let Some(v) = &config.oopz_install_dir {
        paths.oopz_install_dir = v.clone();
    }
    if let Some(v) = &config.roaming_data_dir {
        paths.roaming_data_dir = v.clone();
    }
    if let Some(v) = &config.local_sandbox_dir {
        paths.local_sandbox_dir = v.clone();
    }
    Ok(paths)
}

fn apply_paths_to_config(config: &mut AppConfig, paths: &OopzPaths) {
    config.oopz_install_dir = Some(paths.oopz_install_dir.clone());
    config.oopz_exe_path = Some(paths.oopz_exe_path.clone());
    config.roaming_data_dir = Some(paths.roaming_data_dir.clone());
    config.local_sandbox_dir = Some(paths.local_sandbox_dir.clone());
}

fn copy_dir_contents(src: &Path, dst: &Path) -> Result<(), String> {
    if !src.exists() {
        return Ok(());
    }
    fs::create_dir_all(dst).map_err(|e| e.to_string())?;
    for entry in fs::read_dir(src).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let file_type = entry.file_type().map_err(|e| e.to_string())?;
        let target = dst.join(entry.file_name());
        if file_type.is_dir() {
            copy_dir_contents(&entry.path(), &target)?;
        } else if file_type.is_file() {
            fs::copy(entry.path(), target).map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

fn commit_prepared_dir(prepared: &Path, dst: &Path) -> Result<(), String> {
    let parent = dst.parent().ok_or_else(|| "目标目录无效".to_string())?;
    fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    let old = parent.join(format!(".oopzplus-old-{}", Uuid::new_v4()));
    let had_dst = dst.exists();

    if had_dst {
        fs::rename(dst, &old).map_err(|e| format!("暂存原目录失败 {}: {}", dst.display(), e))?;
    }
    if let Err(error) = fs::rename(prepared, dst) {
        if had_dst {
            let _ = fs::rename(&old, dst);
        }
        return Err(format!("替换目录失败 {}: {}", dst.display(), error));
    }
    if had_dst {
        let _ = fs::remove_dir_all(old);
    }
    Ok(())
}

fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<(), String> {
    if !src.exists() {
        return Ok(());
    }
    let parent = dst.parent().ok_or_else(|| "目标目录无效".to_string())?;
    fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    let staging = parent.join(format!(".oopzplus-copy-{}", Uuid::new_v4()));
    if let Err(error) = copy_dir_contents(src, &staging) {
        let _ = fs::remove_dir_all(&staging);
        return Err(error);
    }
    if let Err(error) = commit_prepared_dir(&staging, dst) {
        let _ = fs::remove_dir_all(&staging);
        return Err(error);
    }
    Ok(())
}

fn copy_backup_children(src_root: &Path, dst_root: &Path) -> Result<(), String> {
    if !src_root.exists() {
        return Ok(());
    }
    fs::create_dir_all(dst_root).map_err(|e| e.to_string())?;
    for entry in fs::read_dir(src_root).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let file_type = entry.file_type().map_err(|e| e.to_string())?;
        let target = dst_root.join(entry.file_name());
        if file_type.is_dir() {
            copy_dir_recursive(&entry.path(), &target)?;
        } else if file_type.is_file() {
            fs::copy(entry.path(), target).map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

fn read_user_detail(path: &Path) -> Option<ImportedCandidate> {
    let detail_path = path
        .join("user_detail_cache_key")
        .join("user_detail_cache_key");
    let raw = fs::read_to_string(detail_path).ok()?;
    let outer: serde_json::Value = serde_json::from_str(&raw).ok()?;
    let inner_raw = outer.get("user_detail_cache_key")?.as_str()?;
    let inner: serde_json::Value = serde_json::from_str(inner_raw).ok()?;
    let data = inner.get("data")?;
    let uid = data.get("uid")?.as_str()?.to_string();
    let display_name = data
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or("未命名账号")
        .to_string();
    Some(ImportedCandidate {
        uid,
        display_name,
        pid: data.get("pid").and_then(|v| v.as_str()).map(str::to_string),
        user_common_id: data
            .get("userCommonId")
            .and_then(|v| v.as_str())
            .map(str::to_string),
        masked_phone: data
            .get("phone")
            .and_then(|v| v.as_str())
            .map(str::to_string),
        avatar_url: data
            .get("avatar")
            .and_then(|v| v.as_str())
            .map(str::to_string),
        has_roaming_state: false,
        has_local_state: false,
        last_write_time: fs::metadata(path)
            .ok()
            .and_then(|m| m.modified().ok())
            .map(|_| now()),
        has_current_login: false,
        can_switch: false,
    })
}

fn avatar_mime(bytes: &[u8], content_type: Option<&str>) -> Option<&'static str> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Some("image/png");
    }
    if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        return Some("image/jpeg");
    }
    if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        return Some("image/gif");
    }
    if bytes.len() >= 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WEBP" {
        return Some("image/webp");
    }
    match content_type
        .unwrap_or_default()
        .split(';')
        .next()
        .unwrap_or_default()
    {
        "image/png" => Some("image/png"),
        "image/jpeg" | "image/jpg" => Some("image/jpeg"),
        "image/gif" => Some("image/gif"),
        "image/webp" => Some("image/webp"),
        "image/avif" => Some("image/avif"),
        _ => None,
    }
}

fn download_avatar_data_url(url: &str) -> Option<String> {
    if !(url.starts_with("https://") || url.starts_with("http://")) {
        return None;
    }
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(6))
        .build()
        .ok()?;
    let response = client.get(url).send().ok()?.error_for_status().ok()?;
    if response
        .content_length()
        .is_some_and(|size| size > MAX_AVATAR_BYTES)
    {
        return None;
    }
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let mut bytes = Vec::new();
    response
        .take(MAX_AVATAR_BYTES + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    if bytes.is_empty() || bytes.len() as u64 > MAX_AVATAR_BYTES {
        return None;
    }
    let mime = avatar_mime(&bytes, content_type.as_deref())?;
    Some(format!(
        "data:{};base64,{}",
        mime,
        general_purpose::STANDARD.encode(bytes)
    ))
}

fn refresh_account_avatar(app: &AppHandle, uid: &str) -> Result<bool, String> {
    let state = app.state::<AppState>();
    let (roaming_dir, account_id, saved_source, has_cached_avatar) = {
        let data = state.data.lock().map_err(|e| e.to_string())?;
        let paths = paths_from_config(&data.config)?;
        let Some(account) = data
            .accounts
            .iter()
            .find(|account| account.uid.as_deref() == Some(uid))
        else {
            return Ok(false);
        };
        (
            paths.roaming_data_dir,
            account.id.clone(),
            account.avatar_source_url.clone(),
            account
                .avatar_url
                .as_deref()
                .is_some_and(|url| url.starts_with("data:image/")),
        )
    };
    let Some(source_url) = read_user_detail(&PathBuf::from(roaming_dir).join(uid))
        .and_then(|candidate| candidate.avatar_url)
        .filter(|url| !url.trim().is_empty())
    else {
        return Ok(false);
    };
    if has_cached_avatar && saved_source.as_deref() == Some(source_url.as_str()) {
        return Ok(false);
    }
    let Some(cached_avatar) = download_avatar_data_url(&source_url) else {
        return Ok(false);
    };

    let mut data = state.data.lock().map_err(|e| e.to_string())?;
    let Some(account) = data
        .accounts
        .iter_mut()
        .find(|account| account.id == account_id)
    else {
        return Ok(false);
    };
    account.avatar_url = Some(cached_avatar);
    account.avatar_source_url = Some(source_url);
    account.updated_at = now();
    save_data(&data)?;
    drop(data);
    let _ = app.emit("app-data-changed", ());
    Ok(true)
}

fn schedule_avatar_refresh(app: AppHandle, uid: String) {
    thread::spawn(move || {
        for delay in [Duration::from_secs(5), Duration::from_secs(10)] {
            thread::sleep(delay);
            if refresh_account_avatar(&app, &uid).unwrap_or(false) {
                break;
            }
        }
    });
}

fn credential_entry(account_id: &str) -> Result<Entry, String> {
    Entry::new(CREDENTIAL_SERVICE, account_id).map_err(|e| e.to_string())
}

fn legacy_credential_entry(account_id: &str) -> Result<Entry, String> {
    Entry::new(LEGACY_CREDENTIAL_SERVICE, account_id).map_err(|e| e.to_string())
}

fn read_secret_raw(account_id: &str) -> Option<String> {
    credential_entry(account_id)
        .ok()
        .and_then(|entry| entry.get_password().ok())
        .or_else(|| {
            legacy_credential_entry(account_id)
                .ok()
                .and_then(|entry| entry.get_password().ok())
        })
}

fn write_secret_raw(account_id: &str, raw: &str) -> Result<(), String> {
    credential_entry(account_id)?
        .set_password(raw)
        .map_err(|e| e.to_string())
}

fn read_secret(account_id: &str) -> SecretPayload {
    let Ok(payload) = credential_entry(account_id)
        .and_then(|entry| entry.get_password().map_err(|e| e.to_string()))
    else {
        return SecretPayload::default();
    };
    serde_json::from_str(&payload).unwrap_or_default()
}

fn write_secret(account_id: &str, payload: &SecretPayload) -> Result<(), String> {
    let raw = serde_json::to_string(payload).map_err(|e| e.to_string())?;
    credential_entry(account_id)?
        .set_password(&raw)
        .map_err(|e| e.to_string())
}

fn store_oopz_login(account_id: &str, login: &str) -> Result<(), String> {
    let mut payload = read_secret(account_id);
    payload.oopz_login = Some(login.to_string());
    write_secret(account_id, &payload)
}

fn read_oopz_login(account_id: &str) -> Option<String> {
    read_secret(account_id).oopz_login
}

fn delete_credential(account_id: &str) {
    if let Ok(entry) = credential_entry(account_id) {
        let _ = entry.delete_credential();
    }
    if let Ok(entry) = legacy_credential_entry(account_id) {
        let _ = entry.delete_credential();
    }
}

fn account_snapshot_dir(account_id: &str) -> Result<PathBuf, String> {
    Ok(accounts_dir()?.join(account_id).join("snapshot"))
}

fn safe_relative_path(value: &str) -> Result<PathBuf, String> {
    let path = PathBuf::from(value);
    if path.components().any(|component| {
        matches!(
            component,
            Component::ParentDir | Component::RootDir | Component::Prefix(_)
        )
    }) {
        return Err("导入文件包含无效路径".to_string());
    }
    Ok(path)
}

fn write_export_files(root: &Path, files: &[ExportedFile]) -> Result<(), String> {
    let parent = root.parent().ok_or_else(|| "账号目录无效".to_string())?;
    fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    let staging = parent.join(format!(".oopzplus-import-{}", Uuid::new_v4()));
    fs::create_dir_all(&staging).map_err(|e| e.to_string())?;
    let write_result = (|| -> Result<(), String> {
        for file in files {
            let relative = safe_relative_path(&file.path)?;
            let target = staging.join(relative);
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent).map_err(|e| e.to_string())?;
            }
            let bytes = general_purpose::STANDARD
                .decode(&file.data_base64)
                .map_err(|e| e.to_string())?;
            fs::write(target, bytes).map_err(|e| e.to_string())?;
        }
        Ok(())
    })();
    if let Err(error) = write_result {
        let _ = fs::remove_dir_all(&staging);
        return Err(error);
    }
    if let Err(error) = commit_prepared_dir(&staging, root) {
        let _ = fs::remove_dir_all(&staging);
        return Err(error);
    }
    Ok(())
}

fn update_tray(app: &AppHandle) {
    if let Ok(menu) = build_tray_menu(app) {
        if let Some(tray) = app.tray_by_id("main-tray") {
            let _ = tray.set_menu(Some(menu));
        }
    }
}

fn refresh_app_data_from_disk(app: &AppHandle) -> AppData {
    let data = load_data();
    let state = app.state::<AppState>();
    if let Ok(mut state_data) = state.data.lock() {
        *state_data = data.clone();
    }
    data
}

fn watch_config_changes(app: AppHandle) {
    thread::spawn(move || {
        let Ok(path) = config_path() else {
            return;
        };
        let Some(parent) = path.parent().map(Path::to_path_buf) else {
            return;
        };
        if fs::create_dir_all(&parent).is_err() {
            return;
        }
        let (tx, rx) = mpsc::channel();
        let watcher_result: notify::Result<RecommendedWatcher> =
            notify::recommended_watcher(move |event| {
                let _ = tx.send(event);
            });
        let Ok(mut watcher) = watcher_result else {
            return;
        };
        if watcher.watch(&parent, RecursiveMode::NonRecursive).is_err() {
            return;
        }
        let mut last_config_bytes = fs::read(&path).ok();
        let config_file_name = path.file_name().map(|name| name.to_os_string());
        while let Ok(event) = rx.recv() {
            let Ok(event) = event else {
                continue;
            };
            let relevant_path = event.paths.iter().any(|changed| {
                changed == &path
                    || changed
                        .file_name()
                        .is_some_and(|name| Some(name) == config_file_name.as_deref())
            });
            let relevant_kind = matches!(
                event.kind,
                EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_)
            );
            if relevant_path && relevant_kind {
                thread::sleep(Duration::from_millis(80));
                let changed_config_bytes = fs::read(&path).ok();
                if changed_config_bytes == last_config_bytes {
                    continue;
                }
                refresh_app_data_from_disk(&app);
                last_config_bytes = fs::read(&path).ok().or(changed_config_bytes);
                update_tray(&app);
                let _ = app.emit("app-data-changed", ());
                thread::sleep(Duration::from_millis(150));
            }
        }
    });
}

fn show_main_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        set_webview_low_memory(&window, false);
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}

fn set_webview_low_memory(window: &WebviewWindow, low_memory: bool) {
    let state = window.app_handle().state::<AppState>();
    if state
        .main_webview_low_memory
        .swap(low_memory, Ordering::SeqCst)
        == low_memory
    {
        return;
    }
    let target = if low_memory {
        COREWEBVIEW2_MEMORY_USAGE_TARGET_LEVEL_LOW
    } else {
        COREWEBVIEW2_MEMORY_USAGE_TARGET_LEVEL_NORMAL
    };
    let _ = window.with_webview(move |webview| unsafe {
        let Ok(core_webview) = webview.controller().CoreWebView2() else {
            return;
        };
        let Ok(core_webview_19) = core_webview.cast::<ICoreWebView2_19>() else {
            return;
        };
        let _ = core_webview_19.SetMemoryUsageTargetLevel(target);
    });
}

fn fit_main_window_to_monitor(window: &WebviewWindow, prefer_default_size: bool) {
    let Ok(Some(monitor)) = window.current_monitor() else {
        return;
    };
    let scale_factor = monitor.scale_factor();
    let work_area = monitor.work_area();
    let available_width = (f64::from(work_area.size.width) / scale_factor - 24.0).max(480.0);
    let available_height = (f64::from(work_area.size.height) / scale_factor - 24.0).max(360.0);
    let min_width = 560.0_f64.min(available_width);
    let min_height = 420.0_f64.min(available_height);
    let _ = window.set_min_size(Some(LogicalSize::new(min_width, min_height)));

    let current = window
        .inner_size()
        .map(|size| size.to_logical::<f64>(scale_factor))
        .unwrap_or_else(|_| LogicalSize::new(980.0, 680.0));
    let requested_width = if prefer_default_size {
        980.0
    } else {
        current.width
    };
    let requested_height = if prefer_default_size {
        680.0
    } else {
        current.height
    };
    let target_width = requested_width.min(available_width).max(min_width);
    let target_height = requested_height.min(available_height).max(min_height);
    if (target_width - current.width).abs() >= 1.0 || (target_height - current.height).abs() >= 1.0
    {
        let _ = window.set_size(LogicalSize::new(target_width, target_height));
    }
}

fn build_tray_menu(app: &AppHandle) -> tauri::Result<Menu<tauri::Wry>> {
    let state = app.state::<AppState>();
    let data = state.data.lock().expect("state poisoned").clone();
    let current_uid = current_registry_login().and_then(|login| uid_from_registry_login(&login));
    let menu = Menu::new(app)?;
    menu.append(&MenuItem::with_id(
        app,
        "show",
        "打开 NEA",
        true,
        None::<&str>,
    )?)?;
    menu.append(&PredefinedMenuItem::separator(app)?)?;
    if data.accounts.is_empty() {
        menu.append(&MenuItem::with_id(
            app,
            "empty",
            "暂无账号",
            false,
            None::<&str>,
        )?)?;
    } else {
        for account in &data.accounts {
            let is_current = account.uid.as_deref() == current_uid.as_deref();
            let label = if is_current {
                format!("{}（登录中）", account.display_name)
            } else if account.has_login_state {
                format!("切换到 {}", account.display_name)
            } else {
                format!("登录 {}", account.display_name)
            };
            menu.append(&MenuItem::with_id(
                app,
                format!("switch:{}", account.id),
                label,
                !is_current,
                None::<&str>,
            )?)?;
        }
    }
    menu.append(&PredefinedMenuItem::separator(app)?)?;
    menu.append(&MenuItem::with_id(
        app,
        "import",
        "刷新账号",
        true,
        None::<&str>,
    )?)?;
    menu.append(&MenuItem::with_id(
        app,
        "rediscover",
        "重新搜索 OOPZ",
        true,
        None::<&str>,
    )?)?;
    menu.append(&PredefinedMenuItem::separator(app)?)?;
    menu.append(&MenuItem::with_id(app, "quit", "退出", true, None::<&str>)?)?;
    Ok(menu)
}

fn get_app_data_inner(app: &AppHandle) -> Result<AppData, String> {
    let mut data = app
        .state::<AppState>()
        .data
        .lock()
        .map_err(|e| e.to_string())?
        .clone();
    schedule_auto_import_current_login(app.clone());
    data.current_login_uid =
        current_registry_login().and_then(|login| uid_from_registry_login(&login));
    Ok(data)
}

#[tauri::command]
async fn get_app_data(app: AppHandle) -> Result<AppData, String> {
    tauri::async_runtime::spawn_blocking(move || get_app_data_inner(&app))
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
async fn get_steam_workspace(state: State<'_, AppState>) -> Result<steam::SteamWorkspace, String> {
    state
        .data
        .lock()
        .map(|data| data.steam.clone())
        .map_err(|error| error.to_string())
}

#[tauri::command]
async fn discover_steam(app: AppHandle) -> Result<steam::SteamWorkspace, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let installation = steam::SteamAdapter::discover_installation()?;
        let accounts = steam::SteamAdapter::read_accounts(&installation)?;
        let state = app.state::<AppState>();
        let mut data = state.data.lock().map_err(|error| error.to_string())?;
        data.steam.installation = Some(installation);
        data.steam.accounts = accounts;
        data.steam.current_account_id = data
            .steam
            .accounts
            .iter()
            .find(|account| account.most_recent)
            .map(|account| account.id.clone());
        save_data(&data)?;
        Ok(data.steam.clone())
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
async fn refresh_steam_accounts(app: AppHandle) -> Result<steam::SteamWorkspace, String> {
    discover_steam(app).await
}

#[tauri::command]
async fn set_steam_userdata_scope(
    app: AppHandle,
    enabled: bool,
) -> Result<steam::SteamWorkspace, String> {
    let state = app.state::<AppState>();
    let mut data = state.data.lock().map_err(|error| error.to_string())?;
    data.steam.include_userdata = enabled;
    save_data(&data)?;
    Ok(data.steam.clone())
}

#[tauri::command]
async fn set_steam_account_note(
    app: AppHandle,
    account_id: String,
    note: String,
) -> Result<steam::SteamWorkspace, String> {
    let state = app.state::<AppState>();
    let mut data = state.data.lock().map_err(|error| error.to_string())?;
    let account = data
        .steam
        .accounts
        .iter_mut()
        .find(|account| account.id == account_id)
        .ok_or_else(|| "Steam 账号不存在".to_string())?;
    let trimmed = note.trim();
    account.note = (!trimmed.is_empty()).then(|| trimmed.chars().take(120).collect());
    save_data(&data)?;
    Ok(data.steam.clone())
}

#[tauri::command]
async fn delete_steam_account(app: AppHandle, account_id: String) -> Result<(), String> {
    let state = app.state::<AppState>();
    let mut data = state.data.lock().map_err(|error| error.to_string())?;
    data.steam
        .accounts
        .retain(|account| account.id != account_id);
    save_data(&data)?;
    let snapshot = storage_dir()?
        .join("workspaces")
        .join("steam")
        .join("accounts")
        .join(account_id);
    if snapshot.exists() {
        fs::remove_dir_all(snapshot)
            .map_err(|error| format!("删除 Steam 账号快照失败: {}", error))?;
    }
    Ok(())
}

#[tauri::command]
async fn switch_steam_account(app: AppHandle, account_id: String) -> Result<SwitchResult, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        let _operation = state
            .switch_operation
            .lock()
            .map_err(|error| error.to_string())?;
        let (installation, include_userdata) = {
            let data = state.data.lock().map_err(|error| error.to_string())?;
            (
                data.steam
                    .installation
                    .clone()
                    .ok_or_else(|| "请先搜索 Steam 安装目录".to_string())?,
                data.steam.include_userdata,
            )
        };
        let adapter = steam::SteamAdapter;
        let adapter_installation = adapters::AppInstallation {
            executable: PathBuf::from(&installation.executable),
            data_dir: PathBuf::from(&installation.install_dir),
        };
        let backup_dir = storage_dir()?
            .join("workspaces")
            .join("steam")
            .join("backups")
            .join(Uuid::new_v4().to_string());
        steam::SteamAdapter::snapshot_login_state(&installation, &backup_dir)?;
        adapter.stop(&adapter_installation)?;
        let switch_result = (|| {
            let account_name = steam::SteamAdapter::activate_account(&installation, &account_id)?;
            if include_userdata {
                let snapshot_dir = storage_dir()?
                    .join("workspaces")
                    .join("steam")
                    .join("accounts")
                    .join(&account_id)
                    .join("userdata");
                steam::SteamAdapter::capture_userdata(&installation, &account_id, &snapshot_dir)?;
            }
            Ok::<_, String>(account_name)
        })();
        if let Err(error) = &switch_result {
            let loginusers = PathBuf::from(&installation.install_dir)
                .join("config")
                .join("loginusers.vdf");
            let _ = fs::copy(backup_dir.join("loginusers.vdf"), loginusers);
            let _ = adapter.start(&adapter_installation);
            return Err(error.clone());
        }
        let account_name = switch_result?;
        steam::SteamAdapter::start_with_login(&adapter_installation, &account_name)?;
        let mut data = state.data.lock().map_err(|error| error.to_string())?;
        data.steam.current_account_id = Some(account_id.clone());
        for account in &mut data.steam.accounts {
            account.most_recent = account.id == account_id;
        }
        save_data(&data)?;
        Ok(SwitchResult {
            ok: true,
            message: format!("已切换到 Steam 账号 {}", account_name),
        })
    })
    .await
    .map_err(|error| error.to_string())?
}

fn schedule_auto_import_current_login(app: AppHandle) {
    let state = app.state::<AppState>();
    if state.auto_import_running.swap(true, Ordering::SeqCst) {
        return;
    }
    tauri::async_runtime::spawn_blocking(move || {
        let _ = auto_import_current_login(app.clone());
        let state = app.state::<AppState>();
        state.auto_import_running.store(false, Ordering::SeqCst);
    });
}

fn auto_import_current_login(app: AppHandle) -> Result<(), String> {
    let Some(login) = current_registry_login() else {
        return Ok(());
    };
    let Some(uid) = uid_from_registry_login(&login) else {
        return Ok(());
    };
    let app_for_state = app.clone();
    let state = app_for_state.state::<AppState>();
    let data = state.data.lock().map_err(|e| e.to_string())?.clone();
    let current_avatar_url = paths_from_config(&data.config)
        .ok()
        .and_then(|paths| read_user_detail(&PathBuf::from(paths.roaming_data_dir).join(&uid)))
        .and_then(|candidate| candidate.avatar_url)
        .filter(|url| !url.trim().is_empty());
    let should_import = match data
        .accounts
        .iter()
        .find(|account| account.uid.as_deref() == Some(uid.as_str()))
    {
        Some(account) => {
            !account.has_login_state
                || !saved_snapshot_exists(account)
                || read_oopz_login(&account.id).is_none()
                || account.avatar_url.as_deref().unwrap_or("").is_empty()
                || current_avatar_url
                    .as_deref()
                    .is_some_and(|url| account.avatar_source_url.as_deref() != Some(url))
        }
        None => true,
    };
    if should_import {
        let _ = import_account_inner(app, state, uid);
    }
    Ok(())
}

fn plugin_status_inner(state: &AppState) -> Result<PluginStatus, String> {
    let plugin_mode_enabled = state
        .data
        .lock()
        .map_err(|e| e.to_string())?
        .config
        .plugin_mode_enabled;
    let system = process_system();
    let watcher_running = is_watcher_running_in(&system);
    let plugin_runtime_running = is_plugin_runtime_running_in(&system);
    let oopz_pids = oopz_process_ids_in(&system);
    let oopz_running = !oopz_pids.is_empty();
    let overlay_visible = oopz_window_info_for_pids(oopz_pids).is_some() && plugin_runtime_running;
    Ok(PluginStatus {
        plugin_mode_enabled,
        watcher_installed: watcher_installed(),
        watcher_running,
        plugin_runtime_running,
        oopz_running,
        overlay_visible,
    })
}

fn plugin_status_quick(state: &AppState) -> Result<PluginStatus, String> {
    let data = state.data.lock().map_err(|e| e.to_string())?;
    Ok(PluginStatus {
        plugin_mode_enabled: data.config.plugin_mode_enabled,
        watcher_installed: watcher_installed(),
        watcher_running: false,
        plugin_runtime_running: false,
        oopz_running: false,
        overlay_visible: false,
    })
}

fn maintain_plugin_environment(enabled: bool, repair: bool) -> Result<(), String> {
    if repair {
        stop_plugin_runtime();
        stop_watcher();
        uninstall_watcher()?;
        if let Ok(watcher) = watcher_path() {
            if watcher.exists() {
                remove_file_with_retries(&watcher)
                    .map_err(|error| format!("清理旧守护进程失败: {}", error))?;
            }
        }
    }

    if enabled {
        install_watcher()?;
        let _ = spawn_watcher();
        if running_oopz_exe().is_some() && !is_plugin_runtime_running() {
            let _ = spawn_plugin_runtime();
        }
    } else {
        uninstall_watcher()?;
        stop_watcher();
        stop_plugin_runtime();
    }
    Ok(())
}

fn schedule_plugin_environment(app: AppHandle, enabled: bool, repair: bool) {
    let state = app.state::<AppState>();
    if state
        .plugin_environment_running
        .swap(true, Ordering::SeqCst)
    {
        return;
    }
    tauri::async_runtime::spawn_blocking(move || {
        let result = maintain_plugin_environment(enabled, repair);
        let state = app.state::<AppState>();
        state
            .plugin_environment_running
            .store(false, Ordering::SeqCst);
        let _ = app.emit(
            "plugin-environment-finished",
            result.err().unwrap_or_default(),
        );
    });
}

#[tauri::command]
async fn get_plugin_status(app: AppHandle) -> Result<PluginStatus, String> {
    let app_for_task = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let state = app_for_task.state::<AppState>();
        plugin_status_inner(&state)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
async fn set_plugin_mode(app: AppHandle, enabled: bool) -> Result<PluginStatus, String> {
    let app_for_task = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let state = app_for_task.state::<AppState>();
        set_plugin_mode_inner(app_for_task.clone(), state, enabled)
    })
    .await
    .map_err(|e| e.to_string())?
}

fn set_plugin_mode_inner(
    app: AppHandle,
    state: State<AppState>,
    enabled: bool,
) -> Result<PluginStatus, String> {
    let _operation = state
        .plugin_operation
        .try_lock()
        .map_err(|_| "插件正在处理，请稍后再试".to_string())?;
    let mut data = state.data.lock().map_err(|e| e.to_string())?;
    data.config.plugin_mode_enabled = enabled;
    data.config.plugin_autostart_enabled = enabled;
    save_data(&data)?;
    drop(data);
    schedule_plugin_environment(app, enabled, false);
    plugin_status_quick(&state)
}

#[tauri::command]
async fn repair_plugin_environment(app: AppHandle) -> Result<PluginStatus, String> {
    let app_for_task = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let state = app_for_task.state::<AppState>();
        repair_plugin_environment_inner(app_for_task.clone(), state)
    })
    .await
    .map_err(|e| e.to_string())?
}

fn repair_plugin_environment_inner(
    app: AppHandle,
    state: State<AppState>,
) -> Result<PluginStatus, String> {
    let _operation = state
        .plugin_operation
        .try_lock()
        .map_err(|_| "插件正在处理，请稍后再试".to_string())?;
    let plugin_enabled = {
        let data = state.data.lock().map_err(|e| e.to_string())?;
        data.config.plugin_mode_enabled
    };
    schedule_plugin_environment(app, plugin_enabled, true);
    plugin_status_quick(&state)
}

#[tauri::command]
async fn plugin_account_action(app: AppHandle, account_id: String) -> Result<SwitchResult, String> {
    let app_for_task = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let state = app_for_task.state::<AppState>();
        switch_account_inner(app_for_task.clone(), state, account_id)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
async fn discover_oopz(app: AppHandle) -> Result<OopzPaths, String> {
    let app_for_task = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let state = app_for_task.state::<AppState>();
        discover_oopz_inner(app_for_task.clone(), state)
    })
    .await
    .map_err(|e| e.to_string())?
}

fn discover_oopz_inner(app: AppHandle, state: State<AppState>) -> Result<OopzPaths, String> {
    state.discovery_cancelled.store(false, Ordering::SeqCst);
    let paths = discover_paths_with_progress(&app, &state.discovery_cancelled)?;
    let mut data = state.data.lock().map_err(|e| e.to_string())?;
    apply_paths_to_config(&mut data.config, &paths);
    save_data(&data)?;
    drop(data);
    update_tray(&app);
    Ok(paths)
}

#[tauri::command]
fn cancel_oopz_discovery(state: State<AppState>) {
    state.discovery_cancelled.store(true, Ordering::SeqCst);
}

#[tauri::command]
async fn set_oopz_directory(app: AppHandle, dir: String) -> Result<OopzPaths, String> {
    let app_for_task = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let state = app_for_task.state::<AppState>();
        set_oopz_directory_inner(app_for_task.clone(), state, dir)
    })
    .await
    .map_err(|e| e.to_string())?
}

fn set_oopz_directory_inner(
    app: AppHandle,
    state: State<AppState>,
    dir: String,
) -> Result<OopzPaths, String> {
    let exe = PathBuf::from(&dir).join("oopz.exe");
    let paths = build_paths_from_exe(&exe, "manual")?;
    let mut data = state.data.lock().map_err(|e| e.to_string())?;
    apply_paths_to_config(&mut data.config, &paths);
    save_data(&data)?;
    drop(data);
    update_tray(&app);
    Ok(paths)
}

#[tauri::command]
async fn validate_configured_paths(app: AppHandle) -> Result<OopzPaths, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        let data = state.data.lock().map_err(|e| e.to_string())?;
        paths_from_config(&data.config)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
async fn scan_oopz_accounts(app: AppHandle) -> Result<Vec<ImportedCandidate>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        scan_oopz_accounts_inner(state)
    })
    .await
    .map_err(|e| e.to_string())?
}

fn scan_oopz_accounts_inner(state: State<AppState>) -> Result<Vec<ImportedCandidate>, String> {
    let data = state.data.lock().map_err(|e| e.to_string())?;
    let paths = paths_from_config(&data.config)?;
    drop(data);

    let roaming = PathBuf::from(&paths.roaming_data_dir);
    let local = PathBuf::from(&paths.local_sandbox_dir);
    let mut candidates: Vec<ImportedCandidate> = Vec::new();
    let current_login_uid =
        current_registry_login().and_then(|login| uid_from_registry_login(&login));

    if roaming.is_dir() {
        for entry in fs::read_dir(&roaming).map_err(|e| e.to_string())? {
            let entry = entry.map_err(|e| e.to_string())?;
            if !entry.file_type().map_err(|e| e.to_string())?.is_dir() {
                continue;
            }
            if let Some(mut candidate) = read_user_detail(&entry.path()) {
                candidate.has_roaming_state = true;
                candidate.has_local_state = local.join(&candidate.uid).is_dir();
                candidate.has_current_login =
                    current_login_uid.as_deref() == Some(candidate.uid.as_str());
                candidate.can_switch = candidate.has_current_login;
                candidates.push(candidate);
            }
        }
    }

    candidates.sort_by(|a, b| a.display_name.cmp(&b.display_name));
    Ok(candidates)
}

#[tauri::command]
async fn import_account(app: AppHandle, uid: String) -> Result<SavedAccount, String> {
    let app_for_task = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let state = app_for_task.state::<AppState>();
        import_account_inner(app_for_task.clone(), state, uid)
    })
    .await
    .map_err(|e| e.to_string())?
}

fn import_account_inner(
    app: AppHandle,
    state: State<AppState>,
    uid: String,
) -> Result<SavedAccount, String> {
    let _operation = state.account_operation.lock().map_err(|e| e.to_string())?;
    let mut data = state.data.lock().map_err(|e| e.to_string())?.clone();
    let paths = paths_from_config(&data.config)?;
    let roaming_src = PathBuf::from(&paths.roaming_data_dir).join(&uid);
    let local_src = PathBuf::from(&paths.local_sandbox_dir).join(&uid);
    let candidate = read_user_detail(&roaming_src).ok_or_else(|| "无法读取账号详情".to_string())?;
    let registry_login = current_registry_login();
    let registry_login_uid = registry_login.as_deref().and_then(uid_from_registry_login);
    let has_login_state = registry_login_uid.as_deref() == Some(uid.as_str());

    let existing_id = data
        .accounts
        .iter()
        .find(|a| a.uid.as_deref() == Some(&uid))
        .map(|a| a.id.clone());
    let existing_account = existing_id
        .as_ref()
        .and_then(|existing_id| data.accounts.iter().find(|a| a.id == *existing_id))
        .cloned();
    let candidate_avatar_url = candidate
        .avatar_url
        .clone()
        .filter(|url| !url.trim().is_empty());
    let cached_avatar_url = candidate_avatar_url
        .as_deref()
        .and_then(download_avatar_data_url);
    let id = existing_id.unwrap_or_else(|| Uuid::new_v4().to_string());
    let snapshot = account_snapshot_dir(&id)?;
    copy_dir_recursive(&roaming_src, &snapshot.join("roaming").join(&uid))?;
    copy_dir_recursive(&local_src, &snapshot.join("local_sandbox").join(&uid))?;
    if has_login_state {
        if let Some(login) = registry_login {
            store_oopz_login(&id, &login)?;
        }
    }

    let timestamp = now();
    let account = SavedAccount {
        id: id.clone(),
        display_name: candidate.display_name,
        uid: Some(uid),
        pid: candidate.pid,
        user_common_id: candidate.user_common_id,
        masked_phone: candidate.masked_phone,
        avatar_url: cached_avatar_url
            .or_else(|| {
                existing_account
                    .as_ref()
                    .and_then(|account| account.avatar_url.clone())
            })
            .or_else(|| candidate_avatar_url.clone()),
        avatar_source_url: candidate_avatar_url.or_else(|| {
            existing_account
                .as_ref()
                .and_then(|account| account.avatar_source_url.clone())
        }),
        login_name: existing_account.as_ref().and_then(|a| a.login_name.clone()),
        note: existing_account.as_ref().and_then(|a| a.note.clone()),
        has_session_snapshot: true,
        has_credential: existing_account
            .as_ref()
            .map(|a| a.has_credential)
            .unwrap_or(false),
        has_login_state,
        created_at: data
            .accounts
            .iter()
            .find(|a| a.id == id)
            .map(|a| a.created_at.clone())
            .unwrap_or_else(|| timestamp.clone()),
        updated_at: timestamp,
        last_used_at: data
            .accounts
            .iter()
            .find(|a| a.id == id)
            .and_then(|a| a.last_used_at.clone()),
    };

    if let Some(pos) = data.accounts.iter().position(|a| a.id == id) {
        data.accounts[pos] = account.clone();
    } else {
        data.accounts.push(account.clone());
    }
    save_data(&data)?;
    *state.data.lock().map_err(|e| e.to_string())? = data;
    update_tray(&app);
    Ok(account)
}

fn collect_export_paths(
    root: &Path,
    current: &Path,
    files: &mut Vec<(PathBuf, String, u64)>,
    total_size: &mut u64,
) -> Result<(), String> {
    if !current.exists() {
        return Ok(());
    }
    for entry in fs::read_dir(current).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let path = entry.path();
        let file_type = entry.file_type().map_err(|e| e.to_string())?;
        if file_type.is_dir() {
            collect_export_paths(root, &path, files, total_size)?;
        } else if file_type.is_file() {
            if files.len() >= MAX_EXPORT_FILES {
                return Err("导出文件数量过多".to_string());
            }
            let size = entry.metadata().map_err(|e| e.to_string())?.len();
            *total_size = total_size
                .checked_add(size)
                .ok_or_else(|| "导出数据大小溢出".to_string())?;
            if *total_size > MAX_EXPORT_PACKAGE_BYTES {
                return Err("导出数据超过 512 MB，请减少账号后重试".to_string());
            }
            let relative = path
                .strip_prefix(root)
                .map_err(|e| e.to_string())?
                .to_string_lossy()
                .replace('\\', "/");
            safe_relative_path(&relative)?;
            files.push((path, relative, size));
        }
    }
    Ok(())
}

fn write_export_package_v3(path: &Path, accounts: &[SavedAccount]) -> Result<(), String> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .ok_or_else(|| "导出路径无效".to_string())?;
    fs::create_dir_all(parent).map_err(|e| format!("创建导出目录失败: {}", e))?;
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| "导出文件名无效".to_string())?;
    let suffix = Uuid::new_v4();
    let temp = parent.join(format!(".{}.{}.tmp", name, suffix));
    let backup = parent.join(format!(".{}.{}.bak", name, suffix));

    let mut manifest_accounts = Vec::with_capacity(accounts.len());
    let mut account_files = Vec::with_capacity(accounts.len());
    let mut total_size = 0u64;
    for (index, account) in accounts.iter().enumerate() {
        let oopz_login = read_oopz_login(&account.id)
            .ok_or_else(|| format!("{} 还不能导出，请先登录一次", account.display_name))?;
        let snapshot = account_snapshot_dir(&account.id)?;
        let mut files = Vec::new();
        collect_export_paths(&snapshot, &snapshot, &mut files, &mut total_size)?;
        if files.is_empty() {
            return Err(format!("{} 没有可导出的本地数据", account.display_name));
        }
        let directory = format!("account-{:03}", index);
        manifest_accounts.push(V3AccountManifest {
            directory: directory.clone(),
            account: ExportedAccount {
                display_name: account.display_name.clone(),
                uid: account.uid.clone(),
                pid: account.pid.clone(),
                user_common_id: account.user_common_id.clone(),
                masked_phone: account.masked_phone.clone(),
                avatar_url: account.avatar_url.clone(),
                note: account.note.clone(),
            },
            oopz_login,
        });
        account_files.push((directory, files));
    }

    let result = (|| -> Result<(), String> {
        let file = fs::File::create(&temp).map_err(|e| format!("创建导出文件失败: {}", e))?;
        let mut archive = ZipWriter::new(file);
        let options = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
        archive
            .start_file("manifest.json", options)
            .map_err(|e| e.to_string())?;
        let manifest = V3ExportManifest {
            format: NEA_EXPORT_FORMAT_V1.to_string(),
            app_id: "oopz".to_string(),
            exported_at: now(),
            accounts: manifest_accounts,
        };
        serde_json::to_writer(&mut archive, &manifest).map_err(|e| e.to_string())?;
        for (directory, files) in account_files {
            for (source, relative, _) in files {
                archive
                    .start_file(format!("accounts/{}/{}", directory, relative), options)
                    .map_err(|e| e.to_string())?;
                let mut source = fs::File::open(source).map_err(|e| e.to_string())?;
                std::io::copy(&mut source, &mut archive).map_err(|e| e.to_string())?;
            }
        }
        archive.finish().map_err(|e| e.to_string())?;
        if fs::metadata(&temp).map_err(|e| e.to_string())?.len() > MAX_V3_ARCHIVE_BYTES {
            return Err("v3 导出包超过文件大小限制".to_string());
        }
        Ok(())
    })();
    if let Err(error) = result {
        let _ = fs::remove_file(&temp);
        return Err(error);
    }
    if path.exists() {
        fs::rename(path, &backup).map_err(|e| format!("备份原导出文件失败: {}", e))?;
    }
    if let Err(error) = fs::rename(&temp, path) {
        if backup.exists() {
            let _ = fs::rename(&backup, path);
        }
        let _ = fs::remove_file(&temp);
        return Err(format!("导出失败: {}", error));
    }
    if backup.exists() {
        let _ = fs::remove_file(backup);
    }
    Ok(())
}

fn export_account_package_inner(
    app: &AppHandle,
    account_id: Option<&str>,
    path: &Path,
) -> Result<usize, String> {
    let state = app.state::<AppState>();
    let _operation = state.account_operation.lock().map_err(|e| e.to_string())?;
    let accounts = {
        let data = state.data.lock().map_err(|e| e.to_string())?;
        match account_id {
            Some(account_id) => vec![data
                .accounts
                .iter()
                .find(|account| account.id == account_id)
                .cloned()
                .ok_or_else(|| "账号不存在".to_string())?],
            None => data
                .accounts
                .iter()
                .filter(|account| account.has_login_state)
                .cloned()
                .collect(),
        }
    };
    if accounts.is_empty() {
        return Err("没有可导出的账号登录态".to_string());
    }
    if accounts.len() > MAX_EXPORT_ACCOUNTS {
        return Err(format!("一次最多导出 {} 个账号", MAX_EXPORT_ACCOUNTS));
    }
    let count = accounts.len();
    write_export_package_v3(path, &accounts)?;
    Ok(count)
}

#[tauri::command]
async fn export_account_package(
    app: AppHandle,
    account_id: String,
    path: String,
) -> Result<usize, String> {
    tauri::async_runtime::spawn_blocking(move || {
        export_account_package_inner(&app, Some(&account_id), Path::new(&path))
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
async fn export_all_accounts_package(app: AppHandle, path: String) -> Result<usize, String> {
    tauri::async_runtime::spawn_blocking(move || {
        export_account_package_inner(&app, None, Path::new(&path))
    })
    .await
    .map_err(|e| e.to_string())?
}

fn read_export_package(path: &Path) -> Result<Vec<ExportedAccountEntry>, String> {
    let size = fs::metadata(path)
        .map_err(|e| format!("读取导入文件失败: {}", e))?
        .len();
    if size == 0 || size > MAX_LEGACY_EXPORT_PACKAGE_BYTES {
        return Err("旧版导入文件为空或超过 128 MB，请先使用旧客户端拆分账号".to_string());
    }
    let raw = fs::read_to_string(path).map_err(|e| format!("读取导入文件失败: {}", e))?;
    let value: serde_json::Value =
        serde_json::from_str(&raw).map_err(|e| format!("导入文件格式不正确: {}", e))?;
    let format = value
        .get("format")
        .and_then(|format| format.as_str())
        .ok_or_else(|| "导入文件缺少格式标识".to_string())?;
    let accounts = match format {
        EXPORT_FORMAT => {
            serde_json::from_value::<AccountExportPackage>(value)
                .map_err(|e| format!("导入文件格式不正确: {}", e))?
                .accounts
        }
        LEGACY_EXPORT_FORMAT => {
            let legacy = serde_json::from_value::<LegacyAccountExportPackage>(value)
                .map_err(|e| format!("旧版导入文件格式不正确: {}", e))?;
            vec![ExportedAccountEntry {
                account: legacy.account,
                oopz_login: legacy.oopz_login,
                files: legacy.files,
            }]
        }
        _ => return Err("不支持的导入文件".to_string()),
    };
    if accounts.is_empty() || accounts.len() > MAX_EXPORT_ACCOUNTS {
        return Err(format!(
            "导入包必须包含 1 到 {} 个账号",
            MAX_EXPORT_ACCOUNTS
        ));
    }
    let file_count: usize = accounts.iter().map(|entry| entry.files.len()).sum();
    if file_count > MAX_EXPORT_FILES {
        return Err("导入包包含的文件数量过多".to_string());
    }
    let mut uids = HashSet::new();
    for entry in &accounts {
        if entry.account.display_name.trim().is_empty()
            || entry.oopz_login.trim().is_empty()
            || entry.files.is_empty()
        {
            return Err("导入文件缺少账号数据".to_string());
        }
        if let Some(uid) = entry.account.uid.as_deref() {
            if !uids.insert(uid) {
                return Err("导入包包含重复账号".to_string());
            }
        }
        for file in &entry.files {
            safe_relative_path(&file.path)?;
            general_purpose::STANDARD
                .decode(&file.data_base64)
                .map_err(|_| "导入文件包含损坏的数据".to_string())?;
        }
    }
    Ok(accounts)
}

fn imported_account_from_export(data: &AppData, exported: ExportedAccount) -> SavedAccount {
    let existing_id = exported.uid.as_ref().and_then(|uid| {
        data.accounts
            .iter()
            .find(|account| account.uid.as_ref() == Some(uid))
            .map(|account| account.id.clone())
    });
    let existing_account = existing_id
        .as_ref()
        .and_then(|id| data.accounts.iter().find(|account| account.id == *id))
        .cloned();
    let id = existing_id.unwrap_or_else(|| Uuid::new_v4().to_string());
    let timestamp = now();
    SavedAccount {
        id: id.clone(),
        display_name: exported.display_name,
        uid: exported.uid,
        pid: exported.pid,
        user_common_id: exported.user_common_id,
        masked_phone: exported.masked_phone,
        avatar_url: exported.avatar_url,
        avatar_source_url: existing_account
            .as_ref()
            .and_then(|account| account.avatar_source_url.clone()),
        login_name: existing_account
            .as_ref()
            .and_then(|account| account.login_name.clone()),
        note: exported.note.or_else(|| {
            existing_account
                .as_ref()
                .and_then(|account| account.note.clone())
        }),
        has_session_snapshot: true,
        has_credential: existing_account
            .as_ref()
            .map(|account| account.has_credential)
            .unwrap_or(false),
        has_login_state: true,
        created_at: existing_account
            .as_ref()
            .map(|account| account.created_at.clone())
            .unwrap_or_else(|| timestamp.clone()),
        updated_at: timestamp,
        last_used_at: existing_account.and_then(|account| account.last_used_at),
    }
}

const MISSING_CREDENTIAL_MARKER: &str = "OOPZPLUS_TRANSACTION_NO_CREDENTIAL";

fn valid_storage_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '-')
}

fn import_transactions_dir() -> Result<PathBuf, String> {
    Ok(storage_dir()?.join(".transactions"))
}

fn write_import_journal(root: &Path, journal: &ImportJournal) -> Result<(), String> {
    let path = root.join("journal.json");
    let temp = root.join("journal.json.tmp");
    let raw = serde_json::to_vec_pretty(journal).map_err(|e| e.to_string())?;
    fs::write(&temp, raw).map_err(|e| e.to_string())?;
    fs::rename(temp, path).map_err(|e| e.to_string())
}

fn cleanup_transaction_credentials(journal: &ImportJournal) {
    for entry in &journal.entries {
        delete_credential(&entry.credential_backup_id);
    }
}

fn rollback_snapshot_entry(
    root: &Path,
    entry: &ImportJournalEntry,
    target: &Path,
) -> Result<(), String> {
    let backup = root.join("backup").join(&entry.account_id).join("snapshot");
    if backup.exists() {
        if target.exists() {
            fs::remove_dir_all(target).map_err(|e| e.to_string())?;
        }
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        fs::rename(backup, target).map_err(|e| e.to_string())?;
    } else if !entry.had_snapshot && entry.phase != "pending" && target.exists() {
        fs::remove_dir_all(target).map_err(|e| e.to_string())?;
    }
    Ok(())
}

fn rollback_import_transaction(root: &Path, journal: &ImportJournal) -> Result<(), String> {
    let mut first_error = None;
    for entry in &journal.entries {
        if !valid_storage_id(&entry.account_id) {
            first_error.get_or_insert_with(|| "导入事务包含无效账号 ID".to_string());
            continue;
        }
        let target = account_snapshot_dir(&entry.account_id)?;
        if let Err(error) = rollback_snapshot_entry(root, entry, &target) {
            first_error.get_or_insert(error);
        }
        if let Some(raw) = read_secret_raw(&entry.credential_backup_id) {
            let result = if raw == MISSING_CREDENTIAL_MARKER {
                delete_credential(&entry.account_id);
                Ok(())
            } else {
                write_secret_raw(&entry.account_id, &raw)
            };
            if let Err(error) = result {
                first_error.get_or_insert(error);
            }
        }
    }
    let config = config_path()?;
    let config_backup = root.join("config.backup");
    if journal.config_existed && config_backup.exists() {
        if let Err(error) = fs::copy(config_backup, config) {
            first_error.get_or_insert_with(|| error.to_string());
        }
    } else if !journal.config_existed {
        let _ = fs::remove_file(config);
    }
    cleanup_transaction_credentials(journal);
    if let Some(error) = first_error {
        Err(error)
    } else {
        let _ = fs::remove_dir_all(root);
        Ok(())
    }
}

fn recover_import_transactions() {
    let Ok(base) = import_transactions_dir() else {
        return;
    };
    let Ok(entries) = fs::read_dir(&base) else {
        return;
    };
    for entry in entries.flatten() {
        let root = entry.path();
        if !root.is_dir() {
            continue;
        }
        let journal = fs::read(root.join("journal.json"))
            .ok()
            .and_then(|raw| serde_json::from_slice::<ImportJournal>(&raw).ok());
        match journal {
            Some(journal) if journal.status == "committed" => {
                cleanup_transaction_credentials(&journal);
                let _ = fs::remove_dir_all(root);
            }
            Some(journal) => {
                let _ = rollback_import_transaction(&root, &journal);
            }
            None => {
                let _ = fs::remove_dir_all(root);
            }
        }
    }
}

fn commit_prepared_import(
    app: &AppHandle,
    root: &Path,
    mut data: AppData,
    prepared: Vec<PreparedImportAccount>,
) -> Result<Vec<SavedAccount>, String> {
    let config = config_path()?;
    let config_existed = config.exists();
    if config_existed {
        fs::copy(&config, root.join("config.backup")).map_err(|e| e.to_string())?;
    }
    let mut journal = ImportJournal {
        id: root
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default()
            .to_string(),
        status: "prepared".to_string(),
        config_existed,
        entries: Vec::with_capacity(prepared.len()),
    };
    for item in &prepared {
        if !valid_storage_id(&item.account.id) {
            return Err("导入账号 ID 无效".to_string());
        }
        let backup_id = format!("__transaction-{}-{}", journal.id, item.account.id);
        journal.entries.push(ImportJournalEntry {
            account_id: item.account.id.clone(),
            had_snapshot: account_snapshot_dir(&item.account.id)?.exists(),
            credential_backup_id: backup_id,
            phase: "pending".to_string(),
        });
    }
    if let Err(error) = write_import_journal(root, &journal) {
        let _ = fs::remove_dir_all(root);
        return Err(error);
    }
    for entry in &journal.entries {
        let old_raw = read_secret_raw(&entry.account_id)
            .unwrap_or_else(|| MISSING_CREDENTIAL_MARKER.to_string());
        if let Err(error) = write_secret_raw(&entry.credential_backup_id, &old_raw) {
            delete_credential(&entry.credential_backup_id);
            let rollback = rollback_import_transaction(root, &journal).err();
            return match rollback {
                Some(rollback) => Err(format!("{}；清理事务失败: {}", error, rollback)),
                None => Err(error),
            };
        }
    }

    let commit_result = (|| -> Result<Vec<SavedAccount>, String> {
        for item in &prepared {
            let entry_index = journal
                .entries
                .iter()
                .position(|entry| entry.account_id == item.account.id)
                .ok_or_else(|| "导入事务清单不完整".to_string())?;
            journal.entries[entry_index].phase = "replacing".to_string();
            write_import_journal(root, &journal)?;
            let target = account_snapshot_dir(&item.account.id)?;
            let backup = root.join("backup").join(&item.account.id).join("snapshot");
            if target.exists() {
                if let Some(parent) = backup.parent() {
                    fs::create_dir_all(parent).map_err(|e| e.to_string())?;
                }
                fs::rename(&target, &backup).map_err(|e| e.to_string())?;
            }
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent).map_err(|e| e.to_string())?;
            }
            fs::rename(&item.staged_snapshot, &target).map_err(|e| e.to_string())?;
            journal.entries[entry_index].phase = "snapshot-moved".to_string();
            write_import_journal(root, &journal)?;
        }
        journal.status = "snapshots-committed".to_string();
        write_import_journal(root, &journal)?;
        for item in &prepared {
            store_oopz_login(&item.account.id, &item.oopz_login)?;
            if let Some(pos) = data
                .accounts
                .iter()
                .position(|account| account.id == item.account.id)
            {
                data.accounts[pos] = item.account.clone();
            } else {
                data.accounts.push(item.account.clone());
            }
        }
        save_data(&data)?;
        *app.state::<AppState>()
            .data
            .lock()
            .map_err(|e| e.to_string())? = data;
        journal.status = "committed".to_string();
        write_import_journal(root, &journal)?;
        Ok(prepared.iter().map(|item| item.account.clone()).collect())
    })();

    match commit_result {
        Ok(imported) => {
            cleanup_transaction_credentials(&journal);
            let _ = fs::remove_dir_all(root);
            Ok(imported)
        }
        Err(error) => {
            let rollback_error = rollback_import_transaction(root, &journal).err();
            *app.state::<AppState>()
                .data
                .lock()
                .map_err(|e| e.to_string())? = load_data();
            match rollback_error {
                Some(rollback) => Err(format!("{}；回滚失败: {}", error, rollback)),
                None => Err(error),
            }
        }
    }
}

fn prepare_legacy_import(
    root: &Path,
    data: &AppData,
    packages: Vec<ExportedAccountEntry>,
) -> Result<Vec<PreparedImportAccount>, String> {
    let mut prepared = Vec::with_capacity(packages.len());
    for package in packages {
        let account = imported_account_from_export(data, package.account);
        let staged_snapshot = root.join("staging").join(&account.id).join("snapshot");
        write_export_files(&staged_snapshot, &package.files)?;
        prepared.push(PreparedImportAccount {
            account,
            oopz_login: package.oopz_login,
            staged_snapshot,
        });
    }
    Ok(prepared)
}

fn read_v3_manifest(archive: &mut ZipArchive<fs::File>) -> Result<V3ExportManifest, String> {
    let mut manifest_file = archive
        .by_name("manifest.json")
        .map_err(|_| "v3 导入包缺少 manifest.json".to_string())?;
    if manifest_file.size() > 1024 * 1024 {
        return Err("导入清单过大".to_string());
    }
    let mut raw = Vec::with_capacity(manifest_file.size() as usize);
    manifest_file
        .read_to_end(&mut raw)
        .map_err(|e| e.to_string())?;
    let manifest: V3ExportManifest = serde_json::from_slice(&raw).map_err(|e| e.to_string())?;
    if (manifest.format != EXPORT_FORMAT_V3 && manifest.format != NEA_EXPORT_FORMAT_V1)
        || !manifest.app_id.eq_ignore_ascii_case("oopz")
        || manifest.accounts.is_empty()
        || manifest.accounts.len() > MAX_EXPORT_ACCOUNTS
    {
        return Err("v3 导入清单无效".to_string());
    }
    Ok(manifest)
}

fn prepare_v3_import(
    root: &Path,
    data: &AppData,
    path: &Path,
) -> Result<Vec<PreparedImportAccount>, String> {
    if fs::metadata(path).map_err(|e| e.to_string())?.len() > MAX_V3_ARCHIVE_BYTES {
        return Err("v3 导入包超过文件大小限制".to_string());
    }
    let file = fs::File::open(path).map_err(|e| e.to_string())?;
    let mut archive = ZipArchive::new(file).map_err(|e| format!("打开 v3 导入包失败: {}", e))?;
    let manifest = read_v3_manifest(&mut archive)?;
    let mut directory_indexes = HashMap::new();
    let mut imported_uids = HashSet::new();
    let mut account_ids = HashSet::new();
    let mut prepared = Vec::with_capacity(manifest.accounts.len());
    for item in manifest.accounts {
        if item.directory.is_empty()
            || !item
                .directory
                .chars()
                .all(|character| character.is_ascii_alphanumeric() || character == '-')
            || directory_indexes.contains_key(&item.directory)
            || item.account.display_name.trim().is_empty()
            || item.oopz_login.trim().is_empty()
        {
            return Err("v3 导入清单包含无效账号".to_string());
        }
        if item
            .account
            .uid
            .as_ref()
            .is_some_and(|uid| !imported_uids.insert(uid.clone()))
        {
            return Err("v3 导入清单包含重复账号".to_string());
        }
        let account = imported_account_from_export(data, item.account);
        if !account_ids.insert(account.id.clone()) {
            return Err("v3 导入清单包含重复目标账号".to_string());
        }
        let prepared_index = prepared.len();
        directory_indexes.insert(item.directory.clone(), prepared_index);
        let staged_snapshot = root.join("staging").join(&account.id).join("snapshot");
        fs::create_dir_all(&staged_snapshot).map_err(|e| e.to_string())?;
        prepared.push(PreparedImportAccount {
            account,
            oopz_login: item.oopz_login,
            staged_snapshot,
        });
    }

    let mut seen = HashSet::new();
    let mut files_per_directory = HashMap::<String, usize>::new();
    let mut total_size = 0u64;
    let mut manifest_count = 0usize;
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index).map_err(|e| e.to_string())?;
        let name = entry.name().replace('\\', "/");
        if name == "manifest.json" {
            manifest_count += 1;
            continue;
        }
        if name.ends_with('/') {
            continue;
        }
        if entry
            .unix_mode()
            .is_some_and(|mode| mode & 0o170000 == 0o120000)
        {
            return Err("v3 导入包不能包含符号链接".to_string());
        }
        let Some(rest) = name.strip_prefix("accounts/") else {
            return Err("v3 导入包包含未知文件".to_string());
        };
        let Some((directory, relative)) = rest.split_once('/') else {
            return Err("v3 导入包路径无效".to_string());
        };
        let relative_path = safe_relative_path(relative)?;
        if relative_path.as_os_str().is_empty()
            || !seen.insert((directory.to_string(), relative.to_string()))
        {
            return Err("v3 导入包包含重复或无效路径".to_string());
        }
        if entry.size() > MAX_EXPORT_PACKAGE_BYTES || seen.len() > MAX_EXPORT_FILES {
            return Err("v3 导入包内容超过限制".to_string());
        }
        let prepared_index = directory_indexes
            .get(directory)
            .copied()
            .ok_or_else(|| "v3 导入包账号目录不存在".to_string())?;
        let target = prepared[prepared_index].staged_snapshot.join(relative_path);
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let mut output = fs::File::create(target).map_err(|e| e.to_string())?;
        let remaining = MAX_EXPORT_PACKAGE_BYTES.saturating_sub(total_size);
        let written = std::io::copy(&mut entry.by_ref().take(remaining + 1), &mut output)
            .map_err(|e| e.to_string())?;
        total_size = total_size
            .checked_add(written)
            .ok_or_else(|| "导入数据大小溢出".to_string())?;
        if total_size > MAX_EXPORT_PACKAGE_BYTES {
            return Err("v3 导入包内容超过限制".to_string());
        }
        if written != entry.size() {
            return Err("v3 导入包文件大小不一致".to_string());
        }
        *files_per_directory
            .entry(directory.to_string())
            .or_default() += 1;
    }
    if manifest_count != 1 {
        return Err("v3 导入包必须包含唯一清单".to_string());
    }
    for directory in directory_indexes.keys() {
        if files_per_directory.get(directory).copied().unwrap_or(0) == 0 {
            return Err("v3 导入包缺少账号文件".to_string());
        }
    }
    Ok(prepared)
}

fn is_v3_package(path: &Path) -> bool {
    let mut header = [0u8; 4];
    fs::File::open(path)
        .and_then(|mut file| file.read_exact(&mut header))
        .is_ok()
        && header == *b"PK\x03\x04"
}

fn import_account_package_inner(app: &AppHandle, path: &Path) -> Result<Vec<SavedAccount>, String> {
    let state = app.state::<AppState>();
    let _operation = state.account_operation.lock().map_err(|e| e.to_string())?;
    let data = state.data.lock().map_err(|e| e.to_string())?.clone();
    let transaction_id = Uuid::new_v4().to_string();
    let root = import_transactions_dir()?.join(&transaction_id);
    fs::create_dir_all(root.join("staging")).map_err(|e| e.to_string())?;
    let prepared_result = if is_v3_package(path) {
        prepare_v3_import(&root, &data, path)
    } else {
        read_export_package(path).and_then(|packages| prepare_legacy_import(&root, &data, packages))
    };
    let prepared = match prepared_result {
        Ok(prepared) => prepared,
        Err(error) => {
            let _ = fs::remove_dir_all(&root);
            return Err(error);
        }
    };
    let config = data.config.clone();
    let imported = commit_prepared_import(app, &root, data, prepared)?;
    update_tray(app);
    ensure_plugin_runtime_for_oopz(&config);
    let _ = app.emit("app-data-changed", ());
    Ok(imported)
}

#[tauri::command]
async fn import_account_package(app: AppHandle, path: String) -> Result<Vec<SavedAccount>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        import_account_package_inner(&app, Path::new(&path))
    })
    .await
    .map_err(|e| e.to_string())?
}

fn emit_wormhole_status(
    app: &AppHandle,
    state: &str,
    direction: &str,
    message: impl Into<String>,
    code: Option<String>,
    progress: Option<(u64, u64)>,
) {
    let (transferred, total) = progress.map_or((None, None), |(transferred, total)| {
        (Some(transferred), Some(total))
    });
    let _ = app.emit(
        "wormhole-status",
        WormholeStatus {
            state: state.to_string(),
            direction: direction.to_string(),
            message: message.into(),
            code,
            transferred,
            total,
        },
    );
}

fn wormhole_relay_hints() -> Result<Vec<transit::RelayHint>, String> {
    let relay_url = transit::DEFAULT_RELAY_SERVER
        .parse()
        .map_err(|e| format!("公共中继地址无效: {}", e))?;
    let hint = transit::RelayHint::from_urls(None, [relay_url])
        .map_err(|e| format!("公共中继配置失败: {}", e))?;
    Ok(vec![hint])
}

fn wormhole_temp_package(prefix: &str) -> PathBuf {
    std::env::temp_dir().join(format!("{}-{}.oopz+", prefix, Uuid::new_v4()))
}

async fn wait_for_quick_share_cancel(app: AppHandle) {
    loop {
        if app
            .state::<AppState>()
            .wormhole_cancelled
            .load(Ordering::SeqCst)
        {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

fn finish_wormhole_operation(app: &AppHandle) {
    let state = app.state::<AppState>();
    state.wormhole_running.store(false, Ordering::SeqCst);
    state.wormhole_cancelled.store(false, Ordering::SeqCst);
}

#[tauri::command]
fn cancel_quick_share(app: AppHandle) -> Result<(), String> {
    let state = app.state::<AppState>();
    if !state.wormhole_running.load(Ordering::SeqCst) {
        return Ok(());
    }
    state.wormhole_cancelled.store(true, Ordering::SeqCst);
    Ok(())
}

#[tauri::command]
async fn start_quick_export(app: AppHandle) -> Result<String, String> {
    if app
        .state::<AppState>()
        .wormhole_running
        .swap(true, Ordering::SeqCst)
    {
        return Err("已有快捷分享或导入正在进行".to_string());
    }
    app.state::<AppState>()
        .wormhole_cancelled
        .store(false, Ordering::SeqCst);
    emit_wormhole_status(
        &app,
        "preparing",
        "send",
        "正在打包全部账号登录态...",
        None,
        None,
    );
    let package_path = wormhole_temp_package("oopz-plus-share");
    let build_app = app.clone();
    let build_path = package_path.clone();
    let build_result = match tauri::async_runtime::spawn_blocking(move || {
        export_account_package_inner(&build_app, None, &build_path)
    })
    .await
    {
        Ok(result) => result,
        Err(error) => Err(format!("打包任务异常结束: {}", error)),
    };
    if let Err(error) = build_result {
        finish_wormhole_operation(&app);
        emit_wormhole_status(&app, "error", "send", &error, None, None);
        return Err(error);
    }

    if app
        .state::<AppState>()
        .wormhole_cancelled
        .load(Ordering::SeqCst)
    {
        let _ = fs::remove_file(&package_path);
        finish_wormhole_operation(&app);
        emit_wormhole_status(&app, "cancelled", "send", QUICK_SHARE_CANCELLED, None, None);
        return Err(QUICK_SHARE_CANCELLED.to_string());
    }

    let create_app = app.clone();
    let mailbox_result = tokio::select! {
        result = tokio::time::timeout(
            Duration::from_secs(30),
            MailboxConnection::create(transfer::APP_CONFIG, WORMHOLE_CODE_WORDS),
        ) => Some(result),
        _ = wait_for_quick_share_cancel(create_app) => None,
    };
    let mailbox = match mailbox_result {
        None => {
            let _ = fs::remove_file(&package_path);
            finish_wormhole_operation(&app);
            emit_wormhole_status(&app, "cancelled", "send", QUICK_SHARE_CANCELLED, None, None);
            return Err(QUICK_SHARE_CANCELLED.to_string());
        }
        Some(Ok(Ok(mailbox))) => mailbox,
        Some(Ok(Err(error))) => {
            let _ = fs::remove_file(&package_path);
            finish_wormhole_operation(&app);
            let message = format!("创建快捷码失败: {}", error);
            emit_wormhole_status(&app, "error", "send", &message, None, None);
            return Err(message);
        }
        Some(Err(_)) => {
            let _ = fs::remove_file(&package_path);
            finish_wormhole_operation(&app);
            let message = "连接 Magic Wormhole 服务超时".to_string();
            emit_wormhole_status(&app, "error", "send", &message, None, None);
            return Err(message);
        }
    };
    let code = mailbox.code().to_string();
    emit_wormhole_status(
        &app,
        "waiting",
        "send",
        "快捷码已生成，等待对方输入...",
        Some(code.clone()),
        None,
    );

    let transfer_app = app.clone();
    let transfer_code = code.clone();
    tauri::async_runtime::spawn(async move {
        let final_code = transfer_code.clone();
        let result = tokio::time::timeout(Duration::from_secs(WORMHOLE_TIMEOUT_SECONDS), async {
            let connect_app = transfer_app.clone();
            let wormhole = tokio::select! {
                result = Wormhole::connect(mailbox) => {
                    result.map_err(|e| format!("建立加密连接失败: {}", e))?
                }
                _ = wait_for_quick_share_cancel(connect_app) => {
                    return Err(QUICK_SHARE_CANCELLED.to_string());
                }
            };
            let offer = transfer::offer::OfferSend::new_file_or_folder(
                "oopz-plus-login-states.oopz+".to_string(),
                &package_path,
            )
            .await
            .map_err(|e| format!("准备传输文件失败: {}", e))?;
            let relay_hints = wormhole_relay_hints()?;
            let connected_app = transfer_app.clone();
            let progress_app = transfer_app.clone();
            let progress_code = transfer_code.clone();
            let cancel_app = transfer_app.clone();
            let last_progress = Arc::new(AtomicU64::new(u64::MAX));
            let progress_marker = last_progress.clone();
            transfer::send(
                wormhole,
                relay_hints,
                transit::Abilities::ALL,
                offer,
                move |_info| {
                    emit_wormhole_status(
                        &connected_app,
                        "transferring",
                        "send",
                        "已连接，正在发送登录态...",
                        Some(transfer_code),
                        None,
                    );
                },
                move |transferred, total| {
                    let percent = if total == 0 {
                        0
                    } else {
                        transferred.saturating_mul(100) / total
                    };
                    if progress_marker.swap(percent, Ordering::Relaxed) != percent {
                        emit_wormhole_status(
                            &progress_app,
                            "transferring",
                            "send",
                            format!("正在发送... {}%", percent),
                            Some(progress_code.clone()),
                            Some((transferred, total)),
                        );
                    }
                },
                wait_for_quick_share_cancel(cancel_app),
            )
            .await
            .map_err(|e| {
                if transfer_app
                    .state::<AppState>()
                    .wormhole_cancelled
                    .load(Ordering::SeqCst)
                {
                    QUICK_SHARE_CANCELLED.to_string()
                } else {
                    format!("发送失败: {}", e)
                }
            })
        })
        .await;
        let _ = fs::remove_file(&package_path);
        let was_cancelled = transfer_app
            .state::<AppState>()
            .wormhole_cancelled
            .load(Ordering::SeqCst);
        finish_wormhole_operation(&transfer_app);
        if was_cancelled {
            emit_wormhole_status(
                &transfer_app,
                "cancelled",
                "send",
                QUICK_SHARE_CANCELLED,
                Some(final_code),
                None,
            );
            return;
        }
        match result {
            Ok(Ok(())) => emit_wormhole_status(
                &transfer_app,
                "complete",
                "send",
                "快捷分享完成",
                Some(final_code),
                None,
            ),
            Ok(Err(error)) => emit_wormhole_status(
                &transfer_app,
                "error",
                "send",
                error,
                Some(final_code),
                None,
            ),
            Err(_) => emit_wormhole_status(
                &transfer_app,
                "error",
                "send",
                "快捷分享已超时，请重新生成代码",
                Some(final_code),
                None,
            ),
        }
    });
    Ok(code)
}

async fn receive_wormhole_package(
    app: &AppHandle,
    code: Code,
    target: &Path,
) -> Result<(), String> {
    let mailbox = MailboxConnection::connect(transfer::APP_CONFIG, code, false)
        .await
        .map_err(|e| format!("连接快捷码失败: {}", e))?;
    let wormhole = Wormhole::connect(mailbox)
        .await
        .map_err(|e| format!("建立加密连接失败: {}", e))?;
    let request = transfer::request_file(
        wormhole,
        wormhole_relay_hints()?,
        transit::Abilities::ALL,
        pending(),
    )
    .await
    .map_err(|e| format!("接收请求失败: {}", e))?
    .ok_or_else(|| "对方已取消传输".to_string())?;
    if !request.file_name().ends_with(".oopz+")
        || request.file_size() == 0
        || request.file_size() > MAX_V3_ARCHIVE_BYTES
    {
        let _ = request.reject().await;
        return Err("对方发送的不是有效 OOPZ+ 登录态包".to_string());
    }
    let mut file = async_fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(target)
        .await
        .map_err(|e| format!("创建接收文件失败: {}", e))?;
    let connected_app = app.clone();
    let progress_app = app.clone();
    let last_progress = Arc::new(AtomicU64::new(u64::MAX));
    let progress_marker = last_progress.clone();
    request
        .accept(
            move |_info| {
                emit_wormhole_status(
                    &connected_app,
                    "transferring",
                    "receive",
                    "已连接，正在接收登录态...",
                    None,
                    None,
                );
            },
            move |transferred, total| {
                let percent = if total == 0 {
                    0
                } else {
                    transferred.saturating_mul(100) / total
                };
                if progress_marker.swap(percent, Ordering::Relaxed) != percent {
                    emit_wormhole_status(
                        &progress_app,
                        "transferring",
                        "receive",
                        format!("正在接收... {}%", percent),
                        None,
                        Some((transferred, total)),
                    );
                }
            },
            &mut file,
            pending(),
        )
        .await
        .map_err(|e| format!("接收失败: {}", e))?;
    file.flush()
        .await
        .map_err(|e| format!("保存接收文件失败: {}", e))?;
    Ok(())
}

#[tauri::command]
async fn quick_import(app: AppHandle, code: String) -> Result<Vec<SavedAccount>, String> {
    let code = code
        .trim()
        .parse::<Code>()
        .map_err(|e| format!("快捷码格式不正确: {}", e))?;
    if app
        .state::<AppState>()
        .wormhole_running
        .swap(true, Ordering::SeqCst)
    {
        return Err("已有快捷分享或导入正在进行".to_string());
    }
    app.state::<AppState>()
        .wormhole_cancelled
        .store(false, Ordering::SeqCst);
    emit_wormhole_status(
        &app,
        "connecting",
        "receive",
        "正在连接发送方...",
        None,
        None,
    );
    let package_path = wormhole_temp_package("oopz-plus-receive");
    let receive_app = app.clone();
    let receive_result = tokio::time::timeout(Duration::from_secs(WORMHOLE_TIMEOUT_SECONDS), async {
        tokio::select! {
            result = receive_wormhole_package(&app, code, &package_path) => result,
            _ = wait_for_quick_share_cancel(receive_app) => Err(QUICK_SHARE_CANCELLED.to_string()),
        }
    })
    .await;
    let result = match receive_result {
        Ok(Ok(())) => {
            emit_wormhole_status(
                &app,
                "importing",
                "receive",
                "接收完成，正在校验并导入...",
                None,
                None,
            );
            let import_app = app.clone();
            let import_path = package_path.clone();
            match tauri::async_runtime::spawn_blocking(move || {
                import_account_package_inner(&import_app, &import_path)
            })
            .await
            {
                Ok(result) => result,
                Err(error) => Err(format!("导入任务异常结束: {}", error)),
            }
        }
        Ok(Err(error)) => Err(error),
        Err(_) => Err("快捷导入已超时，请确认代码并重试".to_string()),
    };
    let _ = fs::remove_file(&package_path);
    finish_wormhole_operation(&app);
    match &result {
        Ok(accounts) => emit_wormhole_status(
            &app,
            "complete",
            "receive",
            format!("快捷导入完成，共 {} 个账号", accounts.len()),
            None,
            None,
        ),
        Err(error) if error == QUICK_SHARE_CANCELLED => emit_wormhole_status(
            &app,
            "cancelled",
            "receive",
            QUICK_SHARE_CANCELLED,
            None,
            None,
        ),
        Err(error) => emit_wormhole_status(&app, "error", "receive", error, None, None),
    }
    result
}

#[tauri::command]
async fn delete_account(app: AppHandle, account_id: String) -> Result<(), String> {
    let app_for_task = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let state = app_for_task.state::<AppState>();
        delete_account_inner(app_for_task.clone(), state, account_id)
    })
    .await
    .map_err(|e| e.to_string())?
}

fn delete_account_inner(
    app: AppHandle,
    state: State<AppState>,
    account_id: String,
) -> Result<(), String> {
    let _operation = state.account_operation.lock().map_err(|e| e.to_string())?;
    let mut data = state.data.lock().map_err(|e| e.to_string())?.clone();
    data.accounts.retain(|a| a.id != account_id);
    save_data(&data)?;
    *state.data.lock().map_err(|e| e.to_string())? = data;
    delete_credential(&account_id);
    let dir = accounts_dir()?.join(&account_id);
    if dir.exists() {
        fs::remove_dir_all(dir).map_err(|e| e.to_string())?;
    }
    update_tray(&app);
    Ok(())
}

#[tauri::command]
async fn open_oopz(app: AppHandle) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        open_oopz_inner(state)
    })
    .await
    .map_err(|e| e.to_string())?
}

fn open_oopz_inner(state: State<AppState>) -> Result<(), String> {
    let paths = {
        let data = state.data.lock().map_err(|e| e.to_string())?;
        paths_from_config(&data.config)?
    };
    Command::new(paths.oopz_exe_path)
        .spawn()
        .map_err(|e| format!("启动 OOPZ 失败: {}", e))?;
    Ok(())
}

fn close_oopz_if_running() -> Result<(), String> {
    let mut system = process_system();
    let pids: Vec<_> = system
        .processes()
        .iter()
        .filter_map(|(pid, process)| is_oopz_process_name(process.name()).then_some(*pid))
        .collect();

    if pids.is_empty() {
        return Ok(());
    }

    for pid in &pids {
        if let Some(process) = system.process(*pid) {
            let _ = process
                .kill_with(Signal::Term)
                .unwrap_or_else(|| process.kill());
        }
    }

    thread::sleep(Duration::from_millis(1200));
    refresh_process_system(&mut system);

    for pid in pids {
        if let Some(process) = system.process(pid) {
            if is_oopz_process_name(process.name()) {
                let _ = process.kill();
            }
        }
    }

    Ok(())
}

fn backup_current(paths: &OopzPaths) -> Result<(), String> {
    let backup = backups_dir()?.join("latest-before-switch");
    let backup_parent = backup.parent().ok_or_else(|| "备份目录无效".to_string())?;
    fs::create_dir_all(backup_parent).map_err(|e| e.to_string())?;
    let staging = backup_parent.join(format!(".oopzplus-backup-{}", Uuid::new_v4()));
    fs::create_dir_all(&staging).map_err(|e| e.to_string())?;
    let Some(login) = current_registry_login() else {
        return commit_prepared_dir(&staging, &backup);
    };
    let write_result = (|| -> Result<(), String> {
        fs::write(staging.join("registry_login.txt"), &login).map_err(|e| e.to_string())?;
        if let Some(uid) = uid_from_registry_login(&login) {
            copy_dir_contents(
                &PathBuf::from(&paths.roaming_data_dir).join(&uid),
                &staging.join("roaming").join(&uid),
            )?;
            copy_dir_contents(
                &PathBuf::from(&paths.local_sandbox_dir).join(&uid),
                &staging.join("local_sandbox").join(&uid),
            )?;
        }
        Ok(())
    })();
    if let Err(error) = write_result {
        let _ = fs::remove_dir_all(&staging);
        return Err(error);
    }
    if let Err(error) = commit_prepared_dir(&staging, &backup) {
        let _ = fs::remove_dir_all(&staging);
        return Err(error);
    }
    Ok(())
}

#[tauri::command]
async fn restore_latest_backup(app: AppHandle) -> Result<SwitchResult, String> {
    let app_for_task = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let state = app_for_task.state::<AppState>();
        restore_latest_backup_inner(state)
    })
    .await
    .map_err(|e| e.to_string())?
}

fn restore_latest_backup_inner(state: State<AppState>) -> Result<SwitchResult, String> {
    let data = state.data.lock().map_err(|e| e.to_string())?;
    let paths = paths_from_config(&data.config)?;
    drop(data);
    let backup = backups_dir()?.join("latest-before-switch");
    if !backup.exists() {
        return Err("没有可恢复的备份".to_string());
    }
    close_oopz_if_running()?;
    copy_backup_children(&backup.join("roaming"), Path::new(&paths.roaming_data_dir))?;
    copy_backup_children(
        &backup.join("local_sandbox"),
        Path::new(&paths.local_sandbox_dir),
    )?;
    let login_backup = backup.join("registry_login.txt");
    if login_backup.exists() {
        let login = fs::read_to_string(login_backup).map_err(|e| e.to_string())?;
        write_registry_login(login.trim())?;
    }
    Command::new(paths.oopz_exe_path)
        .spawn()
        .map_err(|e| e.to_string())?;
    Ok(SwitchResult {
        ok: true,
        message: "已恢复最近一次切换前备份并启动 OOPZ".to_string(),
    })
}

#[tauri::command]
async fn switch_account(app: AppHandle, account_id: String) -> Result<SwitchResult, String> {
    let app_for_task = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let state = app_for_task.state::<AppState>();
        switch_account_inner(app_for_task.clone(), state, account_id)
    })
    .await
    .map_err(|e| e.to_string())?
}

fn switch_account_inner(
    app: AppHandle,
    state: State<AppState>,
    account_id: String,
) -> Result<SwitchResult, String> {
    let _operation = state
        .switch_operation
        .try_lock()
        .map_err(|_| "另一项切号操作正在进行，请稍候".to_string())?;
    let _account_operation = state.account_operation.lock().map_err(|e| e.to_string())?;
    let (paths, account, config) = {
        let data = state.data.lock().map_err(|e| e.to_string())?;
        let paths = paths_from_config(&data.config)?;
        let account = data
            .accounts
            .iter()
            .find(|a| a.id == account_id)
            .cloned()
            .ok_or_else(|| "账号不存在".to_string())?;
        (paths, account, data.config.clone())
    };

    if !account.has_login_state {
        detach_plugin_overlay(&app);
        close_oopz_if_running()?;
        backup_current(&paths)?;
        clear_registry_login()?;
        Command::new(paths.oopz_exe_path)
            .spawn()
            .map_err(|e| e.to_string())?;
        if let Some(uid) = account.uid.clone() {
            schedule_avatar_refresh(app.clone(), uid);
        }
        ensure_plugin_runtime_after_oopz_start(config);
        return Ok(SwitchResult {
            ok: true,
            message: "已打开 OOPZ 登录页。登录完成后回到 OOPZ+ 点刷新。".to_string(),
        });
    }

    let Some(oopz_login) = read_oopz_login(&account.id) else {
        let mut data = state.data.lock().map_err(|e| e.to_string())?;
        if let Some(pos) = data.accounts.iter().position(|a| a.id == account.id) {
            data.accounts[pos].has_login_state = false;
            data.accounts[pos].updated_at = now();
            save_data(&data)?;
        }
        drop(data);
        update_tray(&app);
        Command::new(paths.oopz_exe_path)
            .spawn()
            .map_err(|e| e.to_string())?;
        if let Some(uid) = account.uid.clone() {
            schedule_avatar_refresh(app.clone(), uid);
        }
        ensure_plugin_runtime_after_oopz_start(config);
        return Ok(SwitchResult {
            ok: false,
            message: "这个账号还不能快速切换。请在 OOPZ 里登录一次，然后回到 OOPZ+ 点刷新。"
                .to_string(),
        });
    };

    let uid = account
        .uid
        .clone()
        .ok_or_else(|| "账号缺少 UID".to_string())?;
    let snapshot = account_snapshot_dir(&account.id)?;
    let roaming_snapshot = snapshot.join("roaming").join(&uid);
    let local_snapshot = snapshot.join("local_sandbox").join(&uid);
    if !roaming_snapshot.exists() && !local_snapshot.exists() {
        return Err("账号数据不完整，请打开 OOPZ 登录一次，然后回到 OOPZ+ 点刷新".to_string());
    }

    detach_plugin_overlay(&app);
    close_oopz_if_running()?;
    backup_current(&paths)?;
    copy_dir_recursive(
        &roaming_snapshot,
        &PathBuf::from(&paths.roaming_data_dir).join(&uid),
    )?;
    copy_dir_recursive(
        &local_snapshot,
        &PathBuf::from(&paths.local_sandbox_dir).join(&uid),
    )?;
    write_registry_login(&oopz_login)?;
    verify_registry_login_uid(&uid)?;
    Command::new(paths.oopz_exe_path)
        .spawn()
        .map_err(|e| e.to_string())?;
    schedule_avatar_refresh(app.clone(), uid.clone());
    ensure_plugin_runtime_after_oopz_start(config);

    let mut data = state.data.lock().map_err(|e| e.to_string())?;
    if let Some(pos) = data.accounts.iter().position(|a| a.id == account.id) {
        data.accounts[pos].last_used_at = Some(now());
        data.accounts[pos].updated_at = now();
    }
    save_data(&data)?;
    drop(data);
    update_tray(&app);
    Ok(SwitchResult {
        ok: true,
        message: format!("已切换到 {} 并启动 OOPZ", account.display_name),
    })
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|arg| arg == "--apply-update") {
        apply_update_helper(&args);
        return;
    }
    if args.iter().any(|arg| arg == "--watcher") {
        watcher_loop();
        return;
    }
    let plugin_runtime = args.iter().any(|arg| arg == "--plugin-runtime");
    let _plugin_runtime_mutex = if plugin_runtime {
        match acquire_plugin_runtime_mutex() {
            Ok(Some(handle)) => Some(handle),
            Ok(None) => return,
            Err(error) => {
                eprintln!("{}", error);
                return;
            }
        }
    } else {
        None
    };
    if !plugin_runtime {
        recover_import_transactions();
    }
    let updater_cleanup = args
        .iter()
        .position(|arg| arg == "--cleanup-updater")
        .and_then(|index| args.get(index + 1))
        .map(PathBuf::from);

    let mut builder = tauri::Builder::default();
    if !plugin_runtime {
        builder = builder.plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            show_main_window(app);
        }));
    }

    builder
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .manage(AppState {
            data: Mutex::new(load_data()),
            account_operation: Mutex::new(()),
            switch_operation: Mutex::new(()),
            discovery_cancelled: AtomicBool::new(false),
            auto_import_running: AtomicBool::new(false),
            plugin_operation: Mutex::new(()),
            plugin_environment_running: AtomicBool::new(false),
            overlay_rebind_requested: AtomicBool::new(false),
            overlay_dragging: AtomicBool::new(false),
            update_running: AtomicBool::new(false),
            update_status: Mutex::new(initial_update_status()),
            wormhole_running: AtomicBool::new(false),
            wormhole_cancelled: AtomicBool::new(false),
            main_webview_low_memory: AtomicBool::new(false),
        })
        .invoke_handler(tauri::generate_handler![
            get_app_data,
            get_steam_workspace,
            discover_steam,
            refresh_steam_accounts,
            set_steam_userdata_scope,
            set_steam_account_note,
            delete_steam_account,
            switch_steam_account,
            get_plugin_status,
            set_plugin_mode,
            repair_plugin_environment,
            plugin_account_action,
            discover_oopz,
            set_oopz_directory,
            validate_configured_paths,
            scan_oopz_accounts,
            import_account,
            export_account_package,
            export_all_accounts_package,
            import_account_package,
            start_quick_export,
            cancel_quick_share,
            quick_import,
            cancel_oopz_discovery,
            delete_account,
            open_oopz,
            switch_account,
            restore_latest_backup,
            drag_overlay,
            reset_overlay_position,
            set_overlay_layout,
            get_update_status,
            check_for_updates
        ])
        .setup(move |app| {
            watch_config_changes(app.handle().clone());
            if plugin_runtime {
                if let Some(window) = app.get_webview_window("main") {
                    window.destroy()?;
                }
                WebviewWindowBuilder::new(
                    app,
                    "plugin-overlay",
                    WebviewUrl::App("index.html?overlay=1".into()),
                )
                .title("NEA OOPZ Plugin")
                .decorations(false)
                .transparent(true)
                .shadow(false)
                .focusable(false)
                .skip_taskbar(true)
                .visible(false)
                .resizable(false)
                .inner_size(300.0, 48.0)
                .build()?;
                sync_overlay_loop(app.handle().clone());
                return Ok(());
            }

            let menu = build_tray_menu(app.handle())?;
            let plugin_enabled = app
                .state::<AppState>()
                .data
                .lock()
                .map_err(|e| e.to_string())?
                .config
                .plugin_mode_enabled;
            if plugin_enabled && !is_watcher_running() {
                let _ = spawn_watcher();
            }
            let tray = TrayIconBuilder::with_id("main-tray")
                .tooltip("NEA")
                .icon(app.default_window_icon().unwrap().clone())
                .menu(&menu)
                .show_menu_on_left_click(false)
                .on_tray_icon_event(|tray, event| {
                    if let TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        ..
                    } = event
                    {
                        show_main_window(tray.app_handle());
                    }
                })
                .on_menu_event(|app, event| {
                    let id = event.id().0.as_str();
                    match id {
                        "show" => show_main_window(app),
                        "import" => {
                            let _ = app.emit("tray-action", "import");
                        }
                        "rediscover" => {
                            let _ = app.emit("tray-action", "rediscover");
                        }
                        "quit" => app.exit(0),
                        _ if id.starts_with("switch:") => {
                            let account_id = id.trim_start_matches("switch:").to_string();
                            let app_handle = app.clone();
                            thread::spawn(move || {
                                let state = app_handle.state::<AppState>();
                                let result =
                                    switch_account_inner(app_handle.clone(), state, account_id);
                                let _ = app_handle
                                    .emit("switch-finished", result.map_err(|e| e.to_string()));
                            });
                        }
                        _ => {}
                    }
                })
                .build(app)?;

            if let Some(window) = app.get_webview_window("main") {
                fit_main_window_to_monitor(&window, true);
                let window_for_close = window.clone();
                window.on_window_event(move |event| {
                    if let WindowEvent::CloseRequested { api, .. } = event {
                        api.prevent_close();
                        set_webview_low_memory(&window_for_close, true);
                        let _ = window_for_close.hide();
                    } else if let WindowEvent::Resized(_) = event {
                        let minimized = window_for_close.is_minimized().unwrap_or(false);
                        set_webview_low_memory(&window_for_close, minimized);
                    } else if let WindowEvent::ScaleFactorChanged { .. } = event {
                        fit_main_window_to_monitor(&window_for_close, false);
                    }
                });
            }

            if let Some(path) = updater_cleanup.clone() {
                schedule_updater_cleanup(path);
            }
            process_update_result(app.handle());
            start_auto_update_checks(app.handle().clone());

            let _tray = tray;
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
fn spawn_watcher() -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let watcher_exe = if exe
        .file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| n.eq_ignore_ascii_case(WATCHER_FILE_NAME))
    {
        exe
    } else {
        watcher_path()?
    };
    Command::new(watcher_exe)
        .arg("--watcher")
        .spawn()
        .map_err(|e| format!("启动守护进程失败: {}", e))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copy_dir_recursive_replaces_complete_directory() {
        let root = std::env::temp_dir().join(format!("oopz-plus-test-{}", Uuid::new_v4()));
        let source = root.join("source");
        let destination = root.join("destination");
        fs::create_dir_all(source.join("nested")).unwrap();
        fs::create_dir_all(&destination).unwrap();
        fs::write(source.join("nested").join("new.txt"), "new").unwrap();
        fs::write(destination.join("old.txt"), "old").unwrap();

        copy_dir_recursive(&source, &destination).unwrap();

        assert_eq!(
            fs::read_to_string(destination.join("nested").join("new.txt")).unwrap(),
            "new"
        );
        assert!(!destination.join("old.txt").exists());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn imported_paths_cannot_escape_snapshot_directory() {
        assert!(safe_relative_path("roaming/user/data.json").is_ok());
        assert!(safe_relative_path("../config.json").is_err());
        assert!(safe_relative_path("C:\\Windows\\system.ini").is_err());
    }

    #[test]
    fn export_packages_support_multi_account_legacy_and_validation() {
        let root = std::env::temp_dir().join(format!("oopz-plus-package-test-{}", Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let path = root.join("accounts.oopz+");
        let entry = |uid: &str| ExportedAccountEntry {
            account: ExportedAccount {
                display_name: format!("account-{}", uid),
                uid: Some(uid.to_string()),
                pid: None,
                user_common_id: None,
                masked_phone: None,
                avatar_url: None,
                note: None,
            },
            oopz_login: format!("login-{}", uid),
            files: vec![ExportedFile {
                path: format!("roaming/{}/state.json", uid),
                data_base64: general_purpose::STANDARD.encode(b"state"),
            }],
        };

        let package = AccountExportPackage {
            format: EXPORT_FORMAT.to_string(),
            exported_at: now(),
            accounts: vec![entry("one"), entry("two")],
        };
        fs::write(&path, serde_json::to_vec(&package).unwrap()).unwrap();
        assert_eq!(read_export_package(&path).unwrap().len(), 2);

        let legacy_entry = entry("legacy");
        let legacy = LegacyAccountExportPackage {
            format: LEGACY_EXPORT_FORMAT.to_string(),
            exported_at: now(),
            account: legacy_entry.account,
            oopz_login: legacy_entry.oopz_login,
            files: legacy_entry.files,
        };
        fs::write(&path, serde_json::to_vec(&legacy).unwrap()).unwrap();
        assert_eq!(read_export_package(&path).unwrap().len(), 1);

        let mut unsafe_entry = entry("unsafe");
        unsafe_entry.files[0].path = "../config.json".to_string();
        let unsafe_package = AccountExportPackage {
            format: EXPORT_FORMAT.to_string(),
            exported_at: now(),
            accounts: vec![unsafe_entry],
        };
        fs::write(&path, serde_json::to_vec(&unsafe_package).unwrap()).unwrap();
        assert!(read_export_package(&path).is_err());

        let duplicate_package = AccountExportPackage {
            format: EXPORT_FORMAT.to_string(),
            exported_at: now(),
            accounts: vec![entry("duplicate"), entry("duplicate")],
        };
        fs::write(&path, serde_json::to_vec(&duplicate_package).unwrap()).unwrap();
        assert!(read_export_package(&path).is_err());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn v3_packages_extract_streamingly_and_reject_path_traversal() {
        let root = std::env::temp_dir().join(format!("oopz-plus-v3-test-{}", Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let manifest = V3ExportManifest {
            format: EXPORT_FORMAT_V3.to_string(),
            app_id: "oopz".to_string(),
            exported_at: now(),
            accounts: vec![V3AccountManifest {
                directory: "account-000".to_string(),
                account: ExportedAccount {
                    display_name: "account".to_string(),
                    uid: Some("uid-1".to_string()),
                    pid: None,
                    user_common_id: None,
                    masked_phone: None,
                    avatar_url: None,
                    note: None,
                },
                oopz_login: "login-state".to_string(),
            }],
        };
        let create_package = |path: &Path, entry_name: &str| {
            let file = fs::File::create(path).unwrap();
            let mut archive = ZipWriter::new(file);
            let options =
                SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
            archive.start_file("manifest.json", options).unwrap();
            serde_json::to_writer(&mut archive, &manifest).unwrap();
            archive.start_file(entry_name, options).unwrap();
            archive.write_all(b"snapshot-data").unwrap();
            archive.finish().unwrap();
        };

        let valid = root.join("valid.oopz+");
        create_package(&valid, "accounts/account-000/roaming/uid-1/state.json");
        let valid_transaction = root.join("valid-transaction");
        fs::create_dir_all(&valid_transaction).unwrap();
        let prepared = prepare_v3_import(&valid_transaction, &AppData::default(), &valid).unwrap();
        assert_eq!(prepared.len(), 1);
        assert_eq!(
            fs::read(prepared[0].staged_snapshot.join("roaming/uid-1/state.json")).unwrap(),
            b"snapshot-data"
        );

        let unsafe_package = root.join("unsafe.oopz+");
        create_package(&unsafe_package, "accounts/account-000/../outside.json");
        let unsafe_transaction = root.join("unsafe-transaction");
        fs::create_dir_all(&unsafe_transaction).unwrap();
        assert!(
            prepare_v3_import(&unsafe_transaction, &AppData::default(), &unsafe_package).is_err()
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn snapshot_rollback_only_reverts_started_entries() {
        let root = std::env::temp_dir().join(format!("oopz-plus-rollback-test-{}", Uuid::new_v4()));
        let target = root.join("accounts/account/snapshot");
        fs::create_dir_all(&target).unwrap();
        fs::write(target.join("original.txt"), b"original").unwrap();
        let pending = ImportJournalEntry {
            account_id: "account".to_string(),
            had_snapshot: true,
            credential_backup_id: "unused".to_string(),
            phase: "pending".to_string(),
        };
        rollback_snapshot_entry(&root, &pending, &target).unwrap();
        assert_eq!(fs::read(target.join("original.txt")).unwrap(), b"original");

        let backup = root.join("backup/account/snapshot");
        fs::create_dir_all(&backup).unwrap();
        fs::write(backup.join("original.txt"), b"original").unwrap();
        fs::write(target.join("new.txt"), b"new").unwrap();
        let replacing = ImportJournalEntry {
            phase: "replacing".to_string(),
            ..pending.clone()
        };
        rollback_snapshot_entry(&root, &replacing, &target).unwrap();
        assert_eq!(fs::read(target.join("original.txt")).unwrap(), b"original");
        assert!(!target.join("new.txt").exists());

        let new_target = root.join("accounts/new-account/snapshot");
        fs::create_dir_all(&new_target).unwrap();
        fs::write(new_target.join("new.txt"), b"new").unwrap();
        let new_entry = ImportJournalEntry {
            account_id: "new-account".to_string(),
            had_snapshot: false,
            credential_backup_id: "unused".to_string(),
            phase: "replacing".to_string(),
        };
        rollback_snapshot_entry(&root, &new_entry, &new_target).unwrap();
        assert!(!new_target.exists());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    #[ignore = "requires the public Magic Wormhole services"]
    fn magic_wormhole_public_roundtrip() {
        futures::executor::block_on(async {
            let root =
                std::env::temp_dir().join(format!("oopz-plus-wormhole-test-{}", Uuid::new_v4()));
            fs::create_dir_all(&root).unwrap();
            let source = root.join("source.oopz+");
            let target = root.join("target.oopz+");
            fs::write(&source, b"wormhole-roundtrip").unwrap();

            let mailbox = MailboxConnection::create(transfer::APP_CONFIG, WORMHOLE_CODE_WORDS)
                .await
                .unwrap();
            let code = mailbox.code().clone();
            let sender = async {
                let wormhole = Wormhole::connect(mailbox).await.unwrap();
                let offer = transfer::offer::OfferSend::new_file_or_folder(
                    "roundtrip.oopz+".to_string(),
                    &source,
                )
                .await
                .unwrap();
                transfer::send(
                    wormhole,
                    wormhole_relay_hints().unwrap(),
                    transit::Abilities::ALL,
                    offer,
                    |_| {},
                    |_, _| {},
                    pending(),
                )
                .await
                .unwrap();
            };
            let receiver = async {
                let mailbox = MailboxConnection::connect(transfer::APP_CONFIG, code, false)
                    .await
                    .unwrap();
                let wormhole = Wormhole::connect(mailbox).await.unwrap();
                let request = transfer::request_file(
                    wormhole,
                    wormhole_relay_hints().unwrap(),
                    transit::Abilities::ALL,
                    pending(),
                )
                .await
                .unwrap()
                .unwrap();
                let mut output = async_fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&target)
                    .await
                    .unwrap();
                request
                    .accept(|_| {}, |_, _| {}, &mut output, pending())
                    .await
                    .unwrap();
                output.flush().await.unwrap();
            };
            futures::join!(sender, receiver);

            assert_eq!(fs::read(&target).unwrap(), b"wormhole-roundtrip");
            let _ = fs::remove_dir_all(root);
        });
    }

    #[test]
    fn avatar_format_is_verified_from_file_signature() {
        assert_eq!(
            avatar_mime(b"\x89PNG\r\n\x1a\nrest", None),
            Some("image/png")
        );
        assert_eq!(avatar_mime(b"not-an-image", Some("text/plain")), None);
    }

    #[test]
    fn release_versions_are_compared_numerically() {
        assert_eq!(
            parse_release_version("v1.10.2"),
            Some(([1, 10, 2], "1.10.2".to_string()))
        );
        assert!(parse_release_version("1.2").is_none());
        assert!(parse_release_version("1.2.3-beta").is_none());

        let recent = AppConfig {
            last_update_check_at: Some(Utc::now().to_rfc3339()),
            ..AppConfig::default()
        };
        assert!(!update_check_due(&recent));
        let expired = AppConfig {
            last_update_check_at: Some((Utc::now() - chrono::Duration::minutes(31)).to_rfc3339()),
            ..AppConfig::default()
        };
        assert!(update_check_due(&expired));
    }

    #[test]
    fn update_download_percent_is_bounded() {
        assert_eq!(download_percent(0, 100), 0);
        assert_eq!(download_percent(50, 100), 50);
        assert_eq!(download_percent(150, 100), 100);
        assert_eq!(download_percent(50, 0), 0);
        assert_eq!(download_percent(u64::MAX, u64::MAX), 100);
    }

    #[test]
    fn update_assets_require_expected_origin_name_and_digest() {
        let digest = "a".repeat(64);
        let asset = GitHubAsset {
            name: "OOPZ+_1.2.3_x64_en-US.msi".to_string(),
            browser_download_url:
                "https://github.com/M4rkzzz/oopz-plus/releases/download/v1.2.3/OOPZ%2B_1.2.3_x64_en-US.msi"
                    .to_string(),
            size: 1024,
            digest: Some(format!("sha256:{}", digest)),
        };
        assert_eq!(validate_update_asset(&asset, "1.2.3"), Ok(digest.as_str()));
        assert_eq!(
            github_proxy_url(&asset),
            "https://gh-proxy.com/https://github.com/M4rkzzz/oopz-plus/releases/download/v1.2.3/OOPZ%2B_1.2.3_x64_en-US.msi"
        );

        let untrusted = GitHubAsset {
            browser_download_url: "https://example.com/update.msi".to_string(),
            ..asset
        };
        assert!(validate_update_asset(&untrusted, "1.2.3").is_err());
    }

    #[test]
    fn overlay_geometry_applies_saved_relative_offset() {
        let rect = RECT {
            left: 100,
            top: 200,
            right: 1300,
            bottom: 900,
        };
        let config = AppConfig {
            overlay_offset_x: 12,
            overlay_offset_y: -8,
            ..AppConfig::default()
        };
        assert_eq!(overlay_geometry(rect, &config, 6, 1.0), (832, 207, 252, 52));
        let compact_rect = RECT {
            left: 50,
            top: 75,
            right: 850,
            bottom: 700,
        };
        assert_eq!(
            overlay_geometry(compact_rect, &config, 6, 1.0),
            (132, 342, 252, 52)
        );
        assert_eq!(
            overlay_offset_for_position(
                compact_rect,
                &config,
                6,
                PhysicalPosition::new(210, 390),
                1.0,
            ),
            (90, 40)
        );
        let vertical = AppConfig {
            overlay_vertical: true,
            ..config.clone()
        };
        assert_eq!(
            overlay_geometry(compact_rect, &vertical, 6, 1.0),
            (132, 342, 54, 252)
        );
        assert_eq!(overlay_dimensions(0, false), (52, 52));

        let scaled_rect = RECT {
            left: 75,
            top: 113,
            right: 1275,
            bottom: 1050,
        };
        assert_eq!(
            overlay_geometry(scaled_rect, &config, 6, 1.5),
            (198, 514, 252, 52)
        );
        assert_eq!(
            overlay_offset_for_position(
                scaled_rect,
                &config,
                6,
                PhysicalPosition::new(315, 586),
                1.5,
            ),
            (90, 40)
        );
    }
}
