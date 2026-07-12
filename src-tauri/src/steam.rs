use crate::adapters::{AccountIdentity, AppAdapter, AppInstallation};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    process::Command,
    thread,
    time::Duration,
};
use sysinfo::{ProcessRefreshKind, Signal, System, UpdateKind};
use winreg::{enums::*, RegKey};

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

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct SteamWorkspace {
    pub installation: Option<SteamInstallation>,
    pub accounts: Vec<SteamAccount>,
    pub current_account_id: Option<String>,
    #[serde(default)]
    pub include_userdata: bool,
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

fn write_object(output: &mut String, object: &BTreeMap<String, VdfValue>, depth: usize) {
    let indent = "\t".repeat(depth);
    for (key, value) in object {
        match value {
            VdfValue::Text(text) => {
                output.push_str(&format!(
                    "{}\"{}\"\t\t\"{}\"\n",
                    indent,
                    escape_vdf(key),
                    escape_vdf(text)
                ));
            }
            VdfValue::Object(child) => {
                output.push_str(&format!(
                    "{}\"{}\"\n{}{{\n",
                    indent,
                    escape_vdf(key),
                    indent
                ));
                write_object(output, child, depth + 1);
                output.push_str(&format!("{}}}\n", indent));
            }
        }
    }
}

fn serialize_vdf(object: &BTreeMap<String, VdfValue>) -> String {
    let mut output = String::new();
    write_object(&mut output, object, 0);
    output
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

fn set_text(object: &mut BTreeMap<String, VdfValue>, key: &str, value: &str) {
    if let Some(existing) = object
        .keys()
        .find(|name| name.eq_ignore_ascii_case(key))
        .cloned()
    {
        object.insert(existing, VdfValue::Text(value.to_string()));
    } else {
        object.insert(key.to_string(), VdfValue::Text(value.to_string()));
    }
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

    pub fn activate_account(
        installation: &SteamInstallation,
        steam_id: &str,
    ) -> Result<String, String> {
        let path = PathBuf::from(&installation.install_dir)
            .join("config")
            .join("loginusers.vdf");
        let raw = fs::read_to_string(&path)
            .map_err(|error| format!("读取 Steam 登录状态失败: {}", error))?;
        let mut root = parse_vdf(&raw)?;
        let account_name = select_account(&mut root, steam_id)?;
        let staging = path.with_extension("vdf.nea-new");
        let backup = path.with_extension("vdf.nea-backup");
        fs::write(&staging, serialize_vdf(&root))
            .map_err(|error| format!("写入 Steam 登录状态失败: {}", error))?;
        if backup.exists() {
            fs::remove_file(&backup)
                .map_err(|error| format!("清理 Steam 临时备份失败: {}", error))?;
        }
        fs::rename(&path, &backup)
            .map_err(|error| format!("备份 Steam 登录状态失败: {}", error))?;
        if let Err(error) = fs::rename(&staging, &path) {
            let _ = fs::rename(&backup, &path);
            return Err(format!("替换 Steam 登录状态失败: {}", error));
        }
        let _ = fs::remove_file(backup);
        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        let (key, _) = hkcu
            .create_subkey("Software\\Valve\\Steam")
            .map_err(|error| error.to_string())?;
        key.set_value("AutoLoginUser", &account_name)
            .map_err(|error| format!("设置 Steam 自动登录账号失败: {}", error))?;
        Ok(account_name)
    }

    pub fn snapshot_login_state(
        installation: &SteamInstallation,
        destination: &Path,
    ) -> Result<(), String> {
        fs::create_dir_all(destination).map_err(|error| error.to_string())?;
        let source = PathBuf::from(&installation.install_dir)
            .join("config")
            .join("loginusers.vdf");
        fs::copy(source, destination.join("loginusers.vdf"))
            .map_err(|error| format!("备份 Steam 登录状态失败: {}", error))?;
        Ok(())
    }

    pub fn capture_userdata(
        installation: &SteamInstallation,
        steam_id: &str,
        destination: &Path,
    ) -> Result<(), String> {
        let source = PathBuf::from(&installation.install_dir)
            .join("userdata")
            .join(steam_id);
        if !source.is_dir() {
            return Ok(());
        }
        copy_dir(&source, destination)
    }
}

fn select_account(root: &mut BTreeMap<String, VdfValue>, steam_id: &str) -> Result<String, String> {
    let list = users_mut(root)?;
    let mut selected = None;
    for (id, value) in list.iter_mut() {
        let VdfValue::Object(account) = value else {
            continue;
        };
        let active = id == steam_id;
        set_text(account, "MostRecent", if active { "1" } else { "0" });
        let remembered = text(account, "RememberPassword") == Some("1");
        set_text(
            account,
            "AllowAutoLogin",
            if active && remembered { "1" } else { "0" },
        );
        if active {
            selected = text(account, "AccountName").map(str::to_string);
            set_text(account, "WantsOfflineMode", "0");
            set_text(account, "SkipOfflineModeWarning", "1");
        }
    }
    selected.ok_or_else(|| "目标 Steam 账号已不存在".to_string())
}

fn copy_dir(source: &Path, destination: &Path) -> Result<(), String> {
    fs::create_dir_all(destination).map_err(|error| error.to_string())?;
    for entry in fs::read_dir(source).map_err(|error| error.to_string())? {
        let entry = entry.map_err(|error| error.to_string())?;
        let target = destination.join(entry.file_name());
        if entry
            .file_type()
            .map_err(|error| error.to_string())?
            .is_dir()
        {
            copy_dir(&entry.path(), &target)?;
        } else {
            fs::copy(entry.path(), target).map_err(|error| error.to_string())?;
        }
    }
    Ok(())
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

fn refresh_processes(system: &mut System) {
    system.refresh_processes_specifics(ProcessRefreshKind::new());
}

impl SteamAdapter {
    pub fn start_with_login(
        installation: &AppInstallation,
        account_name: &str,
    ) -> Result<(), String> {
        Command::new(&installation.executable)
            .current_dir(&installation.data_dir)
            .arg("-login")
            .arg(account_name)
            .spawn()
            .map(|_| ())
            .map_err(|error| format!("启动 Steam 失败: {}", error))
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
        Ok(Self::read_accounts(&workspace)?
            .into_iter()
            .find(|account| account.most_recent)
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
        Ok(Self::read_accounts(&workspace)?
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
        process_system()
            .processes()
            .values()
            .any(|process| process.name().eq_ignore_ascii_case("steam.exe"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_updates_loginusers_vdf() {
        let input = "\"users\"\n{\n\"1\" { \"AccountName\" \"one\" \"PersonaName\" \"One\" \"MostRecent\" \"1\" \"RememberPassword\" \"1\" \"AllowAutoLogin\" \"1\" }\n\"2\" { \"AccountName\" \"two\" \"MostRecent\" \"0\" \"RememberPassword\" \"1\" \"AllowAutoLogin\" \"0\" }\n}";
        let mut root = parse_vdf(input).unwrap();
        assert_eq!(users(&root).unwrap().len(), 2);
        assert_eq!(select_account(&mut root, "2").unwrap(), "two");
        let list = users(&root).unwrap();
        let VdfValue::Object(first) = list.get("1").unwrap() else {
            panic!()
        };
        let VdfValue::Object(second) = list.get("2").unwrap() else {
            panic!()
        };
        assert_eq!(text(first, "MostRecent"), Some("0"));
        assert_eq!(text(first, "AllowAutoLogin"), Some("0"));
        assert_eq!(text(second, "MostRecent"), Some("1"));
        assert_eq!(text(second, "AllowAutoLogin"), Some("1"));
        let reparsed = parse_vdf(&serialize_vdf(&root)).unwrap();
        assert_eq!(users(&reparsed).unwrap().len(), 2);
    }

    #[test]
    fn rejects_unclosed_vdf() {
        assert!(parse_vdf("\"users\" { \"1\" {").is_err());
    }
}
