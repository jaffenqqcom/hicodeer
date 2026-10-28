// ZCode reads its hooks from `~/.zcode/cli/setting.json` under
// `hooks.events.<EventName>`. The shape is Claude Code's: a list of entries,
// each with an optional `matcher` and a `hooks` array of command definitions.
// The events and payloads are the same names too, so the hook script installs
// unchanged and only its integration id differs.

use std::io;
use std::path::Path;

use serde_json::{Map, Value};

use super::command::hook_command;
use super::config_edit::{ensure_command_hook, hook_command_variants, is_matching_command_hook};

/// One hook entry the integration registers.
struct EventHook {
    event: &'static str,
    matcher: Option<&'static str>,
    action: &'static str,
}

/// Asking the user a question parks the turn on that answer, so the matching
/// tool call reports blocked while every other tool reports working. ZCode
/// emits the same `PreToolUse` event as Claude Code, so the split is identical.
const ASK_USER_MATCHER: &str = "^AskUserQuestion$";
const OTHER_TOOLS_MATCHER: &str = "^(?!AskUserQuestion$).*$";

const EVENT_HOOKS: &[EventHook] = &[
    EventHook {
        event: "SessionStart",
        matcher: None,
        action: "session",
    },
    EventHook {
        event: "UserPromptSubmit",
        matcher: None,
        action: "working",
    },
    EventHook {
        event: "PreToolUse",
        matcher: Some(ASK_USER_MATCHER),
        action: "blocked",
    },
    EventHook {
        event: "PreToolUse",
        matcher: Some(OTHER_TOOLS_MATCHER),
        action: "working",
    },
    EventHook {
        event: "PermissionRequest",
        matcher: None,
        action: "blocked",
    },
    EventHook {
        event: "PostToolUse",
        matcher: None,
        action: "working",
    },
    EventHook {
        event: "PostToolUseFailure",
        matcher: None,
        action: "working",
    },
    EventHook {
        event: "Stop",
        matcher: None,
        action: "idle",
    },
];

/// Adds the integration's hooks to ZCode's settings document.
///
/// `hooks.enabled` is turned on because ZCode ships it off: without it every
/// entry below is inert and the panel would show no state at all. Everything
/// outside `hooks` is left untouched.
pub(crate) fn install(content: &str, settings_path: &Path, hook_path: &Path) -> io::Result<String> {
    let original = parse_settings(content, settings_path)?;
    let mut desired = original.clone();
    let hooks = hooks_object(&mut desired, settings_path)?;
    set_enabled(hooks);
    let events = events_object(hooks, settings_path)?;

    for hook in EVENT_HOOKS {
        ensure_command_hook(
            events,
            hook.event,
            hook_command(hook_path, Some(hook.action)),
            10,
            hook.matcher,
        )?;
    }

    if desired == original {
        return Ok(content.to_string());
    }
    render(&desired)
}

/// Removes the integration's hooks. `hooks.enabled` is left alone so a user's
/// own hooks keep working, but containers we created and that are now empty are
/// removed so a reinstall cannot inherit them.
pub(crate) fn uninstall(
    content: &str,
    settings_path: &Path,
    hook_path: &Path,
) -> io::Result<(String, bool)> {
    let mut settings = parse_settings(content, settings_path)?;
    let commands: Vec<String> = EVENT_HOOKS
        .iter()
        .flat_map(|hook| hook_command_variants(hook_path, Some(hook.action)))
        .collect();

    let mut changed = false;
    if let Some(hooks) = settings.get_mut("hooks") {
        let hooks = hooks.as_object_mut().ok_or_else(|| {
            io::Error::other(format!("{}: hooks must be an object", settings_path.display()))
        })?;
        if let Some(events) = hooks.get_mut("events") {
            let events = events.as_object_mut().ok_or_else(|| {
                io::Error::other(format!(
                    "{}: hooks.events must be an object",
                    settings_path.display()
                ))
            })?;
            for entries in events.values_mut() {
                let Some(entries) = entries.as_array_mut() else {
                    continue;
                };
                changed |= remove_integration_entries(entries, &commands);
            }
            // Only tidy up containers after we actually removed something, so a
            // user's own empty arrays are left as they were.
            if changed {
                events.retain(|_, entries| {
                    entries
                        .as_array()
                        .is_some_and(|entries| !entries.is_empty())
                });
                if events.is_empty() {
                    hooks.remove("events");
                }
            }
        }
    }

    if !changed {
        return Ok((content.to_string(), false));
    }
    Ok((render(&settings)?, true))
}

/// Drops entries whose `hooks` array holds one of our commands, and entries
/// left without any hook. Reports whether a hook was removed, which is not the
/// same as an entry disappearing: a user hook may share the entry with ours.
fn remove_integration_entries(entries: &mut Vec<Value>, commands: &[String]) -> bool {
    let mut removed_hook = false;
    entries.retain_mut(|entry| {
        let Some(hooks) = entry.get_mut("hooks").and_then(Value::as_array_mut) else {
            return true;
        };
        let before = hooks.len();
        hooks.retain(|hook| {
            !commands
                .iter()
                .any(|command| is_matching_command_hook(hook, command))
        });
        if hooks.len() != before {
            removed_hook = true;
        }
        !hooks.is_empty()
    });
    removed_hook
}

fn parse_settings(content: &str, path: &Path) -> io::Result<Value> {
    if content.trim().is_empty() {
        return Ok(Value::Object(Map::new()));
    }
    serde_json::from_str(content).map_err(|error| {
        io::Error::other(format!("failed to parse {}: {error}", path.display()))
    })
}

fn hooks_object<'a>(settings: &'a mut Value, path: &Path) -> io::Result<&'a mut Map<String, Value>> {
    let object = settings.as_object_mut().ok_or_else(|| {
        io::Error::other(format!("{} must contain a JSON object", path.display()))
    })?;
    object
        .entry("hooks")
        .or_insert_with(|| Value::Object(Map::new()))
        .as_object_mut()
        .ok_or_else(|| io::Error::other(format!("{}: hooks must be an object", path.display())))
}

fn events_object<'a>(
    hooks: &'a mut Map<String, Value>,
    path: &Path,
) -> io::Result<&'a mut Map<String, Value>> {
    hooks
        .entry("events")
        .or_insert_with(|| Value::Object(Map::new()))
        .as_object_mut()
        .ok_or_else(|| {
            io::Error::other(format!("{}: hooks.events must be an object", path.display()))
        })
}

fn set_enabled(hooks: &mut Map<String, Value>) {
    hooks.insert("enabled".to_string(), Value::Bool(true));
}

fn render(settings: &Value) -> io::Result<String> {
    let mut text = serde_json::to_string_pretty(settings).map_err(io::Error::other)?;
    text.push('\n');
    Ok(text)
}
