use crate::adapters::{AccountIdentity, AppAdapter, AppInstallation};
use chrono::{DateTime, Local, NaiveDateTime, Utc};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    fs,
    io::{Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    process::Command,
    sync::{mpsc, Mutex, OnceLock},
    thread,
    time::Duration,
};
use sysinfo::{ProcessRefreshKind, Signal, System, UpdateKind};
use winreg::{enums::*, RegKey};

static LOGINUSERS_WRITE_LOCK: Mutex<()> = Mutex::new(());
static STEAM_LOGIN_TRANSITION_LOCK: Mutex<()> = Mutex::new(());
static NATIVE_SWITCHER_WORKER: OnceLock<mpsc::Sender<(SteamInstallation, String)>> =
    OnceLock::new();

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct SteamInstallation {
    pub install_dir: String,
    pub executable: String,
    pub valid: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct SteamAccount {
    pub id: String,
    pub account_name: String,
    pub display_name: String,
    pub remember_password: bool,
    pub most_recent: bool,
    pub userdata_captured: bool,
    pub last_used_at: Option<String>,
    #[serde(default)]
    pub note: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SteamWebSession {
    pub id: String,
    pub steam_id: Option<String>,
    #[serde(default)]
    pub account_name: Option<String>,
    pub display_name: String,
    #[serde(default)]
    pub note: Option<String>,
    pub created_at: String,
    pub last_verified_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct SteamWorkspace {
    pub installation: Option<SteamInstallation>,
    pub accounts: Vec<SteamAccount>,
    pub current_account_id: Option<String>,
    #[serde(default)]
    pub client_online: bool,
    #[serde(default)]
    pub web_sessions: Vec<SteamWebSession>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SteamConnectionState {
    Unknown,
    Connecting,
    Online,
    Offline,
}

#[derive(Debug)]
pub struct SteamConnectionLogMonitor {
    path: PathBuf,
    offset: u64,
    pending: String,
    target_account_id: u32,
    state: SteamConnectionState,
}

impl SteamConnectionLogMonitor {
    fn new(
        path: PathBuf,
        offset: u64,
        target_account_id: u32,
        state: SteamConnectionState,
    ) -> Self {
        Self {
            path,
            offset,
            pending: String::new(),
            target_account_id,
            state,
        }
    }

    pub fn state(&self) -> SteamConnectionState {
        self.state
    }

    fn consume(&mut self, appended: &[u8]) {
        self.pending.push_str(&String::from_utf8_lossy(appended));
        while let Some(newline) = self.pending.find('\n') {
            let line = self.pending[..newline].trim_end_matches('\r');
            self.state = connection_state_after_line(self.state, line, self.target_account_id);
            self.pending.drain(..=newline);
        }
    }

    pub fn poll(&mut self) -> SteamConnectionState {
        let Ok(mut file) = fs::File::open(&self.path) else {
            return self.state;
        };
        let Ok(length) = file.metadata().map(|metadata| metadata.len()) else {
            return self.state;
        };
        if length < self.offset {
            let previous_path = self.path.with_file_name("connection_log.previous.txt");
            let recovered_rotation_tail =
                fs::File::open(previous_path).ok().and_then(|mut previous| {
                    let previous_length = previous.metadata().ok()?.len();
                    if previous_length < self.offset
                        || previous.seek(SeekFrom::Start(self.offset)).is_err()
                    {
                        return None;
                    }
                    let mut tail = Vec::new();
                    previous.read_to_end(&mut tail).ok()?;
                    Some(tail)
                });
            if let Some(mut tail) = recovered_rotation_tail {
                tail.push(b'\n');
                self.consume(&tail);
            } else {
                self.pending.clear();
                self.state = SteamConnectionState::Unknown;
            }
            self.offset = 0;
        }
        if length == self.offset || file.seek(SeekFrom::Start(self.offset)).is_err() {
            return self.state;
        }
        let mut appended = Vec::with_capacity((length - self.offset).min(64 * 1024) as usize);
        if file.read_to_end(&mut appended).is_err() {
            return self.state;
        }
        self.offset = self.offset.saturating_add(appended.len() as u64);
        self.consume(&appended);
        self.state
    }
}

#[derive(Debug, Clone)]
enum VdfValue {
    Text(String),
    Object(BTreeMap<String, VdfValue>),
}

fn tokenize_vdf(input: &str) -> Result<Vec<String>, String> {
    let mut tokens = Vec::new();
    let mut chars = input.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            c if c.is_whitespace() => {}
            '/' if chars.peek() == Some(&'/') => {
                for next in chars.by_ref() {
                    if next == '\n' {
                        break;
                    }
                }
            }
            '{' | '}' => tokens.push(ch.to_string()),
            '"' => {
                let mut value = String::new();
                let mut escaped = false;
                for next in chars.by_ref() {
                    if escaped {
                        value.push(next);
                        escaped = false;
                    } else if next == '\\' {
                        escaped = true;
                    } else if next == '"' {
                        break;
                    } else {
                        value.push(next);
                    }
                }
                tokens.push(value);
            }
            _ => return Err("Steam VDF 包含无法识别的语法".to_string()),
        }
    }
    Ok(tokens)
}

fn parse_object(
    tokens: &[String],
    index: &mut usize,
    nested: bool,
) -> Result<BTreeMap<String, VdfValue>, String> {
    let mut values = BTreeMap::new();
    while *index < tokens.len() {
        if tokens[*index] == "}" {
            if !nested {
                return Err("Steam VDF 存在多余的结束符".to_string());
            }
            *index += 1;
            return Ok(values);
        }
        let key = tokens[*index].clone();
        *index += 1;
        let Some(token) = tokens.get(*index) else {
            return Err("Steam VDF 字段缺少值".to_string());
        };
        if token == "{" {
            *index += 1;
            values.insert(key, VdfValue::Object(parse_object(tokens, index, true)?));
        } else if token == "}" {
            return Err("Steam VDF 字段缺少值".to_string());
        } else {
            values.insert(key, VdfValue::Text(token.clone()));
            *index += 1;
        }
    }
    if nested {
        Err("Steam VDF 对象未闭合".to_string())
    } else {
        Ok(values)
    }
}

fn parse_vdf(input: &str) -> Result<BTreeMap<String, VdfValue>, String> {
    let tokens = tokenize_vdf(input)?;
    let mut index = 0;
    let result = parse_object(&tokens, &mut index, false)?;
    if index != tokens.len() {
        return Err("Steam VDF 尾部包含无效内容".to_string());
    }
    Ok(result)
}

fn escape_vdf(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

fn write_vdf_object(output: &mut String, object: &BTreeMap<String, VdfValue>, depth: usize) {
    let indent = "\t".repeat(depth);
    for (key, value) in object {
        match value {
            VdfValue::Text(text) => output.push_str(&format!(
                "{}\"{}\"\t\t\"{}\"\n",
                indent,
                escape_vdf(key),
                escape_vdf(text)
            )),
            VdfValue::Object(child) => {
                output.push_str(&format!(
                    "{}\"{}\"\n{}{{\n",
                    indent,
                    escape_vdf(key),
                    indent
                ));
                write_vdf_object(output, child, depth + 1);
                output.push_str(&format!("{}}}\n", indent));
            }
        }
    }
}

fn serialize_vdf(object: &BTreeMap<String, VdfValue>) -> String {
    let mut output = String::new();
    write_vdf_object(&mut output, object, 0);
    output
}

fn set_text(object: &mut BTreeMap<String, VdfValue>, key: &str, value: &str) -> bool {
    if text(object, key) == Some(value) {
        return false;
    }
    if let Some(existing) = object
        .keys()
        .find(|name| name.eq_ignore_ascii_case(key))
        .cloned()
    {
        object.insert(existing, VdfValue::Text(value.to_string()));
    } else {
        object.insert(key.to_string(), VdfValue::Text(value.to_string()));
    }
    true
}

fn users_mut(
    root: &mut BTreeMap<String, VdfValue>,
) -> Result<&mut BTreeMap<String, VdfValue>, String> {
    root.iter_mut()
        .find(|(key, _)| key.eq_ignore_ascii_case("users"))
        .and_then(|(_, value)| match value {
            VdfValue::Object(users) => Some(users),
            _ => None,
        })
        .ok_or_else(|| "Steam loginusers.vdf 缺少 users 对象".to_string())
}

fn replace_text_atomically(path: &Path, contents: &str) -> Result<(), String> {
    let staging = path.with_extension("vdf.nea-new");
    let backup = path.with_extension("vdf.nea-backup");
    fs::write(&staging, contents).map_err(|error| format!("写入 Steam 临时配置失败: {error}"))?;
    if backup.exists() {
        fs::remove_file(&backup).map_err(|error| format!("清理 Steam 临时备份失败: {error}"))?;
    }
    fs::rename(path, &backup).map_err(|error| format!("备份 Steam 配置失败: {error}"))?;
    if let Err(error) = fs::rename(&staging, path) {
        let _ = fs::rename(&backup, path);
        return Err(format!("替换 Steam 配置失败: {error}"));
    }
    let _ = fs::remove_file(backup);
    Ok(())
}

fn text<'a>(object: &'a BTreeMap<String, VdfValue>, key: &str) -> Option<&'a str> {
    object
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case(key))
        .and_then(|(_, value)| match value {
            VdfValue::Text(value) => Some(value.as_str()),
            _ => None,
        })
}

fn users(root: &BTreeMap<String, VdfValue>) -> Result<&BTreeMap<String, VdfValue>, String> {
    root.iter()
        .find(|(key, _)| key.eq_ignore_ascii_case("users"))
        .and_then(|(_, value)| match value {
            VdfValue::Object(users) => Some(users),
            _ => None,
        })
        .ok_or_else(|| "Steam loginusers.vdf 缺少 users 对象".to_string())
}

pub struct SteamAdapter;

impl SteamAdapter {
    pub fn with_login_transition<T>(operation: impl FnOnce() -> T) -> T {
        let _guard = STEAM_LOGIN_TRANSITION_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        operation()
    }

    pub fn account_id32(steam_id64: &str) -> Option<u32> {
        let steam_id = steam_id64.parse::<u64>().ok()?;
        (steam_id64.len() == 17).then_some((steam_id & u32::MAX as u64) as u32)
    }

    fn connection_log_path(installation: &AppInstallation) -> PathBuf {
        installation
            .data_dir
            .join("logs")
            .join("connection_log.txt")
    }

    pub fn monitor_next_connection(
        installation: &AppInstallation,
        steam_id64: &str,
    ) -> Result<SteamConnectionLogMonitor, String> {
        let target_account_id = Self::account_id32(steam_id64)
            .ok_or_else(|| "目标 SteamID64 无效，无法确认客户端在线状态".to_string())?;
        let path = Self::connection_log_path(installation);
        let offset = fs::metadata(&path)
            .map(|metadata| metadata.len())
            .unwrap_or(0);
        Ok(SteamConnectionLogMonitor::new(
            path,
            offset,
            target_account_id,
            SteamConnectionState::Unknown,
        ))
    }

    pub fn monitor_current_connection(
        installation: &AppInstallation,
        steam_id64: &str,
    ) -> Result<SteamConnectionLogMonitor, String> {
        let target_account_id = Self::account_id32(steam_id64)
            .ok_or_else(|| "目标 SteamID64 无效，无法确认客户端在线状态".to_string())?;
        let path = Self::connection_log_path(installation);
        let current = fs::read(&path).unwrap_or_default();
        let offset = current.len() as u64;
        let complete_length = current
            .iter()
            .rposition(|byte| *byte == b'\n')
            .map(|index| index + 1)
            .unwrap_or(0);
        let pending = String::from_utf8_lossy(&current[complete_length..]).into_owned();
        let session_started_at = steam_client_started_at(&process_system());
        let mut state = SteamConnectionState::Unknown;
        if let Some(session_started_at) = session_started_at {
            let previous = installation
                .data_dir
                .join("logs")
                .join("connection_log.previous.txt");
            for contents in [
                fs::read(previous).unwrap_or_default(),
                current[..complete_length].to_vec(),
            ] {
                state = connection_state_from_session_log(
                    state,
                    &contents,
                    target_account_id,
                    session_started_at,
                );
            }
        }
        let mut monitor = SteamConnectionLogMonitor::new(path, offset, target_account_id, state);
        monitor.pending = pending;
        Ok(monitor)
    }

    pub fn is_account_online(installation: &AppInstallation, steam_id64: &str) -> bool {
        Self::is_account_active(steam_id64)
            && Self::monitor_current_connection(installation, steam_id64)
                .is_ok_and(|monitor| monitor.state() == SteamConnectionState::Online)
    }

    pub fn active_user_account_id() -> Option<u32> {
        RegKey::predef(HKEY_CURRENT_USER)
            .open_subkey("Software\\Valve\\Steam\\ActiveProcess")
            .ok()?
            .get_value::<u32, _>("ActiveUser")
            .ok()
            .filter(|account_id| *account_id != 0)
    }

    pub fn is_account_active(steam_id64: &str) -> bool {
        Self::account_id32(steam_id64)
            .is_some_and(|account_id| Self::active_user_account_id() == Some(account_id))
    }

    pub fn client_is_running() -> bool {
        steam_client_running(&process_system())
    }

    pub fn processes_are_running() -> bool {
        steam_process_running(&process_system())
    }

    pub fn suppress_accounts_from_native_switcher(
        installation: &SteamInstallation,
        steam_ids: &HashSet<String>,
    ) -> Result<usize, String> {
        if steam_ids.is_empty() {
            return Ok(0);
        }
        let _write_guard = LOGINUSERS_WRITE_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let path = PathBuf::from(&installation.install_dir)
            .join("config")
            .join("loginusers.vdf");
        let raw = fs::read_to_string(&path)
            .map_err(|error| format!("读取 Steam 原生账号列表失败: {error}"))?;
        let mut root = parse_vdf(&raw)?;
        let mut changed_accounts = 0usize;
        for (steam_id, value) in users_mut(&mut root)? {
            if !steam_ids.contains(steam_id) {
                continue;
            }
            let VdfValue::Object(account) = value else {
                continue;
            };
            let changed = set_text(account, "RememberPassword", "0")
                | set_text(account, "AllowAutoLogin", "0")
                | set_text(account, "MostRecent", "0")
                | set_text(account, "Timestamp", "0");
            changed_accounts += usize::from(changed);
        }
        if changed_accounts > 0 {
            replace_text_atomically(&path, &serialize_vdf(&root))?;
        }
        Ok(changed_accounts)
    }

    pub fn suppress_accounts_from_native_switcher_if_stopped(
        installation: &SteamInstallation,
        steam_ids: &HashSet<String>,
    ) -> Result<Option<usize>, String> {
        Self::with_login_transition(|| {
            if Self::processes_are_running() {
                return Ok(None);
            }
            Self::suppress_accounts_from_native_switcher(installation, steam_ids).map(Some)
        })
    }

    pub fn prepare_account_for_online_login(
        installation: &SteamInstallation,
        steam_id: &str,
    ) -> Result<bool, String> {
        let _write_guard = LOGINUSERS_WRITE_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let path = PathBuf::from(&installation.install_dir)
            .join("config")
            .join("loginusers.vdf");
        let raw = match fs::read_to_string(&path) {
            Ok(raw) => raw,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(error) => return Err(format!("读取 Steam 在线登录配置失败: {error}")),
        };
        let mut root = parse_vdf(&raw)?;
        let Some(VdfValue::Object(account)) = users_mut(&mut root)?.get_mut(steam_id) else {
            return Ok(false);
        };
        let changed = set_text(account, "WantsOfflineMode", "0")
            | set_text(account, "SkipOfflineModeWarning", "1");
        if changed {
            replace_text_atomically(&path, &serialize_vdf(&root))?;
        }
        Ok(changed)
    }

    pub fn keep_account_out_of_native_switcher(
        installation: SteamInstallation,
        steam_id: String,
    ) -> Result<(), String> {
        let steam_ids = HashSet::from([steam_id.clone()]);
        let immediate_result =
            Self::suppress_accounts_from_native_switcher_if_stopped(&installation, &steam_ids)
                .map(|_| ());
        let sender = NATIVE_SWITCHER_WORKER.get_or_init(|| {
            let (sender, receiver) = mpsc::channel();
            thread::spawn(move || native_switcher_worker(receiver));
            sender
        });
        sender
            .send((installation, steam_id))
            .map_err(|_| "Steam 原生账号清理任务已停止".to_string())?;
        immediate_result
    }

    fn registry_install_path() -> Option<PathBuf> {
        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        if let Ok(key) = hkcu.open_subkey("Software\\Valve\\Steam") {
            for name in ["SteamPath", "InstallPath"] {
                if let Ok(value) = key.get_value::<String, _>(name) {
                    let path = PathBuf::from(value.replace('/', "\\"));
                    if path.join("steam.exe").is_file() {
                        return Some(path);
                    }
                }
            }
        }
        for hive in [HKEY_LOCAL_MACHINE] {
            let root = RegKey::predef(hive);
            for key_name in [
                "SOFTWARE\\WOW6432Node\\Valve\\Steam",
                "SOFTWARE\\Valve\\Steam",
            ] {
                if let Ok(key) = root.open_subkey(key_name) {
                    if let Ok(value) = key.get_value::<String, _>("InstallPath") {
                        let path = PathBuf::from(value);
                        if path.join("steam.exe").is_file() {
                            return Some(path);
                        }
                    }
                }
            }
        }
        None
    }

    pub fn discover_installation() -> Result<SteamInstallation, String> {
        let install_dir =
            Self::registry_install_path().ok_or_else(|| "未找到 Steam 安装目录".to_string())?;
        let executable = install_dir.join("steam.exe");
        Ok(SteamInstallation {
            install_dir: install_dir.to_string_lossy().to_string(),
            executable: executable.to_string_lossy().to_string(),
            valid: executable.is_file(),
        })
    }

    pub fn read_accounts(installation: &SteamInstallation) -> Result<Vec<SteamAccount>, String> {
        let raw = fs::read_to_string(
            PathBuf::from(&installation.install_dir)
                .join("config")
                .join("loginusers.vdf"),
        )
        .map_err(|error| format!("读取 Steam 账号列表失败: {}", error))?;
        let root = parse_vdf(&raw)?;
        let mut accounts = Vec::new();
        for (steam_id, value) in users(&root)? {
            let VdfValue::Object(account) = value else {
                continue;
            };
            let account_name = text(account, "AccountName").unwrap_or_default().to_string();
            let persona_name = text(account, "PersonaName")
                .unwrap_or(&account_name)
                .to_string();
            if account_name.is_empty() {
                continue;
            }
            accounts.push(SteamAccount {
                id: steam_id.clone(),
                account_name,
                display_name: persona_name,
                remember_password: text(account, "RememberPassword") == Some("1"),
                most_recent: text(account, "MostRecent") == Some("1"),
                userdata_captured: false,
                last_used_at: None,
                note: None,
            });
        }
        accounts.sort_by(|left, right| {
            right
                .most_recent
                .cmp(&left.most_recent)
                .then_with(|| left.display_name.cmp(&right.display_name))
        });
        Ok(accounts)
    }

    pub fn read_accounts_stable(
        installation: &SteamInstallation,
    ) -> Result<Vec<SteamAccount>, String> {
        let mut last_error = None;
        for attempt in 0..10 {
            match Self::read_accounts(installation) {
                Ok(accounts) => return Ok(accounts),
                Err(error) => last_error = Some(error),
            }
            if attempt < 9 {
                thread::sleep(Duration::from_millis(200));
            }
        }
        Err(last_error.unwrap_or_else(|| "读取 Steam 账号列表失败".to_string()))
    }
}

fn connection_log_timestamp(line: &str) -> Option<NaiveDateTime> {
    let timestamp = line.get(1..20)?;
    NaiveDateTime::parse_from_str(timestamp, "%Y-%m-%d %H:%M:%S").ok()
}

fn connection_state_after_line(
    current: SteamConnectionState,
    line: &str,
    target_account_id: u32,
) -> SteamConnectionState {
    let target = format!("[U:1:{target_account_id}]");
    if !line.contains(&target) {
        return current;
    }
    if line.contains("LogOff()")
        || line.contains("AsyncDisconnect")
        || line.contains("RecvMsgClientLoggedOff")
        || line.contains("Log session ended")
        || line.contains("[Logging Off,")
    {
        return SteamConnectionState::Offline;
    }
    if line.contains("StartAutoReconnect")
        || line.contains("[Connecting,")
        || line.contains("[Connected,")
        || line.contains("[Logging On,")
        || line.contains("LogOn() called")
    {
        return SteamConnectionState::Connecting;
    }
    if line.contains("[Logged On,") {
        return SteamConnectionState::Online;
    }
    if line.contains("[Logged Off,") {
        return SteamConnectionState::Offline;
    }
    current
}

fn connection_state_from_session_log(
    mut state: SteamConnectionState,
    contents: &[u8],
    target_account_id: u32,
    session_started_at: NaiveDateTime,
) -> SteamConnectionState {
    for line in String::from_utf8_lossy(contents).lines() {
        let Some(timestamp) = connection_log_timestamp(line) else {
            continue;
        };
        if timestamp < session_started_at {
            continue;
        }
        state = connection_state_after_line(state, line, target_account_id);
    }
    state
}

fn steam_client_started_at(system: &System) -> Option<NaiveDateTime> {
    let started_at = system
        .processes()
        .values()
        .filter(|process| process.name().eq_ignore_ascii_case("steam.exe"))
        .map(|process| process.start_time())
        .max()?;
    DateTime::<Utc>::from_timestamp(started_at as i64, 0)
        .map(|value| value.with_timezone(&Local).naive_local() - chrono::Duration::seconds(2))
}

fn native_switcher_worker(receiver: mpsc::Receiver<(SteamInstallation, String)>) {
    let mut pending: HashMap<String, (SteamInstallation, HashSet<String>)> = HashMap::new();
    loop {
        let Ok((installation, steam_id)) = receiver.recv() else {
            return;
        };
        pending
            .entry(installation.install_dir.clone())
            .or_insert_with(|| (installation, HashSet::new()))
            .1
            .insert(steam_id);
        let mut system = process_system();
        let mut cleanup_attempts = 0u32;
        loop {
            while let Ok((installation, steam_id)) = receiver.try_recv() {
                pending
                    .entry(installation.install_dir.clone())
                    .or_insert_with(|| (installation, HashSet::new()))
                    .1
                    .insert(steam_id);
            }
            if steam_process_running(&system) {
                match receiver.recv_timeout(Duration::from_secs(2)) {
                    Ok((installation, steam_id)) => {
                        pending
                            .entry(installation.install_dir.clone())
                            .or_insert_with(|| (installation, HashSet::new()))
                            .1
                            .insert(steam_id);
                    }
                    Err(mpsc::RecvTimeoutError::Timeout) => refresh_processes(&mut system),
                    Err(mpsc::RecvTimeoutError::Disconnected) => return,
                }
                continue;
            }
            // Steam may flush loginusers.vdf immediately after its last process exits,
            // or briefly have no process while another NEA switch is starting.
            match receiver.recv_timeout(Duration::from_millis(800)) {
                Ok((installation, steam_id)) => {
                    pending
                        .entry(installation.install_dir.clone())
                        .or_insert_with(|| (installation, HashSet::new()))
                        .1
                        .insert(steam_id);
                    refresh_processes(&mut system);
                    continue;
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => return,
                Err(mpsc::RecvTimeoutError::Timeout) => {}
            }
            refresh_processes(&mut system);
            if steam_process_running(&system) {
                continue;
            }
            let batch = std::mem::take(&mut pending);
            for (install_dir, (installation, steam_ids)) in batch {
                if !matches!(
                    SteamAdapter::suppress_accounts_from_native_switcher_if_stopped(
                        &installation,
                        &steam_ids,
                    ),
                    Ok(Some(_))
                ) {
                    pending.insert(install_dir, (installation, steam_ids));
                }
            }
            if pending.is_empty() {
                break;
            }
            cleanup_attempts += 1;
            if cleanup_attempts >= 5 {
                // Keep failed work in `pending`; the next switch request will retry it.
                break;
            }
            let retry_delay = Duration::from_millis(250u64 * (1u64 << (cleanup_attempts - 1)));
            match receiver.recv_timeout(retry_delay) {
                Ok((installation, steam_id)) => {
                    pending
                        .entry(installation.install_dir.clone())
                        .or_insert_with(|| (installation, HashSet::new()))
                        .1
                        .insert(steam_id);
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => return,
                Err(mpsc::RecvTimeoutError::Timeout) => {}
            }
            refresh_processes(&mut system);
        }
    }
}

fn process_system() -> System {
    let mut system = System::new();
    system.refresh_processes_specifics(ProcessRefreshKind::new().with_exe(UpdateKind::Always));
    system
}

fn steam_process_running(system: &System) -> bool {
    system.processes().values().any(|process| {
        process.name().eq_ignore_ascii_case("steam.exe")
            || process.name().eq_ignore_ascii_case("steamwebhelper.exe")
    })
}

fn steam_client_running(system: &System) -> bool {
    system
        .processes()
        .values()
        .any(|process| process.name().eq_ignore_ascii_case("steam.exe"))
}

fn refresh_processes(system: &mut System) {
    system.refresh_processes_specifics(ProcessRefreshKind::new());
}

fn steam_processes_stably_stopped(system: &mut System) -> bool {
    if steam_process_running(system) {
        return false;
    }
    for _ in 0..4 {
        thread::sleep(Duration::from_millis(200));
        refresh_processes(system);
        if steam_process_running(system) {
            return false;
        }
    }
    true
}

impl SteamAdapter {
    pub fn start_with_credentials(
        installation: &AppInstallation,
        account_name: &str,
        password: &str,
    ) -> Result<(), String> {
        if account_name.trim().is_empty() || password.is_empty() {
            return Err("Steam 账号或密码为空".to_string());
        }
        Command::new(&installation.executable)
            .current_dir(&installation.data_dir)
            .arg("-login")
            .arg(account_name)
            .arg(password)
            .spawn()
            .map(|_| ())
            .map_err(|error| format!("使用账号密码启动 Steam 失败: {}", error))
    }
}

impl AppAdapter for SteamAdapter {
    fn id(&self) -> &'static str {
        "steam"
    }
    fn display_name(&self) -> &'static str {
        "Steam"
    }
    fn discover(&self) -> Result<AppInstallation, String> {
        let found = Self::discover_installation()?;
        Ok(AppInstallation {
            executable: PathBuf::from(found.executable),
            data_dir: PathBuf::from(found.install_dir),
        })
    }
    fn inspect_current_account(
        &self,
        installation: &AppInstallation,
    ) -> Result<Option<AccountIdentity>, String> {
        let workspace = SteamInstallation {
            install_dir: installation.data_dir.to_string_lossy().to_string(),
            executable: installation.executable.to_string_lossy().to_string(),
            valid: true,
        };
        if !Self::client_is_running() {
            return Ok(None);
        }
        let active_user = Self::active_user_account_id();
        Ok(Self::read_accounts_stable(&workspace)?
            .into_iter()
            .find(|account| Self::account_id32(&account.id) == active_user)
            .map(|account| AccountIdentity {
                external_id: account.id,
                display_name: account.display_name,
            }))
    }
    fn scan_accounts(
        &self,
        installation: &AppInstallation,
    ) -> Result<Vec<AccountIdentity>, String> {
        let workspace = SteamInstallation {
            install_dir: installation.data_dir.to_string_lossy().to_string(),
            executable: installation.executable.to_string_lossy().to_string(),
            valid: true,
        };
        Ok(Self::read_accounts_stable(&workspace)?
            .into_iter()
            .map(|account| AccountIdentity {
                external_id: account.id,
                display_name: account.display_name,
            })
            .collect())
    }
    fn stop(&self, installation: &AppInstallation) -> Result<(), String> {
        let mut system = process_system();
        if steam_processes_stably_stopped(&mut system) {
            return Ok(());
        }

        let _ = Command::new(&installation.executable)
            .current_dir(&installation.data_dir)
            .arg("-shutdown")
            .spawn();
        for _ in 0..20 {
            thread::sleep(Duration::from_millis(500));
            refresh_processes(&mut system);
            if !steam_process_running(&system) && steam_processes_stably_stopped(&mut system) {
                return Ok(());
            }
        }

        for process in system.processes().values() {
            if process.name().eq_ignore_ascii_case("steam.exe")
                || process.name().eq_ignore_ascii_case("steamwebhelper.exe")
            {
                let _ = process
                    .kill_with(Signal::Term)
                    .unwrap_or_else(|| process.kill());
            }
        }
        for _ in 0..10 {
            thread::sleep(Duration::from_millis(500));
            refresh_processes(&mut system);
            if !steam_process_running(&system) && steam_processes_stably_stopped(&mut system) {
                return Ok(());
            }
        }
        for process in system.processes().values() {
            if process.name().eq_ignore_ascii_case("steam.exe")
                || process.name().eq_ignore_ascii_case("steamwebhelper.exe")
            {
                let _ = process.kill();
            }
        }
        thread::sleep(Duration::from_millis(500));
        refresh_processes(&mut system);
        if !steam_process_running(&system) && steam_processes_stably_stopped(&mut system) {
            Ok(())
        } else {
            Err("Steam 进程无法完全退出，已中止切号以防登录状态被覆盖".to_string())
        }
    }
    fn start(&self, installation: &AppInstallation) -> Result<(), String> {
        Command::new(&installation.executable)
            .current_dir(&installation.data_dir)
            .spawn()
            .map(|_| ())
            .map_err(|error| format!("启动 Steam 失败: {}", error))
    }
    fn is_running(&self, _installation: &AppInstallation) -> bool {
        Self::client_is_running()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write as _;

    #[test]
    fn parses_loginusers_vdf_as_read_only_metadata() {
        let input = "\"users\"\n{\n\"1\" { \"AccountName\" \"one\" \"PersonaName\" \"One\" \"MostRecent\" \"1\" \"RememberPassword\" \"1\" \"AllowAutoLogin\" \"1\" }\n\"2\" { \"AccountName\" \"two\" \"MostRecent\" \"0\" \"RememberPassword\" \"1\" \"AllowAutoLogin\" \"0\" }\n}";
        let root = parse_vdf(input).unwrap();
        assert_eq!(users(&root).unwrap().len(), 2);
        let list = users(&root).unwrap();
        let VdfValue::Object(first) = list.get("1").unwrap() else {
            panic!()
        };
        let VdfValue::Object(second) = list.get("2").unwrap() else {
            panic!()
        };
        assert_eq!(text(first, "AccountName"), Some("one"));
        assert_eq!(text(first, "PersonaName"), Some("One"));
        assert_eq!(text(second, "AccountName"), Some("two"));
    }

    #[test]
    fn steam_id64_maps_to_the_active_user_account_id() {
        assert_eq!(SteamAdapter::account_id32("76561197960265728"), Some(0));
        assert_eq!(
            SteamAdapter::account_id32("76561199198704913"),
            Some(1_238_439_185)
        );
        assert!(SteamAdapter::account_id32("invalid").is_none());
    }

    #[test]
    fn rejects_unclosed_vdf() {
        assert!(parse_vdf("\"users\" { \"1\" {").is_err());
    }

    #[test]
    fn steam_connection_parser_requires_the_target_to_reach_logged_on() {
        let target = 1_217_010_099;
        let other = 918_826_622;
        let mut state = SteamConnectionState::Unknown;
        state = connection_state_after_line(
            state,
            &format!("[2026-07-17 23:05:17] [Logged On, 4, 27] [U:1:{other}] processing complete"),
            target,
        );
        assert_eq!(state, SteamConnectionState::Unknown);
        state = connection_state_after_line(
            state,
            &format!("[2026-07-17 23:05:19] [Connecting, 4, 0] [U:1:{target}] Connect() starting connection"),
            target,
        );
        assert_eq!(state, SteamConnectionState::Connecting);
        state = connection_state_after_line(
            state,
            &format!("[2026-07-17 23:08:42] [Logged On, 4, 23] [U:1:{target}] RecvMsgClientLogOnResponse() : processing complete"),
            target,
        );
        assert_eq!(state, SteamConnectionState::Online);
        state = connection_state_after_line(
            state,
            &format!("[2026-07-17 23:08:59] [Logged On, 4, 23] [U:1:{target}] RecvMsgClientLoggedOff('Service Unavailable')"),
            target,
        );
        assert_eq!(state, SteamConnectionState::Offline);
        state = connection_state_after_line(
            state,
            &format!("[2026-07-17 23:09:00] [Logged On, 4, 23] [U:1:{target}] LogOff()"),
            target,
        );
        assert_eq!(state, SteamConnectionState::Offline);
        state = connection_state_after_line(
            state,
            &format!("[2026-07-17 23:09:01] [Logged Off, 4, 0] [U:1:{target}] StartAutoReconnect() will start in 30 seconds"),
            target,
        );
        assert_eq!(state, SteamConnectionState::Connecting);
    }

    #[test]
    fn next_connection_monitor_ignores_history_and_buffers_partial_lines() {
        let root = std::env::temp_dir().join(format!(
            "nea-steam-connection-monitor-{}",
            uuid::Uuid::new_v4()
        ));
        let logs = root.join("logs");
        fs::create_dir_all(&logs).unwrap();
        let path = logs.join("connection_log.txt");
        let target = 1_217_010_099;
        let steam_id = (76_561_197_960_265_728u64 + target as u64).to_string();
        fs::write(
            &path,
            format!(
                "[2026-07-17 22:00:00] [Logged On, 4, 23] [U:1:{target}] processing complete\n"
            ),
        )
        .unwrap();
        let installation = AppInstallation {
            executable: root.join("steam.exe"),
            data_dir: root.clone(),
        };
        let mut monitor = SteamAdapter::monitor_next_connection(&installation, &steam_id).unwrap();
        assert_eq!(monitor.poll(), SteamConnectionState::Unknown);

        let mut file = fs::OpenOptions::new().append(true).open(&path).unwrap();
        write!(
            file,
            "[2026-07-17 23:08:41] [Logging On, 4, 23] [U:1:{target}] credentials sent\n[2026-07-17 23:08:42] [Logged On, 4, 23] [U:1:{target}] process"
        )
        .unwrap();
        file.flush().unwrap();
        assert_eq!(monitor.poll(), SteamConnectionState::Connecting);
        writeln!(file, "ing complete").unwrap();
        file.flush().unwrap();
        assert_eq!(monitor.poll(), SteamConnectionState::Online);

        fs::write(
            &path,
            format!(
                "[2026-07-17 23:09:01] [Logged Off, 4, 0] [U:1:{target}] StartAutoReconnect()\n"
            ),
        )
        .unwrap();
        assert_eq!(monitor.poll(), SteamConnectionState::Connecting);
        drop(file);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn connection_monitor_keeps_a_partial_line_across_log_rotation() {
        let root = std::env::temp_dir().join(format!(
            "nea-steam-connection-rotation-{}",
            uuid::Uuid::new_v4()
        ));
        let logs = root.join("logs");
        fs::create_dir_all(&logs).unwrap();
        let path = logs.join("connection_log.txt");
        fs::write(&path, b"history\n").unwrap();
        let target = 1_217_010_099;
        let steam_id = (76_561_197_960_265_728u64 + target as u64).to_string();
        let installation = AppInstallation {
            executable: root.join("steam.exe"),
            data_dir: root.clone(),
        };
        let mut monitor = SteamAdapter::monitor_next_connection(&installation, &steam_id).unwrap();
        let partial = format!("[2026-07-17 23:08:42] [Logged On, 4, 23] [U:1:{target}] process");
        fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap()
            .write_all(partial.as_bytes())
            .unwrap();
        assert_eq!(monitor.poll(), SteamConnectionState::Unknown);
        let previous = logs.join("connection_log.previous.txt");
        fs::rename(&path, &previous).unwrap();
        fs::OpenOptions::new()
            .append(true)
            .open(&previous)
            .unwrap()
            .write_all(b"ing complete\n")
            .unwrap();
        fs::write(&path, b"").unwrap();
        assert_eq!(monitor.poll(), SteamConnectionState::Online);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn prepares_only_the_target_account_for_online_login() {
        let root =
            std::env::temp_dir().join(format!("nea-steam-online-login-{}", uuid::Uuid::new_v4()));
        let config = root.join("config");
        fs::create_dir_all(&config).unwrap();
        fs::write(
            config.join("loginusers.vdf"),
            "\"users\"\n{\n\"76561198000000001\" { \"AccountName\" \"target\" \"WantsOfflineMode\" \"1\" \"SkipOfflineModeWarning\" \"0\" }\n\"76561198000000002\" { \"AccountName\" \"other\" \"WantsOfflineMode\" \"1\" \"SkipOfflineModeWarning\" \"0\" }\n}",
        )
        .unwrap();
        let installation = SteamInstallation {
            install_dir: root.to_string_lossy().to_string(),
            executable: root.join("steam.exe").to_string_lossy().to_string(),
            valid: true,
        };
        assert!(
            SteamAdapter::prepare_account_for_online_login(&installation, "76561198000000001")
                .unwrap()
        );
        assert!(!SteamAdapter::prepare_account_for_online_login(
            &installation,
            "76561198000000001"
        )
        .unwrap());
        let parsed =
            parse_vdf(&fs::read_to_string(config.join("loginusers.vdf")).unwrap()).unwrap();
        let accounts = users(&parsed).unwrap();
        let VdfValue::Object(target) = accounts.get("76561198000000001").unwrap() else {
            panic!()
        };
        let VdfValue::Object(other) = accounts.get("76561198000000002").unwrap() else {
            panic!()
        };
        assert_eq!(text(target, "WantsOfflineMode"), Some("0"));
        assert_eq!(text(target, "SkipOfflineModeWarning"), Some("1"));
        assert_eq!(text(other, "WantsOfflineMode"), Some("1"));
        assert_eq!(text(other, "SkipOfflineModeWarning"), Some("0"));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn suppresses_only_managed_accounts_from_the_native_switcher() {
        let root =
            std::env::temp_dir().join(format!("nea-steam-native-list-{}", uuid::Uuid::new_v4()));
        let config = root.join("config");
        fs::create_dir_all(&config).unwrap();
        fs::write(
            config.join("loginusers.vdf"),
            "\"users\"\n{\n\"76561198000000001\" { \"AccountName\" \"managed\" \"RememberPassword\" \"1\" \"AllowAutoLogin\" \"1\" \"MostRecent\" \"1\" \"Timestamp\" \"999\" }\n\"76561198000000002\" { \"AccountName\" \"native\" \"RememberPassword\" \"1\" \"AllowAutoLogin\" \"1\" \"MostRecent\" \"0\" \"Timestamp\" \"888\" }\n\"76561198000000003\" { \"AccountName\" \"native2\" \"RememberPassword\" \"1\" \"AllowAutoLogin\" \"1\" \"MostRecent\" \"0\" \"Timestamp\" \"777\" }\n}",
        )
        .unwrap();
        let installation = SteamInstallation {
            install_dir: root.to_string_lossy().to_string(),
            executable: root.join("steam.exe").to_string_lossy().to_string(),
            valid: true,
        };

        assert_eq!(
            SteamAdapter::suppress_accounts_from_native_switcher(
                &installation,
                &HashSet::from(["76561198000000001".to_string()]),
            )
            .unwrap(),
            1
        );
        let parsed =
            parse_vdf(&fs::read_to_string(config.join("loginusers.vdf")).unwrap()).unwrap();
        let accounts = users(&parsed).unwrap();
        let VdfValue::Object(managed) = accounts.get("76561198000000001").unwrap() else {
            panic!()
        };
        let VdfValue::Object(native) = accounts.get("76561198000000002").unwrap() else {
            panic!()
        };
        for key in [
            "RememberPassword",
            "AllowAutoLogin",
            "MostRecent",
            "Timestamp",
        ] {
            assert_eq!(text(managed, key), Some("0"));
        }
        assert_eq!(text(native, "RememberPassword"), Some("1"));
        assert_eq!(text(native, "Timestamp"), Some("888"));

        let handles = ["76561198000000002", "76561198000000003"].map(|steam_id| {
            let installation = installation.clone();
            thread::spawn(move || {
                SteamAdapter::suppress_accounts_from_native_switcher(
                    &installation,
                    &HashSet::from([steam_id.to_string()]),
                )
                .unwrap();
            })
        });
        for handle in handles {
            handle.join().unwrap();
        }
        let parsed =
            parse_vdf(&fs::read_to_string(config.join("loginusers.vdf")).unwrap()).unwrap();
        for steam_id in ["76561198000000002", "76561198000000003"] {
            let VdfValue::Object(account) = users(&parsed).unwrap().get(steam_id).unwrap() else {
                panic!()
            };
            assert_eq!(text(account, "RememberPassword"), Some("0"));
            assert_eq!(text(account, "Timestamp"), Some("0"));
        }
        let _ = fs::remove_dir_all(root);
    }
}
