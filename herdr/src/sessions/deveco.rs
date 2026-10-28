//! History scanner for agents built on OpenCode's session store.
//!
//! DevEco Code (`deveco`) and ZCode (`zcode`) both keep their sessions in an
//! OpenCode-derived SQLite database with the same three tables:
//!
//! ```text
//! ~/.local/share/deveco/deveco.db   (DevEco Code)
//! ~/.zcode/cli/db/db.sqlite         (ZCode CLI)
//!
//!   session(id, directory, title, parent_id, time_created, time_updated, …)
//!   message(id, session_id, …)
//!   part(id, message_id, session_id, …)
//! ```
//!
//! FAD has an OpenCode connector, but it only looks at OpenCode's own
//! `~/.local/share/opencode/opencode.db`; the paths are hard-coded there, so
//! these two tools need their own registration. One connector serves both: the
//! query and the row mapping are identical, only the database path differs.
//!
//! Neither tool titles a conversation when it starts: the row keeps a
//! `New session - <timestamp>` placeholder until a summariser replaces it, so
//! a session that was just created would be listed with that placeholder. Those
//! rows get the first user message as their title instead.
//!
//! Rows are read through the `sqlite3` CLI rather than a Rust SQLite binding.
//! `herdr` already links `fsqlite` for FAD's SQLite connectors, but that API is
//! async and driven by an external runtime; a one-shot read-only query over a
//! handful of rows does not justify wiring a second driver into this process,
//! and the CLI keeps the scanner free of a native dependency that would have to
//! be cross-compiled for OHOS.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use franken_agent_detection::{
    Connector, DetectionResult, DiscoveredSourceFile, DiscoveredSourceRole,
    NormalizedConversation, ScanContext, ScanRoot,
};
use serde::Deserialize;

/// The title a session carries before anything has summarised it. Rows still
/// wearing it are named after their first user message instead.
const UNTITLED_PREFIX: &str = "New session";

/// One row of the `session` table, limited to the columns a history row needs.
#[derive(Deserialize)]
struct SessionRow {
    id: String,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    directory: Option<String>,
    #[serde(default)]
    time_updated: Option<i64>,
    #[serde(default)]
    parent_id: Option<String>,
}

/// Which tool a database belongs to. The slug doubles as the agent label the
/// panel matches against `agent.list`.
#[derive(Clone, Copy)]
pub(super) enum OpencodeFamily {
    Deveco,
    Zcode,
}

impl OpencodeFamily {
    pub(super) fn slug(self) -> &'static str {
        match self {
            Self::Deveco => "deveco",
            Self::Zcode => "zcode",
        }
    }

    /// Where the tool keeps its database, relative to `$HOME`.
    fn relative_db_path(self) -> &'static str {
        match self {
            Self::Deveco => ".local/share/deveco/deveco.db",
            Self::Zcode => ".zcode/cli/db/db.sqlite",
        }
    }

    fn db_path(self) -> Option<PathBuf> {
        std::env::var_os("HOME")
            .filter(|home| !home.is_empty())
            .map(|home| PathBuf::from(home).join(self.relative_db_path()))
    }
}

pub(super) struct OpencodeFamilyConnector {
    family: OpencodeFamily,
}

impl OpencodeFamilyConnector {
    pub(super) fn new(family: OpencodeFamily) -> Self {
        Self { family }
    }
}

impl Connector for OpencodeFamilyConnector {
    fn detect(&self) -> DetectionResult {
        match self.family.db_path().filter(|path| path.is_file()) {
            Some(path) => DetectionResult {
                detected: true,
                evidence: vec![path.display().to_string()],
                root_paths: vec![path],
            },
            None => DetectionResult::not_found(),
        }
    }

    fn scan(&self, _ctx: &ScanContext) -> anyhow::Result<Vec<NormalizedConversation>> {
        let Some(db) = self.family.db_path().filter(|path| path.is_file()) else {
            return Ok(Vec::new());
        };
        let slug = self.family.slug();
        let rows: Vec<SessionRow> = query_sessions(&db)
            .map_err(anyhow::Error::msg)?
            .into_iter()
            .filter(|row| !row.id.is_empty())
            .filter(is_root_session)
            .collect();
        // A placeholder title is all a session has until a summariser runs, so
        // the ones that never got that far are named after their first prompt.
        let untitled: Vec<&str> = rows
            .iter()
            .filter(|row| is_untitled(row.title.as_deref()))
            .map(|row| row.id.as_str())
            .collect();
        let first_prompts = if untitled.is_empty() {
            HashMap::new()
        } else {
            query_first_prompts(&db, &untitled)
        };
        Ok(rows
            .into_iter()
            .map(|row| {
                let title = title_for_session(row.title.as_deref(), &row.id, &first_prompts);
                NormalizedConversation {
                    agent_slug: slug.to_owned(),
                    external_id: Some(row.id.clone()),
                    title,
                    workspace: row
                        .directory
                        .filter(|directory| !directory.is_empty())
                        .map(PathBuf::from),
                    // The transcript is inside the database, so the path names
                    // the database plus the row; `delete_session` splits it
                    // again and removes the row through `sqlite3`.
                    source_path: PathBuf::from(format!("{}/{}", db.display(), row.id)),
                    started_at: None,
                    ended_at: row.time_updated,
                    metadata: serde_json::Value::Null,
                    messages: Vec::new(),
                }
            })
            .collect())
    }

    fn discover_source_files(
        &self,
        _ctx: &ScanContext,
    ) -> anyhow::Result<Vec<DiscoveredSourceFile>> {
        let Some(db) = self.family.db_path().filter(|path| path.is_file()) else {
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
                self.family.slug(),
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

/// `path-wal` / `path-shm`: the write-ahead log can hold the newest rows, so it
/// has to be part of the change signature.
fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(suffix);
    PathBuf::from(name)
}

fn modified_ms(metadata: &std::fs::Metadata) -> i64 {
    metadata
        .modified()
        .ok()
        .and_then(|mtime| mtime.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|elapsed| elapsed.as_millis() as i64)
        .unwrap_or(0)
}

/// Removes one DevEco Code session.
pub(super) fn delete_deveco_session(path: &Path) -> Result<(), String> {
    delete_session(OpencodeFamily::Deveco, path)
}

/// Removes one ZCode session.
pub(super) fn delete_zcode_session(path: &Path) -> Result<(), String> {
    delete_session(OpencodeFamily::Zcode, path)
}

/// Removes one session from the family's database.
///
/// The row's source path is `<database>/<session id>`: the transcript lives
/// inside the database, so the database is opened again here. Foreign keys are
/// switched on for this connection because the CLI opens them off by default,
/// and the cascade is what removes the session's messages and parts.
fn delete_session(family: OpencodeFamily, path: &Path) -> Result<(), String> {
    let (database, session_id) = split_source_path(path)?;
    let expected = family
        .db_path()
        .ok_or_else(|| "no store for this agent on this device".to_string())?
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

/// The statement that removes one session; foreign keys are on so the messages
/// and parts that point at it go with it.
fn delete_statement(session_id: &str) -> String {
    format!("PRAGMA foreign_keys=ON; delete from session where id='{session_id}';")
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
        return Err(format!("refusing to delete session {session_id}: unexpected id"));
    }
    Ok((PathBuf::from(database), session_id.to_owned()))
}

/// Where the `sqlite3` CLI lives. The daemon inherits the system `PATH`, which
/// does not include the Homebrew prefix this device installs into, so the usual
/// location is tried first and the bare name is left as a fallback for
/// environments that do put it on `PATH`.
fn sqlite_binary() -> PathBuf {
    let homebrew = std::env::var_os("HOME")
        .filter(|home| !home.is_empty())
        .map(|home| PathBuf::from(home).join(".harmonybrew/bin/sqlite3"));
    match homebrew.filter(|path| path.is_file()) {
        Some(path) => path,
        None => PathBuf::from("sqlite3"),
    }
}

/// Whether a row still wears the placeholder a tool writes at session start,
/// before anything has summarised the conversation.
fn is_untitled(title: Option<&str>) -> bool {
    title.is_some_and(|title| title.trim().starts_with(UNTITLED_PREFIX))
}

/// A sub-agent session belongs to the row that spawned it; listing it
/// separately would show one conversation twice.
fn is_root_session(row: &SessionRow) -> bool {
    row.parent_id.as_deref().is_none_or(str::is_empty)
}

/// The title a history row shows: the stored title when it carries one, and the
/// session's first prompt when the stored title is still the placeholder. A
/// blank title is no title at all.
fn title_for_session(
    title: Option<&str>,
    session_id: &str,
    first_prompts: &HashMap<String, String>,
) -> Option<String> {
    let mut title = title
        .filter(|title| !title.trim().is_empty())
        .map(str::to_owned);
    if is_untitled(title.as_deref()) {
        if let Some(prompt) = first_prompts.get(session_id) {
            title = Some(prompt.clone());
        }
    }
    title
}

/// One `(session, prompt)` pair read for the untitled rows.
#[derive(Deserialize)]
struct PromptRow {
    session_id: String,
    #[serde(default)]
    text: Option<String>,
}

/// Reads the first prompt of every named session in a single query.
///
/// A prompt can run over several lines and the panel draws one row per
/// session, so only its first non-empty line is kept. A session that was
/// created but never used has no user text and stays absent from the map.
///
/// Parts the tool injected into the user turn — the "tool was executed" notice
/// that precedes a resumed tool call, for instance — carry `synthetic: true`
/// and describe the machinery rather than the session, so they are skipped.
fn query_first_prompts(db: &Path, session_ids: &[&str]) -> HashMap<String, String> {
    let listed = session_ids
        .iter()
        .map(|id| format!("'{}'", id.replace('\'', "''")))
        .collect::<Vec<_>>()
        .join(", ");
    let sql = format!(
        "select p.session_id as session_id, json_extract(p.data, '$.text') as text \
         from part p join message m on m.id = p.message_id \
         where json_extract(m.data, '$.role') = 'user' \
           and json_extract(p.data, '$.type') = 'text' \
           and coalesce(json_extract(p.data, '$.synthetic'), 0) = 0 \
           and p.session_id in ({listed}) \
         order by p.time_created"
    );
    let output = sqlite_query(db).arg(&sql).output();
    let Ok(output) = output else {
        return HashMap::new();
    };
    if !output.status.success() {
        tracing::warn!(
            "agent sessions: {} prompt query failed: {}",
            db.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        );
        return HashMap::new();
    }
    parse_first_prompts(&String::from_utf8_lossy(&output.stdout), db)
}

/// The prompt map from one `-json` output. Kept apart from the CLI call so the
/// mapping — first non-empty line, first prompt of each session — is testable.
fn parse_first_prompts(text: &str, db: &Path) -> HashMap<String, String> {
    if text.trim().is_empty() {
        return HashMap::new();
    }
    let rows: Vec<PromptRow> = serde_json::from_str(text).unwrap_or_else(|error| {
        tracing::warn!("agent sessions: {} prompts unreadable: {error}", db.display());
        Vec::new()
    });
    let mut prompts = HashMap::new();
    for row in rows {
        let Some(text) = row.text else {
            continue;
        };
        let Some(prompt) = text
            .lines()
            .map(str::trim)
            .find(|line| !line.is_empty())
        else {
            continue;
        };
        // `order by time_created` puts the session's first prompt first, and
        // the earliest one is the one worth keeping.
        prompts
            .entry(row.session_id)
            .or_insert_with(|| prompt.to_owned());
    }
    prompts
}

/// Reads every session row. A missing CLI, an unreadable database or malformed
/// output yields an empty list: the other connectors still have rows to show.
/// The `sqlite3` invocation every query here uses: JSON output plus a busy
/// timeout.
///
/// The CLI's default busy timeout is zero, so a database the agent CLI is
/// writing to at that moment fails the query outright. Failing means the scan
/// reports Err and the caller keeps its previous rows, which is the honest
/// answer; returning an empty list would look like "this agent has no history"
/// and the empty result would be cached under the new store signature.
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

fn query_sessions(db: &Path) -> Result<Vec<SessionRow>, String> {
    let output = sqlite_query(db)
        .arg(
            "select id, title, directory, time_updated, parent_id \
             from session where id is not null order by time_updated desc",
        )
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
    // `-json` prints nothing at all for an empty result set.
    if text.trim().is_empty() {
        return Ok(Vec::new());
    }
    serde_json::from_str(text).map_err(|error| format!("{} rows unreadable: {error}", db.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Fixtures sit beside the workspace rather than inside the crate, so the
    /// end-to-end scripts under `agent-tests/` read the same samples. These
    /// hold what the `sqlite3` CLI prints, so no database is needed here.
    fn fixture(relative: &str) -> String {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../agent-tests/fixtures/deveco")
            .join(relative);
        std::fs::read_to_string(path).expect("fixture should be readable")
    }

    /// The placeholder is what every session wears until a summariser runs.
    #[test]
    fn is_untitled_matches_only_the_placeholder() {
        assert!(is_untitled(Some("New session - 2026-10-27 03:27:40")));
        assert!(is_untitled(Some("  New session - x")));
        assert!(!is_untitled(Some("Refactor the scanner")));
        assert!(!is_untitled(Some("")));
        assert!(!is_untitled(None));
    }

    /// A sub-agent session is listed through its parent, never on its own.
    #[test]
    fn is_root_session_skips_sub_sessions() {
        let row = |parent_id: Option<&str>| SessionRow {
            id: "ses_1".to_owned(),
            title: None,
            directory: None,
            time_updated: None,
            parent_id: parent_id.map(str::to_owned),
        };
        assert!(is_root_session(&row(None)));
        assert!(is_root_session(&row(Some(""))));
        assert!(!is_root_session(&row(Some("ses_parent"))));
    }

    /// The write-ahead log can hold the newest rows, so it is part of the
    /// change signature.
    #[test]
    fn with_suffix_appends_to_the_whole_path() {
        assert_eq!(
            with_suffix(Path::new("/store/deveco.db"), "-wal"),
            PathBuf::from("/store/deveco.db-wal")
        );
        assert_eq!(
            with_suffix(Path::new("/store/deveco.db"), "-shm"),
            PathBuf::from("/store/deveco.db-shm")
        );
    }

    #[test]
    fn title_for_session_replaces_the_placeholder_with_the_first_prompt() {
        let prompts = HashMap::from([("ses_placeholder".to_owned(), "Add unit tests".to_owned())]);
        assert_eq!(
            title_for_session(
                Some("New session - 2026-10-27 03:27:40"),
                "ses_placeholder",
                &prompts
            )
            .as_deref(),
            Some("Add unit tests")
        );
    }

    /// A session created but never used keeps its placeholder: there is
    /// nothing better to show.
    #[test]
    fn title_for_session_keeps_the_placeholder_without_a_prompt() {
        let prompts = HashMap::new();
        assert_eq!(
            title_for_session(Some("New session - 2026-10-27"), "ses_placeholder", &prompts)
                .as_deref(),
            Some("New session - 2026-10-27")
        );
    }

    /// A blank title is no title at all.
    #[test]
    fn title_for_session_drops_a_blank_title() {
        let prompts = HashMap::new();
        assert_eq!(title_for_session(None, "ses_1", &prompts), None);
        assert_eq!(title_for_session(Some("   "), "ses_1", &prompts), None);
        assert_eq!(
            title_for_session(Some("Refactor the scanner"), "ses_1", &prompts).as_deref(),
            Some("Refactor the scanner")
        );
    }

    #[test]
    fn parse_sessions_reads_query_output() {
        let rows = parse_sessions(&fixture("session_rows.json"), Path::new("deveco.db"));
        assert_eq!(rows.len(), 3, "every row the CLI printed should decode");
        assert_eq!(rows[0].id, "ses_visible");
        assert_eq!(rows[0].title.as_deref(), Some("Refactor the scanner"));
        assert_eq!(rows[0].directory.as_deref(), Some("/repo/demo"));
        assert_eq!(rows[0].time_updated, Some(1793089665165));
        assert_eq!(rows[0].parent_id, None);
        assert_eq!(rows[2].parent_id.as_deref(), Some("ses_visible"));
    }

    /// A half-written or non-JSON line yields no rows instead of failing the
    /// scan.
    #[test]
    fn parse_sessions_survives_truncated_output() {
        let rows = parse_sessions(&fixture("session_rows_corrupt.json"), Path::new("deveco.db"));
        assert!(
            rows.is_empty(),
            "truncated CLI output must decode to no rows"
        );
    }

    /// `-json` prints nothing for an empty result set.
    #[test]
    fn parse_sessions_treats_empty_output_as_no_rows() {
        assert!(
            parse_sessions("", Path::new("deveco.db")).is_empty(),
            "empty CLI output must decode to no rows"
        );
        assert!(
            parse_sessions("  \n", Path::new("deveco.db")).is_empty(),
            "whitespace-only CLI output must decode to no rows"
        );
    }

    /// A prompt can run over several lines and the panel draws one row per
    /// session, so only the first non-empty line of the earliest prompt is
    /// kept.
    #[test]
    fn parse_first_prompts_keeps_the_first_line_of_each_session() {
        let prompts = parse_first_prompts(&fixture("prompt_rows.json"), Path::new("deveco.db"));
        assert_eq!(
            prompts.get("ses_placeholder").map(String::as_str),
            Some("Add unit tests for scanners")
        );
        assert_eq!(
            prompts.get("ses_visible").map(String::as_str),
            Some("Refactor the scanner")
        );
        // A null text is a session with nothing to name it by.
        assert!(!prompts.contains_key("ses_empty"));
    }

    #[test]
    fn parse_first_prompts_survives_truncated_output() {
        let prompts =
            parse_first_prompts(&fixture("prompt_rows_corrupt.json"), Path::new("deveco.db"));
        assert!(
            prompts.is_empty(),
            "truncated prompt output must decode to no prompts"
        );
        assert!(
            parse_first_prompts("", Path::new("deveco.db")).is_empty(),
            "empty prompt output must decode to no prompts"
        );
    }

    /// The row's source path is `<database>/<session id>`, so the database is
    /// the part before the last slash and the id is the rest.
    #[test]
    fn split_source_path_separates_the_database_from_the_id() {
        let (database, session_id) =
            split_source_path(Path::new("/store/deveco.db/ses_visible")).expect("a shape this app writes");
        assert_eq!(database, PathBuf::from("/store/deveco.db"));
        assert_eq!(session_id, "ses_visible");
    }

    /// The id goes into a SQL statement verbatim, so anything beyond the
    /// characters these tools use is refused instead of quoted.
    #[test]
    fn split_source_path_refuses_an_id_it_cannot_place_in_sql() {
        for path in [
            "/store/deveco.db/ses'--",
            "/store/deveco.db/",
            "/store/deveco.db/ses;drop table session;",
        ] {
            let error = split_source_path(Path::new(path))
                .expect_err("an id outside [A-Za-z0-9_-] must be refused");
            assert!(
                error.contains("unexpected id") || error.contains("names no database"),
                "the refusal should name the reason for {path}, got: {error}"
            );
        }
    }

    /// Foreign keys are switched on for this connection: the CLI opens them off
    /// by default, and the cascade is what removes the session's messages.
    #[test]
    fn delete_statement_removes_one_session_with_foreign_keys_on() {
        let statement = delete_statement("ses_visible");
        assert!(statement.starts_with("PRAGMA foreign_keys=ON;"));
        assert_eq!(
            statement,
            "PRAGMA foreign_keys=ON; delete from session where id='ses_visible';"
        );
    }
}

