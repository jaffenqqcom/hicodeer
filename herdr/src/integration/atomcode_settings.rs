// AtomCode reads lifecycle hooks from `~/.atomcode/hooks.json` (and a project
// `.hooks.json`, which needs the workspace to be trusted, so the integration
// only writes the global one). Entries are named objects:
//
//   { "hooks": { "name": { "event": "PreToolUse", "command": "...", ... } } }
//
// The stdin payload is Claude Code's, so the hook script is the Claude one with
// a different integration id.

use std::io;
use std::path::Path;

use serde_json::{Map, Value};

use super::command::hook_command;
use super::config_edit::hook_command_variants;

/// One hook the integration registers.
struct EventHook {
    event: &'static str,
    action: &'static str,
}

/// `StopFailure` is not wired yet: AtomCode reports a failed turn through it,
/// but the panel only needs the idle transition that `Stop` already carries.
const EVENT_HOOKS: &[EventHook] = &[
    EventHook {
        event: "SessionStart",
        action: "session",
    },
    EventHook {
        event: "UserPromptSubmit",
        action: "working",
    },
    EventHook {
        event: "PreToolUse",
        action: "working",
    },
    EventHook {
        event: "PostToolUse",
        action: "working",
    },
    EventHook {
        event: "PostToolUseFailure",
        action: "working",
    },
    EventHook {
        event: "Stop",
        action: "idle",
    },
    EventHook {
        event: "SessionEnd",
        action: "idle",
    },
];

const HOOK_KEY_PREFIX: &str = "hicodeer-agent-state";
const HOOK_TIMEOUT_MS: u64 = 10_000;

pub(crate) fn install(content: &str, hooks_path: &Path, hook_path: &Path) -> io::Result<String> {
    let original = parse(content, hooks_path)?;
    let mut desired = original.clone();
    let hooks = hooks_object(&mut desired, hooks_path)?;

    for hook in EVENT_HOOKS {
        let key = format!("{HOOK_KEY_PREFIX}-{}", kebab_case(hook.event));
        let command = hook_command(hook_path, Some(hook.action));
        if hooks.get(&key).is_some_and(|entry| {
            entry.get("command").and_then(Value::as_str) == Some(command.as_str())
        }) {
            continue;
        }
        let mut entry = Map::new();
        entry.insert("event".to_string(), Value::String(hook.event.to_string()));
        entry.insert("command".to_string(), Value::String(command));
        entry.insert(
            "timeout_ms".to_string(),
            Value::Number(HOOK_TIMEOUT_MS.into()),
        );
        hooks.insert(key, Value::Object(entry));
    }

    if desired == original {
        return Ok(content.to_string());
    }
    render(&desired)
}

pub(crate) fn uninstall(
    content: &str,
    hooks_path: &Path,
    hook_path: &Path,
) -> io::Result<(String, bool)> {
    if content.trim().is_empty() {
        return Ok((content.to_string(), false));
    }
    let mut document = parse(content, hooks_path)?;
    let commands: Vec<String> = EVENT_HOOKS
        .iter()
        .flat_map(|hook| hook_command_variants(hook_path, Some(hook.action)))
        .collect();

    let mut changed = false;
    let mut drop_hooks = false;
    if let Some(hooks) = document.get_mut("hooks") {
        let hooks = hooks.as_object_mut().ok_or_else(|| {
            io::Error::other(format!("{}: hooks must be an object", hooks_path.display()))
        })?;
        let before = hooks.len();
        hooks.retain(|_, entry| {
            !entry
                .get("command")
                .and_then(Value::as_str)
                .is_some_and(|command| commands.iter().any(|candidate| candidate == command))
        });
        changed = hooks.len() != before;
        drop_hooks = changed && hooks.is_empty();
    }
    if drop_hooks {
        let object = document.as_object_mut().ok_or_else(|| {
            io::Error::other(format!(
                "{} must contain a JSON object",
                hooks_path.display()
            ))
        })?;
        object.remove("hooks");
    }

    if !changed {
        return Ok((content.to_string(), false));
    }
    Ok((render(&document)?, true))
}

fn parse(content: &str, hooks_path: &Path) -> io::Result<Value> {
    if content.trim().is_empty() {
        return Ok(Value::Object(Map::new()));
    }
    serde_json::from_str(content).map_err(|error| {
        io::Error::other(format!(
            "failed to parse {}: {error}",
            hooks_path.display()
        ))
    })
}

fn hooks_object<'a>(
    document: &'a mut Value,
    hooks_path: &Path,
) -> io::Result<&'a mut Map<String, Value>> {
    let object = document.as_object_mut().ok_or_else(|| {
        io::Error::other(format!(
            "{} must contain a JSON object",
            hooks_path.display()
        ))
    })?;
    object
        .entry("hooks")
        .or_insert_with(|| Value::Object(Map::new()))
        .as_object_mut()
        .ok_or_else(|| {
            io::Error::other(format!("{}: hooks must be an object", hooks_path.display()))
        })
}

fn render(document: &Value) -> io::Result<String> {
    let mut text = serde_json::to_string_pretty(document).map_err(io::Error::other)?;
    text.push('\n');
    Ok(text)
}

/// `SessionStart` becomes `session-start`, so each hook gets a stable key of its
/// own instead of overwriting the one before it.
fn kebab_case(event: &str) -> String {
    let mut text = String::with_capacity(event.len() + 4);
    for (index, character) in event.char_indices() {
        if character.is_uppercase() && index != 0 {
            text.push('-');
        }
        text.extend(character.to_lowercase());
    }
    text
}
