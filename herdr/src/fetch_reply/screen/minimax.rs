//! MiniMax: a reply starts with a filled circle and runs as indented paragraphs
//! separated by blank rows; an "Ask" panel numbers its answers with spaces
//! instead of punctuation.

/// The glyph a reply row starts with.
const REPLY_MARKERS: [&str; 2] = ["●", "⏺"];

/// The two halves of the hint row the Ask panel prints under its answers.
///
/// It is the panel's own marker: a numbered list inside a reply has no such
/// row, so nothing else on screen can be mistaken for the panel.
const ASK_FOOTER_HINTS: [&str; 2] = ["↑↓ move", "Esc cancel"];

pub(super) fn extract(lines: &[String]) -> Option<String> {
    // Some status lines use the same glyph as a reply, so a block that reads as
    // page chrome is skipped rather than filtered out up front: the reply itself
    // may well start with a row of chrome words.
    let mut end = lines.len();
    while end > 0 {
        let start = lines[..end]
            .iter()
            .rposition(|line| reply_marker(line.trim_start()).is_some())?;
        let block = super::collect_marked_block(&lines[..end], start, reply_marker, |_| false)?;
        // Only a block that is chrome from end to end is a status line; a
        // reply may open with the same words ("用 ↑↓ 选择…") and still be real.
        if block.lines().all(super::is_chrome) {
            end = start;
            continue;
        }
        return Some(block);
    }
    None
}

/// Reads the Ask panel: the rows above it, the question, and the answers.
///
/// The panel writes each answer as "› 1  label" or "  2  label" — the number is
/// separated from its label by spaces alone, which the shared option reader
/// does not accept, so the panel is read here.
pub(super) fn extract_choice(lines: &[String]) -> Option<String> {
    let footer = lines.iter().rposition(|line| is_ask_footer(line))?;
    // Walk up from the footer: the answer list sits directly above it, and the
    // blank row below the question ends that list.
    let mut answers: Vec<(u32, String)> = Vec::new();
    let mut index = footer;
    while index > 0 {
        index -= 1;
        if let Some((number, label)) = ask_option(panel_text(&lines[index])) {
            answers.push((number, label));
            continue;
        }
        if answers.is_empty() {
            continue;
        }
        break;
    }
    answers.reverse();
    // One answer is not a choice; the row is then part of something else.
    if answers.len() < 2 {
        return None;
    }
    // The panel numbers its answers from one upwards, so a row that breaks the
    // run is text inside the question rather than an answer.
    if answers
        .iter()
        .enumerate()
        .any(|(position, (number, _))| *number != position as u32 + 1)
    {
        return None;
    }
    let answers: Vec<String> = answers
        .into_iter()
        .map(|(number, label)| format!("{number}. {label}"))
        .collect();
    let mut question: Vec<String> = Vec::new();
    while index > 0 {
        index -= 1;
        let line = &lines[index];
        let text = panel_text(line);
        if text.is_empty() {
            if question.is_empty() {
                continue;
            }
            break;
        }
        if is_panel_edge(line) {
            break;
        }
        question.push(text.to_string());
    }
    question.reverse();
    let mut body: Vec<String> = Vec::new();
    if let Some(preamble) = super::reply_above_panel(lines, index, reply_marker, |_| false) {
        body.push(preamble);
    }
    body.extend(question);
    body.extend(answers);
    Some(body.join("\n"))
}

fn reply_marker(line: &str) -> Option<&'static str> {
    REPLY_MARKERS
        .into_iter()
        .find(|marker| line.starts_with(marker))
}

/// A panel row without the frame bars on both sides.
fn panel_text(line: &str) -> &str {
    super::strip_frame(line)
        .trim_end_matches(|character: char| matches!(character, '│' | '┃' | '▎'))
        .trim_end()
}

/// A numbered answer row: "› 1  label" or "  2  label".
fn ask_option(text: &str) -> Option<(u32, String)> {
    let rest = text.strip_prefix('›').unwrap_or(text).trim_start();
    let digits_end = rest.find(|character: char| !character.is_ascii_digit())?;
    if digits_end == 0 {
        return None;
    }
    let number: u32 = rest[..digits_end].parse().ok()?;
    let label = rest[digits_end..].trim_start();
    if label.is_empty() {
        return None;
    }
    Some((number, label.to_string()))
}

fn is_ask_footer(line: &str) -> bool {
    ASK_FOOTER_HINTS.iter().all(|hint| line.contains(hint))
}

/// A row drawn from the panel's own border characters.
fn is_panel_edge(line: &str) -> bool {
    let trimmed = line.trim();
    trimmed.starts_with('╭') || trimmed.starts_with('├') || trimmed.starts_with('╰')
}
