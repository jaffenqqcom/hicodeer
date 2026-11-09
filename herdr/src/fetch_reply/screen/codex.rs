//! Codex: a reply starts with a bullet, and the turn ends with a timing line.

pub(super) fn extract(lines: &[String]) -> Option<String> {
    super::last_marked_block(lines, &["•", "●"], &["Worked for", "›"])
}
