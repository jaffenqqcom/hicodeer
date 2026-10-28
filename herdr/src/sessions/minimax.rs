//! History scanner for MiniMax Code (`mcode`).
//!
//! MCode keeps its runtime state in one SQLite database:
//!
//! ```text
//! ~/.minimax/v2/sqlite/runtime-state.sqlite
//!   local_runtime_sessions(session_id, title, workspace_dir, session_kind,
//!                          archived, visibility, parent_session_id,
//!                          created_at_ms, updated_at_ms, …)
//! ```
//!
//! FAD has no MiniMax connector. The `record_json` column carries the same
//! fields as a document, but the columnar columns next to it are what the tool
//! itself reads back (`columnar_version` says how far the migration has run), so
//! the query takes them directly and never parses JSON.
//!
//! Rows are read through the `sqlite3` CLI for the same reason as DevEco Code
//! (`deveco.rs`): a one-shot read-only query does not justify wiring a second
//! SQLite driver into this process.

use std::path::{Path, PathBuf};
use std::process::Command;

use franken_agent_detection::{
    Connector, DetectionResult, DiscoveredSourceFile, DiscoveredSourceRole,
    NormalizedConversation, ScanContext, ScanRoot,
};
use serde::Deserialize;

/// The agent label the panel matches against `agent.list`.
const SLUG: &str = "minimax";

/// Only real conversations belong in the history list: branch/task/peek rows
/// are internal projections of a conversation the user already sees.
const CONVERSATION_KIND: &str = "conversation";

/// One row of `local_runtime_sessions`, limited to the columns a history row
/// needs.
#[derive(Deserialize)]
struct SessionRow {
    session_id: String,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    workspace_dir: Option<String>,
    #[serde(default)]
    updated_at_ms: Option<i64>,
}

pub(super) struct MinimaxConnector;

impl MinimaxConnector {
    pub(super) fn new() -> Self {
        Self
    }
}

impl Connector for MinimaxConnector {
    fn detect(&self) -> DetectionResult {
        match database_path().filter(|path| path.is_file()) {
            Some(path) => DetectionResult {
                detected: true,
                evidence: vec![path.display().to_string()],
                root_paths: vec![path],
            },
            None => DetectionResult::not_found(),
        }
    }

    fn scan(&self, _ctx: &ScanContext) -> anyhow::Result<Vec<NormalizedConversation>> {
        let Some(db) = database_path().filter(|path| path.is_file()) else {
            return Ok(Vec::new());
        };
        Ok(query_sessions(&db)
            .map_err(anyhow::Error::msg)?
            .into_iter()
            .map(|row| conversation(&db, row))
            .collect())
    }

    fn discover_source_files(
        &self,
        _ctx: &ScanContext,
    ) -> anyhow::Result<Vec<DiscoveredSourceFile>> {
        let Some(db) = database_path().filter(|path| path.is_file()) else {
            return Ok(Vec::new());
        };
        let root = db.parent().unwrap_or(&db).to_path_buf();
        let scan_root = ScanRoot::local(root);
        let mut files = Vec::new();
        for path in [db.clone(), with_suffix(&db, "-wal"), with_suffix(&db, "-shm")] {
            let Ok(metadata) = std::fs::metadata(&path) else {
                continue;
            };
            let mut discovered = DiscoveredSourceFile::new(
                SLUG,
                &scan_root,
                path,
                DiscoveredSourceRole::PrimarySessionLog,
                true,
            );
            discovered.size_bytes = Some(metadata.len());
            discovered.modified_at_ms = Some(modified_ms(&metadata));
            files.push(discovered);
        }
        Ok(files)
    }
}

fn home() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .filter(|home| !home.is_empty())
        .map(PathBuf::from)
}

fn database_path() -> Option<PathBuf> {
    home().map(|home| {
        home.join(".minimax")
            .join("v2")
            .join("sqlite")
            .join("runtime-state.sqlite")
    })
}

/// `path-wal` / `path-shm`: the write-ahead log can hold the newest rows, so it
/// has to be part of the change signature.
fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(suffix);
    PathBuf::from(name)
}

/// Removes one MiniMax Code session and the rows that belong to it.
///
/// The row's source path is `<database>/<session id>`. The transcript lives in
/// two side tables that carry the session id without a foreign key, so they are
/// cleared explicitly; the remaining per-session tables cascade once foreign
/// keys are switched on for the connection.
pub(super) fn delete_session(path: &Path) -> Result<(), String> {
    let (database, session_id) = split_source_path(path)?;
    let expected = database_path()
        .ok_or_else(|| "no MiniMax store on this device".to_string())?
        .canonicalize()
        .map_err(|error| format!("resolve the store: {error}"))?;
    let resolved = database
        .canonicalize()
        .map_err(|error| format!("resolve {}: {error}", database.display()))?;
    if resolved != expected {
        return Err(format!(
            "refusing to delete {}: outside the store",
            path.display()
        ));
    }

    let statement = delete_statement(&session_id);
    let output = Command::new(sqlite_binary())
        .arg("-cmd")
        .arg(SQLITE_BUSY_TIMEOUT)
        .arg(&resolved)
        .arg(&statement)
        .output()
        .map_err(|error| format!("open {}: {error}", resolved.display()))?;
    if !output.status.success() {
        return Err(format!(
            "delete from {} failed: {}",
            resolved.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(())
}

/// The statements that remove one session: the two tables that name it without
/// a foreign key first, then the session row, whose per-session tables cascade.
fn delete_statement(session_id: &str) -> String {
    format!(
        "PRAGMA foreign_keys=ON;\
         delete from local_runtime_messages where session_id='{session_id}';\
         delete from local_runtime_message_rows where session_id='{session_id}';\
         delete from local_runtime_sessions where session_id='{session_id}';"
    )
}

/// Splits the `<database>/<session id>` shape this connector reports.
///
/// The id ends up inside a SQL statement, and SQLite's command line offers no
/// binding, so anything outside the characters these tools use is refused
/// instead of quoted.
fn split_source_path(path: &Path) -> Result<(PathBuf, String), String> {
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

/// Where the `sqlite3` CLI lives. The daemon inherits the system `PATH`, which
/// does not include the Homebrew prefix this device installs into.
fn sqlite_binary() -> PathBuf {
    let homebrew = std::env::var_os("HOME")
        .filter(|home| !home.is_empty())
        .map(|home| PathBuf::from(home).join(".harmonybrew/bin/sqlite3"));
    match homebrew.filter(|path| path.is_file()) {
        Some(path) => path,
        None => PathBuf::from("sqlite3"),
    }
}

/// One history row from one database row.
fn conversation(db: &Path, row: SessionRow) -> NormalizedConversation {
    NormalizedConversation {
        agent_slug: SLUG.to_owned(),
        external_id: Some(row.session_id.clone()),
        title: row
            .title
            .map(|title| title.trim().to_owned())
            .filter(|title| !title.is_empty()),
        workspace: row
            .workspace_dir
            .filter(|directory| !directory.is_empty())
            .map(PathBuf::from),
        // The transcript lives inside the database, so the path names the
        // database plus the row; `delete_session` splits it again and removes
        // the row through `sqlite3`.
        source_path: PathBuf::from(format!("{}/{}", db.display(), row.session_id)),
        started_at: None,
        ended_at: row.updated_at_ms,
        metadata: serde_json::Value::Null,
        messages: Vec::new(),
    }
}

/// Reads every visible conversation. A missing CLI, an unreadable database or
/// malformed output yields an empty list: the other connectors still have rows.
fn query_sessions(db: &Path) -> Result<Vec<SessionRow>, String> {
    let sql = format!(
        "select session_id, title, workspace_dir, updated_at_ms \
         from local_runtime_sessions \
         where archived = 0 \
           and visibility = 'visible' \
           and session_kind = '{CONVERSATION_KIND}' \
           and (parent_session_id is null or parent_session_id = '') \
         order by updated_at_ms desc"
    );
    let output = sqlite_query(db)
        .arg(&sql)
        .output()
        .map_err(|error| format!("open {}: {error}", db.display()))?;
    if !output.status.success() {
        return Err(format!(
            "query {} failed: {}",
            db.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    parse_rows(&String::from_utf8_lossy(&output.stdout), db)
}

/// The `sqlite3` invocation every query here uses: JSON output plus a busy
/// timeout.
///
/// The CLI's default busy timeout is zero, so a database the agent CLI is
/// writing to at that moment fails the query outright. Failing means the scan
/// reports Err and the caller keeps its previous rows, which is the honest
/// answer; returning an empty list would look like "this agent has no history"
/// and would be cached under the new store signature.
fn sqlite_query(db: &Path) -> Command {
    let mut command = Command::new(sqlite_binary());
    command
        .arg("-json")
        .arg("-cmd")
        .arg(SQLITE_BUSY_TIMEOUT)
        .arg(db);
    command
}

/// How long a query waits for a database another process is writing.
const SQLITE_BUSY_TIMEOUT: &str = ".timeout 2000";

/// The rows from one `-json` output, folded to "no sessions" on a decode error.
/// Only the tests use this spelling: the live path calls `parse_rows` so a
/// truncated query output is reported instead of looking like an empty store.
#[cfg(test)]
fn parse_sessions(text: &str, db: &Path) -> Vec<SessionRow> {
    parse_rows(text, db).unwrap_or_else(|error| {
        tracing::warn!("agent sessions: {error}");
        Vec::new()
    })
}

/// The rows from one `-json` output, with an unreadable one reported rather
/// than folded into "no sessions".
fn parse_rows(text: &str, db: &Path) -> Result<Vec<SessionRow>, String> {
    if text.trim().is_empty() {
        return Ok(Vec::new());
    }
    serde_json::from_str(text).map_err(|error| format!("{} rows unreadable: {error}", db.display()))
}

fn modified_ms(metadata: &std::fs::Metadata) -> i64 {
    metadata
        .modified()
        .ok()
        .and_then(|mtime| mtime.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|elapsed| elapsed.as_millis() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Fixtures sit beside the workspace rather than inside the crate, so the
    /// end-to-end scripts under `agent-tests/` read the same samples. These
    /// hold what the `sqlite3` CLI prints, so no database is needed here.
    ///
    /// The archived / visibility / session-kind / parent filters live in the
    /// SQL text inside `query_sessions`; exercising them needs a real database
    /// and the `sqlite3` CLI, which these tests deliberately do not depend on,
    /// so they are covered by the end-to-end scripts instead. What is covered
    /// here is everything after the query: decoding the rows and mapping them
    /// to history rows.
    fn fixture(relative: &str) -> String {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../agent-tests/fixtures/minimax")
            .join(relative);
        std::fs::read_to_string(path).expect("fixture should be readable")
    }

    #[test]
    fn parse_sessions_reads_query_output() {
        let rows = parse_sessions(&fixture("session_rows.json"), Path::new("runtime-state.sqlite"));
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].session_id, "mvs_visible");
        assert_eq!(rows[0].title.as_deref(), Some("  Reply with one word  "));
        assert_eq!(rows[0].workspace_dir.as_deref(), Some("/repo/demo"));
        assert_eq!(rows[0].updated_at_ms, Some(1793101792211));
    }

    /// A half-written or non-JSON output yields no rows instead of failing the
    /// scan.
    #[test]
    fn parse_sessions_survives_truncated_output() {
        let rows = parse_sessions(
            &fixture("session_rows_corrupt.json"),
            Path::new("runtime-state.sqlite"),
        );
        assert!(
            rows.is_empty(),
            "truncated CLI output must decode to no rows"
        );
    }

    #[test]
    fn parse_sessions_treats_empty_output_as_no_rows() {
        assert!(
            parse_sessions("", Path::new("runtime-state.sqlite")).is_empty(),
            "empty CLI output must decode to no rows"
        );
        assert!(
            parse_sessions("  \n", Path::new("runtime-state.sqlite")).is_empty(),
            "whitespace-only CLI output must decode to no rows"
        );
    }

    #[test]
    fn conversation_trims_the_title_and_keeps_the_row_fields() {
        let database = Path::new("/store/runtime-state.sqlite");
        let row = SessionRow {
            session_id: "mvs_visible".to_owned(),
            title: Some("  Reply with one word  ".to_owned()),
            workspace_dir: Some("/repo/demo".to_owned()),
            updated_at_ms: Some(1793101792211),
        };
        let history = conversation(database, row);
        assert_eq!(history.agent_slug, "minimax");
        assert_eq!(history.external_id.as_deref(), Some("mvs_visible"));
        assert_eq!(history.title.as_deref(), Some("Reply with one word"));
        assert_eq!(
            history.workspace.as_deref(),
            Some(Path::new("/repo/demo"))
        );
        assert_eq!(history.ended_at, Some(1793101792211));
        assert_eq!(
            history.source_path,
            PathBuf::from("/store/runtime-state.sqlite/mvs_visible")
        );
    }

    /// A blank title is no title at all, and a blank directory no workspace.
    #[test]
    fn conversation_drops_blank_fields() {
        let row = SessionRow {
            session_id: "mvs_blank".to_owned(),
            title: Some("   ".to_owned()),
            workspace_dir: Some(String::new()),
            updated_at_ms: None,
        };
        let history = conversation(Path::new("/store/runtime-state.sqlite"), row);
        assert_eq!(history.title, None);
        assert_eq!(history.workspace, None);
        assert_eq!(history.ended_at, None);
    }

    /// The row's source path is `<database>/<session id>`, so the database is
    /// the part before the last slash and the id is the rest.
    #[test]
    fn split_source_path_separates_the_database_from_the_id() {
        let (database, session_id) = split_source_path(Path::new(
            "/store/runtime-state.sqlite/mvs_visible",
        ))
        .expect("a shape this app writes");
        assert_eq!(database, PathBuf::from("/store/runtime-state.sqlite"));
        assert_eq!(session_id, "mvs_visible");
    }

    /// The id goes into a SQL statement verbatim, so anything beyond the
    /// characters these tools use is refused instead of quoted.
    #[test]
    fn split_source_path_refuses_an_id_it_cannot_place_in_sql() {
        for path in [
            "/store/runtime-state.sqlite/mvs'--",
            "/store/runtime-state.sqlite/",
            "/store/runtime-state.sqlite/mvs;drop table local_runtime_sessions;",
        ] {
            let error = split_source_path(Path::new(path))
                .expect_err("an id outside [A-Za-z0-9_-] must be refused");
            assert!(
                error.contains("unexpected id") || error.contains("names no database"),
                "the refusal should name the reason for {path}, got: {error}"
            );
        }
    }

    /// Two side tables carry the session id without a foreign key, so they are
    /// cleared explicitly; the session row goes last, with foreign keys on so
    /// its per-session tables cascade.
    #[test]
    fn delete_statement_clears_the_side_tables_then_the_session() {
        let statement = delete_statement("mvs_visible");
        assert!(statement.starts_with("PRAGMA foreign_keys=ON;"));
        let messages = statement
            .find("delete from local_runtime_messages where session_id='mvs_visible';")
            .expect("the message rows are cleared");
        let message_rows = statement
            .find("delete from local_runtime_message_rows where session_id='mvs_visible';")
            .expect("the message row index is cleared");
        let session = statement
            .find("delete from local_runtime_sessions where session_id='mvs_visible';")
            .expect("the session row is cleared");
        assert!(
            messages < session && message_rows < session,
            "the side tables must go before the row they describe, got: {statement}"
        );
    }
}
