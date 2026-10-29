//! CodeBuddy history scanner (our own; FAD has no connector for it).
//!
//! CodeBuddy stores sessions the same way Claude does:
//! `~/.codebuddy/projects/<encoded>/<session-id>.jsonl`, one JSON object per
//! line. Two differences from Claude drive this file:
//!
//! 1. The directory encoding replaces every `/` with `-` **except** the
//!    leading one (Claude replaces that one too), so the directory name
//!    cannot be decoded back into a path. The working directory is read from
//!    the `cwd` field inside the file instead.
//! 2. Titles prefer the `ai-title` line's `aiTitle` field, then the
//!    `summary` line's `summary` field.
//!
//! Sessions can reach 140 MiB and the store holds ~760 MiB in total, so the
//! scanner keeps an index of what it already parsed: a session file whose
//! mtime and size are unchanged is answered from the index without opening it,
//! and only new or changed files are read. Corrupt lines are skipped.

use std::collections::HashMap;
use std::io::{BufRead, Read};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use franken_agent_detection::{
    Connector, DetectionResult, DiscoveredSourceFile, DiscoveredSourceRole,
    NormalizedConversation, ScanContext, ScanRoot,
};

use super::CollectedSession;

/// How much of a session file is read. Titles and the working directory sit
/// in the first lines: measured across the 358 files of the real store the
/// `cwd` line ends by byte 627 and the `ai-title` line sits at 1.4 KiB at the
/// median, 2.6 KiB at p90 — only 3 files push it past 32 KiB. The window is
/// 64 KiB so those fit too. Reading to EOF instead cost seconds per scan
/// (files reach 140 MiB, ~760 MiB in total); anything past the window is
/// message bodies we do not need, and the update time comes from the file
/// mtime rather than the last line.
const HEAD_WINDOW_BYTES: u64 = 64 * 1024;

/// Markers used to skip plain message lines without a JSON decode. CodeBuddy
/// writes keys in whatever order the recorder emitted, so a title line really
/// looks like `{"id":…,"timestamp":…,"type":"ai-title","aiTitle":…}` — matching
/// on a `{"type":` prefix silently drops every one of them. Probing for the
/// marker anywhere in the line keeps the fast path without depending on key
/// order; the decode below still verifies the payload.
const TITLE_MARKER: &str = r#""type":"ai-title""#;
const SUMMARY_MARKER: &str = r#""type":"summary""#;

/// What one session file looked like when it was last parsed. While the mtime
/// and size still match, the parsed session is reused and the file is not
/// opened at all.
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

/// Every session file in the store.
///
/// Both the scan and the source discovery walk this list, which is what keeps
/// the cache signature covering exactly the files a scan reads.
fn session_files() -> Option<(PathBuf, Vec<SessionFile>)> {
    let root = projects_root().filter(|root| root.is_dir())?;
    let files = session_files_in(&root)?;
    Some((root, files))
}

/// Every session file under one `projects/` tree.
///
/// Empty files are skipped: they are sessions that never got a first turn, so
/// they carry no title, no cwd and nothing to open.
fn session_files_in(root: &Path) -> Option<Vec<SessionFile>> {
    let project_dirs = std::fs::read_dir(root).ok()?;
    let mut files = Vec::new();
    for dir in project_dirs.flatten() {
        let path = dir.path();
        if !path.is_dir() {
            continue;
        }
        let Ok(entries) = std::fs::read_dir(&path) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_none_or(|ext| ext != "jsonl") {
                continue;
            }
            let Ok(metadata) = std::fs::metadata(&path) else {
                continue;
            };
            if metadata.len() == 0 {
                continue;
            }
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

    // Take the previous index so this scan owns it: anything not seen again is
    // dropped, which is how sessions deleted by the user or the agent go away.
    let previous = FILE_INDEX
        .lock()
        .ok()
        .and_then(|mut guard| guard.take())
        .unwrap_or_default();
    let mut next: HashMap<PathBuf, FileEntry> = HashMap::new();
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

/// Removes one transcript from the projects tree.
///
/// The path arrives over the API, so it is checked against this store before
/// anything is unlinked: an application process cannot unlink here itself (it
/// gets EPERM), which is why the daemon performs it.
///
/// The parse index needs no eviction: a scan rebuilds it from the files that
/// exist, so a removed transcript is simply not seen again.
pub(super) fn delete_session(path: &Path) -> Result<(), String> {
    let Some(root) = projects_root() else {
        return Err("no CodeBuddy store on this device".to_string());
    };
    let root = root
        .canonicalize()
        .map_err(|error| format!("resolve {}: {error}", root.display()))?;
    delete_in(&root, path)
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
        .map_err(|error| format!("delete {}: {error}", path.display()))?;
    Ok(())
}

/// Clears the index when the store itself is unreachable, so a reinstall does
/// not serve stale entries.
fn drop_index() {
    if let Ok(mut guard) = FILE_INDEX.lock() {
        *guard = None;
    }
}

pub(super) fn projects_root() -> Option<PathBuf> {
    if let Ok(custom) = std::env::var("CODEBUDDY_CONFIG_DIR") {
        if !custom.is_empty() {
            return Some(PathBuf::from(custom).join("projects"));
        }
    }
    home_dir().map(|home| home.join(".codebuddy").join("projects"))
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .filter(|home| !home.is_empty())
        .map(PathBuf::from)
}

/// Reads the leading lines of one session file. `modified` is the mtime the
/// caller already read while walking the store: the update time comes from it
/// rather than from the last line, which is what keeps the read to one window.
fn scan_session_file(path: &Path, modified: Option<SystemTime>) -> Option<CollectedSession> {
    let session_id = path.file_stem()?.to_string_lossy().into_owned();
    let file = std::fs::File::open(path).ok()?;
    let reader = std::io::BufReader::new(file.take(HEAD_WINDOW_BYTES));

    let mut title: Option<String> = None;
    let mut summary: Option<String> = None;
    let mut cwd: Option<String> = None;

    for line in reader.lines() {
        // A torn or corrupt line must not take the whole scan down.
        let Ok(line) = line else { continue };
        let trimmed = line.trim_start();
        if trimmed.is_empty() {
            continue;
        }
        if title.is_none() && trimmed.contains(TITLE_MARKER) {
            if let Ok(value) = serde_json::from_str::<serde_json::Value>(trimmed) {
                if let Some(t) = value.get("aiTitle").and_then(|v| v.as_str()) {
                    title = Some(t.to_owned());
                }
            }
        }
        if summary.is_none() && trimmed.contains(SUMMARY_MARKER) {
            if let Ok(value) = serde_json::from_str::<serde_json::Value>(trimmed) {
                if let Some(s) = value.get("summary").and_then(|v| v.as_str()) {
                    summary = Some(s.to_owned());
                }
            }
        }
        if cwd.is_none() && trimmed.contains(r#""cwd""#) {
            if let Ok(value) = serde_json::from_str::<serde_json::Value>(trimmed) {
                if let Some(c) = value.get("cwd").and_then(|v| v.as_str()) {
                    cwd = Some(c.to_owned());
                }
            }
        }
        // The title is what we came for; cwd comes from the same leading lines.
        if title.is_some() && cwd.is_some() {
            break;
        }
    }

    Some(CollectedSession {
        agent: "codebuddy",
        session_id,
        title: title.or(summary),
        workspace_root: cwd.map(PathBuf::from),
        updated_at: modified
            .and_then(|mtime| mtime.duration_since(UNIX_EPOCH).ok())
            .map(|elapsed| elapsed.as_millis() as i64),
        source_path: path.to_path_buf(),
        deletable: true,
    })
}

/// CodeBuddy behind FAD's connector interface.
///
/// FAD has no CodeBuddy connector, but the panel and the scan cache are built
/// around that trait, so the scanner above is wrapped instead of kept on a
/// side path. The parse stays here: the working directory comes from the
/// transcript's own `cwd` line, which is the only way to recover the space
/// because the directory name cannot be decoded back into a path.
pub(super) struct CodeBuddyConnector;

impl CodeBuddyConnector {
    pub(super) const fn new() -> Self {
        Self
    }
}

impl Connector for CodeBuddyConnector {
    fn detect(&self) -> DetectionResult {
        match projects_root().filter(|root| root.is_dir()) {
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
                agent_slug: "codebuddy".to_owned(),
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
    ///
    /// Without this the cache would key on an empty list, sign as zero forever
    /// and keep serving the first answer — new sessions would never appear.
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
                    "codebuddy",
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
            .join("../agent-tests/fixtures/codebuddy")
            .join(relative)
    }

    /// The store walk reads the mtime and hands it to the scanner; the tests do
    /// the same, so they exercise the same argument the daemon passes.
    fn file_modified(path: &Path) -> Option<SystemTime> {
        std::fs::metadata(path)
            .ok()
            .and_then(|meta| meta.modified().ok())
    }

    /// A real-layout session file: the leading line is corrupt, and the title
    /// line carries `id` and `timestamp` before `type` the way CodeBuddy
    /// writes it, because a prefix match on `{"type":` used to pass a test
    /// while dropping every real title line.
    #[test]
    fn scan_session_file_reads_meta_lines_only() {
        let path = fixture("projects/-repo-demo/demo-session.jsonl");
        let session = scan_session_file(&path, file_modified(&path)).expect("session should scan");
        assert_eq!(session.agent, "codebuddy");
        assert_eq!(session.session_id, "demo-session");
        assert_eq!(session.title.as_deref(), Some("Demo session"));
        assert_eq!(
            session.workspace_root.as_deref(),
            Some(Path::new("/repo/demo"))
        );
        assert_eq!(session.source_path, path);
        // The mtime replaces the last line's timestamp, so the scan never has
        // to read past the head window.
        assert!(
            session.updated_at.is_some_and(|updated| updated > 0),
            "mtime should become a positive update time, got {:?}",
            session.updated_at
        );
    }

    /// A session with no `ai-title` line still gets its `summary` line as the
    /// title.
    #[test]
    fn scan_session_file_falls_back_to_summary() {
        let path = fixture("projects/-repo-demo/summary-only.jsonl");
        let session = scan_session_file(&path, file_modified(&path)).expect("session should scan");
        assert_eq!(session.title.as_deref(), Some("only a summary"));
    }

    /// Neither line present: no title at all, rather than an empty string.
    #[test]
    fn scan_session_file_has_no_title_without_title_lines() {
        let path = fixture("projects/-repo-demo/no-title.jsonl");
        let session = scan_session_file(&path, file_modified(&path)).expect("session should scan");
        assert_eq!(session.title, None);
        assert_eq!(
            session.workspace_root.as_deref(),
            Some(Path::new("/repo/demo"))
        );
    }

    /// A deleted session must not fail the scan; it simply has no row.
    #[test]
    fn scan_session_file_missing_file_returns_none() {
        let path = fixture("projects/-repo-demo/does-not-exist.jsonl");
        assert!(
            scan_session_file(&path, None).is_none(),
            "a missing session file must yield no row"
        );
    }

    /// Empty files are sessions that never got a first turn, and only `.jsonl`
    /// files are transcripts.
    #[test]
    fn session_files_in_skips_empty_and_foreign_files() {
        let files = session_files_in(&fixture("projects")).expect("the store should be readable");
        assert_eq!(files.len(), 3, "only non-empty .jsonl files are sessions");
        assert!(
            files.iter().all(|file| file.size > 0),
            "an empty file must be skipped"
        );
        assert!(
            files
                .iter()
                .all(|file| file.path.extension().is_some_and(|ext| ext == "jsonl")),
            "only .jsonl files are transcripts"
        );
    }

    /// An unreachable store yields no files instead of an error, so the scan
    /// clears its index rather than serving stale rows.
    #[test]
    fn session_files_in_tolerates_a_missing_store() {
        let missing =
            std::env::temp_dir().join(format!("hicodeer-agent-codebuddy-missing-{}", std::process::id()));
        assert!(
            session_files_in(&missing).is_none(),
            "the store walk must return None for an unreachable directory"
        );
    }
    /// A directory this test owns, cleared before use so a previous run cannot
    /// make the assertions pass by accident.
    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("hicodeer-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create the scratch directory");
        dir
    }

    /// A transcript inside the store is removed.
    #[test]
    fn delete_in_removes_a_transcript_inside_the_store() {
        let store = scratch("codebuddy-delete");
        let project = store.join("project");
        std::fs::create_dir_all(&project).expect("create the project directory");
        let transcript = project.join("session.jsonl");
        std::fs::write(&transcript, "{}\n").expect("write the transcript");
        let root = store.canonicalize().expect("resolve the store");

        delete_in(&root, &transcript).expect("a path inside the store is removable");

        assert!(!transcript.exists(), "the transcript should be gone");
        std::fs::remove_dir_all(&store).ok();
    }

    /// A path outside the store is refused before anything is unlinked.
    #[test]
    fn delete_in_refuses_a_path_outside_the_store() {
        let store = scratch("codebuddy-outside");
        let outside = store.join("outside");
        std::fs::create_dir_all(&outside).expect("create the outside directory");
        let transcript = outside.join("session.jsonl");
        std::fs::write(&transcript, "{}\n").expect("write the transcript");
        let root = store.join("projects");
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
