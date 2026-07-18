use base64::{engine::general_purpose, Engine};
use md5::{Digest as _, Md5};
use reqwest::{
    blocking::{Client, RequestBuilder},
    header::{HeaderMap, HeaderValue, ACCEPT, CONTENT_TYPE, USER_AGENT},
    redirect::Policy,
};
use rsa::{
    pkcs1v15::SigningKey,
    signature::{SignatureEncoding, Signer},
    BigUint, RsaPrivateKey,
};
use serde::Deserialize;
use serde_json::Value;
use sha2::Sha256;
use std::{env, fs, path::Path, time::Duration};
use uuid::Uuid;

const DETAIL_PATH: &str = "/uni/activity/monthlyTask/v1/detail";
const SIGN_IN_PATH: &str = "/uni/activity/monthlyTask/v1/signIn";
const MAX_APP_SO_BYTES: u64 = 128 * 1024 * 1024;

#[derive(Debug, Clone)]
pub struct DailySignOutcome {
    pub uid: String,
    pub newly_signed: bool,
    pub accumulated_days: Option<u32>,
    pub free_coin_balance: Option<u32>,
    pub reward_name: Option<String>,
    pub reward_quantity: Option<u32>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct OopzLogin {
    uid: String,
    signature: String,
    endpoint: String,
    device_id: String,
}

struct RequestContext {
    login: OopzLogin,
    endpoint: String,
    app_version_number: String,
    time_offset: i64,
    signer: SigningKey<Sha256>,
}

#[derive(Clone)]
struct EmbeddedBlock {
    offset: usize,
    decoded: [u8; 48],
}

pub fn check_and_sign(executable: &Path, encoded_login: &str) -> Result<DailySignOutcome, String> {
    let context = RequestContext::from_installation(executable, encoded_login)?;
    let client = Client::builder()
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(12))
        .redirect(Policy::none())
        .build()
        .map_err(|error| format!("创建签到连接失败: {error}"))?;

    let detail = send_request(&client, &context, "GET", DETAIL_PATH, None)?;
    let detail_data = api_data(&detail)?;
    if bool_field(daily_data(detail_data), "signedToday") {
        return Ok(outcome_from_data(&context.login.uid, false, detail_data));
    }

    let body = "{}";
    let signed = match send_request(&client, &context, "POST", SIGN_IN_PATH, Some(body)) {
        Ok(signed) => signed,
        Err(sign_error) => {
            let confirmed = send_request(&client, &context, "GET", DETAIL_PATH, None)
                .ok()
                .and_then(|response| api_data(&response).ok().cloned());
            if let Some(data) = confirmed.filter(|data| bool_field(daily_data(data), "signedToday"))
            {
                return Ok(outcome_from_data(&context.login.uid, false, &data));
            }
            return Err(sign_error);
        }
    };
    let signed_data = api_data(&signed)?;
    if !bool_field(daily_data(signed_data), "signedToday") {
        return Err("OOPZ 未确认今日签到状态".to_string());
    }
    Ok(outcome_from_data(&context.login.uid, true, signed_data))
}

impl RequestContext {
    fn from_installation(executable: &Path, encoded_login: &str) -> Result<Self, String> {
        let login = decode_login(encoded_login)?;
        let endpoint = allowed_endpoint(&login.endpoint)?;
        let data_dir = executable
            .parent()
            .ok_or_else(|| "OOPZ 安装路径无效".to_string())?
            .join("data");
        let app_version_number = read_app_version_number(&data_dir)?;
        let app_so = data_dir.join("app.so");
        let signer = SigningKey::<Sha256>::new(extract_private_key(&app_so)?);
        Ok(Self {
            login,
            endpoint,
            app_version_number,
            time_offset: read_sign_time_offset(),
            signer,
        })
    }
}

fn decode_login(encoded: &str) -> Result<OopzLogin, String> {
    if encoded.len() > 16 * 1024 {
        return Err("OOPZ 登录状态异常".to_string());
    }
    let decoded = general_purpose::STANDARD
        .decode(encoded)
        .map_err(|_| "OOPZ 登录状态无法解析".to_string())?;
    let login: OopzLogin =
        serde_json::from_slice(&decoded).map_err(|_| "OOPZ 登录状态缺少签到信息".to_string())?;
    if !(1..=64).contains(&login.uid.len())
        || !login.uid.bytes().all(|byte| byte.is_ascii_alphanumeric())
        || !(1..=128).contains(&login.device_id.len())
        || !login.device_id.bytes().all(|byte| byte.is_ascii_graphic())
        || !(32..=4096).contains(&login.signature.len())
        || login.signature.matches('.').count() != 2
    {
        return Err("OOPZ 登录状态已失效或格式异常".to_string());
    }
    Ok(login)
}

fn allowed_endpoint(value: &str) -> Result<String, String> {
    let endpoint = value.trim().trim_end_matches('/');
    if matches!(
        endpoint,
        "https://gateway.oopz.cn" | "https://gateway1.oopz.cn" | "https://gateway2.oopz.cn"
    ) {
        Ok(endpoint.to_string())
    } else {
        Err("OOPZ 签到服务地址未通过校验".to_string())
    }
}

fn read_app_version_number(data_dir: &Path) -> Result<String, String> {
    let version = fs::read_to_string(data_dir.join("flutter_assets/assets/version"))
        .map_err(|_| "无法读取 OOPZ 版本".to_string())?;
    let digits = version
        .bytes()
        .filter(u8::is_ascii_digit)
        .map(char::from)
        .collect::<String>();
    let normalized = digits.trim_start_matches('0');
    if normalized.is_empty() || normalized.len() > 16 {
        return Err("OOPZ 版本格式异常".to_string());
    }
    Ok(normalized.to_string())
}

fn read_sign_time_offset() -> i64 {
    let Some(app_data) = env::var_os("APPDATA") else {
        return 0;
    };
    let preferences = Path::new(&app_data).join("oopz.cn/oopz/shared_preferences.json");
    let Ok(bytes) = fs::read(preferences) else {
        return 0;
    };
    let Ok(value) = serde_json::from_slice::<Value>(&bytes) else {
        return 0;
    };
    value
        .get("flutter.SIGN_TIME_OFFSET")
        .and_then(Value::as_i64)
        .filter(|offset| offset.abs() <= 10 * 60 * 1000)
        .unwrap_or(0)
}

fn send_request(
    client: &Client,
    context: &RequestContext,
    method: &str,
    path: &str,
    body: Option<&str>,
) -> Result<Value, String> {
    let now = chrono::Utc::now().timestamp_millis();
    let request_time = now
        .checked_add(context.time_offset)
        .ok_or_else(|| "系统时间异常".to_string())?;
    let signature = sign_request(&context.signer, path, body, request_time);
    let headers = request_headers(context, request_time, &signature)?;
    let url = format!("{}{}", context.endpoint, path);
    let request: RequestBuilder = match method {
        "GET" => client.get(url),
        "POST" => client.post(url).body(body.unwrap_or_default().to_string()),
        _ => return Err("不支持的 OOPZ 请求方式".to_string()),
    };
    let response = request
        .headers(headers)
        .send()
        .map_err(|error| format!("OOPZ 签到连接失败: {error}"))?;
    if !response.status().is_success() {
        return Err(format!("OOPZ 签到服务返回 {}", response.status().as_u16()));
    }
    let bytes = response
        .bytes()
        .map_err(|_| "OOPZ 签到响应读取失败".to_string())?;
    serde_json::from_slice::<Value>(&bytes).map_err(|_| "OOPZ 签到响应无法解析".to_string())
}

fn request_headers(
    context: &RequestContext,
    request_time: i64,
    request_signature: &str,
) -> Result<HeaderMap, String> {
    let mut headers = HeaderMap::new();
    headers.insert(USER_AGENT, HeaderValue::from_static("Dart/3.8 (dart:io)"));
    headers.insert(ACCEPT, HeaderValue::from_static("*/*"));
    headers.insert(
        CONTENT_TYPE,
        HeaderValue::from_static("application/json;charset=utf-8"),
    );
    for (name, value) in [
        ("Oopz-Person", context.login.uid.as_str()),
        ("Oopz-Request-Id", Uuid::new_v4().to_string().as_str()),
        ("Oopz-Platform", "windows"),
        ("Oopz-Signature", context.login.signature.as_str()),
        ("Oopz-Device-Id", context.login.device_id.as_str()),
        (
            "Oopz-App-Version-Number",
            context.app_version_number.as_str(),
        ),
        ("Oopz-Web", "false"),
        ("Oopz-Sign", request_signature),
        ("Oopz-Time", request_time.to_string().as_str()),
        ("Oopz-Channel", "Windows"),
    ] {
        let name = reqwest::header::HeaderName::from_bytes(name.as_bytes())
            .map_err(|_| "OOPZ 签到请求头异常".to_string())?;
        let value =
            HeaderValue::from_str(value).map_err(|_| "OOPZ 登录状态包含无效字段".to_string())?;
        headers.insert(name, value);
    }
    Ok(headers)
}

fn sign_request(
    signer: &SigningKey<Sha256>,
    path: &str,
    body: Option<&str>,
    request_time: i64,
) -> String {
    let mut md5 = Md5::new();
    md5.update(path.as_bytes());
    if let Some(body) = body {
        md5.update(body.as_bytes());
    }
    let factor = format!("{:x}{request_time}", md5.finalize());
    general_purpose::STANDARD.encode(signer.sign(factor.as_bytes()).to_bytes())
}

fn api_data(response: &Value) -> Result<&Value, String> {
    if response.get("status").and_then(Value::as_bool) != Some(true) {
        let message = ["message", "error", "code"]
            .iter()
            .find_map(|key| response.get(key).and_then(Value::as_str))
            .filter(|value| !value.trim().is_empty())
            .unwrap_or("请求未成功");
        return Err(format!("OOPZ 签到失败: {}", truncate_message(message)));
    }
    response
        .get("data")
        .filter(|data| data.is_object())
        .ok_or_else(|| "OOPZ 签到响应缺少状态".to_string())
}

fn truncate_message(value: &str) -> String {
    value
        .chars()
        .filter(|char| !char.is_control())
        .take(120)
        .collect()
}

fn bool_field(value: &Value, key: &str) -> bool {
    value.get(key).and_then(Value::as_bool).unwrap_or(false)
}

fn daily_data(value: &Value) -> &Value {
    value
        .get("signIn")
        .filter(|sign_in| sign_in.is_object())
        .unwrap_or(value)
}

fn u32_field(value: &Value, key: &str) -> Option<u32> {
    value
        .get(key)
        .and_then(Value::as_u64)
        .and_then(|number| u32::try_from(number).ok())
}

fn outcome_from_data(uid: &str, newly_signed: bool, data: &Value) -> DailySignOutcome {
    let daily = daily_data(data);
    let reward = daily.get("reward").filter(|value| value.is_object());
    DailySignOutcome {
        uid: uid.to_string(),
        newly_signed,
        accumulated_days: u32_field(daily, "accumulatedDays"),
        free_coin_balance: u32_field(data, "freeCoinBalance")
            .or_else(|| u32_field(daily, "freeCoinBalance")),
        reward_name: reward
            .and_then(|value| value.get("name"))
            .and_then(Value::as_str)
            .or_else(|| daily.get("rewardName").and_then(Value::as_str))
            .map(str::to_string),
        reward_quantity: reward
            .and_then(|value| u32_field(value, "quantity"))
            .or_else(|| u32_field(daily, "quantity")),
    }
}

fn extract_private_key(path: &Path) -> Result<RsaPrivateKey, String> {
    let metadata = fs::metadata(path).map_err(|_| "无法读取 OOPZ 签名组件".to_string())?;
    if metadata.len() == 0 || metadata.len() > MAX_APP_SO_BYTES {
        return Err("OOPZ 签名组件大小异常".to_string());
    }
    let blob = fs::read(path).map_err(|_| "无法读取 OOPZ 签名组件".to_string())?;
    let blocks = embedded_blocks(&blob);
    let first_prefix = hex_literal();
    let first = blocks
        .iter()
        .find(|block| block.decoded.starts_with(&first_prefix))
        .ok_or_else(|| "当前 OOPZ 版本暂不支持静默签到".to_string())?;
    let exponent = blocks
        .iter()
        .find(|block| {
            block.decoded[6..11] == [2, 3, 1, 0, 1] && block.decoded[11..15] == [2, 0x82, 1, 1]
        })
        .ok_or_else(|| "当前 OOPZ 版本暂不支持静默签到".to_string())?;
    let marker = |position: usize| {
        blocks
            .iter()
            .filter(|block| block.decoded[position..position + 3] == [2, 0x81, 0x81])
            .collect::<Vec<_>>()
    };
    let markers_8 = marker(8);
    let markers_20 = marker(20);
    let markers_32 = marker(32);
    let markers_44 = marker(44);
    if markers_8.len() != 1
        || markers_20.len() != 1
        || markers_32.len() != 2
        || markers_44.len() != 1
    {
        return Err("当前 OOPZ 版本暂不支持静默签到".to_string());
    }
    let dp_marker = markers_8[0];
    let q_marker = markers_20[0];
    let dq_marker = markers_44[0];
    let fixed = [
        first.offset,
        exponent.offset,
        q_marker.offset,
        dp_marker.offset,
        dq_marker.offset,
    ];

    for p_index in 0..2 {
        let p_marker = markers_32[p_index];
        let qi_marker = markers_32[1 - p_index];
        let pool = blocks
            .iter()
            .filter(|block| {
                !fixed.contains(&block.offset)
                    && block.offset != p_marker.offset
                    && block.offset != qi_marker.offset
            })
            .collect::<Vec<_>>();
        for p_a in &pool {
            for p_b in &pool {
                if p_a.offset == p_b.offset {
                    continue;
                }
                let p_bytes = join_prime(
                    &p_marker.decoded[35..],
                    &p_a.decoded,
                    &p_b.decoded,
                    &q_marker.decoded[..20],
                );
                if p_bytes.len() != 129 || p_bytes[0] != 0 {
                    continue;
                }
                let remaining = pool
                    .iter()
                    .copied()
                    .filter(|block| block.offset != p_a.offset && block.offset != p_b.offset)
                    .collect::<Vec<_>>();
                for q_a in &remaining {
                    for q_b in &remaining {
                        if q_a.offset == q_b.offset {
                            continue;
                        }
                        let q_bytes = join_prime(
                            &q_marker.decoded[23..],
                            &q_a.decoded,
                            &q_b.decoded,
                            &dp_marker.decoded[..8],
                        );
                        if q_bytes.len() != 129 || q_bytes[0] != 0 {
                            continue;
                        }
                        let p = BigUint::from_bytes_be(&p_bytes);
                        let q = BigUint::from_bytes_be(&q_bytes);
                        let product = &p * &q;
                        let raw_modulus = product.to_bytes_be();
                        if raw_modulus.len() != 256 {
                            continue;
                        }
                        let mut modulus = Vec::with_capacity(257);
                        modulus.push(0);
                        modulus.extend_from_slice(&raw_modulus);
                        let prefix = &first.decoded[37..];
                        let suffix = &exponent.decoded[..6];
                        if !modulus.starts_with(prefix) || !modulus.ends_with(suffix) {
                            continue;
                        }
                        let available = remaining
                            .iter()
                            .copied()
                            .filter(|block| {
                                block.offset != q_a.offset && block.offset != q_b.offset
                            })
                            .collect::<Vec<_>>();
                        let middle_start = prefix.len();
                        let middle = (0..5)
                            .map(|index| {
                                let start = middle_start + index * 48;
                                &modulus[start..start + 48]
                            })
                            .collect::<Vec<_>>();
                        let middle_matches = middle.iter().enumerate().all(|(index, part)| {
                            middle[..index].iter().all(|earlier| earlier != part)
                                && available
                                    .iter()
                                    .any(|block| block.decoded.as_slice() == *part)
                        });
                        if middle_matches {
                            return RsaPrivateKey::from_p_q(p, q, BigUint::from(65_537u32))
                                .map_err(|_| "OOPZ 签名组件校验失败".to_string());
                        }
                    }
                }
            }
        }
    }
    Err("当前 OOPZ 版本暂不支持静默签到".to_string())
}

fn embedded_blocks(blob: &[u8]) -> Vec<EmbeddedBlock> {
    let mut blocks = Vec::new();
    let alphabet = |byte: &u8| byte.is_ascii_alphanumeric() || matches!(*byte, b'+' | b'/');
    let mut offset = 0usize;
    while offset.saturating_add(80) <= blob.len() {
        let length = u64::from_le_bytes(blob[offset + 8..offset + 16].try_into().unwrap());
        let encoded = &blob[offset + 16..offset + 80];
        if length == 128 && encoded.iter().all(alphabet) {
            if let Ok(decoded) = general_purpose::STANDARD.decode(encoded) {
                if let Ok(decoded) = decoded.try_into() {
                    blocks.push(EmbeddedBlock { offset, decoded });
                }
            }
        }
        offset += 16;
    }
    blocks
}

fn join_prime(first: &[u8], middle_a: &[u8], middle_b: &[u8], last: &[u8]) -> Vec<u8> {
    [first, middle_a, middle_b, last].concat()
}

fn hex_literal() -> [u8; 37] {
    [
        0x30, 0x82, 0x04, 0xC0, 0x02, 0x01, 0x00, 0x30, 0x0D, 0x06, 0x09, 0x2A, 0x86, 0x48, 0x86,
        0xF7, 0x0D, 0x01, 0x01, 0x01, 0x05, 0x00, 0x04, 0x82, 0x04, 0xAA, 0x30, 0x82, 0x04, 0xA6,
        0x02, 0x01, 0x00, 0x02, 0x82, 0x01, 0x01,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use rsa::traits::PublicKeyParts;

    #[test]
    fn endpoint_allowlist_rejects_redirect_and_host_confusion() {
        assert_eq!(
            allowed_endpoint("https://gateway.oopz.cn/").unwrap(),
            "https://gateway.oopz.cn"
        );
        assert!(allowed_endpoint("http://gateway.oopz.cn").is_err());
        assert!(allowed_endpoint("https://gateway.oopz.cn.example.com").is_err());
        assert!(allowed_endpoint("https://gateway.oopz.cn/path").is_err());
    }

    #[test]
    fn embedded_signer_can_be_loaded_when_fixture_is_available() {
        let Some(path) = std::env::var_os("NEA_OOPZ_APP_SO_FIXTURE") else {
            return;
        };
        let key = extract_private_key(Path::new(&path)).unwrap();
        assert_eq!(key.size(), 256);
    }

    #[test]
    fn nested_daily_detail_is_recognized_without_signing_again() {
        let data = serde_json::json!({
            "freeCoinBalance": 3,
            "signIn": {
                "signedToday": true,
                "accumulatedDays": 2,
                "rewardName": "抽奖币",
                "quantity": 1
            }
        });
        assert!(bool_field(daily_data(&data), "signedToday"));
        let outcome = outcome_from_data("uid", false, &data);
        assert_eq!(outcome.accumulated_days, Some(2));
        assert_eq!(outcome.free_coin_balance, Some(3));
        assert_eq!(outcome.reward_name.as_deref(), Some("抽奖币"));
        assert_eq!(outcome.reward_quantity, Some(1));
    }
}
