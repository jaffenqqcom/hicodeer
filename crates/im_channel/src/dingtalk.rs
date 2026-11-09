//! DingTalk provider.
//!
//! Sending is plain HTTPS. Receiving uses Stream mode: a WebSocket whose URL
//! is obtained from the gateway endpoint, so no inbound address is needed.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::Context as _;
use async_tungstenite::tungstenite::Message;
use futures::StreamExt as _;
use futures::future::BoxFuture;
use tokio::sync::mpsc;

use crate::binding::{ConversationKind, DingTalkCredentials, Platform};
use crate::http::post_json;
use crate::provider::{
    ImProvider, InboundProvider, IncomingMessage, OutgoingMessage,
};
use crate::ws;

const TOKEN_URL: &str = "https://api.dingtalk.com/v1.0/oauth2/accessToken";
const GROUP_SEND_URL: &str = "https://api.dingtalk.com/v1.0/robot/groupMessages/send";
const DIRECT_SEND_URL: &str = "https://api.dingtalk.com/v1.0/robot/oToMessages/batchSend";
const STREAM_OPEN_URL: &str = "https://api.dingtalk.com/v1.0/gateway/connections/open";
/// The robot message topic the bridge subscribes to.
const BOT_MESSAGE_TOPIC: &str = "/v1.0/im/bot/messages/get";
/// DingTalk's text message type; the only one this bridge sends.
const TEXT_MESSAGE_KEY: &str = "sampleText";
/// `conversationType` values on the wire.
const CONVERSATION_TYPE_GROUP: &str = "2";

pub(crate) struct DingTalkProvider {
    state: Arc<DingTalkState>,
}

struct DingTalkState {
    http: Arc<dyn http_client::HttpClient>,
    credentials: DingTalkCredentials,
    conversation_id: Option<String>,
    /// Set once the long connection is up, so the bridge can report it.
    connection: Arc<AtomicBool>,
}

impl DingTalkProvider {
    pub(crate) fn new(
        http: Arc<dyn http_client::HttpClient>,
        credentials: DingTalkCredentials,
        conversation_id: Option<String>,
        connection: Arc<AtomicBool>,
    ) -> Self {
        Self {
            state: Arc::new(DingTalkState {
                http,
                credentials,
                conversation_id,
                connection,
            }),
        }
    }
}

impl DingTalkState {
    async fn access_token(&self) -> anyhow::Result<String> {
        let payload = serde_json::json!({
            "appKey": self.credentials.client_id,
            "appSecret": self.credentials.client_secret,
        });
        let response = post_json(self.http.as_ref(), TOKEN_URL, Vec::new(), &payload).await?;
        response
            .get("accessToken")
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned)
            .context("dingtalk token response has no accessToken")
    }

    async fn send_text(
        &self,
        text: &str,
        conversation: Option<&str>,
        kind: ConversationKind,
    ) -> anyhow::Result<()> {
        // Send even without a learned conversation: the platform's own answer
        // says more than a local refusal, and a bot that defaults to its own
        // chat can still deliver.
        let conversation_id = conversation.or(self.conversation_id.as_deref()).unwrap_or_default();
        let token = self.access_token().await?;
        let msg_param = serde_json::json!({ "content": text }).to_string();
        let (url, payload) = match kind {
            ConversationKind::Group => (
                GROUP_SEND_URL,
                serde_json::json!({
                    "robotCode": self.credentials.robot_code,
                    "openConversationId": conversation_id,
                    "msgKey": TEXT_MESSAGE_KEY,
                    "msgParam": msg_param,
                }),
            ),
            ConversationKind::Direct => (
                DIRECT_SEND_URL,
                serde_json::json!({
                    "robotCode": self.credentials.robot_code,
                    "userIds": [conversation_id],
                    "msgKey": TEXT_MESSAGE_KEY,
                    "msgParam": msg_param,
                }),
            ),
        };
        let headers = vec![
            ("x-acs-dingtalk-access-token".to_string(), token),
            ("Content-Type".to_string(), "application/json".to_string()),
        ];
        post_json(self.http.as_ref(), url, headers, &payload).await?;
        Ok(())
    }

    async fn run_inbound(
        &self,
        sink: mpsc::UnboundedSender<IncomingMessage>,
    ) -> anyhow::Result<()> {
        let payload = serde_json::json!({
            "clientId": self.credentials.client_id,
            "clientSecret": self.credentials.client_secret,
            "subscriptions": [{
                "type": "CALLBACK",
                "topic": BOT_MESSAGE_TOPIC,
            }],
            "ua": "hicodeer/0.1",
        });
        let response = post_json(self.http.as_ref(), STREAM_OPEN_URL, Vec::new(), &payload).await?;
        let endpoint = response
            .get("endpoint")
            .and_then(serde_json::Value::as_str)
            .context("dingtalk stream response has no endpoint")?
            .to_string();
        // The gateway hands out a one-shot ticket that must ride on the
        // WebSocket URL; without it the handshake is answered with 400.
        let ticket = response
            .get("ticket")
            .and_then(serde_json::Value::as_str)
            .context("dingtalk stream response has no ticket")?;
        let mut url = http_client::Url::parse(&endpoint)
            .with_context(|| format!("invalid dingtalk endpoint {endpoint}"))?;
        url.query_pairs_mut().append_pair("ticket", ticket);
        log::info!("im channel: dingtalk stream endpoint discovered");
        let socket = ws::connect(url.as_str(), &[]).await?;
        self.connection.store(true, Ordering::Relaxed);
        let (mut write, mut read) = socket.split();
        while let Some(frame) = read.next().await {
            match frame? {
                Message::Text(text) => {
                    let value: serde_json::Value =
                        serde_json::from_str(text.as_str()).unwrap_or_default();
                    let message_id = value
                        .pointer("/headers/messageId")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or_default()
                        .to_string();
                    let frame_type = value
                        .get("type")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or_default();
                    // Every frame, including the system ping, has to be
                    // acknowledged or the server redelivers it.
                    let ack = serde_json::json!({
                        "code": 200,
                        "headers": {
                            "messageId": message_id,
                            "contentType": "application/json",
                        },
                        "message": "OK",
                        "data": "{\"response\":\"\"}",
                    });
                    write.send(Message::Text(ack.to_string().into())).await?;
                    // Bot messages arrive as CALLBACK frames; EVENT frames are
                    // platform events. Both have to be read.
                    if frame_type == "EVENT" || frame_type == "CALLBACK" {
                        if let Some(message) = parse_event(&value) {
                            log::info!(
                                "im channel: dingtalk message in chat {} ({} chars)",
                                message.conversation_id,
                                message.text.chars().count()
                            );
                            if sink.send(message).is_err() {
                                break;
                            }
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
        Ok(())
    }
}

/// Reads one bot message event.
///
/// `data` is documented as a JSON string, but callbacks also arrive with it as
/// a plain object, so both shapes are read. The frame type is checked by the
/// caller; a bot message is recognised by the fields it carries.
fn parse_event(frame: &serde_json::Value) -> Option<IncomingMessage> {
    let data = match frame.get("data")? {
        serde_json::Value::String(text) => serde_json::from_str(text).ok()?,
        other => other.clone(),
    };
    let conversation_id = data.get("conversationId")?.as_str()?.to_string();
    let message_id = data.get("msgId")?.as_str()?.to_string();
    let sender_id = data
        .get("senderStaffId")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .to_string();
    let text = data
        .pointer("/text/content")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_string();
    // Group and one-to-one chats go through different send endpoints, so the
    // kind travels with the conversation id.
    let kind = match data
        .get("conversationType")
        .and_then(serde_json::Value::as_str)
    {
        Some(CONVERSATION_TYPE_GROUP) => ConversationKind::Group,
        _ => ConversationKind::Direct,
    };
    // Group sends address the conversation, but a one-to-one send addresses
    // the staff user; the conversation id is not accepted there.
    let conversation_id = if kind == ConversationKind::Direct && !sender_id.is_empty() {
        sender_id.clone()
    } else {
        conversation_id
    };
    Some(IncomingMessage {
        conversation_id,
        kind,
        message_id,
        reply_to: None,
        sender_id,
        text,
    })
}

impl ImProvider for DingTalkProvider {
    fn platform(&self) -> Platform {
        Platform::DingTalk
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

impl InboundProvider for DingTalkProvider {
    fn run_inbound(
        &self,
        sink: mpsc::UnboundedSender<IncomingMessage>,
    ) -> BoxFuture<'static, anyhow::Result<()>> {
        let state = self.state.clone();
        Box::pin(async move { state.run_inbound(sink).await })
    }
}
