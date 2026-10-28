//! How an agent is drawn wherever the editor lists one: the mark it wears and
//! how its name is spelled.

use ui::IconName;

/// One agent as the editor draws it.
pub struct AgentPresentation {
    /// The mark the agent wears in every list.
    pub icon: IconName,
    /// How the product writes its own name. This is not `capitalize(label)`:
    /// the products spell themselves `DeepSeek`, `MiniMax`, `CodeBuddy`.
    pub display_name: &'static str,
}

/// Every spelling a product is named by, and how it is drawn.
///
/// Three spellings reach this module: the detector's type name (`agy`,
/// `codebuddy`), the server's integration target (`antigravity-cli`,
/// `code_buddy`) and the label the settings page shows. They all point at one
/// entry, so an agent's mark and its name cannot drift apart.
const AGENT_PRESENTATIONS: &[(&[&str], AgentPresentation)] = &[
    (
        &["atomcode"],
        AgentPresentation {
            icon: IconName::AiAtomcode,
            display_name: "Atomcode",
        },
    ),
    (
        &["agy", "antigravity", "antigravity_cli", "antigravity-cli"],
        AgentPresentation {
            icon: IconName::AiAntigravity,
            display_name: "Antigravity CLI",
        },
    ),
    (
        &["claude"],
        AgentPresentation {
            icon: IconName::AiClaude,
            display_name: "Claude",
        },
    ),
    (
        &["codebuddy", "code_buddy"],
        AgentPresentation {
            icon: IconName::AiCodeBuddy,
            display_name: "CodeBuddy",
        },
    ),
    (
        &["codewhale"],
        AgentPresentation {
            icon: IconName::AiCodewhale,
            display_name: "Codewhale",
        },
    ),
    (
        &["codex"],
        AgentPresentation {
            icon: IconName::AiOpenAi,
            display_name: "Codex",
        },
    ),
    (
        &["copilot"],
        AgentPresentation {
            icon: IconName::Copilot,
            display_name: "Copilot",
        },
    ),
    (
        &["cursor"],
        AgentPresentation {
            icon: IconName::AiCursor,
            display_name: "Cursor",
        },
    ),
    (
        &["deepseek", "dsh"],
        AgentPresentation {
            icon: IconName::AiDeepSeek,
            display_name: "DeepSeek",
        },
    ),
    (
        &["deveco"],
        AgentPresentation {
            icon: IconName::AiDeveco,
            display_name: "DevEco",
        },
    ),
    (
        &["devin"],
        AgentPresentation {
            icon: IconName::AiDevin,
            display_name: "Devin",
        },
    ),
    (
        &["droid"],
        AgentPresentation {
            icon: IconName::AiDroid,
            display_name: "Droid",
        },
    ),
    (
        &["gemini"],
        AgentPresentation {
            icon: IconName::AiGemini,
            display_name: "Gemini",
        },
    ),
    (
        &["grok"],
        AgentPresentation {
            icon: IconName::AiXAi,
            display_name: "Grok",
        },
    ),
    (
        &["hermes"],
        AgentPresentation {
            icon: IconName::AiHermes,
            display_name: "Hermes",
        },
    ),
    (
        &["kilo"],
        AgentPresentation {
            icon: IconName::AiKilo,
            display_name: "Kilo",
        },
    ),
    (
        &["kimi"],
        AgentPresentation {
            icon: IconName::AiKimi,
            display_name: "Kimi",
        },
    ),
    (
        &["mastracode"],
        AgentPresentation {
            icon: IconName::AiMastra,
            display_name: "Mastracode",
        },
    ),
    (
        &["minimax", "mcode"],
        AgentPresentation {
            icon: IconName::AiMiniMax,
            display_name: "MiniMax",
        },
    ),
    (
        &["omp"],
        AgentPresentation {
            icon: IconName::AiOmp,
            display_name: "OMP",
        },
    ),
    (
        &["openclaw"],
        AgentPresentation {
            icon: IconName::AiOpenclaw,
            display_name: "OpenClaw",
        },
    ),
    (
        &["opencode"],
        AgentPresentation {
            icon: IconName::AiOpenCode,
            display_name: "OpenCode",
        },
    ),
    (
        &["opendesk"],
        AgentPresentation {
            icon: IconName::AiOpendesk,
            display_name: "OpenDesk",
        },
    ),
    (
        &["pi"],
        AgentPresentation {
            icon: IconName::AiPi,
            display_name: "Pi",
        },
    ),
    (
        &["qodercli"],
        AgentPresentation {
            icon: IconName::AiQoder,
            display_name: "Qoder",
        },
    ),
    (
        &["qwen"],
        AgentPresentation {
            icon: IconName::AiQwen,
            display_name: "Qwen",
        },
    ),
    (
        &["trae", "traecli"],
        AgentPresentation {
            icon: IconName::AiTrae,
            display_name: "Trae",
        },
    ),
    (
        &["zcode"],
        AgentPresentation {
            icon: IconName::AiZCode,
            display_name: "ZCode",
        },
    ),
];

/// The entry for one spelling, if the table knows it.
fn presentation_of(kind: &str) -> Option<&'static AgentPresentation> {
    AGENT_PRESENTATIONS
        .iter()
        .find(|(spellings, _)| spellings.contains(&kind))
        .map(|(_, presentation)| presentation)
}

/// The mark for one agent type.
///
/// `kind` is either the name the detector reports (`agy`, `codebuddy`) or the
/// server's integration target (`antigravity_cli`, `code_buddy`), so every
/// spelling a product is named by is accepted here. An unknown agent wears the
/// generic sparkle rather than nothing.
pub fn agent_icon(kind: &str) -> IconName {
    presentation_of(kind).map_or(IconName::Sparkle, |presentation| presentation.icon)
}

/// The agents this build reads history from, in the spelling the daemon
/// reports as `agent.sessions[].agent`.
///
/// The settings page lists these under "Show History" so the user can see what
/// is being scanned. The daemon owns the real set (`sessions/fad.rs::fad_stores`
/// plus the stores written here); this list is the display side of the same
/// agreement and has to be updated with it.
///
/// Ordered alphabetically by display name, the same order the settings page's
/// integration grid sorts itself into, so one agent sits in the same place in
/// both grids. Keep a new entry in that order instead of appending it.
pub const SCANNABLE_AGENTS: &[&str] = &[
    "atomcode",
    "claude",
    "codebuddy",
    "codewhale",
    "codex",
    "deepseek",
    "deveco",
    "gemini",
    "hermes",
    "kimi",
    "minimax",
    "openclaw",
    "opencode",
    "opendesk",
    "pi",
    "qwen",
    "zcode",
];

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
/// `antigravity-cli`), which is not how the product writes itself. A label the
/// table does not know falls back to raising its first letter.
pub fn integration_name(label: &str) -> String {
    presentation_of(label).map_or_else(
        || capitalize(label),
        |presentation| presentation.display_name.to_owned(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The mark and the name come from the same entry, so every spelling a
    /// product is named by has to answer both.
    #[test]
    fn every_spelling_resolves_to_one_presentation() {
        for (spellings, presentation) in AGENT_PRESENTATIONS {
            for spelling in spellings.iter() {
                assert_eq!(
                    agent_icon(spelling),
                    presentation.icon,
                    "{spelling} should wear its product's mark"
                );
                assert_eq!(
                    integration_name(spelling),
                    presentation.display_name,
                    "{spelling} should be spelled the product's way"
                );
            }
        }
    }

    /// A type name the table does not know still gets a mark and a name, which
    /// is what keeps a newly added agent from rendering as blank.
    #[test]
    fn unknown_agents_fall_back() {
        assert_eq!(agent_icon("some-new-agent"), IconName::Sparkle);
        assert_eq!(integration_name("some-new-agent"), "Some-new-agent");
    }

    /// Two products spell their names with capitals inside the word, so the
    /// label alone cannot be used as the display name.
    #[test]
    fn product_spellings_are_kept() {
        assert_eq!(integration_name("codebuddy"), "CodeBuddy");
        assert_eq!(integration_name("antigravity-cli"), "Antigravity CLI");
        assert_eq!(integration_name("opencode"), "OpenCode");
        assert_eq!(integration_name("qodercli"), "Qoder");
        assert_eq!(integration_name("omp"), "OMP");
    }
}
