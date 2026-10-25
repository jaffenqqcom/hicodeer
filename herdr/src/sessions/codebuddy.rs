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

pub(crate) fn collect() -> Vec<CollectedSession> {
    let Some(projects_root) = projects_root() else {
        drop_index();
        return Vec::new();
    };
    let Ok(project_dirs) = std::fs::read_dir(&projects_root) else {
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

    for dir in project_dirs.flatten() {
        let path = dir.path();
        if !path.is_dir() {
            continue;
        }
        let Ok(files) = std::fs::read_dir(&path) else {
            continue;
        };
        for file in files.flatten() {
            let file_path = file.path();
            if file_path.extension().is_none_or(|ext| ext != "jsonl") {
                continue;
            }
            let Ok(metadata) = std::fs::metadata(&file_path) else {
                continue;
            };
            // Empty files are sessions that never got a first turn; they carry
            // no title, no cwd and nothing to open.
            if metadata.len() == 0 {
                continue;
            }
            let modified = metadata.modified().ok();
            let size = metadata.len();

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
    }

    if let Ok(mut guard) = FILE_INDEX.lock() {
        *guard = Some(next);
    }
    collected
}

/// Forgets one file so the next scan cannot serve a row for a deleted session.
pub(super) fn forget_path(path: &Path) {
    if let Ok(mut guard) = FILE_INDEX.lock() {
        if let Some(index) = guard.as_mut() {
            index.remove(path);
        }
    }
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
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The store walk reads the mtime and hands it to the scanner; the tests do
    /// the same, so they exercise the same argument the daemon passes.
    fn file_modified(path: &Path) -> Option<SystemTime> {
        std::fs::metadata(path)
            .ok()
            .and_then(|meta| meta.modified().ok())
    }

    /// Writes a small session file in the real store layout and checks the
    /// scanner picks up title fallback, cwd and timestamps.
    ///
    /// The keys are ordered the way CodeBuddy actually writes them — `id` and
    /// `timestamp` before `type` — because a prefix match on `{"type":` used
    /// to pass this test while dropping every real title line.
    #[test]
    fn scan_session_file_reads_meta_lines_only() {
        let base = std::env::temp_dir().join(format!("herdr-cb-test-{}", std::process::id()));
        let dir = base.join("projects").join("-repo-demo");
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("sess-1.jsonl");
        std::fs::write(
            &file,
            concat!(
                r#"{"id":"m1","timestamp":1000,"type":"message","role":"user","content":[{"type":"input_text","text":"hi"}],"cwd":"/repo/demo"}"#,
                "\n",
                r#"{"id":"t1","timestamp":1100,"type":"ai-title","aiTitle":"Demo session","cwd":"/repo/demo"}"#,
                "\n",
                r#"{"id":"m2","timestamp":1200,"type":"message","role":"assistant","content":"working","cwd":"/repo/demo"}"#,
                "\n",
                "this line is not json\n",
                r#"{"id":"s1","timestamp":1300,"type":"summary","summary":"fallback title"}"#,
                "\n",
            ),
        )
        .unwrap();

        let session = scan_session_file(&file, file_modified(&file)).expect("session should scan");
        assert_eq!(session.session_id, "sess-1");
        assert_eq!(session.title.as_deref(), Some("Demo session"));
        assert_eq!(
            session.workspace_root.as_deref(),
            Some(Path::new("/repo/demo"))
        );
        // The mtime replaces the last line's timestamp, so the scan never has
        // to read past the head window.
        assert!(session.updated_at.is_some());

        let _ = std::fs::remove_dir_all(base);
    }

    /// A session with no `ai-title` line still gets its `summary` line as the
    /// title, whatever position `type` sits at in the object.
    #[test]
    fn scan_session_file_falls_back_to_summary() {
        let base = std::env::temp_dir().join(format!("herdr-cb-sum-{}", std::process::id()));
        let dir = base.join("projects").join("-repo-demo");
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("sess-2.jsonl");
        std::fs::write(
            &file,
            concat!(
                r#"{"id":"s1","timestamp":2000,"type":"summary","summary":"only a summary","cwd":"/repo/demo"}"#,
                "\n",
            ),
        )
        .unwrap();

        let session = scan_session_file(&file, file_modified(&file)).expect("session should scan");
        assert_eq!(session.title.as_deref(), Some("only a summary"));

        let _ = std::fs::remove_dir_all(base);
    }
}
