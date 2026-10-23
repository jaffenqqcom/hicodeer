mod onboarding_banner;
mod title_bar_settings;
// [OHOS PORT BEGIN] HiCodeer has no update feed, so the title bar shows no
// version badge and no update affordance.
#[cfg(not(target_env = "ohos"))]
mod update_version;
// [OHOS PORT END]

use arrayvec::ArrayVec;
use git_ui_core::worktree_picker::WorktreePicker;
pub use platform_title_bar::{
    self, DraggedWindowTab, MergeAllWindows, MoveTabToNewWindow, PlatformTitleBar,
    ShowNextWindowTab, ShowPreviousWindowTab,
};
use project::{linked_worktree_short_name, repo_identity_path, repo_identity_path_if_local};

#[cfg(not(target_env = "ohos"))]
use auto_update::AutoUpdateStatus;
use client::{Client, UserStore};

#[cfg(target_env = "ohos")]
use gpui::img;
use gpui::{
    Action, Anchor, AnyElement, App, Context, Element, Entity, Focusable, InteractiveElement,
    IntoElement, MouseButton, ParentElement, Render, StatefulInteractiveElement, Styled,
    Subscription, WeakEntity, Window, actions, div,
};
use onboarding_banner::OnboardingBanner;
use project::{
    Project, git_store::GitStoreEvent, project_settings::ProjectSettings,
    trusted_worktrees::TrustedWorktrees,
};
use remote::RemoteConnectionOptions;
use settings::Settings as _;
#[cfg(target_env = "ohos")]
use settings::{DockPosition, DockSide, SettingsStore, update_settings_file};

#[cfg(target_env = "ohos")]
use fs::Fs;
use std::path::Path;
use std::sync::Arc;
use theme::ActiveTheme;
use title_bar_settings::TitleBarSettings;
use ui::{
    ButtonLike, ContextMenu, IconWithIndicator, Indicator, PopoverMenu, TintColor, Tooltip,
    prelude::*,
};
#[cfg(target_env = "ohos")]
use ui::ContextMenuEntry;
#[cfg(not(target_env = "ohos"))]
use update_version::UpdateVersion;
use util::ResultExt;
use workspace::{MultiWorkspace, ToggleWorktreeSecurity, Workspace};

use zed_actions::OpenRemote;

pub use onboarding_banner::restore_banner;

const MAX_PROJECT_NAME_LENGTH: usize = 40;
const MAX_BRANCH_NAME_LENGTH: usize = 40;
const MAX_SHORT_SHA_LENGTH: usize = 8;

fn linked_worktree_name_anchor<'a>(
    main_worktree_path: Option<&'a Path>,
    repository_identity_path: Option<&'a Path>,
    is_linked_worktree: bool,
) -> Option<&'a Path> {
    main_worktree_path.or_else(|| {
        is_linked_worktree
            .then_some(repository_identity_path)
            .flatten()
    })
}

actions!(
    collab,
    [
        /// Toggles the user menu dropdown.
        ToggleUserMenu,
        /// Toggles the project menu dropdown.
        ToggleProjectMenu,
        /// Switches to a different git branch.
        SwitchBranch,
        /// A debug action to simulate an update being available to test the update banner UI.
        SimulateUpdateAvailable
    ]
);

// ===== [OHOS PORT BEGIN] =====
// Panel layout presets used to live in the `agent_settings` crate, which is not
// compiled on OHOS. They are defined here because the title bar user menu is the
// only consumer. Restore the upstream `agent_settings::PanelLayout` to remove them.
#[cfg(target_env = "ohos")]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WindowLayout {
    /// Editor-first: the project panel and git panel dock on the left.
    Editor,
    /// Agent-first: the project panel and git panel dock on the right.
    Agent,
    /// Any arrangement that matches neither preset.
    Custom,
}

#[cfg(target_env = "ohos")]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Action)]
#[action(namespace = workspace)]
pub struct UseClassicLayout;

#[cfg(target_env = "ohos")]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Action)]
#[action(namespace = workspace)]
pub struct UseAgenticLayout;

/// Reports which preset the current settings correspond to.
///
/// The Agent panel sits opposite the project group in both presets: left of the
/// editor in the Agentic layout, right of it in the Classic one.
#[cfg(target_env = "ohos")]
fn current_layout(cx: &App) -> WindowLayout {
    let store = cx.global::<SettingsStore>();
    let content = store.merged_settings();
    let project_dock = content.project_panel.as_ref().and_then(|panel| panel.dock);
    let outline_dock = content.outline_panel.as_ref().and_then(|panel| panel.dock);
    let git_dock = content.git_panel.as_ref().and_then(|panel| panel.dock);
    let agent_dock = content.agent_panel.as_ref().and_then(|panel| panel.dock);

    let project_group_on_left =
        (project_dock, outline_dock) == (Some(DockSide::Left), Some(DockSide::Left));
    let project_group_on_right =
        (project_dock, outline_dock) == (Some(DockSide::Right), Some(DockSide::Right));
    let agent_not_on = |side: settings::DockPosition| agent_dock != Some(side);

    match (project_group_on_left, project_group_on_right) {
        (true, _)
            if git_dock == Some(DockPosition::Left)
                && agent_not_on(settings::DockPosition::Left) =>
        {
            WindowLayout::Editor
        }
        (_, true)
            if git_dock == Some(DockPosition::Right)
                && agent_not_on(settings::DockPosition::Right) =>
        {
            WindowLayout::Agent
        }
        _ => WindowLayout::Custom,
    }
}

/// Writes the dock positions of the preset into the user settings file.
#[cfg(target_env = "ohos")]
fn set_window_layout(layout: WindowLayout, cx: &App) {
    let (side, git_dock) = match layout {
        WindowLayout::Editor => (DockSide::Left, DockPosition::Left),
        WindowLayout::Agent => (DockSide::Right, DockPosition::Right),
        WindowLayout::Custom => return,
    };

    let agent_dock = match layout {
        WindowLayout::Editor => settings::DockPosition::Right,
        WindowLayout::Agent => settings::DockPosition::Left,
        WindowLayout::Custom => return,
    };

    let fs = <dyn Fs>::global(cx);
    update_settings_file(fs, cx, move |settings, _| {
        settings.project_panel.get_or_insert_default().dock = Some(side);
        settings.outline_panel.get_or_insert_default().dock = Some(side);
        settings.git_panel.get_or_insert_default().dock = Some(git_dock);
        settings.agent_panel.get_or_insert_default().dock = Some(agent_dock);
    });
}
// ===== [OHOS PORT END] =====

pub fn init(cx: &mut App) {
    platform_title_bar::PlatformTitleBar::init(cx);

    cx.observe_new(|workspace: &mut Workspace, window, cx| {
        let Some(window) = window else {
            return;
        };
        let multi_workspace = workspace.multi_workspace().cloned();
        let item = cx.new(|cx| TitleBar::new("title-bar", workspace, multi_workspace, window, cx));
        workspace.set_titlebar_item(item.into(), window, cx);

        #[cfg(target_env = "ohos")]
        {
            workspace.register_action(|_workspace, _: &UseClassicLayout, _window, cx| {
                set_window_layout(WindowLayout::Editor, cx);
            });

            workspace.register_action(|_workspace, _: &UseAgenticLayout, _window, cx| {
                set_window_layout(WindowLayout::Agent, cx);
            });
        }

        #[cfg(not(target_env = "ohos"))]
        workspace.register_action(|workspace, _: &SimulateUpdateAvailable, _window, cx| {
            if let Some(titlebar) = workspace
                .titlebar_item()
                .and_then(|item| item.downcast::<TitleBar>().ok())
            {
                titlebar.update(cx, |titlebar, cx| {
                    titlebar.toggle_update_simulation(cx);
                });
            }
        });
    })
    .detach();
}

pub struct TitleBar {
    platform_titlebar: Entity<PlatformTitleBar>,
    project: Entity<Project>,
    user_store: Entity<UserStore>,
    client: Arc<Client>,
    workspace: WeakEntity<Workspace>,
    multi_workspace: Option<WeakEntity<MultiWorkspace>>,
    _subscriptions: Vec<Subscription>,
    banner: Option<Entity<OnboardingBanner>>,
    #[cfg(not(target_env = "ohos"))]
    update_version: Entity<UpdateVersion>,
}

impl Render for TitleBar {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.multi_workspace.is_none() {
            if let Some(mw) = self
                .workspace
                .upgrade()
                .and_then(|ws| ws.read(cx).multi_workspace().cloned())
            {
                self.multi_workspace = Some(mw.clone());
                self.platform_titlebar.update(cx, |titlebar, _cx| {
                    titlebar.set_multi_workspace(mw);
                });
            }
        }

        let title_bar_settings = *TitleBarSettings::get_global(cx);
        let button_layout = title_bar_settings.button_layout;

        let mut children = <ArrayVec<_, 6>>::new();

        #[cfg(not(target_env = "ohos"))]
        children.push(self.render_project_items(cx));

        if title_bar_settings.show_onboarding_banner {
            if let Some(banner) = &self.banner {
                children.push(banner.clone().into_any_element())
            }
        }

        let status = self.client.status();
        let status = &*status.borrow();

        children.push(
            h_flex()
                .pr_1()
                .gap_1()
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .children(self.render_connection_status(status, cx))
                .children(self.render_version_badge(cx))
                .when(TitleBarSettings::get_global(cx).show_user_menu, |this| {
                    this.child(self.render_user_menu_button(cx))
                })
                .into_any_element(),
        );

        // Application icon at the far left of the title bar. Only HiCodeer's OHOS
        // title bar carries it; the asset is embedded through the asset source.
        #[cfg(target_env = "ohos")]
        children.insert(
            0,
            img("images/hicodeer.png")
                .size_4()
                .flex_none()
                .rounded(px(3.0))
                .mr_1()
                .into_any_element(),
        );

        self.platform_titlebar.update(cx, |this, _| {
            this.set_button_layout(button_layout);
            this.set_children(children);
        });
        self.platform_titlebar.clone().into_any_element()
    }
}

impl TitleBar {
    /// The project name, its host, and the git branch, laid out as one row.
    ///
    /// The OHOS build draws this row inside the Project panel instead of the
    /// title bar, so it has to be callable from there: one implementation keeps
    /// the two placements identical rather than growing a second copy.
    pub fn render_project_items(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let title_bar_settings = *TitleBarSettings::get_global(cx);
        let is_git_enabled = ProjectSettings::get_global(cx).git.enabled.status;
        let mut project_name = None;
        let mut repository = None;
        let mut linked_worktree_name = None;
        if let Some(worktree) = self.effective_active_worktree(cx) {
            repository = self.get_repository_for_worktree(&worktree, cx);
            let worktree_abs_path = worktree.read(cx).abs_path();
            project_name = worktree
                .read(cx)
                .root_name()
                .file_name()
                .map(|name| SharedString::from(name.to_string()));
            if let Some(repo) = &repository {
                let repo = repo.read(cx);
                let identity = repo_identity_path(&repo.common_dir_abs_path, repo.path_style);
                let identity_fallback =
                    repo_identity_path_if_local(&repo.common_dir_abs_path, repo.path_style);
                linked_worktree_name = linked_worktree_name_anchor(
                    repo.main_worktree_abs_path(),
                    identity_fallback,
                    repo.is_linked_worktree(),
                )
                .and_then(|name_anchor_path| {
                    linked_worktree_short_name(
                        name_anchor_path,
                        repo.work_directory_abs_path.as_ref(),
                        repo.path_style,
                    )
                })
                .or_else(|| {
                    repo.is_linked_worktree()
                        .then_some(project_name.clone())
                        .flatten()
                });

                let display_name = if identity.extension() == Some(std::ffi::OsStr::new("git")) {
                    identity.file_stem()
                } else {
                    repo.path_style.file_name(identity)
                };

                if let Some(repo_name) = display_name.and_then(|n| n.to_str()) {
                    let visible_worktrees_in_repo = self.visible_worktrees_in_repository(repo, cx);
                    let name = if visible_worktrees_in_repo == 1 {
                        if let Ok(relative) =
                            worktree_abs_path.strip_prefix(&*repo.work_directory_abs_path)
                        {
                            if relative.as_os_str().is_empty() {
                                repo_name.to_string()
                            } else {
                                format!("{}/{}", repo_name, relative.display())
                            }
                        } else {
                            repo_name.to_string()
                        }
                    } else {
                        repo_name.to_string()
                    };
                    project_name = Some(SharedString::from(name));
                }
            }
        }

        h_flex()
            .h_full()
            .gap_0p5()
            .map(|title_bar| {
                let render_project_items =
                    title_bar_settings.show_branch_name || title_bar_settings.show_project_items;
                title_bar.children(self.render_restricted_mode(cx)).when(
                    render_project_items,
                    |title_bar| {
                        title_bar
                            .when(title_bar_settings.show_project_items, |title_bar| {
                                title_bar
                                    .children(self.render_project_host(cx))
                                    .child(self.render_project_name(project_name, cx))
                            })
                            .when_some(
                                repository.filter(|_| is_git_enabled),
                                |title_bar, repository| {
                                    title_bar.children(self.render_worktree_and_branch(
                                        repository,
                                        linked_worktree_name,
                                        cx,
                                    ))
                                },
                            )
                    },
                )
            })
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .into_any_element()
    }

    pub fn new(
        id: impl Into<ElementId>,
        workspace: &Workspace,
        multi_workspace: Option<WeakEntity<MultiWorkspace>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let project = workspace.project().clone();
        let git_store = project.read(cx).git_store().clone();
        let user_store = workspace.app_state().user_store.clone();
        let client = workspace.app_state().client.clone();

        let mut subscriptions = Vec::new();
        subscriptions.push(
            cx.observe(&workspace.weak_handle().upgrade().unwrap(), |_, _, cx| {
                cx.notify()
            }),
        );

        subscriptions.push(
            cx.subscribe(&git_store, move |_, _, event, cx| match event {
                GitStoreEvent::ActiveRepositoryChanged(_)
                | GitStoreEvent::RepositoryUpdated(_, _, true) => {
                    cx.notify();
                }
                _ => {}
            }),
        );
        subscriptions.push(cx.observe(&user_store, |_a, _, cx| cx.notify()));
        if let Some(workspace_entity) = workspace.weak_handle().upgrade() {
            subscriptions.push(cx.subscribe(
                &workspace_entity,
                |_, _, event: &workspace::Event, cx| {
                    if matches!(event, workspace::Event::WorktreeCreationChanged) {
                        cx.notify();
                    }
                },
            ));
        }
        subscriptions.push(cx.observe_button_layout_changed(window, |_, _, cx| cx.notify()));
        if let Some(trusted_worktrees) = TrustedWorktrees::try_get_global(cx) {
            subscriptions.push(cx.subscribe(&trusted_worktrees, |_, _, _, cx| {
                cx.notify();
            }));
        }

        #[cfg(not(target_env = "ohos"))]
        let update_version = cx.new(|cx| UpdateVersion::new(cx));
        let platform_titlebar = cx.new(|cx| {
            let mut titlebar = PlatformTitleBar::new(id, cx);
            if let Some(mw) = multi_workspace.clone() {
                titlebar = titlebar.with_multi_workspace(mw);
            }
            titlebar
        });

        let banner = None;

        let this = Self {
            platform_titlebar,
            workspace: workspace.weak_handle(),
            multi_workspace,
            project,
            user_store,
            client,
            _subscriptions: subscriptions,
            banner,
            #[cfg(not(target_env = "ohos"))]
            update_version,
        };

        this
    }

    fn worktree_count(&self, cx: &App) -> usize {
        self.project.read(cx).visible_worktrees(cx).count()
    }

    #[cfg(not(target_env = "ohos"))]
    fn toggle_update_simulation(&mut self, cx: &mut Context<Self>) {
        self.update_version
            .update(cx, |banner, cx| banner.update_simulation(cx));
        cx.notify();
    }

    // [OHOS PORT BEGIN] HiCodeer has no update feed, so the title bar shows no
    // version badge next to the connection status.
    #[cfg(target_env = "ohos")]
    fn render_version_badge(&self, _cx: &mut Context<Self>) -> Option<AnyElement> {
        None
    }

    #[cfg(not(target_env = "ohos"))]
    fn render_version_badge(&self, _cx: &mut Context<Self>) -> Option<AnyElement> {
        Some(self.update_version.clone().into_any_element())
    }
    // [OHOS PORT END]

    /// Returns the worktree to display in the title bar.
    /// - Prefer the worktree owning the project's active repository
    /// - Fall back to the first visible worktree
    pub fn effective_active_worktree(&self, cx: &App) -> Option<Entity<project::Worktree>> {
        let project = self.project.read(cx);

        if let Some(repo) = project.active_repository(cx) {
            let repo = repo.read(cx);
            let repo_path = &repo.work_directory_abs_path;

            for worktree in project.visible_worktrees(cx) {
                let worktree_path = worktree.read(cx).abs_path();
                if worktree_path == *repo_path || worktree_path.starts_with(repo_path.as_ref()) {
                    return Some(worktree);
                }
            }
        }

        project.visible_worktrees(cx).next()
    }

    fn get_repository_for_worktree(
        &self,
        worktree: &Entity<project::Worktree>,
        cx: &App,
    ) -> Option<Entity<project::git_store::Repository>> {
        let project = self.project.read(cx);
        let git_store = project.git_store().read(cx);
        let worktree_path = worktree.read(cx).abs_path();

        git_store
            .repositories()
            .values()
            .filter(|repo| {
                let repo_path = &repo.read(cx).work_directory_abs_path;
                worktree_path == *repo_path || worktree_path.starts_with(repo_path.as_ref())
            })
            .max_by_key(|repo| repo.read(cx).work_directory_abs_path.as_os_str().len())
            .cloned()
    }

    fn visible_worktrees_in_repository(
        &self,
        repository: &project::git_store::Repository,
        cx: &App,
    ) -> usize {
        let repo_path = &repository.work_directory_abs_path;
        self.project
            .read(cx)
            .visible_worktrees(cx)
            .filter(|worktree| {
                let worktree_path = worktree.read(cx).abs_path();
                worktree_path == *repo_path || worktree_path.starts_with(repo_path.as_ref())
            })
            .count()
    }

    fn render_remote_project_connection(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let workspace = self.workspace.clone();

        let options = self.project.read(cx).remote_connection_options(cx)?;
        let host: SharedString = options.display_name().into();

        let (nickname, tooltip_title, icon) = match options {
            RemoteConnectionOptions::Ssh(options) => (
                options.nickname.map(|nick| nick.into()),
                localization::localized_str!("Remote Project"),
                IconName::Server,
            ),
            RemoteConnectionOptions::Wsl(_) => (
                None,
                localization::localized_str!("Remote Project"),
                IconName::Linux,
            ),
            RemoteConnectionOptions::Docker(_dev_container_connection) => (
                None,
                localization::localized_str!("Dev Container"),
                IconName::Box,
            ),
            #[cfg(any(test, feature = "test-support"))]
            RemoteConnectionOptions::Mock(_) => (None, "Mock Remote Project", IconName::Server),
        };

        let nickname = nickname.unwrap_or_else(|| host.clone());

        let (indicator_color, meta) = match self.project.read(cx).remote_connection_state(cx)? {
            remote::ConnectionState::Connecting => (Color::Info, {
                let __zed_i18n_arg_0 = format!("{}", host);
                localization::format_message("Connecting to: {host}", &[("host", __zed_i18n_arg_0)])
            }),
            remote::ConnectionState::Connected => (Color::Success, {
                let __zed_i18n_arg_0 = format!("{}", host);
                localization::format_message("Connected to: {host}", &[("host", __zed_i18n_arg_0)])
            }),
            remote::ConnectionState::HeartbeatMissed => (Color::Warning, {
                let __zed_i18n_arg_0 = format!("{}", host);
                localization::format_message(
                    "Connection attempt to {host} missed. Retrying...",
                    &[("host", __zed_i18n_arg_0)],
                )
            }),
            remote::ConnectionState::Reconnecting => (Color::Warning, {
                let __zed_i18n_arg_0 = format!("{}", host);
                localization::format_message(
                    "Lost connection to {host}. Reconnecting...",
                    &[("host", __zed_i18n_arg_0)],
                )
            }),
            remote::ConnectionState::Disconnected => (Color::Error, {
                let __zed_i18n_arg_0 = format!("{}", host);
                localization::format_message(
                    "Disconnected from {host}",
                    &[("host", __zed_i18n_arg_0)],
                )
            }),
        };

        let icon_color = match self.project.read(cx).remote_connection_state(cx)? {
            remote::ConnectionState::Connecting => Color::Info,
            remote::ConnectionState::Connected => Color::Default,
            remote::ConnectionState::HeartbeatMissed => Color::Warning,
            remote::ConnectionState::Reconnecting => Color::Warning,
            remote::ConnectionState::Disconnected => Color::Error,
        };

        let meta = SharedString::from(meta);

        Some(
            PopoverMenu::new("remote-project-menu")
                .menu(move |window, cx| {
                    let workspace_entity = workspace.upgrade()?;
                    let fs = workspace_entity.read(cx).project().read(cx).fs().clone();
                    Some(recent_projects::RemoteServerProjects::popover(
                        fs,
                        workspace.clone(),
                        None,
                        window,
                        cx,
                    ))
                })
                .trigger_with_tooltip(
                    ButtonLike::new("remote_project")
                        .selected_style(ButtonStyle::Tinted(TintColor::Accent))
                        .child(
                            h_flex()
                                .gap_2()
                                .max_w_32()
                                .child(
                                    IconWithIndicator::new(
                                        Icon::new(icon).size(IconSize::Small).color(icon_color),
                                        Some(Indicator::dot().color(indicator_color)),
                                    )
                                    .indicator_border_color(Some(
                                        cx.theme().colors().title_bar_background,
                                    ))
                                    .into_any_element(),
                                )
                                .child(Label::new(nickname).size(LabelSize::Small).truncate()),
                        ),
                    move |_window, cx| {
                        Tooltip::with_meta(
                            tooltip_title,
                            Some(&OpenRemote::default()),
                            meta.clone(),
                            cx,
                        )
                    },
                )
                .anchor(gpui::Anchor::TopLeft)
                .into_any_element(),
        )
    }

    pub fn render_restricted_mode(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let has_restricted_worktrees =
            TrustedWorktrees::has_restricted_worktrees(&self.project.read(cx).worktree_store(), cx);
        if !has_restricted_worktrees {
            return None;
        }

        let button = Button::new(
            "restricted_mode_trigger",
            localization::localized_str!("Restricted Mode"),
        )
        .style(ButtonStyle::Tinted(TintColor::Warning))
        .label_size(LabelSize::Small)
        .color(Color::Warning)
        .start_icon(
            Icon::new(IconName::Warning)
                .size(IconSize::Small)
                .color(Color::Warning),
        )
        .tooltip(|_, cx| {
            Tooltip::with_meta(
                localization::localized_str!("You're in Restricted Mode"),
                Some(&ToggleWorktreeSecurity),
                localization::localized_str!(
                    "Mark this project as trusted and unlock all features"
                ),
                cx,
            )
        })
        .on_click({
            cx.listener(move |this, _, window, cx| {
                this.workspace
                    .update(cx, |workspace, cx| {
                        workspace.show_worktree_trust_security_modal(true, window, cx)
                    })
                    .log_err();
            })
        });

        if ui::utils::MACOS_SDK_26_OR_LATER {
            // Make up for Tahoe's traffic light buttons having less spacing around them
            Some(div().child(button).ml_0p5().into_any_element())
        } else {
            Some(button.into_any_element())
        }
    }

    pub fn render_project_host(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if self.project.read(cx).is_via_remote_server() {
            return self.render_remote_project_connection(cx);
        }

        if self.project.read(cx).is_disconnected(cx) {
            return Some(
                Button::new("disconnected", localization::localized_str!("Disconnected"))
                    .disabled(true)
                    .color(Color::Disabled)
                    .label_size(LabelSize::Small)
                    .into_any_element(),
            );
        }

        let host = self.project.read(cx).host()?;
        let host_user = self.user_store.read(cx).get_cached_user(host.user_id)?;
        let participant_index = self
            .user_store
            .read(cx)
            .participant_indices()
            .get(&host_user.legacy_id)?;

        Some(
            Button::new("project_owner_trigger", host_user.username.clone())
                .color(Color::Player(participant_index.0))
                .label_size(LabelSize::Small)
                .tab_index(0isize)
                .tooltip(move |_, cx| {
                    let tooltip_title = {
                        let __zed_i18n_arg_0 = format!("{}", host_user.username);
                        localization::format_message(
                            "{} is sharing this project. Click to follow.",
                            &[("0", __zed_i18n_arg_0)],
                        )
                    };

                    Tooltip::with_meta(
                        tooltip_title,
                        None,
                        localization::localized_str!("Click to Follow"),
                        cx,
                    )
                })
                .on_click({
                    let host_peer_id = host.peer_id;
                    cx.listener(move |this, _, window, cx| {
                        this.workspace
                            .update(cx, |workspace, cx| {
                                workspace.follow(host_peer_id, window, cx);
                            })
                            .log_err();
                    })
                })
                .into_any_element(),
        )
    }

    fn render_project_name(
        &self,
        name: Option<SharedString>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let workspace = self.workspace.clone();

        let is_project_selected = name.is_some();

        let display_name = if let Some(ref name) = name {
            util::truncate_and_trailoff(name, MAX_PROJECT_NAME_LENGTH)
        } else {
            localization::localized_str!("Open Recent Project").to_string()
        };

        let is_sidebar_open = self
            .multi_workspace
            .as_ref()
            .and_then(|mw| mw.upgrade())
            .map(|mw| mw.read(cx).sidebar_open())
            .unwrap_or(false)
            && PlatformTitleBar::is_multi_workspace_enabled(cx);

        let is_threads_list_view_active = self
            .multi_workspace
            .as_ref()
            .and_then(|mw| mw.upgrade())
            .map(|mw| mw.read(cx).is_threads_list_view_active(cx))
            .unwrap_or(false);

        if is_sidebar_open && is_threads_list_view_active {
            return self
                .render_recent_projects_popover(display_name, is_project_selected, cx)
                .into_any_element();
        }

        let focus_handle = workspace
            .upgrade()
            .map(|w| w.read(cx).focus_handle(cx))
            .unwrap_or_else(|| cx.focus_handle());

        let window_project_groups: Vec<_> = self
            .multi_workspace
            .as_ref()
            .and_then(|mw| mw.upgrade())
            .map(|mw| mw.read(cx).project_group_keys())
            .unwrap_or_default();

        PopoverMenu::new("recent-projects-menu")
            .menu(move |window, cx| {
                Some(recent_projects::RecentProjects::popover(
                    workspace.clone(),
                    window_project_groups.clone(),
                    None,
                    focus_handle.clone(),
                    window,
                    cx,
                ))
            })
            .trigger_with_tooltip(
                Button::new("project_name_trigger", display_name)
                    .label_size(LabelSize::Small)
                    .tab_index(0isize)
                    .when(self.worktree_count(cx) > 1, |this| {
                        this.end_icon(
                            Icon::new(IconName::ChevronDown)
                                .size(IconSize::XSmall)
                                .color(Color::Muted),
                        )
                    })
                    .selected_style(ButtonStyle::Tinted(TintColor::Accent))
                    .when(!is_project_selected, |s| s.color(Color::Muted)),
                move |_window, cx| {
                    Tooltip::for_action(
                        localization::localized_str!("Recent Projects"),
                        &zed_actions::OpenRecent::default(),
                        cx,
                    )
                },
            )
            .anchor(gpui::Anchor::TopLeft)
            .into_any_element()
    }

    fn render_recent_projects_popover(
        &self,
        display_name: String,
        is_project_selected: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let workspace = self.workspace.clone();

        let focus_handle = workspace
            .upgrade()
            .map(|w| w.read(cx).focus_handle(cx))
            .unwrap_or_else(|| cx.focus_handle());

        let window_project_groups: Vec<_> = self
            .multi_workspace
            .as_ref()
            .and_then(|mw| mw.upgrade())
            .map(|mw| mw.read(cx).project_group_keys())
            .unwrap_or_default();

        PopoverMenu::new("sidebar-title-recent-projects-menu")
            .menu(move |window, cx| {
                Some(recent_projects::RecentProjects::popover(
                    workspace.clone(),
                    window_project_groups.clone(),
                    None,
                    focus_handle.clone(),
                    window,
                    cx,
                ))
            })
            .trigger_with_tooltip(
                Button::new("project_name_trigger", display_name)
                    .label_size(LabelSize::Small)
                    .tab_index(0isize)
                    .when(self.worktree_count(cx) > 1, |this| {
                        this.end_icon(
                            Icon::new(IconName::ChevronDown)
                                .size(IconSize::XSmall)
                                .color(Color::Muted),
                        )
                    })
                    .selected_style(ButtonStyle::Tinted(TintColor::Accent))
                    .when(!is_project_selected, |s| s.color(Color::Muted)),
                move |_window, cx| {
                    Tooltip::for_action(
                        localization::localized_str!("Recent Projects"),
                        &zed_actions::OpenRecent::default(),
                        cx,
                    )
                },
            )
            .anchor(gpui::Anchor::TopLeft)
    }

    fn render_worktree_and_branch(
        &self,
        repository: Entity<project::git_store::Repository>,
        linked_worktree_name: Option<SharedString>,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let workspace = self.workspace.upgrade()?;

        let (branch_name, icon_info, is_detached_head) = {
            let repo = repository.read(cx);

            let is_detached_head = repo.branch.is_none();

            let branch_name = repo
                .branch
                .as_ref()
                .map(|branch| branch.name())
                .map(|name| util::truncate_and_trailoff(name, MAX_BRANCH_NAME_LENGTH))
                .or_else(|| {
                    repo.head_commit.as_ref().map(|commit| {
                        commit
                            .sha
                            .chars()
                            .take(MAX_SHORT_SHA_LENGTH)
                            .collect::<String>()
                    })
                });

            let status = repo.status_summary();
            let tracked = status.index + status.worktree;
            let icon_info = if status.conflict > 0 {
                (IconName::Warning, Color::VersionControlConflict)
            } else if tracked.modified > 0 {
                (IconName::SquareDot, Color::VersionControlModified)
            } else if tracked.added > 0 || status.untracked > 0 {
                (IconName::SquarePlus, Color::VersionControlAdded)
            } else if tracked.deleted > 0 {
                (IconName::SquareMinus, Color::VersionControlDeleted)
            } else {
                (IconName::GitBranch, Color::Muted)
            };

            (branch_name, icon_info, is_detached_head)
        };

        let settings = TitleBarSettings::get_global(cx);
        let effective_repository = Some(repository);

        let worktree_label: SharedString = linked_worktree_name.unwrap_or_else(|| "main".into());

        let (creation_in_progress, is_switch) = self
            .workspace
            .upgrade()
            .map(|ws| {
                let creation = ws.read(cx).active_worktree_creation();
                (creation.label.clone(), creation.is_switch)
            })
            .unwrap_or((None, false));
        let is_creating = creation_in_progress.is_some();

        let display_label: SharedString = if let Some(ref name) = creation_in_progress {
            if is_switch {
                format!("Loading {}…", name).into()
            } else {
                format!("Creating {}…", name).into()
            }
        } else {
            worktree_label.clone()
        };

        let worktree_button = settings.show_worktree_name.then(|| {
            let project = self.project.clone();
            let workspace_handle = workspace.downgrade();
            PopoverMenu::new("worktree-picker-menu")
                .menu(move |window, cx| {
                    // When opened from the title bar, focus is on the trigger
                    // button (not a dock), so `focused_dock` is `None`. That's
                    // fine — there's no prior dock focus to restore.
                    Some(cx.new(|cx| {
                        WorktreePicker::new(project.clone(), workspace_handle.clone(), window, cx)
                    }))
                })
                .trigger_with_tooltip(
                    Button::new("worktree_picker_trigger", display_label)
                        .selected_style(ButtonStyle::Tinted(TintColor::Accent))
                        .label_size(LabelSize::Small)
                        .color(Color::Muted)
                        .tab_index(0isize)
                        .loading(is_creating)
                        .start_icon(
                            Icon::new(IconName::GitWorktree)
                                .size(IconSize::XSmall)
                                .color(Color::Muted),
                        ),
                    move |_window, cx| {
                        Tooltip::with_meta(
                            localization::localized_str!("Worktree"),
                            Some(&zed_actions::git::Worktree),
                            {
                                let __zed_i18n_arg_0 = format!("{}", worktree_label);
                                localization::format_message(
                                    "Currently In Use: {}",
                                    &[("0", __zed_i18n_arg_0)],
                                )
                            },
                            cx,
                        )
                    },
                )
                .anchor(gpui::Anchor::TopLeft)
        });

        let branch_picker = branch_name.and_then(|branch_name| {
            settings.show_branch_name.then(|| {
                let branch_tooltip_label = branch_name.clone();
                let (branch_icon, branch_icon_color) = if settings.show_branch_status_icon {
                    icon_info
                } else {
                    (IconName::GitBranch, Color::Muted)
                };

                let trigger = if is_detached_head {
                    Button::new(
                        "project_branch_trigger",
                        localization::localized_str!("Create Branch"),
                    )
                    .selected_style(ButtonStyle::Tinted(TintColor::Accent))
                    .label_size(LabelSize::Small)
                    .tab_index(0isize)
                    .start_icon(
                        Icon::new(IconName::GitBranchPlus)
                            .size(IconSize::XSmall)
                            .color(Color::Muted),
                    )
                } else {
                    Button::new("project_branch_trigger", branch_name)
                        .selected_style(ButtonStyle::Tinted(TintColor::Accent))
                        .label_size(LabelSize::Small)
                        .color(Color::Muted)
                        .tab_index(0isize)
                        .start_icon(
                            Icon::new(branch_icon)
                                .size(IconSize::XSmall)
                                .color(branch_icon_color),
                        )
                };

                PopoverMenu::new("branch-menu")
                    .menu(move |window, cx| {
                        git_ui_core::build_branch_picker(
                            workspace.downgrade(),
                            effective_repository.clone(),
                            window,
                            cx,
                        )
                    })
                    .trigger_with_tooltip(trigger, move |_window, cx| {
                        let meta = if is_detached_head {
                            {
                                let __zed_i18n_arg_0 = format!("{}", branch_tooltip_label);
                                localization::format_message(
                                    "Detached HEAD: {}",
                                    &[("0", __zed_i18n_arg_0)],
                                )
                            }
                        } else {
                            {
                                let __zed_i18n_arg_0 = format!("{}", branch_tooltip_label);
                                localization::format_message(
                                    "Currently Checked Out: {}",
                                    &[("0", __zed_i18n_arg_0)],
                                )
                            }
                        };
                        Tooltip::with_meta(
                            localization::localized_str!("Branch & Stash"),
                            Some(&zed_actions::git::Branch),
                            meta,
                            cx,
                        )
                    })
                    .anchor(gpui::Anchor::TopLeft)
            })
        });

        if worktree_button.is_none() && branch_picker.is_none() {
            return None;
        }

        let show_separator = worktree_button.is_some() && branch_picker.is_some();

        Some(
            h_flex()
                .gap_px()
                .children(worktree_button)
                .when(show_separator, |this| {
                    this.child(
                        Label::new("/")
                            .size(LabelSize::Small)
                            .color(Color::Muted)
                            .alpha(0.25),
                    )
                })
                .children(branch_picker)
                .into_any_element(),
        )
    }

    fn render_connection_status(
        &self,
        status: &client::Status,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        match status {
            client::Status::ConnectionError
            | client::Status::ConnectionLost
            | client::Status::Reauthenticating
            | client::Status::Reconnecting
            | client::Status::ReconnectionError { .. } => Some(
                div()
                    .id("disconnected")
                    .child(Icon::new(IconName::Disconnected).size(IconSize::Small))
                    .tooltip(Tooltip::text(localization::localized_str!("Disconnected")))
                    .into_any_element(),
            ),
            // [OHOS PORT BEGIN] Collaboration is not part of HiCodeer, so the
            // "upgrade to collaborate" prompt and its updater lookup do not
            // exist here.
            #[cfg(not(target_env = "ohos"))]
            client::Status::UpgradeRequired => {
                let auto_updater = auto_update::AutoUpdater::get(cx);
                let label = match auto_updater.map(|auto_update| auto_update.read(cx).status()) {
                    Some(AutoUpdateStatus::Updated { .. }) => {
                        localization::localized_str!("Please restart Zed to Collaborate")
                    }
                    Some(AutoUpdateStatus::Installing { .. })
                    | Some(AutoUpdateStatus::Downloading { .. })
                    | Some(AutoUpdateStatus::Checking) => {
                        localization::localized_str!("Updating...")
                    }
                    Some(AutoUpdateStatus::Idle)
                    | Some(AutoUpdateStatus::Errored { .. })
                    | None => localization::localized_str!("Please update Zed to Collaborate"),
                };

                Some(
                    Button::new("connection-status", label)
                        .label_size(LabelSize::Small)
                        .on_click(|_, window, cx| {
                            if let Some(auto_updater) = auto_update::AutoUpdater::get(cx)
                                && auto_updater.read(cx).status().is_updated()
                            {
                                workspace::reload(cx);
                                return;
                            }
                            auto_update::check(&Default::default(), window, cx);
                        })
                        .into_any_element(),
                )
            }
            _ => None,
        }
    }

    pub fn render_user_menu_button(&mut self, cx: &mut Context<Self>) -> impl Element {
        // [OHOS PORT BEGIN] HiCodeer has no update feed, so the menu never
        // offers to restart into a new version.
        #[cfg(not(target_env = "ohos"))]
        let show_update_button = self.update_version.read(cx).show_update_in_menu_bar();
        // [OHOS PORT END]

        let trigger = ButtonLike::new("user-menu")
            .aria_label(localization::localized_str!("User menu"))
            .tab_index(0isize)
            .child(Icon::new(IconName::Menu).size(IconSize::Small));

        PopoverMenu::new("user-menu")
            .trigger(trigger)
            .menu(move |window, cx| {
                ContextMenu::build(window, cx, |menu, _, _cx| {
                    // [OHOS PORT BEGIN] HiCodeer has no update feed, so the menu
                    // carries no restart-to-update entry.
                    #[cfg(not(target_env = "ohos"))]
                    let menu = menu
                        .when(show_update_button, |this| {
                            this.custom_entry(
                                move |_window, _cx| {
                                    h_flex()
                                        .w_full()
                                        .gap_1()
                                        .justify_between()
                                        .child(
                                            Label::new(localization::localized_str!(
                                                "Restart to update Zed"
                                            ))
                                            .color(Color::Accent),
                                        )
                                        .child(
                                            Icon::new(IconName::Download)
                                                .size(IconSize::Small)
                                                .color(Color::Accent),
                                        )
                                        .into_any_element()
                                },
                                move |_, cx| {
                                    workspace::reload(cx);
                                },
                            )
                            .separator()
                        });
                    // [OHOS PORT END]
                    let menu = menu.action(
                        localization::localized_str!("Settings"),
                        zed_actions::OpenSettings.boxed_clone(),
                    )
                    .action(
                        localization::localized_str!("Keymap"),
                        Box::new(zed_actions::OpenKeymap),
                    )
                    .action(
                        localization::localized_str!("Themes…"),
                        zed_actions::theme_selector::Toggle::default().boxed_clone(),
                    )
                    .action(
                        localization::localized_str!("Icon Themes…"),
                        zed_actions::icon_theme_selector::Toggle::default().boxed_clone(),
                    )
                    .action(
                        localization::localized_str!("Extensions"),
                        zed_actions::Extensions::default().boxed_clone(),
                    );
                    #[cfg(target_env = "ohos")]
                    let menu = menu.submenu(
                        localization::localized_str!("Panel Layout"),
                        move |menu, _window, cx| {
                            let current = current_layout(cx);
                            menu.toggleable_entry(
                                localization::localized_str!("Classic"),
                                current == WindowLayout::Editor,
                                IconPosition::Start,
                                Some(UseClassicLayout.boxed_clone()),
                                move |window, cx| {
                                    window.dispatch_action(UseClassicLayout.boxed_clone(), cx);
                                },
                            )
                            .toggleable_entry(
                                localization::localized_str!("Agentic"),
                                current == WindowLayout::Agent,
                                IconPosition::Start,
                                Some(UseAgenticLayout.boxed_clone()),
                                move |window, cx| {
                                    window.dispatch_action(UseAgenticLayout.boxed_clone(), cx);
                                },
                            )
                            .when(
                                current == WindowLayout::Custom,
                                |menu| {
                                    menu.item(
                                        ContextMenuEntry::new(localization::localized_str!(
                                            "Custom"
                                        ))
                                        .toggleable(IconPosition::Start, true)
                                        .disabled(true),
                                    )
                                },
                            )
                        },
                    );
                    menu
                })
                .into()
            })
            .anchor(Anchor::TopRight)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use util::paths::PathStyle;

    #[test]
    fn test_foreign_path_style_does_not_use_repository_identity_as_name_anchor() {
        let (common_dir, foreign_path_style) = match PathStyle::local() {
            PathStyle::Unix => (Path::new(r"C:\repos\zed"), PathStyle::Windows),
            PathStyle::Windows => (Path::new("/repos/zed"), PathStyle::Unix),
        };
        let repository_identity_path = repo_identity_path_if_local(common_dir, foreign_path_style);

        assert_eq!(
            linked_worktree_name_anchor(None, repository_identity_path, true),
            None
        );
    }

    #[test]
    fn test_local_path_style_uses_bare_repository_worktree_name() {
        let (repository_identity_path, work_directory_path) = match PathStyle::local() {
            PathStyle::Unix => (
                Path::new("/repos/zed"),
                Path::new("/worktrees/zed/plum-warbler/zed"),
            ),
            PathStyle::Windows => (
                Path::new(r"C:\repos\zed"),
                Path::new(r"C:\worktrees\zed\plum-warbler\zed"),
            ),
        };

        let repository_identity_path =
            repo_identity_path_if_local(repository_identity_path, PathStyle::local());
        let name_anchor_path = linked_worktree_name_anchor(None, repository_identity_path, true);

        assert_eq!(
            name_anchor_path.and_then(|name_anchor_path| {
                linked_worktree_short_name(
                    name_anchor_path,
                    work_directory_path,
                    PathStyle::local(),
                )
            }),
            Some("plum-warbler".into())
        );
    }
}
