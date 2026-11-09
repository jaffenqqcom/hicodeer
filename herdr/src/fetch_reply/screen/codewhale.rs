//! CodeWhale: a reply starts with a filled circle.

pub(super) fn extract(lines: &[String]) -> Option<String> {
    super::last_marked_block(lines, &["●", "⏺"], &["▎", "Thought"])
}
