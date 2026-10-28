//! The part of Claude's store that FAD's connector does not apply: sub-agent
//! transcripts live below the session that owns them.
//!
//! Claude writes `<session-id>/subagents/agent-*.jsonl` for a delegated turn.
//! Those files carry the parent's session id, so listing them would add a
//! duplicate row that resumes the parent.

use std::path::{Path, PathBuf};

use franken_agent_detection::NormalizedConversation;

/// Keeps only the transcripts that are a session of their own: one level below
/// the projects root. Rows outside the root are left alone, because another
/// connector's store is not this one's to judge.
pub(super) fn adjust(conversations: &mut Vec<NormalizedConversation>) {
    let Some(root) = projects_root() else {
        return;
    };
    keep_direct_sessions(&root, conversations);
}

/// The filter itself, with the root already resolved.
fn keep_direct_sessions(root: &Path, conversations: &mut Vec<NormalizedConversation>) {
    conversations.retain(|conversation| {
        conversation
            .source_path
            .strip_prefix(root)
            .map(|relative| relative.components().count() == 2)
            .unwrap_or(true)
    });
}

/// Where Claude keeps its project directories; `CLAUDE_CONFIG_DIR` moves it.
fn projects_root() -> Option<PathBuf> {
    if let Ok(custom) = std::env::var("CLAUDE_CONFIG_DIR") {
        if !custom.is_empty() {
            return Some(PathBuf::from(custom).join("projects"));
        }
    }
    std::env::var_os("HOME")
        .filter(|home| !home.is_empty())
        .map(|home| PathBuf::from(home).join(".claude").join("projects"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A transcript one level below the root is a session; one nested deeper is
    /// a sub-agent of it and would show the same conversation twice.
    #[test]
    fn adjust_drops_transcripts_nested_below_a_session() {
        let root = std::env::temp_dir().join(format!("herdr-claude-store-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let session = root.join("project").join("session.jsonl");
        let subagent = root
            .join("project")
            .join("session")
            .join("subagents")
            .join("agent-1.jsonl");
        for path in [&session, &subagent] {
            std::fs::create_dir_all(path.parent().unwrap()).expect("create the directory");
            std::fs::write(path, "{}\n").expect("write the transcript");
        }

        let mut conversations = vec![conversation(&session), conversation(&subagent)];
        keep_direct_sessions(&root, &mut conversations);

        assert_eq!(conversations.len(), 1, "only the session transcript stays");
        assert_eq!(conversations[0].source_path, session);
        std::fs::remove_dir_all(&root).ok();
    }

    fn conversation(source_path: &Path) -> NormalizedConversation {
        NormalizedConversation {
            agent_slug: "claude".to_owned(),
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
