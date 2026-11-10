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
mod qwen;
mod zcode;

/// The reply drawn on this screen, when one can be found.
pub(crate) fn extract(agent: &str, text: &str) -> Option<String> {
    let lines = useful_lines(text);
    // A drawn choice is what the reader has to answer, so it wins over the
    // plain reply above it. Agents that draw their own choice layout read it
    // first: their question can be split in ways the generic reader would cut.
    let own_choice = match agent {
        "codex" => codex::extract_choice(&lines),
        "gemini" => gemini::extract_choice(&lines),
        "kimi" => kimi::extract_choice(&lines),
        "minimax" => minimax::extract_choice(&lines),
        "qwen" => qwen::extract_choice(&lines),
        "zcode" => zcode::extract_choice(&lines),
        _ => None,
    };
    if let Some(choice) = own_choice {
        return Some(choice);
    }
    // MiniMax numbers rows inside an ordinary reply too, and its panel is the
    // only choice it draws, so the generic reader would take a numbered list in
    // a reply for a choice and drop the reply's opening rows.
    if agent != "minimax" {
        if let Some(choice) = choice_block(&lines) {
            return Some(choice);
        }
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
        "qwen" => qwen::extract(&lines),
        "zcode" => zcode::extract(&lines),
        _ => None,
    };
    reply.filter(|reply| !reply.trim().is_empty())
}

/// Strips the frame a row may be drawn inside: some agents put a bar in front
/// of every row of the panel.
pub(super) fn strip_frame(line: &str) -> &str {
    line.trim()
        .trim_start_matches(|character: char| matches!(character, '│' | '┃' | '▎'))
        .trim()
}

/// Parses a numbered option row: "1. label", "1) label", and "[1] label" are
/// all in use.
///
/// The selection cursor some agents draw in front of the highlighted row is
/// stripped first: "❯" is the one the shared readers were built for, and Codex
/// uses "\u{203a}" for the same thing.
pub(super) fn numbered(line: &str) -> Option<(u32, String)> {
    let rest = strip_frame(line);
    let rest = rest.strip_prefix('❯').unwrap_or(rest).trim_start();
    let rest = rest.strip_prefix('\u{203a}').unwrap_or(rest).trim_start();
    let rest = rest.strip_prefix('→').unwrap_or(rest).trim_start();
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

/// The numbered options from the first option on, each with its indented
/// description.
///
/// The placeholder rows ("Type something", "Other", "Chat about this") and the
/// footer end the list.
pub(super) fn collect_numbered_options(lines: &[String], first: usize) -> Vec<String> {
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
    options
}

/// A choice drawn on the screen: its question and numbered options.
///
/// The layout is the same across agents: one numbered line per option, the
/// question above the first one, and a footer that invites a choice.
fn choice_block(lines: &[String]) -> Option<String> {
    // The newest choice wins: an answered question stays on screen above the
    // one being asked now, and its numbering would otherwise be read first.
    let first = lines
        .iter()
        .rposition(|line| numbered(line).is_some_and(|(number, _)| number == 1))?;
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
    // Some agents mark the question row with a leading "?". It is stripped
    // here rather than searched for first: a panel header above a blank row
    // also starts with "?" and would otherwise win.
    let question = (!question_lines.is_empty()).then(|| {
        let mut joined = question_lines.join("\n");
        if let Some(rest) = joined.strip_prefix('?') {
            joined = rest.trim_start().to_string();
        }
        joined
    });
    let options = collect_numbered_options(lines, first);
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

/// How many blank rows may separate a panel from the reply above it.
///
/// The introduction sits right above the panel; a larger gap means the rows
/// belong to an earlier turn rather than to this question.
const MAX_PANEL_GAP_ROWS: usize = 4;

/// The reply block an agent draws directly above its own choice panel.
///
/// Agents introduce a choice with a reply of their own, and that reply is part
/// of what the reader sees. Only blank rows, panel furniture, and the rows the
/// panel predicate names may separate the block from the panel; anything else
/// means the rows above belong to an earlier turn.
pub(super) fn reply_above_panel(
    lines: &[String],
    panel_top: usize,
    marker: fn(&str) -> Option<&'static str>,
    is_panel_row: fn(&str) -> bool,
) -> Option<String> {
    let above = lines.get(..panel_top)?;
    let mut blank_rows = 0;
    let mut cursor = above.len();
    while cursor > 0 {
        cursor -= 1;
        let line = &above[cursor];
        let trimmed = line.trim_start();
        if trimmed.is_empty() {
            blank_rows += 1;
            if blank_rows > MAX_PANEL_GAP_ROWS {
                return None;
            }
            continue;
        }
        blank_rows = 0;
        if marker(trimmed).is_some() {
            return collect_marked_block(above, cursor, marker, is_panel_row);
        }
        if is_panel_row(trimmed) || is_decoration(line) || is_chrome(line) {
            continue;
        }
        // A continuation row of the block; the walk keeps climbing past it.
        if line.starts_with(' ') || line.starts_with('\t') {
            continue;
        }
        return None;
    }
    None
}

/// The rows of a marked block: its marker row and every continuation row
/// indented under it.
///
/// A blank row inside the block is a paragraph break, not its end, because the
/// continuation rows are indented: only a row that is neither blank nor indented
/// can be outside the block.
pub(super) fn collect_marked_block(
    lines: &[String],
    start: usize,
    marker: fn(&str) -> Option<&'static str>,
    is_panel_row: fn(&str) -> bool,
) -> Option<String> {
    let first = lines.get(start)?.trim_start();
    let prefix = marker(first)?;
    let mut body: Vec<String> = vec![first[prefix.len()..].trim_start().to_string()];
    for line in lines.iter().skip(start + 1) {
        let trimmed = line.trim_start();
        if trimmed.is_empty() {
            body.push(String::new());
            continue;
        }
        if is_chrome(line) || is_decoration(line) || is_panel_row(trimmed) {
            break;
        }
        if trimmed.starts_with('\u{2514}') || trimmed.starts_with('\u{203a}') {
            break;
        }
        if marker(trimmed).is_some() {
            break;
        }
        if line.starts_with(' ') || line.starts_with('\t') {
            body.push(trimmed.to_string());
            continue;
        }
        break;
    }
    while body.last().is_some_and(|line| line.trim().is_empty()) {
        body.pop();
    }
    let text = body.join("\n");
    (!text.trim().is_empty()).then_some(text)
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

    #[test]
    fn a_codex_panel_keeps_the_reply_above_its_progress_row() {
        // Captured from codex while it waited on a question: the reply above
        // the progress row belongs in the message, and the progress row does
        // not. The panel repeats the question, so it is printed once.
        let screen = "› 给我出一个选择题，随便什么都可以，用键盘上下键做选择。出提前随便和我说点什么\n\n\n• 又来了，这次随口说一句：我刚刚把终端里的光标来回挪了几次，突然觉得方向键是键盘上最诚实的键——只负责方向，不掺\n  和立场。\n\n  题目：穿越到古代，只能带一样现代物品，你带哪个？\n\n\n  Question 1/1 (1 unanswered)\n  穿越到古代，只能带一样现代物品，你带哪个？\n\n  › 1. 打火机 (Recommended)  生火、照明、取暖，性价比之王。\n    2. 一箱压缩饼干          管饱，但你很快会怀念榨菜和辣条。\n    3. 没信号的手机          没信号，但能当手电筒和镇纸。\n    4. None of the above     Optionally, add details in notes (tab)\n\n  tab to add notes | enter to submit answer | esc to interrupt\n";
        let expected = "又来了，这次随口说一句：我刚刚把终端里的光标来回挪了几次，突然觉得方向键是键盘上最诚实的键——只负责方向，不掺\n和立场。\n\n题目：穿越到古代，只能带一样现代物品，你带哪个？\n1. 打火机 (Recommended)  生火、照明、取暖，性价比之王。\n2. 一箱压缩饼干          管饱，但你很快会怀念榨菜和辣条。\n3. 没信号的手机          没信号，但能当手电筒和镇纸。\n4. None of the above     Optionally, add details in notes (tab)";
        assert_eq!(extract("codex", screen).as_deref(), Some(expected));
    }

    #[test]
    fn a_gemini_panel_keeps_the_reply_above_its_frame() {
        // Captured from gemini while it waited on a question: the reply sits
        // above the panel's own frame.
        let screen = "✦ 随口说一句：crates/ 里那个 fuzzy_nucleo 挺有意思，说明仓库里的模糊匹配还留了两套实现（fuzzy 和基于 nucleo\n  的），迁移大概没做完。这种\"两套并存\"的地方，通常埋着最容易被忽略的 bug。\n\n  题目换了，用上下键选：\n\n╭────────────────────────────────────────────────────────────╮\n│ Answer Questions                                           │\n│                                                            │\n│ 一段 Rust 代码里同时存在一个可变引用和一个不可变引用指向同一数据，编译器的反应是？                         │\n│                                                            │\n│ ▲                                                          │\n│ ● 1.  直接编译报错，不允许共存                             │\n│       借用检查器在编译期就拒绝，这是 Rust 最核心的安全保证之一                                             │\n│   2.  允许，但运行时可能 panic                             │\n│       那是 RefCell / 运行时借用检查的领域                  │\n│   3.  允许，靠 GC 自动协调                                 │\n│       Rust 没有 GC                                         │\n│ ▼                                                          │\n│                                                            │\n│ Enter to select · ↑/↓ to navigate · Esc to cancel          │\n╰────────────────────────────────────────────────────────────╯\n";
        let expected = "随口说一句：crates/ 里那个 fuzzy_nucleo 挺有意思，说明仓库里的模糊匹配还留了两套实现（fuzzy 和基于 nucleo\n的），迁移大概没做完。这种\"两套并存\"的地方，通常埋着最容易被忽略的 bug。\n\n题目换了，用上下键选：\n一段 Rust 代码里同时存在一个可变引用和一个不可变引用指向同一数据，编译器的反应是？\n1. 直接编译报错，不允许共存\n  借用检查器在编译期就拒绝，这是 Rust 最核心的安全保证之一\n2. 允许，但运行时可能 panic\n  那是 RefCell / 运行时借用检查的领域\n3. 允许，靠 GC 自动协调\n  Rust 没有 GC";
        assert_eq!(extract("gemini", screen).as_deref(), Some(expected));
    }

    #[test]
    fn a_kimi_panel_keeps_the_reply_above_its_rule() {
        // Captured from kimi while it waited on a question: the reply sits
        // above the rule that frames the top of the panel.
        let screen = " ● 说点无关的：章鱼有三颗心脏，但游泳时主心脏会停跳——所以对章鱼来说，游泳约等于憋着气冲刺，它们平时更愿意爬。\n   挺有画面感的。\n\n   下面这题，用 ↑ ↓ 选，回车确认：\n ────────────────────────────────────────────────────────────────────────────────────────────\n  question\n\n   超能力    Submit\n\n  ? 如果只能额外获得一种「超能力」，你选哪个？\n\n   → [1] 不用睡觉\n         每天多出 8 小时，但朋友都睡了，可能更孤独\n     [2] 过目不忘\n         看过的书、见过的人全记住，代价是忘不掉尴尬事\n     [3] 瞬间移动\n         通勤时间归零，随时回家吃饭，但可能被当成灵异事件\n     [4] 不用吃饭\n         省下饭钱和时间，但会失去很多人间乐趣\n     [5] Other\n\n   ↑↓ select  1-5 / ↵ choose  ←/→/tab switch  esc cancel\n ────────────────────────────────────────────────────────────────────────────────────────────\n DeepSeek V4.1 Flash 1017 thinking: high  ~/workspace/hicodeer  main [+1973 -113 ↑1]           ctrl+c: cancel\n";
        let expected = "说点无关的：章鱼有三颗心脏，但游泳时主心脏会停跳——所以对章鱼来说，游泳约等于憋着气冲刺，它们平时更愿意爬。\n挺有画面感的。\n\n下面这题，用 ↑ ↓ 选，回车确认：\n如果只能额外获得一种「超能力」，你选哪个？\n1. 不用睡觉\n  每天多出 8 小时，但朋友都睡了，可能更孤独\n2. 过目不忘\n  看过的书、见过的人全记住，代价是忘不掉尴尬事\n3. 瞬间移动\n  通勤时间归零，随时回家吃饭，但可能被当成灵异事件\n4. 不用吃饭\n  省下饭钱和时间，但会失去很多人间乐趣";
        assert_eq!(extract("kimi", screen).as_deref(), Some(expected));
    }

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
    fn codex_choice_reads_the_approval_panel() {
        // Captured from codex 0.164 while it waited on a write inside a
        // read-only sandbox: the question and the answers exist only on screen.
        let screen = "› 用 bash 在这个目录写一个 a.txt，内容 ok，然后只回复两个字：成功\n\n\n• Running printf 'ok\\n' > a.txt\n\n\n  Would you like to run the following command?\n\n  Environment: local\n\n  Reason: 需要在当前目录创建 a.txt 并写入内容 ok，是否允许写入？\n\n  $ printf 'ok\\n' > a.txt\n\n\n› 1. Yes, proceed (y)\n  2. Yes, and don't ask again for commands that start with `printf 'ok\\n' > a.txt` (p)\n  3. No, and tell Codex what to do differently (esc)\n\n  Press enter to confirm or esc to cancel\n";
        let expected = "Would you like to run the following command?\nEnvironment: local\nReason: 需要在当前目录创建 a.txt 并写入内容 ok，是否允许写入？\n$ printf 'ok\\n' > a.txt\n1. Yes, proceed (y)\n2. Yes, and don't ask again for commands that start with `printf 'ok\\n' > a.txt` (p)\n3. No, and tell Codex what to do differently (esc)";
        assert_eq!(extract("codex", screen).as_deref(), Some(expected));
    }

    #[test]
    fn codex_choice_keeps_reading_when_the_cursor_moved() {
        // The highlight sits on the second answer once the user presses down,
        // and the gate is still the same gate.
        let screen = "› 写个文件\n\n• Running printf 'ok\\n' > a.txt\n\n  Would you like to run the following command?\n\n  Environment: local\n\n  Reason: 写文件\n\n  $ printf 'ok\\n' > a.txt\n\n  1. Yes, proceed (y)\n› 2. Yes, and don't ask again (p)\n  3. No, and tell Codex what to do differently (esc)\n\n  Press enter to confirm or esc to cancel\n";
        let reply = extract("codex", screen).expect("the gate");
        assert!(reply.starts_with("Would you like to run the following command?"));
        assert!(reply.contains("1. Yes, proceed (y)"));
        assert!(reply.contains("3. No, and tell Codex what to do differently (esc)"));
    }

    #[test]
    fn codex_gate_with_one_answer_is_not_a_choice() {
        let screen = "  Would you like to continue?\n\n› 1. Yes, proceed (y)\n";
        assert!(codex::extract_choice(&useful_lines(screen)).is_none());
    }

    #[test]
    fn codex_numbered_reply_is_not_a_gate() {
        // A numbered list in an answer carries no cursor, so it stays a reply.
        let screen = "› 列一下步骤\n\n• 步骤如下\n\n  1. 第一步\n  2. 第二步\n\n  Worked for 2s • 1:00 PM\n";
        assert!(codex::extract_choice(&useful_lines(screen)).is_none());
    }

    #[test]
    fn a_real_gemini_panel_keeps_its_question_and_explanations() {
        // Captured from gemini while it waited on a multiple-choice question:
        // the panel is framed, the highlight is a filled circle, and every
        // answer carries an indented explanation.
        let screen = "╭────────────────────────────────────────────────────────────╮\n│ Answer Questions                                           │\n│                                                            │\n│ 下列哪一行执行后，变量 s1 仍然可以继续使用？               │\n│                                                            │\n│ ▲                                                          │\n│ ● 1.  A. let s2 = s1;                                      │\n│       会移动 s1，之后再使用 s1 会报错                      │\n│   2.  B. let s2 = s1.clone();                              │\n│       克隆堆数据，s1 仍可用                                │\n│   3.  C. let s2 = &s1;                                     │\n│       借用，s1 仍可用                                      │\n│ ▼                                                          │\n│                                                            │\n│ Enter to select · ↑/↓ to navigate · Esc to cancel          │\n╰────────────────────────────────────────────────────────────╯\n";
        let expected = "下列哪一行执行后，变量 s1 仍然可以继续使用？\n1. A. let s2 = s1;\n  会移动 s1，之后再使用 s1 会报错\n2. B. let s2 = s1.clone();\n  克隆堆数据，s1 仍可用\n3. C. let s2 = &s1;\n  借用，s1 仍可用";
        assert_eq!(extract("gemini", screen).as_deref(), Some(expected));
    }

    #[test]
    fn a_scrolled_kimi_panel_is_not_read_from_the_reply_above() {
        // The rule that frames the top of the panel scrolled away, so the rows
        // above it cannot be claimed as this panel's introduction.
        let screen = " ● 上一轮的回答\n\n  question\n\n   超能力    Submit\n\n  ? 如果只能额外获得一种「超能力」，你选哪个？\n\n   → [1] 不用睡觉\n     [2] 过目不忘\n\n   ↑↓ select  1-5 / ↵ choose  ←/→/tab switch  esc cancel\n";
        assert!(kimi::extract_choice(&useful_lines(screen)).is_none());
    }

    #[test]
    fn a_scrolled_gemini_panel_is_not_read_from_the_reply_above() {
        // The panel scrolled so far that its top border is gone: without that
        // border the rows above cannot be told from the panel, so nothing here
        // may be pushed as a choice.
        let screen = "✦ 上一轮的回答\n\n│ ▲\n│ ● 1.  A. O(log n)\n│   2.  B. O(n)\n│ ▼\n│ Enter to select · ↑/↓ to navigate · Esc to cancel\n╰────────────────────────────╯\n";
        assert!(gemini::extract_choice(&useful_lines(screen)).is_none());
    }

    #[test]
    fn a_scrolled_gemini_panel_does_not_take_its_border_as_the_question() {
        // The question sits above the visible area, so the frame has to end the
        // search; otherwise the previous answer becomes the question.
        let screen = "│ 上一轮的回答                                        │\n╭────────────────────────────────────────────────────╮\n│ ▲                                                  │\n│ ● 1.  A. O(log n)                                  │\n│   2.  B. O(n)                                      │\n│ ▼                                                  │\n│ Enter to select · ↑/↓ to navigate · Esc to cancel  │\n╰────────────────────────────────────────────────────╯\n";
        assert!(gemini::extract_choice(&useful_lines(screen)).is_none());
    }

    #[test]
    fn a_real_minimax_ask_panel_keeps_its_answers() {
        // Captured from mcode while its Ask panel waited on a choice: the
        // answers are numbered with spaces and no punctuation, which the shared
        // option reader does not accept.
        let screen = " ╭─ Ask ───────────────────────────────────────────────────────────────────────────────────────── 1 of 1 ─╮\n  │ 任务类型                                                                                               │\n  │ 接下来你希望我在 HiCodeer 项目里做哪类工作？（上下键选择，回车确认）                                   │\n  │                                                                                                        │\n  │ › 1  排查一个问题（加日志 / 定位 bug）                                                                 │\n  │   2  梳理代码架构（读代码、画调用链）                                                                  │\n  │   3  审查本地未提交改动                                                                                │\n  │   4  实现或修改一个功能                                                                                │\n  │   5  Other…                                                                                            │\n  ├────────────────────────────────────────────────────────────────────────────────────────────────────────┤\n  │ ↑↓ move · 1-5 select · Enter send · Esc cancel                                                         │\n  ╰────────────────────────────────────────────────────────────────────────────────────────────────────────╯\n";
        let expected = "任务类型\n接下来你希望我在 HiCodeer 项目里做哪类工作？（上下键选择，回车确认）\n1. 排查一个问题（加日志 / 定位 bug）\n2. 梳理代码架构（读代码、画调用链）\n3. 审查本地未提交改动\n4. 实现或修改一个功能\n5. Other…";
        assert_eq!(extract("minimax", screen).as_deref(), Some(expected));
    }

    #[test]
    fn a_minimax_numbered_reply_without_the_panel_is_not_a_choice() {
        // The same numbering appears in an answer, but only the panel prints
        // the hint row that names the keys.
        let screen = "● 步骤如下\n\n  1  第一步\n  2  第二步\n\n  └ Completed in 2s\n";
        assert!(minimax::extract_choice(&useful_lines(screen)).is_none());
    }

    #[test]
    fn a_minimax_reply_keeps_its_paragraphs_across_blank_rows() {
        // Captured from mcode: the greeting, the list of example asks and the
        // closing row are one reply, and the blank rows between them are
        // paragraph breaks rather than the end of it.
        let screen = "● 你好！我是 MiniMax Code，当前工作目录是 /storage/Users/currentUser/workspace/hicodeer（HiCodeer —— Zed\n    向 HarmonyOS NEXT 的移植项目）。\n\n    需要我做点什么？比如：\n\n    - 阅读/理解某段代码，定位某个模块\n    - 排查 bug、调整实现、补测试\n    - 统计 OHOS 移植点、评估升级冲突面\n    - 代码审查（改动、分支、PR）\n    - 跑构建 / clippy / 测试\n\n    直接说需求就行，中文英文都可以。\n\n    ›  Ask Mcode to do anything\n";
        let expected = "你好！我是 MiniMax Code，当前工作目录是 /storage/Users/currentUser/workspace/hicodeer（HiCodeer —— Zed\n向 HarmonyOS NEXT 的移植项目）。\n\n需要我做点什么？比如：\n\n- 阅读/理解某段代码，定位某个模块\n- 排查 bug、调整实现、补测试\n- 统计 OHOS 移植点、评估升级冲突面\n- 代码审查（改动、分支、PR）\n- 跑构建 / clippy / 测试\n\n直接说需求就行，中文英文都可以。";
        assert_eq!(extract("minimax", screen).as_deref(), Some(expected));
    }

    #[test]
    fn a_minimax_numbered_list_inside_a_reply_is_not_a_choice() {
        // Captured from mcode: the numbered rows are part of the answer, and
        // reading them as a choice would drop the greeting above them.
        let screen = "  ● 好，题目来了 —— 用 ↑/↓ 移动，回车确认。\n\n  ✓ Question dismissed\n\n\n   › 你好\n\n\n  └ • Thought for 2.5s\n\n  ● 你好👋 选择题还挂在那儿等你选，用 ↑/↓ 挑一项再按回车就行。\n\n    如果那个选择面板没显示出来或者按不动，直接打字回我序号也可以：\n\n    1. 先摸清仓库，讲清结构和启动流程\n    2. 修一个具体 bug / 加功能\n    3. 统计 OHOS 移植改动面\n    4. 只是试试键盘选择\n\n    选一个数字，或者直接说你想干嘛。\n\n    └ Completed in 3s · ⚡ 138 tok/s\n\n    Message · Enter send · Shift+Enter newline        Tip: /context shows the current Session context budget\n    ────────────────────────────────────────────────────────────────────────────────────────────────────────\n  ›  Ask Mcode to do anything\n    ────────────────────────────────────────────────────────────────────────────────────────────────────────\n\n  ~/workspace/hicodeer │ ◇ 打招呼 │ Auto │ ✦ deepseek-flash · Thinking On │ Context 82% left\n";
        let expected = "你好👋 选择题还挂在那儿等你选，用 ↑/↓ 挑一项再按回车就行。\n\n如果那个选择面板没显示出来或者按不动，直接打字回我序号也可以：\n\n1. 先摸清仓库，讲清结构和启动流程\n2. 修一个具体 bug / 加功能\n3. 统计 OHOS 移植改动面\n4. 只是试试键盘选择\n\n选一个数字，或者直接说你想干嘛。";
        assert_eq!(extract("minimax", screen).as_deref(), Some(expected));
    }

    #[test]
    fn a_minimax_ask_panel_keeps_the_row_above_it() {
        // Captured from mcode: the agent introduces the panel with a sentence of
        // its own, and the reader has to see that sentence as well as the panel.
        let screen = "  ● 来，一道选择题，用 ↑↓ 选，回车确认：\n\n\n  ╭─ Ask ───────────────────────────────────────────────────────────────────────────────────────── 1 of 1 ─╮\n  │ 选一个                                                                                                 │\n  │ 接下来想让我先干哪件事？                                                                               │\n  │                                                                                                        │\n  │ › 1  先摸一遍仓库架构，给我讲讲                                                                        │\n  │   2  统计 OHOS 移植点有多少处                                                                          │\n  │   3  随便找个能修的问题动手改                                                                          │\n  │   4  就答题，别干活                                                                                    │\n  │   5  Other…                                                                                            │\n  ├────────────────────────────────────────────────────────────────────────────────────────────────────────┤\n  │ ↑↓ move · 1-5 select · Enter send · Esc cancel                                                         │\n  ╰────────────────────────────────────────────────────────────────────────────────────────────────────────╯\n";
        let expected = "来，一道选择题，用 ↑↓ 选，回车确认：\n选一个\n接下来想让我先干哪件事？\n1. 先摸一遍仓库架构，给我讲讲\n2. 统计 OHOS 移植点有多少处\n3. 随便找个能修的问题动手改\n4. 就答题，别干活\n5. Other…";
        assert_eq!(extract("minimax", screen).as_deref(), Some(expected));
    }

    #[test]
    fn a_minimax_panel_keeps_the_reply_whose_last_row_sits_above_it() {
        // Captured from mcode: the row above the panel is the last continuation
        // row of the reply, not its first, so the walk has to climb to the row
        // the block starts on.
        let screen = "  ● 当然有，题库不是消耗品 😄\n\n    说点题外话：我一直觉得「选择题」这种形式有意思的地方在于——四个选项里往往藏着一个你误以为对的东西。真正考\n    的不是记忆，而是你有没有在心里把候选答案都过一遍筛子。上面两题你都答得干脆，说明是后者。那这次我加一点难\n    度，换到网络方向：\n\n\n  ╭─ Ask ───────────────────────────────────────────────────────────────────────────────────────── 1 of 1 ─╮\n  │ 网络小测                                                                                               │\n  │ TCP 连接关闭时，主动关闭的一方会进入 TIME_WAIT 并等待 2MSL。这个等待的主要目的是什么？                 │\n  │                                                                                                        │\n  │ › 1  等 2MSL 是为了兜住迟到的 ACK 和残留旧报文                                                         │\n  │   2  为了等被动关闭方先发 FIN                                                                          │\n  │   3  避免端口号发生冲突                                                                                │\n  │   4  保证数据包按序到达                                                                                │\n  │   5  Other…                                                                                            │\n  ├────────────────────────────────────────────────────────────────────────────────────────────────────────┤\n  │ ↑↓ move · 1-5 select · Enter send · Esc cancel                                                         │\n  ╰────────────────────────────────────────────────────────────────────────────────────────────────────────╯\n";
        let expected = "当然有，题库不是消耗品 😄\n\n说点题外话：我一直觉得「选择题」这种形式有意思的地方在于——四个选项里往往藏着一个你误以为对的东西。真正考\n的不是记忆，而是你有没有在心里把候选答案都过一遍筛子。上面两题你都答得干脆，说明是后者。那这次我加一点难\n度，换到网络方向：\n网络小测\nTCP 连接关闭时，主动关闭的一方会进入 TIME_WAIT 并等待 2MSL。这个等待的主要目的是什么？\n1. 等 2MSL 是为了兜住迟到的 ACK 和残留旧报文\n2. 为了等被动关闭方先发 FIN\n3. 避免端口号发生冲突\n4. 保证数据包按序到达\n5. Other…";
        assert_eq!(extract("minimax", screen).as_deref(), Some(expected));
    }

    #[test]
    fn a_minimax_reply_that_opens_with_a_key_hint_is_still_a_reply() {
        // The opening row uses the same words as a chrome hint; only a block
        // that is chrome from end to end is a status line.
        let screen = "  ● 用 ↑↓ 选择，回车确认。\n\n    这是正文。\n\n    └ Completed in 2s\n";
        assert_eq!(
            extract("minimax", screen).as_deref(),
            Some("用 ↑↓ 选择，回车确认。\n\n这是正文。")
        );
    }

    #[test]
    fn a_minimax_panel_with_a_gap_in_its_numbering_is_not_a_choice() {
        // The panel numbers its answers from one upwards, so a gap means these
        // rows are not an answer list at all.
        let screen = "╭─ Ask ────────────────────────────── 1 of 1 ─╮\n│ 小测                                            │\n│ 哪个说法对？                                     │\n│                                                  │\n│ › 1  第一个说法                                   │\n│   3  第三个说法                                   │\n├──────────────────────────────────────────────────┤\n│ ↑↓ move · 1-3 select · Enter send · Esc cancel   │\n╰──────────────────────────────────────────────────╯\n";
        assert!(minimax::extract_choice(&useful_lines(screen)).is_none());
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
    fn a_real_qwen_choice_keeps_its_wrapped_question() {
        // Captured from a live pane: the question wraps over three rows and the
        // panel header above it must never become the question.
        let screen = r#"

  Tips: Try /insight to generate personalized insights from your chat history.
  ● Read context files: AGENTS.md, ~/.qwen/output-language.md

  > 用askuserquestion工具给我出一个选择题，题目长度要50个字以上。不需要逻辑，只是测试连通性

  ∴ Thought briefly (click or ctrl+o to expand)

  ◆ 我现在调用选择题工具生成一道没有逻辑含义、题面超过 50 字的测试题。
  △ Update check skipped (registry error) — run /update to retry.
  ? AskUserQuestion Ask user 1 question ←

   连通测试

   这是一道纯粹用于测试工具连通性的选择题，题目内容本身没有任何实际含义也不涉及任何逻辑推理，你只需要随便选
   一个选项即可，选项之间没有对错之分，也不会影响任何后续操作或结果，请问你更愿意选择下面哪一个看起来顺眼的
   选项呢？

   ❯ 1. 选项甲：蓝色
        第一个选项，仅用于验证选项列表能正常渲染。
     2. 选项乙：绿色
        第二个选项，仅用于验证多选一交互是否正常。
     3. 选项丙：橙色
        第三个选项，仅用于验证选项描述文本能正常显示。
     4. 选项丁：紫色
        第四个选项，仅用于验证四个选项的布局是否完整。
     5. Type something...


   ↑/↓: Navigate | Enter: Select | Esc: Cancel


  ⠏ Waiting for user confirmation...
"#;
        let lines = useful_lines(screen);
        let reply = qwen::extract_choice(&lines).expect("a choice");
        assert!(reply.starts_with("连通测试"), "{reply}");
        assert!(reply.contains("这是一道纯粹用于测试工具连通性的选择题"), "{reply}");
        assert!(reply.contains("选项呢？"), "{reply}");
        assert!(reply.contains("1. 选项甲：蓝色"), "{reply}");
        assert!(reply.contains("  第一个选项，仅用于验证选项列表能正常渲染。"), "{reply}");
        assert!(reply.contains("4. 选项丁：紫色"), "{reply}");
        assert!(!reply.contains("Type something"), "{reply}");
        assert!(!reply.contains("AskUserQuestion"), "{reply}");
    }


    #[test]
    fn qwen_takes_the_newest_of_two_questions() {
        // An answered question stays on screen above the one being asked now.
        let screen = " ? AskUserQuestion Ask user 1 question ←\n\n旧标题\n旧问题？\n\n❯ 1. 甲\n     甲的说明\n  2. 乙\n     乙的说明\n  3. Type something...\n\n ? AskUserQuestion Ask user 1 question ←\n\n新标题\n新问题？\n\n❯ 1. 丙\n     丙的说明\n  2. 丁\n     丁的说明\n  3. Type something...\n";
        let lines = useful_lines(screen);
        let reply = qwen::extract_choice(&lines).expect("a choice");
        assert!(reply.starts_with("新标题"), "{reply}");
        assert!(reply.contains("新问题？"), "{reply}");
        assert!(reply.contains("1. 丙"), "{reply}");
        assert!(!reply.contains("旧标题"), "{reply}");
    }

    #[test]
    fn a_newer_choice_wins_over_the_answered_one() {
        let screen = "旧题？\n\n1. 甲\n   甲的说明\n2. 乙\n   乙的说明\n\n新题？\n\n1. 丙\n   丙的说明\n2. 丁\n   丁的说明\n\nEnter to select\n";
        let lines = useful_lines(screen);
        let reply = choice_block(&lines).expect("a choice");
        assert!(reply.starts_with("新题？"), "{reply}");
        assert!(reply.contains("1. 丙"), "{reply}");
        assert!(!reply.contains("1. 甲"), "{reply}");
    }

    #[test]
    fn qwen_plain_reply_is_the_last_block() {
        let screen = " > 只回复两个字：成功\n\n ◆ 成功\n\n ? for shortcuts\n";
        let lines = useful_lines(screen);
        assert_eq!(qwen::extract(&lines).as_deref(), Some("成功"));
    }

    #[test]
    fn qwen_plain_reply_stops_at_a_tool_result() {
        // Captured from a live pane: the reply sits above a tool result and a
        // status line that would otherwise become the body.
        let screen = r#"
  ● Read context files: AGENTS.md, ~/.qwen/output-language.md

  > 只回复两个字：成功

  ◆ 成功
   ⎿ Stop says:
      Warning: Traceback (most recent call last):

──────────────────────────────────────────────────────────────────────────────────────────────────────────────
>   Type your message or @path/to/file
──────────────────────────────────────────────────────────────────────────────────────────────────────────────
  ➜ hicodeer · git:(main) · deepseek-flash high · 1.0m Context 2.1% used
  Auto mode (shift + tab to cycle) · 1 dream

"#;
        let lines = useful_lines(screen);
        assert_eq!(qwen::extract(&lines).as_deref(), Some("成功"));
    }

    #[test]
    fn chrome_rows_are_never_a_reply() {
        let screen = " ● ask · files: workspace   Shift+Tab to change   work (Tab)\n DeepSeek · deepseek-flash · thinking: max   context 2%\n";
        assert!(extract_lines("claude", screen).is_none());
    }
}
