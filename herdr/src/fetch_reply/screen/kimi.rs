//! Kimi: a reply starts with a filled circle; continuation lines are indented.

pub(super) fn extract(lines: &[String]) -> Option<String> {
    super::last_marked_block(lines, &["●", "⏺"], &["❯", "✦ Kimi"])
}
