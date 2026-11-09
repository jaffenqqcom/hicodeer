//! MiniMax: a reply starts with a filled circle; the thinking line is a `└`.

pub(super) fn extract(lines: &[String]) -> Option<String> {
    super::last_marked_block(lines, &["●", "⏺"], &["└", "›"])
}
