use base64::{engine::general_purpose, Engine};
use chrono::Utc;
use keyring::Entry;
use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{ErrorKind, Read, Write},
    path::{Component, Path, PathBuf},
    process::Command,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Mutex,
    },
    thread,
    time::{Duration, Instant},
};
use sysinfo::{Pid, Signal, System};
use tauri::{
    menu::{Menu, MenuItem, PredefinedMenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    AppHandle, Emitter, LogicalSize, Manager, PhysicalPosition, State, WebviewUrl, WebviewWindow,
    WebviewWindowBuilder, WindowEvent,
};
use uuid::Uuid;
use windows::Win32::Foundation::{BOOL, HWND, LPARAM, RECT};
use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_LBUTTON};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetAncestor, GetWindowRect, GetWindowThreadProcessId, IsIconic, IsWindow,
    IsWindowVisible, SetWindowLongPtrW, GA_ROOTOWNER, GWLP_HWNDPARENT,
};
use winreg::{enums::HKEY_CURRENT_USER, RegKey};

const APP_DIR_NAME: &str = "OOPZ+";
const CREDENTIAL_SERVICE: &str = "OOPZ+";
const WATCHER_FILE_NAME: &str = "oopz-plus-watcher.exe";
const RUN_KEY_PATH: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Run";
const RUN_KEY_NAME: &str = "OOPZ+ Watcher";
const EXPORT_FORMAT: &str = "oopz-plus-account-v1";
const MAX_AVATAR_BYTES: u64 = 2 * 1024 * 1024;
const GITHUB_LATEST_RELEASE_URL: &str =
    "https://api.github.com/repos/M4rkzzz/oopz-plus/releases/latest";
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
    config: AppConfig,
    accounts: Vec<SavedAccount>,
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

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CredentialInput {
    account_id: Option<String>,
    display_name: String,
    login_name: String,
    password: String,
    note: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CredentialView {
    login_name: Option<String>,
    password: Option<String>,
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
    switch_operation: Mutex<()>,
    discovery_cancelled: AtomicBool,
    auto_import_running: AtomicBool,
    plugin_operation: Mutex<()>,
    plugin_environment_running: AtomicBool,
    overlay_rebind_requested: AtomicBool,
    overlay_dragging: AtomicBool,
    update_running: AtomicBool,
    update_status: Mutex<UpdateStatus>,
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
    Ok(base.join(APP_DIR_NAME))
}

fn config_path() -> Result<PathBuf, String> {
    Ok(storage_dir()?.join("config.json"))
}

fn accounts_dir() -> Result<PathBuf, String> {
    Ok(storage_dir()?.join("accounts"))
}

fn backups_dir() -> Result<PathBuf, String> {
    Ok(storage_dir()?.join("backups"))
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
    };
    if let Ok(mut current) = app.state::<AppState>().update_status.lock() {
        *current = status.clone();
    }
    let _ = app.emit("update-status", status.clone());
    status
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
    let expected_name = format!("OOPZ+_{}_x64_en-US.msi", version);
    if !asset.name.eq_ignore_ascii_case(&expected_name) {
        return Err(format!("Release 缺少安装包 {}", expected_name));
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

fn download_update_asset(asset: &GitHubAsset, version: &str) -> Result<PathBuf, String> {
    let expected_name = format!("OOPZ+_{}_x64_en-US.msi", version);
    let expected_digest = validate_update_asset(asset, version)?;

    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(600))
        .build()
        .map_err(|e| e.to_string())?;
    let mut response = client
        .get(&asset.browser_download_url)
        .header(reqwest::header::USER_AGENT, "OOPZ-Plus-Updater")
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
    Ok(target)
}

fn preferred_installed_exe(original_exe: &Path) -> PathBuf {
    let program_files_exe = std::env::var_os("ProgramFiles")
        .map(PathBuf::from)
        .map(|path| path.join("OOPZ+").join("oopz-plus.exe"));
    if original_exe
        .to_string_lossy()
        .to_ascii_lowercase()
        .contains("program files")
        && original_exe.exists()
    {
        original_exe.to_path_buf()
    } else if program_files_exe.as_ref().is_some_and(|path| path.exists()) {
        program_files_exe.unwrap()
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
        let mut system = System::new();
        system.refresh_processes();
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

fn perform_update_check(app: &AppHandle, manual: bool) -> Result<(), String> {
    if !manual {
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
        .header(reqwest::header::USER_AGENT, "OOPZ-Plus-Updater")
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
    let expected_name = format!("OOPZ+_{}_x64_en-US.msi", version);
    let asset = release
        .assets
        .iter()
        .find(|asset| asset.name.eq_ignore_ascii_case(&expected_name))
        .ok_or_else(|| format!("Release 缺少安装包 {}", expected_name))?;
    set_update_status(
        app,
        "downloading",
        Some(version.clone()),
        format!("发现新版本 {}，正在下载...", version),
    );
    let msi_path = download_update_asset(asset, &version)?;
    set_update_status(
        app,
        "installing",
        Some(version.clone()),
        format!("正在安装 {}，程序即将重启...", version),
    );
    launch_update_installer(app, &msi_path, &version)
}

fn schedule_update_check(app: AppHandle, manual: bool) {
    let state = app.state::<AppState>();
    if state.update_running.swap(true, Ordering::SeqCst) {
        return;
    }
    thread::spawn(move || {
        if !manual {
            thread::sleep(Duration::from_secs(3));
        }
        if let Err(error) = perform_update_check(&app, manual) {
            set_update_status(&app, "error", None, error);
        }
        app.state::<AppState>()
            .update_running
            .store(false, Ordering::SeqCst);
    });
}

fn start_auto_update_checks(app: AppHandle) {
    schedule_update_check(app.clone(), false);
    thread::spawn(move || loop {
        thread::sleep(Duration::from_secs(
            UPDATE_CHECK_INTERVAL_MINUTES as u64 * 60,
        ));
        schedule_update_check(app.clone(), false);
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
    schedule_update_check(app.clone(), true);
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

fn running_oopz_exe() -> Option<PathBuf> {
    let system = System::new_all();
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

fn is_plugin_runtime_running() -> bool {
    let system = System::new_all();
    system.processes().values().any(|process| {
        (process.name().eq_ignore_ascii_case("oopz-plus.exe")
            || process.name().eq_ignore_ascii_case(WATCHER_FILE_NAME))
            && process.cmd().iter().any(|arg| arg == "--plugin-runtime")
    })
}

fn is_watcher_running() -> bool {
    let system = System::new_all();
    system.processes().values().any(|process| {
        (process.name().eq_ignore_ascii_case("oopz-plus.exe")
            || process.name().eq_ignore_ascii_case(WATCHER_FILE_NAME))
            && process.cmd().iter().any(|arg| arg == "--watcher")
    })
}

fn stop_watcher() {
    let system = System::new_all();
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
    let system = System::new_all();
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
    loop {
        let data = load_data();
        if !data.config.plugin_mode_enabled {
            thread::sleep(Duration::from_secs(3));
            continue;
        }
        if running_oopz_exe().is_some() && !is_plugin_runtime_running() {
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

fn oopz_process_ids() -> Vec<u32> {
    let system = System::new_all();
    system
        .processes()
        .iter()
        .filter_map(|(pid, process)| is_oopz_process_name(process.name()).then_some(pid.as_u32()))
        .collect()
}

fn oopz_window_info() -> Option<(HWND, RECT)> {
    let pids = oopz_process_ids();
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

fn overlay_geometry(rect: RECT, config: &AppConfig, account_count: usize) -> (i32, i32, u32, u32) {
    let width = rect.right - rect.left;
    let (overlay_width, overlay_height) =
        overlay_dimensions(account_count, config.overlay_vertical);
    if width < 1000 {
        (
            rect.left + 70 + config.overlay_offset_x,
            rect.top + 275 + config.overlay_offset_y,
            overlay_width,
            overlay_height,
        )
    } else {
        (
            rect.left + 720 + config.overlay_offset_x,
            rect.top + 15 + config.overlay_offset_y,
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
) -> (i32, i32) {
    let (base_x, base_y, _, _) = overlay_geometry(
        rect,
        &AppConfig {
            overlay_offset_x: 0,
            overlay_offset_y: 0,
            ..config.clone()
        },
        account_count,
    );
    (
        (position.x - base_x).clamp(-4000, 4000),
        (position.y - base_y).clamp(-4000, 4000),
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
    let (_, rect) = oopz_window_info().ok_or_else(|| "未找到 OOPZ 窗口".to_string())?;
    let window = app
        .get_webview_window("plugin-overlay")
        .ok_or_else(|| "未找到插件浮层".to_string())?;
    let position = window.outer_position().map_err(|e| e.to_string())?;
    let state = app.state::<AppState>();
    let mut data = state.data.lock().map_err(|e| e.to_string())?;
    let (offset_x, offset_y) =
        overlay_offset_for_position(rect, &data.config, data.accounts.len(), position);
    data.config.overlay_offset_x = offset_x;
    data.config.overlay_offset_y = offset_y;
    save_data(&data)
}

#[tauri::command]
fn reset_overlay_position(app: AppHandle, state: State<AppState>) -> Result<(), String> {
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
fn set_overlay_layout(
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
        let mut attached_owner: Option<isize> = None;
        let mut last_geometry: Option<(i32, i32, u32, u32)> = None;
        let mut overlay_visible = false;
        let mut owner_missing_since: Option<Instant> = None;
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
                attached_owner = None;
                last_geometry = None;
                overlay_visible = false;
                owner_missing_since = None;
                last_window_search = Instant::now() - Duration::from_secs(2);
                detach_overlay_window(window);
            }

            let mut current =
                owner.and_then(|hwnd| visible_window_rect(hwnd).map(|rect| (hwnd, rect)));
            if current.is_none() {
                owner_missing_since.get_or_insert_with(Instant::now);
            }

            if current.is_none() && last_window_search.elapsed() >= Duration::from_secs(1) {
                last_window_search = Instant::now();
                current = oopz_window_info();
            }

            if let Some((next_owner, rect)) = current {
                owner_missing_since = None;
                owner = Some(next_owner);
                if attached_owner != Some(next_owner.0) {
                    if let Ok(handle) = window.hwnd() {
                        unsafe {
                            let _ = SetWindowLongPtrW(
                                HWND(handle.0 as isize),
                                GWLP_HWNDPARENT,
                                next_owner.0,
                            );
                        }
                    }
                    attached_owner = Some(next_owner.0);
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
                let (config, account_count) = app
                    .state::<AppState>()
                    .data
                    .lock()
                    .map(|data| (data.config.clone(), data.accounts.len()))
                    .unwrap_or_default();
                let geometry = overlay_geometry(rect, &config, account_count);
                let (x, y, w, h) = geometry;
                if last_geometry.map(|value| (value.0, value.1)) != Some((x, y)) {
                    let _ = window.set_position(PhysicalPosition::new(x, y));
                }
                if last_geometry.map(|value| (value.2, value.3)) != Some((w, h)) {
                    let _ = window.set_size(LogicalSize::new(w as f64, h as f64));
                }
                if !overlay_visible {
                    let _ = window.show();
                    overlay_visible = true;
                }
                last_geometry = Some(geometry);
                thread::sleep(Duration::from_millis(33));
            } else {
                if owner_missing_since
                    .is_some_and(|started| started.elapsed() < Duration::from_secs(2))
                {
                    thread::sleep(Duration::from_millis(100));
                    continue;
                }
                owner = None;
                last_geometry = None;
                if overlay_visible {
                    let _ = window.hide();
                    overlay_visible = false;
                }
                if attached_owner.is_some() {
                    detach_overlay_window(window);
                    attached_owner = None;
                }
                thread::sleep(Duration::from_millis(500));
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

fn store_credential(account_id: &str, login_name: &str, password: &str) -> Result<(), String> {
    let mut payload = read_secret(account_id);
    payload.login_name = Some(login_name.to_string());
    payload.password = Some(password.to_string());
    write_secret(account_id, &payload)
}

fn store_oopz_login(account_id: &str, login: &str) -> Result<(), String> {
    let mut payload = read_secret(account_id);
    payload.oopz_login = Some(login.to_string());
    write_secret(account_id, &payload)
}

fn read_oopz_login(account_id: &str) -> Option<String> {
    read_secret(account_id).oopz_login
}

fn read_credential(account_id: &str) -> Result<CredentialView, String> {
    let value = read_secret(account_id);
    Ok(CredentialView {
        login_name: value.login_name,
        password: value.password,
    })
}

fn delete_credential(account_id: &str) {
    if let Ok(entry) = credential_entry(account_id) {
        let _ = entry.delete_credential();
    }
}

fn account_snapshot_dir(account_id: &str) -> Result<PathBuf, String> {
    Ok(accounts_dir()?.join(account_id).join("snapshot"))
}

fn collect_export_files(
    root: &Path,
    current: &Path,
    files: &mut Vec<ExportedFile>,
) -> Result<(), String> {
    if !current.exists() {
        return Ok(());
    }
    for entry in fs::read_dir(current).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let path = entry.path();
        let file_type = entry.file_type().map_err(|e| e.to_string())?;
        if file_type.is_dir() {
            collect_export_files(root, &path, files)?;
        } else if file_type.is_file() {
            let relative = path
                .strip_prefix(root)
                .map_err(|e| e.to_string())?
                .to_string_lossy()
                .replace('\\', "/");
            let bytes = fs::read(&path).map_err(|e| e.to_string())?;
            files.push(ExportedFile {
                path: relative,
                data_base64: general_purpose::STANDARD.encode(bytes),
            });
        }
    }
    Ok(())
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
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}

fn confirm_main_window_focus_after_click(window: WebviewWindow) {
    if unsafe { GetAsyncKeyState(VK_LBUTTON.0 as i32) >= 0 } {
        return;
    }
    thread::spawn(move || {
        for _ in 0..40 {
            if unsafe { GetAsyncKeyState(VK_LBUTTON.0 as i32) >= 0 } {
                thread::sleep(Duration::from_millis(50));
                let _ = window.set_focus();
                return;
            }
            thread::sleep(Duration::from_millis(25));
        }
    });
}

fn build_tray_menu(app: &AppHandle) -> tauri::Result<Menu<tauri::Wry>> {
    let state = app.state::<AppState>();
    let data = state.data.lock().expect("state poisoned");
    let current_uid = current_registry_login().and_then(|login| uid_from_registry_login(&login));
    let menu = Menu::new(app)?;
    menu.append(&MenuItem::with_id(
        app,
        "show",
        "打开 OOPZ+",
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

#[tauri::command]
fn get_app_data(app: AppHandle) -> Result<AppData, String> {
    let mut data = refresh_app_data_from_disk(&app);
    schedule_auto_import_current_login(app.clone());
    data.current_login_uid =
        current_registry_login().and_then(|login| uid_from_registry_login(&login));
    Ok(data)
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
    let should_import = {
        let data = state.data.lock().map_err(|e| e.to_string())?;
        let current_avatar_url = paths_from_config(&data.config)
            .ok()
            .and_then(|paths| read_user_detail(&PathBuf::from(paths.roaming_data_dir).join(&uid)))
            .and_then(|candidate| candidate.avatar_url)
            .filter(|url| !url.trim().is_empty());
        match data
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
        }
    };
    if should_import {
        let _ = import_account_inner(app, state, uid);
    }
    Ok(())
}

fn plugin_status_inner(state: &AppState) -> Result<PluginStatus, String> {
    let data = state.data.lock().map_err(|e| e.to_string())?;
    Ok(PluginStatus {
        plugin_mode_enabled: data.config.plugin_mode_enabled,
        watcher_installed: watcher_installed(),
        watcher_running: is_watcher_running(),
        plugin_runtime_running: is_plugin_runtime_running(),
        oopz_running: running_oopz_exe().is_some(),
        overlay_visible: oopz_window_info().is_some() && is_plugin_runtime_running(),
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
fn plugin_account_action(app: AppHandle, account_id: String) -> Result<SwitchResult, String> {
    let app_for_state = app.clone();
    let state = app_for_state.state::<AppState>();
    switch_account_inner(app, state, account_id)
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
    let mut data = state.data.lock().map_err(|e| e.to_string())?;
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
    drop(data);
    update_tray(&app);
    Ok(account)
}

#[tauri::command]
fn export_account_package(
    state: State<AppState>,
    account_id: String,
    path: String,
) -> Result<(), String> {
    let account = {
        let data = state.data.lock().map_err(|e| e.to_string())?;
        data.accounts
            .iter()
            .find(|account| account.id == account_id)
            .cloned()
            .ok_or_else(|| "账号不存在".to_string())?
    };
    let oopz_login = read_oopz_login(&account.id)
        .ok_or_else(|| "这个账号还不能导出，请先登录一次".to_string())?;
    let snapshot = account_snapshot_dir(&account.id)?;
    if !saved_snapshot_exists(&account) {
        return Err("这个账号还没有可导出的本地数据".to_string());
    }
    let mut files = Vec::new();
    collect_export_files(&snapshot, &snapshot, &mut files)?;
    if files.is_empty() {
        return Err("这个账号还没有可导出的本地数据".to_string());
    }
    let package = AccountExportPackage {
        format: EXPORT_FORMAT.to_string(),
        exported_at: now(),
        account: ExportedAccount {
            display_name: account.display_name,
            uid: account.uid,
            pid: account.pid,
            user_common_id: account.user_common_id,
            masked_phone: account.masked_phone,
            avatar_url: account.avatar_url,
            note: account.note,
        },
        oopz_login,
        files,
    };
    let raw = serde_json::to_string_pretty(&package).map_err(|e| e.to_string())?;
    fs::write(path, raw).map_err(|e| format!("导出失败: {}", e))
}

#[tauri::command]
fn import_account_package(app: AppHandle, path: String) -> Result<SavedAccount, String> {
    let raw = fs::read_to_string(path).map_err(|e| format!("读取导入文件失败: {}", e))?;
    let package: AccountExportPackage =
        serde_json::from_str(&raw).map_err(|e| format!("导入文件格式不正确: {}", e))?;
    if package.format != EXPORT_FORMAT {
        return Err("不支持的导入文件".to_string());
    }
    if package.oopz_login.is_empty() || package.files.is_empty() {
        return Err("导入文件缺少账号数据".to_string());
    }
    let app_for_state = app.clone();
    let state = app_for_state.state::<AppState>();
    let mut data = state.data.lock().map_err(|e| e.to_string())?;
    let existing_id = package.account.uid.as_ref().and_then(|uid| {
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
    let snapshot = account_snapshot_dir(&id)?;
    write_export_files(&snapshot, &package.files)?;
    store_oopz_login(&id, &package.oopz_login)?;
    let timestamp = now();
    let account = SavedAccount {
        id: id.clone(),
        display_name: package.account.display_name,
        uid: package.account.uid,
        pid: package.account.pid,
        user_common_id: package.account.user_common_id,
        masked_phone: package.account.masked_phone,
        avatar_url: package.account.avatar_url,
        avatar_source_url: existing_account
            .as_ref()
            .and_then(|account| account.avatar_source_url.clone()),
        login_name: existing_account
            .as_ref()
            .and_then(|account| account.login_name.clone()),
        note: package.account.note.or_else(|| {
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
    };
    if let Some(pos) = data.accounts.iter().position(|account| account.id == id) {
        data.accounts[pos] = account.clone();
    } else {
        data.accounts.push(account.clone());
    }
    let config = data.config.clone();
    save_data(&data)?;
    drop(data);
    update_tray(&app);
    ensure_plugin_runtime_for_oopz(&config);
    Ok(account)
}

#[tauri::command]
async fn save_manual_credential(
    app: AppHandle,
    input: CredentialInput,
) -> Result<SavedAccount, String> {
    let app_for_task = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let state = app_for_task.state::<AppState>();
        save_manual_credential_inner(app_for_task.clone(), state, input)
    })
    .await
    .map_err(|e| e.to_string())?
}

fn save_manual_credential_inner(
    app: AppHandle,
    state: State<AppState>,
    mut input: CredentialInput,
) -> Result<SavedAccount, String> {
    let mut data = state.data.lock().map_err(|e| e.to_string())?;
    input.display_name = input.display_name.trim().to_string();
    input.login_name = input.login_name.trim().to_string();
    input.note = input.note.and_then(|note| {
        let trimmed = note.trim().to_string();
        (!trimmed.is_empty()).then_some(trimmed)
    });
    if input.display_name.is_empty() || input.login_name.is_empty() || input.password.is_empty() {
        return Err("名称、账号和密码不能为空".to_string());
    }
    if let Some(account_id) = input.account_id.as_deref() {
        if !data.accounts.iter().any(|account| account.id == account_id) {
            return Err("要更新的账号不存在，请刷新后重试".to_string());
        }
    }
    let timestamp = now();
    let id = input
        .account_id
        .unwrap_or_else(|| Uuid::new_v4().to_string());
    store_credential(&id, &input.login_name, &input.password)?;

    let mut account = data
        .accounts
        .iter()
        .find(|a| a.id == id)
        .cloned()
        .unwrap_or_else(|| SavedAccount {
            id: id.clone(),
            display_name: input.display_name.clone(),
            uid: None,
            pid: None,
            user_common_id: None,
            masked_phone: None,
            avatar_url: None,
            avatar_source_url: None,
            login_name: None,
            note: None,
            has_session_snapshot: false,
            has_credential: false,
            has_login_state: false,
            created_at: timestamp.clone(),
            updated_at: timestamp.clone(),
            last_used_at: None,
        });

    account.display_name = input.display_name;
    account.login_name = Some(input.login_name);
    account.note = input.note;
    account.has_credential = true;
    account.updated_at = timestamp;

    if let Some(pos) = data.accounts.iter().position(|a| a.id == id) {
        data.accounts[pos] = account.clone();
    } else {
        data.accounts.push(account.clone());
    }
    save_data(&data)?;
    drop(data);
    update_tray(&app);
    Ok(account)
}

#[tauri::command]
async fn reveal_credential(account_id: String) -> Result<CredentialView, String> {
    tauri::async_runtime::spawn_blocking(move || read_credential(&account_id))
        .await
        .map_err(|e| e.to_string())?
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
    let mut data = state.data.lock().map_err(|e| e.to_string())?;
    data.accounts.retain(|a| a.id != account_id);
    save_data(&data)?;
    drop(data);
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
    let data = state.data.lock().map_err(|e| e.to_string())?;
    let paths = paths_from_config(&data.config)?;
    Command::new(paths.oopz_exe_path)
        .spawn()
        .map_err(|e| format!("启动 OOPZ 失败: {}", e))?;
    Ok(())
}

fn close_oopz_if_running() -> Result<(), String> {
    let mut system = System::new_all();
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
    system.refresh_processes();

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
            switch_operation: Mutex::new(()),
            discovery_cancelled: AtomicBool::new(false),
            auto_import_running: AtomicBool::new(false),
            plugin_operation: Mutex::new(()),
            plugin_environment_running: AtomicBool::new(false),
            overlay_rebind_requested: AtomicBool::new(false),
            overlay_dragging: AtomicBool::new(false),
            update_running: AtomicBool::new(false),
            update_status: Mutex::new(initial_update_status()),
        })
        .invoke_handler(tauri::generate_handler![
            get_app_data,
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
            import_account_package,
            save_manual_credential,
            cancel_oopz_discovery,
            reveal_credential,
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
                    let _ = window.hide();
                }
                WebviewWindowBuilder::new(
                    app,
                    "plugin-overlay",
                    WebviewUrl::App("index.html?overlay=1".into()),
                )
                .title("OOPZ+ Plugin")
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
                .tooltip("OOPZ+")
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
                let window_for_close = window.clone();
                window.on_window_event(move |event| {
                    if let WindowEvent::CloseRequested { api, .. } = event {
                        api.prevent_close();
                        let _ = window_for_close.hide();
                    } else if let WindowEvent::Focused(true) = event {
                        confirm_main_window_focus_after_click(window_for_close.clone());
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
        assert_eq!(overlay_geometry(rect, &config, 6), (832, 207, 252, 52));
        let compact_rect = RECT {
            left: 50,
            top: 75,
            right: 850,
            bottom: 700,
        };
        assert_eq!(
            overlay_geometry(compact_rect, &config, 6),
            (132, 342, 252, 52)
        );
        assert_eq!(
            overlay_offset_for_position(compact_rect, &config, 6, PhysicalPosition::new(210, 390),),
            (90, 40)
        );
        let vertical = AppConfig {
            overlay_vertical: true,
            ..config
        };
        assert_eq!(
            overlay_geometry(compact_rect, &vertical, 6),
            (132, 342, 54, 252)
        );
        assert_eq!(overlay_dimensions(0, false), (52, 52));
    }
}
