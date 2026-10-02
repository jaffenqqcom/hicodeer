//! OHOS command execution - hybrid local/remote.
//!
//! HarmonyOS's sandbox forbids `exec` of arbitrary external programs, so most
//! binaries (LSP servers, node, chmod, ...) run through the on-device daemon
//! command server: `spawn` hands an [`ExecSpec`](cmd_client::ExecSpec) to the
//! registered `cmd-client` executor and the returned `Child` carries raw byte
//! streams wired to the remote process's stdio. A remote child starts from the
//! daemon's own environment: none of the caller's variables travel, because the
//! two run under different uids and a caller's value can point inside its
//! private sandbox. Only a local child receives the caller's overrides.
//!
//! Tools found on-device in the private HNP install dir (`/data/app/bin`) are
//! the exception: they are forked on the device itself, so they no longer
//! depend on the daemon. The install dir is snapshotted once at startup
//! ([`init_local_tools`]); `spawn` routes a command local when its basename is
//! in that set and forwards everything else to the daemon. Because
//! `/data/app/bin` never changes while the process lives, no per-spawn
//! directory read is needed. If the set is empty (no HNP shipped), every
//! command simply runs through the daemon.
//!
//! This module depends only on the self-contained `cmd-client` crate, which
//! carries its own executor contract (`ExecSpec` / `RemoteCommandExecutor`).
//! It never references a concrete agent or the retired `command-executor` crate.

use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::{ExitStatus, Output};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::task::{Context, Poll, Waker};

use cmd_client::{ExecSpec, FdMode, RemoteCommandExecutor, Signal};
use smol::io::{AsyncRead, AsyncReadExt, AsyncWrite};

/// How a child's standard descriptor is wired.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Stdio {
    /// Connected to the data connection: the business side can read/write it.
    #[default]
    Piped,
    /// The child inherits from the parent descriptor.
    Inherit,
    /// Redirected to `/dev/null`.
    Null,
}

impl Stdio {
    pub fn piped() -> Self {
        Self::Piped
    }

    pub fn inherit() -> Self {
        Self::Inherit
    }

    pub fn null() -> Self {
        Self::Null
    }
}

/// Tools discovered on-device (private HNP install dir), snapshotted once at
/// startup by [`init_local_tools`] and reused for every spawn afterwards.
/// `/data/app/bin` is static for the process lifetime, so the set is built
/// exactly once - no per-command directory read.
static LOCAL_TOOL_NAMES: OnceLock<Vec<String>> = OnceLock::new();

/// Install dir that HarmonyOS mounts private HNP binaries into. Only this
/// (private) location is consulted - public HNP is intentionally NOT a fallback,
/// the tool set is private-only.
const LOCAL_TOOL_BIN_DIRS: &[&str] = &["/data/app/bin"];

/// Scan the private HNP install dir and record every present executable name,
/// so a later [`Command::spawn`] can route it locally without re-reading the
/// directory. Call once at startup (idempotent): the snapshot is taken on the
/// first call and cached. If the dir is missing or unreadable the set stays
/// empty, which means every command is forwarded to the VM - nothing on-device,
/// nothing forced local.
pub fn init_local_tools() {
    LOCAL_TOOL_NAMES.get_or_init(|| {
        let mut names: Vec<String> = Vec::new();
        for dir in LOCAL_TOOL_BIN_DIRS {
            let Ok(entries) = std::fs::read_dir(dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if is_executable_file(&path) {
                    if let Some(name) = path.file_name().and_then(|name| name.to_str()) {
                        if !names.iter().any(|existing| existing == name) {
                            names.push(name.to_string());
                        }
                    }
                }
            }
        }
        names
    });
}

/// Executable mode bits (owner/group/other execute) required on a candidate
/// file before it is treated as runnable.
const EXECUTABLE_MODE_BITS: u32 = 0o111;

/// Names snapshotted as on-device tools, for startup diagnostics.
pub fn local_tool_programs() -> &'static [String] {
    LOCAL_TOOL_NAMES
        .get()
        .map(|names| names.as_slice())
        .unwrap_or(&[])
}

/// True when `program`'s basename was snapshotted as an on-device tool, so a
/// bare name ("git") and an explicit path to the same binary both route local.
fn local_exec_matches(program: &OsStr) -> bool {
    Path::new(program)
        .file_name()
        .and_then(|name| name.to_str())
        .map(|name| {
            LOCAL_TOOL_NAMES
                .get()
                .map(|names| names.iter().any(|existing| existing == name))
                .unwrap_or(false)
        })
        .unwrap_or(false)
}

/// Resolve `program` (bare name or explicit path) to an executable absolute
/// path on this device. Errors carry a user-readable hint that the matching
/// HNP must be shipped in the HAP.
fn resolve_local_program(program: &OsStr) -> io::Result<PathBuf> {
    let path = Path::new(program);
    if path.components().count() > 1 {
        if is_executable_file(path) {
            return Ok(path.to_path_buf());
        }
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!(
                "local {} not found at explicit path {}",
                program.to_string_lossy(),
                path.display()
            ),
        ));
    }
    // On-device tools live only in the private HNP install dir. The process
    // PATH is deliberately not consulted here: a name that resolves there is
    // meant for the daemon, and this path is for local `exec` only.
    for dir in LOCAL_TOOL_BIN_DIRS {
        let candidate = Path::new(dir).join(program);
        if is_executable_file(&candidate) {
            return Ok(candidate);
        }
    }
    Err(io::Error::new(
        io::ErrorKind::NotFound,
        format!(
            "{}: requested local execution but no binary found in the private \
             HNP dir {}. Ship the matching .hnp in module.json5 hnpPackages and \
             rebuild the HAP.",
            program.to_string_lossy(),
            LOCAL_TOOL_BIN_DIRS[0]
        ),
    ))
}

fn is_executable_file(path: &Path) -> bool {
    std::fs::metadata(path)
        .map(|metadata| {
            metadata.is_file()
                && (metadata.permissions().mode() & EXECUTABLE_MODE_BITS) != 0
        })
        .unwrap_or(false)
}

/// Outcome of resolving one local tool, exposed for startup diagnostics.
pub struct LocalToolStatus {
    pub program: String,
    pub resolved: Option<PathBuf>,
    pub executable: bool,
    pub error: Option<String>,
}

/// Non-fatal diagnostic mirror of [`resolve_local_program`]: never errors, so a
/// startup probe can log a missing tool without taking the process down.
pub fn local_tool_status(program: &str) -> LocalToolStatus {
    match resolve_local_program(OsStr::new(program)) {
        Ok(resolved) => {
            let executable = is_executable_file(&resolved);
            LocalToolStatus {
                program: program.to_string(),
                resolved: Some(resolved),
                executable,
                error: None,
            }
        }
        Err(error) => LocalToolStatus {
            program: program.to_string(),
            resolved: None,
            executable: false,
            error: Some(error.to_string()),
        },
    }
}

/// Initializes the global remote command executor. launch-zed calls this once
/// after it has registered the `cmd-client` executor.
pub fn init(_socket_path: &str) -> io::Result<()> {
    if executor().is_err() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            "hicodeerd executor not registered",
        ));
    }
    install_remote_which_resolver();
    Ok(())
}

fn executor() -> io::Result<Arc<dyn RemoteCommandExecutor>> {
    cmd_client::executor().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            "hicodeerd executor not initialized",
        )
    })
}

/// Counts failed resolver queries so an unavailable daemon logs once instead of
/// once per lookup: a startup that probes many programs in a row would
/// otherwise emit one warning per probe. Reset by the next successful query, so
/// a new outage after a recovery is reported again.
static RESOLVER_FAILURES: AtomicUsize = AtomicUsize::new(0);

/// Installs the resolver that answers the `which` lookups the local scan cannot
/// satisfy from the daemon's environment, where the tools the commands actually
/// run live. Only the first installation takes effect, so the startup
/// registration stays authoritative for the process lifetime.
fn install_remote_which_resolver() {
    let resolver: Box<which::Resolver> = Box::new(|name: &OsStr| resolve_in_daemon(name));
    if which::set_resolver(resolver).is_err() {
        log::warn!("command: a which resolver was already installed; keeping the first one");
    }
}

/// Answers one `which` lookup from the daemon environment: the path that
/// environment would run for `name`, or `None` when it has no such program.
/// Never propagates an error - a failed or unsupported query reads as "not
/// found there", so the caller simply keeps its local result.
fn resolve_in_daemon(name: &OsStr) -> Option<Vec<PathBuf>> {
    let executor = match executor() {
        Ok(executor) => executor,
        Err(error) => {
            report_resolver_failure(error);
            return None;
        }
    };
    let name = name.to_string_lossy();
    match executor.resolve_program(&name) {
        Ok(resolved) => {
            RESOLVER_FAILURES.store(0, Ordering::Relaxed);
            resolved.map(|path| vec![path])
        }
        Err(error) => {
            report_resolver_failure(error);
            None
        }
    }
}

/// Logs the first failure of a failure burst and counts the rest.
fn report_resolver_failure(error: io::Error) {
    if RESOLVER_FAILURES.fetch_add(1, Ordering::Relaxed) == 0 {
        log::warn!("command: which resolver query failed: {error}");
    }
}

#[derive(Debug)]
pub struct Command {
    program: OsString,
    args: Vec<OsString>,
    envs: BTreeMap<OsString, Option<OsString>>,
    env_clear: bool,
    current_dir: Option<PathBuf>,
    stdin_cfg: Stdio,
    stdout_cfg: Stdio,
    stderr_cfg: Stdio,
    kill_on_drop: bool,
}

impl Command {
    pub fn new(program: impl AsRef<OsStr>) -> Self {
        Self {
            program: program.as_ref().to_owned(),
            args: Vec::new(),
            envs: BTreeMap::new(),
            env_clear: false,
            current_dir: None,
            stdin_cfg: Stdio::default(),
            stdout_cfg: Stdio::default(),
            stderr_cfg: Stdio::default(),
            kill_on_drop: false,
        }
    }

    pub fn arg(&mut self, arg: impl AsRef<OsStr>) -> &mut Self {
        self.args.push(arg.as_ref().to_owned());
        self
    }

    pub fn args<I, S>(&mut self, args: I) -> &mut Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        self.args
            .extend(args.into_iter().map(|arg| arg.as_ref().to_owned()));
        self
    }

    pub fn get_args(&self) -> impl Iterator<Item = &OsStr> {
        self.args.iter().map(|arg| arg.as_os_str())
    }

    /// Records an override for the child. Honoured only when the child runs on
    /// this device; a remote child starts from the daemon's environment instead
    /// (see `build_spec`).
    pub fn env(&mut self, key: impl AsRef<OsStr>, val: impl AsRef<OsStr>) -> &mut Self {
        self.envs
            .insert(key.as_ref().to_owned(), Some(val.as_ref().to_owned()));
        self
    }

    pub fn envs<I, K, V>(&mut self, vars: I) -> &mut Self
    where
        I: IntoIterator<Item = (K, V)>,
        K: AsRef<OsStr>,
        V: AsRef<OsStr>,
    {
        for (key, val) in vars {
            self.envs
                .insert(key.as_ref().to_owned(), Some(val.as_ref().to_owned()));
        }
        self
    }

    pub fn env_remove(&mut self, key: impl AsRef<OsStr>) -> &mut Self {
        let key = key.as_ref().to_owned();
        if self.env_clear {
            self.envs.remove(&key);
        } else {
            self.envs.insert(key, None);
        }
        self
    }

    pub fn env_clear(&mut self) -> &mut Self {
        self.env_clear = true;
        self.envs.clear();
        self
    }

    pub fn current_dir(&mut self, dir: impl AsRef<Path>) -> &mut Self {
        self.current_dir = Some(dir.as_ref().to_owned());
        self
    }

    pub fn stdin(&mut self, cfg: Stdio) -> &mut Self {
        self.stdin_cfg = cfg;
        self
    }

    pub fn stdout(&mut self, cfg: Stdio) -> &mut Self {
        self.stdout_cfg = cfg;
        self
    }

    pub fn stderr(&mut self, cfg: Stdio) -> &mut Self {
        self.stderr_cfg = cfg;
        self
    }

    pub fn kill_on_drop(&mut self, kill_on_drop: bool) -> &mut Self {
        self.kill_on_drop = kill_on_drop;
        self
    }

    pub fn get_program(&self) -> &OsStr {
        self.program.as_os_str()
    }

    /// Rebuild a routable command from an already-built `std::process::Command`,
    /// copying program, arguments, working directory and the per-variable
    /// environment overrides. `env_clear` is deliberately not recovered: std
    /// exposes no getter for it, so a cleared environment is indistinguishable
    /// from an untouched one, and guessing wrong would silently leak the
    /// caller's environment into the child.
    pub fn from_std(std_command: std::process::Command) -> Self {
        let mut command = Self::new(std_command.get_program());
        command.args(std_command.get_args());
        if let Some(dir) = std_command.get_current_dir() {
            command.current_dir(dir);
        }
        for (key, value) in std_command.get_envs() {
            match value {
                Some(value) => {
                    command.env(key, value);
                }
                None => {
                    command.env_remove(key);
                }
            }
        }
        command
    }

    /// Spawns the command. Tools snapshotted as on-device (git/ssh/curl)
    /// fork on the device itself; everything else is spawned through the VM
    /// executor. Blocking: the remote handshake (plus the SpawnOk wait)
    /// completes here, so the returned `Child` is immediately usable.
    pub fn spawn(&mut self) -> io::Result<Child> {
        if local_exec_matches(&self.program) {
            // A tool that exists on-device is forced local; a missing binary is
            // an error, never a silent fallback to the VM.
            return spawn_local(self);
        }
        let executor = executor()?;
        let child = executor.spawn(self.build_spec())?;
        Ok(Child {
            stdin: child.stdin,
            stdout: child.stdout,
            stderr: child.stderr,
            kind: ChildKind::Remote {
                session_id: child.session_id,
                executor,
            },
            kill_on_drop: self.kill_on_drop,
        })
    }

    pub async fn output(&mut self) -> io::Result<Output> {
        self.spawn()?.output().await
    }

    pub async fn status(&mut self) -> io::Result<ExitStatus> {
        let mut child = self.spawn()?;
        child.status().await
    }

    fn build_spec(&self) -> ExecSpec {
        let program = self.program.to_string_lossy().into_owned();
        let mut spec = ExecSpec::new(program.clone());
        spec.source_program = program;
        spec.args = self
            .args
            .iter()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        spec.cwd_path = self
            .current_dir
            .as_ref()
            .map(|dir| dir.to_string_lossy().into_owned());
        // A remote command carries no environment: it starts from whatever the
        // daemon itself was launched with. The two run under different uids, so
        // the caller's own environment can name roots that only resolve inside
        // its sandbox, and forwarding those would hand the child a path it
        // cannot open. Explicit overrides are honoured only where the child runs
        // on this device -- see `spawn_local`.
        spec.stdin_mode = fd_mode(self.stdin_cfg);
        spec.stdout_mode = fd_mode(self.stdout_cfg);
        spec.stderr_mode = fd_mode(self.stderr_cfg);
        spec
    }
}

fn fd_mode(stdio: Stdio) -> FdMode {
    match stdio {
        Stdio::Piped => FdMode::Piped,
        // Inherit has no meaning for a remote child (the VM has no caller
        // terminal), so it maps to /dev/null like Null.
        Stdio::Inherit | Stdio::Null => FdMode::Null,
    }
}

/// Apply the env entries a local HNP tool needs to find its own runtime:
/// its bin dir prepended on PATH (so git can spawn ssh / git-upload-pack from
/// the same package) plus git's libexec/templates. The tool's bundled .so
/// files deliberately need no LD_LIBRARY_PATH here: every shipped binary
/// carries the official DT_RUNPATH ($ORIGIN/../lib on executables, $ORIGIN on
/// the libraries), and a stray LD_LIBRARY_PATH would only shadow that RUNPATH
/// (LD_LIBRARY_PATH is searched first) - so none is injected, letting the
/// official $ORIGIN resolution drive library loading. Each entry is applied
/// only when the corresponding directory actually exists, so a build without
/// the HNP never depends on hard-coded absolute paths.
fn apply_local_tool_env(command: &mut std::process::Command, resolved: &Path) {
    let Ok(real) = std::fs::canonicalize(resolved) else {
        return;
    };
    let Some(bin_dir) = real.parent() else {
        return;
    };
    let Some(root) = bin_dir.parent() else {
        return;
    };
    let tool_name = real.file_name().and_then(|name| name.to_str()).unwrap_or("");

    // PATH: make the tool's own bin dir resolvable to child processes.
    let want_bin = bin_dir.to_string_lossy().into_owned();
    let current_path = std::env::var_os("PATH").unwrap_or_default();
    let full_path = if current_path.is_empty() {
        want_bin.clone()
    } else {
        format!("{}:{}", want_bin, current_path.to_string_lossy())
    };
    if full_path != want_bin {
        command.env("PATH", full_path);
    }

    // git needs libexec/git-core (git-remote-http/https) and the init templates.
    if tool_name == "git" {
        let git_core = root.join("libexec").join("git-core");
        if git_core.is_dir() {
            command.env("GIT_EXEC_PATH", &git_core);
        }
        let templates = root.join("share").join("git-core").join("templates");
        if templates.is_dir() {
            command.env("GIT_TEMPLATE_DIR", templates);
        }
    }
}

fn local_stdio(stdio: Stdio) -> std::process::Stdio {
    match stdio {
        Stdio::Piped => std::process::Stdio::piped(),
        // A local child genuinely shares the caller's descriptors, so Inherit
        // keeps its real meaning here (the remote path maps it to null instead).
        Stdio::Inherit => std::process::Stdio::inherit(),
        Stdio::Null => std::process::Stdio::null(),
    }
}

/// Fork and exec a local HNP tool on this device. Never touches the remote
/// executor; a resolution/spawn failure surfaces as an error. Plain
/// `std::process` is used deliberately (no `pre_exec`), so the OHOS musl
/// signal-reset / close_fds workarounds required by warp's PTY fork path do not
/// apply here, and exit observation is handed to [`LocalChild`] rather than to
/// `async-process`.
///
/// `std::process::Command` is the workspace's disallowed default because its
/// I/O can block the calling thread; this path accepts that (spawn is a fast
/// fork/exec, and the streams are made non-blocking below) because the
/// `smol::process` alternative is the one that misbehaves on this kernel.
#[allow(clippy::disallowed_methods)]
fn spawn_local(command: &Command) -> io::Result<Child> {
    let exe = resolve_local_program(&command.program)?;
    let mut child_command = std::process::Command::new(&exe);
    child_command.args(&command.args);
    child_command.stdin(local_stdio(command.stdin_cfg));
    child_command.stdout(local_stdio(command.stdout_cfg));
    child_command.stderr(local_stdio(command.stderr_cfg));
    if let Some(dir) = &command.current_dir {
        child_command.current_dir(dir);
    }
    if command.env_clear {
        child_command.env_clear();
    }
    for (key, value) in &command.envs {
        match value {
            Some(value) => {
                child_command.env(key, value);
            }
            None => {
                child_command.env_remove(key);
            }
        }
    }
    apply_local_tool_env(&mut child_command, &exe);
    let mut process = child_command.spawn().map_err(|error| {
        log::error!(
            "util::command::spawn_local: spawn {} failed: {error} (HNP type 'private' may deny \
             main-process exec on this device; flip to 'public' and reinstall if EACCES)",
            exe.display()
        );
        io::Error::new(error.kind(), format!("spawn {} failed: {error}", exe.display()))
    })?;
    // Take the pipes out first, then wrap them. A `?` here used to run before
    // `LocalChild::new` took ownership, so a failed wrap dropped the child
    // without killing or reaping it: a running orphan, and a zombie once it
    // exited. Every failure is now funnelled through `reap_local_child`.
    let stdin_pipe = process.stdin.take();
    let stdout_pipe = process.stdout.take();
    let stderr_pipe = process.stderr.take();
    let stdin = match wrap_child_stdin(stdin_pipe) {
        Ok(stream) => stream,
        Err(error) => return Err(reap_local_child(process, error)),
    };
    let stdout = match wrap_child_stdout(stdout_pipe) {
        Ok(stream) => stream,
        Err(error) => return Err(reap_local_child(process, error)),
    };
    let stderr = match wrap_child_stderr(stderr_pipe) {
        Ok(stream) => stream,
        Err(error) => return Err(reap_local_child(process, error)),
    };
    let child = LocalChild::new(process)?;
    Ok(Child {
        stdin,
        stdout,
        stderr,
        kind: ChildKind::Local { child },
        kill_on_drop: command.kill_on_drop,
    })
}

/// Wrap a piped child stdin as an async stream driven by the smol reactor. A
/// non-piped descriptor was never captured, so it maps to `None`.
fn wrap_child_stdin(
    stream: Option<std::process::ChildStdin>,
) -> io::Result<Option<Box<dyn AsyncWrite + Unpin + Send + Sync>>> {
    match stream {
        Some(stream) => Ok(Some(
            Box::new(smol::Async::new(stream)?) as Box<dyn AsyncWrite + Unpin + Send + Sync>
        )),
        None => Ok(None),
    }
}

/// Wrap a piped child stdout as an async stream driven by the smol reactor.
fn wrap_child_stdout(
    stream: Option<std::process::ChildStdout>,
) -> io::Result<Option<Box<dyn AsyncRead + Unpin + Send + Sync>>> {
    match stream {
        Some(stream) => Ok(Some(
            Box::new(smol::Async::new(stream)?) as Box<dyn AsyncRead + Unpin + Send + Sync>
        )),
        None => Ok(None),
    }
}

/// Wrap a piped child stderr as an async stream driven by the smol reactor.
fn wrap_child_stderr(
    stream: Option<std::process::ChildStderr>,
) -> io::Result<Option<Box<dyn AsyncRead + Unpin + Send + Sync>>> {
    match stream {
        Some(stream) => Ok(Some(
            Box::new(smol::Async::new(stream)?) as Box<dyn AsyncRead + Unpin + Send + Sync>
        )),
        None => Ok(None),
    }
}

/// Kills and reaps a local child whose stdio could not be wrapped. `LocalChild`
/// has not taken the process over yet, so no waiter thread can race this; both
/// outcomes are logged and the original error is returned unchanged.
fn reap_local_child(mut process: std::process::Child, cause: io::Error) -> io::Error {
    let pid = process.id();
    if let Err(error) = process.kill() {
        log::warn!(
            "util::command::spawn_local: killing local child {pid} after stdio setup failed: \
             {error}"
        );
    }
    if let Err(error) = process.wait() {
        log::warn!(
            "util::command::spawn_local: reaping local child {pid} after stdio setup failed: \
             {error}"
        );
    }
    cause
}

/// A locally forked child whose exit is observed by a dedicated blocking waiter
/// thread instead of by polling a `pidfd`.
///
/// HarmonyOS answers `pidfd_open` with a valid descriptor but reports that
/// descriptor as pollable before the process has exited. `async-process` picks
/// its pidfd backend purely from `pidfd_open` succeeding (`wait::available`),
/// so `WaitableChild::poll_wait` never blocks: it loops `try_wait` ->
/// `poll_readable` at full CPU on whichever thread drives the future (in the
/// observed freeze, the foreground one). Local children therefore observe exit
/// through a thread that blocks in `waitpid`, publishes the result to `state`,
/// and wakes the async waiter. A pidfd is still used for `kill`, where the
/// problem is the opposite one: the signal must not reach a recycled pid.
struct LocalChild {
    /// OS process id, valid until the waiter thread has reaped the child.
    pid: u32,
    /// Exit outcome, shared with the waiter thread and the async waiters.
    state: Arc<Mutex<LocalExitState>>,
}

/// The exit result a [`LocalChild`] is waiting for, or the reason the blocking
/// wait itself failed.
enum LocalExit {
    /// The waiter thread has not returned yet.
    Pending,
    /// The child exited with this status.
    Exited(ExitStatus),
    /// The blocking wait failed; the message is the rendered cause.
    Failed(String),
}

/// Shared state between a [`LocalChild`] and its waiter thread.
struct LocalExitState {
    outcome: LocalExit,
    /// Async waiters to wake once `outcome` leaves [`LocalExit::Pending`].
    wakers: Vec<Waker>,
}

impl LocalChild {
    /// Hand `process` to a dedicated thread that blocks in `waitpid` and
    /// publishes the exit. The returned value is immediately usable: `kill` and
    /// `id` act on the pid, and `status` waits on the published outcome.
    fn new(process: std::process::Child) -> io::Result<Self> {
        let pid = process.id();
        let state = Arc::new(Mutex::new(LocalExitState {
            outcome: LocalExit::Pending,
            wakers: Vec::new(),
        }));
        // The child is handed to the thread through a slot so that a failed
        // `spawn` leaves it reachable for a synchronous reap here instead of
        // leaking a zombie.
        let handoff = Arc::new(Mutex::new(Some(process)));
        let waiter_handoff = Arc::clone(&handoff);
        let waiter_state = Arc::clone(&state);
        let started = std::thread::Builder::new()
            .name(format!("ohos-local-wait-{pid}"))
            .spawn(move || {
                let Some(mut process) = waiter_handoff.lock().unwrap().take() else {
                    return;
                };
                let outcome = match process.wait() {
                    Ok(status) => LocalExit::Exited(status),
                    Err(error) => {
                        LocalExit::Failed(format!("wait for local child {pid} failed: {error}"))
                    }
                };
                publish_exit(&waiter_state, outcome);
            });
        if let Err(error) = started {
            log::error!(
                "util::command::spawn_local: cannot start waiter thread for local child {pid}: \
                 {error}"
            );
            if let Some(mut process) = handoff.lock().unwrap().take() {
                if let Err(kill_error) = process.kill() {
                    log::warn!(
                        "util::command::spawn_local: killing local child {pid} after the waiter \
                         thread failed to start failed: {kill_error}"
                    );
                }
                if let Err(wait_error) = process.wait() {
                    log::warn!(
                        "util::command::spawn_local: reaping local child {pid} after the waiter \
                         thread failed to start failed: {wait_error}"
                    );
                }
            }
            return Err(io::Error::new(
                error.kind(),
                format!("cannot start waiter thread for local child {pid}: {error}"),
            ));
        }
        Ok(Self { pid, state })
    }

    fn id(&self) -> u32 {
        self.pid
    }

    fn kill(&mut self) -> io::Result<()> {
        if !matches!(&self.state.lock().unwrap().outcome, LocalExit::Pending) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "local child has already exited",
            ));
        }
        // Signal through a pidfd rather than the bare pid. The waiter thread may
        // reap the child between the check above and the signal below; a raw pid
        // could then name a recycled process and this SIGKILL would land on it.
        // The pidfd pins this child's identity, so the worst case is that an
        // already-exited child reports ESRCH.
        let pidfd = open_pidfd(self.pid)?;
        send_signal_through_pidfd(&pidfd, self.pid as libc::pid_t, libc::SIGKILL)
    }

    fn try_status(&mut self) -> io::Result<Option<ExitStatus>> {
        let state = self.state.lock().unwrap();
        match &state.outcome {
            LocalExit::Pending => Ok(None),
            LocalExit::Exited(status) => Ok(Some(*status)),
            LocalExit::Failed(message) => Err(io::Error::other(message.clone())),
        }
    }

    /// Future resolving once the waiter thread has published the exit. The
    /// clone keeps the future `'static`, so callers can move it elsewhere.
    fn status(&self) -> impl std::future::Future<Output = io::Result<ExitStatus>> + Send + 'static {
        let state = Arc::clone(&self.state);
        async move { futures_lite::future::poll_fn(move |cx| poll_exit(&state, cx)).await }
    }
}

/// Opens a pidfd for `pid` with the raw syscall: this target's libc exposes the
/// syscall number but no wrapper. The descriptor keeps naming that one process
/// even after it is reaped, which is what makes signalling it race-free.
fn open_pidfd(pid: u32) -> io::Result<OwnedFd> {
    // SAFETY: pidfd_open(2) takes (pid, flags) and returns a fresh descriptor or
    // -1; zero flags is the plain lookup.
    let descriptor = unsafe {
        libc::syscall(libc::SYS_pidfd_open, pid as libc::pid_t as libc::c_long, 0 as libc::c_long)
    };
    if descriptor < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: the syscall returned a descriptor that this process owns.
    Ok(unsafe { OwnedFd::from_raw_fd(descriptor as libc::c_int) })
}

/// Delivers `signal` to the process a pidfd refers to. An already-exited child
/// makes the syscall fail with ESRCH instead of reaching another process.
fn send_signal_through_pidfd(
    pidfd: &OwnedFd,
    pid: libc::pid_t,
    signal: libc::c_int,
) -> io::Result<()> {
    // SAFETY: `pidfd` is a live pidfd; a null siginfo and zero flags are the
    // documented way to send a plain signal.
    let result = unsafe {
        libc::syscall(
            libc::SYS_pidfd_send_signal,
            pidfd.as_raw_fd() as libc::c_long,
            signal as libc::c_long,
            std::ptr::null::<libc::siginfo_t>(),
            0 as libc::c_long,
        )
    };
    if result == 0 {
        return Ok(());
    }
    let error = io::Error::last_os_error();
    // Some OHOS kernels predate pidfd_send_signal(2) and reject it with ENOSYS.
    // Fall back to a bare kill instead of leaving kill() permanently broken:
    // the pidfd identity check is lost, so a pid recycled between the caller's
    // liveness check and this signal could receive it. That window is very
    // small, and a working kill is worth more than the guarantee.
    if error.raw_os_error() == Some(libc::ENOSYS) {
        log::warn!(
            "command: pidfd_send_signal is unimplemented (ENOSYS); falling back to kill(pid={pid}, signal={signal})"
        );
        // SAFETY: kill(2) reads no memory; it only names a pid and a signal.
        if unsafe { libc::kill(pid, signal) } == 0 {
            return Ok(());
        }
        return Err(io::Error::last_os_error());
    }
    Err(error)
}

/// Store the final outcome and wake every registered waiter. Runs on the waiter
/// thread; the lock is released before waking so a waker cannot re-enter here.
fn publish_exit(state: &Mutex<LocalExitState>, outcome: LocalExit) {
    let wakers: Vec<Waker> = {
        let mut state = state.lock().unwrap();
        state.outcome = outcome;
        state.wakers.drain(..).collect()
    };
    for waker in wakers {
        waker.wake();
    }
}

/// Poll the shared outcome, registering `cx`'s waker while it is still pending.
fn poll_exit(state: &Mutex<LocalExitState>, cx: &mut Context<'_>) -> Poll<io::Result<ExitStatus>> {
    let mut state = state.lock().unwrap();
    match &state.outcome {
        LocalExit::Pending => {}
        LocalExit::Exited(status) => return Poll::Ready(Ok(*status)),
        LocalExit::Failed(message) => {
            return Poll::Ready(Err(io::Error::other(message.clone())));
        }
    }
    if !state.wakers.iter().any(|waker| waker.will_wake(cx.waker())) {
        state.wakers.push(cx.waker().clone());
    }
    Poll::Pending
}

/// Where the child actually runs: on the VM through the remote executor, or
/// forked locally on this device (a private HNP tool).
enum ChildKind {
    Remote {
        session_id: u64,
        executor: Arc<dyn RemoteCommandExecutor>,
    },
    Local {
        child: LocalChild,
    },
}

// The streams are `Sync` as well as `Send` because `util::process::Child`
// aliases this type on OHOS and its callers require `Send + Sync` (e.g.
// `context_server::Transport`); the concrete streams behind them (smol
// `Async<..>`) already satisfy both.
pub struct Child {
    pub stdin: Option<Box<dyn AsyncWrite + Unpin + Send + Sync>>,
    pub stdout: Option<Box<dyn AsyncRead + Unpin + Send + Sync>>,
    pub stderr: Option<Box<dyn AsyncRead + Unpin + Send + Sync>>,
    kill_on_drop: bool,
    kind: ChildKind,
}

// Trait objects (Box<dyn AsyncWrite/AsyncRead>) do not implement Debug, so a
// hand-written impl is required instead of a derived one. Only scalar fields
// are printed; the underlying streams are intentionally omitted.
impl std::fmt::Debug for Child {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut builder = f.debug_struct("Child");
        builder.field("kill_on_drop", &self.kill_on_drop);
        match &self.kind {
            ChildKind::Remote { session_id, .. } => {
                builder.field("session_id", session_id);
            }
            ChildKind::Local { .. } => {
                builder.field("kind", &"local");
            }
        }
        builder.finish()
    }
}

impl Child {
    pub fn id(&self) -> u32 {
        match &self.kind {
            ChildKind::Remote { session_id, .. } => *session_id as u32,
            ChildKind::Local { child } => child.id(),
        }
    }

    pub fn kill(&mut self) -> io::Result<()> {
        match &mut self.kind {
            ChildKind::Remote { session_id, executor } => {
                executor.signal(*session_id, Signal::SigKill)
            }
            ChildKind::Local { child } => child.kill(),
        }
    }

    pub fn try_status(&mut self) -> io::Result<Option<ExitStatus>> {
        match &mut self.kind {
            ChildKind::Remote { session_id, executor } => {
                Ok(executor.try_exit(*session_id).map(status_from_code))
            }
            ChildKind::Local { child } => child.try_status(),
        }
    }

    pub fn status(
        &mut self,
    ) -> impl std::future::Future<Output = io::Result<ExitStatus>> + Send + 'static {
        // Boxed dyn future: the two arms have unrelated concrete future types,
        // and the opaque return type requires a single concrete type, so both
        // are coerced to Pin<Box<dyn Future + Send>>.
        let future: std::pin::Pin<
            Box<dyn std::future::Future<Output = io::Result<ExitStatus>> + Send>,
        > = match &mut self.kind {
            ChildKind::Remote { session_id, executor } => {
                // Clone the owned pieces so the returned future does not borrow
                // `self`, mirroring the desktop (smol) contract (impl Future +
                // Send + 'static): callers can spawn the status future elsewhere.
                let executor = executor.clone();
                let session_id = *session_id;
                Box::pin(async move {
                    let exit_code = executor.wait_exit_async(session_id).await?;
                    Ok(status_from_code(exit_code))
                })
            }
            ChildKind::Local { child } => {
                // `LocalChild::status` is `'static` (it clones the shared
                // state), so it boxes without borrowing `child` afterwards.
                Box::pin(child.status())
            }
        };
        future
    }

    pub async fn output(mut self) -> io::Result<Output> {
        // Close stdin before waiting: `output` consumes the child and can never
        // feed it, so a command that reads stdin must see EOF or it blocks
        // forever. On the remote path the daemon only forwards that EOF once
        // this side drops its stdin end, so keeping it open hangs the child and
        // this future with it.
        self.stdin.take();

        let status = self.status();

        let stdout = self.stdout.take();
        let stdout_future = async move {
            let mut data = Vec::new();
            if let Some(mut stdout) = stdout {
                stdout.read_to_end(&mut data).await?;
            }
            io::Result::Ok(data)
        };

        let stderr = self.stderr.take();
        let stderr_future = async move {
            let mut data = Vec::new();
            if let Some(mut stderr) = stderr {
                stderr.read_to_end(&mut data).await?;
            }
            io::Result::Ok(data)
        };

        // Read both streams concurrently: reading them one after the other
        // deadlocks, because the relay blocks on a full stderr pipe while the
        // caller is still waiting for stdout to reach EOF.
        let (stdout_buf, stderr_buf) =
            futures_lite::future::try_zip(stdout_future, stderr_future).await?;
        let status = status.await?;

        Ok(Output {
            status,
            stdout: stdout_buf,
            stderr: stderr_buf,
        })
    }

    /// Spawn a process from an already-built `std::process::Command`, routing it
    /// exactly like [`Command::spawn`]: an on-device HNP tool forks locally,
    /// everything else is forwarded to the daemon. This is the entry point for
    /// callers that hold a plain std command they did not build through
    /// [`Command`] (the shared `util::process` call sites), so the sandbox
    /// workaround stays in one place instead of being repeated per caller.
    pub fn spawn(
        std_command: std::process::Command,
        stdin: Stdio,
        stdout: Stdio,
        stderr: Stdio,
    ) -> io::Result<Child> {
        let mut command = Command::from_std(std_command);
        command.stdin(stdin).stdout(stdout).stderr(stderr);
        command.spawn()
    }
}

impl Drop for Child {
    fn drop(&mut self) {
        if self.kill_on_drop {
            match &mut self.kind {
                ChildKind::Remote { session_id, executor } => {
                    // Best-effort: killing an already-exited process group is a
                    // no-op on the server.
                    let _ = executor.signal(*session_id, Signal::SigKill);
                }
                ChildKind::Local { child } => {
                    let _ = child.kill();
                }
            }
        }
    }
}

fn status_from_code(exit_code: Option<i32>) -> ExitStatus {
    match exit_code {
        Some(code) => ExitStatus::from_raw(code << 8),
        // Terminated by a signal; approximate with a non-successful status.
        None => ExitStatus::from_raw(128 << 8),
    }
}
