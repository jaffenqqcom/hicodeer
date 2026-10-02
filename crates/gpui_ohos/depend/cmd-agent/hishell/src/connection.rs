//! Single SSH connection to the hicodeerd command listener.
//!
//! hishell serves exactly one interaction per process: an interactive shell for
//! the whole life of the process, or the one command a `-c` run asks for. It
//! therefore keeps no connection pool. The management handshake delivers this
//! run's dynamic command keys; every use then establishes one connection on
//! demand, uses it, and drops it with the interaction that owns it.
//!
//! A single multi-threaded tokio runtime is shared by every connection and by
//! the pump tasks relaying channel data.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use russh::client::{self, Config, Handle, Handler};
use russh::keys::ssh_key::PublicKey;
use russh::keys::{PrivateKey, PrivateKeyWithHashAlg};

/// Timeout for establishing one SSH connection.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
/// Keepalive interval for a connection: the server runs with no inactivity
/// timeout, so this only guards against intermediate drops.
const KEEPALIVE_INTERVAL: Duration = Duration::from_secs(30);

/// Bytes of traffic before an SSH key re-exchange is requested. The protocol
/// forbids raising this past the ceiling russh itself enforces (see
/// `russh::Limits::new`), and a single interaction carries far less than it.
pub(crate) const REKEY_BYTE_LIMIT: usize = 1 << 30;
/// Time before an SSH key re-exchange is requested, for every connection here
/// and for the management connection alike (see `bootstrap`). Effectively
/// "never": the daemon holds both kinds of connection open indefinitely, and a
/// client frozen by the system cannot answer a rekey, so a short interval would
/// drop exactly the connection whose presence says the instance is still alive.
pub(crate) const REKEY_TIME_LIMIT: Duration = Duration::from_secs(365 * 24 * 60 * 60);

/// Rejects any host key that does not match the expected one. The expected key
/// is the daemon's command host key delivered in this run's `SshInfo`, so a
/// restarted daemon (new key) is rejected and triggers a re-bootstrap.
#[derive(Clone)]
pub struct VerifyHandler {
    pub(crate) expected: PublicKey,
}

impl Handler for VerifyHandler {
    type Error = russh::Error;

    async fn check_server_key(&mut self, server_key: &PublicKey) -> Result<bool, Self::Error> {
        let accepted = server_key == &self.expected;
        if !accepted {
            log::warn!("hishell connection: host key mismatch (hicodeerd restarted?), rejecting");
        }
        Ok(accepted)
    }
}

/// One authenticated SSH session.
pub type SshSession = Handle<VerifyHandler>;

/// Parses an OpenSSH public-key text into a `PublicKey`, keeping only the two
/// key tokens (`<algorithm> <base64>`) so a trailing comment or extra whitespace
/// (as `ssh-keygen` appends) never leaks into the parsed value.
pub(crate) fn host_public_key(text: &str) -> Result<PublicKey, String> {
    let canonical: String = text
        .split_whitespace()
        .take(2)
        .collect::<Vec<_>>()
        .join(" ");
    PublicKey::from_openssh(&canonical).map_err(|err| format!("parse host public key: {err}"))
}

/// Connection parameters for the current daemon command SSH server.
#[derive(Clone)]
pub struct ConnConfig {
    pub host: String,
    pub port: u16,
    /// OpenSSH text of the expected command host public key.
    pub host_public_pem: String,
    /// OpenSSH text of the command client private key.
    pub private_key_pem: String,
    /// Identity presented as the SSH user name, naming this client instance to
    /// the daemon (see `protocol::CLIENT_ID_PREFIX`).
    pub client_id: String,
}

/// Delivers this run's dynamic command keys and establishes one connection per
/// use. It keeps no ready connection: the bootstrap hands it a config, and each
/// [`connect`](Self::connect) opens exactly the one connection its caller asked
/// for and leaves nothing behind.
pub struct ConnectionFactory {
    runtime: Arc<tokio::runtime::Runtime>,
    config: Mutex<Option<Arc<ConnConfig>>>,
    /// Whether the last management round trip failed to connect. Set and cleared
    /// by the bootstrap; read by `SshCommandExecutor::wait_ready`, which fails a
    /// caller at once rather than waiting out its whole budget when the daemon is
    /// plainly not there.
    bootstrap_failed: AtomicBool,
}

impl ConnectionFactory {
    /// Builds the factory and its tokio runtime. Starts no connection: nothing
    /// is opened until a caller asks for one, and there is nothing to open until
    /// the management handshake has delivered a config.
    pub fn new() -> std::io::Result<Arc<Self>> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .map_err(std::io::Error::other)?;
        Ok(Arc::new(Self {
            runtime: Arc::new(runtime),
            config: Mutex::new(None),
            bootstrap_failed: AtomicBool::new(false),
        }))
    }

    /// Exposes the shared runtime so executor pump tasks run on it.
    pub fn runtime(&self) -> &tokio::runtime::Runtime {
        &self.runtime
    }

    /// Records the connection config the management handshake delivered.
    pub fn update_config(&self, config: ConnConfig) {
        *self
            .config
            .lock()
            .unwrap_or_else(|poison| poison.into_inner()) = Some(Arc::new(config));
    }

    /// Drops the config because the management channel is gone: the command
    /// listener's keys could only be stale. Returns whether a config was
    /// actually dropped, so a caller retrying in a loop can report the
    /// transition once rather than once per attempt.
    pub fn clear_config(&self) -> bool {
        self.config
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .take()
            .is_some()
    }

    /// Current connection config, if the bootstrap has configured it yet.
    /// Cloning it is a refcount bump (see the `config` field).
    pub fn config(&self) -> Option<Arc<ConnConfig>> {
        self.config
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .clone()
    }

    /// Records the outcome of the last management round trip (see the
    /// `bootstrap_failed` field).
    pub(crate) fn set_bootstrap_failed(&self, failed: bool) {
        self.bootstrap_failed.store(failed, Ordering::Relaxed);
    }

    /// Whether the last management round trip failed to connect. A caller that
    /// is waiting for a config uses this to give up at once instead of waiting
    /// out its whole budget.
    pub fn bootstrap_failed(&self) -> bool {
        self.bootstrap_failed.load(Ordering::Relaxed)
    }

    /// Establishes one connection on demand and hands it to the caller. Fails at
    /// once when there is no config: the management channel was never
    /// established, so there is nothing to connect to.
    pub fn connect(&self) -> std::io::Result<SshSession> {
        let Some(config) = self.config() else {
            log::warn!("hishell connection: no management channel; hicodeerd unreachable");
            return Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "hicodeerd not connected yet",
            ));
        };
        self.runtime
            .block_on(connect(&config))
            .map_err(std::io::Error::other)
    }
}

/// Establishes and authenticates one SSH connection with the command client key,
/// verifying the command host key against the expected public key.
async fn connect(config: &ConnConfig) -> Result<SshSession, String> {
    let expected_host = host_public_key(&config.host_public_pem)?;
    let mut client_cfg = Config::default();
    client_cfg.keepalive_interval = Some(KEEPALIVE_INTERVAL);
    client_cfg.limits = russh::Limits::new(REKEY_BYTE_LIMIT, REKEY_BYTE_LIMIT, REKEY_TIME_LIMIT);
    let client_config = Arc::new(client_cfg);
    let mut session = tokio::time::timeout(
        CONNECT_TIMEOUT,
        client::connect(
            client_config,
            (config.host.as_str(), config.port),
            VerifyHandler {
                expected: expected_host,
            },
        ),
    )
    .await
    .map_err(|_| format!("connect to {}:{} timed out", config.host, config.port))?
    .map_err(|err| format!("connect to {}:{}: {err}", config.host, config.port))?;

    let key = PrivateKey::from_openssh(&config.private_key_pem)
        .map_err(|err| format!("parse client private key: {err}"))?;
    // Bounded like the connect above: `client::connect` only covers the TCP
    // connection and key exchange, so a daemon that accepts the connection and
    // then stops answering would otherwise park this call for good.
    let auth = tokio::time::timeout(
        CONNECT_TIMEOUT,
        session.authenticate_publickey(
            config.client_id.as_str(),
            PrivateKeyWithHashAlg::new(Arc::new(key), None),
        ),
    )
    .await
    .map_err(|_| format!("publickey auth to {}:{} timed out", config.host, config.port))?
    .map_err(|err| format!("publickey auth: {err}"))?;
    if !auth.success() {
        return Err("publickey auth rejected".to_string());
    }
    Ok(session)
}
