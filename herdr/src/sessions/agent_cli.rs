//! Session removal through the agent's own command line.
//!
//! OpenCode and Hermes keep their transcripts in a SQLite database, but
//! removing one session is more than dropping a row: OpenCode also removes the
//! child sessions, Hermes retires gateway ledger entries, media files and its
//! full-text index. Both tools ship the command that does all of it, so this
//! module runs that command instead of writing SQL by hand and drifting away
//! from the store's own rules.
//!
//! The panel addresses a session by the path the scanner reported, which for
//! both stores is `<database>/<session id>` (`fad.rs` inherits that shape from
//! FAD's connectors). Only the id is passed on to the command; the database
//! half is checked against the store this agent really owns, because the path
//! arrives over the API and a delete must not be talked into touching anything
//! else.

use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

/// How long a delete command may run before it is killed. Both commands answer
/// in under two seconds on this device; the ceiling keeps a hung private server
/// (OpenCode starts one per delete) from pinning a daemon request.
const DELETE_TIMEOUT: Duration = Duration::from_secs(5);

/// How often the wait loop checks whether the command has finished.
const DELETE_POLL_INTERVAL: Duration = Duration::from_millis(50);

/// The removal function for a FAD slug, when that store's delete is a command
/// rather than a file unlink.
pub(super) fn delete_for(slug: &str) -> Option<fn(&Path) -> Result<(), String>> {
    match slug {
        "opencode" => Some(delete_opencode_session),
        "hermes" => Some(delete_hermes_session),
        _ => None,
    }
}

/// Removes one OpenCode session, including its child sessions.
pub(super) fn delete_opencode_session(path: &Path) -> Result<(), String> {
    let session_id = session_id_in_store(path, "opencode", &opencode_databases())?;
    run_delete(
        "opencode",
        &["session", "delete", session_id.as_str(), "--standalone"],
    )
}

/// Removes one Hermes session, including delegate children and the files the
/// store keeps next to the database.
pub(super) fn delete_hermes_session(path: &Path) -> Result<(), String> {
    let session_id = session_id_in_store(path, "hermes", &hermes_databases())?;
    run_delete(
        "hermes",
        &["sessions", "delete", session_id.as_str(), "--yes"],
    )
}

/// The databases OpenCode may be using on this device. FAD treats the XDG data
/// and config directories as fallbacks, so both are accepted here.
fn opencode_databases() -> Vec<PathBuf> {
    let Some(home) = home_dir() else {
        return Vec::new();
    };
    let mut databases = vec![
        home.join(".local/share/opencode/opencode.db"),
        home.join(".config/opencode/opencode.db"),
    ];
    if let Some(data) = directory_from_env("XDG_DATA_HOME") {
        databases.push(data.join("opencode/opencode.db"));
    }
    if let Some(config) = directory_from_env("XDG_CONFIG_HOME") {
        databases.push(config.join("opencode/opencode.db"));
    }
    databases
}

/// The database Hermes may be using on this device. FAD honours
/// `HERMES_HOME`; the default lives under the home directory.
fn hermes_databases() -> Vec<PathBuf> {
    let mut databases = Vec::new();
    if let Some(home) = directory_from_env("HERMES_HOME") {
        databases.push(home.join("state.db"));
    }
    if let Some(home) = home_dir() {
        databases.push(home.join(".hermes/state.db"));
    }
    databases
}

/// Splits `<database>/<session id>`, refusing a path that does not name this
/// agent's store or an id no CLI would accept.
fn session_id_in_store(path: &Path, agent: &str, databases: &[PathBuf]) -> Result<String, String> {
    let (database, session_id) = split_session_path(path)?;
    let resolved = database
        .canonicalize()
        .map_err(|error| format!("resolve {}: {error}", database.display()))?;
    let known = databases
        .iter()
        .filter_map(|candidate| candidate.canonicalize().ok())
        .any(|candidate| candidate == resolved);
    if !known {
        return Err(format!(
            "refusing to delete {}: not the {agent} session store",
            path.display()
        ));
    }
    Ok(session_id)
}

/// Splits the `<database>/<session id>` shape both scanners report.
///
/// The id ends up on a command line, so anything outside the characters these
/// tools use is refused instead of quoted.
fn split_session_path(path: &Path) -> Result<(PathBuf, String), String> {
    let text = path.to_string_lossy();
    let Some((database, session_id)) = text.rsplit_once('/') else {
        return Err(format!("{} names no database", path.display()));
    };
    let well_formed = !session_id.is_empty()
        && session_id
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'));
    if !well_formed {
        return Err(format!(
            "refusing to delete session {session_id}: unexpected id"
        ));
    }
    Ok((PathBuf::from(database), session_id.to_owned()))
}

/// Runs one delete command and turns its output into an error when it fails.
fn run_delete(command_name: &str, arguments: &[&str]) -> Result<(), String> {
    let program = cli_binary(command_name);
    let mut child = Command::new(&program)
        .args(arguments)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("start {}: {error}", program.display()))?;

    let deadline = Instant::now() + DELETE_TIMEOUT;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if Instant::now() >= deadline => {
                // Do not collect output here: a descendant still holding the
                // pipes would block the read and defeat the ceiling.
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!(
                    "{command_name} delete timed out after {}s and was killed",
                    DELETE_TIMEOUT.as_secs()
                ));
            }
            Ok(None) => std::thread::sleep(DELETE_POLL_INTERVAL),
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("wait for {command_name} delete: {error}"));
            }
        }
    }

    let output = child
        .wait_with_output()
        .map_err(|error| format!("collect {command_name} output: {error}"))?;
    if output.status.success() {
        return Ok(());
    }
    Err(format!(
        "{command_name} delete failed: {}",
        output_summary(&output)
    ))
}

/// The last lines a command printed, which is where both tools put the reason
/// a delete was refused.
fn output_summary(output: &Output) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let text = if stderr.trim().is_empty() {
        stdout.trim()
    } else {
        stderr.trim()
    };
    if text.is_empty() {
        return format!("exit status {}", output.status);
    }
    let lines: Vec<&str> = text.lines().rev().take(2).collect();
    lines.into_iter().rev().collect::<Vec<_>>().join(" ")
}

/// Where the agent's command lives. The daemon's `PATH` already carries the
/// Homebrew prefix, but the explicit path keeps the call working on a daemon
/// started without it; the bare name is the fallback for other layouts.
fn cli_binary(name: &str) -> PathBuf {
    let installed = home_dir().map(|home| home.join(".harmonybrew/bin").join(name));
    match installed.filter(|path| path.is_file()) {
        Some(path) => path,
        None => PathBuf::from(name),
    }
}

fn home_dir() -> Option<PathBuf> {
    directory_from_env("HOME")
}

fn directory_from_env(name: &str) -> Option<PathBuf> {
    std::env::var_os(name)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_session_path_separates_the_database_from_the_id() {
        let (database, session_id) =
            split_session_path(Path::new("/store/opencode.db/ses_abc-123_X")).unwrap();
        assert_eq!(database, PathBuf::from("/store/opencode.db"));
        assert_eq!(session_id, "ses_abc-123_X");
    }

    #[test]
    fn split_session_path_refuses_an_id_it_cannot_place_on_a_command_line() {
        assert!(split_session_path(Path::new("/store/opencode.db/ses a")).is_err());
        assert!(split_session_path(Path::new("/store/opencode.db/ses;drop")).is_err());
        assert!(split_session_path(Path::new("/store/opencode.db/")).is_err());
        assert!(split_session_path(Path::new("opencode.db")).is_err());
    }

    #[test]
    fn session_id_in_store_refuses_a_database_outside_the_store() {
        let store = std::env::temp_dir().join(format!(
            "herdr-agent-cli-{}-{}",
            std::process::id(),
            line!()
        ));
        std::fs::create_dir_all(&store).unwrap();
        let known = store.join("opencode.db");
        std::fs::write(&known, b"").unwrap();
        let other = store.join("other.db");
        std::fs::write(&other, b"").unwrap();

        let known_path = known.join("ses_1");
        assert_eq!(
            session_id_in_store(&known_path, "opencode", &[known.clone()]).unwrap(),
            "ses_1"
        );
        assert!(session_id_in_store(&other.join("ses_1"), "opencode", &[known]).is_err());

        std::fs::remove_dir_all(&store).unwrap();
    }
}
