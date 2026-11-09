//! Global owner of the running bridge, and the API the settings page uses.
//!
//! The settings page only knows platforms and field values, so the conversion
//! from typed-in text to stored configuration lives here. Per-platform
//! differences come from the registration table in [`crate::platforms`], so
//! nothing in this file names a platform.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use http_client::HttpClient;

use crate::binding::Platform;
use crate::bridge::ImBridge;
use crate::config::{ImSettings, StoredBinding, StoredConversationKind};
use crate::platforms::{self, CredentialValues};
use crate::registration::PollOutcome;
use crate::store;

static BRIDGE: Mutex<Option<ImBridge>> = Mutex::new(None);
/// Progress of a scan binding, shown by the settings page.
static SCAN: Mutex<Option<ScanState>> = Mutex::new(None);
/// Bumped for every attempt, so late results are attributable.
static SCAN_GENERATION: AtomicU64 = AtomicU64::new(0);
/// Bumped for every bridge start. A replaced bridge that is still starting up
/// must not publish its queue over the channel that replaced it.
static BRIDGE_GENERATION: AtomicU64 = AtomicU64::new(0);

/// Where a scan binding has got to.
#[derive(Clone, Debug)]
pub enum ScanStatus {
    /// Asking the platform for a verification URL.
    Starting,
    /// The page draws this text as a QR code; waiting for the user to confirm.
    Waiting { content: String },
    Failed(String),
}

#[derive(Clone, Debug)]
pub struct ScanState {
    pub platform: Platform,
    /// Identifies this attempt, so a cancelled scan cannot write over a newer
    /// one for the same platform.
    pub generation: u64,
    pub status: ScanStatus,
}

/// What the settings page shows for one channel.
#[derive(Clone, Debug)]
pub struct ChannelStatus {
    pub platform: Platform,
    /// Credential keys that already have a value; the page skips these when it
    /// offers to fill in what a scan could not return.
    pub provided: Vec<String>,
    /// True when every required credential is filled in.
    pub configured: bool,
    /// Whether a chat to post to is known. Sending needs one, and it is
    /// learned from the bot's first inbound message unless the platform
    /// reported the scanning user at bind time.
    pub has_conversation: bool,
}

/// What the settings page shows for the IM section.
#[derive(Clone, Debug, Default)]
pub struct ImStatus {
    /// The channel chosen to connect; `None` means IM is off.
    pub channel: Option<Platform>,
    /// Every channel that has stored credentials, in platform order.
    pub channels: Vec<ChannelStatus>,
    /// Whether the selected channel carries every credential it needs.
    pub configured: bool,
    /// Whether the selected channel's long connection is up.
    pub connected: bool,
}

/// Returns the stored channels plus the live connection flag.
pub fn status() -> anyhow::Result<ImStatus> {
    let settings = store::load()?.unwrap_or_default();
    Ok(describe(&settings, connected()))
}

/// Whether the bridge that is running now reports its long connection up.
fn connected() -> bool {
    BRIDGE
        .lock()
        .ok()
        .and_then(|bridge| bridge.as_ref().map(ImBridge::is_connected))
        .unwrap_or(false)
}

fn describe(settings: &ImSettings, connected: bool) -> ImStatus {
    let channel = settings.channel;
    let configured = channel.is_some_and(|platform| {
        settings
            .binding(platform)
            .is_some_and(|binding| is_configured(platform, binding))
    });
    ImStatus {
        channel,
        channels: Platform::ALL
            .iter()
            .filter_map(|&platform| {
                settings.binding(platform).map(|binding| ChannelStatus {
                    platform,
                    provided: binding
                        .values
                        .iter()
                        .filter(|(_, value)| !value.is_empty())
                        .map(|(key, _)| key.clone())
                        .collect(),
                    configured: is_configured(platform, binding),
                    has_conversation: binding.conversation.is_some(),
                })
            })
            .collect(),
        configured,
        connected: configured && connected,
    }
}

/// True when the binding carries every credential its platform needs.
///
/// Optional fields may be empty: the provider derives their value, such as
/// DingTalk's robot code from the client id.
fn is_configured(platform: Platform, binding: &StoredBinding) -> bool {
    platforms::spec(platform)
        .fields
        .iter()
        .filter(|field| !field.optional)
        .all(|field| {
            binding
                .values
                .get(field.key)
                .is_some_and(|value| !value.trim().is_empty())
        })
}

/// Selects the channel to connect, or turns IM off with `None`.
///
/// The stored configurations stay where they are: switching back to a channel
/// reuses what was typed in before.
pub fn set_channel(channel: Option<Platform>) -> anyhow::Result<()> {
    let settings = store::update(|settings| {
        settings.channel = channel;
        Ok(())
    })?;
    restart(&settings);
    let platform = channel.map(|platform| platform.key()).unwrap_or("off");
    log::info!("im channel: channel set to {platform}");
    Ok(())
}

/// Stores one channel's configuration, replacing what it had before.
pub fn bind(platform: Platform, values: CredentialValues) -> anyhow::Result<()> {
    bind_with_conversation(platform, values, None)
}

/// Stores one channel's configuration. `conversation` is the address a scan
/// reported for the person who bound the channel; when it is present the
/// bridge can post before the bot has received anything.
fn bind_with_conversation(
    platform: Platform,
    values: CredentialValues,
    conversation: Option<String>,
) -> anyhow::Result<()> {
    let settings = store::update(|settings| {
        // Re-saving credentials (a new secret, or DingTalk's robot code) must
        // not throw away the chat the bridge already learned. A different bot
        // identity does: that chat belongs to the bot being replaced.
        let previous = settings.binding(platform);
        let identity_key = platforms::spec(platform)
            .fields
            .first()
            .map(|field| field.key);
        let same_bot = match (previous, identity_key) {
            (Some(binding), Some(key)) => binding.values.get(key) == values.get(key),
            _ => false,
        };
        let kind = previous
            .map(|binding| binding.kind)
            .unwrap_or(StoredConversationKind::Direct);
        let conversation = conversation.clone().or_else(|| {
            if same_bot {
                previous.and_then(|binding| binding.conversation.clone())
            } else {
                None
            }
        });
        settings.store_binding(
            platform,
            StoredBinding {
                values,
                conversation,
                kind,
            },
        );
        Ok(())
    })?;
    restart_if_selected(&settings, platform);
    log::info!("im channel: saved the configuration for {}", platform.key());
    Ok(())
}

/// Removes one channel's configuration and stops the bridge when it was the
/// selected channel. The other channels keep their configuration.
pub fn unbind(platform: Platform) -> anyhow::Result<()> {
    let settings = store::update(|settings| {
        settings.remove_binding(platform);
        Ok(())
    })?;
    if settings.channel == Some(platform) {
        restart(&settings);
    }
    if scan_state().is_some_and(|scan| scan.platform == platform) {
        cancel_scan();
    }
    log::info!("im channel: removed the configuration for {}", platform.key());
    Ok(())
}

/// Fills in one credential after the fact, such as DingTalk's robot code, which
/// the device flow does not return.
pub fn set_field(platform: Platform, key: &str, value: String) -> anyhow::Result<()> {
    let settings = store::update(|settings| {
        let Some(binding) = settings.binding_mut(platform) else {
            anyhow::bail!("{} is not configured", platform.key());
        };
        binding.values.insert(key.to_string(), value);
        Ok(())
    })?;
    restart_if_selected(&settings, platform);
    Ok(())
}

/// Starts the bridge for the selected channel, if there is one.
///
/// Called once at startup so a selection survives an app restart.
pub fn resume() {
    match store::load() {
        Ok(Some(settings)) => restart(&settings),
        Ok(None) => {}
        Err(error) => log::error!("im channel: cannot read the stored configuration: {error}"),
    }
}

fn restart_if_selected(settings: &ImSettings, platform: Platform) {
    if settings.channel == Some(platform) {
        restart(settings);
    }
}

fn restart(settings: &ImSettings) {
    let Ok(mut guard) = BRIDGE.lock() else {
        log::error!("im channel: bridge lock is poisoned");
        return;
    };
    guard.take();
    let Some(platform) = settings.channel else {
        log::info!("im channel: no channel is selected");
        return;
    };
    let Some(binding) = settings.binding(platform) else {
        log::warn!(
            "im channel: {} is selected but has no configuration",
            platform.key()
        );
        return;
    };
    if !is_configured(platform, binding) {
        log::warn!(
            "im channel: {} is selected but its credentials are incomplete",
            platform.key()
        );
        return;
    }
    let generation = BRIDGE_GENERATION.fetch_add(1, Ordering::Relaxed) + 1;
    match ImBridge::start(platform, binding.clone(), generation) {
        Ok(bridge) => {
            *guard = Some(bridge);
            log::info!("im channel: running for {}", platform.key());
        }
        Err(error) => log::error!(
            "im channel: cannot start for {}: {error}",
            platform.key()
        ),
    }
}

/// Returns the scan in progress, if any. Cheap enough for a render path.
pub fn scan_state() -> Option<ScanState> {
    SCAN.lock().ok().and_then(|scan| scan.clone())
}

/// Dismisses the current scan. Its thread notices at the next tick and exits
/// without writing any state, so a cancelled scan can never bind.
pub fn cancel_scan() {
    if let Ok(mut scan) = SCAN.lock() {
        *scan = None;
    }
    log::info!("im channel: scan cancelled");
}

/// Starts a scan binding for \`platform\` on a worker thread.
///
/// Only platforms whose spec declares a device flow can scan; the caller offers
/// the external page for the others.
pub fn start_scan(platform: Platform) {
    if platforms::spec(platform).scan().is_none() {
        log::warn!(
            "im channel: {} has no device flow; use the platform page instead",
            platform.key()
        );
        return;
    }
    if scan_state().is_some_and(|scan| {
        scan.platform == platform
            && matches!(
                scan.status,
                ScanStatus::Starting | ScanStatus::Waiting { .. }
            )
    }) {
        log::warn!(
            "im channel: a scan for {} is already running",
            platform.key()
        );
        return;
    }
    let generation = SCAN_GENERATION.fetch_add(1, Ordering::Relaxed) + 1;
    if let Ok(mut scan) = SCAN.lock() {
        *scan = Some(ScanState {
            platform,
            generation,
            status: ScanStatus::Starting,
        });
    }
    let spawn = std::thread::Builder::new()
        .name("im-scan".to_string())
        .spawn(move || {
            let runtime = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(runtime) => runtime,
                Err(error) => {
                    fail_scan(
                        platform,
                        generation,
                        format!("cannot start the scan runtime: {error}"),
                    );
                    return;
                }
            };
            if let Err(error) = runtime.block_on(run_scan(platform, generation)) {
                fail_scan(platform, generation, format!("{error:#}"));
            }
        });
    if let Err(error) = spawn {
        fail_scan(
            platform,
            generation,
            format!("cannot start the scan thread: {error}"),
        );
    }
}

fn fail_scan(platform: Platform, generation: u64, message: String) {
    log::error!("im channel: scan for {} failed: {message}", platform.key());
    set_scan_status(platform, generation, ScanStatus::Failed(message));
}

/// Replaces the scan state only while it still belongs to this attempt.
fn set_scan_status(platform: Platform, generation: u64, status: ScanStatus) -> bool {
    if let Ok(mut scan) = SCAN.lock() {
        if scan
            .as_ref()
            .is_some_and(|scan| scan.platform == platform && scan.generation == generation)
        {
            *scan = Some(ScanState {
                platform,
                generation,
                status,
            });
            return true;
        }
    }
    false
}

fn is_current_scan(platform: Platform, generation: u64) -> bool {
    SCAN.lock().ok().is_some_and(|scan| {
        scan.as_ref()
            .is_some_and(|scan| scan.platform == platform && scan.generation == generation)
    })
}

async fn run_scan(platform: Platform, generation: u64) -> anyhow::Result<()> {
    let spec = platforms::spec(platform);
    let Some(flow) = spec.scan() else {
        anyhow::bail!("{} has no device flow", spec.display_name);
    };
    let client: Arc<dyn HttpClient> = Arc::new(reqwest_client::ReqwestClient::new());
    let started = (flow.begin)(client.clone()).await?;
    if !set_scan_status(
        platform,
        generation,
        ScanStatus::Waiting {
            content: started.verification_url.clone(),
        },
    ) {
        return Ok(());
    }
    log::info!(
        "im channel: scan for {} waiting for the user",
        platform.key()
    );

    let deadline = Instant::now() + started.expires_in;
    let mut interval = started.interval;
    while Instant::now() < deadline {
        if !is_current_scan(platform, generation) {
            return Ok(());
        }
        tokio::time::sleep(interval).await;
        let outcome = (flow.poll)(client.clone(), started.device_code.clone()).await?;
        match outcome {
            PollOutcome::Pending => {}
            PollOutcome::SlowDown => interval += Duration::from_secs(5),
            PollOutcome::Ready {
                client_id,
                client_secret,
                open_id,
            } => {
                if !is_current_scan(platform, generation) {
                    return Ok(());
                }
                bind_scanned(platform, client_id, client_secret, open_id)?;
                if let Ok(mut scan) = SCAN.lock() {
                    if scan.as_ref().is_some_and(|scan| {
                        scan.platform == platform && scan.generation == generation
                    }) {
                        *scan = None;
                    }
                }
                log::info!("im channel: scan for {} bound", platform.key());
                return Ok(());
            }
        }
    }
    anyhow::bail!("registration window expired before the scan was confirmed")
}

/// Stores the two credentials a device flow returns, under the names the
/// platform's spec declares. Any further field, such as DingTalk's robot code,
/// is filled in afterwards through [`set_field`].
fn bind_scanned(
    platform: Platform,
    client_id: String,
    client_secret: String,
    open_id: Option<String>,
) -> anyhow::Result<()> {
    let spec = platforms::spec(platform);
    let mut values = CredentialValues::new();
    if let Some(field) = spec.fields.first() {
        values.insert(field.key.to_string(), client_id);
    }
    if let Some(field) = spec.fields.get(1) {
        values.insert(field.key.to_string(), client_secret);
    }
    bind_with_conversation(platform, values, open_id)
}
