//! ZCode session: OpenCode-style SQLite, the same schema as DevEco Code. The
//! last assistant message owns one or more `text` parts in the `part` table.

use std::path::Path;

use super::{open_sqlite_reply, FetchError};

pub(super) fn extract(path: &Path) -> Result<String, FetchError> {
    open_sqlite_reply(path)
}
