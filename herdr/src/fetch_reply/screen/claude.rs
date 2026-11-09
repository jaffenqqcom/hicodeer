//! Claude Code: a reply starts with a filled circle and continues indented.

pub(super) fn extract(lines: &[String]) -> Option<String> {
    super::last_marked_block(lines, &["●", "⏺"], &["✻", "❯", "Thought for"])
}
