//! hishell: SSH client that talks to the on-device hicodeerd command server.
//!
//! The binary in this package is what the HiCodeer terminal runs as its local
//! `$SHELL`: the app sandbox forbids exec'ing the system's programs, so hishell
//! connects to hicodeerd over loopback SSH (management bootstrap on 4023,
//! command listener on 4022) and bridges an interactive `/usr/bin/zsh` on the
//! daemon to the local terminal. When hicodeerd is not reachable it replaces
//! itself with the local `/bin/sh` so a terminal always ends up with a shell.
//!
//! This crate is self-contained: it depends on no other gpui_ohos crate and
//! carries its own command-execution contract ([`types`]). It keeps no
//! connection pool -- one interaction per process means one connection at a
//! time (see [`connection::ConnectionFactory`]).

pub mod bootstrap;
pub mod command;
pub mod connection;
pub mod endpoint;
pub mod executor;
pub mod keys;
pub mod protocol;
pub mod pty;
pub mod types;

pub use endpoint::CommandEndpoint;
pub use executor::SshCommandExecutor;
pub use keys::{MGMT_CLIENT_KEY, MGMT_HOST_PUB};
pub use pty::{RemotePty, ResizeHandle};
pub use types::{
    ExecSpec, FdMode, RemoteChild, RemoteCommandExecutor, ShellPtyFuture, Signal,
};
