//! OpenCode session: SQLite `session_message` rows whose `data` is a JSON
//! string containing a `content` array. The newest row carrying text blocks
//! holds the last assistant reply.

use std::path::Path;

use serde_json::Value;

use super::{split_sqlite_source, sqlite_query, text_blocks, FetchError};

/// Number of recent `session_message` rows inspected for a text block.
const RECENT_ROWS: usize = 10;

pub(super) fn extract(path: &Path) -> Result<String, FetchError> {
    let (database, session_id) = split_sqlite_source(path)?;
    let sql = format!(
        "select data from session_message \
         where session_id = '{session_id}' \
         order by time_created desc limit {RECENT_ROWS}"
    );
    let rows = sqlite_query(&database, &sql)?;
    for row in rows.iter() {
        let Some(data) = row.get("data").and_then(Value::as_str) else {
            continue;
        };
        let Ok(decoded) = serde_json::from_str::<Value>(data) else {
            continue;
        };
        let text = text_blocks(decoded.get("content"));
        if !text.is_empty() {
            return Ok(text);
        }
    }
    Err(FetchError(format!(
        "no assistant text in {} for session {}",
        database.display(),
        session_id
    )))
}
