//! Platform-neutral message shapes and the adapter contract.
//!
//! Every platform implements [`ImProvider`]; the bridge never sees platform
//! specifics.

use futures::future::BoxFuture;

use crate::binding::{ConversationKind, Platform};

/// A message to send to the bound conversation.
#[derive(Clone, Debug)]
pub struct OutgoingMessage {
    pub text: String,
    /// Conversation learned from inbound traffic. `None` means the provider
    /// falls back to the one it was constructed with.
    pub conversation_id: Option<String>,
    /// Whether that conversation is a group or a one-to-one chat. Learned
    /// alongside the id, so a provider that addresses the two differently
    /// does not keep using the kind it was constructed with.
    pub kind: ConversationKind,
}

/// A message received from the bound conversation.
#[derive(Clone, Debug)]
pub struct IncomingMessage {
    pub conversation_id: String,
    /// Whether the message came from a group or a one-to-one chat.
    ///
    /// Platforms that address both through the same endpoint report
    /// [`ConversationKind::Group`] and ignore the field when sending.
    pub kind: ConversationKind,
    pub message_id: String,
    /// Id of the message this one replies to, when the platform carries it.
    pub reply_to: Option<String>,
    pub sender_id: String,
    pub text: String,
}

/// One IM platform.
pub trait ImProvider: Send + Sync {
    fn platform(&self) -> Platform;

    /// Sends one message. Fails when the platform rejects it.
    fn send(&self, message: OutgoingMessage) -> BoxFuture<'static, anyhow::Result<()>>;
}

/// The receiving half, which only exists where a long connection can run.
#[cfg(target_env = "ohos")]
pub trait InboundProvider: ImProvider {
    fn run_inbound(
        &self,
        sink: tokio::sync::mpsc::UnboundedSender<IncomingMessage>,
    ) -> BoxFuture<'static, anyhow::Result<()>>;
}
