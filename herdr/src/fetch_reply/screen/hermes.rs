//! Hermes: a reply is plain text; the prompt is a `❯` line and the status bar
//! uses the `☤` glyph.

pub(super) fn extract(lines: &[String]) -> Option<String> {
    super::last_plain_block(lines)
}
