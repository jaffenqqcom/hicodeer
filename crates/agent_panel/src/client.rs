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

use crate::snapshot::Snapshot;

/// Bound on establishing the bridge connection: it is loopback to a process on
/// this device, so anything slower means the daemon is gone rather than busy.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(2);
/// Bound on one request/response. The server answers these from memory.
const READ_TIMEOUT: Duration = Duration::from_secs(3);

/// Why a call produced nothing.
pub(crate) enum ClientError {
    /// The daemon has not reported an embedded agent endpoint yet, or its
    /// management channel is down. Normal while the editor starts.
    NotReady,
    /// The bridge or the API refused the request.
    Failed(String),
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

/// One request over a fresh bridge connection. The token line comes first; the
/// bridge closes the connection when it does not match.
fn call(control: &AgentControl, method: &str, params: Value) -> Result<Value, ClientError> {
    let address = SocketAddr::from((Ipv4Addr::LOCALHOST, control.control_port));
    let mut stream = TcpStream::connect_timeout(&address, CONNECT_TIMEOUT)
        .map_err(|err| ClientError::Failed(format!("connect to the agent bridge: {err}")))?;
    stream
        .set_read_timeout(Some(READ_TIMEOUT))
        .map_err(|err| ClientError::Failed(format!("set read timeout: {err}")))?;
    stream
        .set_write_timeout(Some(READ_TIMEOUT))
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
