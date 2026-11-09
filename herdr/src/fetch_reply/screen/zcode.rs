//! ZCode: no glyph in front of a reply; the prompt is a `›` line.
//!
//! A choice is drawn without numbers: the selected row starts with an arrow and
//! each row puts its description in a second column. The panel header and the
//! first option are also previewed above a `Filter: type to search` row, so the
//! rows above that marker are not the question.

/// The choice drawn on this screen, when the ZCode layout is present.
pub(super) fn extract_choice(lines: &[String]) -> Option<String> {
    let marker = lines
        .iter()
        .rposition(|line| line.to_lowercase().contains("filter: type to search"))?;

    let mut options: Vec<String> = Vec::new();
    for line in lines.iter().skip(marker + 1) {
        if line.trim().is_empty() {
            continue;
        }
        if super::is_decoration(line) || super::is_chrome(line) {
            break;
        }
        let text = line.trim_start().trim_start_matches('→').trim_start();
        let (label, description) = split_columns(text);
        if label.is_empty() {
            break;
        }
        if label.to_lowercase().starts_with("other") {
            break;
        }
        let mut option = label.to_string();
        if let Some(description) = description {
            option.push('\n');
            option.push_str("  ");
            option.push_str(description);
        }
        options.push(option);
    }
    if options.len() < 2 {
        return None;
    }

    // Groups above the marker, nearest first: the preview, then the question.
    let mut groups: Vec<Vec<String>> = Vec::new();
    let mut current: Vec<String> = Vec::new();
    for line in lines[..marker].iter().rev() {
        let trimmed = line.trim();
        let panel = is_panel_row(trimmed);
        if trimmed.is_empty() || super::is_decoration(line) || panel {
            if !current.is_empty() {
                current.reverse();
                groups.push(current);
                current = Vec::new();
            }
            if panel && !groups.is_empty() {
                break;
            }
            continue;
        }
        current.push(trimmed.to_string());
    }
    if !current.is_empty() {
        current.reverse();
        groups.push(current);
    }

    let preview_label = options
        .first()
        .and_then(|option| option.lines().next())
        .map(str::to_string);
    let question = groups
        .into_iter()
        .find(|group| group.first() != preview_label.as_ref())
        .map(|group| group.join("\n"));

    let mut parts = Vec::new();
    if let Some(question) = question.filter(|question| !question.trim().is_empty()) {
        parts.push(question);
    }
    parts.extend(options);
    Some(parts.join("\n"))
}

/// The panel rows that sit above the question and are never part of it.
fn is_panel_row(line: &str) -> bool {
    let lowered = line.to_lowercase();
    lowered.starts_with("awaiting")
        || lowered.starts_with("activity")
        || line.starts_with('◆')
        || line.starts_with('○')
        || lowered.contains("waiting for permission")
}

/// Splits a row into its label column and its description column.
///
/// Two or more spaces separate them; a single space stays inside the label.
fn split_columns(text: &str) -> (&str, Option<&str>) {
    let mut spaces = 0usize;
    for (position, character) in text.char_indices() {
        if character == ' ' {
            spaces += 1;
            continue;
        }
        if spaces >= 2 {
            let label = text[..position - spaces].trim_end();
            let description = text[position..].trim();
            if !label.is_empty() && !description.is_empty() {
                return (label, Some(description));
            }
        }
        spaces = 0;
    }
    (text.trim(), None)
}

/// A reply is the last block of plain text.
pub(super) fn extract(lines: &[String]) -> Option<String> {
    extract_choice(lines).or_else(|| super::last_plain_block(lines))
}

#[cfg(test)]
mod tests {
    use super::super::useful_lines;
    use super::extract_choice;

    #[test]
    fn a_filtered_choice_keeps_the_title_and_question() {
        let screen = " ◆ Question 1 question · waiting for permission\n  Awaiting 1 answer\n\n Activity · 1 active tool\n  ○ AskUserQuestion\n\n超能力 · 1/1\n如果你现在可以免费获得任意一种超能力，你会选择哪一种？\n\n瞬间移动\n想去哪里就去哪里，通勤和排队从此与你无关。\n\nFilter: type to search\n\n→ 瞬间移动                        想去哪里就去哪里，通勤和排队从此与你无关。\n  时间暂停                        按下暂停键，整个世界都等你。\n  读心术                          别人嘴上说的和心里想的，你一次全都听得到。\n  Other...                        Enter a different answer\n";
        let lines = useful_lines(screen);
        let reply = extract_choice(&lines).expect("a choice");
        assert!(reply.starts_with("超能力 · 1/1"), "{reply}");
        assert!(reply.contains("如果你现在可以免费获得任意一种超能力"), "{reply}");
        assert!(reply.contains("瞬间移动"));
        assert!(reply.contains("想去哪里就去哪里"));
        assert!(reply.contains("  按下暂停键"));
        assert!(!reply.contains("Other"));
    }
}
