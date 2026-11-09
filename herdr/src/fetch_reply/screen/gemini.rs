//! Gemini: a reply starts with a four-pointed star.

pub(super) fn extract(lines: &[String]) -> Option<String> {
    super::last_marked_block(lines, &["✦"], &[">", "▀", "▄"])
}
