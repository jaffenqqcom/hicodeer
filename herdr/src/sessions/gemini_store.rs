//! The part of Gemini's store that FAD's connector does not apply: the directory
//! the session actually ran in.
//!
//! Gemini keeps its transcripts under `~/.gemini/tmp/<slug>/chats/`, where
//! `<slug>` is the name `~/.gemini/projects.json` gave the directory the CLI was
//! started in. FAD reports that storage directory as the session's workspace, so
//! the panel files the row under `~/.gemini/tmp/tmp` and reopens the session
//! from there. Reversing the map restores the directory the user worked in —
//! which is also where `gemini --resume <id>` has to run, because the CLI only
//! resolves an id among the chats of its current project.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use franken_agent_detection::NormalizedConversation;

/// Gemini's directory under the home directory; `GEMINI_DIR` in the CLI.
const GEMINI_DIRECTORY: &str = ".gemini";

/// The project registry Gemini maintains: one absolute directory per slug.
const PROJECTS_FILE: &str = "projects.json";

/// Transcripts of one project live here, one directory below its slug.
const CHATS_DIRECTORY: &str = "chats";

pub(super) fn adjust(conversations: &mut Vec<NormalizedConversation>) {
    let Some(directories) = project_directories() else {
        return;
    };
    apply(&directories, conversations);
}

/// Replaces the workspace of every row this module recognises. Rows whose path
/// is not a Gemini chat are left alone: another connector's store is not this
/// one's to judge.
fn apply(directories: &HashMap<String, PathBuf>, conversations: &mut [NormalizedConversation]) {
    for conversation in conversations.iter_mut() {
        let Some(slug) = storage_slug(&conversation.source_path) else {
            continue;
        };
        if let Some(directory) = directories.get(slug) {
            conversation.workspace = Some(directory.clone());
        }
    }
}

/// The slug from `…/.gemini/tmp/<slug>/chats/<file>`. The `.gemini` check keeps
/// a transcript from another store that happens to end in `chats/` untouched.
fn storage_slug(source_path: &Path) -> Option<&str> {
    let chats = source_path.parent()?;
    if chats.file_name()? != CHATS_DIRECTORY {
        return None;
    }
    let store = chats.parent()?;
    if store.parent()?.parent()?.file_name()? != GEMINI_DIRECTORY {
        return None;
    }
    store.file_name()?.to_str()
}

/// Slug to directory, read out of `~/.gemini/projects.json`; `None` when the
/// registry is missing or unreadable, which leaves every row as FAD scanned it.
fn project_directories() -> Option<HashMap<String, PathBuf>> {
    let home = std::env::var_os("HOME").filter(|home| !home.is_empty())?;
    let registry = PathBuf::from(home)
        .join(GEMINI_DIRECTORY)
        .join(PROJECTS_FILE);
    let text = std::fs::read_to_string(&registry).ok()?;
    let document: serde_json::Value = serde_json::from_str(&text).ok()?;
    let projects = document.get("projects")?.as_object()?;
    let mut directories = HashMap::new();
    for (directory, slug) in projects {
        if let Some(slug) = slug.as_str().filter(|slug| !slug.is_empty()) {
            directories.insert(slug.to_owned(), PathBuf::from(directory));
        }
    }
    Some(directories)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn conversation(source_path: &str) -> NormalizedConversation {
        NormalizedConversation {
            agent_slug: "gemini".to_owned(),
            external_id: Some("session-id".to_owned()),
            title: None,
            workspace: Some("/storage/Users/currentUser/.gemini/tmp/tmp".to_owned().into()),
            source_path: PathBuf::from(source_path),
            started_at: None,
            ended_at: None,
            metadata: serde_json::Value::Null,
            messages: Vec::new(),
        }
    }

    /// The storage directory FAD reports is replaced with the directory the CLI
    /// was started in, taken from the project registry.
    #[test]
    fn apply_restores_the_directory_the_session_ran_in() {
        let mut directories = HashMap::new();
        directories.insert(
            "tmp".to_owned(),
            PathBuf::from("/storage/Users/currentUser/tmp"),
        );
        let mut conversations = vec![conversation(
            "/storage/Users/currentUser/.gemini/tmp/tmp/chats/session-1.jsonl",
        )];

        apply(&directories, &mut conversations);

        assert_eq!(
            conversations[0].workspace.as_deref(),
            Some(Path::new("/storage/Users/currentUser/tmp"))
        );
    }

    /// A project the registry does not know keeps what the scanner reported, so
    /// a stale registry degrades to the old behaviour instead of dropping rows.
    #[test]
    fn apply_keeps_the_scanned_workspace_for_an_unknown_slug() {
        let mut conversations = vec![conversation(
            "/storage/Users/currentUser/.gemini/tmp/other/chats/session-1.jsonl",
        )];

        apply(&HashMap::new(), &mut conversations);

        assert_eq!(
            conversations[0].workspace.as_deref(),
            Some(Path::new("/storage/Users/currentUser/.gemini/tmp/tmp"))
        );
    }

    /// Paths that do not end in `<slug>/chats/` below `.gemini` belong to some
    /// other store and must not be rewritten.
    #[test]
    fn apply_ignores_paths_outside_the_gemini_store() {
        let mut directories = HashMap::new();
        directories.insert("logs".to_owned(), PathBuf::from("/repo/demo"));
        let mut conversations = vec![
            conversation("/storage/Users/currentUser/.other/tmp/logs/chats/session-1.jsonl"),
            conversation("/repo/demo/chats/session-1.jsonl"),
        ];
        let before: Vec<Option<PathBuf>> = conversations
            .iter()
            .map(|conversation| conversation.workspace.clone())
            .collect();

        apply(&directories, &mut conversations);

        let after: Vec<Option<PathBuf>> = conversations
            .iter()
            .map(|conversation| conversation.workspace.clone())
            .collect();
        assert_eq!(before, after);
    }
}
