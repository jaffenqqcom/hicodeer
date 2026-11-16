//! Device-flow registration for Feishu and DingTalk.
//!
//! Both platforms accept an OAuth 2.0 device authorization flow (RFC 8628):
//! the app asks for a verification URL, the user scans it on the phone, and
//! the platform hands back the app credentials when the user confirms. This is
//! what makes binding possible without a developer-console trip.
//!
//! QQ has no such endpoint; it only offers a web page that shows the
//! credentials after the user creates a bot, so that platform stays manual.

use std::io::Write as _;
use std::sync::Mutex;
use std::time::Duration;

use anyhow::Context as _;
use base64::Engine as _;
use flate2::Compression;
use flate2::write::GzEncoder;
use http_client::{AsyncBody, HttpClient, Method, Request};

use crate::binding::CredentialValues;
use crate::http::get_json;

/// Bot name pre-filled on the confirmation page and on QQ's creation page.
pub const BOT_NAME: &str = "HiCodeer";
/// Capabilities the created Feishu app must have, or the bot could never send
/// a notification or receive a reply. The confirmation page shows them as
/// pre-selected permissions.
const FEISHU_ADDONS: &str = concat!(
    r#"{"scopes":{"tenant":["im:message:send_as_bot"]},"#,
    r#""events":{"items":{"tenant":["im.message.receive_v1"]}}}"#
);

const FEISHU_REGISTRATION: &str = "https://accounts.feishu.cn/oauth/v1/app/registration";
const DINGTALK_BASE: &str = "https://oapi.dingtalk.com";
/// QQ's bot platform entry; signing in leads to the bot list and the page that
/// shows a bot's AppID and AppSecret.
pub const QQ_CREATE_URL: &str = "https://q.qq.com/";

/// Feishu's fallback when the server omits the field.
const DEFAULT_INTERVAL: Duration = Duration::from_secs(5);
const DEFAULT_EXPIRES: Duration = Duration::from_secs(600);

pub struct DeviceFlow {
    pub device_code: String,
    pub verification_url: String,
    pub interval: Duration,
    pub expires_in: Duration,
}

pub enum PollOutcome {
    Pending,
    SlowDown,
    Ready {
        /// The credentials the platform handed back, keyed the way the
        /// platform's spec names its fields.
        values: CredentialValues,
        /// The scanning user, when the platform reports it. Feishu addresses a
        /// person by open_id, so a message can be sent before any chat exists.
        open_id: Option<String>,
    },
}

/// What to do with a response whose status is not 2xx.
#[derive(Clone, Copy, PartialEq, Eq)]
enum ErrorStatus {
    /// The status itself is the failure.
    Fatal,
    /// A 4xx body may still carry the protocol's answer. Feishu reports
    /// `authorization_pending` with 400 while the user has not confirmed yet,
    /// so treating 4xx as fatal would abort every scan on its first poll.
    InspectBody,
}

async fn post_form(
    client: &dyn HttpClient,
    uri: &str,
    body: String,
    error_status: ErrorStatus,
) -> anyhow::Result<serde_json::Value> {
    let request = Request::builder()
        .method(Method::POST)
        .uri(uri)
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body(AsyncBody::from(body))?;
    send_json(client, request, uri, error_status).await
}

async fn post_json_body(
    client: &dyn HttpClient,
    uri: &str,
    headers: &[(String, String)],
    payload: &serde_json::Value,
    error_status: ErrorStatus,
) -> anyhow::Result<serde_json::Value> {
    let mut builder = Request::builder()
        .method(Method::POST)
        .uri(uri)
        .header("Content-Type", "application/json");
    for (name, value) in headers {
        builder = builder.header(name, value);
    }
    let request = builder.body(AsyncBody::from(serde_json::to_vec(payload)?))?;
    send_json(client, request, uri, error_status).await
}

async fn send_json(
    client: &dyn HttpClient,
    request: Request<AsyncBody>,
    uri: &str,
    error_status: ErrorStatus,
) -> anyhow::Result<serde_json::Value> {
    use futures::AsyncReadExt as _;
    let mut response = client.send(request).await?;
    let status = response.status();
    let mut text = String::new();
    response.body_mut().read_to_string(&mut text).await?;
    let inspectable =
        error_status == ErrorStatus::InspectBody && status.is_client_error();
    if !status.is_success() && !inspectable {
        anyhow::bail!("registration request to {uri} failed with {status}: {text}");
    }
    serde_json::from_str(&text)
        .map_err(|error| anyhow::anyhow!("registration response is not JSON: {error}: {text}"))
}

/// Adds the parameters the confirmation page expects.
fn with_query(url: &str, params: &[(&str, &str)]) -> String {
    let separator = if url.contains('?') { '&' } else { '?' };
    let mut result = String::from(url);
    result.push(separator);
    let mut first = true;
    for (key, value) in params {
        if !first {
            result.push('&');
        }
        first = false;
        result.push_str(key);
        result.push('=');
        result.push_str(value);
    }
    result
}

/// Encodes the capability list the way the confirmation page reads it: JSON,
/// gzipped, then base64url without padding.
fn feishu_addons_parameter() -> anyhow::Result<String> {
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder
        .write_all(FEISHU_ADDONS.as_bytes())
        .context("cannot compress the Feishu capability list")?;
    let compressed = encoder
        .finish()
        .context("cannot finish the Feishu capability list")?;
    Ok(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(compressed))
}

/// Begins a Feishu device flow.
pub(crate) async fn begin_feishu(client: &dyn HttpClient) -> anyhow::Result<DeviceFlow> {
    let init = post_form(
        client,
        FEISHU_REGISTRATION,
        "action=init".to_string(),
        ErrorStatus::Fatal,
    ).await?;
    let methods = init
        .get("supported_auth_methods")
        .and_then(serde_json::Value::as_array)
        .cloned()
        .unwrap_or_default();
    let supports_secret = methods
        .iter()
        .any(|method| method.as_str() == Some("client_secret"));
    anyhow::ensure!(
        supports_secret,
        "feishu does not offer client_secret registration"
    );

    let begin = post_form(
        client,
        FEISHU_REGISTRATION,
        "action=begin&archetype=PersonalAgent&auth_method=client_secret&request_user_info=open_id"
            .to_string(),
        ErrorStatus::Fatal,
    )
    .await?;
    let device_code = begin
        .get("device_code")
        .and_then(serde_json::Value::as_str)
        .context("feishu begin has no device_code")?
        .to_string();
    let verification = begin
        .get("verification_uri_complete")
        .and_then(serde_json::Value::as_str)
        .context("feishu begin has no verification_uri_complete")?;
    let addons = feishu_addons_parameter()?;
    let verification_url = with_query(
        verification,
        &[
            ("from", "sdk"),
            ("tp", "sdk"),
            ("source", "hicodeer"),
            ("name", BOT_NAME),
            ("addons", &addons),
        ],
    );
    let interval = begin
        .get("interval")
        .and_then(serde_json::Value::as_u64)
        .map(Duration::from_secs)
        .unwrap_or(DEFAULT_INTERVAL);
    let expires_in = begin
        .get("expires_in")
        .and_then(serde_json::Value::as_u64)
        .map(Duration::from_secs)
        .unwrap_or(DEFAULT_EXPIRES);
    Ok(DeviceFlow {
        device_code,
        verification_url,
        interval,
        expires_in,
    })
}

/// Polls a Feishu device flow once.
pub(crate) async fn poll_feishu(
    client: &dyn HttpClient,
    device_code: &str,
) -> anyhow::Result<PollOutcome> {
    let body = format!("action=poll&device_code={device_code}");
    let response = post_form(client, FEISHU_REGISTRATION, body, ErrorStatus::InspectBody).await?;
    if let (Some(client_id), Some(client_secret)) = (
        response
            .get("client_id")
            .and_then(serde_json::Value::as_str),
        response
            .get("client_secret")
            .and_then(serde_json::Value::as_str),
    ) {
        let open_id = response
            .pointer("/user_info/open_id")
            .and_then(serde_json::Value::as_str)
            .or_else(|| {
                response
                    .get("open_id")
                    .and_then(serde_json::Value::as_str)
            })
            .filter(|open_id| !open_id.is_empty())
            .map(str::to_owned);
        let mut values = CredentialValues::new();
        values.insert("app_id".to_string(), client_id.to_string());
        values.insert("app_secret".to_string(), client_secret.to_string());
        return Ok(PollOutcome::Ready { values, open_id });
    }
    match response
        .get("error")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
    {
        "authorization_pending" => Ok(PollOutcome::Pending),
        "slow_down" => Ok(PollOutcome::SlowDown),
        "access_denied" => anyhow::bail!("feishu registration was denied"),
        "expired_token" => anyhow::bail!("feishu registration window expired"),
        other => anyhow::bail!("feishu registration failed: {other}: {response}"),
    }
}

/// Begins a DingTalk device flow.
pub(crate) async fn begin_dingtalk(client: &dyn HttpClient) -> anyhow::Result<DeviceFlow> {
    let init = post_json_body(
        client,
        &format!("{DINGTALK_BASE}/app/registration/init"),
        &[],
        &serde_json::json!({ "source": BOT_NAME }),
        ErrorStatus::Fatal,
    )
    .await?;
    let nonce = init
        .get("nonce")
        .and_then(serde_json::Value::as_str)
        .context("dingtalk init has no nonce")?
        .to_string();
    let begin = post_json_body(
        client,
        &format!("{DINGTALK_BASE}/app/registration/begin"),
        &[],
        &serde_json::json!({ "nonce": nonce }),
        ErrorStatus::Fatal,
    )
    .await?;
    let device_code = begin
        .get("device_code")
        .and_then(serde_json::Value::as_str)
        .context("dingtalk begin has no device_code")?
        .to_string();
    let verification_url = begin
        .get("verification_uri_complete")
        .and_then(serde_json::Value::as_str)
        .context("dingtalk begin has no verification_uri_complete")?
        .to_string();
    let interval = begin
        .get("interval")
        .and_then(serde_json::Value::as_u64)
        .map(Duration::from_secs)
        .unwrap_or(DEFAULT_INTERVAL)
        .max(Duration::from_secs(2));
    let expires_in = begin
        .get("expires_in")
        .and_then(serde_json::Value::as_u64)
        .map(Duration::from_secs)
        .unwrap_or_else(|| Duration::from_secs(7200));
    Ok(DeviceFlow {
        device_code,
        verification_url,
        interval,
        expires_in,
    })
}

/// Polls a DingTalk device flow once.
pub(crate) async fn poll_dingtalk(
    client: &dyn HttpClient,
    device_code: &str,
) -> anyhow::Result<PollOutcome> {
    let response = post_json_body(
        client,
        &format!("{DINGTALK_BASE}/app/registration/poll"),
        &[],
        &serde_json::json!({ "device_code": device_code }),
        ErrorStatus::InspectBody,
    )
    .await?;
    let status = response
        .get("status")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .to_ascii_uppercase();
    match status.as_str() {
        // SCANNED means the code was read but the app is still being created,
        // so it is an intermediate state like WAITING, not a failure.
        "WAITING" | "SCANNED" => Ok(PollOutcome::Pending),
        "SUCCESS" => {
            let client_id = response
                .get("client_id")
                .and_then(serde_json::Value::as_str)
                .context("dingtalk success carried no client_id")?
                .to_string();
            let client_secret = response
                .get("client_secret")
                .and_then(serde_json::Value::as_str)
                .context("dingtalk success carried no client_secret")?
                .to_string();
            let mut values = CredentialValues::new();
            values.insert("client_id".to_string(), client_id);
            values.insert("client_secret".to_string(), client_secret);
            Ok(PollOutcome::Ready {
                values,
                open_id: None,
            })
        }
        "FAIL" => {
            let reason = response
                .get("fail_reason")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("unknown reason");
            anyhow::bail!("dingtalk registration failed: {reason}")
        }
        "EXPIRED" => anyhow::bail!("dingtalk registration window expired"),
        other => anyhow::bail!("dingtalk registration returned {other}: {response}"),
    }
}

// ---------------------------------------------------------------------------
// WeChat (iLink) QR login
// ---------------------------------------------------------------------------

/// iLink's API host. A login may report a different host for later calls, which
/// the binding stores as its `base_url` field.
const WECHAT_BASE: &str = "https://ilinkai.weixin.qq.com";
/// `ilink_appid` declared by the channel package this client mirrors.
const WECHAT_APP_ID: &str = "bot";
/// `iLink-App-ClientVersion` packs the channel version as
/// major<<16 | minor<<8 | patch, so 2.4.10 becomes 0x2040A.
const WECHAT_CLIENT_VERSION: u64 = (2 << 16) | (4 << 8) | 10;
/// The `bot_type` selecting the ClawBot channel.
const WECHAT_BOT_TYPE: &str = "3";
/// How long WeChat keeps the login QR code scannable.
const WECHAT_QR_EXPIRES: Duration = Duration::from_secs(600);
/// The status call holds the connection for up to 35 seconds, so this is only
/// the pause between two calls.
const WECHAT_POLL_INTERVAL: Duration = Duration::from_secs(1);
/// Host a `scaned_but_redirect` answer pointed the polling at.
///
/// A login runs one scan at a time, so one slot is enough; the flow clears it
/// when it begins and when it finishes.
static WECHAT_POLL_HOST: Mutex<Option<String>> = Mutex::new(None);

fn wechat_headers() -> Vec<(String, String)> {
    vec![
        ("iLink-App-Id".to_string(), WECHAT_APP_ID.to_string()),
        (
            "iLink-App-ClientVersion".to_string(),
            WECHAT_CLIENT_VERSION.to_string(),
        ),
    ]
}

/// Percent-encodes a query value, keeping the unreserved characters.
fn encode_query_value(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                encoded.push(byte as char);
            }
            _ => encoded.push_str(&format!("%{byte:02X}")),
        }
    }
    encoded
}

fn absolute_host(host: &str) -> String {
    if host.starts_with("http://") || host.starts_with("https://") {
        host.to_string()
    } else {
        format!("https://{host}")
    }
}

/// Begins a WeChat login: asks for a QR code and returns the content the
/// settings page draws.
pub(crate) async fn begin_wechat(client: &dyn HttpClient) -> anyhow::Result<DeviceFlow> {
    if let Ok(mut host) = WECHAT_POLL_HOST.lock() {
        *host = None;
    }
    let uri = format!("{WECHAT_BASE}/ilink/bot/get_bot_qrcode?bot_type={WECHAT_BOT_TYPE}");
    let response = post_json_body(
        client,
        &uri,
        &wechat_headers(),
        &serde_json::json!({ "local_token_list": [] }),
        ErrorStatus::Fatal,
    )
    .await?;
    let qrcode = response
        .get("qrcode")
        .and_then(serde_json::Value::as_str)
        .context("wechat qr response has no qrcode")?
        .to_string();
    let content = response
        .get("qrcode_img_content")
        .and_then(serde_json::Value::as_str)
        .context("wechat qr response has no qrcode_img_content")?
        .to_string();
    log::info!("im channel: wechat login qr issued");
    Ok(DeviceFlow {
        device_code: qrcode,
        verification_url: content,
        interval: WECHAT_POLL_INTERVAL,
        expires_in: WECHAT_QR_EXPIRES,
    })
}

/// Polls a WeChat login once. A confirmed scan hands back the bot token, the
/// bot id and the user who scanned, which become the stored credentials.
pub(crate) async fn poll_wechat(
    client: &dyn HttpClient,
    device_code: &str,
) -> anyhow::Result<PollOutcome> {
    let host = WECHAT_POLL_HOST
        .lock()
        .ok()
        .and_then(|host| host.clone())
        .unwrap_or_else(|| WECHAT_BASE.to_string());
    let uri = format!(
        "{host}/ilink/bot/get_qrcode_status?qrcode={}",
        encode_query_value(device_code)
    );
    let response = get_json(client, &uri, wechat_headers()).await?;
    let status = response
        .get("status")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default();
    match status {
        "wait" | "scaned" => Ok(PollOutcome::Pending),
        "scaned_but_redirect" => {
            if let Some(host) = response
                .get("redirect_host")
                .and_then(serde_json::Value::as_str)
                .filter(|host| !host.is_empty())
            {
                if let Ok(mut slot) = WECHAT_POLL_HOST.lock() {
                    *slot = Some(absolute_host(host));
                }
            }
            Ok(PollOutcome::Pending)
        }
        "confirmed" => {
            let bot_id = response
                .get("ilink_bot_id")
                .and_then(serde_json::Value::as_str)
                .context("wechat login confirmed without ilink_bot_id")?
                .to_string();
            let bot_token = response
                .get("bot_token")
                .and_then(serde_json::Value::as_str)
                .context("wechat login confirmed without bot_token")?
                .to_string();
            let owner = response
                .get("ilink_user_id")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_string();
            let mut values = CredentialValues::new();
            values.insert("bot_id".to_string(), bot_id);
            values.insert("bot_token".to_string(), bot_token);
            values.insert("owner_id".to_string(), owner.clone());
            if let Some(base_url) = response
                .get("baseurl")
                .and_then(serde_json::Value::as_str)
                .filter(|base_url| !base_url.is_empty())
            {
                values.insert("base_url".to_string(), absolute_host(base_url));
            }
            if let Ok(mut host) = WECHAT_POLL_HOST.lock() {
                *host = None;
            }
            log::info!("im channel: wechat login confirmed");
            Ok(PollOutcome::Ready {
                values,
                open_id: (!owner.is_empty()).then_some(owner),
            })
        }
        "binded_redirect" => anyhow::bail!(
            "this WeChat account is already bound to another instance; unbind it there first"
        ),
        "need_verifycode" | "verify_code_blocked" => anyhow::bail!(
            "WeChat asks for a phone verification code, which this build cannot collect"
        ),
        "expired" => anyhow::bail!("the WeChat login QR code expired"),
        other => anyhow::bail!("wechat login returned {other}: {response}"),
    }
}

#[cfg(test)]
mod tests {
    use std::io::Read as _;

    use super::*;

    #[test]
    fn addons_survive_gzip_and_base64() {
        let encoded = feishu_addons_parameter().expect("encode the capability list");
        let compressed = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(encoded)
            .expect("decode base64url");
        let mut json = String::new();
        flate2::read::GzDecoder::new(compressed.as_slice())
            .read_to_string(&mut json)
            .expect("gunzip");
        assert_eq!(json, FEISHU_ADDONS);

        let value: serde_json::Value = serde_json::from_str(&json).expect("parse json");
        assert_eq!(
            value.pointer("/scopes/tenant/0").and_then(|v| v.as_str()),
            Some("im:message:send_as_bot")
        );
        assert_eq!(
            value
                .pointer("/events/items/tenant/0")
                .and_then(|v| v.as_str()),
            Some("im.message.receive_v1")
        );
    }
}
