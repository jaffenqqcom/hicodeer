//! Agent panel: the GPUI front end for the agent server hosted by hicodeerd.
//!
//! The server's own sidebar is hidden (see its `ui.sidebar_start_collapsed`), so
//! this panel is where the user sees and drives spaces and agents. Its data is
//! the server's, read over the daemon's control bridge: each space is drawn
//! with its agents nested one level underneath, in the server's order, carrying
//! the server's own status for each agent.

pub mod agent_display;
mod client;
pub(crate) mod controller;
pub mod herdr_settings;
mod session_service;
mod snapshot;

use std::sync::Arc;
use std::time::{Duration, Instant};

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

#[cfg(target_env = "ohos")]
use audio::{Audio, Sound};
use editor::{Editor, actions::SelectAll};
use fs::Fs;
use gpui::{
    Action, AnyElement, App, AsyncWindowContext, Context, ElementId, Entity, EventEmitter,
    FocusHandle, Focusable, FontWeight, MouseButton, Pixels, Render, SharedString, Subscription,
    Task, TaskExt, WeakEntity, Window, actions, div, prelude::*, px,
};
use menu;
#[cfg(target_env = "ohos")]
use notifications::status_toast::StatusToast;
use settings::{IntoGpui, RegisterSetting, Settings, SettingsStore, update_settings_file};
use terminal_view::TerminalView;
use theme::ActiveTheme as _;
use ui::{
    Color, Icon, IconName, IconSize, Label, LabelCommon, LabelSize, Tooltip, prelude::*,
};
#[cfg(target_env = "ohos")]
use workspace::MultiWorkspace;
use workspace::{
    SaveIntent, Workspace,
    dock::{DockPosition, Panel, PanelEvent},
};

use crate::client::ClientError;
use crate::controller::AgentController as _;
use crate::session_service::{HistoryView, SessionKey, SessionService};
use crate::snapshot::{Agent, AgentStatus, Snapshot, Space};

/// Cadence of the refresh loop. The API path used here has no push channel for
/// agent status, so the tree is re-read at this interval: fast enough that a
/// status change reads as live, slow enough to be two idle requests.
const REFRESH_INTERVAL: Duration = Duration::from_millis(1000);
/// Refresh failures in a row that mean this panel is disconnected from the
/// server, rather than having seen one dropped request. At the refresh cadence
/// this is about three seconds, which outlasts a daemon restart or a device that
/// was briefly frozen.
const DISCONNECT_CLOSE_THRESHOLD: usize = 3;
/// Minimum gap between two attempts to add the Agent terminal. Deploying is
/// asynchronous, so without this a refresh that lands mid-deploy would start a
/// second terminal while the first is still being placed.
const DEPLOY_RETRY_INTERVAL: Duration = Duration::from_secs(2);
/// How long a started deploy may take to show up as a terminal before another
/// one is attempted. Adding the terminal goes through the project's terminal
/// task, which spawns the client process; on a slow device that can outlast the
/// retry interval, and retrying then would place a second tab next to one that
/// is still coming up.
const DEPLOY_SETTLE_TIMEOUT: Duration = Duration::from_secs(15);
/// How often the panel re-reads the server's launch state while a resumed
/// session is starting. The server reports `launch_pending` / `interactive_ready`
/// itself; this is only the poll cadence, not a deadline.
const START_POLL_INTERVAL: Duration = Duration::from_millis(500);
/// A hard cap on watching one launch, so a server that stops answering cannot
/// leave an observer running forever. The server's own start deadline is thirty
/// seconds, so anything still unresolved here is a broken endpoint, not a slow
/// agent.
const START_WATCH_LIMIT: Duration = Duration::from_secs(90);
/// How long a click may claim a row's highlight before the server's own state
/// has to confirm it. A refresh plus one slow request has to fit inside this,
/// so a dropped request does not leave the wrong row highlighted forever.
const PENDING_FOCUS_TIMEOUT: Duration = Duration::from_secs(5);
/// Indentation of an agent row under its space label.
const AGENT_INDENT: Pixels = px(16.0);
/// Width of the icon slot every row starts with, so status markers, header
/// icons, and labels all share one left edge.
const STATUS_SLOT_SIZE: Pixels = px(16.0);
/// Left and right padding of a row, matching the thread sidebar's entries.
const ROW_SIDE_PADDING: Pixels = px(6.0);
/// Name of the daemon the agent server runs in. It is a public HNP, so it is on
/// `PATH` in a normal terminal.
const AGENT_DAEMON_PROGRAM: &str = "hicodeerd";
/// Size of a space's label: one step above the header entries, which stay
/// small, so a space reads as the heading of its block.
const SPACE_LABEL_SIZE: LabelSize = LabelSize::Default;
/// Width used when neither the user's settings nor the packaged defaults give
/// the panel one. The OHOS defaults set the same value. It is one full-width
/// (CJK) character wider than upstream's 224px, so a Chinese label plus its
/// row padding does not get truncated.
const DEFAULT_PANEL_WIDTH: Pixels = px(238.0);
/// Key the onboarding flow writes once the user finishes it. Mirrors
/// `onboarding::FIRST_OPEN`; spelled out here so this crate does not depend on
/// the onboarding crate.
#[cfg(target_env = "ohos")]
const ONBOARDING_FIRST_OPEN_KEY: &str = "first_open";

// [OHOS PORT BEGIN] Until the first-run onboarding flow is completed the panel
// does not open itself, so the setup page owns the whole window. The status bar
// entry stays available, and the flag is read on demand rather than cached, so
// finishing onboarding in the same session takes effect immediately.
#[cfg(target_env = "ohos")]
fn hidden_until_onboarding() -> bool {
    !matches!(
        db::kvp::GlobalKeyValueStore::global().read_kvp(ONBOARDING_FIRST_OPEN_KEY),
        Ok(Some(_))
    )
}

#[cfg(not(target_env = "ohos"))]
fn hidden_until_onboarding() -> bool {
    false
}
// [OHOS PORT END]

actions!(
    agent_panel,
    [
        /// Toggles the Agent panel.
        Toggle,
        /// Focuses the Agent panel.
        ToggleFocus,
        /// Creates a new space and focuses it.
        NewSpace,
    ]
);

/// A row that was just clicked, claimed as focused before the server has
/// confirmed it. The panel reads this instead of waiting a whole refresh for
/// the highlight to move, which is what made the click feel lagged.
struct PendingFocus {
    /// Public pane id of the agent row.
    target: String,
    at: Instant,
}

/// A space whose label is being edited in place.
struct RenameTarget {
    workspace_id: String,
}

/// What the panel shows while it has no tree to draw.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Unavailable {
    WaitingForServer,
    ServerUnavailable,
}

/// A state transition worth telling the user about.
///
/// The server owns this rule (its `notification_toast_for_state_change`), but
/// the events built from it only reach a terminal client. The panel repeats
/// the rule instead of subscribing to those events.
///
/// `Done` is the server's "finished and not looked at" state, and it only
/// appears for a pane the server believes the user is away from. Here the
/// agent terminal is the only client, and it never reports losing focus, so
/// the server keeps considering the pane watched: a finished turn arrives as
/// plain `Idle` and `Done` is never sent. Treating `Idle` as the finish as
/// well is what makes the panel's own notification reachable at all; whether
/// the user is actually looking is decided separately, from the window.
#[cfg(target_env = "ohos")]
#[derive(Clone, Copy, PartialEq, Eq)]
enum AgentNotification {
    /// The agent is waiting for the user.
    NeedsAttention,
    /// The agent stopped working on its own.
    Finished,
}

#[cfg(target_env = "ohos")]
impl AgentNotification {
    fn from_transition(previous: AgentStatus, current: AgentStatus) -> Option<Self> {
        match (previous, current) {
            (previous, AgentStatus::Blocked) if previous != AgentStatus::Blocked => {
                Some(Self::NeedsAttention)
            }
            (AgentStatus::Working, AgentStatus::Done | AgentStatus::Idle) => {
                Some(Self::Finished)
            }
            (AgentStatus::Blocked, AgentStatus::Done) => Some(Self::Finished),
            _ => None,
        }
    }

    fn text(self) -> &'static str {
        match self {
            Self::NeedsAttention => localization::localized_str!("needs attention"),
            Self::Finished => localization::localized_str!("finished"),
        }
    }
}

/// Where the panel docks, whether its status bar button shows, and how wide it
/// opens. Written by the layout presets in the title bar as well as by dragging
/// the panel to another dock.
#[derive(Debug, Clone, PartialEq, RegisterSetting)]
pub struct AgentPanelSettings {
    pub button: bool,
    pub dock: DockPosition,
    pub default_width: Pixels,
    pub starts_open: bool,
    /// Whether agent integrations are installed as soon as the endpoint can
    /// answer for them. Written by the settings page.
    pub auto_install_integrations: bool,
    /// Whether the history view lists sessions scanned from the agents this
    /// build reads. Written by the settings page; the scan itself is skipped
    /// while this is off, so the daemon is not asked for a list nobody shows.
    pub show_history: bool,
}

impl Settings for AgentPanelSettings {
    fn from_settings(content: &settings::SettingsContent) -> Self {
        let panel = content.agent_panel.clone().unwrap_or_default();
        Self {
            button: panel.button.unwrap_or(true),
            dock: panel.dock.unwrap_or(settings::DockPosition::Left).into(),
            default_width: panel
                .default_width
                .map(IntoGpui::into_gpui)
                .unwrap_or(DEFAULT_PANEL_WIDTH),
            starts_open: panel.starts_open.unwrap_or(true),
            auto_install_integrations: panel.auto_install_integrations.unwrap_or(true),
            show_history: panel.show_history.unwrap_or(true),
        }
    }
}

pub fn init(cx: &mut App) {
    AgentPanelSettings::register(cx);
    #[cfg(target_env = "ohos")]
    {
        cx.set_global(herdr_settings::AgentIntegrations::default());
        cx.set_global(herdr_settings::TerminalSettingsCache::default());
        // Runs before the agent client can start, so a fresh install opens on
        // the terminal instead of the client's welcome screen.
        herdr_settings::disable_startup_onboarding();
    }

    cx.observe_new(|workspace: &mut Workspace, _, cx| {
        register(workspace, cx);
    })
    .detach();
}

/// Adds the panel's actions to a workspace.
fn register(workspace: &mut Workspace, _cx: &mut Context<Workspace>) {
    workspace.register_action(|workspace, _: &ToggleFocus, window, cx| {
        workspace.toggle_panel_focus::<AgentPanel>(window, cx);
    });
    workspace.register_action(|workspace, _: &Toggle, window, cx| {
        if !workspace.toggle_panel_focus::<AgentPanel>(window, cx) {
            workspace.close_panel::<AgentPanel>(window, cx);
        }
    });
    workspace.register_action(|workspace, _: &NewSpace, _window, cx| {
        if let Some(panel) = workspace.panel::<AgentPanel>(cx) {
            panel.update(cx, |panel, cx| panel.create_space(cx));
        }
    });
}

/// Whether the workspace currently holds the Agent terminal.
fn workspace_has_agent_terminal(workspace: &Workspace, cx: &App) -> bool {
    workspace.panes().iter().any(|pane| {
        pane.read(cx)
            .items_of_type::<TerminalView>()
            .any(|view| view.read(cx).is_agent_terminal())
    })
}

/// Closes every Agent terminal in the workspace, whether its client is still
/// running or the tab was left behind by a client that already exited.
fn close_agent_terminals(
    workspace: &mut Workspace,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) {
    let targets: Vec<_> = workspace
        .panes()
        .iter()
        .flat_map(|pane| {
            pane.read(cx)
                .items_of_type::<TerminalView>()
                .filter(|view| view.read(cx).is_agent_terminal())
                .map(|view| (pane.clone(), view))
                .collect::<Vec<_>>()
        })
        .collect();
    for (pane, view) in targets {
        // The tab refuses to close while it is pinned, which is what stops the
        // user from closing it; clearing the title is how its owner closes it.
        view.update(cx, |view, cx| view.clear_pinned_title(cx));
        let item_id = view.entity_id();
        pane.update(cx, |pane, cx| {
            pane.close_item_by_id(item_id, SaveIntent::Close, window, cx)
                .detach_and_log_err(cx);
        });
    }
}

/// A session this panel opened and is hiding from history until the server's
/// snapshot catches up. Tied to the pane that runs it, so the entry cannot
/// outlive the launch it describes.
struct OptimisticOpenSession {
    agent: String,
    session: String,
    pane_id: String,
}

pub struct AgentPanel {
    focus_handle: FocusHandle,
    workspace: WeakEntity<Workspace>,
    fs: Arc<dyn Fs>,
    spaces: Vec<Space>,
    unavailable: Option<Unavailable>,
    /// Refresh failures in a row. One is not a disconnection: a daemon restart
    /// or a frozen device drops a request and answers the next one.
    consecutive_failures: usize,
    /// When the terminal was last added, so a refresh during the add does not
    /// start another one.
    last_deploy_attempt: Option<std::time::Instant>,
    /// Whether the terminal should exist, as last decided by a refresh.
    wanted: bool,
    /// Daemon token the Agent terminal was last started against, or `None` when
    /// the panel has not started one in this run.
    ///
    /// The client is bound to the daemon run that started it: a daemon issues a
    /// new token every time it starts, and a client that outlives its daemon
    /// exits on the shutdown notice instead of reconnecting. Comparing the token
    /// is therefore what tells a live tab from one whose process is gone -- the
    /// tab itself still looks present, so nothing else in the workspace says so.
    deployed_agent_token: Option<String>,
    /// Whether the workspace has to be brought in line with `wanted`. Set by a
    /// refresh and acted on during render, which is where a window is available:
    /// an entity update also needs a window, and a workspace is not a window's
    /// root entity, so `update_in` on it fails.
    needs_sync: bool,
    /// Agent row that was clicked and is waiting for the server to confirm it.
    pending_agent: Option<PendingFocus>,
    /// Space whose label is being edited in place.
    renaming: Option<RenameTarget>,
    rename_editor: Option<Entity<Editor>>,
    rename_subscription: Option<Subscription>,
    _settings_subscription: Subscription,
    _refresh: Task<()>,
    /// Last state the server reported for each agent, keyed by its public pane
    /// id. The refresh has no change feed, so a transition is only visible by
    /// comparing one snapshot with the previous one.
    #[cfg(target_env = "ohos")]
    last_agent_status: HashMap<String, AgentStatus>,
    /// Whether a snapshot has been recorded. The first one fills the map
    /// without notifying: every agent in it would look newly changed.
    #[cfg(target_env = "ohos")]
    status_recorded: bool,
    /// Agents whose last transition still needs the user's attention, keyed by
    /// public pane id. The row shows a bell beside the state marker until the
    /// user opens that agent, from the row or from the notification toast.
    ///
    /// Unlike the sound and the toast, this mark has no setting: it is the
    /// panel's own record of which agent changed, so it stays visible when
    /// either notification is switched off.
    #[cfg(target_env = "ohos")]
    attention_agents: HashSet<String>,
    /// Whether the window was active as of the last activation change. A
    /// notification is skipped while the user is looking at the agent's own
    /// terminal.
    #[cfg(target_env = "ohos")]
    window_active: bool,
    /// Keeps the activation observer alive. Registered on the first render,
    /// because the panel has no window when it is constructed.
    #[cfg(target_env = "ohos")]
    window_activation: Option<gpui::Subscription>,
    /// History-session state store and view builder (design doc §5).
    session_service: SessionService,
    /// The history view as of the last session scan. `None` until the first
    /// scan answers.
    history: Option<HistoryView>,
    /// The last scan, kept so pin/archive/rename can rebuild the view without
    /// asking the daemon again.
    last_sessions: Vec<controller::RawSession>,
    /// Open sessions as of the last snapshot, `(agent, session value)` pairs.
    last_open_sessions: Vec<(String, String)>,
    /// Sessions this panel has just opened but the status snapshot does not
    /// report yet. Without them the row stays clickable between "open finished"
    /// and the next poll, so a second click starts the same session again.
    optimistic_open_sessions: Vec<OptimisticOpenSession>,
    /// Whether the last session scan failed. The history area shows an empty
    /// state with a retry hint instead of pretending there is no history.
    sessions_failed: bool,
    /// Session whose row is in the "opening" state.
    opening: Option<SessionKey>,
    /// History session whose title is being edited in place.
    history_renaming: Option<SessionKey>,
    history_rename_editor: Option<Entity<Editor>>,
    history_rename_subscription: Option<Subscription>,
    /// History group ids the user expanded. Empty means every group starts
    /// collapsed, which is how the panel first draws History and Archived.
    expanded_history_groups: HashSet<String>,
    /// Set while a history scan is in flight, so collapsing and expanding a
    /// group in quick succession does not queue two scans.
    history_loading: bool,
}

impl AgentPanel {
    /// Builds the panel for a workspace the same way the other dock panels are
    /// built, so `initialize_panels` can join it with the rest.
    pub async fn load(
        workspace: WeakEntity<Workspace>,
        mut cx: AsyncWindowContext,
    ) -> anyhow::Result<Entity<Self>> {
        workspace.update_in(&mut cx, |workspace, _window, cx| {
            let fs = workspace.app_state().fs.clone();
            cx.new(|cx| Self::new(workspace, fs, cx))
        })
    }

    pub fn new(workspace: &Workspace, fs: Arc<dyn Fs>, cx: &mut Context<Self>) -> Self {
        let refresh = cx.spawn(async move |this, cx| {
            loop {
                let fetched = if cmd_client::agent_control().is_some() {
                    cx.background_spawn(async { client::fetch() }).await
                } else {
                    Err(ClientError::NotReady)
                };
                if this.update(cx, |this, cx| this.apply(fetched, cx)).is_err() {
                    return;
                }
                cx.background_executor().timer(REFRESH_INTERVAL).await;
            }
        });

        // Data files live under a directory per feature, matching the rest of
        // the app's data dir (`config/`, `db/`, `logs/`, …).
        let state_path = paths::data_dir().join("agent").join("history.json");
        Self {
            focus_handle: cx.focus_handle(),
            workspace: workspace.weak_handle(),
            fs,
            spaces: Vec::new(),
            unavailable: Some(Unavailable::WaitingForServer),
            consecutive_failures: 0,
            last_deploy_attempt: None,
            wanted: false,
            deployed_agent_token: None,
            needs_sync: false,
            pending_agent: None,
            renaming: None,
            rename_editor: None,
            rename_subscription: None,
            _settings_subscription: cx.observe_global::<SettingsStore>(|_, cx| cx.notify()),
            _refresh: refresh,
            #[cfg(target_env = "ohos")]
            last_agent_status: HashMap::new(),
            #[cfg(target_env = "ohos")]
            status_recorded: false,
            #[cfg(target_env = "ohos")]
            attention_agents: HashSet::new(),
            #[cfg(target_env = "ohos")]
            window_active: false,
            #[cfg(target_env = "ohos")]
            window_activation: None,
            session_service: SessionService::load(state_path),
            history: None,
            last_sessions: Vec::new(),
            last_open_sessions: Vec::new(),
            optimistic_open_sessions: Vec::new(),
            sessions_failed: false,
            opening: None,
            history_renaming: None,
            history_rename_editor: None,
            history_rename_subscription: None,
            // The archive group is folded by design; everything else starts open.
            expanded_history_groups: HashSet::new(),
            history_loading: false,
        }
    }

    fn apply(&mut self, fetched: Result<Snapshot, ClientError>, cx: &mut Context<Self>) {
        match fetched {
            Ok(snapshot) => {
                #[cfg(target_env = "ohos")]
                self.notify_agent_transitions(&snapshot.spaces, cx);
                self.spaces = snapshot.spaces.clone();
                self.unavailable = None;
                self.consecutive_failures = 0;
                self.wanted = true;
                self.needs_sync = true;
                self.reconcile_pending_focus();
                #[cfg(target_env = "ohos")]
                self.ensure_integrations(cx);
                let open_sessions_changed = self.refresh_open_sessions(&snapshot);
                if open_sessions_changed && self.history.is_some() {
                    // The history view filters on which sessions are already
                    // open, so a status change can move a row between groups —
                    // but only rebuild when that set actually changed, not on
                    // every one-second status refresh.
                    self.rebuild_history();
                }
                // The history list is a separate endpoint from the status poll
                // and is not refreshed on a timer. It is primed here until the
                // first scan lands: right after startup the agent endpoint is
                // often not ready yet, and a single attempt would leave the
                // section empty for the rest of the session.
                if self.history.is_none() && !self.history_loading {
                    self.refresh_history(cx);
                }
            }
            Err(ClientError::NotReady) => {
                // No endpoint at all: the daemon went away and the bootstrap
                // cleared what it had reported. That is a disconnection just as
                // much as a refused connection is, and the terminal it left
                // behind is a dead tab, so it counts the same way.
                #[cfg(target_env = "ohos")]
                {
                    self.status_recorded = false;
                }
                self.unavailable = Some(Unavailable::WaitingForServer);
                self.note_disconnect();
                self.clear_history();
            }
            Err(ClientError::Failed(message)) => {
                // One line per outage: the refresh runs once a second, so
                // logging every failure would bury the first, useful error.
                if self.consecutive_failures == 0 {
                    log::warn!("agent panel: refresh failed: {message}");
                }
                // Whatever changed while the endpoint was unreachable is not
                // news the user needs one toast per agent for.
                #[cfg(target_env = "ohos")]
                {
                    self.status_recorded = false;
                }
                self.unavailable = Some(Unavailable::ServerUnavailable);
                // A failed request is not a disconnection: the daemon can be
                // busy (a history scan holds its main loop) or one read can
                // time out. Counting it here would close the agent terminal —
                // and kill the TUI running in it — because a request was slow,
                // so only `NotReady` counts toward that.
                self.consecutive_failures += 1;
                self.clear_history();
            }
        }
        cx.notify();
    }

    /// Turns one refresh into at most one notification.
    ///
    /// The server reports states, not transitions, so a change is only visible
    /// by comparing this snapshot with the previous one. The first snapshot
    /// after the panel starts only records state: without that, every agent in
    /// it would look newly changed.
    #[cfg(target_env = "ohos")]
    fn notify_agent_transitions(&mut self, spaces: &[Space], cx: &mut Context<Self>) {
        let mut current = HashMap::new();
        for space in spaces {
            for agent in &space.agents {
                current.insert(agent.pane_id.clone(), agent.status);
            }
        }
        let first_snapshot = !self.status_recorded;
        self.status_recorded = true;
        // An agent that left the list takes its mark with it, so a pane id
        // reused later does not inherit a stale bell.
        self.attention_agents
            .retain(|pane_id| current.contains_key(pane_id));
        let previous = std::mem::replace(&mut self.last_agent_status, current);

        herdr_settings::migrate_legacy_delivery(cx);
        if first_snapshot {
            return;
        }
        let notifications_enabled = herdr_settings::notifications_enabled(cx);

        let sound_enabled = herdr_settings::sound_enabled(cx);

        // One event feeds all three outputs: the row mark, the sound and the
        // toast. They share the transition test, and each adds only its own
        // switch, so a refresh carrying several events shows, rings and toasts
        // every one of them instead of letting one win.
        for space in spaces {
            for agent in &space.agents {
                // A new working turn means the user has taken the agent over:
                // they answered the block or sent the next prompt. The bell has
                // done its job, even when the agent was opened from the client's
                // own TUI rather than from this row.
                if agent.status == AgentStatus::Working {
                    self.attention_agents.remove(&agent.pane_id);
                }
                let Some(previous_status) = previous.get(&agent.pane_id).copied() else {
                    continue;
                };
                let Some(kind) = AgentNotification::from_transition(previous_status, agent.status)
                else {
                    continue;
                };
                // The state alone cannot tell a finished turn from one the
                // user stopped: both go Working -> Idle. The server marks real
                // completions on `completion_seq` and leaves it unset when the
                // turn was interrupted, so an unmarked finish is not an event.
                if kind == AgentNotification::Finished && agent.completion_seq.is_none() {
                    continue;
                }
                // The row mark needs no switch: it is the panel's own record of
                // which agent changed.
                self.attention_agents.insert(agent.pane_id.clone());
                if sound_enabled {
                    match kind {
                        AgentNotification::NeedsAttention => audio::play_system_bell(),
                        AgentNotification::Finished => {
                            Audio::play_sound(Sound::AgentDone, cx);
                        }
                    }
                }
                // The toast is skipped for the agent in the tab the user is
                // already looking at: its output is on screen, so a toast would
                // only repeat it.
                if !notifications_enabled || self.toast_is_silent(space, agent, cx) {
                    continue;
                }
                let text = format!(
                    "{} {}: {}",
                    client::display_name(&agent.pane_id, &agent.name),
                    kind.text(),
                    space.label
                );
                self.show_agent_notification(agent.pane_id.clone(), text, cx);
            }
        }
    }

    /// Whether a toast about this agent would duplicate what is on screen.
    ///
    /// The test is the space and tab the server has in front, not the pane:
    /// `Agent::focused` also requires the agent's own pane to hold pane focus,
    /// so it goes false when the user moved pane focus somewhere else in the
    /// same tab (a split, or the client's own sidebar) and the toast then
    /// repeats a tab the user is already watching.
    ///
    /// Any terminal in front counts, not only the fixed Agent terminal: an
    /// agent can also run in a terminal the user opened themselves, and that
    /// tab is still the agent's own output. Only when something else is in
    /// front -- a settings page, an editor -- does the news need a toast.
    #[cfg(target_env = "ohos")]
    fn toast_is_silent(&self, space: &Space, agent: &Agent, cx: &App) -> bool {
        if !self.window_active || !space.focused || space.active_tab_id != agent.tab_id {
            return false;
        }
        self.workspace.upgrade().is_some_and(|workspace| {
            workspace
                .read(cx)
                .active_item_as::<TerminalView>(cx)
                .is_some()
        })
    }

    /// Shows one agent notification whose action jumps to that agent.
    #[cfg(target_env = "ohos")]
    fn show_agent_notification(&mut self, pane_id: String, text: String, cx: &mut Context<Self>) {
        let Some(workspace) = self.workspace.upgrade() else {
            return;
        };
        let panel = cx.entity();
        // The toast's action runs after this call returns, so it owns its own
        // handle to the workspace.
        let toast_workspace = workspace.clone();
        let toast = StatusToast::new(text, cx, move |this, _cx| {
            this.icon(Icon::new(IconName::Bell).size(IconSize::Small))
                .action(localization::localized_str!("Open"), move |window, cx| {
                    let Some(handle) = window.window_handle().downcast::<MultiWorkspace>() else {
                        log::error!("agent panel: the window root is not a MultiWorkspace");
                        return;
                    };
                    cx.activate(true);
                    let pane_id = pane_id.clone();
                    let workspace = toast_workspace.clone();
                    let panel = panel.clone();
                    cx.defer(move |cx| {
                        if let Err(error) = handle.update(cx, |multi_workspace, window, cx| {
                            multi_workspace.activate(workspace.clone(), None, window, cx);
                            panel.update(cx, |panel, cx| {
                                panel.focus_agent(pane_id.clone(), window, cx);
                            });
                        }) {
                            log::error!(
                                "agent panel: cannot open the notification target: {error:?}"
                            );
                        }
                    });
                })
                .dismiss_button(true)
        });
        workspace.update(cx, |workspace, cx| {
            workspace.toggle_status_toast(toast, cx);
        });
    }

    /// Installs missing agent integrations once the endpoint can answer, and
    /// re-checks after a while so an agent installed later is picked up. The
    /// switch belongs to the settings page; this loop only reacts to it.
    #[cfg(target_env = "ohos")]
    fn ensure_integrations(&mut self, cx: &mut Context<Self>) {
        if !AgentPanelSettings::get_global(cx).auto_install_integrations {
            return;
        }
        if !herdr_settings::automatic_check_due(cx) {
            return;
        }
        herdr_settings::refresh_integrations(cx, true);
    }

    /// Drops a click claim once the server agrees with it, or once it is old
    /// enough that the server clearly will not. Without the timeout a focus
    /// request that failed would leave the click's row highlighted forever.
    fn reconcile_pending_focus(&mut self) {
        let agent_confirmed = self.pending_agent.as_ref().is_some_and(|pending| {
            pending.at.elapsed() >= PENDING_FOCUS_TIMEOUT
                || self
                    .spaces
                    .iter()
                    .flat_map(|space| space.agents.iter())
                    .any(|agent| agent.pane_id == pending.target && agent.focused)
        });
        if agent_confirmed {
            self.pending_agent = None;
        }
    }

    /// Counts one refresh that found no endpoint at all, closing the terminal
    /// once those form a run long enough to call it a disconnection.
    ///
    /// Only `NotReady` calls this: a request that merely failed or timed out is
    /// a busy server, not a gone one, and closing the tab over it would take a
    /// running TUI down with it.
    fn note_disconnect(&mut self) {
        self.consecutive_failures += 1;
        // Exactly at the threshold, not beyond it: the terminal is gone after
        // this, and repeating the close every second would only walk the panes
        // for nothing.
        if self.consecutive_failures == DISCONNECT_CLOSE_THRESHOLD {
            self.wanted = false;
            self.needs_sync = true;
        }
    }

    /// Hands the pending agent-terminal change to the next turn of the event
    /// loop.
    ///
    /// It cannot run inside render: the workspace is borrowed for the whole
    /// render pass, and updating it there would be a borrow conflict. A task
    /// started here runs after the pass, and a `spawn_in` context carries the
    /// window that is current now -- the one captured when the panel was loaded
    /// can already have been replaced during start-up.
    fn schedule_agent_terminal_sync(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.needs_sync {
            return;
        }
        self.needs_sync = false;
        cx.spawn_in(window, async move |this, cx| {
            if let Err(err) =
                this.update_in(cx, |this, window, cx| this.sync_agent_terminal(window, cx))
            {
                log::warn!("agent panel: could not sync the Agent terminal: {err:?}");
            }
        })
        .detach();
    }

    /// Brings the workspace in line with `wanted`, adding or closing the Agent
    /// terminal as needed.
    ///
    /// Runs during render because that is where a window is available: adding or
    /// closing a tab needs one, and a workspace cannot be updated through
    /// `update_in` -- it is not a window's root entity, so that path fails with
    /// "entity has no current window".
    ///
    /// The workspace is also still settling when the server first answers:
    /// restoring the previous session replaces the active pane's tabs, and a
    /// terminal added before that finishes is dropped with the pane it landed
    /// in, closing its pty and ending the TUI on stdin EOF. Re-checking on every
    /// refresh repairs that a couple of seconds later instead of leaving the tab
    /// missing for the rest of the session.
    fn sync_agent_terminal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(workspace) = self.workspace.upgrade() else {
            return;
        };

        // A daemon restart invalidates every client it was serving: a client
        // exits on the shutdown notice rather than reconnecting, and the pinned
        // tab it ran in stays behind looking exactly like a live one. The daemon
        // issues a new token on every run, so comparing it against the run the
        // terminal was started with is what detects that leftover.
        let Some(control) = cmd_client::agent_control() else {
            return;
        };

        let present = workspace_has_agent_terminal(workspace.read(cx), cx);
        if present && self.deployed_agent_token.as_deref() != Some(control.token.as_str()) {
            // The tab outlived the daemon that started it. Close it whichever way
            // `wanted` points: leaving it would either block the replacement or
            // keep a dead tab around when the panel wants no terminal at all.
            self.deployed_agent_token = None;
            workspace.update(cx, |workspace, cx| {
                close_agent_terminals(workspace, window, cx);
            });
            if self.wanted {
                self.needs_sync = true;
            }
            return;
        }

        if self.wanted == present {
            return;
        }

        if self.wanted {
            // Already started one for this daemon run: wait for it to land
            // instead of adding a second tab beside it.
            let deploy_still_settling = self.deployed_agent_token.as_deref()
                == Some(control.token.as_str())
                && self
                    .last_deploy_attempt
                    .is_some_and(|at| at.elapsed() < DEPLOY_SETTLE_TIMEOUT);
            if deploy_still_settling {
                self.needs_sync = true;
                return;
            }
            if let Some(last_attempt) = self.last_deploy_attempt
                && last_attempt.elapsed() < DEPLOY_RETRY_INTERVAL
            {
                // Let the next refresh try again rather than spinning here.
                self.needs_sync = true;
                return;
            }
            self.last_deploy_attempt = Some(std::time::Instant::now());
            self.deployed_agent_token = Some(control.token.clone());
            workspace.update(cx, |workspace, cx| {
                // The leftover tab keeps its pinned title, so the panel closes it
                // itself before adding the replacement; otherwise the pane would
                // hold two Agent terminals and only the new one would be live.
                close_agent_terminals(workspace, window, cx);
                TerminalView::deploy_agent_terminal(
                    workspace,
                    control.socket_path,
                    control.config_path,
                    window,
                    cx,
                )
                .detach_and_log_err(cx);
            });
        } else {
            self.deployed_agent_token = None;
            workspace.update(cx, |workspace, cx| {
                close_agent_terminals(workspace, window, cx);
            });
        }
    }

    /// Creates a space and focuses it, the way the server's own `new` entry does.
    fn create_space(&mut self, cx: &mut Context<Self>) {
        let workspace = self.workspace.clone();
        cx.spawn(async move |_this, cx| {
            let cwd = workspace
                .update(cx, |workspace, cx| {
                    workspace
                        .project()
                        .read(cx)
                        .worktrees(cx)
                        .next()
                        .map(|worktree| worktree.read(cx).abs_path().to_string_lossy().to_string())
                })
                .ok()
                .flatten();
            if let Err(err) =
                cx.background_spawn(async move { client::create_space(cwd.as_deref()) })
                    .await
            {
                log_client_error("create space", err);
            }
        })
        .detach();
    }

    /// Brings the Agent terminal to the front.
    ///
    /// The panel drives the TUI, so acting on a row has to make the result
    /// visible: the terminal can be connected the whole time while the user is
    /// looking at an editor tab, and the switch would otherwise happen out of
    /// sight.
    fn reveal_agent_terminal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(workspace) = self.workspace.upgrade() else {
            return;
        };
        workspace.update(cx, |workspace, cx| {
            let agent_view = workspace.panes().iter().find_map(|pane| {
                pane.read(cx)
                    .items_of_type::<TerminalView>()
                    .find(|view| view.read(cx).is_agent_terminal())
            });
            if let Some(view) = agent_view {
                workspace.activate_item(&view, true, true, window, cx);
            }
        });
    }

    /// Focuses one space, which switches the terminal to it. A space row never
    /// carries the highlight -- only the agent row inside it does -- so this
    /// claims nothing and waits for no confirmation.
    fn focus_space(&mut self, workspace_id: String, window: &mut Window, cx: &mut Context<Self>) {
        self.reveal_agent_terminal(window, cx);
        cx.spawn(async move |_this, cx| {
            if let Err(err) =
                cx.background_spawn(async move { client::focus_space(&workspace_id) })
                    .await
            {
                log_client_error("focus space", err);
            }
        })
        .detach();
    }

    /// Focuses one agent's pane, which also switches the space and tab it lives
    /// in. The row is claimed so the highlight moves on the click, not a refresh
    /// later.
    fn focus_agent(&mut self, pane_id: String, window: &mut Window, cx: &mut Context<Self>) {
        // Both ways of opening an agent -- its row and the notification toast
        // -- come through here, so this is the one place the mark is cleared.
        #[cfg(target_env = "ohos")]
        self.attention_agents.remove(&pane_id);
        self.reveal_agent_terminal(window, cx);
        self.pending_agent = Some(PendingFocus {
            target: pane_id.clone(),
            at: Instant::now(),
        });
        cx.notify();
        cx.spawn(async move |_this, cx| {
            if let Err(err) =
                cx.background_spawn(async move { client::focus_agent(&pane_id) })
                    .await
            {
                log_client_error("focus agent", err);
            }
        })
        .detach();
    }

    /// Starts editing one space's label in place. The editor is created here
    /// rather than in `render` so the row can draw it and route its confirm and
    /// cancel actions to `finish_renaming`.
    fn begin_rename(&mut self, workspace_id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(label) = self
            .spaces
            .iter()
            .find(|space| space.id == workspace_id)
            .map(|space| space.label.clone())
        else {
            return;
        };

        let editor = cx.new(|cx| Editor::single_line(window, cx));
        let subscription = cx.subscribe_in(&editor, window, {
            let editor = editor.clone();
            move |_this, _, event, window, cx| {
                if let editor::EditorEvent::Blurred = event {
                    // Focus moves while the click that caused it settles; check
                    // on the next turn rather than cancelling mid-gesture.
                    let editor = editor.clone();
                    cx.defer_in(window, move |this, window, cx| {
                        let still_current = this
                            .rename_editor
                            .as_ref()
                            .is_some_and(|current| current == &editor);
                        if still_current && !editor.focus_handle(cx).is_focused(window) {
                            this.finish_renaming(false, window, cx);
                        }
                    });
                }
            }
        });

        self.renaming = Some(RenameTarget {
            workspace_id: workspace_id.to_string(),
        });
        self.rename_editor = Some(editor.clone());
        self.rename_subscription = Some(subscription);

        editor.update(cx, |editor, cx| {
            editor.set_text(label, window, cx);
            editor.select_all(&SelectAll, window, cx);
            editor.focus_handle(cx).focus(window, cx);
        });
        cx.notify();
    }

    /// Ends the in-place rename, saving the edited label when asked to.
    fn finish_renaming(&mut self, save: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(editor) = self.rename_editor.take() else {
            return;
        };
        self.rename_subscription = None;
        let Some(target) = self.renaming.take() else {
            return;
        };

        if save {
            let label = editor.read(cx).text(cx).trim().to_string();
            // An empty label would erase the space's name; the server treats that as
            // "no change" too, so keep the old one.
            if !label.is_empty() {
                self.rename_space(target.workspace_id, label, cx);
            }
        }
        cx.notify();
        self.focus_handle.focus(window, cx);
    }

    fn rename_space(&self, workspace_id: String, label: String, cx: &mut Context<Self>) {
        cx.spawn(async move |_this, cx| {
            if let Err(err) = cx
                .background_spawn(async move { client::rename_space(&workspace_id, &label) })
                .await
            {
                log_client_error("rename space", err);
            }
        })
        .detach();
    }

    /// One space: its status marker and name. A space row is the container for
    /// its agents, so it never carries the selection highlight itself.
    ///
    /// A right click opens the same two entries the server offers on a space:
    /// rename and close. Rename edits the label in place, in the row itself.
    fn render_space(&self, space: &Space, cx: &mut Context<Self>) -> AnyElement {
        let id = space.id.clone();
        let rename_editor = if self
            .renaming
            .as_ref()
            .is_some_and(|target| target.workspace_id == space.id)
        {
            self.rename_editor.clone()
        } else {
            None
        };
        let panel = cx.entity();
        let label = space.label.clone();

        let title: AnyElement = if let Some(editor) = rename_editor {
            let confirm_panel = panel.clone();
            let cancel_panel = panel.clone();
            // The label stays in place, transparent, so the row keeps its size;
            // the editor is painted over the same box. An editor that has to lay
            // itself out inside a fixed-height flex slot renders blank here, so
            // it is given a full-size overlay instead.
            div()
                .id(ElementId::Name(SharedString::from(format!(
                    "agent-space-rename-{}",
                    space.id
                ))))
                .relative()
                .flex_1()
                .h_6()
                // Clicking into the label being edited must not also count as
                // clicking the row, which would switch the space.
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_click(|_, _, cx| cx.stop_propagation())
                .child(
                    Label::new(label)
                        .size(SPACE_LABEL_SIZE)
                        .weight(FontWeight::BOLD)
                        .single_line()
                        .alpha(0.),
                )
                .child(
                    div()
                        .absolute()
                        .top_0()
                        .left_0()
                        .size_full()
                        .child(editor)
                        .on_action(move |_: &menu::Confirm, window, cx| {
                            let _ = confirm_panel
                                .update(cx, |panel, cx| panel.finish_renaming(true, window, cx));
                        })
                        .on_action(move |_: &menu::Cancel, window, cx| {
                            let _ = cancel_panel
                                .update(cx, |panel, cx| panel.finish_renaming(false, window, cx));
                        }),
                )
                .into_any_element()
        } else {
            Label::new(label)
                .size(SPACE_LABEL_SIZE)
                .weight(FontWeight::BOLD)
                .into_any_element()
        };

        let row_id = SharedString::from(format!("agent-space-{}", space.id));
        row_container(ElementId::Name(row_id.clone()), px(0.0), false, cx)
            .group(row_id.clone())
            .child(
                h_flex()
                    .relative()
                    .w_full()
                    .gap_1p5()
                    .child(space_icon_slot())
                    .child(h_flex().flex_1().min_w_0().h_6().child(title))
                    // Same affordance as the history rows: the actions appear
                    // on hover at the row's right edge instead of behind a
                    // context menu.
                    .child(
                        h_flex()
                            .absolute()
                            .right_0()
                            .top_0()
                            .bottom_0()
                            .items_center()
                            .gap_0p5()
                            .px_1()
                            .bg(cx.theme().colors().ghost_element_hover)
                            .visible_on_hover(row_id.clone())
                            .child(
                                IconButton::new(
                                    ElementId::Name(SharedString::from(format!(
                                        "agent-space-rename-button-{}",
                                        space.id
                                    ))),
                                    IconName::Pencil,
                                )
                                .icon_size(IconSize::Small)
                                .icon_color(Color::Muted)
                                .tooltip(move |_, cx| {
                                    Tooltip::simple(localization::localized_str!("Rename"), cx)
                                })
                                .on_click({
                                    let panel = panel.clone();
                                    let target = space.id.clone();
                                    move |_, window, cx| {
                                        let _ = panel.update(cx, |panel, cx| {
                                            panel.begin_rename(&target, window, cx);
                                        });
                                    }
                                }),
                            )
                            .child(
                                IconButton::new(
                                    ElementId::Name(SharedString::from(format!(
                                        "agent-space-close-button-{}",
                                        space.id
                                    ))),
                                    IconName::Trash,
                                )
                                .icon_size(IconSize::Small)
                                .icon_color(Color::Muted)
                                .tooltip(move |_, cx| {
                                    Tooltip::simple(localization::localized_str!("Close"), cx)
                                })
                                .on_click({
                                    let panel = panel.clone();
                                    let target = space.id.clone();
                                    move |_, _, cx| {
                                        let _ = panel.update(cx, |panel, cx| {
                                            panel.close_space(target.clone(), cx);
                                        });
                                    }
                                }),
                            ),
                    ),
            )
            .on_click({
                let panel = panel.clone();
                move |_, window, cx| {
                    let _ = panel.update(cx, |panel, cx| {
                        panel.focus_space(id.clone(), window, cx);
                    });
                }
            })
            .into_any_element()
    }

    /// One agent, drawn one level under the space it works in. Only this row
    /// carries the selection highlight: the space above it is a heading.
    fn render_agent_row(&self, agent: &Agent, cx: &mut Context<Self>) -> AnyElement {
        let pane_id = agent.pane_id.clone();
        // A claimed row owns the highlight, so the one that was lit from the
        // previous snapshot goes dark on this frame instead of a second later.
        let focused = match self.pending_agent.as_ref() {
            Some(pending) => pending.target == agent.pane_id,
            None => agent.focused,
        };
        let panel = cx.entity();
        let (marker, marker_color) = status_marker(agent.status);

        row_container(
            ElementId::Name(SharedString::from(format!("agent-row-{}", agent.id))),
            AGENT_INDENT,
            focused,
            cx,
        )
        .child(
            h_flex()
                .w_full()
                .gap_1p5()
                // The bell sits at the row's leading edge, before the state
                // marker, so the eye lands on it before reading the row.
                .when(self.agent_needs_attention(&agent.pane_id), |this| {
                    this.child(
                        Icon::new(IconName::BellRing)
                            .size(IconSize::Small)
                            .color(Color::Warning),
                    )
                })
                .child(status_slot(marker, marker_color))
                .when(!agent.kind.is_empty(), |this| {
                    this.child(
                        Icon::new(agent_display::agent_icon(&agent.kind))
                            .size(IconSize::Small)
                            .color(Color::Muted),
                    )
                })
                .child(
                    h_flex()
                        .flex_1()
                        .min_w_0()
                        .h_6()
                        .child(Label::new(agent_display::capitalize(client::display_name(
                            &agent.pane_id,
                            &agent.name,
                        )))),
                ),
        )
        .on_click(move |_, window, cx| {
            let _ = panel.update(cx, |panel, cx| panel.focus_agent(pane_id.clone(), window, cx));
        })
        .into_any_element()
    }

    /// Whether this agent's row should wear the attention bell.
    ///
    /// Kept separate from the status marker: the marker describes what the
    /// server reports now, while the bell says the row has not been opened
    /// since its last transition.
    fn agent_needs_attention(&self, pane_id: &str) -> bool {
        #[cfg(target_env = "ohos")]
        {
            self.attention_agents.contains(pane_id)
        }
        #[cfg(not(target_env = "ohos"))]
        {
            let _ = pane_id;
            false
        }
    }

    // ------------------------------------------------------------------
    // History sessions (design doc §5–§7)
    // ------------------------------------------------------------------

    /// Keeps the "which sessions are already open" bookkeeping the history view
    /// filters on. Runs on every status refresh, does no I/O, and reports
    /// whether anything the history view depends on actually moved.
    ///
    /// Only the open set matters: group names come from the session's own
    /// folder, so a space being created, closed or renamed does not change any
    /// group header.
    fn refresh_open_sessions(&mut self, snapshot: &Snapshot) -> bool {
        let reported: Vec<(String, String)> = snapshot
            .spaces
            .iter()
            .flat_map(|space| space.agents.iter().filter_map(|agent| agent.agent_session.clone()))
            .collect();
        let live_panes: Vec<&str> = snapshot
            .spaces
            .iter()
            .flat_map(|space| space.agents.iter().map(|agent| agent.pane_id.as_str()))
            .collect();
        // Keep the optimism only while it can still come true: drop it once the
        // session is reported, and also once its pane is gone from the list —
        // a launch that died before its first report must not hide the history
        // row for the rest of the session.
        self.optimistic_open_sessions.retain(|entry| {
            let reported_now = reported
                .iter()
                .any(|(agent, value)| agent == &entry.agent && value == &entry.session);
            let pane_lives = live_panes.iter().any(|pane_id| *pane_id == entry.pane_id);
            !reported_now && pane_lives
        });
        let mut open_sessions = reported;
        for entry in &self.optimistic_open_sessions {
            let pair = (entry.agent.clone(), entry.session.clone());
            if !open_sessions.contains(&pair) {
                open_sessions.push(pair);
            }
        }
        let changed = open_sessions != self.last_open_sessions;
        self.last_open_sessions = open_sessions;
        changed
    }

    /// Pulls the history list once.
    ///
    /// This is a separate endpoint from the once-a-second status poll and is
    /// deliberately not polled: the list only changes when an agent writes a
    /// session file, not while the panel sits open. The scan itself is cheap on
    /// the daemon side (an indexed stat walk unless a file changed).
    fn refresh_history(&mut self, cx: &mut Context<Self>) {
        if self.history_loading
            || cmd_client::agent_control().is_none()
            || !AgentPanelSettings::get_global(cx).show_history
        {
            return;
        }
        self.history_loading = true;
        cx.spawn(async move |this, cx| {
            let scan = cx.background_spawn(async { client::fetch_sessions() }).await;
            let _ = this.update(cx, |this, cx| {
                this.history_loading = false;
                this.apply_history_scan(scan, cx);
            });
        })
        .detach();
    }

    /// Drops the history view when the endpoint is gone. The rows describe
    /// sessions on the other side of a connection that no longer exists, and
    /// every one of them would fail to open, so they are removed rather than
    /// left as a dead list. A later successful refresh primes it again.
    fn clear_history(&mut self) {
        if self.history.is_some() || !self.last_sessions.is_empty() {
            self.history = None;
            self.last_sessions.clear();
        }
        self.history_loading = false;
        self.opening = None;
    }

    /// Applies one scan answer. Runs only when a scan actually landed; every
    /// other refresh keeps the previous view.
    fn apply_history_scan(
        &mut self,
        scan: Result<Vec<controller::RawSession>, ClientError>,
        cx: &mut Context<Self>,
    ) {
        match scan {
            Ok(list) => {
                self.last_sessions = list;
                self.sessions_failed = false;
                self.rebuild_history();
            }
            Err(err) => {
                if !self.sessions_failed {
                    log::warn!("agent panel: session scan failed: {err}");
                }
                self.sessions_failed = true;
            }
        }
        cx.notify();
    }

    /// Rebuilds the view from the cached scan plus current state. Called after
    /// a scan and after every pin/archive/rename, so the row moves instantly.
    fn rebuild_history(&mut self) {
        let view = self.session_service.history(
            self.last_sessions.clone(),
            &self.last_open_sessions,
        );
        self.history = Some(view);
    }

    /// Opens one history session in a fresh space (creating it at the
    /// session's root first when needed). A session whose store never recorded a
    /// root is sent without one, and the server picks the directory itself.
    ///
    /// `agent.start` succeeding does not mean the CLI actually restored the
    /// session (a corrupt store only shows inside the TUI), so the spawn is
    /// followed by a short watch: if the agent does not report that session
    /// within ten seconds, the user gets a toast instead of silence.
    fn open_history_session(&mut self, key: SessionKey, cx: &mut Context<Self>) {
        // One session is opened at a time. A second click while that open is in
        // flight would create another space and start the same session twice.
        if self.opening.is_some() {
            return;
        }
        let Some(view) = self.history.as_ref() else {
            return;
        };
        let Some(entry) = view.find(&key).cloned() else {
            return;
        };
        // Which agents can resume is the server's call: it owns the per-agent
        // resume command and answers `resume_unsupported` for the rest.
        let kind = key.agent.clone();
        self.opening = Some(key.clone());
        let opened_key = (key.agent.clone(), key.session_id.clone());
        cx.notify();

        let request = controller::OpenSessionRequest {
            workspace_root: entry.workspace_root.as_ref().map(PathBuf::from),
            agent_kind: kind,
            session_id: key.session_id.clone(),
        };
        cx.spawn(async move |this, cx| {
            let opened = cx
                .background_spawn(async move {
                    controller::HerdrController.open_session(request)
                })
                .await;
            match opened {
                Ok(opened) => {
                    // The server owns the start state. It reports
                    // `launch_pending` until the agent reaches its prompt, and
                    // drops the agent from the list once its own deadline
                    // passes. The panel follows that instead of timing the start
                    // itself: a slow CLI start is not a failed resume, and the
                    // server already knows how long to wait.
                    let watched_pane = opened.pane_id.clone();
                    let started_at = Instant::now();
                    let mut failed = false;
                    loop {
                        if started_at.elapsed() > START_WATCH_LIMIT {
                            // Only a server that never answers back can get
                            // here; its own deadline is well inside this one.
                            failed = true;
                            break;
                        }
                        cx.background_executor()
                            .timer(START_POLL_INTERVAL)
                            .await;
                        let snapshot = cx.background_spawn(async { client::fetch() }).await;
                        let Ok(snapshot) = snapshot else {
                            // No endpoint: the refresh loop is already tearing
                            // the history view down, so stop watching.
                            break;
                        };
                        let agent = snapshot
                            .spaces
                            .iter()
                            .flat_map(|space| space.agents.iter())
                            .find(|agent| agent.pane_id == watched_pane);
                        match agent {
                            // The only success signal the server reports.
                            Some(agent) if agent.interactive_ready => break,
                            // Still starting, by the server's own clock.
                            Some(agent) if agent.launch_pending => continue,
                            // Present but no longer managed (a restored
                            // session, for instance): nothing left to follow.
                            Some(_) => break,
                            // The server gave up on this launch and released the
                            // name, which is how it reports a failed start.
                            None => {
                                failed = true;
                                break;
                            }
                        }
                    }
                    let _ = this.update(cx, |this, cx| {
                        this.opening = None;
                        if !failed {
                            // Hide the row now: the next status poll would only
                            // do it a moment later, and a click in between used
                            // to start the same session a second time. The entry
                            // follows the pane, so it is dropped as soon as that
                            // pane stops being reported.
                            let entry = OptimisticOpenSession {
                                agent: opened_key.0.clone(),
                                session: opened_key.1.clone(),
                                pane_id: watched_pane.clone(),
                            };
                            if !this
                                .optimistic_open_sessions
                                .iter()
                                .any(|open| open.pane_id == entry.pane_id)
                            {
                                this.optimistic_open_sessions.push(entry);
                            }
                            if !this.last_open_sessions.contains(&opened_key) {
                                this.last_open_sessions.push(opened_key.clone());
                            }
                            this.rebuild_history();
                        }
                        if failed {
                            log::warn!(
                                "agent panel: session in pane {watched_pane} never became ready"
                            );
                            this.show_history_toast(
                                localization::localized_str!("Session could not be resumed"),
                                None,
                                cx,
                            );
                        }
                        // Focus the new agent and bring the terminal forward.
                        this.pending_agent = Some(PendingFocus {
                            target: opened.pane_id.clone(),
                            at: Instant::now(),
                        });
                    });
                }
                Err(err) => {
                    log::warn!("agent panel: opening a history session failed: {err}");
                    let _ = this.update(cx, |this, cx| {
                        this.opening = None;
                        this.show_history_toast(
                            &format!(
                                "{}: {err}",
                                localization::localized_str!("Failed to open session")
                            ),
                            None,
                            cx,
                        );
                    });
                }
            }
        })
        .detach();
    }

    fn toggle_history_pin(&mut self, key: SessionKey, pinned: bool, cx: &mut Context<Self>) {
        if let Err(err) = self.session_service.pin(&key, pinned) {
            log::error!("agent panel: cannot pin session: {err}");
            self.show_history_toast(
                localization::localized_str!("Failed to pin session"),
                None,
                cx,
            );
            return;
        }
        self.rebuild_history();
        cx.notify();
    }

    fn archive_history_session(
        &mut self,
        key: SessionKey,
        archived: bool,
        cx: &mut Context<Self>,
    ) {
        if let Err(err) = self.session_service.archive(&key, archived) {
            log::error!("agent panel: cannot archive session: {err}");
            self.show_history_toast(
                localization::localized_str!("Failed to archive session"),
                None,
                cx,
            );
            return;
        }
        self.rebuild_history();
        let message = if archived {
            localization::localized_str!("Moved to archived")
        } else {
            localization::localized_str!("Restored from archived")
        };
        self.show_history_toast(message, None, cx);
        cx.notify();
    }

    fn begin_history_rename(&mut self, key: SessionKey, window: &mut Window, cx: &mut Context<Self>) {
        let Some(view) = self.history.as_ref() else {
            return;
        };
        let Some(entry) = view.find(&key) else {
            return;
        };
        let title = entry.title.clone();

        let editor = cx.new(|cx| Editor::single_line(window, cx));
        let subscription = cx.subscribe_in(&editor, window, {
            let editor = editor.clone();
            move |_this, _, event, window, cx| {
                if let editor::EditorEvent::Blurred = event {
                    let editor = editor.clone();
                    cx.defer_in(window, move |this, window, cx| {
                        let still_current = this
                            .history_rename_editor
                            .as_ref()
                            .is_some_and(|current| current == &editor);
                        if still_current && !editor.focus_handle(cx).is_focused(window) {
                            this.finish_history_rename(false, window, cx);
                        }
                    });
                }
            }
        });
        self.history_renaming = Some(key);
        self.history_rename_editor = Some(editor.clone());
        self.history_rename_subscription = Some(subscription);
        editor.update(cx, |editor, cx| {
            editor.set_text(title, window, cx);
            editor.select_all(&SelectAll, window, cx);
            editor.focus_handle(cx).focus(window, cx);
        });
        cx.notify();
    }

    fn finish_history_rename(
        &mut self,
        save: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(editor) = self.history_rename_editor.take() else {
            return;
        };
        self.history_rename_subscription = None;
        let Some(key) = self.history_renaming.take() else {
            return;
        };
        if save {
            let title = editor.read(cx).text(cx).trim().to_string();
            if !title.is_empty() {
                if let Err(err) = self
                    .session_service
                    .rename(&key, Some(title.clone()))
                {
                    log::error!("agent panel: cannot rename session: {err}");
                } else {
                    self.rebuild_history();
                }
            }
        }
        cx.notify();
        self.focus_handle.focus(window, cx);
    }

    /// One status toast from the history view. The panel shares the
    /// workspace's single toast slot with the agent notifications.
    fn show_history_toast(&mut self, text: &str, _cx_kind: Option<()>, cx: &mut Context<Self>) {
        #[cfg(target_env = "ohos")]
        {
            let Some(workspace) = self.workspace.upgrade() else {
                return;
            };
            let toast = StatusToast::new(text.to_owned(), cx, |this, _cx| {
                this.dismiss_button(true)
            });
            workspace.update(cx, |workspace, cx| {
                workspace.toggle_status_toast(toast, cx);
            });
        }
        #[cfg(not(target_env = "ohos"))]
        {
            log::info!("agent panel: {text}");
        }
    }

    /// Deletes one session from the agent's own store and from the list.
    ///
    /// The daemon does the unlinking: an app process gets `EPERM` removing a
    /// file from the agent's store. The row only disappears once the daemon
    /// confirms it, so a refused delete leaves the list honest.
    ///
    /// The path comes from the row that was clicked rather than from a lookup
    /// in the last scan: a scan that landed between the render and the click
    /// would otherwise make the button do nothing at all.
    fn delete_history_session(
        &mut self,
        key: SessionKey,
        source_path: String,
        cx: &mut Context<Self>,
    ) {
        let agent = key.agent.clone();
        cx.spawn(async move |this, cx| {
            let deleted = cx
                .background_spawn(async move { client::delete_session(&agent, &source_path) })
                .await;
            let _ = this.update(cx, |this, cx| {
                match deleted {
                    Ok(()) => {
                        this.last_sessions.retain(|session| {
                            !(session.agent == key.agent && session.session_id == key.session_id)
                        });
                        if let Err(error) = this.session_service.forget(&key) {
                            log::warn!(
                                "agent panel: dropping deleted session state failed: {error}"
                            );
                        }
                        this.rebuild_history();
                        this.show_history_toast(
                            localization::localized_str!("Session deleted"),
                            None,
                            cx,
                        );
                    }
                    Err(error) => {
                        log::error!("agent panel: deleting session failed: {error}");
                        this.show_history_toast(
                            localization::localized_str!("Failed to delete session"),
                            None,
                            cx,
                        );
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// The history area: separator, groups, rows. Rendered under the tree of
    /// open spaces inside the same scroll region.
    fn render_history(&mut self, cx: &mut Context<Self>) -> AnyElement {
        // The settings page can take the whole history view away. Checked before
        // the cached view so a stale scan cannot draw after the switch is off.
        if !AgentPanelSettings::get_global(cx).show_history {
            return div().into_any_element();
        }
        // Cloned so the per-group renderers can take `&mut self`.
        let Some(view) = self.history.clone() else {
            // No scan has landed yet. A failure is the only thing worth saying
            // here: returning early skips the per-tree blocks below, so the
            // message cannot be emitted from down there.
            if self.sessions_failed {
                return v_flex()
                    .w_full()
                    .pt_3()
                    .border_t_1()
                    .border_color(cx.theme().colors().border)
                    .child(
                        Label::new(localization::localized_str!(
                            "History could not be loaded - retrying"
                        ))
                        .size(LabelSize::Small)
                        .color(Color::Muted),
                    )
                    .into_any_element();
            }
            return div().into_any_element();
        };

        // Two trees: "History" (pinned, open spaces, and folders that are not
        // open) and "Archived", which is a sibling section, not a group inside
        // the first one.
        let mut history_blocks: Vec<AnyElement> = Vec::new();
        if !view.pinned.is_empty() {
            history_blocks.push(self.render_history_group(
                "pinned",
                localization::localized_str!("Pinned"),
                &view.pinned,
                false,
                cx,
            ));
        }
        for (key, label, sessions) in &view.groups {
            history_blocks.push(self.render_history_group(
                &format!("folder:{key}"),
                label,
                sessions,
                false,
                cx,
            ));
        }

        let mut archived_blocks: Vec<AnyElement> = Vec::new();
        for (key, label, sessions) in &view.archived {
            archived_blocks.push(self.render_history_group(
                &format!("archived:{key}"),
                label,
                sessions,
                true,
                cx,
            ));
        }

        let history_ids = history_group_ids(&view);
        let archived_ids = archived_group_ids(&view);
        let history_collapsed = !history_ids.is_empty()
            && history_ids
                .iter()
                .all(|id| !self.expanded_history_groups.contains(id));
        let archived_collapsed = !archived_ids.is_empty()
            && archived_ids
                .iter()
                .all(|id| !self.expanded_history_groups.contains(id));
        let history_section = self.render_history_section(
            localization::localized_str!("History"),
            history_collapsed,
            false,
            cx,
        );
        let has_archived = !view.archived.is_empty();
        let archived_section = has_archived.then(|| {
            self.render_history_section(
                localization::localized_str!("Archived"),
                archived_collapsed,
                true,
                cx,
            )
        });

        v_flex()
            .w_full()
            .pt_3()
            .border_t_1()
            .border_color(cx.theme().colors().border)
            .child(history_section)
            .children(history_blocks)
            .when(has_archived, |this| {
                this.child(
                    v_flex()
                        .w_full()
                        .mt_2()
                        .pt_2()
                        .border_t_1()
                        .border_color(cx.theme().colors().border)
                        .child(archived_section.unwrap_or_else(|| div().into_any_element()))
                        .children(archived_blocks),
                )
            })
            .into_any_element()
    }

    /// Section header for one of the two history trees: the label on the left,
    /// and on the right a chevron that expands or collapses every group at
    /// once — the same plain chevron the diff view puts next to a file, rather
    /// than a bordered button.
    fn render_history_section(
        &mut self,
        label: &'static str,
        collapsed: bool,
        archived: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let panel = cx.entity();
        h_flex()
            .w_full()
            .px_2()
            .pb_1()
            .justify_between()
            .child(
                Label::new(label)
                    .size(LabelSize::Small)
                    .weight(FontWeight::BOLD)
                    .color(Color::Muted),
            )
            .child(
                h_flex()
                    .id(ElementId::Name(SharedString::from(format!(
                        "history-collapse-{}",
                        if archived { "archived" } else { "history" }
                    ))))
                    .cursor_pointer()
                    .tooltip(move |_, cx| {
                        Tooltip::simple(
                            if collapsed {
                                localization::localized_str!("Expand all")
                            } else {
                                localization::localized_str!("Collapse all")
                            },
                            cx,
                        )
                    })
                    .on_click(move |_, _, cx| {
                        let _ = panel.update(cx, |panel, cx| {
                            panel.toggle_all_history_groups(archived, cx);
                        });
                    })
                    .child(
                        // Expanded shows the "collapse" glyph, collapsed shows
                        // the "expand" one — the same pair the search results
                        // and the solo diff view use for their columns.
                        Icon::new(if collapsed {
                            IconName::ChevronUpDown
                        } else {
                            IconName::ChevronDownUp
                        })
                        .size(IconSize::Small)
                        .color(Color::Muted),
                    ),
            )
            .into_any_element()
    }

    /// Expands or collapses every group of one tree, so a long list does not
    /// have to be opened one group at a time.
    fn toggle_all_history_groups(&mut self, archived: bool, cx: &mut Context<Self>) {
        let Some(view) = self.history.clone() else {
            return;
        };
        let ids = if archived {
            archived_group_ids(&view)
        } else {
            history_group_ids(&view)
        };
        let all_collapsed =
            !ids.is_empty() && ids.iter().all(|id| !self.expanded_history_groups.contains(id));
        for id in ids {
            if all_collapsed {
                self.expanded_history_groups.insert(id);
            } else {
                self.expanded_history_groups.remove(&id);
            }
        }
        if all_collapsed {
            // Expanding means the user is about to read history, so this is
            // when the list is refreshed — the same rule as opening one group.
            self.refresh_history(cx);
        }
        cx.notify();
    }

    /// One collapsible group header plus its session rows.
    fn render_history_group(
        &mut self,
        group_id: &str,
        label: &str,
        sessions: &[session_service::SessionEntry],
        archived: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let collapsed = !self.expanded_history_groups.contains(group_id);
        let mut block = v_flex().w_full();
        block = block.child(
            row_container(
                ElementId::Name(SharedString::from(format!("history-group-{group_id}"))),
                px(0.0),
                false,
                cx,
            )
            .on_click({
                let group_id = group_id.to_owned();
                let panel = cx.entity();
                move |_, _, cx| {
                    let _ = panel.update(cx, |panel, cx| {
                        // Groups start collapsed, so the first click expands.
                        // Expanding means the user is looking at history now,
                        // which is when the list is refreshed.
                        if !panel.expanded_history_groups.remove(&group_id) {
                            panel.expanded_history_groups.insert(group_id.clone());
                            panel.refresh_history(cx);
                        }
                    });
                }
            })
            .child(
                h_flex()
                    .w_full()
                    .gap_1p5()
                    .child(
                        h_flex().min_w(STATUS_SLOT_SIZE).justify_center().child(
                            Icon::new(if collapsed {
                                IconName::ChevronRight
                            } else {
                                IconName::ChevronDown
                            })
                            .size(IconSize::Small)
                            .color(Color::Muted),
                        ),
                    )
                    .child(
                        h_flex()
                            .flex_1()
                            .min_w_0()
                            .gap_1()
                            .child(
                                Label::new(label.to_owned())
                                    .size(LabelSize::Small)
                                    .weight(FontWeight::BOLD),
                            )
                            .child(
                                Label::new(sessions.len().to_string())
                                    .size(LabelSize::Small)
                                    .color(Color::Muted),
                            ),
                    ),
            ),
        );
        if !collapsed {
            for entry in sessions {
                block = block.child(self.render_history_row(entry, archived, cx));
            }
        }
        block.into_any_element()
    }

    /// One history row: agent icon, title (or the rename editor), relative
    /// time, and the hover actions on the right.
    fn render_history_row(
        &mut self,
        entry: &session_service::SessionEntry,
        archived: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let key = entry.key.clone();
        let opening = self
            .opening
            .as_ref()
            .is_some_and(|opening| *opening == key);
        let renaming = self
            .history_renaming
            .as_ref()
            .is_some_and(|renaming| *renaming == key);
        let panel = cx.entity();
        let click_key = key.clone();
        let pin_key = key.clone();
        let archive_key = key.clone();
        let rename_key = key.clone();
        let delete_key = key.clone();
        let delete_path = entry.source_path.clone();
        let editor = if renaming {
            self.history_rename_editor.clone()
        } else {
            None
        };

        let title: AnyElement = if let Some(editor) = editor {
            let confirm_panel = panel.clone();
            let cancel_panel = panel.clone();
            div()
                .id(ElementId::Name(SharedString::from(format!(
                    "history-rename-{}:{}",
                    key.agent, key.session_id
                ))))
                .relative()
                .flex_1()
                .h_6()
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_click(|_, _, cx| cx.stop_propagation())
                .child(
                    div()
                        .absolute()
                        .top_0()
                        .left_0()
                        .size_full()
                        .child(editor)
                        .on_action(move |_: &menu::Confirm, window, cx| {
                            let _ = confirm_panel.update(cx, |panel, cx| {
                                panel.finish_history_rename(true, window, cx)
                            });
                        })
                        .on_action(move |_: &menu::Cancel, window, cx| {
                            let _ = cancel_panel.update(cx, |panel, cx| {
                                panel.finish_history_rename(false, window, cx)
                            });
                        }),
                )
                .into_any_element()
        } else {
            let mut label = Label::new(entry.title.clone())
                .size(LabelSize::Default)
                .single_line()
                .truncate();
            if archived {
                label = label.color(Color::Muted);
            }
            // Session titles are whole sentences, so the row shows a single
            // truncated line and the hover card carries the full text.
            let full_title = entry.title.clone();
            h_flex()
                .id(ElementId::Name(SharedString::from(format!(
                    "history-title-{}:{}",
                    key.agent, key.session_id
                ))))
                .flex_1()
                .min_w_0()
                .child(label)
                .tooltip(Tooltip::text(full_title.clone()))
                .into_any_element()
        };

        let time_label = relative_history_time(entry.updated_at);
        let (marker, marker_color) = if opening {
            (SharedString::from("◐"), Color::Warning)
        } else {
            (SharedString::from(""), Color::Muted)
        };

        let row_id = SharedString::from(format!(
            "history-row-{}:{}",
            key.agent, key.session_id
        ));
        let mut row = row_container(ElementId::Name(row_id.clone()), AGENT_INDENT, false, cx)
        .group(row_id.clone())
        .when(archived, |row| row.opacity(0.6))
        .child(
            h_flex()
                .relative()
                .w_full()
                .gap_1p5()
                .child(status_slot(marker, marker_color))
                .child(history_agent_icon(&key.agent))
                .child(title)
                // The time sits at the right edge, lined up with the section's
                // "click to open" hint above the list.
                .child(
                    h_flex().ml_auto().flex_none().child(
                        Label::new(time_label)
                            .size(LabelSize::Small)
                            .color(Color::Muted),
                    ),
                )
                // The actions float over the row's right edge and cover the end
                // of the title while the pointer is on the row.
                .child(
                    h_flex()
                        .absolute()
                        .right_0()
                        .top_0()
                        .bottom_0()
                        .items_center()
                        .gap_0p5()
                        .px_1()
                        .bg(cx.theme().colors().ghost_element_hover)
                        .visible_on_hover(row_id.clone())
                        .when(!archived, |this| {
                            this.child({
                                let pinned_label = if entry.pinned {
                                    localization::localized_str!("Unpin")
                                } else {
                                    localization::localized_str!("Pin")
                                };
                                IconButton::new(
                                    ElementId::Name(SharedString::from(format!(
                                        "history-pin-{}:{}",
                                        key.agent, key.session_id
                                    ))),
                                    if entry.pinned {
                                        IconName::Unpin
                                    } else {
                                        IconName::Pin
                                    },
                                )
                                .icon_size(IconSize::Small)
                                .icon_color(if entry.pinned {
                                    Color::Accent
                                } else {
                                    Color::Muted
                                })
                                .tooltip(move |_, cx| {
                                    Tooltip::simple(pinned_label, cx)
                                })
                                .on_click({
                                    let panel = panel.clone();
                                    let pinned = entry.pinned;
                                    move |_, _, cx| {
                                        let _ = panel.update(cx, |panel, cx| {
                                            panel.toggle_history_pin(
                                                pin_key.clone(),
                                                !pinned,
                                                cx,
                                            );
                                        });
                                    }
                                })
                            })
                            .child({
                                IconButton::new(
                                    ElementId::Name(SharedString::from(format!(
                                        "history-rename-{}:{}",
                                        key.agent, key.session_id
                                    ))),
                                    IconName::Pencil,
                                )
                                .icon_size(IconSize::Small)
                                .icon_color(Color::Muted)
                                .tooltip(move |_, cx| {
                                    Tooltip::simple(localization::localized_str!("Rename"), cx)
                                })
                                .on_click({
                                    let panel = panel.clone();
                                    let windowless_key = rename_key.clone();
                                    move |_, window, cx| {
                                        let _ = panel.update(cx, |panel, cx| {
                                            panel.begin_history_rename(
                                                windowless_key.clone(),
                                                window,
                                                cx,
                                            );
                                        });
                                    }
                                })
                            })
                        })
                        .child({
                            let archive_label = if archived {
                                localization::localized_str!("Unarchive")
                            } else {
                                localization::localized_str!("Archive")
                            };
                            IconButton::new(
                                ElementId::Name(SharedString::from(format!(
                                    "history-archive-{}:{}",
                                    key.agent, key.session_id
                                ))),
                                if archived {
                                    IconName::RotateCcw
                                } else {
                                    IconName::Archive
                                },
                            )
                            .icon_size(IconSize::Small)
                            .icon_color(Color::Muted)
                            .tooltip(move |_, cx| Tooltip::simple(archive_label, cx))
                            .on_click({
                                let panel = panel.clone();
                                let was_archived = archived;
                                move |_, _, cx| {
                                    let _ = panel.update(cx, |panel, cx| {
                                        panel.archive_history_session(
                                            archive_key.clone(),
                                            !was_archived,
                                            cx,
                                        );
                                    });
                                }
                            })
                        })
                        .when(entry.deletable, |row| {
                            row.child({
                                IconButton::new(
                                    ElementId::Name(SharedString::from(format!(
                                        "history-delete-{}:{}",
                                        key.agent, key.session_id
                                    ))),
                                    IconName::Trash,
                                )
                                .icon_size(IconSize::Small)
                                .icon_color(Color::Muted)
                                .tooltip(move |_, cx| {
                                    Tooltip::simple(localization::localized_str!("Delete"), cx)
                                })
                                .on_click({
                                    let panel = panel.clone();
                                    let delete_key = delete_key.clone();
                                    let delete_path = delete_path.clone();
                                    move |_, _, cx| {
                                        let _ = panel.update(cx, |panel, cx| {
                                            panel.delete_history_session(
                                                delete_key.clone(),
                                                delete_path.clone(),
                                                cx,
                                            );
                                        });
                                    }
                                })
                            })
                        })
                ),
        );

        if !renaming {
            let open_key = click_key.clone();
            row = row.on_click(move |_, _, cx| {
                let _ = panel.update(cx, |panel, cx| {
                    panel.open_history_session(open_key.clone(), cx);
                });
            });
        }
        row.into_any_element()
    }

    /// Closes one space and its terminal.
    fn close_space(&self, workspace_id: String, cx: &mut Context<Self>) {
        cx.spawn(async move |_this, cx| {
            if let Err(err) =
                cx.background_spawn(async move { client::close_space(&workspace_id) })
                    .await
            {
                log_client_error("close space", err);
            }
        })
        .detach();
    }

    /// Shown in place of the tree while there is no server to read from.
    ///
    /// The server is a separate process the system starts, not a child of the
    /// editor, so nothing here can start it on the user's behalf. The panel
    /// therefore has to name the command to run instead of only reporting that
    /// the tree is missing, and it centres the message so it reads as the state
    /// of the whole panel rather than a stray line at the top.
    fn render_disconnected(&self, _cx: &mut Context<Self>) -> AnyElement {
        // One sentence, no heading: there is nothing to name here beyond the
        // command, and the panel is narrow enough that a heading would wrap and
        // push the instruction down. The program name comes from the constant so
        // the text cannot drift from what the panel actually launches.
        let message = localization::format_message(
            "Run \"{}\" in your system terminal to unlock all HiCodeer features.",
            &[("0", AGENT_DAEMON_PROGRAM.to_string())],
        );

        v_flex()
            .size_full()
            .justify_center()
            .items_center()
            .px_3()
            .child(
                // Same type as the Project Panel's empty-state "Open Project"
                // text: the default label size and text colour, not the small
                // muted variant used for secondary metadata.
                Label::new(message)
                    .size(LabelSize::Default)
                    .color(Color::Default)
                    .text_center(),
            )
            .into_any_element()
    }

    /// Header of the panel: the space creator, pinned above the scrolling tree.
    /// Settings live in the editor's own settings page, so the panel has no
    /// entry of its own.
    fn render_actions(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        // No horizontal padding here: the rows below it carry their own side
        // padding, and an extra layer shifted this row 4px right of the tree.
        v_flex()
            .id("agent-panel-actions")
            .w_full()
            .py_1()
            .gap_0p5()
            .child(
                row_container("agent-panel-new-space", px(0.0), false, cx)
                    .child(action_content(
                        IconName::Plus,
                        localization::localized_str!("New Space"),
                    ))
                    .on_click(cx.listener(|this, _, _window, cx| this.create_space(cx))),
            )
    }
}

/// One row, laid out the way the thread sidebar lays out its entries: 4px above
/// and below, 6px at the sides, rounded, selection and hover painted on the row
/// itself. `indent` shifts the content right without shrinking the row, so a
/// nested row keeps the same hit area and the same background shape.
fn row_container(
    id: impl Into<ElementId>,
    indent: Pixels,
    selected: bool,
    cx: &App,
) -> gpui::Stateful<Div> {
    let colors = cx.theme().colors();
    div()
        .id(id)
        .w_full()
        .py_1()
        .pl(ROW_SIDE_PADDING + indent)
        .pr(ROW_SIDE_PADDING)
        .rounded_sm()
        .cursor_pointer()
        .when(selected, |this| this.bg(colors.ghost_element_selected))
        .hover(|style| style.bg(colors.ghost_element_hover))
}

/// Icon and label of one header entry, laid out so the icon sits in the same
/// slot the tree's status markers use.
fn action_content(icon: IconName, label: impl Into<SharedString>) -> impl IntoElement {
    h_flex()
        .gap_1p5()
        .child(
            h_flex()
                .min_w(STATUS_SLOT_SIZE)
                .justify_center()
                .child(Icon::new(icon).size(IconSize::Small).color(Color::Muted)),
        )
        .child(
            h_flex()
                .h_6()
                .child(Label::new(label).size(LabelSize::Small)),
        )
}

/// Reports a failed panel action. These are one-shot user gestures, so a
/// failure is worth a line rather than a state change on screen.
fn log_client_error(action: &str, err: ClientError) {
    let reason = match err {
        ClientError::NotReady => "the agent server is not ready".to_string(),
        ClientError::Failed(message) => message,
    };
    log::warn!("agent panel: {action} failed: {reason}");
}

/// The marker drawn before a row, in the same slot the sidebar uses for a
/// thread's icon.
///
/// These are the terminal's "distinct symbols" indicators: one glyph per state,
/// so the row reads the same way in the panel as in the terminal, and the shape
/// alone already separates blocked, working, done, and idle. Colour repeats the
/// state but is not what carries it.
fn status_marker(status: AgentStatus) -> (SharedString, Color) {
    match status {
        AgentStatus::Blocked => ("!".into(), Color::Warning),
        AgentStatus::Working => ("◐".into(), Color::Warning),
        AgentStatus::Done => ("✓".into(), Color::Created),
        AgentStatus::Idle => ("○".into(), Color::Success),
        AgentStatus::Unknown => ("·".into(), Color::Disabled),
    }
}

/// The icon a space row starts with: the same one the Agent panel itself wears
/// in the status bar, because a space is this panel's unit, not an agent. An
/// agent's working state belongs on the agent row.
fn space_icon_slot() -> impl IntoElement {
    h_flex()
        .min_w(STATUS_SLOT_SIZE)
        .justify_center()
        .child(
            Icon::new(IconName::Sparkle)
                .size(IconSize::Small)
                .color(Color::Muted),
        )
}

/// The status marker in its fixed-width slot, so every row's label starts at the
/// same place whatever glyph it draws.
fn status_slot(marker: SharedString, color: Color) -> impl IntoElement {
    h_flex()
        .min_w(STATUS_SLOT_SIZE)
        .justify_center()
        .child(Label::new(marker).size(LabelSize::Small).color(color))
}

impl Focusable for AgentPanel {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl EventEmitter<PanelEvent> for AgentPanel {}

impl Render for AgentPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        #[cfg(target_env = "ohos")]
        if self.window_activation.is_none() {
            // The callback only fires on changes, and the window is already
            // active by the time the panel first renders, so sampling once
            // before subscribing is what makes the first frames truthful.
            self.window_active = window.is_window_active();
            self.window_activation = Some(cx.observe_window_activation(
                window,
                |this, window, _cx| {
                    this.window_active = window.is_window_active();
                },
            ));
        }
        self.schedule_agent_terminal_sync(window, cx);
        let history = self.render_history(cx);
        let tree = if self.unavailable.is_some() {
            self.render_disconnected(cx)
        } else if self.spaces.is_empty() {
            v_flex()
                .p_2()
                .child(
                    Label::new(localization::localized_str!("No spaces yet"))
                        .size(LabelSize::Small)
                        .color(Color::Muted),
                )
                .into_any_element()
        } else {
            // One list: each space is a heading with its agents under it. The
            // boundary above a space is the same 1px top border the thread
            // sidebar puts between project groups, so a space and its agents
            // read as one block, flush against the line with no extra gap.
            let mut rows: Vec<AnyElement> = Vec::new();
            for (index, space) in self.spaces.iter().enumerate() {
                let mut block = v_flex()
                    .w_full()
                    .border_t_1()
                    .border_color(cx.theme().colors().border);
                // Everything after the first space carries the same gap below
                // its line as the header has above the first one.
                if index > 0 {
                    block = block.pt_1();
                }
                block = block.child(self.render_space(space, cx));
                for agent in &space.agents {
                    block = block.child(self.render_agent_row(agent, cx));
                }
                rows.push(block.into_any_element());
            }
            v_flex().w_full().children(rows).into_any_element()
        };

        v_flex()
            .id("agent-panel")
            .key_context("AgentPanel")
            .track_focus(&self.focus_handle)
            .size_full()
            .overflow_hidden()
            .child(self.render_actions(cx))
            .child(
                v_flex()
                    .id("agent-panel-tree")
                    .flex_1()
                    .overflow_y_scroll()
                    .child(tree)
                    .child(history),
            )
    }
}

/// Group identifiers of the "History" tree, in render order. Used to expand or
/// collapse the whole tree at once. The identifiers match the ones
/// `render_history` gives the group rows.
fn history_group_ids(view: &HistoryView) -> Vec<String> {
    let mut ids = Vec::new();
    if !view.pinned.is_empty() {
        ids.push("pinned".to_owned());
    }
    ids.extend(view.groups.iter().map(|(key, _, _)| format!("folder:{key}")));
    ids
}

/// The same for the archive tree.
fn archived_group_ids(view: &HistoryView) -> Vec<String> {
    view.archived
        .iter()
        .map(|(key, _, _)| format!("archived:{key}"))
        .collect()
}

/// The history row's time, in the thread sidebar's coarse style.
fn relative_history_time(updated_at: Option<i64>) -> SharedString {
    let Some(updated_at) = updated_at else {
        return "".to_owned().into();
    };
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);
    let elapsed = (now_ms - updated_at).max(0);
    let label = if elapsed < 60_000 {
        localization::localized_str!("now").to_owned()
    } else if elapsed < 3_600_000 {
        format!(
            "{}{}",
            elapsed / 60_000,
            localization::localized_str!("min")
        )
    } else if elapsed < 86_400_000 {
        format!(
            "{}{}",
            elapsed / 3_600_000,
            localization::localized_str!("hour")
        )
    } else if elapsed < 604_800_000 {
        format!(
            "{}{}",
            elapsed / 86_400_000,
            localization::localized_str!("day")
        )
    } else {
        format!(
            "{}{}",
            elapsed / 604_800_000,
            localization::localized_str!("week")
        )
    };
    label.into()
}

/// The agent icon for a history row. `agent_display::agent_icon` already has a
/// fallback for agents without a glyph, so every connector the daemon collects
/// can wear its own mark instead of the generic terminal.
fn history_agent_icon(agent: &str) -> Icon {
    Icon::new(agent_display::agent_icon(agent)).size(IconSize::Small)
}

impl Panel for AgentPanel {
    fn persistent_name() -> &'static str {
        "Agent Panel"
    }

    fn panel_key() -> &'static str {
        "AgentPanel"
    }

    fn is_agent_panel(&self) -> bool {
        true
    }

    fn position(&self, _window: &Window, cx: &App) -> DockPosition {
        AgentPanelSettings::get_global(cx).dock
    }

    fn position_is_valid(&self, position: DockPosition) -> bool {
        matches!(position, DockPosition::Left | DockPosition::Right)
    }

    fn set_position(
        &mut self,
        position: DockPosition,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        update_settings_file(self.fs.clone(), cx, move |settings, _| {
            settings.agent_panel.get_or_insert_default().dock = Some(position.into())
        });
    }

    fn default_size(&self, _window: &Window, cx: &App) -> Pixels {
        AgentPanelSettings::get_global(cx).default_width
    }

    fn icon(&self, _window: &Window, cx: &App) -> Option<IconName> {
        AgentPanelSettings::get_global(cx)
            .button
            .then_some(IconName::Sparkle)
    }

    fn icon_tooltip(&self, _window: &Window, _cx: &App) -> Option<&'static str> {
        Some(localization::localized_str!("Agent Panel"))
    }

    fn toggle_action(&self) -> Box<dyn Action> {
        Box::new(ToggleFocus)
    }

    fn starts_open(&self, _window: &Window, cx: &App) -> bool {
        !hidden_until_onboarding() && AgentPanelSettings::get_global(cx).starts_open
    }

    fn starts_open_overrides_restore(&self, cx: &App) -> bool {
        !hidden_until_onboarding() && AgentPanelSettings::get_global(cx).starts_open
    }

    fn activation_priority(&self) -> u32 {
        0
    }
}
