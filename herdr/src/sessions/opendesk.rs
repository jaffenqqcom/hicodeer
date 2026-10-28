//! History scanner for OpenDesk CLI (`opendesk`).
//!
//! OpenDesk keeps every task as one JSON document plus an index of them:
//!
//! ```text
//! ~/.opendesk/tasks/task_list.json      [{ task_id, task_name, parent_id, … }]
//! ~/.opendesk/tasks/<task id>.json      { full transcript, … }
//! ```
//!
//! FAD has no OpenDesk connector. Only the index is read: it carries the id, the
//! task name and the timestamps, while each task document holds the whole
//! transcript and would be read for nothing. Entries with a `parent_id` are
//! sub-tasks of a task already in the list, so they are skipped for the same
//! reason as Codewhale's spawn depth.
//!
//! `updated_at` is RFC 3339 text and the `time` crate is built here without its
//! parser, so the task document's modification time stands in for it: OpenDesk
//! rewrites that document as the task advances.

use std::io::Read;
use std::path::{Path, PathBuf};

use franken_agent_detection::{
    Connector, DetectionResult, DiscoveredSourceFile, DiscoveredSourceRole,
    NormalizedConversation, ScanContext, ScanRoot,
};
use serde::Deserialize;

/// The agent label the panel matches against `agent.list`.
const SLUG: &str = "opendesk";

/// The file name of the task index inside the tasks directory.
const TASK_INDEX_FILE: &str = "task_list.json";

/// How much of a task document is read while looking for the working
/// directory. The system prompt sits at the top of `chat_context`; the rest of
/// the file is transcript, which this scanner never needs.
const WORKSPACE_SCAN_BYTES: u64 = 64 * 1024;

/// Labels the system prompt puts in front of the directory a task ran in.
const WORKSPACE_MARKERS: [&str; 2] = ["工作目录：", "working directory:"];

/// One entry of the task index, limited to the fields a history row needs.
#[derive(Deserialize)]
struct TaskEntry {
    task_id: String,
    #[serde(default)]
    task_name: Option<String>,
    #[serde(default)]
    parent_id: Option<String>,
}

pub(super) struct OpendeskConnector;

impl OpendeskConnector {
    pub(super) fn new() -> Self {
        Self
    }
}

impl Connector for OpendeskConnector {
    fn detect(&self) -> DetectionResult {
        match tasks_root().filter(|path| path.is_dir()) {
            Some(root) => DetectionResult {
                detected: true,
                evidence: vec![root.display().to_string()],
                root_paths: vec![root],
            },
            None => DetectionResult::not_found(),
        }
    }

    fn scan(&self, _ctx: &ScanContext) -> anyhow::Result<Vec<NormalizedConversation>> {
        let Some(root) = tasks_root().filter(|path| path.is_dir()) else {
            return Ok(Vec::new());
        };
        Ok(read_index(&root.join(TASK_INDEX_FILE))
            .into_iter()
            .filter(|task| !is_subtask(task))
            .map(|task| conversation(&root, task))
            .collect())
    }

    fn discover_source_files(
        &self,
        _ctx: &ScanContext,
    ) -> anyhow::Result<Vec<DiscoveredSourceFile>> {
        let Some(root) = tasks_root().filter(|path| path.is_dir()) else {
            return Ok(Vec::new());
        };
        // The index is rewritten whenever a task is added or advanced, so it
        // alone tells the cache whether anything changed.
        let index = root.join(TASK_INDEX_FILE);
        let Ok(metadata) = std::fs::metadata(&index) else {
            return Ok(Vec::new());
        };
        let scan_root = ScanRoot::local(root);
        let mut discovered = DiscoveredSourceFile::new(
            SLUG,
            &scan_root,
            index,
            DiscoveredSourceRole::PrimarySessionLog,
            true,
        );
        discovered.size_bytes = Some(metadata.len());
        discovered.modified_at_ms = Some(modified_ms(&metadata));
        Ok(vec![discovered])
    }
}

fn home() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .filter(|home| !home.is_empty())
        .map(PathBuf::from)
}

fn tasks_root() -> Option<PathBuf> {
    home().map(|home| home.join(".opendesk").join("tasks"))
}

/// Removes one task, the sub-tasks that point at it, and their index entries.
///
/// A task's document and its index entry are separate writes, so both go; the
/// sub-tasks would otherwise stay listed with a parent that no longer exists.
pub(super) fn delete_session(path: &Path) -> Result<(), String> {
    let Some(root) = tasks_root() else {
        return Err("no OpenDesk store on this device".to_string());
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
    let index_path = root.join(TASK_INDEX_FILE);
    // Only a task document may be removed. The index names every task in the
    // store, so unlinking it would hide them all at once.
    let is_document = resolved.parent() == Some(root)
        && resolved.extension().and_then(|extension| extension.to_str()) == Some("json")
        && resolved != index_path;
    if !is_document {
        return Err(format!(
            "refusing to delete {}: not a task document",
            path.display()
        ));
    }
    let Some(task_id) = resolved.file_stem().and_then(|stem| stem.to_str()) else {
        return Err(format!("{} names no task", path.display()));
    };

    // The index carries fields this scanner does not read (status, timestamps),
    // so entries are filtered as raw JSON: rewriting them from `TaskEntry` would
    // drop whatever the CLI put there for the sessions that stay.
    let mut remaining = Vec::new();
    let mut removed_any = false;
    for entry in read_raw_index(&index_path)? {
        let field = |name: &str| entry.get(name).and_then(serde_json::Value::as_str);
        let is_child = field("parent_id") == Some(task_id);
        if field("task_id") != Some(task_id) && !is_child {
            remaining.push(entry);
            continue;
        }
        let document = root.join(format!("{}.json", field("task_id").unwrap_or_default()));
        match std::fs::remove_file(&document) {
            Ok(()) => removed_any = true,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(format!("delete {}: {error}", document.display())),
        }
    }
    if !removed_any {
        // A document can exist without an index entry.
        std::fs::remove_file(&resolved)
            .map_err(|error| format!("delete {}: {error}", path.display()))?;
    }

    if index_path.exists() {
        let rendered = serde_json::to_string_pretty(&remaining)
            .map_err(|error| format!("render {}: {error}", index_path.display()))?;
        write_atomically(&index_path, rendered.as_bytes())?;
    }
    Ok(())
}

/// Replaces a file in one rename.
///
/// The index names every task in the store, so a write that stopped halfway
/// would hide all of them: the scan parses the file as a whole and an unreadable
/// one yields no rows.
fn write_atomically(path: &Path, contents: &[u8]) -> Result<(), String> {
    let staging = path.with_extension("json.tmp");
    std::fs::write(&staging, contents)
        .map_err(|error| format!("write {}: {error}", staging.display()))?;
    std::fs::rename(&staging, path).map_err(|error| {
        let _ = std::fs::remove_file(&staging);
        format!("replace {}: {error}", path.display())
    })
}

/// The index as written, without narrowing the entries to the fields a row
/// needs. A missing index is no entries; an unreadable one is an error, so a
/// delete never rewrites a file it could not understand.
fn read_raw_index(path: &Path) -> Result<Vec<serde_json::Value>, String> {
    let Ok(contents) = std::fs::read_to_string(path) else {
        return Ok(Vec::new());
    };
    if contents.trim().is_empty() {
        return Ok(Vec::new());
    }
    match serde_json::from_str(&contents) {
        Ok(entries) => Ok(entries),
        Err(error) => Err(format!("read {}: {error}", path.display())),
    }
}

fn is_subtask(task: &TaskEntry) -> bool {
    task.parent_id
        .as_deref()
        .is_some_and(|parent| !parent.is_empty())
}

/// The index file is an array; anything else (missing, empty, half written)
/// yields no rows and leaves the other connectors' histories intact.
fn read_index(path: &Path) -> Vec<TaskEntry> {
    let Ok(contents) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    if contents.trim().is_empty() {
        return Vec::new();
    }
    serde_json::from_str(&contents).unwrap_or_else(|error| {
        tracing::warn!("agent sessions: {} unreadable: {error}", path.display());
        Vec::new()
    })
}

fn conversation(root: &Path, task: TaskEntry) -> NormalizedConversation {
    let source_path = root.join(format!("{}.json", task.task_id));
    let ended_at = std::fs::metadata(&source_path)
        .ok()
        .map(|metadata| modified_ms(&metadata));
    NormalizedConversation {
        agent_slug: SLUG.to_owned(),
        external_id: Some(task.task_id),
        title: task
            .task_name
            .map(|name| name.trim().to_owned())
            .filter(|name| !name.is_empty()),
        workspace: workspace_of(&source_path),
        source_path,
        started_at: None,
        ended_at,
        metadata: serde_json::Value::Null,
        messages: Vec::new(),
    }
}

/// The directory a task ran in, read from the system prompt OpenDesk writes for
/// it.
///
/// The task document has no field for it: the directory appears only as a
/// labelled line in the prompt (" - 工作目录：<path>", or its English form). Only
/// the head of the document is read — the prompt is at the top of
/// `chat_context`, while the rest of the file is the transcript.
fn workspace_of(document: &Path) -> Option<PathBuf> {
    let file = std::fs::File::open(document).ok()?;
    let mut head = Vec::new();
    file.take(WORKSPACE_SCAN_BYTES).read_to_end(&mut head).ok()?;
    workspace_in(&String::from_utf8_lossy(&head))
}

fn workspace_in(text: &str) -> Option<PathBuf> {
    // The English marker is matched without regard to case; the Chinese one has
    // no case to fold.
    let lowered = text.to_ascii_lowercase();
    let (offset, marker_len) = WORKSPACE_MARKERS
        .iter()
        .find_map(|marker| lowered.find(marker).map(|offset| (offset, marker.len())))?;
    let value = text[offset + marker_len..]
        .split(['\\', '"', '\n', '\r'])
        .next()
        .map(str::trim)
        .filter(|value| value.starts_with('/'))?;
    Some(PathBuf::from(value))
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
    fn tasks_directory() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../agent-tests/fixtures/opendesk/tasks")
    }

    fn task(task_id: &str, task_name: Option<&str>, parent_id: Option<&str>) -> TaskEntry {
        TaskEntry {
            task_id: task_id.to_owned(),
            task_name: task_name.map(str::to_owned),
            parent_id: parent_id.map(str::to_owned),
        }
    }

    #[test]
    fn read_index_reads_entries() {
        let entries = read_index(&tasks_directory().join(TASK_INDEX_FILE));
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].task_id, "task_parent");
        assert_eq!(entries[0].task_name.as_deref(), Some("Audit the scanner"));
        assert_eq!(entries[0].parent_id, None);
        assert_eq!(entries[1].parent_id.as_deref(), Some("task_parent"));
    }

    /// Only a non-empty `parent_id` marks a sub-task; a task that names no
    /// parent is a row of its own.
    #[test]
    fn is_subtask_requires_a_parent_id() {
        assert!(is_subtask(&task("task_child", Some("sub task"), Some("task_parent"))));
        assert!(!is_subtask(&task("task_parent", Some("root"), None)));
        assert!(!is_subtask(&task("task_blank", Some("root"), Some(""))));
    }

    /// The row names the task document, which is what the cache watches for a
    /// change; the document's mtime stands in for the RFC 3339 `updated_at`.
    #[test]
    fn conversation_names_the_task_document() {
        let root = tasks_directory();
        let row = conversation(&root, task("task_parent", Some(" Audit the scanner "), None));
        assert_eq!(row.agent_slug, "opendesk");
        assert_eq!(row.external_id.as_deref(), Some("task_parent"));
        assert_eq!(row.title.as_deref(), Some("Audit the scanner"));
        // The directory comes from the labelled line in the system prompt.
        assert_eq!(row.workspace, Some(PathBuf::from("/repo/demo")));
        assert_eq!(row.source_path, root.join("task_parent.json"));
        assert!(
            row.ended_at.is_some_and(|updated| updated > 0),
            "mtime should become a positive update time, got {:?}",
            row.ended_at
        );
    }

    /// A task with no document and a blank name still yields a row: the id is
    /// enough to list it.
    #[test]
    fn conversation_keeps_a_row_without_a_document() {
        let root = tasks_directory();
        let row = conversation(&root, task("task_blank", Some("   "), Some("")));
        assert_eq!(row.title, None);
        assert_eq!(row.ended_at, None);
        assert_eq!(row.source_path, root.join("task_blank.json"));
    }

    /// Both prompt spellings name the directory, and the value stops at the
    /// escaped newline that follows it inside the JSON document.
    #[test]
    fn workspace_in_reads_the_labelled_line() {
        let chinese = r#"{"content":" - 工作目录：/repo/demo\n - 用户家目录：/home/demo\n"}"#;
        assert_eq!(workspace_in(chinese), Some(PathBuf::from("/repo/demo")));
        let english = r#"{"content":" - Working Directory: /work/tree\n - Home: /home/demo\n"}"#;
        assert_eq!(workspace_in(english), Some(PathBuf::from("/work/tree")));
    }

    /// A prompt without the line, or with a value that is not an absolute path,
    /// yields no directory rather than a guess.
    #[test]
    fn workspace_in_rejects_what_it_cannot_trust() {
        assert_eq!(workspace_in(r#"{"content":"no directory line here"}"#), None);
        assert_eq!(workspace_in(r#"{"content":" - 工作目录：relative/path\n"}"#), None);
    }

    /// A half-written index yields no rows and leaves the other connectors'
    /// histories intact.
    #[test]
    fn read_index_survives_truncated_file() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../agent-tests/fixtures/opendesk/broken")
            .join(TASK_INDEX_FILE);
        assert!(
            read_index(&path).is_empty(),
            "an index that is not valid JSON must yield no rows"
        );
    }

    #[test]
    fn read_index_missing_file_returns_empty() {
        assert!(
            read_index(&tasks_directory().join("missing.json")).is_empty(),
            "a missing index must yield no rows"
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

    /// A task, the sub-tasks pointing at it and their index entries go
    /// together, and the entries that stay keep the fields this scanner does
    /// not read.
    #[test]
    fn delete_in_removes_the_task_its_children_and_their_index_entries() {
        let store = scratch("opendesk-delete");
        for task_id in ["task_parent", "task_child", "task_keep"] {
            std::fs::write(store.join(format!("{task_id}.json")), "{}\n")
                .expect("write a task document");
        }
        let index = store.join(TASK_INDEX_FILE);
        std::fs::write(
            &index,
            r#"[
                {"task_id":"task_parent","parent_id":null,"task_name":"parent","task_status":"stopped"},
                {"task_id":"task_child","parent_id":"task_parent","task_name":"child","task_status":"stopped"},
                {"task_id":"task_keep","parent_id":null,"task_name":"keep","task_status":"running"}
            ]"#,
        )
        .expect("write the index");
        let root = store.canonicalize().expect("resolve the store");

        delete_in(&root, &store.join("task_parent.json")).expect("a task in the store is removable");

        assert!(!store.join("task_parent.json").exists());
        assert!(!store.join("task_child.json").exists(), "sub-tasks go with the parent");
        assert!(store.join("task_keep.json").exists(), "other tasks must survive");
        let remaining: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&index).expect("read the index"))
                .expect("the index stays valid JSON");
        let entries = remaining.as_array().expect("the index is an array");
        assert_eq!(entries.len(), 1, "only the untouched task is left, got {entries:?}");
        assert_eq!(entries[0]["task_id"], serde_json::json!("task_keep"));
        assert_eq!(
            entries[0]["task_status"],
            serde_json::json!("running"),
            "fields this scanner does not read must be written back unchanged"
        );
        std::fs::remove_dir_all(&store).ok();
    }

    /// The index names every task in the store, so it is not a session and must
    /// never be removable through the session path.
    #[test]
    fn delete_in_refuses_the_index_itself() {
        let store = scratch("opendesk-index");
        let index = store.join(TASK_INDEX_FILE);
        std::fs::write(
            &index,
            r#"[{"task_id":"task_parent","parent_id":null,"task_name":"parent"}]"#,
        )
        .expect("write the index");
        std::fs::write(store.join("task_parent.json"), "{}\n").expect("write the task document");
        let root = store.canonicalize().expect("resolve the store");

        let error = delete_in(&root, &index).expect_err("the index is not a session");

        assert!(
            error.contains("not a task document"),
            "the refusal should name the reason, got: {error}"
        );
        assert!(index.exists(), "the store index must survive");
        assert!(
            store.join("task_parent.json").exists(),
            "the task it lists must survive too"
        );
        std::fs::remove_dir_all(&store).ok();
    }

    /// A path outside the store is refused before anything is unlinked.
    #[test]
    fn delete_in_refuses_a_path_outside_the_store() {
        let store = scratch("opendesk-outside");
        let outside = store.join("outside");
        std::fs::create_dir_all(&outside).expect("create the outside directory");
        let document = outside.join("task_parent.json");
        std::fs::write(&document, "{}\n").expect("write the document");
        let root = store.join("tasks");
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

