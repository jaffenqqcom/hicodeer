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
use std::time::Duration;

use anyhow::Context as _;
use base64::Engine as _;
use flate2::Compression;
use flate2::write::GzEncoder;
use http_client::{AsyncBody, HttpClient, Method, Request};

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
        client_id: String,
        client_secret: String,
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
    payload: &serde_json::Value,
    error_status: ErrorStatus,
) -> anyhow::Result<serde_json::Value> {
    let request = Request::builder()
        .method(Method::POST)
        .uri(uri)
        .header("Content-Type", "application/json")
        .body(AsyncBody::from(serde_json::to_vec(payload)?))?;
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
        return Ok(PollOutcome::Ready {
            client_id: client_id.to_string(),
            client_secret: client_secret.to_string(),
            open_id,
        });
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
            Ok(PollOutcome::Ready {
                client_id,
                client_secret,
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
