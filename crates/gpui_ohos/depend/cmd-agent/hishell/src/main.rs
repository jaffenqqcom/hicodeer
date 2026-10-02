//! hishell: the local `$SHELL` of the HiCodeer terminal, bridged to hicodeerd.
//!
//! The host application runs in a sandbox that cannot exec programs installed by
//! the system command-line tools. hicodeerd runs outside that sandbox, so this
//! binary opens an interactive pty on it and connects the local terminal to that
//! pty: what is typed here reaches the shell, what the shell prints reaches the
//! terminal, and window changes are forwarded. The session therefore runs with
//! hicodeerd's permissions, which is how a sandboxed session reaches the
//! system's own programs.
//!
//! The management keys are compiled in (see `hishell::keys`), so the bridge
//! needs no configuration. Without hicodeerd the bridge reports the failure and
//! then replaces itself with the local `/bin/sh`, so a terminal started before
//! the daemon still gets a shell.

mod logger;

use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::process::CommandExt;
use std::sync::atomic::{AtomicI32, Ordering};
use std::time::Duration;

use hishell::{
    CommandEndpoint, ExecSpec, FdMode, RemoteCommandExecutor, RemotePty, SshCommandExecutor,
};
use smol::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

/// Program hicodeerd runs on the pty. Absolute, so hicodeerd's own PATH cannot
/// decide which shell the bridge ends up talking to.
const SHELL_PROGRAM: &str = "/usr/bin/zsh";
/// Shell bundled in the system, at the path this sandbox may exec. Used when
/// hicodeerd is not running, so the caller still ends up with a shell.
const FALLBACK_SHELL_PATH: &str = "/bin/sh";
/// Arguments used when the caller passed none, which is the case when hishell is
/// run by hand or started by the terminal as its shell. They are zsh options.
const DEFAULT_SHELL_ARGS: &[&str] = &["-g"];
/// Reported when hicodeerd cannot be reached. Kept verbatim: it is the only
/// thing that tells the user how to get the privileged session back.
pub(crate) const DAEMON_NOT_READY_MESSAGE: &str =
    "hicodeerd is not reachable，run hicodeerd in system terminal to run full terminal。未能连接 hicodeerd，在系统命令行终端运行hicodeerd，以获取完整的命令行功能";
/// How long to wait for the management handshake before giving up. A daemon that
/// is not running is reported at once, because the connection is refused; this
/// only bounds a listener that accepts and then never answers.
pub(crate) const READY_TIMEOUT: Duration = Duration::from_secs(10);
/// Backstop interval for re-reading the local window size when no `SIGWINCH`
/// has arrived. `SIGWINCH` covers every ordinary window change, so this only
/// guards against a missed signal; it replaces the 5 Hz poll that used to run
/// for the whole session.
const RESIZE_BACKSTOP: Duration = Duration::from_secs(5);
/// Terminal size assumed when the local terminal reports none (not a terminal,
/// or a zero-sized window).
const FALLBACK_COLS: u32 = 80;
const FALLBACK_ROWS: u32 = 24;
/// Size the host application opens the local pty with before it has laid the
/// terminal out, derived from the debug bounds in `crates/terminal/src/terminal.rs`:
/// `DEBUG_TERMINAL_WIDTH / DEBUG_CELL_WIDTH` = 500 / 5 = 100 columns and
/// `DEBUG_TERMINAL_HEIGHT / DEBUG_LINE_HEIGHT` = 30 / 5 = 6 rows. A local size
/// equal to this is treated as "the layout has not run yet", not as a size the
/// user asked for; see `settled_local_size`.
const PRE_LAYOUT_COLS: u32 = 100;
const PRE_LAYOUT_ROWS: u32 = 6;
/// How often the local size is re-read while it still looks like the pre-layout
/// debug size. A short interval, never a busy loop.
const SIZE_SETTLE_POLL: Duration = Duration::from_millis(20);
/// How long the local size is given to settle before the last read is used
/// anyway. Bounded so a terminal that really is 100x6, or one whose layout never
/// lands, still starts promptly instead of hanging.
const SIZE_SETTLE_TIMEOUT: Duration = Duration::from_millis(500);
/// Upper bound on the number of re-reads in `settled_local_size`. An attempt
/// count, not a deadline, so a read that is slow cannot extend the wait. It is
/// derived from the timeout and the poll interval so the two stay in step.
const SIZE_SETTLE_ATTEMPTS: u32 =
    (SIZE_SETTLE_TIMEOUT.as_millis() / SIZE_SETTLE_POLL.as_millis()) as u32;
/// Bytes moved per relay iteration.
const RELAY_CHUNK: usize = 8192;
/// Status this process exits with when a command ended without a status of its
/// own -- killed by a signal, or its session dropped. 128 is what a shell
/// reports for an unknown termination, and it is non-zero, so the caller does
/// not read the run as a success.
pub(crate) const EXIT_WITHOUT_STATUS: i32 = 128;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|arg| arg == "--help") {
        print_help();
        return;
    }
    let logging = args.iter().any(|arg| arg == "--log");
    logger::init(logging);
    finish(run(&args));
}

/// Reports how a run ended and exits with its status.
fn finish(result: Result<i32, String>) -> ! {
    match result {
        // The status is handed on rather than flattened: a caller that ran a
        // one-off command reads it to tell a command that ran from one that did
        // not, and an interactive session has no status of its own (0).
        Ok(code) => std::process::exit(code),
        Err(err) => {
            // The same reason the logger is always installed: on the device there
            // is no console this text reaches, so the failure has to be recorded
            // there too or nothing about it survives.
            log::error!("hishell: {err}");
            // This is the whole output of a failed run, so it prints with or
            // without --log.
            eprintln!("hishell: {err}");
            std::process::exit(1);
        }
    }
}

/// Prints what this binary accepts. Runs before a logger is installed, so
/// `--help` reads the same whatever else is on the command line.
fn print_help() {
    println!("hishell - run an interactive shell on hicodeerd from this terminal");
    println!();
    println!("Usage: hishell [--log] [--help]");
    println!();
    println!("Opens {SHELL_PROGRAM} on the running hicodeerd and connects it to the current");
    println!("terminal, or runs one `-c` command on that shell and hands back what it printed");
    println!("and the status it exited with. Without hicodeerd the bridge prints the failure and");
    println!("replaces itself with {FALLBACK_SHELL_PATH} instead.");
    println!();
    println!("Options:");
    println!("  --log   Add debug-level diagnostics to hilog (on the device). Without");
    println!("          it the key nodes of a run are still recorded there.");
    println!("  --help  Print this help and exit.");
}

fn run(args: &[String]) -> Result<i32, String> {
    match requested_invocation(args) {
        Invocation::Interactive(requested) => run_interactive(&requested),
        Invocation::Command(shell_args) => run_command(&shell_args),
    }
}

/// Runs an interactive shell on hicodeerd and bridges this terminal to it.
fn run_interactive(requested: &RequestedShell) -> Result<i32, String> {
    // A terminal executor: this process runs exactly one pty for its whole life,
    // so it opens only the one connection the pty needs.
    let executor = match SshCommandExecutor::new(CommandEndpoint::ohos_default()) {
        Ok(executor) => executor,
        Err(err) => {
            // The terminal has to end up with a shell even when the bridge
            // cannot be built at all, the same as when the daemon is
            // unreachable. The zsh-specific default arguments are not handed to
            // the fallback: it is a plain sh, which would reject them.
            log::error!("hishell: cannot start the bridge: {err}");
            eprintln!("hishell: {DAEMON_NOT_READY_MESSAGE}");
            return exec_fallback_shell(requested.argv0.as_deref(), &[]);
        }
    };
    if let Err(err) = executor.wait_ready(READY_TIMEOUT) {
        log::error!("hishell: hicodeerd is not ready: {err}");
        // The terminal has to end up with a shell either way, so say why the
        // bridge is stepping aside before becoming that shell. The zsh-specific
        // default arguments are not handed to the fallback: it is a plain sh,
        // which would reject them.
        eprintln!("hishell: {DAEMON_NOT_READY_MESSAGE}");
        return exec_fallback_shell(requested.argv0.as_deref(), &[]);
    }

    // Read the terminal size as late as this side can, and only once it looks
    // settled. The host application lays the terminal out after it starts the
    // shell, and opens the local pty at its pre-layout debug size until then; a
    // shell started on that size draws its first prompt for the wrong width and
    // prints zsh's `PROMPT_EOL_MARK` (`%`) on a line of its own, too early for
    // the correction in `bridge` to reach it.
    let (cols, rows) = settled_local_size().unwrap_or((FALLBACK_COLS, FALLBACK_ROWS));

    // The caller starts this process in the directory its terminal should open
    // in -- the project (worktree) directory -- so the shell is asked to start
    // there too. Entering it is best effort: the daemon runs `cd <dir>
    // 2>/dev/null` before the `exec` (see `pty::shell_command`), so a path that
    // names no directory on the daemon's side -- the app's sandbox before the
    // user has picked a home directory -- leaves the shell in the daemon's own
    // directory instead of failing to start.
    let cwd = working_directory();
    let pty = smol::block_on(executor.open_shell_pty(
        cols,
        rows,
        cwd.as_deref(),
        SHELL_PROGRAM,
        &requested.args,
    ))
    .map_err(|err| format!("cannot open a shell on hicodeerd: {err}"))?;

    // Raw mode only once the shell is up: a failure above leaves the terminal
    // untouched, so the error message stays readable. Without raw mode the shell
    // still runs on the already-open pty, so the terminal is not left without
    // one; the guard restores the saved settings when `run_interactive` returns.
    let _raw = match RawMode::enable(libc::STDIN_FILENO) {
        Ok(raw) => Some(raw),
        Err(err) => {
            log::warn!("hishell: cannot put the terminal into raw mode: {err}");
            None
        }
    };
    bridge(pty, cols, rows).map(|()| 0)
}

/// Runs one command the caller asked for with `-c` on the same shell the
/// terminal runs, and returns the status this process should exit with.
///
/// The command is a shell's own: the caller reads what it printed, and its
/// status decides whether that output is usable (the terminal only accepts a
/// list of executables from a run that succeeded). Running it on an interactive
/// pty instead would corrupt both -- a pty turns every `\n` into `\r\n`, and a
/// failure to set up the terminal would be reported in place of the command's
/// own outcome.
fn run_command(shell_args: &[String]) -> Result<i32, String> {
    // The caller starts this process in the directory the command is meant to run
    // in -- the terminal's own working directory -- so the command is given the
    // same one. A child of the daemon would otherwise start in the daemon's own
    // directory, where a command like `git status` reads the wrong repository, or
    // none at all. The interactive bridge hands over its own directory the same
    // way, so the shell opens where the terminal does.
    let cwd = working_directory();

    // A terminal executor here too: this process serves one command and exits,
    // so it opens only the one connection the command needs.
    let executor = SshCommandExecutor::new(CommandEndpoint::ohos_default())
        .map_err(|err| format!("cannot start the bridge: {err}"))?;
    if let Err(err) = executor.wait_ready(READY_TIMEOUT) {
        log::error!("hishell: hicodeerd is not ready for a command: {err}");
        eprintln!("hishell: {DAEMON_NOT_READY_MESSAGE}");
        return exec_fallback_shell(None, shell_args);
    }

    let mut spec = ExecSpec::new(SHELL_PROGRAM);
    spec.args = shell_args.to_vec();
    spec.cwd_path = cwd;
    // The command is meant to see the environment its caller set up for it --
    // `PATH` above all, since it is what the command enumerates -- while the rest
    // of the environment stays the daemon's own. That is the one a program
    // outside the sandbox should start from: its `HOME` and the scratch
    // directory it is given per session are real paths there, while this side's
    // copies of those name sandbox paths (`TMPDIR` in particular, which the
    // daemon deliberately replaces per client).
    match std::env::var("PATH") {
        Ok(path) => {
            spec.env.insert("PATH".to_string(), path);
        }
        Err(err) => {
            log::warn!("hishell: no PATH to hand the command: {err}");
        }
    }
    // A `-c` run is given no input by its caller (the terminal runs these with a
    // null stdin), so the command is given none either.
    spec.stdin_mode = FdMode::Null;

    let mut child = executor
        .spawn(spec)
        .map_err(|err| format!("cannot run the command on hicodeerd: {err}"))?;
    let session_id = child.session_id;

    let copied: Result<(), String> = smol::block_on(async move {
        let mut child_stdout = child
            .stdout
            .take()
            .ok_or_else(|| "hicodeerd returned no command output stream".to_string())?;
        let mut child_stderr = child
            .stderr
            .take()
            .ok_or_else(|| "hicodeerd returned no command error stream".to_string())?;
        let mut local_stdout = smol::Unblock::new(std::io::stdout());
        let mut local_stderr = smol::Unblock::new(std::io::stderr());
        // Both streams are drained at once: waiting on one while the command
        // fills the other would block for good. The child -- and with it the
        // caller's end of its stdin -- is dropped when this block ends.
        let (stdout_done, stderr_done) = smol::future::zip(
            relay_to_local(&mut child_stdout, &mut local_stdout),
            relay_to_local(&mut child_stderr, &mut local_stderr),
        )
        .await;
        match (stdout_done, stderr_done) {
            (Err(err), _) | (_, Err(err)) => {
                Err(format!("cannot copy the command's output: {err}"))
            }
            (Ok(()), Ok(())) => Ok(()),
        }
    });
    copied?;

    let exit = smol::block_on(executor.wait_exit_async(session_id))
        .map_err(|err| format!("cannot read the command's exit status: {err}"))?;
    Ok(exit.unwrap_or(EXIT_WITHOUT_STATUS))
}

/// What the caller asked this process to do.
enum Invocation {
    /// Start an interactive shell, the way the terminal starts this bridge as
    /// its shell.
    Interactive(RequestedShell),
    /// Run one command and report what it printed and the status it exited
    /// with: the caller started this process the way it starts a shell for a
    /// one-off command (`-c` with a command of its own) and reads the result.
    Command(Vec<String>),
}

/// The shell invocation the caller asked hishell for.
struct RequestedShell {
    /// Name the shell reports as its own, which makes zsh a login shell. The
    /// terminal asks for `-zsh`. Only the local fallback can honour it: the
    /// hicodeerd path starts the shell through the daemon's `sh -c`, which has
    /// no way to set it.
    argv0: Option<String>,
    /// Arguments to hand the shell, exactly as the caller passed them.
    args: Vec<String>,
}

/// The invocation used when the caller passed none, i.e. when hishell is run by
/// hand or started by the terminal, which passes no shell arguments.
fn default_shell_invocation() -> RequestedShell {
    RequestedShell {
        argv0: None,
        args: DEFAULT_SHELL_ARGS.iter().map(|arg| arg.to_string()).collect(),
    }
}

/// Reads what to do out of `args`.
///
/// An interactive shell is asked for the way the terminal starts hishell:
/// `hishell -c "exec -a <argv0> '<program>' <args...>"`. Any other `-c` is a
/// command the caller wants run -- that is how the terminal runs a one-off
/// command -- and the invocation is a shell's own, so it is forwarded as it
/// stands. Any argument that is neither `-c` nor names a command (a login flag
/// such as `-l`, which the terminal may pass) is tolerated: it selects the
/// default zsh invocation. Nothing about the arguments is assumed: whatever the
/// caller passes is what the shell gets, on both the hicodeerd path and the
/// local fallback.
fn requested_invocation(args: &[String]) -> Invocation {
    let Some(command) = args
        .iter()
        .position(|arg| arg == "-c")
        .and_then(|index| args.get(index + 1))
    else {
        return Invocation::Interactive(default_shell_invocation());
    };

    let mut words = split_shell_words(command).into_iter();
    if words.next().as_deref() != Some("exec") {
        return Invocation::Command(forwarded_shell_args(args));
    }
    let mut words = words.peekable();
    let mut argv0 = None;
    if words.peek().map(String::as_str) == Some("-a") {
        words.next();
        argv0 = words.next();
    }
    // The next word is the program the terminal wanted to start, which is this
    // bridge itself. The shell is what has to run instead, so it is dropped
    // rather than forwarded.
    let program = words.next();
    if program.is_none() {
        log::warn!("hishell: the caller's shell invocation named no program");
    }
    let args: Vec<String> = words.collect();
    Invocation::Interactive(RequestedShell { argv0, args })
}

/// Every argument but this process's own, i.e. the whole invocation to hand the
/// shell. `--log` widens this process's logging and is not the shell's; `--help`
/// is answered before this point.
fn forwarded_shell_args(args: &[String]) -> Vec<String> {
    args.iter()
        .filter(|arg| arg.as_str() != "--log")
        .cloned()
        .collect()
}

/// Splits `command` into words the way the shell that generated it parses them:
/// whitespace separates words, quotes group them, and a backslash escapes the
/// next character.
///
/// The terminal builds its invocation as a command string for the shell's `-c`,
/// so recovering the arguments means re-reading that string under the same
/// rules. Only the quoting the terminal itself emits has to be understood.
fn split_shell_words(command: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut started = false;
    let mut characters = command.chars().peekable();
    while let Some(character) = characters.next() {
        match character {
            '\'' => {
                started = true;
                for quoted in characters.by_ref() {
                    if quoted == '\'' {
                        break;
                    }
                    word.push(quoted);
                }
            }
            '"' => {
                started = true;
                while let Some(quoted) = characters.next() {
                    if quoted == '"' {
                        break;
                    }
                    if quoted == '\\' {
                        if let Some(escaped) = characters.next() {
                            word.push(escaped);
                        }
                        continue;
                    }
                    word.push(quoted);
                }
            }
            '\\' => {
                started = true;
                if let Some(escaped) = characters.next() {
                    word.push(escaped);
                }
            }
            character if character.is_whitespace() => {
                if started {
                    words.push(std::mem::take(&mut word));
                    started = false;
                }
            }
            character => {
                started = true;
                word.push(character);
            }
        }
    }
    if started {
        words.push(word);
    }
    words
}

/// Replaces this process with the local shell running `args`, so the requested
/// shell or command still runs while hicodeerd is not running. Returns only when
/// the replacement failed, because `exec` does not come back on success.
fn exec_fallback_shell(argv0: Option<&str>, args: &[String]) -> Result<i32, String> {
    let mut command = std::process::Command::new(FALLBACK_SHELL_PATH);
    if let Some(argv0) = argv0 {
        command.arg0(argv0);
    }
    command.args(args);
    let err = command.exec();
    log::error!("hishell: exec {FALLBACK_SHELL_PATH} failed: {err}");
    Err(format!(
        "cannot start the fallback shell {FALLBACK_SHELL_PATH}: {err}"
    ))
}

/// Copies bytes in both directions between the local terminal and the remote
/// shell until either side closes, forwarding window changes as they happen.
///
/// `opened_cols`/`opened_rows` are the dimensions the remote pty was opened with.
fn bridge(mut pty: RemotePty, opened_cols: u32, opened_rows: u32) -> Result<(), String> {
    let mut remote_stdout = pty
        .stdout
        .take()
        .ok_or_else(|| "hicodeerd returned no shell output stream".to_string())?;
    let mut remote_stdin = pty
        .stdin
        .take()
        .ok_or_else(|| "hicodeerd accepted no shell input stream".to_string())?;
    // `pty` itself stays alive below: it owns the resize channel the relay task
    // reads from, and dropping it would end the relay.
    let resize = pty.resize_handle();

    // The local side is blocking stdio driven on a worker thread, so a read that
    // waits for a keystroke cannot stall the copy running the other way.
    let mut local_stdout = smol::Unblock::new(std::io::stdout());
    let mut local_stdin = smol::Unblock::new(std::io::stdin());

    // The size to compare against is the one the remote pty was opened with, not
    // a fresh read of the local terminal: the host application lays this terminal
    // out after it starts the shell, so it very often reports a different size by
    // the time the bridge runs. Comparing against a fresh read would treat that
    // size as the baseline and never forward it, leaving the shell on the stale
    // one -- and a shell whose width exceeds the terminal's own prints the
    // `PROMPT_EOL_MARK` (`%`) on a line of its own at every prompt. The first
    // check runs before the first wait so that size is sent before the shell
    // prints its first prompt.
    let (mut last_cols, mut last_rows) = (opened_cols, opened_rows);
    // Window changes arrive as SIGWINCH, so the size is read when the signal
    // fires instead of being polled for. The self-pipe makes the signal
    // awaitable: the handler only writes a byte, which is async-signal-safe, and
    // this task waits for that byte on the smol reactor.
    let mut resize_signals = sigwinch_reader()?;
    let resize_task = smol::spawn(async move {
        let mut signal_byte = [0u8; 1];
        loop {
            if let Some((cols, rows)) = local_size() {
                if cols != last_cols || rows != last_rows {
                    resize.resize(cols, rows);
                    last_cols = cols;
                    last_rows = rows;
                }
            }
            // Wait for a SIGWINCH (a byte through the self-pipe) or the backstop,
            // whichever comes first. The check above runs before the first wait,
            // so the size is sent before the shell prints its first prompt.
            smol::future::or(
                async {
                    if let Err(error) = resize_signals.read(&mut signal_byte).await {
                        log::warn!("hishell: reading the resize pipe failed: {error}");
                    }
                },
                async {
                    smol::Timer::after(RESIZE_BACKSTOP).await;
                },
            )
            .await;
        }
    });

    let outcome = smol::block_on(async {
        let outcome = smol::future::or(
            async {
                relay_to_local(&mut remote_stdout, &mut local_stdout)
                    .await
                    .map(|_| "the remote shell closed the session")
            },
            async {
                relay_to_remote(&mut local_stdin, &mut remote_stdin)
                    .await
                    .map(|_| "the local terminal closed its input")
            },
        )
        .await;
        // The resize task loops for as long as the bridge lives, so it is
        // cancelled rather than awaited to completion.
        let _ = resize_task.cancel().await;
        outcome
    });
    match outcome {
        Ok(_why) => Ok(()),
        Err(err) => Err(format!("the session ended with an error: {err}")),
    }
}

/// Write end of the self-pipe a `SIGWINCH` handler uses to wake the resize
/// task. Negative when no handler is installed.
static SIGWINCH_WRITE_FD: AtomicI32 = AtomicI32::new(-1);

/// `SIGWINCH` handler: stores one byte in the self-pipe. Writing to a pipe is
/// async-signal-safe, and that is the only work done here.
extern "C" fn on_sigwinch(_signal: libc::c_int) {
    let descriptor = SIGWINCH_WRITE_FD.load(Ordering::Relaxed);
    if descriptor >= 0 {
        let byte = 1u8;
        // SAFETY: `descriptor` is the write end stored before the handler was
        // installed, and writing a single byte to it is the sole action here.
        unsafe { libc::write(descriptor, &byte as *const u8 as *const libc::c_void, 1) };
    }
}

/// Installs a `SIGWINCH` handler and returns the readable end of the self-pipe
/// it writes to, so a window change can be awaited instead of polled for. The
/// write end is kept open for the life of the process: the handler reaches it
/// through `SIGWINCH_WRITE_FD`, not through a binding.
fn sigwinch_reader() -> Result<smol::Async<std::fs::File>, String> {
    let mut pipe: [libc::c_int; 2] = [0; 2];
    // SAFETY: pipe(2) fills both descriptors on success.
    if unsafe { libc::pipe(pipe.as_mut_ptr()) } != 0 {
        return Err(format!(
            "cannot create the resize pipe: {}",
            std::io::Error::last_os_error()
        ));
    }
    // SAFETY: pipe(2) returned two fresh descriptors that this process owns.
    let (read_end, write_end) =
        unsafe { (OwnedFd::from_raw_fd(pipe[0]), OwnedFd::from_raw_fd(pipe[1])) };
    SIGWINCH_WRITE_FD.store(write_end.as_raw_fd(), Ordering::Relaxed);
    // Make the write end non-blocking: the handler must never block if the pipe
    // is full, and a dropped notification only delays the next size check.
    // SAFETY: both fcntl calls take this descriptor and plain flag values.
    let flags = unsafe { libc::fcntl(write_end.as_raw_fd(), libc::F_GETFL) };
    if flags < 0 {
        log::warn!(
            "hishell: cannot read the resize pipe flags: {}",
            std::io::Error::last_os_error()
        );
    } else if unsafe {
        libc::fcntl(write_end.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK)
    } < 0
    {
        log::warn!(
            "hishell: cannot make the resize pipe non-blocking: {}",
            std::io::Error::last_os_error()
        );
    }
    // SAFETY: a zeroed sigaction is a valid starting point; only the handler and
    // the restart flag are set, and both are plain data.
    let mut action: libc::sigaction = unsafe { std::mem::zeroed() };
    action.sa_sigaction = on_sigwinch as extern "C" fn(libc::c_int) as usize;
    action.sa_flags = libc::SA_RESTART;
    // SAFETY: `action` is fully initialised above; the handler only performs an
    // atomic load and a write(2).
    if unsafe { libc::sigaction(libc::SIGWINCH, &action, std::ptr::null_mut()) } != 0 {
        SIGWINCH_WRITE_FD.store(-1, Ordering::Relaxed);
        return Err(format!(
            "cannot install the SIGWINCH handler: {}",
            std::io::Error::last_os_error()
        ));
    }
    // Keep the write end open for the whole process: the handler addresses it
    // through the static, never through a binding.
    std::mem::forget(write_end);
    // The readable end is wrapped in a `File` because smol's `Async` needs
    // `Read` for its `AsyncRead` impl, which a bare `OwnedFd` does not provide.
    smol::Async::new(std::fs::File::from(read_end))
        .map_err(|err| format!("cannot register the resize pipe: {err}"))
}

/// Copies remote shell output into the local terminal.
///
/// The flush after every chunk is required, not cosmetic: `std::io::stdout` is
/// line-buffered, so a prompt or a key echo (neither ends in a newline) would
/// otherwise sit in the user-space buffer and never reach the terminal.
pub(crate) async fn relay_to_local<R, W>(remote: &mut R, local: &mut W) -> std::io::Result<()>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let mut buf = [0u8; RELAY_CHUNK];
    loop {
        let n = remote.read(&mut buf).await?;
        if n == 0 {
            return Ok(());
        }
        local.write_all(&buf[..n]).await?;
        local.flush().await?;
    }
}

/// Copies the local terminal's input to the remote shell.
pub(crate) async fn relay_to_remote<R, W>(local: &mut R, remote: &mut W) -> std::io::Result<()>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let mut buf = [0u8; RELAY_CHUNK];
    loop {
        let n = local.read(&mut buf).await?;
        if n == 0 {
            return Ok(());
        }
        remote.write_all(&buf[..n]).await?;
        remote.flush().await?;
    }
}

/// Terminal raw mode, restored when dropped.
struct RawMode {
    fd: RawFd,
    saved: libc::termios,
}

impl RawMode {
    /// Puts `fd` into raw mode, saving the settings to restore on drop.
    fn enable(fd: RawFd) -> Result<Self, String> {
        let mut saved = std::mem::MaybeUninit::<libc::termios>::uninit();
        // SAFETY: tcgetattr fills the whole struct when it succeeds.
        if unsafe { libc::tcgetattr(fd, saved.as_mut_ptr()) } != 0 {
            return Err(format!(
                "cannot read the terminal settings: {}",
                std::io::Error::last_os_error()
            ));
        }
        // SAFETY: tcgetattr succeeded, so the struct is initialized.
        let saved = unsafe { saved.assume_init() };
        let mut raw = saved;
        // SAFETY: raw is a valid termios; cfmakeraw only writes into it.
        unsafe { libc::cfmakeraw(&mut raw) };
        // SAFETY: raw is a valid termios for this terminal fd.
        if unsafe { libc::tcsetattr(fd, libc::TCSANOW, &raw) } != 0 {
            return Err(format!(
                "cannot put the terminal into raw mode: {}",
                std::io::Error::last_os_error()
            ));
        }
        Ok(Self { fd, saved })
    }
}

impl Drop for RawMode {
    fn drop(&mut self) {
        // SAFETY: `saved` was read from this fd by tcgetattr.
        if unsafe { libc::tcsetattr(self.fd, libc::TCSANOW, &self.saved) } != 0 {
            log::warn!(
                "hishell: cannot restore the terminal settings: {}",
                std::io::Error::last_os_error()
            );
        }
    }
}

/// The directory this process was started in, or `None` when it cannot be named.
/// The protocol carries text, so a directory that is not valid text -- which a
/// caller that passes its own as text cannot produce -- is reported as none
/// rather than sent on in a form that would name a different directory.
pub(crate) fn working_directory() -> Option<String> {
    match std::env::current_dir() {
        Ok(cwd) => match cwd.to_str() {
            Some(cwd) => Some(cwd.to_string()),
            None => {
                log::warn!("hishell: the working directory {cwd:?} is not valid text");
                None
            }
        },
        Err(err) => {
            log::warn!("hishell: cannot read the working directory: {err}");
            None
        }
    }
}

/// Size of the local terminal, or `None` when it is not a terminal or reports a
/// zero-sized window.
fn local_size() -> Option<(u32, u32)> {
    let mut size = libc::winsize {
        ws_row: 0,
        ws_col: 0,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    // SAFETY: TIOCGWINSZ writes a winsize through the pointer it is given.
    if unsafe { libc::ioctl(libc::STDIN_FILENO, libc::TIOCGWINSZ, &mut size) } != 0 {
        return None;
    }
    if size.ws_col == 0 || size.ws_row == 0 {
        return None;
    }
    Some((size.ws_col as u32, size.ws_row as u32))
}

/// Size of the local terminal once it has settled, i.e. after the host
/// application has laid the terminal out.
///
/// The host application opens the local pty at its pre-layout debug size (see
/// `PRE_LAYOUT_COLS`) and lays it out afterwards. A size that is not the debug
/// one is already final, so it is returned at once: a settled terminal pays no
/// delay. A size still equal to the debug one is re-read at a short interval
/// until it changes or the bounded timeout passes; the last read is used either
/// way, so the shell always starts. `None` (not a terminal, or a zero-sized
/// window) is returned at once, because there is nothing to settle.
fn settled_local_size() -> Option<(u32, u32)> {
    let mut size = local_size();
    if size != Some((PRE_LAYOUT_COLS, PRE_LAYOUT_ROWS)) {
        return size;
    }
    for _ in 0..SIZE_SETTLE_ATTEMPTS {
        std::thread::sleep(SIZE_SETTLE_POLL);
        size = local_size();
        if size != Some((PRE_LAYOUT_COLS, PRE_LAYOUT_ROWS)) {
            return size;
        }
    }
    // The layout did not land within the attempts allowed. The size may still be
    // the stale pre-layout one, which is what the `%` comes from; `bridge`
    // forwards the real size as soon as it is readable, so say why the shell may
    // open on the wrong width.
    log::warn!(
        "hishell: local size {size:?} did not settle after {SIZE_SETTLE_ATTEMPTS} reads within \
         {SIZE_SETTLE_TIMEOUT:?}"
    );
    size
}
