//! QQ official bot provider.
//!
//! The bot API is JSON over HTTPS for sending and a Gateway WebSocket for
//! receiving. Group and direct chats use different paths, so the conversation
//! kind is stored with the binding.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::Context as _;
use async_tungstenite::tungstenite::Message;
use futures::StreamExt as _;
use futures::future::BoxFuture;
use tokio::sync::mpsc;

use crate::binding::{ConversationKind, Platform, QqCredentials};
use crate::http::{get_json, post_json, string_field};
use crate::provider::{
    ImProvider, InboundProvider, IncomingMessage, OutgoingMessage,
};
use crate::ws;

const TOKEN_URL: &str = "https://bots.qq.com/app/getAppAccessToken";
const API_BASE: &str = "https://api.bot.qq.com";

/// Error codes meaning the message being replied to is no longer usable:
/// `40034005` ("回复消息msg_id已过期") and `304027` (MSG_EXPIRE). Once one of
/// these comes back the stored id must be dropped, or every later notification
/// fails the same way.
const EXPIRED_MESSAGE_CODES: [&str; 2] = ["40034005", "304027"];

/// Gateway opcodes, from the QQ bot gateway protocol.
const OP_DISPATCH: i64 = 0;
const OP_HEARTBEAT: i64 = 1;
const OP_IDENTIFY: i64 = 2;
const OP_HELLO: i64 = 10;
/// Group @-messages and one-to-one messages share one intent.
const INTENT_GROUP_AND_C2C: i64 = 1 << 25;
/// Used until the gateway's Hello frame reports the real interval.
const DEFAULT_HEARTBEAT: Duration = Duration::from_secs(30);

const EVENT_C2C_MESSAGE: &str = "C2C_MESSAGE_CREATE";
const EVENT_GROUP_AT_MESSAGE: &str = "GROUP_AT_MESSAGE_CREATE";

pub(crate) struct QqProvider {
    state: Arc<QqState>,
}

struct QqState {
    http: Arc<dyn http_client::HttpClient>,
    credentials: QqCredentials,
    conversation_id: Option<String>,
    /// Set once the long connection is up, so the bridge can report it.
    connection: Arc<AtomicBool>,
    /// Last inbound message per conversation, so a notification can go out as
    /// a passive reply.
    ///
    /// QQ only lets most bots answer a user who spoke first; a push without a
    /// message id is rejected unless the bot has the proactive-message grant.
    /// One slot per conversation keeps a message in one chat from being
    /// answered with another chat's id.
    replies: Mutex<HashMap<String, ReplyTarget>>,
}

#[derive(Clone)]
struct ReplyTarget {
    message_id: String,
    /// Replies to the same message must increment this.
    sequence: u32,
}

impl QqProvider {
    pub(crate) fn new(
        http: Arc<dyn http_client::HttpClient>,
        credentials: QqCredentials,
        conversation_id: Option<String>,
        connection: Arc<AtomicBool>,
    ) -> Self {
        Self {
            state: Arc::new(QqState {
                http,
                credentials,
                conversation_id,
                connection,
                replies: Mutex::new(HashMap::new()),
            }),
        }
    }
}

impl QqState {
    async fn access_token(&self) -> anyhow::Result<String> {
        let payload = serde_json::json!({
            "appId": self.credentials.app_id,
            "clientSecret": self.credentials.app_secret,
        });
        let response = post_json(self.http.as_ref(), TOKEN_URL, Vec::new(), &payload).await?;
        response
            .get("access_token")
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned)
            .context("qq token response has no access_token")
    }

    fn authorization(&self, token: &str) -> Vec<(String, String)> {
        vec![
            ("Authorization".to_string(), format!("QQBot {token}")),
            ("Content-Type".to_string(), "application/json".to_string()),
        ]
    }

    async fn send_text(
        &self,
        text: &str,
        conversation: Option<&str>,
        kind: ConversationKind,
    ) -> anyhow::Result<()> {
        // Send even without a learned conversation: the platform's own answer
        // says more than a local refusal.
        let conversation_id = conversation.or(self.conversation_id.as_deref()).unwrap_or_default();
        let token = self.access_token().await?;
        let url = match kind {
            ConversationKind::Group => {
                format!("{}/v2/groups/{conversation_id}/messages", API_BASE)
            }
            ConversationKind::Direct => {
                format!("{}/v2/users/{conversation_id}/messages", API_BASE)
            }
        };
        let payload = serde_json::json!({
            "content": text,
            "msg_type": 0,
        });
        let reply = self.replies.lock().ok().and_then(|mut replies| {
            replies.get_mut(conversation_id).map(|target| {
                target.sequence += 1;
                (target.message_id.clone(), target.sequence)
            })
        });
        let mut payload = payload;
        match reply {
            Some((message_id, sequence)) => {
                payload["msg_id"] = serde_json::Value::String(message_id);
                payload["msg_seq"] = serde_json::Value::from(sequence);
            }
            None => {
                log::warn!(
                    "im channel: qq has no inbound message to reply to, sending as a proactive message which most bots are not allowed to do"
                );
            }
        }
        if let Err(error) = post_json(
            self.http.as_ref(),
            &url,
            self.authorization(&token),
            &payload,
        )
        .await
        {
            let detail = format!("{error:#}");
            if !EXPIRED_MESSAGE_CODES
                .iter()
                .any(|code| detail.contains(code))
            {
                return Err(error);
            }
            if let Ok(mut replies) = self.replies.lock() {
                replies.remove(conversation_id);
            }
            anyhow::bail!(
                "qq can no longer answer this chat: the message it was replying to expired. Send the bot another message. ({detail})"
            );
        }
        Ok(())
    }

    async fn run_inbound(
        &self,
        sink: mpsc::UnboundedSender<IncomingMessage>,
    ) -> anyhow::Result<()> {
        let token = self.access_token().await?;
        let gateway = get_json(
            self.http.as_ref(),
            &format!("{}/gateway", API_BASE),
            self.authorization(&token),
        )
        .await?;
        let url = string_field(&gateway, "url")?.to_string();
        log::info!("im channel: qq gateway discovered");
        let socket = ws::connect(&url, &[]).await?;
        self.connection.store(true, Ordering::Relaxed);
        let (mut write, mut read) = socket.split();
        let mut heartbeat = tokio::time::interval(DEFAULT_HEARTBEAT);
        let mut sequence: i64 = 0;
        loop {
            tokio::select! {
                _ = heartbeat.tick() => {
                    let payload = serde_json::json!({ "op": OP_HEARTBEAT, "d": sequence });
                    write.send(Message::Text(payload.to_string().into())).await?;
                }
                frame = read.next() => {
                    let Some(frame) = frame else {
                        log::warn!("im channel: qq gateway closed");
                        break;
                    };
                    let frame = frame?;
                    match frame {
                        Message::Text(text) => {
                            let value: serde_json::Value =
                                serde_json::from_str(text.as_str()).unwrap_or_default();
                            if let Some(seq) = value.get("s").and_then(serde_json::Value::as_i64) {
                                sequence = seq;
                            }
                            match value.get("op").and_then(serde_json::Value::as_i64) {
                                Some(OP_HELLO) => {
                                    if let Some(interval_ms) = value
                                        .pointer("/d/heartbeat_interval")
                                        .and_then(serde_json::Value::as_u64)
                                    {
                                        heartbeat = tokio::time::interval(
                                            Duration::from_millis(interval_ms),
                                        );
                                    }
                                    let identify = serde_json::json!({
                                        "op": OP_IDENTIFY,
                                        "d": {
                                            "token": format!("QQBot {token}"),
                                            "intents": INTENT_GROUP_AND_C2C,
                                            "properties": {
                                                "$os": "linux",
                                                "$browser": "hicodeer",
                                                "$device": "hicodeer",
                                            },
                                        },
                                    });
                                    write
                                        .send(Message::Text(identify.to_string().into()))
                                        .await?;
                                    log::info!("im channel: qq identify sent");
                                }
                                Some(OP_DISPATCH) => {
                                    let event = value
                                        .get("t")
                                        .and_then(serde_json::Value::as_str)
                                        .unwrap_or("");
                                    log::info!("im channel: qq dispatch {event}");
                                    if let Some(message) = parse_dispatch(&value) {
                                        if let Ok(mut replies) = self.replies.lock() {
                                            replies.insert(
                                                message.conversation_id.clone(),
                                                ReplyTarget {
                                                    message_id: message.message_id.clone(),
                                                    sequence: 0,
                                                },
                                            );
                                        }
                                        log::debug!(
                                            "im channel: qq message in chat {} ({} chars)",
                                            message.conversation_id,
                                            message.text.chars().count()
                                        );
                                        if sink.send(message).is_err() {
                                            break;
                                        }
                                    }
                                }
                                Some(op @ (7 | 9)) => {
                                    log::warn!("im channel: qq gateway asked to reconnect (op {op})");
                                }
                                Some(op) => log::debug!("im channel: qq gateway op {op}"),
                                None => {}
                            }
                        }
                        Message::Ping(data) => {
                            write.send(Message::Pong(data)).await?;
                        }
                        Message::Close(frame) => {
                            log::warn!("im channel: qq gateway closed: {frame:?}");
                            break;
                        }
                        _ => {}
                    }
                }
            }
        }
        Ok(())
    }
}

/// Turns one dispatch frame into a message, when it is a chat message.
fn parse_dispatch(value: &serde_json::Value) -> Option<IncomingMessage> {
    let event = value.get("t").and_then(serde_json::Value::as_str)?;
    let data = value.get("d")?;
    let message_id = data
        .get("id")
        .and_then(serde_json::Value::as_str)?
        .to_string();
    let content = data
        .get("content")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .to_string();
    let (conversation_id, sender_id, kind) = match event {
        EVENT_C2C_MESSAGE => {
            // Newer payloads use `user_openid`; older ones carry `id`.
            let openid = data
                .pointer("/author/user_openid")
                .or_else(|| data.pointer("/author/id"))
                .and_then(serde_json::Value::as_str)?
                .to_string();
            (openid.clone(), openid, ConversationKind::Direct)
        }
        EVENT_GROUP_AT_MESSAGE => (
            data.get("group_openid")
                .and_then(serde_json::Value::as_str)?
                .to_string(),
            data.pointer("/author/member_openid")
                .or_else(|| data.pointer("/author/id"))
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_string(),
            ConversationKind::Group,
        ),
        _ => return None,
    };
    Some(IncomingMessage {
        conversation_id,
        kind,
        message_id,
        reply_to: None,
        sender_id,
        text: content.trim().to_string(),
    })
}

impl ImProvider for QqProvider {
    fn platform(&self) -> Platform {
        Platform::Qq
    }

    fn send(&self, message: OutgoingMessage) -> BoxFuture<'static, anyhow::Result<()>> {
        let state = self.state.clone();
        Box::pin(async move {
            state
                .send_text(
                    &message.text,
                    message.conversation_id.as_deref(),
                    message.kind,
                )
                .await
        })
    }
}

impl InboundProvider for QqProvider {
    fn run_inbound(
        &self,
        sink: mpsc::UnboundedSender<IncomingMessage>,
    ) -> BoxFuture<'static, anyhow::Result<()>> {
        let state = self.state.clone();
        Box::pin(async move { state.run_inbound(sink).await })
    }
}
