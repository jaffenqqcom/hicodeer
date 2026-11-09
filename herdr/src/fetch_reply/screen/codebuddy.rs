//! CodeBuddy: the same filled-circle reply marker as Claude Code.
//!
//! The prompt is a `>` line and the turn ends with a timing line, so the block
//! stops there.

pub(super) fn extract(lines: &[String]) -> Option<String> {
    super::last_marked_block(
        lines,
        &["●", "⏺"],
        &["∴ Thought", "✔ Worked", "✻", ">"],
    )
}
