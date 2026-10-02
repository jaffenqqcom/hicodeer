//! Command-backend endpoint: which daemon this bridge talks to.
//!
//! hishell links exactly one executor, against the on-device hicodeerd on
//! loopback 4022/4023. The QEMU guest backend is not part of this bridge.

use crate::protocol::{COMMAND_PORT, LOOPBACK_ADDR, MANAGEMENT_PORT};

/// The endpoint (host + ports) of the daemon command backend.
#[derive(Clone, Debug)]
pub struct CommandEndpoint {
    /// Address of the fixed-key management listener used for bootstrap.
    pub mgmt_host: String,
    /// Port of the management listener.
    pub mgmt_port: u16,
    /// Address of the dynamic-key command listener used for connections.
    pub command_host: String,
    /// Port of the command listener.
    pub command_port: u16,
}

impl Default for CommandEndpoint {
    fn default() -> Self {
        Self {
            mgmt_host: LOOPBACK_ADDR.to_string(),
            mgmt_port: MANAGEMENT_PORT,
            command_host: LOOPBACK_ADDR.to_string(),
            command_port: COMMAND_PORT,
        }
    }
}

impl CommandEndpoint {
    /// The on-device (OHOS) hicodeerd backend: management 4023, command 4022.
    pub fn ohos_default() -> Self {
        Self::default()
    }
}
