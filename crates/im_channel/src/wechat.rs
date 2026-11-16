//! WeChat iLink bot provider.
//!
//! Unlike the other platforms this one is HTTPS only: inbound messages come
//! from a long poll, and every reply has to echo the `context_token` of the
//! message it answers. The token is issued per inbound message, so the newest
//! one per conversation is kept and reused.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use base64::Engine as _;
use futures::future::BoxFuture;
use ring::rand::{SecureRandom as _, SystemRandom};
use tokio::sync::mpsc;

use crate::binding::{ConversationKind, Platform, WechatCredentials};
use crate::http::post_json;
use crate::provider::{ImProvider, InboundProvider, IncomingMessage, OutgoingMessage};

/// Host used when the login reported no `baseurl` of its own.
const DEFAULT_API_BASE: &str = "https://ilinkai.weixin.qq.com";
/// The channel build this client identifies as, sent in every request body.
const CHANNEL_VERSION: &str = "2.4.10";
/// Product token in the request body, in the User-Agent style the backend
/// expects.
const BOT_AGENT: &str = "HiCodeer/1.0.0";
/// `ilink_appid` declared by the channel package this client mirrors.
const APP_ID: &str = "bot";
/// `iLink-App-ClientVersion` packs the channel version as
/// major<<16 | minor<<8 | patch, so 2.4.10 becomes 0x2040A.
const CLIENT_VERSION: u64 = (2 << 16) | (4 << 8) | 10;
/// The server reports the bot token as stale with this code.
const SESSION_EXPIRED_ERRCODE: i64 = -14;
/// Pause after a failed poll that is not a stale token.
const RETRY_DELAY: Duration = Duration::from_secs(5);
/// Only user messages are delivered to the bridge; bot messages are our own
/// echoes.
const MESSAGE_TYPE_USER: i64 = 1;
const MESSAGE_TYPE_BOT: i64 = 2;
/// A reply is a finished message, not a streaming chunk.
const MESSAGE_STATE_FINISH: i64 = 2;
/// The only item kind this bridge reads or writes.
const ITEM_TYPE_TEXT: i64 = 1;

pub(crate) struct WechatProvider {
    state: Arc<WechatState>,
}

struct WechatState {
    http: Arc<dyn http_client::HttpClient>,
    credentials: WechatCredentials,
    conversation_id: Option<String>,
    /// Set once a poll has succeeded, so the bridge can report it.
    connection: Arc<AtomicBool>,
    /// Newest context token per conversation; a send without one is refused by
    /// the server, so the map is the only way to answer.
    contexts: Mutex<HashMap<String, String>>,
    /// Cursor the long poll resumes from.
    cursor: Mutex<String>,
}

impl WechatProvider {
    pub(crate) fn new(
        http: Arc<dyn http_client::HttpClient>,
        credentials: WechatCredentials,
        conversation_id: Option<String>,
        connection: Arc<AtomicBool>,
    ) -> Self {
        Self {
            state: Arc::new(WechatState {
                http,
                credentials,
                conversation_id,
                connection,
                contexts: Mutex::new(HashMap::new()),
                cursor: Mutex::new(String::new()),
            }),
        }
    }
}

/// Builds the headers every iLink call carries.
fn wechat_headers(token: &str) -> Vec<(String, String)> {
    let mut random = [0u8; 4];
    let uin = match SystemRandom::new().fill(&mut random) {
        Ok(()) => u32::from_be_bytes(random),
        Err(_) => {
            log::warn!("im channel: wechat cannot read a random value for X-WECHAT-UIN");
            0
        }
    };
    let uin = base64::engine::general_purpose::STANDARD.encode(uin.to_string());
    vec![
        ("AuthorizationType".to_string(), "ilink_bot_token".to_string()),
        ("Authorization".to_string(), format!("Bearer {token}")),
        ("X-WECHAT-UIN".to_string(), uin),
        ("iLink-App-Id".to_string(), APP_ID.to_string()),
        (
            "iLink-App-ClientVersion".to_string(),
            CLIENT_VERSION.to_string(),
        ),
    ]
}

/// `base_info` that every request body carries.
fn base_info() -> serde_json::Value {
    serde_json::json!({
        "channel_version": CHANNEL_VERSION,
        "bot_agent": BOT_AGENT,
    })
}

/// A fresh id for one outbound message.
fn client_id() -> String {
    let mut bytes = [0u8; 8];
    if let Err(error) = SystemRandom::new().fill(&mut bytes) {
        log::warn!("im channel: wechat cannot read a random value for a client id: {error}");
    }
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

impl WechatState {
    fn api_base(&self) -> &str {
        if self.credentials.base_url.is_empty() {
            DEFAULT_API_BASE
        } else {
            self.credentials.base_url.as_str()
        }
    }

    fn endpoint(&self, path: &str) -> String {
        format!("{}/{path}", self.api_base())
    }

    /// True when a message comes from the user who bound the channel. A bot
    /// can be reached by anyone who knows it, and this bridge answers a single
    /// conversation, so other senders are refused rather than mixed in.
    fn is_owner(&self, sender: &str) -> bool {
        self.credentials.owner_id.is_empty() || self.credentials.owner_id == sender
    }

    async fn send_text(
        &self,
        text: &str,
        conversation: Option<&str>,
    ) -> anyhow::Result<()> {
        let target = conversation
            .or(self.conversation_id.as_deref())
            .unwrap_or_default();
        anyhow::ensure!(
            !target.is_empty(),
            "wechat has no chat to send to yet; send the bot a message first"
        );
        let context = self
            .contexts
            .lock()
            .ok()
            .and_then(|contexts| contexts.get(target).cloned());
        if context.is_none() {
            log::warn!(
                "im channel: wechat has no context token for this chat; the server may refuse the message"
            );
        }
        let mut message = serde_json::json!({
            "from_user_id": "",
            "to_user_id": target,
            "client_id": client_id(),
            "message_type": MESSAGE_TYPE_BOT,
            "message_state": MESSAGE_STATE_FINISH,
            "item_list": [{
                "type": ITEM_TYPE_TEXT,
                "text_item": { "text": text },
            }],
        });
        if let Some(context) = context {
            message["context_token"] = serde_json::Value::String(context);
        }
        let payload = serde_json::json!({ "msg": message, "base_info": base_info() });
        let response = post_json(
            self.http.as_ref(),
            &self.endpoint("ilink/bot/sendmessage"),
            wechat_headers(&self.credentials.bot_token),
            &payload,
        )
        .await?;
        let ret = response
            .get("ret")
            .and_then(serde_json::Value::as_i64)
            .unwrap_or(0);
        if ret != 0 {
            anyhow::bail!("wechat refused the message: ret={ret}: {response}");
        }
        Ok(())
    }

    async fn run_inbound(
        &self,
        sink: mpsc::UnboundedSender<IncomingMessage>,
    ) -> anyhow::Result<()> {
        // A long poll parks the request until a message arrives, so the first
        // answer can take the server's whole hold time. The channel is usable
        // as soon as the poll is issued; a failed request clears the flag.
        self.connection.store(true, Ordering::Relaxed);
        loop {
            let cursor = self
                .cursor
                .lock()
                .ok()
                .map(|cursor| cursor.clone())
                .unwrap_or_default();
            let payload = serde_json::json!({
                "get_updates_buf": cursor,
                "base_info": base_info(),
            });
            let response = post_json(
                self.http.as_ref(),
                &self.endpoint("ilink/bot/getupdates"),
                wechat_headers(&self.credentials.bot_token),
                &payload,
            )
            .await?;
            let ret = response
                .get("ret")
                .and_then(serde_json::Value::as_i64)
                .unwrap_or(0);
            let errcode = response
                .get("errcode")
                .and_then(serde_json::Value::as_i64)
                .unwrap_or(0);
            if ret != 0 || errcode != 0 {
                if ret == SESSION_EXPIRED_ERRCODE || errcode == SESSION_EXPIRED_ERRCODE {
                    self.connection.store(false, Ordering::Relaxed);
                    anyhow::bail!(
                        "the WeChat session expired; bind the channel again with a new QR scan"
                    );
                }
                log::warn!("im channel: wechat poll failed: ret={ret} errcode={errcode}: {response}");
                tokio::time::sleep(RETRY_DELAY).await;
                continue;
            }
            self.connection.store(true, Ordering::Relaxed);
            if let Some(next) = response
                .get("get_updates_buf")
                .and_then(serde_json::Value::as_str)
            {
                if let Ok(mut cursor) = self.cursor.lock() {
                    *cursor = next.to_string();
                }
            }
            let Some(messages) = response
                .get("msgs")
                .and_then(serde_json::Value::as_array)
                .cloned()
            else {
                continue;
            };
            for value in messages {
                let Some((message, context)) = parse_message(&value) else {
                    continue;
                };
                if !self.is_owner(&message.sender_id) {
                    log::warn!(
                        "im channel: wechat ignored a message from a user other than the binding owner"
                    );
                    continue;
                }
                if !context.is_empty() {
                    if let Ok(mut contexts) = self.contexts.lock() {
                        contexts.insert(message.conversation_id.clone(), context);
                    }
                }
                log::debug!(
                    "im channel: wechat message in chat {} ({} chars)",
                    message.conversation_id,
                    message.text.chars().count()
                );
                if sink.send(message).is_err() {
                    return Ok(());
                }
            }
        }
    }
}

/// Turns one polled message into a chat message plus its context token.
///
/// Only text items are read: media needs the CDN download path, which this
/// bridge does not carry.
fn parse_message(value: &serde_json::Value) -> Option<(IncomingMessage, String)> {
    let message_type = value
        .get("message_type")
        .and_then(serde_json::Value::as_i64)?;
    if message_type != MESSAGE_TYPE_USER {
        return None;
    }
    let sender = value
        .get("from_user_id")
        .and_then(serde_json::Value::as_str)
        .filter(|sender| !sender.is_empty())?
        .to_string();
    let text = value
        .get("item_list")
        .and_then(serde_json::Value::as_array)?
        .iter()
        .find_map(|item| {
            let kind = item.get("type").and_then(serde_json::Value::as_i64)?;
            if kind != ITEM_TYPE_TEXT {
                return None;
            }
            item.pointer("/text_item/text")
                .and_then(serde_json::Value::as_str)
        })?
        .to_string();
    let message_id = value
        .get("client_id")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .to_string();
    let context = value
        .get("context_token")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .to_string();
    Some((
        IncomingMessage {
            conversation_id: sender.clone(),
            kind: ConversationKind::Direct,
            message_id,
            reply_to: None,
            sender_id: sender,
            text: text.trim().to_string(),
        },
        context,
    ))
}

impl ImProvider for WechatProvider {
    fn platform(&self) -> Platform {
        Platform::Wechat
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

impl InboundProvider for WechatProvider {
    fn run_inbound(
        &self,
        sink: mpsc::UnboundedSender<IncomingMessage>,
    ) -> BoxFuture<'static, anyhow::Result<()>> {
        let state = self.state.clone();
        Box::pin(async move { state.run_inbound(sink).await })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_text_from_a_user_becomes_a_message() {
        let value = serde_json::json!({
            "message_type": MESSAGE_TYPE_USER,
            "from_user_id": "user-1",
            "client_id": "client-1",
            "context_token": "context-1",
            "item_list": [{ "type": ITEM_TYPE_TEXT, "text_item": { "text": "  hi  " } }],
        });
        let (message, context) = parse_message(&value).expect("a text message");
        assert_eq!(message.conversation_id, "user-1");
        assert_eq!(message.sender_id, "user-1");
        assert_eq!(message.text, "hi");
        assert_eq!(context, "context-1");
    }

    #[test]
    fn bot_echoes_and_media_are_skipped() {
        let echo = serde_json::json!({
            "message_type": MESSAGE_TYPE_BOT,
            "from_user_id": "bot",
            "item_list": [{ "type": ITEM_TYPE_TEXT, "text_item": { "text": "hi" } }],
        });
        assert!(parse_message(&echo).is_none());

        let image = serde_json::json!({
            "message_type": MESSAGE_TYPE_USER,
            "from_user_id": "user-1",
            "item_list": [{ "type": 2 }],
        });
        assert!(parse_message(&image).is_none());
    }
}
