use std::fs;
use std::io;
use std::path::Path;

pub(crate) fn remove_file_if_exists(path: &Path) -> io::Result<bool> {
    match fs::remove_file(path) {
        Ok(()) => Ok(true),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(err) => Err(err),
    }
}

pub(crate) fn legacy_bash_hook_path(hook_path: &Path) -> std::path::PathBuf {
    hook_path.with_file_name("hicodeer-agent-state.sh")
}

/// Removes a hook script left by a build that used the previous file name.
/// Both markers are accepted because the product name changed twice: the
/// current one, and the `HICODEER_AGENT_` spelling an intermediate build wrote.
pub(crate) fn remove_legacy_bash_hook_file(hook_path: &Path) -> io::Result<bool> {
    let legacy_path = legacy_bash_hook_path(hook_path);
    if legacy_path == hook_path {
        return Ok(false);
    }
    let content = match fs::read_to_string(&legacy_path) {
        Ok(content) => content,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(err) => return Err(err),
    };

    if content.contains("HICODEER_INTEGRATION_ID=") || content.contains("HICODEER_AGENT_INTEGRATION_ID=")
    {
        fs::remove_file(legacy_path)?;
        return Ok(true);
    }

    Ok(false)
}

pub(crate) fn remove_dir_all_if_exists(path: &Path) -> io::Result<bool> {
    match fs::remove_dir_all(path) {
        Ok(()) => Ok(true),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(err) => Err(err),
    }
}

pub(crate) fn make_executable(_path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        let mut perms = fs::metadata(_path)?.permissions();
        perms.set_mode(0o755);
        fs::set_permissions(_path, perms)?;
    }

    Ok(())
}

/// Reads a configuration document, treating a missing file as empty so the
/// caller can fold its own entries into whatever the agent already wrote.
pub(crate) fn read_text_if_exists(path: &Path) -> io::Result<String> {
    match fs::read_to_string(path) {
        Ok(contents) => Ok(contents),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(String::new()),
        Err(err) => Err(err),
    }
}

/// Agents that hold credentials or telemetry next to their hook document keep
/// the file owner-only; a hook document written by us matches that mode.
pub(crate) fn make_private_file(_path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        let mut perms = fs::metadata(_path)?.permissions();
        perms.set_mode(0o600);
        fs::set_permissions(_path, perms)?;
    }

    Ok(())
}
