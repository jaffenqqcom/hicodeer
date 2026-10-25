//! Claude history via the FAD connector.
//!
//! FAD reads `~/.claude/projects/**/*.jsonl` with the agent's own environment
//! overrides (`CLAUDE_CONFIG_DIR`, …) and fills `NormalizedConversation::workspace`
//! from the per-line cwd, which is what we match workspaces on. Session
//! format is officially internal to Claude, so this whole path is pinned to
//! the FAD rev in Cargo.toml.
//!
//! FAD always walks the whole store itself, so there is no per-file hook to
//! skip unchanged sessions. Instead the store's signature — path, mtime and
//! size of every session file — is compared first: an unchanged store reuses
//! the previous result and FAD is not run at all.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::UNIX_EPOCH;

use franken_agent_detection::connectors::{claude_code::ClaudeCodeConnector, Connector, ScanContext};

use super::CollectedSession;

struct CachedScan {
    signature: u64,
    sessions: Vec<CollectedSession>,
}

static CACHE: Mutex<Option<CachedScan>> = Mutex::new(None);

pub(crate) fn collect() -> Vec<CollectedSession> {
    let signature = store_signature();
    if let Some(signature) = signature {
        if let Ok(guard) = CACHE.lock() {
            if let Some(cached) = guard.as_ref() {
                if cached.signature == signature {
                    return cached.sessions.clone();
                }
            }
        }
    }

    let sessions = scan();

    // Without a signature the store could not be inspected, so caching it
    // would risk pinning a stale result.
    if let Some(signature) = signature {
        if let Ok(mut guard) = CACHE.lock() {
            *guard = Some(CachedScan {
                signature,
                sessions: sessions.clone(),
            });
        }
    }
    sessions
}

fn scan() -> Vec<CollectedSession> {
    let connector = ClaudeCodeConnector::new();
    // The data dir backs FAD's own bookkeeping; claude scanning works from its
    // default probe roots, so a scratch dir is fine here.
    let data_dir = std::env::temp_dir().join("herdr-fad-scan");
    let ctx = ScanContext::local_default(data_dir, None);
    let conversations = match connector.scan(&ctx) {
        Ok(conversations) => conversations,
        Err(error) => {
            tracing::warn!("agent sessions: claude store scan failed: {error}");
            return Vec::new();
        }
    };

    conversations
        .into_iter()
        .map(|conversation| {
            let source_path = conversation.source_path;
            let updated_at = conversation
                .ended_at
                .or(conversation.started_at)
                .or_else(|| path_mtime_ms(&source_path));
            CollectedSession {
                agent: "claude",
                session_id: conversation
                    .external_id
                    .clone()
                    .unwrap_or_else(|| fallback_id(&source_path)),
                title: conversation.title,
                workspace_root: conversation.workspace,
                updated_at,
                source_path,
            }
        })
        .collect()
}

/// Hashes every session file's path, mtime and size. Cheap (one `stat` per
/// file) compared to what FAD does, and enough to tell "nothing changed".
fn store_signature() -> Option<u64> {
    let root = store_root()?;
    if !root.is_dir() {
        return None;
    }
    let mut entries: Vec<(PathBuf, u128, u64)> = Vec::new();
    collect_entries(&root, &mut entries);
    if entries.is_empty() {
        // An empty store is a legitimate state, but there is nothing to gain
        // from caching it; let FAD run and report nothing.
        return None;
    }
    entries.sort();
    let mut hasher = DefaultHasher::new();
    for (path, modified, size) in entries {
        path.hash(&mut hasher);
        modified.hash(&mut hasher);
        size.hash(&mut hasher);
    }
    Some(hasher.finish())
}

fn collect_entries(root: &Path, entries: &mut Vec<(PathBuf, u128, u64)>) {
    let Ok(read_dir) = std::fs::read_dir(root) else {
        return;
    };
    for entry in read_dir.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_entries(&path, entries);
            continue;
        }
        if path.extension().is_none_or(|ext| ext != "jsonl") {
            continue;
        }
        let Ok(metadata) = std::fs::metadata(&path) else {
            continue;
        };
        let modified = metadata
            .modified()
            .ok()
            .and_then(|mtime| mtime.duration_since(UNIX_EPOCH).ok())
            .map_or(0, |d| d.as_nanos());
        entries.push((path, modified, metadata.len()));
    }
}

/// Where FAD's default probe root points. `CLAUDE_CONFIG_DIR` moves it, and
/// the signature has to follow the same store.
pub(super) fn store_root() -> Option<PathBuf> {
    if let Ok(custom) = std::env::var("CLAUDE_CONFIG_DIR") {
        if !custom.is_empty() {
            return Some(PathBuf::from(custom).join("projects"));
        }
    }
    std::env::var_os("HOME")
        .filter(|home| !home.is_empty())
        .map(|home| PathBuf::from(home).join(".claude").join("projects"))
}

fn fallback_id(path: &std::path::Path) -> String {
    path.file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn path_mtime_ms(path: &std::path::Path) -> Option<i64> {
    std::fs::metadata(path)
        .and_then(|meta| meta.modified())
        .ok()
        .and_then(|mtime| mtime.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as i64)
}
