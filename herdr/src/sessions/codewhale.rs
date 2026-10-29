//! History scanner for Codewhale (`codewhale`).
//!
//! Codewhale keeps one JSON document per session under its state directory:
//!
//! ```text
//! ~/.codewhale/sessions/<uuid>.json
//!   metadata: { id, title, workspace, created_at, updated_at, … }
//!   messages: [ … ]
//!   journal:  { spawn_depth, … }
//! ```
//!
//! FAD has no Codewhale connector, so this one reads the document directly and
//! keeps only the metadata block: the panel shows a title, the directory the
//! session ran in and when it was last touched, and nothing else is used. The
//! document grows with the transcript (`messages` plus a `system_prompt` that is
//! already 20 KB in a one-turn session), so it is streamed through
//! `serde_json::from_reader` with only `metadata` and `journal` declared: serde
//! walks the fields it was not asked for without building a tree, which keeps
//! the scanner's memory flat no matter how long the session got.
//!
//! `updated_at` is RFC 3339 text rather than an integer, and the `time` crate is
//! built here without its parser, so the file's modification time stands in for
//! it: Codewhale rewrites the document whenever the session advances, which
//! makes the two agree to within one write.

use std::path::{Path, PathBuf};

use franken_agent_detection::{
    Connector, DetectionResult, DiscoveredSourceFile, DiscoveredSourceRole,
    NormalizedConversation, ScanContext, ScanRoot,
};

/// The agent label the panel matches against `agent.list`. FAD does not know
/// this tool, so the slug is ours to choose and this is the spelling the panel
/// is told to expect.
const SLUG: &str = "codewhale";

/// Sessions deeper than this belong to a sub-agent and would show the same
/// conversation twice.
const ROOT_SPAWN_DEPTH: i64 = 0;

/// Codewhale's bookkeeping file beside the sessions. It maps a session id to the
/// window that has it open, so it belongs to no single session and must never be
/// removed by a session delete.
const BOOT_OWNERS_FILE: &str = "session_boot_owners.json";

pub(super) struct CodewhaleConnector;

impl CodewhaleConnector {
    pub(super) fn new() -> Self {
        Self
    }
}

impl Connector for CodewhaleConnector {
    fn detect(&self) -> DetectionResult {
        match sessions_root().filter(|path| path.is_dir()) {
            Some(root) => DetectionResult {
                detected: true,
                evidence: vec![root.display().to_string()],
                root_paths: vec![root],
            },
            None => DetectionResult::not_found(),
        }
    }

    fn scan(&self, _ctx: &ScanContext) -> anyhow::Result<Vec<NormalizedConversation>> {
        let Some(root) = sessions_root().filter(|path| path.is_dir()) else {
            return Ok(Vec::new());
        };
        Ok(session_files(&root)
            .into_iter()
            .filter_map(|path| read_session(&path))
            .collect())
    }

    fn discover_source_files(
        &self,
        _ctx: &ScanContext,
    ) -> anyhow::Result<Vec<DiscoveredSourceFile>> {
        let Some(root) = sessions_root().filter(|path| path.is_dir()) else {
            return Ok(Vec::new());
        };
        let scan_root = ScanRoot::local(root.clone());
        let mut files = Vec::new();
        for path in session_files(&root) {
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

fn sessions_root() -> Option<PathBuf> {
    home().map(|home| home.join(".codewhale").join("sessions"))
}

/// Removes one session: its document, the directory of the same name beside it,
/// and its entry in the boot-owner map.
///
/// The boot-owner entry is what makes a later resume say "open in another
/// window", so it has to go with the session.
pub(super) fn delete_session(path: &Path) -> Result<(), String> {
    let Some(root) = sessions_root() else {
        return Err("no Codewhale store on this device".to_string());
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
    // One session is one document in the store's own directory. The boot-owner
    // map sits beside them and belongs to every session, so it is excluded here
    // rather than only in the list the panel is given.
    let is_document = resolved.parent() == Some(root)
        && resolved.extension().and_then(|extension| extension.to_str()) == Some("json")
        && resolved.file_name().and_then(|name| name.to_str()) != Some(BOOT_OWNERS_FILE);
    if !is_document {
        return Err(format!(
            "refusing to delete {}: not a session document",
            path.display()
        ));
    }
    let Some(session_id) = resolved
        .file_stem()
        .and_then(|stem| stem.to_str())
        .map(str::to_owned)
    else {
        return Err(format!("{} names no session", path.display()));
    };
    // The id is about to name a directory, so it has to look like one of the
    // ids Codewhale writes; `.` and `..` would otherwise point at the store
    // itself.
    let well_formed = !session_id.is_empty()
        && session_id
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '-');
    if !well_formed {
        return Err(format!(
            "refusing to delete {}: unexpected session id",
            path.display()
        ));
    }

    std::fs::remove_file(&resolved)
        .map_err(|error| format!("delete {}: {error}", path.display()))?;
    let sidecar = root.join(&session_id);
    if sidecar.is_dir() {
        std::fs::remove_dir_all(&sidecar)
            .map_err(|error| format!("delete {}: {error}", sidecar.display()))?;
    }
    forget_boot_owner(&root, &session_id);
    Ok(())
}

/// Drops one entry from `session_boot_owners.json`, leaving the rest alone.
///
/// Written through a sibling file and a rename: the map is shared by every
/// session, so a write that stopped halfway would lose the boot owners of
/// sessions that were never touched.
fn forget_boot_owner(root: &Path, session_id: &str) {
    let owners_path = root.join(BOOT_OWNERS_FILE);
    let Ok(text) = std::fs::read_to_string(&owners_path) else {
        return;
    };
    let Ok(serde_json::Value::Object(mut owners)) = serde_json::from_str(&text) else {
        return;
    };
    if owners.remove(session_id).is_none() {
        return;
    }
    let rendered = serde_json::Value::Object(owners).to_string();
    let staging = owners_path.with_extension("json.tmp");
    let written = std::fs::write(&staging, rendered)
        .and_then(|()| std::fs::rename(&staging, &owners_path));
    if let Err(error) = written {
        let _ = std::fs::remove_file(&staging);
        tracing::warn!(
            "agent sessions: cannot update {}: {error}",
            owners_path.display()
        );
    }
}

/// Every session document, skipping the bookkeeping files Codewhale keeps
/// beside them (`session_boot_owners.json`) and its dot-directories.
fn session_files(root: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    entries
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "json"))
        .filter(|path| {
            path.file_stem()
                .and_then(|stem| stem.to_str())
                .is_some_and(|stem| !stem.starts_with('.'))
        })
        .collect()
}

/// The only two blocks a row is built from. Everything else in the document
/// (`messages`, `system_prompt`, `leaf_id`) is skipped by serde without being
/// materialized, so the cost of a scan does not follow the transcript length.
#[derive(serde::Deserialize)]
struct SessionDocument {
    #[serde(default)]
    metadata: Option<serde_json::Value>,
    #[serde(default)]
    journal: Option<serde_json::Value>,
}

fn read_session(path: &Path) -> Option<NormalizedConversation> {
    let file = std::fs::File::open(path).ok()?;
    let document: SessionDocument = serde_json::from_reader(std::io::BufReader::new(file)).ok()?;
    // A struct also decodes from a positional array, and `session_boot_owners.json`
    // beside the sessions is a plain array — without this check its first element
    // would land in `metadata` and the bookkeeping file would become a row.
    let metadata = document.metadata.as_ref()?.as_object()?;
    // A run that was started but never reached a model has no metadata id yet.
    let id = metadata
        .get("id")
        .and_then(serde_json::Value::as_str)
        .filter(|id| !id.is_empty())
        .map(str::to_owned)
        .or_else(|| path.file_stem().map(|stem| stem.to_string_lossy().into_owned()))?;
    let spawn_depth = document
        .journal
        .as_ref()
        .and_then(serde_json::Value::as_object)
        .and_then(|journal| journal.get("spawn_depth"))
        .and_then(serde_json::Value::as_i64)
        .unwrap_or(ROOT_SPAWN_DEPTH);
    if spawn_depth != ROOT_SPAWN_DEPTH {
        return None;
    }
    let title = metadata
        .get("title")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|title| !title.is_empty())
        .map(str::to_owned);
    let workspace = metadata
        .get("workspace")
        .and_then(serde_json::Value::as_str)
        .filter(|workspace| !workspace.is_empty())
        .map(PathBuf::from);
    let ended_at = std::fs::metadata(path)
        .ok()
        .map(|metadata| modified_ms(&metadata));
    Some(NormalizedConversation {
        agent_slug: SLUG.to_owned(),
        external_id: Some(id),
        title,
        workspace,
        source_path: path.to_path_buf(),
        started_at: None,
        ended_at,
        metadata: serde_json::Value::Null,
        messages: Vec::new(),
    })
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
    use std::ffi::OsStr;

    use super::*;

    /// Fixtures sit beside the workspace rather than inside the crate, so the
    /// end-to-end scripts under `agent-tests/` read the same samples.
    fn sessions_directory() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../agent-tests/fixtures/codewhale/sessions")
    }

    #[test]
    fn read_session_reads_metadata() {
        let path = sessions_directory().join("11111111-1111-4111-8111-111111111111.json");
        let session = read_session(&path).expect("session document should scan");
        assert_eq!(session.agent_slug, "codewhale");
        assert_eq!(
            session.external_id.as_deref(),
            Some("11111111-1111-4111-8111-111111111111")
        );
        assert_eq!(session.title.as_deref(), Some("Fix the build"));
        assert_eq!(
            session.workspace.as_deref(),
            Some(Path::new("/repo/demo"))
        );
        assert_eq!(session.source_path, path);
        // `updated_at` is RFC 3339 text and the scanner stands the file mtime in
        // for it, so only positivity is stable.
        assert!(
            session.ended_at.is_some_and(|updated| updated > 0),
            "mtime should become a positive update time, got {:?}",
            session.ended_at
        );
    }

    /// A run deeper than the root belongs to a sub-agent and would show the
    /// same conversation twice.
    #[test]
    fn read_session_skips_delegated_run() {
        let path = sessions_directory().join("22222222-2222-4222-8222-222222222222.json");
        assert!(
            read_session(&path).is_none(),
            "journal.spawn_depth = 1 must be filtered out of the row list"
        );
    }

    /// A run that was started but never reached a model has no metadata id, so
    /// the file stem names it; a blank title stays no title.
    #[test]
    fn read_session_falls_back_to_the_file_stem() {
        let path = sessions_directory().join("33333333-3333-4333-8333-333333333333.json");
        let session = read_session(&path).expect("session document should scan");
        assert_eq!(
            session.external_id.as_deref(),
            Some("33333333-3333-4333-8333-333333333333")
        );
        assert_eq!(session.title, None);
        assert_eq!(session.workspace, None);
    }

    /// The bookkeeping file beside the sessions is a JSON document without a
    /// metadata block, so it carries no row.
    #[test]
    fn read_session_ignores_bookkeeping_files() {
        let path = sessions_directory().join("session_boot_owners.json");
        assert!(
            read_session(&path).is_none(),
            "a JSON document without a metadata block must yield no row"
        );
    }

    #[test]
    fn read_session_missing_file_returns_none() {
        let path = sessions_directory().join("44444444-4444-4444-8444-444444444444.json");
        assert!(
            read_session(&path).is_none(),
            "a missing session document must yield no row"
        );
    }

    /// The document grows with the transcript. Reading only `metadata` and
    /// `journal` must keep that growth out of the scan, so a multi-megabyte
    /// `messages` and `system_prompt` still yield the row.
    #[test]
    fn read_session_reads_metadata_beside_a_large_transcript() {
        let store = scratch("codewhale-large");
        let path = store.join("55555555-5555-4555-8555-555555555555.json");
        let filler = "x".repeat(4 * 1024 * 1024);
        let document = format!(
            r#"{{"schema_version":1,"metadata":{{"id":"55555555-5555-4555-8555-555555555555","title":"Long one","workspace":"/repo/long"}},"messages":[{{"content":"{filler}"}}],"journal":{{"spawn_depth":0}},"system_prompt":"{filler}"}}"#
        );
        std::fs::write(&path, document).expect("write the document");

        let session = read_session(&path).expect("a large transcript still scans");

        assert_eq!(session.title.as_deref(), Some("Long one"));
        assert_eq!(session.workspace.as_deref(), Some(Path::new("/repo/long")));
        std::fs::remove_dir_all(&store).ok();
    }

    /// A store directory that does not exist yields no files, not an error.
    #[test]
    fn session_files_tolerates_a_missing_directory() {
        let missing =
            std::env::temp_dir().join(format!("hicodeer-agent-codewhale-missing-{}", std::process::id()));
        assert!(
            session_files(&missing).is_empty(),
            "a missing store directory must yield no files"
        );
    }

    /// Dot files are bookkeeping; every other JSON document is offered to the
    /// parser, which drops the ones without a metadata block.
    #[test]
    fn session_files_skips_dot_files_and_scan_drops_the_rest() {
        let root = sessions_directory();
        let files = session_files(&root);
        assert!(
            !files.iter().any(|path| path.file_name()
                == Some(OsStr::new(".hidden.json"))),
            "dot files must not be scanned, got {files:?}"
        );
        assert_eq!(files.len(), 4, "three sessions plus the boot-owner file");
        let sessions: Vec<_> = files.iter().filter_map(|path| read_session(path)).collect();
        // The boot-owner file has no metadata block and the delegated run sits
        // below the root depth, so two of the four documents become rows.
        assert_eq!(sessions.len(), 2, "got {sessions:?}");
    }

    /// A directory this test owns, cleared before use so a previous run cannot
    /// make the assertions pass by accident.
    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("hicodeer-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create the scratch directory");
        dir
    }

    /// One session is a document, a directory of the same name and its
    /// boot-owner entry; all three go, and the neighbouring session stays.
    #[test]
    fn delete_in_removes_the_document_directory_and_boot_owner() {
        let store = scratch("codewhale-delete");
        let session_id = "11111111-1111-4111-8111-111111111111";
        let document = store.join(format!("{session_id}.json"));
        std::fs::write(&document, "{}\n").expect("write the document");
        let sidecar = store.join(session_id);
        std::fs::create_dir_all(&sidecar).expect("create the session directory");
        std::fs::write(sidecar.join("journal.json"), "{}\n").expect("write the journal");
        let neighbour = store.join("bbbbbbbb-2222-4222-8222-222222222222.json");
        std::fs::write(&neighbour, "{}\n").expect("write the neighbouring session");
        let owners = store.join("session_boot_owners.json");
        std::fs::write(
            &owners,
            format!(r#"{{"{session_id}":"boot-a","bbbbbbbb-2222-4222-8222-222222222222":"boot-b"}}"#),
        )
        .expect("write the boot owners");
        let root = store.canonicalize().expect("resolve the store");

        delete_in(&root, &document).expect("a session inside the store is removable");

        assert!(!document.exists(), "the document should be gone");
        assert!(!sidecar.exists(), "the session directory should be gone");
        assert!(neighbour.exists(), "a different session must survive");
        let remaining = std::fs::read_to_string(&owners).expect("read the boot owners");
        assert!(
            !remaining.contains(session_id),
            "the boot-owner entry must be dropped, got: {remaining}"
        );
        assert!(
            remaining.contains("bbbbbbbb-2222-4222-8222-222222222222"),
            "other sessions keep their boot-owner entry, got: {remaining}"
        );
        std::fs::remove_dir_all(&store).ok();
    }

    /// The boot-owner map is shared by every session, so it is not removable
    /// through the session path even though it sits in the same directory.
    #[test]
    fn delete_in_refuses_the_boot_owner_map() {
        let store = scratch("codewhale-boot-owners");
        let owners = store.join(BOOT_OWNERS_FILE);
        std::fs::write(&owners, r#"{"11111111-1111-4111-8111-111111111111":"boot-a"}"#)
            .expect("write the boot owners");
        std::fs::write(
            store.join("11111111-1111-4111-8111-111111111111.json"),
            "{}\n",
        )
        .expect("write the document");
        let root = store.canonicalize().expect("resolve the store");

        let error = delete_in(&root, &owners).expect_err("the bookkeeping file is not a session");

        assert!(
            error.contains("not a session document"),
            "the refusal should name the reason, got: {error}"
        );
        assert!(owners.exists(), "the store's bookkeeping must survive");
        assert!(
            store
                .join("11111111-1111-4111-8111-111111111111.json")
                .exists(),
            "the session it describes must survive too"
        );
        std::fs::remove_dir_all(&store).ok();
    }

    /// A path outside the store is refused before anything is unlinked.
    #[test]
    fn delete_in_refuses_a_path_outside_the_store() {
        let store = scratch("codewhale-outside");
        let outside = store.join("outside");
        std::fs::create_dir_all(&outside).expect("create the outside directory");
        let document = outside.join("11111111-1111-4111-8111-111111111111.json");
        std::fs::write(&document, "{}\n").expect("write the document");
        let root = store.join("sessions");
        std::fs::create_dir_all(&root).expect("create the store");
        let root = root.canonicalize().expect("resolve the store");

        let error = delete_in(&root, &document).expect_err("outside the store must be refused");

        assert!(
            error.contains("outside the store"),
            "the refusal should name the reason, got: {error}"
        );
        assert!(
            document.exists(),
            "a refused delete must leave the file alone"
        );
        std::fs::remove_dir_all(&store).ok();
    }
}

