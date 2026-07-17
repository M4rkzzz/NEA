use base64::{engine::general_purpose, Engine};
use chrono::Utc;
use futures::{future::pending, stream, AsyncWriteExt, StreamExt, TryStreamExt};
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
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
        mpsc, Arc, Mutex,
    },
    thread,
    time::{Duration, Instant},
};
use sysinfo::{Pid, ProcessRefreshKind, Signal, System, UpdateKind};
use tauri::{
    menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem, Submenu},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    webview::{Cookie, PageLoadEvent},
    AppHandle, Emitter, LogicalPosition, LogicalSize, Manager, PhysicalPosition, State, WebviewUrl,
    WebviewWindow, WebviewWindowBuilder, WindowEvent,
};
use uuid::Uuid;
use webview2_com::Microsoft::Web::WebView2::Win32::{
    ICoreWebView2_19, COREWEBVIEW2_MEMORY_USAGE_TARGET_LEVEL_LOW,
    COREWEBVIEW2_MEMORY_USAGE_TARGET_LEVEL_NORMAL,
};
use windows::core::{w, PCWSTR};
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
mod perfect_arena;
mod steam;
use adapters::AppAdapter;

const APP_DIR_NAME: &str = "NEA";
const LEGACY_APP_DIR_NAME: &str = "OOPZ+";
const CURRENT_SCHEMA_VERSION: u32 = 4;
const CREDENTIAL_SERVICE: &str = "NEA";
const LEGACY_CREDENTIAL_SERVICE: &str = "OOPZ+";
const APP_EXECUTABLE_NAME: &str = "nea.exe";
const LEGACY_APP_EXECUTABLE_NAME: &str = "oopz-plus.exe";
const WATCHER_FILE_NAME: &str = "nea-watcher.exe";
const LEGACY_WATCHER_FILE_NAME: &str = "oopz-plus-watcher.exe";
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
const RUN_KEY_NAME: &str = "NEA Watcher";
const LEGACY_RUN_KEY_NAME: &str = "OOPZ+ Watcher";
const LEGACY_EXPORT_FORMAT_V1: &str = "oopz-plus-account-v1";
const LEGACY_EXPORT_FORMAT_V2: &str = "oopz-plus-package-v2";
const LEGACY_EXPORT_FORMAT_V3: &str = "oopz-plus-package-v3";
const NEA_EXPORT_FORMAT_V1: &str = "nea-package-v1";
const MAX_EXPORT_PACKAGE_BYTES: u64 = 512 * 1024 * 1024;
const MAX_V3_ARCHIVE_BYTES: u64 = 528 * 1024 * 1024;
const MAX_NEA_SHARE_ARCHIVE_BYTES: u64 = 2 * 1024 * 1024 * 1024;
const MAX_NEA_SHARE_CONTENT_BYTES: u64 = 4 * 1024 * 1024 * 1024;
const MAX_LEGACY_EXPORT_PACKAGE_BYTES: u64 = 128 * 1024 * 1024;
const MAX_EXPORT_ACCOUNTS: usize = 100;
const MAX_EXPORT_FILES: usize = 100_000;
const WORMHOLE_TIMEOUT_SECONDS: u64 = 10 * 60;
const WORMHOLE_CODE_WORDS: usize = 4;
const NEA_FREE_TRANSIT_RELAY: &str = "tcp://relay.mw.leastauthority.com:4001";
const QUICK_SHARE_CANCELLED: &str = "快捷分享已取消";
const MAX_AVATAR_BYTES: u64 = 2 * 1024 * 1024;
const PERFECT_AVATAR_CACHE_MARKER: &str = "nea-cache://perfect-avatar";
const GITHUB_LATEST_RELEASE_URL: &str = "https://api.github.com/repos/M4rkzzz/NEA/releases/latest";
const GITHUB_DOWNLOAD_PROXY_PREFIX: &str = "https://gh-proxy.com/";
const MAX_UPDATE_BYTES: u64 = 150 * 1024 * 1024;
const UPDATE_CHECK_INTERVAL_MINUTES: i64 = 30;
static CONFIG_WRITES_BLOCKED: AtomicBool = AtomicBool::new(false);
static CONFIG_WRITE_LOCK: Mutex<()> = Mutex::new(());
static LAST_INTERNAL_CONFIG_BYTES: Mutex<Option<Vec<u8>>> = Mutex::new(None);

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
    #[serde(default)]
    steam_credentials: Vec<SteamSavedCredential>,
    #[serde(default)]
    steam_identities: Vec<SteamIdentity>,
    #[serde(default)]
    steam_native_switcher_exclusions: HashSet<String>,
    #[serde(default)]
    perfect_profiles: HashMap<String, perfect_arena::PerfectArenaProfile>,
    #[serde(default)]
    perfect_unavailable_account_ids: HashSet<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    current_login_uid: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SteamSavedCredential {
    account_name: String,
    password: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    steam_id: Option<String>,
    updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct SteamIdentityCapabilities {
    web_login: bool,
    credential: bool,
    perfect_profile: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SteamIdentity {
    id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    steam_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    account_name: Option<String>,
    display_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    note: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    client_account_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    web_session_id: Option<String>,
    capabilities: SteamIdentityCapabilities,
    created_at: String,
    updated_at: String,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    package_bytes: Option<u64>,
}

const NEA_SHARE_FORMAT_V1: &str = "nea-wormhole-share-v1";
const NEA_SHARE_FORMAT_V2: &str = "nea-wormhole-share-v2";
const MAX_SHARED_WEB_SESSIONS: usize = 100;
const MAX_SHARED_COOKIES_PER_SESSION: usize = 256;
const MAX_SHARED_COOKIE_BYTES: usize = 16 * 1024;
const MAX_SHARED_STEAM_ACCOUNT_NAME_BYTES: usize = 128;
const MAX_SHARED_STEAM_PASSWORD_BYTES: usize = 512;
const MAX_SHARE_MANIFEST_BYTES: u64 = 16 * 1024 * 1024;
const STEAM_ID64_INDIVIDUAL_BASE: u64 = 76_561_197_960_265_728;

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct QuickShareSelection {
    #[serde(default)]
    oopz_account_ids: Vec<String>,
    #[serde(default)]
    steam_accounts: Vec<QuickSteamShareSelection>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct QuickSteamShareSelection {
    steam_id: String,
    #[serde(default)]
    web_login: bool,
    #[serde(default)]
    credential: bool,
    #[serde(default)]
    perfect: bool,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SharedSteamCredential {
    account_name: String,
    password: String,
    steam_id: String,
}

impl std::fmt::Debug for SharedSteamCredential {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SharedSteamCredential")
            .field("account_name", &self.account_name)
            .field("password", &"[redacted]")
            .field("steam_id", &self.steam_id)
            .finish()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SharedWebSession {
    kind: String,
    session: steam::SteamWebSession,
    cookies: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    perfect_profile: Option<perfect_arena::PerfectArenaProfile>,
    #[serde(default)]
    perfect_unavailable: bool,
    #[serde(default)]
    perfect_files: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct NeaShareManifest {
    format: String,
    exported_at: String,
    has_oopz_package: bool,
    web_sessions: Vec<SharedWebSession>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    steam_credentials: Vec<SharedSteamCredential>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct QuickImportResult {
    oopz_accounts: Vec<SavedAccount>,
    steam_web_accounts: usize,
    perfect_accounts: usize,
    steam_web_added: usize,
    steam_web_updated: usize,
    perfect_added: usize,
    perfect_updated: usize,
    steam_credentials_accounts: usize,
    steam_credentials_added: usize,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct QuickShareExportResult {
    accounts: usize,
    package_bytes: u64,
}

type QuickShareMaterial = (
    Vec<SavedAccount>,
    Vec<SharedWebSession>,
    Vec<SharedSteamCredential>,
);

struct PreparedQuickImport {
    root: PathBuf,
    manifest: NeaShareManifest,
    oopz_package: Option<PathBuf>,
    perfect_files: Vec<(String, String, PathBuf)>,
}

struct StagedQuickWebSession {
    item: SharedWebSession,
    session: steam::SteamWebSession,
    stage_dir: PathBuf,
    target_dir: PathBuf,
    target_existed: bool,
    perfect_existed: bool,
}

struct AppliedSharePath {
    target: PathBuf,
    backup: Option<PathBuf>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct QuickShareRollbackPath {
    target: PathBuf,
    backup: Option<PathBuf>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct QuickShareCredentialRollback {
    steam_id: String,
    normalized_account_name: String,
    updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct QuickShareRollbackJournal {
    affected_steam_ids: Vec<String>,
    web_sessions: Vec<steam::SteamWebSession>,
    perfect_profiles: HashMap<String, perfect_arena::PerfectArenaProfile>,
    #[serde(default)]
    added_credentials: Vec<QuickShareCredentialRollback>,
    paths: Vec<QuickShareRollbackPath>,
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
    switch_running: AtomicBool,
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
    steam_web_import_running: AtomicBool,
    steam_bulk_import_running: AtomicBool,
    steam_bulk_import_cancelled: AtomicBool,
    steam_import_duplicate_sessions: Mutex<HashMap<String, String>>,
    steam_capability_running: AtomicBool,
    steam_capability_paused: AtomicBool,
    steam_capability_cancelled: AtomicBool,
    main_webview_low_memory: AtomicBool,
    tray_perfect_available_only: AtomicBool,
}

fn initial_app_state() -> AppState {
    AppState {
        data: Mutex::new(load_data()),
        account_operation: Mutex::new(()),
        switch_operation: Mutex::new(()),
        switch_running: AtomicBool::new(false),
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
        steam_web_import_running: AtomicBool::new(false),
        steam_bulk_import_running: AtomicBool::new(false),
        steam_bulk_import_cancelled: AtomicBool::new(false),
        steam_import_duplicate_sessions: Mutex::new(HashMap::new()),
        steam_capability_running: AtomicBool::new(false),
        steam_capability_paused: AtomicBool::new(false),
        steam_capability_cancelled: AtomicBool::new(false),
        main_webview_low_memory: AtomicBool::new(false),
        tray_perfect_available_only: AtomicBool::new(false),
    }
}

struct SwitchActivityGuard {
    app: AppHandle,
}

struct SteamWebImportGuard {
    app: AppHandle,
}

struct SteamBulkImportGuard {
    app: AppHandle,
}

struct SteamCapabilityGuard {
    app: AppHandle,
}

impl Drop for SteamWebImportGuard {
    fn drop(&mut self) {
        self.app
            .state::<AppState>()
            .steam_web_import_running
            .store(false, Ordering::SeqCst);
    }
}

fn acquire_steam_web_import(app: &AppHandle) -> Result<SteamWebImportGuard, String> {
    if app
        .state::<AppState>()
        .steam_web_import_running
        .swap(true, Ordering::SeqCst)
    {
        return Err("已有 Steam 网页账号批量导入正在进行".to_string());
    }
    Ok(SteamWebImportGuard { app: app.clone() })
}

impl Drop for SteamBulkImportGuard {
    fn drop(&mut self) {
        let state = self.app.state::<AppState>();
        state
            .steam_bulk_import_cancelled
            .store(false, Ordering::SeqCst);
        state
            .steam_bulk_import_running
            .store(false, Ordering::SeqCst);
        let _ = self.app.emit("steam-bulk-import-state", false);
    }
}

fn acquire_steam_bulk_import(app: &AppHandle) -> Result<SteamBulkImportGuard, String> {
    let state = app.state::<AppState>();
    if state.steam_bulk_import_running.swap(true, Ordering::SeqCst) {
        return Err("已有 Steam 网页账号批量导入正在进行".to_string());
    }
    state
        .steam_bulk_import_cancelled
        .store(false, Ordering::SeqCst);
    let _ = app.emit("steam-bulk-import-state", true);
    Ok(SteamBulkImportGuard { app: app.clone() })
}

fn steam_capability_status(app: &AppHandle) -> SteamCapabilityStatus {
    let state = app.state::<AppState>();
    SteamCapabilityStatus {
        running: state.steam_capability_running.load(Ordering::SeqCst),
        paused: state.steam_capability_paused.load(Ordering::SeqCst),
        cancelling: state.steam_capability_cancelled.load(Ordering::SeqCst),
    }
}

fn emit_steam_capability_status(app: &AppHandle) -> SteamCapabilityStatus {
    let status = steam_capability_status(app);
    let _ = app.emit("steam-capability-state", status.clone());
    status
}

impl Drop for SteamCapabilityGuard {
    fn drop(&mut self) {
        let state = self.app.state::<AppState>();
        state
            .steam_capability_running
            .store(false, Ordering::SeqCst);
        state.steam_capability_paused.store(false, Ordering::SeqCst);
        state
            .steam_capability_cancelled
            .store(false, Ordering::SeqCst);
        emit_steam_capability_status(&self.app);
    }
}

fn acquire_steam_capability(app: &AppHandle) -> Result<SteamCapabilityGuard, String> {
    let state = app.state::<AppState>();
    if state.steam_capability_running.swap(true, Ordering::SeqCst) {
        return Err("Steam 登录方式补全正在进行".to_string());
    }
    state.steam_capability_paused.store(false, Ordering::SeqCst);
    state
        .steam_capability_cancelled
        .store(false, Ordering::SeqCst);
    emit_steam_capability_status(app);
    Ok(SteamCapabilityGuard { app: app.clone() })
}

impl Drop for SwitchActivityGuard {
    fn drop(&mut self) {
        self.app
            .state::<AppState>()
            .switch_running
            .store(false, Ordering::SeqCst);
        update_tray(&self.app);
    }
}

fn acquire_switch_activity(app: &AppHandle) -> Result<SwitchActivityGuard, String> {
    if app
        .state::<AppState>()
        .switch_running
        .swap(true, Ordering::SeqCst)
    {
        return Err("另一项切号或恢复操作正在进行，请稍候".to_string());
    }
    update_tray(app);
    Ok(SwitchActivityGuard { app: app.clone() })
}

struct NamedMutex(HANDLE);

impl Drop for NamedMutex {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

struct PluginRuntimeMutex {
    _current: NamedMutex,
    _legacy: NamedMutex,
}

fn create_named_mutex(name: PCWSTR) -> Result<Option<NamedMutex>, String> {
    unsafe {
        let handle =
            CreateMutexW(None, false, name).map_err(|e| format!("创建插件单实例锁失败: {}", e))?;
        if GetLastError().is_err() {
            let _ = CloseHandle(handle);
            Ok(None)
        } else {
            Ok(Some(NamedMutex(handle)))
        }
    }
}

fn acquire_plugin_runtime_mutex() -> Result<Option<PluginRuntimeMutex>, String> {
    let Some(legacy) = create_named_mutex(w!("Local\\OOPZPlus.PluginRuntime"))? else {
        return Ok(None);
    };
    let Some(current) = create_named_mutex(w!("Local\\NEA.PluginRuntime"))? else {
        return Ok(None);
    };
    Ok(Some(PluginRuntimeMutex {
        _current: current,
        _legacy: legacy,
    }))
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
    fs::create_dir_all(&current).map_err(|error| error.to_string())?;
    let marker = current.join("migration-from-oopz-plus-v2.txt");
    if legacy.exists() && !marker.exists() && migrate_legacy_storage(&legacy, &current).is_ok() {
        let _ = fs::write(marker, now());
    }
    organize_storage_layout(&current)?;
    Ok(current)
}

fn organize_storage_layout(current: &Path) -> Result<(), String> {
    let oopz_workspace = current.join("workspaces").join("oopz");
    let legacy_root = current.join("legacy").join("oopz-root");
    for folder in ["accounts", "backups"] {
        let legacy_folder = current.join(folder);
        let workspace_folder = oopz_workspace.join(folder);
        if legacy_folder.exists() {
            copy_directory_missing(&legacy_folder, &workspace_folder)?;
            fs::create_dir_all(&legacy_root).map_err(|error| error.to_string())?;
            let archived = legacy_root.join(folder);
            if !archived.exists() {
                fs::rename(&legacy_folder, &archived)
                    .map_err(|error| format!("归档旧版 {folder} 目录失败: {error}"))?;
            }
        }
    }
    for directory in ["runtime", "recovery", "legacy"] {
        fs::create_dir_all(current.join(directory)).map_err(|error| error.to_string())?;
    }
    let runtime = current.join("runtime");
    for file_name in ["update-completed.txt", "update-error.txt"] {
        let old = current.join(file_name);
        let next = runtime.join(file_name);
        if old.exists() && !next.exists() {
            let _ = fs::rename(old, next);
        }
    }
    Ok(())
}

fn migrate_legacy_storage(legacy: &Path, current: &Path) -> Result<(), String> {
    copy_directory_missing(legacy, current)?;
    let legacy_config = legacy.join("config.json");
    let current_config = current.join("config.json");
    let Ok(legacy_raw) = fs::read_to_string(&legacy_config) else {
        return Ok(());
    };
    let Ok(mut legacy_data) = serde_json::from_str::<AppData>(&legacy_raw) else {
        return Ok(());
    };
    let mut current_data = fs::read_to_string(&current_config)
        .ok()
        .and_then(|raw| serde_json::from_str::<AppData>(&raw).ok())
        .unwrap_or_default();
    for account in legacy_data.accounts.drain(..) {
        let exists = current_data.accounts.iter().any(|saved| {
            saved.id == account.id
                || (saved.uid.is_some() && saved.uid.as_deref() == account.uid.as_deref())
        });
        if !exists {
            current_data.accounts.push(account);
        }
    }
    if current_data.config.oopz_install_dir.is_none() {
        current_data.config.oopz_install_dir = legacy_data.config.oopz_install_dir;
    }
    if current_data.config.oopz_exe_path.is_none() {
        current_data.config.oopz_exe_path = legacy_data.config.oopz_exe_path;
    }
    if current_data.config.roaming_data_dir.is_none() {
        current_data.config.roaming_data_dir = legacy_data.config.roaming_data_dir;
    }
    if current_data.config.local_sandbox_dir.is_none() {
        current_data.config.local_sandbox_dir = legacy_data.config.local_sandbox_dir;
    }
    for account in legacy_data.steam.accounts {
        if !current_data
            .steam
            .accounts
            .iter()
            .any(|saved| saved.id == account.id)
        {
            current_data.steam.accounts.push(account);
        }
    }
    current_data.schema_version = current_data.schema_version.max(CURRENT_SCHEMA_VERSION);
    let raw = serde_json::to_string_pretty(&current_data).map_err(|error| error.to_string())?;
    let temp = current_config.with_extension("json.migration.tmp");
    let backup = current_config.with_extension("json.migration.bak");
    fs::write(&temp, raw).map_err(|error| error.to_string())?;
    if backup.exists() {
        let _ = fs::remove_file(&backup);
    }
    if current_config.exists() {
        fs::rename(&current_config, &backup).map_err(|error| error.to_string())?;
    }
    if let Err(error) = fs::rename(&temp, &current_config) {
        if backup.exists() {
            let _ = fs::rename(&backup, &current_config);
        }
        return Err(error.to_string());
    }
    if backup.exists() {
        let _ = fs::remove_file(backup);
    }
    Ok(())
}

fn copy_directory_missing(source: &Path, destination: &Path) -> Result<(), String> {
    fs::create_dir_all(destination).map_err(|error| error.to_string())?;
    for entry in fs::read_dir(source).map_err(|error| error.to_string())? {
        let entry = entry.map_err(|error| error.to_string())?;
        let target = destination.join(entry.file_name());
        if entry
            .file_type()
            .map_err(|error| error.to_string())?
            .is_dir()
        {
            copy_directory_missing(&entry.path(), &target)?;
        } else if !target.exists() {
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

fn runtime_dir() -> Result<PathBuf, String> {
    let path = storage_dir()?.join("runtime");
    fs::create_dir_all(&path).map_err(|error| error.to_string())?;
    Ok(path)
}

fn recovery_dir() -> Result<PathBuf, String> {
    let path = storage_dir()?.join("recovery");
    fs::create_dir_all(&path).map_err(|error| error.to_string())?;
    Ok(path)
}

fn update_marker_path() -> Result<PathBuf, String> {
    Ok(runtime_dir()?.join("update-completed.txt"))
}

fn update_error_path() -> Result<PathBuf, String> {
    Ok(runtime_dir()?.join("update-error.txt"))
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
    if ![
        "https://github.com/M4rkzzz/NEA/releases/download/",
        "https://github.com/M4rkzzz/oopz-plus/releases/download/",
    ]
    .iter()
    .any(|prefix| asset.browser_download_url.starts_with(prefix))
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
    let partial = temp_dir.join(format!("nea-{}.msi.part", version));
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
                path.join("NEA").join(APP_EXECUTABLE_NAME),
                path.join("NEA").join(LEGACY_APP_EXECUTABLE_NAME),
                path.join("OOPZ+").join(LEGACY_APP_EXECUTABLE_NAME),
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
    let helper = std::env::temp_dir().join(format!("nea-updater-{}.exe", Uuid::new_v4()));
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

fn parse_app_data_file(path: &Path) -> Option<(AppData, String)> {
    let raw = fs::read_to_string(path).ok()?;
    serde_json::from_str::<AppData>(&raw)
        .ok()
        .map(|data| (data, raw))
}

fn recover_config_file(path: &Path) -> Option<(AppData, String)> {
    let backup = path.with_extension("json.bak");
    let temp = path.with_extension("json.tmp");
    if let Some(valid) = parse_app_data_file(path) {
        CONFIG_WRITES_BLOCKED.store(false, Ordering::SeqCst);
        return Some(valid);
    }
    for candidate in [&backup, &temp] {
        let Some((data, raw)) = parse_app_data_file(candidate) else {
            continue;
        };
        let recovery = path.with_extension("json.recovery.tmp");
        if fs::write(&recovery, &raw).is_err() {
            continue;
        }
        if path.exists() {
            let corrupt = path.with_extension(format!(
                "json.corrupt-{}",
                Utc::now().format("%Y%m%d%H%M%S")
            ));
            if fs::rename(path, corrupt).is_err() {
                let _ = fs::remove_file(&recovery);
                continue;
            }
        }
        if fs::rename(&recovery, path).is_ok() {
            CONFIG_WRITES_BLOCKED.store(false, Ordering::SeqCst);
            return Some((data, raw));
        }
        let _ = fs::remove_file(recovery);
    }
    let recovery_files_exist = path.exists() || backup.exists() || temp.exists();
    CONFIG_WRITES_BLOCKED.store(recovery_files_exist, Ordering::SeqCst);
    None
}

fn load_data() -> AppData {
    if ensure_storage().is_err() {
        return AppData::default();
    }
    if let Ok(root) = storage_dir() {
        recover_staged_deletions(&root);
        let legacy_trash = root.join("trash");
        if fs::read_dir(&legacy_trash)
            .ok()
            .is_some_and(|mut entries| entries.next().is_none())
        {
            let _ = fs::remove_dir(legacy_trash);
        }
    }
    let Ok(path) = config_path() else {
        return AppData::default();
    };
    let Some((mut data, raw)) = recover_config_file(&path) else {
        return AppData::default();
    };
    data.schema_version = data.schema_version.max(CURRENT_SCHEMA_VERSION);
    migrate_current_login_state(&mut data);
    migrate_avatar_sources(&mut data);
    externalize_perfect_profile_avatars(&mut data);
    reconcile_account_readiness(&mut data);
    reconcile_saved_steam_credentials(&mut data);
    reconcile_steam_identities(&mut data);
    if let Ok(next_raw) = serde_json::to_string_pretty(&data) {
        if next_raw != raw {
            let _ = save_data(&data);
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
    save_data_inner(data, false)
}

fn save_verified_recovery_data(data: &AppData) -> Result<(), String> {
    save_data_inner(data, true)
}

fn save_data_inner(data: &AppData, allow_blocked_recovery: bool) -> Result<(), String> {
    let _write_guard = CONFIG_WRITE_LOCK
        .lock()
        .map_err(|error| format!("配置写入锁异常: {error}"))?;
    if CONFIG_WRITES_BLOCKED.load(Ordering::SeqCst) && !allow_blocked_recovery {
        return Err("NEA 配置文件损坏且无法自动恢复，已阻止写入以保护现有账号数据".to_string());
    }
    ensure_storage()?;
    let mut canonical = data.clone();
    externalize_perfect_profile_avatars(&mut canonical);
    reconcile_steam_identities(&mut canonical);
    let raw = serde_json::to_vec_pretty(&canonical).map_err(|e| e.to_string())?;
    let path = config_path()?;
    let temp = path.with_extension("json.tmp");
    let backup = path.with_extension("json.bak");
    fs::write(&temp, &raw).map_err(|e| format!("写入配置失败: {}", e))?;

    if path.exists() {
        if backup.exists() {
            let _ = fs::remove_file(&backup);
        }
        fs::rename(&path, &backup).map_err(|e| format!("备份原配置失败: {}", e))?;
    }
    if let Err(error) = fs::rename(&temp, &path) {
        if backup.exists() {
            let _ = fs::rename(&backup, &path);
        }
        let _ = fs::remove_file(&temp);
        return Err(format!("保存配置失败: {}", error));
    }
    *LAST_INTERNAL_CONFIG_BYTES
        .lock()
        .map_err(|error| format!("配置写入状态锁异常: {error}"))? = Some(raw);
    CONFIG_WRITES_BLOCKED.store(false, Ordering::SeqCst);
    Ok(())
}

fn commit_data_update_with<T, U, P>(
    data: &Mutex<AppData>,
    update: U,
    persist: P,
) -> Result<T, String>
where
    U: FnOnce(&mut AppData) -> Result<T, String>,
    P: FnOnce(&AppData) -> Result<(), String>,
{
    let mut current = data.lock().map_err(|error| error.to_string())?;
    let mut next = current.clone();
    let result = update(&mut next)?;
    persist(&next)?;
    *current = next;
    Ok(result)
}

fn commit_app_data_update<T, U>(state: &AppState, update: U) -> Result<T, String>
where
    U: FnOnce(&mut AppData) -> Result<T, String>,
{
    commit_data_update_with(
        &state.data,
        |data| {
            let result = update(data)?;
            reconcile_steam_identities(data);
            Ok(result)
        },
        save_data,
    )
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

fn is_nea_runtime_process_name(name: &str) -> bool {
    name.eq_ignore_ascii_case(APP_EXECUTABLE_NAME)
        || name.eq_ignore_ascii_case(LEGACY_APP_EXECUTABLE_NAME)
        || name.eq_ignore_ascii_case(WATCHER_FILE_NAME)
        || name.eq_ignore_ascii_case(LEGACY_WATCHER_FILE_NAME)
}

fn is_watcher_executable_name(name: &str) -> bool {
    name.eq_ignore_ascii_case(WATCHER_FILE_NAME)
        || name.eq_ignore_ascii_case(LEGACY_WATCHER_FILE_NAME)
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
        is_nea_runtime_process_name(process.name())
            && process.cmd().iter().any(|arg| arg == "--plugin-runtime")
    })
}

fn is_plugin_runtime_running() -> bool {
    is_plugin_runtime_running_in(&process_system())
}

fn is_watcher_running_in(system: &System) -> bool {
    system.processes().values().any(|process| {
        is_nea_runtime_process_name(process.name())
            && process.cmd().iter().any(|arg| arg == "--watcher")
    })
}

fn is_watcher_running() -> bool {
    is_watcher_running_in(&process_system())
}

fn stop_watcher() {
    let system = process_system();
    for process in system.processes().values() {
        if is_nea_runtime_process_name(process.name())
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
        if is_nea_runtime_process_name(process.name())
            && process.cmd().iter().any(|arg| arg == "--plugin-runtime")
        {
            let _ = process
                .kill_with(Signal::Term)
                .unwrap_or_else(|| process.kill());
        }
    }
}

fn watcher_path() -> Result<PathBuf, String> {
    Ok(runtime_dir()?.join(WATCHER_FILE_NAME))
}

fn legacy_watcher_path() -> Result<PathBuf, String> {
    Ok(storage_dir()?.join(LEGACY_WATCHER_FILE_NAME))
}

fn root_current_watcher_path() -> Result<PathBuf, String> {
    Ok(storage_dir()?.join(WATCHER_FILE_NAME))
}

fn remove_installed_watchers() -> Result<(), String> {
    for watcher in [
        watcher_path()?,
        root_current_watcher_path()?,
        legacy_watcher_path()?,
    ] {
        if watcher.exists() {
            remove_file_with_retries(&watcher)?;
        }
    }
    Ok(())
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
    for old_watcher in [legacy_watcher_path(), root_current_watcher_path()]
        .into_iter()
        .flatten()
    {
        if old_watcher.exists() {
            remove_file_with_retries(&old_watcher)
                .map_err(|error| format!("清理旧守护进程失败: {}", error))?;
        }
    }
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
    let _ = key.delete_value(LEGACY_RUN_KEY_NAME);
    key.set_value(RUN_KEY_NAME, &command)
        .map_err(|e| format!("注册守护自启动失败: {}", e))
}

fn uninstall_watcher() -> Result<(), String> {
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    if let Ok(key) = hkcu.open_subkey_with_flags(RUN_KEY_PATH, winreg::enums::KEY_SET_VALUE) {
        for value_name in [RUN_KEY_NAME, LEGACY_RUN_KEY_NAME] {
            match key.delete_value(value_name) {
                Ok(_) => {}
                Err(error) if error.kind() == ErrorKind::NotFound => {}
                Err(error) => return Err(format!("取消守护自启动失败: {}", error)),
            }
        }
    }
    Ok(())
}

fn watcher_installed() -> bool {
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    hkcu.open_subkey(RUN_KEY_PATH)
        .map(|key| {
            [RUN_KEY_NAME, LEGACY_RUN_KEY_NAME]
                .iter()
                .any(|name| key.get_value::<String, _>(*name).is_ok())
        })
        .unwrap_or(false)
}

fn watcher_registration_is_current() -> bool {
    let Ok(watcher) = watcher_path() else {
        return false;
    };
    let expected = format!("\"{}\" --watcher", watcher.display());
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    hkcu.open_subkey(RUN_KEY_PATH)
        .and_then(|key| key.get_value::<String, _>(RUN_KEY_NAME))
        .is_ok_and(|value| value.eq_ignore_ascii_case(&expected))
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
    let old = parent.join(format!(".nea-old-{}", Uuid::new_v4()));
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

fn rollback_replaced_dir(dst: &Path, previous: &Path, had_dst: bool) -> Result<(), String> {
    if dst.exists() {
        fs::remove_dir_all(dst)
            .map_err(|error| format!("移除未提交目录失败 {}: {}", dst.display(), error))?;
    }
    if had_dst {
        fs::rename(previous, dst)
            .map_err(|error| format!("恢复原目录失败 {}: {}", dst.display(), error))?;
    }
    Ok(())
}

fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<(), String> {
    if !src.exists() {
        return Ok(());
    }
    let parent = dst.parent().ok_or_else(|| "目标目录无效".to_string())?;
    fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    let staging = parent.join(format!(".nea-copy-{}", Uuid::new_v4()));
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
    if bytes.len() >= 12
        && &bytes[4..8] == b"ftyp"
        && (&bytes[8..12] == b"avif" || &bytes[8..12] == b"avis")
    {
        return Some("image/avif");
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

fn perfect_avatar_dir_at(storage_root: &Path) -> Result<PathBuf, String> {
    let path = storage_root
        .join("workspaces")
        .join("perfect")
        .join("avatars");
    fs::create_dir_all(&path).map_err(|error| error.to_string())?;
    Ok(path)
}

fn perfect_avatar_dir() -> Result<PathBuf, String> {
    perfect_avatar_dir_at(&storage_dir()?)
}

fn perfect_avatar_path_at(storage_root: &Path, steam_id: &str) -> Result<PathBuf, String> {
    if !is_valid_steam_id64(steam_id) {
        return Err("完美账号 SteamID 无效".to_string());
    }
    Ok(perfect_avatar_dir_at(storage_root)?.join(format!("{steam_id}.img")))
}

fn perfect_avatar_path(steam_id: &str) -> Result<PathBuf, String> {
    perfect_avatar_path_at(&storage_dir()?, steam_id)
}

fn decode_avatar_data_url(value: &str) -> Option<Vec<u8>> {
    let (metadata, encoded) = value.split_once(',')?;
    if !metadata.starts_with("data:image/") || !metadata.ends_with(";base64") {
        return None;
    }
    let bytes = general_purpose::STANDARD.decode(encoded).ok()?;
    if bytes.is_empty() || bytes.len() as u64 > MAX_AVATAR_BYTES {
        return None;
    }
    avatar_mime(&bytes, Some(&metadata[5..metadata.len() - 7]))?;
    Some(bytes)
}

fn store_perfect_avatar_data_url_at(
    storage_root: &Path,
    steam_id: &str,
    data_url: &str,
) -> Result<(), String> {
    let bytes =
        decode_avatar_data_url(data_url).ok_or_else(|| "完美账号头像缓存格式无效".to_string())?;
    let target = perfect_avatar_path_at(storage_root, steam_id)?;
    if fs::read(&target).ok().as_deref() == Some(bytes.as_slice()) {
        return Ok(());
    }
    let temp = target.with_extension(format!("{}.tmp", Uuid::new_v4()));
    fs::write(&temp, &bytes).map_err(|error| format!("写入完美头像缓存失败: {error}"))?;
    if target.exists() {
        fs::remove_file(&target).map_err(|error| format!("替换完美头像缓存失败: {error}"))?;
    }
    if let Err(error) = fs::rename(&temp, &target) {
        let _ = fs::remove_file(&temp);
        return Err(format!("提交完美头像缓存失败: {error}"));
    }
    Ok(())
}

fn store_perfect_avatar_data_url(steam_id: &str, data_url: &str) -> Result<(), String> {
    store_perfect_avatar_data_url_at(&storage_dir()?, steam_id, data_url)
}

fn externalize_perfect_profile_avatars(data: &mut AppData) -> bool {
    let mut changed = false;
    for profile in data.perfect_profiles.values_mut() {
        if profile.avatar_source_url.is_none()
            && profile
                .avatar_url
                .as_deref()
                .is_some_and(|url| url.starts_with("http://") || url.starts_with("https://"))
        {
            profile.avatar_source_url = profile.avatar_url.clone();
            changed = true;
        }
        let Some(data_url) = profile
            .avatar_url
            .clone()
            .filter(|url| url.starts_with("data:image/"))
        else {
            continue;
        };
        if store_perfect_avatar_data_url(&profile.steam_id, &data_url).is_ok() {
            profile.avatar_url = Some(
                profile
                    .avatar_source_url
                    .clone()
                    .unwrap_or_else(|| PERFECT_AVATAR_CACHE_MARKER.to_string()),
            );
            changed = true;
        }
    }
    changed
}

fn hydrate_perfect_profile_avatar_at(
    storage_root: &Path,
    profile: &mut perfect_arena::PerfectArenaProfile,
) {
    let Some(bytes) = perfect_avatar_path_at(storage_root, &profile.steam_id)
        .ok()
        .and_then(|path| fs::read(path).ok())
        .filter(|bytes| !bytes.is_empty() && bytes.len() as u64 <= MAX_AVATAR_BYTES)
    else {
        if profile.avatar_url.as_deref() == Some(PERFECT_AVATAR_CACHE_MARKER) {
            profile.avatar_url.clone_from(&profile.avatar_source_url);
        }
        return;
    };
    if let Some(mime) = avatar_mime(&bytes, None) {
        profile.avatar_url = Some(format!(
            "data:{mime};base64,{}",
            general_purpose::STANDARD.encode(bytes)
        ));
    }
}

fn hydrate_perfect_profile_avatar(profile: &mut perfect_arena::PerfectArenaProfile) {
    if let Ok(storage_root) = storage_dir() {
        hydrate_perfect_profile_avatar_at(&storage_root, profile);
    }
}

fn hydrate_perfect_profile_avatars(profiles: &mut [perfect_arena::PerfectArenaProfile]) {
    for profile in profiles {
        hydrate_perfect_profile_avatar(profile);
    }
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
    if let Some(raw) = credential_entry(account_id)
        .ok()
        .and_then(|entry| entry.get_password().ok())
    {
        return Some(raw);
    }
    let raw = legacy_credential_entry(account_id)
        .ok()
        .and_then(|entry| entry.get_password().ok())?;
    let _ = write_secret_raw(account_id, &raw);
    Some(raw)
}

fn write_secret_raw(account_id: &str, raw: &str) -> Result<(), String> {
    credential_entry(account_id)?
        .set_password(raw)
        .map_err(|e| e.to_string())
}

fn read_secret(account_id: &str) -> SecretPayload {
    let Some(payload) = read_secret_raw(account_id) else {
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

fn is_safe_account_component(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value.chars().all(|character| {
            character.is_ascii_alphanumeric() || character == '-' || character == '_'
        })
}

fn validate_exported_oopz_identity(
    account: &ExportedAccount,
    oopz_login: &str,
) -> Result<String, String> {
    let uid = account
        .uid
        .as_deref()
        .filter(|uid| is_safe_account_component(uid))
        .ok_or_else(|| "导入包包含无效 OOPZ UID".to_string())?;
    let login_uid = uid_from_registry_login(oopz_login)
        .filter(|login_uid| is_safe_account_component(login_uid))
        .ok_or_else(|| "导入包中的 OOPZ 登录状态无法识别账号".to_string())?;
    if login_uid != uid {
        return Err("导入包中的 OOPZ 登录状态与账号 UID 不匹配".to_string());
    }
    Ok(uid.to_string())
}

fn write_export_files(root: &Path, files: &[ExportedFile]) -> Result<(), String> {
    let parent = root.parent().ok_or_else(|| "账号目录无效".to_string())?;
    fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    let staging = parent.join(format!(".nea-import-{}", Uuid::new_v4()));
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

fn rebuild_and_reopen_perfect_tray_menu(app: &AppHandle) -> Result<(), String> {
    let BuiltTrayMenus { root, perfect } =
        build_tray_menus(app).map_err(|error| error.to_string())?;
    let tray = app
        .tray_by_id("main-tray")
        .ok_or_else(|| "未找到系统托盘".to_string())?;
    tray.set_menu(Some(root))
        .map_err(|error| error.to_string())?;
    let window = app
        .get_webview_window("main")
        .ok_or_else(|| "未找到 NEA 主窗口".to_string())?;
    window
        .popup_menu(&perfect)
        .map_err(|error| error.to_string())
}

fn toggle_perfect_available_filter(app: &AppHandle) {
    app.state::<AppState>()
        .tray_perfect_available_only
        .fetch_xor(true, Ordering::SeqCst);
    if let Err(error) = rebuild_and_reopen_perfect_tray_menu(app) {
        app.state::<AppState>()
            .tray_perfect_available_only
            .fetch_xor(true, Ordering::SeqCst);
        update_tray(app);
        finish_tray_switch(app, Err(format!("无法更新完美账号筛选菜单: {error}")));
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
                let Some(changed_config_bytes) = changed_config_bytes else {
                    continue;
                };
                if serde_json::from_slice::<AppData>(&changed_config_bytes).is_err() {
                    continue;
                }
                let internal_write = LAST_INTERNAL_CONFIG_BYTES
                    .lock()
                    .ok()
                    .and_then(|bytes| bytes.clone())
                    .is_some_and(|bytes| bytes == changed_config_bytes);
                if internal_write {
                    last_config_bytes = Some(changed_config_bytes);
                    continue;
                }
                refresh_app_data_from_disk(&app);
                last_config_bytes = fs::read(&path).ok();
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

fn finish_tray_switch(app: &AppHandle, result: Result<SwitchResult, String>) {
    if tray_switch_failed(&result) {
        show_main_window(app);
    }
    let _ = app.emit("switch-finished", result);
}

fn tray_switch_failed(result: &Result<SwitchResult, String>) -> bool {
    match result {
        Ok(result) => !result.ok,
        Err(_) => true,
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

#[derive(Debug, Clone)]
struct TrayAccountMenuModel {
    id: String,
    label: String,
    sort_label: String,
    disambiguator: String,
    enabled: bool,
    current: bool,
    last_used_at: Option<String>,
}

#[derive(Debug, Clone)]
struct TrayActionMenuModel {
    id: String,
    label: String,
    enabled: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PerfectTrayRank {
    GoldAPlus,
    APlus,
    A,
    GoldBPlus,
    BPlus,
    B,
    GoldCPlus,
    CPlus,
    C,
    D,
    Pending,
}

impl PerfectTrayRank {
    const DISPLAY_ORDER: [Self; 11] = [
        Self::GoldAPlus,
        Self::APlus,
        Self::A,
        Self::GoldBPlus,
        Self::BPlus,
        Self::B,
        Self::GoldCPlus,
        Self::CPlus,
        Self::C,
        Self::D,
        Self::Pending,
    ];

    fn from_score(score: Option<i64>) -> Self {
        match score {
            None => Self::Pending,
            Some(score) if score <= 1000 => Self::D,
            Some(score) if score <= 1150 => Self::C,
            Some(score) if score <= 1300 => Self::CPlus,
            Some(score) if score <= 1450 => Self::GoldCPlus,
            Some(score) if score <= 1600 => Self::B,
            Some(score) if score <= 1750 => Self::BPlus,
            Some(score) if score <= 1900 => Self::GoldBPlus,
            Some(score) if score <= 2050 => Self::A,
            Some(score) if score <= 2200 => Self::APlus,
            Some(_) => Self::GoldAPlus,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::GoldAPlus => "金A+",
            Self::APlus => "A+",
            Self::A => "A",
            Self::GoldBPlus => "金B+",
            Self::BPlus => "B+",
            Self::B => "B",
            Self::GoldCPlus => "金C+",
            Self::CPlus => "C+",
            Self::C => "C",
            Self::D => "D",
            Self::Pending => "待检测",
        }
    }
}

#[derive(Debug, Clone)]
struct TrayPerfectAccountMenuModel {
    label: String,
    sort_label: String,
    disambiguator: String,
    rank: PerfectTrayRank,
    blocked: bool,
    enabled: bool,
    current: bool,
    actions: Vec<TrayActionMenuModel>,
}

#[derive(Debug, Clone, Default)]
struct TrayMenuRuntime {
    current_oopz_uid: Option<String>,
    current_steam_id: Option<String>,
    current_perfect_id: Option<String>,
    busy: bool,
    steam_ready: bool,
    perfect_ready: bool,
    perfect_available_only: bool,
}

fn tray_identifier_suffix(value: &str) -> String {
    let mut suffix = value
        .chars()
        .filter(|character| !character.is_control() && !character.is_whitespace())
        .rev()
        .take(4)
        .collect::<Vec<_>>();
    suffix.reverse();
    let suffix = suffix.into_iter().collect::<String>();
    if suffix.is_empty() {
        "----".to_string()
    } else {
        suffix
    }
}

fn sanitize_tray_label(value: &str, max_chars: usize) -> String {
    let mut sanitized = String::new();
    let mut pending_space = false;
    for character in value.chars() {
        if character.is_control() || character.is_whitespace() {
            pending_space = !sanitized.is_empty();
            continue;
        }
        if pending_space {
            sanitized.push(' ');
            pending_space = false;
        }
        sanitized.push(character);
    }
    if sanitized.chars().count() <= max_chars {
        return sanitized;
    }
    let mut truncated = sanitized
        .chars()
        .take(max_chars.saturating_sub(1))
        .collect::<String>();
    truncated.push('…');
    truncated
}

fn tray_public_name(value: &str, pending: &str, identifier: &str) -> (String, bool) {
    let value = sanitize_tray_label(value, 48);
    if value.is_empty() {
        (
            format!("{pending} · {}", tray_identifier_suffix(identifier)),
            false,
        )
    } else {
        (value, true)
    }
}

fn steam_community_tray_name(data: &AppData, identity: &SteamIdentity) -> (String, bool) {
    let display_name = sanitize_tray_label(&identity.display_name, 48);
    let steam_id = identity.steam_id.as_deref();
    let is_account_name = identity
        .account_name
        .as_deref()
        .into_iter()
        .chain(
            data.steam
                .accounts
                .iter()
                .filter(|account| Some(account.id.as_str()) == steam_id)
                .map(|account| account.account_name.as_str()),
        )
        .chain(
            data.steam
                .web_sessions
                .iter()
                .filter(|session| session.steam_id.as_deref() == steam_id)
                .filter_map(|session| session.account_name.as_deref()),
        )
        .chain(
            data.steam_credentials
                .iter()
                .filter(|credential| credential.steam_id.as_deref() == steam_id)
                .map(|credential| credential.account_name.as_str()),
        )
        .any(|account_name| {
            display_name.eq_ignore_ascii_case(&sanitize_tray_label(account_name, 48))
        });
    let is_steam_id = identity
        .steam_id
        .as_deref()
        .is_some_and(|steam_id| display_name == steam_id);
    let looks_like_steam_id = display_name.len() == 17
        && display_name
            .chars()
            .all(|character| character.is_ascii_digit());
    if display_name.is_empty()
        || display_name == "待登录网页账号"
        || is_account_name
        || is_steam_id
        || looks_like_steam_id
    {
        let identifier = identity.steam_id.as_deref().unwrap_or(&identity.id);
        return (
            format!("社区 ID 待获取 · {}", tray_identifier_suffix(identifier)),
            false,
        );
    }
    (display_name, true)
}

fn perfect_rank(score: i64) -> &'static str {
    PerfectTrayRank::from_score(Some(score)).label()
}

fn perfect_profile_is_high_risk(profile: Option<&perfect_arena::PerfectArenaProfile>) -> bool {
    profile.is_some_and(|profile| {
        profile.high_risk == Some(true) || profile.reputation_requires_verification == Some(true)
    })
}

fn perfect_account_is_blocked(data: &AppData, steam_id: &str) -> bool {
    data.perfect_unavailable_account_ids.contains(steam_id)
        || perfect_profile_is_high_risk(data.perfect_profiles.get(steam_id))
}

fn perfect_tray_summary(data: &AppData, steam_id: &str) -> (String, bool) {
    let profile = data.perfect_profiles.get(steam_id);
    let (name, known_name) = tray_public_name(
        profile
            .and_then(|profile| profile.nickname.as_deref())
            .unwrap_or_default(),
        "完美名称待获取",
        steam_id,
    );
    let mut parts = vec![name];
    if let Some(score) = profile.and_then(|profile| profile.score) {
        parts.push(format!("{}{score}", perfect_rank(score)));
    }
    let reputation = if data.perfect_unavailable_account_ids.contains(steam_id) {
        Some("不可用")
    } else if perfect_profile_is_high_risk(profile) {
        Some("高危")
    } else {
        profile
            .and_then(|profile| profile.reputation_level.as_deref())
            .map(str::trim)
            .filter(|value| !value.is_empty())
    };
    if let Some(reputation) = reputation {
        let reputation = sanitize_tray_label(reputation, 16);
        if !reputation.is_empty() {
            parts.push(reputation);
        }
    }
    (parts.join(" "), known_name)
}

fn tray_current_label(label: String, current: bool) -> String {
    if current {
        format!("✓ {label}")
    } else {
        label
    }
}

fn sort_tray_accounts(accounts: &mut [TrayAccountMenuModel]) {
    accounts.sort_by(|left, right| {
        right
            .current
            .cmp(&left.current)
            .then_with(|| right.last_used_at.cmp(&left.last_used_at))
            .then_with(|| {
                left.sort_label
                    .to_lowercase()
                    .cmp(&right.sort_label.to_lowercase())
            })
            .then_with(|| left.id.cmp(&right.id))
    });
}

fn disambiguate_tray_accounts(accounts: &mut [TrayAccountMenuModel]) {
    let counts = accounts.iter().fold(HashMap::new(), |mut counts, account| {
        *counts
            .entry(account.sort_label.to_lowercase())
            .or_insert(0usize) += 1;
        counts
    });
    for account in accounts {
        if counts
            .get(&account.sort_label.to_lowercase())
            .is_some_and(|count| *count > 1)
        {
            account.label = tray_current_label(
                format!("{} · {}", account.sort_label, account.disambiguator),
                account.current,
            );
        }
    }
}

fn disambiguate_perfect_tray_accounts(accounts: &mut [TrayPerfectAccountMenuModel]) {
    let counts = accounts.iter().fold(HashMap::new(), |mut counts, account| {
        *counts
            .entry(account.sort_label.to_lowercase())
            .or_insert(0usize) += 1;
        counts
    });
    for account in accounts {
        if counts
            .get(&account.sort_label.to_lowercase())
            .is_some_and(|count| *count > 1)
        {
            account.label = tray_current_label(
                format!("{} · {}", account.sort_label, account.disambiguator),
                account.current,
            );
        }
    }
}

fn oopz_tray_accounts(data: &AppData, runtime: &TrayMenuRuntime) -> Vec<TrayAccountMenuModel> {
    let mut accounts = data
        .accounts
        .iter()
        .map(|account| {
            let identifier = account.uid.as_deref().unwrap_or(&account.id);
            let (name, _known_name) =
                tray_public_name(&account.display_name, "昵称待获取", identifier);
            let current = runtime
                .current_oopz_uid
                .as_deref()
                .is_some_and(|uid| account.uid.as_deref() == Some(uid));
            TrayAccountMenuModel {
                id: format!("oopz-switch:{}", account.id),
                label: tray_current_label(name.clone(), current),
                sort_label: name,
                disambiguator: tray_identifier_suffix(identifier),
                enabled: !current && !runtime.busy,
                current,
                last_used_at: account.last_used_at.clone(),
            }
        })
        .collect::<Vec<_>>();
    disambiguate_tray_accounts(&mut accounts);
    sort_tray_accounts(&mut accounts);
    accounts
}

fn steam_tray_accounts(data: &AppData, runtime: &TrayMenuRuntime) -> Vec<TrayAccountMenuModel> {
    let mut accounts = data
        .steam_identities
        .iter()
        .filter(|identity| identity.capabilities.credential && identity.steam_id.is_some())
        .map(|identity| {
            let steam_id = identity.steam_id.as_deref().expect("filtered SteamID");
            let (name, _known_name) = steam_community_tray_name(data, identity);
            let current = runtime.current_steam_id.as_deref() == Some(steam_id);
            let last_used_at = data
                .steam
                .accounts
                .iter()
                .find(|account| account.id == steam_id)
                .and_then(|account| account.last_used_at.clone());
            TrayAccountMenuModel {
                id: format!("steam-switch:{steam_id}"),
                label: tray_current_label(name.clone(), current),
                sort_label: name,
                disambiguator: tray_identifier_suffix(steam_id),
                enabled: runtime.steam_ready && !current && !runtime.busy,
                current,
                last_used_at,
            }
        })
        .collect::<Vec<_>>();
    disambiguate_tray_accounts(&mut accounts);
    sort_tray_accounts(&mut accounts);
    accounts
}

fn perfect_tray_accounts(
    data: &AppData,
    runtime: &TrayMenuRuntime,
) -> Vec<TrayPerfectAccountMenuModel> {
    let mut accounts = data
        .steam_identities
        .iter()
        .filter(|identity| identity.capabilities.web_login)
        .filter_map(|identity| {
            let steam_id = identity
                .steam_id
                .as_deref()
                .filter(|steam_id| is_valid_steam_id64(steam_id))?;
            let session_id = identity.web_session_id.as_deref()?;
            let profile = data.perfect_profiles.get(steam_id);
            let rank = PerfectTrayRank::from_score(profile.and_then(|profile| profile.score));
            let blocked = perfect_account_is_blocked(data, steam_id);
            let (summary, _known_name) = perfect_tray_summary(data, steam_id);
            let perfect_current = runtime.current_perfect_id.as_deref() == Some(steam_id);
            let steam_current = runtime.current_steam_id.as_deref() == Some(steam_id);
            let has_credential = has_saved_credential_for_steam_id(data, steam_id);
            let only_enabled = runtime.perfect_ready && !perfect_current && !runtime.busy;
            let sync_enabled = runtime.perfect_ready
                && (steam_current || runtime.steam_ready && has_credential)
                && !(perfect_current && steam_current)
                && !runtime.busy;
            let only_label = if !runtime.perfect_ready {
                "仅切换完美（未找到客户端）".to_string()
            } else {
                "仅切换完美".to_string()
            };
            let sync_label = if !runtime.perfect_ready {
                "同步切换 Steam + 完美（未找到完美客户端）".to_string()
            } else if !steam_current && !runtime.steam_ready {
                "同步切换 Steam + 完美（Steam 未就绪）".to_string()
            } else if !steam_current && !has_credential {
                "同步切换 Steam + 完美（未保存 Steam 账密）".to_string()
            } else {
                "同步切换 Steam + 完美".to_string()
            };
            Some(TrayPerfectAccountMenuModel {
                label: tray_current_label(summary.clone(), perfect_current),
                sort_label: summary,
                disambiguator: tray_identifier_suffix(steam_id),
                rank,
                blocked,
                enabled: !runtime.busy,
                current: perfect_current,
                actions: vec![
                    TrayActionMenuModel {
                        id: format!("perfect-only:{session_id}"),
                        label: only_label,
                        enabled: only_enabled,
                    },
                    TrayActionMenuModel {
                        id: format!("perfect-sync:{session_id}"),
                        label: sync_label,
                        enabled: sync_enabled,
                    },
                ],
            })
        })
        .collect::<Vec<_>>();
    disambiguate_perfect_tray_accounts(&mut accounts);
    accounts.sort_by(|left, right| {
        right
            .current
            .cmp(&left.current)
            .then_with(|| {
                left.sort_label
                    .to_lowercase()
                    .cmp(&right.sort_label.to_lowercase())
            })
            .then_with(|| left.actions[0].id.cmp(&right.actions[0].id))
    });
    if runtime.perfect_available_only {
        accounts.retain(|account| !account.blocked);
    }
    accounts
}

struct BuiltTrayMenus {
    root: Menu<tauri::Wry>,
    perfect: Submenu<tauri::Wry>,
}

fn build_tray_menus(app: &AppHandle) -> tauri::Result<BuiltTrayMenus> {
    let state = app.state::<AppState>();
    let (mut data, busy) = {
        let data = state
            .data
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        (data, state.switch_running.load(Ordering::SeqCst))
    };
    reconcile_steam_identities(&mut data);
    let steam_client_running = steam::SteamAdapter::client_is_running();
    let steam_active_account_id = steam_client_running
        .then(steam::SteamAdapter::active_user_account_id)
        .flatten();
    let current_steam_id = steam_active_account_id.and_then(|active_account_id| {
        data.steam_identities
            .iter()
            .filter_map(|identity| identity.steam_id.as_deref())
            .find(|steam_id| steam::SteamAdapter::account_id32(steam_id) == Some(active_account_id))
            .map(str::to_string)
    });
    let perfect_workspace = perfect_arena::workspace(&data.steam);
    let runtime = TrayMenuRuntime {
        current_oopz_uid: current_registry_login()
            .and_then(|login| uid_from_registry_login(&login)),
        current_steam_id,
        current_perfect_id: perfect_workspace.current_account_id,
        busy,
        steam_ready: data
            .steam
            .installation
            .as_ref()
            .is_some_and(|installation| installation.valid),
        perfect_ready: perfect_workspace
            .installation
            .as_ref()
            .is_some_and(|installation| installation.valid),
        perfect_available_only: state.tray_perfect_available_only.load(Ordering::SeqCst),
    };
    let menu = Menu::new(app)?;
    menu.append(&MenuItem::with_id(
        app,
        "show",
        "打开 NEA",
        true,
        None::<&str>,
    )?)?;
    menu.append(&PredefinedMenuItem::separator(app)?)?;
    if runtime.busy {
        menu.append(&MenuItem::with_id(
            app,
            "switch-busy",
            "正在处理账号…",
            false,
            None::<&str>,
        )?)?;
        menu.append(&PredefinedMenuItem::separator(app)?)?;
    }

    let oopz_menu = Submenu::new(app, "OOPZ", !runtime.busy)?;
    let oopz_accounts = oopz_tray_accounts(&data, &runtime);
    if oopz_accounts.is_empty() {
        oopz_menu.append(&MenuItem::with_id(
            app,
            "oopz-empty",
            "暂无可切换账号",
            false,
            None::<&str>,
        )?)?;
    } else {
        for account in oopz_accounts {
            oopz_menu.append(&MenuItem::with_id(
                app,
                account.id,
                account.label,
                account.enabled,
                None::<&str>,
            )?)?;
        }
    }
    menu.append(&oopz_menu)?;

    let steam_menu = Submenu::new(app, "Steam", !runtime.busy)?;
    let steam_accounts = steam_tray_accounts(&data, &runtime);
    if steam_accounts.is_empty() {
        steam_menu.append(&MenuItem::with_id(
            app,
            "steam-empty",
            "暂无可切换账号",
            false,
            None::<&str>,
        )?)?;
    } else {
        for account in steam_accounts {
            steam_menu.append(&MenuItem::with_id(
                app,
                account.id,
                account.label,
                account.enabled,
                None::<&str>,
            )?)?;
        }
    }
    menu.append(&steam_menu)?;

    let perfect_menu = Submenu::with_id(app, "perfect-menu", "完美对战平台", !runtime.busy)?;
    perfect_menu.append(&CheckMenuItem::with_id(
        app,
        "perfect-available-only",
        "仅显示可用号",
        !runtime.busy,
        runtime.perfect_available_only,
        None::<&str>,
    )?)?;
    perfect_menu.append(&PredefinedMenuItem::separator(app)?)?;
    let perfect_accounts = perfect_tray_accounts(&data, &runtime);
    if perfect_accounts.is_empty() {
        perfect_menu.append(&MenuItem::with_id(
            app,
            "perfect-empty",
            if runtime.perfect_available_only {
                "暂无可用账号"
            } else {
                "暂无可切换账号"
            },
            false,
            None::<&str>,
        )?)?;
    } else {
        for rank in PerfectTrayRank::DISPLAY_ORDER {
            let rank_accounts = perfect_accounts
                .iter()
                .filter(|account| account.rank == rank)
                .collect::<Vec<_>>();
            if rank_accounts.is_empty() {
                continue;
            }
            let rank_current = rank_accounts.iter().any(|account| account.current);
            let rank_menu = Submenu::new(
                app,
                tray_current_label(rank.label().to_string(), rank_current),
                !runtime.busy,
            )?;
            for account in rank_accounts {
                let account_menu = Submenu::new(app, account.label.clone(), account.enabled)?;
                for action in &account.actions {
                    account_menu.append(&MenuItem::with_id(
                        app,
                        action.id.clone(),
                        action.label.clone(),
                        action.enabled,
                        None::<&str>,
                    )?)?;
                }
                rank_menu.append(&account_menu)?;
            }
            perfect_menu.append(&rank_menu)?;
        }
    }
    menu.append(&perfect_menu)?;
    menu.append(&PredefinedMenuItem::separator(app)?)?;
    menu.append(&MenuItem::with_id(
        app,
        "quit",
        "退出 NEA",
        true,
        None::<&str>,
    )?)?;
    Ok(BuiltTrayMenus {
        root: menu,
        perfect: perfect_menu,
    })
}

fn build_tray_menu(app: &AppHandle) -> tauri::Result<Menu<tauri::Wry>> {
    Ok(build_tray_menus(app)?.root)
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
    data.steam.current_account_id = apply_actual_steam_active_user(&mut data.steam.accounts);
    reconcile_steam_identities(&mut data);
    data.perfect_profiles.clear();
    Ok(data)
}

#[tauri::command]
async fn get_app_data(app: AppHandle) -> Result<AppData, String> {
    tauri::async_runtime::spawn_blocking(move || get_app_data_inner(&app))
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
fn get_config_health() -> Result<(), String> {
    if CONFIG_WRITES_BLOCKED.load(Ordering::SeqCst) {
        Err("NEA 配置文件损坏且无法自动恢复，已阻止写入以保护现有账号数据".to_string())
    } else {
        Ok(())
    }
}

#[tauri::command]
async fn get_steam_workspace(state: State<'_, AppState>) -> Result<steam::SteamWorkspace, String> {
    state
        .data
        .lock()
        .map(|data| {
            let mut workspace = data.steam.clone();
            workspace.current_account_id = apply_actual_steam_active_user(&mut workspace.accounts);
            workspace
        })
        .map_err(|error| error.to_string())
}

fn apply_actual_steam_active_user(accounts: &mut [steam::SteamAccount]) -> Option<String> {
    apply_steam_runtime_state(
        accounts,
        steam::SteamAdapter::client_is_running(),
        steam::SteamAdapter::active_user_account_id(),
    )
}

fn apply_steam_runtime_state(
    accounts: &mut [steam::SteamAccount],
    client_running: bool,
    active_account_id: Option<u32>,
) -> Option<String> {
    apply_steam_active_user(
        accounts,
        client_running.then_some(active_account_id).flatten(),
    )
}

fn apply_steam_active_user(
    accounts: &mut [steam::SteamAccount],
    active_account_id: Option<u32>,
) -> Option<String> {
    let mut current_steam_id = None;
    for account in accounts {
        let active = active_account_id.is_some()
            && steam::SteamAdapter::account_id32(&account.id) == active_account_id;
        account.most_recent = active;
        if active {
            current_steam_id = Some(account.id.clone());
        }
    }
    current_steam_id
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SteamLoginWaitOutcome {
    LoggedIn,
    ClientExited,
    TimedOut,
}

fn wait_for_actual_steam_login<F>(
    installation: &adapters::AppInstallation,
    target_steam_id: &str,
    timeout: Duration,
    mut on_wait: F,
) -> Result<SteamLoginWaitOutcome, String>
where
    F: FnMut(Duration),
{
    let adapter = steam::SteamAdapter;
    let started = Instant::now();
    let mut stable_polls = 0u8;
    let mut stopped_polls = 0u8;
    let mut ever_running = false;
    let mut last_progress_interval = 0u64;
    loop {
        let running = adapter.is_running(installation);
        ever_running |= running;
        let active = running && steam::SteamAdapter::is_account_active(target_steam_id);
        stable_polls = if active {
            stable_polls.saturating_add(1)
        } else {
            0
        };
        if stable_polls >= 4 {
            return Ok(SteamLoginWaitOutcome::LoggedIn);
        }
        stopped_polls = if running {
            0
        } else {
            stopped_polls.saturating_add(1)
        };
        let elapsed = started.elapsed();
        let progress_interval = elapsed.as_secs() / 15;
        if progress_interval > last_progress_interval {
            last_progress_interval = progress_interval;
            on_wait(elapsed);
        }
        if elapsed >= Duration::from_secs(20)
            && stopped_polls >= 40
            && (ever_running || elapsed >= Duration::from_secs(30))
        {
            return Ok(SteamLoginWaitOutcome::ClientExited);
        }
        if elapsed >= timeout {
            return Ok(SteamLoginWaitOutcome::TimedOut);
        }
        thread::sleep(Duration::from_millis(500));
    }
}

#[tauri::command]
async fn discover_steam(app: AppHandle) -> Result<steam::SteamWorkspace, String> {
    let native_switcher_exclusions = app
        .state::<AppState>()
        .data
        .lock()
        .map_err(|error| error.to_string())?
        .steam_native_switcher_exclusions
        .clone();
    let (installation, mut accounts) = tauri::async_runtime::spawn_blocking(move || {
        let installation = steam::SteamAdapter::discover_installation()?;
        if !steam::SteamAdapter::processes_are_running() {
            let _ = steam::SteamAdapter::suppress_accounts_from_native_switcher(
                &installation,
                &native_switcher_exclusions,
            );
        }
        let accounts = steam::SteamAdapter::read_accounts_stable(&installation)?;
        Ok::<_, String>((installation, accounts))
    })
    .await
    .map_err(|error| error.to_string())??;
    let display_names = stream::iter(
        accounts
            .iter()
            .filter(|account| steam_account_display_name_needs_refresh(account))
            .map(|account| account.id.clone())
            .collect::<HashSet<_>>(),
    )
    .map(|steam_id| {
        let app = app.clone();
        async move {
            resolve_steam_display_name(&app, &steam_id)
                .await
                .map(|display_name| (steam_id, display_name))
        }
    })
    .buffer_unordered(8)
    .filter_map(|resolved| async move { resolved })
    .collect::<HashMap<_, _>>()
    .await;
    for account in &mut accounts {
        if let Some(display_name) = display_names.get(&account.id) {
            account.display_name.clone_from(display_name);
        }
    }
    let state = app.state::<AppState>();
    let mut data = state.data.lock().map_err(|error| error.to_string())?;
    data.steam.installation = Some(installation);
    data.steam.accounts = accounts;
    data.steam.current_account_id = apply_actual_steam_active_user(&mut data.steam.accounts);
    reconcile_saved_steam_credentials(&mut data);
    reconcile_steam_identities(&mut data);
    save_data(&data)?;
    let workspace = data.steam.clone();
    drop(data);
    update_tray(&app);
    Ok(workspace)
}

#[tauri::command]
async fn refresh_steam_accounts(app: AppHandle) -> Result<steam::SteamWorkspace, String> {
    discover_steam(app).await
}

const STEAM_WEB_LOGIN_URL: &str =
    "https://store.steampowered.com/login/?redir=account%2F&redir_ssl=1";
const STEAM_WEB_ACCOUNT_URL: &str = "https://store.steampowered.com/account/";
const PERFECT_STEAM_OAUTH_URL: &str = "https://pvp.wanmei.com/csgo/pwaSteam";
const MAX_STEAM_TEXT_IMPORT_ACCOUNTS: usize = 100;
const MAX_PARALLEL_STEAM_IMPORT_WINDOWS: usize = 4;
const STEAM_VERIFICATION_WINDOW_TITLE: &str = "__NEA_STEAM_VERIFICATION_REQUIRED__";
const STEAM_INVALID_CREDENTIALS_WINDOW_TITLE: &str = "__NEA_STEAM_INVALID_CREDENTIALS__";
const STEAM_TOKEN_PROTECTED_WINDOW_TITLE: &str = "__NEA_STEAM_TOKEN_PROTECTED__";
const STEAM_VERIFICATION_URL_MARKER: &str = "__nea_steam_verification_required__";
const STEAM_INVALID_CREDENTIALS_URL_MARKER: &str = "__nea_steam_invalid_credentials__";
const STEAM_TOKEN_PROTECTED_URL_MARKER: &str = "__nea_steam_token_protected__";
const PERFECT_OAUTH_AUTOMATION_SCRIPT: &str = r#"
(() => {
  if (window.__neaPerfectOauthAutomation) return;
  window.__neaPerfectOauthAutomation = true;
  const accepted = new Set(['登录', '允许', 'sign in', 'allow']);
  const clickOfficialAction = () => {
    if (location.hostname.toLowerCase() !== 'store.steampowered.com') return;
    for (const element of document.querySelectorAll('button, input[type="submit"]')) {
      const label = String(element.innerText || element.value || '').trim().toLowerCase();
      if (!accepted.has(label) || element.disabled || element.dataset.neaClicked) continue;
      element.dataset.neaClicked = 'true';
      setTimeout(() => element.click(), 350);
      break;
    }
  };
  addEventListener('DOMContentLoaded', clickOfficialAction);
  setInterval(clickOfficialAction, 600);
})();
"#;
const PERFECT_OAUTH_LOOP_STOP_SCRIPT: &str = r#"
(() => {
  window.stop();
  document.open();
  document.write('<!doctype html><meta charset="utf-8"><title>NEA - 完美授权异常</title><body style="margin:0;background:#171d25;color:#d6d7d8;font:16px Microsoft YaHei UI,sans-serif;display:grid;place-items:center;min-height:100vh"><main style="text-align:center"><h2>完美平台未接受本次授权</h2><p>已停止重复授权，请返回 NEA 查看提示。</p></main></body>');
  document.close();
})();
"#;

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SteamCredentialInput {
    account: String,
    password: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SteamBulkImportResult {
    imported: usize,
    failed: usize,
    cancelled: usize,
    skipped_existing: usize,
    skipped_existing_accounts: Vec<String>,
    skipped_duplicate_input: usize,
    invalid_credential_accounts: Vec<String>,
    token_protected_accounts: Vec<String>,
    verification_required_accounts: Vec<String>,
    failed_accounts: Vec<String>,
    cancelled_accounts: Vec<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SteamCapabilityCompletionResult {
    checked: usize,
    processed: usize,
    already_complete: usize,
    web_completed: usize,
    cancelled: bool,
    verification_required_accounts: Vec<String>,
    failed_accounts: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct SteamCapabilityStatus {
    running: bool,
    paused: bool,
    cancelling: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct StorageOptimizationResult {
    before_bytes: u64,
    after_bytes: u64,
    freed_bytes: u64,
    optimized_sessions: usize,
    removed_orphan_sessions: usize,
    cached_perfect_avatars: usize,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SteamImportPreview {
    existing_accounts: Vec<String>,
    duplicate_input_accounts: Vec<String>,
}

fn normalized_steam_account_name(value: &str) -> String {
    value.trim().to_ascii_lowercase()
}

fn is_valid_steam_id64(value: &str) -> bool {
    value.len() == 17
        && value.starts_with("7656119")
        && value.chars().all(|character| character.is_ascii_digit())
}

fn steam64_for_account_name(data: &AppData, account_name: &str) -> Option<String> {
    let normalized = normalized_steam_account_name(account_name);
    data.steam
        .accounts
        .iter()
        .find(|account| {
            normalized_steam_account_name(&account.account_name) == normalized
                && is_valid_steam_id64(&account.id)
        })
        .map(|account| account.id.clone())
        .or_else(|| {
            data.steam.web_sessions.iter().find_map(|session| {
                let steam_id = session
                    .steam_id
                    .as_deref()
                    .filter(|id| is_valid_steam_id64(id))?;
                session
                    .account_name
                    .as_deref()
                    .is_some_and(|name| normalized_steam_account_name(name) == normalized)
                    .then(|| steam_id.to_string())
            })
        })
        .or_else(|| {
            data.steam_credentials.iter().find_map(|credential| {
                let steam_id = credential
                    .steam_id
                    .as_deref()
                    .filter(|id| is_valid_steam_id64(id))?;
                (normalized_steam_account_name(&credential.account_name) == normalized)
                    .then(|| steam_id.to_string())
            })
        })
}

fn has_verified_steam_web_login(
    data: &AppData,
    steam_id: &str,
    excluded_web_session_id: Option<&str>,
) -> bool {
    is_valid_steam_id64(steam_id)
        && data.steam.web_sessions.iter().any(|session| {
            session.id != excluded_web_session_id.unwrap_or_default()
                && session.steam_id.as_deref() == Some(steam_id)
                && session.steam_id.as_deref().is_some_and(is_valid_steam_id64)
        })
}

fn steam_import_duplicate_id(data: &AppData, account_name: &str) -> Option<String> {
    let steam_id = steam64_for_account_name(data, account_name)?;
    has_verified_steam_web_login(data, &steam_id, None).then_some(steam_id)
}

fn ensure_steam_identity<'a>(
    identities: &'a mut HashMap<String, SteamIdentity>,
    existing: &HashMap<String, SteamIdentity>,
    id: String,
    steam_id: Option<String>,
    account_name: Option<String>,
    display_name: String,
) -> &'a mut SteamIdentity {
    identities.entry(id.clone()).or_insert_with(|| {
        let previous = existing.get(&id);
        let incoming_is_fallback = display_name.trim().is_empty()
            || steam_id.as_deref() == Some(display_name.trim())
            || account_name
                .as_deref()
                .is_some_and(|name| display_name.trim().eq_ignore_ascii_case(name.trim()))
            || display_name == "待登录网页账号";
        let display_name = previous
            .filter(|_| incoming_is_fallback)
            .map(|identity| identity.display_name.trim())
            .filter(|previous_name| {
                !previous_name.is_empty()
                    && steam_id.as_deref() != Some(*previous_name)
                    && !account_name
                        .as_deref()
                        .is_some_and(|name| previous_name.eq_ignore_ascii_case(name.trim()))
                    && *previous_name != "待登录网页账号"
            })
            .map(str::to_string)
            .unwrap_or(display_name);
        SteamIdentity {
            id,
            steam_id,
            account_name,
            display_name,
            note: previous.and_then(|identity| identity.note.clone()),
            client_account_id: None,
            web_session_id: None,
            capabilities: SteamIdentityCapabilities::default(),
            created_at: previous
                .map(|identity| identity.created_at.clone())
                .unwrap_or_else(now),
            updated_at: previous
                .map(|identity| identity.updated_at.clone())
                .unwrap_or_else(now),
        }
    })
}

fn reconcile_steam_identities(data: &mut AppData) {
    let existing = data
        .steam_identities
        .iter()
        .map(|identity| (identity.id.clone(), identity.clone()))
        .collect::<HashMap<_, _>>();
    let mut account_to_steam_id = HashMap::<String, String>::new();
    for account in &data.steam.accounts {
        if is_valid_steam_id64(&account.id) {
            account_to_steam_id.insert(
                normalized_steam_account_name(&account.account_name),
                account.id.clone(),
            );
        }
    }
    for session in &data.steam.web_sessions {
        if let (Some(account_name), Some(steam_id)) = (&session.account_name, &session.steam_id) {
            if is_valid_steam_id64(steam_id) {
                account_to_steam_id.insert(
                    normalized_steam_account_name(account_name),
                    steam_id.clone(),
                );
            }
        }
    }
    for credential in &data.steam_credentials {
        if let Some(steam_id) = &credential.steam_id {
            if is_valid_steam_id64(steam_id) {
                account_to_steam_id.insert(
                    normalized_steam_account_name(&credential.account_name),
                    steam_id.clone(),
                );
            }
        }
    }

    let mut identities = HashMap::<String, SteamIdentity>::new();

    for account in &data.steam.accounts {
        let steam_id = is_valid_steam_id64(&account.id).then(|| account.id.clone());
        let id = steam_id.clone().unwrap_or_else(|| {
            format!(
                "pending:{}",
                normalized_steam_account_name(&account.account_name)
            )
        });
        let identity = ensure_steam_identity(
            &mut identities,
            &existing,
            id,
            steam_id,
            Some(account.account_name.clone()),
            account.display_name.clone(),
        );
        identity.client_account_id = Some(account.id.clone());
        if identity.note.is_none() {
            identity.note.clone_from(&account.note);
        }
    }
    for session in &data.steam.web_sessions {
        let Some(linked_steam_id) = session
            .steam_id
            .clone()
            .filter(|steam_id| is_valid_steam_id64(steam_id))
            .or_else(|| {
                session
                    .account_name
                    .as_deref()
                    .and_then(|name| account_to_steam_id.get(&normalized_steam_account_name(name)))
                    .cloned()
            })
        else {
            continue;
        };
        let id = linked_steam_id.clone();
        let identity = ensure_steam_identity(
            &mut identities,
            &existing,
            id,
            Some(linked_steam_id),
            session.account_name.clone(),
            session.display_name.clone(),
        );
        identity.web_session_id = Some(session.id.clone());
        identity.capabilities.web_login =
            session.steam_id.as_deref().is_some_and(is_valid_steam_id64);
        if identity.account_name.is_none() {
            identity.account_name.clone_from(&session.account_name);
        }
        if session.note.is_some() {
            identity.note.clone_from(&session.note);
        }
        if identity.client_account_id.is_none()
            && !session.display_name.trim().is_empty()
            && session.display_name != "待登录网页账号"
        {
            identity.display_name.clone_from(&session.display_name);
        }
    }
    for credential in &data.steam_credentials {
        let linked_steam_id = credential
            .steam_id
            .clone()
            .filter(|steam_id| is_valid_steam_id64(steam_id))
            .or_else(|| {
                account_to_steam_id
                    .get(&normalized_steam_account_name(&credential.account_name))
                    .cloned()
            });
        let normalized = normalized_steam_account_name(&credential.account_name);
        let identity_key = linked_steam_id.clone().or_else(|| {
            identities.iter().find_map(|(key, identity)| {
                identity
                    .account_name
                    .as_deref()
                    .is_some_and(|name| normalized_steam_account_name(name) == normalized)
                    .then(|| key.clone())
            })
        });
        let id = identity_key.unwrap_or_else(|| format!("pending:{normalized}"));
        let identity = ensure_steam_identity(
            &mut identities,
            &existing,
            id,
            linked_steam_id,
            Some(credential.account_name.clone()),
            credential.account_name.clone(),
        );
        identity.capabilities.credential = true;
        if identity.account_name.is_none() {
            identity.account_name = Some(credential.account_name.clone());
        }
    }
    for (steam_id, profile) in &data.perfect_profiles {
        if !is_valid_steam_id64(steam_id) {
            continue;
        }
        if let Some(identity) = identities.get_mut(steam_id) {
            identity.capabilities.perfect_profile = profile.found;
        }
    }
    let mut result = identities
        .into_values()
        .filter(|identity| identity.capabilities.web_login || identity.capabilities.credential)
        .collect::<Vec<_>>();
    result.sort_by(|left, right| {
        left.steam_id
            .is_none()
            .cmp(&right.steam_id.is_none())
            .then_with(|| left.display_name.cmp(&right.display_name))
            .then_with(|| left.id.cmp(&right.id))
    });
    data.steam_identities = result;
}

fn steam_import_preview(data: &AppData, accounts: &[String]) -> SteamImportPreview {
    let mut seen = HashSet::new();
    let mut existing_accounts = Vec::new();
    let mut duplicate_input_accounts = Vec::new();
    for account in accounts {
        let normalized = normalized_steam_account_name(account);
        let resolved_steam_id = steam64_for_account_name(data, account);
        let duplicate_key = resolved_steam_id
            .as_deref()
            .filter(|steam_id| is_valid_steam_id64(steam_id))
            .map(|steam_id| format!("steam:{steam_id}"))
            .unwrap_or_else(|| format!("account:{normalized}"));
        if !seen.insert(duplicate_key) {
            duplicate_input_accounts.push(account.trim().to_string());
        } else if resolved_steam_id
            .as_deref()
            .is_some_and(|steam_id| has_verified_steam_web_login(data, steam_id, None))
        {
            existing_accounts.push(account.trim().to_string());
        }
    }
    SteamImportPreview {
        existing_accounts,
        duplicate_input_accounts,
    }
}

fn insert_missing_steam_credentials(
    data: &mut AppData,
    credentials: &[SteamCredentialInput],
    steam_ids_with_saved_credentials: &HashSet<String>,
) {
    reconcile_steam_identities(data);
    for input in credentials {
        let normalized = normalized_steam_account_name(&input.account);
        let matching_steam_id = steam64_for_account_name(data, &input.account);
        if matching_steam_id
            .as_ref()
            .is_some_and(|steam_id| steam_ids_with_saved_credentials.contains(steam_id))
            || data
                .steam_credentials
                .iter()
                .any(|saved| normalized_steam_account_name(&saved.account_name) == normalized)
        {
            continue;
        }
        data.steam_credentials.push(SteamSavedCredential {
            account_name: input.account.trim().to_string(),
            password: input.password.clone(),
            steam_id: matching_steam_id,
            updated_at: now(),
        });
    }
}

fn credential_for_steam_identity<'a>(
    data: &'a AppData,
    steam_id: Option<&str>,
    account_name: Option<&str>,
) -> Option<&'a SteamSavedCredential> {
    data.steam_credentials.iter().find(|credential| {
        steam_id.is_some() && credential.steam_id.as_deref() == steam_id
            || account_name.is_some_and(|name| {
                normalized_steam_account_name(&credential.account_name)
                    == normalized_steam_account_name(name)
            })
    })
}

fn established_account_names_for_steam_id(data: &AppData, steam_id: &str) -> HashSet<String> {
    data.steam
        .accounts
        .iter()
        .filter(|account| account.id == steam_id)
        .map(|account| normalized_steam_account_name(&account.account_name))
        .chain(
            data.steam
                .web_sessions
                .iter()
                .filter(|session| session.steam_id.as_deref() == Some(steam_id))
                .filter_map(|session| session.account_name.as_deref())
                .map(normalized_steam_account_name),
        )
        .collect()
}

fn has_saved_credential_for_steam_id(data: &AppData, steam_id: &str) -> bool {
    let established_names = established_account_names_for_steam_id(data, steam_id);
    data.steam_credentials.iter().any(|credential| {
        credential.steam_id.as_deref() == Some(steam_id)
            || established_names.contains(&normalized_steam_account_name(&credential.account_name))
    })
}

fn deduplicate_credentials_for_steam_id(data: &mut AppData, steam_id: &str) {
    let established_names = established_account_names_for_steam_id(data, steam_id);
    let preferred_index = data
        .steam_credentials
        .iter()
        .enumerate()
        .find(|(_, credential)| {
            credential.steam_id.as_deref() == Some(steam_id)
                && established_names
                    .contains(&normalized_steam_account_name(&credential.account_name))
        })
        .map(|(index, _)| index)
        .or_else(|| {
            data.steam_credentials
                .iter()
                .position(|credential| credential.steam_id.as_deref() == Some(steam_id))
        });
    let Some(preferred_index) = preferred_index else {
        return;
    };
    let mut index = 0usize;
    data.steam_credentials.retain(|credential| {
        let keep = credential.steam_id.as_deref() != Some(steam_id) || index == preferred_index;
        index += 1;
        keep
    });
}

fn bind_credentials_to_steam_id(
    data: &mut AppData,
    steam_id: &str,
    verified_account_name: Option<&str>,
) {
    let established_names = established_account_names_for_steam_id(data, steam_id);
    for credential in &mut data.steam_credentials {
        let normalized = normalized_steam_account_name(&credential.account_name);
        let matches_verified_account = verified_account_name
            .is_some_and(|account_name| normalized == normalized_steam_account_name(account_name));
        if matches_verified_account
            || credential.steam_id.as_deref() == Some(steam_id)
            || established_names.contains(&normalized)
        {
            credential.steam_id = Some(steam_id.to_string());
            credential.updated_at = now();
        }
    }
}

fn reconcile_saved_steam_credentials(data: &mut AppData) {
    let mut valid_steam_ids = HashSet::<String>::new();
    let mut name_to_steam_id = HashMap::<String, String>::new();
    for account in &data.steam.accounts {
        let normalized = normalized_steam_account_name(&account.account_name);
        if is_valid_steam_id64(&account.id) {
            valid_steam_ids.insert(account.id.clone());
            name_to_steam_id.insert(normalized, account.id.clone());
        }
    }
    for session in &data.steam.web_sessions {
        let Some(steam_id) = session
            .steam_id
            .as_deref()
            .filter(|steam_id| is_valid_steam_id64(steam_id))
        else {
            continue;
        };
        valid_steam_ids.insert(steam_id.to_string());
        if let Some(account_name) = session.account_name.as_deref() {
            let normalized = normalized_steam_account_name(account_name);
            name_to_steam_id.insert(normalized, steam_id.to_string());
        }
    }
    for credential in &mut data.steam_credentials {
        if let Some(steam_id) =
            name_to_steam_id.get(&normalized_steam_account_name(&credential.account_name))
        {
            credential.steam_id = Some(steam_id.clone());
        }
    }
    for steam_id in valid_steam_ids {
        deduplicate_credentials_for_steam_id(data, &steam_id);
    }
}

fn steam_identity_for_account_name(
    data: &mut AppData,
    account_name: &str,
) -> Option<SteamIdentity> {
    reconcile_steam_identities(data);
    let normalized = normalized_steam_account_name(account_name);
    data.steam_identities
        .iter()
        .find(|identity| {
            identity
                .account_name
                .as_deref()
                .is_some_and(|name| normalized_steam_account_name(name) == normalized)
        })
        .cloned()
}

fn steam_credential_automation_script(
    credentials: &SteamCredentialInput,
) -> Result<String, String> {
    let encoded = general_purpose::STANDARD.encode(
        serde_json::to_vec(credentials)
            .map_err(|error| format!("编码 Steam 登录信息失败: {error}"))?,
    );
    Ok(format!(
        r#"
(() => {{
  if (window.__neaSteamCredentialAutomation) return;
  window.__neaSteamCredentialAutomation = true;
  const raw = Uint8Array.from(atob('{encoded}'), c => c.charCodeAt(0));
  const credentials = JSON.parse(new TextDecoder().decode(raw));
  let submitted = false;
  const verificationTitle = '{verification_title}';
  const invalidCredentialsTitle = '{invalid_credentials_title}';
  const tokenProtectedTitle = '{token_protected_title}';
  const verificationMarker = '{verification_marker}';
  const invalidCredentialsMarker = '{invalid_credentials_marker}';
  const tokenProtectedMarker = '{token_protected_marker}';
  const markUrlState = marker => {{
    if (window.top !== window || location.hash === `#${{marker}}`) return;
    try {{
      history.replaceState(history.state, '', `${{location.pathname}}${{location.search}}#${{marker}}`);
    }} catch (_) {{
      location.hash = marker;
    }}
  }};
  const markImportState = (title, marker) => {{
    document.title = title;
    markUrlState(marker);
  }};
  const visible = element => {{
    if (!(element instanceof HTMLElement)) return false;
    const style = getComputedStyle(element);
    const rect = element.getBoundingClientRect();
    return style.display !== 'none' && style.visibility !== 'hidden' &&
      Number(style.opacity || '1') > 0 && rect.width > 0 && rect.height > 0;
  }};
  let lastQrRetryClick = 0;
  let qrRetryCheckQueued = false;
  const qrLoginPhrases = [
    '通过二维码使用 steam 手机应用登录',
    '或者用二维码登录',
    '使用 steam 手机应用扫码登录',
    'sign in with a qr code',
    'use the steam mobile app to sign in via qr code'
  ];
  const qrRetryPhrases = ['重试', '刷新', 'retry', 'refresh', 'reload'];
  const refreshExpiredQrCode = () => {{
    if (window.top !== window || location.hostname.toLowerCase() !== 'store.steampowered.com') return;
    const anchors = [...document.querySelectorAll('div, span, p')]
      .filter(element => {{
        if (!visible(element)) return false;
        const text = String(element.textContent || '').replace(/\s+/g, ' ').trim().toLowerCase();
        return text.length > 0 && text.length <= 180 &&
          qrLoginPhrases.some(phrase => text.includes(phrase));
      }})
      .sort((left, right) =>
        String(left.textContent || '').length - String(right.textContent || '').length
      );
    const anchor = anchors[0];
    if (!anchor) return;
    const anchorRect = anchor.getBoundingClientRect();
    let container = anchor.parentElement;
    for (let depth = 0; container && depth < 7; depth += 1, container = container.parentElement) {{
      const candidates = [...container.querySelectorAll('button, [role="button"], a, [tabindex], div')]
        .filter(element => {{
          if (!visible(element) || element === anchor || element.contains(anchor)) return false;
          const rect = element.getBoundingClientRect();
          const label = `${{element.getAttribute('aria-label') || ''}} ${{element.getAttribute('title') || ''}} ${{element.textContent || ''}}`
            .replace(/\s+/g, ' ').trim().toLowerCase();
          const explicitRetry = qrRetryPhrases.some(phrase => label.includes(phrase));
          const pointerControl = element.tagName === 'BUTTON' ||
            element.getAttribute('role') === 'button' || getComputedStyle(element).cursor === 'pointer';
          const squareControl = rect.width >= 32 && rect.width <= 140 &&
            rect.height >= 32 && rect.height <= 140 && Math.abs(rect.width - rect.height) <= 36;
          const centeredOnQr = Math.abs(
            (rect.left + rect.width / 2) - (anchorRect.left + anchorRect.width / 2)
          ) <= 180 && rect.bottom <= anchorRect.top + 36;
          return pointerControl && squareControl && centeredOnQr &&
            (explicitRetry || element.querySelector('svg, img'));
        }})
        .sort((left, right) => {{
          const leftRect = left.getBoundingClientRect();
          const rightRect = right.getBoundingClientRect();
          const leftDistance = Math.abs((leftRect.left + leftRect.width / 2) -
            (anchorRect.left + anchorRect.width / 2));
          const rightDistance = Math.abs((rightRect.left + rightRect.width / 2) -
            (anchorRect.left + anchorRect.width / 2));
          return leftDistance - rightDistance || leftRect.width - rightRect.width;
        }});
      const candidate = candidates[0];
      if (!candidate) continue;
      const currentTime = Date.now();
      if (currentTime - lastQrRetryClick < 600) return;
      lastQrRetryClick = currentTime;
      candidate.click();
      return;
    }}
  }};
  const scheduleQrRetryCheck = () => {{
    if (qrRetryCheckQueued) return;
    qrRetryCheckQueued = true;
    queueMicrotask(() => {{
      qrRetryCheckQueued = false;
      refreshExpiredQrCode();
    }});
  }};
  const observeQrRetry = () => {{
    scheduleQrRetryCheck();
    if (!document.documentElement) return;
    new MutationObserver(scheduleQrRetryCheck).observe(document.documentElement, {{
      subtree: true,
      childList: true,
      attributes: true,
      attributeFilter: ['class', 'style', 'aria-label', 'title']
    }});
  }};
  const setValue = (input, value) => {{
    const setter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').set;
    setter.call(input, value);
    input.dispatchEvent(new Event('input', {{ bubbles: true }}));
    input.dispatchEvent(new Event('change', {{ bubbles: true }}));
  }};
  const fillAndSubmit = () => {{
    if (submitted || window.top !== window ||
        location.hostname.toLowerCase() !== 'store.steampowered.com' ||
        !location.pathname.toLowerCase().startsWith('/login')) return;
    const password = [...document.querySelectorAll('input[type="password"]')].find(visible);
    if (!password) return;
    let loginRoot = password.closest('form');
    if (!loginRoot) {{
      let candidate = password.parentElement;
      while (candidate && candidate !== document.body) {{
        const visibleTextInputs = [...candidate.querySelectorAll(
          'input[type="text"], input[type="email"], input:not([type])'
        )].filter(visible);
        const visiblePasswords = [...candidate.querySelectorAll('input[type="password"]')]
          .filter(visible);
        if (visibleTextInputs.length === 1 && visiblePasswords.length === 1) {{
          loginRoot = candidate;
          break;
        }}
        candidate = candidate.parentElement;
      }}
    }}
    if (!loginRoot) return;
    const textInputs = [...loginRoot.querySelectorAll(
      'input[autocomplete="username"], input[name*="account" i], input[name*="user" i], input[type="text"], input[type="email"], input:not([type])'
    )].filter(visible);
    const account = textInputs.find(input => input !== password);
    if (!account || !loginRoot.contains(password)) return;
    const submit = [...loginRoot.querySelectorAll('button, input[type="submit"]')].find(element => {{
      if (!visible(element) || element.disabled) return false;
      const text = String(element.innerText || element.value || '').trim().toLowerCase();
      return text === '登录' || text === 'sign in';
    }});
    if (!submit) return;
    account.setAttribute('autocomplete', 'off');
    password.setAttribute('autocomplete', 'off');
    setValue(account, credentials.account);
    setValue(password, credentials.password);
    submitted = true;
    setTimeout(() => submit.click(), 500);
  }};
  const detectVerificationChallenge = () => {{
    if (location.hostname.toLowerCase() !== 'store.steampowered.com' ||
        document.title === tokenProtectedTitle || document.title === invalidCredentialsTitle) return;
    const text = String(document.body?.innerText || '').replace(/\s+/g, ' ').toLowerCase();
    const phrases = [
      '我们已将验证码发送至您的电子邮件',
      '输入我们发送到您电子邮件的代码',
      'we sent a code to your email',
      'enter the code we sent to your email'
    ];
    const oneTimeCode = [...document.querySelectorAll('input')].some(input =>
      visible(input) && (
        input.autocomplete === 'one-time-code' ||
        /auth|guard|twofactor|code/i.test(`${{input.name}} ${{input.id}}`)
      )
    );
    if (phrases.some(phrase => text.includes(phrase)) ||
        (oneTimeCode && (text.includes('电子邮件') || text.includes('email')))) {{
      markImportState(verificationTitle, verificationMarker);
    }}
  }};
  const detectTokenProtection = () => {{
    if (location.hostname.toLowerCase() !== 'store.steampowered.com' ||
        document.title === invalidCredentialsTitle || document.title === verificationTitle) return;
    const text = String(document.body?.innerText || '').replace(/\s+/g, ' ').toLowerCase();
    const phrases = [
      '此账户受到手机验证器保护',
      '此帐户受到手机验证器保护',
      '输入您 steam 手机应用上的代码',
      '输入您的 steam 令牌验证码',
      '使用 steam 手机应用来确认登录',
      '使用 steam 手机应用确认登录',
      'this account is protected by a steam guard mobile authenticator',
      'enter the code from your steam mobile app',
      'use the steam mobile app to confirm your sign in',
      'approve the sign in request in the steam mobile app'
    ];
    if (phrases.some(phrase => text.includes(phrase))) {{
      markImportState(tokenProtectedTitle, tokenProtectedMarker);
    }}
  }};
  const detectInvalidCredentials = () => {{
    if (!submitted || location.hostname.toLowerCase() !== 'store.steampowered.com' ||
        document.title === tokenProtectedTitle || document.title === verificationTitle) return;
    const text = String(document.body?.innerText || '').replace(/\s+/g, ' ').toLowerCase();
    const phrases = [
      '请核对您的密码和帐户名称并重试',
      '请核对您的密码和账户名称并重试',
      '帐户名称或密码不正确',
      '账户名称或密码不正确',
      'please check your password and account name and try again',
      'the account name or password that you have entered is incorrect',
      'incorrect account name or password'
    ];
    if (phrases.some(phrase => text.includes(phrase))) {{
      markImportState(invalidCredentialsTitle, invalidCredentialsMarker);
    }}
  }};
  addEventListener('DOMContentLoaded', fillAndSubmit);
  addEventListener('DOMContentLoaded', observeQrRetry);
  addEventListener('DOMContentLoaded', detectVerificationChallenge);
  addEventListener('DOMContentLoaded', detectTokenProtection);
  addEventListener('DOMContentLoaded', detectInvalidCredentials);
  setInterval(fillAndSubmit, 400);
  setInterval(refreshExpiredQrCode, 250);
  setInterval(detectVerificationChallenge, 250);
  setInterval(detectTokenProtection, 180);
  setInterval(detectInvalidCredentials, 200);
}})();
"#,
        verification_title = STEAM_VERIFICATION_WINDOW_TITLE,
        invalid_credentials_title = STEAM_INVALID_CREDENTIALS_WINDOW_TITLE,
        token_protected_title = STEAM_TOKEN_PROTECTED_WINDOW_TITLE,
        verification_marker = STEAM_VERIFICATION_URL_MARKER,
        invalid_credentials_marker = STEAM_INVALID_CREDENTIALS_URL_MARKER,
        token_protected_marker = STEAM_TOKEN_PROTECTED_URL_MARKER
    ))
}

fn clear_sensitive_string(value: &mut String) {
    let byte_length = value.len();
    if byte_length > 0 {
        value.replace_range(.., &"\0".repeat(byte_length));
        value.clear();
    }
}

fn steam_web_session_dir(session_id: &str) -> Result<PathBuf, String> {
    if session_id.is_empty()
        || !session_id
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '-')
    {
        return Err("Steam 网页会话 ID 无效".to_string());
    }
    Ok(storage_dir()?
        .join("workspaces")
        .join("steam")
        .join("web-sessions")
        .join(session_id))
}

const STEAM_WEBVIEW_CACHE_PATHS: &[&str] = &[
    r"EBWebView\Default\Cache",
    r"EBWebView\Default\Code Cache",
    r"EBWebView\Default\GPUCache",
    r"EBWebView\Default\DawnWebGPUCache",
    r"EBWebView\Default\DawnGraphiteCache",
    r"EBWebView\GrShaderCache",
    r"EBWebView\ShaderCache",
    r"EBWebView\BrowserMetrics",
    r"EBWebView\Crashpad",
    r"EBWebView\component_crx_cache",
    r"EBWebView\extensions_crx_cache",
    r"EBWebView\GPUPersistentCache",
    r"EBWebView\Subresource Filter",
    r"EBWebView\Speech Recognition",
    r"EBWebView\hyphen-data",
];

fn path_size(path: &Path) -> u64 {
    if fs::symlink_metadata(path).is_ok_and(|metadata| metadata.file_type().is_symlink()) {
        return 0;
    }
    if path.is_file() {
        return fs::metadata(path)
            .map(|metadata| metadata.len())
            .unwrap_or(0);
    }
    fs::read_dir(path)
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| path_size(&entry.path()))
        .sum()
}

fn remove_path_with_retries(path: &Path) -> Result<(), String> {
    let mut last_error = None;
    for _ in 0..8 {
        let result = if path.is_dir() {
            fs::remove_dir_all(path)
        } else {
            fs::remove_file(path)
        };
        match result {
            Ok(()) => return Ok(()),
            Err(error) if error.kind() == ErrorKind::NotFound => return Ok(()),
            Err(error) => {
                last_error = Some(error);
                thread::sleep(Duration::from_millis(250));
            }
        }
    }
    Err(format!(
        "清理缓存失败: {}: {}",
        path.display(),
        last_error
            .map(|error| error.to_string())
            .unwrap_or_else(|| "未知错误".to_string())
    ))
}

fn cleanup_steam_webview_cache_at(profile_root: &Path) -> u64 {
    let mut freed = 0u64;
    for relative in STEAM_WEBVIEW_CACHE_PATHS {
        let target = profile_root.join(relative);
        if !target.starts_with(profile_root) || !target.exists() {
            continue;
        }
        if fs::symlink_metadata(&target).is_ok_and(|metadata| metadata.file_type().is_symlink()) {
            continue;
        }
        let size = path_size(&target);
        if remove_path_with_retries(&target).is_ok() {
            freed = freed.saturating_add(size);
        }
    }
    freed
}

fn cleanup_steam_webview_cache(session_id: &str) -> Result<u64, String> {
    let profile_root = steam_web_session_dir(session_id)?.join("webview2");
    Ok(cleanup_steam_webview_cache_at(&profile_root))
}

fn schedule_steam_webview_cache_cleanup(session_id: String) {
    thread::spawn(move || {
        thread::sleep(Duration::from_millis(1200));
        let _ = cleanup_steam_webview_cache(&session_id);
    });
}

fn cleanup_orphan_perfect_avatars(saved_ids: &HashSet<String>) {
    let Ok(root) = perfect_avatar_dir() else {
        return;
    };
    for entry in fs::read_dir(&root).into_iter().flatten().flatten() {
        let path = entry.path();
        let Some(stem) = path.file_stem().and_then(|value| value.to_str()) else {
            continue;
        };
        if path.is_file() && !saved_ids.contains(stem) {
            let _ = fs::remove_file(path);
        }
    }
}

fn optimize_storage_inner(app: &AppHandle) -> Result<StorageOptimizationResult, String> {
    let root = storage_dir()?;
    let before_bytes = path_size(&root);
    let (saved_sessions, saved_profile_ids, cached_perfect_avatars) = {
        let state = app.state::<AppState>();
        let mut data = state.data.lock().map_err(|error| error.to_string())?;
        externalize_perfect_profile_avatars(&mut data);
        // A second atomic save also replaces a pre-migration oversized backup
        // with the already externalized compact configuration.
        save_data(&data)?;
        (
            data.steam
                .web_sessions
                .iter()
                .map(|session| session.id.clone())
                .collect::<HashSet<_>>(),
            data.perfect_profiles
                .keys()
                .cloned()
                .collect::<HashSet<_>>(),
            data.perfect_profiles
                .keys()
                .filter(|steam_id| perfect_avatar_path(steam_id).is_ok_and(|path| path.is_file()))
                .count(),
        )
    };
    let sessions_root = root.join("workspaces").join("steam").join("web-sessions");
    let mut optimized_sessions = 0usize;
    let mut removed_orphan_sessions = 0usize;
    for entry in fs::read_dir(&sessions_root).into_iter().flatten().flatten() {
        let path = entry.path();
        let is_real_directory = entry
            .file_type()
            .is_ok_and(|file_type| file_type.is_dir() && !file_type.is_symlink());
        if !is_real_directory || !path.starts_with(&sessions_root) {
            continue;
        }
        let Some(session_id) = entry.file_name().to_str().map(str::to_string) else {
            continue;
        };
        if session_id.starts_with("share-stage-") {
            continue;
        }
        if app
            .get_webview_window(&steam_web_window_label(&session_id))
            .is_some()
        {
            continue;
        }
        if saved_sessions.contains(&session_id) {
            let _ = cleanup_steam_webview_cache(&session_id);
            optimized_sessions += 1;
        } else if valid_storage_id(&session_id) && remove_path_with_retries(&path).is_ok() {
            removed_orphan_sessions += 1;
        }
    }
    cleanup_orphan_perfect_avatars(&saved_profile_ids);
    let after_bytes = path_size(&root);
    Ok(StorageOptimizationResult {
        before_bytes,
        after_bytes,
        freed_bytes: before_bytes.saturating_sub(after_bytes),
        optimized_sessions,
        removed_orphan_sessions,
        cached_perfect_avatars,
    })
}

#[tauri::command]
async fn optimize_storage(app: AppHandle) -> Result<StorageOptimizationResult, String> {
    let _steam_operation = acquire_steam_web_import(&app)?;
    tauri::async_runtime::spawn_blocking(move || optimize_storage_inner(&app))
        .await
        .map_err(|error| error.to_string())?
}

fn schedule_storage_maintenance(app: AppHandle) {
    tauri::async_runtime::spawn_blocking(move || {
        thread::sleep(Duration::from_secs(2));
        let Ok(_steam_operation) = acquire_steam_web_import(&app) else {
            return;
        };
        if let Ok(result) = optimize_storage_inner(&app) {
            let _ = app.emit("storage-optimized", result);
        }
    });
}

fn steam_web_window_label(session_id: &str) -> String {
    format!("steam-web-{}", session_id)
}

fn build_steam_web_window(
    app: &AppHandle,
    session_id: &str,
    visible: bool,
    auto_complete: bool,
    credentials: Option<&SteamCredentialInput>,
) -> Result<WebviewWindow, String> {
    let skip_complete_duplicates = credentials.is_some();
    let label = steam_web_window_label(session_id);
    let url = tauri::Url::parse(STEAM_WEB_LOGIN_URL).map_err(|error| error.to_string())?;
    let data_dir = steam_web_session_dir(session_id)?.join("webview2");
    fs::create_dir_all(&data_dir)
        .map_err(|error| format!("创建 Steam 网页会话目录失败: {}", error))?;
    let mut builder = WebviewWindowBuilder::new(app, &label, WebviewUrl::External(url))
        .title("NEA - Steam 网页账号")
        .data_directory(data_dir)
        .additional_browser_args("--disk-cache-size=1048576 --media-cache-size=1048576")
        .inner_size(1180.0, 780.0)
        .min_inner_size(760.0, 520.0)
        .visible(visible)
        .center();
    if let Some(credentials) = credentials {
        builder = builder.initialization_script(steam_credential_automation_script(credentials)?);
        builder = builder.on_document_title_changed(|window, document_title| {
            if steam_import_outcome_from_markers(Some(&document_title), None).is_some() {
                let _ = window.set_title(&document_title);
            }
        });
    }
    if auto_complete {
        let session_id = session_id.to_string();
        let checking = Arc::new(AtomicBool::new(false));
        builder = builder.on_page_load(move |window, payload| {
            if payload.event() != PageLoadEvent::Finished
                || payload.url().host_str() != Some("store.steampowered.com")
                || checking.swap(true, Ordering::SeqCst)
            {
                return;
            }
            let session_id = session_id.clone();
            let checking = checking.clone();
            tauri::async_runtime::spawn(async move {
                tokio::time::sleep(Duration::from_millis(250)).await;
                let app = window.app_handle().clone();
                match steam_id_from_web_window(window.clone()).await {
                    Ok(Some(steam_id)) => {
                        let display_name = resolve_steam_display_name(&app, &steam_id).await;
                        match persist_verified_steam_web_session(
                            &app,
                            &session_id,
                            &steam_id,
                            display_name.as_deref(),
                            skip_complete_duplicates,
                        ) {
                            Ok(VerifiedSteamWebSessionPersist::Saved(display_name)) => {
                                let _ = app.emit("app-data-changed", ());
                                let _ = app.emit("steam-web-session-verified", display_name);
                                let _ = window.destroy();
                            }
                            Ok(VerifiedSteamWebSessionPersist::DuplicateSkipped(display_name)) => {
                                let _ = app.emit("app-data-changed", ());
                                let _ = app.emit("steam-web-session-duplicate", display_name);
                                let _ = window.destroy();
                            }
                            Err(error) => {
                                let _ = app.emit("steam-web-session-error", error);
                                checking.store(false, Ordering::SeqCst);
                            }
                        }
                    }
                    Ok(None) => checking.store(false, Ordering::SeqCst),
                    Err(error) => {
                        let _ = app.emit("steam-web-session-error", error);
                        checking.store(false, Ordering::SeqCst);
                    }
                }
            });
        });
    }
    let window = builder
        .build()
        .map_err(|error| format!("打开 Steam 网页账号窗口失败: {}", error))?;
    let cleanup_session_id = session_id.to_string();
    window.on_window_event(move |event| {
        if matches!(event, WindowEvent::Destroyed) {
            schedule_steam_webview_cache_cleanup(cleanup_session_id.clone());
        }
    });
    Ok(window)
}

fn open_steam_web_window(app: &AppHandle, session_id: &str) -> Result<(), String> {
    let label = steam_web_window_label(session_id);
    let url = tauri::Url::parse(STEAM_WEB_ACCOUNT_URL).map_err(|error| error.to_string())?;
    if let Some(window) = app.get_webview_window(&label) {
        window.navigate(url).map_err(|error| error.to_string())?;
        window.show().map_err(|error| error.to_string())?;
        window.set_focus().map_err(|error| error.to_string())?;
        return Ok(());
    }
    build_steam_web_window(app, session_id, true, false, None).map(|_| ())
}

fn build_perfect_oauth_window(
    app: &AppHandle,
    session_id: &str,
) -> Result<(WebviewWindow, Arc<AtomicBool>), String> {
    let label = steam_web_window_label(session_id);
    let url = tauri::Url::parse(PERFECT_STEAM_OAUTH_URL).map_err(|error| error.to_string())?;
    let data_dir = steam_web_session_dir(session_id)?.join("webview2");
    fs::create_dir_all(&data_dir).map_err(|error| format!("打开 Steam 网页会话失败: {}", error))?;
    let steam_page_starts = Arc::new(AtomicUsize::new(0));
    let loop_detected = Arc::new(AtomicBool::new(false));
    let page_starts = steam_page_starts.clone();
    let detected = loop_detected.clone();
    let window = WebviewWindowBuilder::new(app, &label, WebviewUrl::External(url))
        .title("NEA - 完美世界竞技平台 Steam 授权")
        .data_directory(data_dir)
        .additional_browser_args("--disk-cache-size=1048576 --media-cache-size=1048576")
        .initialization_script(PERFECT_OAUTH_AUTOMATION_SCRIPT)
        .on_page_load(move |window, payload| {
            if payload.event() != PageLoadEvent::Started
                || payload.url().host_str() != Some("store.steampowered.com")
            {
                return;
            }
            if page_starts.fetch_add(1, Ordering::SeqCst) >= 2 {
                detected.store(true, Ordering::SeqCst);
                let _ = window.eval(PERFECT_OAUTH_LOOP_STOP_SCRIPT);
            }
        })
        .inner_size(1180.0, 780.0)
        .min_inner_size(760.0, 520.0)
        .center()
        .build()
        .map_err(|error| format!("打开完美 Steam 授权窗口失败: {}", error))?;
    Ok((window, loop_detected))
}

fn steam_id_from_web_cookie(value: &str) -> Option<String> {
    let steam_id: String = value
        .chars()
        .take_while(|character| character.is_ascii_digit())
        .collect();
    (steam_id.len() == 17).then_some(steam_id)
}

fn recover_steam_web_session_from_disk(session_id: &str) -> Option<steam::SteamWebSession> {
    let path = config_path().ok()?;
    let candidates = [
        path.clone(),
        path.with_extension("json.tmp"),
        path.with_extension("json.bak"),
    ];
    candidates.into_iter().find_map(|candidate| {
        parse_app_data_file(&candidate).and_then(|(data, _)| {
            data.steam
                .web_sessions
                .into_iter()
                .find(|session| session.id == session_id)
        })
    })
}

async fn steam_id_from_web_window(window: WebviewWindow) -> Result<Option<String>, String> {
    let url =
        tauri::Url::parse("https://store.steampowered.com/").map_err(|error| error.to_string())?;
    let cookies = tauri::async_runtime::spawn_blocking(move || window.cookies_for_url(url))
        .await
        .map_err(|error| error.to_string())?
        .map_err(|error| format!("读取 Steam 网页登录状态失败: {}", error))?;
    Ok(cookies
        .iter()
        .find(|cookie| cookie.name().eq_ignore_ascii_case("steamLoginSecure"))
        .and_then(|cookie| steam_id_from_web_cookie(cookie.value())))
}

fn steam_display_name_from_profile_xml(raw: &str) -> Option<String> {
    let value = if let Some(start) = raw.find("<steamID><![CDATA[") {
        let content = &raw[start + "<steamID><![CDATA[".len()..];
        content.find("]]></steamID>").map(|end| &content[..end])?
    } else {
        let start = raw.find("<steamID>")? + "<steamID>".len();
        let content = &raw[start..];
        let end = content.find("</steamID>")?;
        &content[..end]
    };
    let decoded = value
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&");
    let cleaned = decoded
        .trim()
        .chars()
        .filter(|character| !character.is_control())
        .take(128)
        .collect::<String>();
    (!cleaned.is_empty()).then_some(cleaned)
}

fn fetch_steam_display_name(steam_id: &str) -> Option<String> {
    if !is_valid_steam_id64(steam_id) {
        return None;
    }
    let client = reqwest::blocking::Client::builder()
        .connect_timeout(Duration::from_secs(4))
        .timeout(Duration::from_secs(7))
        .build()
        .ok()?;
    let response = client
        .get(format!(
            "https://steamcommunity.com/profiles/{steam_id}?xml=1"
        ))
        .header(
            reqwest::header::USER_AGENT,
            "NEA/1.2 Steam profile resolver",
        )
        .send()
        .ok()?
        .error_for_status()
        .ok()?;
    if response
        .content_length()
        .is_some_and(|length| length > 256 * 1024)
    {
        return None;
    }
    let bytes = response.bytes().ok()?;
    if bytes.len() > 256 * 1024 {
        return None;
    }
    steam_display_name_from_profile_xml(&String::from_utf8_lossy(&bytes))
}

fn stored_steam_display_name(app: &AppHandle, steam_id: &str) -> Option<String> {
    let state = app.state::<AppState>();
    let data = state.data.lock().ok()?;
    data.steam
        .accounts
        .iter()
        .find(|account| {
            account.id == steam_id && !steam_account_display_name_needs_refresh(account)
        })
        .map(|account| account.display_name.clone())
        .or_else(|| {
            data.steam.web_sessions.iter().find_map(|session| {
                let display_name = session.display_name.trim();
                (session.steam_id.as_deref() == Some(steam_id)
                    && !display_name.is_empty()
                    && display_name != steam_id
                    && session.account_name.as_deref() != Some(display_name))
                .then(|| display_name.to_string())
            })
        })
}

fn steam_account_display_name_needs_refresh(account: &steam::SteamAccount) -> bool {
    let display_name = account.display_name.trim();
    display_name.is_empty()
        || display_name == account.id
        || display_name.eq_ignore_ascii_case(account.account_name.trim())
}

fn steam_web_session_display_name_needs_refresh(session: &steam::SteamWebSession) -> bool {
    let display_name = session.display_name.trim();
    display_name.is_empty()
        || display_name == "待登录网页账号"
        || session.steam_id.as_deref() == Some(display_name)
        || session.account_name.as_deref() == Some(display_name)
}

async fn resolve_steam_display_name(app: &AppHandle, steam_id: &str) -> Option<String> {
    if let Some(display_name) = stored_steam_display_name(app, steam_id) {
        return Some(display_name);
    }
    let steam_id = steam_id.to_string();
    tauri::async_runtime::spawn_blocking(move || fetch_steam_display_name(&steam_id))
        .await
        .ok()
        .flatten()
}

async fn repair_stored_steam_display_names(app: AppHandle) -> Result<usize, String> {
    let steam_ids = {
        let state = app.state::<AppState>();
        let data = state.data.lock().map_err(|error| error.to_string())?;
        data.steam
            .accounts
            .iter()
            .filter(|account| steam_account_display_name_needs_refresh(account))
            .map(|account| account.id.clone())
            .chain(
                data.steam
                    .web_sessions
                    .iter()
                    .filter(|session| steam_web_session_display_name_needs_refresh(session))
                    .filter_map(|session| session.steam_id.clone()),
            )
            .filter(|steam_id| is_valid_steam_id64(steam_id))
            .collect::<HashSet<_>>()
    };
    let resolved = stream::iter(steam_ids)
        .map(|steam_id| {
            let app = app.clone();
            async move {
                resolve_steam_display_name(&app, &steam_id)
                    .await
                    .map(|display_name| (steam_id, display_name))
            }
        })
        .buffer_unordered(8)
        .filter_map(|resolved| async move { resolved })
        .collect::<HashMap<_, _>>()
        .await;
    if resolved.is_empty() {
        return Ok(0);
    }
    let repaired = {
        let state = app.state::<AppState>();
        commit_app_data_update(&state, |data| {
            let mut repaired = 0usize;
            for account in &mut data.steam.accounts {
                if steam_account_display_name_needs_refresh(account) {
                    if let Some(display_name) = resolved.get(&account.id) {
                        account.display_name.clone_from(display_name);
                        repaired += 1;
                    }
                }
            }
            for session in &mut data.steam.web_sessions {
                if steam_web_session_display_name_needs_refresh(session) {
                    if let Some(display_name) = session
                        .steam_id
                        .as_deref()
                        .and_then(|steam_id| resolved.get(steam_id))
                    {
                        session.display_name.clone_from(display_name);
                        repaired += 1;
                    }
                }
            }
            reconcile_steam_identities(data);
            Ok(repaired)
        })?
    };
    if repaired > 0 {
        update_tray(&app);
        let _ = app.emit("app-data-changed", ());
    }
    Ok(repaired)
}

enum VerifiedSteamWebSessionPersist {
    Saved(String),
    DuplicateSkipped(String),
}

fn persist_verified_steam_web_session(
    app: &AppHandle,
    session_id: &str,
    steam_id: &str,
    resolved_display_name: Option<&str>,
    skip_complete_duplicates: bool,
) -> Result<VerifiedSteamWebSessionPersist, String> {
    let state = app.state::<AppState>();
    let mut current_data = state.data.lock().map_err(|error| error.to_string())?;
    let mut next_data = current_data.clone();
    let fallback_display_name = resolved_display_name
        .filter(|display_name| !display_name.trim().is_empty())
        .map(str::to_string)
        .or_else(|| {
            next_data
                .steam
                .accounts
                .iter()
                .find(|account| account.id == steam_id)
                .map(|account| account.display_name.clone())
        })
        .unwrap_or_else(|| steam_id.to_string());
    if !next_data
        .steam
        .web_sessions
        .iter()
        .any(|session| session.id == session_id)
    {
        if let Some(recovered) = recover_steam_web_session_from_disk(session_id) {
            next_data.steam.web_sessions.push(recovered);
        }
    }
    let session = next_data
        .steam
        .web_sessions
        .iter_mut()
        .find(|session| session.id == session_id)
        .ok_or_else(|| "Steam 网页会话不存在".to_string())?;
    session.steam_id = Some(steam_id.to_string());
    session.display_name = fallback_display_name;
    session.last_verified_at = Some(Utc::now().to_rfc3339());
    let verified_account_name = session.account_name.clone();
    bind_credentials_to_steam_id(&mut next_data, steam_id, verified_account_name.as_deref());

    if skip_complete_duplicates {
        deduplicate_credentials_for_steam_id(&mut next_data, steam_id);
        if let Some(display_name) =
            merge_complete_duplicate_web_import(&mut next_data, session_id, steam_id)
        {
            save_data(&next_data)?;
            *current_data = next_data;
            drop(current_data);
            if let Ok(mut duplicates) = state.steam_import_duplicate_sessions.lock() {
                duplicates.insert(session_id.to_string(), display_name.clone());
            }
            cleanup_steam_web_session_directories(app, vec![session_id.to_string()]);
            update_tray(app);
            return Ok(VerifiedSteamWebSessionPersist::DuplicateSkipped(
                display_name,
            ));
        }
    }
    let (deduplicated, removed_session_ids) = deduplicate_steam_web_sessions(
        std::mem::take(&mut next_data.steam.web_sessions),
        Some(session_id),
    );
    next_data.steam.web_sessions = deduplicated;
    reconcile_steam_identities(&mut next_data);
    let display_name = next_data
        .steam
        .web_sessions
        .iter()
        .find(|session| session.id == session_id)
        .map(steam_web_session_primary_name)
        .ok_or_else(|| "Steam 网页会话去重失败".to_string())?;
    save_data(&next_data)?;
    *current_data = next_data;
    drop(current_data);
    cleanup_steam_web_session_directories(app, removed_session_ids);
    update_tray(app);
    Ok(VerifiedSteamWebSessionPersist::Saved(display_name))
}

fn steam_web_session_primary_name(session: &steam::SteamWebSession) -> String {
    session
        .account_name
        .as_deref()
        .filter(|name| !name.trim().is_empty())
        .unwrap_or(&session.display_name)
        .to_string()
}

fn merge_steam_web_session(
    primary: &mut steam::SteamWebSession,
    duplicate: &steam::SteamWebSession,
) {
    if primary.account_name.is_none() {
        primary.account_name = duplicate.account_name.clone();
    }
    if primary.note.is_none() {
        primary.note = duplicate.note.clone();
    }
    let primary_is_fallback = primary.display_name.trim().is_empty()
        || primary.display_name == "待登录网页账号"
        || primary.steam_id.as_deref() == Some(primary.display_name.as_str())
        || primary.account_name.as_deref() == Some(primary.display_name.as_str());
    let duplicate_is_useful = !duplicate.display_name.trim().is_empty()
        && duplicate.display_name != "待登录网页账号"
        && duplicate.steam_id.as_deref() != Some(duplicate.display_name.as_str())
        && duplicate.account_name.as_deref() != Some(duplicate.display_name.as_str());
    if primary_is_fallback && duplicate_is_useful {
        primary.display_name.clone_from(&duplicate.display_name);
    }
}

fn merge_complete_duplicate_web_import(
    data: &mut AppData,
    imported_session_id: &str,
    steam_id: &str,
) -> Option<String> {
    if !has_verified_steam_web_login(data, steam_id, Some(imported_session_id)) {
        return None;
    }
    let imported = data
        .steam
        .web_sessions
        .iter()
        .find(|session| session.id == imported_session_id)?
        .clone();
    if let Some(existing) = data.steam.web_sessions.iter_mut().find(|session| {
        session.id != imported_session_id && session.steam_id.as_deref() == Some(steam_id)
    }) {
        merge_steam_web_session(existing, &imported);
    }
    data.steam
        .web_sessions
        .retain(|session| session.id != imported_session_id);
    reconcile_steam_identities(data);
    Some(
        data.steam
            .web_sessions
            .iter()
            .find(|session| session.steam_id.as_deref() == Some(steam_id))
            .map(steam_web_session_primary_name)
            .or_else(|| {
                data.steam
                    .accounts
                    .iter()
                    .find(|account| account.id == steam_id)
                    .map(|account| account.display_name.clone())
            })
            .unwrap_or_else(|| steam_id.to_string()),
    )
}

fn deduplicate_steam_web_sessions(
    sessions: Vec<steam::SteamWebSession>,
    preferred_session_id: Option<&str>,
) -> (Vec<steam::SteamWebSession>, Vec<String>) {
    let mut unique = Vec::<steam::SteamWebSession>::with_capacity(sessions.len());
    let mut indices = HashMap::<String, usize>::new();
    let mut removed = Vec::new();
    for session in sessions {
        let Some(steam_id) = session.steam_id.clone() else {
            unique.push(session);
            continue;
        };
        let Some(&existing_index) = indices.get(&steam_id) else {
            indices.insert(steam_id, unique.len());
            unique.push(session);
            continue;
        };
        let prefer_new = preferred_session_id == Some(session.id.as_str());
        if prefer_new {
            let replaced = std::mem::replace(&mut unique[existing_index], session);
            merge_steam_web_session(&mut unique[existing_index], &replaced);
            removed.push(replaced.id);
        } else {
            merge_steam_web_session(&mut unique[existing_index], &session);
            removed.push(session.id);
        }
    }
    (unique, removed)
}

fn cleanup_steam_web_session_directories(app: &AppHandle, session_ids: Vec<String>) {
    for session_id in session_ids {
        if let Some(window) = app.get_webview_window(&steam_web_window_label(&session_id)) {
            let _ = window.destroy();
        }
        let Ok(directory) = steam_web_session_dir(&session_id) else {
            continue;
        };
        tauri::async_runtime::spawn(async move {
            tokio::time::sleep(Duration::from_millis(500)).await;
            let _ =
                tauri::async_runtime::spawn_blocking(move || fs::remove_dir_all(directory)).await;
        });
    }
}

#[tauri::command]
async fn create_steam_web_session(app: AppHandle) -> Result<steam::SteamWorkspace, String> {
    let session = steam::SteamWebSession {
        id: Uuid::new_v4().to_string(),
        steam_id: None,
        account_name: None,
        display_name: "待登录网页账号".to_string(),
        note: None,
        created_at: Utc::now().to_rfc3339(),
        last_verified_at: None,
    };
    {
        let state = app.state::<AppState>();
        commit_app_data_update(&state, |data| {
            data.steam.web_sessions.push(session.clone());
            Ok(())
        })?;
    }
    if let Err(error) = build_steam_web_window(&app, &session.id, true, true, None) {
        let state = app.state::<AppState>();
        let rollback = commit_app_data_update(&state, |data| {
            data.steam.web_sessions.retain(|item| item.id != session.id);
            Ok(())
        });
        return match rollback {
            Ok(()) => Err(error),
            Err(rollback_error) => Err(format!(
                "{error}；同时无法回滚未创建的网页账号：{rollback_error}"
            )),
        };
    }
    app.state::<AppState>()
        .data
        .lock()
        .map(|data| data.steam.clone())
        .map_err(|error| error.to_string())
}

async fn discard_unverified_steam_web_session(
    app: &AppHandle,
    session_id: &str,
) -> Result<(), String> {
    if let Some(window) = app.get_webview_window(&steam_web_window_label(session_id)) {
        let _ = window.destroy();
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    let removed = {
        let state = app.state::<AppState>();
        let mut current_data = state.data.lock().map_err(|error| error.to_string())?;
        let mut next_data = current_data.clone();
        let before = next_data.steam.web_sessions.len();
        next_data
            .steam
            .web_sessions
            .retain(|session| session.id != session_id || session.steam_id.is_some());
        let removed = next_data.steam.web_sessions.len() != before;
        if removed {
            save_data(&next_data)?;
            *current_data = next_data;
        }
        removed
    };
    if removed {
        if let Ok(directory) = steam_web_session_dir(session_id) {
            let _ =
                tauri::async_runtime::spawn_blocking(move || fs::remove_dir_all(directory)).await;
        }
    }
    let _ = app.emit("app-data-changed", ());
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SteamImportAccountOutcome {
    Verified,
    SkippedExisting,
    InvalidCredentials,
    HasToken,
    VerificationRequired,
    Failed,
    Cancelled,
}

fn steam_import_outcome_from_markers(
    native_title: Option<&str>,
    url_fragment: Option<&str>,
) -> Option<SteamImportAccountOutcome> {
    if native_title == Some(STEAM_INVALID_CREDENTIALS_WINDOW_TITLE)
        || url_fragment == Some(STEAM_INVALID_CREDENTIALS_URL_MARKER)
    {
        Some(SteamImportAccountOutcome::InvalidCredentials)
    } else if native_title == Some(STEAM_TOKEN_PROTECTED_WINDOW_TITLE)
        || url_fragment == Some(STEAM_TOKEN_PROTECTED_URL_MARKER)
    {
        Some(SteamImportAccountOutcome::HasToken)
    } else if native_title == Some(STEAM_VERIFICATION_WINDOW_TITLE)
        || url_fragment == Some(STEAM_VERIFICATION_URL_MARKER)
    {
        Some(SteamImportAccountOutcome::VerificationRequired)
    } else {
        None
    }
}

struct SteamImportAccountResult {
    account: String,
    outcome: SteamImportAccountOutcome,
}

fn steam_import_is_cancelled(app: &AppHandle, capability_controlled: bool) -> bool {
    let state = app.state::<AppState>();
    if capability_controlled {
        state.steam_capability_cancelled.load(Ordering::SeqCst)
    } else {
        state.steam_bulk_import_cancelled.load(Ordering::SeqCst)
    }
}

fn steam_web_import_session_is_verified(app: &AppHandle, session_id: &str) -> bool {
    app.state::<AppState>()
        .data
        .lock()
        .ok()
        .is_some_and(|data| {
            data.steam
                .web_sessions
                .iter()
                .find(|saved| saved.id == session_id)
                .and_then(|saved| saved.steam_id.as_ref())
                .is_some()
        })
}

async fn import_single_steam_web_account(
    app: AppHandle,
    mut credentials: SteamCredentialInput,
    started: Arc<AtomicUsize>,
    completed: Arc<AtomicUsize>,
    total: usize,
    parallel: bool,
    capability_controlled: bool,
) -> SteamImportAccountResult {
    let account_label = credentials.account.clone();
    if capability_controlled && !wait_for_steam_capability_permission(&app).await {
        clear_sensitive_string(&mut credentials.password);
        return SteamImportAccountResult {
            account: account_label,
            outcome: SteamImportAccountOutcome::Cancelled,
        };
    }
    if steam_import_is_cancelled(&app, capability_controlled) {
        clear_sensitive_string(&mut credentials.password);
        return SteamImportAccountResult {
            account: account_label,
            outcome: SteamImportAccountOutcome::Cancelled,
        };
    }
    let session = steam::SteamWebSession {
        id: Uuid::new_v4().to_string(),
        steam_id: None,
        account_name: Some(credentials.account.clone()),
        display_name: credentials.account.clone(),
        note: None,
        created_at: Utc::now().to_rfc3339(),
        last_verified_at: None,
    };
    let session_saved = {
        let state = app.state::<AppState>();
        commit_app_data_update(&state, |data| {
            data.steam.web_sessions.push(session.clone());
            Ok(())
        })
    };
    if let Err(error) = session_saved {
        let _ = app.emit(
            "steam-bulk-import-progress",
            format!("{} 创建网页登录会话失败: {}", account_label, error),
        );
        clear_sensitive_string(&mut credentials.password);
        return SteamImportAccountResult {
            account: account_label,
            outcome: SteamImportAccountOutcome::Failed,
        };
    }

    let opened = started.fetch_add(1, Ordering::SeqCst) + 1;
    let progress = if parallel {
        format!(
            "正在并行登录 Steam 网页账号 {}/{}（同时最多 {} 个窗口）",
            opened, total, MAX_PARALLEL_STEAM_IMPORT_WINDOWS
        )
    } else {
        format!("正在建立 Steam 网页登录 {opened}/{total}")
    };
    let _ = app.emit("steam-bulk-import-progress", progress);
    let window_result = build_steam_web_window(&app, &session.id, true, true, Some(&credentials));
    if let Ok(window) = &window_result {
        let slot = (opened - 1) % MAX_PARALLEL_STEAM_IMPORT_WINDOWS;
        let _ = window.set_position(LogicalPosition::new(
            42.0 + slot as f64 * 30.0,
            42.0 + slot as f64 * 26.0,
        ));
    }
    clear_sensitive_string(&mut credentials.password);
    let outcome = if let Err(error) = window_result {
        let _ = discard_unverified_steam_web_session(&app, &session.id).await;
        let _ = app.emit(
            "steam-bulk-import-progress",
            format!("{} 登录窗口打开失败: {}", account_label, error),
        );
        SteamImportAccountOutcome::Failed
    } else {
        let mut detected = loop {
            let duplicate_detected = {
                let state = app.state::<AppState>();
                state
                    .steam_import_duplicate_sessions
                    .lock()
                    .ok()
                    .and_then(|mut duplicates| duplicates.remove(&session.id))
                    .is_some()
            };
            if duplicate_detected {
                break SteamImportAccountOutcome::SkippedExisting;
            }
            if steam_web_import_session_is_verified(&app, &session.id) {
                break SteamImportAccountOutcome::Verified;
            }
            let Some(window) = app.get_webview_window(&steam_web_window_label(&session.id)) else {
                break if steam_import_is_cancelled(&app, capability_controlled) {
                    SteamImportAccountOutcome::Cancelled
                } else {
                    SteamImportAccountOutcome::Failed
                };
            };
            let native_title = window.title().ok();
            let current_url = window.url().ok();
            let url_fragment = current_url.as_ref().and_then(tauri::Url::fragment);
            if let Some(outcome) =
                steam_import_outcome_from_markers(native_title.as_deref(), url_fragment)
            {
                break outcome;
            }
            if steam_import_is_cancelled(&app, capability_controlled) {
                break SteamImportAccountOutcome::Cancelled;
            }
            tokio::time::sleep(Duration::from_millis(350)).await;
        };
        if !matches!(detected, SteamImportAccountOutcome::Verified) {
            if steam_web_import_session_is_verified(&app, &session.id) {
                detected = SteamImportAccountOutcome::Verified;
            } else {
                let _ = discard_unverified_steam_web_session(&app, &session.id).await;
                if steam_web_import_session_is_verified(&app, &session.id) {
                    detected = SteamImportAccountOutcome::Verified;
                }
            }
        }
        detected
    };
    let finished = completed.fetch_add(1, Ordering::SeqCst) + 1;
    let detail = match outcome {
        SteamImportAccountOutcome::Verified => "登录态已保存",
        SteamImportAccountOutcome::SkippedExisting => "识别到相同 Steam64 的已有网页登录态，已合并",
        SteamImportAccountOutcome::InvalidCredentials => "账号或密码错误",
        SteamImportAccountOutcome::HasToken => "有令牌",
        SteamImportAccountOutcome::VerificationRequired => "需要 Steam 验证，已跳过",
        SteamImportAccountOutcome::Failed => "登录失败或窗口已关闭",
        SteamImportAccountOutcome::Cancelled => "已取消",
    };
    let _ = app.emit(
        "steam-bulk-import-progress",
        format!("{account_label}：{detail}（已完成 {finished}/{total}）"),
    );
    SteamImportAccountResult {
        account: account_label,
        outcome,
    }
}

#[tauri::command]
fn preview_steam_web_import(
    state: State<'_, AppState>,
    accounts: Vec<String>,
) -> Result<SteamImportPreview, String> {
    let data = state.data.lock().map_err(|error| error.to_string())?;
    Ok(steam_import_preview(&data, &accounts))
}

#[tauri::command]
fn cancel_steam_web_import(app: AppHandle) -> bool {
    let state = app.state::<AppState>();
    if !state.steam_bulk_import_running.load(Ordering::SeqCst) {
        return false;
    }
    if !state
        .steam_bulk_import_cancelled
        .swap(true, Ordering::SeqCst)
    {
        let _ = app.emit(
            "steam-bulk-import-progress",
            "正在取消 Steam 网页账号导入...",
        );
    }
    true
}

#[tauri::command]
async fn import_steam_web_accounts_from_text(
    app: AppHandle,
    accounts: Vec<SteamCredentialInput>,
    _skip_existing: bool,
) -> Result<SteamBulkImportResult, String> {
    let _import_guard = acquire_steam_web_import(&app)?;
    let _bulk_import_guard = acquire_steam_bulk_import(&app)?;
    if accounts.is_empty() {
        return Err("没有可导入的 Steam 网页账号".to_string());
    }
    if accounts.len() > MAX_STEAM_TEXT_IMPORT_ACCOUNTS {
        return Err(format!(
            "一次最多导入 {} 个 Steam 网页账号",
            MAX_STEAM_TEXT_IMPORT_ACCOUNTS
        ));
    }
    for credentials in &accounts {
        let account = credentials.account.trim();
        if account.is_empty()
            || account.len() > 128
            || account.chars().any(char::is_whitespace)
            || credentials.password.is_empty()
            || credentials.password.len() > 512
        {
            return Err("Steam 账号文本包含无效的账号或密码".to_string());
        }
    }

    let account_names = accounts
        .iter()
        .map(|credentials| credentials.account.clone())
        .collect::<Vec<_>>();
    let preview = {
        let state = app.state::<AppState>();
        let data = state.data.lock().map_err(|error| error.to_string())?;
        steam_import_preview(&data, &account_names)
    };
    let preflight_data = {
        let state = app.state::<AppState>();
        let snapshot = state
            .data
            .lock()
            .map_err(|error| error.to_string())?
            .clone();
        snapshot
    };
    let mut seen = HashSet::new();
    let mut skipped_existing_accounts = Vec::new();
    let mut accounts_to_import = Vec::new();
    for mut credentials in accounts {
        let normalized = normalized_steam_account_name(&credentials.account);
        let duplicate_key = steam64_for_account_name(&preflight_data, &credentials.account)
            .map(|steam_id| format!("steam:{steam_id}"))
            .unwrap_or_else(|| format!("account:{normalized}"));
        if !seen.insert(duplicate_key) {
            clear_sensitive_string(&mut credentials.password);
            continue;
        }
        if let Some(steam_id) = steam_import_duplicate_id(&preflight_data, &credentials.account) {
            if has_saved_credential_for_steam_id(&preflight_data, &steam_id) {
                skipped_existing_accounts.push(credentials.account.trim().to_string());
                clear_sensitive_string(&mut credentials.password);
                continue;
            }
        }
        accounts_to_import.push(credentials);
    }
    let credential_backups = accounts_to_import
        .iter()
        .map(|credentials| {
            let normalized = normalized_steam_account_name(&credentials.account);
            let existing = preflight_data
                .steam_credentials
                .iter()
                .find(|saved| normalized_steam_account_name(&saved.account_name) == normalized)
                .cloned();
            (normalized, existing)
        })
        .collect::<HashMap<_, _>>();
    let steam_ids_with_saved_credentials = accounts_to_import
        .iter()
        .filter_map(|credentials| {
            let steam_id = steam64_for_account_name(&preflight_data, &credentials.account)?;
            has_saved_credential_for_steam_id(&preflight_data, &steam_id).then_some(steam_id)
        })
        .collect::<HashSet<_>>();
    let credential_commit = {
        let state = app.state::<AppState>();
        commit_app_data_update(&state, |data| {
            insert_missing_steam_credentials(
                data,
                &accounts_to_import,
                &steam_ids_with_saved_credentials,
            );
            reconcile_steam_identities(data);
            Ok(())
        })
    };
    if let Err(error) = credential_commit {
        for credentials in &mut accounts_to_import {
            clear_sensitive_string(&mut credentials.password);
        }
        return Err(error);
    }

    let total = accounts_to_import.len();
    let mut imported = 0usize;
    let mut failed = 0usize;
    let mut cancelled_accounts = Vec::new();
    let mut invalid_credential_accounts = Vec::new();
    let mut token_protected_accounts = Vec::new();
    let mut verification_required_accounts = Vec::new();
    let mut failed_accounts = Vec::new();
    let mut credential_rollback_accounts = Vec::new();
    let started = Arc::new(AtomicUsize::new(0));
    let completed = Arc::new(AtomicUsize::new(0));
    let results = stream::iter(accounts_to_import)
        .map(|credentials| {
            import_single_steam_web_account(
                app.clone(),
                credentials,
                started.clone(),
                completed.clone(),
                total,
                true,
                false,
            )
        })
        .buffer_unordered(MAX_PARALLEL_STEAM_IMPORT_WINDOWS)
        .collect::<Vec<_>>()
        .await;
    for result in results {
        match result.outcome {
            SteamImportAccountOutcome::Verified => imported += 1,
            SteamImportAccountOutcome::SkippedExisting => {
                skipped_existing_accounts.push(result.account);
            }
            SteamImportAccountOutcome::InvalidCredentials => {
                failed += 1;
                credential_rollback_accounts.push(result.account.clone());
                invalid_credential_accounts.push(result.account);
            }
            SteamImportAccountOutcome::HasToken => {
                failed += 1;
                token_protected_accounts.push(result.account);
            }
            SteamImportAccountOutcome::VerificationRequired => {
                verification_required_accounts.push(result.account);
            }
            SteamImportAccountOutcome::Failed => {
                failed += 1;
                credential_rollback_accounts.push(result.account.clone());
                failed_accounts.push(result.account);
            }
            SteamImportAccountOutcome::Cancelled => {
                credential_rollback_accounts.push(result.account.clone());
                cancelled_accounts.push(result.account);
            }
        }
    }
    let mut reported = HashSet::new();
    skipped_existing_accounts
        .retain(|account| reported.insert(normalized_steam_account_name(account)));
    {
        let state = app.state::<AppState>();
        commit_app_data_update(&state, |data| {
            for account in &credential_rollback_accounts {
                let normalized = normalized_steam_account_name(account);
                data.steam_credentials.retain(|credential| {
                    normalized_steam_account_name(&credential.account_name) != normalized
                });
                if let Some(Some(backup)) = credential_backups.get(&normalized) {
                    data.steam_credentials.push(backup.clone());
                }
            }
            reconcile_saved_steam_credentials(data);
            reconcile_steam_identities(data);
            Ok(())
        })?;
    }
    update_tray(&app);
    Ok(SteamBulkImportResult {
        imported,
        failed,
        cancelled: cancelled_accounts.len(),
        skipped_existing: skipped_existing_accounts.len(),
        skipped_existing_accounts,
        skipped_duplicate_input: preview.duplicate_input_accounts.len(),
        invalid_credential_accounts,
        token_protected_accounts,
        verification_required_accounts,
        failed_accounts,
        cancelled_accounts,
    })
}

#[tauri::command]
async fn open_steam_web_session(app: AppHandle, session_id: String) -> Result<(), String> {
    let exists = app
        .state::<AppState>()
        .data
        .lock()
        .map_err(|error| error.to_string())?
        .steam
        .web_sessions
        .iter()
        .any(|session| session.id == session_id);
    if !exists {
        return Err("Steam 网页会话不存在".to_string());
    }
    open_steam_web_window(&app, &session_id)
}

#[tauri::command]
async fn refresh_steam_web_sessions(app: AppHandle) -> Result<steam::SteamWorkspace, String> {
    let session_ids = app
        .state::<AppState>()
        .data
        .lock()
        .map_err(|error| error.to_string())?
        .steam
        .web_sessions
        .iter()
        .map(|session| session.id.clone())
        .collect::<Vec<_>>();
    let cookie_url =
        tauri::Url::parse("https://store.steampowered.com/").map_err(|error| error.to_string())?;
    let mut verified = HashMap::new();
    for session_id in session_ids {
        let (window, temporary) = match app.get_webview_window(&steam_web_window_label(&session_id))
        {
            Some(window) => (window, false),
            None => (
                build_steam_web_window(&app, &session_id, false, false, None)?,
                true,
            ),
        };
        if temporary {
            tokio::time::sleep(Duration::from_millis(750)).await;
        }
        let url = cookie_url.clone();
        let cookie_window = window.clone();
        let cookies =
            tauri::async_runtime::spawn_blocking(move || cookie_window.cookies_for_url(url))
                .await
                .map_err(|error| error.to_string())?
                .map_err(|error| format!("读取 Steam 网页登录状态失败: {}", error))?;
        if temporary {
            let _ = window.destroy();
        }
        let steam_id = cookies
            .iter()
            .find(|cookie| cookie.name().eq_ignore_ascii_case("steamLoginSecure"))
            .and_then(|cookie| steam_id_from_web_cookie(cookie.value()));
        if let Some(steam_id) = steam_id {
            verified.insert(session_id, steam_id);
        }
    }
    let display_name_candidates = {
        let state = app.state::<AppState>();
        let data = state.data.lock().map_err(|error| error.to_string())?;
        verified
            .iter()
            .filter_map(|(session_id, steam_id)| {
                data.steam
                    .web_sessions
                    .iter()
                    .find(|session| session.id == *session_id)
                    .filter(|session| steam_web_session_display_name_needs_refresh(session))
                    .map(|_| steam_id.clone())
            })
            .collect::<HashSet<_>>()
    };
    let resolved_display_names = stream::iter(display_name_candidates)
        .map(|steam_id| {
            let app = app.clone();
            async move {
                resolve_steam_display_name(&app, &steam_id)
                    .await
                    .map(|display_name| (steam_id, display_name))
            }
        })
        .buffer_unordered(6)
        .filter_map(|resolved| async move { resolved })
        .collect::<HashMap<_, _>>()
        .await;
    let state = app.state::<AppState>();
    let (workspace, removed_session_ids) = commit_app_data_update(&state, |data| {
        let account_names = data
            .steam
            .accounts
            .iter()
            .map(|account| (account.id.clone(), account.display_name.clone()))
            .collect::<HashMap<_, _>>();
        let verified_at = Utc::now().to_rfc3339();
        for session in &mut data.steam.web_sessions {
            if let Some(steam_id) = verified.get(&session.id) {
                session.steam_id = Some(steam_id.clone());
                if let Some(display_name) = resolved_display_names.get(steam_id) {
                    session.display_name.clone_from(display_name);
                } else if steam_web_session_display_name_needs_refresh(session) {
                    session.display_name = account_names
                        .get(steam_id)
                        .cloned()
                        .unwrap_or_else(|| steam_id.clone());
                }
                session.last_verified_at = Some(verified_at.clone());
            } else {
                session.steam_id = None;
                session.last_verified_at = None;
            }
        }
        let (deduplicated, removed_session_ids) =
            deduplicate_steam_web_sessions(std::mem::take(&mut data.steam.web_sessions), None);
        data.steam.web_sessions = deduplicated;
        reconcile_saved_steam_credentials(data);
        Ok((data.steam.clone(), removed_session_ids))
    })?;
    cleanup_steam_web_session_directories(&app, removed_session_ids);
    update_tray(&app);
    Ok(workspace)
}

#[tauri::command]
async fn set_steam_web_session_note(
    app: AppHandle,
    session_id: String,
    note: String,
) -> Result<steam::SteamWorkspace, String> {
    let state = app.state::<AppState>();
    commit_app_data_update(&state, |data| {
        let session = data
            .steam
            .web_sessions
            .iter_mut()
            .find(|session| session.id == session_id)
            .ok_or_else(|| "Steam 网页会话不存在".to_string())?;
        let trimmed = note.trim();
        session.note = (!trimmed.is_empty()).then(|| trimmed.chars().take(120).collect());
        Ok(data.steam.clone())
    })
}

#[tauri::command]
async fn set_steam_identity_note(
    app: AppHandle,
    identity_id: String,
    note: String,
) -> Result<AppData, String> {
    let state = app.state::<AppState>();
    commit_app_data_update(&state, |data| {
        reconcile_steam_identities(data);
        let identity = data
            .steam_identities
            .iter()
            .find(|identity| identity.id == identity_id)
            .cloned()
            .ok_or_else(|| "Steam 核心账号不存在".to_string())?;
        let trimmed = note.trim();
        let note = (!trimmed.is_empty()).then(|| trimmed.chars().take(120).collect::<String>());
        if let Some(account_id) = identity.client_account_id.as_deref() {
            if let Some(account) = data
                .steam
                .accounts
                .iter_mut()
                .find(|account| account.id == account_id)
            {
                account.note.clone_from(&note);
            }
        }
        if let Some(session_id) = identity.web_session_id.as_deref() {
            if let Some(session) = data
                .steam
                .web_sessions
                .iter_mut()
                .find(|session| session.id == session_id)
            {
                session.note.clone_from(&note);
            }
        }
        if let Some(saved_identity) = data
            .steam_identities
            .iter_mut()
            .find(|saved| saved.id == identity_id)
        {
            saved_identity.note = note;
            saved_identity.updated_at = now();
        }
        Ok(())
    })?;
    let result = state
        .data
        .lock()
        .map_err(|error| error.to_string())?
        .clone();
    update_tray(&app);
    Ok(result)
}

#[tauri::command]
async fn delete_steam_web_session(app: AppHandle, session_id: String) -> Result<(), String> {
    let _activity = acquire_switch_activity(&app)?;
    let _import_guard = acquire_steam_web_import(&app)?;
    let session_dir = steam_web_session_dir(&session_id)?;
    if let Some(window) = app.get_webview_window(&steam_web_window_label(&session_id)) {
        window.destroy().map_err(|error| error.to_string())?;
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    let staged = stage_for_deletion(&session_dir)?;
    let state = app.state::<AppState>();
    let delete_result = commit_app_data_update(&state, |data| {
        if !data
            .steam
            .web_sessions
            .iter()
            .any(|session| session.id == session_id)
        {
            return Err("Steam 网页会话不存在".to_string());
        }
        data.steam
            .web_sessions
            .retain(|session| session.id != session_id);
        reconcile_saved_steam_credentials(data);
        Ok(())
    });
    if let Err(error) = delete_result {
        if let Some(staged) = &staged {
            rollback_staged_deletion(staged);
        }
        return Err(error);
    }
    if let Some(staged) = &staged {
        mark_staged_deletion_committed(staged);
    }
    finish_staged_deletion(staged);
    update_tray(&app);
    let _ = app.emit("app-data-changed", ());
    Ok(())
}

#[tauri::command]
fn delete_steam_saved_credential(app: AppHandle, identity_id: String) -> Result<AppData, String> {
    let _activity = acquire_switch_activity(&app)?;
    let _import_guard = acquire_steam_web_import(&app)?;
    let state = app.state::<AppState>();
    commit_app_data_update(&state, |data| {
        reconcile_steam_identities(data);
        let identity = data
            .steam_identities
            .iter()
            .find(|identity| identity.id == identity_id)
            .cloned()
            .ok_or_else(|| "Steam 账号不存在".to_string())?;
        let normalized_account = identity
            .account_name
            .as_deref()
            .map(normalized_steam_account_name);
        let before = data.steam_credentials.len();
        data.steam_credentials.retain(|credential| {
            let same_steam_id = identity
                .steam_id
                .as_deref()
                .is_some_and(|steam_id| credential.steam_id.as_deref() == Some(steam_id));
            let same_account = normalized_account.as_deref().is_some_and(|account| {
                normalized_steam_account_name(&credential.account_name) == account
            });
            !same_steam_id && !same_account
        });
        if data.steam_credentials.len() == before {
            return Err("该 Steam 账号没有已保存账密".to_string());
        }
        if let Some(steam_id) = identity.steam_id {
            data.steam_native_switcher_exclusions.remove(&steam_id);
        }
        reconcile_saved_steam_credentials(data);
        Ok(())
    })?;
    let latest = state
        .data
        .lock()
        .map_err(|error| error.to_string())?
        .clone();
    update_tray(&app);
    let _ = app.emit("app-data-changed", ());
    Ok(latest)
}

fn perfect_profile_is_complete(profile: &perfect_arena::PerfectArenaProfile) -> bool {
    profile.found
        && profile.nickname.is_some()
        && profile.avatar_url.is_some()
        && profile.score.is_some()
        && profile.player_identity.is_some()
        && profile.reputation_level.is_some()
}

fn merge_perfect_profile(
    saved: &mut perfect_arena::PerfectArenaProfile,
    fresh: perfect_arena::PerfectArenaProfile,
) {
    if !fresh.found {
        return;
    }
    let saved_updated = saved
        .updated_at
        .as_deref()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or_default();
    let fresh_updated = fresh
        .updated_at
        .as_deref()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or_default();
    let overwrite = fresh_updated >= saved_updated;
    let mut content_changed = false;

    macro_rules! merge_field {
        ($field:ident) => {
            if fresh.$field.is_some() && (overwrite || saved.$field.is_none()) {
                content_changed |= saved.$field != fresh.$field;
                saved.$field = fresh.$field;
            }
        };
    }

    saved.found = true;
    merge_field!(nickname);
    merge_field!(avatar_url);
    merge_field!(avatar_source_url);
    merge_field!(score);
    merge_field!(season);
    merge_field!(player_identity);
    merge_field!(high_risk);
    merge_field!(reputation_requires_verification);
    merge_field!(reputation_points);
    merge_field!(reputation_level);
    if fresh_updated > saved_updated && content_changed {
        saved.updated_at = fresh.updated_at;
    }
}

fn cache_perfect_profile_avatar(
    profile: &mut perfect_arena::PerfectArenaProfile,
    previous: Option<&perfect_arena::PerfectArenaProfile>,
) {
    let Some(source_url) = profile
        .avatar_url
        .clone()
        .filter(|url| url.starts_with("https://") || url.starts_with("http://"))
    else {
        return;
    };
    profile.avatar_source_url = Some(source_url.clone());
    if previous.is_some_and(|cached| {
        cached.avatar_source_url.as_deref() == Some(source_url.as_str())
            && perfect_avatar_path(&profile.steam_id).is_ok_and(|path| path.is_file())
    }) {
        profile.avatar_url = Some(PERFECT_AVATAR_CACHE_MARKER.to_string());
        return;
    }
    if let Some(data_url) = download_avatar_data_url(&source_url) {
        if store_perfect_avatar_data_url(&profile.steam_id, &data_url).is_ok() {
            profile.avatar_url = Some(PERFECT_AVATAR_CACHE_MARKER.to_string());
        } else {
            profile.avatar_url = Some(source_url);
        }
    }
}

async fn refresh_perfect_profiles_for_ids(
    app: &AppHandle,
    steam_ids: Vec<String>,
    wait_for_complete: bool,
) -> Result<Vec<perfect_arena::PerfectArenaProfile>, String> {
    let steam_ids = steam_ids
        .into_iter()
        .filter(|id| id.len() == 17 && id.chars().all(|character| character.is_ascii_digit()))
        .collect::<HashSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    let previous = {
        let state = app.state::<AppState>();
        let data = state.data.lock().map_err(|error| error.to_string())?;
        data.perfect_profiles.clone()
    };
    let query_ids = steam_ids.clone();
    let fresh = tauri::async_runtime::spawn_blocking(move || {
        let online = perfect_arena::online_profiles(&query_ids).unwrap_or_default();
        let deadline = Instant::now() + Duration::from_secs(12);
        let cached = loop {
            let profiles = perfect_arena::cached_profiles(&query_ids);
            if !wait_for_complete
                || profiles.iter().all(perfect_profile_is_complete)
                || Instant::now() >= deadline
            {
                break profiles;
            }
            thread::sleep(Duration::from_millis(750));
        };
        let mut merged = online
            .into_iter()
            .map(|profile| (profile.steam_id.clone(), profile))
            .collect::<HashMap<_, _>>();
        for cached_profile in cached {
            if !cached_profile.found {
                continue;
            }
            let entry = merged
                .entry(cached_profile.steam_id.clone())
                .or_insert_with(|| cached_profile.clone());
            if entry.nickname.is_none() {
                entry.nickname = cached_profile.nickname.clone();
            }
            if entry.avatar_url.is_none() {
                entry.avatar_url = cached_profile.avatar_url.clone();
            }
            if entry.score.is_none() {
                entry.score = cached_profile.score;
            }
            if entry.season.is_none() {
                entry.season = cached_profile.season.clone();
            }
            if cached_profile.player_identity.is_some() {
                entry.player_identity = cached_profile.player_identity.clone();
            }
            if cached_profile.high_risk.is_some() {
                entry.high_risk = cached_profile.high_risk;
            }
            if cached_profile.reputation_requires_verification.is_some() {
                entry.reputation_requires_verification =
                    cached_profile.reputation_requires_verification;
            }
            if cached_profile.reputation_points.is_some() {
                entry.reputation_points = cached_profile.reputation_points;
                entry.reputation_level = cached_profile.reputation_level.clone();
            }
            entry.found = true;
        }
        query_ids
            .iter()
            .filter_map(|id| merged.remove(id))
            .collect::<Vec<_>>()
    })
    .await
    .map_err(|error| error.to_string())?;
    let previous_for_avatar = previous.clone();
    let fresh = tauri::async_runtime::spawn_blocking(move || {
        fresh
            .into_iter()
            .map(|mut profile| {
                let steam_id = profile.steam_id.clone();
                cache_perfect_profile_avatar(&mut profile, previous_for_avatar.get(&steam_id));
                profile
            })
            .collect::<Vec<_>>()
    })
    .await
    .map_err(|error| error.to_string())?;

    let state = app.state::<AppState>();
    let mut data = state.data.lock().map_err(|error| error.to_string())?;
    let mut changed = false;
    for profile in fresh {
        let is_new = !data.perfect_profiles.contains_key(&profile.steam_id);
        let saved = data
            .perfect_profiles
            .entry(profile.steam_id.clone())
            .or_insert_with(|| profile.clone());
        let before = saved.clone();
        merge_perfect_profile(saved, profile);
        changed |= is_new || *saved != before;
    }
    if changed {
        save_data(&data)?;
    }
    let mut profiles = steam_ids
        .iter()
        .filter_map(|id| data.perfect_profiles.get(id).cloned())
        .collect::<Vec<_>>();
    drop(data);
    hydrate_perfect_profile_avatars(&mut profiles);
    Ok(profiles)
}

#[tauri::command]
async fn switch_perfect_web_account(
    app: AppHandle,
    session_id: String,
) -> Result<SwitchResult, String> {
    switch_perfect_web_account_impl(app, session_id, true).await
}

async fn switch_perfect_web_account_impl(
    app: AppHandle,
    session_id: String,
    acquire_activity: bool,
) -> Result<SwitchResult, String> {
    let _activity = acquire_activity
        .then(|| acquire_switch_activity(&app))
        .transpose()?;
    let (steam_id, display_name) = {
        let state = app.state::<AppState>();
        let data = state.data.lock().map_err(|error| error.to_string())?;
        let session = data
            .steam
            .web_sessions
            .iter()
            .find(|session| session.id == session_id)
            .ok_or_else(|| "Steam 网页会话不存在".to_string())?;
        let steam_id = session
            .steam_id
            .clone()
            .ok_or_else(|| "请先登录并识别该 Steam 网页账号".to_string())?;
        let display_name = data
            .perfect_profiles
            .get(&steam_id)
            .and_then(|profile| profile.nickname.as_deref())
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| format!("完美账号 · {}", tray_identifier_suffix(&steam_id)));
        (steam_id, display_name)
    };
    let installation = perfect_arena::discover_installation()?;
    let installation_for_prepare = installation.clone();
    let started_at = tauri::async_runtime::spawn_blocking(move || {
        perfect_arena::prepare_oauth_login(&installation_for_prepare)
    })
    .await
    .map_err(|_| "完美切号后台任务异常终止，请重试".to_string())??;

    let label = steam_web_window_label(&session_id);
    if let Some(window) = app.get_webview_window(&label) {
        window
            .destroy()
            .map_err(|_| "关闭旧 Steam 授权窗口失败，请关闭后重试".to_string())?;
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    let (oauth_window, oauth_loop_detected) = build_perfect_oauth_window(&app, &session_id)?;
    let oauth_cancelled = Arc::new(AtomicBool::new(false));
    let cancel_on_close = oauth_cancelled.clone();
    let cleanup_session_id = session_id.clone();
    oauth_window.on_window_event(move |event| {
        if matches!(
            event,
            WindowEvent::CloseRequested { .. } | WindowEvent::Destroyed
        ) {
            cancel_on_close.store(true, Ordering::SeqCst);
            if matches!(event, WindowEvent::Destroyed) {
                schedule_steam_webview_cache_cleanup(cleanup_session_id.clone());
            }
        }
    });
    let target_id = steam_id.clone();
    let wait_cancelled = oauth_cancelled.clone();
    let login_result = tauri::async_runtime::spawn_blocking(move || {
        perfect_arena::wait_for_oauth_login(
            &target_id,
            started_at,
            Duration::from_secs(120),
            wait_cancelled,
            oauth_loop_detected,
        )
    })
    .await
    .map_err(|_| "等待完美账号登录的后台任务异常终止，请重试".to_string())?;
    if login_result.is_ok() {
        let _ = oauth_window.destroy();
    }
    login_result?;
    let all_steam_ids = {
        let state = app.state::<AppState>();
        let data = state.data.lock().map_err(|error| error.to_string())?;
        data.steam
            .web_sessions
            .iter()
            .filter_map(|session| session.steam_id.clone())
            .chain(data.steam.accounts.iter().map(|account| account.id.clone()))
            .collect::<HashSet<_>>()
            .into_iter()
            .collect::<Vec<_>>()
    };
    let _ = refresh_perfect_profiles_for_ids(&app, all_steam_ids, false).await;
    Ok(SwitchResult {
        ok: true,
        message: format!("已通过 Steam 网页认证切换完美账号：{}", display_name),
    })
}

#[tauri::command]
async fn get_perfect_arena_workspace(
    state: State<'_, AppState>,
) -> Result<perfect_arena::PerfectArenaWorkspace, String> {
    let steam = state
        .data
        .lock()
        .map_err(|error| error.to_string())?
        .steam
        .clone();
    Ok(perfect_arena::workspace(&steam))
}

#[tauri::command]
async fn get_perfect_arena_profiles(
    app: AppHandle,
) -> Result<Vec<perfect_arena::PerfectArenaProfile>, String> {
    let steam_ids = {
        let state = app.state::<AppState>();
        let data = state.data.lock().map_err(|error| error.to_string())?;
        data.steam
            .web_sessions
            .iter()
            .filter_map(|session| session.steam_id.clone())
            .chain(data.steam.accounts.iter().map(|account| account.id.clone()))
            .collect::<HashSet<_>>()
            .into_iter()
            .collect::<Vec<_>>()
    };
    let profiles = refresh_perfect_profiles_for_ids(&app, steam_ids, false).await?;
    update_tray(&app);
    Ok(profiles)
}

#[tauri::command]
fn set_perfect_account_unavailable(
    app: AppHandle,
    steam_id: String,
    unavailable: bool,
) -> Result<Vec<String>, String> {
    if !is_valid_steam_id64(&steam_id) {
        return Err("完美账号 SteamID 无效".to_string());
    }
    let state = app.state::<AppState>();
    let account_ids = commit_app_data_update(&state, |data| {
        if !data
            .steam
            .web_sessions
            .iter()
            .any(|session| session.steam_id.as_deref() == Some(&steam_id))
        {
            return Err("完美账号不存在".to_string());
        }
        if unavailable {
            data.perfect_unavailable_account_ids
                .insert(steam_id.clone());
        } else {
            data.perfect_unavailable_account_ids.remove(&steam_id);
        }
        let mut account_ids = data
            .perfect_unavailable_account_ids
            .iter()
            .cloned()
            .collect::<Vec<_>>();
        account_ids.sort();
        Ok(account_ids)
    })?;
    update_tray(&app);
    let _ = app.emit("app-data-changed", ());
    Ok(account_ids)
}

#[tauri::command]
async fn discover_perfect_arena(
    app: AppHandle,
) -> Result<perfect_arena::PerfectArenaWorkspace, String> {
    if app
        .state::<AppState>()
        .data
        .lock()
        .map_err(|error| error.to_string())?
        .steam
        .accounts
        .is_empty()
    {
        discover_steam(app.clone()).await?;
    }
    get_perfect_arena_workspace(app.state::<AppState>()).await
}

#[derive(Serialize, Deserialize)]
struct StagedDeletionMarker {
    original: PathBuf,
    staged: PathBuf,
    committed: bool,
}

struct StagedDeletion {
    original: PathBuf,
    staged: PathBuf,
    marker: PathBuf,
}

fn stage_for_deletion(path: &Path) -> Result<Option<StagedDeletion>, String> {
    if !path.exists() {
        return Ok(None);
    }
    let trash = recovery_dir()?.join("trash");
    fs::create_dir_all(&trash).map_err(|error| format!("创建删除暂存区失败: {error}"))?;
    let id = Uuid::new_v4().to_string();
    let staged = trash.join(format!("{id}.data"));
    let marker = trash.join(format!("{id}.json"));
    fs::rename(path, &staged).map_err(|error| format!("暂存待删除数据失败: {error}"))?;
    let marker_data = StagedDeletionMarker {
        original: path.to_path_buf(),
        staged: staged.clone(),
        committed: false,
    };
    if let Err(error) = fs::write(
        &marker,
        serde_json::to_vec(&marker_data).map_err(|error| error.to_string())?,
    ) {
        let _ = fs::rename(&staged, path);
        return Err(format!("记录删除事务失败: {error}"));
    }
    Ok(Some(StagedDeletion {
        original: path.to_path_buf(),
        staged,
        marker,
    }))
}

fn rollback_staged_deletion(staged: &StagedDeletion) {
    if staged.staged.exists() && !staged.original.exists() {
        let _ = fs::rename(&staged.staged, &staged.original);
    }
    let _ = fs::remove_file(&staged.marker);
}

fn mark_staged_deletion_committed(staged: &StagedDeletion) {
    let marker_data = StagedDeletionMarker {
        original: staged.original.clone(),
        staged: staged.staged.clone(),
        committed: true,
    };
    if let Ok(raw) = serde_json::to_vec(&marker_data) {
        let _ = fs::write(&staged.marker, raw);
    }
}

fn finish_staged_deletion(staged: Option<StagedDeletion>) {
    if let Some(staged) = staged {
        let _ = fs::remove_dir_all(&staged.staged);
        if !staged.staged.exists() {
            let _ = fs::remove_file(staged.marker);
        }
    }
}

fn recover_staged_deletions_in(storage_root: &Path, trash: &Path) {
    for entry in fs::read_dir(trash).into_iter().flatten().flatten() {
        let marker_path = entry.path();
        if marker_path.extension().and_then(|value| value.to_str()) != Some("json") {
            continue;
        }
        let Some(marker) = fs::read(&marker_path)
            .ok()
            .and_then(|raw| serde_json::from_slice::<StagedDeletionMarker>(&raw).ok())
        else {
            continue;
        };
        if !marker.original.starts_with(storage_root) || !marker.staged.starts_with(trash) {
            continue;
        }
        let resolved = if marker.committed {
            let _ = fs::remove_dir_all(&marker.staged);
            !marker.staged.exists()
        } else if marker.staged.exists() && !marker.original.exists() {
            if let Some(parent) = marker.original.parent() {
                let _ = fs::create_dir_all(parent);
            }
            let _ = fs::rename(&marker.staged, &marker.original);
            marker.original.exists()
        } else {
            marker.original.exists()
        };
        if resolved {
            let _ = fs::remove_file(marker_path);
        }
    }
}

fn recover_staged_deletions(storage_root: &Path) {
    recover_staged_deletions_in(storage_root, &storage_root.join("trash"));
    recover_staged_deletions_in(storage_root, &storage_root.join("recovery").join("trash"));
}

fn persist_actual_steam_client_state(
    state: &AppState,
    installation: &steam::SteamInstallation,
    expected_current_id: Option<&str>,
    native_switcher_exclusion: Option<&str>,
) -> Result<Option<String>, String> {
    let mut accounts = steam::SteamAdapter::read_accounts_stable(installation)?;
    let current_account_id = apply_actual_steam_active_user(&mut accounts);
    if expected_current_id.is_some() && current_account_id.as_deref() != expected_current_id {
        return Err("Steam 实际登录账号与目标账号不一致".to_string());
    }
    commit_app_data_update(state, |data| {
        data.steam.accounts = accounts;
        data.steam
            .current_account_id
            .clone_from(&current_account_id);
        if let Some(steam_id) = native_switcher_exclusion {
            data.steam_native_switcher_exclusions
                .insert(steam_id.to_string());
        }
        reconcile_saved_steam_credentials(data);
        Ok(current_account_id)
    })
}

#[tauri::command]
async fn switch_steam_account(app: AppHandle, account_id: String) -> Result<SwitchResult, String> {
    switch_steam_account_impl(app, account_id, true).await
}

async fn switch_steam_account_impl(
    app: AppHandle,
    account_id: String,
    acquire_activity: bool,
) -> Result<SwitchResult, String> {
    let _activity = acquire_activity
        .then(|| acquire_switch_activity(&app))
        .transpose()?;
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        let _operation = state
            .switch_operation
            .lock()
            .map_err(|error| error.to_string())?;
        let (
            installation,
            mut saved_credential,
            mut previous_login,
            native_switcher_exclusions,
        ) = {
            let mut data = state
                .data
                .lock()
                .map_err(|error| error.to_string())?
                .clone();
            reconcile_steam_identities(&mut data);
            let installation = data
                .steam
                .installation
                .clone()
                .ok_or_else(|| "请先搜索 Steam 安装目录".to_string())?;
            let identity = data
                .steam_identities
                .iter()
                .find(|identity| identity.steam_id.as_deref() == Some(&account_id))
                .ok_or_else(|| "Steam 账号不存在或尚未识别 64 位 SteamID".to_string())?;
            let credential = credential_for_steam_identity(
                &data,
                Some(&account_id),
                identity.account_name.as_deref(),
            )
            .cloned()
            .ok_or_else(|| "该 Steam 账号没有保存账号密码，无法打开客户端".to_string())?;
            let active_account_id = steam::SteamAdapter::client_is_running()
                .then(steam::SteamAdapter::active_user_account_id)
                .flatten();
            let previous_account_id = data
                .steam
                .accounts
                .iter()
                .find(|account| {
                    active_account_id.is_some()
                        && steam::SteamAdapter::account_id32(&account.id) == active_account_id
                })
                .map(|account| account.id.clone());
            let previous_login = previous_account_id
                .as_deref()
                .filter(|previous_id| *previous_id != account_id)
                .and_then(|previous_id| {
                    let previous_identity = data
                        .steam_identities
                        .iter()
                        .find(|identity| identity.steam_id.as_deref() == Some(previous_id));
                    credential_for_steam_identity(
                        &data,
                        Some(previous_id),
                        previous_identity.and_then(|identity| identity.account_name.as_deref()),
                    )
                    .cloned()
                    .map(|credential| (previous_id.to_string(), credential))
                });
            (
                installation,
                credential,
                previous_login,
                data.steam_native_switcher_exclusions.clone(),
            )
        };
        let adapter = steam::SteamAdapter;
        let adapter_installation = adapters::AppInstallation {
            executable: PathBuf::from(&installation.executable),
            data_dir: PathBuf::from(&installation.install_dir),
        };
        let was_running = adapter.is_running(&adapter_installation);
        if was_running && steam::SteamAdapter::is_account_active(&account_id) {
            return Ok(SwitchResult {
                ok: true,
                message: format!("Steam 账号 {} 已在客户端登录", saved_credential.account_name),
            });
        }
        adapter.stop(&adapter_installation)?;
        if let Err(error) = steam::SteamAdapter::suppress_accounts_from_native_switcher(
            &installation,
            &native_switcher_exclusions,
        ) {
            let _ = app.emit(
                "steam-client-switch-progress",
                format!("Steam 已退出，但暂时无法整理原生最近账号：{error}"),
            );
        }
        let switch_result = (|| -> Result<String, String> {
            let _ = app.emit(
                "steam-client-switch-progress",
                "正在使用已保存账号密码登录 Steam 客户端...",
            );
            let account_name = saved_credential.account_name.clone();
            let start_result = steam::SteamAdapter::start_with_credentials(
                &adapter_installation,
                &saved_credential.account_name,
                &saved_credential.password,
            );
            clear_sensitive_string(&mut saved_credential.password);
            start_result?;
            let login_outcome = wait_for_actual_steam_login(
                &adapter_installation,
                &account_id,
                Duration::from_secs(300),
                |elapsed| {
                    let message = if elapsed < Duration::from_secs(60) {
                        "Steam 正在连接登录服务，请稍候..."
                    } else if elapsed < Duration::from_secs(180) {
                        "Steam 网络响应较慢，NEA 将继续等待而不会提前判定失败..."
                    } else {
                        "仍在等待 Steam 完成登录；如有 Steam Guard 窗口，请先完成验证..."
                    };
                    let _ = app.emit("steam-client-switch-progress", message);
                },
            )?;
            match login_outcome {
                SteamLoginWaitOutcome::LoggedIn => {}
                SteamLoginWaitOutcome::ClientExited => {
                    return Err("Steam 客户端在登录完成前意外退出".to_string());
                }
                SteamLoginWaitOutcome::TimedOut => {
                    return Err(
                        "Steam 未在五分钟内完成目标账号登录，可能需要 Steam Guard 验证或当前网络无法连接登录服务".to_string(),
                    );
                }
            }
            let _ = app.emit(
                "steam-client-switch-progress",
                "Steam 已确认目标账号实际登录，正在同步状态...",
            );
            let _ = steam::SteamAdapter::keep_account_out_of_native_switcher(
                installation.clone(),
                account_id.clone(),
            );
            let current_account_id = persist_actual_steam_client_state(
                &state,
                &installation,
                Some(&account_id),
                Some(&account_id),
            )?;
            debug_assert_eq!(current_account_id.as_deref(), Some(account_id.as_str()));
            Ok(account_name)
        })();
        let account_name = match switch_result {
            Ok(account_name) => account_name,
            Err(error) => {
                clear_sensitive_string(&mut saved_credential.password);
                let stop_result = adapter.stop(&adapter_installation);
                if stop_result.is_ok() {
                    let mut cleanup_ids = native_switcher_exclusions.clone();
                    cleanup_ids.insert(account_id.clone());
                    let _ = steam::SteamAdapter::suppress_accounts_from_native_switcher(
                        &installation,
                        &cleanup_ids,
                    );
                }
                let recovery_result = if let Err(stop_error) = stop_result {
                    Some(Err(format!("目标 Steam 无法完全退出：{stop_error}")))
                } else if was_running {
                    previous_login.as_mut().map(|(previous_id, credential)| {
                        let _ = app.emit(
                            "steam-client-switch-progress",
                            "目标账号登录未完成，正在恢复原 Steam 账号...",
                        );
                        let result = (|| -> Result<(), String> {
                            let start_result = steam::SteamAdapter::start_with_credentials(
                                &adapter_installation,
                                &credential.account_name,
                                &credential.password,
                            );
                            clear_sensitive_string(&mut credential.password);
                            start_result?;
                            let outcome = wait_for_actual_steam_login(
                                &adapter_installation,
                                previous_id,
                                Duration::from_secs(180),
                                |_| {},
                            )?;
                            match outcome {
                                SteamLoginWaitOutcome::LoggedIn => {}
                                SteamLoginWaitOutcome::ClientExited => {
                                    return Err("Steam 客户端在恢复期间意外退出".to_string());
                                }
                                SteamLoginWaitOutcome::TimedOut => {
                                    return Err("三分钟内未确认原 Steam 账号恢复".to_string());
                                }
                            }
                            persist_actual_steam_client_state(
                                &state,
                                &installation,
                                Some(previous_id),
                                None,
                            )?;
                            if native_switcher_exclusions.contains(previous_id) {
                                let _ = steam::SteamAdapter::keep_account_out_of_native_switcher(
                                    installation.clone(),
                                    previous_id.clone(),
                                );
                            }
                            let _ = app.emit("app-data-changed", ());
                            Ok(())
                        })();
                        clear_sensitive_string(&mut credential.password);
                        result
                    })
                } else {
                    None
                };
                return match recovery_result {
                    Some(Ok(())) => Err(format!("{error}；已恢复原 Steam 账号")),
                    Some(Err(recovery_error)) => {
                        Err(format!("{error}；恢复原 Steam 账号失败：{recovery_error}"))
                    }
                    None if was_running && previous_login.is_none() => {
                        Err(format!("{error}；原账号没有保存账密，无法自动恢复"))
                    }
                    None => Err(error),
                };
            }
        };
        clear_sensitive_string(&mut saved_credential.password);
        if let Some((_, credential)) = previous_login.as_mut() {
            clear_sensitive_string(&mut credential.password);
        }
        Ok(SwitchResult {
            ok: true,
            message: format!("已切换到 Steam 账号 {}", account_name),
        })
    })
    .await
    .map_err(|error| error.to_string())?
}

async fn wait_for_steam_capability_permission(app: &AppHandle) -> bool {
    let mut pause_announced = false;
    loop {
        let state = app.state::<AppState>();
        if state.steam_capability_cancelled.load(Ordering::SeqCst) {
            return false;
        }
        if !state.steam_capability_paused.load(Ordering::SeqCst) {
            return true;
        }
        if !pause_announced {
            let _ = app.emit(
                "steam-capability-progress",
                "登录方式补全已暂停，点击继续后处理下一安全步骤",
            );
            pause_announced = true;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}

#[tauri::command]
fn get_steam_capability_status(app: AppHandle) -> SteamCapabilityStatus {
    steam_capability_status(&app)
}

#[tauri::command]
fn set_steam_capability_paused(
    app: AppHandle,
    paused: bool,
) -> Result<SteamCapabilityStatus, String> {
    let state = app.state::<AppState>();
    if !state.steam_capability_running.load(Ordering::SeqCst) {
        return Err("Steam 登录方式补全未在运行".to_string());
    }
    if state.steam_capability_cancelled.load(Ordering::SeqCst) {
        return Err("Steam 登录方式补全正在取消".to_string());
    }
    state
        .steam_capability_paused
        .store(paused, Ordering::SeqCst);
    let message = if paused {
        "已请求暂停，将在当前安全步骤结束后暂停"
    } else {
        "已继续 Steam 登录方式补全"
    };
    let _ = app.emit("steam-capability-progress", message);
    Ok(emit_steam_capability_status(&app))
}

#[tauri::command]
fn cancel_steam_capability_completion(app: AppHandle) -> SteamCapabilityStatus {
    let state = app.state::<AppState>();
    if state.steam_capability_running.load(Ordering::SeqCst) {
        state
            .steam_capability_cancelled
            .store(true, Ordering::SeqCst);
        state.steam_capability_paused.store(false, Ordering::SeqCst);
        let _ = app.emit(
            "steam-capability-progress",
            "正在取消 Steam 网页登录补全...",
        );
    }
    emit_steam_capability_status(&app)
}

#[tauri::command]
async fn complete_steam_capabilities(
    app: AppHandle,
) -> Result<SteamCapabilityCompletionResult, String> {
    let _activity = acquire_switch_activity(&app)?;
    let _import_guard = acquire_steam_web_import(&app)?;
    let _capability_guard = acquire_steam_capability(&app)?;
    let _ = app.emit(
        "steam-capability-progress",
        "正在检查已保存账密对应的 Steam 网页登录...",
    );
    let has_web_sessions = !app
        .state::<AppState>()
        .data
        .lock()
        .map_err(|error| error.to_string())?
        .steam
        .web_sessions
        .is_empty();
    if has_web_sessions {
        refresh_steam_web_sessions(app.clone())
            .await
            .map_err(|error| format!("无法验证现有 Steam 网页登录状态，已停止补全：{error}"))?;
    }
    let credentials = {
        let state = app.state::<AppState>();
        let data = state.data.lock().map_err(|error| error.to_string())?;
        data.steam_credentials.clone()
    };
    if credentials.is_empty() {
        return Err("没有已保存账密的 Steam 账号".to_string());
    }

    let checked = credentials.len();
    let mut processed = 0usize;
    let mut already_complete = 0usize;
    let mut web_completed = 0usize;
    let mut cancelled = false;
    let mut verification_required_accounts = Vec::new();
    let mut failed_accounts = Vec::new();
    let web_started = Arc::new(AtomicUsize::new(0));
    let web_completed_count = Arc::new(AtomicUsize::new(0));

    for (index, mut credential) in credentials.into_iter().enumerate() {
        if !wait_for_steam_capability_permission(&app).await {
            cancelled = true;
            clear_sensitive_string(&mut credential.password);
            break;
        }
        let account_label = credential.account_name.clone();
        let _ = app.emit(
            "steam-capability-progress",
            format!("正在检查 {}（{}/{checked}）", account_label, index + 1),
        );
        let identity = {
            let state = app.state::<AppState>();
            let mut data = state.data.lock().map_err(|error| error.to_string())?;
            steam_identity_for_account_name(&mut data, &credential.account_name)
        };
        if identity
            .as_ref()
            .is_some_and(|identity| identity.capabilities.web_login)
        {
            already_complete += 1;
            processed += 1;
            clear_sensitive_string(&mut credential.password);
            continue;
        }

        if !identity
            .as_ref()
            .is_some_and(|identity| identity.capabilities.web_login)
        {
            let result = import_single_steam_web_account(
                app.clone(),
                SteamCredentialInput {
                    account: credential.account_name.clone(),
                    password: credential.password.clone(),
                },
                web_started.clone(),
                web_completed_count.clone(),
                checked,
                false,
                true,
            )
            .await;
            match result.outcome {
                SteamImportAccountOutcome::Verified => web_completed += 1,
                SteamImportAccountOutcome::SkippedExisting => already_complete += 1,
                SteamImportAccountOutcome::InvalidCredentials => {
                    failed_accounts.push(format!("{}：账号或密码错误", account_label));
                }
                SteamImportAccountOutcome::HasToken => {
                    failed_accounts.push(format!("{}：有令牌", account_label));
                }
                SteamImportAccountOutcome::VerificationRequired => {
                    verification_required_accounts.push(account_label.clone());
                }
                SteamImportAccountOutcome::Failed => {
                    failed_accounts.push(format!("{}：网页登录失败", account_label));
                }
                SteamImportAccountOutcome::Cancelled => cancelled = true,
            }
            if cancelled {
                processed += 1;
                clear_sensitive_string(&mut credential.password);
                break;
            }
        }
        processed += 1;
        clear_sensitive_string(&mut credential.password);
        if cancelled {
            break;
        }
    }

    cancelled |= app
        .state::<AppState>()
        .steam_capability_cancelled
        .load(Ordering::SeqCst);

    {
        let state = app.state::<AppState>();
        let mut data = state.data.lock().map_err(|error| error.to_string())?;
        reconcile_steam_identities(&mut data);
        save_data(&data)?;
    }
    update_tray(&app);
    let _ = app.emit("app-data-changed", ());
    let _ = app.emit(
        "steam-capability-progress",
        if cancelled {
            "Steam 网页登录补全已取消"
        } else {
            "Steam 网页登录检查完成"
        },
    );
    Ok(SteamCapabilityCompletionResult {
        checked,
        processed,
        already_complete,
        web_completed,
        cancelled,
        verification_required_accounts,
        failed_accounts,
    })
}

#[tauri::command]
async fn switch_steam_and_perfect_account(
    app: AppHandle,
    session_id: String,
) -> Result<SwitchResult, String> {
    let _activity = acquire_switch_activity(&app)?;
    let (steam_id, has_saved_credential) = {
        let state = app.state::<AppState>();
        let mut data = state
            .data
            .lock()
            .map_err(|error| error.to_string())?
            .clone();
        reconcile_steam_identities(&mut data);
        if !data
            .steam
            .web_sessions
            .iter()
            .any(|session| session.id == session_id)
        {
            return Err("完美网页账号不存在".to_string());
        }
        let identity = data
            .steam_identities
            .iter()
            .find(|identity| identity.web_session_id.as_deref() == Some(&session_id))
            .ok_or_else(|| "完美网页账号尚未归入 Steam 核心身份".to_string())?;
        let steam_id = identity
            .steam_id
            .clone()
            .ok_or_else(|| "请先登录并识别该 Steam 核心身份".to_string())?;
        let has_saved_credential =
            credential_for_steam_identity(&data, Some(&steam_id), identity.account_name.as_deref())
                .is_some();
        (steam_id, has_saved_credential)
    };
    let steam_is_current = steam::SteamAdapter::is_account_logged_in(&steam_id);

    let steam_was_switched = !steam_is_current;
    if steam_was_switched {
        if !has_saved_credential {
            return Err("该 Steam 账号没有保存账号密码，无法同步登录客户端".to_string());
        }
        switch_steam_account_impl(app.clone(), steam_id.clone(), false).await?;
    }
    let perfect_result = switch_perfect_web_account_impl(app, session_id, false)
        .await
        .map_err(|error| {
            if steam_was_switched {
                format!("Steam 已切换，但完美账号切换未完成: {error}")
            } else {
                error
            }
        })?;
    Ok(SwitchResult {
        ok: perfect_result.ok,
        message: "Steam 与完美账号已同步切换".to_string(),
    })
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
        remove_installed_watchers().map_err(|error| format!("清理守护进程失败: {}", error))?;
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
        remove_installed_watchers().map_err(|error| format!("清理守护进程失败: {}", error))?;
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

fn merge_imported_oopz_accounts(
    accounts: &mut Vec<SavedAccount>,
    imported: &[SavedAccount],
) -> Result<(), String> {
    for incoming in imported {
        let id_position = accounts.iter().position(|saved| saved.id == incoming.id);
        let uid_position = incoming.uid.as_ref().and_then(|uid| {
            accounts
                .iter()
                .position(|saved| saved.uid.as_ref() == Some(uid))
        });
        match (id_position, uid_position) {
            (Some(id_index), Some(uid_index)) if id_index != uid_index => {
                return Err(format!(
                    "OOPZ 账号在导入期间发生身份冲突，请重新导入：{}",
                    incoming.display_name
                ));
            }
            (Some(index), _) => accounts[index] = incoming.clone(),
            (None, Some(_)) => {
                return Err(format!(
                    "OOPZ 账号在导入期间发生变化，请重新导入：{}",
                    incoming.display_name
                ));
            }
            (None, None) => accounts.push(incoming.clone()),
        }
    }
    Ok(())
}

fn import_account_inner(
    app: AppHandle,
    state: State<AppState>,
    uid: String,
) -> Result<SavedAccount, String> {
    let _operation = state.account_operation.lock().map_err(|e| e.to_string())?;
    let data = state.data.lock().map_err(|e| e.to_string())?.clone();
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
    let snapshot_parent = snapshot
        .parent()
        .ok_or_else(|| "账号快照目录无效".to_string())?;
    fs::create_dir_all(snapshot_parent).map_err(|error| error.to_string())?;
    let transaction_id = Uuid::new_v4();
    let prepared_snapshot = snapshot_parent.join(format!(".nea-import-{transaction_id}"));
    let previous_snapshot = snapshot_parent.join(format!(".nea-before-import-{transaction_id}"));
    let preparation = (|| -> Result<(), String> {
        if snapshot.exists() {
            copy_dir_contents(&snapshot, &prepared_snapshot)?;
        } else {
            fs::create_dir_all(&prepared_snapshot).map_err(|error| error.to_string())?;
        }
        copy_dir_recursive(&roaming_src, &prepared_snapshot.join("roaming").join(&uid))?;
        copy_dir_recursive(
            &local_src,
            &prepared_snapshot.join("local_sandbox").join(&uid),
        )?;
        Ok(())
    })();
    if let Err(error) = preparation {
        let _ = fs::remove_dir_all(&prepared_snapshot);
        return Err(error);
    }
    let had_snapshot = snapshot.exists();
    if had_snapshot {
        if let Err(error) = fs::rename(&snapshot, &previous_snapshot) {
            let _ = fs::remove_dir_all(&prepared_snapshot);
            return Err(format!("暂存原账号快照失败: {error}"));
        }
    }
    if let Err(error) = fs::rename(&prepared_snapshot, &snapshot) {
        let rollback = rollback_replaced_dir(&snapshot, &previous_snapshot, had_snapshot).err();
        let _ = fs::remove_dir_all(&prepared_snapshot);
        return match rollback {
            Some(rollback) => Err(format!("替换账号快照失败: {error}；恢复失败: {rollback}")),
            None => Err(format!("替换账号快照失败: {error}")),
        };
    }

    let previous_secret = has_login_state.then(|| read_secret_raw(&id));
    let rollback_import = || -> Result<(), String> {
        let mut first_error = None;
        if let Some(previous) = &previous_secret {
            if let Some(raw) = previous {
                if let Err(error) = write_secret_raw(&id, raw) {
                    first_error.get_or_insert(error);
                }
            } else {
                delete_credential(&id);
            }
        }
        if let Err(error) = rollback_replaced_dir(&snapshot, &previous_snapshot, had_snapshot) {
            first_error.get_or_insert(error);
        }
        first_error.map_or(Ok(()), Err)
    };
    if has_login_state {
        if let Some(login) = registry_login.as_deref() {
            if let Err(error) = store_oopz_login(&id, login) {
                let rollback = rollback_import().err();
                return match rollback {
                    Some(rollback) => {
                        Err(format!("保存账号凭据失败: {error}；恢复失败: {rollback}"))
                    }
                    None => Err(error),
                };
            }
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

    let committed = commit_app_data_update(&state, |latest| {
        merge_imported_oopz_accounts(&mut latest.accounts, std::slice::from_ref(&account))?;
        Ok(account.clone())
    });
    let committed = match committed {
        Ok(committed) => committed,
        Err(error) => {
            let rollback = rollback_import().err();
            return match rollback {
                Some(rollback) => Err(format!("{error}；恢复账号快照失败: {rollback}")),
                None => Err(error),
            };
        }
    };
    if had_snapshot {
        let _ = fs::remove_dir_all(&previous_snapshot);
    }
    update_tray(&app);
    Ok(committed)
}

fn collect_export_paths(
    root: &Path,
    current: &Path,
    files: &mut Vec<(PathBuf, String, u64)>,
    total_size: &mut u64,
    cancelled: Option<&AtomicBool>,
) -> Result<(), String> {
    ensure_share_packaging_not_cancelled(cancelled)?;
    if !current.exists() {
        return Ok(());
    }
    for entry in fs::read_dir(current).map_err(|e| e.to_string())? {
        ensure_share_packaging_not_cancelled(cancelled)?;
        let entry = entry.map_err(|e| e.to_string())?;
        let path = entry.path();
        let file_type = entry.file_type().map_err(|e| e.to_string())?;
        if file_type.is_dir() {
            collect_export_paths(root, &path, files, total_size, cancelled)?;
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

fn ensure_share_packaging_not_cancelled(cancelled: Option<&AtomicBool>) -> Result<(), String> {
    if cancelled.is_some_and(|cancelled| cancelled.load(Ordering::SeqCst)) {
        Err(QUICK_SHARE_CANCELLED.to_string())
    } else {
        Ok(())
    }
}

fn copy_with_share_cancellation_buffer<R: Read, W: Write>(
    source: &mut R,
    target: &mut W,
    cancelled: Option<&AtomicBool>,
    buffer: &mut [u8],
) -> Result<u64, String> {
    let mut total = 0u64;
    loop {
        ensure_share_packaging_not_cancelled(cancelled)?;
        let read = source.read(buffer).map_err(|error| error.to_string())?;
        if read == 0 {
            return Ok(total);
        }
        target
            .write_all(&buffer[..read])
            .map_err(|error| error.to_string())?;
        total = total
            .checked_add(read as u64)
            .ok_or_else(|| "分享包数据大小溢出".to_string())?;
    }
}

fn write_export_package_v3(
    path: &Path,
    accounts: &[SavedAccount],
    cancelled: Option<&AtomicBool>,
) -> Result<(), String> {
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
        ensure_share_packaging_not_cancelled(cancelled)?;
        let oopz_login = read_oopz_login(&account.id)
            .ok_or_else(|| format!("{} 还不能导出，请先登录一次", account.display_name))?;
        validate_exported_oopz_identity(
            &ExportedAccount {
                display_name: account.display_name.clone(),
                uid: account.uid.clone(),
                pid: account.pid.clone(),
                user_common_id: account.user_common_id.clone(),
                masked_phone: account.masked_phone.clone(),
                avatar_url: account.avatar_url.clone(),
                note: account.note.clone(),
            },
            &oopz_login,
        )?;
        let snapshot = account_snapshot_dir(&account.id)?;
        let mut files = Vec::new();
        collect_export_paths(&snapshot, &snapshot, &mut files, &mut total_size, cancelled)?;
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
        let mut copy_buffer = vec![0u8; 256 * 1024];
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
                ensure_share_packaging_not_cancelled(cancelled)?;
                archive
                    .start_file(format!("accounts/{}/{}", directory, relative), options)
                    .map_err(|e| e.to_string())?;
                let mut source = fs::File::open(source).map_err(|e| e.to_string())?;
                copy_with_share_cancellation_buffer(
                    &mut source,
                    &mut archive,
                    cancelled,
                    &mut copy_buffer,
                )?;
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
    write_export_package_v3(path, &accounts, None)?;
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
        LEGACY_EXPORT_FORMAT_V2 => {
            serde_json::from_value::<AccountExportPackage>(value)
                .map_err(|e| format!("导入文件格式不正确: {}", e))?
                .accounts
        }
        LEGACY_EXPORT_FORMAT_V1 => {
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
        let uid = validate_exported_oopz_identity(&entry.account, &entry.oopz_login)?;
        if !uids.insert(uid) {
            return Err("导入包包含重复账号".to_string());
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

const MISSING_CREDENTIAL_MARKER: &str = "NEA_TRANSACTION_NO_CREDENTIAL";
const LEGACY_MISSING_CREDENTIAL_MARKER: &str = "OOPZPLUS_TRANSACTION_NO_CREDENTIAL";

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

fn restore_oopz_accounts_from_import_backup(
    root: &Path,
    journal: &ImportJournal,
) -> Result<(), String> {
    let original_data = if journal.config_existed {
        let backup = root.join("config.backup");
        parse_app_data_file(&backup)
            .map(|(data, _)| data)
            .ok_or_else(|| "无法读取导入前的 OOPZ 账号配置".to_string())?
    } else {
        AppData::default()
    };
    let config = config_path()?;
    let (mut latest, requires_verified_recovery) = match recover_config_file(&config) {
        Some((data, _)) => (data, false),
        None => (original_data.clone(), true),
    };
    latest.accounts = original_data.accounts;
    if requires_verified_recovery {
        save_verified_recovery_data(&latest)
    } else {
        save_data(&latest)
    }
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
            let result =
                if raw == MISSING_CREDENTIAL_MARKER || raw == LEGACY_MISSING_CREDENTIAL_MARKER {
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
    if let Err(error) = restore_oopz_accounts_from_import_backup(root, journal) {
        first_error.get_or_insert(error);
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
        }
        let imported = prepared
            .iter()
            .map(|item| item.account.clone())
            .collect::<Vec<_>>();
        let state = app.state::<AppState>();
        commit_app_data_update(&state, |latest| {
            merge_imported_oopz_accounts(&mut latest.accounts, &imported)
        })?;
        journal.status = "committed".to_string();
        write_import_journal(root, &journal)?;
        Ok(imported)
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
        validate_exported_oopz_identity(&package.account, &package.oopz_login)?;
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
    if (manifest.format != LEGACY_EXPORT_FORMAT_V3 && manifest.format != NEA_EXPORT_FORMAT_V1)
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
        let uid = validate_exported_oopz_identity(&item.account, &item.oopz_login)?;
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
        if !imported_uids.insert(uid) {
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
    let imported = commit_prepared_import(app, &root, prepared)?;
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
    emit_wormhole_status_with_package_size(app, state, direction, message, code, progress, None);
}

fn emit_wormhole_status_with_package_size(
    app: &AppHandle,
    state: &str,
    direction: &str,
    message: impl Into<String>,
    code: Option<String>,
    progress: Option<(u64, u64)>,
    package_bytes: Option<u64>,
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
            package_bytes,
        },
    );
}

fn wormhole_relay_hints() -> Result<Vec<transit::RelayHint>, String> {
    let relay_url = NEA_FREE_TRANSIT_RELAY
        .parse()
        .map_err(|e| format!("免费中继地址无效: {}", e))?;
    let preferred =
        transit::RelayHint::from_urls(Some("Winden / Least Authority".to_string()), [relay_url])
            .map_err(|e| format!("免费中继配置失败: {}", e))?;
    Ok(vec![preferred])
}

fn wormhole_temp_package(prefix: &str, extension: &str) -> PathBuf {
    std::env::temp_dir().join(format!("{}-{}.{}", prefix, Uuid::new_v4(), extension))
}

fn is_stale_share_artifact(path: &Path) -> bool {
    path.metadata()
        .and_then(|metadata| metadata.modified())
        .ok()
        .and_then(|modified| modified.elapsed().ok())
        .is_some_and(|age| age >= Duration::from_secs(2 * 60 * 60))
}

fn matches_uuid_artifact(name: &str, prefix: &str, suffix: &str) -> bool {
    name.strip_prefix(prefix)
        .and_then(|value| value.strip_suffix(suffix))
        .is_some_and(|value| Uuid::parse_str(value).is_ok())
}

fn write_quick_share_rollback_journal(
    root: &Path,
    journal: &QuickShareRollbackJournal,
) -> Result<(), String> {
    let path = root.join("rollback-journal.json");
    let temp = root.join("rollback-journal.json.tmp");
    let backup = root.join("rollback-journal.json.bak");
    let raw = serde_json::to_vec_pretty(journal).map_err(|error| error.to_string())?;
    fs::write(&temp, raw).map_err(|error| format!("写入分享回滚记录失败: {error}"))?;
    if path.exists() {
        if backup.exists() {
            fs::remove_file(&backup).map_err(|error| format!("清理旧分享回滚记录失败: {error}"))?;
        }
        fs::rename(&path, &backup).map_err(|error| format!("备份分享回滚记录失败: {error}"))?;
    }
    if let Err(error) = fs::rename(&temp, &path) {
        if backup.exists() {
            let _ = fs::rename(&backup, &path);
        }
        let _ = fs::remove_file(&temp);
        return Err(format!("提交分享回滚记录失败: {error}"));
    }
    if backup.exists() {
        let _ = fs::remove_file(backup);
    }
    Ok(())
}

fn record_quick_share_rollback_path(
    root: &Path,
    journal: &mut QuickShareRollbackJournal,
    target: &Path,
    backup: Option<PathBuf>,
) -> Result<usize, String> {
    let index = journal.paths.len();
    journal.paths.push(QuickShareRollbackPath {
        target: target.to_path_buf(),
        backup,
    });
    if let Err(error) = write_quick_share_rollback_journal(root, journal) {
        journal.paths.pop();
        return Err(error);
    }
    Ok(index)
}

fn apply_quick_share_data_rollback(
    current: &mut AppData,
    affected_steam_ids: &HashSet<String>,
    web_sessions: &[steam::SteamWebSession],
    perfect_profiles: &HashMap<String, perfect_arena::PerfectArenaProfile>,
    added_credentials: &[QuickShareCredentialRollback],
) {
    current.steam.web_sessions.retain(|session| {
        !session
            .steam_id
            .as_ref()
            .is_some_and(|steam_id| affected_steam_ids.contains(steam_id))
    });
    current
        .steam
        .web_sessions
        .extend(web_sessions.iter().cloned());
    for steam_id in affected_steam_ids {
        if let Some(profile) = perfect_profiles.get(steam_id) {
            current
                .perfect_profiles
                .insert(steam_id.clone(), profile.clone());
        } else {
            current.perfect_profiles.remove(steam_id);
        }
    }
    current.steam_credentials.retain(|credential| {
        !added_credentials.iter().any(|added| {
            credential.steam_id.as_deref() == Some(added.steam_id.as_str())
                && normalized_steam_account_name(&credential.account_name)
                    == added.normalized_account_name
                && credential.updated_at == added.updated_at
        })
    });
    reconcile_steam_identities(current);
}

fn valid_quick_share_recovery_target(target: &Path, affected_steam_ids: &HashSet<String>) -> bool {
    let web_root = storage_dir()
        .ok()
        .map(|root| root.join("workspaces").join("steam").join("web-sessions"));
    if web_root.as_deref() == target.parent() {
        return target
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(valid_storage_id);
    }
    let Some(database_dir) = perfect_arena::account_database_dir() else {
        return false;
    };
    if target.parent() != Some(database_dir.as_path()) {
        return false;
    }
    let Some(file_name) = target.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    affected_steam_ids.iter().any(|steam_id| {
        perfect_share_file_names(steam_id)
            .iter()
            .any(|allowed| allowed == file_name)
    })
}

fn recover_quick_share_transaction(root: &Path) -> Result<(), String> {
    if root.join("committed").is_file() {
        fs::remove_dir_all(root).map_err(|error| error.to_string())?;
        return Ok(());
    }
    let journal = [
        root.join("rollback-journal.json"),
        root.join("rollback-journal.json.bak"),
        root.join("rollback-journal.json.tmp"),
    ]
    .into_iter()
    .find_map(|path| {
        fs::read(path)
            .ok()
            .and_then(|raw| serde_json::from_slice::<QuickShareRollbackJournal>(&raw).ok())
    })
    .ok_or_else(|| "分享回滚记录缺失或损坏".to_string())?;
    let affected_steam_ids = journal
        .affected_steam_ids
        .iter()
        .filter(|steam_id| validate_shared_steam_id(steam_id))
        .cloned()
        .collect::<HashSet<_>>();
    if affected_steam_ids.len() != journal.affected_steam_ids.len() {
        return Err("分享回滚记录包含无效 SteamID".to_string());
    }
    let mut added_credential_keys = HashSet::new();
    for credential in &journal.added_credentials {
        if !affected_steam_ids.contains(&credential.steam_id)
            || !validate_shared_steam_id(&credential.steam_id)
            || credential.normalized_account_name.is_empty()
            || normalized_steam_account_name(&credential.normalized_account_name)
                != credential.normalized_account_name
            || chrono::DateTime::parse_from_rfc3339(&credential.updated_at).is_err()
            || !added_credential_keys.insert((
                credential.steam_id.clone(),
                credential.normalized_account_name.clone(),
            ))
        {
            return Err("分享回滚记录包含无效账密账号".to_string());
        }
    }
    if journal.web_sessions.iter().any(|session| {
        !valid_storage_id(&session.id)
            || !session
                .steam_id
                .as_ref()
                .is_some_and(|steam_id| affected_steam_ids.contains(steam_id))
    }) || journal.perfect_profiles.iter().any(|(steam_id, profile)| {
        !affected_steam_ids.contains(steam_id) || profile.steam_id != *steam_id
    }) {
        return Err("分享回滚记录中的账号数据无效".to_string());
    }
    let has_perfect_targets = perfect_arena::account_database_dir().is_some_and(|database_dir| {
        journal
            .paths
            .iter()
            .any(|path| path.target.parent() == Some(database_dir.as_path()))
    });
    if has_perfect_targets {
        perfect_arena::stop_for_share_transfer()?;
    }
    for (index, path) in journal.paths.iter().enumerate().rev() {
        if !valid_quick_share_recovery_target(&path.target, &affected_steam_ids) {
            return Err("分享回滚记录包含不安全目标路径".to_string());
        }
        if let Some(backup) = &path.backup {
            let expected = root.join("rollback").join(format!("item-{index}"));
            if backup != &expected {
                return Err("分享回滚记录包含不安全备份路径".to_string());
            }
            if backup.exists() {
                if path.target.exists() {
                    remove_share_path_checked(&path.target)
                        .map_err(|error| format!("清理半提交分享数据失败: {error}"))?;
                }
                if let Some(parent) = path.target.parent() {
                    fs::create_dir_all(parent).map_err(|error| error.to_string())?;
                }
                rename_share_path(backup, &path.target, "恢复异常退出前分享数据失败")?;
            }
        } else if path.target.exists() {
            remove_share_path_checked(&path.target)
                .map_err(|error| format!("清理异常退出新增分享数据失败: {error}"))?;
        }
    }
    let config = config_path()?;
    let (mut data, _) =
        recover_config_file(&config).ok_or_else(|| "无法读取待恢复的 NEA 配置".to_string())?;
    apply_quick_share_data_rollback(
        &mut data,
        &affected_steam_ids,
        &journal.web_sessions,
        &journal.perfect_profiles,
        &journal.added_credentials,
    );
    save_data(&data)?;
    fs::remove_dir_all(root).map_err(|error| format!("清理已恢复分享事务失败: {error}"))
}

fn recover_quick_share_transactions() {
    let Ok(recovery_root) = storage_dir().map(|root| root.join("recovery")) else {
        return;
    };
    let Ok(entries) = fs::read_dir(recovery_root) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        if !path.is_dir() || !matches_uuid_artifact(&name, "share-import-", "") {
            continue;
        }
        let has_journal = [
            "rollback-journal.json",
            "rollback-journal.json.bak",
            "rollback-journal.json.tmp",
        ]
        .iter()
        .any(|name| path.join(name).is_file());
        if has_journal || path.join("committed").is_file() {
            if let Err(error) = recover_quick_share_transaction(&path) {
                eprintln!("NEA 分享事务恢复失败（{}）：{}", path.display(), error);
            }
        }
    }
}

fn cleanup_stale_share_artifacts() {
    if let Ok(entries) = fs::read_dir(std::env::temp_dir()) {
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().to_string();
            let transfer_file = path.is_file()
                && (matches_uuid_artifact(&name, "nea-share-", ".nea")
                    || matches_uuid_artifact(&name, "nea-share-", ".nea-share")
                    || matches_uuid_artifact(&name, "nea-receive-", ".nea")
                    || matches_uuid_artifact(&name, "nea-receive-", ".nea-share"));
            let import_directory = path.is_dir()
                && matches_uuid_artifact(&name, "nea-share-import-", "")
                && (!directory_has_entries(&path.join("rollback"))
                    || path.join("committed").is_file());
            // 传输临时包可能包含明文 Steam 账密。严格 UUID 命名可确认是 NEA
            // 自己创建的文件，因此启动时立即清理；解包目录仍保留超时保护。
            if transfer_file || (import_directory && is_stale_share_artifact(&path)) {
                remove_share_path(&path);
            }
        }
    }
    if let Ok(recovery_root) = storage_dir().map(|root| root.join("recovery")) {
        if let Ok(entries) = fs::read_dir(recovery_root) {
            for entry in entries.flatten() {
                let path = entry.path();
                let name = entry.file_name().to_string_lossy().to_string();
                if !path.is_dir() || !matches_uuid_artifact(&name, "share-import-", "") {
                    continue;
                }
                let has_journal = [
                    "rollback-journal.json",
                    "rollback-journal.json.bak",
                    "rollback-journal.json.tmp",
                ]
                .iter()
                .any(|file| path.join(file).is_file());
                let safe_to_remove = path.join("committed").is_file() || !has_journal;
                if safe_to_remove && is_stale_share_artifact(&path) {
                    remove_share_path(&path);
                }
            }
        }
    }
    let Ok(root) =
        storage_dir().map(|root| root.join("workspaces").join("steam").join("web-sessions"))
    else {
        return;
    };
    if let Ok(entries) = fs::read_dir(root) {
        for entry in entries.flatten() {
            let path = entry.path();
            if entry
                .file_name()
                .to_string_lossy()
                .strip_prefix("share-stage-")
                .is_some_and(|value| Uuid::parse_str(value).is_ok())
                && is_stale_share_artifact(&path)
            {
                remove_share_path(&path);
            }
        }
    }
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

fn collect_web_session_cookies(
    app: &AppHandle,
    session: &steam::SteamWebSession,
) -> Result<Vec<String>, String> {
    let label = steam_web_window_label(&session.id);
    let (window, temporary) = match app.get_webview_window(&label) {
        Some(window) => (window, false),
        None => (
            build_steam_web_window(app, &session.id, false, false, None)?,
            true,
        ),
    };
    if temporary {
        thread::sleep(Duration::from_millis(200));
    }
    let result = window
        .cookies()
        .map_err(|error| format!("读取 {} 的网页登录态失败: {}", session.display_name, error))?
        .into_iter()
        .filter(is_required_steam_share_cookie)
        .map(|cookie| cookie.to_string())
        .collect::<Vec<_>>();
    if temporary {
        let _ = window.destroy();
    }
    let expected_steam_id = session
        .steam_id
        .as_deref()
        .filter(|steam_id| validate_shared_steam_id(steam_id))
        .ok_or_else(|| format!("{} 缺少有效 SteamID", session.display_name))?;
    let login_cookies = result
        .iter()
        .filter_map(|raw| Cookie::parse(raw.clone()).ok())
        .filter(|cookie| cookie.name().eq_ignore_ascii_case("steamLoginSecure"))
        .collect::<Vec<_>>();
    if login_cookies.is_empty()
        || login_cookies.iter().any(|cookie| {
            steam_id_from_web_cookie(cookie.value()).as_deref() != Some(expected_steam_id)
        })
    {
        return Err(format!(
            "{} 的 Steam 网页登录态已失效或与账号不匹配",
            session.display_name
        ));
    }
    Ok(result)
}

fn steam_account_id_from_store_html(raw: &str) -> Option<u32> {
    let value = raw.split_once("var g_AccountID =")?.1.trim_start();
    let digits = value
        .chars()
        .take_while(|character| character.is_ascii_digit())
        .collect::<String>();
    (!digits.is_empty()).then(|| digits.parse().ok()).flatten()
}

fn steam_store_cookie_header(cookies: &[String]) -> Result<String, String> {
    let host = "store.steampowered.com";
    let mut selected = HashMap::<String, (usize, String)>::new();
    for raw in cookies {
        let cookie = Cookie::parse(raw.clone())
            .map_err(|_| "Steam 网页 Cookie 格式无效".to_string())?
            .into_owned();
        if !is_required_steam_share_cookie(&cookie) {
            continue;
        }
        let Some(domain) = cookie.domain().map(|domain| domain.trim_start_matches('.')) else {
            continue;
        };
        if host != domain && !host.ends_with(&format!(".{domain}")) {
            continue;
        }
        let key = cookie.name().to_ascii_lowercase();
        let candidate = (
            domain.len(),
            format!("{}={}", cookie.name(), cookie.value()),
        );
        if selected
            .get(&key)
            .is_none_or(|current| candidate.0 >= current.0)
        {
            selected.insert(key, candidate);
        }
    }
    if !selected.contains_key("steamloginsecure") {
        return Err("分享包缺少 Steam 商店网页登录 Cookie".to_string());
    }
    let mut values = selected
        .into_values()
        .map(|(_, value)| value)
        .collect::<Vec<_>>();
    values.sort_unstable();
    Ok(values.join("; "))
}

async fn verify_shared_steam_session_online(
    client: &reqwest::Client,
    cookies: &[String],
    expected_steam_id: &str,
) -> Result<(), String> {
    let expected_account_id = expected_steam_id
        .parse::<u64>()
        .ok()
        .and_then(|steam_id| steam_id.checked_sub(STEAM_ID64_INDIVIDUAL_BASE))
        .and_then(|account_id| u32::try_from(account_id).ok())
        .filter(|account_id| *account_id != 0)
        .ok_or_else(|| format!("SteamID {expected_steam_id} 无效"))?;
    let cookie_header = steam_store_cookie_header(cookies)?;
    let response = client
        .get(STEAM_WEB_ACCOUNT_URL)
        .header(
            reqwest::header::USER_AGENT,
            "NEA/1.2 Steam session verifier",
        )
        .header(reqwest::header::COOKIE, cookie_header)
        .send()
        .await
        .map_err(|error| format!("联网校验 Steam 网页登录态失败: {error}"))?
        .error_for_status()
        .map_err(|error| format!("Steam 网页登录态校验请求失败: {error}"))?;
    if response
        .content_length()
        .is_some_and(|length| length > 2 * 1024 * 1024)
    {
        return Err("Steam 网页登录态校验响应异常".to_string());
    }
    let body = response
        .bytes()
        .await
        .map_err(|error| format!("读取 Steam 登录态校验响应失败: {error}"))?;
    if body.len() > 2 * 1024 * 1024 {
        return Err("Steam 网页登录态校验响应异常".to_string());
    }
    let body = String::from_utf8_lossy(&body);
    let account_id = steam_account_id_from_store_html(&body)
        .ok_or_else(|| "Steam 网页登录态已失效或无法确认账号".to_string())?;
    if account_id != expected_account_id {
        return Err(format!(
            "Steam 网页登录态与 SteamID {expected_steam_id} 不匹配"
        ));
    }
    Ok(())
}

fn shared_steam_verifier_client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(12))
        .redirect(reqwest::redirect::Policy::limited(5))
        .build()
        .map_err(|error| format!("创建 Steam 登录态校验请求失败: {error}"))
}

fn is_steam_cookie_domain_allowed(cookie: &Cookie<'_>) -> bool {
    cookie.domain().is_some_and(|domain| {
        let domain = domain.trim_start_matches('.').to_ascii_lowercase();
        domain == "steampowered.com"
            || domain.ends_with(".steampowered.com")
            || domain == "steamcommunity.com"
            || domain.ends_with(".steamcommunity.com")
    })
}

fn is_required_steam_share_cookie(cookie: &Cookie<'_>) -> bool {
    is_steam_cookie_domain_allowed(cookie)
        && matches!(
            cookie.name().to_ascii_lowercase().as_str(),
            "steamloginsecure" | "steamrefresh_steam" | "steamrememberlogin" | "sessionid"
        )
}

fn shared_steam_credential(
    credentials: &[SteamSavedCredential],
    steam_id: &str,
    account_name: Option<&str>,
) -> Option<SharedSteamCredential> {
    let saved = credentials
        .iter()
        .find(|credential| credential.steam_id.as_deref() == Some(steam_id))
        .or_else(|| {
            let account_name = account_name?;
            let normalized = normalized_steam_account_name(account_name);
            credentials.iter().find(|credential| {
                credential.steam_id.is_none()
                    && normalized_steam_account_name(&credential.account_name) == normalized
            })
        })?;
    Some(SharedSteamCredential {
        account_name: saved.account_name.trim().to_string(),
        password: saved.password.clone(),
        steam_id: steam_id.to_string(),
    })
}

fn validate_shared_steam_credential(
    steam_id: &str,
    credential: Option<&SharedSteamCredential>,
) -> Result<(), String> {
    let Some(credential) = credential else {
        return Ok(());
    };
    if credential.steam_id != steam_id {
        return Err("分享包中的 Steam 账密与账号不匹配".to_string());
    }
    let account_name = credential.account_name.trim();
    if credential.account_name != account_name
        || account_name.is_empty()
        || account_name.len() > MAX_SHARED_STEAM_ACCOUNT_NAME_BYTES
        || account_name.chars().any(char::is_whitespace)
        || account_name.chars().any(char::is_control)
        || credential.password.is_empty()
        || credential.password.len() > MAX_SHARED_STEAM_PASSWORD_BYTES
        || credential.password.contains('\0')
    {
        return Err("分享包中的 Steam 账密无效".to_string());
    }
    Ok(())
}

fn shared_steam_credential_conflicts_with_local_identity(
    data: &AppData,
    credential: &SharedSteamCredential,
) -> bool {
    let normalized = normalized_steam_account_name(&credential.account_name);
    let mut established_for_target =
        established_account_names_for_steam_id(data, &credential.steam_id);
    established_for_target.remove("");
    if !established_for_target.is_empty() && !established_for_target.contains(&normalized) {
        return true;
    }
    if data.steam.accounts.iter().any(|account| {
        account.id != credential.steam_id
            && normalized_steam_account_name(&account.account_name) == normalized
    }) || data.steam.web_sessions.iter().any(|session| {
        session.steam_id.as_deref().is_some_and(|steam_id| {
            steam_id != credential.steam_id
                && session.account_name.as_deref().is_some_and(|account_name| {
                    normalized_steam_account_name(account_name) == normalized
                })
        })
    }) {
        return true;
    }
    data.steam_credentials.iter().any(|saved| {
        normalized_steam_account_name(&saved.account_name) == normalized
            && (saved
                .steam_id
                .as_deref()
                .is_some_and(|steam_id| steam_id != credential.steam_id)
                || (saved.steam_id.is_none() && saved.password != credential.password))
    })
}

fn has_shared_steam_credential(data: &AppData, credential: &SharedSteamCredential) -> bool {
    has_saved_credential_for_steam_id(data, &credential.steam_id)
        || data.steam_credentials.iter().any(|saved| {
            normalized_steam_account_name(&saved.account_name)
                == normalized_steam_account_name(&credential.account_name)
        })
}

fn insert_shared_steam_credential_if_missing(
    data: &mut AppData,
    credential: &SharedSteamCredential,
    updated_at: &str,
) -> bool {
    if has_shared_steam_credential(data, credential) {
        return false;
    }
    data.steam_credentials.push(SteamSavedCredential {
        account_name: credential.account_name.trim().to_string(),
        password: credential.password.clone(),
        steam_id: Some(credential.steam_id.clone()),
        updated_at: updated_at.to_string(),
    });
    true
}

fn prepare_quick_share_material(
    app: &AppHandle,
    selection: &QuickShareSelection,
) -> Result<QuickShareMaterial, String> {
    ensure_share_packaging_not_cancelled(Some(&app.state::<AppState>().wormhole_cancelled))?;
    let oopz_ids = selection
        .oopz_account_ids
        .iter()
        .cloned()
        .collect::<HashSet<_>>();
    if oopz_ids.is_empty() && selection.steam_accounts.is_empty() {
        return Err("请至少选择一个可分享账号".to_string());
    }
    if oopz_ids.len() > MAX_EXPORT_ACCOUNTS {
        return Err(format!("一次最多分享 {} 个 OOPZ 账号", MAX_EXPORT_ACCOUNTS));
    }
    if selection.steam_accounts.len() > MAX_SHARED_WEB_SESSIONS {
        return Err(format!(
            "一次最多分享 {} 个 Steam 账号",
            MAX_SHARED_WEB_SESSIONS
        ));
    }
    let mut selected_steam_ids = HashSet::new();
    for account in &selection.steam_accounts {
        if !validate_shared_steam_id(&account.steam_id)
            || (!account.web_login && !account.credential && !account.perfect)
        {
            return Err("所选 Steam 账号或分享能力无效".to_string());
        }
        if !selected_steam_ids.insert(account.steam_id.clone()) {
            return Err("所选 Steam 账号重复".to_string());
        }
    }
    let (oopz_accounts, mut data) = {
        let state = app.state::<AppState>();
        let data = state.data.lock().map_err(|error| error.to_string())?;
        let oopz_accounts = data
            .accounts
            .iter()
            .filter(|account| oopz_ids.contains(&account.id) && account.has_login_state)
            .cloned()
            .collect::<Vec<_>>();
        (oopz_accounts, data.clone())
    };
    if oopz_accounts.len() != oopz_ids.len() {
        return Err("所选 OOPZ 账号包含不可分享或不存在的登录态".to_string());
    }
    reconcile_saved_steam_credentials(&mut data);
    reconcile_steam_identities(&mut data);
    if selection
        .steam_accounts
        .iter()
        .any(|account| account.perfect)
    {
        perfect_arena::stop_for_share_transfer()?;
    }
    let mut shared_sessions = Vec::new();
    let mut shared_credentials = HashMap::<String, SharedSteamCredential>::new();
    for account in &selection.steam_accounts {
        ensure_share_packaging_not_cancelled(Some(&app.state::<AppState>().wormhole_cancelled))?;
        let steam_id = account.steam_id.as_str();
        let session = data
            .steam
            .web_sessions
            .iter()
            .find(|session| session.steam_id.as_deref() == Some(steam_id))
            .cloned();
        let account_name = session
            .as_ref()
            .and_then(|session| session.account_name.as_deref())
            .or_else(|| {
                data.steam_identities
                    .iter()
                    .find(|identity| identity.steam_id.as_deref() == Some(steam_id))
                    .and_then(|identity| identity.account_name.as_deref())
            });
        let credential = shared_steam_credential(&data.steam_credentials, steam_id, account_name);
        if account.web_login && session.is_none() {
            return Err(format!("Steam 账号 {steam_id} 没有可分享的网页登录态"));
        }
        if account.perfect && session.is_none() {
            return Err(format!("完美账号 {steam_id} 缺少 Steam 网页登录"));
        }
        if account.credential {
            let credential = credential
                .as_ref()
                .ok_or_else(|| format!("Steam 账号 {steam_id} 没有已保存账密"))?;
            validate_shared_steam_credential(steam_id, Some(credential))?;
            shared_credentials.insert(steam_id.to_string(), credential.clone());
        }
        if !account.web_login && !account.perfect {
            continue;
        }
        let mut session = session.expect("web session existence was checked");
        if let Some(credential) = account.credential.then_some(credential.as_ref()).flatten() {
            session.account_name = Some(credential.account_name.clone());
        }
        let perfect_files = if account.perfect {
            perfect_arena::account_database_files(steam_id)
                .into_iter()
                .filter_map(|path| {
                    path.file_name()
                        .and_then(|name| name.to_str())
                        .map(str::to_string)
                })
                .filter(|file_name| is_valid_perfect_share_file_name(steam_id, file_name))
                .collect()
        } else {
            Vec::new()
        };
        if account.perfect {
            validate_perfect_share_file_set(steam_id, &perfect_files)?;
        }
        let cookies = collect_web_session_cookies(app, &session)?;
        ensure_share_packaging_not_cancelled(Some(&app.state::<AppState>().wormhole_cancelled))?;
        if cookies.is_empty()
            || cookies.len() > MAX_SHARED_COOKIES_PER_SESSION
            || cookies
                .iter()
                .any(|cookie| cookie.is_empty() || cookie.len() > MAX_SHARED_COOKIE_BYTES)
        {
            return Err(format!(
                "{} 的 Steam 网页 Cookie 超过分享限制",
                session.display_name
            ));
        }
        shared_sessions.push(SharedWebSession {
            kind: if account.perfect {
                "perfect"
            } else {
                "steam-web"
            }
            .to_string(),
            cookies,
            perfect_profile: account
                .perfect
                .then(|| data.perfect_profiles.get(steam_id).cloned())
                .flatten(),
            // “不可用”是本机人工分类，不应随登录态传播到其他设备。
            perfect_unavailable: false,
            perfect_files,
            session,
        });
    }
    let mut shared_credentials = shared_credentials.into_values().collect::<Vec<_>>();
    shared_credentials.sort_unstable_by(|left, right| left.steam_id.cmp(&right.steam_id));
    Ok((oopz_accounts, shared_sessions, shared_credentials))
}

fn write_quick_share_package(
    path: &Path,
    oopz_accounts: &[SavedAccount],
    web_sessions: &[SharedWebSession],
    steam_credentials: &[SharedSteamCredential],
    cancelled: Option<&AtomicBool>,
) -> Result<(), String> {
    let steam_ids = web_sessions
        .iter()
        .filter_map(|item| item.session.steam_id.clone())
        .chain(
            steam_credentials
                .iter()
                .map(|credential| credential.steam_id.clone()),
        )
        .collect::<HashSet<_>>();
    if oopz_accounts.len() > MAX_EXPORT_ACCOUNTS
        || web_sessions.len() > MAX_SHARED_WEB_SESSIONS
        || steam_credentials.len() > MAX_SHARED_WEB_SESSIONS
        || steam_ids.len() > MAX_SHARED_WEB_SESSIONS
    {
        return Err("分享账号数量超过限制".to_string());
    }
    let credential_steam_ids = steam_credentials
        .iter()
        .map(|credential| credential.steam_id.as_str())
        .collect::<HashSet<_>>();
    let credential_account_names = steam_credentials
        .iter()
        .map(|credential| normalized_steam_account_name(&credential.account_name))
        .collect::<HashSet<_>>();
    if credential_steam_ids.len() != steam_credentials.len()
        || credential_account_names.len() != steam_credentials.len()
    {
        return Err("分享包包含重复的 Steam 账密账号".to_string());
    }
    for credential in steam_credentials {
        validate_shared_steam_credential(&credential.steam_id, Some(credential))?;
    }
    let oopz_package = path.with_extension(format!("{}.oopz.tmp", Uuid::new_v4()));
    if !oopz_accounts.is_empty() {
        write_export_package_v3(&oopz_package, oopz_accounts, cancelled)?;
    }
    let result = (|| -> Result<(), String> {
        let manifest = NeaShareManifest {
            format: if steam_credentials.is_empty() {
                NEA_SHARE_FORMAT_V1
            } else {
                NEA_SHARE_FORMAT_V2
            }
            .to_string(),
            exported_at: now(),
            has_oopz_package: !oopz_accounts.is_empty(),
            web_sessions: web_sessions.to_vec(),
            steam_credentials: steam_credentials.to_vec(),
        };
        let manifest_bytes = serde_json::to_vec(&manifest).map_err(|error| error.to_string())?;
        if manifest_bytes.is_empty() || manifest_bytes.len() as u64 > MAX_SHARE_MANIFEST_BYTES {
            return Err("NEA 分享清单超过限制".to_string());
        }
        let mut source_bytes = (manifest_bytes.len() as u64)
            .checked_add(1024 * 1024)
            .ok_or_else(|| "分享包大小溢出".to_string())?;
        if oopz_package.is_file() {
            source_bytes = source_bytes
                .checked_add(
                    fs::metadata(&oopz_package)
                        .map_err(|error| error.to_string())?
                        .len(),
                )
                .ok_or_else(|| "分享包大小溢出".to_string())?;
        }
        let mut perfect_sources = Vec::<(String, String, PathBuf)>::new();
        for item in web_sessions.iter().filter(|item| item.kind == "perfect") {
            ensure_share_packaging_not_cancelled(cancelled)?;
            let steam_id = item
                .session
                .steam_id
                .as_deref()
                .filter(|steam_id| validate_shared_steam_id(steam_id))
                .ok_or_else(|| "完美账号缺少有效 SteamID".to_string())?;
            validate_perfect_share_file_set(steam_id, &item.perfect_files)?;
            if item
                .perfect_files
                .iter()
                .any(|file_name| !is_valid_perfect_share_file_name(steam_id, file_name))
            {
                return Err("完美平台临时 SHM 文件不会写入分享包".to_string());
            }
            let database_dir = perfect_arena::account_database_dir()
                .ok_or_else(|| "无法定位完美世界竞技平台数据目录".to_string())?;
            for file_name in &item.perfect_files {
                let source_path = database_dir.join(file_name);
                if !source_path.is_file() {
                    return Err(format!("完美账号数据库文件已不存在: {file_name}"));
                }
                validate_perfect_share_file(&source_path, file_name)?;
                source_bytes = source_bytes
                    .checked_add(
                        fs::metadata(&source_path)
                            .map_err(|error| error.to_string())?
                            .len(),
                    )
                    .ok_or_else(|| "分享包大小溢出".to_string())?;
                if source_bytes > MAX_NEA_SHARE_ARCHIVE_BYTES {
                    return Err("NEA 分享包预计超过 2048 MB，已停止导出".to_string());
                }
                perfect_sources.push((steam_id.to_string(), file_name.clone(), source_path));
            }
        }
        if source_bytes > MAX_NEA_SHARE_ARCHIVE_BYTES {
            return Err("NEA 分享包预计超过 2048 MB，已停止导出".to_string());
        }
        let file = fs::File::create(path).map_err(|error| error.to_string())?;
        let mut archive = ZipWriter::new(file);
        let options = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
        let mut copy_buffer = vec![0u8; 256 * 1024];
        archive
            .start_file("manifest.json", options)
            .map_err(|error| error.to_string())?;
        archive
            .write_all(&manifest_bytes)
            .map_err(|error| error.to_string())?;
        if !oopz_accounts.is_empty() {
            archive
                .start_file("oopz/accounts.nea", options)
                .map_err(|error| error.to_string())?;
            let mut source = fs::File::open(&oopz_package).map_err(|error| error.to_string())?;
            copy_with_share_cancellation_buffer(
                &mut source,
                &mut archive,
                cancelled,
                &mut copy_buffer,
            )?;
        }
        for (steam_id, file_name, source_path) in perfect_sources {
            ensure_share_packaging_not_cancelled(cancelled)?;
            archive
                .start_file(format!("perfect/{steam_id}/{file_name}"), options)
                .map_err(|error| error.to_string())?;
            let mut source = fs::File::open(source_path).map_err(|error| error.to_string())?;
            copy_with_share_cancellation_buffer(
                &mut source,
                &mut archive,
                cancelled,
                &mut copy_buffer,
            )?;
        }
        archive.finish().map_err(|error| error.to_string())?;
        Ok(())
    })();
    let _ = fs::remove_file(oopz_package);
    result?;
    let package_size = fs::metadata(path).map_err(|error| error.to_string())?.len();
    if package_size == 0 || package_size > MAX_NEA_SHARE_ARCHIVE_BYTES {
        let _ = fs::remove_file(path);
        return Err(format!(
            "NEA 分享包大小为 {} MB，允许范围为 1 字节至 2048 MB",
            package_size / 1024 / 1024
        ));
    }
    Ok(())
}

fn write_quick_share_package_atomic(
    path: &Path,
    oopz_accounts: &[SavedAccount],
    web_sessions: &[SharedWebSession],
    steam_credentials: &[SharedSteamCredential],
    cancelled: Option<&AtomicBool>,
) -> Result<u64, String> {
    if !path
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("nea-share"))
    {
        return Err("跨平台分享包必须使用 .nea-share 扩展名".to_string());
    }
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .ok_or_else(|| "分享包导出路径无效".to_string())?;
    fs::create_dir_all(parent).map_err(|error| format!("创建导出目录失败: {error}"))?;
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| "分享包文件名无效".to_string())?;
    let suffix = Uuid::new_v4();
    let temp = parent.join(format!(".{file_name}.{suffix}.tmp"));
    let backup = parent.join(format!(".{file_name}.{suffix}.bak"));
    if let Err(error) = write_quick_share_package(
        &temp,
        oopz_accounts,
        web_sessions,
        steam_credentials,
        cancelled,
    ) {
        let _ = fs::remove_file(&temp);
        return Err(error);
    }
    let package_bytes = match fs::metadata(&temp) {
        Ok(metadata) => metadata.len(),
        Err(error) => {
            let _ = fs::remove_file(&temp);
            return Err(format!("读取分享包大小失败: {error}"));
        }
    };
    if let Err(error) = ensure_share_packaging_not_cancelled(cancelled) {
        let _ = fs::remove_file(&temp);
        return Err(error);
    }
    if path.exists() {
        if let Err(error) = rename_share_path(path, &backup, "备份原分享包失败") {
            let _ = fs::remove_file(&temp);
            return Err(error);
        }
    }
    if let Err(error) = rename_share_path(&temp, path, "导出分享包失败") {
        if backup.exists() {
            if let Err(restore_error) = rename_share_path(&backup, path, "恢复原分享包失败")
            {
                let temp_cleanup = fs::remove_file(&temp).err();
                return Err(format!(
                    "{error}；{restore_error}。原文件备份保留在 {}{}",
                    backup.display(),
                    temp_cleanup.map_or_else(String::new, |cleanup| format!(
                        "；临时包 {} 清理失败：{cleanup}",
                        temp.display()
                    ))
                ));
            }
        }
        if let Err(cleanup) = fs::remove_file(&temp) {
            return Err(format!(
                "{error}；原文件已恢复，但临时包 {} 清理失败：{cleanup}",
                temp.display()
            ));
        }
        return Err(error);
    }
    if backup.exists() {
        fs::remove_file(&backup).map_err(|error| {
            format!(
                "分享包已导出，但旧文件备份清理失败，请手动删除 {}：{error}",
                backup.display()
            )
        })?;
    }
    Ok(package_bytes)
}

#[tauri::command]
async fn export_quick_share_package_file(
    app: AppHandle,
    selection: QuickShareSelection,
    path: String,
) -> Result<QuickShareExportResult, String> {
    let _activity = acquire_switch_activity(&app)?;
    let _steam_import_guard = acquire_steam_web_import(&app)?;
    if app
        .state::<AppState>()
        .wormhole_running
        .swap(true, Ordering::SeqCst)
    {
        return Err("已有分享或导入正在进行".to_string());
    }
    app.state::<AppState>()
        .wormhole_cancelled
        .store(false, Ordering::SeqCst);
    let result = async {
        let (oopz_accounts, web_sessions, steam_credentials) =
            prepare_quick_share_material(&app, &selection)?;
        let account_count = oopz_accounts.len()
            + web_sessions
                .iter()
                .filter_map(|item| item.session.steam_id.clone())
                .chain(
                    steam_credentials
                        .iter()
                        .map(|credential| credential.steam_id.clone()),
                )
                .collect::<HashSet<_>>()
                .len();
        let target = PathBuf::from(path);
        let build_app = app.clone();
        let package_bytes = tauri::async_runtime::spawn_blocking(move || {
            let state = build_app.state::<AppState>();
            let _account_operation = state
                .account_operation
                .lock()
                .map_err(|error| error.to_string())?;
            write_quick_share_package_atomic(
                &target,
                &oopz_accounts,
                &web_sessions,
                &steam_credentials,
                Some(&state.wormhole_cancelled),
            )
        })
        .await
        .map_err(|error| format!("导出分享包任务异常结束: {error}"))??;
        Ok(QuickShareExportResult {
            accounts: account_count,
            package_bytes,
        })
    }
    .await;
    finish_wormhole_operation(&app);
    result
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
async fn start_quick_export(
    app: AppHandle,
    selection: QuickShareSelection,
) -> Result<String, String> {
    let _activity = acquire_switch_activity(&app)?;
    let _steam_import_guard = acquire_steam_web_import(&app)?;
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
        "正在打包所选账号登录态...",
        None,
        None,
    );
    let package_path = wormhole_temp_package("nea-share", "nea-share");
    let material = prepare_quick_share_material(&app, &selection);
    let (oopz_accounts, web_sessions, steam_credentials) = match material {
        Ok(material) => material,
        Err(error) => {
            finish_wormhole_operation(&app);
            emit_wormhole_status(
                &app,
                if error == QUICK_SHARE_CANCELLED {
                    "cancelled"
                } else {
                    "error"
                },
                "send",
                &error,
                None,
                None,
            );
            return Err(error);
        }
    };
    let build_path = package_path.clone();
    let build_app = app.clone();
    let build_result = match tauri::async_runtime::spawn_blocking(move || {
        let state = build_app.state::<AppState>();
        let _account_operation = state
            .account_operation
            .lock()
            .map_err(|error| error.to_string())?;
        write_quick_share_package(
            &build_path,
            &oopz_accounts,
            &web_sessions,
            &steam_credentials,
            Some(&state.wormhole_cancelled),
        )
    })
    .await
    {
        Ok(result) => result,
        Err(error) => Err(format!("打包任务异常结束: {}", error)),
    };
    if let Err(error) = build_result {
        let _ = fs::remove_file(&package_path);
        finish_wormhole_operation(&app);
        emit_wormhole_status(
            &app,
            if error == QUICK_SHARE_CANCELLED {
                "cancelled"
            } else {
                "error"
            },
            "send",
            &error,
            None,
            None,
        );
        return Err(error);
    }
    let package_bytes = match fs::metadata(&package_path) {
        Ok(metadata) => metadata.len(),
        Err(error) => {
            let _ = fs::remove_file(&package_path);
            finish_wormhole_operation(&app);
            let message = format!("读取分享包大小失败: {}", error);
            emit_wormhole_status(&app, "error", "send", &message, None, None);
            return Err(message);
        }
    };

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
    emit_wormhole_status_with_package_size(
        &app,
        "waiting",
        "send",
        "快捷码已生成，等待对方输入...",
        Some(code.clone()),
        None,
        Some(package_bytes),
    );

    let transfer_app = app.clone();
    let transfer_code = code.clone();
    tauri::async_runtime::spawn(async move {
        let final_code = transfer_code.clone();
        let result = async {
            let connect_app = transfer_app.clone();
            let wormhole =
                tokio::time::timeout(Duration::from_secs(WORMHOLE_TIMEOUT_SECONDS), async {
                    tokio::select! {
                        result = Wormhole::connect(mailbox) => {
                            result.map_err(|e| format!("建立加密连接失败: {}", e))
                        }
                        _ = wait_for_quick_share_cancel(connect_app) => {
                            Err(QUICK_SHARE_CANCELLED.to_string())
                        }
                    }
                })
                .await
                .map_err(|_| "等待对方接收已超时，请重新生成代码".to_string())??;
            let offer = transfer::offer::OfferSend::new_file_or_folder(
                // 传输名保留 .nea，以兼容尚未升级的接收端；手工落盘包使用 .nea-share 区分 OOPZ 包。
                "nea-account-share.nea".to_string(),
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
                    let percent = transferred
                        .saturating_mul(100)
                        .checked_div(total)
                        .unwrap_or(0);
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
        }
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
            Ok(()) => emit_wormhole_status(
                &transfer_app,
                "complete",
                "send",
                "快捷分享完成",
                Some(final_code),
                None,
            ),
            Err(error) => emit_wormhole_status(
                &transfer_app,
                "error",
                "send",
                error,
                Some(final_code),
                None,
            ),
        }
    });
    Ok(code)
}

fn validate_shared_steam_id(value: &str) -> bool {
    is_valid_steam_id64(value)
        && value.parse::<u64>().ok().is_some_and(|steam_id| {
            steam_id > STEAM_ID64_INDIVIDUAL_BASE
                && steam_id <= STEAM_ID64_INDIVIDUAL_BASE + u64::from(u32::MAX)
        })
}

fn is_valid_perfect_share_file_name(steam_id: &str, file_name: &str) -> bool {
    let Some(suffix) = file_name.strip_prefix(&format!("{steam_id}.")) else {
        return false;
    };
    matches!(suffix, "IM3.db" | "IM3.db-wal" | "IPC.db" | "IPC.db-wal")
}

fn is_compatible_perfect_share_file_name(steam_id: &str, file_name: &str) -> bool {
    let Some(suffix) = file_name.strip_prefix(&format!("{steam_id}.")) else {
        return false;
    };
    matches!(
        suffix,
        "IM3.db" | "IM3.db-shm" | "IM3.db-wal" | "IPC.db" | "IPC.db-shm" | "IPC.db-wal"
    )
}

fn perfect_share_file_names(steam_id: &str) -> [String; 6] {
    [
        format!("{steam_id}.IM3.db"),
        format!("{steam_id}.IM3.db-shm"),
        format!("{steam_id}.IM3.db-wal"),
        format!("{steam_id}.IPC.db"),
        format!("{steam_id}.IPC.db-shm"),
        format!("{steam_id}.IPC.db-wal"),
    ]
}

fn validate_perfect_share_file_set(steam_id: &str, file_names: &[String]) -> Result<(), String> {
    let names = file_names
        .iter()
        .map(String::as_str)
        .collect::<HashSet<_>>();
    if names.len() != file_names.len()
        || file_names
            .iter()
            .any(|file_name| !is_compatible_perfect_share_file_name(steam_id, file_name))
    {
        return Err("完美平台数据库文件清单无效".to_string());
    }
    for family in ["IM3", "IPC"] {
        let main = format!("{steam_id}.{family}.db");
        let has_sidecar = [
            format!("{steam_id}.{family}.db-shm"),
            format!("{steam_id}.{family}.db-wal"),
        ]
        .iter()
        .any(|name| names.contains(name.as_str()));
        if has_sidecar && !names.contains(main.as_str()) {
            return Err(format!(
                "完美平台 {family} 数据缺少主数据库，不能只分享 WAL/SHM"
            ));
        }
    }
    Ok(())
}

fn validate_perfect_share_file(path: &Path, file_name: &str) -> Result<(), String> {
    if !file_name.ends_with(".db") {
        return Ok(());
    }
    let connection = rusqlite::Connection::open_with_flags(
        path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|error| format!("完美平台数据库无法只读打开 {file_name}: {error}"))?;
    let integrity = connection
        .query_row("PRAGMA quick_check(1)", [], |row| row.get::<_, String>(0))
        .map_err(|error| format!("完美平台数据库完整性检查失败 {file_name}: {error}"))?;
    if integrity != "ok" {
        return Err(format!(
            "完美平台数据库完整性检查失败 {file_name}: {integrity}"
        ));
    }
    if file_name.contains(".IPC.db") {
        let known_schema = connection
            .prepare("SELECT org, data, update_time FROM IPC_MEMORY_CACHE__prod LIMIT 0")
            .is_ok()
            || connection
                .prepare("SELECT key, data, update_time FROM IPC_CACHE__prod LIMIT 0")
                .is_ok();
        if !known_schema {
            return Err(format!("完美平台 IPC 数据库结构无法识别: {file_name}"));
        }
    }
    Ok(())
}

fn prepare_quick_import_package(
    path: &Path,
    cancelled: Option<&AtomicBool>,
) -> Result<PreparedQuickImport, String> {
    ensure_share_packaging_not_cancelled(cancelled)?;
    let package_size = fs::metadata(path).map_err(|error| error.to_string())?.len();
    if package_size == 0 || package_size > MAX_NEA_SHARE_ARCHIVE_BYTES {
        return Err(format!(
            "NEA 分享包大小为 {} MB，允许范围为 1 字节至 2048 MB",
            package_size / 1024 / 1024
        ));
    }
    let file = fs::File::open(path).map_err(|error| error.to_string())?;
    let mut archive =
        ZipArchive::new(file).map_err(|error| format!("打开 NEA 分享包失败: {error}"))?;
    let manifest = {
        let entry = archive
            .by_name("manifest.json")
            .map_err(|_| "NEA 分享包缺少清单".to_string())?;
        if entry.size() == 0 || entry.size() > MAX_SHARE_MANIFEST_BYTES {
            return Err("NEA 分享清单大小无效".to_string());
        }
        let mut raw = Vec::with_capacity(entry.size() as usize);
        entry
            .take(MAX_SHARE_MANIFEST_BYTES + 1)
            .read_to_end(&mut raw)
            .map_err(|error| format!("读取 NEA 分享清单失败: {error}"))?;
        if raw.len() as u64 > MAX_SHARE_MANIFEST_BYTES {
            return Err("NEA 分享清单超过限制".to_string());
        }
        serde_json::from_slice::<NeaShareManifest>(&raw)
            .map_err(|error| format!("NEA 分享清单格式错误: {error}"))?
    };
    if manifest.format != NEA_SHARE_FORMAT_V1 && manifest.format != NEA_SHARE_FORMAT_V2 {
        return Err("不支持此 NEA 分享包版本".to_string());
    }
    if manifest.format == NEA_SHARE_FORMAT_V1 && !manifest.steam_credentials.is_empty() {
        return Err("旧版 NEA 分享包不应包含 Steam 账密".to_string());
    }
    if !manifest.has_oopz_package
        && manifest.web_sessions.is_empty()
        && manifest.steam_credentials.is_empty()
    {
        return Err("NEA 分享包不包含任何账号".to_string());
    }
    if manifest.web_sessions.len() > MAX_SHARED_WEB_SESSIONS
        || manifest.steam_credentials.len() > MAX_SHARED_WEB_SESSIONS
    {
        return Err("NEA 分享包中的 Steam 账号过多".to_string());
    }
    let mut web_steam_ids = HashSet::new();
    let mut expected_perfect_files = HashSet::new();
    for item in &manifest.web_sessions {
        if item.kind != "steam-web" && item.kind != "steam" && item.kind != "perfect" {
            return Err("NEA 分享包包含未知账号类型".to_string());
        }
        let steam_id = item
            .session
            .steam_id
            .as_deref()
            .filter(|value| validate_shared_steam_id(value))
            .ok_or_else(|| "NEA 分享包包含无效 SteamID".to_string())?;
        if !web_steam_ids.insert(steam_id.to_string()) {
            return Err("NEA 分享包包含重复的 Steam 账号网页登录态".to_string());
        }
        if item.cookies.is_empty() || item.cookies.len() > MAX_SHARED_COOKIES_PER_SESSION {
            return Err(format!("Steam 网页账号 {steam_id} 的 Cookie 数量无效"));
        }
        if item
            .cookies
            .iter()
            .any(|cookie| cookie.is_empty() || cookie.len() > MAX_SHARED_COOKIE_BYTES)
        {
            return Err(format!("Steam 网页账号 {steam_id} 的 Cookie 大小无效"));
        }
        let parsed_cookies = item
            .cookies
            .iter()
            .map(|raw| Cookie::parse(raw.clone()).map(Cookie::into_owned))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| format!("Steam 网页账号 {steam_id} 的 Cookie 格式无效"))?;
        if parsed_cookies
            .iter()
            .any(|cookie| !is_steam_cookie_domain_allowed(cookie))
        {
            return Err(format!("Steam 网页账号 {steam_id} 包含非 Steam 域 Cookie"));
        }
        let login_cookies = parsed_cookies
            .iter()
            .filter(|cookie| cookie.name().eq_ignore_ascii_case("steamLoginSecure"))
            .collect::<Vec<_>>();
        if login_cookies.is_empty()
            || login_cookies
                .iter()
                .any(|cookie| steam_id_from_web_cookie(cookie.value()).as_deref() != Some(steam_id))
        {
            return Err(format!("Steam 网页账号 {steam_id} 的登录态与账号不匹配"));
        }
        if item.kind != "perfect"
            && (!item.perfect_files.is_empty() || item.perfect_profile.is_some())
        {
            return Err("Steam 分享项不应包含完美平台数据".to_string());
        }
        if let Some(profile) = &item.perfect_profile {
            if item.kind != "perfect" || profile.steam_id != steam_id {
                return Err("完美平台画像与 SteamID 不匹配".to_string());
            }
        }
        for file_name in &item.perfect_files {
            let relative = safe_relative_path(file_name)?;
            if item.kind != "perfect" || relative.components().count() != 1 {
                return Err("完美平台数据库文件名无效".to_string());
            }
            if !is_compatible_perfect_share_file_name(steam_id, file_name)
                || !expected_perfect_files.insert((steam_id.to_string(), file_name.clone()))
            {
                return Err("完美平台数据库文件清单无效".to_string());
            }
        }
        if item.kind == "perfect" {
            validate_perfect_share_file_set(steam_id, &item.perfect_files)?;
        }
    }
    let mut credential_steam_ids = HashSet::new();
    let mut credential_account_names = HashSet::new();
    for credential in &manifest.steam_credentials {
        let steam_id = credential.steam_id.as_str();
        if !validate_shared_steam_id(steam_id) {
            return Err("NEA 分享包包含无效的 Steam 账密账号".to_string());
        }
        validate_shared_steam_credential(steam_id, Some(credential))?;
        if !credential_steam_ids.insert(steam_id.to_string())
            || !credential_account_names
                .insert(normalized_steam_account_name(&credential.account_name))
        {
            return Err("NEA 分享包包含重复的 Steam 账密账号".to_string());
        }
    }
    for item in &manifest.web_sessions {
        let Some(steam_id) = item.session.steam_id.as_deref() else {
            continue;
        };
        let Some(account_name) = item.session.account_name.as_deref() else {
            continue;
        };
        if manifest.steam_credentials.iter().any(|credential| {
            credential.steam_id == steam_id
                && normalized_steam_account_name(&credential.account_name)
                    != normalized_steam_account_name(account_name)
        }) {
            return Err("NEA 分享包中的 Steam 网页态与账密账号名不一致".to_string());
        }
    }
    if web_steam_ids.union(&credential_steam_ids).count() > MAX_SHARED_WEB_SESSIONS {
        return Err("NEA 分享包中的 Steam 账号过多".to_string());
    }
    let expected_entry_count = 1usize
        .checked_add(usize::from(manifest.has_oopz_package))
        .and_then(|count| count.checked_add(expected_perfect_files.len()))
        .ok_or_else(|| "NEA 分享包文件数量溢出".to_string())?;
    if archive.len() != expected_entry_count {
        return Err("NEA 分享包文件数量与清单不一致".to_string());
    }

    let root = storage_dir()?
        .join("recovery")
        .join(format!("share-import-{}", Uuid::new_v4()));
    fs::create_dir_all(&root).map_err(|error| error.to_string())?;
    let result = (|| -> Result<PreparedQuickImport, String> {
        let oopz_package = manifest
            .has_oopz_package
            .then(|| root.join("oopz-accounts.nea"));
        let mut found_oopz = false;
        let mut found_perfect = HashSet::new();
        let mut perfect_files = Vec::new();
        let mut total_uncompressed = 0u64;
        let mut manifest_entries = 0usize;
        let mut copy_buffer = vec![0u8; 256 * 1024];
        for index in 0..archive.len() {
            ensure_share_packaging_not_cancelled(cancelled)?;
            let mut entry = archive.by_index(index).map_err(|error| error.to_string())?;
            if entry.is_dir() {
                return Err("NEA 分享包不应包含目录项".to_string());
            }
            total_uncompressed = total_uncompressed
                .checked_add(entry.size())
                .ok_or_else(|| "NEA 分享包内容大小溢出".to_string())?;
            if total_uncompressed > MAX_NEA_SHARE_CONTENT_BYTES {
                return Err("NEA 分享包解压内容超过限制".to_string());
            }
            let enclosed = entry
                .enclosed_name()
                .ok_or_else(|| "NEA 分享包包含不安全路径".to_string())?;
            let name = enclosed
                .to_str()
                .ok_or_else(|| "NEA 分享包包含非 Unicode 路径".to_string())?
                .replace('\\', "/");
            if name == "manifest.json" {
                manifest_entries += 1;
                if manifest_entries > 1 {
                    return Err("NEA 分享包包含重复清单".to_string());
                }
                continue;
            }
            let target = if name == "oopz/accounts.nea" && manifest.has_oopz_package && !found_oopz
            {
                found_oopz = true;
                oopz_package.clone().expect("oopz target must exist")
            } else {
                let parts = name.split('/').collect::<Vec<_>>();
                if parts.len() != 3 || parts[0] != "perfect" {
                    return Err("NEA 分享包包含未声明文件".to_string());
                }
                let key = (parts[1].to_string(), parts[2].to_string());
                if !expected_perfect_files.contains(&key) || !found_perfect.insert(key.clone()) {
                    return Err("NEA 分享包包含未声明或重复的完美平台文件".to_string());
                }
                let target = root.join("perfect").join(&key.0).join(&key.1);
                perfect_files.push((key.0, key.1.clone(), target.clone()));
                target
            };
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent).map_err(|error| error.to_string())?;
            }
            let mut output = fs::File::create(&target).map_err(|error| error.to_string())?;
            let written = copy_with_share_cancellation_buffer(
                &mut entry,
                &mut output,
                cancelled,
                &mut copy_buffer,
            )?;
            if written != entry.size() {
                return Err("NEA 分享包文件大小不一致".to_string());
            }
        }
        if manifest_entries != 1
            || found_oopz != manifest.has_oopz_package
            || found_perfect != expected_perfect_files
        {
            return Err("NEA 分享包缺少已声明的账号文件".to_string());
        }
        for (_, file_name, path) in &perfect_files {
            if file_name.ends_with(".db-shm") {
                fs::remove_file(path)
                    .map_err(|error| format!("忽略完美平台临时 SHM 文件失败: {error}"))?;
            }
        }
        for (_, file_name, path) in &perfect_files {
            validate_perfect_share_file(path, file_name)?;
        }
        Ok(PreparedQuickImport {
            root: root.clone(),
            manifest,
            oopz_package,
            perfect_files,
        })
    })();
    if result.is_err() {
        let _ = fs::remove_dir_all(&root);
    }
    result
}

fn remove_share_path(path: &Path) {
    let _ = remove_share_path_checked(path);
}

fn remove_share_path_checked(path: &Path) -> Result<(), String> {
    if path.is_dir() {
        fs::remove_dir_all(path).map_err(|error| error.to_string())?;
    } else {
        fs::remove_file(path).map_err(|error| error.to_string())?;
    }
    Ok(())
}

fn rename_share_path(source: &Path, target: &Path, action: &str) -> Result<(), String> {
    let mut last_error = None;
    for attempt in 0..20 {
        match fs::rename(source, target) {
            Ok(()) => return Ok(()),
            Err(error) => last_error = Some(error),
        }
        if attempt < 19 {
            thread::sleep(Duration::from_millis(50));
        }
    }
    Err(format!(
        "{action}: {}",
        last_error
            .map(|error| error.to_string())
            .unwrap_or_else(|| "未知文件错误".to_string())
    ))
}

fn apply_staged_share_path(
    staged: &Path,
    target: &Path,
    backup_root: &Path,
    index: usize,
) -> Result<AppliedSharePath, String> {
    let parent = target
        .parent()
        .ok_or_else(|| "分享数据目标目录无效".to_string())?;
    fs::create_dir_all(parent).map_err(|error| format!("创建分享数据目录失败: {error}"))?;
    fs::create_dir_all(backup_root).map_err(|error| format!("创建分享回滚目录失败: {error}"))?;
    let backup = target
        .exists()
        .then(|| backup_root.join(format!("item-{index}")));
    if let Some(backup) = &backup {
        rename_share_path(target, backup, "备份原分享数据失败")?;
    }
    if let Err(error) = rename_share_path(staged, target, "提交分享数据失败") {
        if let Some(backup) = &backup {
            if let Err(restore_error) = rename_share_path(backup, target, "恢复原分享数据失败")
            {
                return Err(format!(
                    "{error}；{restore_error}。原数据保留在 {}",
                    backup.display()
                ));
            }
        }
        return Err(error);
    }
    Ok(AppliedSharePath {
        target: target.to_path_buf(),
        backup,
    })
}

fn backup_share_target_removal(
    target: &Path,
    backup_root: &Path,
    index: usize,
) -> Result<Option<AppliedSharePath>, String> {
    if !target.exists() {
        return Ok(None);
    }
    fs::create_dir_all(backup_root).map_err(|error| format!("创建分享回滚目录失败: {error}"))?;
    let backup = backup_root.join(format!("item-{index}"));
    rename_share_path(target, &backup, "备份待清理的旧分享数据失败")?;
    Ok(Some(AppliedSharePath {
        target: target.to_path_buf(),
        backup: Some(backup),
    }))
}

fn rollback_applied_share_paths(applied: &mut Vec<AppliedSharePath>) -> Result<(), String> {
    let mut errors = Vec::new();
    while let Some(change) = applied.pop() {
        if change.target.exists() {
            if let Err(error) = remove_share_path_checked(&change.target) {
                errors.push(format!("清理 {} 失败: {}", change.target.display(), error));
                continue;
            }
        }
        if let Some(backup) = change.backup {
            if let Err(error) = rename_share_path(&backup, &change.target, "恢复原分享数据失败")
            {
                errors.push(format!("{}: {}", change.target.display(), error));
            }
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(format!("恢复原分享数据失败: {}", errors.join("；")))
    }
}

fn directory_has_entries(path: &Path) -> bool {
    fs::read_dir(path)
        .ok()
        .is_some_and(|mut entries| entries.next().is_some())
}

fn rollback_quick_share_data(
    state: &AppState,
    before_commit: &AppData,
    affected_steam_ids: &HashSet<String>,
    added_credentials: &[QuickShareCredentialRollback],
) -> Result<(), String> {
    commit_app_data_update(state, |current| {
        current.steam.web_sessions.retain(|session| {
            !session
                .steam_id
                .as_ref()
                .is_some_and(|steam_id| affected_steam_ids.contains(steam_id))
        });
        current.steam.web_sessions.extend(
            before_commit
                .steam
                .web_sessions
                .iter()
                .filter(|session| {
                    session
                        .steam_id
                        .as_ref()
                        .is_some_and(|steam_id| affected_steam_ids.contains(steam_id))
                })
                .cloned(),
        );
        for steam_id in affected_steam_ids {
            if let Some(profile) = before_commit.perfect_profiles.get(steam_id) {
                current
                    .perfect_profiles
                    .insert(steam_id.clone(), profile.clone());
            } else {
                current.perfect_profiles.remove(steam_id);
            }
        }
        current.steam_credentials.retain(|credential| {
            !added_credentials.iter().any(|added| {
                credential.steam_id.as_deref() == Some(added.steam_id.as_str())
                    && normalized_steam_account_name(&credential.account_name)
                        == added.normalized_account_name
                    && credential.updated_at == added.updated_at
            })
        });
        reconcile_steam_identities(current);
        Ok(())
    })
}

fn ensure_quick_import_not_cancelled(app: &AppHandle, cancellable: bool) -> Result<(), String> {
    if cancellable
        && app
            .state::<AppState>()
            .wormhole_cancelled
            .load(Ordering::SeqCst)
    {
        Err(QUICK_SHARE_CANCELLED.to_string())
    } else {
        Ok(())
    }
}

async fn import_quick_share_package(
    app: &AppHandle,
    path: &Path,
    cancellable: bool,
) -> Result<QuickImportResult, String> {
    ensure_quick_import_not_cancelled(app, cancellable)?;
    let prepare_path = path.to_path_buf();
    let prepare_app = app.clone();
    let prepared = tauri::async_runtime::spawn_blocking(move || {
        let state = prepare_app.state::<AppState>();
        prepare_quick_import_package(
            &prepare_path,
            cancellable.then_some(&state.wormhole_cancelled),
        )
    })
    .await
    .map_err(|error| format!("解析分享包任务异常结束: {error}"))??;
    if !prepared.manifest.web_sessions.is_empty() {
        if cancellable {
            emit_wormhole_status(
                app,
                "importing",
                "receive",
                "正在联网确认 Steam 网页登录态...",
                None,
                None,
            );
        }
        let verification_inputs = prepared
            .manifest
            .web_sessions
            .iter()
            .map(|item| {
                (
                    item.cookies.clone(),
                    item.session
                        .steam_id
                        .clone()
                        .expect("share manifest steam id was validated"),
                )
            })
            .collect::<Vec<_>>();
        let verifier = match shared_steam_verifier_client() {
            Ok(client) => client,
            Err(error) => {
                let _ = fs::remove_dir_all(&prepared.root);
                return Err(error);
            }
        };
        let verification_result = stream::iter(verification_inputs)
            .map(|(cookies, steam_id)| {
                let verify_app = app.clone();
                let client = verifier.clone();
                async move {
                    ensure_quick_import_not_cancelled(&verify_app, cancellable)?;
                    let verification = async {
                        verify_shared_steam_session_online(&client, &cookies, &steam_id)
                            .await
                            .map_err(|error| format!("Steam 网页账号 {steam_id} 校验失败：{error}"))
                    };
                    if cancellable {
                        tokio::select! {
                            result = verification => result,
                            _ = wait_for_quick_share_cancel(verify_app) => {
                                Err(QUICK_SHARE_CANCELLED.to_string())
                            }
                        }
                    } else {
                        verification.await
                    }
                }
            })
            .buffer_unordered(6)
            .try_collect::<Vec<_>>()
            .await;
        if let Err(error) = verification_result {
            let _ = fs::remove_dir_all(&prepared.root);
            return Err(error);
        }
    }
    let before_data = app
        .state::<AppState>()
        .data
        .lock()
        .map_err(|error| error.to_string())?
        .clone();
    for incoming in &prepared.manifest.steam_credentials {
        if shared_steam_credential_conflicts_with_local_identity(&before_data, incoming) {
            let _ = fs::remove_dir_all(&prepared.root);
            return Err(format!(
                "本机已保存 Steam 账号 {} 的其他登录信息，未导入分享包",
                incoming.account_name
            ));
        }
    }
    let backup_root = prepared.root.join("rollback");
    let affected_steam_ids = prepared
        .manifest
        .web_sessions
        .iter()
        .filter_map(|item| item.session.steam_id.clone())
        .chain(
            prepared
                .manifest
                .steam_credentials
                .iter()
                .map(|credential| credential.steam_id.clone()),
        )
        .collect::<HashSet<_>>();
    let mut staged_sessions = Vec::<StagedQuickWebSession>::new();
    let mut applied_paths = Vec::<AppliedSharePath>::new();
    let mut data_before_share_commit = None::<AppData>;
    let mut rollback_journal = None::<QuickShareRollbackJournal>;
    let mut added_credential_rollbacks = Vec::<QuickShareCredentialRollback>::new();
    let result = async {
        for item in &prepared.manifest.web_sessions {
            ensure_quick_import_not_cancelled(app, cancellable)?;
            let steam_id = item
                .session
                .steam_id
                .as_deref()
                .ok_or_else(|| "分享包缺少 SteamID".to_string())?;
            let existing = before_data
                .steam
                .web_sessions
                .iter()
                .find(|session| session.steam_id.as_deref() == Some(steam_id))
                .cloned();
            let perfect_existed = item.kind == "perfect"
                && (before_data.perfect_profiles.contains_key(steam_id)
                    || !perfect_arena::account_database_files(steam_id).is_empty());
            let new_session = existing.is_none();
            let trusted_native_account_name = before_data
                .steam
                .accounts
                .iter()
                .find(|account| account.id == steam_id)
                .map(|account| account.account_name.clone());
            let trusted_saved_account_name = before_data
                .steam_credentials
                .iter()
                .find(|credential| credential.steam_id.as_deref() == Some(steam_id))
                .map(|credential| credential.account_name.clone());
            let shared_credential = prepared
                .manifest
                .steam_credentials
                .iter()
                .find(|credential| credential.steam_id == steam_id);
            let trusted_shared_account_name =
                shared_credential.map(|credential| credential.account_name.clone());
            let mut session = existing.unwrap_or_else(|| {
                let mut session = item.session.clone();
                session.id = Uuid::new_v4().to_string();
                // 分享包内的网页 session.accountName 不足以证明本机账密归属，
                // 只采用本机已知映射或同包内经过冲突检查的账密账号名。
                session.account_name = trusted_native_account_name
                    .clone()
                    .or_else(|| trusted_saved_account_name.clone())
                    .or_else(|| trusted_shared_account_name.clone());
                session
            });
            session.steam_id = Some(steam_id.to_string());
            session.last_verified_at = Some(Utc::now().to_rfc3339());
            if session.display_name.trim().is_empty() {
                session.display_name = steam_id.to_string();
            }
            let stage_id = format!("share-stage-{}", Uuid::new_v4());
            let stage_dir = steam_web_session_dir(&stage_id)?;
            let target_dir = steam_web_session_dir(&session.id)?;
            let window = build_steam_web_window(app, &stage_id, false, false, None)?;
            let cookie_result = (|| -> Result<(), String> {
                for raw in &item.cookies {
                    let cookie = Cookie::parse(raw.clone())
                        .map_err(|_| format!("Steam 网页账号 {steam_id} 的 Cookie 格式无效"))?
                        .into_owned();
                    if !is_required_steam_share_cookie(&cookie) {
                        continue;
                    }
                    window
                        .set_cookie(cookie)
                        .map_err(|error| format!("恢复 Steam 网页账号 {steam_id} 失败: {error}"))?;
                }
                Ok(())
            })();
            if let Err(error) = cookie_result {
                let _ = window.destroy();
                let _ = fs::remove_dir_all(&stage_dir);
                return Err(error);
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
            let restored_steam_id = steam_id_from_web_window(window.clone()).await;
            let _ = window.destroy();
            if restored_steam_id?.as_deref() != Some(steam_id) {
                let _ = fs::remove_dir_all(&stage_dir);
                return Err(format!("恢复 Steam 网页账号 {steam_id} 后校验失败"));
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
            let _ = cleanup_steam_webview_cache_at(&stage_dir.join("webview2"));
            staged_sessions.push(StagedQuickWebSession {
                item: item.clone(),
                session,
                stage_dir,
                target_dir,
                target_existed: !new_session,
                perfect_existed,
            });
        }

        ensure_quick_import_not_cancelled(app, cancellable)?;
        if cancellable {
            emit_wormhole_status(
                app,
                "committing",
                "receive",
                "校验完成，正在提交账号数据（此阶段不可取消）...",
                None,
                None,
            );
        }
        for staged in &staged_sessions {
            if let Some(window) =
                app.get_webview_window(&steam_web_window_label(&staged.session.id))
            {
                let _ = window.destroy();
            }
        }
        if !staged_sessions.is_empty() {
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
        let rollback_snapshot = app
            .state::<AppState>()
            .data
            .lock()
            .map_err(|error| error.to_string())?
            .clone();
        let mut journal = QuickShareRollbackJournal {
            affected_steam_ids: affected_steam_ids.iter().cloned().collect(),
            web_sessions: rollback_snapshot
                .steam
                .web_sessions
                .iter()
                .filter(|session| {
                    session
                        .steam_id
                        .as_ref()
                        .is_some_and(|steam_id| affected_steam_ids.contains(steam_id))
                })
                .cloned()
                .collect(),
            perfect_profiles: rollback_snapshot
                .perfect_profiles
                .iter()
                .filter(|(steam_id, _)| affected_steam_ids.contains(*steam_id))
                .map(|(steam_id, profile)| (steam_id.clone(), profile.clone()))
                .collect(),
            added_credentials: {
                let updated_at = now();
                prepared
                    .manifest
                    .steam_credentials
                    .iter()
                    .filter(|credential| {
                        !has_shared_steam_credential(&rollback_snapshot, credential)
                    })
                    .map(|credential| QuickShareCredentialRollback {
                        steam_id: credential.steam_id.clone(),
                        normalized_account_name: normalized_steam_account_name(
                            &credential.account_name,
                        ),
                        updated_at: updated_at.clone(),
                    })
                    .collect()
            },
            paths: Vec::new(),
        };
        journal.affected_steam_ids.sort_unstable();
        journal.added_credentials.sort_unstable_by(|left, right| {
            left.steam_id.cmp(&right.steam_id).then_with(|| {
                left.normalized_account_name
                    .cmp(&right.normalized_account_name)
            })
        });
        added_credential_rollbacks.clone_from(&journal.added_credentials);
        write_quick_share_rollback_journal(&prepared.root, &journal)?;
        rollback_journal = Some(journal);
        for staged in &staged_sessions {
            let backup = staged.target_dir.exists().then(|| {
                backup_root.join(format!(
                    "item-{}",
                    rollback_journal
                        .as_ref()
                        .expect("journal exists")
                        .paths
                        .len()
                ))
            });
            let index = record_quick_share_rollback_path(
                &prepared.root,
                rollback_journal.as_mut().expect("journal exists"),
                &staged.target_dir,
                backup,
            )?;
            applied_paths.push(apply_staged_share_path(
                &staged.stage_dir,
                &staged.target_dir,
                &backup_root,
                index,
            )?);
        }

        if !prepared.perfect_files.is_empty() {
            perfect_arena::stop_for_share_transfer()?;
            let database_dir = perfect_arena::account_database_dir()
                .ok_or_else(|| "无法定位完美世界竞技平台数据目录".to_string())?;
            fs::create_dir_all(&database_dir)
                .map_err(|error| format!("创建完美平台数据目录失败: {error}"))?;
            let staged_perfect_files = prepared
                .perfect_files
                .iter()
                .filter(|(_, file_name, path)| !file_name.ends_with(".db-shm") && path.is_file())
                .map(|(steam_id, file_name, path)| {
                    ((steam_id.clone(), file_name.clone()), path.clone())
                })
                .collect::<HashMap<_, _>>();
            for item in prepared
                .manifest
                .web_sessions
                .iter()
                .filter(|item| item.kind == "perfect" && !item.perfect_files.is_empty())
            {
                let steam_id = item
                    .session
                    .steam_id
                    .as_deref()
                    .expect("perfect share SteamID was validated");
                let declared = item
                    .perfect_files
                    .iter()
                    .map(String::as_str)
                    .collect::<HashSet<_>>();
                let all_names = perfect_share_file_names(steam_id);
                for family in [0..3, 3..6] {
                    if !declared.contains(all_names[family.start].as_str()) {
                        continue;
                    }
                    for file_name in &all_names[family] {
                        let target = database_dir.join(file_name);
                        if let Some(staged) =
                            staged_perfect_files.get(&(steam_id.to_string(), file_name.clone()))
                        {
                            let next_index = rollback_journal
                                .as_ref()
                                .expect("journal exists")
                                .paths
                                .len();
                            let backup = target
                                .exists()
                                .then(|| backup_root.join(format!("item-{next_index}")));
                            let index = record_quick_share_rollback_path(
                                &prepared.root,
                                rollback_journal.as_mut().expect("journal exists"),
                                &target,
                                backup,
                            )?;
                            applied_paths.push(apply_staged_share_path(
                                staged,
                                &target,
                                &backup_root,
                                index,
                            )?);
                        } else if target.exists() {
                            let next_index = rollback_journal
                                .as_ref()
                                .expect("journal exists")
                                .paths
                                .len();
                            let backup = backup_root.join(format!("item-{next_index}"));
                            let index = record_quick_share_rollback_path(
                                &prepared.root,
                                rollback_journal.as_mut().expect("journal exists"),
                                &target,
                                Some(backup),
                            )?;
                            let applied =
                                backup_share_target_removal(&target, &backup_root, index)?
                                    .expect("target existence was checked");
                            applied_paths.push(applied);
                        }
                    }
                }
            }
        }

        let session_updates = staged_sessions
            .iter()
            .map(|staged| {
                (
                    staged.item.clone(),
                    staged.session.clone(),
                    staged.target_existed,
                    staged.perfect_existed,
                )
            })
            .collect::<Vec<_>>();
        let shared_credentials = prepared.manifest.steam_credentials.clone();
        let credential_commit_markers = added_credential_rollbacks.clone();
        let state = app.state::<AppState>();
        let (
            steam_web_accounts,
            perfect_accounts,
            steam_web_added,
            steam_web_updated,
            perfect_added,
            perfect_updated,
            steam_credentials_accounts,
            steam_credentials_added,
            before_share_commit,
        ) = commit_app_data_update(&state, move |next_data| {
            if let Some(conflict) = shared_credentials.iter().find(|credential| {
                shared_steam_credential_conflicts_with_local_identity(next_data, credential)
            }) {
                return Err(format!(
                    "本机 Steam 账号 {} 的登录信息刚刚发生变化，请重新导入",
                    conflict.account_name
                ));
            }
            let before_share_commit = next_data.clone();
            let mut steam_web_accounts = 0usize;
            let mut perfect_accounts = 0usize;
            let mut steam_web_added = 0usize;
            let mut steam_web_updated = 0usize;
            let mut perfect_added = 0usize;
            let mut perfect_updated = 0usize;
            let mut steam_credentials_added = 0usize;
            let steam_credentials_accounts = shared_credentials.len();
            for (item, imported, target_existed, perfect_existed) in session_updates {
                if let Some(existing) = next_data
                    .steam
                    .web_sessions
                    .iter_mut()
                    .find(|session| session.steam_id == imported.steam_id)
                {
                    merge_steam_web_session(existing, &imported);
                    existing.last_verified_at = imported.last_verified_at.clone();
                } else {
                    next_data.steam.web_sessions.push(imported);
                }
                let steam_id = item
                    .session
                    .steam_id
                    .as_deref()
                    .expect("validated steam id");
                steam_web_accounts += 1;
                if target_existed {
                    steam_web_updated += 1;
                } else {
                    steam_web_added += 1;
                }
                if item.kind == "perfect" {
                    perfect_accounts += 1;
                    if perfect_existed {
                        perfect_updated += 1;
                    } else {
                        perfect_added += 1;
                    }
                    if let Some(profile) = &item.perfect_profile {
                        if let Some(saved) = next_data.perfect_profiles.get_mut(steam_id) {
                            merge_perfect_profile(saved, profile.clone());
                        } else {
                            next_data
                                .perfect_profiles
                                .insert(steam_id.to_string(), profile.clone());
                        }
                    }
                }
            }
            for credential in &shared_credentials {
                let marker = credential_commit_markers.iter().find(|marker| {
                    marker.steam_id == credential.steam_id
                        && marker.normalized_account_name
                            == normalized_steam_account_name(&credential.account_name)
                });
                if marker.is_some_and(|marker| {
                    insert_shared_steam_credential_if_missing(
                        next_data,
                        credential,
                        &marker.updated_at,
                    )
                }) {
                    steam_credentials_added += 1;
                }
            }
            reconcile_steam_identities(next_data);
            Ok((
                steam_web_accounts,
                perfect_accounts,
                steam_web_added,
                steam_web_updated,
                perfect_added,
                perfect_updated,
                steam_credentials_accounts,
                steam_credentials_added,
                before_share_commit,
            ))
        })?;
        data_before_share_commit = Some(before_share_commit);
        let oopz_accounts = if let Some(path) = &prepared.oopz_package {
            let import_app = app.clone();
            let import_path = path.clone();
            tauri::async_runtime::spawn_blocking(move || {
                import_account_package_inner(&import_app, &import_path)
            })
            .await
            .map_err(|error| format!("导入 OOPZ 账号任务异常结束: {error}"))??
        } else {
            Vec::new()
        };
        let _ = fs::write(prepared.root.join("committed"), now());
        update_tray(app);
        let _ = app.emit("app-data-changed", ());
        Ok(QuickImportResult {
            oopz_accounts,
            steam_web_accounts,
            perfect_accounts,
            steam_web_added,
            steam_web_updated,
            perfect_added,
            perfect_updated,
            steam_credentials_accounts,
            steam_credentials_added,
        })
    }
    .await;
    let mut rollback_error = if result.is_err() {
        let paths_error = rollback_applied_share_paths(&mut applied_paths).err();
        let data_error = data_before_share_commit.as_ref().and_then(|before_commit| {
            rollback_quick_share_data(
                &app.state::<AppState>(),
                before_commit,
                &affected_steam_ids,
                &added_credential_rollbacks,
            )
            .err()
        });
        match (paths_error, data_error) {
            (None, None) => None,
            (Some(paths), None) => Some(paths),
            (None, Some(data)) => Some(format!("恢复原配置失败: {data}")),
            (Some(paths), Some(data)) => Some(format!("{paths}；恢复原配置失败: {data}")),
        }
    } else {
        None
    };
    if result.is_err() && data_before_share_commit.is_some() {
        update_tray(app);
        let _ = app.emit("app-data-changed", ());
    }
    for staged in &staged_sessions {
        if staged.stage_dir.exists() {
            let _ = fs::remove_dir_all(&staged.stage_dir);
        }
    }
    if result.is_ok() {
        let _ = fs::write(prepared.root.join("committed"), now());
    }
    let recovery_needed =
        result.is_err() && (rollback_error.is_some() || directory_has_entries(&backup_root));
    if recovery_needed {
        if let Some(path) = &prepared.oopz_package {
            remove_share_path(path);
        }
        remove_share_path(&prepared.root.join("perfect"));
        let preservation = format!(
            "原数据未被删除，恢复事务保留在 {}，下次启动会自动重试",
            prepared.root.display()
        );
        rollback_error = Some(match rollback_error {
            Some(error) => format!("{error}；{preservation}"),
            None => preservation,
        });
    } else {
        let _ = fs::remove_dir_all(&prepared.root);
    }
    match (result, rollback_error) {
        (Err(error), Some(rollback)) => Err(format!("{error}；{rollback}")),
        (result, _) => result,
    }
}

async fn receive_wormhole_package(
    app: &AppHandle,
    code: Code,
    target: &Path,
) -> Result<bool, String> {
    let request = tokio::time::timeout(Duration::from_secs(WORMHOLE_TIMEOUT_SECONDS), async {
        let mailbox = MailboxConnection::connect(transfer::APP_CONFIG, code, false)
            .await
            .map_err(|e| format!("连接快捷码失败: {}", e))?;
        let wormhole = Wormhole::connect(mailbox)
            .await
            .map_err(|e| format!("建立加密连接失败: {}", e))?;
        transfer::request_file(
            wormhole,
            wormhole_relay_hints()?,
            transit::Abilities::ALL,
            pending(),
        )
        .await
        .map_err(|e| format!("接收请求失败: {}", e))?
        .ok_or_else(|| "对方已取消传输".to_string())
    })
    .await
    .map_err(|_| "等待发送方响应已超时，请确认代码后重试".to_string())??;
    let legacy_oopz_package = request.file_name().ends_with(".oopz+");
    let maximum_size = if legacy_oopz_package {
        MAX_V3_ARCHIVE_BYTES
    } else {
        MAX_NEA_SHARE_ARCHIVE_BYTES
    };
    if !(request.file_name().ends_with(".nea-share")
        || request.file_name().ends_with(".nea")
        || legacy_oopz_package)
        || request.file_size() == 0
        || request.file_size() > maximum_size
    {
        let _ = request.reject().await;
        return Err("对方发送的不是有效 NEA 登录态包".to_string());
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
                let percent = transferred
                    .saturating_mul(100)
                    .checked_div(total)
                    .unwrap_or(0);
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
    Ok(legacy_oopz_package)
}

#[tauri::command]
async fn quick_import(app: AppHandle, code: String) -> Result<QuickImportResult, String> {
    let _activity = acquire_switch_activity(&app)?;
    let _steam_import_guard = acquire_steam_web_import(&app)?;
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
    let package_path = wormhole_temp_package("nea-receive", "nea-share");
    let receive_app = app.clone();
    let receive_result = tokio::select! {
        result = receive_wormhole_package(&app, code, &package_path) => result,
        _ = wait_for_quick_share_cancel(receive_app) => Err(QUICK_SHARE_CANCELLED.to_string()),
    };
    let result = match receive_result {
        Ok(legacy_oopz_package) => {
            emit_wormhole_status(
                &app,
                "importing",
                "receive",
                "接收完成，正在校验并导入...",
                None,
                None,
            );
            if legacy_oopz_package {
                if let Err(error) = ensure_quick_import_not_cancelled(&app, true) {
                    Err(error)
                } else {
                    emit_wormhole_status(
                        &app,
                        "committing",
                        "receive",
                        "校验完成，正在提交 OOPZ 账号（此阶段不可取消）...",
                        None,
                        None,
                    );
                    let import_app = app.clone();
                    let import_path = package_path.clone();
                    match tauri::async_runtime::spawn_blocking(move || {
                        import_account_package_inner(&import_app, &import_path).map(
                            |oopz_accounts| QuickImportResult {
                                oopz_accounts,
                                steam_web_accounts: 0,
                                perfect_accounts: 0,
                                steam_web_added: 0,
                                steam_web_updated: 0,
                                perfect_added: 0,
                                perfect_updated: 0,
                                steam_credentials_accounts: 0,
                                steam_credentials_added: 0,
                            },
                        )
                    })
                    .await
                    {
                        Ok(result) => result,
                        Err(error) => Err(format!("导入旧版 OOPZ 分享包任务异常结束: {error}")),
                    }
                }
            } else {
                import_quick_share_package(&app, &package_path, true).await
            }
        }
        Err(error) => Err(error),
    };
    let _ = fs::remove_file(&package_path);
    finish_wormhole_operation(&app);
    match &result {
        Ok(imported) => emit_wormhole_status(
            &app,
            "complete",
            "receive",
            format!(
                "快捷导入完成：OOPZ {} 个、Steam 网页态 {} 个、Steam 账密 {} 个（新增 {}）、完美平台 {} 个",
                imported.oopz_accounts.len(),
                imported.steam_web_accounts,
                imported.steam_credentials_accounts,
                imported.steam_credentials_added,
                imported.perfect_accounts
            ),
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
async fn import_quick_share_package_file(
    app: AppHandle,
    path: String,
) -> Result<QuickImportResult, String> {
    let source = PathBuf::from(path);
    if !source
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("nea-share"))
    {
        return Err("请选择 .nea-share 跨平台分享包".to_string());
    }
    let _activity = acquire_switch_activity(&app)?;
    let _steam_import_guard = acquire_steam_web_import(&app)?;
    if app
        .state::<AppState>()
        .wormhole_running
        .swap(true, Ordering::SeqCst)
    {
        return Err("已有分享或导入正在进行".to_string());
    }
    app.state::<AppState>()
        .wormhole_cancelled
        .store(false, Ordering::SeqCst);
    emit_wormhole_status(
        &app,
        "importing",
        "receive",
        "正在校验并导入跨平台分享包...",
        None,
        None,
    );
    let result = import_quick_share_package(&app, &source, true).await;
    finish_wormhole_operation(&app);
    match &result {
        Ok(imported) => emit_wormhole_status(
            &app,
            "complete",
            "receive",
            format!(
                "分享包导入完成：OOPZ {} 个、Steam 网页态 {} 个、Steam 账密 {} 个（新增 {}）、完美平台 {} 个",
                imported.oopz_accounts.len(),
                imported.steam_web_accounts,
                imported.steam_credentials_accounts,
                imported.steam_credentials_added,
                imported.perfect_accounts
            ),
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
    let dir = accounts_dir()?.join(&account_id);
    let staged = stage_for_deletion(&dir)?;
    let mut next_data = state.data.lock().map_err(|e| e.to_string())?.clone();
    if !next_data
        .accounts
        .iter()
        .any(|account| account.id == account_id)
    {
        if let Some(staged) = &staged {
            rollback_staged_deletion(staged);
        }
        return Err("账号不存在".to_string());
    }
    let deleting_uid = next_data
        .accounts
        .iter()
        .find(|account| account.id == account_id)
        .and_then(|account| account.uid.as_deref());
    let actual_current_uid = current_registry_login()
        .as_deref()
        .and_then(uid_from_registry_login);
    let deleting_current = deleting_uid.is_some_and(|uid| {
        actual_current_uid.as_deref() == Some(uid)
            || next_data.current_login_uid.as_deref() == Some(uid)
    });
    if deleting_current {
        if let Some(staged) = &staged {
            rollback_staged_deletion(staged);
        }
        return Err("当前登录账号不能删除，请先切换账号或退出 OOPZ".to_string());
    }
    next_data
        .accounts
        .retain(|account| account.id != account_id);
    if let Err(error) = save_data(&next_data) {
        if let Some(staged) = &staged {
            rollback_staged_deletion(staged);
        }
        return Err(error);
    }
    if let Some(staged) = &staged {
        mark_staged_deletion_committed(staged);
    }
    *state.data.lock().map_err(|e| e.to_string())? = next_data;
    delete_credential(&account_id);
    finish_staged_deletion(staged);
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
    let running_pids = |system: &System| {
        system
            .processes()
            .iter()
            .filter_map(|(pid, process)| is_oopz_process_name(process.name()).then_some(*pid))
            .collect::<Vec<_>>()
    };
    let pids = running_pids(&system);

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

    for pid in running_pids(&system) {
        if let Some(process) = system.process(pid) {
            if is_oopz_process_name(process.name()) {
                let _ = process.kill();
            }
        }
    }
    for _ in 0..12 {
        thread::sleep(Duration::from_millis(250));
        refresh_process_system(&mut system);
        if running_pids(&system).is_empty() {
            return Ok(());
        }
    }
    let remaining = running_pids(&system);
    Err(format!(
        "无法安全关闭 OOPZ（仍有 {} 个进程），已取消切号以避免账号数据损坏",
        remaining.len()
    ))
}

fn backup_current(paths: &OopzPaths) -> Result<(), String> {
    let backup = backups_dir()?.join("latest-before-switch");
    let backup_parent = backup.parent().ok_or_else(|| "备份目录无效".to_string())?;
    fs::create_dir_all(backup_parent).map_err(|e| e.to_string())?;
    let staging = backup_parent.join(format!(".nea-backup-{}", Uuid::new_v4()));
    fs::create_dir_all(&staging).map_err(|e| e.to_string())?;
    let Some(login) = current_registry_login() else {
        fs::write(staging.join("logged_out"), b"1").map_err(|e| e.to_string())?;
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
    let _activity = acquire_switch_activity(&app)?;
    let app_for_task = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let state = app_for_task.state::<AppState>();
        restore_latest_backup_inner(&state)
    })
    .await
    .map_err(|e| e.to_string())?
}

fn restore_latest_backup_inner(state: &AppState) -> Result<SwitchResult, String> {
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
    if backup.join("logged_out").exists() {
        clear_registry_login()?;
    } else if login_backup.exists() {
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
    let _activity = acquire_switch_activity(&app)?;
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
            message: "已打开 OOPZ 登录页。登录完成后回到 NEA 点刷新。".to_string(),
        });
    }

    let Some(oopz_login) = read_oopz_login(&account.id) else {
        commit_app_data_update(&state, |data| {
            if let Some(saved) = data
                .accounts
                .iter_mut()
                .find(|saved| saved.id == account.id)
            {
                saved.has_login_state = false;
                saved.updated_at = now();
            }
            Ok(())
        })?;
        update_tray(&app);
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
            ok: false,
            message: "这个账号还不能快速切换。请在 OOPZ 里登录一次，然后回到 NEA 点刷新。"
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
        return Err("账号数据不完整，请打开 OOPZ 登录一次，然后回到 NEA 点刷新".to_string());
    }

    detach_plugin_overlay(&app);
    close_oopz_if_running()?;
    backup_current(&paths)?;
    let apply_result = (|| -> Result<(), String> {
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
        Command::new(&paths.oopz_exe_path)
            .spawn()
            .map_err(|e| e.to_string())?;
        Ok(())
    })();
    if let Err(error) = apply_result {
        return match restore_latest_backup_inner(&state) {
            Ok(_) => Err(format!("切号失败，已恢复切换前账号：{error}")),
            Err(rollback_error) => Err(format!(
                "切号失败：{error}；自动恢复也失败：{rollback_error}"
            )),
        };
    }
    schedule_avatar_refresh(app.clone(), uid.clone());
    ensure_plugin_runtime_after_oopz_start(config);

    let mut data = state.data.lock().map_err(|e| e.to_string())?;
    if let Some(pos) = data.accounts.iter().position(|a| a.id == account.id) {
        data.accounts[pos].last_used_at = Some(now());
        data.accounts[pos].updated_at = now();
    }
    save_data(&data)?;
    drop(data);
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
        .invoke_handler(tauri::generate_handler![
            get_app_data,
            get_config_health,
            get_steam_workspace,
            discover_steam,
            refresh_steam_accounts,
            optimize_storage,
            create_steam_web_session,
            preview_steam_web_import,
            import_steam_web_accounts_from_text,
            cancel_steam_web_import,
            open_steam_web_session,
            refresh_steam_web_sessions,
            set_steam_web_session_note,
            set_steam_identity_note,
            delete_steam_web_session,
            delete_steam_saved_credential,
            get_steam_capability_status,
            set_steam_capability_paused,
            cancel_steam_capability_completion,
            complete_steam_capabilities,
            switch_perfect_web_account,
            get_perfect_arena_workspace,
            get_perfect_arena_profiles,
            set_perfect_account_unavailable,
            discover_perfect_arena,
            switch_steam_and_perfect_account,
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
            export_quick_share_package_file,
            import_quick_share_package_file,
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
            if !plugin_runtime {
                recover_import_transactions();
                recover_quick_share_transactions();
                cleanup_stale_share_artifacts();
            }
            app.manage(initial_app_state());
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
            if plugin_enabled {
                if !watcher_registration_is_current() {
                    let _ = install_watcher();
                }
                if !is_watcher_running() {
                    let _ = spawn_watcher();
                }
            }
            let mut tray_builder =
                TrayIconBuilder::with_id("main-tray").tooltip("NEA · 左键打开，右键切换账号");
            if let Some(icon) = app.default_window_icon() {
                tray_builder = tray_builder.icon(icon.clone());
            }
            let tray = tray_builder
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
                        "quit" => app.exit(0),
                        "perfect-available-only" => toggle_perfect_available_filter(app),
                        _ if id.starts_with("oopz-switch:") => {
                            let account_id = id.trim_start_matches("oopz-switch:").to_string();
                            let app_handle = app.clone();
                            thread::spawn(move || {
                                let state = app_handle.state::<AppState>();
                                let result =
                                    switch_account_inner(app_handle.clone(), state, account_id);
                                finish_tray_switch(
                                    &app_handle,
                                    result.map_err(|error| error.to_string()),
                                );
                            });
                        }
                        _ if id.starts_with("steam-switch:") => {
                            let account_id = id.trim_start_matches("steam-switch:").to_string();
                            let app_handle = app.clone();
                            thread::spawn(move || {
                                let result = tauri::async_runtime::block_on(switch_steam_account(
                                    app_handle.clone(),
                                    account_id,
                                ));
                                finish_tray_switch(
                                    &app_handle,
                                    result.map_err(|error| error.to_string()),
                                );
                            });
                        }
                        _ if id.starts_with("perfect-only:") => {
                            let session_id = id.trim_start_matches("perfect-only:").to_string();
                            let app_handle = app.clone();
                            thread::spawn(move || {
                                let result = tauri::async_runtime::block_on(
                                    switch_perfect_web_account(app_handle.clone(), session_id),
                                );
                                finish_tray_switch(
                                    &app_handle,
                                    result.map_err(|error| error.to_string()),
                                );
                            });
                        }
                        _ if id.starts_with("perfect-sync:") => {
                            let session_id = id.trim_start_matches("perfect-sync:").to_string();
                            let app_handle = app.clone();
                            thread::spawn(move || {
                                let result = tauri::async_runtime::block_on(
                                    switch_steam_and_perfect_account(
                                        app_handle.clone(),
                                        session_id,
                                    ),
                                );
                                finish_tray_switch(
                                    &app_handle,
                                    result.map_err(|error| error.to_string()),
                                );
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
            schedule_storage_maintenance(app.handle().clone());
            let display_name_app = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                let _ = repair_stored_steam_display_names(display_name_app).await;
            });

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
        .is_some_and(is_watcher_executable_name)
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
    fn current_runtime_names_use_nea_and_keep_legacy_detection() {
        assert_eq!(env!("CARGO_PKG_NAME"), "nea");
        assert!(is_nea_runtime_process_name("nea.exe"));
        assert!(is_nea_runtime_process_name("NEA-WATCHER.EXE"));
        assert!(is_nea_runtime_process_name("oopz-plus.exe"));
        assert!(is_nea_runtime_process_name("OOPZ-PLUS-WATCHER.EXE"));
        assert!(is_watcher_executable_name("nea-watcher.exe"));
        assert!(is_watcher_executable_name("oopz-plus-watcher.exe"));
        assert!(!is_nea_runtime_process_name("oopz.exe"));
    }

    #[test]
    fn dev_smoke_concurrent_steam_session_commits_preserve_every_update() {
        let data = Arc::new(Mutex::new(AppData::default()));
        let handles = (0..16)
            .map(|index| {
                let data = data.clone();
                thread::spawn(move || {
                    commit_data_update_with(
                        &data,
                        |next| {
                            next.steam.web_sessions.push(steam::SteamWebSession {
                                id: format!("session-{index}"),
                                steam_id: None,
                                account_name: Some(format!("account-{index}")),
                                display_name: format!("Account {index}"),
                                note: None,
                                created_at: now(),
                                last_verified_at: None,
                            });
                            Ok(())
                        },
                        |_| Ok(()),
                    )
                })
            })
            .collect::<Vec<_>>();
        for handle in handles {
            handle.join().unwrap().unwrap();
        }
        let data = data.lock().unwrap();
        assert_eq!(data.steam.web_sessions.len(), 16);
        for index in 0..16 {
            assert!(data
                .steam
                .web_sessions
                .iter()
                .any(|session| session.id == format!("session-{index}")));
        }
    }

    #[test]
    fn storage_layout_archives_legacy_roots_without_losing_workspace_data() {
        let root = std::env::temp_dir().join(format!("nea-layout-test-{}", Uuid::new_v4()));
        fs::create_dir_all(root.join("accounts").join("account-a")).unwrap();
        fs::create_dir_all(root.join("backups").join("backup-a")).unwrap();
        fs::write(
            root.join("accounts").join("account-a").join("state.bin"),
            b"account",
        )
        .unwrap();
        fs::write(
            root.join("backups").join("backup-a").join("state.bin"),
            b"backup",
        )
        .unwrap();
        fs::write(root.join("update-completed.txt"), b"1.2.5").unwrap();

        organize_storage_layout(&root).unwrap();

        assert!(root
            .join("workspaces/oopz/accounts/account-a/state.bin")
            .is_file());
        assert!(root
            .join("workspaces/oopz/backups/backup-a/state.bin")
            .is_file());
        assert!(root
            .join("legacy/oopz-root/accounts/account-a/state.bin")
            .is_file());
        assert!(root
            .join("legacy/oopz-root/backups/backup-a/state.bin")
            .is_file());
        assert!(!root.join("accounts").exists());
        assert!(!root.join("backups").exists());
        assert!(root.join("runtime/update-completed.txt").is_file());
        assert!(root.join("recovery").is_dir());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn webview_cache_cleanup_preserves_login_state_files() {
        let root = std::env::temp_dir().join(format!("nea-webview-test-{}", Uuid::new_v4()));
        let cache = root.join(r"EBWebView\Default\Cache");
        let code_cache = root.join(r"EBWebView\Default\Code Cache");
        let network = root.join(r"EBWebView\Default\Network");
        let local_storage = root.join(r"EBWebView\Default\Local Storage\leveldb");
        for directory in [&cache, &code_cache, &network, &local_storage] {
            fs::create_dir_all(directory).unwrap();
        }
        fs::write(cache.join("cache.bin"), vec![1u8; 1024]).unwrap();
        fs::write(code_cache.join("code.bin"), vec![2u8; 2048]).unwrap();
        fs::write(network.join("Cookies"), b"cookie-state").unwrap();
        fs::write(local_storage.join("000001.log"), b"local-state").unwrap();

        let freed = cleanup_steam_webview_cache_at(&root);

        assert!(freed >= 3072);
        assert!(!cache.exists());
        assert!(!code_cache.exists());
        assert_eq!(fs::read(network.join("Cookies")).unwrap(), b"cookie-state");
        assert_eq!(
            fs::read(local_storage.join("000001.log")).unwrap(),
            b"local-state"
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn perfect_avatars_are_stored_outside_config_and_hydrated_on_read() {
        let root = std::env::temp_dir().join(format!("nea-avatar-test-{}", Uuid::new_v4()));
        let steam_id = "76561198000000099";
        let data_url = "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAusB9Y9Zl8sAAAAASUVORK5CYII=";
        store_perfect_avatar_data_url_at(&root, steam_id, data_url).unwrap();
        let mut profile = perfect_arena::PerfectArenaProfile {
            steam_id: steam_id.to_string(),
            found: true,
            nickname: Some("Avatar Test".to_string()),
            avatar_url: Some(PERFECT_AVATAR_CACHE_MARKER.to_string()),
            avatar_source_url: None,
            score: None,
            season: None,
            player_identity: None,
            high_risk: None,
            reputation_requires_verification: None,
            reputation_points: None,
            reputation_level: None,
            updated_at: None,
        };

        hydrate_perfect_profile_avatar_at(&root, &mut profile);

        assert!(perfect_avatar_path_at(&root, steam_id).unwrap().is_file());
        assert!(profile
            .avatar_url
            .as_deref()
            .is_some_and(|value| value.starts_with("data:image/png;base64,")));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn steam_import_preflight_detects_existing_and_duplicate_accounts_before_login() {
        let mut data = AppData::default();
        data.steam.web_sessions.push(steam::SteamWebSession {
            id: "verified-session".to_string(),
            steam_id: Some("76561198000000001".to_string()),
            account_name: Some("ExistingUser".to_string()),
            display_name: "Existing".to_string(),
            note: None,
            created_at: now(),
            last_verified_at: Some(now()),
        });
        let preview = steam_import_preview(
            &data,
            &[
                "existinguser".to_string(),
                "NewUser".to_string(),
                "newuser".to_string(),
            ],
        );
        assert_eq!(preview.existing_accounts, vec!["existinguser"]);
        assert_eq!(preview.duplicate_input_accounts, vec!["newuser"]);
    }

    #[test]
    fn steam_import_preflight_deduplicates_by_steam64_but_does_not_skip_client_only_accounts() {
        let steam_id = "76561198000000021";
        let mut data = AppData::default();
        data.steam.accounts.push(steam::SteamAccount {
            id: steam_id.to_string(),
            account_name: "client-name".to_string(),
            display_name: "Existing Client".to_string(),
            remember_password: true,
            most_recent: false,
            userdata_captured: false,
            last_used_at: None,
            note: None,
        });
        for account_name in ["first-alias", "second-alias"] {
            data.steam_credentials.push(SteamSavedCredential {
                account_name: account_name.to_string(),
                password: "local-password".to_string(),
                steam_id: Some(steam_id.to_string()),
                updated_at: now(),
            });
        }

        let preview = steam_import_preview(
            &data,
            &["first-alias".to_string(), "second-alias".to_string()],
        );

        assert!(preview.existing_accounts.is_empty());
        assert_eq!(preview.duplicate_input_accounts, vec!["second-alias"]);
    }

    #[test]
    fn steam64_without_a_login_state_is_not_a_complete_duplicate() {
        let steam_id = "76561198000000022";
        let mut data = AppData::default();
        data.steam_credentials.push(SteamSavedCredential {
            account_name: "credential-only".to_string(),
            password: "local-password".to_string(),
            steam_id: Some(steam_id.to_string()),
            updated_at: now(),
        });

        assert_eq!(
            steam64_for_account_name(&data, "credential-only").as_deref(),
            Some(steam_id)
        );
        assert!(steam_import_duplicate_id(&data, "credential-only").is_none());

        data.steam.web_sessions.push(steam::SteamWebSession {
            id: "current-import".to_string(),
            steam_id: Some(steam_id.to_string()),
            account_name: Some("credential-only".to_string()),
            display_name: "Current Import".to_string(),
            note: None,
            created_at: now(),
            last_verified_at: Some(now()),
        });
        assert!(!has_verified_steam_web_login(
            &data,
            steam_id,
            Some("current-import")
        ));

        data.steam.accounts.push(steam::SteamAccount {
            id: steam_id.to_string(),
            account_name: "credential-only".to_string(),
            display_name: "Existing Client".to_string(),
            remember_password: false,
            most_recent: false,
            userdata_captured: false,
            last_used_at: None,
            note: None,
        });
        assert!(!has_verified_steam_web_login(
            &data,
            steam_id,
            Some("current-import")
        ));
    }

    #[test]
    fn dev_smoke_duplicate_login_state_still_saves_missing_credentials_without_overwrite() {
        let steam_id = "76561198000000023";
        let mut data = AppData::default();
        data.steam.accounts.push(steam::SteamAccount {
            id: steam_id.to_string(),
            account_name: "existing-client".to_string(),
            display_name: "Existing Client".to_string(),
            remember_password: true,
            most_recent: false,
            userdata_captured: false,
            last_used_at: None,
            note: None,
        });
        let imported = SteamCredentialInput {
            account: "existing-client".to_string(),
            password: "first-password".to_string(),
        };

        insert_missing_steam_credentials(&mut data, &[imported], &HashSet::new());
        assert_eq!(data.steam_credentials.len(), 1);
        assert_eq!(
            data.steam_credentials[0].steam_id.as_deref(),
            Some(steam_id)
        );

        let repeated = SteamCredentialInput {
            account: "existing-client".to_string(),
            password: "do-not-overwrite".to_string(),
        };
        insert_missing_steam_credentials(
            &mut data,
            &[repeated],
            &HashSet::from([steam_id.to_string()]),
        );
        assert_eq!(data.steam_credentials.len(), 1);
        assert_eq!(data.steam_credentials[0].password, "first-password");

        let same_id = SharedSteamCredential {
            account_name: "renamed-client".to_string(),
            password: "shared-must-not-overwrite".to_string(),
            steam_id: steam_id.to_string(),
        };
        assert!(shared_steam_credential_conflicts_with_local_identity(
            &data, &same_id
        ));
        assert!(!insert_shared_steam_credential_if_missing(
            &mut data,
            &same_id,
            "2026-07-16T00:00:00Z",
        ));
        let same_name = SharedSteamCredential {
            account_name: "existing-client".to_string(),
            password: "shared-name-must-not-overwrite".to_string(),
            steam_id: "76561198000000024".to_string(),
        };
        assert!(shared_steam_credential_conflicts_with_local_identity(
            &data, &same_name
        ));
        assert!(!insert_shared_steam_credential_if_missing(
            &mut data,
            &same_name,
            "2026-07-16T00:00:00Z",
        ));
        assert_eq!(data.steam_credentials.len(), 1);
        assert_eq!(data.steam_credentials[0].password, "first-password");
    }

    #[test]
    fn existing_unlinked_credentials_are_bound_instead_of_replaced() {
        let steam_id = "76561198000000024";
        let mut data = AppData::default();
        data.steam.accounts.push(steam::SteamAccount {
            id: steam_id.to_string(),
            account_name: "known-name".to_string(),
            display_name: "Known Client".to_string(),
            remember_password: true,
            most_recent: false,
            userdata_captured: false,
            last_used_at: None,
            note: None,
        });
        data.steam_credentials.push(SteamSavedCredential {
            account_name: "known-name".to_string(),
            password: "existing-password".to_string(),
            steam_id: None,
            updated_at: now(),
        });
        let imported = SteamCredentialInput {
            account: "different-alias".to_string(),
            password: "new-password".to_string(),
        };

        insert_missing_steam_credentials(&mut data, &[imported], &HashSet::new());
        bind_credentials_to_steam_id(&mut data, steam_id, Some("different-alias"));
        deduplicate_credentials_for_steam_id(&mut data, steam_id);
        assert_eq!(data.steam_credentials.len(), 1);
        assert_eq!(data.steam_credentials[0].password, "existing-password");
        assert_eq!(
            data.steam_credentials[0].steam_id.as_deref(),
            Some(steam_id)
        );
    }

    #[test]
    fn post_login_duplicate_keeps_the_established_credential() {
        let steam_id = "76561198000000025";
        let mut data = AppData::default();
        data.steam.accounts.push(steam::SteamAccount {
            id: steam_id.to_string(),
            account_name: "established-name".to_string(),
            display_name: "Established".to_string(),
            remember_password: true,
            most_recent: false,
            userdata_captured: false,
            last_used_at: None,
            note: None,
        });
        data.steam_credentials.extend([
            SteamSavedCredential {
                account_name: "established-name".to_string(),
                password: "keep-this".to_string(),
                steam_id: Some(steam_id.to_string()),
                updated_at: now(),
            },
            SteamSavedCredential {
                account_name: "late-resolved-alias".to_string(),
                password: "discard-this".to_string(),
                steam_id: Some(steam_id.to_string()),
                updated_at: now(),
            },
        ]);

        deduplicate_credentials_for_steam_id(&mut data, steam_id);

        assert_eq!(data.steam_credentials.len(), 1);
        assert_eq!(data.steam_credentials[0].account_name, "established-name");
        assert_eq!(data.steam_credentials[0].password, "keep-this");
    }

    #[test]
    fn saved_steam_credentials_link_account_name_and_steam_id() {
        let mut data = AppData::default();
        data.steam.web_sessions.push(steam::SteamWebSession {
            id: "verified-session".to_string(),
            steam_id: Some("76561198000000001".to_string()),
            account_name: Some("LinkedUser".to_string()),
            display_name: "Linked".to_string(),
            note: None,
            created_at: now(),
            last_verified_at: Some(now()),
        });
        insert_missing_steam_credentials(
            &mut data,
            &[SteamCredentialInput {
                account: "linkeduser".to_string(),
                password: "saved-locally".to_string(),
            }],
            &HashSet::new(),
        );
        let credential =
            credential_for_steam_identity(&data, Some("76561198000000001"), None).unwrap();
        assert_eq!(credential.account_name, "linkeduser");
        assert_eq!(credential.password, "saved-locally");
    }

    #[test]
    fn saved_credentials_are_a_standalone_client_capability() {
        let mut data = AppData::default();
        data.steam_credentials.push(SteamSavedCredential {
            account_name: "MergeUser".to_string(),
            password: "local-password".to_string(),
            steam_id: None,
            updated_at: now(),
        });
        reconcile_steam_identities(&mut data);
        assert_eq!(data.steam_identities.len(), 1);
        assert_eq!(data.steam_identities[0].id, "pending:mergeuser");
        assert!(data.steam_identities[0].capabilities.credential);

        data.steam.web_sessions.push(steam::SteamWebSession {
            id: "web-session".to_string(),
            steam_id: Some("76561198000000009".to_string()),
            account_name: Some("mergeuser".to_string()),
            display_name: "Merged Player".to_string(),
            note: None,
            created_at: now(),
            last_verified_at: Some(now()),
        });
        reconcile_steam_identities(&mut data);
        assert_eq!(data.steam_identities.len(), 1);
        let identity = &data.steam_identities[0];
        assert_eq!(identity.id, "76561198000000009");
        assert_eq!(identity.steam_id.as_deref(), Some("76561198000000009"));
        assert!(identity.capabilities.web_login);
        assert!(identity.capabilities.credential);
    }

    #[test]
    fn saved_credentials_are_never_pruned_with_local_login_metadata() {
        let mut data = AppData::default();
        data.steam_credentials.extend([
            SteamSavedCredential {
                account_name: "orphan".to_string(),
                password: "keep-orphan".to_string(),
                steam_id: Some("76561198000000041".to_string()),
                updated_at: now(),
            },
            SteamSavedCredential {
                account_name: "kept-client".to_string(),
                password: "keep".to_string(),
                steam_id: None,
                updated_at: now(),
            },
        ]);
        data.steam.accounts.push(steam::SteamAccount {
            id: "76561198000000042".to_string(),
            account_name: "kept-client".to_string(),
            display_name: "Kept Client".to_string(),
            remember_password: false,
            most_recent: false,
            userdata_captured: false,
            last_used_at: None,
            note: None,
        });

        reconcile_saved_steam_credentials(&mut data);
        assert_eq!(data.steam_credentials.len(), 2);
        let linked = data
            .steam_credentials
            .iter()
            .find(|credential| credential.account_name == "kept-client")
            .unwrap();
        assert_eq!(linked.steam_id.as_deref(), Some("76561198000000042"));
    }

    #[test]
    fn steam_identity_collects_all_capabilities_under_steam_id() {
        let steam_id = "76561198000000010";
        let mut data = AppData::default();
        data.steam.accounts.push(steam::SteamAccount {
            id: steam_id.to_string(),
            account_name: "allcaps".to_string(),
            display_name: "Client Name".to_string(),
            remember_password: true,
            most_recent: false,
            userdata_captured: false,
            last_used_at: None,
            note: Some("不要显示此备注".to_string()),
        });
        data.steam.web_sessions.push(steam::SteamWebSession {
            id: "allcaps-web".to_string(),
            steam_id: Some(steam_id.to_string()),
            account_name: Some("allcaps".to_string()),
            display_name: "Web Name".to_string(),
            note: None,
            created_at: now(),
            last_verified_at: Some(now()),
        });
        data.steam_credentials.push(SteamSavedCredential {
            account_name: "allcaps".to_string(),
            password: "local-password".to_string(),
            steam_id: Some(steam_id.to_string()),
            updated_at: now(),
        });
        data.perfect_profiles.insert(
            steam_id.to_string(),
            perfect_arena::PerfectArenaProfile {
                steam_id: steam_id.to_string(),
                found: true,
                nickname: Some("Perfect Name".to_string()),
                avatar_url: None,
                avatar_source_url: None,
                score: Some(1200),
                season: None,
                player_identity: Some("老兵".to_string()),
                high_risk: Some(false),
                reputation_requires_verification: Some(false),
                reputation_points: Some(100),
                reputation_level: Some("优秀".to_string()),
                updated_at: Some(now()),
            },
        );
        reconcile_steam_identities(&mut data);
        assert_eq!(data.steam_identities.len(), 1);
        let capabilities = &data.steam_identities[0].capabilities;
        assert!(capabilities.web_login);
        assert!(capabilities.credential);
        assert!(capabilities.perfect_profile);

        let runtime = TrayMenuRuntime {
            current_perfect_id: Some(steam_id.to_string()),
            steam_ready: true,
            perfect_ready: true,
            ..TrayMenuRuntime::default()
        };
        let steam_accounts = steam_tray_accounts(&data, &runtime);
        assert_eq!(steam_accounts.len(), 1);
        assert_eq!(steam_accounts[0].label, "Client Name");
        assert!(!steam_accounts[0].label.contains("allcaps"));
        assert!(!steam_accounts[0].label.contains(steam_id));
        assert!(!steam_accounts[0].label.contains("不要显示此备注"));
        let unavailable_runtime = TrayMenuRuntime::default();
        assert!(!steam_tray_accounts(&data, &unavailable_runtime)[0].enabled);

        assert_eq!(sanitize_tray_label("  Client\n\0Name\t", 48), "Client Name");
        let long_label = sanitize_tray_label(&"名".repeat(80), 48);
        assert_eq!(long_label.chars().count(), 48);
        assert!(long_label.ends_with('…'));
        assert!(tray_switch_failed(&Err("失败".to_string())));
        assert!(tray_switch_failed(&Ok(SwitchResult {
            ok: false,
            message: "需要用户处理".to_string(),
        })));
        assert!(!tray_switch_failed(&Ok(SwitchResult {
            ok: true,
            message: "完成".to_string(),
        })));

        let perfect_accounts = perfect_tray_accounts(&data, &runtime);
        assert_eq!(perfect_accounts.len(), 1);
        assert_eq!(perfect_accounts[0].label, "✓ Perfect Name C+1200 优秀");
        assert_eq!(perfect_accounts[0].rank, PerfectTrayRank::CPlus);
        assert!(!perfect_accounts[0].blocked);
        assert_eq!(perfect_accounts[0].actions.len(), 2);
        assert_eq!(
            perfect_accounts[0].actions[0].id,
            "perfect-only:allcaps-web"
        );
        assert_eq!(perfect_accounts[0].actions[0].label, "仅切换完美");
        assert!(!perfect_accounts[0].actions[0].enabled);
        assert_eq!(
            perfect_accounts[0].actions[1].id,
            "perfect-sync:allcaps-web"
        );
        assert!(perfect_accounts[0].actions[1].enabled);

        let rank_cases = [
            (None, PerfectTrayRank::Pending),
            (Some(1000), PerfectTrayRank::D),
            (Some(1001), PerfectTrayRank::C),
            (Some(1150), PerfectTrayRank::C),
            (Some(1151), PerfectTrayRank::CPlus),
            (Some(1300), PerfectTrayRank::CPlus),
            (Some(1301), PerfectTrayRank::GoldCPlus),
            (Some(1450), PerfectTrayRank::GoldCPlus),
            (Some(1451), PerfectTrayRank::B),
            (Some(1600), PerfectTrayRank::B),
            (Some(1601), PerfectTrayRank::BPlus),
            (Some(1750), PerfectTrayRank::BPlus),
            (Some(1751), PerfectTrayRank::GoldBPlus),
            (Some(1900), PerfectTrayRank::GoldBPlus),
            (Some(1901), PerfectTrayRank::A),
            (Some(2050), PerfectTrayRank::A),
            (Some(2051), PerfectTrayRank::APlus),
            (Some(2200), PerfectTrayRank::APlus),
            (Some(2201), PerfectTrayRank::GoldAPlus),
        ];
        for (score, expected) in rank_cases {
            assert_eq!(PerfectTrayRank::from_score(score), expected);
        }

        let filtered_runtime = TrayMenuRuntime {
            perfect_available_only: true,
            ..runtime.clone()
        };
        let mut blocked_data = data.clone();
        blocked_data
            .perfect_unavailable_account_ids
            .insert(steam_id.to_string());
        let visible_blocked_accounts = perfect_tray_accounts(&blocked_data, &runtime);
        assert_eq!(visible_blocked_accounts.len(), 1);
        assert!(visible_blocked_accounts[0].blocked);
        assert!(visible_blocked_accounts[0].label.ends_with("不可用"));
        assert!(perfect_tray_accounts(&blocked_data, &filtered_runtime).is_empty());
        assert_eq!(
            perfect_tray_summary(&blocked_data, steam_id).0,
            "Perfect Name C+1200 不可用"
        );

        blocked_data.perfect_unavailable_account_ids.clear();
        blocked_data
            .perfect_profiles
            .get_mut(steam_id)
            .expect("test profile")
            .high_risk = Some(true);
        assert!(perfect_tray_accounts(&blocked_data, &filtered_runtime).is_empty());
        assert_eq!(
            perfect_tray_summary(&blocked_data, steam_id).0,
            "Perfect Name C+1200 高危"
        );

        {
            let pending_profile = blocked_data
                .perfect_profiles
                .get_mut(steam_id)
                .expect("test profile");
            pending_profile.high_risk = Some(false);
            pending_profile.score = None;
        }
        let pending_accounts = perfect_tray_accounts(&blocked_data, &filtered_runtime);
        assert_eq!(pending_accounts.len(), 1);
        assert_eq!(pending_accounts[0].rank, PerfectTrayRank::Pending);
        blocked_data
            .perfect_profiles
            .get_mut(steam_id)
            .expect("test profile")
            .reputation_requires_verification = Some(true);
        assert!(perfect_tray_accounts(&blocked_data, &filtered_runtime).is_empty());

        let mut fallback_identity = data.steam_identities[0].clone();
        fallback_identity.display_name = "allcaps".to_string();
        let (fallback, known) = steam_community_tray_name(&data, &fallback_identity);
        assert!(!known);
        assert!(fallback.starts_with("社区 ID 待获取"));
        assert!(!fallback.contains("allcaps"));
        assert!(!fallback.contains(steam_id));

        let mut fallback_data = data.clone();
        fallback_data.steam_identities[0].display_name = "allcaps".to_string();
        let fallback_accounts = steam_tray_accounts(&fallback_data, &runtime);
        assert!(fallback_accounts[0].enabled);
        assert!(fallback_accounts[0].label.starts_with("社区 ID 待获取"));

        fallback_data.steam_credentials.push(SteamSavedCredential {
            account_name: "historical-login".to_string(),
            password: "historical-password".to_string(),
            steam_id: Some(steam_id.to_string()),
            updated_at: now(),
        });
        fallback_identity.display_name = "historical-login".to_string();
        assert!(!steam_community_tray_name(&fallback_data, &fallback_identity).1);
        fallback_identity.display_name = "76561198000000999".to_string();
        assert!(!steam_community_tray_name(&fallback_data, &fallback_identity).1);

        let second_steam_id = "76561198000000011";
        let mut duplicate_name_data = data.clone();
        duplicate_name_data
            .steam
            .accounts
            .push(steam::SteamAccount {
                id: second_steam_id.to_string(),
                account_name: "another-login".to_string(),
                display_name: "Client Name".to_string(),
                remember_password: true,
                most_recent: false,
                userdata_captured: false,
                last_used_at: None,
                note: None,
            });
        duplicate_name_data
            .steam_credentials
            .push(SteamSavedCredential {
                account_name: "another-login".to_string(),
                password: "another-password".to_string(),
                steam_id: Some(second_steam_id.to_string()),
                updated_at: now(),
            });
        duplicate_name_data
            .steam
            .web_sessions
            .push(steam::SteamWebSession {
                id: "another-web".to_string(),
                steam_id: Some(second_steam_id.to_string()),
                account_name: Some("another-login".to_string()),
                display_name: "Another Web Name".to_string(),
                note: None,
                created_at: now(),
                last_verified_at: Some(now()),
            });
        let mut second_profile = duplicate_name_data.perfect_profiles[steam_id].clone();
        second_profile.steam_id = second_steam_id.to_string();
        duplicate_name_data
            .perfect_profiles
            .insert(second_steam_id.to_string(), second_profile);
        reconcile_steam_identities(&mut duplicate_name_data);
        let duplicate_steam_accounts = steam_tray_accounts(&duplicate_name_data, &runtime);
        assert_eq!(duplicate_steam_accounts.len(), 2);
        assert!(duplicate_steam_accounts
            .iter()
            .any(|account| account.label == "Client Name · 0010"));
        assert!(duplicate_steam_accounts
            .iter()
            .any(|account| account.label == "Client Name · 0011"));
        let duplicate_perfect_accounts = perfect_tray_accounts(&duplicate_name_data, &runtime);
        assert_eq!(duplicate_perfect_accounts.len(), 2);
        assert!(duplicate_perfect_accounts
            .iter()
            .any(|account| account.label == "✓ Perfect Name C+1200 优秀 · 0010"));
        assert!(duplicate_perfect_accounts
            .iter()
            .any(|account| account.label == "Perfect Name C+1200 优秀 · 0011"));

        let mut duplicate_data = data.clone();
        let mut duplicate_session = duplicate_data.steam.web_sessions[0].clone();
        duplicate_session.id = "duplicate-web".to_string();
        duplicate_data.steam.web_sessions.push(duplicate_session);
        reconcile_steam_identities(&mut duplicate_data);
        assert_eq!(perfect_tray_accounts(&duplicate_data, &runtime).len(), 1);

        let busy_runtime = TrayMenuRuntime {
            busy: true,
            steam_ready: true,
            perfect_ready: true,
            ..TrayMenuRuntime::default()
        };
        assert!(perfect_tray_accounts(&data, &busy_runtime)[0]
            .actions
            .iter()
            .all(|action| !action.enabled));
    }

    #[test]
    fn perfect_profile_alone_does_not_leave_an_empty_steam_account() {
        let steam_id = "76561198000000011";
        let mut data = AppData::default();
        data.perfect_profiles.insert(
            steam_id.to_string(),
            perfect_arena::PerfectArenaProfile {
                steam_id: steam_id.to_string(),
                found: true,
                nickname: Some("Only Perfect".to_string()),
                avatar_url: None,
                avatar_source_url: None,
                score: None,
                season: None,
                player_identity: None,
                high_risk: None,
                reputation_requires_verification: None,
                reputation_points: None,
                reputation_level: None,
                updated_at: Some(now()),
            },
        );
        reconcile_steam_identities(&mut data);
        assert!(data.steam_identities.is_empty());
    }

    fn test_shared_web_session(steam_id: &str) -> SharedWebSession {
        SharedWebSession {
            kind: "steam-web".to_string(),
            session: steam::SteamWebSession {
                id: "source-session".to_string(),
                steam_id: Some(steam_id.to_string()),
                account_name: Some("tester".to_string()),
                display_name: "Tester".to_string(),
                note: None,
                created_at: now(),
                last_verified_at: Some(now()),
            },
            cookies: vec![format!(
                "steamLoginSecure={steam_id}%7Ctoken; Domain=store.steampowered.com; Path=/; Secure; HttpOnly"
            )],
            perfect_profile: None,
            perfect_unavailable: false,
            perfect_files: Vec::new(),
        }
    }

    fn test_shared_steam_credential(steam_id: &str, password: &str) -> SharedSteamCredential {
        SharedSteamCredential {
            account_name: "tester".to_string(),
            password: password.to_string(),
            steam_id: steam_id.to_string(),
        }
    }

    fn write_test_share_package(
        path: &Path,
        manifest: &NeaShareManifest,
        extra_file: Option<&str>,
    ) {
        let file = fs::File::create(path).unwrap();
        let mut archive = ZipWriter::new(file);
        let options = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
        archive.start_file("manifest.json", options).unwrap();
        serde_json::to_writer(&mut archive, manifest).unwrap();
        if let Some(name) = extra_file {
            archive.start_file(name, options).unwrap();
            archive.write_all(b"unexpected").unwrap();
        }
        archive.finish().unwrap();
    }

    fn write_test_share_package_files(
        path: &Path,
        manifest: &NeaShareManifest,
        files: &[(&str, &[u8])],
    ) {
        let file = fs::File::create(path).unwrap();
        let mut archive = ZipWriter::new(file);
        let options = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
        archive.start_file("manifest.json", options).unwrap();
        serde_json::to_writer(&mut archive, manifest).unwrap();
        for (name, contents) in files {
            archive.start_file(*name, options).unwrap();
            archive.write_all(contents).unwrap();
        }
        archive.finish().unwrap();
    }

    #[test]
    fn quick_share_manifest_accepts_matching_steam_cookie() {
        let path = std::env::temp_dir().join(format!("nea-share-test-{}.nea", Uuid::new_v4()));
        let manifest = NeaShareManifest {
            format: NEA_SHARE_FORMAT_V1.to_string(),
            exported_at: now(),
            has_oopz_package: false,
            web_sessions: vec![test_shared_web_session("76561198000000001")],
            steam_credentials: Vec::new(),
        };
        write_test_share_package(&path, &manifest, None);
        let prepared = prepare_quick_import_package(&path, None).unwrap();
        assert_eq!(prepared.manifest.format, NEA_SHARE_FORMAT_V1);
        assert_eq!(prepared.manifest.web_sessions.len(), 1);
        assert_eq!(prepared.manifest.web_sessions[0].kind, "steam-web");
        assert!(prepared.manifest.steam_credentials.is_empty());
        let _ = fs::remove_dir_all(prepared.root);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn unified_steam_share_selection_supports_independent_capabilities() {
        let selection = QuickShareSelection {
            oopz_account_ids: Vec::new(),
            steam_accounts: vec![
                QuickSteamShareSelection {
                    steam_id: "76561198000000001".to_string(),
                    web_login: true,
                    credential: false,
                    perfect: false,
                },
                QuickSteamShareSelection {
                    steam_id: "76561198000000002".to_string(),
                    web_login: false,
                    credential: true,
                    perfect: false,
                },
                QuickSteamShareSelection {
                    steam_id: "76561198000000003".to_string(),
                    web_login: false,
                    credential: true,
                    perfect: true,
                },
            ],
        };
        assert!(selection.steam_accounts[0].web_login);
        assert!(selection.steam_accounts[1].credential);
        assert!(selection.steam_accounts[2].perfect);
    }

    #[test]
    fn quick_share_manifest_rejects_cookie_for_another_account() {
        let path = std::env::temp_dir().join(format!("nea-share-test-{}.nea", Uuid::new_v4()));
        let mut item = test_shared_web_session("76561198000000001");
        item.cookies = test_shared_web_session("76561198000000002").cookies;
        let manifest = NeaShareManifest {
            format: NEA_SHARE_FORMAT_V1.to_string(),
            exported_at: now(),
            has_oopz_package: false,
            web_sessions: vec![item],
            steam_credentials: Vec::new(),
        };
        write_test_share_package(&path, &manifest, None);
        assert!(prepare_quick_import_package(&path, None).is_err());
        let _ = fs::remove_file(path);

        let credential_path =
            std::env::temp_dir().join(format!("nea-share-test-{}.nea", Uuid::new_v4()));
        let invalid_credential = SharedSteamCredential {
            account_name: "invalid account".to_string(),
            password: "secret".to_string(),
            steam_id: "76561198000000001".to_string(),
        };
        let manifest = NeaShareManifest {
            format: NEA_SHARE_FORMAT_V2.to_string(),
            exported_at: now(),
            has_oopz_package: false,
            web_sessions: Vec::new(),
            steam_credentials: vec![invalid_credential],
        };
        write_test_share_package(&credential_path, &manifest, None);
        assert!(prepare_quick_import_package(&credential_path, None).is_err());
        let _ = fs::remove_file(credential_path);
    }

    #[test]
    fn quick_share_package_rejects_undeclared_files() {
        let path = std::env::temp_dir().join(format!("nea-share-test-{}.nea", Uuid::new_v4()));
        let manifest = NeaShareManifest {
            format: NEA_SHARE_FORMAT_V1.to_string(),
            exported_at: now(),
            has_oopz_package: false,
            web_sessions: Vec::new(),
            steam_credentials: Vec::new(),
        };
        write_test_share_package(&path, &manifest, Some("unexpected.bin"));
        assert!(prepare_quick_import_package(&path, None).is_err());
        let _ = fs::remove_file(path);
    }

    #[test]
    fn quick_share_package_rejects_empty_manifest() {
        let path = std::env::temp_dir().join(format!("nea-share-test-{}.nea", Uuid::new_v4()));
        let manifest = NeaShareManifest {
            format: NEA_SHARE_FORMAT_V1.to_string(),
            exported_at: now(),
            has_oopz_package: false,
            web_sessions: Vec::new(),
            steam_credentials: Vec::new(),
        };
        write_test_share_package(&path, &manifest, None);
        assert!(prepare_quick_import_package(&path, None).is_err());
        let _ = fs::remove_file(path);
    }

    #[test]
    fn quick_share_parser_ignores_legacy_machine_auth_cookie() {
        let path = std::env::temp_dir().join(format!("nea-share-test-{}.nea", Uuid::new_v4()));
        let mut item = test_shared_web_session("76561198000000001");
        item.cookies.push(
            "steamMachineAuth76561198000000001=secret; Domain=store.steampowered.com; Path=/"
                .to_string(),
        );
        let manifest = NeaShareManifest {
            format: NEA_SHARE_FORMAT_V1.to_string(),
            exported_at: now(),
            has_oopz_package: false,
            web_sessions: vec![item],
            steam_credentials: Vec::new(),
        };
        write_test_share_package(&path, &manifest, None);
        let prepared = prepare_quick_import_package(&path, None).unwrap();
        let _ = fs::remove_dir_all(prepared.root);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn quick_share_parser_rejects_malformed_login_cookie() {
        let path = std::env::temp_dir().join(format!("nea-share-test-{}.nea", Uuid::new_v4()));
        let mut item = test_shared_web_session("76561198000000001");
        item.cookies
            .push("steamLoginSecure=invalid; Domain=store.steampowered.com; Path=/".to_string());
        let manifest = NeaShareManifest {
            format: NEA_SHARE_FORMAT_V1.to_string(),
            exported_at: now(),
            has_oopz_package: false,
            web_sessions: vec![item],
            steam_credentials: Vec::new(),
        };
        write_test_share_package(&path, &manifest, None);
        assert!(prepare_quick_import_package(&path, None).is_err());
        let _ = fs::remove_file(path);
    }

    #[test]
    fn quick_share_perfect_database_requires_main_and_valid_sqlite() {
        let steam_id = "76561198000000001";
        let main_name = format!("{steam_id}.IM3.db");
        let mut item = test_shared_web_session(steam_id);
        item.kind = "perfect".to_string();
        item.perfect_files = vec![main_name.clone()];
        let manifest = NeaShareManifest {
            format: NEA_SHARE_FORMAT_V1.to_string(),
            exported_at: now(),
            has_oopz_package: false,
            web_sessions: vec![item.clone()],
            steam_credentials: Vec::new(),
        };
        let valid = std::env::temp_dir().join(format!("nea-share-test-{}.nea", Uuid::new_v4()));
        let entry_name = format!("perfect/{steam_id}/{main_name}");
        let valid_database =
            std::env::temp_dir().join(format!("nea-perfect-test-{}.db", Uuid::new_v4()));
        {
            let connection = rusqlite::Connection::open(&valid_database).unwrap();
            connection
                .execute_batch("PRAGMA user_version = 1;")
                .unwrap();
        }
        let valid_database_bytes = fs::read(&valid_database).unwrap();
        let _ = fs::remove_file(valid_database);
        write_test_share_package_files(
            &valid,
            &manifest,
            &[(entry_name.as_str(), valid_database_bytes.as_slice())],
        );
        let prepared = prepare_quick_import_package(&valid, None).unwrap();
        let _ = fs::remove_dir_all(prepared.root);
        let _ = fs::remove_file(valid);

        let shm_name = format!("{steam_id}.IM3.db-shm");
        let mut legacy_item = item.clone();
        legacy_item.perfect_files = vec![main_name.clone(), shm_name.clone()];
        let legacy_manifest = NeaShareManifest {
            format: NEA_SHARE_FORMAT_V1.to_string(),
            exported_at: now(),
            has_oopz_package: false,
            web_sessions: vec![legacy_item],
            steam_credentials: Vec::new(),
        };
        let legacy = std::env::temp_dir().join(format!("nea-share-test-{}.nea", Uuid::new_v4()));
        let shm_entry = format!("perfect/{steam_id}/{shm_name}");
        write_test_share_package_files(
            &legacy,
            &legacy_manifest,
            &[
                (entry_name.as_str(), valid_database_bytes.as_slice()),
                (shm_entry.as_str(), b"legacy transient shm"),
            ],
        );
        let prepared = prepare_quick_import_package(&legacy, None).unwrap();
        assert!(prepared
            .perfect_files
            .iter()
            .filter(|(_, name, _)| name.ends_with(".db-shm"))
            .all(|(_, _, path)| !path.exists()));
        let _ = fs::remove_dir_all(prepared.root);
        let _ = fs::remove_file(legacy);

        let invalid = std::env::temp_dir().join(format!("nea-share-test-{}.nea", Uuid::new_v4()));
        write_test_share_package_files(
            &invalid,
            &manifest,
            &[(entry_name.as_str(), b"SQLite format 3\0valid")],
        );
        assert!(prepare_quick_import_package(&invalid, None).is_err());
        let _ = fs::remove_file(invalid);

        let unknown_ipc =
            std::env::temp_dir().join(format!("nea-perfect-test-{}.db", Uuid::new_v4()));
        {
            let connection = rusqlite::Connection::open(&unknown_ipc).unwrap();
            connection
                .execute_batch("CREATE TABLE unrelated(value TEXT);")
                .unwrap();
        }
        let unknown_ipc_bytes = fs::read(&unknown_ipc).unwrap();
        let _ = fs::remove_file(unknown_ipc);
        let ipc_name = format!("{steam_id}.IPC.db");
        let mut ipc_item = test_shared_web_session(steam_id);
        ipc_item.kind = "perfect".to_string();
        ipc_item.perfect_files = vec![ipc_name.clone()];
        let ipc_manifest = NeaShareManifest {
            format: NEA_SHARE_FORMAT_V1.to_string(),
            exported_at: now(),
            has_oopz_package: false,
            web_sessions: vec![ipc_item],
            steam_credentials: Vec::new(),
        };
        let ipc = std::env::temp_dir().join(format!("nea-share-test-{}.nea", Uuid::new_v4()));
        let ipc_entry = format!("perfect/{steam_id}/{ipc_name}");
        write_test_share_package_files(
            &ipc,
            &ipc_manifest,
            &[(ipc_entry.as_str(), unknown_ipc_bytes.as_slice())],
        );
        assert!(prepare_quick_import_package(&ipc, None).is_err());
        let _ = fs::remove_file(ipc);

        item.perfect_files = vec![format!("{steam_id}.IM3.db-wal")];
        let sidecar_only = NeaShareManifest {
            format: NEA_SHARE_FORMAT_V1.to_string(),
            exported_at: now(),
            has_oopz_package: false,
            web_sessions: vec![item],
            steam_credentials: Vec::new(),
        };
        let sidecar = std::env::temp_dir().join(format!("nea-share-test-{}.nea", Uuid::new_v4()));
        let sidecar_entry = format!("perfect/{steam_id}/{steam_id}.IM3.db-wal");
        write_test_share_package_files(
            &sidecar,
            &sidecar_only,
            &[(sidecar_entry.as_str(), b"wal")],
        );
        assert!(prepare_quick_import_package(&sidecar, None).is_err());
        let _ = fs::remove_file(sidecar);
    }

    #[test]
    fn dev_smoke_manual_share_writer_roundtrips_and_replaces_existing_file() {
        let root = std::env::temp_dir().join(format!("nea-share-writer-{}", Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let path = root.join("accounts.nea-share");
        fs::write(&path, b"old").unwrap();
        let steam_id = "76561198000000001";
        let bytes = write_quick_share_package_atomic(
            &path,
            &[],
            &[test_shared_web_session(steam_id)],
            &[],
            None,
        )
        .unwrap();
        assert!(bytes > 0);
        let prepared = prepare_quick_import_package(&path, None).unwrap();
        assert_eq!(prepared.manifest.web_sessions.len(), 1);
        assert!(prepared.manifest.steam_credentials.is_empty());
        let _ = fs::remove_dir_all(prepared.root);

        write_quick_share_package_atomic(
            &path,
            &[],
            &[],
            &[test_shared_steam_credential(steam_id, "credential-only")],
            None,
        )
        .unwrap();
        let prepared = prepare_quick_import_package(&path, None).unwrap();
        assert_eq!(prepared.manifest.format, NEA_SHARE_FORMAT_V2);
        assert!(prepared.manifest.web_sessions.is_empty());
        assert_eq!(prepared.manifest.steam_credentials.len(), 1);
        let _ = fs::remove_dir_all(prepared.root);

        write_quick_share_package_atomic(
            &path,
            &[],
            &[test_shared_web_session(steam_id)],
            &[test_shared_steam_credential(steam_id, "both")],
            None,
        )
        .unwrap();
        let prepared = prepare_quick_import_package(&path, None).unwrap();
        assert_eq!(prepared.manifest.format, NEA_SHARE_FORMAT_V2);
        assert_eq!(prepared.manifest.web_sessions.len(), 1);
        assert_eq!(prepared.manifest.steam_credentials.len(), 1);
        let _ = fs::remove_dir_all(prepared.root);
        assert!(write_quick_share_package_atomic(
            &root.join("accounts.nea"),
            &[],
            &[test_shared_web_session(steam_id)],
            &[],
            None,
        )
        .is_err());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn recovers_corrupt_config_from_valid_backup() {
        let root = std::env::temp_dir().join(format!("nea-config-recovery-{}", Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let path = root.join("config.json");
        fs::write(&path, "{broken").unwrap();
        let expected = AppData {
            schema_version: 7,
            ..AppData::default()
        };
        fs::write(
            path.with_extension("json.bak"),
            serde_json::to_string_pretty(&expected).unwrap(),
        )
        .unwrap();

        let (recovered, _) = recover_config_file(&path).unwrap();
        assert_eq!(recovered.schema_version, 7);
        assert!(parse_app_data_file(&path).is_some());
        assert!(!CONFIG_WRITES_BLOCKED.load(Ordering::SeqCst));
        assert!(fs::read_dir(&root).unwrap().flatten().any(|entry| entry
            .file_name()
            .to_string_lossy()
            .starts_with("config.json.corrupt-")));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn staged_deletion_can_be_rolled_back() {
        let root = std::env::temp_dir().join(format!("nea-delete-rollback-{}", Uuid::new_v4()));
        let original = root.join("account");
        let staged = root.join("trash");
        let marker = root.join("trash.json");
        fs::create_dir_all(&original).unwrap();
        fs::write(original.join("state.bin"), b"state").unwrap();
        fs::rename(&original, &staged).unwrap();
        fs::write(&marker, b"marker").unwrap();
        rollback_staged_deletion(&StagedDeletion {
            original: original.clone(),
            staged,
            marker,
        });
        assert_eq!(fs::read(original.join("state.bin")).unwrap(), b"state");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn startup_recovers_interrupted_deletion_transaction() {
        let root = std::env::temp_dir().join(format!("nea-delete-recovery-{}", Uuid::new_v4()));
        let trash = root.join("trash");
        let original = root.join("accounts").join("account-1");
        let staged = trash.join("operation.data");
        let marker = trash.join("operation.json");
        fs::create_dir_all(&staged).unwrap();
        fs::write(staged.join("state.bin"), b"state").unwrap();
        fs::write(
            &marker,
            serde_json::to_vec(&StagedDeletionMarker {
                original: original.clone(),
                staged: staged.clone(),
                committed: false,
            })
            .unwrap(),
        )
        .unwrap();
        recover_staged_deletions(&root);
        assert_eq!(fs::read(original.join("state.bin")).unwrap(), b"state");
        assert!(!marker.exists());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn steam_credential_script_does_not_embed_plaintext() {
        let credentials = SteamCredentialInput {
            account: "account-name".to_string(),
            password: "private-password".to_string(),
        };
        let script = steam_credential_automation_script(&credentials).unwrap();
        assert!(!script.contains("account-name"));
        assert!(!script.contains("private-password"));
        assert!(script.contains("atob("));
        assert!(script.contains("location.pathname.toLowerCase().startsWith('/login')"));
        assert!(script.contains("password.closest('form')"));
        assert!(script.contains("loginRoot.querySelectorAll('button, input[type=\"submit\"]')"));
        assert!(script.contains(STEAM_VERIFICATION_WINDOW_TITLE));
        assert!(script.contains(STEAM_INVALID_CREDENTIALS_WINDOW_TITLE));
        assert!(script.contains(STEAM_TOKEN_PROTECTED_WINDOW_TITLE));
        assert!(script.contains(STEAM_VERIFICATION_URL_MARKER));
        assert!(script.contains(STEAM_INVALID_CREDENTIALS_URL_MARKER));
        assert!(script.contains(STEAM_TOKEN_PROTECTED_URL_MARKER));
        assert!(script.contains("history.replaceState"));
        assert!(script.contains("refreshExpiredQrCode"));
        assert!(script.contains("new MutationObserver(scheduleQrRetryCheck)"));
        assert!(script
            .contains("const qrRetryPhrases = ['重试', '刷新', 'retry', 'refresh', 'reload']"));
        assert!(script.contains("candidate.click()"));
        assert!(script.contains("请核对您的密码和帐户名称并重试"));
        assert!(script.contains("此账户受到手机验证器保护"));
        assert!(script.contains("使用 steam 手机应用来确认登录"));
        assert!(script.contains("输入您 steam 手机应用上的代码"));
        assert!(script.contains("autocomplete === 'one-time-code'"));
        assert!(!script.contains("document.querySelector('input[type=\"text\"]"));

        let mut password = credentials.password;
        clear_sensitive_string(&mut password);
        assert!(password.is_empty());
    }

    #[test]
    fn steam_import_error_markers_do_not_depend_on_the_native_window_title() {
        assert_eq!(
            steam_import_outcome_from_markers(None, Some(STEAM_INVALID_CREDENTIALS_URL_MARKER)),
            Some(SteamImportAccountOutcome::InvalidCredentials)
        );
        assert_eq!(
            steam_import_outcome_from_markers(
                Some("NEA - Steam 网页账号"),
                Some(STEAM_TOKEN_PROTECTED_URL_MARKER),
            ),
            Some(SteamImportAccountOutcome::HasToken)
        );
        assert_eq!(
            steam_import_outcome_from_markers(
                Some("NEA - Steam 网页账号"),
                Some(STEAM_VERIFICATION_URL_MARKER),
            ),
            Some(SteamImportAccountOutcome::VerificationRequired)
        );
    }

    #[test]
    fn perfect_oauth_automation_keeps_normal_flow_and_has_a_loop_stop_page() {
        assert!(PERFECT_OAUTH_AUTOMATION_SCRIPT.contains("const accepted ="));
        assert!(!PERFECT_OAUTH_AUTOMATION_SCRIPT.contains("window.name"));
        assert!(PERFECT_OAUTH_LOOP_STOP_SCRIPT.contains("已停止重复授权"));
    }

    #[cfg(feature = "custom-protocol")]
    #[test]
    fn production_context_embeds_frontend_entrypoint() {
        let context: tauri::Context<tauri::Wry> = tauri::generate_context!();
        let entrypoint = tauri::utils::assets::AssetKey::from("/index.html");
        let html = context
            .assets()
            .get(&entrypoint)
            .expect("production context must embed /index.html");
        let html = std::str::from_utf8(html.as_ref())
            .expect("embedded production index must be valid UTF-8");
        assert!(html.contains("<div id=\"root\"></div>"));
        assert!(html.contains("id=\"nea-boot\""));

        let source_index = include_str!("../../index.html");
        let loader_at = source_index
            .find("id=\"nea-boot\"")
            .expect("source index must define the boot loader");
        let module_at = source_index
            .find("src=\"/src/main.tsx\"")
            .expect("source index must load the main module");
        assert!(
            loader_at < module_at,
            "boot loader must be parsed before the main module"
        );
        assert!(source_index.contains("classList.add(\"nea-overlay\")"));
        assert!(source_index.contains("html.nea-overlay #nea-boot"));
        assert!(source_index.contains("prefers-reduced-motion: reduce"));
        assert!(source_index.contains("src=\"/nea-brand-dark.png\" width=\"784\" height=\"334\""));
        assert!(source_index.contains("src=\"/nea-brand-light.png\" width=\"784\" height=\"334\""));

        let script_at = html
            .find("<script type=\"module\"")
            .expect("production index must load a module");
        let src_relative = html[script_at..]
            .find("src=\"")
            .expect("production module must have a source");
        let src_at = script_at + src_relative + "src=\"".len();
        let src_end = src_at
            + html[src_at..]
                .find('"')
                .expect("production module source must be closed");
        let module_path = &html[src_at..src_end];
        let module = context
            .assets()
            .get(&tauri::utils::assets::AssetKey::from(module_path))
            .expect("production context must embed the main module");
        assert!(module
            .windows(b"nea-app-ready".len())
            .any(|window| window == b"nea-app-ready"));

        let main_window = context
            .config()
            .app
            .windows
            .iter()
            .find(|window| window.label == "main")
            .expect("tauri config must define the main window");
        assert_eq!(
            main_window.background_color,
            Some(tauri::utils::config::Color(234, 244, 248, 255))
        );
    }

    #[test]
    fn steam_web_sessions_deduplicate_by_steam_id_and_keep_account_name() {
        let old = steam::SteamWebSession {
            id: "old-session".to_string(),
            steam_id: Some("76561199000000001".to_string()),
            account_name: None,
            display_name: "76561199000000001".to_string(),
            note: Some("保留备注".to_string()),
            created_at: "2026-01-01T00:00:00Z".to_string(),
            last_verified_at: Some("2026-01-01T00:00:00Z".to_string()),
        };
        let imported = steam::SteamWebSession {
            id: "new-session".to_string(),
            steam_id: Some("76561199000000001".to_string()),
            account_name: Some("steam-login-name".to_string()),
            display_name: "steam-login-name".to_string(),
            note: None,
            created_at: "2026-01-02T00:00:00Z".to_string(),
            last_verified_at: Some("2026-01-02T00:00:00Z".to_string()),
        };

        let (sessions, removed) =
            deduplicate_steam_web_sessions(vec![old, imported], Some("new-session"));

        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].id, "new-session");
        assert_eq!(
            sessions[0].account_name.as_deref(),
            Some("steam-login-name")
        );
        assert_eq!(sessions[0].note.as_deref(), Some("保留备注"));
        assert_eq!(
            steam_web_session_primary_name(&sessions[0]),
            "steam-login-name"
        );
        assert_eq!(removed, vec!["old-session"]);
    }

    #[test]
    fn completed_duplicate_import_keeps_existing_entry_and_fills_missing_info() {
        let steam_id = "76561199000000031";
        let mut data = AppData::default();
        data.steam.web_sessions.push(steam::SteamWebSession {
            id: "existing-session".to_string(),
            steam_id: Some(steam_id.to_string()),
            account_name: None,
            display_name: steam_id.to_string(),
            note: Some("原备注".to_string()),
            created_at: "2026-01-01T00:00:00Z".to_string(),
            last_verified_at: Some("2026-01-01T00:00:00Z".to_string()),
        });
        data.steam.web_sessions.push(steam::SteamWebSession {
            id: "imported-session".to_string(),
            steam_id: Some(steam_id.to_string()),
            account_name: Some("resolved-login-name".to_string()),
            display_name: "Resolved Player".to_string(),
            note: None,
            created_at: "2026-01-02T00:00:00Z".to_string(),
            last_verified_at: Some("2026-01-02T00:00:00Z".to_string()),
        });

        let label =
            merge_complete_duplicate_web_import(&mut data, "imported-session", steam_id).unwrap();

        assert_eq!(label, "resolved-login-name");
        assert_eq!(data.steam.web_sessions.len(), 1);
        assert_eq!(data.steam.web_sessions[0].id, "existing-session");
        assert_eq!(
            data.steam.web_sessions[0].account_name.as_deref(),
            Some("resolved-login-name")
        );
        assert_eq!(data.steam.web_sessions[0].note.as_deref(), Some("原备注"));
        assert_eq!(data.steam_identities.len(), 1);
    }

    #[test]
    fn dev_smoke_local_client_metadata_alone_does_not_discard_a_new_web_login() {
        let steam_id = "76561199000000032";
        let mut data = AppData::default();
        data.steam.accounts.push(steam::SteamAccount {
            id: steam_id.to_string(),
            account_name: "existing-client".to_string(),
            display_name: "Existing Client".to_string(),
            remember_password: false,
            most_recent: false,
            userdata_captured: false,
            last_used_at: None,
            note: None,
        });
        data.steam.web_sessions.push(steam::SteamWebSession {
            id: "imported-session".to_string(),
            steam_id: Some(steam_id.to_string()),
            account_name: Some("unknown-alias".to_string()),
            display_name: "Unknown Alias".to_string(),
            note: None,
            created_at: now(),
            last_verified_at: Some(now()),
        });

        let label = merge_complete_duplicate_web_import(&mut data, "imported-session", steam_id);
        reconcile_steam_identities(&mut data);

        assert!(label.is_none());
        assert_eq!(data.steam.web_sessions.len(), 1);
        assert_eq!(data.steam_identities.len(), 1);
        assert!(data.steam_identities[0].capabilities.web_login);
    }

    #[test]
    fn local_client_metadata_without_web_or_credentials_is_not_retained() {
        let mut data = AppData::default();
        data.steam.accounts.push(steam::SteamAccount {
            id: "76561199000000033".to_string(),
            account_name: "metadata-only".to_string(),
            display_name: "Metadata Only".to_string(),
            remember_password: true,
            most_recent: true,
            userdata_captured: false,
            last_used_at: None,
            note: None,
        });

        reconcile_steam_identities(&mut data);

        assert!(data.steam_identities.is_empty());
    }

    #[test]
    fn credential_identity_keeps_persona_name_after_native_metadata_is_removed() {
        let steam_id = "76561199000000034";
        let mut data = AppData::default();
        data.steam.accounts.push(steam::SteamAccount {
            id: steam_id.to_string(),
            account_name: "login-name".to_string(),
            display_name: "Stable Persona".to_string(),
            remember_password: true,
            most_recent: false,
            userdata_captured: false,
            last_used_at: None,
            note: None,
        });
        data.steam_credentials.push(SteamSavedCredential {
            account_name: "login-name".to_string(),
            password: "password".to_string(),
            steam_id: Some(steam_id.to_string()),
            updated_at: now(),
        });
        reconcile_steam_identities(&mut data);
        assert_eq!(data.steam_identities[0].display_name, "Stable Persona");

        data.steam.accounts.clear();
        reconcile_steam_identities(&mut data);

        assert_eq!(data.steam_identities.len(), 1);
        assert_eq!(data.steam_identities[0].display_name, "Stable Persona");
        assert!(data.steam_identities[0].capabilities.credential);
    }

    #[test]
    fn extracts_only_steam64_from_secure_login_cookie() {
        assert_eq!(
            steam_id_from_web_cookie("76561199198704913%7C%7Csecret").as_deref(),
            Some("76561199198704913")
        );
        assert_eq!(
            steam_id_from_web_cookie("76561199198704913||secret").as_deref(),
            Some("76561199198704913")
        );
        assert!(steam_id_from_web_cookie("not-a-cookie").is_none());
    }

    #[test]
    fn most_recent_selection_is_not_treated_as_an_actual_steam_login() {
        let mut accounts = vec![
            steam::SteamAccount {
                id: "76561197960265729".to_string(),
                account_name: "one".to_string(),
                display_name: "One".to_string(),
                remember_password: true,
                most_recent: true,
                userdata_captured: false,
                last_used_at: None,
                note: None,
            },
            steam::SteamAccount {
                id: "76561197960265730".to_string(),
                account_name: "two".to_string(),
                display_name: "Two".to_string(),
                remember_password: true,
                most_recent: false,
                userdata_captured: false,
                last_used_at: None,
                note: None,
            },
        ];

        assert!(apply_steam_active_user(&mut accounts, None).is_none());
        assert!(accounts.iter().all(|account| !account.most_recent));
        assert_eq!(
            apply_steam_active_user(&mut accounts, Some(2)).as_deref(),
            Some("76561197960265730")
        );
        assert!(!accounts[0].most_recent);
        assert!(accounts[1].most_recent);

        assert!(apply_steam_runtime_state(&mut accounts, false, Some(2)).is_none());
        assert!(accounts.iter().all(|account| !account.most_recent));
    }

    #[test]
    fn extracts_steam_display_name_from_profile_xml() {
        assert_eq!(
            steam_display_name_from_profile_xml(
                "<profile><steamID><![CDATA[玩家 & Friends]]></steamID></profile>"
            )
            .as_deref(),
            Some("玩家 & Friends")
        );
        assert_eq!(
            steam_display_name_from_profile_xml(
                "<profile><steamID>Player &amp; Friends</steamID></profile>"
            )
            .as_deref(),
            Some("Player & Friends")
        );
        assert!(steam_display_name_from_profile_xml("<profile />").is_none());
    }

    #[test]
    fn login_account_name_is_not_treated_as_a_steam_display_name() {
        let session = steam::SteamWebSession {
            id: "session".to_string(),
            steam_id: Some("76561199000000033".to_string()),
            account_name: Some("private-login-name".to_string()),
            display_name: "private-login-name".to_string(),
            note: None,
            created_at: now(),
            last_verified_at: Some(now()),
        };
        assert!(steam_web_session_display_name_needs_refresh(&session));

        let account = steam::SteamAccount {
            id: "76561199000000033".to_string(),
            account_name: "private-login-name".to_string(),
            display_name: "private-login-name".to_string(),
            remember_password: true,
            most_recent: false,
            userdata_captured: false,
            last_used_at: None,
            note: None,
        };
        assert!(steam_account_display_name_needs_refresh(&account));
    }

    fn test_account(id: &str, uid: &str) -> serde_json::Value {
        serde_json::json!({
            "id": id,
            "displayName": id,
            "uid": uid,
            "pid": null,
            "userCommonId": null,
            "maskedPhone": null,
            "avatarUrl": null,
            "loginName": null,
            "note": null,
            "hasSessionSnapshot": true,
            "hasCredential": true,
            "hasLoginState": true,
            "createdAt": "2026-01-01T00:00:00Z",
            "updatedAt": "2026-01-01T00:00:00Z",
            "lastUsedAt": null
        })
    }

    fn test_config(path: Option<&str>) -> serde_json::Value {
        serde_json::json!({
            "oopzInstallDir": path,
            "oopzExePath": null,
            "roamingDataDir": null,
            "localSandboxDir": null,
            "pluginModeEnabled": false,
            "pluginAutostartEnabled": false,
            "overlayOffsetX": 0,
            "overlayOffsetY": 0,
            "overlayVertical": false
        })
    }

    #[test]
    fn interrupted_legacy_migration_merges_missing_data_without_overwrite() {
        let root = std::env::temp_dir().join(format!("nea-migration-test-{}", Uuid::new_v4()));
        let legacy = root.join("OOPZ+");
        let current = root.join("NEA");
        fs::create_dir_all(legacy.join("accounts").join("legacy")).unwrap();
        fs::create_dir_all(current.join("accounts").join("current")).unwrap();
        fs::write(legacy.join("accounts/legacy/state.bin"), "legacy").unwrap();
        fs::write(current.join("accounts/current/state.bin"), "current").unwrap();
        let legacy_data = serde_json::json!({
            "schemaVersion": 0,
            "config": test_config(Some("legacy-path")),
            "accounts": [test_account("legacy", "uid-legacy")],
            "steam": { "installation": null, "accounts": [], "currentAccountId": null }
        });
        let current_data = serde_json::json!({
            "schemaVersion": 2,
            "config": test_config(Some("current-path")),
            "accounts": [test_account("current", "uid-current")],
            "steam": { "installation": null, "accounts": [], "currentAccountId": null }
        });
        fs::write(
            legacy.join("config.json"),
            serde_json::to_vec(&legacy_data).unwrap(),
        )
        .unwrap();
        fs::write(
            current.join("config.json"),
            serde_json::to_vec(&current_data).unwrap(),
        )
        .unwrap();

        migrate_legacy_storage(&legacy, &current).unwrap();

        let merged: AppData =
            serde_json::from_slice(&fs::read(current.join("config.json")).unwrap()).unwrap();
        assert_eq!(
            merged.config.oopz_install_dir.as_deref(),
            Some("current-path")
        );
        assert_eq!(merged.accounts.len(), 2);
        assert_eq!(
            fs::read_to_string(current.join("accounts/legacy/state.bin")).unwrap(),
            "legacy"
        );
        assert_eq!(
            fs::read_to_string(current.join("accounts/current/state.bin")).unwrap(),
            "current"
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn copy_dir_recursive_replaces_complete_directory() {
        let root = std::env::temp_dir().join(format!("nea-test-{}", Uuid::new_v4()));
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
    fn oopz_import_merge_updates_only_matching_accounts_and_rejects_identity_conflicts() {
        let existing: SavedAccount =
            serde_json::from_value(test_account("existing", "uid-existing")).unwrap();
        let unrelated: SavedAccount =
            serde_json::from_value(test_account("unrelated", "uid-unrelated")).unwrap();
        let mut accounts = vec![existing.clone(), unrelated.clone()];
        let mut updated = existing.clone();
        updated.display_name = "updated".to_string();

        merge_imported_oopz_accounts(&mut accounts, std::slice::from_ref(&updated)).unwrap();

        assert_eq!(accounts.len(), 2);
        assert_eq!(accounts[0].display_name, "updated");
        assert_eq!(accounts[1].display_name, unrelated.display_name);

        let mut conflicting = updated;
        conflicting.id = "different-id".to_string();
        assert!(merge_imported_oopz_accounts(&mut accounts, &[conflicting]).is_err());
    }

    #[test]
    fn oopz_import_commit_preserves_latest_non_oopz_state() {
        let imported: SavedAccount =
            serde_json::from_value(test_account("imported", "uid-imported")).unwrap();
        let mut latest = AppData::default();
        latest
            .steam_native_switcher_exclusions
            .insert("76561198000000000".to_string());
        latest
            .perfect_unavailable_account_ids
            .insert("76561198000000001".to_string());
        latest.config.plugin_mode_enabled = true;
        let data = Mutex::new(latest);

        commit_data_update_with(
            &data,
            |current| {
                merge_imported_oopz_accounts(&mut current.accounts, std::slice::from_ref(&imported))
            },
            |_| Ok(()),
        )
        .unwrap();

        let committed = data.lock().unwrap();
        assert_eq!(committed.accounts.len(), 1);
        assert!(committed.config.plugin_mode_enabled);
        assert!(committed
            .steam_native_switcher_exclusions
            .contains("76561198000000000"));
        assert!(committed
            .perfect_unavailable_account_ids
            .contains("76561198000000001"));
    }

    #[test]
    fn export_packages_support_multi_account_legacy_and_validation() {
        let root = std::env::temp_dir().join(format!("nea-package-test-{}", Uuid::new_v4()));
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
            oopz_login: general_purpose::STANDARD
                .encode(serde_json::json!({ "uid": uid }).to_string()),
            files: vec![ExportedFile {
                path: format!("roaming/{}/state.json", uid),
                data_base64: general_purpose::STANDARD.encode(b"state"),
            }],
        };

        let package = AccountExportPackage {
            format: LEGACY_EXPORT_FORMAT_V2.to_string(),
            exported_at: now(),
            accounts: vec![entry("one"), entry("two")],
        };
        fs::write(&path, serde_json::to_vec(&package).unwrap()).unwrap();
        assert_eq!(read_export_package(&path).unwrap().len(), 2);

        let legacy_entry = entry("legacy");
        let legacy = LegacyAccountExportPackage {
            format: LEGACY_EXPORT_FORMAT_V1.to_string(),
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
            format: LEGACY_EXPORT_FORMAT_V2.to_string(),
            exported_at: now(),
            accounts: vec![unsafe_entry],
        };
        fs::write(&path, serde_json::to_vec(&unsafe_package).unwrap()).unwrap();
        assert!(read_export_package(&path).is_err());

        let duplicate_package = AccountExportPackage {
            format: LEGACY_EXPORT_FORMAT_V2.to_string(),
            exported_at: now(),
            accounts: vec![entry("duplicate"), entry("duplicate")],
        };
        fs::write(&path, serde_json::to_vec(&duplicate_package).unwrap()).unwrap();
        assert!(read_export_package(&path).is_err());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn v3_packages_extract_streamingly_and_reject_path_traversal() {
        let root = std::env::temp_dir().join(format!("nea-v3-test-{}", Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let manifest = V3ExportManifest {
            format: LEGACY_EXPORT_FORMAT_V3.to_string(),
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
                oopz_login: general_purpose::STANDARD
                    .encode(serde_json::json!({ "uid": "uid-1" }).to_string()),
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
    fn oopz_share_identity_rejects_unsafe_or_mismatched_uid() {
        let account = |uid: &str| ExportedAccount {
            display_name: "account".to_string(),
            uid: Some(uid.to_string()),
            pid: None,
            user_common_id: None,
            masked_phone: None,
            avatar_url: None,
            note: None,
        };
        let login = |uid: &str| {
            general_purpose::STANDARD.encode(serde_json::json!({ "uid": uid }).to_string())
        };
        assert_eq!(
            validate_exported_oopz_identity(&account("safe_uid-1"), &login("safe_uid-1")).unwrap(),
            "safe_uid-1"
        );
        assert!(
            validate_exported_oopz_identity(&account("../outside"), &login("../outside")).is_err()
        );
        assert!(validate_exported_oopz_identity(&account("uid-one"), &login("uid-two")).is_err());
    }

    #[test]
    fn steam_share_cookie_allowlist_excludes_machine_auth() {
        let login = Cookie::parse(
            "steamLoginSecure=76561198000000000%7Ctoken; Domain=store.steampowered.com; Path=/"
                .to_string(),
        )
        .unwrap()
        .into_owned();
        let machine = Cookie::parse(
            "steamMachineAuth76561198000000000=secret; Domain=store.steampowered.com; Path=/"
                .to_string(),
        )
        .unwrap()
        .into_owned();
        assert!(is_required_steam_share_cookie(&login));
        assert!(!is_required_steam_share_cookie(&machine));
    }

    #[test]
    fn dev_smoke_share_path_commit_rolls_back_every_applied_item() {
        let root = std::env::temp_dir().join(format!("nea-share-rollback-{}", Uuid::new_v4()));
        let backup = root.join("backup");
        let mut applied = Vec::new();
        for index in 0..2 {
            let staged = root.join(format!("staged-{index}.txt"));
            let target = root.join(format!("target-{index}.txt"));
            fs::create_dir_all(&root).unwrap();
            fs::write(&staged, format!("new-{index}")).unwrap();
            fs::write(&target, format!("old-{index}")).unwrap();
            applied.push(apply_staged_share_path(&staged, &target, &backup, index).unwrap());
            assert_eq!(fs::read_to_string(&target).unwrap(), format!("new-{index}"));
        }
        rollback_applied_share_paths(&mut applied).unwrap();
        for index in 0..2 {
            assert_eq!(
                fs::read_to_string(root.join(format!("target-{index}.txt"))).unwrap(),
                format!("old-{index}")
            );
        }
        let affected_id = "76561198000000001";
        let unaffected_id = "76561198000000002";
        let mut data = AppData::default();
        let added_at = now();
        data.steam_credentials = vec![
            SteamSavedCredential {
                account_name: "remove-on-rollback".to_string(),
                password: "new-secret".to_string(),
                steam_id: Some(affected_id.to_string()),
                updated_at: added_at.clone(),
            },
            SteamSavedCredential {
                account_name: "keep-on-rollback".to_string(),
                password: "existing-secret".to_string(),
                steam_id: Some(unaffected_id.to_string()),
                updated_at: now(),
            },
        ];
        apply_quick_share_data_rollback(
            &mut data,
            &HashSet::from([affected_id.to_string()]),
            &[],
            &HashMap::new(),
            &[QuickShareCredentialRollback {
                steam_id: affected_id.to_string(),
                normalized_account_name: "remove-on-rollback".to_string(),
                updated_at: added_at,
            }],
        );
        assert_eq!(data.steam_credentials.len(), 1);
        assert_eq!(data.steam_credentials[0].account_name, "keep-on-rollback");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn share_path_rollback_removes_new_targets_and_restores_failed_replace() {
        let root = std::env::temp_dir().join(format!("nea-share-rollback-{}", Uuid::new_v4()));
        let backup = root.join("backup");
        fs::create_dir_all(&root).unwrap();

        let staged_new = root.join("staged-new.txt");
        let target_new = root.join("target-new.txt");
        fs::write(&staged_new, b"new").unwrap();
        let mut applied =
            vec![apply_staged_share_path(&staged_new, &target_new, &backup, 0).unwrap()];
        rollback_applied_share_paths(&mut applied).unwrap();
        assert!(!target_new.exists());

        let missing_staged = root.join("missing.txt");
        let existing_target = root.join("existing.txt");
        fs::write(&existing_target, b"original").unwrap();
        assert!(apply_staged_share_path(&missing_staged, &existing_target, &backup, 1).is_err());
        assert_eq!(fs::read(&existing_target).unwrap(), b"original");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn quick_share_rollback_journal_replaces_previous_version() {
        let root = std::env::temp_dir().join(format!("nea-share-journal-{}", Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let mut journal = QuickShareRollbackJournal {
            affected_steam_ids: vec!["76561198000000001".to_string()],
            web_sessions: Vec::new(),
            perfect_profiles: HashMap::new(),
            added_credentials: vec![QuickShareCredentialRollback {
                steam_id: "76561198000000001".to_string(),
                normalized_account_name: "remove-on-rollback".to_string(),
                updated_at: "2026-07-16T00:00:00Z".to_string(),
            }],
            paths: Vec::new(),
        };
        write_quick_share_rollback_journal(&root, &journal).unwrap();
        record_quick_share_rollback_path(
            &root,
            &mut journal,
            &PathBuf::from(r"C:\target"),
            Some(root.join("rollback").join("item-0")),
        )
        .unwrap();
        let saved: QuickShareRollbackJournal =
            serde_json::from_slice(&fs::read(root.join("rollback-journal.json")).unwrap()).unwrap();
        assert_eq!(saved.paths.len(), 1);
        let raw = fs::read_to_string(root.join("rollback-journal.json")).unwrap();
        assert!(!raw.contains("new-secret"));
        assert!(!raw.contains("existing-secret"));
        assert!(!root.join("rollback-journal.json.bak").exists());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn quick_share_data_rollback_preserves_unaffected_sessions() {
        let affected_id = "76561198000000001";
        let unaffected_id = "76561198000000002";
        let before = test_shared_web_session(affected_id).session;
        let unaffected = test_shared_web_session(unaffected_id).session;
        let mut current = AppData::default();
        let mut imported = before.clone();
        imported.display_name = "Imported".to_string();
        current.steam.web_sessions = vec![imported, unaffected.clone()];
        apply_quick_share_data_rollback(
            &mut current,
            &HashSet::from([affected_id.to_string()]),
            std::slice::from_ref(&before),
            &HashMap::new(),
            &[],
        );
        assert!(current
            .steam
            .web_sessions
            .iter()
            .any(|session| session.steam_id.as_deref() == Some(affected_id)
                && session.display_name == before.display_name));
        assert!(current
            .steam
            .web_sessions
            .iter()
            .any(|session| session.id == unaffected.id));
    }

    #[test]
    fn steam_store_account_page_parser_requires_matching_account_id() {
        assert!(!validate_shared_steam_id("76561197960265728"));
        assert!(validate_shared_steam_id("76561198000000001"));
        assert_eq!(
            steam_account_id_from_store_html("<script>var g_AccountID = 39734273;</script>"),
            Some(39_734_273)
        );
        assert_eq!(
            steam_account_id_from_store_html("<script>var g_AccountID = 0;</script>"),
            Some(0)
        );
        assert_eq!(steam_account_id_from_store_html("Sign In"), None);
    }

    #[test]
    fn stale_share_cleanup_pattern_only_accepts_generated_uuid_names() {
        let id = Uuid::new_v4();
        assert!(matches_uuid_artifact(
            &format!("nea-share-{id}.nea"),
            "nea-share-",
            ".nea"
        ));
        assert!(matches_uuid_artifact(
            &format!("nea-share-{id}.nea-share"),
            "nea-share-",
            ".nea-share"
        ));
        assert!(!matches_uuid_artifact(
            "nea-share-my-export.nea",
            "nea-share-",
            ".nea"
        ));
    }

    #[test]
    fn share_packaging_copy_honors_cancellation() {
        let cancelled = AtomicBool::new(true);
        let mut source = std::io::Cursor::new(vec![1u8; 1024]);
        let mut target = Vec::new();
        let mut buffer = vec![0u8; 256 * 1024];
        assert_eq!(
            copy_with_share_cancellation_buffer(
                &mut source,
                &mut target,
                Some(&cancelled),
                &mut buffer,
            )
            .unwrap_err(),
            QUICK_SHARE_CANCELLED
        );
        assert!(target.is_empty());
    }

    #[test]
    fn snapshot_rollback_only_reverts_started_entries() {
        let root = std::env::temp_dir().join(format!("nea-rollback-test-{}", Uuid::new_v4()));
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
            let root = std::env::temp_dir().join(format!("nea-wormhole-test-{}", Uuid::new_v4()));
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
