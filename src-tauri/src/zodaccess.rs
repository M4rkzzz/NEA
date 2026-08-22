use chrono::Local;
use reqwest::{
    blocking::{Client, Response},
    header::{ACCEPT, CONTENT_TYPE, COOKIE, LOCATION, REFERER},
    redirect::Policy,
    StatusCode,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{collections::HashSet, io::Read, time::Duration};

pub const ORIGIN: &str = "https://kp.zodaccyes.com";
pub const USER_URL: &str = "https://kp.zodaccyes.com/user";
const CHECKIN_URL: &str = "https://kp.zodaccyes.com/user/checkin";
const MAX_DASHBOARD_BYTES: u64 = 256 * 1024;
const MAX_CHECKIN_BYTES: u64 = 32 * 1024;
const MAX_SESSION_COOKIES: usize = 64;
const MAX_COOKIE_VALUE_BYTES: usize = 4096;
const MAX_COOKIE_HEADER_BYTES: usize = 16 * 1024;

fn default_account_enabled() -> bool {
    true
}

fn is_false(value: &bool) -> bool {
    !*value
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ZodAccessWorkspace {
    #[serde(default)]
    pub auto_sign_enabled: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub running: bool,
    #[serde(default)]
    pub accounts: Vec<ZodAccessAccount>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ZodAccessAccount {
    pub id: String,
    pub display_name: String,
    #[serde(default = "default_account_enabled")]
    pub enabled: bool,
    pub session_state: String,
    pub check_state: String,
    pub message: String,
    #[serde(default)]
    pub signed_today: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_success_date: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_checked_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_signed_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ZodAccessSession {
    pub cookies: Vec<SessionCookie>,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionCookie {
    pub name: String,
    pub value: String,
}

impl std::fmt::Debug for ZodAccessSession {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ZodAccessSession")
            .field(
                "cookies",
                &format_args!("[redacted; {}]", self.cookies.len()),
            )
            .finish()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DashboardState {
    SignedToday,
    ReadyToSign,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CheckOutcome {
    pub newly_signed: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckFailureKind {
    ReauthRequired,
    Temporary,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckFailure {
    pub kind: CheckFailureKind,
    pub message: String,
}

impl CheckFailure {
    fn reauth() -> Self {
        Self {
            kind: CheckFailureKind::ReauthRequired,
            message: "登录状态已失效，请重新登录".to_string(),
        }
    }

    fn temporary(message: impl Into<String>) -> Self {
        Self {
            kind: CheckFailureKind::Temporary,
            message: message.into(),
        }
    }
}

pub fn today_key() -> String {
    Local::now().format("%Y-%m-%d").to_string()
}

pub fn is_allowed_navigation_url(value: &str) -> bool {
    if value == "about:blank" {
        return true;
    }
    reqwest::Url::parse(value).is_ok_and(|url| has_official_origin(&url))
}

pub fn is_user_page_url(value: &str) -> bool {
    reqwest::Url::parse(value).is_ok_and(|url| has_official_origin(&url) && url.path() == "/user")
}

fn has_official_origin(url: &reqwest::Url) -> bool {
    url.scheme() == "https"
        && url.host_str() == Some("kp.zodaccyes.com")
        && url.port_or_known_default() == Some(443)
}

pub fn normalize_workspace_for_today(workspace: &mut ZodAccessWorkspace, today: &str) {
    workspace.running = false;
    for account in &mut workspace.accounts {
        account.signed_today = account.last_success_date.as_deref() == Some(today);
        if account.check_state == "checking" {
            account.check_state = if account.signed_today {
                "signed"
            } else {
                "waiting"
            }
            .to_string();
            account.message = if account.signed_today {
                "今日已签到"
            } else {
                "等待检查"
            }
            .to_string();
        }
    }
}

pub fn eligible_account_ids(workspace: &ZodAccessWorkspace, today: &str) -> Vec<String> {
    if !workspace.auto_sign_enabled {
        return Vec::new();
    }
    workspace
        .accounts
        .iter()
        .filter(|account| {
            account.enabled
                && account.session_state == "ready"
                && account.last_success_date.as_deref() != Some(today)
        })
        .map(|account| account.id.clone())
        .collect()
}

pub fn credential_cleanup_is_committed(workspace: &ZodAccessWorkspace, account_id: &str) -> bool {
    !workspace
        .accounts
        .iter()
        .any(|account| account.id == account_id)
}

pub fn unique_display_name<'a>(
    accounts: impl IntoIterator<Item = &'a ZodAccessAccount>,
    proposed: &str,
    exclude_id: Option<&str>,
) -> String {
    let base = proposed.trim();
    let base = if base.is_empty() {
        "ZodAccess 账号"
    } else {
        base
    };
    let used = accounts
        .into_iter()
        .filter(|account| exclude_id != Some(account.id.as_str()))
        .map(|account| account.display_name.as_str())
        .collect::<HashSet<_>>();
    if !used.contains(base) {
        return base.to_string();
    }
    for suffix in 2..=999 {
        let candidate = format!("{base} ({suffix})");
        if !used.contains(candidate.as_str()) {
            return candidate;
        }
    }
    format!("{base} ({})", uuid::Uuid::new_v4().simple())
}

pub fn validate_session(session: ZodAccessSession) -> Result<ZodAccessSession, String> {
    if session.cookies.is_empty() || session.cookies.len() > MAX_SESSION_COOKIES {
        return Err("ZodAccess 登录状态不完整".to_string());
    }
    let mut seen = HashSet::new();
    let mut total = 0usize;
    for cookie in &session.cookies {
        if cookie.name.is_empty()
            || cookie.name.len() > 128
            || !cookie.name.bytes().all(valid_cookie_name_byte)
            || cookie.value.len() > MAX_COOKIE_VALUE_BYTES
            || cookie
                .value
                .bytes()
                .any(|byte| byte <= 0x20 || byte == b';' || byte == 0x7f)
            || !seen.insert(cookie.name.as_str())
        {
            return Err("ZodAccess 登录状态格式异常".to_string());
        }
        total = total.saturating_add(cookie.name.len() + cookie.value.len() + 2);
    }
    if total > MAX_COOKIE_HEADER_BYTES {
        return Err("ZodAccess 登录状态过大".to_string());
    }
    Ok(session)
}

fn valid_cookie_name_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric()
        || matches!(
            byte,
            b'!' | b'#'
                | b'$'
                | b'%'
                | b'&'
                | b'\''
                | b'*'
                | b'+'
                | b'-'
                | b'.'
                | b'^'
                | b'_'
                | b'`'
                | b'|'
                | b'~'
        )
}

fn cookie_header(session: &ZodAccessSession) -> Result<String, CheckFailure> {
    validate_session(session.clone())
        .map_err(|_| CheckFailure::temporary("ZodAccess 登录状态无法读取"))?;
    Ok(session
        .cookies
        .iter()
        .map(|cookie| format!("{}={}", cookie.name, cookie.value))
        .collect::<Vec<_>>()
        .join("; "))
}

fn client() -> Result<Client, CheckFailure> {
    Client::builder()
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(12))
        .redirect(Policy::none())
        .build()
        .map_err(|_| CheckFailure::temporary("无法创建 ZodAccess 签到连接"))
}

pub fn inspect_session(session: &ZodAccessSession) -> Result<DashboardState, CheckFailure> {
    inspect_session_with_client(&client()?, session)
}

pub fn check_and_sign(session: &ZodAccessSession) -> Result<CheckOutcome, CheckFailure> {
    let client = client()?;
    match inspect_session_with_client(&client, session)? {
        DashboardState::SignedToday => Ok(CheckOutcome {
            newly_signed: false,
        }),
        DashboardState::ReadyToSign => submit_checkin(&client, session),
    }
}

fn inspect_session_with_client(
    client: &Client,
    session: &ZodAccessSession,
) -> Result<DashboardState, CheckFailure> {
    let response = client
        .get(USER_URL)
        .header(ACCEPT, "text/html,application/xhtml+xml")
        .header(COOKIE, cookie_header(session)?)
        .send()
        .map_err(|_| CheckFailure::temporary("ZodAccess 连接失败，稍后重试"))?;
    let raw = read_response(response, "text/html", MAX_DASHBOARD_BYTES)?;
    classify_dashboard(&raw)
}

fn submit_checkin(
    client: &Client,
    session: &ZodAccessSession,
) -> Result<CheckOutcome, CheckFailure> {
    let response = client
        .post(CHECKIN_URL)
        .header(ACCEPT, "application/json")
        .header(COOKIE, cookie_header(session)?)
        .header(REFERER, USER_URL)
        .header("X-Requested-With", "XMLHttpRequest")
        .send()
        .map_err(|_| CheckFailure::temporary("ZodAccess 签到连接失败，稍后重试"))?;
    let raw = read_response(response, "application/json", MAX_CHECKIN_BYTES)?;
    let payload: Value = serde_json::from_slice(&raw)
        .map_err(|_| CheckFailure::temporary("ZodAccess 签到响应无法识别"))?;
    match classify_checkin_payload(&payload)? {
        true => Ok(CheckOutcome { newly_signed: true }),
        false => match inspect_session_with_client(client, session)? {
            DashboardState::SignedToday => Ok(CheckOutcome {
                newly_signed: false,
            }),
            DashboardState::ReadyToSign => Err(CheckFailure::temporary(
                "ZodAccess 未确认签到成功，稍后重试",
            )),
        },
    }
}

fn classify_checkin_payload(payload: &Value) -> Result<bool, CheckFailure> {
    match payload.get("ret").and_then(Value::as_i64) {
        Some(1) => Ok(true),
        Some(0) => Ok(false),
        _ => Err(CheckFailure::temporary("ZodAccess 签到响应无法识别")),
    }
}

fn is_login_redirect(location: &str) -> bool {
    reqwest::Url::parse(ORIGIN)
        .ok()
        .and_then(|base| base.join(location).ok())
        .is_some_and(|url| has_official_origin(&url) && url.path() == "/auth/login")
}

fn validate_response_size(content_length: Option<u64>, max_bytes: u64) -> Result<(), CheckFailure> {
    if content_length.is_some_and(|length| length > max_bytes) {
        return Err(CheckFailure::temporary("ZodAccess 响应过大"));
    }
    Ok(())
}

fn read_response(
    response: Response,
    expected_content_type: &str,
    max_bytes: u64,
) -> Result<Vec<u8>, CheckFailure> {
    let status = response.status();
    if status.is_redirection() {
        let location = response
            .headers()
            .get(LOCATION)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default();
        if is_login_redirect(location) {
            return Err(CheckFailure::reauth());
        }
        return Err(CheckFailure::temporary("ZodAccess 返回了未识别的跳转"));
    }
    if status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN {
        return Err(CheckFailure::reauth());
    }
    if !status.is_success() {
        return Err(CheckFailure::temporary(format!(
            "ZodAccess 服务暂时不可用（{}）",
            status.as_u16()
        )));
    }
    let content_type = response
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_ascii_lowercase();
    if content_type.split(';').next().map(str::trim) != Some(expected_content_type) {
        return Err(CheckFailure::temporary("ZodAccess 返回了未识别的内容"));
    }
    validate_response_size(response.content_length(), max_bytes)?;
    let mut raw = Vec::new();
    response
        .take(max_bytes + 1)
        .read_to_end(&mut raw)
        .map_err(|_| CheckFailure::temporary("无法读取 ZodAccess 响应"))?;
    if raw.len() as u64 > max_bytes {
        return Err(CheckFailure::temporary("ZodAccess 响应过大"));
    }
    Ok(raw)
}

fn classify_dashboard(raw: &[u8]) -> Result<DashboardState, CheckFailure> {
    let html = std::str::from_utf8(raw)
        .map_err(|_| CheckFailure::temporary("ZodAccess 用户页无法识别"))?;
    let normalized = html.to_ascii_lowercase();
    if normalized.contains("id=\"kt_sign_in_form\"")
        || normalized.contains("id='kt_sign_in_form'")
        || normalized.contains("<title>登录")
    {
        return Err(CheckFailure::reauth());
    }
    if html.contains("今日已签到") {
        return Ok(DashboardState::SignedToday);
    }
    if normalized.contains("id=\"checkin\"")
        || normalized.contains("id='checkin'")
        || html.contains("每日签到")
    {
        return Ok(DashboardState::ReadyToSign);
    }
    Err(CheckFailure::temporary("无法识别 ZodAccess 签到页面"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn account(id: &str, name: &str) -> ZodAccessAccount {
        ZodAccessAccount {
            id: id.to_string(),
            display_name: name.to_string(),
            enabled: true,
            session_state: "ready".to_string(),
            check_state: "waiting".to_string(),
            message: "等待检查".to_string(),
            signed_today: false,
            last_success_date: None,
            last_checked_at: None,
            last_signed_at: None,
            created_at: "2026-08-22T00:00:00Z".to_string(),
            updated_at: "2026-08-22T00:00:00Z".to_string(),
        }
    }

    #[test]
    fn dashboard_requires_multiple_known_signals() {
        assert_eq!(
            classify_dashboard(r#"<a class="disabled">今日已签到</a>"#.as_bytes()),
            Ok(DashboardState::SignedToday)
        );
        assert_eq!(
            classify_dashboard(r#"<a id="checkin">每日签到</a>"#.as_bytes()),
            Ok(DashboardState::ReadyToSign)
        );
        assert_eq!(
            classify_dashboard(br#"<form id="kt_sign_in_form"></form>"#)
                .unwrap_err()
                .kind,
            CheckFailureKind::ReauthRequired
        );
        assert!(classify_dashboard(b"<main>unknown</main>").is_err());
    }

    #[test]
    fn navigation_only_allows_the_official_https_origin() {
        assert!(is_allowed_navigation_url("about:blank"));
        assert!(is_allowed_navigation_url(
            "https://kp.zodaccyes.com/auth/login"
        ));
        assert!(!is_allowed_navigation_url(
            "http://kp.zodaccyes.com/auth/login"
        ));
        assert!(!is_allowed_navigation_url(
            "https://kp.zodaccyes.com.evil.example/user"
        ));
        assert!(!is_allowed_navigation_url(
            "https://ks.zodaccyes.com/auth/login"
        ));
        assert!(is_user_page_url("https://kp.zodaccyes.com/user?from=login"));
        assert!(!is_user_page_url("https://kp.zodaccyes.com/user/settings"));
    }

    #[test]
    fn login_redirects_only_match_the_official_login_page() {
        assert!(is_login_redirect("/auth/login"));
        assert!(is_login_redirect(
            "https://kp.zodaccyes.com/auth/login?expired=1"
        ));
        assert!(!is_login_redirect("//evil.example/auth/login"));
        assert!(!is_login_redirect("/user"));
    }

    #[test]
    fn checkin_json_requires_a_known_numeric_result() {
        assert_eq!(
            classify_checkin_payload(&serde_json::json!({ "ret": 1 })),
            Ok(true)
        );
        assert_eq!(
            classify_checkin_payload(&serde_json::json!({ "ret": 0 })),
            Ok(false)
        );
        assert!(classify_checkin_payload(&serde_json::json!({ "ret": "1" })).is_err());
        assert!(classify_checkin_payload(&serde_json::json!({ "ok": true })).is_err());
    }

    #[test]
    fn response_size_limit_rejects_oversized_declared_bodies() {
        assert!(validate_response_size(Some(32 * 1024), 32 * 1024).is_ok());
        assert!(validate_response_size(Some(32 * 1024 + 1), 32 * 1024).is_err());
        assert!(validate_response_size(None, 32 * 1024).is_ok());
    }

    #[test]
    fn session_validation_rejects_injection_and_duplicates() {
        let valid = ZodAccessSession {
            cookies: vec![SessionCookie {
                name: "session".to_string(),
                value: "encoded-value".to_string(),
            }],
        };
        assert!(validate_session(valid).is_ok());
        assert!(validate_session(ZodAccessSession {
            cookies: vec![SessionCookie {
                name: "session".to_string(),
                value: "value; injected=1".to_string(),
            }],
        })
        .is_err());
        assert!(validate_session(ZodAccessSession {
            cookies: vec![
                SessionCookie {
                    name: "session".to_string(),
                    value: "one".to_string(),
                },
                SessionCookie {
                    name: "session".to_string(),
                    value: "two".to_string(),
                },
            ],
        })
        .is_err());
    }

    #[test]
    fn sensitive_session_debug_is_redacted() {
        let session = ZodAccessSession {
            cookies: vec![SessionCookie {
                name: "session".to_string(),
                value: "fixture-value-to-redact".to_string(),
            }],
        };
        let rendered = format!("{session:?}");
        assert!(!rendered.contains("fixture-value-to-redact"));
        assert!(rendered.contains("redacted"));
    }

    #[test]
    fn scheduler_respects_global_account_session_and_day_state() {
        let mut workspace = ZodAccessWorkspace {
            auto_sign_enabled: true,
            running: false,
            accounts: vec![
                account("one", "One"),
                account("two", "Two"),
                account("three", "Three"),
                account("four", "Four"),
            ],
        };
        workspace.accounts[1].enabled = false;
        workspace.accounts[2].last_success_date = Some("2026-08-22".to_string());
        workspace.accounts[3].session_state = "reauthRequired".to_string();
        assert_eq!(eligible_account_ids(&workspace, "2026-08-22"), vec!["one"]);
        workspace.auto_sign_enabled = false;
        assert!(eligible_account_ids(&workspace, "2026-08-22").is_empty());
    }

    #[test]
    fn duplicate_display_names_are_preserved_with_suffixes() {
        let existing = vec![account("one", "Zod"), account("two", "Zod (2)")];
        assert_eq!(unique_display_name(&existing, "Zod", None), "Zod (3)");
        assert_eq!(unique_display_name(&existing, "Zod", Some("one")), "Zod");
    }

    #[test]
    fn credential_cleanup_waits_for_the_configured_account_to_be_removed() {
        let mut workspace = ZodAccessWorkspace {
            auto_sign_enabled: true,
            running: false,
            accounts: vec![account("one", "One")],
        };
        assert!(!credential_cleanup_is_committed(&workspace, "one"));
        workspace.accounts.clear();
        assert!(credential_cleanup_is_committed(&workspace, "one"));
    }

    #[test]
    fn old_workspace_json_uses_safe_defaults() {
        let workspace: ZodAccessWorkspace = serde_json::from_str("{}").unwrap();
        assert!(!workspace.auto_sign_enabled);
        assert!(!workspace.running);
        assert!(workspace.accounts.is_empty());
    }
}
