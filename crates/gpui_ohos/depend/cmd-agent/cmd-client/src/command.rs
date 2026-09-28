//! Translates an `ExecSpec` into a POSIX sh command string for the daemon.
//!
//! A child of the daemon starts from the daemon's own environment; the spec's
//! env is emitted as an exec prefix only for the entries a caller sets
//! explicitly, and started LSPs are addressed by absolute path. Values and paths
//! travel verbatim (no mapping: the daemon and the host application share the
//! same device filesystem). The cwd is created before cd-ing. A reserved first
//! line carries the session id that the daemon uses to associate the exec with
//! its in-memory session table.

use crate::types::{ExecSpec, FdMode};

/// Builds the shell command string for one exec request.
pub fn build_command(spec: &ExecSpec, session_id: u64) -> String {
    // Working directory: created then entered. Missing cwd must not fail the
    // whole command (a stale path would otherwise make git/LSP unusable).
    let mut parts: Vec<String> = Vec::new();
    if let Some(cwd) = &spec.cwd_path {
        if !cwd.is_empty() {
            parts.push(format!("mkdir -p {}", sh_quote(cwd)));
            parts.push(format!("cd {}", sh_quote(cwd)));
        }
    }

    // Environment: only the entries a caller sets explicitly are emitted as an
    // exec prefix; a child otherwise inherits the daemon's own environment.
    let mut envs: Vec<String> = Vec::new();
    for (key, value) in &spec.env {
        envs.push(format!("{}={}", key, sh_quote(value)));
    }

    let mut command = vec![sh_quote(&spec.binary)];
    command.extend(spec.args.iter().map(|arg| sh_quote(arg)));
    let mut line = format!("{} exec {}", envs.join(" "), command.join(" "));

    // Fd redirection: /dev/null for Null modes, otherwise the SSH channel.
    match spec.stdin_mode {
        FdMode::Null => line.push_str(" </dev/null"),
        FdMode::Piped => {}
    }
    match spec.stdout_mode {
        FdMode::Null => line.push_str(" >/dev/null"),
        FdMode::Piped => {}
    }
    match spec.stderr_mode {
        FdMode::Null => line.push_str(" 2>/dev/null"),
        FdMode::Piped => {}
    }
    parts.push(line);

    crate::protocol::sid_payload(session_id, &parts.join(" && "))
}

/// Quotes a value for a POSIX shell: single quotes with `'\''` escaping.
pub fn sh_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

/// Builds the shell command that has the daemon run `which` in its own
/// environment: the reply is the path that environment's PATH holds for the
/// name. `|| true` turns "not found" (a non-zero exit with no output) into a
/// successful empty reply, so absence comes back as an empty answer rather than
/// as a failing command.
///
/// The `|| true` is load-bearing and must not be dropped. The daemon
/// special-cases a plain `which` call that failed by writing `which: not found:
/// <name>` to stdout (see `hicodeerd/src/exec.rs`), and it recognizes that shape
/// only when the command carries no shell operator. The `|| true` keeps this
/// command out of that special case, so the reply holds either a path or
/// nothing - never a message the caller would misread as a path.
pub fn resolve_program_command(name: &str) -> String {
    format!("which -- {} || true", sh_quote(name))
}
