//! On-device command server for the host application.
//!
//! Runs as an independent executable (a public HNP shipped in the host HAP,
//! launched on the OHOS device via hdc / the system). It listens on two
//! loopback SSH ports:
//! - 4022 (command): dynamic keys freshly generated on every start, serves
//!   command exec over a russh server, and
//! - 4023 (management): fixed build-time keys, serving `BOOTSTRAP_COMMAND` so a
//!   cmd-client can fetch this run's dynamic command keys.
//!
//! Replaces the previous external-VM command backends (openeuler-agent /
//! qemu-agent): the daemon runs on the same device as the host application and can spawn
//! arbitrary OHOS command-line programs, removing the VM dependency.

mod exec;
mod fanotify_patch;
mod agent_control;
mod keygen;
mod keys;
mod logger;
mod management;
mod peers;
mod shim;
mod protocol;
mod pty;
mod session_tmp;
mod sign_elf;
mod sshd;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use russh::server as russh_server;
use tokio::net::TcpListener;

use crate::management::ManagementServer;
use crate::protocol::{COMMAND_PORT, LOOPBACK_ADDR, MANAGEMENT_PORT};
use crate::sshd::{ConnectionHandler, SshServer};

/// Env var overriding the address both SSH listeners bind to. The daemon build
/// that runs inside the QEMU guest must bind `0.0.0.0` so the host-side slirp
/// hostfwd rules can reach it; the OHOS build keeps the loopback default.
const BIND_ADDR_ENV: &str = "HICODEERD_BIND_ADDR";
/// Env var overriding the directory the fixed management host private key is
/// read from. The default is `<package root>/conf`, where the HNP installs it.
const CONF_DIR_ENV: &str = "HICODEERD_CONF_DIR";
/// Env var (guest mode): when set to "1", periodically reclaims guest dcache.
/// The embedded virtiofsd backend holds one O_PATH fd per guest-looked-up inode
/// until the guest sends FUSE_FORGET; a guest with ample RAM rarely evicts its
/// dcache, so fds would grow unbounded and exhaust the process. Writing
/// `drop_caches=2` forces dentry/inode eviction -> FUSE_FORGET -> fd release.
const DROP_CACHES_ENV: &str = "HICODEERD_DROP_CACHES";
/// Sysfs knob to write and the value (2 = drop unused dentries/inodes only).
const DROP_CACHES_PATH: &str = "/proc/sys/vm/drop_caches";
const DROP_CACHES_VALUE: &str = "2";
/// Reclaim cadence (see the 2026-09-03 virtiofsd fd-exhaustion record).
const DROP_CACHES_INTERVAL: std::time::Duration = std::time::Duration::from_secs(15);
/// Bytes of traffic before an SSH key re-exchange is requested. The protocol
/// forbids raising this past the ceiling russh itself enforces (see
/// `russh::Limits::new`), and neither listener carries anywhere near it.
const REKEY_BYTE_LIMIT: usize = 1 << 30;
/// Time before an SSH key re-exchange is requested. An app frozen by the system
/// (`nap-background`) or a suspended device stops answering altogether, so
/// anything short of this would rekey into silence and drop the connection --
/// which the management listener reads as the client being gone, and answers by
/// taking down everything that client started (see `peers`). A year is
/// effectively "never" for a connection that only ever carries a few kilobytes.
const REKEY_TIME_LIMIT: std::time::Duration =
    std::time::Duration::from_secs(365 * 24 * 60 * 60);
/// Directory under the host application's home holding the embedded agent
/// server's state.
const AGENT_STATE_DIR: &str = ".hicodeer";
/// Subdirectory of [`AGENT_STATE_DIR`] that holds the sockets.
const AGENT_RUN_DIR: &str = "agent";
/// Variable the agent server reads for its JSON API socket; the client socket
/// path derives from it, so one value places both. The name itself belongs to
/// the embedded server's protocol and cannot be renamed here.
const AGENT_SOCKET_PATH_ENV: &str = "HICODEER_AGENT_SOCKET_PATH";
/// File name of the JSON API socket inside the agent run directory.
const AGENT_API_SOCKET_FILE: &str = "agent.sock";
/// Variable the agent server reads for the client protocol socket's mode.
const AGENT_CLIENT_SOCKET_MODE_ENV: &str = "HICODEER_AGENT_SOCKET_MODE";
/// Mode the Agent terminal's client needs: it connects from the application
/// account, not this daemon's.
const AGENT_CLIENT_SOCKET_MODE: &str = "0666";
/// Variable that overrides the config file the agent server reads.
const AGENT_CONFIG_PATH_ENV: &str = "HICODEER_AGENT_CONFIG_PATH";
/// Config file shipped in the package's `conf` directory.
const AGENT_CONFIG_FILE: &str = "agent-config.toml";
/// Records which packaged config the writable copy was made from.
const AGENT_CONFIG_STAMP_FILE: &str = "agent-config.stamp";
/// FNV-1a parameters, used to fingerprint the packaged config.
const FNV_OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;
/// Argument that carries the socket path the daemon reported to the host
/// application. The two processes run under different accounts and do not
/// share a home directory, and the terminal command does not inherit the
/// daemon's environment, so the path travels on the command line.
const AGENT_SOCKET_ARGUMENT: &str = "--socket";
/// Hidden subcommand: run the agent TUI client in the calling terminal against
/// this daemon's embedded server. Not a listener mode; it exits with the TUI.
const AGENT_TUI_SUBCOMMAND: &str = "agent-tui";

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some(AGENT_TUI_SUBCOMMAND) {
        let Some(socket_path) = socket_path_argument(&args[1..]).map(str::to_string) else {
            // The socket path is the only link to the daemon: without it the
            // client would fall back to a default the application account
            // cannot reach, so fail here instead of inside the TUI.
            tui_report(&format!(
                "missing {AGENT_SOCKET_ARGUMENT} <path>; start this subcommand from the host application"
            ));
            std::process::exit(1);
        };
        // The client names its protocol socket from the API socket path it is
        // given. That name must be unique to this process: the terminal shell
        // (hishell) runs on the daemon side, so this process usually shares the
        // daemon's filesystem view -- binding the derived name directly would
        // replace the socket the daemon's own client bridge forwards to, and
        // two windows would replace each other's proxy. Deriving a per-process
        // name keeps every client on its own path; the proxy still reaches the
        // daemon's loopback bridge.
        let unique_api_socket_path = unique_api_socket_path(Path::new(&socket_path));
        let client_path = client_socket_path(&unique_api_socket_path);
        if let Err(err) = agent_control::serve_local_client_proxy(
            &client_path,
            agent_control::AGENT_CLIENT_PORT,
        ) {
            tui_report(&format!(
                "cannot publish the client socket at {}: {err}",
                client_path.display()
            ));
            std::process::exit(1);
        }
        std::env::set_var(AGENT_SOCKET_PATH_ENV, &unique_api_socket_path);
        // The packaged config is read-only, and the client rewrites the file it
        // loaded to record first-run state (its onboarding flag), so pointing it
        // at the package fails with EACCES. Put a writable copy beside the
        // sockets instead -- inside the application's own sandbox, which this
        // process can write -- and let the client load that.
        if let Some(package_config) = std::env::var_os(AGENT_CONFIG_PATH_ENV) {
            let state_dir = Path::new(&socket_path).parent().and_then(Path::parent);
            match state_dir {
                Some(state_dir) => {
                    match writable_agent_config(state_dir, Path::new(&package_config)) {
                        Ok(local_config) => std::env::set_var(AGENT_CONFIG_PATH_ENV, local_config),
                        Err(err) => {
                            std::env::remove_var(AGENT_CONFIG_PATH_ENV);
                            tui_report(&format!(
                                "cannot prepare the config from {}: {err}",
                                package_config.to_string_lossy()
                            ));
                        }
                    }
                }
                None => {
                    std::env::remove_var(AGENT_CONFIG_PATH_ENV);
                    tui_report("cannot prepare the config: the socket path has no state directory");
                }
            }
        }
        let result = agent_server::run_client_entry();
        if let Err(err) = result {
            tui_report(&format!("client stopped: {err}"));
            std::process::exit(1);
        }
        return;
    }
    if args.iter().any(|arg| arg == "--help") {
        print_help();
        return;
    }
    // Silent by default; pass --log to enable stdout + hilog (OHOS) logging.
    let logging = args.iter().any(|arg| arg == "--log");
    logger::init(logging);
    if let Err(err) = run() {
        // Startup failure must surface even in silent mode.
        eprintln!("hicodeerd fatal: {err}");
        std::process::exit(1);
    }
}

/// Reports why the TUI client could not start, on the terminal.
fn tui_report(line: &str) {
    eprintln!("hicodeerd {AGENT_TUI_SUBCOMMAND}: {line}");
}

/// Reads `--socket <path>` from the arguments that follow the TUI subcommand.
/// The host application passes the path the daemon reported, so the client
/// never has to rediscover it: the two processes run under different accounts
/// and do not share a home directory, and the terminal command does not inherit
/// the daemon's environment.
fn socket_path_argument(args: &[String]) -> Option<&str> {
    let index = args.iter().position(|arg| arg == AGENT_SOCKET_ARGUMENT)?;
    args.get(index + 1).map(String::as_str)
}

/// Prints what this binary accepts. Runs before any listener is bound and
/// before a logger is installed, so `--help` reads the same whatever else is
/// on the command line.
fn print_help() {
    println!("hicodeerd - on-device command server for the host application");
    println!();
    println!("Usage: hicodeerd [--log] [--help]");
    println!("       hicodeerd {AGENT_TUI_SUBCOMMAND} {AGENT_SOCKET_ARGUMENT} <path>");
    println!();
    println!("Listens on the loopback ports {COMMAND_PORT} (command) and {MANAGEMENT_PORT} (management). The");
    println!("management listener hands a fresh client the keys this run's command");
    println!("listener accepts. The embedded agent server is hosted in the same copy");
    println!("of the process; its terminal client is the {AGENT_TUI_SUBCOMMAND}");
    println!("subcommand, started by the host application inside an editor terminal.");
    println!();
    println!("Options:");
    println!("  --log   Enable logging. Without it the daemon is silent: no logger is");
    println!("          installed and every record is dropped. With it, records go to");
    println!("          hilog on the device and to stdout on a plain host. Only");
    println!("          failures are recorded either way.");
    println!("  --help  Print this help and exit.");
    println!();
    println!("Subcommands:");
    println!("  {AGENT_TUI_SUBCOMMAND}  Run the agent TUI client in the calling terminal, attached to");
    println!("                this daemon's embedded server. Internal: not a listener mode.");
}

/// Root of the package this executable was installed into: the directory that
/// carries `bin/` and the rest of the payload, one level above the executable.
/// Reading `/proc/self/exe` follows any exec symlink to the real installed
/// layout, which is what `conf` and the preloads are addressed relative to.
///
/// On OHOS that is the version directory of this daemon's HNP
/// (`<pkg>.org/<pkg>_<version>/`), which the system installs and replaces as a
/// whole; inside the guest it is the `qemu` directory the host stages.
pub(crate) fn package_root() -> std::io::Result<PathBuf> {
    // `/proc/self/exe` is the reliable source: it follows the exec symlink to
    // the real installed layout. Some hosts do not mount procfs, so fall back to
    // asking the runtime rather than assuming either one works.
    let exe = match std::fs::read_link("/proc/self/exe") {
        Ok(exe) => exe,
        Err(err) => {
            log::warn!("daemon: read /proc/self/exe failed ({err}); asking the runtime instead");
            std::env::current_exe()?
        }
    };
    let bin_dir = exe
        .parent()
        .ok_or_else(|| std::io::Error::other("executable has no parent dir"))?;
    bin_dir
        .parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| std::io::Error::other("bin dir has no parent dir"))
}

/// Resolves the directory holding the fixed management host private key:
/// `CONF_DIR_ENV` when set, otherwise `<package root>/conf` (a `conf` directory
/// next to the `bin` that contains this executable).
fn conf_dir() -> std::io::Result<PathBuf> {
    if let Some(dir) = std::env::var_os(CONF_DIR_ENV) {
        return Ok(PathBuf::from(dir));
    }
    // The HNP layout is <pkg>/bin/<daemon binary> + <pkg>/conf/... .
    Ok(package_root()?.join("conf"))
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    // Exports the preloads every program spawned from here on has to inherit.
    // Resolved from the package root, so it needs nothing from a client. A
    // package that ships no preloads leaves the environment untouched.
    shim::install();
    // Fixed management keys: the host private key comes from the package's
    // `conf` directory (0600, never embedded -- the daemon is a 0755 public
    // HNP), the authorized client public key is compiled in (see `keys`).
    let conf = conf_dir()?;
    let (mgmt_host_key, mgmt_authorized) = keys::mgmt_keys(&conf)?;

    // Dynamic keys for this run's command listener (never persisted).
    let dynamic = keygen::generate()?;
    // The server binds one socket and the bridge forwards to it, so the path is
    // decided once, before either starts. See `agent_socket_path` for where it
    // lives: the application's home is on a shared filesystem that cannot bind
    // a UNIX socket, so the path is not derived from it.
    let agent_socket_path = agent_socket_path();
    std::env::set_var(AGENT_SOCKET_PATH_ENV, &agent_socket_path);
    // The agent server reads its settings from this file. It ships inside the
    // package, so the defaults this build needs (no server sidebar: the Agent panel
    // in the editor shows the same tree) do not depend on the user's home or on the
    // writable state directory.
    let agent_config_path = conf.join(AGENT_CONFIG_FILE);
    if std::env::var_os(AGENT_CONFIG_PATH_ENV).is_none() && agent_config_path.is_file() {
        std::env::set_var(AGENT_CONFIG_PATH_ENV, &agent_config_path);
    }
    // The host application cannot open the server's owner-only API socket, so it
    // is published on loopback instead; the endpoint and this run's token travel
    // in the bootstrap payload below. A failure here costs the Agent panel, not the
    // command service, so it is reported and start-up continues.
    let agent_control = match agent_control::start(agent_socket_path.clone()) {
        Ok(mut control) => {
            // The TUI client runs under the application account and does not
            // inherit this environment, so it would fall back to the defaults
            // for everything this file sets. Hand it the path instead: the
            // package directory is world-readable.
            if agent_config_path.is_file() {
                control.config_path = Some(agent_config_path.to_string_lossy().into_owned());
            }
            Some(control)
        }
        Err(err) => {
            log::warn!("daemon: agent control listener did not start: {err}");
            None
        }
    };
    // The TUI client runs under the application account, which cannot see this
    // daemon's socket tree, so its protocol socket is republished on loopback
    // too. Failure costs the Agent terminal only.
    if let Err(err) =
        agent_control::start_client_bridge(client_socket_path(&agent_socket_path))
    {
        log::warn!("daemon: agent client bridge did not start: {err}");
    }
    let ssh_info = protocol::SshInfo {
        command_port: COMMAND_PORT,
        command_host_key_pem: keygen::public_openssh(dynamic.host_key.public_key())?,
        client_private_key_pem: keygen::private_openssh(&dynamic.client_private)?,
        agent: agent_control,
    };
    // Never log any key material (host key, client key, PEMs) in any mode.

    let bind_host =
        std::env::var(BIND_ADDR_ENV).unwrap_or_else(|_| LOOPBACK_ADDR.to_string());
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    rt.block_on(async move {
        let command_addr = format!("{bind_host}:{COMMAND_PORT}");
        let mgmt_addr = format!("{bind_host}:{MANAGEMENT_PORT}");
        let command_listener = TcpListener::bind(&command_addr).await?;
        let mgmt_listener = TcpListener::bind(&mgmt_addr).await?;

        // Command listener config: this run's dynamic keys.
        let command_authorized = vec![dynamic.client_public];
        let command_server = SshServer::new(command_authorized);
        let command_config = Arc::new(russh_server::Config {
            keys: vec![dynamic.host_key],
            // cmd-client keeps a pool of long-lived SSH connections; never let
            // the server reap an idle pooled connection.
            inactivity_timeout: None,
            limits: russh::Limits::new(REKEY_BYTE_LIMIT, REKEY_BYTE_LIMIT, REKEY_TIME_LIMIT),
            ..Default::default()
        });

        // Management listener config: the fixed build-time keys.
        let mgmt_server = ManagementServer::new(vec![mgmt_authorized], &ssh_info);
        let mgmt_config = Arc::new(russh_server::Config {
            keys: vec![mgmt_host_key],
            inactivity_timeout: None,
            // This connection's presence is what says the client is alive
            // (see `peers`), so it must never be asked to rekey while its
            // process may be frozen and unable to answer.
            limits: russh::Limits::new(REKEY_BYTE_LIMIT, REKEY_BYTE_LIMIT, REKEY_TIME_LIMIT),
            ..Default::default()
        });

        maybe_spawn_drop_caches();
        let mut agent_server_task = start_agent_server().map(|server| tokio::spawn(server.run()));
        let mut shutdown = ShutdownSignals::new()?;
        tokio::select! {
            result = async {
                tokio::try_join!(
                    accept_command(command_listener, command_config, command_server),
                    accept_management(mgmt_listener, mgmt_config, mgmt_server),
                )
            } => {
                result?;
            }
            name = shutdown.recv() => {
                log::warn!("daemon: {name} received; shutting down");
            }
            _ = wait_agent_server(&mut agent_server_task) => {
                log::error!("daemon: embedded agent server stopped unexpectedly");
            }
        }
        if let Some(task) = agent_server_task {
            task.abort();
            let _ = task.await;
        }
        // Both ways out converge here: nothing a client started may outlive the
        // daemon that owns it.
        peers::retire_all("daemon exiting");
        Ok(())
    })
}

/// In guest mode (`DROP_CACHES_ENV=1`) periodically reclaims the guest
/// dcache so the in-process virtiofsd backend releases its O_PATH fds
/// (see `DROP_CACHES_*` constants).
fn maybe_spawn_drop_caches() {
    if std::env::var(DROP_CACHES_ENV).as_deref() != Ok("1") {
        return;
    }
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(DROP_CACHES_INTERVAL);
        interval.tick().await; // immediate first tick
        loop {
            interval.tick().await;
            if let Err(err) = std::fs::write(DROP_CACHES_PATH, DROP_CACHES_VALUE) {
                log::warn!("hicodeerd: drop_caches write failed: {err}");
            }
        }
    });
}

/// Builds the in-process agent server. A failure is not fatal: the daemon keeps
/// serving commands (the editor stays usable, only the Agent panel backend is
/// missing), so the reason is recorded and start-up continues.
fn start_agent_server() -> Option<agent_server::EmbeddedServer> {
    // The Agent terminal runs the TUI client under the application account,
    // while this server runs under the daemon's. The client socket is
    // owner-only by default, which that account cannot open, so it is widened
    // for this build; the API socket stays owner-only and is reached through
    // the token-guarded bridge instead.
    std::env::set_var(AGENT_CLIENT_SOCKET_MODE_ENV, AGENT_CLIENT_SOCKET_MODE);

    match agent_server::build_embedded() {
        Ok(server) => Some(server),
        Err(err) => {
            log::warn!("daemon: agent server did not start: {err}");
            None
        }
    }
}

/// Path of the agent server's API socket.
///
/// It lives under the bundle's shared files root, the one directory both this
/// daemon and the host application can reach. It deliberately does not follow
/// the application's home: that home lives on a shared user filesystem which
/// does not support binding a UNIX socket at all (the bind fails with EPERM),
/// so a socket placed there could never be opened.
fn agent_socket_path() -> PathBuf {
    let config_root = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/data/storage/el2/base/haps/entry/files"));
    config_root
        .join(AGENT_STATE_DIR)
        .join(AGENT_RUN_DIR)
        .join(AGENT_API_SOCKET_FILE)
}

/// Per-process stand-in for the API socket path, used only to derive the local
/// client socket name.
///
/// The TUI client never opens this path. It reads it as the API socket and
/// derives its protocol socket from it, so a per-process stem gives every client
/// a protocol socket of its own. The parent directory is kept: it is the one
/// directory this process can bind a socket in, whether it runs beside the
/// daemon or inside the application's sandbox.
fn unique_api_socket_path(api_socket_path: &Path) -> PathBuf {
    let stem = api_socket_path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("agent");
    let parent = api_socket_path.parent().unwrap_or_else(|| Path::new(""));
    parent.join(format!("{stem}-{}.sock", std::process::id()))
}

/// Client protocol socket the server derives from its API socket: the same
/// directory, with `-client` inserted before the extension. Mirrored here
/// because the TUI client, which runs in a different process, has to name the
/// path the server bound.
fn client_socket_path(api_socket_path: &Path) -> PathBuf {
    let stem = api_socket_path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("agent");
    let parent = api_socket_path.parent().unwrap_or_else(|| Path::new(""));
    parent.join(format!("{stem}-client.sock"))
}

/// Copies the packaged config into `state_dir` and returns the copy.
///
/// The package directory is read-only, but the client saves its first-run state
/// back into the config it loaded, so the client has to load a file it can
/// write. The copy is refreshed only when the packaged file changes: between
/// package updates it carries the client's own edits.
fn writable_agent_config(state_dir: &Path, package_config: &Path) -> std::io::Result<PathBuf> {
    let local_config = state_dir.join(AGENT_CONFIG_FILE);
    let stamp = state_dir.join(AGENT_CONFIG_STAMP_FILE);
    let fingerprint = config_fingerprint(package_config)?;
    let unchanged = std::fs::read_to_string(&stamp)
        .map(|saved| saved == fingerprint)
        .unwrap_or(false);
    if local_config.is_file() && unchanged {
        return Ok(local_config);
    }
    std::fs::create_dir_all(state_dir)?;
    std::fs::copy(package_config, &local_config)?;
    std::fs::write(&stamp, fingerprint)?;
    Ok(local_config)
}

/// Identifies one build's packaged config, so a new package refreshes the copy
/// while a running client's own edits survive.
fn config_fingerprint(path: &Path) -> std::io::Result<String> {
    let bytes = std::fs::read(path)?;
    let mut hash = FNV_OFFSET_BASIS;
    for byte in &bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    Ok(format!("{hash:016x}-{}", bytes.len()))
}

/// Resolves when the embedded agent-server task finishes. With no task it stays
/// forever so it never short-circuits the daemon's `select!`.
async fn wait_agent_server(task: &mut Option<tokio::task::JoinHandle<std::io::Result<()>>>) {
    match task {
        Some(handle) => {
            let _ = handle.await;
        }
        None => std::future::pending().await,
    }
}

/// Accepts command-listener connections and serves each with a fresh handler.
async fn accept_command(
    listener: TcpListener,
    config: Arc<russh_server::Config>,
    server: SshServer,
) -> std::io::Result<()> {
    loop {
        let (stream, _peer) = listener.accept().await?;
        let handler: ConnectionHandler = server.new_connection();
        let config = config.clone();
        tokio::spawn(async move {
            let _ = russh_server::run_stream(config, stream, handler).await;
        });
    }
}

/// Accepts management-listener connections and serves each with a fresh handler.
async fn accept_management(
    listener: TcpListener,
    config: Arc<russh_server::Config>,
    server: ManagementServer,
) -> std::io::Result<()> {
    loop {
        let (stream, _peer) = listener.accept().await?;
        let handler = server.new_connection();
        let config = config.clone();
        tokio::spawn(async move {
            let _ = russh_server::run_stream(config, stream, handler).await;
        });
    }
}

/// The signals that ask the daemon to stop. Registered once, before the accept
/// loops start, so one arriving during start-up is not missed.
struct ShutdownSignals {
    terminate: tokio::signal::unix::Signal,
    interrupt: tokio::signal::unix::Signal,
    hangup: tokio::signal::unix::Signal,
}

impl ShutdownSignals {
    fn new() -> std::io::Result<Self> {
        use tokio::signal::unix::{signal, SignalKind};
        Ok(Self {
            terminate: signal(SignalKind::terminate())?,
            interrupt: signal(SignalKind::interrupt())?,
            hangup: signal(SignalKind::hangup())?,
        })
    }

    /// Resolves to the name of the first of them to arrive. SIGHUP is included
    /// because the daemon is normally started from an interactive shell, which
    /// sends it when that terminal goes away.
    async fn recv(&mut self) -> &'static str {
        tokio::select! {
            _ = self.terminate.recv() => "SIGTERM",
            _ = self.interrupt.recv() => "SIGINT",
            _ = self.hangup.recv() => "SIGHUP",
        }
    }
}
