//! Agent panel: the GPUI front end for the agent server hosted by hicodeerd.
//!
//! The server's own sidebar is hidden (see its `ui.sidebar_start_collapsed`), so
//! this panel is where the user sees and drives spaces and agents. Its data is
//! the server's, read over the daemon's control bridge: each space is drawn
//! with its agents nested one level underneath, in the server's order, carrying
//! the server's own status for each agent.

pub mod agent_display;
mod client;
pub mod herdr_settings;
mod snapshot;

use std::sync::Arc;
use std::time::{Duration, Instant};

use editor::{Editor, actions::SelectAll};
use fs::Fs;
use gpui::{
    Action, AnyElement, App, AsyncWindowContext, Context, ElementId, Entity, EventEmitter,
    FocusHandle, Focusable, FontWeight, MouseButton, Pixels, Render, SharedString, Subscription,
    Task, TaskExt, WeakEntity, Window, actions, div, prelude::*, px,
};
use menu;
use settings::{IntoGpui, RegisterSetting, Settings, SettingsStore, update_settings_file};
use terminal_view::TerminalView;
use theme::ActiveTheme as _;
use ui::{
    Color, ContextMenu, Icon, IconName, IconSize, Label, LabelCommon, LabelSize, prelude::*,
    right_click_menu,
};
use workspace::{
    SaveIntent, Workspace,
    dock::{DockPosition, Panel, PanelEvent},
};

use crate::client::ClientError;
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
/// Key sequence that opens the agent client's own settings screen.
///
/// The client's `menu` entry routes to its settings action, but the menu itself
/// only opens from a mouse click on the footer its sidebar draws -- and this
/// build hides that sidebar. Its default keybinding reaches the same action, so
/// the panel sends the prefix key (Ctrl-B) followed by `s` to the terminal it
/// hosts, which is what selecting that menu entry does.
const AGENT_SETTINGS_KEY_SEQUENCE: &[u8] = &[0x02, b's'];
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
        }
    }
}

pub fn init(cx: &mut App) {
    AgentPanelSettings::register(cx);
    #[cfg(target_env = "ohos")]
    {
        cx.set_global(herdr_settings::AgentIntegrations::default());
        cx.set_global(herdr_settings::TerminalSettingsCache::default());
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
                if this
                    .update(cx, |this, cx| this.apply(fetched, cx))
                    .is_err()
                {
                    return;
                }
                cx.background_executor().timer(REFRESH_INTERVAL).await;
            }
        });

        Self {
            focus_handle: cx.focus_handle(),
            workspace: workspace.weak_handle(),
            fs,
            spaces: Vec::new(),
            unavailable: Some(Unavailable::WaitingForServer),
            consecutive_failures: 0,
            last_deploy_attempt: None,
            wanted: false,
            needs_sync: false,
            pending_agent: None,
            renaming: None,
            rename_editor: None,
            rename_subscription: None,
            _settings_subscription: cx.observe_global::<SettingsStore>(|_, cx| cx.notify()),
            _refresh: refresh,
        }
    }

    fn apply(&mut self, fetched: Result<Snapshot, ClientError>, cx: &mut Context<Self>) {
        match fetched {
            Ok(snapshot) => {
                self.spaces = snapshot.spaces;
                self.unavailable = None;
                self.consecutive_failures = 0;
                self.wanted = true;
                self.needs_sync = true;
                self.reconcile_pending_focus();
                #[cfg(target_env = "ohos")]
                self.ensure_integrations(cx);
            }
            Err(ClientError::NotReady) => {
                // No endpoint at all: the daemon went away and the bootstrap
                // cleared what it had reported. That is a disconnection just as
                // much as a refused connection is, and the terminal it left
                // behind is a dead tab, so it counts the same way.
                self.unavailable = Some(Unavailable::WaitingForServer);
                self.note_disconnect();
            }
            Err(ClientError::Failed(message)) => {
                // One line per outage: the refresh runs once a second, so
                // logging every failure would bury the first, useful error.
                if self.consecutive_failures == 0 {
                    log::warn!("agent panel: refresh failed: {message}");
                }
                self.unavailable = Some(Unavailable::ServerUnavailable);
                self.note_disconnect();
            }
        }
        cx.notify();
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

    /// Counts one failed refresh, closing the terminal once failures form a
    /// run long enough to call it a disconnection.
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
        let present = workspace_has_agent_terminal(workspace.read(cx), cx);

        if self.wanted == present {
            return;
        }

        if self.wanted {
            let Some(control) = cmd_client::agent_control() else {
                return;
            };
            if let Some(last_attempt) = self.last_deploy_attempt
                && last_attempt.elapsed() < DEPLOY_RETRY_INTERVAL
            {
                // Let the next refresh try again rather than spinning here.
                self.needs_sync = true;
                return;
            }
            self.last_deploy_attempt = Some(std::time::Instant::now());
            workspace.update(cx, |workspace, cx| {
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
            workspace.update(cx, |workspace, cx| {
                let targets: Vec<_> = workspace
                    .panes()
                    .iter()
                    .filter_map(|pane| {
                        let agent = pane
                            .read(cx)
                            .items_of_type::<TerminalView>()
                            .find(|view| view.read(cx).is_agent_terminal())?;
                        Some((pane.clone(), agent))
                    })
                    .collect();
                for (pane, view) in targets {
                    // The tab refuses to close while it is pinned, which is what
                    // stops the user from closing it; clearing the title is how
                    // its owner closes it.
                    view.update(cx, |view, cx| view.clear_pinned_title(cx));
                    let item_id = view.entity_id();
                    pane.update(cx, |pane, cx| {
                        pane.close_item_by_id(item_id, SaveIntent::Close, window, cx)
                            .detach_and_log_err(cx);
                    });
                }
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

    /// Asks the agent client to open its own settings screen.
    ///
    /// This is the client's settings, not the editor's: the panel drives that
    /// client, so the entry has to act on it. The terminal is brought forward
    /// first, otherwise the screen would open where the user cannot see it.
    fn open_agent_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.reveal_agent_terminal(window, cx);
        let Some(workspace) = self.workspace.upgrade() else {
            return;
        };
        workspace.update(cx, |workspace, cx| {
            let agent_view = workspace.panes().iter().find_map(|pane| {
                pane.read(cx)
                    .items_of_type::<TerminalView>()
                    .find(|view| view.read(cx).is_agent_terminal())
            });
            let Some(view) = agent_view else {
                log::warn!("agent panel: no Agent terminal to send the settings key to");
                return;
            };
            view.update(cx, |view, cx| {
                view.send_input(AGENT_SETTINGS_KEY_SEQUENCE, cx);
            });
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
        let menu_space_id = space.id.clone();
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

        let row = row_container(
            ElementId::Name(SharedString::from(format!("agent-space-{}", space.id))),
            px(0.0),
            false,
            cx,
        )
        .child(
            h_flex()
                .w_full()
                .gap_1p5()
                .child(space_icon_slot())
                .child(h_flex().flex_1().min_w_0().h_6().child(title)),
        )
        .on_click({
            let panel = panel.clone();
            move |_, window, cx| {
                let _ = panel.update(cx, |panel, cx| panel.focus_space(id.clone(), window, cx));
            }
        });

        right_click_menu(ElementId::Name(SharedString::from(format!(
            "agent-space-menu-{}",
            menu_space_id
        ))))
        .trigger(|_, _, _| row)
        .menu(move |window, cx| {
            let rename_panel = panel.clone();
            let close_panel = panel.clone();
            let rename_target = menu_space_id.clone();
            let close_target = menu_space_id.clone();
            ContextMenu::build(window, cx, move |mut menu, window, _cx| {
                menu = menu.entry(
                    localization::localized_str!("Rename"),
                    None,
                    window.handler_for(&rename_panel, move |panel, window, cx| {
                        panel.begin_rename(&rename_target, window, cx);
                    }),
                );
                menu = menu.entry(
                    localization::localized_str!("Close"),
                    None,
                    window.handler_for(&close_panel, move |panel, _window, cx| {
                        panel.close_space(close_target.clone(), cx);
                    }),
                );
                menu
            })
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
                        .child(Label::new(agent_display::capitalize(&agent.name))),
                ),
        )
        .on_click(move |_, window, cx| {
            let _ = panel.update(cx, |panel, cx| panel.focus_agent(pane_id.clone(), window, cx));
        })
        .into_any_element()
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

    /// Header of the panel: the two global entries, settings first and the
    /// space creator under it. Both stay pinned above the scrolling tree.
    fn render_actions(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        v_flex()
            .id("agent-panel-actions")
            .w_full()
            .px_1()
            .py_1()
            .gap_0p5()
            .child(
                row_container("agent-panel-settings", px(0.0), false, cx)
                    .child(action_content(
                        IconName::Settings,
                        localization::localized_str!("Settings"),
                    ))
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.open_agent_settings(window, cx);
                    })),
            )
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
        AgentStatus::Blocked => ("×".into(), Color::Error),
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
        self.schedule_agent_terminal_sync(window, cx);
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
                    .child(tree),
            )
    }
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
