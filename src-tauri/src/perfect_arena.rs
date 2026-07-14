use crate::steam::{SteamAccount, SteamWorkspace};
use aes::{
    cipher::{block_padding::Pkcs7, BlockDecryptMut, KeyInit},
    Aes256,
};
use base64::{engine::general_purpose::STANDARD as BASE64_STANDARD, Engine as _};
use rusqlite::{Connection, OpenFlags};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::{HashMap, HashSet},
    fs,
    net::TcpStream,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    thread,
    time::{Duration, Instant, SystemTime},
};
use sysinfo::{ProcessRefreshKind, Signal, System, UpdateKind};
use winreg::{enums::*, RegKey};

const EXECUTABLE_NAME: &str = "完美世界竞技平台.exe";
const CACHE_KEY_PREFIX: &str = "G#r%*VCDYj6P5$mny0838MhH8d";
const PROFILE_BATCH_SIZE: usize = 50;
const SIGNATURE_SCRIPT: &str = r#"
const [addonPath, dllPath, randnum, timestamp, encodedBody] = process.argv.slice(1);
const addon = require(addonPath);
addon.sendJsonMessage(JSON.stringify({ msg_id: "INIT_DRIVE", dll_path: dllPath }));
const body = Buffer.from(encodedBody, "base64").toString("utf8");
const signature = addon.sendJsonMessage(JSON.stringify({
  msg_id: "SWAP_DATA",
  data: JSON.stringify({ randnum, ts: timestamp, data: body, version: 1 }),
}));
if (!/^[a-f0-9]{40}$/i.test(signature)) process.exit(2);
process.stdout.write(signature);
"#;

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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PerfectArenaProfile {
    pub steam_id: String,
    pub found: bool,
    pub nickname: Option<String>,
    pub avatar_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub avatar_source_url: Option<String>,
    pub score: Option<i64>,
    pub season: Option<String>,
    pub player_identity: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub high_risk: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reputation_requires_verification: Option<bool>,
    pub reputation_points: Option<i64>,
    pub reputation_level: Option<String>,
    pub updated_at: Option<String>,
}

#[derive(Debug)]
struct CacheRow {
    owner_steam_id: Option<String>,
    org: String,
    data: String,
    updated_at: u64,
}

type Aes256EcbDecryptor = ecb::Decryptor<Aes256>;

fn decrypt_cache_text(value: &str) -> Option<String> {
    let (salt, ciphertext) = value.split_once("$$")?;
    let key = format!("{}{}", CACHE_KEY_PREFIX, salt);
    if key.len() != 32 {
        return None;
    }
    let mut decoded = BASE64_STANDARD.decode(ciphertext).ok()?;
    let plaintext = Aes256EcbDecryptor::new_from_slice(key.as_bytes())
        .ok()?
        .decrypt_padded_mut::<Pkcs7>(&mut decoded)
        .ok()?;
    String::from_utf8(plaintext.to_vec()).ok()
}

fn json_id(value: &Value) -> Option<String> {
    match value {
        Value::String(value) => Some(value.clone()),
        Value::Number(value) => Some(value.to_string()),
        _ => None,
    }
}

fn json_i64(value: Option<&Value>) -> Option<i64> {
    value.and_then(|value| {
        value
            .as_i64()
            .or_else(|| value.as_u64().and_then(|value| i64::try_from(value).ok()))
            .or_else(|| value.as_str()?.parse().ok())
    })
}

fn json_bool(value: Option<&Value>) -> Option<bool> {
    value.and_then(|value| {
        value
            .as_bool()
            .or_else(|| value.as_i64().map(|value| value != 0))
            .or_else(|| {
                value
                    .as_str()
                    .map(|value| value == "1" || value.eq_ignore_ascii_case("true"))
            })
    })
}

fn empty_profile(steam_id: String) -> PerfectArenaProfile {
    PerfectArenaProfile {
        steam_id,
        found: false,
        nickname: None,
        avatar_url: None,
        avatar_source_url: None,
        score: None,
        season: None,
        player_identity: None,
        high_risk: None,
        reputation_requires_verification: None,
        reputation_points: None,
        reputation_level: None,
        updated_at: None,
    }
}

fn signature_from_output(output: &[u8]) -> Option<String> {
    String::from_utf8_lossy(output)
        .split_whitespace()
        .rev()
        .find(|value| value.len() == 40 && value.chars().all(|ch| ch.is_ascii_hexdigit()))
        .map(str::to_string)
}

fn generate_official_signature(
    installation: &PerfectArenaInstallation,
    body: &str,
    randnum: &str,
    timestamp: &str,
) -> Result<String, String> {
    let install_dir = PathBuf::from(&installation.install_dir);
    let addon = install_dir.join("plugin").join("gameaddon.node");
    let dll = install_dir.join("plugin").join("PvpAlive.dll");
    if !addon.is_file() || !dll.is_file() {
        return Err("完美平台签名组件不完整".to_string());
    }
    let mut child = Command::new(&installation.executable)
        .env("ELECTRON_RUN_AS_NODE", "1")
        .arg("-e")
        .arg(SIGNATURE_SCRIPT)
        .arg(addon)
        .arg(dll)
        .arg(randnum)
        .arg(timestamp)
        .arg(BASE64_STANDARD.encode(body))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| format!("启动完美平台签名组件失败: {error}"))?;
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if child
            .try_wait()
            .map_err(|error| format!("等待完美平台签名组件失败: {error}"))?
            .is_some()
        {
            break;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err("完美平台签名组件响应超时".to_string());
        }
        thread::sleep(Duration::from_millis(25));
    }
    let output = child
        .wait_with_output()
        .map_err(|error| format!("读取完美平台签名失败: {error}"))?;
    signature_from_output(&output.stdout).ok_or_else(|| "完美平台签名生成失败".to_string())
}

fn reputation_level(points: i64, low_priority: bool) -> String {
    if low_priority || points < 72 {
        "低优先"
    } else if points < 75 {
        "风险观察"
    } else if points < 100 {
        "良好"
    } else {
        "优秀"
    }
    .to_string()
}

fn apply_player_identity(profile: &mut PerfectArenaProfile, identity: Option<i64>) {
    if profile.high_risk.is_some() {
        return;
    }
    match identity {
        Some(-10) => {
            profile.high_risk = Some(true);
            profile.reputation_requires_verification = Some(true);
            profile.player_identity = None;
        }
        Some(0) => {
            profile.high_risk = Some(false);
            profile.player_identity = Some("新手".to_string());
        }
        Some(10) => {
            profile.high_risk = Some(false);
            profile.player_identity = Some("老兵".to_string());
        }
        Some(20) => {
            profile.high_risk = Some(false);
            profile.player_identity = Some("绿色".to_string());
        }
        _ => {}
    }
}

fn profiles_from_batch_response(
    value: &Value,
    updated_at: u64,
) -> Result<Vec<PerfectArenaProfile>, String> {
    if json_i64(value.get("code")) != Some(0) {
        return Err(value
            .get("msg")
            .and_then(Value::as_str)
            .unwrap_or("完美平台资料接口返回错误")
            .to_string());
    }
    let users = value
        .pointer("/data/users")
        .and_then(Value::as_array)
        .ok_or_else(|| "完美平台资料响应缺少账号列表".to_string())?;
    Ok(users
        .iter()
        .filter_map(|user| {
            let steam_id = user.get("steam_id").and_then(json_id)?;
            let mut profile = empty_profile(steam_id);
            profile.found = true;
            profile.nickname = user
                .get("nickname")
                .and_then(Value::as_str)
                .filter(|value| !value.trim().is_empty())
                .map(str::to_string);
            profile.avatar_url = user
                .get("avatar")
                .and_then(Value::as_str)
                .filter(|value| !value.trim().is_empty())
                .map(str::to_string);
            profile.score = json_i64(user.get("score"));
            profile.season = user
                .get("season")
                .and_then(Value::as_str)
                .filter(|value| !value.trim().is_empty())
                .map(str::to_string);
            apply_player_identity(&mut profile, json_i64(user.get("identity")));
            if profile.player_identity.is_none() && profile.high_risk != Some(true) {
                profile.player_identity = if json_bool(user.get("is_green")) == Some(true) {
                    Some("绿色".to_string())
                } else if profile.score.unwrap_or_default() > 0
                    || json_i64(user.get("pwLevel")).unwrap_or_default() > 1
                {
                    Some("老兵".to_string())
                } else {
                    Some("新手".to_string())
                };
            }
            let points = json_i64(user.get("reputationPoints")).filter(|points| *points > 0);
            if let Some(points) = points {
                profile.reputation_points = Some(points);
                profile.reputation_level = Some(reputation_level(
                    points,
                    json_bool(user.get("inLowPriority")).unwrap_or(false),
                ));
            }
            profile.updated_at = Some(updated_at.to_string());
            Some(profile)
        })
        .collect())
}

pub fn online_profiles(steam_ids: &[String]) -> Result<Vec<PerfectArenaProfile>, String> {
    let installation = discover_installation()?;
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(15))
        .build()
        .map_err(|error| error.to_string())?;
    let updated_at = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;
    let mut profiles = Vec::new();
    for (batch_index, batch) in steam_ids.chunks(PROFILE_BATCH_SIZE).enumerate() {
        let body = serde_json::json!({
            "steam_ids": batch,
            "with_ladder_info": 1,
            "with_green_info": 1,
            "with_perfect_power": 1,
        })
        .to_string();
        let timestamp = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
            .to_string();
        let randnum = (100_000
            + ((updated_at as usize + batch_index * 7_919 + std::process::id() as usize)
                % 900_000))
            .to_string();
        let signature = generate_official_signature(&installation, &body, &randnum, &timestamp)?;
        let response_text = client
            .post("https://pwa-account.wmpvp.com/user/getBySteamIds")
            .query(&[
                ("a", "20000"),
                ("r", randnum.as_str()),
                ("s", signature.as_str()),
                ("t", timestamp.as_str()),
            ])
            .header("Content-Type", "application/json")
            .header("Referer", "https://client.wmpvp.com")
            .body(body)
            .send()
            .and_then(|response| response.error_for_status())
            .map_err(|error| format!("批量获取完美账号资料失败: {error}"))?
            .text()
            .map_err(|error| format!("读取完美账号资料失败: {error}"))?;
        let response = serde_json::from_str::<Value>(&response_text)
            .map_err(|error| format!("解析完美账号资料失败: {error}"))?;
        profiles.extend(profiles_from_batch_response(&response, updated_at)?);
    }
    Ok(profiles)
}

fn read_cache_rows(database: &Path) -> Vec<CacheRow> {
    let owner_steam_id = database
        .file_name()
        .and_then(|name| name.to_str())
        .and_then(steam_id_from_database_name);
    let Ok(connection) = Connection::open_with_flags(
        database,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    ) else {
        return Vec::new();
    };
    let Ok(mut statement) = connection.prepare(
        "SELECT org, data, update_time FROM IPC_MEMORY_CACHE__prod \
         WHERE org IN (\
           'USER_GET_STEAM_INFOS_REQ',\
           'USER_MT_GET_USER_REPUTATION_INFO_REQ',\
           'CSGO_OVERVIEW_GET_CARD_INFO_REQ',\
           'CSGO_OVERVIEW_GET_SEASON_STATS_REQ',\
           'COMMON_GET_QUERY_MATCH_PLAYER_REQ'\
         )",
    ) else {
        return Vec::new();
    };
    statement
        .query_map([], |row| {
            let updated_at = row
                .get::<_, String>(2)
                .ok()
                .and_then(|value| value.parse().ok())
                .unwrap_or_default();
            Ok(CacheRow {
                owner_steam_id: owner_steam_id.clone(),
                org: row.get(0)?,
                data: row.get(1)?,
                updated_at,
            })
        })
        .into_iter()
        .flatten()
        .flatten()
        .collect()
}

fn touch_profile(profile: &mut PerfectArenaProfile, updated_at: u64) {
    let current = profile
        .updated_at
        .as_deref()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or_default();
    if updated_at > current {
        profile.updated_at = Some(updated_at.to_string());
    }
    profile.found = true;
}

fn profile_for<'a>(
    profiles: &'a mut HashMap<String, PerfectArenaProfile>,
    requested: &HashSet<String>,
    id: &str,
) -> Option<&'a mut PerfectArenaProfile> {
    let id = nearest_requested_id(requested, id)?;
    profiles.get_mut(&id)
}

fn nearest_requested_id(requested: &HashSet<String>, reported: &str) -> Option<String> {
    if requested.contains(reported) {
        return Some(reported.to_string());
    }
    let reported = reported.parse::<u64>().ok()?;
    let mut candidates = requested
        .iter()
        .filter_map(|id| {
            let numeric = id.parse::<u64>().ok()?;
            let distance = numeric.abs_diff(reported);
            (distance <= 16).then_some((distance, id.clone()))
        })
        .collect::<Vec<_>>();
    candidates.sort_by_key(|(distance, _)| *distance);
    match candidates.as_slice() {
        [(distance, id), ..]
            if candidates
                .get(1)
                .is_none_or(|(next_distance, _)| next_distance > distance) =>
        {
            Some(id.clone())
        }
        _ => None,
    }
}

pub fn cached_profiles(steam_ids: &[String]) -> Vec<PerfectArenaProfile> {
    let requested = steam_ids
        .iter()
        .filter(|id| id.len() == 17 && id.chars().all(|character| character.is_ascii_digit()))
        .cloned()
        .collect::<HashSet<_>>();
    let mut profiles = requested
        .iter()
        .map(|id| (id.clone(), empty_profile(id.clone())))
        .collect::<HashMap<_, _>>();
    if requested.is_empty() {
        return Vec::new();
    }

    let Some(database_dir) = std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .map(|path| path.join("Wmpvp").join("db"))
    else {
        return profiles.into_values().collect();
    };
    let mut rows = fs::read_dir(database_dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|entry| entry.file_name().to_string_lossy().ends_with(".IPC.db"))
        .flat_map(|entry| read_cache_rows(&entry.path()))
        .collect::<Vec<_>>();
    rows.sort_by_key(|row| std::cmp::Reverse(row.updated_at));

    for row in rows {
        let Some(data) = decrypt_cache_text(&row.data)
            .and_then(|data| serde_json::from_str::<Value>(&data).ok())
        else {
            continue;
        };
        match row.org.as_str() {
            "CSGO_OVERVIEW_GET_CARD_INFO_REQ" => {
                let Some(card) = data.pointer("/data/card_info").and_then(Value::as_object) else {
                    continue;
                };
                let Some(id) = card.get("steamId").and_then(json_id) else {
                    continue;
                };
                let Some(profile) = profile_for(&mut profiles, &requested, &id) else {
                    continue;
                };
                touch_profile(profile, row.updated_at);
                profile.nickname = card
                    .get("nickname")
                    .and_then(Value::as_str)
                    .filter(|value| !value.trim().is_empty())
                    .map(str::to_string);
                profile.avatar_url = card
                    .get("avatar")
                    .and_then(Value::as_str)
                    .filter(|value| !value.trim().is_empty())
                    .map(str::to_string);
                profile.score = json_i64(card.get("score")).or(profile.score);
                apply_player_identity(profile, json_i64(card.get("identity")));
                if profile.player_identity.is_none()
                    && profile.high_risk != Some(true)
                    && json_bool(card.get("is_green")) == Some(true)
                {
                    profile.player_identity = Some("绿色".to_string());
                }
            }
            "USER_GET_STEAM_INFOS_REQ" => {
                let Some(users) = data.pointer("/data/users").and_then(Value::as_array) else {
                    continue;
                };
                for user in users {
                    let Some(id) = user.get("steam_id").and_then(json_id) else {
                        continue;
                    };
                    let Some(profile) = profile_for(&mut profiles, &requested, &id) else {
                        continue;
                    };
                    touch_profile(profile, row.updated_at);
                    if profile.nickname.is_none() {
                        profile.nickname = user
                            .get("nickname")
                            .and_then(Value::as_str)
                            .map(str::to_string);
                    }
                    if profile.avatar_url.is_none() {
                        profile.avatar_url = user
                            .get("avatar")
                            .and_then(Value::as_str)
                            .map(str::to_string);
                    }
                    if profile.score.is_none() {
                        profile.score = json_i64(user.get("score"));
                    }
                    if profile.season.is_none() {
                        profile.season = user
                            .get("season")
                            .and_then(Value::as_str)
                            .map(str::to_string);
                    }
                    if json_bool(user.get("is_green")) == Some(true) {
                        profile.player_identity = Some("绿色".to_string());
                    }
                    apply_player_identity(profile, json_i64(user.get("identity")));
                }
            }
            "CSGO_OVERVIEW_GET_SEASON_STATS_REQ" => {
                let Some(ladder) = data.pointer("/data/ladder").and_then(Value::as_object) else {
                    continue;
                };
                let Some(id) = ladder.get("steam_id").and_then(json_id) else {
                    continue;
                };
                let Some(profile) = profile_for(&mut profiles, &requested, &id) else {
                    continue;
                };
                touch_profile(profile, row.updated_at);
                if profile.score.is_none() {
                    profile.score = json_i64(ladder.get("score"));
                }
                if profile.season.is_none() {
                    profile.season = ladder
                        .get("season")
                        .and_then(Value::as_str)
                        .map(str::to_string);
                }
            }
            "USER_MT_GET_USER_REPUTATION_INFO_REQ" => {
                let Some(info) = data.get("data").and_then(Value::as_object) else {
                    if json_i64(data.get("code")) == Some(0)
                        && data.get("data").is_some_and(Value::is_null)
                        && row
                            .owner_steam_id
                            .as_ref()
                            .is_some_and(|id| requested.contains(id))
                    {
                        let id = row.owner_steam_id.as_deref().unwrap_or_default();
                        if let Some(profile) = profile_for(&mut profiles, &requested, id) {
                            touch_profile(profile, row.updated_at);
                            if profile.reputation_points.is_none()
                                && profile.reputation_requires_verification.is_none()
                            {
                                profile.high_risk = Some(true);
                                profile.reputation_requires_verification = Some(true);
                            }
                        }
                    }
                    continue;
                };
                let Some(id) = info.get("uid").and_then(json_id) else {
                    continue;
                };
                let Some(id) = nearest_requested_id(&requested, &id) else {
                    continue;
                };
                let Some(profile) = profile_for(&mut profiles, &requested, &id) else {
                    continue;
                };
                touch_profile(profile, row.updated_at);
                if profile.reputation_points.is_none() {
                    profile.reputation_points = json_i64(info.get("totalScore"));
                    let low_priority = json_bool(info.get("lowPriority")).unwrap_or(false);
                    profile.reputation_level = profile
                        .reputation_points
                        .map(|points| reputation_level(points, low_priority));
                    if profile.reputation_points.is_some() {
                        profile.reputation_requires_verification = Some(false);
                        profile.high_risk.get_or_insert(false);
                    }
                }
            }
            "COMMON_GET_QUERY_MATCH_PLAYER_REQ" => {
                let Some(players) = data
                    .pointer("/data/players_info")
                    .and_then(Value::as_object)
                else {
                    continue;
                };
                for (key, player) in players {
                    let id = player
                        .get("steam_id")
                        .and_then(json_id)
                        .or_else(|| player.get("uid").and_then(json_id))
                        .unwrap_or_else(|| key.clone());
                    let Some(profile) = profile_for(&mut profiles, &requested, &id) else {
                        continue;
                    };
                    touch_profile(profile, row.updated_at);
                    apply_player_identity(profile, json_i64(player.get("identity")));
                    if profile.player_identity.is_none()
                        && profile.high_risk != Some(true)
                        && json_bool(player.get("is_green")) == Some(true)
                    {
                        profile.player_identity = Some("绿色".to_string());
                    }
                    let points =
                        json_i64(player.get("reputationPoints")).filter(|points| *points > 0);
                    if let Some(points) = points {
                        profile.reputation_points = Some(points);
                        profile.reputation_level = Some(reputation_level(
                            points,
                            json_bool(player.get("inLowPriority")).unwrap_or(false),
                        ));
                    }
                }
            }
            _ => {}
        }
    }

    steam_ids
        .iter()
        .filter_map(|id| profiles.remove(id))
        .collect()
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

fn stop(installation: &PerfectArenaInstallation) -> Result<(), String> {
    let mut system = process_system();
    for process in system.processes().values() {
        if is_platform_process(process, installation) {
            let _ = process
                .kill_with(Signal::Term)
                .unwrap_or_else(|| process.kill());
        }
    }
    for _ in 0..24 {
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

fn start(installation: &PerfectArenaInstallation) -> Result<(), String> {
    Command::new(&installation.executable)
        .current_dir(&installation.install_dir)
        .spawn()
        .map(|_| ())
        .map_err(|error| format!("启动完美世界竞技平台失败: {}", error))
}

fn wait_for_oauth_callback(timeout: Duration) -> Result<(), String> {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if TcpStream::connect_timeout(
            &"127.0.0.1:50000"
                .parse()
                .map_err(|error| format!("完美回调地址无效: {}", error))?,
            Duration::from_millis(300),
        )
        .is_ok()
        {
            return Ok(());
        }
        thread::sleep(Duration::from_millis(500));
    }
    Err("完美世界竞技平台登录回调服务未就绪".to_string())
}

pub fn prepare_oauth_login(installation: &PerfectArenaInstallation) -> Result<SystemTime, String> {
    ensure_games_stopped()?;
    stop(installation)?;
    let started_at = SystemTime::now();
    start(installation)?;
    wait_for_oauth_callback(Duration::from_secs(30))?;
    Ok(started_at)
}

pub fn stop_for_share_transfer() -> Result<(), String> {
    if let Ok(installation) = discover_installation() {
        ensure_games_stopped()?;
        stop(&installation)?;
    }
    Ok(())
}

pub fn account_database_files(steam_id: &str) -> Vec<PathBuf> {
    let Some(database_dir) = std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .map(|path| path.join("Wmpvp").join("db"))
    else {
        return Vec::new();
    };
    fs::read_dir(database_dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().to_string();
            (steam_id_from_database_name(&name).as_deref() == Some(steam_id)
                && entry.file_type().ok().is_some_and(|kind| kind.is_file()))
            .then_some(entry.path())
        })
        .collect()
}

pub fn account_database_dir() -> Option<PathBuf> {
    std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .map(|path| path.join("Wmpvp").join("db"))
}

fn target_database_updated_after(steam_id: &str, after: SystemTime) -> bool {
    let Some(database_dir) = std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .map(|path| path.join("Wmpvp").join("db"))
    else {
        return false;
    };
    fs::read_dir(database_dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|entry| {
            steam_id_from_database_name(&entry.file_name().to_string_lossy()).as_deref()
                == Some(steam_id)
        })
        .any(|entry| {
            entry
                .metadata()
                .and_then(|metadata| metadata.modified())
                .is_ok_and(|modified| modified >= after)
        })
}

pub fn wait_for_oauth_login(
    steam_id: &str,
    started_at: SystemTime,
    timeout: Duration,
    cancelled: Arc<AtomicBool>,
    loop_detected: Arc<AtomicBool>,
) -> Result<(), String> {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if cancelled.load(Ordering::SeqCst) {
            return Err("已取消完美账号切换".to_string());
        }
        if target_database_updated_after(steam_id, started_at) {
            return Ok(());
        }
        if loop_detected.load(Ordering::SeqCst) {
            return Err(
                "完美平台未接受该账号的 Steam 授权，已停止重复授权；该账号可能需要先在完美客户端完成账号确认或手机号验证"
                    .to_string(),
            );
        }
        thread::sleep(Duration::from_millis(500));
    }
    Err(
        "Steam 授权已提交，但完美账号登录未完成；请检查完美窗口是否要求手机号验证或账号确认"
            .to_string(),
    )
}

fn current_account_from_database() -> Option<String> {
    let database_dir = std::env::var_os("APPDATA")
        .map(PathBuf::from)?
        .join("Wmpvp")
        .join("db");
    let mut latest = None;
    for entry in fs::read_dir(database_dir).ok()?.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if !name.ends_with(".IPC.db-wal") {
            continue;
        }
        let Some(steam_id) = steam_id_from_database_name(&name) else {
            continue;
        };
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        if metadata.len() == 0 {
            continue;
        }
        let Ok(modified) = metadata.modified() else {
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
        .filter(|id| {
            steam.accounts.iter().any(|account| &account.id == id)
                || steam
                    .web_sessions
                    .iter()
                    .any(|session| session.steam_id.as_deref() == Some(id))
        });
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
    fn decrypts_official_cache_payload() {
        let encrypted = "123456$$0YiIRP/50YjHN4zRAUE+LOGkhxYLmkM05FYj86glPBA=";
        assert_eq!(
            decrypt_cache_text(encrypted).as_deref(),
            Some(r#"{"code":0,"data":{"users":[]}}"#)
        );
        assert!(decrypt_cache_text("invalid").is_none());
    }

    #[test]
    fn repairs_unique_rounded_steam64_ids_only() {
        let requested = ["76561199198704913".to_string()]
            .into_iter()
            .collect::<HashSet<_>>();
        assert_eq!(
            nearest_requested_id(&requested, "76561199198704910").as_deref(),
            Some("76561199198704913")
        );

        let rounded_by_json = ["76561199258467880".to_string()]
            .into_iter()
            .collect::<HashSet<_>>();
        assert_eq!(
            nearest_requested_id(&rounded_by_json, "76561199258467870").as_deref(),
            Some("76561199258467880")
        );

        let ambiguous = [
            "76561199198704909".to_string(),
            "76561199198704911".to_string(),
        ]
        .into_iter()
        .collect::<HashSet<_>>();
        assert!(nearest_requested_id(&ambiguous, "76561199198704910").is_none());
    }

    #[test]
    fn recognizes_account_scoped_database_names() {
        assert_eq!(
            steam_id_from_database_name("76561199123456789.IPC.db-wal").as_deref(),
            Some("76561199123456789")
        );
        assert!(steam_id_from_database_name("shared.IPC.db").is_none());
        assert!(steam_id_from_database_name("7656119912345678.IPC.db").is_none());
    }

    #[test]
    fn oauth_wait_stops_immediately_when_cancelled() {
        let cancelled = Arc::new(AtomicBool::new(true));
        let started = Instant::now();
        let result = wait_for_oauth_login(
            "76561199123456789",
            SystemTime::now(),
            Duration::from_secs(120),
            cancelled,
            Arc::new(AtomicBool::new(false)),
        );
        assert_eq!(result.unwrap_err(), "已取消完美账号切换");
        assert!(started.elapsed() < Duration::from_millis(100));
    }

    #[test]
    fn parses_batch_profiles_without_treating_hidden_reputation_as_zero() {
        let response = serde_json::json!({
            "code": 0,
            "msg": "成功",
            "data": { "users": [
                {
                    "steam_id": "76561199198704913",
                    "nickname": "Player A",
                    "avatar": "https://example.invalid/avatar.png",
                    "score": 1461,
                    "pwLevel": 6,
                    "is_green": 0,
                    "reputationPoints": 0
                },
                {
                    "steam_id": "76561199200211616",
                    "nickname": "Player B",
                    "score": 0,
                    "pwLevel": 1,
                    "is_green": 1,
                    "reputationPoints": 100,
                    "inLowPriority": false
                }
            ]}
        });
        let profiles = profiles_from_batch_response(&response, 123).unwrap();
        assert_eq!(profiles[0].player_identity.as_deref(), Some("老兵"));
        assert_eq!(profiles[0].reputation_points, None);
        assert_eq!(profiles[1].player_identity.as_deref(), Some("绿色"));
        assert_eq!(profiles[1].reputation_level.as_deref(), Some("优秀"));
    }

    #[test]
    fn detects_high_risk_identity_without_replacing_player_category() {
        let response = serde_json::json!({
            "code": 0,
            "data": { "users": [{
                "steam_id": "76561199259110336",
                "nickname": "Risk Player",
                "score": 1200,
                "identity": -10,
                "reputationPoints": 0
            }]}
        });
        let profiles = profiles_from_batch_response(&response, 123).unwrap();
        assert_eq!(profiles[0].high_risk, Some(true));
        assert_eq!(profiles[0].reputation_requires_verification, Some(true));
        assert_eq!(profiles[0].player_identity, None);
        assert_eq!(profiles[0].reputation_level, None);
    }

    #[test]
    fn extracts_only_valid_official_signatures() {
        assert_eq!(
            signature_from_output(b"startup output\n0123456789abcdef0123456789abcdef01234567")
                .as_deref(),
            Some("0123456789abcdef0123456789abcdef01234567")
        );
        assert!(signature_from_output(b"not-a-signature").is_none());
    }

    #[test]
    #[ignore = "requires a locally installed Perfect World Arena client and network"]
    fn queries_multiple_profiles_with_official_client_signature() {
        let ids = std::env::var("NEA_PERFECT_TEST_IDS")
            .unwrap()
            .split(',')
            .map(str::to_string)
            .collect::<Vec<_>>();
        let profiles = online_profiles(&ids).unwrap();
        assert_eq!(profiles.len(), ids.len());
        assert!(profiles.iter().all(|profile| profile.found));
    }
}
