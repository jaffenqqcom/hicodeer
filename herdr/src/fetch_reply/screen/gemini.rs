//! Gemini: a reply starts with a four-pointed star, and an "Answer Questions"
//! panel draws its question above the answers.

/// The cursor Gemini draws in front of the highlighted answer.
const SELECTION_CURSORS: [char; 2] = ['\u{25cf}', '\u{25cb}'];

/// The markers drawn above and below a scrollable answer list.
const SCROLL_MARKERS: [char; 2] = ['\u{25b2}', '\u{25bc}'];

pub(super) fn extract(lines: &[String]) -> Option<String> {
    super::last_marked_block(lines, &["\u{2726}"], &[">", "\u{2580}", "\u{2584}"])
}

/// Reads the question panel: the question row down to the last numbered answer.
///
/// The panel is the only place the question exists, because the transcript
/// records just the tool call that caused it. Gemini marks the highlighted
/// answer with a filled circle rather than the cursor the shared reader knows,
/// indents each answer's explanation underneath it, and closes every row with
/// the frame bar.
pub(super) fn extract_choice(lines: &[String]) -> Option<String> {
    let first_answer = lines
        .iter()
        .rposition(|line| choice_row(line).is_some_and(|(number, _)| number == 1))?;
    let answers = answer_rows(lines, first_answer);
    // One answer is not a choice; the row is then part of the reply above it.
    if answers.len() < 2 {
        return None;
    }
    // The frame's top row is the panel's own edge: without it the rows above
    // cannot be told apart from the panel, and an earlier turn's reply would be
    // pushed in its place.
    let panel_top = panel_top(lines, first_answer)?;
    let question = question_block(lines, first_answer)?;
    let mut body: Vec<String> = Vec::new();
    if let Some(preamble) = super::reply_above_panel(lines, panel_top, reply_marker, |_| false) {
        body.push(preamble);
    }
    body.push(question);
    body.extend(answers);
    Some(body.join("\n"))
}

/// The row the panel's frame starts on.
///
/// The reply that introduces the panel sits above the frame, so the frame is
/// where the walk towards that reply starts.
fn panel_top(lines: &[String], first_answer: usize) -> Option<usize> {
    lines[..first_answer]
        .iter()
        .rposition(|line| line.trim_start().starts_with('\u{256d}'))
}

/// The glyph a reply row starts with.
fn reply_marker(line: &str) -> Option<&'static str> {
    line.starts_with('\u{2726}').then_some("\u{2726}")
}

/// A row of the panel without its frame, on both sides.
fn panel_text(line: &str) -> &str {
    super::strip_frame(line)
        .trim_end_matches(|character: char| matches!(character, '\u{2502}' | '\u{2503}' | '\u{258e}'))
        .trim_end()
}

/// A numbered answer row, with the selection cursor stripped.
fn choice_row(line: &str) -> Option<(u32, String)> {
    let text = panel_text(line);
    let text = SELECTION_CURSORS
        .iter()
        .find_map(|cursor| text.strip_prefix(*cursor))
        .unwrap_or(text)
        .trim_start();
    super::numbered(text)
}

/// The numbered answers, each followed by the explanation indented under it.
fn answer_rows(lines: &[String], first_answer: usize) -> Vec<String> {
    let mut answers: Vec<String> = Vec::new();
    for line in lines.iter().skip(first_answer) {
        let text = panel_text(line);
        if is_scroll_marker(text) {
            continue;
        }
        if let Some((number, label)) = choice_row(line) {
            answers.push(format!("{number}. {label}"));
            continue;
        }
        if text.is_empty() {
            continue;
        }
        if super::is_chrome(line) || super::is_decoration(text) || is_frame_row(text) {
            break;
        }
        if let Some(last) = answers.last_mut() {
            last.push('\n');
            last.push_str("  ");
            last.push_str(text);
        }
    }
    answers
}

/// The question rows directly above the answers, as one block.
///
/// The panel separates the question from its title with a blank row, so the
/// block ends there and the title stays out of the message.
fn question_block(lines: &[String], answers_start: usize) -> Option<String> {
    let mut rows: Vec<String> = Vec::new();
    for index in (0..answers_start).rev() {
        let line = &lines[index];
        let text = panel_text(line);
        if text.is_empty() || is_scroll_marker(text) {
            if rows.is_empty() {
                continue;
            }
            break;
        }
        if super::is_chrome(line) || super::is_decoration(text) || is_frame_row(text) {
            break;
        }
        rows.push(text.to_string());
    }
    rows.reverse();
    (!rows.is_empty()).then(|| rows.join("\n"))
}

/// A row drawn only of border characters: the panel's own frame.
///
/// The frame has to end both scans, otherwise a panel scrolled up so far that
/// its question is off screen would read the border (or the transcript above
/// it) as the question.
fn is_frame_row(text: &str) -> bool {
    let trimmed = text.trim();
    !trimmed.is_empty()
        && trimmed
            .chars()
            .all(|character| ('\u{2500}'..='\u{257f}').contains(&character))
}

fn is_scroll_marker(text: &str) -> bool {
    let trimmed = text.trim();
    SCROLL_MARKERS.iter().any(|marker| trimmed.starts_with(*marker))
}
