//! The part of Kimi Code's store that FAD's connector does not apply: where a
//! session ran.
//!
//! Kimi keeps its transcript at
//! `sessions/<cwd-hash>/<session>/agents/main/wire.jsonl` and FAD leaves
//! `workspace` empty, but the sibling `state.json` records `cwd`. The hash in
//! the directory name is one-way, so this is the only way to recover the
//! directory.

use std::path::{Path, PathBuf};

use franken_agent_detection::NormalizedConversation;

/// Fills in the directory each Kimi session ran in, leaving rows that already
/// carry one alone.
pub(super) fn adjust(conversations: &mut Vec<NormalizedConversation>) {
    for conversation in conversations.iter_mut() {
        if conversation.workspace.is_some() {
            continue;
        }
        let Some(store) = conversation
            .source_path
            .parent()
            .and_then(Path::parent)
            .and_then(Path::parent)
        else {
            continue;
        };
        let Ok(text) = std::fs::read_to_string(store.join("state.json")) else {
            continue;
        };
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else {
            continue;
        };
        conversation.workspace = value
            .get("cwd")
            .and_then(serde_json::Value::as_str)
            .filter(|cwd| !cwd.is_empty())
            .map(PathBuf::from);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `wire.jsonl` sits three levels below the directory that holds
    /// `state.json`, so a row gets the directory named there.
    #[test]
    fn adjust_reads_the_sibling_state_file() {
        let store = std::env::temp_dir().join(format!("herdr-kimi-store-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&store);
        let wire = store.join("agents").join("main").join("wire.jsonl");
        std::fs::create_dir_all(wire.parent().unwrap()).expect("create the session directory");
        std::fs::write(&wire, "{}\n").expect("write the transcript");
        std::fs::write(store.join("state.json"), r#"{"cwd":"/repo/demo"}"#)
            .expect("write the state file");

        let mut conversations = vec![conversation(&wire)];
        adjust(&mut conversations);

        assert_eq!(conversations[0].workspace.as_deref(), Some(Path::new("/repo/demo")));
        std::fs::remove_dir_all(&store).ok();
    }

    /// A missing or unreadable state file leaves the row without a directory
    /// rather than failing the scan.
    #[test]
    fn adjust_tolerates_a_missing_state_file() {
        let store = std::env::temp_dir().join(format!("herdr-kimi-empty-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&store);
        let wire = store.join("agents").join("main").join("wire.jsonl");
        std::fs::create_dir_all(wire.parent().unwrap()).expect("create the session directory");
        std::fs::write(&wire, "{}\n").expect("write the transcript");

        let mut conversations = vec![conversation(&wire)];
        adjust(&mut conversations);

        assert_eq!(conversations[0].workspace, None);
        std::fs::remove_dir_all(&store).ok();
    }

    fn conversation(source_path: &Path) -> NormalizedConversation {
        NormalizedConversation {
            agent_slug: "kimi".to_owned(),
            external_id: None,
            title: None,
            workspace: None,
            source_path: source_path.to_path_buf(),
            started_at: None,
            ended_at: None,
            metadata: serde_json::Value::Null,
            messages: Vec::new(),
        }
    }
}
