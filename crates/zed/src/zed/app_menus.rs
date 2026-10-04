use collab_ui::collab_panel;
use gpui::{App, Menu, MenuItem, OsAction};
use project::DisableAiSettings;
use release_channel::ReleaseChannel;
use settings::Settings;
use terminal_view::terminal_panel;
use zed_actions::{Quit, assistant, debug_panel, dev, git_panel, project_panel};

fn application_menu_name(name: &'static str) -> gpui::SharedString {
    if cfg!(target_os = "macos") {
        localization::translate_static(name).into()
    } else {
        name.into()
    }
}

// [OHOS PORT BEGIN] HiCodeer has no documentation site, social account, or
// hiring page, so the Help menu omits them. To restore the upstream menu items,
// delete this block.
#[cfg(target_env = "ohos")]
fn upstream_help_links() -> Vec<MenuItem> {
    Vec::new()
}
#[cfg(not(target_env = "ohos"))]
// [OHOS PORT END]
fn upstream_help_links() -> Vec<MenuItem> {
    vec![
        MenuItem::action(
            localization::localized_str!("Documentation"),
            super::OpenBrowser {
                url: "https://zed.dev/docs".into(),
            },
        ),
        MenuItem::action(localization::localized_str!("Zed Repository"), feedback::OpenZedRepo),
        MenuItem::action(
            localization::localized_str!("Zed Twitter"),
            super::OpenBrowser {
                url: "https://twitter.com/zeddotdev".into(),
            },
        ),
        MenuItem::action(
            localization::localized_str!("Join the Team"),
            super::OpenBrowser {
                url: "https://zed.dev/jobs".into(),
            },
        ),
    ]
}
// [OHOS PORT END]

pub fn app_menus(cx: &mut App) -> Vec<Menu> {
    let mut view_items = vec![
        MenuItem::action(
            localization::localized_str!("Zoom In"),
            zed_actions::IncreaseBufferFontSize { persist: false },
        ),
        MenuItem::action(
            localization::localized_str!("Zoom Out"),
            zed_actions::DecreaseBufferFontSize { persist: false },
        ),
        MenuItem::action(
            localization::localized_str!("Reset Zoom"),
            zed_actions::ResetBufferFontSize { persist: false },
        ),
        MenuItem::action(
            localization::localized_str!("Reset All Zoom"),
            zed_actions::ResetAllZoom { persist: false },
        ),
        MenuItem::separator(),
        MenuItem::action(localization::localized_str!("Toggle Left Dock"), workspace::ToggleLeftDock),
        MenuItem::action(localization::localized_str!("Toggle Right Dock"), workspace::ToggleRightDock),
        MenuItem::action(localization::localized_str!("Toggle Bottom Dock"), workspace::ToggleBottomDock),
        MenuItem::action(localization::localized_str!("Toggle All Docks"), workspace::ToggleAllDocks),
        MenuItem::submenu(Menu {
            name: localization::localized_str!("Editor Layout").into(),
            disabled: false,
            items: vec![
                MenuItem::action(localization::localized_str!("Split Up"), workspace::SplitUp::default()),
                MenuItem::action(localization::localized_str!("Split Down"), workspace::SplitDown::default()),
                MenuItem::action(localization::localized_str!("Split Left"), workspace::SplitLeft::default()),
                MenuItem::action(localization::localized_str!("Split Right"), workspace::SplitRight::default()),
            ],
        }),
        MenuItem::separator(),
        MenuItem::action(localization::localized_str!("Project Panel"), project_panel::ToggleFocus),
        MenuItem::action(localization::localized_str!("Outline Panel"), outline_panel::ToggleFocus),
        MenuItem::action(localization::localized_str!("Collab Panel"), collab_panel::ToggleFocus),
        MenuItem::action(localization::localized_str!("Terminal Panel"), terminal_panel::Toggle),
        MenuItem::action(localization::localized_str!("Debugger Panel"), debug_panel::ToggleFocus),
    ];

    if !DisableAiSettings::get_global(cx).disable_ai {
        view_items.push(MenuItem::action(localization::localized_str!("Agent Panel"), assistant::ToggleFocus));
    }

    view_items.extend([
        MenuItem::action(localization::localized_str!("Git Panel"), git_panel::ToggleFocus),
        MenuItem::separator(),
        MenuItem::action(localization::localized_str!("Diagnostics"), diagnostics::Deploy),
        MenuItem::separator(),
    ]);

    if ReleaseChannel::try_global(cx) == Some(ReleaseChannel::Dev) {
        view_items.push(MenuItem::action(
            localization::localized_str!("Toggle GPUI Inspector"),
            dev::ToggleInspector,
        ));
        view_items.push(MenuItem::separator());
    }

    // [OHOS PORT BEGIN] The platform draws the window controls itself, so the
    // Window menu has nothing to act on and is left out. To restore it, delete
    // this block and the `window_menu_items` chain below.
    #[cfg(target_env = "ohos")]
    let window_menu_items: Vec<Menu> = Vec::new();
    // [OHOS PORT END]
    // [OHOS PORT BEGIN] To restore the Window menu, delete this block.
    #[cfg(not(target_env = "ohos"))]
    let window_menu_items: Vec<Menu> = vec![Menu {
        name: application_menu_name("Window"),
        disabled: false,
        items: vec![
            MenuItem::action(localization::localized_str!("Minimize"), super::Minimize),
            MenuItem::action(localization::localized_str!("Zoom"), super::Zoom),
            MenuItem::separator(),
        ],
    }];
    // [OHOS PORT END]

    // [OHOS PORT BEGIN] Only the licence viewer stays: everything else in this
    // menu either opens an upstream web property or reports to a service
    // HiCodeer does not run. To restore the upstream menu, delete this block.
    #[cfg(target_env = "ohos")]
    let help_menu_items: Vec<MenuItem> = vec![MenuItem::action(
        localization::localized_str!("View Dependency Licenses"),
        zed_actions::OpenLicenses,
    )];
    // [OHOS PORT END]
    // [OHOS PORT BEGIN] To restore the upstream Help menu, delete this block.
    #[cfg(not(target_env = "ohos"))]
    let help_menu_items: Vec<MenuItem> = vec![
        MenuItem::action(
            localization::localized_str!("View Release Notes Locally"),
            auto_update_ui::ViewReleaseNotesLocally,
        ),
        MenuItem::action(localization::localized_str!("View Telemetry"), zed_actions::OpenTelemetryLog),
        MenuItem::action(localization::localized_str!("View Dependency Licenses"), zed_actions::OpenLicenses),
        MenuItem::action(localization::localized_str!("Show Welcome"), onboarding::ShowWelcome),
        MenuItem::separator(),
        MenuItem::action(localization::localized_str!("File Bug Report..."), zed_actions::feedback::FileBugReport),
        MenuItem::action(localization::localized_str!("Request Feature..."), zed_actions::feedback::RequestFeature),
        MenuItem::action(localization::localized_str!("Email Us..."), zed_actions::feedback::EmailZed),
    ]
    .into_iter()
    .chain(upstream_help_links())
    .collect();
    // [OHOS PORT END]

    vec![
        Menu {
            name: application_menu_name("Zed"),
            disabled: false,
            items: vec![
                MenuItem::action(localization::localized_str!("About Zed"), zed_actions::About),
                // [OHOS PORT BEGIN] HiCodeer does not auto-update, so the menu
                // entry that triggers the check is hidden. To restore the
                // upstream behaviour, delete this block.
                #[cfg(not(target_env = "ohos"))]
                MenuItem::action(localization::localized_str!("Check for Updates"), auto_update::Check),
                // [OHOS PORT END]
                MenuItem::separator(),
                MenuItem::submenu(Menu::new(localization::localized_str!("Settings")).items([
                    MenuItem::action(localization::localized_str!("Open Settings"), zed_actions::OpenSettings),
                    MenuItem::action(localization::localized_str!("Open Settings File"), super::OpenSettingsFile),
                    MenuItem::action(localization::localized_str!("Open Project Settings"), zed_actions::OpenProjectSettings),
                    MenuItem::action(localization::localized_str!("Open Project Settings File"), super::OpenProjectSettingsFile),
                    MenuItem::action(localization::localized_str!("Open Default Settings"), super::OpenDefaultSettings),
                    MenuItem::separator(),
                    MenuItem::action(localization::localized_str!("Open Keymap"), zed_actions::OpenKeymap),
                    MenuItem::action(localization::localized_str!("Open Keymap File"), zed_actions::OpenKeymapFile),
                    MenuItem::action(localization::localized_str!("Open Default Key Bindings"), zed_actions::OpenDefaultKeymap),
                    MenuItem::separator(),
                    MenuItem::action(
                        localization::localized_str!("Select Theme..."),
                        zed_actions::theme_selector::Toggle::default(),
                    ),
                    MenuItem::action(
                        localization::localized_str!("Select Icon Theme..."),
                        zed_actions::icon_theme_selector::Toggle::default(),
                    ),
                ])),
                MenuItem::separator(),
                #[cfg(target_os = "macos")]
                MenuItem::os_submenu("Services", gpui::SystemMenuType::Services),
                MenuItem::separator(),
                MenuItem::action(localization::localized_str!("Extensions"), zed_actions::Extensions::default()),
                #[cfg(not(target_os = "windows"))]
                MenuItem::action(localization::localized_str!("Install CLI"), install_cli::InstallCliBinary),
                MenuItem::separator(),
                #[cfg(target_os = "macos")]
                MenuItem::action(localization::localized_str!("Hide Zed"), super::Hide),
                #[cfg(target_os = "macos")]
                MenuItem::action(localization::localized_str!("Hide Others"), super::HideOthers),
                #[cfg(target_os = "macos")]
                MenuItem::action(localization::localized_str!("Show All"), super::ShowAll),
                MenuItem::separator(),
                MenuItem::action(localization::localized_str!("Quit Zed"), Quit),
            ],
        },
        Menu {
            name: application_menu_name("File"),
            disabled: false,
            items: vec![
                MenuItem::action(localization::localized_str!("New"), workspace::NewFile),
                MenuItem::action(localization::localized_str!("New Window"), workspace::NewWindow),
                MenuItem::separator(),
                #[cfg(not(target_os = "macos"))]
                MenuItem::action(localization::localized_str!("Open File..."), workspace::OpenFiles),
                MenuItem::action(
                    if cfg!(not(target_os = "macos")) {
                        localization::localized_str!("Open Folder...")
                    } else {
                        "Open…"
                    },
                    workspace::Open::default(),
                ),
                MenuItem::action(localization::localized_str!("Open Recent…"), zed_actions::OpenRecent::default()),
                MenuItem::action(localization::localized_str!("Open Remote…"), zed_actions::OpenRemote::default()),
                MenuItem::separator(),
                MenuItem::action(localization::localized_str!("Add Folder to Project…"), workspace::AddFolderToProject),
                MenuItem::separator(),
                MenuItem::action(localization::localized_str!("Save"), workspace::Save { save_intent: None }),
                MenuItem::action(localization::localized_str!("Save As…"), workspace::SaveAs),
                MenuItem::action(localization::localized_str!("Save All"), workspace::SaveAll { save_intent: None }),
                MenuItem::separator(),
                MenuItem::action(
                    localization::localized_str!("Close Editor"),
                    workspace::CloseActiveItem {
                        save_intent: None,
                        close_pinned: true,
                    },
                ),
                MenuItem::action(localization::localized_str!("Close Project"), workspace::CloseProject),
                MenuItem::action(localization::localized_str!("Close Window"), workspace::CloseWindow),
            ],
        },
        Menu {
            name: application_menu_name("Edit"),
            disabled: false,
            items: vec![
                MenuItem::os_action(localization::localized_str!("Undo"), editor::actions::Undo, OsAction::Undo),
                MenuItem::os_action(localization::localized_str!("Redo"), editor::actions::Redo, OsAction::Redo),
                MenuItem::separator(),
                MenuItem::os_action(localization::localized_str!("Cut"), editor::actions::Cut, OsAction::Cut),
                MenuItem::os_action(localization::localized_str!("Copy"), editor::actions::Copy, OsAction::Copy),
                MenuItem::action(localization::localized_str!("Copy and Trim"), editor::actions::CopyAndTrim),
                MenuItem::os_action(localization::localized_str!("Paste"), editor::actions::Paste, OsAction::Paste),
                MenuItem::separator(),
                MenuItem::action(localization::localized_str!("Find"), search::buffer_search::Deploy::find()),
                MenuItem::action(localization::localized_str!("Find in Project"), workspace::DeploySearch::default()),
                MenuItem::separator(),
                MenuItem::action(
                    localization::localized_str!("Toggle Line Comment"),
                    editor::actions::ToggleComments::default(),
                ),
            ],
        },
        Menu {
            name: application_menu_name("Selection"),
            disabled: false,
            items: vec![
                MenuItem::os_action(
                    localization::localized_str!("Select All"),
                    editor::actions::SelectAll,
                    OsAction::SelectAll,
                ),
                MenuItem::action(localization::localized_str!("Expand Selection"), editor::actions::SelectLargerSyntaxNode),
                MenuItem::action(localization::localized_str!("Shrink Selection"), editor::actions::SelectSmallerSyntaxNode),
                MenuItem::action(localization::localized_str!("Select Next Sibling"), editor::actions::SelectNextSyntaxNode),
                MenuItem::action(
                    localization::localized_str!("Select Previous Sibling"),
                    editor::actions::SelectPreviousSyntaxNode,
                ),
                MenuItem::separator(),
                MenuItem::action(
                    localization::localized_str!("Add Cursor Above"),
                    editor::actions::AddSelectionAbove {
                        skip_soft_wrap: true,
                    },
                ),
                MenuItem::action(
                    localization::localized_str!("Add Cursor Below"),
                    editor::actions::AddSelectionBelow {
                        skip_soft_wrap: true,
                    },
                ),
                MenuItem::action(
                    localization::localized_str!("Select Next Occurrence"),
                    editor::actions::SelectNext {
                        replace_newest: false,
                    },
                ),
                MenuItem::action(
                    localization::localized_str!("Select Previous Occurrence"),
                    editor::actions::SelectPrevious {
                        replace_newest: false,
                    },
                ),
                MenuItem::action(localization::localized_str!("Select All Occurrences"), editor::actions::SelectAllMatches),
                MenuItem::separator(),
                MenuItem::action(localization::localized_str!("Move Line Up"), editor::actions::MoveLineUp),
                MenuItem::action(localization::localized_str!("Move Line Down"), editor::actions::MoveLineDown),
                MenuItem::action(localization::localized_str!("Duplicate Selection"), editor::actions::DuplicateLineDown),
            ],
        },
        Menu {
            name: application_menu_name("View"),
            disabled: false,
            items: view_items,
        },
        Menu {
            name: application_menu_name("Go"),
            disabled: false,
            items: vec![
                MenuItem::action(localization::localized_str!("Back"), workspace::GoBack),
                MenuItem::action(localization::localized_str!("Forward"), workspace::GoForward),
                MenuItem::separator(),
                MenuItem::action(localization::localized_str!("Command Palette..."), zed_actions::command_palette::Toggle),
                MenuItem::separator(),
                MenuItem::action(localization::localized_str!("Go to File..."), workspace::ToggleFileFinder::default()),
                // MenuItem::action("Go to Symbol in Project", project_symbols::Toggle),
                MenuItem::action(
                    localization::localized_str!("Go to Symbol in Editor..."),
                    zed_actions::outline::ToggleOutline,
                ),
                MenuItem::action(localization::localized_str!("Go to Line/Column..."), editor::actions::ToggleGoToLine),
                MenuItem::separator(),
                MenuItem::action(
                    localization::localized_str!("Go to Definition"),
                    editor::actions::GoToDefinition::default(),
                ),
                MenuItem::action(
                    localization::localized_str!("Go to Declaration"),
                    editor::actions::GoToDeclaration::default(),
                ),
                MenuItem::action(
                    localization::localized_str!("Go to Type Definition"),
                    editor::actions::GoToTypeDefinition::default(),
                ),
                MenuItem::action(
                    localization::localized_str!("Find All References"),
                    editor::actions::FindAllReferences::default(),
                ),
                MenuItem::action(localization::localized_str!("Show Incoming Calls"), call_hierarchy::ShowIncomingCalls),
                MenuItem::action(localization::localized_str!("Show Outgoing Calls"), call_hierarchy::ShowOutgoingCalls),
                MenuItem::separator(),
                MenuItem::action(localization::localized_str!("Next Problem"), editor::actions::GoToDiagnostic::default()),
                MenuItem::action(
                    localization::localized_str!("Previous Problem"),
                    editor::actions::GoToPreviousDiagnostic::default(),
                ),
            ],
        },
        Menu {
            name: application_menu_name("Run"),
            disabled: false,
            items: vec![
                MenuItem::action(
                    localization::localized_str!("Spawn Task"),
                    zed_actions::Spawn::ViaModal {
                        reveal_target: None,
                    },
                ),
                MenuItem::action(localization::localized_str!("Start Debugger"), debugger_ui::Start),
                MenuItem::separator(),
                MenuItem::action(localization::localized_str!("Edit tasks.json…"), zed_actions::OpenProjectTasks),
                MenuItem::action(localization::localized_str!("Edit debug.json…"), zed_actions::OpenProjectDebugTasks),
                MenuItem::separator(),
                MenuItem::action(localization::localized_str!("Continue"), debugger_ui::Continue),
                MenuItem::action(localization::localized_str!("Step Over"), debugger_ui::StepOver),
                MenuItem::action(localization::localized_str!("Step Into"), debugger_ui::StepInto),
                MenuItem::action(localization::localized_str!("Step Out"), debugger_ui::StepOut),
                MenuItem::separator(),
                MenuItem::action(localization::localized_str!("Toggle Breakpoint"), editor::actions::ToggleBreakpoint),
                MenuItem::action(localization::localized_str!("Edit Breakpoint"), editor::actions::EditLogBreakpoint),
                MenuItem::action(localization::localized_str!("Clear All Breakpoints"), debugger_ui::ClearAllBreakpoints),
            ],
        },
    ]
    .into_iter()
    .chain(window_menu_items)
    .chain(vec![Menu {
        name: application_menu_name("Help"),
        disabled: false,
        items: help_menu_items,
    }])
    .collect()
}
