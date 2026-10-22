//! cmd-client: SSH client that talks to the on-device daemon command server.
//!
//! The host application (whose app sandbox forbids exec'ing external programs) links this
//! crate: `util::command`'s OHOS path routes remote commands through a
//! [`SshCommandExecutor`] registered here, which connects to the daemon over
//! loopback SSH (management bootstrap on 4023, command pool on 4022). This
//! crate is self-contained: it depends on no other gpui_ohos crate and carries
//! its own command-execution contract ([`types`]).

pub mod bootstrap;
pub mod command;
pub mod endpoint;
pub mod executor;
pub mod keys;
pub mod pool;
pub mod protocol;
pub mod pty;
pub mod types;

use std::sync::{Arc, OnceLock};

pub use endpoint::CommandEndpoint;
pub use executor::SshCommandExecutor;
pub use keys::{MGMT_CLIENT_KEY, MGMT_HOST_PUB};
pub use pty::{RemotePty, ResizeHandle};
pub use types::{
    ExecSpec, FdMode, RemoteChild, RemoteCommandExecutor, ShellPtyFuture, Signal,
};

static EXECUTOR: OnceLock<Arc<dyn RemoteCommandExecutor>> = OnceLock::new();

/// Endpoint of the agent control bridge, as last reported by the management
/// bootstrap. The daemon regenerates the token every run, so this is replaced
/// on every successful round and cleared when the channel drops.
static AGENT_CONTROL: std::sync::Mutex<Option<protocol::AgentControl>> =
    std::sync::Mutex::new(None);

/// Records the agent control endpoint reported by the management bootstrap.
pub fn set_agent_control(control: Option<protocol::AgentControl>) {
    let mut slot = AGENT_CONTROL
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    // Losing the endpoint is the only abnormal transition worth a line: the
    // Agent panel falls back to "waiting for the server" until the next
    // bootstrap round. Establishing it, and the repeated identical rounds, are
    // the normal path. Never include the token.
    if control.is_none() && slot.is_some() {
        log::warn!("cmd-client: agent control endpoint cleared");
    }
    *slot = control;
}

/// Endpoint and token for the embedded agent API. `None` until a bootstrap round
/// has reported them, when the management channel is down, and on a daemon
/// build that hosts no agent server.
pub fn agent_control() -> Option<protocol::AgentControl> {
    AGENT_CONTROL
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
        .clone()
}

/// Registers the process-wide remote command executor. Called once at startup
/// by launch-zed after the executor is constructed.
pub fn init_executor(executor: Arc<dyn RemoteCommandExecutor>) -> Result<(), ()> {
    EXECUTOR.set(executor).map_err(|_| ())
}

/// Returns the registered executor, or `None` if not yet initialized.
pub fn executor() -> Option<Arc<dyn RemoteCommandExecutor>> {
    EXECUTOR.get().cloned()
}

/// True once a command executor is registered and ready for use.
pub fn is_ready() -> bool {
    EXECUTOR.get().is_some()
}
