//! Remote thin-client launcher over SSH command stdio.

use super::{args::*, process::wait_with_output_timeout, restart_policy::*, shell_quote};
use base64::Engine as _;
use std::fs;
use std::io::{self, IsTerminal, Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};

use interprocess::local_socket::traits::Listener as _;
#[cfg(all(test, unix))]
use interprocess::local_socket::traits::Stream as _;
use interprocess::local_socket::ListenerNonblockingMode;
use interprocess::TryClone as _;
use serde::Deserialize;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    mpsc, Arc,
};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

const BRIDGE_ACCEPT_POLL: Duration = Duration::from_millis(50);
const BRIDGE_IO_POLL: Duration = Duration::from_millis(1);
const BRIDGE_SOCKET_PERMISSION_MODE: u32 = 0o600;
const REMOTE_SERVER_SHUTDOWN_CONFIRM_TIMEOUT: Duration = Duration::from_secs(5);
const NONINTERACTIVE_SSH_COMMAND_TIMEOUT: Duration = Duration::from_secs(15);
const NONINTERACTIVE_SSH_STDERR_LIMIT: usize = 16 * 1024;
const BRIDGE_FAILURE_REPORT_TIMEOUT: Duration = Duration::from_secs(1);
const REMOTE_SERVER_SHUTDOWN_POLL_INTERVAL: Duration = Duration::from_millis(100);
const REMOTE_OUTPUT_READY_MARKER: &str = "hicodeer-remote-output-ready:1";
const WINDOWS_REMOTE_PATH_MARKER: &str = "hicodeer-remote-path:1:";
const SSH_CONTROL_SOCKET_NAME: &str = "ctl";
pub(crate) fn run_remote(remote: RemoteLaunch) -> io::Result<()> {
    let session_name = crate::session::active_name()
        .unwrap_or_else(|| crate::session::DEFAULT_SESSION_NAME.to_string());
    let local_socket = local_forward_socket_path(&remote.target, &session_name);
    let program = std::env::args()
        .next()
        .unwrap_or_else(|| "herdr".to_string());
    let reattach_command = reattach_command(
        &program,
        &remote.target,
        &session_name,
        remote.keybindings,
        remote.live_handoff,
    );
    let manage_ssh_config = crate::config::Config::load()
        .config
        .remote
        .manage_ssh_config;
    let require_surface_interest = crate::client::endpoint::EndpointCatalog::load()
        .map(|catalog| catalog.contains_enabled_target_session(&remote.target, &session_name))
        .unwrap_or(false);
    let remote_ssh = RemoteSsh::new(
        remote.target.clone(),
        manage_ssh_config,
        session_name.clone(),
    );
    let prepared_remote =
        prepare_remote_herdr(&remote_ssh, require_surface_interest)?;
    ensure_remote_server_ready(
        &remote_ssh,
        &prepared_remote.remote_herdr,
        remote.live_handoff,
        require_surface_interest,
    )?;

    let _bridge = SshStdioBridge::start(
        remote.target.clone(),
        prepared_remote.remote_herdr,
        local_socket.clone(),
        session_name.clone(),
        remote_ssh.options(),
        false,
    )?;

    run_client_process(
        &local_socket,
        &reattach_command,
        remote.keybindings,
        &remote.target,
        &session_name,
    )
}

pub(crate) fn check_saved_ssh(target: &str, session: &str) -> io::Result<()> {
    super::validate_remote_target(target)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?;
    crate::session::validate_name(session)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?;
    let mut ssh = RemoteSsh::new_noninteractive(target.to_owned());
    ssh.session_name = session.to_owned();
    let remote = find_installed_remote_herdr(&ssh)?;
    match remote_server_status(&ssh, &remote, false)? {
        RemoteServerStatus::Running {
            endpoint_protocol_generation,
            surface_interest,
            health_check,
            detached_server_daemon,
            ..
        } if remote_server_restart_reason(
            endpoint_protocol_generation,
            detached_server_daemon,
            true,
            surface_interest,
            health_check,
        )
        .is_none() =>
        {
            Ok(())
        }
        _ => Err(io::Error::other(format!(
            "remote HiCodeer server is stopped or incompatible; run `{}`",
            super::saved_ssh_bootstrap_command(target, session),
        ))),
    }
}

pub(crate) struct SavedSshSetup {
    ssh: RemoteSsh,
    remote_herdr: RemoteHerdr,
    candidates: Vec<RemoteHerdr>,
}

impl SavedSshSetup {
    pub(crate) fn connect(target: &str) -> io::Result<Self> {
        super::validate_remote_target(target)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?;
        let manage = crate::config::Config::load()
            .config
            .remote
            .manage_ssh_config;
        let ssh = RemoteSsh::new(
            target.to_owned(),
            manage,
            crate::session::DEFAULT_SESSION_NAME.to_owned(),
        );
        let remote_herdr =
            RemoteHerdr::for_platform(detect_remote_platform(&ssh)?.into_setup_platform());
        let candidates = remote_binary_candidates(&ssh, &remote_herdr)?;
        Ok(Self {
            ssh,
            remote_herdr,
            candidates,
        })
    }

    pub(crate) fn running_sessions(&self) -> io::Result<Vec<String>> {
        // A host without a usable HiCodeer reports no sessions; the caller
        // decides whether to keep going with the default session.
        if self.candidates.is_empty() {
            return Ok(Vec::new());
        }
        let mut failure = String::new();
        for candidate in &self.candidates {
            let output = self.ssh.shell_output(
                &candidate.platform,
                &candidate.executable.command(&["session", "list", "--json"]),
            )?;
            if output.status.code() == Some(255) {
                return Err(command_failed("remote SSH connection failed", &output));
            }
            if !output.status.success() {
                failure = command_failed("remote session query failed", &output).to_string();
                continue;
            }
            match serde_json::from_slice::<RemoteSessionListJson>(&output.stdout) {
                Ok(list) => {
                    return Ok(list
                        .sessions
                        .into_iter()
                        .filter(|session| {
                            session.running && crate::session::validate_name(&session.name).is_ok()
                        })
                        .map(|session| session.name)
                        .collect())
                }
                Err(error) => failure = format!("invalid remote session list: {error}"),
            }
        }
        Err(io::Error::other(format!(
            "could not discover remote HiCodeer sessions: {failure}; specify --remote-session to continue"
        )))
    }

    pub(crate) fn prepare(
        mut self,
        session_name: &str,
    ) -> io::Result<Option<crate::client::endpoint::SshMachineMetadata>> {
        crate::session::validate_name(session_name)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?;
        self.ssh.session_name = session_name.to_owned();
        let Self {
            ssh,
            remote_herdr,
            candidates,
        } = self;
        let prepared =
            prepare_discovered_remote_herdr(&ssh, remote_herdr, &candidates, true)?;
        ensure_remote_server_ready(&ssh, &prepared.remote_herdr, false, true)?;

        // The bridge already owns daemon startup. EOF closes only this temporary attachment,
        // leaving the named server running even when no local TUI is open yet.
        let command = prepared
            .remote_herdr
            .executable
            .saved_bridge_command(session_name);
        let output = ssh.shell_output(&prepared.remote_herdr.platform, &command)?;
        if !output.status.success() {
            return Err(command_failed("remote server startup failed", &output));
        }
        match remote_server_status(&ssh, &prepared.remote_herdr, true)? {
            RemoteServerStatus::Running {
                endpoint_protocol_generation,
                surface_interest,
                health_check,
                detached_server_daemon,
                ..
            } if remote_server_restart_reason(
                endpoint_protocol_generation,
                detached_server_daemon,
                true,
                surface_interest,
                health_check,
            )
            .is_none() =>
            {
                Ok(prepared.remote_herdr.machine_metadata().or_else(|| {
                    discover_remote_api_metadata(&ssh, session_name)
                        .inspect_err(
                            |error| tracing::debug!(%error, "could not capture SSH setup metadata"),
                        )
                        .ok()
                }))
            }
            _ => Err(io::Error::other(
                "remote server is not ready for saved machines",
            )),
        }
    }
}

#[derive(Deserialize)]
struct RemoteSessionListJson {
    sessions: Vec<RemoteSessionJson>,
}

#[derive(Deserialize)]
struct RemoteSessionJson {
    name: String,
    running: bool,
}

#[derive(Debug, PartialEq, Eq)]
struct DetectedRemotePlatform {
    platform: RemotePlatform,
    setup_note: Option<&'static str>,
}

impl DetectedRemotePlatform {
    fn into_setup_platform(self) -> RemotePlatform {
        if let Some(note) = self.setup_note {
            eprintln!("note: {note}");
        }
        self.platform
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RemotePlatform {
    os: &'static str,
    arch: &'static str,
}

impl RemotePlatform {
    fn from_uname(os: &str, arch: &str) -> Option<Self> {
        let os = match os.trim() {
            "Linux" => "linux",
            "Darwin" => "macos",
            _ => return None,
        };
        let arch = match arch.trim() {
            "x86_64" | "amd64" => "x86_64",
            "aarch64" | "arm64" => "aarch64",
            _ => return None,
        };
        Some(Self { os, arch })
    }

    fn is_windows(&self) -> bool {
        self.os == "windows"
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum RemoteExecutable {
    PosixShellPath(String),
    WindowsPath(String),
}

impl RemoteExecutable {
    fn display(&self) -> &str {
        match self {
            Self::PosixShellPath(path) | Self::WindowsPath(path) => path,
        }
    }

    fn command(&self, args: &[&str]) -> String {
        match self {
            Self::PosixShellPath(path) => {
                let mut command = path.clone();
                for arg in args {
                    command.push(' ');
                    command.push_str(&shell_quote(arg));
                }
                command
            }
            Self::WindowsPath(path) => windows_powershell_script_command(
                &windows_powershell_application_script(path, args),
            ),
        }
    }

    fn session_command(&self, session_name: &str, args: &[&str]) -> String {
        self.command(&Self::session_args(session_name, args))
    }

    fn session_args<'a>(session_name: &'a str, args: &[&'a str]) -> Vec<&'a str> {
        let mut session_args = Vec::with_capacity(args.len() + 2);
        if session_name != crate::session::DEFAULT_SESSION_NAME {
            session_args.extend(["--session", session_name]);
        }
        session_args.extend_from_slice(args);
        session_args
    }


    fn status_client_command(&self) -> String {
        match self {
            Self::PosixShellPath(path) => format!(
                "test -x {path} && {}",
                self.command(&["status", "client", "--json"])
            ),
            Self::WindowsPath(_) => self.command(&["status", "client", "--json"]),
        }
    }

    fn bridge_command(&self, session_name: &str) -> String {
        self.bridge_command_with_idle_timeout(session_name, false)
    }

    fn bridge_command_with_idle_timeout(&self, session_name: &str, idle_timeout: bool) -> String {
        let command = if idle_timeout {
            &["remote-client-bridge", "--idle-timeout-v1"][..]
        } else {
            &["remote-client-bridge"][..]
        };
        let args = Self::session_args(session_name, command);
        match self {
            Self::PosixShellPath(_) => {
                posix_remote_output_command(&format!("exec {}", self.command(&args)))
            }
            Self::WindowsPath(path) => {
                windows_powershell_streaming_application_command(path, &args)
            }
        }
    }

    fn saved_bridge_command(&self, session_name: &str) -> String {
        let args = Self::session_args(session_name, &["remote-client-bridge"]);
        match self {
            Self::PosixShellPath(_) => format!("exec {} </dev/null", self.command(&args)),
            Self::WindowsPath(path) => {
                windows_powershell_streaming_application_command(path, &args)
            }
        }
    }

    fn live_handoff_command(&self, session_name: &str, protocol: u32, version: &str) -> String {
        let protocol = protocol.to_string();
        match self {
            Self::PosixShellPath(path) => format!(
                "{} --import-exe {path} --expected-protocol {} --expected-version {}",
                self.session_command(session_name, &["server", "live-handoff"]),
                shell_quote(&protocol),
                shell_quote(version),
            ),
            Self::WindowsPath(path) => self.session_command(
                session_name,
                &[
                    "server",
                    "live-handoff",
                    "--import-exe",
                    path,
                    "--expected-protocol",
                    &protocol,
                    "--expected-version",
                    version,
                ],
            ),
        }
    }
}

#[derive(Debug, Clone)]
pub(super) struct RemoteHerdr {
    executable: RemoteExecutable,
    resolved_executable: Option<String>,
    platform: RemotePlatform,
    bridge_idle_timeout: bool,
}

impl RemoteHerdr {
    pub(super) fn machine_metadata(&self) -> Option<crate::client::endpoint::SshMachineMetadata> {
        let executable = self.resolved_executable.clone()?;
        let metadata = crate::client::endpoint::SshMachineMetadata {
            os: self.platform.os.to_owned(),
            executable,
        };
        metadata.is_valid().then_some(metadata)
    }

    fn for_platform(platform: RemotePlatform) -> Self {
        let executable = if platform.is_windows() {
            RemoteExecutable::WindowsPath("hicodeer.exe".to_string())
        } else {
            RemoteExecutable::PosixShellPath("\"$HOME/.local/bin/hicodeer\"".to_string())
        };
        Self {
            executable,
            resolved_executable: None,
            platform,
            bridge_idle_timeout: false,
        }
    }

    fn with_posix_path(mut self, path: &str) -> Self {
        self.executable = RemoteExecutable::PosixShellPath(shell_quote(path));
        self.resolved_executable = Some(path.to_owned());
        self
    }

    fn with_windows_path(mut self, path: String) -> Self {
        self.resolved_executable = Some(path.clone());
        self.executable = RemoteExecutable::WindowsPath(path);
        self
    }
}

fn windows_powershell_application_script(path: &str, args: &[&str]) -> String {
    let mut script = format!("& {}", crate::platform::quote_powershell_arg(path));
    for arg in args {
        script.push(' ');
        script.push_str(&crate::platform::quote_powershell_arg(arg));
    }
    script.push_str("; exit $LASTEXITCODE");
    script
}

fn windows_powershell_streaming_application_command(path: &str, args: &[&str]) -> String {
    windows_powershell_script_command(&windows_powershell_streaming_application_script(path, args))
}

fn windows_powershell_streaming_application_script(path: &str, args: &[&str]) -> String {
    let command_line = args
        .iter()
        .map(|arg| crate::platform::quote_windows_command_line_arg(arg))
        .collect::<Vec<_>>()
        .join(" ");
    // Start-Process -Wait waits for descendants, including a cold-started server.
    // Retain the handle so Windows PowerShell 5.1 keeps the application's exit code.
    format!(
        "$process = Start-Process -FilePath {} -ArgumentList {} -NoNewWindow -PassThru -ErrorAction Stop; $null = $process.Handle; $process.WaitForExit(); exit $process.ExitCode",
        crate::platform::quote_powershell_arg(path),
        crate::platform::quote_powershell_arg(&command_line),
    )
}

fn posix_remote_output_command(command: &str) -> String {
    format!("printf '\n%s\n' '{REMOTE_OUTPUT_READY_MARKER}'\n{command}")
}

fn windows_powershell_script_command(script: &str) -> String {
    // `$ProgressPreference = 'SilentlyContinue'` stops PowerShell from
    // serializing progress records (for example module auto-loading's
    // "Preparing modules for first use.") into the stderr CLIXML stream that
    // Herdr forwards to the terminal.
    let script = format!(
        "[Console]::Out.WriteLine(); [Console]::Out.WriteLine('{REMOTE_OUTPUT_READY_MARKER}'); [Console]::Out.Flush(); $ProgressPreference = 'SilentlyContinue'; {script}"
    );
    let utf16 = script
        .encode_utf16()
        .flat_map(u16::to_le_bytes)
        .collect::<Vec<_>>();
    let encoded = base64::engine::general_purpose::STANDARD.encode(utf16);
    format!("powershell.exe -NoLogo -NoProfile -NonInteractive -EncodedCommand {encoded}")
}










fn current_version() -> String {
    crate::build_info::version()
}




pub(super) struct PreparedRemoteHerdr {
    pub(super) remote_herdr: RemoteHerdr,
}

#[derive(Clone)]
pub(super) struct ManagedSshOptions {
    config_path: PathBuf,
    control_path: Option<PathBuf>,
    // Bridge workers may launch SSH after the helper that created this config
    // has gone away. The last options owner removes only the temporary config.
    _directory: Arc<ManagedSshConfigDirectory>,
}

struct ManagedSshConfig {
    options: ManagedSshOptions,
}

struct ManagedSshConfigDirectory(PathBuf);

impl Drop for ManagedSshConfigDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// Classify only SSH authentication diagnostics, not transport failures or
/// unknown/changed host keys. This does not imply permission to prompt.
pub(crate) fn ssh_error_requires_authentication(message: &str) -> bool {
    let message = message.to_ascii_lowercase();
    if message.contains("host key verification failed")
        || message.contains("remote host identification has changed")
    {
        return false;
    }
    (message.contains("permission denied")
        && ["(publickey", "(keyboard-interactive", "(password"]
            .iter()
            .any(|method| message.contains(method)))
        || (message.contains("signing failed")
            && (message.contains("sign_and_send_pubkey") || message.contains("agent")))
}

/// Keep this owner alive until the child has exited: OpenSSH reads its temporary
/// config after spawn. Dropping it never stops the shared authenticated master.
pub(crate) struct SshAuthenticationCommand {
    pub(crate) command: Command,
    _config: ManagedSshConfig,
}

pub(crate) fn ssh_authentication_command(target: &str) -> io::Result<SshAuthenticationCommand> {
    if target.is_empty() || target.starts_with('-') || target.chars().any(char::is_control) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid SSH target",
        ));
    }
    if !crate::platform::remote_ssh_config_paths().multiplexing {
        return Err(io::Error::new(io::ErrorKind::Unsupported, "interactive SSH recovery requires Unix OpenSSH multiplexing; authenticate outside HiCodeer on this platform"));
    }
    if !crate::config::Config::load()
        .config
        .remote
        .manage_ssh_config
    {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "interactive SSH recovery requires remote.manage_ssh_config=true",
        ));
    }
    let config = write_managed_ssh_config(target)?;
    Ok(authentication_command_with_config(target, config))
}

fn authentication_command_with_config(
    target: &str,
    config: ManagedSshConfig,
) -> SshAuthenticationCommand {
    let mut command = Command::new("ssh");
    apply_managed_ssh_options(&mut command, Some(&config.options));
    command
        .env("SSH_ASKPASS_REQUIRE", "never")
        .env_remove("SSH_ASKPASS")
        .arg("-o")
        .arg("BatchMode=no")
        .arg("-o")
        .arg("StrictHostKeyChecking=yes")
        .arg("-o")
        .arg("NumberOfPasswordPrompts=3")
        .arg("-T")
        .arg(target)
        .arg("exit");
    SshAuthenticationCommand {
        command,
        _config: config,
    }
}

pub(super) struct RemoteSsh {
    target: String,
    session_name: String,
    managed_config: Option<ManagedSshConfig>,
    noninteractive: bool,
}

impl RemoteSsh {
    fn new(target: String, manage_ssh_config: bool, session_name: String) -> Self {
        let managed_config = if manage_ssh_config {
            write_managed_ssh_config(&target)
                .inspect_err(|err| {
                    tracing::debug!(%err, "could not write managed ssh config; using plain ssh");
                })
                .ok()
        } else {
            None
        };

        Self {
            target,
            session_name,
            managed_config,
            noninteractive: false,
        }
    }

    pub(super) fn new_noninteractive(target: String) -> Self {
        let manage = crate::platform::remote_ssh_config_paths().multiplexing
            && crate::config::Config::load()
                .config
                .remote
                .manage_ssh_config;
        let mut ssh = Self::new(target, manage, crate::session::DEFAULT_SESSION_NAME.into());
        ssh.noninteractive = true;
        ssh
    }

    fn target(&self) -> &str {
        &self.target
    }

    fn destination(&self) -> String {
        format!("{} (session {})", self.target, self.session_name)
    }

    pub(super) fn options(&self) -> Option<&ManagedSshOptions> {
        self.managed_config.as_ref().map(|config| &config.options)
    }

    fn command(&self) -> Command {
        let mut command = self.base_command();
        if self.noninteractive {
            apply_noninteractive_ssh_options(&mut command);
        }
        command.arg("-T").arg(&self.target);
        command
    }

    fn base_command(&self) -> Command {
        let mut command = Command::new("ssh");
        apply_managed_ssh_options(&mut command, self.options());
        command
    }

    fn sh_output(&self, script: &str) -> io::Result<Output> {
        let script = posix_remote_output_command(script);
        let mut child = self
            .command()
            .arg("/bin/sh -s")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;

        if !self.noninteractive {
            return normalize_remote_output(output_with_forwarded_stderr(
                child,
                Some(script.as_bytes()),
            )?);
        }

        let write_result = if let Some(mut stdin) = child.stdin.take() {
            stdin.write_all(script.as_bytes())
        } else {
            Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "ssh bootstrap stdin missing",
            ))
        };
        let output = wait_with_output_timeout(child, NONINTERACTIVE_SSH_COMMAND_TIMEOUT)?;
        write_result?;
        normalize_remote_output(output)
    }

    fn framed_user_shell_output(&self, remote_command: &str) -> io::Result<Output> {
        let mut command = self.command();
        command
            // Windows OpenSSH can still read the console with stdin redirected to NUL.
            .arg("-n")
            .arg(remote_command)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let output = if self.noninteractive {
            wait_with_output_timeout(command.spawn()?, NONINTERACTIVE_SSH_COMMAND_TIMEOUT)
        } else {
            output_with_forwarded_stderr(command.spawn()?, None)
        }?;
        normalize_remote_output(output)
    }

    fn posix_user_shell_output(&self, remote_command: &str) -> io::Result<Output> {
        self.framed_user_shell_output(&posix_remote_output_command(remote_command))
    }

    fn shell_output(&self, platform: &RemotePlatform, remote_command: &str) -> io::Result<Output> {
        if platform.is_windows() {
            self.framed_user_shell_output(remote_command)
        } else {
            self.sh_output(remote_command)
        }
    }


}

// Only interactive setup uses this relay. Background probes retain their
// capture-only timeout path so SSH diagnostics cannot overwrite the active TUI.
fn output_with_forwarded_stderr(mut child: Child, stdin: Option<&[u8]>) -> io::Result<Output> {
    let mut child_stderr = child
        .stderr
        .take()
        .ok_or_else(|| io::Error::new(io::ErrorKind::BrokenPipe, "ssh command stderr missing"))?;
    let stderr_relay = thread::spawn(move || -> io::Result<Vec<u8>> {
        let mut captured = Vec::new();
        let mut buffer = [0_u8; 8 * 1024];
        let mut destination = io::stderr();

        loop {
            let read = child_stderr.read(&mut buffer)?;
            if read == 0 {
                break;
            }
            captured.extend_from_slice(&buffer[..read]);
            if destination.write_all(&buffer[..read]).is_ok() {
                let _ = destination.flush();
            }
        }

        Ok(captured)
    });

    let write_result = if let Some(bytes) = stdin {
        if let Some(mut child_stdin) = child.stdin.take() {
            child_stdin.write_all(bytes)
        } else {
            Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "ssh bootstrap stdin missing",
            ))
        }
    } else {
        Ok(())
    };
    let output_result = child.wait_with_output();
    let stderr_result = stderr_relay
        .join()
        .map_err(|_| io::Error::other("ssh stderr relay panicked"))?;

    let mut output = output_result?;
    write_result?;
    output.stderr = stderr_result?;
    Ok(output)
}

fn normalize_remote_output(mut output: Output) -> io::Result<Output> {
    normalize_remote_stdout(&mut output.stdout, output.status.success())?;
    Ok(output)
}

fn normalize_remote_stdout(stdout: &mut Vec<u8>, command_succeeded: bool) -> io::Result<()> {
    let consumed = {
        let mut reader = io::Cursor::new(stdout.as_slice());
        match discard_remote_output_preamble(&mut reader) {
            Ok(()) => reader.position() as usize,
            Err(_) if !command_succeeded => return Ok(()),
            Err(err) => return Err(err),
        }
    };
    stdout.drain(..consumed);
    Ok(())
}





fn decode_windows_remote_path(encoded: &str) -> io::Result<String> {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .map_err(|err| {
            io::Error::other(format!("remote path result is not valid base64: {err}"))
        })?;
    let path = String::from_utf8(bytes)
        .map_err(|err| io::Error::other(format!("remote path result is not valid UTF-8: {err}")))?;
    if path.is_empty() {
        return Err(io::Error::other("remote path result is empty"));
    }
    Ok(path)
}

fn apply_noninteractive_ssh_options(command: &mut Command) {
    command
        .arg("-o")
        .arg("BatchMode=yes")
        .arg("-o")
        .arg("NumberOfPasswordPrompts=0")
        .arg("-o")
        .arg("StrictHostKeyChecking=yes")
        .arg("-o")
        .arg("ConnectTimeout=10")
        .arg("-o")
        .arg("ConnectionAttempts=1")
        .arg("-o")
        .arg("ServerAliveInterval=15")
        .arg("-o")
        .arg("ServerAliveCountMax=4");
}

fn apply_managed_ssh_options(command: &mut Command, options: Option<&ManagedSshOptions>) {
    // Compress the first connection too: multiplexed bridges inherit the master's transport.
    command.arg("-C");
    let Some(options) = options else {
        return;
    };

    command.arg("-F").arg(&options.config_path);
    if let Some(control_path) = &options.control_path {
        // User ControlPaths may be shared across isolated Herdr configs (or
        // explicitly disabled). Managed auth must use our scoped transport;
        // never stop or unlink a master belonging to the user's SSH setup.
        command
            .arg("-S")
            .arg(control_path)
            .arg("-o")
            .arg("ControlMaster=auto")
            .arg("-o")
            .arg("ControlPersist=600");
    }
}


pub(super) fn prepare_remote_herdr(
    ssh: &RemoteSsh,
    require_surface_interest: bool,
) -> io::Result<PreparedRemoteHerdr> {
    let platform = detect_remote_platform(ssh)?.into_setup_platform();
    let remote_herdr = RemoteHerdr::for_platform(platform);
    let candidates = remote_binary_candidates(ssh, &remote_herdr)?;
    prepare_discovered_remote_herdr(ssh, remote_herdr, &candidates, require_surface_interest)
}

fn prepare_discovered_remote_herdr(
    ssh: &RemoteSsh,
    remote_herdr: RemoteHerdr,
    candidates: &[RemoteHerdr],
    require_surface_interest: bool,
) -> io::Result<PreparedRemoteHerdr> {
    // Automatic installation on the remote host was removed. Only a binary
    // that is already present and usable is accepted.
    for candidate in candidates {
        if remote_binary_supports_endpoint_requirement(ssh, candidate, require_surface_interest)
            .unwrap_or(false)
        {
            return Ok(PreparedRemoteHerdr {
                remote_herdr: candidate.clone(),
            });
        }
    }
    if remote_binary_supports_endpoint_requirement(ssh, &remote_herdr, require_surface_interest)? {
        return Ok(PreparedRemoteHerdr { remote_herdr });
    }
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        format!(
            "no usable HiCodeer on {}; install it on that machine manually, then reconnect",
            ssh.target()
        ),
    ))
}

pub(super) fn find_installed_remote_herdr(ssh: &RemoteSsh) -> io::Result<RemoteHerdr> {
    let platform = detect_remote_platform(ssh)?.platform;
    let remote_herdr = RemoteHerdr::for_platform(platform);
    let candidates = remote_binary_candidates(ssh, &remote_herdr)?;
    for mut candidate in candidates {
        if let Some(status) = remote_client_status(ssh, &candidate)? {
            if status.supports_endpoint_requirement(&candidate.platform, true) {
                candidate.bridge_idle_timeout = status.remote_bridge_idle_timeout;
                return Ok(candidate);
            }
        }
    }
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        format!(
            "matching HiCodeer is not ready on {}; install HiCodeer on that machine, then reconnect",
            ssh.target()
        ),
    ))
}


pub(super) fn discover_remote_api_metadata(
    ssh: &RemoteSsh,
    session: &str,
) -> io::Result<crate::client::endpoint::SshMachineMetadata> {
    let platform = detect_remote_platform(ssh)?.platform;
    if !platform.is_windows() {
        let output =
            ssh.framed_user_shell_output(&posix_remote_api_discovery_command(&platform, session))?;
        if !output.status.success() {
            return Err(command_failed("remote binary discovery failed", &output));
        }
        let metadata = crate::client::endpoint::SshMachineMetadata {
            os: platform.os.to_owned(),
            executable: String::from_utf8_lossy(&output.stdout).trim().to_owned(),
        };
        if !metadata.is_valid() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "invalid remote HiCodeer executable path",
            ));
        }
        return Ok(metadata);
    }
    let remote_herdr = RemoteHerdr::for_platform(platform);
    let candidates = remote_binary_candidates(ssh, &remote_herdr)?;
    for candidate in candidates {
        let probe =
            ssh.framed_user_shell_output(&remote_api_bridge_command(&candidate, session, true))?;
        if probe.status.code() == Some(255) {
            return Err(command_failed("remote SSH connection failed", &probe));
        }
        if probe.status.success()
            && String::from_utf8_lossy(&probe.stdout).trim() == "hicodeer-api-bridge-v1"
        {
            return Ok(crate::client::endpoint::SshMachineMetadata {
                os: "windows".into(),
                executable: candidate.executable.display().to_owned(),
            });
        }
    }
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "remote HiCodeer does not support machine API forwarding; update HiCodeer on this machine",
    ))
}

fn detect_remote_platform(ssh: &RemoteSsh) -> io::Result<DetectedRemotePlatform> {
    let output = ssh.sh_output("uname -s\nuname -m\n")?;
    let mut windows_uname_hint = false;
    let posix_error = if output.status.success() {
        let stdout = String::from_utf8_lossy(&output.stdout);
        let mut lines = stdout.lines();
        let os = lines.next().unwrap_or_default();
        let arch = lines.next().unwrap_or_default();
        if let Some(platform) = RemotePlatform::from_uname(os, arch) {
            return Ok(DetectedRemotePlatform {
                platform,
                setup_note: None,
            });
        }
        windows_uname_hint = looks_like_windows_uname(os);
        io::Error::other(format!(
            "unsupported remote platform: {} {}",
            os.trim(),
            arch.trim()
        ))
    } else {
        command_failed("remote platform detection failed", &output)
    };

    if !should_try_windows_probe(
        output.status.success(),
        output.status.code(),
        windows_uname_hint,
    ) {
        return Err(posix_error);
    }
    let windows_output = match ssh.framed_user_shell_output(&windows_platform_probe_command()) {
        Ok(output) if output.status.success() => output,
        _ => return Err(posix_error),
    };
    match parse_windows_platform_probe(&String::from_utf8_lossy(&windows_output.stdout)) {
        Ok(Some(platform)) => Ok(platform),
        Ok(None) => Err(posix_error),
        Err(message) => Err(io::Error::other(message)),
    }
}

fn should_try_windows_probe(
    success: bool,
    exit_code: Option<i32>,
    windows_uname_hint: bool,
) -> bool {
    (success && windows_uname_hint) || (!success && exit_code.is_some_and(|code| code != 255))
}

fn looks_like_windows_uname(os: &str) -> bool {
    let os = os.trim().to_ascii_uppercase();
    ["MINGW", "MSYS", "CYGWIN"]
        .iter()
        .any(|prefix| os.starts_with(prefix))
}

fn windows_platform_probe_command() -> String {
    windows_powershell_script_command(
        "$arch = if ($env:PROCESSOR_ARCHITEW6432) { $env:PROCESSOR_ARCHITEW6432 } else { $env:PROCESSOR_ARCHITECTURE }; [Console]::Out.WriteLine('hicodeer-windows:' + $arch); exit 0",
    )
}

fn parse_windows_platform_probe(stdout: &str) -> Result<Option<DetectedRemotePlatform>, String> {
    let Some(arch) = stdout
        .lines()
        .find_map(|line| line.trim().strip_prefix("hicodeer-windows:"))
    else {
        return Ok(None);
    };
    let setup_note = match arch.trim().to_ascii_uppercase().as_str() {
        "AMD64" | "X86_64" => None,
        "ARM64" => Some(
            "Windows ARM64 support is best-effort and uses x64 emulation; bugs and issues are expected.",
        ),
        arch => return Err(format!("unsupported remote platform: Windows {arch}")),
    };
    Ok(Some(DetectedRemotePlatform {
        platform: RemotePlatform {
            os: "windows",
            arch: "x86_64",
        },
        setup_note,
    }))
}

fn remote_binary_candidates(
    ssh: &RemoteSsh,
    remote_herdr: &RemoteHerdr,
) -> io::Result<Vec<RemoteHerdr>> {
    if remote_herdr.platform.is_windows() {
        let output = ssh.framed_user_shell_output(&windows_remote_binary_candidate_command())?;
        if !output.status.success() {
            return Err(command_failed("remote binary discovery failed", &output));
        }
        return Ok(windows_remote_binary_candidates(
            remote_herdr,
            &String::from_utf8_lossy(&output.stdout),
        ));
    }

    let mut candidates = Vec::new();

    if let Some(path_candidate) = remote_binary_on_path_any(ssh, remote_herdr)? {
        push_if_new_remote_binary_candidate(&mut candidates, path_candidate);
    }

    let output = ssh.sh_output(&known_remote_binary_candidate_script(
        &remote_herdr.platform,
    ))?;
    if !output.status.success() {
        return Err(command_failed("remote binary discovery failed", &output));
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    for candidate in remote_herdrs_from_path_discovery(remote_herdr, &stdout) {
        push_if_new_remote_binary_candidate(&mut candidates, candidate);
    }

    Ok(candidates)
}

fn windows_remote_binary_candidate_command() -> String {
    windows_powershell_script_command(&format!(
        r#"function Emit-HerdrPath([string]$CandidatePath) {{ if ([string]::IsNullOrWhiteSpace($CandidatePath) -or -not (Test-Path -LiteralPath $CandidatePath -PathType Leaf)) {{ return }}; $candidateFullPath = [System.IO.Path]::GetFullPath($CandidatePath); $encodedCandidate = [System.Convert]::ToBase64String([System.Text.Encoding]::UTF8.GetBytes($candidateFullPath)); [Console]::Out.WriteLine('{WINDOWS_REMOTE_PATH_MARKER}' + $encodedCandidate) }}; $pathCommand = Get-Command hicodeer.exe -CommandType Application -ErrorAction SilentlyContinue | Select-Object -First 1; if ($null -ne $pathCommand) {{ Emit-HerdrPath $pathCommand.Source }}; $userSid = [System.Security.Principal.WindowsIdentity]::GetCurrent().User.Value; Get-CimInstance Win32_Process -Filter "Name = 'hicodeer.exe' OR Name = 'hicodeer-dev.exe'" -ErrorAction SilentlyContinue | ForEach-Object {{ $owner = Invoke-CimMethod -InputObject $_ -MethodName GetOwnerSid -ErrorAction SilentlyContinue; if ($owner.ReturnValue -eq 0 -and $owner.Sid -eq $userSid) {{ Emit-HerdrPath $_.ExecutablePath }} }}; $herdrHome = if ([string]::IsNullOrWhiteSpace($env:HICODEER_HOME)) {{ Join-Path $env:USERPROFILE '.hicodeer' }} else {{ $env:HICODEER_HOME }}; $activeJunction = Get-Item -LiteralPath (Join-Path $herdrHome 'packages\standalone\current') -Force -ErrorAction SilentlyContinue; if ($null -ne $activeJunction -and -not [string]::IsNullOrWhiteSpace([string]$activeJunction.Target)) {{ Emit-HerdrPath (Join-Path ([string]$activeJunction.Target) 'hicodeer.exe') }}; exit 0"#
    ))
}

fn windows_remote_binary_candidates(remote_herdr: &RemoteHerdr, stdout: &str) -> Vec<RemoteHerdr> {
    let mut candidates = Vec::new();
    for path in stdout.lines().filter_map(|line| {
        line.trim()
            .strip_prefix(WINDOWS_REMOTE_PATH_MARKER)
            .and_then(|encoded| decode_windows_remote_path(encoded).ok())
    }) {
        push_if_new_remote_binary_candidate(
            &mut candidates,
            remote_herdr.clone().with_windows_path(path),
        );
    }
    candidates
}

fn push_if_new_remote_binary_candidate(candidates: &mut Vec<RemoteHerdr>, candidate: RemoteHerdr) {
    if !candidates
        .iter()
        .any(|existing| existing.executable == candidate.executable)
    {
        candidates.push(candidate);
    }
}

fn known_remote_binary_candidate_script(platform: &RemotePlatform) -> String {
    let mut script = String::from(
        r#"home=${HOME:-}
user=${USER:-}"#,
    );
    script.push_str(
        r#"
emit() {
    path=$1
    if [ -n "$path" ] && [ -x "$path" ]; then
        printf '%s\n' "$path"
    fi
}
if [ -n "$home" ]; then
    emit "$home/.local/bin/hicodeer"
fi
"#,
    );
    if platform.os == "macos" {
        script.push_str(
            r#"    emit "/opt/homebrew/bin/hicodeer"
"#,
        );
    } else if platform.os == "linux" {
        script.push_str(
            r#"    emit "/home/linuxbrew/.linuxbrew/bin/hicodeer"
"#,
        );
    }
    script.push_str(
        r#"emit "/usr/local/bin/hicodeer"
if [ -n "$home" ]; then
    emit "$home/.nix-profile/bin/hicodeer"
fi
if [ -n "$user" ]; then
    emit "/etc/profiles/per-user/$user/bin/hicodeer"
fi
emit "/nix/var/nix/profiles/default/bin/hicodeer"
emit "/run/current-system/sw/bin/hicodeer"
"#,
    );

    script
}

fn remote_binary_on_path_any(
    ssh: &RemoteSsh,
    remote_herdr: &RemoteHerdr,
) -> io::Result<Option<RemoteHerdr>> {
    let output = ssh.posix_user_shell_output("command -v hicodeer")?;
    if output.status.success() {
        let stdout = String::from_utf8_lossy(&output.stdout);
        if let Some(candidate) = remote_herdr_from_path_discovery(remote_herdr, &stdout) {
            return Ok(Some(candidate));
        }
    }

    // Non-POSIX login shells such as xonsh reject `command -v`; retry through
    // /bin/sh while retaining the login-shell probe for shell-initialized PATHs.
    let output = ssh.sh_output("command -v hicodeer\n")?;
    if !output.status.success() {
        return Ok(None);
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    Ok(remote_herdr_from_path_discovery(remote_herdr, &stdout))
}

fn remote_herdrs_from_path_discovery(remote_herdr: &RemoteHerdr, stdout: &str) -> Vec<RemoteHerdr> {
    stdout
        .lines()
        .filter_map(|path| remote_herdr_from_path(remote_herdr, path))
        .collect()
}

fn remote_herdr_from_path_discovery(
    remote_herdr: &RemoteHerdr,
    stdout: &str,
) -> Option<RemoteHerdr> {
    stdout
        .lines()
        .find_map(|path| remote_herdr_from_path(remote_herdr, path))
}

fn remote_herdr_from_path(remote_herdr: &RemoteHerdr, path: &str) -> Option<RemoteHerdr> {
    let path = path.trim();
    if !path.starts_with('/') {
        return None;
    }
    if is_mise_shim_path(path) {
        return None;
    }
    Some(remote_herdr.clone().with_posix_path(path))
}

fn is_mise_shim_path(path: &str) -> bool {
    path.ends_with("/mise/shims/hicodeer")
}

fn remote_client_status(
    ssh: &RemoteSsh,
    remote_herdr: &RemoteHerdr,
) -> io::Result<Option<RemoteClientStatusJson>> {
    let command = remote_herdr.executable.status_client_command();
    let output = ssh.shell_output(&remote_herdr.platform, &command)?;
    if !output.status.success() {
        if output.status.code() == Some(255) {
            return Err(command_failed("remote SSH connection failed", &output));
        }
        return Ok(None);
    }
    Ok(parse_client_status_json(&String::from_utf8_lossy(
        &output.stdout,
    )))
}

fn remote_binary_supports_endpoint_requirement(
    ssh: &RemoteSsh,
    remote_herdr: &RemoteHerdr,
    require_surface_interest: bool,
) -> io::Result<bool> {
    Ok(
        remote_client_status(ssh, remote_herdr)?.is_some_and(|status| {
            status.supports_endpoint_requirement(&remote_herdr.platform, require_surface_interest)
        }),
    )
}







#[derive(Debug, Clone, PartialEq, Eq)]
enum RemoteServerStatus {
    Running {
        version: Option<String>,
        endpoint_protocol_generation: Option<u32>,
        surface_interest: bool,
        health_check: bool,
        live_handoff: bool,
        detached_server_daemon: bool,
    },
    NotRunning,
}

impl RemoteServerStatus {
    fn with_endpoint_negotiation(
        mut self,
        negotiation: &crate::client::endpoint::EndpointNegotiation,
    ) -> Self {
        if let Self::Running {
            surface_interest,
            health_check,
            ..
        } = &mut self
        {
            *surface_interest = negotiation.supports_surface_interest();
            *health_check = negotiation.supports_health_check();
        }
        self
    }
}

fn ensure_remote_server_ready(
    ssh: &RemoteSsh,
    remote_herdr: &RemoteHerdr,
    live_handoff_enabled: bool,
    require_surface_interest: bool,
) -> io::Result<()> {
    let status = remote_server_status(ssh, remote_herdr, require_surface_interest)?;
    let RemoteServerStatus::Running {
        version,
        endpoint_protocol_generation,
        surface_interest,
        health_check,
        live_handoff,
        detached_server_daemon,
    } = status
    else {
        return Ok(());
    };

    let Some(reason) = remote_server_restart_reason(
        endpoint_protocol_generation,
        detached_server_daemon,
        require_surface_interest,
        surface_interest,
        health_check,
    ) else {
        return Ok(());
    };

    if live_handoff_enabled && live_handoff {
        match live_handoff_remote_server(ssh, remote_herdr) {
            Ok(()) => return Ok(()),
            Err(err) => {
                eprintln!("remote live handoff failed: {err}");
                eprintln!("falling back to remote server restart.");
            }
        }
    }

    if confirm_remote_server_stop(&ssh.destination(), version.as_deref(), reason)? {
        stop_remote_server(ssh, remote_herdr)?;
    }
    Ok(())
}


fn remote_server_status(
    ssh: &RemoteSsh,
    remote_herdr: &RemoteHerdr,
    require_surface_interest: bool,
) -> io::Result<RemoteServerStatus> {
    let command = remote_herdr
        .executable
        .session_command(&ssh.session_name, &["status", "server", "--json"]);
    let output = ssh.shell_output(&remote_herdr.platform, &command)?;
    if !output.status.success() {
        return Err(command_failed("remote server status failed", &output));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let status = parse_remote_server_status_json(stdout.trim())?;
    if require_surface_interest
        && matches!(
            status,
            RemoteServerStatus::Running {
                endpoint_protocol_generation: Some(
                    crate::protocol::endpoint::ENDPOINT_PROTOCOL_GENERATION
                ),
                surface_interest: true,
                health_check: true,
                ..
            }
        )
    {
        // Older status helpers omit newer capabilities. Ask the live endpoint rather than
        // assuming that the installed binary and the running daemon support the same features.
        let negotiation = probe_remote_endpoint(ssh, remote_herdr)?;
        return Ok(status.with_endpoint_negotiation(&negotiation));
    }
    Ok(status)
}

fn probe_remote_endpoint(
    ssh: &RemoteSsh,
    remote_herdr: &RemoteHerdr,
) -> io::Result<crate::client::endpoint::EndpointNegotiation> {
    let path = local_forward_socket_path(ssh.target(), &ssh.session_name);
    let bridge = SshStdioBridge::start(
        ssh.target.clone(),
        remote_herdr.clone(),
        path.clone(),
        ssh.session_name.clone(),
        ssh.options(),
        true,
    )?;
    let mut stream = crate::ipc::connect_local_stream(&path)?;
    // Use the saved client's noninteractive path. This metadata-only attachment never
    // acquires a surface or sends pane input.
    match crate::client::probe_endpoint_negotiation(&mut stream) {
        Ok(negotiation) => Ok(negotiation),
        Err(probe_error) => Err(bridge.reported_failure().unwrap_or(probe_error)),
    }
}

#[derive(Debug, Deserialize)]
struct RemoteClientStatusJson {
    #[serde(default)]
    version: Option<String>,
    #[serde(default)]
    protocol: Option<u32>,
    #[serde(default)]
    endpoint_protocol_generation: Option<u32>,
    #[serde(default)]
    endpoint_capabilities: Vec<String>,
    #[serde(default)]
    remote_host_bridge: bool,
    #[serde(default)]
    remote_bridge_idle_timeout: bool,
}

impl RemoteClientStatusJson {
    fn supports_endpoint_requirement(
        &self,
        platform: &RemotePlatform,
        require_surface_interest: bool,
    ) -> bool {
        self.endpoint_protocol_generation
            == Some(crate::protocol::endpoint::ENDPOINT_PROTOCOL_GENERATION)
            && (!platform.is_windows() || self.remote_host_bridge)
            && (!require_surface_interest
                || [
                    crate::protocol::endpoint::SURFACE_INTEREST_CAPABILITY,
                    crate::protocol::endpoint::PRESENTATION_EFFECTS_FENCE_CAPABILITY,
                    crate::protocol::endpoint::HEALTH_CHECK_CAPABILITY,
                ]
                .iter()
                .all(|required| {
                    self.endpoint_capabilities
                        .iter()
                        .any(|capability| capability == required)
                }))
    }
}

#[derive(Debug, Deserialize)]
struct RemoteServerStatusJson {
    running: bool,
    version: Option<String>,
    capabilities: Option<RemoteServerCapabilitiesJson>,
}

#[derive(Debug, Deserialize)]
struct RemoteServerCapabilitiesJson {
    live_handoff: bool,
    #[serde(default)]
    detached_server_daemon: bool,
    #[serde(default)]
    endpoint_protocol_generation: Option<u32>,
    #[serde(default)]
    surface_interest: bool,
    #[serde(default)]
    health_check: bool,
}

fn parse_client_status_json(status: &str) -> Option<RemoteClientStatusJson> {
    status
        .lines()
        .rev()
        .filter(|line| !line.trim().is_empty())
        .filter_map(|line| serde_json::from_str::<RemoteClientStatusJson>(line).ok())
        .find(|status| {
            status.version.is_some()
                || status.protocol.is_some()
                || status.endpoint_protocol_generation.is_some()
                || !status.endpoint_capabilities.is_empty()
        })
}

fn parse_remote_server_status_json(status: &str) -> io::Result<RemoteServerStatus> {
    let parsed: RemoteServerStatusJson = serde_json::from_str(status).map_err(|err| {
        io::Error::other(format!(
            "could not parse remote server status JSON from `{status}`: {err}"
        ))
    })?;
    if !parsed.running {
        return Ok(RemoteServerStatus::NotRunning);
    }

    let capabilities = parsed.capabilities;

    Ok(RemoteServerStatus::Running {
        version: parsed.version,
        endpoint_protocol_generation: capabilities
            .as_ref()
            .and_then(|capabilities| capabilities.endpoint_protocol_generation),
        surface_interest: capabilities
            .as_ref()
            .is_some_and(|capabilities| capabilities.surface_interest),
        health_check: capabilities
            .as_ref()
            .is_some_and(|capabilities| capabilities.health_check),
        live_handoff: capabilities
            .as_ref()
            .is_some_and(|capabilities| capabilities.live_handoff),
        detached_server_daemon: capabilities
            .as_ref()
            .is_some_and(|capabilities| capabilities.detached_server_daemon),
    })
}

fn confirm_remote_server_stop(
    target: &str,
    version: Option<&str>,
    reason: RemoteServerRestartReason,
) -> io::Result<bool> {
    let required_upgrade = matches!(
        reason,
        RemoteServerRestartReason::EndpointProtocol
            | RemoteServerRestartReason::SurfaceInterest
            | RemoteServerRestartReason::HealthCheck
    );
    if !io::stdin().is_terminal() {
        if required_upgrade {
            return Err(io::Error::other(format!(
                "remote hicodeerd server on {target} needs one final update before this client can attach; run from an interactive terminal to approve updating it"
            )));
        }

        eprintln!(
            "remote hicodeerd server on {target} is still running v{}; it will use {} after it restarts.",
            version_label(version),
            current_version()
        );
        return Ok(false);
    }

    eprintln!("remote hicodeerd server on {target} is currently running:");
    eprintln!("  server: v{}", version_label(version));
    eprintln!("  prepared binary: {}", current_version());
    eprintln!();

    match reason {
        RemoteServerRestartReason::EndpointProtocol => {
            eprintln!(
                "the remote server predates HiCodeer's stable endpoint protocol and must update before this client can attach."
            );
        }
        RemoteServerRestartReason::SurfaceInterest => {
            eprintln!(
                "the remote server must restart before it can join saved SSH endpoint federation."
            );
        }
        RemoteServerRestartReason::HealthCheck => {
            eprintln!("the remote server must restart to enable saved SSH endpoint health checks.");
        }
        RemoteServerRestartReason::DaemonDetach => {
            eprintln!(
                "the remote server was started by a HiCodeer build that may not survive SSH connection loss. restart it so network drops disconnect only this client."
            );
        }
    }

    eprintln!("This stops active remote pane processes, including shells, agents, dev servers, and tests.");
    let prompt = if required_upgrade {
        "stop and update the remote server, then continue attaching? [y/N] "
    } else {
        "restart the remote server now? [y/N] "
    };
    eprint!("{prompt}");
    io::stderr().flush()?;

    if read_remote_confirmation(&mut io::stdin().lock(), false)? {
        return Ok(true);
    }
    if required_upgrade {
        return Err(io::Error::new(
            io::ErrorKind::Interrupted,
            "remote hicodeerd server stop cancelled",
        ));
    }

    Ok(false)
}

fn live_handoff_remote_server(ssh: &RemoteSsh, remote_herdr: &RemoteHerdr) -> io::Result<()> {
    let status = remote_client_status(ssh, remote_herdr)?.ok_or_else(|| {
        io::Error::other("could not inspect the prepared remote HiCodeer binary before live handoff")
    })?;
    let protocol = status.protocol.ok_or_else(|| {
        io::Error::other("prepared remote HiCodeer did not report its private protocol")
    })?;
    let version = status
        .version
        .filter(|version| !version.is_empty())
        .ok_or_else(|| io::Error::other("prepared remote HiCodeer did not report its version"))?;
    let command =
        remote_herdr
            .executable
            .live_handoff_command(&ssh.session_name, protocol, &version);
    let output = ssh.shell_output(&remote_herdr.platform, &command)?;
    if !output.status.success() {
        return Err(command_failed("remote server live handoff failed", &output));
    }

    eprintln!(
        "handed off the remote hicodeerd server on {}; reconnecting to the prepared server.",
        ssh.target()
    );
    Ok(())
}

fn stop_remote_server(ssh: &RemoteSsh, remote_herdr: &RemoteHerdr) -> io::Result<()> {
    let command = remote_herdr
        .executable
        .session_command(&ssh.session_name, &["server", "stop"]);
    let output = ssh.shell_output(&remote_herdr.platform, &command)?;
    if !output.status.success() {
        return Err(command_failed("remote server stop failed", &output));
    }

    wait_for_remote_server_shutdown(ssh, remote_herdr)?;
    eprintln!(
        "stopped the remote hicodeerd server on {}; it will restart when the remote client bridge attaches.",
        ssh.target()
    );
    Ok(())
}

fn wait_for_remote_server_shutdown(ssh: &RemoteSsh, remote_herdr: &RemoteHerdr) -> io::Result<()> {
    let deadline = Instant::now() + REMOTE_SERVER_SHUTDOWN_CONFIRM_TIMEOUT;
    loop {
        if remote_server_status(ssh, remote_herdr, false)? == RemoteServerStatus::NotRunning {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                format!(
                    "shutdown was requested, but the old remote hicodeerd server on {target} is still responding after {} seconds",
                    REMOTE_SERVER_SHUTDOWN_CONFIRM_TIMEOUT.as_secs(),
                    target = ssh.target()
                ),
            ));
        }
        thread::sleep(REMOTE_SERVER_SHUTDOWN_POLL_INTERVAL);
    }
}

fn version_label(version: Option<&str>) -> &str {
    version.unwrap_or("unknown")
}









fn read_remote_confirmation(reader: &mut impl io::BufRead, default: bool) -> io::Result<bool> {
    let mut answer = String::new();
    if reader.read_line(&mut answer)? == 0 {
        return Err(io::Error::new(
            io::ErrorKind::Interrupted,
            "remote setup cancelled",
        ));
    }
    match answer.trim().to_ascii_lowercase().as_str() {
        "y" | "yes" => Ok(true),
        "n" | "no" => Ok(false),
        "" => Ok(default),
        _ => Err(io::Error::new(
            io::ErrorKind::Interrupted,
            "remote setup cancelled: expected yes or no",
        )),
    }
}


fn posix_remote_api_discovery_command(platform: &RemotePlatform, session: &str) -> String {
    let script = format!(
        r#"set -f
candidates=$(
command -v hicodeer
{discovery}
)
IFS='
'
for candidate in $candidates; do
    case "$candidate" in
        */mise/shims/hicodeer) continue ;;
        /*) ;;
        *) continue ;;
    esac
    [ -x "$candidate" ] || continue
    if capability=$("$candidate" --session {session} remote-api-bridge --check </dev/null 2>/dev/null) && [ "$capability" = hicodeer-api-bridge-v1 ]; then
        printf '%s\n' "$candidate"
        exit 0
    fi
done
printf '%s\n' 'remote HiCodeer does not support machine API forwarding; update HiCodeer on this machine' >&2
exit 2"#,
        discovery = known_remote_binary_candidate_script(platform),
        session = shell_quote(session),
    );
    format!(
        "/bin/sh -c {}",
        shell_quote(&posix_remote_output_command(&script))
    )
}

pub(super) const STALE_API_METADATA: &str = "hicodeer-machine-metadata-stale-v1";

pub(super) fn cached_remote_api_command(
    metadata: &crate::client::endpoint::SshMachineMetadata,
    session: &str,
) -> String {
    if metadata.os == "windows" {
        let path = crate::platform::quote_powershell_arg(&metadata.executable);
        let session_arg = crate::platform::quote_powershell_arg(session);
        let probe = format!(
            "$capability = & {path} --session {session_arg} remote-api-bridge --check 2>$null; if ($LASTEXITCODE -ne 0 -or $capability -ne 'hicodeer-api-bridge-v1') {{ [Console]::Error.WriteLine('{STALE_API_METADATA}'); exit 78 }}; "
        );
        return windows_powershell_script_command(&format!(
            "{probe}{}",
            windows_powershell_streaming_application_script(
                &metadata.executable,
                &["--session", session, "remote-api-bridge"]
            ),
        ));
    }
    let path = shell_quote(&metadata.executable);
    let session = shell_quote(session);
    let script = format!(
        "if capability=$({path} --session {session} remote-api-bridge --check </dev/null 2>/dev/null) && [ \"$capability\" = hicodeer-api-bridge-v1 ]; then\n{}\nelse\n    printf '%s\\n' '{STALE_API_METADATA}' >&2\n    exit 78\nfi",
        posix_remote_output_command(&format!("exec {path} --session {session} remote-api-bridge")),
    );
    format!("/bin/sh -c {}", shell_quote(&script))
}

pub(super) fn remote_api_bridge_command(
    remote_herdr: &RemoteHerdr,
    session_name: &str,
    check: bool,
) -> String {
    let mut args = vec!["--session", session_name, "remote-api-bridge"];
    if check {
        args.push("--check");
    }
    match &remote_herdr.executable {
        RemoteExecutable::PosixShellPath(_) => {
            posix_remote_output_command(&format!("exec {}", remote_herdr.executable.command(&args)))
        }
        RemoteExecutable::WindowsPath(path) => {
            windows_powershell_streaming_application_command(path, &args)
        }
    }
}
fn reattach_command(
    program: &str,
    target: &str,
    session_name: &str,
    keybindings: RemoteKeybindings,
    live_handoff: bool,
) -> String {
    let program = crate::platform::remote_reattach_program(program);
    let target = crate::platform::remote_reattach_argument(target);
    let mut command = format!("{program} --remote {target}");
    if keybindings != RemoteKeybindings::Local {
        command.push_str(" --remote-keybindings ");
        command.push_str(keybindings.as_str());
    }
    if live_handoff {
        command.push_str(" --handoff");
    }
    if session_name != crate::session::DEFAULT_SESSION_NAME {
        command.push_str(" --session ");
        command.push_str(&crate::platform::remote_reattach_argument(session_name));
    }
    command
}

fn command_failed(context: &str, output: &Output) -> io::Error {
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stderr = stderr.trim();
    if stderr.is_empty() {
        io::Error::other(format!("{context}: {}", output.status))
    } else {
        io::Error::other(format!("{context}: {stderr}"))
    }
}

pub(super) struct SshStdioBridge {
    local_socket: PathBuf,
    socket_identity: crate::ipc::SocketFileIdentity,
    should_stop: Arc<AtomicBool>,
    failure_rx: mpsc::Receiver<io::Error>,
    thread: Option<JoinHandle<()>>,
}

impl SshStdioBridge {
    pub(super) fn start(
        target: String,
        remote_herdr: RemoteHerdr,
        local_socket: PathBuf,
        session_name: String,
        ssh_options: Option<&ManagedSshOptions>,
        noninteractive: bool,
    ) -> io::Result<Self> {
        Self::start_command(
            target,
            if noninteractive && remote_herdr.bridge_idle_timeout {
                remote_herdr
                    .executable
                    .bridge_command_with_idle_timeout(&session_name, true)
            } else {
                remote_herdr.executable.bridge_command(&session_name)
            },
            local_socket,
            ssh_options,
            noninteractive,
        )
    }

    pub(super) fn start_command(
        target: String,
        remote_command: String,
        local_socket: PathBuf,
        ssh_options: Option<&ManagedSshOptions>,
        noninteractive: bool,
    ) -> io::Result<Self> {
        crate::ipc::prepare_socket_path(&local_socket, |path| {
            format!("remote bridge is already listening at {}", path.display())
        })?;
        let listener = crate::ipc::bind_private_local_listener(&local_socket)?;
        let socket_identity = crate::ipc::socket_file_identity(&local_socket)?;
        if let Err(err) =
            crate::ipc::restrict_socket_permissions(&local_socket, BRIDGE_SOCKET_PERMISSION_MODE)
        {
            let _ = crate::ipc::remove_socket_file_if_owned(&local_socket, &socket_identity);
            return Err(err);
        }
        if let Err(err) = listener.set_nonblocking(ListenerNonblockingMode::Accept) {
            let _ = crate::ipc::remove_socket_file_if_owned(&local_socket, &socket_identity);
            return Err(err);
        }

        let should_stop = Arc::new(AtomicBool::new(false));
        let thread_stop = Arc::clone(&should_stop);
        let thread_ssh_options = ssh_options.cloned();
        let (failure_tx, failure_rx) = mpsc::sync_channel(1);
        let thread = thread::spawn(move || {
            while !thread_stop.load(Ordering::Acquire) {
                match listener.accept() {
                    Ok(stream) => {
                        let stream = match prepare_remote_bridge_stream(stream) {
                            Ok(stream) => stream,
                            Err(err) => {
                                tracing::error!(
                                    error = %err,
                                    "remote bridge failed to prepare client socket"
                                );
                                continue;
                            }
                        };
                        if let Err(err) = bridge_connection(
                            stream,
                            &target,
                            &remote_command,
                            thread_ssh_options.as_ref(),
                            noninteractive,
                            &thread_stop,
                        ) {
                            let _ =
                                failure_tx.try_send(io::Error::new(err.kind(), err.to_string()));
                            if noninteractive {
                                tracing::warn!(error = %err, "saved SSH endpoint bridge failed");
                            } else {
                                eprintln!("hicodeer: remote bridge failed: {err}");
                            }
                        }
                    }
                    Err(err) if err.kind() == io::ErrorKind::WouldBlock => {
                        thread::sleep(BRIDGE_ACCEPT_POLL);
                    }
                    Err(err) => {
                        if noninteractive {
                            tracing::warn!(error = %err, "saved SSH endpoint listener failed");
                        } else {
                            eprintln!("hicodeer: remote bridge listener failed: {err}");
                        }
                        break;
                    }
                }
            }
        });

        Ok(Self {
            local_socket,
            socket_identity,
            should_stop,
            failure_rx,
            thread: Some(thread),
        })
    }

    pub(super) fn reported_failure(&self) -> Option<io::Error> {
        self.failure_rx
            .recv_timeout(BRIDGE_FAILURE_REPORT_TIMEOUT)
            .ok()
    }
}

fn prepare_remote_bridge_stream(
    mut stream: crate::ipc::LocalStream,
) -> io::Result<crate::ipc::LocalStream> {
    crate::ipc::set_local_stream_polling(&mut stream, false)?;
    Ok(stream)
}

impl Drop for SshStdioBridge {
    fn drop(&mut self) {
        self.should_stop.store(true, Ordering::Release);
        #[cfg(unix)]
        let _ = crate::ipc::remove_socket_file_if_owned(&self.local_socket, &self.socket_identity);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        #[cfg(windows)]
        let _ = crate::ipc::remove_socket_file_if_owned(&self.local_socket, &self.socket_identity);
    }
}

fn ssh_config_quote(path: &str) -> String {
    format!("\"{path}\"")
}

fn ssh_config_include_path(path: &Path) -> String {
    let path = path.to_string_lossy();
    if std::path::MAIN_SEPARATOR == '\\' {
        ssh_config_quote(&path.replace('\\', "/"))
    } else {
        ssh_config_quote(&path)
    }
}

/// Returns the `Include` value for the user's SSH config, or `None` when there
/// is nothing useful to include.
///
/// Git for Windows' OpenSSH (MSYS) does not resolve Windows drive-letter paths
/// inside `Include`, so an absolute `C:/.../.ssh/config` path is silently
/// ignored when herdr runs under Git Bash and host aliases stop resolving.
/// `~/.ssh/config` is expanded by both Windows OpenSSH (to the user profile)
/// and MSYS OpenSSH (through `HOME`), so each shell's `ssh` reads the same user
/// config it would read by default. A missing config is harmless because
/// OpenSSH ignores an `Include` that matches nothing.
#[cfg(windows)]
fn ssh_user_config_include(_path: Option<&Path>) -> Option<String> {
    Some(ssh_config_quote("~/.ssh/config"))
}

#[cfg(not(windows))]
fn ssh_user_config_include(path: Option<&Path>) -> Option<String> {
    path.filter(|path| path.is_file())
        .map(ssh_config_include_path)
}

/// Builds a temporary ssh config that includes the user's settings first, so
/// OpenSSH's first-value-wins behavior preserves explicit user keepalives.
fn write_managed_ssh_config(target: &str) -> io::Result<ManagedSshConfig> {
    let paths = crate::platform::remote_ssh_config_paths();
    let control_path = if paths.multiplexing {
        Some(crate::platform::shared_ssh_control_path(
            &crate::config::config_path(),
            target,
        )?)
    } else {
        None
    };

    let dir = crate::platform::create_remote_ssh_config_dir(SSH_CONTROL_SOCKET_NAME)?;
    let path = dir.join("config");
    let mut contents = String::new();
    if let Some(include) = ssh_user_config_include(paths.user_config.as_deref()) {
        contents.push_str(&format!("Include {include}\n"));
    }
    if let Some(system_config) = paths.system_config.filter(|path| path.is_file()) {
        contents.push_str(&format!(
            "Include {}\n",
            ssh_config_include_path(&system_config)
        ));
    }
    contents.push_str("Host *\n");
    contents.push_str("  ServerAliveInterval 15\n");
    contents.push_str("  ServerAliveCountMax 4\n");

    let write_result = (|| {
        let mut file = crate::platform::create_remote_ssh_config_file(&path)?;
        file.write_all(contents.as_bytes())
    })();
    if let Err(err) = write_result {
        let _ = fs::remove_dir_all(&dir);
        return Err(err);
    }
    Ok(ManagedSshConfig {
        options: ManagedSshOptions {
            config_path: path,
            control_path,
            _directory: Arc::new(ManagedSshConfigDirectory(dir)),
        },
    })
}

struct BridgeUploadStop {
    stopped: AtomicBool,
    wake: crate::platform::RemoteBridgeWake,
}

impl BridgeUploadStop {
    fn new() -> io::Result<Self> {
        Ok(Self {
            stopped: AtomicBool::new(false),
            wake: crate::platform::RemoteBridgeWake::new()?,
        })
    }

    fn cancel(&self) {
        if !self.stopped.swap(true, Ordering::AcqRel) {
            if let Err(error) = self.wake.cancel() {
                tracing::debug!(%error, "remote bridge read cancellation failed");
            }
        }
    }

    fn is_stopped(&self) -> bool {
        self.stopped.load(Ordering::Acquire)
    }
}

#[cfg(all(test, unix))]
pub(crate) fn bridge_upload_cancellation_for_test(
    stream: crate::ipc::LocalStream,
    mut writer: impl io::Write + Send + 'static,
) -> impl FnOnce() {
    stream.set_nonblocking(true).unwrap();
    let stop = Arc::new(BridgeUploadStop::new().unwrap());
    let worker_stop = Arc::clone(&stop);
    let (done_tx, done_rx) = std::sync::mpsc::channel();
    let worker = thread::spawn(move || {
        let closed = AtomicBool::new(false);
        let result = copy_local_stream_to_writer(
            stream,
            &mut writer,
            &worker_stop,
            &AtomicBool::new(false),
            &closed,
        );
        done_tx
            .send((result, closed.load(Ordering::Acquire)))
            .unwrap();
    });
    move || {
        stop.cancel();
        let (result, closed) = done_rx.recv_timeout(Duration::from_secs(3)).unwrap();
        worker.join().unwrap();
        result.unwrap();
        assert!(!closed, "upload cancellation must not report peer EOF");
    }
}

fn bridge_connection(
    mut stream: crate::ipc::LocalStream,
    target: &str,
    remote_command: &str,
    ssh_options: Option<&ManagedSshOptions>,
    noninteractive: bool,
    bridge_stop: &Arc<AtomicBool>,
) -> io::Result<()> {
    let upload_stop = Arc::new(BridgeUploadStop::new()?);
    let mut command = Command::new("ssh");
    apply_managed_ssh_options(&mut command, ssh_options);
    if noninteractive {
        apply_noninteractive_ssh_options(&mut command);
    }
    command
        .arg("-T")
        .arg(target)
        .arg(remote_command)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(if noninteractive {
            Stdio::piped()
        } else {
            Stdio::inherit()
        });

    let mut child = command
        .spawn()
        .map_err(|err| io::Error::new(err.kind(), format!("failed to start ssh bridge: {err}")))?;
    let mut child_stdin = match child.stdin.take() {
        Some(stdin) => stdin,
        None => return terminate_bridge_child(child, "ssh bridge stdin missing"),
    };
    let child_stdout = match child.stdout.take() {
        Some(stdout) => stdout,
        None => return terminate_bridge_child(child, "ssh bridge stdout missing"),
    };
    let stderr_reader = if noninteractive {
        let Some(child_stderr) = child.stderr.take() else {
            return terminate_bridge_child(child, "ssh bridge stderr missing");
        };
        Some(thread::spawn(move || capture_ssh_stderr(child_stderr)))
    } else {
        None
    };
    let stream_to_child = match stream.try_clone() {
        Ok(stream) => stream,
        Err(err) => {
            let _ = child.kill();
            let _ = child.wait();
            return Err(err);
        }
    };
    if let Err(err) = crate::ipc::set_local_stream_polling(&mut stream, true) {
        let _ = child.kill();
        let _ = child.wait();
        return Err(err);
    }
    let mut child_to_stream = stream;

    let connection_stop = Arc::new(AtomicBool::new(false));
    let upload_failed = Arc::new(AtomicBool::new(false));
    let download_done = Arc::new(AtomicBool::new(false));
    let client_closed = Arc::new(AtomicBool::new(false));
    let upload_cancel = Arc::clone(&upload_stop);
    let upload_bridge_stop = Arc::clone(bridge_stop);
    let upload_failed_worker = Arc::clone(&upload_failed);
    let upload_client_closed = Arc::clone(&client_closed);
    let upload = thread::spawn(move || {
        let result = copy_local_stream_to_writer(
            stream_to_child,
            &mut child_stdin,
            &upload_cancel,
            &upload_bridge_stop,
            &upload_client_closed,
        );
        upload_failed_worker.store(result.is_err(), Ordering::Release);
        result
    });
    let download_stop = Arc::clone(&connection_stop);
    let download_bridge_stop = Arc::clone(bridge_stop);
    let download_done_worker = Arc::clone(&download_done);
    let download_upload_stop = Arc::clone(&upload_stop);
    let download = thread::spawn(move || {
        let mut child_stdout = io::BufReader::new(child_stdout);
        let result = discard_remote_output_preamble(&mut child_stdout).and_then(|()| {
            copy_reader_to_local_stream(
                &mut child_stdout,
                &mut child_to_stream,
                &download_stop,
                &download_bridge_stop,
            )
        });
        download_done_worker.store(true, Ordering::Release);
        download_upload_stop.cancel();
        result
    });

    let mut stopped_at = None;
    let (status_result, child_exited) = loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                upload_stop.cancel();
                break (Ok(status), true);
            }
            Ok(None) => {}
            Err(err) => {
                connection_stop.store(true, Ordering::Release);
                upload_stop.cancel();
                let _ = child.kill();
                let _ = child.wait();
                break (Err(err), false);
            }
        }
        if bridge_stop.load(Ordering::Acquire) {
            connection_stop.store(true, Ordering::Release);
            upload_stop.cancel();
            let _ = child.kill();
            break (child.wait(), false);
        }
        if client_closed.load(Ordering::Acquire)
            || upload_failed.load(Ordering::Acquire)
            || download_done.load(Ordering::Acquire)
        {
            upload_stop.cancel();
            let stopped_at = stopped_at.get_or_insert_with(Instant::now);
            if stopped_at.elapsed() >= Duration::from_millis(250) {
                connection_stop.store(true, Ordering::Release);
                let _ = child.kill();
                break (child.wait(), false);
            }
        }
        thread::sleep(BRIDGE_ACCEPT_POLL);
    };
    upload_stop.cancel();
    if !child_exited {
        connection_stop.store(true, Ordering::Release);
    }
    let upload_result = upload
        .join()
        .map_err(|_| io::Error::other("remote bridge upload worker panicked"))?;
    let download_result = download
        .join()
        .map_err(|_| io::Error::other("remote bridge download worker panicked"))?;
    let stderr = match stderr_reader {
        Some(reader) => reader
            .join()
            .map_err(|_| io::Error::other("SSH stderr reader panicked"))??,
        None => Vec::new(),
    };
    let status = status_result?;

    let stopping = bridge_stop.load(Ordering::Acquire);
    let client_closed = client_closed.load(Ordering::Acquire);
    if child_exited && !status.success() && !stopping && !client_closed {
        return Err(ssh_bridge_exit_error(status, &stderr));
    }
    if !stopping && !client_closed {
        upload_result.map_err(|err| {
            io::Error::new(err.kind(), format!("remote bridge upload failed: {err}"))
        })?;
        download_result.map_err(|err| {
            io::Error::new(err.kind(), format!("remote bridge download failed: {err}"))
        })?;
    }

    if status.success() || stopping || client_closed {
        Ok(())
    } else {
        Err(ssh_bridge_exit_error(status, &stderr))
    }
}

fn ssh_bridge_exit_error(status: std::process::ExitStatus, stderr: &[u8]) -> io::Error {
    let stderr = String::from_utf8_lossy(stderr);
    let stderr = stderr.trim();
    let message = if stderr.is_empty() {
        format!("ssh bridge exited with {status}")
    } else {
        format!("remote SSH connection failed: {stderr}")
    };
    io::Error::new(io::ErrorKind::ConnectionAborted, message)
}

fn capture_ssh_stderr(mut stderr: impl io::Read) -> io::Result<Vec<u8>> {
    let mut captured = Vec::new();
    let mut buffer = [0_u8; 4 * 1024];
    loop {
        let read = stderr.read(&mut buffer)?;
        if read == 0 {
            return Ok(captured);
        }
        let remaining = NONINTERACTIVE_SSH_STDERR_LIMIT.saturating_sub(captured.len());
        captured.extend_from_slice(&buffer[..read.min(remaining)]);
    }
}

fn discard_remote_output_preamble(reader: &mut impl io::BufRead) -> io::Result<()> {
    let marker = REMOTE_OUTPUT_READY_MARKER.as_bytes();
    let mut matched = 0;
    let mut matching = true;
    loop {
        let (consumed, ready) = {
            let buffer = reader.fill_buf()?;
            if buffer.is_empty() {
                if matching && matched == marker.len() {
                    return Ok(());
                }
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "remote command exited before producing its output marker",
                ));
            }

            let mut consumed = 0;
            let mut ready = false;
            for &byte in buffer {
                consumed += 1;
                if byte == b'\n' {
                    if matching && matched == marker.len() {
                        ready = true;
                        break;
                    }
                    matched = 0;
                    matching = true;
                } else if matching && matched < marker.len() && byte == marker[matched] {
                    matched += 1;
                } else if matching && (matched != marker.len() || byte != b'\r') {
                    matching = false;
                }
            }
            (consumed, ready)
        };
        reader.consume(consumed);
        if ready {
            return Ok(());
        }
    }
}

fn terminate_bridge_child(mut child: std::process::Child, message: &'static str) -> io::Result<()> {
    let _ = child.kill();
    let _ = child.wait();
    Err(io::Error::new(io::ErrorKind::BrokenPipe, message))
}

fn copy_reader_to_local_stream<R: io::Read>(
    reader: &mut R,
    stream: &mut crate::ipc::LocalStream,
    connection_stop: &AtomicBool,
    bridge_stop: &AtomicBool,
) -> io::Result<u64> {
    let mut buffer = [0_u8; 16 * 1024];
    let mut total = 0;

    loop {
        let read = match reader.read(&mut buffer) {
            Ok(0) => return Ok(total),
            Ok(read) => read,
            Err(err) if err.kind() == io::ErrorKind::Interrupted => continue,
            Err(err) => return Err(err),
        };
        let mut written = 0;
        while written < read {
            if connection_stop.load(Ordering::Acquire) || bridge_stop.load(Ordering::Acquire) {
                return Ok(total);
            }
            let chunk_len = (read - written).min(4 * 1024);
            match stream.write(&buffer[written..written + chunk_len]) {
                Ok(0) => thread::sleep(BRIDGE_IO_POLL),
                Ok(count) => written += count,
                Err(err) if err.kind() == io::ErrorKind::Interrupted => continue,
                Err(err) if err.kind() == io::ErrorKind::WouldBlock => {
                    thread::sleep(BRIDGE_IO_POLL);
                }
                Err(err) => return Err(err),
            }
        }
        stream.flush()?;
        total += read as u64;
    }
}

fn copy_local_stream_to_writer<W: io::Write>(
    mut stream: crate::ipc::LocalStream,
    writer: &mut W,
    connection_stop: &BridgeUploadStop,
    bridge_stop: &AtomicBool,
    client_closed: &AtomicBool,
) -> io::Result<u64> {
    let mut buffer = [0_u8; 16 * 1024];
    let mut total = 0;

    while !connection_stop.is_stopped() && !bridge_stop.load(Ordering::Acquire) {
        #[cfg(all(test, unix))]
        tests::UPLOAD_READ_ATTEMPTS.with(|attempts| {
            if let Some(attempts) = attempts.borrow().as_ref() {
                attempts.fetch_add(1, Ordering::Relaxed);
            }
        });
        match crate::ipc::poll_local_stream_read_count(&mut stream, &mut buffer)? {
            crate::ipc::LocalStreamReadCount::Data(read) => {
                writer.write_all(&buffer[..read])?;
                writer.flush()?;
                total += read as u64;
            }
            crate::ipc::LocalStreamReadCount::Pending => {
                connection_stop.wake.wait(&stream)?;
            }
            crate::ipc::LocalStreamReadCount::Closed => {
                client_closed.store(true, Ordering::Release);
                break;
            }
        }
    }

    Ok(total)
}

fn run_client_process(
    local_socket: &Path,
    reattach_command: &str,
    keybindings: RemoteKeybindings,
    target: &str,
    session: &str,
) -> io::Result<()> {
    let exe = std::env::current_exe()?;
    let mut command = Command::new(exe);
    command.arg("client");
    configure_remote_client_environment(
        &mut command,
        local_socket,
        reattach_command,
        keybindings,
        target,
        session,
    )?;
    let status = command
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()?;

    if status.success() {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::Interrupted,
            format!("remote client exited with {status}"),
        ))
    }
}

fn configure_remote_client_environment(
    command: &mut Command,
    local_socket: &Path,
    reattach_command: &str,
    keybindings: RemoteKeybindings,
    target: &str,
    session: &str,
) -> io::Result<()> {
    let preference_identity =
        serde_json::to_string(&(target, session)).map_err(io::Error::other)?;
    command
        .env(
            crate::server::socket_paths::CLIENT_SOCKET_PATH_ENV_VAR,
            local_socket,
        )
        .env(REMOTE_PREFERENCES_ENV_VAR, preference_identity)
        .env(REATTACH_COMMAND_ENV_VAR, reattach_command)
        .env(REMOTE_KEYBINDINGS_ENV_VAR, keybindings.as_str())
        .env_remove(crate::api::SOCKET_PATH_ENV_VAR);
    Ok(())
}

fn local_forward_socket_path(target: &str, session_name: &str) -> PathBuf {
    let pid = std::process::id();
    let target_clean = sanitize_path_component(target);
    let session_clean = sanitize_path_component(session_name);
    let readable_name = format!("hicodeer-remote-{pid}-{target_clean}-{session_clean}.sock");
    let target_prefix: String = target_clean.chars().take(8).collect();
    let hash = short_socket_hash(target, session_name);
    let short_name = format!("hicodeer-r-{pid}-{target_prefix}-{hash}.sock");
    crate::platform::remote_bridge_endpoint_path(&readable_name, &short_name)
}

#[cfg(all(test, unix))]
fn fits_unix_socket_path(path: &Path) -> bool {
    use std::os::unix::ffi::OsStrExt;

    path.as_os_str().as_bytes().len() <= 103
}

fn short_socket_hash(target: &str, session: &str) -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut hasher = DefaultHasher::new();
    target.hash(&mut hasher);
    0u8.hash(&mut hasher);
    session.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

fn sanitize_path_component(input: &str) -> String {
    let sanitized: String = input
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-') {
                ch
            } else {
                '-'
            }
        })
        .collect();

    sanitized.trim_matches('-').chars().take(32).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remote_client_preferences_cross_process_handoff() {
        let root =
            std::env::temp_dir().join(format!("hicodeer-preference-handoff-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        for bridge in ["bridge-100.sock", "bridge-200.sock"] {
            let mut command = Command::new(std::env::current_exe().unwrap());
            command.args(["--exact", "client::shell::tests::keybindings_settings::remote_client_preferences_process_child", "--nocapture"]);
            command
                .env("HICODEER_TEST_PREFERENCES_CHILD", "1")
                .env("XDG_STATE_HOME", &root);
            // A stale inherited identity must be replaced by this launch's identity.
            command.env(REMOTE_PREFERENCES_ENV_VAR, r#"["wrong", "session"]"#);
            configure_remote_client_environment(
                &mut command,
                &root.join(bridge),
                "hicodeerd --remote dev",
                RemoteKeybindings::Local,
                "dev",
                "agents",
            )
            .unwrap();
            let output = command.output().unwrap();
            assert!(
                output.status.success(),
                "child failed: {}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            assert!(
                String::from_utf8_lossy(&output.stdout).contains("1 passed"),
                "child test did not run: {}",
                String::from_utf8_lossy(&output.stdout)
            );
        }
        fs::remove_dir_all(root).unwrap();
    }

    fn decode_windows_command(command: &str) -> String {
        let encoded = command
            .strip_prefix("powershell.exe -NoLogo -NoProfile -NonInteractive -EncodedCommand ")
            .expect("encoded PowerShell command");
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(encoded)
            .expect("base64");
        let utf16 = bytes
            .chunks_exact(2)
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
            .collect::<Vec<_>>();
        String::from_utf16(&utf16).expect("UTF-16LE")
    }

    #[cfg(unix)]
    thread_local! {
        pub(super) static UPLOAD_READ_ATTEMPTS: std::cell::RefCell<Option<Arc<std::sync::atomic::AtomicUsize>>> = const { std::cell::RefCell::new(None) };
    }

    #[cfg(unix)]
    fn upload_test_streams(name: &str) -> (crate::ipc::LocalStream, crate::ipc::LocalStream) {
        let socket = local_forward_socket_path(name, "upload-test");
        let listener = crate::ipc::bind_private_local_listener(&socket).unwrap();
        let client = crate::ipc::connect_local_stream(&socket).unwrap();
        let server = listener.accept().unwrap();
        server.set_nonblocking(true).unwrap();
        drop(listener);
        std::fs::remove_file(socket).unwrap();
        (client, server)
    }

    #[cfg(unix)]
    #[test]
    fn bridge_upload_idle_waits_without_repeated_reads_and_cancels() {
        use std::sync::atomic::AtomicUsize;
        use std::sync::mpsc;

        let (mut client, stream) = upload_test_streams("idle");
        let attempts = Arc::new(AtomicUsize::new(0));
        let worker_attempts = Arc::clone(&attempts);
        let stop = Arc::new(BridgeUploadStop::new().unwrap());
        let worker_stop = Arc::clone(&stop);
        let (done_tx, done_rx) = mpsc::channel();
        let worker = thread::spawn(move || {
            UPLOAD_READ_ATTEMPTS.with(|slot| *slot.borrow_mut() = Some(worker_attempts));
            let mut output = Vec::new();
            let closed = AtomicBool::new(false);
            let result = copy_local_stream_to_writer(
                stream,
                &mut output,
                &worker_stop,
                &AtomicBool::new(false),
                &closed,
            );
            done_tx
                .send((result, output, closed.load(Ordering::Acquire)))
                .unwrap();
        });
        let deadline = Instant::now() + Duration::from_secs(5);
        while attempts.load(Ordering::Relaxed) == 0 {
            assert!(Instant::now() < deadline, "upload worker did not start");
            thread::sleep(Duration::from_millis(1));
        }
        thread::sleep(Duration::from_millis(100));
        let idle_reads = attempts.load(Ordering::Relaxed);
        client.write_all(b"pane input").unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while attempts.load(Ordering::Relaxed) < idle_reads + 2 {
            assert!(
                Instant::now() < deadline,
                "input did not wake the upload worker"
            );
            thread::sleep(Duration::from_millis(1));
        }
        thread::sleep(Duration::from_millis(100));
        let reads_after_input = attempts.load(Ordering::Relaxed);
        stop.cancel();
        let (result, output, closed) = done_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        worker.join().unwrap();
        assert_eq!(result.unwrap(), 10);
        assert_eq!(output, b"pane input");
        assert!(!closed, "cancellation is not a peer disconnect");
        assert_eq!(idle_reads, 1, "idle forwarding must wait, not retry reads");
        assert_eq!(
            reads_after_input, 3,
            "forwarding must sleep again after input"
        );
    }

    #[cfg(unix)]
    #[test]
    fn bridge_upload_cancel_before_wait_preserves_download() {
        use std::io::Read as _;

        let (mut client, stream) = upload_test_streams("cancel-before-wait");
        let mut download = stream.try_clone().unwrap();
        let stop = BridgeUploadStop::new().unwrap();
        stop.cancel();
        stop.cancel();
        let closed = AtomicBool::new(false);
        let count = copy_local_stream_to_writer(
            stream,
            &mut Vec::new(),
            &stop,
            &AtomicBool::new(false),
            &closed,
        )
        .unwrap();
        assert_eq!(count, 0);
        assert!(!closed.load(Ordering::Acquire));
        download.write_all(b"final frame").unwrap();
        let mut output = [0; 11];
        client.read_exact(&mut output).unwrap();
        assert_eq!(&output, b"final frame");
    }

    #[cfg(unix)]
    #[test]
    fn bridge_upload_cancel_between_stop_check_and_wait_is_retained() {
        let (_client, stream) = upload_test_streams("cancel-before-poll");
        let stop = BridgeUploadStop::new().unwrap();
        assert!(!stop.is_stopped());
        stop.cancel();
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        let worker = thread::spawn(move || {
            done_tx.send(stop.wake.wait(&stream)).unwrap();
        });
        done_rx
            .recv_timeout(Duration::from_secs(2))
            .unwrap()
            .unwrap();
        worker.join().unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn bridge_upload_drains_input_before_peer_eof() {
        let (mut client, stream) = upload_test_streams("drain");
        let payload = vec![b'x'; 1024 * 1024];
        let expected = payload.clone();
        let worker = thread::spawn(move || {
            let stop = BridgeUploadStop::new().unwrap();
            let mut output = Vec::new();
            let closed = AtomicBool::new(false);
            let count = copy_local_stream_to_writer(
                stream,
                &mut output,
                &stop,
                &AtomicBool::new(false),
                &closed,
            )
            .unwrap();
            assert!(closed.load(Ordering::Acquire));
            assert_eq!(count, output.len() as u64);
            output
        });
        client.write_all(&payload).unwrap();
        drop(client);
        assert_eq!(worker.join().unwrap(), expected);
    }

    #[cfg(unix)]
    #[test]
    fn bridge_socket_is_user_only() {
        use std::os::unix::fs::PermissionsExt;

        let socket = std::env::temp_dir().join(format!(
            "hicodeer-bridge-permissions-test-{}.sock",
            std::process::id()
        ));
        let remote_herdr = RemoteHerdr::for_platform(RemotePlatform {
            os: "linux",
            arch: "x86_64",
        });
        let bridge = SshStdioBridge::start(
            "example".to_string(),
            remote_herdr,
            socket.clone(),
            "default".to_string(),
            None,
            false,
        )
        .expect("start bridge listener");

        let mode = std::fs::metadata(&socket).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, BRIDGE_SOCKET_PERMISSION_MODE);

        drop(bridge);
        let _ = std::fs::remove_file(socket);
    }

    #[cfg(unix)]
    #[test]
    fn accepted_bridge_stream_is_reset_to_blocking() {
        use std::os::fd::AsRawFd as _;

        fn is_nonblocking(stream: &crate::ipc::LocalStream) -> bool {
            let fd = match stream {
                crate::ipc::LocalStream::UdSocket(stream) => stream.inner().as_raw_fd(),
            };
            // SAFETY: F_GETFL only reads flags from the live descriptor owned by `stream`.
            let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
            assert!(flags >= 0, "fcntl(F_GETFL): {}", io::Error::last_os_error());
            flags & libc::O_NONBLOCK != 0
        }

        let socket = std::env::temp_dir().join(format!(
            "hicodeer-bridge-blocking-test-{}.sock",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&socket);
        let listener = crate::ipc::bind_private_local_listener(&socket).expect("bind listener");
        let client = crate::ipc::connect_local_stream(&socket).expect("connect client");
        let mut server = listener.accept().expect("accept client");

        crate::ipc::set_local_stream_polling(&mut server, true)
            .expect("force the macOS accepted-stream state");
        assert!(is_nonblocking(&server));
        let server = prepare_remote_bridge_stream(server).expect("prepare bridge stream");
        assert!(!is_nonblocking(&server));

        drop(server);
        drop(client);
        drop(listener);
        let _ = std::fs::remove_file(socket);
    }

    #[cfg(windows)]
    #[test]
    fn bridge_stream_delivers_large_frame_before_delayed_reply() {
        use std::io::Read as _;
        use std::sync::mpsc;
        use std::time::{SystemTime, UNIX_EPOCH};

        fn read_frame(stream: &mut crate::ipc::LocalStream) -> Vec<u8> {
            let mut length = [0; 4];
            stream.read_exact(&mut length).expect("read frame length");
            let length = usize::try_from(u32::from_le_bytes(length)).expect("frame length fits");
            let mut body = vec![0; length];
            stream.read_exact(&mut body).expect("read frame body");
            body
        }

        fn write_frame(stream: &mut crate::ipc::LocalStream, body: &[u8]) {
            let length = u32::try_from(body.len()).expect("frame length fits");
            stream
                .write_all(&length.to_le_bytes())
                .expect("write frame length");
            stream.write_all(body).expect("write frame body");
        }

        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock after epoch")
            .as_nanos();
        let socket = std::env::temp_dir().join(format!(
            "hicodeer-bridge-large-frame-{}-{nonce}.sock",
            std::process::id()
        ));
        let listener = crate::ipc::bind_private_local_listener(&socket).expect("bind listener");
        let (large_received_tx, large_received_rx) = mpsc::channel();
        let client_socket = socket.clone();
        let client = thread::spawn(move || {
            let mut stream =
                crate::ipc::connect_local_stream(&client_socket).expect("connect client");
            assert_eq!(read_frame(&mut stream), b"welcome");
            assert_eq!(read_frame(&mut stream), vec![b'x'; 16 * 1024]);
            large_received_tx.send(()).expect("signal large frame");
            assert_eq!(read_frame(&mut stream), b"pong");
        });
        let mut server = prepare_remote_bridge_stream(listener.accept().expect("accept client"))
            .expect("prepare bridge stream");

        crate::ipc::set_local_stream_polling(&mut server, true)
            .expect("enable bridge read polling");
        write_frame(&mut server, b"welcome");
        write_frame(&mut server, &vec![b'x'; 16 * 1024]);
        large_received_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("client received large frame");
        write_frame(&mut server, b"pong");

        client.join().expect("client thread");
        drop(server);
        drop(listener);
        let _ = std::fs::remove_file(socket);
    }

    #[test]
    fn bridge_drop_while_waiting_for_client_is_bounded() {
        let socket = local_forward_socket_path("drop-test", "default");
        let remote_herdr = RemoteHerdr::for_platform(RemotePlatform {
            os: "linux",
            arch: "x86_64",
        });
        let bridge = SshStdioBridge::start(
            "example".to_string(),
            remote_herdr,
            socket.clone(),
            "default".to_string(),
            None,
            false,
        )
        .expect("start bridge listener");
        let started = Instant::now();

        drop(bridge);

        assert!(started.elapsed() < Duration::from_secs(1));
        assert!(!socket.exists());
    }

    #[cfg(unix)]
    #[test]
    fn managed_ssh_config_includes_user_config_then_fallback() {
        use std::os::unix::fs::PermissionsExt;

        let managed_config = write_managed_ssh_config("example").expect("write managed config");
        let path = managed_config.options.config_path.clone();
        let control_path = managed_config
            .options
            .control_path
            .clone()
            .expect("Unix managed config has a control path");
        let contents = std::fs::read_to_string(&path).expect("read keepalive config");

        // herdr's fallback transport settings are present...
        assert!(
            contents.contains("Host *"),
            "config should add a Host * fallback block: {contents}"
        );
        assert!(
            contents.contains("ServerAliveInterval 15"),
            "config should set the keepalive interval: {contents}"
        );
        assert!(
            contents.contains("ServerAliveCountMax 4"),
            "config should set the keepalive count: {contents}"
        );
        assert!(!contents.contains("ControlMaster"));
        assert!(!contents.contains("ControlPersist"));
        assert!(!contents.contains("ControlPath"));
        // ...and any user config is Included (quoted) BEFORE it so
        // first-value-wins keeps the user's own settings.
        if let Some(home) = std::env::var_os("HOME") {
            let user_config = PathBuf::from(home).join(".ssh").join("config");
            if user_config.is_file() {
                let include = format!(
                    "Include {}",
                    ssh_config_quote(&user_config.to_string_lossy())
                );
                let include_at = contents.find(&include).expect("user config Included");
                let fallback_at = contents.find("Host *").expect("fallback present");
                assert!(
                    include_at < fallback_at,
                    "user config must be Included before herdr's fallback: {contents}"
                );
            }
        }

        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(
            mode, BRIDGE_SOCKET_PERMISSION_MODE,
            "keepalive config must be user-only"
        );
        // The config lives in a private 0700 dir, not a predictable temp path.
        let dir = path.parent().expect("config has a parent dir");
        let dir_mode = std::fs::metadata(dir).unwrap().permissions().mode() & 0o777;
        assert_eq!(dir_mode, 0o700, "ssh config dir must be user-only");
        assert!(
            fits_unix_socket_path(&control_path),
            "control socket path must fit portable Unix socket limits"
        );

        drop(managed_config);
    }

    #[cfg(unix)]
    #[test]
    fn shared_ssh_transport_survives_helper_config_drop() {
        let first = write_managed_ssh_config("example").unwrap();
        let second = write_managed_ssh_config("example").unwrap();
        let socket = first.options.control_path.clone().unwrap();
        assert_eq!(Some(&socket), second.options.control_path.as_ref());
        assert_ne!(socket.parent(), first.options.config_path.parent());
        let config_path = first.options.config_path.clone();
        drop(first);
        assert!(!config_path.exists());
        assert!(socket.parent().unwrap().is_dir());
    }

    #[test]
    fn ssh_authentication_diagnostics_are_narrow() {
        for message in [
            "user@host: Permission denied (publickey).",
            "Permission denied (keyboard-interactive,password).",
            "Permission denied (password).",
            "sign_and_send_pubkey: signing failed for ED25519 from agent: agent refused operation",
        ] {
            assert!(ssh_error_requires_authentication(message), "{message}");
        }
        for message in [
            "Host key verification failed.",
            "REMOTE HOST IDENTIFICATION HAS CHANGED!",
            "Permission denied opening /tmp/file",
            "Connection refused",
            "agent disconnected",
            "Permission denied (publickey). Host key verification failed.",
        ] {
            assert!(!ssh_error_requires_authentication(message), "{message}");
        }
    }

    #[test]
    fn bridge_options_keep_temporary_config_alive_after_helper_drop() {
        let config = write_managed_ssh_config("example").unwrap();
        let path = config.options.config_path.clone();
        let worker_options = config.options.clone();
        drop(config);
        assert!(path.is_file());
        drop(worker_options);
        assert!(!path.exists());
    }

    #[cfg(unix)]
    #[test]
    fn authentication_command_uses_shared_transport_without_askpass_or_host_key_relaxation() {
        let config = write_managed_ssh_config("example").unwrap();
        let setup = RemoteSsh::new("example".into(), true, "other-session".into());
        assert_eq!(
            config.options.control_path,
            setup.options().unwrap().control_path
        );
        let authentication = authentication_command_with_config("example", config);
        let command = &authentication.command;
        assert_eq!(command.get_program(), "ssh");
        let args = command
            .get_args()
            .map(|arg| arg.to_string_lossy())
            .collect::<Vec<_>>();
        for required in [
            "ControlMaster=auto",
            "ControlPersist=600",
            "BatchMode=no",
            "StrictHostKeyChecking=yes",
        ] {
            assert!(args.iter().any(|arg| arg == required), "missing {required}");
        }
        assert_eq!(&args[args.len() - 3..], &["-T", "example", "exit"]);
        let env = command.get_envs().collect::<Vec<_>>();
        assert!(env.iter().any(
            |(key, value)| *key == std::ffi::OsStr::new("SSH_ASKPASS_REQUIRE")
                && *value == Some(std::ffi::OsStr::new("never"))
        ));
        assert!(env
            .iter()
            .any(|(key, value)| *key == std::ffi::OsStr::new("SSH_ASKPASS") && value.is_none()));
    }

    #[test]
    fn authentication_command_rejects_option_injection() {
        assert_eq!(
            ssh_authentication_command("-oProxyCommand=bad")
                .err()
                .unwrap()
                .kind(),
            io::ErrorKind::InvalidInput
        );
    }

    #[test]
    fn unmanaged_ssh_setup_preserves_plain_transport() {
        let ssh = RemoteSsh::new("example".into(), false, "main".into());
        assert!(ssh.options().is_none());
        assert!(!ssh.command().get_args().any(|arg| arg == "-F"));
    }

    #[test]
    fn ssh_config_quote_wraps_path_with_spaces() {
        assert_eq!(
            ssh_config_quote("/home/a b/.ssh/config"),
            "\"/home/a b/.ssh/config\""
        );
    }

    #[cfg(unix)]
    #[test]
    fn remote_ssh_command_uses_managed_config_when_present() {
        let managed_config = write_managed_ssh_config("example").expect("write managed config");
        let config_path = managed_config.options.config_path.clone();
        let control_path = managed_config.options.control_path.clone().unwrap();
        let ssh = RemoteSsh {
            target: "example".to_string(),
            session_name: crate::session::DEFAULT_SESSION_NAME.into(),
            managed_config: Some(managed_config),
            noninteractive: false,
        };

        let command = ssh.command();
        let args = command
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect::<Vec<_>>();

        assert_eq!(
            args,
            vec![
                "-C".to_string(),
                "-F".to_string(),
                config_path.to_string_lossy().into_owned(),
                "-S".to_string(),
                control_path.to_string_lossy().into_owned(),
                "-o".to_string(),
                "ControlMaster=auto".to_string(),
                "-o".to_string(),
                "ControlPersist=600".to_string(),
                "-T".to_string(),
                "example".to_string(),
            ]
        );

    }

    #[cfg(windows)]
    #[test]
    fn windows_managed_ssh_config_uses_keepalives_without_control_socket() {
        let managed_config = write_managed_ssh_config("example").expect("write managed config");
        let config_path = managed_config.options.config_path.clone();
        assert!(managed_config.options.control_path.is_none());
        let contents = std::fs::read_to_string(&config_path).expect("read managed config");
        assert!(contents.contains("ServerAliveInterval 15"));
        assert!(contents.contains("ServerAliveCountMax 4"));
        // Git Bash's MSYS OpenSSH ignores drive-letter `Include` paths, so the
        // user config must be referenced through `~` for aliases to resolve.
        let include_at = contents
            .find("Include \"~/.ssh/config\"")
            .expect("user config Included through home");
        let fallback_at = contents.find("Host *").expect("fallback present");
        assert!(
            include_at < fallback_at,
            "user config must be Included before herdr's fallback: {contents}"
        );

        let ssh = RemoteSsh {
            target: "example".to_string(),
            session_name: crate::session::DEFAULT_SESSION_NAME.into(),
            managed_config: Some(managed_config),
            noninteractive: false,
        };
        let args = ssh
            .command()
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert_eq!(
            args,
            vec![
                "-C".to_string(),
                "-F".to_string(),
                config_path.to_string_lossy().into_owned(),
                "-T".to_string(),
                "example".to_string(),
            ]
        );
        let scp_args = ssh
            .scp_command()
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert_eq!(
            scp_args,
            vec![
                "-C".to_string(),
                "-F".to_string(),
                config_path.to_string_lossy().into_owned(),
            ]
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows_ssh_config_include_uses_forward_slashes() {
        assert_eq!(
            ssh_config_include_path(Path::new(r"C:\Users\A B\.ssh\config")),
            r#""C:/Users/A B/.ssh/config""#
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows_ssh_user_config_include_uses_home_shorthand() {
        // MSYS/Git Bash OpenSSH does not resolve `C:/...` in `Include`, so the
        // user config is referenced through `~` regardless of the profile path.
        assert_eq!(
            ssh_user_config_include(Some(Path::new(r"C:\Users\A B\.ssh\config"))),
            Some(r#""~/.ssh/config""#.to_string())
        );
        assert_eq!(
            ssh_user_config_include(None),
            Some(r#""~/.ssh/config""#.to_string())
        );
    }

    #[cfg(unix)]
    #[test]
    fn endpoint_probe_preserves_setup_ssh_options() {
        let managed_config = write_managed_ssh_config("example").expect("write managed config");
        let marker = managed_config
            .options
            .config_path
            .with_file_name("probe-ran");
        fs::write(
            &managed_config.options.config_path,
            format!(
                "Host *\n  ProxyCommand /bin/sh -c {}\n",
                shell_quote(&format!(
                    ": > {}; exit 1",
                    shell_quote(&marker.to_string_lossy())
                ))
            ),
        )
        .expect("write isolated probe config");
        let ssh = RemoteSsh {
            target: "hicodeer-probe.invalid".into(),
            session_name: "probe-options".into(),
            managed_config: Some(managed_config),
            noninteractive: false,
        };
        let remote = RemoteHerdr::for_platform(RemotePlatform {
            os: "linux",
            arch: "x86_64",
        });

        let error = probe_remote_endpoint(&ssh, &remote).expect_err("proxy refuses connection");

        assert!(
            marker.exists(),
            "endpoint probe discarded the authenticated setup's SSH options: {error}"
        );
    }

    #[test]
    fn noninteractive_ssh_stderr_capture_is_bounded() {
        let stderr = vec![b'x'; NONINTERACTIVE_SSH_STDERR_LIMIT + 4096];
        let captured = capture_ssh_stderr(stderr.as_slice()).expect("capture stderr");
        assert_eq!(captured.len(), NONINTERACTIVE_SSH_STDERR_LIMIT);
    }

    #[test]
    fn noninteractive_ssh_command_cannot_prompt_or_accept_unknown_hosts() {
        let ssh = RemoteSsh::new_noninteractive("example".into());
        let args = ssh
            .command()
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        for required in [
            "-C",
            "BatchMode=yes",
            "NumberOfPasswordPrompts=0",
            "StrictHostKeyChecking=yes",
            "ConnectTimeout=10",
            "ConnectionAttempts=1",
            "ServerAliveInterval=15",
            "ServerAliveCountMax=4",
        ] {
            assert!(args.iter().any(|arg| arg == required), "missing {required}");
        }
        assert_eq!(args.iter().any(|arg| arg == "-F"), ssh.options().is_some());
    }

    #[test]
    fn remote_setup_approval_requires_input_and_rejects_unrecognized_answers() {
        for default in [false, true] {
            for input in ["", "maybe\n"] {
                assert_eq!(
                    read_remote_confirmation(&mut input.as_bytes(), default)
                        .unwrap_err()
                        .kind(),
                    io::ErrorKind::Interrupted
                );
            }
            assert_eq!(
                read_remote_confirmation(&mut "\n".as_bytes(), default).unwrap(),
                default
            );
            assert!(read_remote_confirmation(&mut "YES\n".as_bytes(), default).unwrap());
            assert!(!read_remote_confirmation(&mut "no\n".as_bytes(), default).unwrap());
        }
    }

    #[test]
    fn saved_machine_compatibility_uses_capabilities_not_release_or_private_protocol() {
        let linux = RemotePlatform {
            os: "linux",
            arch: "x86_64",
        };
        let mut status = RemoteClientStatusJson {
            version: Some("0.1.0".into()),
            protocol: Some(1),
            endpoint_protocol_generation: Some(
                crate::protocol::endpoint::ENDPOINT_PROTOCOL_GENERATION,
            ),
            endpoint_capabilities: vec![
                crate::protocol::endpoint::SURFACE_INTEREST_CAPABILITY.into(),
                crate::protocol::endpoint::PRESENTATION_EFFECTS_FENCE_CAPABILITY.into(),
                crate::protocol::endpoint::HEALTH_CHECK_CAPABILITY.into(),
            ],
            remote_host_bridge: false,
            remote_bridge_idle_timeout: false,
        };
        assert!(status.supports_endpoint_requirement(&linux, true));
        for index in 0..status.endpoint_capabilities.len() {
            let removed = status.endpoint_capabilities.remove(index);
            assert!(!status.supports_endpoint_requirement(&linux, true));
            assert!(status.supports_endpoint_requirement(&linux, false));
            status.endpoint_capabilities.insert(index, removed);
        }
        status.endpoint_protocol_generation = None;
        assert!(!status.supports_endpoint_requirement(&linux, true));
    }

    #[test]
    fn saved_machine_server_commands_are_scoped_to_the_explicit_session() {
        let herdr =
            RemoteHerdr::for_platform(RemotePlatform::from_uname("Linux", "x86_64").unwrap());
        for (args, command) in [
            (&["status", "server", "--json"][..], "status server --json"),
            (&["server", "stop"][..], "server stop"),
            (&["remote-client-bridge"][..], "remote-client-bridge"),
        ] {
            assert_eq!(
                herdr.executable.session_command("agents", args),
                format!("{} --session agents {command}", herdr.executable.display())
            );
            assert_eq!(
                herdr
                    .executable
                    .session_command(crate::session::DEFAULT_SESSION_NAME, args),
                format!("{} {command}", herdr.executable.display())
            );
        }
        assert!(herdr
            .executable
            .live_handoff_command("agents", 19, "0.7.9")
            .starts_with(&format!(
                "{} --session agents server live-handoff",
                herdr.executable.display()
            )));
    }

    #[test]
    fn remote_ssh_commands_compress_without_managed_config() {
        let ssh = RemoteSsh {
            target: "example".to_string(),
            session_name: crate::session::DEFAULT_SESSION_NAME.into(),
            managed_config: None,
            noninteractive: false,
        };

        let command = ssh.command();
        let args = command
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect::<Vec<_>>();

        assert_eq!(args, vec!["-C", "-T", "example"]);
    }

    #[test]
    fn extract_remote_args_removes_space_form() {
        let args = vec![
            "herdr".into(),
            "--remote".into(),
            "dev".into(),
            "--help".into(),
        ];
        let (cleaned, remote) = extract_remote_args(&args).unwrap();
        assert_eq!(cleaned, vec!["herdr", "--help"]);
        let remote = remote.unwrap();
        assert_eq!(remote.target, "dev");
        assert_eq!(remote.keybindings, RemoteKeybindings::Local);
    }

    #[test]
    fn extract_remote_args_removes_equals_form() {
        let args = vec!["herdr".into(), "--remote=user@host".into()];
        let (cleaned, remote) = extract_remote_args(&args).unwrap();
        assert_eq!(cleaned, vec!["herdr"]);
        let remote = remote.unwrap();
        assert_eq!(remote.target, "user@host");
        assert_eq!(remote.keybindings, RemoteKeybindings::Local);
    }

    #[test]
    fn extract_remote_args_accepts_remote_keybindings_server() {
        let args = vec![
            "herdr".into(),
            "--remote".into(),
            "dev".into(),
            "--remote-keybindings=server".into(),
        ];
        let (cleaned, remote) = extract_remote_args(&args).unwrap();
        assert_eq!(cleaned, vec!["herdr"]);
        let remote = remote.unwrap();
        assert_eq!(remote.target, "dev");
        assert_eq!(remote.keybindings, RemoteKeybindings::Server);
    }

    #[test]
    fn extract_remote_args_accepts_remote_keybindings_space_form() {
        let args = vec![
            "herdr".into(),
            "--remote=dev".into(),
            "--remote-keybindings".into(),
            "server".into(),
        ];
        let (cleaned, remote) = extract_remote_args(&args).unwrap();
        assert_eq!(cleaned, vec!["herdr"]);
        assert_eq!(remote.unwrap().keybindings, RemoteKeybindings::Server);
    }

    #[test]
    fn extract_remote_args_accepts_explicit_handoff() {
        let args = vec!["herdr".into(), "--remote=dev".into(), "--handoff".into()];

        let (cleaned, remote) = extract_remote_args(&args).unwrap();

        assert_eq!(cleaned, vec!["herdr"]);
        let remote = remote.unwrap();
        assert_eq!(remote.target, "dev");
        assert!(remote.live_handoff);
    }

    #[test]
    fn extract_remote_args_preserves_child_remote_options_after_separator() {
        let args = vec![
            "herdr".into(),
            "agent".into(),
            "start".into(),
            "repro".into(),
            "--".into(),
            "child".into(),
            "--remote".into(),
            "dev".into(),
            "--remote-keybindings=server".into(),
            "--handoff".into(),
        ];

        let (cleaned, remote) = extract_remote_args(&args).unwrap();

        assert_eq!(cleaned, args);
        assert!(remote.is_none());
    }

    #[test]
    fn extract_remote_args_preserves_handoff_without_remote() {
        let args = vec!["herdr".into(), "update".into(), "--handoff".into()];

        let (cleaned, remote) = extract_remote_args(&args).unwrap();

        assert_eq!(cleaned, args);
        assert!(remote.is_none());
    }

    #[test]
    fn extract_remote_args_rejects_remote_keybindings_without_remote() {
        let args = vec!["herdr".into(), "--remote-keybindings=server".into()];
        let err = extract_remote_args(&args).unwrap_err();
        assert_eq!(err, "--remote-keybindings requires --remote");
    }

    #[test]
    fn extract_remote_args_rejects_duplicate_remote_keybindings() {
        let args = vec![
            "herdr".into(),
            "--remote=dev".into(),
            "--remote-keybindings=local".into(),
            "--remote-keybindings=server".into(),
        ];
        let err = extract_remote_args(&args).unwrap_err();
        assert_eq!(err, "--remote-keybindings can only be specified once");
    }

    #[test]
    fn extract_remote_args_requires_value() {
        let args = vec!["herdr".into(), "--remote".into()];
        let err = extract_remote_args(&args).unwrap_err();
        assert_eq!(err, "missing value for --remote");
    }

    #[test]
    fn extract_remote_args_rejects_empty_value() {
        let args = vec!["herdr".into(), "--remote=".into()];
        let err = extract_remote_args(&args).unwrap_err();
        assert_eq!(err, "missing value for --remote");
    }

    #[test]
    fn extract_remote_args_rejects_duplicate_values() {
        let args = vec![
            "herdr".into(),
            "--remote=dev".into(),
            "--remote=prod".into(),
        ];
        let err = extract_remote_args(&args).unwrap_err();
        assert_eq!(err, "--remote can only be specified once");
    }

    #[test]
    fn extract_remote_args_rejects_option_like_target() {
        let args = vec!["herdr".into(), "--remote".into(), "-oProxyCommand=x".into()];
        let err = extract_remote_args(&args).unwrap_err();
        assert_eq!(err, "--remote target must not start with '-'");
    }

    #[test]
    fn sanitize_path_component_removes_shell_sensitive_chars() {
        assert_eq!(sanitize_path_component("user@host:22"), "user-host-22");
    }

    #[test]
    fn remote_platform_maps_uname_values() {
        assert_eq!(
            RemotePlatform::from_uname("Linux", "amd64").unwrap(),
            RemotePlatform {
                os: "linux",
                arch: "x86_64",
            }
        );
        assert_eq!(
            RemotePlatform::from_uname("Darwin", "arm64").unwrap(),
            RemotePlatform {
                os: "macos",
                arch: "aarch64",
            }
        );
        assert!(RemotePlatform::from_uname("FreeBSD", "x86_64").is_none());
    }

    #[test]
    fn windows_platform_probe_preserves_native_and_unsupported_architectures() {
        for arch in ["AMD64", "x86_64"] {
            assert_eq!(
                parse_windows_platform_probe(&format!("profile noise\r\nhicodeer-windows:{arch}\r\n"))
                    .unwrap(),
                Some(DetectedRemotePlatform {
                    platform: RemotePlatform {
                        os: "windows",
                        arch: "x86_64",
                    },
                    setup_note: None,
                })
            );
        }
        assert_eq!(parse_windows_platform_probe("other output").unwrap(), None);
        assert_eq!(
            parse_windows_platform_probe("hicodeer-windows:x86").unwrap_err(),
            "unsupported remote platform: Windows X86"
        );
    }

    #[test]
    fn windows_arm64_probe_selects_x64_package() {
        let platform = parse_windows_platform_probe("hicodeer-windows:ARM64")
            .expect("Windows ARM64 should use the x64 build on a best-effort basis")
            .expect("the Windows platform marker should be recognized");
        assert_eq!(platform.platform.os, "windows");
        assert_eq!(platform.platform.arch, "x86_64");
        let note = platform
            .setup_note
            .expect("ARM64 setup must warn about emulation");
        assert!(note.contains("best-effort"));
        assert!(note.contains("x64 emulation"));
        assert!(note.contains("bugs and issues are expected"));
    }

    #[test]
    fn windows_probe_preserves_transport_and_unsupported_unix_failures() {
        let cases = [
            (true, Some(0), false, false),
            (true, Some(0), true, true),
            (false, Some(255), false, false),
            (false, None, false, false),
            (false, Some(1), false, true),
        ];
        for (success, exit_code, windows_uname_hint, expected) in cases {
            assert_eq!(
                should_try_windows_probe(success, exit_code, windows_uname_hint),
                expected,
                "success={success}, exit_code={exit_code:?}, windows_uname_hint={windows_uname_hint}"
            );
        }

        for (os, expected) in [
            ("MINGW64_NT-10.0-26100", true),
            ("MSYS_NT-10.0", true),
            ("CYGWIN_NT-10.0", true),
            ("FreeBSD", false),
        ] {
            assert_eq!(looks_like_windows_uname(os), expected, "{os}");
        }
    }

    #[cfg(windows)]
    #[test]
    fn windows_bridge_returns_application_exit_while_descendant_is_running() {
        let pid_file = std::env::temp_dir().join(format!(
            "herdr bridge descendant {}-{}.pid",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("current time")
                .as_nanos()
        ));
        let script = format!(
            "$child = Start-Process powershell.exe -ArgumentList '-NoProfile -NonInteractive -Command Start-Sleep -Seconds 30' -NoNewWindow -PassThru; Set-Content -LiteralPath {} -Value $child.Id; exit 23",
            crate::platform::quote_powershell_arg(&pid_file.to_string_lossy())
        );
        let encoded = base64::engine::general_purpose::STANDARD.encode(
            script
                .encode_utf16()
                .flat_map(u16::to_le_bytes)
                .collect::<Vec<_>>(),
        );
        let command = windows_powershell_streaming_application_command(
            "powershell.exe",
            &["-NoProfile", "-NonInteractive", "-EncodedCommand", &encoded],
        );
        let mut launcher = Command::new("powershell.exe");
        launcher
            .args(["-NoProfile", "-NonInteractive", "-EncodedCommand"])
            .arg(command.split_whitespace().last().unwrap())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        crate::platform::configure_background_command(&mut launcher);
        let mut launcher = launcher.spawn().expect("launch Windows bridge command");
        let deadline = Instant::now() + Duration::from_secs(10);
        let status = loop {
            let status = launcher.try_wait().expect("poll bridge launcher");
            if status.is_some() || Instant::now() >= deadline {
                break status;
            }
            thread::sleep(Duration::from_millis(20));
        };
        if status.is_none() {
            let mut cleanup = Command::new("taskkill.exe");
            cleanup.args(["/PID", &launcher.id().to_string(), "/T", "/F"]);
            crate::platform::configure_background_command(&mut cleanup);
            let _ = cleanup.output();
            let _ = launcher.kill();
        }
        let _ = launcher.wait();
        // Clean up the launcher before a missing PID can fail the test.
        let descendant = fs::read_to_string(&pid_file);
        let _ = fs::remove_file(pid_file);
        let descendant = descendant.expect("descendant PID");
        let descendant = descendant.trim().parse::<u32>().expect("numeric PID");
        let mut cleanup = Command::new("powershell.exe");
        cleanup
            .args(["-NoProfile", "-NonInteractive", "-Command"])
            .arg(format!("Stop-Process -Id {descendant} -ErrorAction Stop"));
        crate::platform::configure_background_command(&mut cleanup);
        let descendant_was_running = cleanup.status().expect("stop test descendant").success();
        assert!(
            descendant_was_running,
            "descendant must outlive the application"
        );
        assert_eq!(status.and_then(|status| status.code()), Some(23));
    }

    #[test]
    fn machine_metadata_keeps_raw_resolved_paths_not_shell_expressions() {
        let remote = RemoteHerdr::for_platform(RemotePlatform {
            os: "linux",
            arch: "x86_64",
        });
        assert!(remote.machine_metadata().is_none());
        let path = "/home/user's files/$literal/herdr";
        let resolved = remote.with_posix_path(path);
        assert_eq!(resolved.machine_metadata().unwrap().executable, path);
        assert_eq!(resolved.executable.display(), shell_quote(path));
        let remote = RemoteHerdr::for_platform(RemotePlatform {
            os: "windows",
            arch: "x86_64",
        });
        assert!(remote.machine_metadata().is_none());
        let path = r"C:\Users\A B\hicodeer.exe";
        assert_eq!(
            remote
                .with_windows_path(path.into())
                .machine_metadata()
                .unwrap()
                .executable,
            path
        );
    }

    #[test]
    fn cached_windows_api_command_checks_before_starting_the_stream() {
        let path = r"C:\Users\A'B\hicodeer.exe";
        let command = cached_remote_api_command(
            &crate::client::endpoint::SshMachineMetadata {
                os: "windows".into(),
                executable: path.into(),
            },
            "fleet",
        );
        let encoded = command.split_whitespace().last().unwrap();
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(encoded)
            .unwrap();
        let words = bytes
            .chunks_exact(2)
            .map(|chunk| u16::from_le_bytes([chunk[0], chunk[1]]))
            .collect::<Vec<_>>();
        let script = String::from_utf16(&words).unwrap();
        assert!(script.contains(&crate::platform::quote_powershell_arg(path)));
        assert!(script.contains(STALE_API_METADATA));
        assert!(
            script.find("remote-api-bridge --check").unwrap()
                < script.find("Start-Process").unwrap()
        );
        assert!(script.contains("$LASTEXITCODE -ne 0"));
        assert!(script.contains("-NoNewWindow -PassThru"));
        assert!(script.contains("--session fleet remote-api-bridge"));
    }

    #[test]
    fn windows_remote_commands_use_one_encoded_powershell_grammar() {
        let executable = RemoteExecutable::WindowsPath("hicodeer.exe".to_string());
        let commands = [
            (
                "platform probe",
                windows_platform_probe_command(),
                "$arch = if ($env:PROCESSOR_ARCHITEW6432) { $env:PROCESSOR_ARCHITEW6432 } else { $env:PROCESSOR_ARCHITECTURE }; [Console]::Out.WriteLine('hicodeer-windows:' + $arch); exit 0",
            ),
            (
                "client status",
                executable.status_client_command(),
                "& hicodeer.exe status client '--json'; exit $LASTEXITCODE",
            ),
            (
                "named server status",
                executable.session_command("agents", &["status", "server", "--json"]),
                "& hicodeer.exe '--session' agents status server '--json'; exit $LASTEXITCODE",
            ),
            (
                "server stop",
                executable.session_command("agents", &["server", "stop"]),
                "& hicodeer.exe '--session' agents server stop; exit $LASTEXITCODE",
            ),
            (
                "direct bridge",
                executable.bridge_command("agents"),
                "$process = Start-Process -FilePath hicodeer.exe -ArgumentList '--session agents remote-client-bridge' -NoNewWindow -PassThru -ErrorAction Stop; $null = $process.Handle; $process.WaitForExit(); exit $process.ExitCode",
            ),
            (
                "API bridge with explicit default session",
                remote_api_bridge_command(&RemoteHerdr::for_platform(RemotePlatform { os: "windows", arch: "x86_64" }), "default", false),
                "$process = Start-Process -FilePath hicodeer.exe -ArgumentList '--session default remote-api-bridge' -NoNewWindow -PassThru -ErrorAction Stop; $null = $process.Handle; $process.WaitForExit(); exit $process.ExitCode",
            ),
            (
                "API bridge capability probe",
                remote_api_bridge_command(&RemoteHerdr::for_platform(RemotePlatform { os: "windows", arch: "x86_64" }), "agents", true),
                "$process = Start-Process -FilePath hicodeer.exe -ArgumentList '--session agents remote-api-bridge --check' -NoNewWindow -PassThru -ErrorAction Stop; $null = $process.Handle; $process.WaitForExit(); exit $process.ExitCode",
            ),
            (
                "saved bridge with closed stdin",
                executable.saved_bridge_command("agents"),
                "$process = Start-Process -FilePath hicodeer.exe -ArgumentList '--session agents remote-client-bridge' -NoNewWindow -PassThru -ErrorAction Stop; $null = $process.Handle; $process.WaitForExit(); exit $process.ExitCode",
            ),
        ];

        let preamble = format!(
            "[Console]::Out.WriteLine(); [Console]::Out.WriteLine('{REMOTE_OUTPUT_READY_MARKER}'); [Console]::Out.Flush(); $ProgressPreference = 'SilentlyContinue'; "
        );
        for (label, command, expected) in commands {
            let script = decode_windows_command(&command);
            let script = script
                .strip_prefix(&preamble)
                .unwrap_or_else(|| panic!("{label} lacks shared output marker: {script}"));
            assert_eq!(script, expected, "{label}");
        }
    }

    #[test]
    fn windows_remote_commands_suppress_powershell_progress_records() {
        // PowerShell serializes progress records (such as module auto-loading's
        // "Preparing modules for first use.") to the stderr CLIXML stream when
        // stderr is redirected under `-EncodedCommand`. Herdr forwards that
        // stderr, so the shared preamble must silence progress.
        let script = decode_windows_command(&windows_powershell_script_command("exit 0"));
        let preference = "$ProgressPreference = 'SilentlyContinue'; ";
        assert!(script.contains(preference), "{script}");
        assert!(
            script.find(preference).unwrap() < script.find("exit 0").unwrap(),
            "{script}"
        );
    }

    #[test]
    fn windows_candidate_discovery_prefers_path_then_active_release() {
        let remote_herdr = RemoteHerdr::for_platform(RemotePlatform {
            os: "windows",
            arch: "x86_64",
        });
        let path = r"C:\stale\hicodeer.exe";
        let active = r"C:\Users\test\.hicodeer\packages\standalone\releases\current\hicodeer.exe";
        let stdout = format!(
            "{WINDOWS_REMOTE_PATH_MARKER}{}\nnoise\n{WINDOWS_REMOTE_PATH_MARKER}{}\n{WINDOWS_REMOTE_PATH_MARKER}{}\n",
            base64::engine::general_purpose::STANDARD.encode(path),
            base64::engine::general_purpose::STANDARD.encode(active),
            base64::engine::general_purpose::STANDARD.encode(active),
        );
        let candidates = windows_remote_binary_candidates(&remote_herdr, &stdout);
        assert_eq!(candidates.len(), 2);
        assert_eq!(
            candidates[0].executable,
            RemoteExecutable::WindowsPath(path.to_string())
        );
        assert_eq!(
            candidates[1].executable,
            RemoteExecutable::WindowsPath(active.to_string())
        );

        let command = decode_windows_command(&windows_remote_binary_candidate_command());
        assert!(command.contains("Get-Command hicodeer.exe"));
        assert!(command.contains("$activeJunction.Target"));
    }

    #[cfg(windows)]
    #[test]
    fn windows_process_candidates_require_a_verified_current_user_owner() {
        // Supply ownership results without needing another Windows account.
        // Execute the real discovery script, including its path validation.
        let fixture = r#"
function Get-Command {}
function Get-Item {}
function Get-CimInstance {
    $sid = [System.Security.Principal.WindowsIdentity]::GetCurrent().User.Value
    [pscustomobject]@{ ExecutablePath=$env:HICODEER_TEST_EXE; OwnerSid=$sid; OwnerResult=0 }
    [pscustomobject]@{ ExecutablePath="$env:SystemRoot\System32\cmd.exe"; OwnerSid='S-1-0-0'; OwnerResult=0 }
    [pscustomobject]@{ ExecutablePath="$PSHOME\powershell.exe"; OwnerSid=$sid; OwnerResult=2 }
}
function Invoke-CimMethod {
    param($InputObject)
    [pscustomobject]@{ ReturnValue=$InputObject.OwnerResult; Sid=$InputObject.OwnerSid }
}
function Get-Process {
    Get-CimInstance | ForEach-Object { [pscustomobject]@{ Path=$_.ExecutablePath } }
}
"#;
        let executable = std::env::current_exe().unwrap();
        let script = format!(
            "{fixture}\n{}",
            decode_windows_command(&windows_remote_binary_candidate_command())
        );
        let encoded = windows_powershell_script_command(&script);
        let mut command = Command::new("powershell.exe");
        command
            .args(["-NoProfile", "-NonInteractive", "-EncodedCommand"])
            .arg(encoded.split_whitespace().last().unwrap())
            .env("HICODEER_TEST_EXE", &executable);
        crate::platform::configure_background_command(&mut command);
        let output = command.output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let paths = String::from_utf8(output.stdout)
            .unwrap()
            .lines()
            .filter_map(|line| line.strip_prefix(WINDOWS_REMOTE_PATH_MARKER))
            .map(|path| decode_windows_remote_path(path).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(paths, [executable.to_string_lossy().into_owned()]);
    }

    #[test]
    fn remote_output_framing_discards_any_banner_and_preserves_binary() {
        let payload = [0, 1, 2, 0xff, b'\n'];
        let mut input = vec![b'x'; 4 * 1024 * 1024];
        input.extend_from_slice(b"\r\nhicodeer-remote-output-ready:1\r\n");
        input.extend_from_slice(&payload);
        let mut reader = io::BufReader::with_capacity(17, io::Cursor::new(input));

        discard_remote_output_preamble(&mut reader).unwrap();
        let mut output = Vec::new();
        io::Read::read_to_end(&mut reader, &mut output).unwrap();
        assert_eq!(output, payload);

        let mut missing = b"profile output without marker".to_vec();
        assert!(normalize_remote_stdout(&mut missing, true).is_err());
        normalize_remote_stdout(&mut missing, false).unwrap();
        assert_eq!(missing, b"profile output without marker");

        let mut platform = b"profile output\nhicodeer-remote-output-ready:1\nLinux\nx86_64\n".to_vec();
        normalize_remote_stdout(&mut platform, true).unwrap();
        let platform = String::from_utf8(platform).unwrap();
        let mut lines = platform.lines();
        assert_eq!(
            RemotePlatform::from_uname(lines.next().unwrap(), lines.next().unwrap()),
            Some(RemotePlatform {
                os: "linux",
                arch: "x86_64"
            })
        );
    }

    #[cfg(unix)]
    #[test]
    fn reattach_command_includes_remote_and_session() {
        assert_eq!(
            reattach_command(
                "target/release/herdr",
                "user@host",
                "work",
                RemoteKeybindings::Local,
                false,
            ),
            "target/release/herdr --remote user@host --session work"
        );
        assert_eq!(
            reattach_command(
                "herdr",
                "host name",
                crate::session::DEFAULT_SESSION_NAME,
                RemoteKeybindings::Local,
                false,
            ),
            "hicodeerd --remote 'host name'"
        );
        assert_eq!(
            reattach_command(
                "herdr",
                "host",
                crate::session::DEFAULT_SESSION_NAME,
                RemoteKeybindings::Server,
                false,
            ),
            "hicodeerd --remote host --remote-keybindings server"
        );
        assert_eq!(
            reattach_command(
                "herdr",
                "host",
                crate::session::DEFAULT_SESSION_NAME,
                RemoteKeybindings::Local,
                true,
            ),
            "hicodeerd --remote host --handoff"
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows_reattach_command_uses_current_executable() {
        let executable = std::env::current_exe().expect("current test executable");
        assert_eq!(
            reattach_command(
                r"C:\Program Files\HiCodeer\hicodeer.exe",
                "host'name",
                "work'name",
                RemoteKeybindings::Local,
                false,
            ),
            format!(
                "& '{}' --remote 'host''name' --session 'work''name'",
                executable.display().to_string().replace('\'', "''")
            )
        );
    }

    #[test]
    fn remote_api_bridge_always_selects_the_saved_session() {
        let remote_herdr = RemoteHerdr::for_platform(RemotePlatform {
            os: "linux",
            arch: "x86_64",
        });
        for session in ["default", "agents"] {
            assert_eq!(
                remote_api_bridge_command(&remote_herdr, session, false),
                posix_remote_output_command(&format!(
                    "exec \"$HOME/.local/bin/hicodeer\" --session {session} remote-api-bridge"
                ))
            );
        }
    }

    #[test]
    fn remote_bridge_idle_timeout_requires_explicit_support_and_opt_in() {
        let legacy = parse_client_status_json(r#"{"endpoint_protocol_generation":1}"#).unwrap();
        assert!(!legacy.remote_bridge_idle_timeout);
        let current = parse_client_status_json(
            r#"{"endpoint_protocol_generation":1,"remote_bridge_idle_timeout":true}"#,
        )
        .unwrap();
        assert!(current.remote_bridge_idle_timeout);
        let remote = RemoteHerdr::for_platform(RemotePlatform {
            os: "linux",
            arch: "x86_64",
        });
        assert!(remote
            .executable
            .bridge_command_with_idle_timeout("agents", true)
            .ends_with(" --session agents remote-client-bridge --idle-timeout-v1"));
    }

    #[test]
    fn remote_bridge_command_uses_installed_binary() {
        let remote_herdr = RemoteHerdr::for_platform(RemotePlatform {
            os: "linux",
            arch: "x86_64",
        });
        assert_eq!(
            remote_herdr
                .executable
                .bridge_command(crate::session::DEFAULT_SESSION_NAME),
            "printf '\n%s\n' 'hicodeer-remote-output-ready:1'\nexec \"$HOME/.local/bin/hicodeer\" remote-client-bridge"
        );
        assert_eq!(
            remote_herdr.executable.saved_bridge_command("agents"),
            "exec \"$HOME/.local/bin/hicodeer\" --session agents remote-client-bridge </dev/null"
        );
    }

    #[test]
    fn remote_path_discovery_uses_path_binary() {
        let remote_herdr = RemoteHerdr::for_platform(RemotePlatform {
            os: "linux",
            arch: "x86_64",
        });
        let remote_herdr = remote_herdr_from_path_discovery(&remote_herdr, "/usr/bin/hicodeer\n")
            .expect("path binary");

        assert_eq!(
            remote_herdr
                .executable
                .bridge_command(crate::session::DEFAULT_SESSION_NAME),
            "printf '\n%s\n' 'hicodeer-remote-output-ready:1'\nexec /usr/bin/hicodeer remote-client-bridge"
        );
    }

    #[test]
    fn remote_path_discovery_quotes_discovered_binary() {
        let remote_herdr = RemoteHerdr::for_platform(RemotePlatform {
            os: "linux",
            arch: "x86_64",
        });
        let remote_herdr =
            remote_herdr_from_path_discovery(&remote_herdr, "/opt/hicodeer bin/hicodeer\n")
                .expect("path binary");

        assert_eq!(
            remote_herdr
                .executable
                .bridge_command(crate::session::DEFAULT_SESSION_NAME),
            "printf '\n%s\n' 'hicodeer-remote-output-ready:1'\nexec '/opt/hicodeer bin/hicodeer' remote-client-bridge"
        );
    }

    #[test]
    fn remote_path_discovery_uses_macos_path_binary() {
        let remote_herdr = RemoteHerdr::for_platform(RemotePlatform {
            os: "macos",
            arch: "aarch64",
        });
        let remote_herdr =
            remote_herdr_from_path_discovery(&remote_herdr, "/opt/homebrew/bin/hicodeer\n")
                .expect("path binary");

        assert_eq!(
            remote_herdr
                .executable
                .bridge_command(crate::session::DEFAULT_SESSION_NAME),
            "printf '\n%s\n' 'hicodeer-remote-output-ready:1'\nexec /opt/homebrew/bin/hicodeer remote-client-bridge"
        );
        assert_eq!(remote_herdr.platform.os, "macos");
    }

    #[test]
    fn remote_path_discovery_reads_multiple_absolute_paths() {
        let remote_herdr = RemoteHerdr::for_platform(RemotePlatform {
            os: "linux",
            arch: "x86_64",
        });
        let candidates = remote_herdrs_from_path_discovery(
            &remote_herdr,
            "/usr/bin/hicodeer\nbin/hicodeer\n /opt/hicodeer bin/hicodeer\n",
        );

        assert_eq!(candidates.len(), 2);
        assert_eq!(
            candidates[0].executable,
            RemoteExecutable::PosixShellPath("/usr/bin/hicodeer".to_string())
        );
        assert_eq!(
            candidates[1].executable,
            RemoteExecutable::PosixShellPath("'/opt/hicodeer bin/hicodeer'".to_string())
        );
    }

    #[test]
    fn remote_path_discovery_ignores_mise_shims() {
        let remote_herdr = RemoteHerdr::for_platform(RemotePlatform {
            os: "linux",
            arch: "x86_64",
        });
        let candidates = remote_herdrs_from_path_discovery(
            &remote_herdr,
            "/home/can/.local/share/mise/shims/hicodeer\n/home/can/.local/bin/hicodeer\n",
        );

        assert_eq!(candidates.len(), 1);
        assert_eq!(
            candidates[0].executable,
            RemoteExecutable::PosixShellPath(
                "/home/can/.local/bin/hicodeer".to_string()
            )
        );
    }

    #[test]
    fn known_remote_binary_candidate_script_covers_local_and_nix_paths() {
        let script = known_remote_binary_candidate_script(&RemotePlatform {
            os: "linux",
            arch: "x86_64",
        });

        assert!(script.contains("emit \"$home/.local/bin/hicodeer\""));
        assert!(script.contains("emit \"/usr/local/bin/hicodeer\""));
        assert!(script.contains("emit \"$home/.nix-profile/bin/hicodeer\""));
        assert!(script.contains("emit \"/etc/profiles/per-user/$user/bin/hicodeer\""));
        assert!(script.contains("emit \"/run/current-system/sw/bin/hicodeer\""));
        assert!(script.contains("emit \"/home/linuxbrew/.linuxbrew/bin/hicodeer\""));
        assert!(!script.contains("emit \"/opt/homebrew/bin/hicodeer\""));
        assert!(!script.contains("herdr"));
    }

    #[test]
    fn known_remote_binary_candidate_script_includes_macos_homebrew_paths() {
        let script = known_remote_binary_candidate_script(&RemotePlatform {
            os: "macos",
            arch: "aarch64",
        });

        assert!(script.contains("emit \"/opt/homebrew/bin/hicodeer\""));
        assert!(script.contains("emit \"/usr/local/bin/hicodeer\""));
        assert!(!script.contains("emit \"/home/linuxbrew/.linuxbrew/bin/hicodeer\""));
    }

    #[test]
    fn remote_path_discovery_quotes_single_quotes_in_discovered_binary() {
        let remote_herdr = RemoteHerdr::for_platform(RemotePlatform {
            os: "linux",
            arch: "x86_64",
        });
        let remote_herdr =
            remote_herdr_from_path_discovery(&remote_herdr, "/opt/hicodeer's/bin/hicodeer\n")
                .expect("path binary");

        assert_eq!(
            remote_herdr
                .executable
                .bridge_command(crate::session::DEFAULT_SESSION_NAME),
            "printf '\n%s\n' 'hicodeer-remote-output-ready:1'\nexec '/opt/hicodeer'\\''s/bin/hicodeer' remote-client-bridge"
        );
    }

    #[test]
    fn remote_path_discovery_ignores_relative_paths() {
        let remote_herdr = RemoteHerdr::for_platform(RemotePlatform {
            os: "linux",
            arch: "x86_64",
        });
        let remote_herdr = remote_herdr_from_path_discovery(&remote_herdr, "bin/hicodeer\n");

        assert!(remote_herdr.is_none());
    }

    #[test]
    fn remote_path_discovery_ignores_empty_output() {
        let remote_herdr = RemoteHerdr::for_platform(RemotePlatform {
            os: "linux",
            arch: "x86_64",
        });
        let remote_herdr = remote_herdr_from_path_discovery(&remote_herdr, "\n");

        assert!(remote_herdr.is_none());
    }

    #[test]
    fn parse_client_status_json_reads_last_json_record() {
        let status = parse_client_status_json(
            "wrapper output\n{\"version\":\"0.8.0\",\"protocol\":20,\"endpoint_protocol_generation\":1,\"endpoint_capabilities\":[\"surface_interest\",\"health_check\"]}\n{\"wrapper\":true}\n",
        )
        .unwrap();
        assert_eq!(status.version.as_deref(), Some("0.8.0"));
        assert_eq!(status.protocol, Some(20));
        assert_eq!(status.endpoint_protocol_generation, Some(1));
        assert_eq!(
            status.endpoint_capabilities,
            vec!["surface_interest", "health_check"]
        );
        assert!(
            parse_client_status_json(r#"{"endpoint_protocol_generation":"unknown"}"#).is_none()
        );
    }

    #[test]
    fn windows_reuse_requires_explicit_remote_host_bridge() {
        let windows = RemotePlatform {
            os: "windows",
            arch: "x86_64",
        };
        let linux = RemotePlatform {
            os: "linux",
            arch: "x86_64",
        };
        let old =
            parse_client_status_json(r#"{"version":"0.8.2","endpoint_protocol_generation":1}"#)
                .unwrap();
        let current = parse_client_status_json(
            r#"{"version":"0.9.0","endpoint_protocol_generation":1,"remote_host_bridge":true}"#,
        )
        .unwrap();

        assert!(!old.supports_endpoint_requirement(&windows, false));
        assert!(current.supports_endpoint_requirement(&windows, false));
        assert!(old.supports_endpoint_requirement(&linux, false));
    }

    #[test]
    fn parse_remote_server_status_json_reads_running_server() {
        assert_eq!(
            parse_remote_server_status_json(
                r#"{"status":"running","running":true,"version":"0.6.0","protocol":8,"capabilities":{"live_handoff":true,"detached_server_daemon":true,"endpoint_protocol_generation":1,"surface_interest":true,"health_check":true}}"#
            )
            .unwrap(),
            RemoteServerStatus::Running {
                version: Some("0.6.0".into()),
                endpoint_protocol_generation: Some(1),
                surface_interest: true,
                health_check: true,
                live_handoff: true,
                detached_server_daemon: true
            }
        );
    }

    #[test]
    fn parse_remote_server_status_json_treats_missing_capability_as_old_server() {
        assert_eq!(
            parse_remote_server_status_json(
                r#"{"status":"running","running":true,"version":"0.6.0","protocol":8}"#
            )
            .unwrap(),
            RemoteServerStatus::Running {
                version: Some("0.6.0".into()),
                endpoint_protocol_generation: None,
                surface_interest: false,
                health_check: false,
                live_handoff: false,
                detached_server_daemon: false
            }
        );
    }

    #[test]
    fn parse_remote_server_status_json_reads_stopped_server() {
        assert_eq!(
            parse_remote_server_status_json(
                r#"{"status":"not_running","running":false,"version":null,"protocol":null}"#
            )
            .unwrap(),
            RemoteServerStatus::NotRunning
        );
    }

    #[test]
    fn remote_live_handoff_uses_prepared_binary_identity() {
        let remote_herdr = RemoteHerdr::for_platform(RemotePlatform {
            os: "linux",
            arch: "x86_64",
        });
        let command = remote_herdr.executable.live_handoff_command(
            crate::session::DEFAULT_SESSION_NAME,
            19,
            "0.7.9",
        );
        assert!(command.contains("--expected-protocol 19"));
        assert!(command.contains("--expected-version 0.7.9"));
        assert!(!command.contains(&format!(
            "--expected-protocol {} --expected-version {}",
            crate::protocol::PROTOCOL_VERSION,
            current_version()
        )));
    }

    #[cfg(unix)]
    fn remote_env_lock() -> &'static std::sync::Mutex<()> {
        static LOCK: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
        LOCK.get_or_init(|| std::sync::Mutex::new(()))
    }

    #[cfg(unix)]
    fn socket_path_byte_len(path: &Path) -> usize {
        use std::os::unix::ffi::OsStrExt;
        path.as_os_str().as_bytes().len()
    }

    #[cfg(unix)]
    #[test]
    fn local_forward_socket_path_uses_readable_name_when_it_fits() {
        let _guard = remote_env_lock().lock().unwrap();
        // Short target + session leave plenty of room — keep the human-
        // readable form so the socket path stays grep-friendly.
        let path = local_forward_socket_path("dev", "default");
        let filename = path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_string();
        assert!(
            filename.starts_with("hicodeer-remote-"),
            "expected readable name, got {filename}"
        );
        assert!(filename.contains("-dev-default."), "got {filename}");
        assert!(
            fits_unix_socket_path(&path),
            "socket path too long: {} ({} bytes)",
            path.display(),
            socket_path_byte_len(&path)
        );
    }

    #[cfg(unix)]
    #[test]
    fn local_forward_socket_path_fits_in_sun_path() {
        let _guard = remote_env_lock().lock().unwrap();
        // Worst case for the readable form: macOS-style 49-char TMPDIR +
        // max-length sanitized components. Should fall back to the hashed
        // short name, which fits under TMPDIR.
        let target = "longish-host.example.com";
        let session = "a-fairly-long-session-name-here";
        let path = local_forward_socket_path(target, session);
        assert!(
            fits_unix_socket_path(&path),
            "socket path too long for sun_path: {} ({} bytes)",
            path.display(),
            socket_path_byte_len(&path)
        );
    }

    #[cfg(unix)]
    #[test]
    fn local_forward_socket_path_falls_back_to_tmp_when_dir_is_long() {
        let _guard = remote_env_lock().lock().unwrap();
        // Force a TMPDIR long enough that even the hashed short name cannot
        // fit inside it. The fallback should drop to /tmp.
        let prior = std::env::var_os("TMPDIR");
        let long_dir = std::env::temp_dir().join("a".repeat(80));
        let _ = fs::create_dir_all(&long_dir);
        std::env::set_var("TMPDIR", &long_dir);

        let path = local_forward_socket_path("longish-host.example.com", "default");
        let fits = fits_unix_socket_path(&path);
        let parent = path.parent().map(Path::to_path_buf);
        let filename = path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_string();

        match prior {
            Some(v) => std::env::set_var("TMPDIR", v),
            None => std::env::remove_var("TMPDIR"),
        }
        let _ = fs::remove_dir_all(&long_dir);

        assert!(fits, "fallback path still overflows: {}", path.display());
        assert_eq!(parent.as_deref(), Some(Path::new("/tmp")));
        assert!(
            filename.starts_with("hicodeer-r-"),
            "expected hashed fallback, got {filename}"
        );
    }

}
