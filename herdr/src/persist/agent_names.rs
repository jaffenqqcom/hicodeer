//! Remembers agent renames so they survive a daemon restart.
//!
//! The table lives beside `session.json` in `agent-names.json`. It is read on
//! every call instead of being cached, because the same data directory can be
//! written by another process and a stale cache would resurrect an old name.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use tracing::{error, warn};

/// Bumped when the on-disk shape changes in a way older code cannot read.
const SCHEMA_VERSION: u32 = 2;
/// The shape written before `last_touched` was added. It is still accepted on
/// read so a rename remembered by an older build is not lost on upgrade.
const LEGACY_SCHEMA_VERSION: u32 = 1;
/// Maximum number of remembered names kept on disk. Eviction is lazy: it only
/// runs when `store` would push the table past this limit.
const MAX_AGENT_NAME_ENTRIES: usize = 100;
/// Timestamp for entries that carry no `last_touched` yet (v1 migration) and
/// for a clock that cannot be read. Zero sorts oldest, so a running agent
/// refreshes it on the next touch before eviction can consider dropping it.
const UNKNOWN_LAST_TOUCHED: u64 = 0;
/// Extension of the temporary file written before the table replaces the old one.
const TEMPORARY_FILE_EXTENSION: &str = "json.tmp";

#[derive(Debug, Clone, Deserialize, Serialize)]
struct AgentNameEntry {
    name: String,
    #[serde(default)]
    last_touched: u64,
}

/// On-disk value of one entry. v1 wrote a bare name; v2 writes an object with
/// a timestamp. Accepting both lets an upgrade read the old file without losing
/// anything and rewrite it in the new shape on the next `store`.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum RawNameValue {
    Entry(AgentNameEntry),
    Legacy(String),
}

#[derive(Debug, Deserialize)]
struct RawAgentNameTable {
    schema_version: u32,
    names: BTreeMap<String, RawNameValue>,
}

#[derive(Debug, Deserialize, Serialize)]
struct AgentNameTable {
    schema_version: u32,
    names: BTreeMap<String, AgentNameEntry>,
}

impl Default for AgentNameTable {
    fn default() -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            names: BTreeMap::new(),
        }
    }
}

/// Why a table could not be loaded for writing. A malformed or future-schema
/// file must never be replaced by a fresh table, or every other remembered
/// name in it would be silently lost.
#[derive(Debug)]
pub(crate) enum AgentNamesError {
    LoadRejected(String),
    Io(std::io::Error),
}

impl std::fmt::Display for AgentNamesError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::LoadRejected(reason) => {
                write!(formatter, "agent name table rejected: {reason}")
            }
            Self::Io(err) => write!(formatter, "agent name table write failed: {err}"),
        }
    }
}

impl std::error::Error for AgentNamesError {}

enum LoadOutcome {
    Loaded(AgentNameTable),
    Missing,
    Rejected(String),
}

fn table_path() -> PathBuf {
    crate::session::data_dir().join("agent-names.json")
}

/// Session identity key: `<source>:<session value>`.
pub(crate) fn key_for(source: &str, value: &str) -> String {
    format!("{source}:{value}")
}

fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis().min(u128::from(u64::MAX)) as u64)
        .unwrap_or(UNKNOWN_LAST_TOUCHED)
}

fn parse_table(content: &str) -> Result<AgentNameTable, String> {
    let raw: RawAgentNameTable =
        serde_json::from_str(content).map_err(|err| format!("parse failed: {err}"))?;
    if raw.schema_version != SCHEMA_VERSION && raw.schema_version != LEGACY_SCHEMA_VERSION {
        return Err(format!("unsupported schema version {}", raw.schema_version));
    }
    let names = raw
        .names
        .into_iter()
        .map(|(key, value)| {
            let entry = match value {
                RawNameValue::Entry(entry) => entry,
                RawNameValue::Legacy(name) => AgentNameEntry {
                    name,
                    last_touched: UNKNOWN_LAST_TOUCHED,
                },
            };
            (key, entry)
        })
        .collect();
    Ok(AgentNameTable {
        schema_version: SCHEMA_VERSION,
        names,
    })
}

fn load_from(path: &Path) -> LoadOutcome {
    let content = match fs::read_to_string(path) {
        Ok(content) => content,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return LoadOutcome::Missing,
        Err(err) => {
            warn!(path = %path.display(), err = %err, "failed to read agent names");
            return LoadOutcome::Rejected(format!("read failed: {err}"));
        }
    };
    match parse_table(&content) {
        Ok(table) => LoadOutcome::Loaded(table),
        Err(reason) => {
            warn!(path = %path.display(), reason = %reason, "failed to parse agent names");
            LoadOutcome::Rejected(reason)
        }
    }
}

/// Read path: never blocks. A missing, malformed or future-schema file is
/// treated as empty so `lookup` still returns `None`.
fn load_for_read(path: &Path) -> AgentNameTable {
    match load_from(path) {
        LoadOutcome::Loaded(table) => table,
        LoadOutcome::Missing | LoadOutcome::Rejected(_) => AgentNameTable::default(),
    }
}

/// Write path: a malformed or future-schema file is a hard error, so `store`
/// cannot replace it with a table that only contains the new entry.
fn load_for_write(path: &Path) -> Result<AgentNameTable, AgentNamesError> {
    match load_from(path) {
        LoadOutcome::Loaded(table) => Ok(table),
        LoadOutcome::Missing => Ok(AgentNameTable::default()),
        LoadOutcome::Rejected(reason) => Err(AgentNamesError::LoadRejected(reason)),
    }
}

fn save_to(path: &Path, table: &AgentNameTable) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_string_pretty(table)?;
    let temporary = path.with_extension(TEMPORARY_FILE_EXTENSION);
    let mut file = fs::File::create(&temporary)?;
    let result = file
        .write_all(json.as_bytes())
        .and_then(|()| file.sync_all());
    drop(file);
    if let Err(err) = result.and_then(|()| crate::platform::replace_file(&temporary, path)) {
        if let Err(cleanup_err) = fs::remove_file(&temporary) {
            warn!(path = %temporary.display(), err = %cleanup_err, "failed to remove temporary agent names file");
        }
        return Err(err);
    }
    crate::platform::sync_parent_directory(path.parent().unwrap_or_else(|| Path::new(".")))
}

/// Returns the remembered name for a session identity.
pub(crate) fn lookup(source: &str, value: &str) -> Option<String> {
    let table = load_for_read(&table_path());
    table
        .names
        .get(&key_for(source, value))
        .map(|entry| entry.name.clone())
}

/// Remembers a rename for a session identity.
///
/// `in_use_keys` holds the table keys of agents that are currently running.
/// Their timestamps are refreshed on every write and eviction never removes
/// them, so a running conversation keeps its name even when its timestamp is
/// stale (a clock that moved back, or a v1 entry with no timestamp).
pub(crate) fn store(
    source: &str,
    value: &str,
    name: &str,
    in_use_keys: &BTreeSet<String>,
) -> Result<(), AgentNamesError> {
    let path = table_path();
    let mut table = load_for_write(&path)?;
    let key = key_for(source, value);
    let now = now_millis();
    table.names.insert(
        key.clone(),
        AgentNameEntry {
            name: name.to_string(),
            last_touched: now,
        },
    );
    touch_and_evict(&mut table, in_use_keys, &key, now);
    if let Err(err) = save_to(&path, &table) {
        error!(path = %path.display(), err = %err, "failed to save agent names");
        return Err(AgentNamesError::Io(err));
    }
    Ok(())
}

fn touch_and_evict(
    table: &mut AgentNameTable,
    in_use_keys: &BTreeSet<String>,
    written_key: &str,
    now: u64,
) {
    for key in in_use_keys {
        if let Some(entry) = table.names.get_mut(key) {
            entry.last_touched = now;
        }
    }
    if let Some(entry) = table.names.get_mut(written_key) {
        entry.last_touched = now;
    }
    if table.names.len() <= MAX_AGENT_NAME_ENTRIES {
        return;
    }
    let mut evictable: Vec<(u64, String)> = table
        .names
        .iter()
        .filter(|(key, _)| !in_use_keys.contains(key.as_str()) && key.as_str() != written_key)
        .map(|(key, entry)| (entry.last_touched, key.to_string()))
        .collect();
    evictable.sort_by(|left, right| left.0.cmp(&right.0).then_with(|| left.1.cmp(&right.1)));
    let excess = table.names.len() - MAX_AGENT_NAME_ENTRIES;
    for (_timestamp, key) in evictable.into_iter().take(excess) {
        table.names.remove(&key);
    }
}

/// Global reset: drops the whole file. A missing file is already the empty
/// state, so it is treated as success; the next `store` recreates the table.
pub(crate) fn clear_all() -> Result<(), AgentNamesError> {
    let path = table_path();
    match fs::remove_file(&path) {
        Ok(()) => crate::platform::sync_parent_directory(
            path.parent().unwrap_or_else(|| Path::new(".")),
        )
        .map_err(AgentNamesError::Io),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => {
            error!(path = %path.display(), err = %err, "failed to clear agent names");
            Err(AgentNamesError::Io(err))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with_temp_config<T>(run: impl FnOnce() -> T) -> T {
        let _guard = crate::config::test_config_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let base = std::env::temp_dir().join(format!(
            "hicodeer-agent-names-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&base).unwrap();
        let previous = std::env::var_os("XDG_CONFIG_HOME");
        std::env::set_var("XDG_CONFIG_HOME", &base);
        let result = run();
        match previous {
            Some(value) => std::env::set_var("XDG_CONFIG_HOME", value),
            None => std::env::remove_var("XDG_CONFIG_HOME"),
        }
        if let Err(err) = fs::remove_dir_all(&base) {
            eprintln!(
                "agent name test cleanup failed for {}: {err}",
                base.display()
            );
        }
        result
    }

    fn write_table(contents: &str) {
        let path = table_path();
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, contents).unwrap();
    }

    fn read_table_json() -> serde_json::Value {
        let content = fs::read_to_string(table_path()).unwrap();
        serde_json::from_str(&content).unwrap()
    }

    fn empty_in_use() -> BTreeSet<String> {
        BTreeSet::new()
    }

    #[test]
    fn store_lookup_round_trip() {
        with_temp_config(|| {
            assert_eq!(lookup("hicodeer:codebuddy", "session-1"), None);

            store("hicodeer:codebuddy", "session-1", "reviewer", &empty_in_use()).unwrap();

            assert_eq!(
                lookup("hicodeer:codebuddy", "session-1").as_deref(),
                Some("reviewer")
            );
            assert_eq!(lookup("hicodeer:codebuddy", "session-2"), None);
        });
    }

    #[test]
    fn lookup_returns_none_when_the_file_is_missing() {
        with_temp_config(|| {
            assert!(!table_path().exists());
            assert_eq!(lookup("hicodeer:codebuddy", "session-1"), None);
        });
    }

    #[test]
    fn lookup_ignores_a_corrupt_file() {
        with_temp_config(|| {
            write_table("{ not json");
            assert_eq!(lookup("hicodeer:codebuddy", "session-1"), None);
        });
    }

    #[test]
    fn lookup_ignores_an_unsupported_schema_version() {
        with_temp_config(|| {
            write_table(
                r#"{"schema_version": 99, "names": {"hicodeer:codebuddy:session-1": "reviewer"}}"#,
            );
            assert_eq!(lookup("hicodeer:codebuddy", "session-1"), None);
        });
    }

    #[test]
    fn v1_table_is_read_and_upgraded_without_losing_names() {
        with_temp_config(|| {
            write_table(
                r#"{"schema_version": 1, "names": {"hicodeer:codebuddy:session-1": "reviewer"}}"#,
            );
            assert_eq!(
                lookup("hicodeer:codebuddy", "session-1").as_deref(),
                Some("reviewer"),
                "a v1 name must still be readable"
            );

            store(
                "hicodeer:codebuddy",
                "session-2",
                "planner",
                &empty_in_use(),
            )
            .unwrap();

            assert_eq!(
                lookup("hicodeer:codebuddy", "session-1").as_deref(),
                Some("reviewer"),
                "the migrated name must survive the rewrite as v2"
            );
            assert_eq!(
                lookup("hicodeer:codebuddy", "session-2").as_deref(),
                Some("planner")
            );
            assert_eq!(
                read_table_json()["schema_version"].as_u64(),
                Some(u64::from(SCHEMA_VERSION))
            );
        });
    }

    #[test]
    fn unsupported_schema_rejects_store_without_overwriting() {
        with_temp_config(|| {
            let original = r#"{"schema_version": 99, "names": {"hicodeer:codebuddy:session-1": "reviewer"}}"#;
            write_table(original);
            assert_eq!(lookup("hicodeer:codebuddy", "session-1"), None);

            let result = store(
                "hicodeer:codebuddy",
                "session-2",
                "planner",
                &empty_in_use(),
            );
            assert!(result.is_err(), "saving over an unknown schema must fail");
            assert_eq!(
                fs::read_to_string(table_path()).unwrap(),
                original,
                "a rejected file must not be replaced by a one-entry table"
            );
        });
    }

    #[test]
    fn corrupt_file_rejects_store_without_overwriting() {
        with_temp_config(|| {
            let original = "{ not json";
            write_table(original);
            let result = store(
                "hicodeer:codebuddy",
                "session-1",
                "reviewer",
                &empty_in_use(),
            );
            assert!(result.is_err(), "saving over a corrupt file must fail");
            assert_eq!(
                fs::read_to_string(table_path()).unwrap(),
                original,
                "the corrupt file must not be replaced"
            );
        });
    }

    #[test]
    fn store_returns_error_when_the_table_cannot_be_written() {
        with_temp_config(|| {
            store(
                "hicodeer:codebuddy",
                "session-1",
                "reviewer",
                &empty_in_use(),
            )
            .unwrap();
            // The atomic writer creates this exact temporary path; occupying it
            // with a directory makes the create call fail.
            let temporary = table_path().with_extension(TEMPORARY_FILE_EXTENSION);
            fs::create_dir_all(&temporary).unwrap();

            let result = store(
                "hicodeer:codebuddy",
                "session-2",
                "planner",
                &empty_in_use(),
            );
            assert!(result.is_err());
            assert_eq!(
                lookup("hicodeer:codebuddy", "session-1").as_deref(),
                Some("reviewer"),
                "the previous table must remain intact"
            );
        });
    }

    #[test]
    fn store_refreshes_last_touched_for_in_use_entries() {
        with_temp_config(|| {
            let key = key_for("hicodeer:codebuddy", "session-1");
            write_table(&format!(
                r#"{{"schema_version": {SCHEMA_VERSION}, "names": {{"{key}": {{"name": "reviewer", "last_touched": 1}}}}}}"#
            ));

            let mut in_use = BTreeSet::new();
            in_use.insert(key.clone());
            store(
                "hicodeer:codebuddy",
                "session-2",
                "planner",
                &in_use,
            )
            .unwrap();

            let refreshed = read_table_json()["names"][key.as_str()]["last_touched"]
                .as_u64()
                .unwrap();
            assert!(refreshed > 1, "an in-use entry must be touched to now");
        });
    }

    #[test]
    fn store_evicts_oldest_when_over_limit_and_keeps_in_use_entries() {
        with_temp_config(|| {
            let mut names = serde_json::Map::new();
            for index in 0..MAX_AGENT_NAME_ENTRIES {
                let key = key_for("hicodeer:codebuddy", &format!("session-{index}"));
                names.insert(
                    key,
                    serde_json::json!({
                        "name": (format!("name-{index}")),
                        "last_touched": (index as u64)
                    }),
                );
            }
            let full = serde_json::json!({
                "schema_version": (SCHEMA_VERSION),
                "names": (names)
            });
            write_table(&serde_json::to_string(&full).unwrap());

            // The oldest entry is still running; eviction must skip it and drop
            // the next oldest instead.
            let mut in_use = BTreeSet::new();
            in_use.insert(key_for("hicodeer:codebuddy", "session-0"));
            store("hicodeer:codebuddy", "fresh", "fresh-name", &in_use).unwrap();

            assert_eq!(
                lookup("hicodeer:codebuddy", "session-0").as_deref(),
                Some("name-0"),
                "a running entry must never be evicted"
            );
            assert_eq!(
                lookup("hicodeer:codebuddy", "session-1"),
                None,
                "the oldest idle entry should be evicted"
            );
            assert_eq!(
                lookup("hicodeer:codebuddy", "fresh").as_deref(),
                Some("fresh-name")
            );
            assert_eq!(
                read_table_json()["names"].as_object().unwrap().len(),
                MAX_AGENT_NAME_ENTRIES
            );
        });
    }

    #[test]
    fn clear_all_removes_the_file_and_next_store_rebuilds() {
        with_temp_config(|| {
            store(
                "hicodeer:codebuddy",
                "session-1",
                "reviewer",
                &empty_in_use(),
            )
            .unwrap();
            assert!(table_path().exists());

            clear_all().unwrap();
            assert!(!table_path().exists());
            assert_eq!(lookup("hicodeer:codebuddy", "session-1"), None);
            // Clearing an already-empty table is idempotent.
            clear_all().unwrap();

            store(
                "hicodeer:codebuddy",
                "session-2",
                "planner",
                &empty_in_use(),
            )
            .unwrap();
            assert_eq!(
                lookup("hicodeer:codebuddy", "session-2").as_deref(),
                Some("planner")
            );
        });
    }
}
