//! History scanner for AtomCode (`atomcode`).
//!
//! AtomCode groups its sessions by project directory and writes a small JSON
//! index beside each transcript:
//!
//! ```text
//! ~/.atomcode/sessions/<project hash>/<session id>.index
//!   { id, name, working_dir, created_at, updated_at, turn_count, … }
//! ~/.atomcode/sessions/<project hash>/<session id>.events
//! ```
//!
//! FAD has no AtomCode connector. Only the index is read: it already carries
//! the name, the directory the session ran in and integer millisecond
//! timestamps, while `.events` is the append-only transcript.
//!
//! A session directory can also hold `.rewind` and `.lease` files; they are
//! bookkeeping and are skipped by the `.index` extension filter alone.

use std::path::{Path, PathBuf};

use franken_agent_detection::{
    Connector, DetectionResult, DiscoveredSourceFile, DiscoveredSourceRole,
    NormalizedConversation, ScanContext, ScanRoot,
};

/// The agent label the panel matches against `agent.list`.
const SLUG: &str = "atomcode";

/// The extension AtomCode gives the per-session metadata file.
const INDEX_EXTENSION: &str = "index";

pub(super) struct AtomcodeConnector;

impl AtomcodeConnector {
    pub(super) fn new() -> Self {
        Self
    }
}

impl Connector for AtomcodeConnector {
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
        Ok(index_files(&root)
            .into_iter()
            .filter_map(|path| read_index(&path))
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
        for path in index_files(&root) {
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
    home().map(|home| home.join(".atomcode").join("sessions"))
}

/// Removes every file that belongs to one AtomCode session.
///
/// A session is a set of sidecars that share a stem (`.index`, `.events`,
/// `.rewind`, `.lease`, `.meta.lock`), not one document, so removal works from
/// the name rather than from the single file the row pointed at.
pub(super) fn delete_session(path: &Path) -> Result<(), String> {
    let Some(root) = sessions_root() else {
        return Err("no AtomCode store on this device".to_string());
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
    let Some(directory) = resolved.parent() else {
        return Err(format!("{} names no directory", path.display()));
    };
    // A session lives one project directory below the store root, and the row
    // names its `.index`. Anything else would delete the sidecars of a session
    // the caller did not point at.
    let is_index = resolved
        .extension()
        .is_some_and(|extension| extension == INDEX_EXTENSION)
        && directory.parent() == Some(root);
    if !is_index {
        return Err(format!(
            "refusing to delete {}: not a session index",
            path.display()
        ));
    }
    let Some(session_id) = resolved.file_stem().and_then(|stem| stem.to_str()) else {
        return Err(format!("{} names no session", path.display()));
    };

    let entries = std::fs::read_dir(directory)
        .map_err(|error| format!("read {}: {error}", directory.display()))?;
    let mut removed = 0usize;
    for entry in entries.flatten() {
        let candidate = entry.path();
        // Every sidecar is `<stem>.<kind>`, so a name belongs to this session
        // when it is the stem itself or the stem followed by a dot. A plain
        // prefix match would also take a different session whose id starts the
        // same way.
        let belongs = candidate
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| {
                name == session_id
                    || name
                        .strip_prefix(session_id)
                        .is_some_and(|rest| rest.starts_with('.'))
            });
        if !belongs {
            continue;
        }
        std::fs::remove_file(&candidate)
            .map_err(|error| format!("delete {}: {error}", candidate.display()))?;
        removed += 1;
    }
    if removed == 0 {
        return Err(format!("nothing belonged to session {session_id}"));
    }
    Ok(())
}

/// Every `<project>/<session>.index`, one directory deep.
fn index_files(root: &Path) -> Vec<PathBuf> {
    let Ok(projects) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    projects
        .filter_map(|project| project.ok())
        .map(|project| project.path())
        .filter(|path| path.is_dir())
        .flat_map(|project| {
            let Ok(entries) = std::fs::read_dir(&project) else {
                return Vec::new();
            };
            entries
                .filter_map(|entry| entry.ok())
                .map(|entry| entry.path())
                .filter(|path| {
                    path.extension()
                        .is_some_and(|extension| extension == INDEX_EXTENSION)
                })
                .collect::<Vec<_>>()
        })
        .collect()
}

fn read_index(path: &Path) -> Option<NormalizedConversation> {
    let contents = std::fs::read_to_string(path).ok()?;
    let index: serde_json::Value = serde_json::from_str(&contents).ok()?;
    let id = index
        .get("id")
        .and_then(serde_json::Value::as_str)
        .filter(|id| !id.is_empty())
        .map(str::to_owned)
        .or_else(|| path.file_stem().map(|stem| stem.to_string_lossy().into_owned()))?;
    let title = index
        .get("name")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(str::to_owned);
    let workspace = index
        .get("working_dir")
        .and_then(serde_json::Value::as_str)
        .filter(|directory| !directory.is_empty())
        .map(PathBuf::from);
    let ended_at = index
        .get("updated_at")
        .and_then(serde_json::Value::as_i64)
        .or_else(|| {
            std::fs::metadata(path)
                .ok()
                .map(|metadata| modified_ms(&metadata))
        });
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
    use super::*;

    /// Fixtures sit beside the workspace rather than inside the crate, so the
    /// end-to-end scripts under `agent-tests/` read the same samples.
    fn fixture(relative: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../agent-tests/fixtures/atomcode")
            .join(relative)
    }

    /// `.events`, `.rewind`, `.lease` and `.meta.lock` sit beside the index and
    /// are bookkeeping; only `.index` files are sessions.
    #[test]
    fn index_files_keeps_only_index_files() {
        let files = index_files(&fixture("sessions"));
        assert_eq!(files.len(), 2, "got {files:?}");
        assert!(
            files
                .iter()
                .all(|path| path.extension().is_some_and(|extension| extension == "index")),
            "non-index files must be skipped, got {files:?}"
        );
    }

    #[test]
    fn read_index_reads_metadata() {
        let path = fixture("sessions/11830b6384e6beea/f814563a-9d4e-4d56-ade6-1dce6974c69f.index");
        let session = read_index(&path).expect("index should scan");
        assert_eq!(session.agent_slug, "atomcode");
        assert_eq!(
            session.external_id.as_deref(),
            Some("f814563a-9d4e-4d56-ade6-1dce6974c69f")
        );
        assert_eq!(session.title.as_deref(), Some("Fix the build"));
        assert_eq!(
            session.workspace.as_deref(),
            Some(Path::new("/repo/demo"))
        );
        assert_eq!(session.ended_at, Some(1793095090983));
        assert_eq!(session.source_path, path);
    }

    /// An index without an id is named by its file stem, a blank name stays no
    /// title, and a missing `updated_at` falls back to the file mtime.
    #[test]
    fn read_index_falls_back_to_the_file_stem() {
        let path = fixture("sessions/11830b6384e6beea/aaaaaaa1-0000-4000-8000-000000000001.index");
        let session = read_index(&path).expect("index should scan");
        assert_eq!(
            session.external_id.as_deref(),
            Some("aaaaaaa1-0000-4000-8000-000000000001")
        );
        assert_eq!(session.title, None);
        assert_eq!(session.workspace, None);
        assert!(
            session.ended_at.is_some_and(|updated| updated > 0),
            "mtime should become a positive update time, got {:?}",
            session.ended_at
        );
    }

    /// A half-written index must drop one session, not the whole scan.
    #[test]
    fn read_index_survives_truncated_file() {
        let path = fixture("broken/f814563a-9d4e-4d56-ade6-1dce6974c69f.index");
        assert!(
            read_index(&path).is_none(),
            "an index that is not valid JSON must yield no row"
        );
    }

    #[test]
    fn read_index_missing_file_returns_none() {
        let path = fixture("sessions/11830b6384e6beea/gone.index");
        assert!(
            read_index(&path).is_none(),
            "a missing index must yield no row"
        );
    }

    /// A store directory that does not exist yields no files, not an error.
    #[test]
    fn index_files_tolerates_a_missing_directory() {
        let missing =
            std::env::temp_dir().join(format!("herdr-atomcode-missing-{}", std::process::id()));
        assert!(
            index_files(&missing).is_empty(),
            "a missing store directory must yield no files"
        );
    }

    /// A directory this test owns, cleared before use so a previous run cannot
    /// make the assertions pass by accident.
    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("herdr-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create the scratch directory");
        dir
    }

    /// A session is a set of sidecars sharing a stem, so removing the row the
    /// user picked removes every file of that session and nothing else.
    #[test]
    fn delete_in_removes_every_sidecar_of_one_session() {
        let store = scratch("atomcode-delete");
        let project = store.join("11830b6384e6beea");
        std::fs::create_dir_all(&project).expect("create the project directory");
        let index = project.join("aaaaaaaa-0000-4000-8000-000000000001.index");
        let sidecars = [
            index.clone(),
            project.join("aaaaaaaa-0000-4000-8000-000000000001.events"),
            project.join("aaaaaaaa-0000-4000-8000-000000000001.rewind"),
            project.join("aaaaaaaa-0000-4000-8000-000000000001.lease"),
        ];
        for path in &sidecars {
            std::fs::write(path, "{}\n").expect("write a sidecar");
        }
        let neighbour = project.join("bbbbbbbb-0000-4000-8000-000000000002.index");
        std::fs::write(&neighbour, "{}\n").expect("write the neighbouring session");
        let root = store.canonicalize().expect("resolve the store");

        delete_in(&root, &index).expect("a session inside the store is removable");

        for path in &sidecars {
            assert!(!path.exists(), "{} should be gone", path.display());
        }
        assert!(
            neighbour.exists(),
            "a different session must survive the delete"
        );
        std::fs::remove_dir_all(&store).ok();
    }

    /// The row names a `.index`; the sidecars beside it are reached through it,
    /// never directly, so a request for one is refused.
    #[test]
    fn delete_in_refuses_a_sidecar_named_directly() {
        let store = scratch("atomcode-sidecar");
        let project = store.join("11830b6384e6beea");
        std::fs::create_dir_all(&project).expect("create the project directory");
        let index = project.join("aaaaaaaa-0000-4000-8000-000000000001.index");
        let events = project.join("aaaaaaaa-0000-4000-8000-000000000001.events");
        std::fs::write(&index, "{}\n").expect("write the index");
        std::fs::write(&events, "{}\n").expect("write the events file");
        let root = store.canonicalize().expect("resolve the store");

        let error = delete_in(&root, &events).expect_err("a sidecar is not an entry point");

        assert!(
            error.contains("not a session index"),
            "the refusal should name the reason, got: {error}"
        );
        assert!(
            index.exists() && events.exists(),
            "a refused delete must leave both files alone"
        );
        std::fs::remove_dir_all(&store).ok();
    }

    /// A path outside the store is refused before anything is unlinked.
    #[test]
    fn delete_in_refuses_a_path_outside_the_store() {
        let store = scratch("atomcode-outside");
        let outside = store.join("outside");
        std::fs::create_dir_all(&outside).expect("create the outside directory");
        let index = outside.join("aaaaaaaa-0000-4000-8000-000000000001.index");
        std::fs::write(&index, "{}\n").expect("write the index");
        let root = store.join("sessions");
        std::fs::create_dir_all(&root).expect("create the store");
        let root = root.canonicalize().expect("resolve the store");

        let error = delete_in(&root, &index).expect_err("outside the store must be refused");

        assert!(
            error.contains("outside the store"),
            "the refusal should name the reason, got: {error}"
        );
        assert!(
            index.exists(),
            "a refused delete must leave the file alone"
        );
        std::fs::remove_dir_all(&store).ok();
    }
}

