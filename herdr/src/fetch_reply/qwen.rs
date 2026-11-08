//! Qwen Code transcript: `assistant` records carry `message.parts[].text`.

use std::path::Path;

use serde_json::Value;

use super::{jsonl_records, read_tail, text_blocks, FetchError};

pub(super) fn extract(path: &Path) -> Result<String, FetchError> {
    let records = jsonl_records(&read_tail(path)?);
    let mut blocks: Vec<String> = Vec::new();
    for record in records.iter().rev() {
        match record.get("type").and_then(Value::as_str) {
            Some("user") => break,
            Some("assistant") => {
                let text = text_blocks(
                    record
                        .get("message")
                        .and_then(|message| message.get("parts")),
                );
                if !text.is_empty() {
                    blocks.push(text);
                }
            }
            _ => {}
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
