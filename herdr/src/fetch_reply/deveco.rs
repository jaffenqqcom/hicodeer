//! DevEco Code session: OpenCode-style SQLite. The last assistant message owns
//! one or more `text` parts in the `part` table.

use std::path::Path;

use super::{open_sqlite_reply, FetchError};

pub(super) fn extract(path: &Path) -> Result<String, FetchError> {
    open_sqlite_reply(path)
}
