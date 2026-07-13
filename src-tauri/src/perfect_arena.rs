use crate::steam::{SteamAccount, SteamWorkspace};
use serde::Serialize;
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    thread,
    time::Duration,
};
use sysinfo::{ProcessRefreshKind, Signal, System, UpdateKind};
use winreg::{enums::*, RegKey};

const EXECUTABLE_NAME: &str = "完美世界竞技平台.exe";

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PerfectArenaInstallation {
    pub install_dir: String,
    pub executable: String,
    pub valid: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PerfectArenaWorkspace {
    pub installation: Option<PerfectArenaInstallation>,
    pub accounts: Vec<SteamAccount>,
    pub current_account_id: Option<String>,
    pub running: bool,
}

fn process_system() -> System {
    let mut system = System::new();
    system.refresh_processes_specifics(
        ProcessRefreshKind::new()
            .with_exe(UpdateKind::Always)
            .with_cmd(UpdateKind::Always),
    );
    system
}

fn candidate_from_registry(root: &RegKey, flags: u32) -> Option<PathBuf> {
    let uninstall = root
        .open_subkey_with_flags(
            "Software\\Microsoft\\Windows\\CurrentVersion\\Uninstall",
            flags,
        )
        .ok()?;
    for name in uninstall.enum_keys().flatten() {
        let Ok(key) = uninstall.open_subkey_with_flags(name, flags) else {
            continue;
        };
        let display_name = key
            .get_value::<String, _>("DisplayName")
            .unwrap_or_default();
        if !display_name.contains("完美世界竞技平台") {
            continue;
        }
        if let Ok(directory) = key.get_value::<String, _>("InstallLocation") {
            let executable = PathBuf::from(directory).join(EXECUTABLE_NAME);
            if executable.is_file() {
                return Some(executable);
            }
        }
        if let Ok(icon) = key.get_value::<String, _>("DisplayIcon") {
            let executable = PathBuf::from(icon.trim_matches('"'));
            if executable.is_file() {
                return Some(executable);
            }
        }
    }
    None
}

pub fn discover_installation() -> Result<PerfectArenaInstallation, String> {
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let hklm = RegKey::predef(HKEY_LOCAL_MACHINE);
    let mut candidates = Vec::new();
    if let Some(path) = candidate_from_registry(&hkcu, KEY_READ) {
        candidates.push(path);
    }
    if let Some(path) = candidate_from_registry(&hklm, KEY_READ | KEY_WOW64_32KEY) {
        candidates.push(path);
    }
    if let Some(program_files) = std::env::var_os("ProgramFiles(x86)") {
        candidates.push(
            PathBuf::from(program_files)
                .join("perfectworldarena")
                .join(EXECUTABLE_NAME),
        );
    }
    let executable = candidates
        .into_iter()
        .find(|path| path.is_file())
        .ok_or_else(|| "未找到完美世界竞技平台，请先安装客户端".to_string())?;
    let install_dir = executable
        .parent()
        .ok_or_else(|| "完美世界竞技平台安装目录无效".to_string())?;
    Ok(PerfectArenaInstallation {
        install_dir: install_dir.to_string_lossy().to_string(),
        executable: executable.to_string_lossy().to_string(),
        valid: true,
    })
}

fn is_platform_process(
    process: &sysinfo::Process,
    installation: &PerfectArenaInstallation,
) -> bool {
    process
        .exe()
        .is_some_and(|path| path.starts_with(Path::new(&installation.install_dir)))
        || process.name().eq_ignore_ascii_case(EXECUTABLE_NAME)
        || process.name().eq_ignore_ascii_case("完美世界竞技平台")
}

pub fn is_running(installation: &PerfectArenaInstallation) -> bool {
    process_system()
        .processes()
        .values()
        .any(|process| is_platform_process(process, installation))
}

pub fn stop(installation: &PerfectArenaInstallation) -> Result<(), String> {
    let mut system = process_system();
    for process in system.processes().values() {
        if is_platform_process(process, installation) {
            let _ = process
                .kill_with(Signal::Term)
                .unwrap_or_else(|| process.kill());
        }
    }
    for _ in 0..20 {
        thread::sleep(Duration::from_millis(250));
        system.refresh_processes_specifics(ProcessRefreshKind::new().with_exe(UpdateKind::Always));
        if !system
            .processes()
            .values()
            .any(|process| is_platform_process(process, installation))
        {
            return Ok(());
        }
    }
    Err("完美世界竞技平台无法完全退出，已中止切号".to_string())
}

pub fn start(installation: &PerfectArenaInstallation) -> Result<(), String> {
    Command::new(&installation.executable)
        .current_dir(&installation.install_dir)
        .spawn()
        .map(|_| ())
        .map_err(|error| format!("启动完美世界竞技平台失败: {}", error))
}

pub fn ensure_games_stopped() -> Result<(), String> {
    let system = process_system();
    if system.processes().values().any(|process| {
        ["cs2.exe", "csgo.exe", "dota2.exe"]
            .iter()
            .any(|name| process.name().eq_ignore_ascii_case(name))
    }) {
        return Err("检测到游戏正在运行，请退出游戏后再切换账号".to_string());
    }
    Ok(())
}

fn current_account_from_database() -> Option<String> {
    let database_dir = std::env::var_os("APPDATA")
        .map(PathBuf::from)?
        .join("Wmpvp")
        .join("db");
    let mut latest = None;
    for entry in fs::read_dir(database_dir).ok()?.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        let Some(steam_id) = steam_id_from_database_name(&name) else {
            continue;
        };
        let Ok(modified) = entry.metadata().and_then(|metadata| metadata.modified()) else {
            continue;
        };
        if latest
            .as_ref()
            .is_none_or(|(_, latest_modified)| modified > *latest_modified)
        {
            latest = Some((steam_id, modified));
        }
    }
    latest.map(|(steam_id, _)| steam_id)
}

fn steam_id_from_database_name(name: &str) -> Option<String> {
    let steam_id = name.split('.').next()?;
    (steam_id.len() == 17 && steam_id.chars().all(|character| character.is_ascii_digit()))
        .then(|| steam_id.to_string())
}

pub fn workspace(steam: &SteamWorkspace) -> PerfectArenaWorkspace {
    let installation = discover_installation().ok();
    let running = installation.as_ref().is_some_and(is_running);
    let current_account_id = running
        .then(current_account_from_database)
        .flatten()
        .filter(|id| steam.accounts.iter().any(|account| &account.id == id));
    PerfectArenaWorkspace {
        installation,
        accounts: steam.accounts.clone(),
        current_account_id,
        running,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_account_scoped_database_names() {
        assert_eq!(
            steam_id_from_database_name("76561199123456789.IPC.db-wal").as_deref(),
            Some("76561199123456789")
        );
        assert!(steam_id_from_database_name("shared.IPC.db").is_none());
        assert!(steam_id_from_database_name("7656119912345678.IPC.db").is_none());
    }
}
