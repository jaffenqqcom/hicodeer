//! Remembers agent renames so they survive a daemon restart.
//!
//! The table lives beside `session.json` in `agent-names.json`. It is read on
//! every call instead of being cached, because the same data directory can be
//! written by another process and a stale cache would resurrect an old name.

use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tracing::warn;

/// Bumped when the on-disk shape changes in a way older code cannot read.
const SCHEMA_VERSION: u32 = 1;
/// Extension of the temporary file written before the table replaces the old one.
const TEMPORARY_FILE_EXTENSION: &str = "json.tmp";

#[derive(Debug, Deserialize, Serialize)]
struct AgentNameTable {
    schema_version: u32,
    names: BTreeMap<String, String>,
}

impl Default for AgentNameTable {
    fn default() -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            names: BTreeMap::new(),
        }
    }
}

fn table_path() -> PathBuf {
    crate::session::data_dir().join("agent-names.json")
}

/// Session identity key: `<source>:<session value>`.
fn key_for(source: &str, value: &str) -> String {
    format!("{source}:{value}")
}

fn load_from(path: &Path) -> AgentNameTable {
    let content = match fs::read_to_string(path) {
        Ok(content) => content,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return AgentNameTable::default(),
        Err(err) => {
            warn!(path = %path.display(), err = %err, "failed to read agent names");
            return AgentNameTable::default();
        }
    };
    match serde_json::from_str::<AgentNameTable>(&content) {
        Ok(table) if table.schema_version == SCHEMA_VERSION => table,
        Ok(table) => {
            warn!(
                path = %path.display(),
                schema_version = table.schema_version,
                "ignoring agent names written by an unsupported schema version"
            );
            AgentNameTable::default()
        }
        Err(err) => {
            warn!(path = %path.display(), err = %err, "failed to parse agent names");
            AgentNameTable::default()
        }
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
        let _ = fs::remove_file(&temporary);
        return Err(err);
    }
    crate::platform::sync_parent_directory(path.parent().unwrap_or_else(|| Path::new(".")))
}

/// Returns the remembered name for a session identity.
pub(crate) fn lookup(source: &str, value: &str) -> Option<String> {
    let table = load_from(&table_path());
    table.names.get(&key_for(source, value)).cloned()
}

/// Remembers a rename for a session identity.
pub(crate) fn store(source: &str, value: &str, name: &str) {
    let path = table_path();
    let mut table = load_from(&path);
    table.names.insert(key_for(source, value), name.to_string());
    if let Err(err) = save_to(&path, &table) {
        warn!(path = %path.display(), err = %err, "failed to save agent names");
    }
}

/// Forgets a rename for a session identity.
pub(crate) fn remove(source: &str, value: &str) {
    let path = table_path();
    let mut table = load_from(&path);
    if table.names.remove(&key_for(source, value)).is_none() {
        return;
    }
    if let Err(err) = save_to(&path, &table) {
        warn!(path = %path.display(), err = %err, "failed to save agent names");
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
        let _ = fs::remove_dir_all(&base);
        result
    }

    fn write_table(contents: &str) {
        let path = table_path();
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, contents).unwrap();
    }

    #[test]
    fn store_lookup_and_remove_round_trip() {
        with_temp_config(|| {
            assert_eq!(lookup("hicodeer:codebuddy", "session-1"), None);

            store("hicodeer:codebuddy", "session-1", "reviewer");

            assert_eq!(
                lookup("hicodeer:codebuddy", "session-1").as_deref(),
                Some("reviewer")
            );
            assert_eq!(lookup("hicodeer:codebuddy", "session-2"), None);

            remove("hicodeer:codebuddy", "session-1");

            assert_eq!(lookup("hicodeer:codebuddy", "session-1"), None);
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
}
