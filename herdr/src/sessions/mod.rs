//! Historical agent-session collection for the panel's history view.
//!
//! The daemon only collects: dedup, ordering, pin/archive state and
//! open-session filtering all live in the panel (see `移植记录/design/`
//! `2026-10-24-agent-history-panel-design.md` at the repository root).
//! Collection runs through FAD's connectors and the stores this repository
//! implements (CodeBuddy, DeepSeek Harness, DevEco Code, ZCode, Codewhale,
//! AtomCode, MiniMax Code, OpenDesk); both kinds implement the same interface
//! and are registered side by side in `fad.rs`.

mod atomcode;
mod claude_store;
mod codebuddy;
mod codewhale;
mod codex_store;
mod deepseek;
mod deveco;
mod fad;
mod gemini_store;
mod kimi_store;
mod minimax;
mod opendesk;

use std::collections::HashSet;
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
    /// Whether the store this session came from can remove one entry. The
    /// capability belongs to the store, so it is decided where the store is
    /// registered rather than by a path rule here. A store that builds these
    /// rows itself fills `true`; `fad::collect` then replaces the value with
    /// the registry's answer, so that field is not the place to decide this.
    pub deletable: bool,
}

/// Collects history from every store the daemon knows — FAD's connectors plus
/// the ones this repository implements — and matches each session's working
/// directory against the open workspaces' identity cwds. `workspace_roots`
/// carries `(workspace_id, identity_cwd)` pairs from the live herdr state; a
/// match means "a space is open at exactly this directory", which the panel
/// uses only to decide where a resumed session goes — never to name a group.
///
/// Called on every `agent.sessions` request rather than on a timer. Each
/// connector keeps its own index of what it already parsed (`fad.rs` caches on
/// a store signature, `codebuddy.rs` on each file's mtime and size), so a call
/// costs one `stat` per session file plus a read of whatever appeared or
/// changed since the last call — a full re-read only happens when the store
/// itself changed.
pub(crate) fn collect_sessions(workspace_roots: &[(String, PathBuf)]) -> Vec<RawSessionDto> {
    let mut collected = fad::collect();

    // A session can be described by two stores (FAD registers Pi twice, once
    // per store kind). Keep the freshest row per (agent, session id) so the
    // list never shows the same conversation twice.
    collected.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
    let mut seen: HashSet<(&'static str, String)> = HashSet::new();
    collected.retain(|session| seen.insert((session.agent, session.session_id.clone())));

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
                deletable: session.deletable,
            }
        })
        .collect();
    // Newest first within the response; the panel regroups and re-sorts
    // anyway, but a stable order here keeps the cache transparent.
    sessions.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
    sessions
}

/// Removes one session through the store that produced it.
///
/// The knowledge of what a session file is made of — how to find it, what else
/// belongs to it, which cache to drop — lives in that agent's store, so this
/// only routes the request. The panel addresses a session by its agent and its
/// source path; an agent whose store cannot remove entries says so here instead
/// of pretending the deletion worked.
pub(crate) fn delete_session(agent: &str, source_path: &str) -> Result<(), String> {
    fad::delete_session(agent, Path::new(source_path))
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
