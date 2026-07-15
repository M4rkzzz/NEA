use crate::adapters::{AccountIdentity, AppAdapter, AppInstallation};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, HashSet},
    fs,
    path::{Path, PathBuf},
    process::Command,
    sync::Mutex,
    thread,
    time::Duration,
};
use sysinfo::{ProcessRefreshKind, Signal, System, UpdateKind};
use winreg::{enums::*, RegKey};

static LOGINUSERS_WRITE_LOCK: Mutex<()> = Mutex::new(());

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
    pub web_sessions: Vec<SteamWebSession>,
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
    pub fn account_id32(steam_id64: &str) -> Option<u32> {
        let steam_id = steam_id64.parse::<u64>().ok()?;
        (steam_id64.len() == 17).then_some((steam_id & u32::MAX as u64) as u32)
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

    pub fn is_account_logged_in(steam_id64: &str) -> bool {
        Self::client_is_running() && Self::is_account_active(steam_id64)
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

    pub fn keep_account_out_of_native_switcher(
        installation: SteamInstallation,
        steam_id: String,
    ) -> Result<(), String> {
        let steam_ids = HashSet::from([steam_id.clone()]);
        let immediate_result = if Self::processes_are_running() {
            Ok(())
        } else {
            Self::suppress_accounts_from_native_switcher(&installation, &steam_ids).map(|_| ())
        };
        thread::spawn(move || {
            let mut system = process_system();
            loop {
                while steam_process_running(&system) {
                    thread::sleep(Duration::from_secs(2));
                    refresh_processes(&mut system);
                }
                // Steam may flush loginusers.vdf immediately after its last process exits,
                // or briefly have no process while another NEA switch is starting.
                thread::sleep(Duration::from_millis(800));
                refresh_processes(&mut system);
                if steam_process_running(&system) {
                    continue;
                }
                let _ = Self::suppress_accounts_from_native_switcher(&installation, &steam_ids);
                break;
            }
        });
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
        if !steam_process_running(&system) {
            return Ok(());
        }

        let _ = Command::new(&installation.executable)
            .current_dir(&installation.data_dir)
            .arg("-shutdown")
            .spawn();
        for _ in 0..20 {
            thread::sleep(Duration::from_millis(500));
            refresh_processes(&mut system);
            if !steam_process_running(&system) {
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
            if !steam_process_running(&system) {
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
        if steam_process_running(&system) {
            Err("Steam 进程无法完全退出，已中止切号以防登录状态被覆盖".to_string())
        } else {
            Ok(())
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
