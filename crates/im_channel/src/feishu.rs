//! Feishu / Lark provider.
//!
//! Sending is JSON over HTTPS. Receiving uses the long-connection endpoint:
//! Feishu hands back a WebSocket URL and frames are protobuf `pbbp2.Frame`
//! messages, so this module owns the minimal frame definition.
//!
//! Frame semantics follow the official client: `method` 0 is a control frame
//! (ping/pong) and 1 is a data frame (events). A data frame must be answered
//! with a data frame carrying `biz_rt`, or the server redelivers it.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use anyhow::Context as _;
use async_tungstenite::tungstenite::Message;
use futures::future::BoxFuture;
use futures::{SinkExt as _, StreamExt as _};
use prost::Message as ProstMessage;
use tokio::sync::mpsc;

use crate::binding::{ConversationKind, FeishuCredentials, Platform};
use crate::http::post_json;
use crate::provider::{
    ImProvider, InboundProvider, IncomingMessage, OutgoingMessage,
};
use crate::ws;

const TOKEN_URL: &str = "https://open.feishu.cn/open-apis/auth/v3/tenant_access_token/internal";
const MESSAGES_URL: &str = "https://open.feishu.cn/open-apis/im/v1/messages";
/// Long connections live outside the OpenAPI prefix: the prefixed path answers
/// 404, while the bare one returns the WebSocket URL, or an error code in the
/// body when the credentials are wrong.
const WS_ENDPOINT_URL: &str = "https://open.feishu.cn/callback/ws/endpoint";

/// Frame method values used by the long connection.
const METHOD_CONTROL: i32 = 0;
const METHOD_DATA: i32 = 1;
const HEADER_TYPE: &str = "type";
const HEADER_TYPE_PING: &str = "ping";
const HEADER_TYPE_EVENT: &str = "event";
/// Used until the endpoint handshake reports `ClientConfig.PingInterval`.
const DEFAULT_PING_INTERVAL: Duration = Duration::from_secs(120);
/// Header the server reads to decide whether a frame was acknowledged.
const ACK_HEADER_KEY: &str = "biz_rt";
/// Success code carried in the acknowledgement payload.
const ACK_OK_CODE: u16 = 200;
/// Query parameter that names the service the connection belongs to.
const SERVICE_ID_PARAM: &str = "service_id";

#[derive(Clone, PartialEq, ProstMessage)]
struct PbHeader {
    #[prost(string, required, tag = "1")]
    key: String,
    #[prost(string, required, tag = "2")]
    value: String,
}

/// `pbbp2.Frame`, the envelope of every long-connection packet.
#[derive(Clone, PartialEq, ProstMessage)]
struct PbFrame {
    #[prost(uint64, required, tag = "1")]
    seq_id: u64,
    #[prost(uint64, required, tag = "2")]
    log_id: u64,
    #[prost(int32, required, tag = "3")]
    service: i32,
    #[prost(int32, required, tag = "4")]
    method: i32,
    #[prost(message, repeated, tag = "5")]
    headers: Vec<PbHeader>,
    #[prost(string, optional, tag = "6")]
    payload_encoding: Option<String>,
    #[prost(string, optional, tag = "7")]
    payload_type: Option<String>,
    #[prost(bytes = "vec", optional, tag = "8")]
    payload: Option<Vec<u8>>,
    #[prost(string, optional, tag = "9")]
    log_id_new: Option<String>,
}

impl PbFrame {
    fn header(&self, key: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|header| header.key == key)
            .map(|header| header.value.as_str())
    }
}

pub(crate) struct FeishuProvider {
    state: Arc<FeishuState>,
}

struct FeishuState {
    http: Arc<dyn http_client::HttpClient>,
    credentials: FeishuCredentials,
    /// Chat id learned from the first inbound message; sending needs it.
    chat_id: Option<String>,
    /// Set once the long connection is up, so the bridge can report it.
    connection: Arc<AtomicBool>,
}

impl FeishuProvider {
    pub(crate) fn new(
        http: Arc<dyn http_client::HttpClient>,
        credentials: FeishuCredentials,
        chat_id: Option<String>,
        connection: Arc<AtomicBool>,
    ) -> Self {
        Self {
            state: Arc::new(FeishuState {
                http,
                credentials,
                chat_id,
                connection,
            }),
        }
    }
}

impl FeishuState {
    async fn tenant_token(&self) -> anyhow::Result<String> {
        let payload = serde_json::json!({
            "app_id": self.credentials.app_id,
            "app_secret": self.credentials.app_secret,
        });
        let response = post_json(self.http.as_ref(), TOKEN_URL, Vec::new(), &payload).await?;
        let code = response
            .get("code")
            .and_then(serde_json::Value::as_i64)
            .unwrap_or(-1);
        if code != 0 {
            anyhow::bail!("feishu token request failed: {response}");
        }
        response
            .get("tenant_access_token")
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned)
            .context("feishu token response has no tenant_access_token")
    }

    async fn send_text(
        &self,
        text: &str,
        conversation: Option<&str>,
    ) -> anyhow::Result<()> {
        let chat_id = conversation
            .or(self.chat_id.as_deref())
            .context("feishu is not bound to a chat yet; send the bot a message first")?;
        // A scan reports the user's open_id, so the first message can go out
        // before any chat exists. Chat ids start with "oc_", open ids with
        // "ou_", and the two are addressed through different id types.
        let receive_id_type = if chat_id.starts_with("ou_") {
            "open_id"
        } else {
            "chat_id"
        };
        let url = format!("{MESSAGES_URL}?receive_id_type={receive_id_type}");
        let token = self.tenant_token().await?;
        let content = serde_json::json!({ "text": text }).to_string();
        let payload = serde_json::json!({
            "receive_id": chat_id,
            "msg_type": "text",
            "content": content,
        });
        let headers = vec![
            ("Authorization".to_string(), format!("Bearer {token}")),
            ("Content-Type".to_string(), "application/json".to_string()),
        ];
        let response = post_json(self.http.as_ref(), &url, headers, &payload).await?;
        let code = response
            .get("code")
            .and_then(serde_json::Value::as_i64)
            .unwrap_or(-1);
        if code != 0 {
            anyhow::bail!("feishu send failed: {response}");
        }
        log::debug!("im channel: feishu accepted the message");
        Ok(())
    }

    async fn run_inbound(
        &self,
        sink: mpsc::UnboundedSender<IncomingMessage>,
    ) -> anyhow::Result<()> {
        let payload = serde_json::json!({
            "AppID": self.credentials.app_id,
            "AppSecret": self.credentials.app_secret,
        });
        let response = post_json(self.http.as_ref(), WS_ENDPOINT_URL, Vec::new(), &payload).await?;
        let code = response
            .get("code")
            .and_then(serde_json::Value::as_i64)
            .unwrap_or(-1);
        if code != 0 {
            anyhow::bail!("feishu long-connection endpoint failed: {response}");
        }
        let url = response
            .pointer("/data/URL")
            .and_then(serde_json::Value::as_str)
            .context("feishu endpoint response has no data.URL")?
            .to_string();
        let ping_interval = response
            .pointer("/data/ClientConfig/PingInterval")
            .and_then(serde_json::Value::as_u64)
            .filter(|seconds| *seconds > 0)
            .map(Duration::from_secs)
            .unwrap_or(DEFAULT_PING_INTERVAL);
        let service = service_id_from_url(&url);
        log::info!(
            "im channel: feishu long connection discovered, ping {}s, service {}",
            ping_interval.as_secs(),
            service
        );

        let socket = ws::connect(&url, &[]).await?;
        self.connection.store(true, Ordering::Relaxed);
        let (mut write, mut read) = socket.split();
        let mut ping = tokio::time::interval(ping_interval);
        // The first tick fires immediately; skip it so the connection is not
        // pinged before the handshake settles.
        ping.tick().await;
        loop {
            tokio::select! {
                _ = ping.tick() => {
                    let frame = PbFrame {
                        seq_id: 0,
                        log_id: 0,
                        service,
                        method: METHOD_CONTROL,
                        headers: vec![PbHeader {
                            key: HEADER_TYPE.to_string(),
                            value: HEADER_TYPE_PING.to_string(),
                        }],
                        payload_encoding: None,
                        payload_type: None,
                        payload: None,
                        log_id_new: None,
                    };
                    write
                        .send(Message::Binary(frame.encode_to_vec().into()))
                        .await?;
                }
                incoming = read.next() => {
                    let Some(frame) = incoming else {
                        log::warn!("im channel: feishu long connection closed");
                        break;
                    };
                    match frame? {
                        Message::Binary(bytes) => {
                            let frame = match PbFrame::decode(bytes.as_ref()) {
                                Ok(frame) => frame,
                                Err(error) => {
                                    log::warn!("im channel: feishu frame decode failed: {error}");
                                    continue;
                                }
                            };
                            if frame.method == METHOD_CONTROL {
                                // Pong may carry an updated ClientConfig; the
                                // interval only ever grows, so keep it simple.
                                continue;
                            }
                            if frame.method != METHOD_DATA {
                                continue;
                            }
                            let frame_type = frame.header(HEADER_TYPE).unwrap_or_default();
                            if !frame_type.is_empty() && frame_type != HEADER_TYPE_EVENT {
                                continue;
                            }
                            let started = Instant::now();
                            let message = frame
                                .payload
                                .as_deref()
                                .and_then(parse_event);
                            acknowledge(&mut write, &frame, started.elapsed()).await?;
                            if let Some(message) = message {
                                if sink.send(message).is_err() {
                                    break;
                                }
                            }
                        }
                        Message::Ping(data) => {
                            write.send(Message::Pong(data)).await?;
                        }
                        Message::Close(_) => break,
                        _ => {}
                    }
                }
            }
        }
        Ok(())
    }
}

/// Reads `service_id` from the endpoint URL; the server matches it on ping.
fn service_id_from_url(url: &str) -> i32 {
    http_client::Url::parse(url)
        .ok()
        .and_then(|url| {
            url.query_pairs()
                .find(|(key, _)| key == SERVICE_ID_PARAM)
                .and_then(|(_, value)| value.parse::<i32>().ok())
        })
        .unwrap_or_default()
}

/// Echoes the frame back as a data frame with `biz_rt`, Feishu's
/// acknowledgement.
async fn acknowledge<S>(write: &mut S, frame: &PbFrame, elapsed: Duration) -> anyhow::Result<()>
where
    S: futures::Sink<Message, Error = async_tungstenite::tungstenite::Error> + Unpin,
{
    let mut headers = frame.headers.clone();
    headers.push(PbHeader {
        key: ACK_HEADER_KEY.to_string(),
        // The official client sends the processing time negated.
        value: format!("-{}", elapsed.as_millis()),
    });
    let payload = serde_json::json!({ "code": ACK_OK_CODE })
        .to_string()
        .into_bytes();
    let ack = PbFrame {
        seq_id: frame.seq_id,
        log_id: frame.log_id,
        service: frame.service,
        method: METHOD_DATA,
        headers,
        payload_encoding: None,
        payload_type: None,
        payload: Some(payload),
        log_id_new: None,
    };
    write
        .send(Message::Binary(ack.encode_to_vec().into()))
        .await?;
    Ok(())
}

fn parse_event(payload: &[u8]) -> Option<IncomingMessage> {
    let value: serde_json::Value = serde_json::from_slice(payload).ok()?;
    let event_type = value.pointer("/header/event_type")?.as_str()?;
    if event_type != "im.message.receive_v1" {
        return None;
    }
    let message = value.pointer("/event/message")?;
    let conversation_id = message.get("chat_id")?.as_str()?.to_string();
    let message_id = message.get("message_id")?.as_str()?.to_string();
    // Sending addresses every chat as `chat_id`, so the kind is recorded rather
    // than needed; "p2p" is the only value that is not a group.
    let kind = match message.get("chat_type").and_then(serde_json::Value::as_str) {
        Some("p2p") => ConversationKind::Direct,
        _ => ConversationKind::Group,
    };
    let sender_id = value
        .pointer("/event/sender/sender_id/open_id")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .to_string();
    let content = message
        .get("content")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("{}");
    let text = serde_json::from_str::<serde_json::Value>(content)
        .ok()
        .and_then(|content| {
            content
                .get("text")
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned)
        })
        .unwrap_or_default();
    Some(IncomingMessage {
        conversation_id,
        kind,
        message_id,
        reply_to: None,
        sender_id,
        text,
    })
}

impl ImProvider for FeishuProvider {
    fn platform(&self) -> Platform {
        Platform::Feishu
    }

    fn send(&self, message: OutgoingMessage) -> BoxFuture<'static, anyhow::Result<()>> {
        let state = self.state.clone();
        Box::pin(async move {
            state
                .send_text(&message.text, message.conversation_id.as_deref())
                .await
        })
    }
}

impl InboundProvider for FeishuProvider {
    fn run_inbound(
        &self,
        sink: mpsc::UnboundedSender<IncomingMessage>,
    ) -> BoxFuture<'static, anyhow::Result<()>> {
        let state = self.state.clone();
        Box::pin(async move { state.run_inbound(sink).await })
    }
}
