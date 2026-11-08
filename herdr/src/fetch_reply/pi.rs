//! pi transcript: `message` records wrap a `message` object with a role and
//! content array.

use std::path::Path;

use serde_json::Value;

use super::{jsonl_records, read_tail, text_blocks, FetchError};

pub(super) fn extract(path: &Path) -> Result<String, FetchError> {
    let records = jsonl_records(&read_tail(path)?);
    for record in records.iter().rev() {
        if record.get("type").and_then(Value::as_str) != Some("message") {
            continue;
        }
        let message = record.get("message");
        let is_assistant = message
            .and_then(|value| value.get("role"))
            .and_then(Value::as_str)
            == Some("assistant");
        let is_user = message
            .and_then(|value| value.get("role"))
            .and_then(Value::as_str)
            == Some("user");
        if is_user {
            break;
        }
        if is_assistant {
            let text = text_blocks(message.and_then(|value| value.get("content")));
            if !text.is_empty() {
                return Ok(text);
            }
        }
    }
    Err(FetchError(format!(
        "no assistant message in {}",
        path.display()
    )))
}
