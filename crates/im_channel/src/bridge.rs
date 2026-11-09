//! Runs the bound channel: one connection in, one queue out.
//!
//! Nothing in this file knows what a message says. A text typed in the chat is
//! handed to the host through [`crate::transport`], and a text the host queues
//! there is sent to the bound chat.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use http_client::HttpClient;
use tokio::sync::mpsc;

use crate::binding::{ConversationKind, Platform};
use crate::config::{StoredBinding, StoredConversationKind};
use crate::platforms;
use crate::provider::{InboundProvider, IncomingMessage, OutgoingMessage};
use crate::store;
use crate::transport;

/// How often the loop looks at the stop flag while the connection is idle.
const IDLE_TICK: Duration = Duration::from_secs(1);
/// First pause before retrying a dropped long connection.
const RECONNECT_MIN: Duration = Duration::from_secs(5);
/// Ceiling for that pause; a chat app that is down should not be hammered.
const RECONNECT_MAX: Duration = Duration::from_secs(60);

/// Handle to the running bridge thread.
pub(crate) struct ImBridge {
    stop: Arc<AtomicBool>,
    connected: Arc<AtomicBool>,
    /// The inbound task, kept so switching channels can cancel it. Dropping
    /// the socket is the only way to wake a read that is already parked.
    inbound: Arc<Mutex<Option<tokio::task::JoinHandle<()>>>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl ImBridge {
    pub(crate) fn start(
        platform: Platform,
        binding: StoredBinding,
        generation: u64,
    ) -> anyhow::Result<Self> {
        let stop = Arc::new(AtomicBool::new(false));
        let connected = Arc::new(AtomicBool::new(false));
        let inbound = Arc::new(Mutex::new(None));
        let thread = std::thread::Builder::new()
            .name("im-bridge".to_string())
            .spawn({
                let stop = stop.clone();
                let connected = connected.clone();
                let inbound = inbound.clone();
                move || {
                    if let Err(error) = run(platform, binding, connected, stop, inbound, generation) {
                        log::error!("im channel failed: {error:#}");
                    }
                }
            })?;
        log::info!("im channel: started for {}", platform.key());
        Ok(Self {
            stop,
            connected,
            inbound,
            thread: Some(thread),
        })
    }

    /// Whether this bridge's long connection is up.
    pub(crate) fn is_connected(&self) -> bool {
        self.connected.load(Ordering::Relaxed)
    }

    pub(crate) fn stop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        self.connected.store(false, Ordering::Relaxed);
        // Switching channels has to close the old socket at once: while the
        // inbound loop is parked in a read it would otherwise keep receiving
        // messages.
        if let Ok(mut slot) = self.inbound.lock() {
            if let Some(handle) = slot.take() {
                handle.abort();
            }
        }
        // The thread may be parked in a socket read, so the handle is dropped
        // rather than joined: the UI must not wait on the network.
        self.thread.take();
        log::info!("im channel: stop requested");
    }
}

impl Drop for ImBridge {
    fn drop(&mut self) {
        self.stop();
    }
}

fn run(
    platform: Platform,
    binding: StoredBinding,
    connected: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
    inbound_slot: Arc<Mutex<Option<tokio::task::JoinHandle<()>>>>,
    generation: u64,
) -> anyhow::Result<()> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    runtime.block_on(async move {
        let kind: ConversationKind = binding.kind.into();
        let conversation = Arc::new(Mutex::new(
            binding.conversation.clone().map(|id| (id, kind)),
        ));
        let http: Arc<dyn HttpClient> = Arc::new(reqwest_client::ReqwestClient::new());
        let provider = build_provider(platform, &binding, http, connected.clone())?;

        let (texts, mut outbound) = mpsc::unbounded_channel::<String>();
        transport::install_outbound(generation, texts);
        let (sender, mut inbound) = mpsc::unbounded_channel::<IncomingMessage>();

        let inbound_task = tokio::spawn(run_inbound_forever(
            provider.clone(),
            sender,
            connected.clone(),
            stop.clone(),
        ));
        if let Ok(mut slot) = inbound_slot.lock() {
            *slot = Some(inbound_task);
        }

        let sending_provider = provider.clone();
        let sending_conversation = conversation.clone();
        let send_task = tokio::spawn(async move {
            while let Some(text) = outbound.recv().await {
                let target = match sending_conversation.lock() {
                    Ok(slot) => slot.clone(),
                    Err(_) => {
                        log::error!("im channel: the conversation lock is poisoned");
                        continue;
                    }
                };
                let Some((conversation_id, kind)) = target else {
                    log::warn!(
                        "im channel: {} has no chat yet; dropping a message",
                        platform.key()
                    );
                    continue;
                };
                let message = OutgoingMessage {
                    text,
                    conversation_id: Some(conversation_id),
                    kind,
                };
                if let Err(error) = sending_provider.send(message).await {
                    log::warn!("im channel: send failed: {error}");
                }
            }
        });

        loop {
            if stop.load(Ordering::Relaxed) {
                break;
            }
            tokio::select! {
                Some(message) = inbound.recv() => {
                    if let Ok(mut slot) = conversation.lock() {
                        *slot = Some((message.conversation_id.clone(), message.kind));
                    }
                    remember_conversation(platform, &message);
                    transport::deliver_inbound(message.text);
                }
                _ = tokio::time::sleep(IDLE_TICK) => {}
            }
        }

        send_task.abort();
        if let Ok(mut slot) = inbound_slot.lock() {
            if let Some(handle) = slot.take() {
                handle.abort();
            }
        }
        transport::clear_outbound(generation);
        Ok(())
    })
}

/// Keeps the long connection alive.
///
/// Every platform drops its socket eventually (token expiry, network change,
/// server restart) and none of them has a way to push to us again, so the
/// loop reopens it. The pause grows so a platform that stays down costs a
/// reconnect per minute rather than one per second.
async fn run_inbound_forever(
    provider: Arc<dyn InboundProvider>,
    sink: mpsc::UnboundedSender<IncomingMessage>,
    connected: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
) {
    let mut pause = RECONNECT_MIN;
    while !stop.load(Ordering::Relaxed) {
        let connected_at = std::time::Instant::now();
        match provider.run_inbound(sink.clone()).await {
            Ok(()) => log::info!("im channel: inbound connection closed"),
            Err(error) => log::warn!("im channel: inbound connection stopped: {error:#}"),
        }
        connected.store(false, Ordering::Relaxed);
        if stop.load(Ordering::Relaxed) {
            break;
        }
        // A connection that lasted counts as healthy, so the next failure
        // starts from the short pause again instead of the ceiling.
        if connected_at.elapsed() > RECONNECT_MAX {
            pause = RECONNECT_MIN;
        }
        tokio::time::sleep(pause).await;
        pause = (pause * 2).min(RECONNECT_MAX);
    }
}

fn build_provider(
    platform: Platform,
    binding: &StoredBinding,
    http: Arc<dyn HttpClient>,
    connection: Arc<AtomicBool>,
) -> anyhow::Result<Arc<dyn InboundProvider>> {
    let spec = platforms::spec(platform);
    Ok(spec.make_provider(
        http,
        &binding.values,
        binding.conversation.clone(),
        binding.kind.into(),
        connection,
    ))
}

/// Stores the conversation learned from inbound traffic, so sending keeps
/// working after a restart.
///
/// The file is read again instead of reusing the configuration this run
/// started with: another channel may have been configured since, and writing
/// the stale copy back would undo it.
fn remember_conversation(platform: Platform, message: &IncomingMessage) {
    let kind = StoredConversationKind::from(message.kind);
    let result = store::update(|stored| {
        let Some(binding) = stored.binding_mut(platform) else {
            return Ok(());
        };
        if binding.conversation.as_deref() == Some(message.conversation_id.as_str())
            && binding.kind == kind
        {
            return Ok(());
        }
        binding.conversation = Some(message.conversation_id.clone());
        binding.kind = kind;
        Ok(())
    });
    if let Err(error) = result {
        log::warn!("im channel: cannot remember the conversation: {error}");
    }
}
