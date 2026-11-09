//! Pulls the last reply out of a terminal screen.
//!
//! The screen is the only source that has a question the agent has drawn but
//! not written yet. Every agent draws its transcript differently, and a newer
//! version of the same agent may change the glyphs, so each extractor works
//! from structure (a marker line plus its indented continuation) rather than
//! from fixed row numbers or a fixed layout.

mod claude;
mod codebuddy;
mod codewhale;
mod codex;
mod deveco;
mod gemini;
mod hermes;
mod kimi;
mod minimax;
mod opencode;
mod pi;
mod zcode;

/// The reply drawn on this screen, when one can be found.
pub(crate) fn extract(agent: &str, text: &str) -> Option<String> {
    let lines = useful_lines(text);
    // A drawn choice is what the reader has to answer, so it wins over the
    // plain reply above it.
    if let Some(choice) = choice_block(&lines) {
        return Some(choice);
    }
    let reply = match agent {
        "claude" => claude::extract(&lines),
        "codebuddy" => codebuddy::extract(&lines),
        "codewhale" => codewhale::extract(&lines),
        "codex" => codex::extract(&lines),
        "deveco" => deveco::extract(&lines),
        "gemini" => gemini::extract(&lines),
        "hermes" => hermes::extract(&lines),
        "kimi" => kimi::extract(&lines),
        "minimax" => minimax::extract(&lines),
        "opencode" => opencode::extract(&lines),
        "pi" => pi::extract(&lines),
        "zcode" => zcode::extract(&lines),
        _ => None,
    };
    reply.filter(|reply| !reply.trim().is_empty())
}

/// A choice drawn on the screen: its question and numbered options.
///
/// The layout is the same across agents: one numbered line per option, the
/// question above the first one, and a footer that invites a choice.
fn choice_block(lines: &[String]) -> Option<String> {
    // The option can sit inside a frame: some agents draw a bar in front of
    // every row of the panel, so the bar is stripped before the number.
    fn strip_frame(line: &str) -> &str {
        line.trim()
            .trim_start_matches(|character: char| matches!(character, '│' | '┃' | '▎'))
            .trim()
    }

    fn numbered(line: &str) -> Option<(u32, String)> {
        let rest = strip_frame(line);
        let rest = rest.strip_prefix('❯').unwrap_or(rest).trim_start();
        let rest = rest.strip_prefix('→').unwrap_or(rest).trim_start();
        // "1. label", "1) label", and "[1] label" are all in use.
        let (number, label) = if let Some(bracketed) = rest.strip_prefix('[') {
            let (number, label) = bracketed.split_once(']')?;
            (number, label)
        } else if let Some((number, label)) = rest.split_once(". ") {
            (number, label)
        } else if let Some((number, label)) = rest.split_once(')') {
            (number, label)
        } else {
            let (number, label) = rest.split_once('.')?;
            (number, label)
        };
        let number: u32 = number.trim().parse().ok()?;
        let label = label.trim();
        if label.is_empty() {
            return None;
        }
        Some((number, label.to_string()))
    }

    let first = lines.iter().position(|line| {
        numbered(line).is_some_and(|(number, _)| number == 1)
    })?;
    // Some agents mark the question with a leading "?", which is more precise
    // than reading upwards: the row above can be a panel title.
    let marked_question = lines[..first]
        .iter()
        .rev()
        .find_map(|line| {
            let text = strip_frame(line);
            text.strip_prefix('?').map(|rest| rest.trim().to_string())
        })
        .filter(|question| !question.is_empty());
    // The question can wrap over several rows. Read upwards until the frame or
    // a blank row ends it, then put the rows back in reading order.
    let mut question_lines: Vec<String> = Vec::new();
    for line in lines[..first].iter().rev() {
        let trimmed = line.trim();
        if trimmed.is_empty() || is_decoration(line) || is_chrome(line) {
            if question_lines.is_empty() {
                continue;
            }
            break;
        }
        if trimmed.starts_with('←') || trimmed.contains("Submit") {
            if question_lines.is_empty() {
                continue;
            }
            break;
        }
        let text = strip_frame(trimmed);
        if text.is_empty() {
            continue;
        }
        question_lines.push(text.to_string());
    }
    question_lines.reverse();
    let question = marked_question.or_else(|| {
        (!question_lines.is_empty()).then(|| question_lines.join("\n"))
    });
    let mut options: Vec<String> = Vec::new();
    for line in lines.iter().skip(first) {
        let trimmed = line.trim();
        if let Some((number, label)) = numbered(line) {
            if label.starts_with("Type something")
                || label.starts_with("Type your own answer")
                || label.starts_with("Chat about this")
                || label == "Other"
            {
                break;
            }
            options.push(format!("{number}. {label}"));
            continue;
        }
        if trimmed.is_empty() {
            continue;
        }
        if trimmed.starts_with("Submit")
            || trimmed.starts_with("Chat about this")
            || is_chrome(line)
            || is_decoration(line)
        {
            break;
        }
        // A description indented under the option it belongs to: keep it, one
        // level in, because it is what tells the reader the options apart.
        if line.starts_with(' ') && !options.is_empty() {
            if let Some(last) = options.last_mut() {
                last.push('\n');
                last.push_str("  ");
                last.push_str(strip_frame(trimmed));
            }
            continue;
        }
        break;
    }
    if options.is_empty() {
        return None;
    }
    let mut parts = Vec::new();
    if let Some(question) = question {
        parts.push(question);
    }
    parts.extend(options);
    Some(parts.join("\n"))
}

/// Screen lines with trailing spaces gone. Blank rows are kept so they can end
/// a block.
pub(super) fn useful_lines(text: &str) -> Vec<String> {
    text.lines()
        .map(|line| line.trim_end().to_string())
        .collect()
}

/// A row that carries no text of its own.
pub(super) fn is_decoration(line: &str) -> bool {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return true;
    }
    trimmed.chars().all(|character| {
        matches!(
            character,
            '─' | '━'
                | '═'
                | '▀'
                | '▄'
                | '█'
                | '╹'
                | '╻'
                | '│'
                | '┃'
                | '|'
                | '-'
                | '='
                | '+'
                | '·'
                | '…'
        )
    })
}

/// A status bar, hint line, or timing line: drawn around the transcript and
/// never part of a reply.
pub(super) fn is_chrome(line: &str) -> bool {
    let lowered = line.to_lowercase();
    const MARKERS: &[&str] = &[
        "esc to interrupt",
        "ctrl+c to",
        "? for shortcuts",
        "shift+tab",
        "shift+enter",
        "auto mode on",
        "for agents",
        "context ",
        "tokens",
        "tok/s",
        " t/s",
        "model set to",
        "type your message",
        "type a follow-up",
        "type a message",
        "enter send",
        "enter to select",
        "to navigate",
        "to cancel",
        "for commands",
        "ctrl+p",
        "ai-generated content",
        "tips for getting started",
        "release notes",
        "update available",
        "press / to use commands",
        "ask · files",
        "work bar",
        "↑↓",
        "↵ choose",
        "esc cancel",
        "build ·",
        "▣ build",
        "✧ ai-generated",
    ];
    MARKERS.iter().any(|marker| lowered.contains(marker))
}

/// True when the marked line is a tool call rather than a reply: its first
/// continuation is the tool's result or a call summary.
fn is_tool_block(lines: &[String], start: usize) -> bool {
    for line in lines.iter().skip(start + 1).take(3) {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let lowered = trimmed.to_lowercase();
        const COMPLETION: &[&str] = &[
            "thought", "completed", "worked", "brewed", "cooked", "baked", "sautéed",
        ];
        if COMPLETION.iter().any(|word| lowered.contains(word)) {
            return false;
        }
        return trimmed.starts_with('└')
            || trimmed.starts_with('⠸')
            || trimmed.starts_with('⎿')
            || trimmed.contains("call (ctrl+o");
    }
    false
}

/// The block that starts at the last line beginning with one of `markers`.
///
/// Tool calls are skipped, and so is a block whose text is page chrome (some
/// agents draw their status line with the same glyph as a reply).
pub(super) fn last_marked_block(
    lines: &[String],
    markers: &[&str],
    stop_markers: &[&str],
) -> Option<String> {
    let mut end = lines.len();
    while end > 0 {
        let start = lines[..end].iter().rposition(|line| {
            let trimmed = line.trim_start();
            markers.iter().any(|marker| trimmed.starts_with(marker))
        })?;
        if is_tool_block(lines, start) {
            end = start;
            continue;
        }
        let block = collect_marked(lines, start, markers, stop_markers);
        if block
            .lines()
            .next()
            .is_some_and(|first| is_chrome(first))
        {
            end = start;
            continue;
        }
        return Some(block);
    }
    None
}

fn collect_marked(
    lines: &[String],
    start: usize,
    markers: &[&str],
    stop_markers: &[&str],
) -> String {
    let first = lines[start].trim_start();
    let Some(marker) = markers.iter().find(|marker| first.starts_with(**marker)) else {
        return String::new();
    };
    let mut body = vec![first[marker.len()..].trim_start().to_string()];
    for line in lines.iter().skip(start + 1) {
        let trimmed = line.trim_start();
        if trimmed.is_empty() || is_decoration(line) || is_chrome(line) {
            break;
        }
        if trimmed.starts_with('└') {
            break;
        }
        if stop_markers.iter().any(|marker| trimmed.starts_with(marker)) {
            break;
        }
        if markers.iter().any(|marker| trimmed.starts_with(marker)) {
            break;
        }
        if line.starts_with(' ') || line.starts_with('\t') {
            body.push(trimmed.to_string());
            continue;
        }
        break;
    }
    body.join("\n")
}

/// The last block of plain text, for agents that draw no reply glyph.
///
/// Blocks are separated by blank rows or chrome rows. A block that is only a
/// prompt line is skipped, so the reply above it is what comes back.
pub(super) fn last_plain_block(lines: &[String]) -> Option<String> {
    fn is_prompt(line: &str) -> bool {
        let trimmed = line.trim_start();
        ["┃", "❯", "›", ">", "▎", "│"]
            .iter()
            .any(|marker| trimmed.starts_with(marker))
    }
    let mut end = lines.len();
    loop {
        while end > 0 {
            let line = &lines[end - 1];
            if line.trim().is_empty() || is_decoration(line) || is_chrome(line) {
                end -= 1;
            } else {
                break;
            }
        }
        if end == 0 {
            return None;
        }
        let mut start = end;
        while start > 0 {
            let line = &lines[start - 1];
            if line.trim().is_empty() || is_decoration(line) || is_chrome(line) {
                break;
            }
            start -= 1;
        }
        if end - start == 1 && is_prompt(&lines[start]) {
            end = start;
            continue;
        }
        let block: Vec<&str> = lines[start..end]
            .iter()
            .map(|line| line.trim())
            .collect();
        return Some(block.join("\n"));
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    fn extract_lines(agent: &str, text: &str) -> Option<String> {
        let lines = useful_lines(text);
        if let Some(choice) = choice_block(&lines) {
            return Some(choice);
        }
        match agent {
            "claude" => claude::extract(&lines),
            "codebuddy" => codebuddy::extract(&lines),
            "codex" => codex::extract(&lines),
            "opencode" => opencode::extract(&lines),
            _ => None,
        }
    }

    #[test]
    fn claude_reply_is_the_last_bullet() {
        let screen = "❯ 只回复两个字：成功\n\n● 主人，成功\n\n✻ Baked for 2s · done 12:39 AM\n";
        assert_eq!(extract_lines("claude", screen).as_deref(), Some("主人，成功"));
    }

    #[test]
    fn claude_skips_a_tool_block() {
        let screen = "● Read herdr/src/main.rs\n  ⎿ 42 lines\n\n● 成功\n\n✻ Baked for 2s\n";
        assert_eq!(extract_lines("claude", screen).as_deref(), Some("成功"));
    }

    #[test]
    fn a_drawn_choice_wins_over_the_reply() {
        let screen = "● 成功\n\n> 用选择题问我\n\n←  ☐ 方案  ✔ Submit  →\n\n选哪个方案\n\n❯ 1. Alpha\n     方案 Alpha\n  2. Beta\n     方案 Beta\n  3. Type something\n  Submit\n\nEnter to select · ↑/↓ to navigate · Esc to cancel\n";
        let reply = extract_lines("codebuddy", screen).expect("a choice");
        assert!(reply.contains("选哪个方案"));
        assert!(reply.contains("1. Alpha"));
        assert!(reply.contains("2. Beta"));
        assert!(reply.contains("  方案 Alpha"));
        assert!(!reply.contains("Type something"));
    }

    #[test]
    fn codex_reply_stops_at_the_timing_line() {
        let screen = "› 只回复两个字：成功\n\n• 成功\n\n  Worked for 2s • 2:40 PM\n";
        assert_eq!(extract_lines("codex", screen).as_deref(), Some("成功"));
    }

    #[test]
    fn opencode_plain_block_skips_the_prompt() {
        let screen = "  ┃\n  ┃  用三行短句分别写 第一行、第二行、第三行\n  ┃\n\n     第一行\n     第二行\n     第三行\n\n     Build · DeepSeek V4.1 Flash · 10.7s\n\n  ┃  Build · footer\n";
        assert_eq!(
            extract_lines("opencode", screen).as_deref(),
            Some("第一行\n第二行\n第三行")
        );
    }

    #[test]
    fn a_wrapped_question_keeps_every_row() {
        let screen = "│ 当前工作区有大量未提交改动（agent-tests 目录整体删除、Cargo.toml / Cargo.lock\n│ 修改等），希望我接下来优先做哪件事？\n\n❯ 1. 审查改动\n     逐个核对删除记录。\n  2. 先编译验证\n     直接跑 debug 构建。\n  3. Type something.\n\nEnter to select · Esc to cancel\n";
        let reply = extract_lines("claude", screen).expect("a choice");
        assert!(reply.starts_with("当前工作区有大量未提交改动"));
        assert!(reply.contains("修改等），希望我接下来优先做哪件事？"));
        assert!(reply.contains("1. 审查改动"));
        assert!(reply.contains("  逐个核对删除记录。"));
    }

    #[test]
    fn a_framed_choice_keeps_its_question_and_descriptions() {
        let screen = "  ┃\n  ┃  接下来想让我做什么？\n  ┃\n  ┃  1. 新建鸿蒙项目\n  ┃     创建一个 HarmonyOS 应用脚手架\n  ┃  2. 写个 ArkTS 示例\n  ┃     写一个页面或组件示例\n  ┃  5. Type your own answer\n  ┃\n  ┃  ↑↓ select  enter submit  esc dismiss\n";
        let reply = extract_lines("deveco", screen).expect("a choice");
        assert!(reply.starts_with("接下来想让我做什么？"), "{reply}");
        assert!(reply.contains("1. 新建鸿蒙项目"));
        assert!(reply.contains("  创建一个 HarmonyOS 应用脚手架"));
        assert!(!reply.contains("Type your own answer"));
        assert!(!reply.contains('┃'));
    }

    #[test]
    fn a_bracketed_choice_uses_its_question_mark_row() {
        let screen = " ● 刚才那道选择题你直接关掉了，我没有替你选。\n\n  question\n   优先级    Submit\n\n  ? HarmonyOS 终端工具这三个方向，先做哪个？\n\n   → [1] 先做多窗口支持\n         优先补多窗口，让用户能同时开多个终端窗口并各自独立运行。\n     [2] 先稳会话恢复\n         把已有的会话恢复打磨到可靠。\n     [3] 先整理配色与字号\n         统一配色与字号。\n\n Enter to select · Esc to cancel\n";
        let reply = extract_lines("kimi", screen).expect("a choice");
        assert!(reply.starts_with("HarmonyOS 终端工具这三个方向，先做哪个？"), "{reply}");
        assert!(reply.contains("1. 先做多窗口支持"));
        assert!(reply.contains("  优先补多窗口"));
        assert!(reply.contains("3. 先整理配色与字号"));
        assert!(!reply.contains("↑↓"));
        assert!(!reply.contains("Other"));
    }

    #[test]
    fn chrome_rows_are_never_a_reply() {
        let screen = " ● ask · files: workspace   Shift+Tab to change   work (Tab)\n DeepSeek · deepseek-flash · thinking: max   context 2%\n";
        assert!(extract_lines("claude", screen).is_none());
    }
}
