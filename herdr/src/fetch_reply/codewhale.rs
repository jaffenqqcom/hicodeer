//! Codewhale session: a single JSON document with a `messages` array.

use std::path::Path;

use serde_json::Value;

use super::text_blocks;
use super::FetchError;

pub(super) fn extract(path: &Path) -> Result<String, FetchError> {
    let text = std::fs::read_to_string(path)
        .map_err(|error| FetchError(format!("read {}: {error}", path.display())))?;
    let document: Value = serde_json::from_str(&text)
        .map_err(|error| FetchError(format!("decode {}: {error}", path.display())))?;
    let Some(messages) = document.get("messages").and_then(Value::as_array) else {
        return Err(FetchError(format!(
            "no messages array in {}",
            path.display()
        )));
    };
    for message in messages.iter().rev() {
        if message.get("role").and_then(Value::as_str) == Some("user") {
            break;
        }
        if message.get("role").and_then(Value::as_str) == Some("assistant") {
            let reply = text_blocks(message.get("content"));
            if !reply.is_empty() {
                return Ok(reply);
            }
        }
    }
    Err(FetchError(format!(
        "no assistant message in {}",
        path.display()
    )))
}
