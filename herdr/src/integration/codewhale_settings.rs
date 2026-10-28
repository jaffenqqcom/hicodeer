// Codewhale takes its hooks from the `[hooks]` table of the user's
// `config.toml`, with one `[[hooks.hooks]]` entry per event. The payload is
// Claude Code's JSON on stdin, and the session id is also exported as
// `CODEWHALE_SESSION_ID`, so the hook script is the Claude one with a different
// integration id.
//
// The file is edited in place through `toml_edit` because Codewhale ships a
// commented default config: rebuilding it from a value tree would erase those
// comments.

use std::io;
use std::path::Path;

use toml_edit::{value, ArrayOfTables, DocumentMut, Item, Table};

use super::command::hook_command;
use super::config_edit::hook_command_variants;

/// One hook the integration registers.
struct EventHook {
    event: &'static str,
    action: &'static str,
}

/// `session_busy` fires when an approval prompt opens, a question is presented,
/// or a goal continuation parks — the states the panel draws as blocked.
/// `session_idle` covers the settle back to the prompt after a turn.
const EVENT_HOOKS: &[EventHook] = &[
    EventHook {
        event: "session_start",
        action: "session",
    },
    EventHook {
        event: "message_submit",
        action: "working",
    },
    EventHook {
        event: "tool_call_before",
        action: "working",
    },
    EventHook {
        event: "tool_call_after",
        action: "working",
    },
    EventHook {
        event: "session_idle",
        action: "idle",
    },
    EventHook {
        event: "turn_end",
        action: "idle",
    },
    EventHook {
        event: "session_busy",
        action: "blocked",
    },
    EventHook {
        event: "session_end",
        action: "idle",
    },
];

const HOOK_NAME: &str = "hicodeer agent state";

pub(crate) fn install(content: &str, config_path: &Path, hook_path: &Path) -> io::Result<String> {
    let original = content.to_string();
    let mut document = parse(content, config_path)?;
    set_hooks_enabled(&mut document, config_path)?;
    let entries = hook_entries(&mut document, config_path)?;

    for hook in EVENT_HOOKS {
        let command = hook_command(hook_path, Some(hook.action));
        if entries.iter().any(|table| is_our_entry(table, hook.event, &command)) {
            continue;
        }
        let mut table = Table::new();
        table["name"] = value(HOOK_NAME);
        table["event"] = value(hook.event);
        table["command"] = value(command);
        table["timeout_secs"] = value(10_i64);
        entries.push(table);
    }

    let rendered = document.to_string();
    Ok(if rendered == original { original } else { rendered })
}

pub(crate) fn uninstall(
    content: &str,
    config_path: &Path,
    hook_path: &Path,
) -> io::Result<(String, bool)> {
    if content.trim().is_empty() {
        return Ok((content.to_string(), false));
    }
    let mut document = parse(content, config_path)?;
    let commands: Vec<String> = EVENT_HOOKS
        .iter()
        .flat_map(|hook| hook_command_variants(hook_path, Some(hook.action)))
        .collect();

    let mut changed = false;
    if let Some(hooks) = document.get_mut("hooks") {
        let hooks = hooks.as_table_mut().ok_or_else(|| {
            io::Error::other(format!(
                "{}: hooks must be a table",
                config_path.display()
            ))
        })?;
        if let Some(entries) = hooks.get_mut("hooks") {
            let entries = entries.as_array_of_tables_mut().ok_or_else(|| {
                io::Error::other(format!(
                    "{}: hooks.hooks must be an array of tables",
                    config_path.display()
                ))
            })?;
            let before = entries.len();
            let mut kept = ArrayOfTables::new();
            for table in entries.iter() {
                let ours = table
                    .get("command")
                    .and_then(Item::as_str)
                    .is_some_and(|command| commands.iter().any(|candidate| candidate == command));
                if !ours {
                    kept.push(table.clone());
                }
            }
            changed = kept.len() != before;
            if changed {
                // An empty array of tables would serialise a bare header and be
                // inherited by a reinstall, so drop the key entirely.
                if kept.is_empty() {
                    hooks.remove("hooks");
                } else {
                    *entries = kept;
                }
            }
        }
    }

    let rendered = document.to_string();
    Ok(if changed {
        (rendered, true)
    } else {
        (content.to_string(), false)
    })
}

fn parse(content: &str, config_path: &Path) -> io::Result<DocumentMut> {
    content.parse::<DocumentMut>().map_err(|error| {
        io::Error::other(format!(
            "failed to parse {}: {error}",
            config_path.display()
        ))
    })
}

fn set_hooks_enabled(document: &mut DocumentMut, config_path: &Path) -> io::Result<()> {
    if !document.contains_key("hooks") {
        document["hooks"] = Item::Table(Table::new());
    }
    let hooks = document["hooks"].as_table_mut().ok_or_else(|| {
        io::Error::other(format!("{}: hooks must be a table", config_path.display()))
    })?;
    hooks["enabled"] = value(true);
    Ok(())
}

fn hook_entries<'a>(
    document: &'a mut DocumentMut,
    config_path: &Path,
) -> io::Result<&'a mut ArrayOfTables> {
    let hooks = document["hooks"].as_table_mut().ok_or_else(|| {
        io::Error::other(format!("{}: hooks must be a table", config_path.display()))
    })?;
    if !hooks.contains_key("hooks") {
        hooks["hooks"] = Item::ArrayOfTables(ArrayOfTables::new());
    }
    hooks["hooks"].as_array_of_tables_mut().ok_or_else(|| {
        io::Error::other(format!(
            "{}: hooks.hooks must be an array of tables",
            config_path.display()
        ))
    })
}

fn is_our_entry(table: &Table, event: &str, command: &str) -> bool {
    table.get("event").and_then(Item::as_str) == Some(event)
        && table.get("command").and_then(Item::as_str) == Some(command)
}
