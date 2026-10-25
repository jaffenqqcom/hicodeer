//! Historical agent-session collection for the panel's history view.
//!
//! The daemon only collects: dedup, ordering, pin/archive state and
//! open-session filtering all live in the panel (see the design doc in
//! `../移植记录/design/2026-10-24-agent-history-panel-design.md`). Two sources
//! feed [`collect_sessions`]: the FAD Claude connector and our own CodeBuddy
//! scanner (same store layout, different directory encoding and title lines).

mod codebuddy;
mod claude;

use std::path::{Path, PathBuf};

use crate::api::schema::response::RawSessionDto;

/// One collected session before workspace matching.
#[derive(Clone)]
pub(crate) struct CollectedSession {
    pub agent: &'static str,
    pub session_id: String,
    pub title: Option<String>,
    pub workspace_root: Option<PathBuf>,
    pub updated_at: Option<i64>,
    pub source_path: PathBuf,
}

/// Collects Claude + CodeBuddy history and matches each session's working
/// directory against the open workspaces' identity cwds. `workspace_roots`
/// carries `(workspace_id, identity_cwd)` pairs from the live herdr state; a
/// match means "a space is open at exactly this directory", which the panel
/// uses only to decide where a resumed session goes — never to name a group.
///
/// Called on every `agent.sessions` request rather than on a timer. Both
/// scanners keep their own index of what they already parsed, so a call costs
/// one `stat` per session file (microseconds for the whole store) plus a read
/// of whatever appeared or changed since the last call — a full re-read only
/// happens when the store itself changed.
pub(crate) fn collect_sessions(workspace_roots: &[(String, PathBuf)]) -> Vec<RawSessionDto> {
    let mut collected = Vec::new();
    collected.extend(claude::collect());
    collected.extend(codebuddy::collect());

    let mut sessions: Vec<RawSessionDto> = collected
        .into_iter()
        .map(|session| {
            let workspace_root = session
                .workspace_root
                .as_deref()
                .map(|root| root.to_string_lossy().into_owned());
            let workspace_id = session
                .workspace_root
                .as_deref()
                .and_then(|root| match_workspace(root, workspace_roots))
                .map(str::to_owned);
            RawSessionDto {
                agent: session.agent.to_owned(),
                session_id: session.session_id,
                title: session.title,
                workspace_root,
                updated_at: session.updated_at,
                source_path: session
                    .source_path
                    .to_str()
                    .map(str::to_owned)
                    .unwrap_or_else(|| {
                        // The panel deletes through this path, so a lossy one
                        // is worth a line: the row still opens, but it cannot
                        // be removed from the list.
                        tracing::warn!(
                            "agent sessions: source path is not UTF-8: {:?}",
                            session.source_path
                        );
                        String::new()
                    }),
                workspace_id,
            }
        })
        .collect();
    // Newest first within the response; the panel regroups and re-sorts
    // anyway, but a stable order here keeps the cache transparent.
    sessions.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
    sessions
}

/// Deletes one session file.
///
/// The path arrives over the API, so it is checked against the two stores the
/// scanner reads before anything is removed: the file has to be a `.jsonl`
/// inside one of those `projects/` trees. The panel cannot do this itself — an
/// app process gets `EPERM` when unlinking from these trees — so the daemon,
/// which runs as the user, is the one performing it.
pub(crate) fn delete_session(source_path: &str) -> Result<(), String> {
    let path = PathBuf::from(source_path);
    if path.extension().is_none_or(|extension| extension != "jsonl") {
        return Err(format!("refusing to delete {source_path}: not a session file"));
    }

    let stores: Vec<PathBuf> = [claude::store_root(), codebuddy::projects_root()]
        .into_iter()
        .flatten()
        .filter_map(|root| root.canonicalize().ok())
        .collect();
    let resolved = path
        .canonicalize()
        .map_err(|error| format!("resolve {source_path}: {error}"))?;
    if !stores.iter().any(|store| resolved.starts_with(store)) {
        return Err(format!(
            "refusing to delete {source_path}: outside the session stores"
        ));
    }

    std::fs::remove_file(&resolved).map_err(|error| format!("delete {source_path}: {error}"))?;
    // The next scan must not answer from a row that points at a gone file. The
    // Claude side keys off the store signature, which changes on its own.
    codebuddy::forget_path(&resolved);
    Ok(())
}

/// Matches a session's working directory to an open workspace. The session has
/// to have run in exactly that directory, not somewhere below it.
///
/// Prefix matching was wrong here: a workspace opened at the home directory
/// contains every project the user works in, so it claimed all of them at once
/// and the panel ended up with a single group named after that one space.
///
/// Paths are compared after canonicalizing what exists on disk. A second,
/// case-folded pass runs only when the exact one misses: this device's store
/// (hmdfs) is case-insensitive, so `/repo/Demo` and `/repo/demo` are one
/// directory, but `canonicalize` returns the spelling it was handed and the
/// strings then differ. Folding stays a fallback so a case-sensitive file
/// system still resolves on the exact spelling first.
fn match_workspace<'a>(
    session_root: &Path,
    workspace_roots: &'a [(String, PathBuf)],
) -> Option<&'a str> {
    let session_root = normalize_slashes(&normalize_existing(session_root));
    let normalized: Vec<PathBuf> = workspace_roots
        .iter()
        .map(|(_, root)| normalize_slashes(&normalize_existing(root)))
        .collect();
    if let Some(index) = normalized.iter().position(|root| *root == session_root) {
        return Some(workspace_roots[index].0.as_str());
    }
    let folded = fold_case(&session_root);
    normalized
        .iter()
        .position(|root| fold_case(root) == folded)
        .map(|index| workspace_roots[index].0.as_str())
}

/// A trailing slash makes two spellings of one directory compare as prefix
/// relations instead of equality, so it is dropped before matching.
fn normalize_slashes(path: &Path) -> PathBuf {
    PathBuf::from(path.to_string_lossy().trim_end_matches('/'))
}

fn fold_case(path: &Path) -> PathBuf {
    normalize_slashes(path).to_string_lossy().to_lowercase().into()
}

/// Canonicalizes the longest existing prefix so `/repo` vs `/repo/` and
/// symlinked homes compare equal, without failing on deleted directories.
fn normalize_existing(path: &Path) -> PathBuf {
    let mut current = path.to_path_buf();
    let mut suffix = Vec::new();
    loop {
        match current.canonicalize() {
            Ok(resolved) => {
                let mut resolved = resolved;
                for part in suffix.iter().rev() {
                    resolved.push(part);
                }
                return resolved;
            }
            Err(_) => match current.parent() {
                Some(parent) => {
                    if let Some(name) = current.file_name() {
                        suffix.push(name.to_owned());
                    }
                    current = parent.to_path_buf();
                }
                // No parent left and nothing canonicalized: give the original back.
                None => return path.to_path_buf(),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn match_workspace_requires_the_same_directory() {
        let roots = vec![
            ("w1".to_owned(), PathBuf::from("/repo")),
            ("w2".to_owned(), PathBuf::from("/repo/sub")),
        ];
        assert_eq!(match_workspace(Path::new("/repo"), &roots), Some("w1"));
        assert_eq!(match_workspace(Path::new("/repo/sub"), &roots), Some("w2"));
        // A session below a workspace is not that workspace's: a space opened at
        // the home directory would otherwise claim every project under it.
        assert_eq!(match_workspace(Path::new("/repo/sub/dir"), &roots), None);
        assert_eq!(match_workspace(Path::new("/elsewhere"), &roots), None);
    }
}
