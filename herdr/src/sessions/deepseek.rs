//! DeepSeek Harness history scanner (FAD has no connector for it).
//!
//! Store layout, from `@deepseek-ai/dsh-session-persistence-jsonl`:
//!
//! ```text
//! $DSH_HOME/sessions/            (default: ~/.dsh/sessions)
//!   --<normalized-cwd>--/        (or _no-cwd/)
//!     session-<uuid>/
//!       session.v<N>.jsonl.zstd  (default encoding; N is the store schema)
//!       session.v<N>.jsonl       (only with compression: 'none')
//! ```
//!
//! The schema version is part of the file name and moves with the store, so
//! [`is_transcript`] matches the shape instead of one spelling: a 0.2.x store
//! writes `session.v4.jsonl.zstd`, and an older one wrote `session.jsonl.zstd`.
//!
//! The first logical line of a transcript is its header —
//! `{"type":"session","version":4,"id":"session-…","cwd":…,"createdAt":…,"delegationDepth":0,…}`
//! — and everything after it is the event log. The default artifact is zstd, so
//! the header is decompressed before parsing; plaintext transcripts take the
//! same path with the file reader alone.
//!
//! Header fields are what the row needs: `id` is the value `dsh` resumes by
//! (the directory name is an escaped form of it, deliberately lossy), and `cwd`
//! is the space the session ran in. `delegationDepth` non-zero marks a
//! sub-agent, which is part of its parent's transcript rather than a row. The
//! title is not in the header: it comes from the first user message the event
//! log records, which is why a few more lines are read than the header needs.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use franken_agent_detection::{
    Connector, DetectionResult, DiscoveredSourceFile, DiscoveredSourceRole,
    NormalizedConversation, ScanContext, ScanRoot,
};

use super::CollectedSession;

/// How many lines past the header are inspected for the first user message.
/// The event log puts it within the first handful of records.
const TITLE_SCAN_LINES: usize = 24;

/// Whether a file name is a transcript. The store carries its schema version in
/// the name (`session.v4.jsonl.zstd`), so both the versioned and the bare form
/// are accepted; the compression suffix differs with the store's setting.
fn is_transcript(name: &str) -> bool {
    name.starts_with("session") && (name.ends_with(".jsonl") || name.ends_with(".jsonl.zstd"))
}

#[derive(Clone)]
struct FileEntry {
    modified: Option<SystemTime>,
    size: u64,
    session: CollectedSession,
}

static FILE_INDEX: Mutex<Option<HashMap<PathBuf, FileEntry>>> = Mutex::new(None);

/// One file a scan would read, with the numbers the index keys on.
struct SessionFile {
    path: PathBuf,
    modified: Option<SystemTime>,
    size: u64,
}

/// Where the harness keeps all user data: `$DSH_HOME`, then `~/.dsh`.
fn store_root() -> Option<PathBuf> {
    if let Ok(custom) = std::env::var("DSH_HOME") {
        if !custom.trim().is_empty() {
            return Some(PathBuf::from(custom).join("sessions"));
        }
    }
    std::env::var_os("HOME")
        .filter(|home| !home.is_empty())
        .map(|home| PathBuf::from(home).join(".dsh").join("sessions"))
}

/// Every transcript in the store.
///
/// Both the scan and the source discovery walk this list, so the cache
/// signature always covers exactly the files a scan reads.
fn session_files() -> Option<(PathBuf, Vec<SessionFile>)> {
    let root = store_root().filter(|root| root.is_dir())?;
    let files = session_files_in(&root)?;
    Some((root, files))
}

/// Every transcript under one `sessions/` tree.
///
/// Two levels: one directory per working directory, one per session.
fn session_files_in(root: &Path) -> Option<Vec<SessionFile>> {
    let projects = std::fs::read_dir(root).ok()?;
    let mut files = Vec::new();
    for project in projects.flatten() {
        if !project.path().is_dir() {
            continue;
        }
        let Ok(sessions) = std::fs::read_dir(project.path()) else {
            continue;
        };
        for session in sessions.flatten() {
            let directory = session.path();
            if !directory.is_dir() {
                continue;
            }
            let path = std::fs::read_dir(&directory).ok().and_then(|entries| {
                let mut candidates: Vec<std::path::PathBuf> = entries
                    .flatten()
                    .map(|entry| entry.path())
                    .filter(|path| {
                        path.file_name()
                            .and_then(|name| name.to_str())
                            .is_some_and(is_transcript)
                    })
                    .collect();
                // A store that switched encodings (or schema versions) can hold
                // more than one transcript for one session. Choosing by
                // directory order would be arbitrary and could differ between
                // scans, which would make the store signature meaningless. The
                // compressed form is the store's default, so it wins; the rest
                // break ties by name.
                candidates.sort_by_key(|path| {
                    (
                        !path.extension().is_some_and(|extension| extension == "zstd"),
                        path.clone(),
                    )
                });
                candidates.into_iter().next()
            });
            let Some(path) = path else {
                continue;
            };
            let Ok(metadata) = std::fs::metadata(&path) else {
                continue;
            };
            files.push(SessionFile {
                path,
                modified: metadata.modified().ok(),
                size: metadata.len(),
            });
        }
    }
    Some(files)
}

pub(crate) fn collect() -> Vec<CollectedSession> {
    let Some((_root, files)) = session_files() else {
        drop_index();
        return Vec::new();
    };

    let previous = FILE_INDEX
        .lock()
        .ok()
        .and_then(|mut guard| guard.take())
        .unwrap_or_default();
    let mut next = HashMap::new();
    let mut collected = Vec::new();

    for file in files {
        let file_path = file.path;
        let modified = file.modified;
        let size = file.size;

        if let Some(entry) = previous.get(&file_path) {
            if entry.modified == modified && entry.size == size {
                collected.push(entry.session.clone());
                next.insert(file_path, entry.clone());
                continue;
            }
        }

        if let Some(session) = scan_session_file(&file_path, modified) {
            next.insert(
                file_path,
                FileEntry {
                    modified,
                    size,
                    session: session.clone(),
                },
            );
            collected.push(session);
        }
    }

    if let Ok(mut guard) = FILE_INDEX.lock() {
        *guard = Some(next);
    }
    collected
}

fn drop_index() {
    if let Ok(mut guard) = FILE_INDEX.lock() {
        *guard = None;
    }
}

/// Removes one transcript and drops the parse index.
///
/// The index maps a file to what was parsed from it, so a removed file has to
/// invalidate it; the next scan rebuilds it from what is on disk.
pub(super) fn delete_session(path: &Path) -> Result<(), String> {
    let Some(root) = store_root() else {
        return Err("no DeepSeek Harness store on this device".to_string());
    };
    let root = root
        .canonicalize()
        .map_err(|error| format!("resolve {}: {error}", root.display()))?;
    delete_in(&root, path)?;
    drop_index();
    Ok(())
}

/// The removal itself, with the store root already resolved.
fn delete_in(root: &Path, path: &Path) -> Result<(), String> {
    let resolved = path
        .canonicalize()
        .map_err(|error| format!("resolve {}: {error}", path.display()))?;
    if !resolved.starts_with(root) {
        return Err(format!(
            "refusing to delete {}: outside the store",
            path.display()
        ));
    }
    std::fs::remove_file(&resolved)
        .map_err(|error| format!("delete {}: {error}", path.display()))
}

/// Reads a transcript's header line. Anything unreadable — a torn first frame, a
/// store of the other encoding, a log without a header — is skipped instead of
/// failing the whole scan.
fn scan_session_file(path: &Path, modified: Option<SystemTime>) -> Option<CollectedSession> {
    let handle = std::fs::File::open(path).ok()?;
    let reader: Box<dyn Read> = if path.extension().is_some_and(|extension| extension == "zstd") {
        Box::new(zstd::stream::read::Decoder::new(handle).ok()?)
    } else {
        Box::new(handle)
    };
    let mut lines = BufReader::new(reader).lines();
    let header_line = lines.next()?.ok()?;
    let header: serde_json::Value = serde_json::from_str(&header_line).ok()?;

    if header.get("type").and_then(|value| value.as_str()) != Some("session") {
        return None;
    }
    // A delegated session is part of its parent's transcript, not a row.
    if header
        .get("delegationDepth")
        .and_then(|value| value.as_u64())
        .unwrap_or(0)
        != 0
    {
        return None;
    }
    let session_id = header.get("id").and_then(|value| value.as_str())?;
    if session_id.is_empty() {
        return None;
    }

    Some(CollectedSession {
        agent: "deepseek",
        session_id: session_id.to_owned(),
        title: lines
            .take(TITLE_SCAN_LINES)
            .find_map(|line| line.ok().and_then(|line| first_user_text(&line))),
        workspace_root: header
            .get("cwd")
            .and_then(|value| value.as_str())
            .filter(|cwd| !cwd.is_empty())
            .map(PathBuf::from),
        updated_at: modified
            .and_then(|mtime| mtime.duration_since(UNIX_EPOCH).ok())
            .map(|elapsed| elapsed.as_millis() as i64),
        source_path: path.to_path_buf(),
        deletable: true,
    })
}

/// Pulls the first user message out of one event line. The store writes the
/// prompt as an inbox splice (`agent/inbox/spliced`) whose `inserted` entries
/// carry the message content, so a title is available without replaying the log.
fn first_user_text(line: &str) -> Option<String> {
    let event: serde_json::Value = serde_json::from_str(line).ok()?;
    if event.get("type").and_then(|value| value.as_str()) != Some("agent/inbox/spliced") {
        return None;
    }
    let inserted = event.get("data")?.get("inserted")?.as_array()?;
    for message in inserted {
        if message.get("role").and_then(|value| value.as_str()) != Some("user") {
            continue;
        }
        let Some(parts) = message.get("content").and_then(|value| value.as_array()) else {
            continue;
        };
        let text = parts
            .iter()
            .filter(|part| part.get("type").and_then(|value| value.as_str()) == Some("text"))
            .filter_map(|part| part.get("text").and_then(|value| value.as_str()))
            .collect::<Vec<_>>()
            .join(" ");
        let text = text.trim();
        if !text.is_empty() {
            return Some(text.to_owned());
        }
    }
    None
}

/// DeepSeek Harness behind FAD's connector interface.
pub(super) struct DeepSeekConnector;

impl DeepSeekConnector {
    pub(super) const fn new() -> Self {
        Self
    }
}

impl Connector for DeepSeekConnector {
    fn detect(&self) -> DetectionResult {
        match store_root().filter(|root| root.is_dir()) {
            Some(root) => DetectionResult {
                detected: true,
                evidence: vec![root.display().to_string()],
                root_paths: vec![root],
            },
            None => DetectionResult::not_found(),
        }
    }

    fn scan(&self, _ctx: &ScanContext) -> anyhow::Result<Vec<NormalizedConversation>> {
        Ok(collect()
            .into_iter()
            .map(|session| NormalizedConversation {
                agent_slug: "deepseek".to_owned(),
                external_id: Some(session.session_id),
                title: session.title,
                workspace: session.workspace_root,
                source_path: session.source_path,
                started_at: None,
                ended_at: session.updated_at,
                metadata: serde_json::Value::Null,
                messages: Vec::new(),
            })
            .collect())
    }

    /// The files `scan` would read, in the shape the scan cache expects.
    fn discover_source_files(
        &self,
        _ctx: &ScanContext,
    ) -> anyhow::Result<Vec<DiscoveredSourceFile>> {
        let Some((root, files)) = session_files() else {
            return Ok(Vec::new());
        };
        let scan_root = ScanRoot::local(root);
        Ok(files
            .into_iter()
            .map(|file| {
                let mut discovered = DiscoveredSourceFile::new(
                    "deepseek",
                    &scan_root,
                    file.path,
                    DiscoveredSourceRole::PrimarySessionLog,
                    true,
                );
                discovered.size_bytes = Some(file.size);
                discovered.modified_at_ms = file.modified.and_then(|modified| {
                    modified
                        .duration_since(UNIX_EPOCH)
                        .ok()
                        .map(|elapsed| elapsed.as_millis() as i64)
                });
                discovered
            })
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Fixtures sit beside the workspace rather than inside the crate, so the
    /// end-to-end scripts under `agent-tests/` read the same samples.
    fn fixture(relative: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../agent-tests/fixtures/deepseek")
            .join(relative)
    }

    /// The store walk reads the mtime and hands it to the scanner; the tests do
    /// the same.
    fn file_modified(path: &Path) -> Option<SystemTime> {
        std::fs::metadata(path)
            .ok()
            .and_then(|meta| meta.modified().ok())
    }

    /// The schema version is part of the file name and moves with the store, so
    /// both the versioned and the bare spelling have to match, with either
    /// encoding.
    #[test]
    fn is_transcript_accepts_both_encodings_and_versions() {
        assert!(is_transcript("session.v4.jsonl.zstd"));
        assert!(is_transcript("session.v4.jsonl"));
        assert!(is_transcript("session.jsonl.zstd"));
        assert!(!is_transcript("session.lock"));
        assert!(!is_transcript("session_boot_owners.json"));
        assert!(!is_transcript("other.v4.jsonl"));
    }

    #[test]
    fn scan_session_file_reads_header_and_first_prompt() {
        let path = fixture("sessions/--repo-demo--/session-alpha/session.v4.jsonl");
        let session = scan_session_file(&path, file_modified(&path)).expect("session should scan");
        assert_eq!(session.agent, "deepseek");
        assert_eq!(session.session_id, "session-alpha");
        // Every `text` part of the first user turn joins into one title.
        assert_eq!(
            session.title.as_deref(),
            Some("Fix the build and run the tests")
        );
        assert_eq!(
            session.workspace_root.as_deref(),
            Some(Path::new("/repo/demo"))
        );
        assert_eq!(session.source_path, path);
        assert!(
            session.updated_at.is_some_and(|updated| updated > 0),
            "mtime should become a positive update time, got {:?}",
            session.updated_at
        );
    }

    /// A delegated session is part of its parent's transcript, not a row.
    #[test]
    fn scan_session_file_skips_delegated_session() {
        let path = fixture("sessions/--repo-demo--/session-delegated/session.v4.jsonl");
        assert!(
            scan_session_file(&path, file_modified(&path)).is_none(),
            "delegationDepth = 1 must be filtered out of the row list"
        );
    }

    /// A torn first frame must drop one session, not the whole scan.
    #[test]
    fn scan_session_file_skips_torn_header() {
        let path = fixture("sessions/--repo-demo--/session-broken/session.v4.jsonl");
        assert!(
            scan_session_file(&path, file_modified(&path)).is_none(),
            "a header that is not valid JSON must yield no row"
        );
    }

    /// A session with no user turn yet has no title, rather than an empty one.
    #[test]
    fn scan_session_file_has_no_title_without_user_turns() {
        let path = fixture("sessions/--repo-demo--/session-untitled/session.v4.jsonl");
        let session = scan_session_file(&path, file_modified(&path)).expect("session should scan");
        assert_eq!(session.session_id, "session-untitled");
        assert_eq!(session.title, None);
        assert_eq!(
            session.workspace_root.as_deref(),
            Some(Path::new("/repo/demo"))
        );
    }

    /// The default store encoding is zstd; the header has to be decompressed
    /// before it can be read. The fixture is compressed at test time so the
    /// sample itself stays reviewable.
    #[test]
    fn scan_session_file_reads_zstd_transcript() {
        let source = fixture("sessions/--repo-demo--/session-alpha/session.v4.jsonl");
        let bytes = std::fs::read(&source).expect("fixture should be readable");
        let compressed = zstd::stream::encode_all(bytes.as_slice(), 0).expect("fixture should compress");

        let directory =
            std::env::temp_dir().join(format!("herdr-deepseek-zstd-{}", std::process::id()));
        std::fs::create_dir_all(&directory).expect("temp directory should be creatable");
        let path = directory.join("session.v4.jsonl.zstd");
        std::fs::write(&path, compressed).expect("transcript should be writable");

        let session = scan_session_file(&path, file_modified(&path)).expect("session should scan");
        assert_eq!(session.session_id, "session-alpha");
        assert_eq!(
            session.title.as_deref(),
            Some("Fix the build and run the tests")
        );

        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn scan_session_file_missing_file_returns_none() {
        let path = fixture("sessions/--repo-demo--/session-gone/session.v4.jsonl");
        assert!(
            scan_session_file(&path, None).is_none(),
            "a missing transcript must yield no row"
        );
    }

    /// One transcript per session directory; a directory holding only its lock
    /// file has no transcript and no row.
    #[test]
    fn session_files_in_finds_one_transcript_per_session() {
        let files = session_files_in(&fixture("sessions")).expect("the store should be readable");
        assert_eq!(files.len(), 4, "one transcript per session directory");
        assert!(
            files.iter().all(|file| file
                .path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(is_transcript)),
            "only transcript file names are picked up"
        );
        assert!(files.iter().all(|file| file.size > 0));
    }

    /// An unreachable store yields no files instead of an error, so the scan
    /// clears its index rather than serving stale rows.
    #[test]
    fn session_files_in_tolerates_a_missing_store() {
        let missing =
            std::env::temp_dir().join(format!("herdr-deepseek-missing-{}", std::process::id()));
        assert!(
            session_files_in(&missing).is_none(),
            "the store walk must return None for an unreachable directory"
        );
    }

    /// Only the spliced inbox event carries a user turn, and only its user
    /// messages count.
    #[test]
    fn first_user_text_reads_user_turns_only() {
        let spliced = concat!(
            r#"{"type":"agent/inbox/spliced","seq":3,"data":{"inserted":["#,
            r#"{"role":"assistant","content":[{"type":"text","text":"no"}]},"#,
            r#"{"role":"user","content":[{"type":"text","text":"yes"}]}]}}"#,
        );
        assert_eq!(first_user_text(spliced).as_deref(), Some("yes"));
        assert_eq!(first_user_text(r#"{"type":"permission/preset"}"#), None);
        assert_eq!(first_user_text("not json"), None);
    }

    /// A directory this test owns, cleared before use so a previous run cannot
    /// make the assertions pass by accident.
    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("herdr-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create the scratch directory");
        dir
    }

    /// A transcript inside the store is removed, whether or not it is
    /// compressed: the store keeps the encoding in the file name.
    #[test]
    fn delete_in_removes_a_transcript_inside_the_store() {
        let store = scratch("deepseek-delete");
        let session = store.join("--repo-demo--").join("session-alpha");
        std::fs::create_dir_all(&session).expect("create the session directory");
        let transcript = session.join("session.v4.jsonl.zstd");
        std::fs::write(&transcript, "header\n").expect("write the transcript");
        let root = store.canonicalize().expect("resolve the store");

        delete_in(&root, &transcript).expect("a path inside the store is removable");

        assert!(!transcript.exists(), "the transcript should be gone");
        assert!(
            session.exists(),
            "only the transcript goes; its directory stays"
        );
        std::fs::remove_dir_all(&store).ok();
    }

    /// A path outside the store is refused before anything is unlinked.
    #[test]
    fn delete_in_refuses_a_path_outside_the_store() {
        let store = scratch("deepseek-outside");
        let outside = store.join("outside");
        std::fs::create_dir_all(&outside).expect("create the outside directory");
        let transcript = outside.join("session.v4.jsonl");
        std::fs::write(&transcript, "header\n").expect("write the transcript");
        let root = store.join("sessions");
        std::fs::create_dir_all(&root).expect("create the store");
        let root = root.canonicalize().expect("resolve the store");

        let error = delete_in(&root, &transcript).expect_err("outside the store must be refused");

        assert!(
            error.contains("outside the store"),
            "the refusal should name the reason, got: {error}"
        );
        assert!(
            transcript.exists(),
            "a refused delete must leave the file alone"
        );
        std::fs::remove_dir_all(&store).ok();
    }
}
