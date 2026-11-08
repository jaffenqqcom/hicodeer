//! CodeBuddy transcript: newline-delimited JSON with `role` on `message`
//! records and `output_text` blocks in `content`.

use std::path::Path;

use serde_json::Value;

use super::{jsonl_records, read_tail, text_blocks, FetchError};

pub(super) fn extract(path: &Path) -> Result<String, FetchError> {
    let records = jsonl_records(&read_tail(path)?);
    let mut blocks: Vec<String> = Vec::new();
    for record in records.iter().rev() {
        let role = record.get("role").and_then(Value::as_str);
        if role == Some("user") {
            break;
        }
        if record.get("type").and_then(Value::as_str) == Some("message")
            && role == Some("assistant")
        {
            let text = text_blocks(record.get("content"));
            if !text.is_empty() {
                blocks.push(text);
            }
        }
    }
    blocks.reverse();
    if blocks.is_empty() {
        return Err(FetchError(format!(
            "no assistant text in {}",
            path.display()
        )));
    }
    Ok(blocks.join("\n"))
}
