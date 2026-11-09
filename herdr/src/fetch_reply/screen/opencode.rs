//! OpenCode: no glyph in front of a reply. Blocks are separated by blank rows
//! and a `Build ·` footer closes the turn.

pub(super) fn extract(lines: &[String]) -> Option<String> {
    super::last_plain_block(lines)
}
