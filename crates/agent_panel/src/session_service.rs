//! Normalizing, filtering and remembering history sessions (design doc §5).
//!
//! The daemon only collects; everything user-visible about a session beyond
//! its raw content — its pinned/archived/renamed state, whether it is already
//! open — lives here. State survives restarts in one JSON file under the app
//! data dir, written atomically so a crash never leaves a partial file.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::controller::{ControllerError, RawSession};

/// One session, addressed by the pair that is unique across agents.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct SessionKey {
    pub agent: String,
    pub session_id: String,
}

impl SessionKey {
    pub(crate) fn new(agent: &str, session_id: &str) -> Self {
        Self {
            agent: agent.to_owned(),
            session_id: session_id.to_owned(),
        }
    }

    fn to_json_key(&self) -> String {
        format!("{}:{}", self.agent, self.session_id)
    }
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
struct SessionStateEntry {
    #[serde(default)]
    pinned: bool,
    #[serde(default)]
    archived: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    title_override: Option<String>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct SessionState {
    version: u32,
    #[serde(default)]
    sessions: HashMap<String, SessionStateEntry>,
}

/// One history row after normalization, ready to render.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SessionEntry {
    pub key: SessionKey,
    pub title: String,
    pub updated_at: Option<i64>,
    pub source_path: String,
    pub workspace_root: Option<String>,
    pub pinned: bool,
    pub archived: bool,
    /// Whether the store behind this row can remove it.
    pub deletable: bool,
}

/// The history view: pinned rows first, then one group per folder the sessions
/// ran in plus a catch-all for sessions whose folder the scanner could not
/// recover, then the archive. Groups with nothing in them are dropped here, so
/// the UI never renders an empty shell.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HistoryView {
    pub pinned: Vec<SessionEntry>,
    /// `(folder key, folder label, sessions)`, ordered by label.
    pub groups: Vec<(String, String, Vec<SessionEntry>)>,
    /// `(folder key, folder label, sessions)` for archived rows.
    pub archived: Vec<(String, String, Vec<SessionEntry>)>,
}

impl HistoryView {
    /// The row for one session, wherever it currently sits.
    pub(crate) fn find(&self, key: &SessionKey) -> Option<&SessionEntry> {
        self.pinned
            .iter()
            .chain(self.groups.iter().flat_map(|(_, _, sessions)| sessions.iter()))
            .chain(
                self.archived
                    .iter()
                    .flat_map(|(_, _, sessions)| sessions.iter()),
            )
            .find(|entry| &entry.key == key)
    }
}

pub(crate) struct SessionService {
    state_path: PathBuf,
    state: SessionState,
}

impl SessionService {
    /// Loads the state file; a missing file is the normal first-run case.
    pub(crate) fn load(state_path: PathBuf) -> Self {
        let state = std::fs::read_to_string(&state_path)
            .ok()
            .and_then(|content| serde_json::from_str(&content).ok())
            .unwrap_or_default();
        Self { state_path, state }
    }

    pub(crate) fn pin(&mut self, key: &SessionKey, pinned: bool) -> Result<(), ControllerError> {
        self.entry_mut(key).pinned = pinned;
        self.save()
    }

    pub(crate) fn archive(&mut self, key: &SessionKey, archived: bool)
    -> Result<(), ControllerError> {
        self.entry_mut(key).archived = archived;
        self.save()
    }

    pub(crate) fn rename(
        &mut self,
        key: &SessionKey,
        title: Option<String>,
    ) -> Result<(), ControllerError> {
        self.entry_mut(key).title_override = title;
        self.save()
    }

    /// Builds the history view from one daemon scan plus the currently open
    /// sessions (as `(agent, session value)` pairs from `agents()`).
    ///
    /// "Open" compares the agent-reported session value, not the workspace:
    /// a space being open does not mean the session in it is the one the
    /// row points at. Building the view only reads the state file; an entry is
    /// written when the user pins, archives or renames a row.
    pub(crate) fn history(
        &self,
        sessions: Vec<RawSession>,
        open_sessions: &[(String, String)],
    ) -> HistoryView {
        let mut view = HistoryView {
            pinned: Vec::new(),
            groups: Vec::new(),
            archived: Vec::new(),
        };
        // The two trees are different vectors, so they need their own index:
        // sharing one made a folder that had rows in both look up a position in
        // the other list and panic on the read.
        let mut group_index: HashMap<String, usize> = HashMap::new();
        let mut archived_index: HashMap<String, usize> = HashMap::new();

        for raw in sessions {
            let key = SessionKey::new(&raw.agent, &raw.session_id);
            let is_open = open_sessions
                .iter()
                .any(|(agent, value)| agent == &raw.agent && value == &raw.session_id);
            if is_open {
                continue;
            }
            // Read-only: a session the user never pinned, archived or renamed
            // leaves no entry behind, so the state file only holds real
            // decisions instead of one blank row per scanned session.
            let entry_state = self.state.sessions.get(&key.to_json_key());
            let pinned = entry_state.is_some_and(|state| state.pinned);
            let archived = entry_state.is_some_and(|state| state.archived);
            let title = entry_state
                .and_then(|state| state.title_override.clone())
                .or_else(|| raw.title.clone())
                .unwrap_or_else(|| localization::localized_str!("Untitled session").to_owned());
            let entry = SessionEntry {
                key: key.clone(),
                title,
                updated_at: raw.updated_at,
                source_path: raw.source_path,
                workspace_root: raw.workspace_root.clone(),
                pinned,
                archived,
                deletable: raw.deletable,
            };

            // Groups are named after the folder the session ran in, never after
            // an open space. A space's label follows whatever that space is
            // called right now, so reusing it made the same session appear
            // under a different name (or no name at all) just because the user
            // opened or renamed a space. A folder name cannot move.
            let (group_key, group_label) = group_for(raw.workspace_root.as_deref());

            if pinned {
                view.pinned.push(entry);
            } else if archived {
                push_grouped(
                    &mut view.archived,
                    &mut archived_index,
                    group_key,
                    group_label,
                    entry,
                );
            } else {
                push_grouped(
                    &mut view.groups,
                    &mut group_index,
                    group_key,
                    group_label,
                    entry,
                );
            }
        }

        for group in view.groups.iter_mut().chain(view.archived.iter_mut()) {
            group.2.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
        }
        view.pinned.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
        // Folders are listed by name so the order is as stable as the names.
        sort_groups(&mut view.groups);
        sort_groups(&mut view.archived);
        view
    }

    /// Drops the remembered pin/archive/rename state of a session that is gone
    /// from disk, so a later session with the same id starts clean.
    pub(crate) fn forget(&mut self, key: &SessionKey) -> Result<(), ControllerError> {
        if self.state.sessions.remove(&key.to_json_key()).is_some() {
            self.save()?;
        }
        Ok(())
    }

    fn entry_mut(&mut self, key: &SessionKey) -> &mut SessionStateEntry {
        self.state
            .sessions
            .entry(key.to_json_key())
            .or_default()
    }

    fn save(&self) -> Result<(), ControllerError> {
        let content = serde_json::to_string_pretty(&self.state)
            .map_err(|error| ControllerError::Unavailable(error.to_string()))?;
        write_atomically(&self.state_path, &content).map_err(ControllerError::Io)
    }
}

fn push_grouped(
    groups: &mut Vec<(String, String, Vec<SessionEntry>)>,
    index: &mut HashMap<String, usize>,
    group_key: String,
    group_label: String,
    entry: SessionEntry,
) {
    match index.get(&group_key) {
        Some(&position) => groups[position].2.push(entry),
        None => {
            index.insert(group_key.clone(), groups.len());
            groups.push((group_key, group_label, vec![entry]));
        }
    }
}

/// Group key for sessions whose scanner could not recover a working directory.
/// Empty, because every folder group is keyed by a non-empty path.
const UNKNOWN_FOLDER_KEY: &str = "";

/// Label of the catch-all group. Localized like every other string the panel
/// shows, so it reads the same way in a translated build.
fn unknown_folder_label() -> String {
    localization::localized_str!("Unknown folder").to_owned()
}

/// Group key and label for where a session ran. A session with no recoverable
/// directory still gets a group: it can be resumed once the panel supplies a
/// directory, so hiding the row would hide a session the user can open.
fn group_for(root: Option<&str>) -> (String, String) {
    let Some(root) = root.map(str::trim).filter(|root| !root.is_empty()) else {
        return (UNKNOWN_FOLDER_KEY.to_owned(), unknown_folder_label());
    };
    let (key, label) = folder_group(root);
    // A session started in the filesystem root trims down to an empty key,
    // which is this file's marker for "no folder"; keep the two apart so the
    // row is not filed under the catch-all.
    if key.is_empty() {
        return (root.to_owned(), root.to_owned());
    }
    (key, label)
}

/// Folders are listed by name, and the catch-all group always comes last: it is
/// not a project, so it must not sit between two real ones.
fn sort_groups(groups: &mut [(String, String, Vec<SessionEntry>)]) {
    groups.sort_by(|a, b| {
        a.0.is_empty()
            .cmp(&b.0.is_empty())
            .then_with(|| a.1.cmp(&b.1))
    });
}

/// Group key and label for the folder a session ran in. The key folds case so
/// two spellings of one folder stay together; the label is what the user reads.
fn folder_group(root: &str) -> (String, String) {
    let trimmed = root.trim_end_matches('/');
    (trimmed.to_lowercase(), folder_label(trimmed))
}

/// Display name for the folder a session ran in: `github/demo` when the folder
/// is nested under a project root, plain `demo` when it sits directly under a
/// well-known parent. Both history trees use this, so a folder reads the same
/// whether its sessions are active or archived.
fn folder_label(root: &str) -> String {
    const PLAIN_PARENTS: [&str; 3] = ["workspace", "currentUser", "tmp"];
    let root = root.trim_end_matches('/');
    let mut parts = root.rsplit('/');
    let name = parts.next().filter(|name| !name.is_empty());
    match (name, parts.next()) {
        (Some(name), Some(parent)) if !parent.is_empty() && !PLAIN_PARENTS.contains(&parent) => {
            format!("{parent}/{name}")
        }
        (Some(name), _) => name.to_owned(),
        // Only reachable for an empty root, which the caller's path trim has
        // already reduced to nothing. There is no name to borrow, so an empty
        // label is returned rather than a word belonging to one of the trees.
        (None, _) => String::new(),
    }
}

/// Same atomic-write shape the agent settings file uses: write a sibling
/// temporary file, then rename over the target.
fn write_atomically(path: &Path, content: &str) -> std::io::Result<()> {
    let Some(parent) = path.parent() else {
        return Err(std::io::Error::other("state path has no directory"));
    };
    std::fs::create_dir_all(parent)?;
    let file_name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "history.json".to_owned());
    let temporary = parent.join(format!("{file_name}.{}.tmp", std::process::id()));
    std::fs::write(&temporary, content)?;
    std::fs::rename(&temporary, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raw(agent: &str, id: &str, title: &str, updated_at: i64) -> RawSession {
        RawSession {
            agent: agent.to_owned(),
            session_id: id.to_owned(),
            title: Some(title.to_owned()),
            workspace_root: Some("/repo".to_owned()),
            updated_at: Some(updated_at),
            source_path: format!("/store/{id}.jsonl"),
            deletable: true,
        }
    }

    /// The view takes owned pairs, so the tests spell their fixtures the same
    /// way the panel builds them from a snapshot.
    fn pairs(items: &[(&str, &str)]) -> Vec<(String, String)> {
        items
            .iter()
            .map(|(first, second)| ((*first).to_owned(), (*second).to_owned()))
            .collect()
    }

    #[test]
    fn open_sessions_are_hidden_and_state_survives_a_reload() {
        let path = std::env::temp_dir().join(format!("herdr-hist-{}.json", std::process::id()));
        let mut service = SessionService::load(path.clone());
        let view = service.history(
            vec![raw("codebuddy", "a1", "first", 100), raw("codebuddy", "a2", "second", 200)],
            &pairs(&[("codebuddy", "a1")]),
        );
        // a1 is open → hidden; a2 lands in the w1 group.
        assert_eq!(view.pinned.len(), 0);
        assert_eq!(view.groups.len(), 1);
        assert_eq!(view.groups[0].2.len(), 1);
        assert_eq!(view.groups[0].2[0].title, "second");

        service.pin(&SessionKey::new("codebuddy", "a2"), true).unwrap();
        let reopened = SessionService::load(path.clone());
        let view = reopened.history(
            vec![raw("codebuddy", "a2", "second", 200)],
            &[],
        );
        assert_eq!(view.pinned.len(), 1, "pin state must survive a reload");

        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn rename_overrides_and_archive_hides_from_the_main_groups() {
        let path = std::env::temp_dir().join(format!("herdr-hist-{}b.json", std::process::id()));
        let mut service = SessionService::load(path.clone());
        let key = SessionKey::new("claude", "c1");
        service.rename(&key, Some("my name".to_owned())).unwrap();
        service.archive(&key, true).unwrap();
        let view = service.history(
            vec![raw("claude", "c1", "original", 100)],
            &[],
        );
        assert_eq!(view.groups.len(), 0, "archived rows leave their folder group");
        assert_eq!(view.archived.len(), 1);
        assert_eq!(view.archived[0].2[0].title, "my name");

        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn sessions_without_a_folder_share_one_group_that_sorts_last() {
        let path = std::env::temp_dir().join(format!("herdr-hist-{}c.json", std::process::id()));
        let service = SessionService::load(path);

        let mut orphan = raw("hermes", "h1", "no folder", 300);
        orphan.workspace_root = None;
        let view = service.history(
            vec![
                orphan,
                raw("claude", "c1", "has folder", 100),
                raw("claude", "c2", "same folder", 200),
            ],
            &[],
        );

        assert_eq!(view.groups.len(), 2, "the orphan row still gets a group");
        assert_eq!(view.groups[0].1, "repo", "a real folder is unaffected");
        assert_eq!(view.groups[0].2.len(), 2);
        let (key, label, rows) = view.groups.last().expect("unknown folder group");
        assert_eq!(key, UNKNOWN_FOLDER_KEY);
        assert_eq!(label, &unknown_folder_label());
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].key, SessionKey::new("hermes", "h1"));
    }

    /// The filesystem root is a real directory, so a session started there must
    /// not fall into the catch-all group that is keyed by an empty string.
    #[test]
    fn a_session_in_the_filesystem_root_keeps_its_own_group() {
        let path = std::env::temp_dir().join(format!("herdr-hist-{}d.json", std::process::id()));
        let service = SessionService::load(path);

        let mut at_root = raw("claude", "r1", "at root", 100);
        at_root.workspace_root = Some("/".to_owned());
        let view = service.history(vec![at_root], &[]);

        assert_eq!(view.groups.len(), 1);
        let (key, label, rows) = &view.groups[0];
        assert_eq!(key, "/");
        assert_eq!(label, "/");
        assert_eq!(rows.len(), 1);
    }
}
