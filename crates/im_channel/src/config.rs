//! Persisted shape of the IM channels.
//!
//! The stored shape is platform-agnostic: one selected channel plus a map of
//! channel configurations keyed by the platform key. Keeping a map rather than
//! a struct per platform is what lets a new platform be added without touching
//! this file.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::binding::{ConversationKind, Platform};

fn default_direct() -> StoredConversationKind {
    StoredConversationKind::Direct
}

/// Everything the settings page owns, stored encrypted.
#[derive(Clone, Default, Serialize, Deserialize)]
pub(crate) struct ImSettings {
    /// The channel the bridge connects; `None` means IM is off.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel: Option<Platform>,
    /// One configuration per channel, keyed by [`Platform::key`].
    #[serde(default)]
    pub channels: BTreeMap<String, StoredBinding>,
}

/// One channel's configuration, with the fields its spec declared.
///
/// The platform is the map key, so it is not repeated here: one fact, one place
/// to read it, and no way for the two to disagree.
#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct StoredBinding {
    /// Credential fields keyed by the name the platform's spec gives them.
    #[serde(default)]
    pub values: BTreeMap<String, String>,
    /// Learned from the first inbound message; sending needs it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conversation: Option<String>,
    #[serde(default = "default_direct")]
    pub kind: StoredConversationKind,
}

/// Serde-friendly mirror of ConversationKind.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum StoredConversationKind {
    Group,
    Direct,
}

impl From<StoredConversationKind> for ConversationKind {
    fn from(kind: StoredConversationKind) -> Self {
        match kind {
            StoredConversationKind::Group => Self::Group,
            StoredConversationKind::Direct => Self::Direct,
        }
    }
}

impl From<ConversationKind> for StoredConversationKind {
    fn from(kind: ConversationKind) -> Self {
        match kind {
            ConversationKind::Group => Self::Group,
            ConversationKind::Direct => Self::Direct,
        }
    }
}

impl ImSettings {
    /// The stored configuration of one channel, if it has one.
    pub(crate) fn binding(&self, platform: Platform) -> Option<&StoredBinding> {
        self.channels.get(platform.key())
    }

    /// The stored configuration of one channel, if it has one.
    pub(crate) fn binding_mut(&mut self, platform: Platform) -> Option<&mut StoredBinding> {
        self.channels.get_mut(platform.key())
    }

    /// Replaces one channel's configuration, leaving the others alone.
    pub(crate) fn store_binding(&mut self, platform: Platform, binding: StoredBinding) {
        self.channels.insert(platform.key().to_string(), binding);
    }

    /// Drops one channel's configuration, leaving the others alone.
    pub(crate) fn remove_binding(&mut self, platform: Platform) {
        self.channels.remove(platform.key());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A channel configuration and the selection round-trip through JSON.
    #[test]
    fn round_trips_channels_and_the_selection() {
        let mut values = BTreeMap::new();
        values.insert("app_id".to_string(), "a".to_string());
        let mut other_values = BTreeMap::new();
        other_values.insert("client_id".to_string(), "c".to_string());
        let mut settings = ImSettings {
            channel: Some(Platform::DingTalk),
            ..ImSettings::default()
        };
        settings.store_binding(
            Platform::Qq,
            StoredBinding {
                values,
                conversation: Some("c".to_string()),
                kind: StoredConversationKind::Direct,
            },
        );
        settings.store_binding(
            Platform::DingTalk,
            StoredBinding {
                values: other_values,
                conversation: None,
                kind: StoredConversationKind::Group,
            },
        );

        let text = serde_json::to_string(&settings).expect("serialise");
        let back: ImSettings = serde_json::from_str(&text).expect("deserialise");

        assert_eq!(back.channel, Some(Platform::DingTalk));
        let qq = back.binding(Platform::Qq).expect("qq configuration");
        assert_eq!(qq.values.get("app_id").map(String::as_str), Some("a"));
        assert_eq!(qq.conversation.as_deref(), Some("c"));
        let dingtalk = back.binding(Platform::DingTalk).expect("dingtalk configuration");
        assert_eq!(dingtalk.kind, StoredConversationKind::Group);
        assert!(back.binding(Platform::Feishu).is_none());
    }

    /// Turning IM off keeps the stored channel configurations.
    #[test]
    fn off_keeps_the_configurations() {
        let mut settings = ImSettings::default();
        settings.store_binding(
            Platform::Feishu,
            StoredBinding {
                values: BTreeMap::new(),
                conversation: None,
                kind: StoredConversationKind::Direct,
            },
        );

        let text = serde_json::to_string(&settings).expect("serialise");
        let back: ImSettings = serde_json::from_str(&text).expect("deserialise");

        assert_eq!(back.channel, None);
        assert!(back.binding(Platform::Feishu).is_some());
    }
}
