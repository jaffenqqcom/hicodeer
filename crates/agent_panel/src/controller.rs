//! The panel's view of its agent host, as one trait.
//!
//! `AgentController` is the seam between the panel UI and whatever drives
//! agents. `HerdrController` is the only implementation today and forwards to
//! `client.rs`'s blocking bridge calls; the panel wraps calls in
//! `cx.background_spawn` exactly as it did when calling `client.rs` directly.
//!
//! Three honest limits every implementer must keep in mind (design doc §3):
//! 1. `OpenSessionRequest::workspace_root` is passed to the host's
//!    workspace-create as-is; validity is the host's business.
//! 2. `agent_kind` carries no resume-support guarantee: `agent.start` takes
//!    free-form argv, and an unsupported combination only shows up as an
//!    error inside the agent TUI.
//! 3. A successful `agent.start` is not a successful resume. The CLI may
//!    fail to restore the session internally; the panel observes the agent's
//!    session id for a while afterwards to compensate (design doc §5).

// The trait is the host contract fixed by design doc §3, so it is defined in
// full even where the panel still reaches `client.rs` directly for a call it
// already had. The history path consumes `sessions`, `open_session` and
// `host_terminal`; the remaining methods are the surface a second host
// implementation plugs into. Only the trait carries the allowance — keeping it
// module-wide would also silence a genuinely unused item added here later.
use std::path::{Path, PathBuf};

use crate::client::{self, ClientError};

// The data types below are the host contract: the panel reads part of them and
// a second host implementation fills the rest, so a field the current panel
// does not consult is still defined.
#[allow(dead_code)]
pub struct WorkspaceRef {
    pub id: String,
    pub label: String,
}

#[allow(dead_code)]
pub struct AgentRef {
    pub pane_id: String,
    pub workspace_id: String,
    pub kind: String,
    pub status: crate::snapshot::AgentStatus,
    pub session: Option<SessionRef>,
    pub focused: bool,
}

#[derive(Clone, PartialEq, Eq)]
pub struct SessionRef {
    pub agent: String,
    pub id: String,
}

pub struct OpenSessionRequest {
    pub workspace_root: PathBuf,
    /// The space the daemon matched to this session, when one is open at its
    /// root. Present means "open a tab there"; absent means the root has no
    /// space yet and one has to be created.
    pub workspace_id: Option<String>,
    pub agent_kind: String,
    pub session_id: String,
}

#[allow(dead_code)]
pub struct OpenedSession {
    pub workspace_id: String,
    pub pane_id: String,
}

/// One historical agent session as the daemon scanned it. Mirrors the
/// daemon's `RawSessionDto` field for field; workspace matching already
/// happened daemon-side (`workspace_id` is `Some` only when the space is
/// open).
#[derive(Clone)]
pub struct RawSession {
    pub agent: String,
    pub session_id: String,
    pub title: Option<String>,
    pub workspace_root: Option<String>,
    pub updated_at: Option<i64>,
    pub source_path: String,
    pub workspace_id: Option<String>,
}

#[derive(Debug)]
pub enum ControllerError {
    /// The daemon has not reported its endpoint yet.
    NotReady,
    /// The bridge or the API refused the request.
    Unavailable(String),
    /// The host rejected the request with its own error code. Not produced by
    /// today's bridge, which carries the code inside the message instead; it
    /// stays in the contract for a host that reports codes structurally.
    #[allow(dead_code)]
    Rejected { code: String, message: String },
    /// A local file operation failed (state store, not the host).
    Io(std::io::Error),
}

impl From<ClientError> for ControllerError {
    fn from(error: ClientError) -> Self {
        match error {
            ClientError::NotReady => ControllerError::NotReady,
            ClientError::Failed(message) => ControllerError::Unavailable(message),
        }
    }
}

impl std::fmt::Display for ControllerError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ControllerError::NotReady => write!(formatter, "the agent endpoint is not ready"),
            ControllerError::Unavailable(message) => write!(formatter, "{message}"),
            ControllerError::Rejected { message, .. } => write!(formatter, "{message}"),
            ControllerError::Io(error) => write!(formatter, "{error}"),
        }
    }
}

pub type Result<T> = std::result::Result<T, ControllerError>;

#[allow(dead_code)]
pub trait AgentController: Send + Sync {
    /// None = this host has no deployable agent terminal (long-term state);
    /// transient unreadiness goes through `ControllerError::NotReady` on the
    /// calls instead of a forked channel.
    fn host_terminal(&self) -> Option<HostTerminalSpec>;

    fn workspaces(&self) -> Result<Vec<WorkspaceRef>>;
    fn create_workspace(&self, root: &Path, label: Option<&str>) -> Result<WorkspaceRef>;
    fn focus_workspace(&self, id: &str) -> Result<()>;
    fn rename_workspace(&self, id: &str, label: &str) -> Result<()>;
    fn close_workspace(&self, id: &str) -> Result<()>;

    fn agents(&self) -> Result<Vec<AgentRef>>;
    fn focus_agent(&self, pane_id: &str) -> Result<()>;

    /// History sessions the daemon scanned from each agent's own store.
    fn sessions(&self) -> Result<Vec<RawSession>>;

    /// Opens a history session: creates a workspace at the session's root,
    /// adds a tab there, and starts the agent CLI with its resume argv. See
    /// the three limits on the trait docs.
    fn open_session(&self, request: OpenSessionRequest) -> Result<OpenedSession>;

    fn integrations(&self) -> Result<Vec<crate::herdr_settings::Integration>>;
    fn install_integration(&self, target: &str) -> Result<Vec<crate::herdr_settings::Integration>>;
    fn reload_config(&self) -> Result<()>;
}

/// How to deploy the agent terminal for this host.
#[allow(dead_code)]
pub struct HostTerminalSpec {
    pub socket_path: String,
    pub config_path: String,
}

/// The real controller: forwards to the daemon bridge in `client.rs`.
pub struct HerdrController;

impl AgentController for HerdrController {
    fn host_terminal(&self) -> Option<HostTerminalSpec> {
        cmd_client::agent_control().map(|control| HostTerminalSpec {
            socket_path: control.socket_path,
            config_path: control.config_path.unwrap_or_default(),
        })
    }

    fn workspaces(&self) -> Result<Vec<WorkspaceRef>> {
        let snapshot = client::fetch()?;
        Ok(snapshot
            .spaces
            .iter()
            .map(|space| WorkspaceRef {
                id: space.id.clone(),
                label: space.label.clone(),
            })
            .collect())
    }

    fn create_workspace(&self, root: &Path, label: Option<&str>) -> Result<WorkspaceRef> {
        let (id, label) = client::create_space_with_label(
            root.to_str(),
            label,
        )?;
        Ok(WorkspaceRef { id, label })
    }

    fn focus_workspace(&self, id: &str) -> Result<()> {
        client::focus_space(id)?;
        Ok(())
    }

    fn rename_workspace(&self, id: &str, label: &str) -> Result<()> {
        client::rename_space(id, label)?;
        Ok(())
    }

    fn close_workspace(&self, id: &str) -> Result<()> {
        client::close_space(id)?;
        Ok(())
    }

    fn agents(&self) -> Result<Vec<AgentRef>> {
        let snapshot = client::fetch()?;
        Ok(snapshot
            .spaces
            .iter()
            .flat_map(|space| {
                space.agents.iter().map(move |agent| AgentRef {
                    pane_id: agent.pane_id.clone(),
                    workspace_id: space.id.clone(),
                    kind: agent.kind.clone(),
                    status: agent.status,
                    session: agent.agent_session.as_ref().map(|(agent, value)| SessionRef {
                        agent: agent.clone(),
                        id: value.clone(),
                    }),
                    focused: agent.focused,
                })
            })
            .collect())
    }

    fn focus_agent(&self, pane_id: &str) -> Result<()> {
        client::focus_agent(pane_id)?;
        Ok(())
    }

    fn sessions(&self) -> Result<Vec<RawSession>> {
        client::fetch_sessions().map_err(ControllerError::from)
    }

    fn open_session(&self, request: OpenSessionRequest) -> Result<OpenedSession> {
        client::open_session(request).map_err(ControllerError::from)
    }

    fn integrations(&self) -> Result<Vec<crate::herdr_settings::Integration>> {
        Ok(client::refresh_integrations(false)?)
    }

    fn install_integration(&self, target: &str) -> Result<Vec<crate::herdr_settings::Integration>> {
        Ok(client::install_integration(target)?)
    }

    fn reload_config(&self) -> Result<()> {
        client::reload_config()?;
        Ok(())
    }
}
