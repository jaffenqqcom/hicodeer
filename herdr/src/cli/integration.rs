use crate::api::schema::IntegrationTarget;

pub(super) fn run_integration_command(args: &[String]) -> std::io::Result<i32> {
    let Some(subcommand) = args.first().map(|arg| arg.as_str()) else {
        print_integration_help();
        return Ok(0);
    };

    match subcommand {
        "install" => integration_install(&args[1..]),
        "uninstall" => integration_uninstall(&args[1..]),
        "status" => integration_status(&args[1..]),
        "help" | "--help" | "-h" => {
            print_integration_help();
            Ok(0)
        }
        _ => {
            print_integration_help();
            Ok(2)
        }
    }
}

fn integration_status(args: &[String]) -> std::io::Result<i32> {
    let outdated_only = match args {
        [] => false,
        [flag] if flag == "--outdated-only" => true,
        _ => {
            eprintln!("usage: hicodeerd integration status [--outdated-only]");
            return Ok(2);
        }
    };

    if outdated_only {
        crate::integration::print_outdated_update_notice();
        return Ok(0);
    }

    for status in crate::integration::installed_integration_statuses() {
        let target = crate::integration::integration_target_label(status.target);
        let state = describe_integration_state(
            status.state,
            status.installed_version,
            status.expected_version,
        );
        println!("{target}: {state} ({})", status.path.display());
    }

    if let Some(status) = crate::integration::experimental_letta_integration_status() {
        let state = describe_integration_state(
            status.state,
            status.installed_version,
            status.expected_version,
        );
        println!(
            "{} (experimental): {state} ({})",
            status.label,
            status.path.display()
        );
    }

    Ok(0)
}

fn describe_integration_state(
    state: crate::integration::IntegrationStatusKind,
    installed_version: Option<u32>,
    expected_version: u32,
) -> String {
    let version = match installed_version {
        Some(version) => format!("v{version}"),
        None => "legacy".to_string(),
    };
    match state {
        crate::integration::IntegrationStatusKind::NotInstalled => "not installed".to_string(),
        crate::integration::IntegrationStatusKind::Current => format!("current ({version})"),
        crate::integration::IntegrationStatusKind::Outdated
            if installed_version.is_some_and(|installed| installed >= expected_version) =>
        {
            format!("needs repair ({version})")
        }
        crate::integration::IntegrationStatusKind::Outdated => {
            format!("outdated ({version} < v{expected_version})")
        }
    }
}

fn integration_install(args: &[String]) -> std::io::Result<i32> {
    let Some(target) = parse_integration_target(args, "install")? else {
        return Ok(2);
    };

    let installed = match target {
        IntegrationCommandTarget::Builtin(target) => crate::integration::install_target(target),
        IntegrationCommandTarget::Letta => crate::integration::install_experimental_letta(),
    };
    match installed {
        Ok(messages) => {
            print_integration_messages(messages);
            Ok(0)
        }
        Err(err) => {
            eprintln!("{err}");
            Ok(1)
        }
    }
}

fn integration_uninstall(args: &[String]) -> std::io::Result<i32> {
    let Some(target) = parse_integration_target(args, "uninstall")? else {
        return Ok(2);
    };

    let removed = match target {
        IntegrationCommandTarget::Builtin(target) => crate::integration::uninstall_target(target),
        IntegrationCommandTarget::Letta => crate::integration::uninstall_experimental_letta(),
    };
    match removed {
        Ok(messages) => {
            print_integration_messages(messages);
            Ok(0)
        }
        Err(err) => {
            eprintln!("{err}");
            Ok(1)
        }
    }
}

fn print_integration_messages(messages: Vec<String>) {
    for message in messages {
        println!("{message}");
    }
}

/// Integration target accepted by the CLI. Letta is deliberately kept out of
/// the frozen client endpoint `IntegrationTarget` enum and is handled as an
/// experimental CLI-only target until the agent registry replaces it.
enum IntegrationCommandTarget {
    Builtin(IntegrationTarget),
    Letta,
}

fn parse_integration_target(
    args: &[String],
    action: &str,
) -> std::io::Result<Option<IntegrationCommandTarget>> {
    if args.len() != 1 {
        eprintln!(
            "usage: hicodeerd integration {action} <{}>",
            integration_target_usage_list()
        );
        return Ok(None);
    }
    let target = args[0].as_str();

    // Letta is deliberately outside the frozen client endpoint enum, so the
    // registry lookup below cannot return it.
    if target == "letta" {
        return Ok(Some(IntegrationCommandTarget::Letta));
    }
    if let Some(parsed) = crate::integration::integration_target_from_label(target) {
        return Ok(Some(IntegrationCommandTarget::Builtin(parsed)));
    }

    eprintln!("unknown integration target: {target}");
    eprintln!(
        "currently supported: {}",
        integration_target_usage_list().replace('|', ", ")
    );
    Ok(None)
}

/// Every installable target label, in registry order, followed by the
/// CLI-only experimental one.
fn integration_target_usage_list() -> String {
    let mut labels = crate::integration::integration_target_labels();
    labels.push("letta");
    labels.join("|")
}

fn print_integration_help() {
    let mut labels = crate::integration::integration_target_labels();
    labels.push("letta");
    eprintln!("hicodeerd integration commands:");
    for label in &labels {
        eprintln!("  hicodeerd integration install {label}");
    }
    for label in &labels {
        eprintln!("  hicodeerd integration uninstall {label}");
    }
    eprintln!("  hicodeerd integration status [--outdated-only]");
}
