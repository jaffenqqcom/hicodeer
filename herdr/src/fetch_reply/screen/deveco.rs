//! DevEco: same layout as OpenCode, with a `▣ Build` footer.

pub(super) fn extract(lines: &[String]) -> Option<String> {
    super::last_plain_block(lines)
}
