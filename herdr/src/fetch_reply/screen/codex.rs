//! Codex: an approval gate draws its question above the numbered answers, and
//! a finished answer is a bullet block that ends at the timing line.

/// How many rows above the answers the question is allowed to start.
///
/// The gate is drawn right above the answers; a limit keeps an unrelated
/// question in the transcript from reaching into the message if Codex ever
/// stops drawing one here.
const QUESTION_SEARCH_ROWS: usize = 12;

/// Rows of the gate that label a value rather than ask the question.
///
/// The reason line ends with a question mark of its own, so it must not win
/// over the row that actually asks what to do.
const VALUE_FIELDS: &[&str] = &["Environment:", "Reason:", "Command:", "Workdir:"];

pub(super) fn extract(lines: &[String]) -> Option<String> {
    super::last_marked_block(lines, &["\u{2022}", "\u{25cf}"], &["Worked for", "\u{203a}"])
}

/// Reads the approval gate: the question row down to the numbered answers.
///
/// The gate is the only place an approval question exists, because the
/// transcript records just the tool call that caused it. The selection cursor
/// tells this list apart from a numbered list inside a reply, but it sits on
/// whichever answer is highlighted, so the block is found by its rows and only
/// needs the cursor somewhere inside it.
pub(super) fn extract_choice(lines: &[String]) -> Option<String> {
    let answers_start = answer_block_start(lines)?;
    let options = super::collect_numbered_options(lines, answers_start);
    // One answer is not a choice; the row is then part of the reply above it.
    if options.len() < 2 {
        return None;
    }
    let question = question_row(lines, answers_start)?;
    let mut body: Vec<String> = Vec::new();
    // Only a question panel has a reply above it; an approval gate prints the
    // tool call it is asking about there instead, and that row is not a reply.
    if progress_row(lines, question).is_some() {
        if let Some(preamble) =
            super::reply_above_panel(lines, question, reply_marker, is_panel_header)
        {
            body.push(preamble);
        }
    }
    let mut question_rows: Vec<String> = Vec::new();
    for line in &lines[question..answers_start] {
        let text = super::strip_frame(line);
        if text.is_empty() || super::is_chrome(line) || is_bullet(text) {
            continue;
        }
        question_rows.push(text.to_string());
    }
    // The reply above the panel often ends by restating the question, and the
    // panel repeats it once more; the reader only needs it once.
    let preamble_restates_question = body.last().is_some_and(|preamble| {
        preamble
            .lines()
            .rev()
            .find(|row| !row.trim().is_empty())
            .is_some_and(|row| {
                question_rows
                    .first()
                    .is_some_and(|question| row.trim_end().ends_with(question.as_str()))
            })
    });
    if !preamble_restates_question {
        body.extend(question_rows);
    }
    body.extend(options);
    Some(body.join("\n"))
}

/// The row the answer list starts on.
///
/// The list is located by the cursor on any of its rows, then walked back to
/// the first answer, so a highlighted second answer does not hide the block.
fn answer_block_start(lines: &[String]) -> Option<usize> {
    let cursor_index = lines.iter().rposition(|line| {
        super::strip_frame(line).starts_with('\u{203a}') && super::numbered(line).is_some()
    })?;
    let mut start = cursor_index;
    while start > 0 && super::numbered(&lines[start - 1]).is_some() {
        start -= 1;
    }
    Some(start)
}

/// The row that asks the question, searched upwards from the answers.
///
/// The search stops at the tool call that caused the gate: anything above it
/// belongs to the earlier conversation.
fn question_row(lines: &[String], answers_start: usize) -> Option<usize> {
    let lowest = answers_start.saturating_sub(QUESTION_SEARCH_ROWS);
    for index in (lowest..answers_start).rev() {
        let line = &lines[index];
        let text = super::strip_frame(line);
        if is_bullet(text) {
            return None;
        }
        if text.is_empty() || super::is_chrome(line) || is_value_field(text) {
            continue;
        }
        if text.ends_with('?') || text.ends_with('\u{ff1f}') {
            return Some(index);
        }
    }
    None
}

/// The glyph a reply row starts with.
const REPLY_MARKERS: [&str; 2] = ["\u{2022}", "\u{25cf}"];

fn reply_marker(line: &str) -> Option<&'static str> {
    REPLY_MARKERS
        .into_iter()
        .find(|marker| line.starts_with(marker))
}

fn is_bullet(text: &str) -> bool {
    reply_marker(text).is_some()
}

/// The row that reports how many questions the panel holds.
///
/// It separates a question panel from its reply, so it is also what tells the
/// two panels apart: an approval gate has no such row.
fn progress_row(lines: &[String], question: usize) -> Option<usize> {
    for index in (0..question).rev() {
        let text = super::strip_frame(&lines[index]);
        if text.is_empty() {
            continue;
        }
        return is_panel_header(text).then_some(index);
    }
    None
}

/// The progress row a question panel prints above its question.
fn is_panel_header(text: &str) -> bool {
    text.starts_with("Question ") && text.contains('/')
}

fn is_value_field(text: &str) -> bool {
    VALUE_FIELDS.iter().any(|field| text.starts_with(field))
}
