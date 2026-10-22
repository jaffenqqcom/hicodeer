use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use anyhow::{Context as _, Result};
use askpass::EncryptedPassword;
use editor::Editor;
use futures::{FutureExt as _, channel::oneshot, select};
use gpui::{AppContext, AsyncApp, PromptLevel, WindowHandle};

use project::trusted_worktrees;
use remote::{
    DockerConnectionOptions, Interactive, RemoteConnection, RemoteConnectionOptions,
    SshConnectionOptions,
};
pub use settings::SshConnection;
use settings::{DevContainerConnection, ExtendingVec, RegisterSetting, Settings, WslConnection};
use util::paths::PathWithPosition;
use workspace::{
    AppState, MultiWorkspace, OpenOptions, SerializedWorkspaceLocation, Workspace,
    find_existing_workspace,
};

pub use remote_connection::{
    RemoteClientDelegate, RemoteConnectionModal, RemoteConnectionPrompt, SshConnectionHeader,
    connect,
};

#[derive(RegisterSetting)]
pub struct RemoteSettings {
    pub ssh_connections: ExtendingVec<SshConnection>,
    pub wsl_connections: ExtendingVec<WslConnection>,
    /// Whether to read ~/.ssh/config for ssh connection sources.
    pub read_ssh_config: bool,
}

impl RemoteSettings {
    pub fn ssh_connections(&self) -> impl Iterator<Item = SshConnection> + use<> {
        self.ssh_connections.clone().0.into_iter()
    }

    pub fn wsl_connections(&self) -> impl Iterator<Item = WslConnection> + use<> {
        self.wsl_connections.clone().0.into_iter()
    }

    pub fn fill_connection_options_from_settings(&self, options: &mut SshConnectionOptions) {
        for conn in self.ssh_connections() {
            if conn.host == options.host.to_string()
                && conn.username == options.username
                && conn.port == options.port
            {
                options.nickname = conn.nickname;
                options.upload_binary_over_ssh = conn.upload_binary_over_ssh.unwrap_or_default();
                options.args = Some(conn.args);
                options.port_forwards = conn.port_forwards;
                break;
            }
        }
    }

    pub fn connection_options_for(
        &self,
        host: String,
        port: Option<u16>,
        username: Option<String>,
    ) -> SshConnectionOptions {
        let mut options = SshConnectionOptions {
            host: host.into(),
            port,
            username,
            ..Default::default()
        };
        self.fill_connection_options_from_settings(&mut options);
        options
    }
}

#[derive(Clone, PartialEq)]
pub enum Connection {
    Ssh(SshConnection),
    Wsl(WslConnection),
    DevContainer(DevContainerConnection),
}

impl From<Connection> for RemoteConnectionOptions {
    fn from(val: Connection) -> Self {
        match val {
            Connection::Ssh(conn) => RemoteConnectionOptions::Ssh(conn.into()),
            Connection::Wsl(conn) => RemoteConnectionOptions::Wsl(conn.into()),
            Connection::DevContainer(conn) => {
                RemoteConnectionOptions::Docker(DockerConnectionOptions {
                    name: conn.name,
                    remote_user: conn.remote_user,
                    container_id: conn.container_id,
                    upload_binary_over_docker_exec: false,
                    use_podman: conn.use_podman,
                    remote_env: conn.remote_env,
                })
            }
        }
    }
}

impl From<SshConnection> for Connection {
    fn from(val: SshConnection) -> Self {
        Connection::Ssh(val)
    }
}

impl From<WslConnection> for Connection {
    fn from(val: WslConnection) -> Self {
        Connection::Wsl(val)
    }
}

impl Settings for RemoteSettings {
    fn from_settings(content: &settings::SettingsContent) -> Self {
        let remote = &content.remote;
        Self {
            ssh_connections: remote.ssh_connections.clone().unwrap_or_default().into(),
            wsl_connections: remote.wsl_connections.clone().unwrap_or_default().into(),
            read_ssh_config: remote.read_ssh_config.unwrap(),
        }
    }
}

pub async fn open_remote_project(
    connection_options: RemoteConnectionOptions,
    paths: Vec<PathBuf>,
    app_state: Arc<AppState>,
    open_options: workspace::OpenOptions,
    cx: &mut AsyncApp,
) -> Result<WindowHandle<MultiWorkspace>> {
    let created_new_window = open_options.requesting_window.is_none();

    let (existing, open_visible) = find_existing_workspace(
        &paths,
        &open_options,
        &SerializedWorkspaceLocation::Remote(connection_options.clone()),
        cx,
    )
    .await;

    if let Some((existing_window, existing_workspace)) = existing {
        let remote_connection = cx.update(|cx| {
            existing_workspace
                .read(cx)
                .project()
                .read(cx)
                .remote_client()
                .and_then(|client| client.read(cx).remote_connection())
        });

        if let Some(remote_connection) = remote_connection {
            let (resolved_paths, paths_with_positions) =
                determine_paths_with_positions(&remote_connection, paths).await;

            let open_results = existing_window
                .update(cx, |multi_workspace, window, cx| {
                    window.activate_window();
                    multi_workspace.activate(existing_workspace.clone(), None, window, cx);
                    existing_workspace.update(cx, |workspace, cx| {
                        workspace.open_paths(
                            resolved_paths,
                            OpenOptions {
                                visible: Some(open_visible),
                                ..Default::default()
                            },
                            None,
                            window,
                            cx,
                        )
                    })
                })?
                .await;

            _ = existing_window.update(cx, |multi_workspace, _, cx| {
                let workspace = multi_workspace.workspace().clone();
                workspace.update(cx, |workspace, cx| {
                    for item in open_results.iter().flatten() {
                        if let Err(e) = item {
                            workspace.show_error(format!("{e}"), cx);
                        }
                    }
                });
            });

            let items = open_results
                .into_iter()
                .map(|r| r.and_then(|r| r.ok()))
                .collect::<Vec<_>>();
            navigate_to_positions(&existing_window, items, &paths_with_positions, cx);

            return Ok(existing_window);
        }
        // If the remote connection is dead (e.g. server not running after failed reconnect),
        // fall through to establish a fresh connection instead of showing an error.
        log::info!(
            "existing remote workspace found but connection is dead, starting fresh connection"
        );
    }

    let (window, initial_workspace) = if let Some(window) = open_options.requesting_window {
        let workspace = window.update(cx, |multi_workspace, _, _| {
            multi_workspace.workspace().clone()
        })?;
        (window, workspace)
    } else {
        let workspace_position = cx
            .update(|cx| {
                workspace::remote_workspace_position_from_db(connection_options.clone(), &paths, cx)
            })
            .await
            .context("fetching remote workspace position from db")?;

        let mut options =
            cx.update(|cx| (app_state.build_window_options)(workspace_position.display, cx));
        options.window_bounds = workspace_position.window_bounds;

        let window = cx.open_window(options, |window, cx| {
            let project = project::Project::local(
                app_state.client.clone(),
                app_state.node_runtime.clone(),
                app_state.user_store.clone(),
                app_state.languages.clone(),
                app_state.fs.clone(),
                None,
                project::LocalProjectFlags {
                    init_worktree_trust: false,
                    ..Default::default()
                },
                cx,
            );
            let workspace = cx.new(|cx| {
                let mut workspace = Workspace::new(None, project, app_state.clone(), window, cx);
                workspace.centered_layout = workspace_position.centered_layout;
                workspace
            });
            cx.new(|cx| MultiWorkspace::new(workspace, window, cx))
        })?;
        let workspace = window.update(cx, |multi_workspace, _, _cx| {
            multi_workspace.workspace().clone()
        })?;
        (window, workspace)
    };

    loop {
        let (cancel_tx, mut cancel_rx) = oneshot::channel();
        let delegate = window.update(cx, {
            let paths = paths.clone();
            let connection_options = connection_options.clone();
            let initial_workspace = initial_workspace.clone();
            move |_multi_workspace: &mut MultiWorkspace, window, cx| {
                window.activate_window();
                initial_workspace.update(cx, |workspace, cx| {
                    workspace.hide_modal(window, cx);
                    workspace.toggle_modal(window, cx, |window, cx| {
                        RemoteConnectionModal::new(&connection_options, paths, window, cx)
                    });

                    let ui = workspace
                        .active_modal::<RemoteConnectionModal>(cx)?
                        .read(cx)
                        .prompt
                        .clone();

                    ui.update(cx, |ui, _cx| {
                        ui.set_cancellation_tx(cancel_tx);
                    });

                    Some(Arc::new(RemoteClientDelegate::new(
                        window.window_handle(),
                        ui.downgrade(),
                        if let RemoteConnectionOptions::Ssh(options) = &connection_options {
                            options
                                .password
                                .as_deref()
                                .and_then(|pw| EncryptedPassword::try_from(pw).ok())
                        } else {
                            None
                        },
                    )))
                })
            }
        })?;

        let Some(delegate) = delegate else { break };

        let connection = remote::connect(connection_options.clone(), delegate.clone(), cx);
        let connection = select! {
            _ = cancel_rx => {
                initial_workspace.update(cx, |workspace, cx| {
                    if let Some(ui) = workspace.active_modal::<RemoteConnectionModal>(cx) {
                        ui.update(cx, |modal, cx| modal.finished(cx))
                    }
                });

                break;
            },
            result = connection.fuse() => result,
        };
        let remote_connection = match connection {
            Ok(connection) => connection,
            Err(e) => {
                initial_workspace.update(cx, |workspace, cx| {
                    if let Some(ui) = workspace.active_modal::<RemoteConnectionModal>(cx) {
                        ui.update(cx, |modal, cx| modal.finished(cx))
                    }
                });
                log::error!("Failed to open project: {e:#}");
                let response = window
                    .update(cx, |_, window, cx| {
                        window.prompt(
                            PromptLevel::Critical,
                            match connection_options {
                                RemoteConnectionOptions::Ssh(_) => localization::localized_str!("Failed to connect over SSH"),
                                RemoteConnectionOptions::Wsl(_) => localization::localized_str!("Failed to connect to WSL"),
                                RemoteConnectionOptions::Docker(_) => {
                                    localization::localized_str!("Failed to connect to Dev Container")
                                }
                                #[cfg(any(test, feature = "test-support"))]
                                RemoteConnectionOptions::Mock(_) => {
                                    localization::localized_str!("Failed to connect to mock server")
                                }
                            },
                            Some(&format!("{e:#}")),
                            &[localization::localized_str!("Retry"), localization::localized_str!("Cancel")],
                            cx,
                        )
                    })?
                    .await;

                if response == Ok(0) {
                    continue;
                }

                if created_new_window {
                    window
                        .update(cx, |_, window, _| window.remove_window())
                        .ok();
                }
                return Ok(window);
            }
        };

        let (paths, paths_with_positions) =
            determine_paths_with_positions(&remote_connection, paths.clone()).await;

        let opened = cx
            .update(|cx| {
                workspace::open_remote_project_with_new_connection(
                    window,
                    remote_connection,
                    cancel_rx,
                    delegate.clone(),
                    app_state.clone(),
                    paths.clone(),
                    cx,
                )
            })
            .await;

        initial_workspace.update(cx, |workspace, cx| {
            if let Some(ui) = workspace.active_modal::<RemoteConnectionModal>(cx) {
                ui.update(cx, |modal, cx| modal.finished(cx))
            }
        });

        match opened {
            Err(e) => {
                log::error!("Failed to open project: {e:#}");
                let response = window
                    .update(cx, |_, window, cx| {
                        window.prompt(
                            PromptLevel::Critical,
                            match connection_options {
                                RemoteConnectionOptions::Ssh(_) => localization::localized_str!("Failed to connect over SSH"),
                                RemoteConnectionOptions::Wsl(_) => localization::localized_str!("Failed to connect to WSL"),
                                RemoteConnectionOptions::Docker(_) => {
                                    localization::localized_str!("Failed to connect to Dev Container")
                                }
                                #[cfg(any(test, feature = "test-support"))]
                                RemoteConnectionOptions::Mock(_) => {
                                    localization::localized_str!("Failed to connect to mock server")
                                }
                            },
                            Some(&format!("{e:#}")),
                            &[localization::localized_str!("Retry"), localization::localized_str!("Cancel")],
                            cx,
                        )
                    })?
                    .await;
                if response == Ok(0) {
                    continue;
                }

                if created_new_window {
                    window
                        .update(cx, |_, window, _| window.remove_window())
                        .ok();
                }
                initial_workspace.update(cx, |workspace, cx| {
                    trusted_worktrees::track_worktree_trust(
                        workspace.project().read(cx).worktree_store(),
                        None,
                        None,
                        None,
                        cx,
                    );
                });
            }

            Ok((_, items)) => {
                navigate_to_positions(&window, items, &paths_with_positions, cx);
            }
        }

        break;
    }

    Ok(window)
}

pub fn navigate_to_positions(
    window: &WindowHandle<MultiWorkspace>,
    items: impl IntoIterator<Item = Option<Box<dyn workspace::item::ItemHandle>>>,
    positions: &[PathWithPosition],
    cx: &mut AsyncApp,
) {
    for (item, path) in items.into_iter().zip(positions) {
        let Some(item) = item else {
            continue;
        };
        let Some(row) = path.row else {
            continue;
        };
        if let Some(active_editor) = item.downcast::<Editor>() {
            window
                .update(cx, |_, window, cx| {
                    active_editor.update(cx, |editor, cx| {
                        let row = row.saturating_sub(1);
                        let col = path.column.unwrap_or(0).saturating_sub(1);
                        let Some(buffer) = editor.buffer().read(cx).as_singleton() else {
                            return;
                        };
                        let buffer_snapshot = buffer.read(cx).snapshot();
                        let point = buffer_snapshot.point_from_external_input(row, col);
                        editor.go_to_singleton_buffer_point(point, window, cx);
                    });
                })
                .ok();
        }
    }
}

pub(crate) async fn determine_paths_with_positions(
    remote_connection: &Arc<dyn RemoteConnection>,
    mut paths: Vec<PathBuf>,
) -> (Vec<PathBuf>, Vec<PathWithPosition>) {
    let mut paths_with_positions = Vec::<PathWithPosition>::new();
    for path in &mut paths {
        if let Some(path_str) = path.to_str() {
            let path_with_position = PathWithPosition::parse_str(&path_str);
            if path_with_position.row.is_some() {
                if !path_exists(&remote_connection, &path).await {
                    *path = path_with_position.path.clone();
                    paths_with_positions.push(path_with_position);
                    continue;
                }
            }
        }
        paths_with_positions.push(PathWithPosition::from_path(path.clone()))
    }
    (paths, paths_with_positions)
}

async fn path_exists(connection: &Arc<dyn RemoteConnection>, path: &Path) -> bool {
    let Ok(command) = connection.build_command(
        Some("test".to_string()),
        &["-e".to_owned(), path.to_string_lossy().to_string()],
        &Default::default(),
        None,
        None,
        Interactive::No,
    ) else {
        return false;
    };
    let Ok(mut child) = util::command::new_command(command.program)
        .args(command.args)
        .envs(command.env)
        .spawn()
    else {
        return false;
    };
    child.status().await.is_ok_and(|status| status.success())
}
