//! Interactive shell (pty) sessions against the daemon backend.
//!
//! A terminal that wants an interactive shell asks the backend for a pty and
//! then execs a command on it; the daemon runs that command on a backend-side
//! openpty (`/usr/bin/zsh` on the device). This module opens the channel,
//! requests the pty, execs the command, and relays the master data to the caller
//! through socketpairs (mirroring the exec `RemoteChild` plumbing): shell output
//! arrives on `stdout`, keystrokes go to `stdin`, and `resize` forwards window
//! changes over SSH so full-screen programs keep their layout.

use std::io::Cursor;
use std::os::unix::net::UnixStream;

use russh::ChannelMsg;
use smol::io::{AsyncRead, AsyncWrite};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::connection::SshSession;

/// Terminal type advertised in the pty request; Zed's terminal emulates it.
const TERM_TYPE: &str = "xterm-256color";
/// Names of the two requests a pty session is made of, in the order they are
/// sent and answered: the pty request first, then the exec. They are used to
/// report which one the daemon answered.
const PTY_REQUEST: &str = "pty";
const EXEC_REQUEST: &str = "exec";
/// Bytes read from the socketpair per relay iteration.
const PTY_CHUNK: usize = 8192;

/// A live interactive shell session on a remote pty.
pub struct RemotePty {
    /// Caller-side stdin: write keystrokes here.
    pub stdin: Option<Box<dyn AsyncWrite + Unpin + Send>>,
    /// Caller-side stdout: shell output arrives here.
    pub stdout: Option<Box<dyn AsyncRead + Unpin + Send>>,
    resize_tx: tokio::sync::mpsc::UnboundedSender<(u32, u32)>,
}

impl RemotePty {
    /// Forwards a terminal resize to the remote shell.
    pub fn resize(&self, cols: u32, rows: u32) {
        // A failed send means the relay task already ended (the pty session
        // closed); a resize arriving after that is a benign race and is dropped.
        if self.resize_tx.send((cols, rows)).is_err() {
            log::debug!("hishell pty: resize after the relay task ended");
        }
    }

    /// Returns a cheap cloneable handle for forwarding resizes from a thread
    /// that does not own the session.
    pub fn resize_handle(&self) -> ResizeHandle {
        ResizeHandle {
            tx: self.resize_tx.clone(),
        }
    }
}

/// Cloneable resize forwarder for a live remote pty.
#[derive(Clone)]
pub struct ResizeHandle {
    tx: tokio::sync::mpsc::UnboundedSender<(u32, u32)>,
}

impl ResizeHandle {
    /// Forwards a terminal resize to the remote shell.
    pub fn resize(&self, cols: u32, rows: u32) {
        // See `RemotePty::resize`: a failed send means the relay task ended.
        if self.tx.send((cols, rows)).is_err() {
            log::debug!("hishell pty: resize after the relay task ended");
        }
    }
}

/// Builds the shell command run on the backend pty: enter the caller's
/// directory when it exists there (best effort: a stale path must not block the
/// shell), then replace the wrapper shell with the program and arguments the
/// caller specified. Nothing is substituted: whatever the caller asks for is
/// what the pty runs. An empty program is refused rather than defaulted, so a
/// caller that names no shell is told instead of silently getting one.
pub(crate) fn shell_command(
    program: &str,
    args: &[String],
    cwd: Option<&str>,
) -> std::io::Result<String> {
    if program.is_empty() {
        log::error!("hishell pty: shell_command called with an empty program");
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "pty shell: caller specified no program",
        ));
    }
    let mut command = vec![crate::command::sh_quote(program)];
    command.extend(args.iter().map(|arg| crate::command::sh_quote(arg)));
    let exec = format!("exec {}", command.join(" "));
    Ok(match cwd.filter(|dir| !dir.is_empty()) {
        Some(dir) => format!("cd {} 2>/dev/null; {exec}", crate::command::sh_quote(dir)),
        None => exec,
    })
}

/// Opens an interactive shell channel on `conn`: pty request, then `command`
/// exec (the caller passes `exec <program> <args>`, optionally preceded by a `cd`). Both
/// steps are accepted by the daemon before this future resolves, so a backend
/// that cannot serve a pty surfaces an error and the caller falls back to a
/// local shell, and the channel is ready for input the moment the caller has
/// something to write. The relay task then runs on the current tokio runtime
/// until the channel closes.
pub(crate) async fn open_shell_pty(
    conn: SshSession,
    cols: u32,
    rows: u32,
    command: &str,
) -> std::io::Result<RemotePty> {
    let mut channel = conn
        .channel_open_session()
        .await
        .map_err(|err| std::io::Error::other(format!("open channel: {err}")))?;
    channel
        .request_pty(true, TERM_TYPE, cols, rows, 0, 0, &[])
        .await
        .map_err(|err| std::io::Error::other(format!("request pty: {err}")))?;
    channel
        .exec(true, command)
        .await
        .map_err(|err| std::io::Error::other(format!("exec shell: {err}")))?;

    // Both requests asked the daemon to answer, and the answers are what make
    // this channel usable. The daemon registers its pty master for the channel
    // as part of taking the exec request and answers that request only after it
    // has, so until the second answer arrives the channel has no pty session yet:
    // input sent in that window is routed to the stdin pipe of an exec child that
    // does not exist, and is dropped. The caller's first write is the terminal's
    // bootstrap command, so a session that sends it before the answer arrives is
    // left on a shell that never received its command -- which the terminal shows
    // as a tab stuck on `Starting zsh` with no prompt.
    for request in [PTY_REQUEST, EXEC_REQUEST] {
        match channel.wait().await {
            Some(ChannelMsg::Success) => {}
            Some(ChannelMsg::Failure) => {
                log::error!("hishell pty: the daemon refused the {request} request");
                return Err(std::io::Error::other(format!(
                    "the daemon refused the {request} request"
                )));
            }
            Some(other) => {
                log::error!(
                    "hishell pty: the daemon answered the {request} request with {other:?}"
                );
                return Err(std::io::Error::other(format!(
                    "the daemon answered the {request} request with {other:?}"
                )));
            }
            None => {
                log::error!(
                    "hishell pty: the channel closed while the daemon was answering the {request} \
                     request"
                );
                return Err(std::io::Error::other(format!(
                    "the channel closed while the daemon was answering the {request} request"
                )));
            }
        }
    }

    let (stdout_reader, stdout_pump) = UnixStream::pair()?;
    let (stdin_pump, stdin_writer) = UnixStream::pair()?;

    let stdout: Box<dyn AsyncRead + Unpin + Send> = Box::new(smol::Async::new(stdout_reader)?);
    let stdin: Box<dyn AsyncWrite + Unpin + Send> = Box::new(smol::Async::new(stdin_writer)?);

    let (resize_tx, mut resize_rx) = tokio::sync::mpsc::unbounded_channel::<(u32, u32)>();

    tokio::spawn(async move {
        for s in [&stdout_pump, &stdin_pump] {
            if let Err(err) = s.set_nonblocking(true) {
                log::error!("hishell pty: set nonblocking: {err}");
                return;
            }
        }
        let mut stdout_w = match tokio::net::UnixStream::from_std(stdout_pump) {
            Ok(s) => s,
            Err(err) => {
                log::error!("hishell pty: wrap stdout: {err}");
                return;
            }
        };
        let mut stdin_r = match tokio::net::UnixStream::from_std(stdin_pump) {
            Ok(s) => s,
            Err(err) => {
                log::error!("hishell pty: wrap stdin: {err}");
                return;
            }
        };

        let mut buf = [0u8; PTY_CHUNK];
        loop {
            tokio::select! {
                msg = channel.wait() => {
                    match msg {
                        Some(ChannelMsg::Data { data }) => {
                            if stdout_w.write_all(&data).await.is_err() {
                                log::warn!("hishell pty: write stdout failed");
                                break;
                            }
                        }
                        Some(ChannelMsg::ExtendedData { data, .. }) => {
                            if stdout_w.write_all(&data).await.is_err() {
                                log::warn!("hishell pty: write stderr-as-out failed");
                                break;
                            }
                        }
                        Some(ChannelMsg::Close) | None => break,
                        Some(_) => {}
                    }
                }
                n = stdin_r.read(&mut buf) => {
                    match n {
                        Ok(0) => break,
                        Ok(n) => {
                            if channel.data(Cursor::new(&buf[..n])).await.is_err() {
                                log::warn!("hishell pty: send input failed");
                                break;
                            }
                        }
                        Err(err) => {
                            log::warn!("hishell pty: read stdin: {err}");
                            break;
                        }
                    }
                }
                resize = resize_rx.recv() => {
                    match resize {
                        Some((cols, rows)) => {
                            if channel.window_change(cols, rows, 0, 0).await.is_err() {
                                log::warn!("hishell pty: window_change failed");
                            }
                        }
                        None => break,
                    }
                }
            }
        }
        // Local socketpair end: a shutdown failure here is unexpected and is
        // reported rather than swallowed.
        if let Err(err) = stdout_w.shutdown().await {
            log::warn!("hishell pty: stdout shutdown: {err}");
        }
    });

    Ok(RemotePty {
        stdin: Some(stdin),
        stdout: Some(stdout),
        resize_tx,
    })
}
