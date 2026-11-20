//! Agent integration state shared between the editor's settings page and the
//! panel. The sound and notification switches now live in settings.json and
//! are read from `AgentPanelSettings`; this module only tracks the integration
//! targets the server reports.

use std::time::{Duration, Instant};

use gpui::{App, AppContext as _, BorrowAppContext as _, Global};
use settings::Settings as _;

use crate::client;

/// How long a completed integration check counts as current.
const INTEGRATION_CHECK_INTERVAL: Duration = Duration::from_secs(60);

/// Whether the editor presents agent notifications, as shown by the settings
/// switch. Only the two states the page produces are represented here.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToastDelivery {
    Off,
    System,
}

/// Installation state the server reports for one integration target.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IntegrationState {
    NotInstalled,
    Current,
    Outdated,
}

/// One integration target and what the server says about it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Integration {
    pub target: String,
    pub label: String,
    pub state: IntegrationState,
    pub needs_install: bool,
}

impl Integration {
    /// Whether the integration itself is installed and current.
    pub fn installed(&self) -> bool {
        self.state == IntegrationState::Current
    }
}

/// Everything the settings page shows about the agent terminal.
#[derive(Clone, Debug)]
pub struct AgentSettings {
    pub sound_enabled: bool,
    pub toast: ToastDelivery,
    pub auto_install_integrations: bool,
    pub show_history: bool,
    pub prevent_idle_sleep: bool,
    pub integrations: Vec<Integration>,
    pub integrations_checked: bool,
}

impl Default for AgentSettings {
    fn default() -> Self {
        Self {
            sound_enabled: true,
            toast: ToastDelivery::System,
            auto_install_integrations: true,
            show_history: true,
            prevent_idle_sleep: false,
            integrations: Vec::new(),
            integrations_checked: false,
        }
    }
}

/// Integration state shared with the panel and the settings page.
#[derive(Default)]
pub struct AgentIntegrations {
    pub entries: Vec<Integration>,
    pub checked_at: Option<Instant>,
    pub checking: bool,
}

impl Global for AgentIntegrations {}


/// Reads what the settings page shows. The sound and notification switches
/// come from `AgentPanelSettings` (settings.json); integrations come from the
/// server state shared with the panel.
pub fn snapshot(cx: &App) -> AgentSettings {
    let integrations = cx.global::<AgentIntegrations>();
    let panel = crate::AgentPanelSettings::get_global(cx);
    AgentSettings {
        sound_enabled: panel.sound_enabled,
        toast: if panel.notifications_enabled {
            ToastDelivery::System
        } else {
            ToastDelivery::Off
        },
        auto_install_integrations: panel.auto_install_integrations,
        show_history: panel.show_history,
        prevent_idle_sleep: panel.prevent_idle_sleep,
        integrations: integrations.entries.clone(),
        integrations_checked: integrations.checked_at.is_some(),
    }
}

/// Installs one integration target and refreshes the list afterwards.
pub fn install_integration(target: String, cx: &mut App) {
    {
        let state = cx.global::<AgentIntegrations>();
        if state.checking {
            return;
        }
    }
    cx.update_global::<AgentIntegrations, _>(|state, _| state.checking = true);
    cx.spawn(async move |cx| {
        let entries = cx
            .background_spawn(async move { client::install_integration(&target) })
            .await;
        cx.update(|cx| {
            cx.update_global::<AgentIntegrations, _>(|state, _| {
                state.checking = false;
                state.checked_at = Some(Instant::now());
                match entries {
                    Ok(entries) => state.entries = entries,
                    Err(error) => log::warn!("agent settings: install failed: {error}"),
                }
            });
            cx.refresh_windows();
        });
    })
    .detach();
}

/// Refreshes the integration list, installing whatever is missing when
/// `auto_install` is set.
pub fn refresh_integrations(cx: &mut App, auto_install: bool) {
    {
        let state = cx.global::<AgentIntegrations>();
        if state.checking {
            return;
        }
    }
    cx.update_global::<AgentIntegrations, _>(|state, _| state.checking = true);
    cx.spawn(async move |cx| {
        let entries = cx
            .background_spawn(async move { client::refresh_integrations(auto_install) })
            .await;
        cx.update(|cx| {
            cx.update_global::<AgentIntegrations, _>(|state, _| {
                state.checking = false;
                state.checked_at = Some(Instant::now());
                match entries {
                    Ok(entries) => state.entries = entries,
                    Err(error) => log::warn!("agent settings: integration check failed: {error}"),
                }
            });
            cx.refresh_windows();
        });
    })
    .detach();
}

/// Whether an automatic check is due, so the panel's refresh loop runs it
/// instead of doing it on every tick.
pub fn automatic_check_due(cx: &App) -> bool {
    let state = cx.global::<AgentIntegrations>();
    if state.checking {
        return false;
    }
    match state.checked_at {
        Some(checked_at) => checked_at.elapsed() >= INTEGRATION_CHECK_INTERVAL,
        None => true,
    }
}
