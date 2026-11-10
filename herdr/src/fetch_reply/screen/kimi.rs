//! Kimi: a reply starts with a filled circle; continuation lines are indented,
//! and an Ask panel frames its question between two rules.

/// The glyph a reply row starts with.
const REPLY_MARKERS: [&str; 2] = ["\u{25cf}", "\u{23fa}"];

/// The two halves of the hint row the Ask panel prints under its answers.
const ASK_FOOTER_HINTS: [&str; 2] = ["\u{2191}\u{2193} select", "esc cancel"];

/// The shortest rule that can frame the panel.
///
/// A rule drawn inside a reply is shorter than the terminal width, so a short
/// run of rule characters must not end the walk towards the reply above.
const RULE_MIN_WIDTH: usize = 10;

pub(super) fn extract(lines: &[String]) -> Option<String> {
    super::last_marked_block(lines, &["\u{25cf}", "\u{23fa}"], &["\u{276f}", "\u{2726} Kimi"])
}

/// Reads the Ask panel: the reply above it, the question, and the answers.
///
/// The panel writes each answer as "→ [1] label", and indents the explanation
/// under the answer it belongs to. The reply that introduces the panel sits
/// above the rule framing the top of the panel.
pub(super) fn extract_choice(lines: &[String]) -> Option<String> {
    let footer = lines.iter().rposition(|line| is_ask_footer(line))?;
    let first_answer = lines[..footer]
        .iter()
        .rposition(|line| super::numbered(line).is_some_and(|(number, _)| number == 1))?;
    let options = super::collect_numbered_options(lines, first_answer);
    // One answer is not a choice; the row is then part of the reply above it.
    if options.len() < 2 {
        return None;
    }
    let question_start = lines[..first_answer]
        .iter()
        .rposition(|line| super::strip_frame(line).starts_with('?'))?;
    let question: Vec<String> = lines[question_start..first_answer]
        .iter()
        .filter_map(|line| {
            let text = super::strip_frame(line);
            if text.is_empty() || super::is_chrome(line) || super::is_decoration(line) {
                return None;
            }
            let text = text.strip_prefix('?').unwrap_or(text).trim_start();
            (!text.is_empty()).then(|| text.to_string())
        })
        .collect();
    // The rule above the question is the panel's own top edge: without it the
    // rows above cannot be told apart from the panel, and an earlier turn's
    // reply would be pushed in its place.
    let panel_top = panel_top(lines, question_start)?;
    let mut body: Vec<String> = Vec::new();
    if let Some(preamble) = super::reply_above_panel(lines, panel_top, reply_marker, |_| false) {
        body.push(preamble);
    }
    body.extend(question);
    body.extend(options);
    Some(body.join("
"))
}

/// The rule that frames the top of the panel.
fn panel_top(lines: &[String], question_start: usize) -> Option<usize> {
    lines[..question_start].iter().rposition(|line| {
        super::is_decoration(line) && line.trim().chars().count() >= RULE_MIN_WIDTH
    })
}

fn reply_marker(line: &str) -> Option<&'static str> {
    REPLY_MARKERS
        .into_iter()
        .find(|marker| line.starts_with(marker))
}

fn is_ask_footer(line: &str) -> bool {
    ASK_FOOTER_HINTS.iter().all(|hint| line.contains(hint))
}
