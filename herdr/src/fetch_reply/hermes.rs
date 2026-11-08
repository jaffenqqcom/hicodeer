//! Hermes session: SQLite `messages` table with explicit `role` and `content`
//! columns. The most recent `assistant` row holds the reply.

use std::path::Path;

use serde_json::Value;

use super::{split_sqlite_source, sqlite_query, FetchError};

/// The welcome box only shows the first eight hex characters of a session id,
/// so such a value matches the full `local-<hash>` row by prefix. Anything else
/// is a full id and has to match exactly.
fn session_selector(session_id: &str) -> String {
    const SHORT_ID_LEN: usize = 8;
    let escaped = session_id.replace('\'', "''");
    let is_short_hex = session_id.len() == SHORT_ID_LEN
        && session_id.chars().all(|ch| ch.is_ascii_hexdigit());
    if is_short_hex {
        format!("session_id like 'local-{escaped}%'")
    } else {
        format!("session_id = '{escaped}'")
    }
}

pub(super) fn extract(path: &Path) -> Result<String, FetchError> {
    let (database, session_id) = split_sqlite_source(path)?;
    // Tool-loop steps are also stored as assistant rows, with
    // finish_reason = 'tool_calls'; only a stopped or truncated turn carries
    // the user-visible answer. Rows written by providers that omit the column
    // are accepted as well, so a missing value never hides a reply.
    let sql = format!(
        "select content from messages \
         where {} and role = 'assistant' \
         and (finish_reason is null or finish_reason in ('stop', 'length')) \
         and trim(content) <> '' \
         order by id desc limit 1",
        session_selector(&session_id)
    );
    let rows = sqlite_query(&database, &sql)?;
    if let Some(text) = rows
        .first()
        .and_then(|row| row.get("content"))
        .and_then(Value::as_str)
        .filter(|text| !text.is_empty())
    {
        return Ok(text.to_owned());
    }
    Err(FetchError(format!(
        "no assistant text in {} for session {}",
        database.display(),
        session_id
    )))
}

#[cfg(test)]
mod tests {
    use super::session_selector;

    #[test]
    fn short_screen_id_matches_by_prefix() {
        assert_eq!(
            session_selector("bbb2b94b"),
            "session_id like 'local-bbb2b94b%'"
        );
    }

    #[test]
    fn full_session_id_matches_exactly() {
        assert_eq!(
            session_selector("local-bbb2b94ba29f"),
            "session_id = 'local-bbb2b94ba29f'"
        );
    }

    #[test]
    fn non_hex_value_is_not_treated_as_a_screen_prefix() {
        assert_eq!(
            session_selector("zz2b94ba"),
            "session_id = 'zz2b94ba'"
        );
    }
}
