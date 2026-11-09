//! Qwen: the choice panel starts with a "? AskUserQuestion" header, then the
//! question as a title, a blank row, and a wrapped paragraph.
//!
//! The generic reader stops at the first blank row above the options, which
//! would drop the title, so Qwen reads everything down from its own header.

/// A reply starts with a filled diamond; its continuation is indented and a
/// tool result opens with a corner mark.
pub(super) fn extract(lines: &[String]) -> Option<String> {
    let start = lines
        .iter()
        .rposition(|line| line.trim_start().starts_with('◆'))?;
    let first = lines[start].trim_start().trim_start_matches('◆').trim();
    let mut body = vec![first.to_string()];
    for line in lines.iter().skip(start + 1) {
        let trimmed = line.trim_start();
        if trimmed.is_empty() || super::is_decoration(line) || super::is_chrome(line) {
            break;
        }
        // A tool result or a panel bar ends the reply.
        if trimmed.starts_with('⎿') || trimmed.starts_with('│') || trimmed.starts_with('┃') {
            break;
        }
        if line.starts_with(' ') || line.starts_with('\t') {
            body.push(trimmed.to_string());
            continue;
        }
        break;
    }
    let reply = body.join("\n");
    (!reply.trim().is_empty()).then_some(reply)
}

pub(super) fn extract_choice(lines: &[String]) -> Option<String> {
    // The newest panel wins: a pane keeps the previous question on screen, so
    // reading from the first header would answer the question already handled.
    let panel = lines.iter().rposition(|line| {
        let text = line.trim();
        text.starts_with('?') && text.contains("AskUserQuestion")
    })?;
    let first = lines
        .iter()
        .enumerate()
        .skip(panel + 1)
        .find_map(|(index, line)| {
            super::numbered(line)
                .is_some_and(|(number, _)| number == 1)
                .then_some(index)
        })?;

    // Everything between the panel header and the first option is the
    // question, the title row included.
    let question_lines: Vec<String> = lines[panel + 1..first]
        .iter()
        .filter(|line| !line.trim().is_empty())
        .filter(|line| !super::is_chrome(line) && !super::is_decoration(line))
        .map(|line| line.trim().to_string())
        .collect();

    let options = super::collect_numbered_options(lines, first);
    if options.is_empty() {
        return None;
    }
    let mut parts = Vec::new();
    if !question_lines.is_empty() {
        parts.push(question_lines.join("\n"));
    }
    parts.extend(options);
    Some(parts.join("\n"))
}
