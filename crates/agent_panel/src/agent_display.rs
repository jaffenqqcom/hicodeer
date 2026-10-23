//! How an agent is drawn wherever the editor lists one: the mark it wears and
//! how its name is spelled.

use ui::IconName;

/// The mark for one agent type.
///
/// `kind` is either the name the detector reports (`agy`, `codebuddy`) or the
/// server's integration target (`antigravity_cli`, `code_buddy`), so every
/// spelling a product is named by is accepted here.
pub fn agent_icon(kind: &str) -> IconName {
    match kind {
        "claude" => IconName::AiClaude,
        "gemini" => IconName::AiGemini,
        "codex" => IconName::AiOpenAi,
        "copilot" => IconName::Copilot,
        "opencode" => IconName::AiOpenCode,
        "grok" => IconName::AiXAi,
        "deepseek" | "dsh" => IconName::AiDeepSeek,
        "zcode" => IconName::AiZCode,
        "minimax" | "mcode" => IconName::AiMiniMax,
        "trae" | "traecli" => IconName::AiTrae,
        "agy" | "antigravity" | "antigravity_cli" | "antigravity-cli" => IconName::AiAntigravity,
        "codebuddy" | "code_buddy" => IconName::AiCodeBuddy,
        "cursor" => IconName::AiCursor,
        "devin" => IconName::AiDevin,
        "droid" => IconName::AiDroid,
        "hermes" => IconName::AiHermes,
        "kilo" => IconName::AiKilo,
        "kimi" => IconName::AiKimi,
        "mastracode" => IconName::AiMastra,
        "omp" => IconName::AiOmp,
        "pi" => IconName::AiPi,
        "qodercli" => IconName::AiQoder,
        "qwen" => IconName::AiQwen,
        _ => IconName::Sparkle,
    }
}

/// Raises the first letter of an agent's name.
///
/// The detector reports type names in lower case (`claude`, `qodercli`), and
/// the panel lists them beside space labels that are already capitalised, so
/// the first letter is raised here rather than changing the server's spelling.
pub fn capitalize(name: &str) -> String {
    let mut characters = name.chars();
    match characters.next() {
        Some(first) => first.to_uppercase().chain(characters).collect(),
        None => String::new(),
    }
}

/// How an integration's server-side label is spelled in the settings page.
///
/// The server names a target the way its command is spelled (`codebuddy`,
/// `antigravity-cli`), which is not how the product writes itself. Every other
/// label is a single word, so raising its first letter is enough.
pub fn integration_name(label: &str) -> String {
    match label {
        "codebuddy" => "CodeBuddy".to_string(),
        "antigravity-cli" => "Antigravity CLI".to_string(),
        "opencode" => "OpenCode".to_string(),
        "qodercli" => "Qoder".to_string(),
        "omp" => "OMP".to_string(),
        other => capitalize(other),
    }
}
