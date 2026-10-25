//! Reading and driving the agent server over the daemon's control bridge.
//!
//! The Agent panel shows exactly what the server's own tree shows, so it reads
//! the JSON API instead of keeping a parallel model: `workspace.list` gives the
//! spaces, and `agent.list` gives the agents with the status the server itself
//! computed. One request is served per connection, so each call opens its own
//! short connection over the token-guarded loopback bridge.

use std::io::{BufRead, BufReader, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpStream};
use std::time::Duration;

use cmd_client::protocol::{AGENT_CONTROL_TOKEN_PREFIX, AgentControl};
use serde_json::{Value, json};

use crate::herdr_settings::{Integration, IntegrationState};
use crate::snapshot::Snapshot;

/// Bound on establishing the bridge connection: it is loopback to a process on
/// this device, so anything slower means the daemon is gone rather than busy.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(2);
/// Bound on one request/response. The server answers these from memory.
const READ_TIMEOUT: Duration = Duration::from_secs(3);
/// Bound on an integration install. Installing writes files under the agent's
/// own directory, so it is not an in-memory answer like the rest.
const INSTALL_TIMEOUT: Duration = Duration::from_secs(30);

/// Why a call produced nothing.
pub(crate) enum ClientError {
    /// The daemon has not reported an embedded agent endpoint yet, or its
    /// management channel is down. Normal while the editor starts.
    NotReady,
    /// The bridge or the API refused the request.
    Failed(String),
}

impl std::fmt::Display for ClientError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ClientError::NotReady => write!(formatter, "the agent endpoint is not ready"),
            ClientError::Failed(message) => write!(formatter, "{message}"),
        }
    }
}

/// Reads the current spaces and the agents running in them.
pub(crate) fn fetch() -> Result<Snapshot, ClientError> {
    let control = cmd_client::agent_control().ok_or(ClientError::NotReady)?;
    let workspaces = call(&control, "workspace.list", json!({}))?;
    let agents = call(&control, "agent.list", json!({}))?;
    Snapshot::from_api(&workspaces, &agents)
}

/// Focuses one space, which switches the TUI to it.
pub(crate) fn focus_space(workspace_id: &str) -> Result<(), ClientError> {
    let control = cmd_client::agent_control().ok_or(ClientError::NotReady)?;
    call(
        &control,
        "workspace.focus",
        json!({ "workspace_id": workspace_id }),
    )?;
    Ok(())
}

/// Focuses one agent's pane. `target` must be the agent's public pane id: the
/// server resolves that or an agent name, and a terminal id resolves to
/// nothing.
pub(crate) fn focus_agent(pane_id: &str) -> Result<(), ClientError> {
    let control = cmd_client::agent_control().ok_or(ClientError::NotReady)?;
    call(&control, "agent.focus", json!({ "target": pane_id }))?;
    Ok(())
}

/// Renames one space.
pub(crate) fn rename_space(workspace_id: &str, label: &str) -> Result<(), ClientError> {
    let control = cmd_client::agent_control().ok_or(ClientError::NotReady)?;
    call(
        &control,
        "workspace.rename",
        json!({ "workspace_id": workspace_id, "label": label }),
    )?;
    Ok(())
}

/// Closes one space.
pub(crate) fn close_space(workspace_id: &str) -> Result<(), ClientError> {
    let control = cmd_client::agent_control().ok_or(ClientError::NotReady)?;
    call(
        &control,
        "workspace.close",
        json!({ "workspace_id": workspace_id }),
    )?;
    Ok(())
}

/// Creates a space at `cwd` (when the editor has one) and focuses it.
pub(crate) fn create_space(cwd: Option<&str>) -> Result<(), ClientError> {
    let control = cmd_client::agent_control().ok_or(ClientError::NotReady)?;
    let mut params = json!({ "focus": true });
    if let Some(cwd) = cwd {
        params["cwd"] = json!(cwd);
    }
    call(&control, "workspace.create", params)?;
    Ok(())
}

/// Creates a space at `cwd` with an explicit label and returns its id.
pub(crate) fn create_space_with_label(
    cwd: Option<&str>,
    label: Option<&str>,
) -> Result<(String, String), ClientError> {
    let control = cmd_client::agent_control().ok_or(ClientError::NotReady)?;
    let mut params = json!({ "focus": true });
    if let Some(cwd) = cwd {
        params["cwd"] = json!(cwd);
    }
    if let Some(label) = label {
        params["label"] = json!(label);
    }
    let result = call(&control, "workspace.create", params)?;
    parse_workspace_created(&result)
}

/// One worktree's id and display label out of a `workspace.create` response.
fn parse_workspace_created(result: &Value) -> Result<(String, String), ClientError> {
    let workspace = result
        .get("workspace")
        .ok_or_else(|| ClientError::Failed("workspace.create carried no workspace".into()))?;
    let id = workspace
        .get("workspace_id")
        .and_then(Value::as_str)
        .ok_or_else(|| ClientError::Failed("workspace.create carried no id".into()))?
        .to_owned();
    let label = workspace
        .get("label")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    Ok((id, label))
}

/// Reads the daemon-scanned history sessions (Claude + CodeBuddy today).
pub(crate) fn fetch_sessions() -> Result<Vec<crate::controller::RawSession>, ClientError> {
    let control = cmd_client::agent_control().ok_or(ClientError::NotReady)?;
    let result = call(&control, "agent.sessions", json!({}))?;
    let entries = result
        .get("sessions")
        .and_then(Value::as_array)
        .ok_or_else(|| ClientError::Failed("agent.sessions carried no sessions".into()))?;
    let mut sessions = Vec::with_capacity(entries.len());
    for entry in entries {
        let (Some(agent), Some(session_id), Some(source_path)) = (
            entry.get("agent").and_then(Value::as_str),
            entry.get("session_id").and_then(Value::as_str),
            entry.get("source_path").and_then(Value::as_str),
        ) else {
            log::warn!("agent sessions: entry without agent/session_id/source_path: {entry}");
            continue;
        };
        sessions.push(crate::controller::RawSession {
            agent: agent.to_owned(),
            session_id: session_id.to_owned(),
            title: entry
                .get("title")
                .and_then(Value::as_str)
                .map(str::to_owned),
            workspace_root: entry
                .get("workspace_root")
                .and_then(Value::as_str)
                .map(str::to_owned),
            updated_at: entry.get("updated_at").and_then(Value::as_i64),
            source_path: source_path.to_owned(),
            workspace_id: entry
                .get("workspace_id")
                .and_then(Value::as_str)
                .map(str::to_owned),
        });
    }
    Ok(sessions)
}

/// Opens one history session: a tab in the space already open at the session's
/// root — creating the space only when none is open — and then the agent CLI
/// with its resume argv.
///
/// The resume argv is `--resume <session_id>` for the first-batch agents
/// (claude, codebuddy). The host accepts free-form argv, so an agent whose
/// CLI lacks that flag only shows the failure inside its own TUI.
pub(crate) fn open_session(
    request: crate::controller::OpenSessionRequest,
) -> Result<crate::controller::OpenedSession, ClientError> {
    let control = cmd_client::agent_control().ok_or(ClientError::NotReady)?;
    let cwd = request.workspace_root.to_string_lossy().into_owned();

    // A space already open at this root is reused: creating a second one would
    // split the same project across two spaces in the history view.
    let (workspace_id, pane_id) = match request.workspace_id.clone() {
        Some(workspace_id) => {
            // Tabs are numbered instead of titled with the session name: those
            // are whole sentences and wreck the tab strip.
            let label = next_tab_label(&control, &workspace_id)?;
            let tab = call(
                &control,
                "tab.create",
                json!({
                    "workspace_id": workspace_id,
                    "cwd": cwd,
                    "focus": true,
                    "label": label,
                }),
            )?;
            let pane_id = root_pane_id(&tab, "tab.create")?;
            (workspace_id, pane_id)
        }
        None => {
            // `workspace.create` already opens a tab with a shell in it, so the
            // agent takes over that pane. Asking for a second tab here is what
            // left an empty shell tab next to every resumed session.
            let created = call(
                &control,
                "workspace.create",
                json!({ "cwd": cwd, "focus": true }),
            )?;
            let (workspace_id, _) = parse_workspace_created(&created)?;
            let pane_id = root_pane_id(&created, "workspace.create")?;
            if let Some(tab_id) = created
                .get("tab")
                .and_then(|tab| tab.get("tab_id"))
                .and_then(Value::as_str)
            {
                if let Err(error) = call(
                    &control,
                    "tab.rename",
                    json!({ "tab_id": tab_id, "label": "1" }),
                ) {
                    log::warn!("agent panel: naming the new space's tab failed: {error}");
                }
            }
            (workspace_id, pane_id)
        }
    };
    start_agent_resuming(&control, &request, &pane_id)?;
    Ok(crate::controller::OpenedSession {
        workspace_id,
        pane_id,
    })
}

fn root_pane_id(response: &Value, method: &str) -> Result<String, ClientError> {
    response
        .get("root_pane")
        .and_then(|pane| pane.get("pane_id"))
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| ClientError::Failed(format!("{method} carried no root pane")))
}

/// The tab strip names tabs by number, so a new tab takes the highest existing
/// number plus one. Counting the tabs instead would repeat a number as soon as a
/// middle tab had been closed.
fn next_tab_label(control: &AgentControl, workspace_id: &str) -> Result<String, ClientError> {
    let listed = call(control, "tab.list", json!({ "workspace_id": workspace_id }))?;
    let highest = listed
        .get("tabs")
        .and_then(Value::as_array)
        .map(|tabs| {
            tabs.iter()
                .filter_map(|tab| tab.get("number").and_then(Value::as_u64))
                .max()
                .unwrap_or(0)
        })
        .unwrap_or(0);
    Ok((highest + 1).to_string())
}

/// Starts the agent with its resume argv.
///
/// A tab's shell is not interactive the instant the tab exists, so the first
/// attempt can find the target busy or unavailable; those are retried briefly.
/// The agent is named after its kind, and only when the server already holds
/// that name does the retry add a number — a busy pane is not a reason to show
/// a `codebuddy-2` row in the panel.
fn start_agent_resuming(
    control: &AgentControl,
    request: &crate::controller::OpenSessionRequest,
    pane_id: &str,
) -> Result<(), ClientError> {
    const ATTEMPTS: usize = 6;
    const RETRY_DELAY: Duration = Duration::from_millis(300);

    let kind = sanitize_agent_kind(&request.agent_kind);
    let mut name = kind.clone();
    let mut last_error = None;
    for attempt in 0..ATTEMPTS {
        if attempt > 0 {
            std::thread::sleep(RETRY_DELAY);
        }
        let params = json!({
            "name": name.clone(),
            "kind": request.agent_kind,
            "pane_id": pane_id,
            "args": ["--resume", request.session_id],
            "timeout_ms": 30_000,
        });
        match call_with_timeout(control, "agent.start", params, Duration::from_secs(35)) {
            Ok(_) => return Ok(()),
            Err(error) => {
                let message = error_message(&error);
                if message.contains("agent_name_taken") {
                    name = format!("{kind}-{}", attempt + 2);
                }
                let retry = message.contains("agent_pane_busy")
                    || message.contains("agent_pane_unavailable")
                    || message.contains("agent_name_taken");
                if !retry {
                    return Err(error);
                }
                last_error = Some(error);
            }
        }
    }
    Err(last_error.unwrap_or_else(|| ClientError::Failed("agent.start failed".to_owned())))
}

fn error_message(error: &ClientError) -> String {
    match error {
        ClientError::NotReady => String::new(),
        ClientError::Failed(message) => message.to_lowercase(),
    }
}

/// Deletes one session file through the daemon.
///
/// The app process gets `EPERM` unlinking from the agent's store, so this has
/// to run on the daemon side, which runs as the user; the daemon also checks
/// the path against its own store roots before removing anything.
pub(crate) fn delete_session(source_path: &str) -> Result<(), ClientError> {
    let control = cmd_client::agent_control().ok_or(ClientError::NotReady)?;
    call(
        &control,
        "agent.session.delete",
        json!({ "source_path": source_path }),
    )?;
    Ok(())
}

/// Keeps only `[a-z0-9-]` so the name passes the server's agent-name rules.
fn sanitize_agent_kind(kind: &str) -> String {
    let mut name: String = kind
        .to_lowercase()
        .chars()
        .map(|ch| match ch {
            'a'..='z' | '0'..='9' | '-' | '_' => ch,
            _ => '-',
        })
        .collect();
    if !name.chars().next().is_some_and(|first| first.is_ascii_lowercase()) {
        name.insert(0, 'a');
    }
    name
}

/// Asks the server to re-read its configuration. The server answers by asking
/// every connected client to reload its own settings, which is the same path
/// the client's own settings page takes.
pub(crate) fn reload_config() -> Result<(), ClientError> {
    let control = cmd_client::agent_control().ok_or(ClientError::NotReady)?;
    call(&control, "server.reload_config", json!({}))?;
    Ok(())
}

/// Reads the integration list, installing whatever is missing when asked.
///
/// Installation writes files under the agent's own directory, so each target is
/// installed on its own and one failure does not stop the rest. The list is
/// re-read afterwards because the server computes the state from disk.
pub(crate) fn refresh_integrations(auto_install: bool) -> Result<Vec<Integration>, ClientError> {
    let control = cmd_client::agent_control().ok_or(ClientError::NotReady)?;
    let listed = integration_list(&control)?;
    if !auto_install {
        return Ok(listed);
    }
    let mut installed_any = false;
    for integration in &listed {
        if !integration.needs_install {
            continue;
        }
        match integration_install(&control, &integration.target) {
            Ok(()) => installed_any = true,
            Err(error) => log::warn!(
                "agent settings: cannot install {}: {error}",
                integration.target
            ),
        }
    }
    if installed_any {
        integration_list(&control)
    } else {
        Ok(listed)
    }
}

/// Installs one target and re-reads the list. Used by the per-agent cell, which
/// must not touch integrations the user did not click.
pub(crate) fn install_integration(target: &str) -> Result<Vec<Integration>, ClientError> {
    let control = cmd_client::agent_control().ok_or(ClientError::NotReady)?;
    integration_install(&control, target)?;
    integration_list(&control)
}

fn integration_list(control: &AgentControl) -> Result<Vec<Integration>, ClientError> {
    let result = call(control, "integration.list", json!({}))?;
    parse_integrations(&result)
}

fn integration_install(control: &AgentControl, target: &str) -> Result<(), ClientError> {
    call_with_timeout(
        control,
        "integration.install",
        json!({ "target": target }),
        INSTALL_TIMEOUT,
    )?;
    Ok(())
}

fn parse_integrations(result: &Value) -> Result<Vec<Integration>, ClientError> {
    let Some(entries) = result.get("integrations").and_then(Value::as_array) else {
        return Err(ClientError::Failed(
            "integration.list: the response has no integration list".to_string(),
        ));
    };
    let mut integrations = Vec::with_capacity(entries.len());
    for entry in entries {
        let (Some(target), Some(label)) = (
            entry.get("target").and_then(Value::as_str),
            entry.get("label").and_then(Value::as_str),
        ) else {
            log::warn!("agent settings: integration entry without a target or label: {entry}");
            continue;
        };
        let available = entry
            .get("available")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let state = match entry.get("state").and_then(Value::as_str).unwrap_or_default() {
            "current" => IntegrationState::Current,
            "outdated" => IntegrationState::Outdated,
            _ => IntegrationState::NotInstalled,
        };
        let needs_install =
            state == IntegrationState::Outdated || (available && state == IntegrationState::NotInstalled);
        integrations.push(Integration {
            target: target.to_string(),
            label: label.to_string(),
            state,
            needs_install,
        });
    }
    Ok(integrations)
}

/// One request over a fresh bridge connection. The token line comes first; the
/// bridge closes the connection when it does not match.
fn call(control: &AgentControl, method: &str, params: Value) -> Result<Value, ClientError> {
    call_with_timeout(control, method, params, READ_TIMEOUT)
}

fn call_with_timeout(
    control: &AgentControl,
    method: &str,
    params: Value,
    timeout: Duration,
) -> Result<Value, ClientError> {
    let address = SocketAddr::from((Ipv4Addr::LOCALHOST, control.control_port));
    let mut stream = TcpStream::connect_timeout(&address, CONNECT_TIMEOUT)
        .map_err(|err| ClientError::Failed(format!("connect to the agent bridge: {err}")))?;
    stream
        .set_read_timeout(Some(timeout))
        .map_err(|err| ClientError::Failed(format!("set read timeout: {err}")))?;
    stream
        .set_write_timeout(Some(timeout))
        .map_err(|err| ClientError::Failed(format!("set write timeout: {err}")))?;

    writeln!(stream, "{AGENT_CONTROL_TOKEN_PREFIX} {}", control.token)
        .map_err(|err| ClientError::Failed(format!("send control token: {err}")))?;
    let request = json!({ "id": method, "method": method, "params": params });
    writeln!(stream, "{request}")
        .map_err(|err| ClientError::Failed(format!("send {method}: {err}")))?;

    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    reader
        .read_line(&mut line)
        .map_err(|err| ClientError::Failed(format!("read {method} response: {err}")))?;
    if line.is_empty() {
        return Err(ClientError::Failed(format!(
            "{method}: the bridge closed without a response"
        )));
    }

    let value: Value = serde_json::from_str(&line)
        .map_err(|err| ClientError::Failed(format!("parse {method} response: {err}")))?;
    if let Some(error) = value.get("error") {
        return Err(ClientError::Failed(format!("{method}: {error}")));
    }
    Ok(value.get("result").cloned().unwrap_or(Value::Null))
}
