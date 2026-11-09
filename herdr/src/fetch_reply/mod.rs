//! Per-agent adapters that recover the last assistant reply from an agent's
//! own session store, for IM completion notifications.
//!
//! Each supported agent persists its transcript differently: newline-delimited
//! JSON, a single JSON document, or SQLite. One module per agent keeps each
//! format isolated; this root owns only the registry and shared helpers.
//!
//! A reply supplied by a Stop hook wins over the file: transcript files may be
//! written asynchronously and can lag behind the hook event.

pub(crate) mod screen;

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

mod atomcode;
mod claude;
mod codebuddy;
mod codewhale;
mod codex;
mod deveco;
mod gemini;
mod hermes;
mod kimi;
mod opencode;
mod pi;
mod qwen;
mod zcode;

/// Maximum bytes read from the tail of a transcript file.
const TAIL_BYTES: u64 = 2 * 1024 * 1024;
/// Busy timeout for a `sqlite3` query against a database an agent is writing.
const SQLITE_BUSY_TIMEOUT: &str = ".timeout 2000";

/// An adapter turns a session source path into the last assistant reply.
pub(crate) type Extractor = fn(&Path) -> Result<String, FetchError>;

#[derive(Debug)]
pub(crate) struct FetchError(pub String);

/// Returns the adapter registered for `agent`, if one exists.
pub(crate) fn extractor(agent: &str) -> Option<Extractor> {
    let chosen: Extractor = match agent {
        "atomcode" => atomcode::extract,
        "claude" => claude::extract,
        "codebuddy" => codebuddy::extract,
        "codewhale" => codewhale::extract,
        "codex" => codex::extract,
        "deveco" => deveco::extract,
        "gemini" => gemini::extract,
        "hermes" => hermes::extract,
        "kimi" => kimi::extract,
        "opencode" => opencode::extract,
        "pi" => pi::extract,
        "qwen" => qwen::extract,
        "zcode" => zcode::extract,
        _ => return None,
    };
    Some(chosen)
}

/// Recovers the last assistant reply for `agent`.
///
/// `hook_reply` is the final-text field from the Stop hook payload; when
/// present it is authoritative. Otherwise the agent's own session store at
/// `source_path` is parsed by that agent's adapter.
///
/// Every call reads the store: agents write their transcript asynchronously, so
/// a cached copy can lag behind the turn the caller is asking about.
pub(crate) fn fetch_reply(
    agent: &str,
    source_path: Option<&Path>,
    session_id: Option<&str>,
    cwd: Option<&str>,
    hook_reply: Option<&str>,
) -> Option<String> {
    tracing::info!(
        "fetch_reply: enter agent={agent} source_path={:?} has_hook_reply={}",
        source_path,
        hook_reply.is_some()
    );
    if let Some(text) = hook_reply.map(str::trim).filter(|text| !text.is_empty()) {
        tracing::debug!(
            "fetch_reply: using hook reply ({} chars)",
            text.chars().count()
        );
        return Some(text.to_owned());
    }
    let Some(adapter) = extractor(agent) else {
        tracing::warn!("fetch_reply: no adapter registered for {agent}");
        return None;
    };
    let Some(resolved) = resolve_source_path(agent, session_id, cwd, source_path) else {
        tracing::warn!("fetch_reply: no session path for {agent}");
        return None;
    };
    let path = resolved.as_path();
    match adapter(path) {
        Ok(text) if !text.trim().is_empty() => {
            tracing::info!(
                "fetch_reply: recovered {} chars for {agent}",
                text.chars().count()
            );
            Some(text)
        }
        Ok(_) => {
            tracing::warn!("fetch_reply: empty reply for {agent} at {}", path.display());
            None
        }
        Err(error) => {
            tracing::warn!(
                "fetch_reply: failed for {agent} at {}: {}",
                path.display(),
                error.0
            );
            None
        }
    }
}

/// The store to parse for `agent`.
///
/// Agents that keep every conversation in one database report a hook
/// transcript path that is not the store (zcode's points inside its hook temp
/// file), so the known database plus the session id is the reliable source for
/// them. For file-per-session agents there is no database to fall back to, so
/// the hook path is used as reported.
fn resolve_source_path(
    agent: &str,
    session_id: Option<&str>,
    cwd: Option<&str>,
    source_path: Option<&Path>,
) -> Option<PathBuf> {
    let store = default_source_path(agent, session_id, cwd);
    if matches!(agent, "qwen" | "kimi" | "codewhale" | "gemini") {
        // These report (or can report) their transcript directly; the derived
        // location is only a fallback for hooks that did not carry one.
        return source_path.map(Path::to_path_buf).or(store);
    }
    store.or_else(|| source_path.map(Path::to_path_buf))
}

/// Builds the store path for agents that keep every conversation in one
/// database and therefore report only a session id.
///
/// These agents' hooks carry no transcript path, so the database location comes
/// from the agent itself and the session id is appended in the
/// `<database>/<session_id>` form the SQLite adapters split on.
fn default_source_path(
    agent: &str,
    session_id: Option<&str>,
    cwd: Option<&str>,
) -> Option<PathBuf> {
    let home = PathBuf::from(std::env::var_os("HOME").filter(|home| !home.is_empty())?);
    let session_id = session_id.filter(|id| !id.is_empty())?;
    // Qwen names its transcript after the session id under a directory made
    // from the working directory, with every separator turned into a dash.
    if agent == "qwen" {
        let cwd = cwd.filter(|cwd| !cwd.is_empty())?;
        let project = cwd.replace('/', "-");
        return Some(
            home.join(".qwen/projects")
                .join(project)
                .join("chats")
                .join(format!("{session_id}.jsonl")),
        );
    }
    // Kimi and Codewhale name their stores in ways the hook id does not spell
    // out, so the directory has to be searched instead of composed.
    match agent {
        "kimi" => return find_kimi_transcript(&home, session_id),
        "codewhale" => return find_codewhale_transcript(&home, cwd),
        "gemini" => return find_gemini_transcript(&home, cwd),
        _ => {}
    }
    let relative = match agent {
        "deveco" => ".local/share/deveco/deveco.db",
        "zcode" => ".zcode/cli/db/db.sqlite",
        "opencode" => ".local/share/opencode/opencode.db",
        "hermes" => ".hermes/state.db",
        _ => return None,
    };
    let database = home.join(relative);
    Some(PathBuf::from(format!(
        "{}/{}",
        database.display(),
        session_id
    )))
}

/// Kimi stores each session under `sessions/<project>/<session id>/agents/main`.
///
/// The id arrives either bare or already carrying the `session_` prefix the
/// directory uses, so both spellings are tried.
fn find_kimi_transcript(home: &Path, session_id: &str) -> Option<PathBuf> {
    let root = home.join(".kimi-code/sessions");
    let mut names = vec![session_id.to_owned()];
    if !session_id.starts_with("session_") {
        names.push(format!("session_{session_id}"));
    }
    for project in std::fs::read_dir(root).ok()? {
        let project = project.ok()?.path();
        for name in &names {
            let candidate = project.join(name).join("agents/main/wire.jsonl");
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

/// Codewhale's hook reports `sess_…` while the store names files after a UUID
/// and never records that id, so the newest session for the same workspace is
/// the only mapping available. Only the most recent files are inspected to
/// keep a failed lookup cheap.
fn find_codewhale_transcript(home: &Path, cwd: Option<&str>) -> Option<PathBuf> {
    const INSPECT_LIMIT: usize = 8;
    let cwd = cwd.filter(|cwd| !cwd.is_empty())?;
    let root = home.join(".codewhale/sessions");
    let mut candidates: Vec<(std::time::SystemTime, PathBuf)> = Vec::new();
    for entry in std::fs::read_dir(root).ok()? {
        let path = entry.ok()?.path();
        if path.extension().and_then(|extension| extension.to_str()) != Some("json") {
            continue;
        }
        if path.file_name().and_then(|name| name.to_str()) == Some("session_boot_owners.json") {
            continue;
        }
        let Ok(modified) = std::fs::metadata(&path).and_then(|metadata| metadata.modified()) else {
            continue;
        };
        candidates.push((modified, path));
    }
    candidates.sort_by(|left, right| right.0.cmp(&left.0));
    for (_, path) in candidates.into_iter().take(INSPECT_LIMIT) {
        let Some(document) = std::fs::read_to_string(&path)
            .ok()
            .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        else {
            continue;
        };
        let workspace = document
            .get("metadata")
            .and_then(|metadata| metadata.get("workspace"))
            .and_then(Value::as_str);
        if workspace == Some(cwd) {
            return Some(path);
        }
    }
    None
}

/// Reads the tail of a transcript file. Large session files only need their
/// last turn, so the head is skipped rather than loaded into memory.
pub(super) fn read_tail(path: &Path) -> Result<String, FetchError> {
    let mut file = File::open(path)
        .map_err(|error| FetchError(format!("open {}: {error}", path.display())))?;
    let size = file
        .metadata()
        .map_err(|error| FetchError(format!("stat {}: {error}", path.display())))?
        .len();
    let start = size.saturating_sub(TAIL_BYTES);
    file.seek(SeekFrom::Start(start))
        .map_err(|error| FetchError(format!("seek {}: {error}", path.display())))?;
    let mut buffer = String::new();
    file.read_to_string(&mut buffer)
        .map_err(|error| FetchError(format!("read {}: {error}", path.display())))?;
    Ok(buffer)
}

/// Parses every complete JSON object line in a transcript tail.
pub(super) fn jsonl_records(text: &str) -> Vec<Value> {
    text.lines()
        .filter(|line| line.trim_start().starts_with('{'))
        .filter_map(|line| serde_json::from_str::<Value>(line.trim()).ok())
        .collect()
}

/// Concatenates the text blocks of a message content array or string.
/// Tool calls, thinking blocks and other non-text parts are intentionally
/// skipped; they are not part of the user-visible answer.
pub(super) fn text_blocks(value: Option<&Value>) -> String {
    let Some(value) = value else {
        return String::new();
    };
    match value {
        Value::String(text) => text.to_owned(),
        Value::Array(items) => items
            .iter()
            .filter_map(|item| {
                let object = item.as_object()?;
                // Qwen marks its reasoning parts with `thought: true` and no
                // type; they are not part of the reply.
                if object.get("thought").and_then(Value::as_bool) == Some(true) {
                    return None;
                }
                let kind = object
                    .get("type")
                    .and_then(Value::as_str)
                    .unwrap_or("text");
                if !matches!(kind, "text" | "output_text" | "input_text") {
                    return None;
                }
                object
                    .get("text")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            })
            .collect::<Vec<String>>()
            .join("\n"),
        _ => String::new(),
    }
}

/// Splits a SQLite source path (`<database>/<session id>`) into its parts.
/// Session stores keep many conversations in one database, so the id is
/// required to avoid returning another conversation's reply.
pub(super) fn split_sqlite_source(path: &Path) -> Result<(PathBuf, String), FetchError> {
    let text = path
        .to_str()
        .ok_or_else(|| FetchError(format!("non-utf8 source path {}", path.display())))?;
    let Some((database, session_id)) = text.rsplit_once('/') else {
        return Err(FetchError(format!(
            "source path has no session id: {}",
            path.display()
        )));
    };
    if session_id.is_empty() {
        return Err(FetchError(format!(
            "source path has an empty session id: {}",
            path.display()
        )));
    }
    Ok((PathBuf::from(database), escape_sql_literal(session_id)))
}

fn escape_sql_literal(value: &str) -> String {
    value.replace('\'', "''")
}

fn sqlite_binary() -> PathBuf {
    match std::env::var_os("HOME") {
        Some(home) => PathBuf::from(home).join(".harmonybrew/bin/sqlite3"),
        None => PathBuf::from("sqlite3"),
    }
}

/// Reads the last assistant reply from an OpenCode-style SQLite store
/// (DevEco Code and ZCode share the schema): `part` rows of type `text` belong
/// to a `message` row whose `data.role` is `assistant`. Only the parts of the
/// most recent assistant message are returned, in their original order.
pub(super) fn open_sqlite_reply(path: &Path) -> Result<String, FetchError> {
    let (database, session_id) = split_sqlite_source(path)?;
    let sql = format!(
        "select json_extract(p.data, '$.text') as text from part p \
         where p.message_id = ( \
           select p2.message_id from part p2 \
           join message m2 on p2.message_id = m2.id \
           where p2.session_id = '{session_id}' \
             and json_extract(p2.data, '$.type') = 'text' \
             and json_extract(m2.data, '$.role') = 'assistant' \
           order by p2.time_created desc limit 1 \
         ) \
         and json_extract(p.data, '$.type') = 'text' \
         order by p.time_created asc"
    );
    let rows = sqlite_query(&database, &sql)?;
    let texts: Vec<String> = rows
        .iter()
        .filter_map(|row| row.get("text").and_then(Value::as_str).map(str::to_owned))
        .collect();
    if texts.is_empty() {
        return Err(FetchError(format!(
            "no assistant text in {} for session {}",
            database.display(),
            session_id
        )));
    }
    Ok(texts.join("\n"))
}

/// Runs one `sqlite3 -json` query and decodes the returned rows.
pub(super) fn sqlite_query(database: &Path, sql: &str) -> Result<Vec<Value>, FetchError> {
    let output = Command::new(sqlite_binary())
        .arg("-json")
        .arg("-cmd")
        .arg(SQLITE_BUSY_TIMEOUT)
        .arg(database)
        .arg(sql)
        .output()
        .map_err(|error| FetchError(format!("run sqlite3 for {}: {error}", database.display())))?;
    if !output.status.success() {
        return Err(FetchError(format!(
            "query {} failed: {}",
            database.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    let text = String::from_utf8_lossy(&output.stdout);
    if text.trim().is_empty() {
        return Ok(Vec::new());
    }
    serde_json::from_str(&text)
        .map_err(|error| FetchError(format!("decode rows from {}: {error}", database.display())))
}


/// Gemini keeps one recording per chat under tmp/<slug>/chats, where the slug
/// is the name ~/.gemini/projects.json gave the working directory. The hook
/// usually passes the recording path directly, so this only runs when that
/// path is missing (for example at SessionStart, before the file exists).
fn find_gemini_transcript(home: &Path, cwd: Option<&str>) -> Option<PathBuf> {
    let cwd = cwd.filter(|cwd| !cwd.is_empty())?;
    let slug = gemini_project_slug(home, cwd)?;
    let chats = home.join(".gemini/tmp").join(slug).join("chats");
    let mut newest: Option<(std::time::SystemTime, PathBuf)> = None;
    for entry in std::fs::read_dir(&chats).ok()? {
        let path = entry.ok()?.path();
        if path.extension().and_then(|extension| extension.to_str()) != Some("jsonl") {
            continue;
        }
        let Ok(modified) = std::fs::metadata(&path).and_then(|metadata| metadata.modified()) else {
            continue;
        };
        if newest
            .as_ref()
            .map_or(true, |(newest_time, _)| modified > *newest_time)
        {
            newest = Some((modified, path));
        }
    }
    newest.map(|(_, path)| path)
}

fn gemini_project_slug(home: &Path, cwd: &str) -> Option<String> {
    let raw = std::fs::read_to_string(home.join(".gemini/projects.json")).ok()?;
    let document: Value = serde_json::from_str(&raw).ok()?;
    let projects = document.get("projects")?.as_object()?;
    projects
        .get(cwd)
        .and_then(Value::as_str)
        .map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hook_reply_wins_over_the_file() {
        let reply = fetch_reply("claude", None, None, None, Some("  done  "));
        assert_eq!(reply.as_deref(), Some("done"));
    }

    #[test]
    fn unknown_agent_has_no_adapter() {
        assert!(extractor("unknown").is_none());
        assert_eq!(fetch_reply("unknown", None, None, None, None), None);
    }

    #[test]
    fn text_blocks_skip_thinking_and_tool_parts() {
        let content = serde_json::json!([
            {"type": "think", "text": "hidden"},
            {"type": "text", "text": "first"},
            {"type": "output_text", "text": "second"}
        ]);
        assert_eq!(text_blocks(Some(&content)), "first\nsecond");
    }

    #[test]
    fn a_database_agent_ignores_the_hook_transcript() {
        if std::env::var_os("HOME").is_none() {
            return;
        }
        let resolved = resolve_source_path(
            "zcode",
            Some("sess_1"),
            None,
            Some(Path::new("/tmp/zcode-hook-abc/transcript.jsonl")),
        )
        .expect("resolved");
        let text = resolved.to_string_lossy();
        assert!(text.ends_with("/.zcode/cli/db/db.sqlite/sess_1"), "{text}");
    }

    #[test]
    fn a_file_agent_keeps_the_hook_transcript() {
        let path = Path::new("/tmp/session.jsonl");
        let resolved = resolve_source_path("claude", Some("sess_1"), None, Some(path));
        assert_eq!(resolved.as_deref(), Some(path));
    }

    #[test]
    fn qwen_prefers_the_hook_transcript_and_falls_back_to_the_working_directory() {
        if std::env::var_os("HOME").is_none() {
            return;
        }
        let path = Path::new("/home/u/.qwen/projects/-repo/chats/s1.jsonl");
        assert_eq!(
            resolve_source_path("qwen", Some("s1"), Some("/repo"), Some(path)).as_deref(),
            Some(path)
        );
        let derived =
            resolve_source_path("qwen", Some("s1"), Some("/repo"), None).expect("derived");
        let text = derived.to_string_lossy();
        assert!(text.ends_with("/.qwen/projects/-repo/chats/s1.jsonl"), "{text}");
    }

    #[test]
    fn thought_parts_are_not_part_of_the_reply() {
        let content = serde_json::json!([
            {"text": "reasoning", "thought": true},
            {"text": "answer"}
        ]);
        assert_eq!(text_blocks(Some(&content)), "answer");
    }

    #[test]
    fn kimi_session_directory_is_found_by_id() {
        let root = std::env::temp_dir().join(format!(
            "fetch-reply-kimi-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let wire = root.join(".kimi-code/sessions/project/session_s1/agents/main/wire.jsonl");
        std::fs::create_dir_all(wire.parent().unwrap()).unwrap();
        std::fs::write(&wire, "").unwrap();
        assert_eq!(
            find_kimi_transcript(&root, "s1").as_deref(),
            Some(wire.as_path())
        );
        assert_eq!(
            find_kimi_transcript(&root, "session_s1").as_deref(),
            Some(wire.as_path())
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn codewhale_session_is_found_by_workspace() {
        let root = std::env::temp_dir().join(format!(
            "fetch-reply-codewhale-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let sessions = root.join(".codewhale/sessions");
        std::fs::create_dir_all(&sessions).unwrap();
        std::fs::write(
            sessions.join("a.json"),
            "{\"metadata\":{\"workspace\":\"/repo\"},\"messages\":[]}",
        )
        .unwrap();
        std::fs::write(
            sessions.join("b.json"),
            "{\"metadata\":{\"workspace\":\"/other\"},\"messages\":[]}",
        )
        .unwrap();
        let found = find_codewhale_transcript(&root, Some("/repo")).expect("found");
        assert_eq!(found, sessions.join("a.json"));
        assert!(find_codewhale_transcript(&root, Some("/missing")).is_none());
        let _ = std::fs::remove_dir_all(&root);
    }

            #[test]
    fn gemini_recording_is_found_through_the_project_slug() {
        let root = std::env::temp_dir().join(format!(
            "fetch-reply-gemini-slug-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let chats = root.join(".gemini/tmp/demo/chats");
        std::fs::create_dir_all(&chats).unwrap();
        std::fs::write(
            root.join(".gemini/projects.json"),
            r#"{"projects":{"/repo/demo":"demo"}}"#,
        )
        .unwrap();
        let older = chats.join("session-old.jsonl");
        let newer = chats.join("session-new.jsonl");
        std::fs::write(&older, "{\"type\":\"gemini\",\"content\":\"old\"}\n").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(&newer, "{\"type\":\"gemini\",\"content\":\"new\"}\n").unwrap();

        assert_eq!(
            find_gemini_transcript(&root, Some("/repo/demo")).as_deref(),
            Some(newer.as_path())
        );
        assert!(find_gemini_transcript(&root, Some("/repo/unknown")).is_none());

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn gemini_reads_line_delimited_recordings() {
        let dir = std::env::temp_dir().join(format!(
            "fetch-reply-gemini-jsonl-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("session.jsonl");
        std::fs::write(
            &path,
            "{\"type\":\"user\",\"content\":[{\"text\":\"hi\"}]}\n\
             {\"type\":\"gemini\",\"content\":\"line answer\",\"thoughts\":[]}\n\
             {\"$set\":{\"lastUpdated\":\"2026-01-01T00:00:00Z\"}}\n",
        )
        .unwrap();

        assert_eq!(super::gemini::extract(&path).unwrap(), "line answer");

        let _ = std::fs::remove_dir_all(&dir);
    }

#[test]
    fn gemini_reads_text_from_every_recorded_shape() {
        let dir = std::env::temp_dir().join(format!(
            "fetch-reply-gemini-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("session.json");

        let cases = [
            (
                "string",
                r#"{"messages":[{"type":"user","content":"hi"},{"type":"gemini","content":"plain answer"}]}"#,
                "plain answer",
            ),
            (
                "parts array",
                r#"{"messages":[{"type":"gemini","content":[{"text":"first"},{"text":" second"}]}]}"#,
                "first\n second",
            ),
            (
                "parts object",
                r#"{"messages":[{"type":"gemini","content":{"parts":[{"text":"wrapped"}]}}]}"#,
                "wrapped",
            ),
        ];

        for (label, body, expected) in cases {
            std::fs::write(&path, body).unwrap();
            let reply = super::gemini::extract(&path)
                .unwrap_or_else(|error| panic!("{label}: {}", error.0));
            assert_eq!(reply, expected, "{label}");
        }

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn sqlite_source_path_is_split() {
        let (database, id) =
            split_sqlite_source(Path::new("/data/app.db/ses_abc")).expect("split");
        assert_eq!(database, PathBuf::from("/data/app.db"));
        assert_eq!(id, "ses_abc");
    }
}
