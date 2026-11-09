//! QR code module matrix.
//!
//! The settings page draws the matrix itself. The OHOS backend has no image
//! pipeline (`img` elements never reach the screen), so a PNG would be
//! invisible; plain rectangles are drawn by the same quad path as every other
//! widget.

use anyhow::Context as _;
use qrcode::{Color, QrCode};

/// Returns one entry per row of the QR symbol, `true` for a dark module.
pub fn modules(content: &str) -> anyhow::Result<Vec<Vec<bool>>> {
    let code = QrCode::new(content.as_bytes()).context("cannot encode the QR payload")?;
    let width = code.width();
    let colors = code.to_colors();
    Ok((0..width)
        .map(|row| {
            (0..width)
                .map(|column| colors[row * width + column] == Color::Dark)
                .collect()
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matrix_is_square_and_has_finder_patterns() {
        let modules = modules("https://example.com").expect("encode");
        let side = modules.len();
        assert!(side >= 21, "a QR symbol is at least 21 modules wide");
        assert!(modules.iter().all(|row| row.len() == side));

        // The finder pattern starts with a full dark row and column at the
        // top-left corner; a symbol without it would not scan.
        assert!(modules[0].iter().take(7).all(|module| *module));
        assert!(modules.iter().take(7).all(|row| row[0]));
    }
}
