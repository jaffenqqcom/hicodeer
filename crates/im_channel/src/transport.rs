//! The whole surface the host sees: send a text, receive a text.
//!
//! Nothing here knows what the messages are about. A text goes out, a text
//! comes in, and the host decides what it means.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

/// Where a text typed in the chat goes.
pub type InboundHandler = Arc<dyn Fn(String) + Send + Sync>;

static OUTBOUND: Mutex<Option<(u64, tokio::sync::mpsc::UnboundedSender<String>)>> = Mutex::new(None);
static OUTBOUND_GENERATION: AtomicU64 = AtomicU64::new(0);
static INBOUND: Mutex<Option<InboundHandler>> = Mutex::new(None);

/// Queues one text for the bound chat.
///
/// Silent when no channel is connected: callers are notification paths that
/// must never block or fail.
pub fn send_text(text: String) {
    log::info!("im channel: queueing a {} character message", text.chars().count());
    let Ok(guard) = OUTBOUND.lock() else {
        log::error!("im channel: the outbound queue lock is poisoned");
        return;
    };
    let Some((_, sender)) = guard.as_ref() else {
        log::debug!("im channel: no channel is connected; dropping a text");
        return;
    };
    if sender.send(text).is_err() {
        log::warn!("im channel: the outbound queue is closed");
    }
}

/// Replaces the handler that receives every text typed in the chat.
pub fn set_inbound_handler(handler: InboundHandler) {
    match INBOUND.lock() {
        Ok(mut slot) => *slot = Some(handler),
        Err(_) => log::error!("im channel: the inbound handler lock is poisoned"),
    }
}

/// Publishes the queue of the bridge that owns `generation`.
///
/// A bridge being replaced can reach this point after its successor has already
/// published, so an older generation is refused: the newer channel keeps the
/// queue.
pub(crate) fn install_outbound(generation: u64, sender: tokio::sync::mpsc::UnboundedSender<String>) {
    let newest = OUTBOUND_GENERATION.fetch_max(generation, Ordering::Relaxed);
    if newest > generation {
        log::debug!("im channel: ignoring an outbound queue from an older bridge");
        return;
    }
    match OUTBOUND.lock() {
        Ok(mut slot) => *slot = Some((generation, sender)),
        Err(_) => log::error!("im channel: the outbound queue lock is poisoned"),
    }
}

/// Drops the queue, but only when it still belongs to the given generation.
///
/// A bridge that is being replaced finishes after its successor has already
/// published a queue, so a blind clear would silence the new channel.
pub(crate) fn clear_outbound(generation: u64) {
    if let Ok(mut slot) = OUTBOUND.lock() {
        if slot.as_ref().is_some_and(|(current, _)| *current == generation) {
            *slot = None;
        }
    }
}

/// Hands one text typed in the chat to the host.
///
/// The host reads the agent list and prompts an agent, both of which block.
/// Running that here would stall the socket loop that has to keep acknowledging
/// the platform, so the handover goes to its own thread.
pub(crate) fn deliver_inbound(text: String) {
    let handler = INBOUND.lock().ok().and_then(|slot| slot.clone());
    let Some(handler) = handler else {
        log::debug!("im channel: no inbound handler is installed; dropping a text");
        return;
    };
    let spawn = std::thread::Builder::new()
        .name("im-inbound".to_string())
        .spawn(move || handler(text));
    if let Err(error) = spawn {
        log::warn!("im channel: cannot start the inbound task: {error}");
    }
}
