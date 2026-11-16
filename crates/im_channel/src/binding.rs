//! The IM platforms this build knows how to talk to.
//!
//! Every platform can keep a configuration at the same time; only the selected
//! one runs a connection. Switching platforms closes the old connection before
//! opening the new one.

use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Serialize};

/// Credential fields keyed by name, as stored.
pub type CredentialValues = BTreeMap<String, String>;

/// IM platforms the bridge can talk to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Platform {
    Feishu,
    DingTalk,
    Qq,
    Wechat,
}

impl Platform {
    /// Every platform, in the order the settings table lists them.
    pub const ALL: [Self; 4] = [Self::Feishu, Self::DingTalk, Self::Qq, Self::Wechat];

    /// Stable identifier used in persisted settings.
    pub fn key(self) -> &'static str {
        match self {
            Self::Feishu => "feishu",
            Self::DingTalk => "dingtalk",
            Self::Qq => "qq",
            Self::Wechat => "wechat",
        }
    }

    /// Name shown in the settings page.
    pub fn display_name(self) -> &'static str {
        match self {
            Self::Feishu => "Feishu",
            Self::DingTalk => "DingTalk",
            Self::Qq => "QQ",
            Self::Wechat => "WeChat",
        }
    }
}

/// Whether the bound conversation is a group or a one-to-one chat.
///
/// Feishu derives this from the chat id, but DingTalk and QQ address groups
/// and people through different endpoints, so the kind has to be stored.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConversationKind {
    Group,
    Direct,
}

/// Credentials for a Feishu/Lark self-built app.
pub struct FeishuCredentials {
    pub app_id: String,
    pub app_secret: String,
}

/// Credentials for a DingTalk app using Stream mode.
pub struct DingTalkCredentials {
    pub client_id: String,
    pub client_secret: String,
    pub robot_code: String,
}

/// Credentials for a QQ official bot.
pub struct QqCredentials {
    pub app_id: String,
    pub app_secret: String,
}

/// Credentials for a WeChat iLink bot.
///
/// The token comes from the QR login; the owner is the WeChat user who scanned
/// it, and the base URL is the host the login reported for later calls.
pub struct WechatCredentials {
    pub bot_id: String,
    pub bot_token: String,
    pub owner_id: String,
    pub base_url: String,
}

// Secrets are redacted so an accidental `{:?}` in a log cannot leak them.
impl fmt::Debug for FeishuCredentials {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("FeishuCredentials")
            .field("app_id", &self.app_id)
            .field("app_secret", &"<redacted>")
            .finish()
    }
}

impl fmt::Debug for DingTalkCredentials {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DingTalkCredentials")
            .field("client_id", &self.client_id)
            .field("client_secret", &"<redacted>")
            .field("robot_code", &self.robot_code)
            .finish()
    }
}

impl fmt::Debug for QqCredentials {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("QqCredentials")
            .field("app_id", &self.app_id)
            .field("app_secret", &"<redacted>")
            .finish()
    }
}

impl fmt::Debug for WechatCredentials {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("WechatCredentials")
            .field("bot_id", &self.bot_id)
            .field("bot_token", &"<redacted>")
            .field("owner_id", &self.owner_id)
            .field("base_url", &self.base_url)
            .finish()
    }
}

/// The single IM binding.
#[derive(Debug)]
pub enum ImBinding {
    Feishu(FeishuCredentials),
    DingTalk(DingTalkCredentials),
    Qq(QqCredentials),
    Wechat(WechatCredentials),
}

impl ImBinding {
    pub fn platform(&self) -> Platform {
        match self {
            Self::Feishu(_) => Platform::Feishu,
            Self::DingTalk(_) => Platform::DingTalk,
            Self::Qq(_) => Platform::Qq,
            Self::Wechat(_) => Platform::Wechat,
        }
    }
}
