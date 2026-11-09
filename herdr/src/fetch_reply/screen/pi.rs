//! Pi: no prompt glyph and no reply glyph. The last block of plain text is the
//! reply.

pub(super) fn extract(lines: &[String]) -> Option<String> {
    super::last_plain_block(lines)
}
