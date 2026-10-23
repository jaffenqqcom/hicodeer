use gpui::{App, Context};
use itertools::Itertools as _;
use settings::{
    LanguageSettingsContent, SemanticTokens,
    SettingsContent,
};
use std::sync::{Arc, OnceLock};
use strum::{EnumMessage, IntoDiscriminant as _, VariantArray};
use theme::SystemAppearance;
use ui::IntoElement;

use crate::{
    ActionLink, DynamicItem, PROJECT, SettingField, SettingItem, SettingsFieldMetadata,
    SettingsPage, SettingsPageItem, SubPageLink, USER, active_language, all_language_names,
};

const DEFAULT_STRING: String = String::new();
/// A default empty string reference. Useful in `pick` functions for cases either in dynamic item fields, or when dealing with `settings::Maybe`
/// to avoid the "NO DEFAULT" case.
const DEFAULT_EMPTY_STRING: Option<&String> = Some(&DEFAULT_STRING);

macro_rules! concat_sections {
    (@vec, $($arr:expr),+ $(,)?) => {{
        let total_len = 0_usize $(+ $arr.len())+;
        let mut out = Vec::with_capacity(total_len);

        $(
            out.extend($arr);
        )+

        out
    }};

    ($($arr:expr),+ $(,)?) => {{
        let total_len = 0_usize $(+ $arr.len())+;

        let mut out: Box<[std::mem::MaybeUninit<_>]> = Box::new_uninit_slice(total_len);

        let mut index = 0usize;
        $(
            let array = $arr;
            for item in array {
                out[index].write(item);
                index += 1;
            }
        )+

        debug_assert_eq!(index, total_len);

        // SAFETY: we wrote exactly `total_len` elements.
        unsafe { out.assume_init() }
    }};
}

pub(crate) fn settings_data(cx: &App) -> Vec<SettingsPage> {
    vec![
        general_page(cx),
        appearance_page(),
        keymap_page(),
        editor_page(),
        languages_and_tools_page(cx),
        search_and_files_page(),
        window_and_layout_page(),
        panels_page(),
        terminal_page(),
        version_control_page(),
        network_page(),
        developer_page(cx),
    ]
}

fn developer_page(cx: &App) -> SettingsPage {
    use feature_flags::FeatureFlagAppExt as _;

    let mut items: Vec<SettingsPageItem> = Vec::new();

    // Feature flag overrides are a staff-only affordance, so only surface the section when the overrides are enabled.
    if cx.feature_flag_overrides_enabled() {
        items.push(SettingsPageItem::SectionHeader(localization::localized_str!("Feature Flags")));
        items.push(SettingsPageItem::SubPageLink(SubPageLink {
            title: localization::localized_str!("Feature Flags").into(),
            r#type: Default::default(),
            description: None,
            search_aliases: &[],
            json_path: Some("feature_flags"),
            in_json: true,
            files: USER,
            render: crate::pages::render_feature_flags_page,
        }));
    }

    items.push(SettingsPageItem::SectionHeader(localization::localized_str!("Instrumentation")));
    items.push(SettingsPageItem::SettingItem(SettingItem {
        title: localization::localized_str!("Performance Profiler"),
        description: localization::localized_str!("Collect timing data for foreground and background executor tasks so they can be inspected via `zed: open performance profiler`. May lead to increased memory usage."),
        field: Box::new(SettingField {
            organization_override: None,
            json_path: Some("instrumentation.performance_profiler.enabled"),
            pick: |settings_content| {
                settings_content
                    .instrumentation
                    .as_ref()
                    .and_then(|i| i.performance_profiler.as_ref())
                    .and_then(|p| p.enabled.as_ref())
            },
            write: |settings_content, value, _| {
                settings_content
                    .instrumentation
                    .get_or_insert_default()
                    .performance_profiler
                    .get_or_insert_default()
                    .enabled = value;
            },
        }),
        metadata: None,
        files: USER,
    }));

    SettingsPage {
        title: localization::localized_str!("Developer"),
        items: items.into_boxed_slice(),
    }
}

fn general_page(cx: &App) -> SettingsPage {
    fn general_settings_section(_cx: &App) -> Vec<SettingsPageItem> {
        vec![
            crate::components::locale_setting_item(),
            SettingsPageItem::SectionHeader(localization::localized_str!("General Settings")),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Accessible Mode"),
                description: localization::localized_str!("Optimize HiCodeer's interface for assistive technology such as screen readers. When enabled, otherwise-collapsed controls stay expanded and keyboard-reachable."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("accessible_mode"),
                    pick: |settings_content| settings_content.workspace.accessible_mode.as_ref(),
                    write: |settings_content, value, _| {
                        settings_content.workspace.accessible_mode = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("When Closing With No Tabs"),
                description: localization::localized_str!("What to do when using the 'close active item' action with no tabs."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("when_closing_with_no_tabs"),
                    pick: |settings_content| {
                        settings_content
                            .workspace
                            .when_closing_with_no_tabs
                            .as_ref()
                    },
                    write: |settings_content, value, _| {
                        settings_content.workspace.when_closing_with_no_tabs = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("On New Window"),
                description: localization::localized_str!("What to show when opening a new window."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("on_new_window"),
                    pick: |settings_content| settings_content.workspace.on_new_window.as_ref(),
                    write: |settings_content, value, _| {
                        settings_content.workspace.on_new_window = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("On Last Window Closed"),
                description: localization::localized_str!("What to do when the last window is closed."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("on_last_window_closed"),
                    pick: |settings_content| {
                        settings_content.workspace.on_last_window_closed.as_ref()
                    },
                    write: |settings_content, value, _| {
                        settings_content.workspace.on_last_window_closed = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Use System Path Prompts"),
                description: localization::localized_str!("Use native OS dialogs for 'Open' and 'Save As'."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("use_system_path_prompts"),
                    pick: |settings_content| {
                        settings_content.workspace.use_system_path_prompts.as_ref()
                    },
                    write: |settings_content, value, _| {
                        settings_content.workspace.use_system_path_prompts = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Use System Prompts"),
                description: localization::localized_str!("Use native OS dialogs for confirmations."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("use_system_prompts"),
                    pick: |settings_content| settings_content.workspace.use_system_prompts.as_ref(),
                    write: |settings_content, value, _| {
                        settings_content.workspace.use_system_prompts = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Redact Private Values"),
                description: localization::localized_str!("Hide the values of variables in private files."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("redact_private_values"),
                    pick: |settings_content| settings_content.editor.redact_private_values.as_ref(),
                    write: |settings_content, value, _| {
                        settings_content.editor.redact_private_values = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Private Files"),
                description: localization::localized_str!("Globs to match against file paths to determine if a file is private."),
                field: Box::new(
                    SettingField {
                        organization_override: None,
                        json_path: Some("private_files"),
                        pick: |settings_content| {
                            settings_content.project.worktree.private_files.as_ref()
                        },
                        write: |settings_content, value, _| {
                            settings_content.project.worktree.private_files = value;
                        },
                    }
                    .unimplemented(),
                ),
                metadata: None,
                files: USER,
            }),
            // [OHOS PORT BEGIN] There is no command line launcher on this
            // platform, so there is no behavior to configure for it.
            #[cfg(not(target_env = "ohos"))]
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("CLI Default Open Behavior"),
                description: localization::localized_str!("How `zed <path>` opens directories when no flag is specified."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("cli_default_open_behavior"),
                    pick: |settings_content| {
                        settings_content
                            .workspace
                            .cli_default_open_behavior
                            .as_ref()
                    },
                    write: |settings_content, value, _| {
                        settings_content.workspace.cli_default_open_behavior = value;
                    },
                }),
                metadata: Some(Box::new(SettingsFieldMetadata {
                    should_do_titlecase: Some(false),
                    ..Default::default()
                })),
                files: USER,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Reveal If Open"),
                description: localization::localized_str!("when enabled, zed will prefer already-open buffers."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("reveal_if_open"),
                    pick: |settings_content| settings_content.workspace.reveal_if_open.as_ref(),
                    write: |settings_content, value, _| {
                        settings_content.workspace.reveal_if_open = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Default Open Behavior"),
                description: localization::localized_str!("How projects open from the UI by default."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("default_open_behavior"),
                    pick: |settings_content| {
                        settings_content.workspace.default_open_behavior.as_ref()
                    },
                    write: |settings_content, value, _| {
                        settings_content.workspace.default_open_behavior = value;
                    },
                }),
                metadata: Some(Box::new(SettingsFieldMetadata {
                    should_do_titlecase: Some(false),
                    ..Default::default()
                })),
                files: USER,
            }),
        ]
    }
    fn security_section() -> [SettingsPageItem; 2] {
        [
            SettingsPageItem::SectionHeader(localization::localized_str!("Security")),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Trust All Projects By Default"),
                description: localization::localized_str!("When opening HiCodeer, avoid Restricted Mode by auto-trusting all projects, enabling use of all features without having to give permission to each new project."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("session.trust_all_projects"),
                    pick: |settings_content| {
                        settings_content
                            .session
                            .as_ref()
                            .and_then(|session| session.trust_all_worktrees.as_ref())
                    },
                    write: |settings_content, value, _| {
                        settings_content
                            .session
                            .get_or_insert_default()
                            .trust_all_worktrees = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
        ]
    }

    fn workspace_restoration_section() -> [SettingsPageItem; 3] {
        [
            SettingsPageItem::SectionHeader(localization::localized_str!("Workspace Restoration")),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Restore Unsaved Buffers"),
                description: localization::localized_str!("Whether or not to restore unsaved buffers on restart."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("session.restore_unsaved_buffers"),
                    pick: |settings_content| {
                        settings_content
                            .session
                            .as_ref()
                            .and_then(|session| session.restore_unsaved_buffers.as_ref())
                    },
                    write: |settings_content, value, _| {
                        settings_content
                            .session
                            .get_or_insert_default()
                            .restore_unsaved_buffers = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Restore On Startup"),
                description: localization::localized_str!("What to restore from the previous session when opening HiCodeer."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("restore_on_startup"),
                    pick: |settings_content| settings_content.workspace.restore_on_startup.as_ref(),
                    write: |settings_content, value, _| {
                        settings_content.workspace.restore_on_startup = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
        ]
    }

    fn scoped_settings_section() -> [SettingsPageItem; 3] {
        [
            SettingsPageItem::SectionHeader(localization::localized_str!("Scoped Settings")),
            SettingsPageItem::SettingItem(SettingItem {
                files: USER,
                title: localization::localized_str!("Preview Channel"),
                description: localization::localized_str!("Which settings should be activated only in Preview build of HiCodeer."),
                field: Box::new(
                    SettingField {
                        organization_override: None,
                        json_path: Some("preview_channel_settings"),
                        pick: |settings_content| Some(settings_content),
                        write: |_settings_content, _value, _| {},
                    }
                    .unimplemented(),
                ),
                metadata: None,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                files: USER,
                title: localization::localized_str!("Settings Profiles"),
                description: localization::localized_str!("Any number of settings profiles that are temporarily applied on top of your existing user settings."),
                field: Box::new(
                    SettingField {
                        organization_override: None,
                        json_path: Some("settings_profiles"),
                        pick: |settings_content| Some(settings_content),
                        write: |_settings_content, _value, _| {},
                    }
                    .unimplemented(),
                ),
                metadata: None,
            }),
        ]
    }

    // [OHOS PORT BEGIN] HiCodeer reports no telemetry, so the privacy switches
    // have nothing to control.
    #[cfg(not(target_env = "ohos"))]
    fn privacy_section() -> [SettingsPageItem; 4] {
        [
            SettingsPageItem::SectionHeader(localization::localized_str!("Privacy")),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Telemetry Diagnostics"),
                description: localization::localized_str!("Send debug information like crash reports."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("telemetry.diagnostics"),
                    pick: |settings_content| {
                        settings_content
                            .telemetry
                            .as_ref()
                            .and_then(|telemetry| telemetry.diagnostics.as_ref())
                    },
                    write: |settings_content, value, _| {
                        settings_content
                            .telemetry
                            .get_or_insert_default()
                            .diagnostics = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Telemetry Metrics"),
                description: localization::localized_str!("Send anonymized usage data like what languages you're using HiCodeer with."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("telemetry.metrics"),
                    pick: |settings_content| {
                        settings_content
                            .telemetry
                            .as_ref()
                            .and_then(|telemetry| telemetry.metrics.as_ref())
                    },
                    write: |settings_content, value, _| {
                        settings_content.telemetry.get_or_insert_default().metrics = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Anthropic Data Retention"),
                description: localization::localized_str!("Allow sending requests to Anthropic models that cannot be offered with Zero Data Retention."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("telemetry.anthropic_retention"),
                    pick: |settings_content| {
                        settings_content
                            .telemetry
                            .as_ref()
                            .and_then(|telemetry| telemetry.anthropic_retention.as_ref())
                    },
                    write: |settings_content, value, _| {
                        settings_content
                            .telemetry
                            .get_or_insert_default()
                            .anthropic_retention = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
        ]
    }
    // [OHOS PORT END]

    // [OHOS PORT BEGIN] HiCodeer has no update feed, so there is nothing to
    // configure here.
    #[cfg(not(target_env = "ohos"))]
    fn auto_update_section() -> [SettingsPageItem; 2] {
        [
            SettingsPageItem::SectionHeader(localization::localized_str!("Auto Update")),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Auto Update"),
                description: localization::localized_str!("Whether or not to automatically check for updates."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("auto_update"),
                    pick: |settings_content| settings_content.auto_update.as_ref(),
                    write: |settings_content, value, _| {
                        settings_content.auto_update = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
        ]
    }
    // [OHOS PORT END]

    fn extensions_section() -> [SettingsPageItem; 2] {
        [
            SettingsPageItem::SectionHeader("Extensions"),
            SettingsPageItem::SettingItem(SettingItem {
                title: "Suggest Extensions",
                description: "Whether to suggest installing extensions based on the files you open.",
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("suggest_extensions"),
                    pick: |settings_content| Some(&settings_content.extension.suggest_extensions),
                    write: |settings_content, value, _| {
                        settings_content.extension.suggest_extensions = value.unwrap_or(true);
                    },
                }),
                metadata: None,
                files: USER,
            }),
        ]
    }

    let mut items = concat_sections!(
        @vec,
        general_settings_section(cx),
        security_section(),
        workspace_restoration_section(),
        scoped_settings_section(),
    );
    // [OHOS PORT BEGIN] No telemetry and no update feed means neither section
    // has anything to show.
    #[cfg(not(target_env = "ohos"))]
    {
        items.extend(privacy_section());
        items.extend(auto_update_section());
    }
    // [OHOS PORT END]
    items.extend(extensions_section());

    SettingsPage {
        title: localization::localized_str!("General"),
        items: items.into(),
    }
}

fn appearance_page() -> SettingsPage {
    fn theme_section() -> [SettingsPageItem; 3] {
        [
            SettingsPageItem::SectionHeader(localization::localized_str!("Theme")),
            SettingsPageItem::DynamicItem(DynamicItem {
                discriminant: SettingItem {
                    files: USER,
                    title: localization::localized_str!("Theme Mode"),
                    description: localization::localized_str!("Choose a static, fixed theme or dynamically select themes based on appearance and light/dark modes."),
                    field: Box::new(SettingField {
                        organization_override: None,
                        json_path: Some("theme$"),
                        pick: |settings_content| {
                            Some(&dynamic_variants::<settings::ThemeSelection>()[
                                settings_content
                                    .theme
                                    .theme
                                    .as_ref()?
                                    .discriminant() as usize])
                        },
                        write: |settings_content, value, app: &App| {
                            let Some(value) = value else {
                                settings_content.theme.theme = None;
                                return;
                            };
                            let settings_value = settings_content.theme.theme.get_or_insert_default();
                            *settings_value = match value {
                                settings::ThemeSelectionDiscriminants::Static => {
                                    let name = match settings_value {
                                        settings::ThemeSelection::Static(_) => return,
                                        settings::ThemeSelection::Dynamic { mode, light, dark } => {
                                            match mode {
                                                theme_settings::ThemeAppearanceMode::Light => light.clone(),
                                                theme_settings::ThemeAppearanceMode::Dark => dark.clone(),
                                                theme_settings::ThemeAppearanceMode::System => {
                                                    if SystemAppearance::global(app).is_light() {
                                                        light.clone()
                                                    } else {
                                                        dark.clone()
                                                    }
                                                }
                                            }
                                        },
                                    };
                                    settings::ThemeSelection::Static(name)
                                },
                                settings::ThemeSelectionDiscriminants::Dynamic => {
                                    let static_name = match settings_value {
                                        settings::ThemeSelection::Static(theme_name) => theme_name.clone(),
                                        settings::ThemeSelection::Dynamic {..} => return,
                                    };

                                    settings::ThemeSelection::Dynamic {
                                        mode: settings::ThemeAppearanceMode::System,
                                        light: static_name.clone(),
                                        dark: static_name,
                                    }
                                },
                            };
                        },
                    }),
                    metadata: None,
                },
                pick_discriminant: |settings_content| {
                    Some(settings_content.theme.theme.as_ref()?.discriminant() as usize)
                },
                fields: dynamic_variants::<settings::ThemeSelection>().into_iter().map(|variant| {
                    match variant {
                        settings::ThemeSelectionDiscriminants::Static => vec![
                            SettingItem {
                                files: USER,
                                title: localization::localized_str!("Theme Name"),
                                description: localization::localized_str!("The name of your selected theme."),
                                field: Box::new(SettingField {
                                    organization_override: None,
                                    json_path: Some("theme"),
                                    pick: |settings_content| {
                                        match settings_content.theme.theme.as_ref() {
                                            Some(settings::ThemeSelection::Static(name)) => Some(name),
                                            _ => None
                                        }
                                    },
                                    write: |settings_content, value, _| {
                                        let Some(value) = value else {
                                            return;
                                        };
                                        match settings_content
                                            .theme
                                            .theme.get_or_insert_default() {
                                                settings::ThemeSelection::Static(theme_name) => *theme_name = value,
                                                _ => return
                                            }
                                    },
                                }),
                                metadata: None,
                            }
                        ],
                        settings::ThemeSelectionDiscriminants::Dynamic => vec![
                            SettingItem {
                                files: USER,
                                title: localization::localized_str!("Mode"),
                                description: localization::localized_str!("Choose whether to use the selected light or dark theme or to follow your OS appearance configuration."),
                                field: Box::new(SettingField {
                                    organization_override: None,
                                    json_path: Some("theme.mode"),
                                    pick: |settings_content| {
                                        match settings_content.theme.theme.as_ref() {
                                            Some(settings::ThemeSelection::Dynamic { mode, ..}) => Some(mode),
                                            _ => None
                                        }
                                    },
                                    write: |settings_content, value, _| {
                                        let Some(value) = value else {
                                            return;
                                        };
                                        match settings_content
                                            .theme
                                            .theme.get_or_insert_default() {
                                                settings::ThemeSelection::Dynamic{ mode, ..} => *mode = value,
                                                _ => return
                                            }
                                    },
                                }),
                                metadata: None,
                            },
                            SettingItem {
                                files: USER,
                                title: localization::localized_str!("Light Theme"),
                                description: localization::localized_str!("The theme to use when mode is set to light, or when mode is set to system and it is in light mode."),
                                field: Box::new(SettingField {
                                    organization_override: None,
                                    json_path: Some("theme.light"),
                                    pick: |settings_content| {
                                        match settings_content.theme.theme.as_ref() {
                                            Some(settings::ThemeSelection::Dynamic { light, ..}) => Some(light),
                                            _ => None
                                        }
                                    },
                                    write: |settings_content, value, _| {
                                        let Some(value) = value else {
                                            return;
                                        };
                                        match settings_content
                                            .theme
                                            .theme.get_or_insert_default() {
                                                settings::ThemeSelection::Dynamic{ light, ..} => *light = value,
                                                _ => return
                                            }
                                    },
                                }),
                                metadata: None,
                            },
                            SettingItem {
                                files: USER,
                                title: localization::localized_str!("Dark Theme"),
                                description: localization::localized_str!("The theme to use when mode is set to dark, or when mode is set to system and it is in dark mode."),
                                field: Box::new(SettingField {
                                    organization_override: None,
                                    json_path: Some("theme.dark"),
                                    pick: |settings_content| {
                                        match settings_content.theme.theme.as_ref() {
                                            Some(settings::ThemeSelection::Dynamic { dark, ..}) => Some(dark),
                                            _ => None
                                        }
                                    },
                                    write: |settings_content, value, _| {
                                        let Some(value) = value else {
                                            return;
                                        };
                                        match settings_content
                                            .theme
                                            .theme.get_or_insert_default() {
                                                settings::ThemeSelection::Dynamic{ dark, ..} => *dark = value,
                                                _ => return
                                            }
                                    },
                                }),
                                metadata: None,
                            }
                        ],
                    }
                }).collect(),
            }),
            SettingsPageItem::DynamicItem(DynamicItem {
                discriminant: SettingItem {
                    files: USER,
                    title: localization::localized_str!("Icon Theme"),
                    description: localization::localized_str!("The custom set of icons HiCodeer will associate with files and directories."),
                    field: Box::new(SettingField {
                        organization_override: None,
                        json_path: Some("icon_theme$"),
                        pick: |settings_content| {
                            Some(&dynamic_variants::<settings::IconThemeSelection>()[
                                settings_content
                                    .theme
                                    .icon_theme
                                    .as_ref()?
                                    .discriminant() as usize])
                        },
                        write: |settings_content, value, app| {
                            let Some(value) = value else {
                                settings_content.theme.icon_theme = None;
                                return;
                            };
                            let settings_value = settings_content.theme.icon_theme.get_or_insert_with(|| {
                                settings::IconThemeSelection::Static(settings::IconThemeName(theme::default_icon_theme().name.clone().into()))
                            });
                            *settings_value = match value {
                                settings::IconThemeSelectionDiscriminants::Static => {
                                    let name = match settings_value {
                                        settings::IconThemeSelection::Static(_) => return,
                                        settings::IconThemeSelection::Dynamic { mode, light, dark } => {
                                            match mode {
                                                theme_settings::ThemeAppearanceMode::Light => light.clone(),
                                                theme_settings::ThemeAppearanceMode::Dark => dark.clone(),
                                                theme_settings::ThemeAppearanceMode::System => {
                                                    if SystemAppearance::global(app).is_light() {
                                                        light.clone()
                                                    } else {
                                                        dark.clone()
                                                    }
                                                }
                                            }
                                        },
                                    };
                                    settings::IconThemeSelection::Static(name)
                                },
                                settings::IconThemeSelectionDiscriminants::Dynamic => {
                                    let static_name = match settings_value {
                                        settings::IconThemeSelection::Static(theme_name) => theme_name.clone(),
                                        settings::IconThemeSelection::Dynamic {..} => return,
                                    };

                                    settings::IconThemeSelection::Dynamic {
                                        mode: settings::ThemeAppearanceMode::System,
                                        light: static_name.clone(),
                                        dark: static_name,
                                    }
                                },
                            };
                        },
                    }),
                    metadata: None,
                },
                pick_discriminant: |settings_content| {
                    Some(settings_content.theme.icon_theme.as_ref()?.discriminant() as usize)
                },
                fields: dynamic_variants::<settings::IconThemeSelection>().into_iter().map(|variant| {
                    match variant {
                        settings::IconThemeSelectionDiscriminants::Static => vec![
                            SettingItem {
                                files: USER,
                                title: localization::localized_str!("Icon Theme Name"),
                                description: localization::localized_str!("The name of your selected icon theme."),
                                field: Box::new(SettingField {
                                    organization_override: None,
                                    json_path: Some("icon_theme$string"),
                                    pick: |settings_content| {
                                        match settings_content.theme.icon_theme.as_ref() {
                                            Some(settings::IconThemeSelection::Static(name)) => Some(name),
                                            _ => None
                                        }
                                    },
                                    write: |settings_content, value, _| {
                                        let Some(value) = value else {
                                            return;
                                        };
                                        match settings_content
                                            .theme
                                            .icon_theme.as_mut() {
                                                Some(settings::IconThemeSelection::Static(theme_name)) => *theme_name = value,
                                                _ => return
                                            }
                                    },
                                }),
                                metadata: None,
                            }
                        ],
                        settings::IconThemeSelectionDiscriminants::Dynamic => vec![
                            SettingItem {
                                files: USER,
                                title: localization::localized_str!("Mode"),
                                description: localization::localized_str!("Choose whether to use the selected light or dark icon theme or to follow your OS appearance configuration."),
                                field: Box::new(SettingField {
                                    organization_override: None,
                                    json_path: Some("icon_theme"),
                                    pick: |settings_content| {
                                        match settings_content.theme.icon_theme.as_ref() {
                                            Some(settings::IconThemeSelection::Dynamic { mode, ..}) => Some(mode),
                                            _ => None
                                        }
                                    },
                                    write: |settings_content, value, _| {
                                        let Some(value) = value else {
                                            return;
                                        };
                                        match settings_content
                                            .theme
                                            .icon_theme.as_mut() {
                                                Some(settings::IconThemeSelection::Dynamic{ mode, ..}) => *mode = value,
                                                _ => return
                                            }
                                    },
                                }),
                                metadata: None,
                            },
                            SettingItem {
                                files: USER,
                                title: localization::localized_str!("Light Icon Theme"),
                                description: localization::localized_str!("The icon theme to use when mode is set to light, or when mode is set to system and it is in light mode."),
                                field: Box::new(SettingField {
                                    organization_override: None,
                                    json_path: Some("icon_theme.light"),
                                    pick: |settings_content| {
                                        match settings_content.theme.icon_theme.as_ref() {
                                            Some(settings::IconThemeSelection::Dynamic { light, ..}) => Some(light),
                                            _ => None
                                        }
                                    },
                                    write: |settings_content, value, _| {
                                        let Some(value) = value else {
                                            return;
                                        };
                                        match settings_content
                                            .theme
                                            .icon_theme.as_mut() {
                                                Some(settings::IconThemeSelection::Dynamic{ light, ..}) => *light = value,
                                                _ => return
                                            }
                                    },
                                }),
                                metadata: None,
                            },
                            SettingItem {
                                files: USER,
                                title: localization::localized_str!("Dark Icon Theme"),
                                description: localization::localized_str!("The icon theme to use when mode is set to dark, or when mode is set to system and it is in dark mode."),
                                field: Box::new(SettingField {
                                    organization_override: None,
                                    json_path: Some("icon_theme.dark"),
                                    pick: |settings_content| {
                                        match settings_content.theme.icon_theme.as_ref() {
                                            Some(settings::IconThemeSelection::Dynamic { dark, ..}) => Some(dark),
                                            _ => None
                                        }
                                    },
                                    write: |settings_content, value, _| {
                                        let Some(value) = value else {
                                            return;
                                        };
                                        match settings_content
                                            .theme
                                            .icon_theme.as_mut() {
                                                Some(settings::IconThemeSelection::Dynamic{ dark, ..}) => *dark = value,
                                                _ => return
                                            }
                                    },
                                }),
                                metadata: None,
                            }
                        ],
                    }
                }).collect(),
            }),
        ]
    }

    fn buffer_font_section() -> [SettingsPageItem; 7] {
        [
            SettingsPageItem::SectionHeader(localization::localized_str!("Buffer Font")),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Font Family"),
                description: localization::localized_str!("Font family for editor text."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("buffer_font_family"),
                    pick: |settings_content| settings_content.theme.buffer_font_family.as_ref(),
                    write: |settings_content, value, _| {
                        settings_content.theme.buffer_font_family = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Font Size"),
                description: localization::localized_str!("Font size for editor text."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("buffer_font_size"),
                    pick: |settings_content| settings_content.theme.buffer_font_size.as_ref(),
                    write: |settings_content, value, _| {
                        settings_content.theme.buffer_font_size = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Font Weight"),
                description: localization::localized_str!("Font weight for editor text (100-900)."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("buffer_font_weight"),
                    pick: |settings_content| settings_content.theme.buffer_font_weight.as_ref(),
                    write: |settings_content, value, _| {
                        settings_content.theme.buffer_font_weight = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
            SettingsPageItem::DynamicItem(DynamicItem {
                discriminant: SettingItem {
                    files: USER,
                    title: localization::localized_str!("Line Height"),
                    description: localization::localized_str!("Line height for editor text."),
                    field: Box::new(SettingField {
                        organization_override: None,
                        json_path: Some("buffer_line_height$"),
                        pick: |settings_content| {
                            Some(
                                &dynamic_variants::<settings::BufferLineHeight>()[settings_content
                                    .theme
                                    .buffer_line_height
                                    .as_ref()?
                                    .discriminant()
                                    as usize],
                            )
                        },
                        write: |settings_content, value, _| {
                            let Some(value) = value else {
                                settings_content.theme.buffer_line_height = None;
                                return;
                            };
                            let settings_value = settings_content
                                .theme
                                .buffer_line_height
                                .get_or_insert_with(|| settings::BufferLineHeight::default());
                            *settings_value = match value {
                                settings::BufferLineHeightDiscriminants::Comfortable => {
                                    settings::BufferLineHeight::Comfortable
                                }
                                settings::BufferLineHeightDiscriminants::Standard => {
                                    settings::BufferLineHeight::Standard
                                }
                                settings::BufferLineHeightDiscriminants::Custom => {
                                    let custom_value =
                                        theme_settings::buffer_line_height_from_settings(
                                            *settings_value,
                                        )
                                        .value();
                                    settings::BufferLineHeight::Custom(custom_value)
                                }
                            };
                        },
                    }),
                    metadata: None,
                },
                pick_discriminant: |settings_content| {
                    Some(
                        settings_content
                            .theme
                            .buffer_line_height
                            .as_ref()?
                            .discriminant() as usize,
                    )
                },
                fields: dynamic_variants::<settings::BufferLineHeight>()
                    .into_iter()
                    .map(|variant| match variant {
                        settings::BufferLineHeightDiscriminants::Comfortable => vec![],
                        settings::BufferLineHeightDiscriminants::Standard => vec![],
                        settings::BufferLineHeightDiscriminants::Custom => vec![SettingItem {
                            files: USER,
                            title: localization::localized_str!("Custom Line Height"),
                            description: localization::localized_str!("Custom line height value (must be at least 1.0)."),
                            field: Box::new(SettingField {
                                organization_override: None,
                                json_path: Some("buffer_line_height"),
                                pick: |settings_content| match settings_content
                                    .theme
                                    .buffer_line_height
                                    .as_ref()
                                {
                                    Some(settings::BufferLineHeight::Custom(value)) => Some(value),
                                    _ => None,
                                },
                                write: |settings_content, value, _| {
                                    let Some(value) = value else {
                                        return;
                                    };
                                    match settings_content.theme.buffer_line_height.as_mut() {
                                        Some(settings::BufferLineHeight::Custom(line_height)) => {
                                            *line_height = f32::max(value, 1.0)
                                        }
                                        _ => return,
                                    }
                                },
                            }),
                            metadata: None,
                        }],
                    })
                    .collect(),
            }),
            SettingsPageItem::SettingItem(SettingItem {
                files: USER,
                title: localization::localized_str!("Font Features"),
                description: localization::localized_str!("The OpenType features to enable for rendering in text buffers."),
                field: Box::new(
                    SettingField {
                        organization_override: None,
                        json_path: Some("buffer_font_features"),
                        pick: |settings_content| {
                            settings_content.theme.buffer_font_features.as_ref()
                        },
                        write: |settings_content, value, _| {
                            settings_content.theme.buffer_font_features = value;
                        },
                    }
                    .unimplemented(),
                ),
                metadata: None,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                files: USER,
                title: localization::localized_str!("Font Fallbacks"),
                description: localization::localized_str!("The font fallbacks to use for rendering in text buffers."),
                field: Box::new(
                    SettingField {
                        organization_override: None,
                        json_path: Some("buffer_font_fallbacks"),
                        pick: |settings_content| {
                            settings_content.theme.buffer_font_fallbacks.as_ref()
                        },
                        write: |settings_content, value, _| {
                            settings_content.theme.buffer_font_fallbacks = value;
                        },
                    }
                    .unimplemented(),
                ),
                metadata: None,
            }),
        ]
    }

    fn ui_font_section() -> [SettingsPageItem; 6] {
        [
            SettingsPageItem::SectionHeader(localization::localized_str!("UI Font")),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Font Family"),
                description: localization::localized_str!("Font family for UI elements."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("ui_font_family"),
                    pick: |settings_content| settings_content.theme.ui_font_family.as_ref(),
                    write: |settings_content, value, _| {
                        settings_content.theme.ui_font_family = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Font Size"),
                description: localization::localized_str!("Font size for UI elements."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("ui_font_size"),
                    pick: |settings_content| settings_content.theme.ui_font_size.as_ref(),
                    write: |settings_content, value, _| {
                        settings_content.theme.ui_font_size = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Font Weight"),
                description: localization::localized_str!("Font weight for UI elements (100-900)."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("ui_font_weight"),
                    pick: |settings_content| settings_content.theme.ui_font_weight.as_ref(),
                    write: |settings_content, value, _| {
                        settings_content.theme.ui_font_weight = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                files: USER,
                title: localization::localized_str!("Font Features"),
                description: localization::localized_str!("The OpenType features to enable for rendering in UI elements."),
                field: Box::new(
                    SettingField {
                        organization_override: None,
                        json_path: Some("ui_font_features"),
                        pick: |settings_content| settings_content.theme.ui_font_features.as_ref(),
                        write: |settings_content, value, _| {
                            settings_content.theme.ui_font_features = value;
                        },
                    }
                    .unimplemented(),
                ),
                metadata: None,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                files: USER,
                title: localization::localized_str!("Font Fallbacks"),
                description: localization::localized_str!("The font fallbacks to use for rendering in the UI."),
                field: Box::new(
                    SettingField {
                        organization_override: None,
                        json_path: Some("ui_font_fallbacks"),
                        pick: |settings_content| settings_content.theme.ui_font_fallbacks.as_ref(),
                        write: |settings_content, value, _| {
                            settings_content.theme.ui_font_fallbacks = value;
                        },
                    }
                    .unimplemented(),
                ),
                metadata: None,
            }),
        ]
    }


    fn markdown_preview_font_section() -> [SettingsPageItem; 5] {
        [
            SettingsPageItem::SectionHeader(localization::localized_str!("Markdown Preview Font")),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Font Family"),
                description: localization::localized_str!("Font family for the markdown preview. Falls back to the UI font family."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("markdown_preview.font_family"),
                    pick: |settings_content| {
                        settings_content
                            .markdown_preview
                            .as_ref()?
                            .font_family
                            .as_ref()
                            .or(settings_content.theme.ui_font_family.as_ref())
                    },
                    write: |settings_content, value, _| {
                        settings_content
                            .markdown_preview
                            .get_or_insert_default()
                            .font_family = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Code Font Family"),
                description: localization::localized_str!("Font family for code blocks in the markdown preview. Falls back to the editor font family."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("markdown_preview.code_font_family"),
                    pick: |settings_content| {
                        settings_content
                            .markdown_preview
                            .as_ref()?
                            .code_font_family
                            .as_ref()
                            .or(settings_content.theme.buffer_font_family.as_ref())
                    },
                    write: |settings_content, value, _| {
                        settings_content
                            .markdown_preview
                            .get_or_insert_default()
                            .code_font_family = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Font Size"),
                description: localization::localized_str!("Font size for the markdown preview. Falls back to the editor font size."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("markdown_preview.font_size"),
                    pick: |settings_content| {
                        settings_content
                            .markdown_preview
                            .as_ref()
                            .and_then(|preview| preview.font_size.as_ref())
                            .or(settings_content.theme.buffer_font_size.as_ref())
                    },
                    write: |settings_content, value, _| {
                        settings_content
                            .markdown_preview
                            .get_or_insert_default()
                            .font_size = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: "Heading Font Weight",
                description: "Font weight for headings in the markdown preview, in CSS units from 100 to 900.",
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("markdown_preview.heading_font_weight"),
                    pick: |settings_content| {
                        settings_content
                            .markdown_preview
                            .as_ref()?
                            .heading_font_weight
                            .as_ref()
                    },
                    write: |settings_content, value, _| {
                        settings_content
                            .markdown_preview
                            .get_or_insert_default()
                            .heading_font_weight = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
        ]
    }

    fn mermaid_font_section() -> [SettingsPageItem; 2] {
        [
            SettingsPageItem::SectionHeader("Mermaid Font"),
            SettingsPageItem::SettingItem(SettingItem {
                title: "Font Family",
                description: "Font family for Mermaid diagrams in the agent panel and markdown preview. Falls back to the UI font family.",
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("mermaid_font_family"),
                    pick: |settings_content| {
                        settings_content
                            .theme
                            .mermaid_font_family
                            .as_ref()
                            .or(settings_content.theme.ui_font_family.as_ref())
                    },
                    write: |settings_content, value, _| {
                        settings_content.theme.mermaid_font_family = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
        ]
    }

    fn text_rendering_section() -> [SettingsPageItem; 2] {
        [
            SettingsPageItem::SectionHeader(localization::localized_str!("Text Rendering")),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Text Rendering Mode"),
                description: localization::localized_str!("The text rendering mode to use."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("text_rendering_mode"),
                    pick: |settings_content| {
                        settings_content.workspace.text_rendering_mode.as_ref()
                    },
                    write: |settings_content, value, _| {
                        settings_content.workspace.text_rendering_mode = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
        ]
    }

    fn cursor_section() -> [SettingsPageItem; 7] {
        [
            SettingsPageItem::SectionHeader(localization::localized_str!("Cursor")),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Multi Cursor Modifier"),
                description: localization::localized_str!("Modifier key for adding multiple cursors."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("multi_cursor_modifier"),
                    pick: |settings_content| settings_content.editor.multi_cursor_modifier.as_ref(),
                    write: |settings_content, value, _| {
                        settings_content.editor.multi_cursor_modifier = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Cursor Blink"),
                description: localization::localized_str!("Whether the cursor blinks in the editor."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("cursor_blink"),
                    pick: |settings_content| settings_content.editor.cursor_blink.as_ref(),
                    write: |settings_content, value, _| {
                        settings_content.editor.cursor_blink = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Cursor Animation"),
                description: localization::localized_str!("Whether the cursor smoothly animates when moving around the editor."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("cursor_animation.enabled"),
                    pick: |settings_content| {
                        settings_content
                            .editor
                            .cursor_animation
                            .as_ref()?
                            .enabled
                            .as_ref()
                    },
                    write: |settings_content, value, _| {
                        settings_content
                            .editor
                            .cursor_animation
                            .get_or_insert_default()
                            .enabled = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Cursor Shape"),
                description: localization::localized_str!("Cursor shape for the editor."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("cursor_shape"),
                    pick: |settings_content| settings_content.editor.cursor_shape.as_ref(),
                    write: |settings_content, value, _| {
                        settings_content.editor.cursor_shape = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Hide Mouse"),
                description: localization::localized_str!("When to hide the mouse cursor."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("hide_mouse"),
                    pick: |settings_content| settings_content.hide_mouse.as_ref(),
                    write: |settings_content, value, _| {
                        settings_content.hide_mouse = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Reduce Motion"),
                description: localization::localized_str!("Whether to reduce non-essential motion, such as loading spinners, by rendering them in a static state."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("reduce_motion"),
                    pick: |settings_content| settings_content.reduce_motion.as_ref(),
                    write: |settings_content, value, _| {
                        settings_content.reduce_motion = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
        ]
    }

    fn highlighting_section() -> [SettingsPageItem; 6] {
        [
            SettingsPageItem::SectionHeader(localization::localized_str!("Highlighting")),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Unnecessary Code Fade"),
                description: localization::localized_str!("How much to fade out unused code (0.0 - 0.9)."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("unnecessary_code_fade"),
                    pick: |settings_content| settings_content.theme.unnecessary_code_fade.as_ref(),
                    write: |settings_content, value, _| {
                        settings_content.theme.unnecessary_code_fade = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Current Line Highlight"),
                description: localization::localized_str!("How to highlight the current line."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("current_line_highlight"),
                    pick: |settings_content| {
                        settings_content.editor.current_line_highlight.as_ref()
                    },
                    write: |settings_content, value, _| {
                        settings_content.editor.current_line_highlight = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Selection Highlight"),
                description: localization::localized_str!("Highlight all occurrences of selected text."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("selection_highlight"),
                    pick: |settings_content| settings_content.editor.selection_highlight.as_ref(),
                    write: |settings_content, value, _| {
                        settings_content.editor.selection_highlight = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Rounded Selection"),
                description: localization::localized_str!("Whether the text selection should have rounded corners."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("rounded_selection"),
                    pick: |settings_content| settings_content.editor.rounded_selection.as_ref(),
                    write: |settings_content, value, _| {
                        settings_content.editor.rounded_selection = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Minimum Contrast For Highlights"),
                description: localization::localized_str!("The minimum APCA perceptual contrast to maintain when rendering text over highlight backgrounds."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("minimum_contrast_for_highlights"),
                    pick: |settings_content| {
                        settings_content
                            .editor
                            .minimum_contrast_for_highlights
                            .as_ref()
                    },
                    write: |settings_content, value, _| {
                        settings_content.editor.minimum_contrast_for_highlights = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
        ]
    }

    fn guides_section() -> [SettingsPageItem; 3] {
        [
            SettingsPageItem::SectionHeader(localization::localized_str!("Guides")),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Show Wrap Guides"),
                description: localization::localized_str!("Show wrap guides (vertical rulers)."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("show_wrap_guides"),
                    pick: |settings_content| {
                        settings_content
                            .project
                            .all_languages
                            .defaults
                            .show_wrap_guides
                            .as_ref()
                    },
                    write: |settings_content, value, _| {
                        settings_content
                            .project
                            .all_languages
                            .defaults
                            .show_wrap_guides = value;
                    },
                }),
                metadata: None,
                files: USER | PROJECT,
            }),
            // todo(settings_ui): This needs a custom component
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Wrap Guides"),
                description: localization::localized_str!("Character counts at which to show wrap guides."),
                field: Box::new(
                    SettingField {
                        organization_override: None,
                        json_path: Some("wrap_guides"),
                        pick: |settings_content| {
                            settings_content
                                .project
                                .all_languages
                                .defaults
                                .wrap_guides
                                .as_ref()
                        },
                        write: |settings_content, value, _| {
                            settings_content.project.all_languages.defaults.wrap_guides = value;
                        },
                    }
                    .unimplemented(),
                ),
                metadata: None,
                files: USER | PROJECT,
            }),
        ]
    }

    let items: Box<[SettingsPageItem]> = concat_sections!(
        theme_section(),
        buffer_font_section(),
        ui_font_section(),
        markdown_preview_font_section(),
        mermaid_font_section(),
        text_rendering_section(),
        cursor_section(),
        highlighting_section(),
        guides_section(),
    );

    SettingsPage {
        title: localization::localized_str!("Appearance"),
        items,
    }
}

fn keymap_page() -> SettingsPage {
    fn keybindings_section() -> [SettingsPageItem; 2] {
        [
            SettingsPageItem::SectionHeader(localization::localized_str!("Keybindings")),
            SettingsPageItem::ActionLink(ActionLink {
                title: localization::localized_str!("Edit Keybindings").into(),
                description: Some(localization::localized_str!("Customize keybindings in the keymap editor.").into()),
                button_text: localization::localized_str!("Open Keymap").into(),
                on_click: Arc::new(
                    |settings_window, _window, cx: &mut Context<crate::SettingsWindow>| {
                        let Some(original_window) = settings_window.original_window else {
                            return;
                        };

                        // ===== [OHOS PORT BEGIN] the settings page is a tab, not
                        // a window =====
                        // The editor is opened by calling it rather than by
                        // dispatching `OpenKeymap`: dispatching from this
                        // context goes through `WindowHandle::update`, which
                        // fails while the settings window is still open, so the
                        // window is then removed with the editor never created.
                        // `open_in_workspace_then_close_settings_tab` is shared
                        // with `open_current_settings_file`. To restore the
                        // upstream behaviour, delete this block.
                        #[cfg(target_env = "ohos")]
                        {
                            crate::open_in_workspace_then_close_settings_tab(
                                original_window,
                                cx.entity_id(),
                                cx,
                                |workspace, window, cx| {
                                    keymap_editor::open_keymap_editor(None, workspace, window, cx)
                                },
                            );
                            return;
                        }
                        // ===== [OHOS PORT END] =====

                        // ===== [OHOS PORT BEGIN] the branch above returns, so on
                        // this platform `window` would be unused and the code
                        // below dead. Gated to keep both away from the build.
                        // To restore the upstream behaviour, delete this block
                        // and the one above.
                        #[cfg(not(target_env = "ohos"))]
                        {
                            original_window
                                .update(cx, |_workspace, original_window, cx| {
                                    original_window
                                        .dispatch_action(zed_actions::OpenKeymap.boxed_clone(), cx);
                                    original_window.activate_window();
                                })
                                .ok();
                            window.remove_window();
                        }
                        // ===== [OHOS PORT END] =====
                    },
                ),
                files: USER,
            }),
        ]
    }

    fn base_keymap_section() -> [SettingsPageItem; 2] {
        [
            SettingsPageItem::SectionHeader(localization::localized_str!("Base Keymap")),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Base Keymap"),
                description: localization::localized_str!("The name of a base set of key bindings to use."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("base_keymap"),
                    pick: |settings_content| settings_content.base_keymap.as_ref(),
                    write: |settings_content, value, _| {
                        settings_content.base_keymap = value;
                    },
                }),
                metadata: Some(Box::new(SettingsFieldMetadata {
                    should_do_titlecase: Some(false),
                    ..Default::default()
                })),
                files: USER,
            }),
        ]
    }

    fn modal_editing_section() -> [SettingsPageItem; 3] {
        [
            SettingsPageItem::SectionHeader(localization::localized_str!("Modal Editing")),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Vim Mode"),
                description: localization::localized_str!("Enable Vim mode and key bindings."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("vim_mode"),
                    pick: |settings_content| settings_content.vim_mode.as_ref(),
                    write: write_vim_mode,
                }),
                metadata: None,
                files: USER,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Helix Mode"),
                description: localization::localized_str!("Enable Helix mode and key bindings."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("helix_mode"),
                    pick: |settings_content| settings_content.helix_mode.as_ref(),
                    write: write_helix_mode,
                }),
                metadata: None,
                files: USER,
            }),
        ]
    }

    let items: Box<[SettingsPageItem]> = concat_sections!(
        keybindings_section(),
        base_keymap_section(),
        modal_editing_section(),
    );

    SettingsPage {
        title: localization::localized_str!("Keymap"),
        items,
    }
}

fn editor_page() -> SettingsPage {
    fn auto_save_section() -> [SettingsPageItem; 2] {
        [
            SettingsPageItem::SectionHeader(localization::localized_str!("Auto Save")),
            SettingsPageItem::DynamicItem(DynamicItem {
                discriminant: SettingItem {
                    files: USER,
                    title: localization::localized_str!("Auto Save Mode"),
                    description: localization::localized_str!("When to auto save buffer changes."),
                    field: Box::new(SettingField {
                        organization_override: None,
                        json_path: Some("autosave$"),
                        pick: |settings_content| {
                            Some(
                                &dynamic_variants::<settings::AutosaveSetting>()[settings_content
                                    .workspace
                                    .autosave
                                    .as_ref()?
                                    .discriminant()
                                    as usize],
                            )
                        },
                        write: |settings_content, value, _| {
                            let Some(value) = value else {
                                settings_content.workspace.autosave = None;
                                return;
                            };
                            let settings_value = settings_content
                                .workspace
                                .autosave
                                .get_or_insert_with(|| settings::AutosaveSetting::Off);
                            *settings_value = match value {
                                settings::AutosaveSettingDiscriminants::Off => {
                                    settings::AutosaveSetting::Off
                                }
                                settings::AutosaveSettingDiscriminants::AfterDelay => {
                                    let milliseconds = match settings_value {
                                        settings::AutosaveSetting::AfterDelay { milliseconds } => {
                                            *milliseconds
                                        }
                                        _ => settings::DelayMs(1000),
                                    };
                                    settings::AutosaveSetting::AfterDelay { milliseconds }
                                }
                                settings::AutosaveSettingDiscriminants::OnFocusChange => {
                                    settings::AutosaveSetting::OnFocusChange
                                }
                                settings::AutosaveSettingDiscriminants::OnWindowChange => {
                                    settings::AutosaveSetting::OnWindowChange
                                }
                            };
                        },
                    }),
                    metadata: None,
                },
                pick_discriminant: |settings_content| {
                    Some(settings_content.workspace.autosave.as_ref()?.discriminant() as usize)
                },
                fields: dynamic_variants::<settings::AutosaveSetting>()
                    .into_iter()
                    .map(|variant| match variant {
                        settings::AutosaveSettingDiscriminants::Off => vec![],
                        settings::AutosaveSettingDiscriminants::AfterDelay => vec![SettingItem {
                            files: USER,
                            title: localization::localized_str!("Delay (milliseconds)"),
                            description: localization::localized_str!("Save after inactivity period (in milliseconds)."),
                            field: Box::new(SettingField {
                                organization_override: None,
                                json_path: Some("autosave.after_delay.milliseconds"),
                                pick: |settings_content| match settings_content
                                    .workspace
                                    .autosave
                                    .as_ref()
                                {
                                    Some(settings::AutosaveSetting::AfterDelay {
                                        milliseconds,
                                    }) => Some(milliseconds),
                                    _ => None,
                                },
                                write: |settings_content, value, _| {
                                    let Some(value) = value else {
                                        settings_content.workspace.autosave = None;
                                        return;
                                    };
                                    match settings_content.workspace.autosave.as_mut() {
                                        Some(settings::AutosaveSetting::AfterDelay {
                                            milliseconds,
                                        }) => *milliseconds = value,
                                        _ => return,
                                    }
                                },
                            }),
                            metadata: None,
                        }],
                        settings::AutosaveSettingDiscriminants::OnFocusChange => vec![],
                        settings::AutosaveSettingDiscriminants::OnWindowChange => vec![],
                    })
                    .collect(),
            }),
        ]
    }

    fn which_key_section() -> [SettingsPageItem; 3] {
        [
            SettingsPageItem::SectionHeader(localization::localized_str!("Which-key Menu")),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Show Which-key Menu"),
                description: localization::localized_str!("Display the which-key menu with matching bindings while a multi-stroke binding is pending. The pending keystrokes indicator remains visible, but its binding preview popover is disabled."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("which_key.enabled"),
                    pick: |settings_content| {
                        settings_content
                            .which_key
                            .as_ref()
                            .and_then(|settings| settings.enabled.as_ref())
                    },
                    write: |settings_content, value, _| {
                        settings_content.which_key.get_or_insert_default().enabled = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Menu Delay"),
                description: localization::localized_str!("Delay in milliseconds before the which-key menu appears."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("which_key.delay_ms"),
                    pick: |settings_content| {
                        settings_content
                            .which_key
                            .as_ref()
                            .and_then(|settings| settings.delay_ms.as_ref())
                    },
                    write: |settings_content, value, _| {
                        settings_content.which_key.get_or_insert_default().delay_ms = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
        ]
    }

    fn multibuffer_section() -> [SettingsPageItem; 7] {
        [
            SettingsPageItem::SectionHeader(localization::localized_str!("Multibuffer")),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Double Click In Multibuffer"),
                description: localization::localized_str!("What to do when multibuffer is double-clicked in some of its excerpts."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("double_click_in_multibuffer"),
                    pick: |settings_content| {
                        settings_content.editor.double_click_in_multibuffer.as_ref()
                    },
                    write: |settings_content, value, _| {
                        settings_content.editor.double_click_in_multibuffer = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Expand Excerpt Lines"),
                description: localization::localized_str!("How many lines to expand the multibuffer excerpts by default."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("expand_excerpt_lines"),
                    pick: |settings_content| settings_content.editor.expand_excerpt_lines.as_ref(),
                    write: |settings_content, value, _| {
                        settings_content.editor.expand_excerpt_lines = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Excerpt Context Lines"),
                description: localization::localized_str!("How many lines of context to provide in multibuffer excerpts by default."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("excerpt_context_lines"),
                    pick: |settings_content| settings_content.editor.excerpt_context_lines.as_ref(),
                    write: |settings_content, value, _| {
                        settings_content.editor.excerpt_context_lines = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Expand Outlines With Depth"),
                description: localization::localized_str!("Default depth to expand outline items in the current file."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("outline_panel.expand_outlines_with_depth"),
                    pick: |settings_content| {
                        settings_content
                            .outline_panel
                            .as_ref()
                            .and_then(|outline_panel| {
                                outline_panel.expand_outlines_with_depth.as_ref()
                            })
                    },
                    write: |settings_content, value, _| {
                        settings_content
                            .outline_panel
                            .get_or_insert_default()
                            .expand_outlines_with_depth = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Diff View Style"),
                description: localization::localized_str!("How to display diffs in the editor."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("diff_view_style"),
                    pick: |settings_content| settings_content.editor.diff_view_style.as_ref(),
                    write: |settings_content, value, _| {
                        settings_content.editor.diff_view_style = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Minimum Split Diff Width"),
                description: localization::localized_str!("The minimum width (in columns) at which the split diff view is used. When the editor is narrower, the diff view automatically switches to unified mode. Set to 0 to disable."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("minimum_split_diff_width"),
                    pick: |settings_content| {
                        settings_content.editor.minimum_split_diff_width.as_ref()
                    },
                    write: |settings_content, value, _| {
                        settings_content.editor.minimum_split_diff_width = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
        ]
    }

    fn scrolling_section() -> [SettingsPageItem; 9] {
        [
            SettingsPageItem::SectionHeader(localization::localized_str!("Scrolling")),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Scroll Beyond Last Line"),
                description: localization::localized_str!("Whether the editor will scroll beyond the last line."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("scroll_beyond_last_line"),
                    pick: |settings_content| {
                        settings_content.editor.scroll_beyond_last_line.as_ref()
                    },
                    write: |settings_content, value, _| {
                        settings_content.editor.scroll_beyond_last_line = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Vertical Scroll Margin"),
                description: localization::localized_str!("The number of lines to keep above/below the cursor when auto-scrolling."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("vertical_scroll_margin"),
                    pick: |settings_content| {
                        settings_content.editor.vertical_scroll_margin.as_ref()
                    },
                    write: |settings_content, value, _| {
                        settings_content.editor.vertical_scroll_margin = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Horizontal Scroll Margin"),
                description: localization::localized_str!("The number of characters to keep on either side when scrolling with the mouse."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("horizontal_scroll_margin"),
                    pick: |settings_content| {
                        settings_content.editor.horizontal_scroll_margin.as_ref()
                    },
                    write: |settings_content, value, _| {
                        settings_content.editor.horizontal_scroll_margin = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Scroll Sensitivity"),
                description: localization::localized_str!("Scroll sensitivity multiplier for both horizontal and vertical scrolling."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("scroll_sensitivity"),
                    pick: |settings_content| settings_content.editor.scroll_sensitivity.as_ref(),
                    write: |settings_content, value, _| {
                        settings_content.editor.scroll_sensitivity = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Mouse Wheel Zoom"),
                description: localization::localized_str!("Whether to zoom the editor font size with the mouse wheel while holding the primary modifier key."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("mouse_wheel_zoom"),
                    pick: |settings_content| settings_content.editor.mouse_wheel_zoom.as_ref(),
                    write: |settings_content, value, _| {
                        settings_content.editor.mouse_wheel_zoom = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Fast Scroll Sensitivity"),
                description: localization::localized_str!("Fast scroll sensitivity multiplier for both horizontal and vertical scrolling."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("fast_scroll_sensitivity"),
                    pick: |settings_content| {
                        settings_content.editor.fast_scroll_sensitivity.as_ref()
                    },
                    write: |settings_content, value, _| {
                        settings_content.editor.fast_scroll_sensitivity = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Autoscroll On Clicks"),
                description: localization::localized_str!("Whether to scroll when clicking near the edge of the visible text area."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("autoscroll_on_clicks"),
                    pick: |settings_content| settings_content.editor.autoscroll_on_clicks.as_ref(),
                    write: |settings_content, value, _| {
                        settings_content.editor.autoscroll_on_clicks = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Sticky Scroll"),
                description: localization::localized_str!("Whether to stick scopes to the top of the editor"),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("sticky_scroll.enabled"),
                    pick: |settings_content| {
                        settings_content
                            .editor
                            .sticky_scroll
                            .as_ref()
                            .and_then(|sticky_scroll| sticky_scroll.enabled.as_ref())
                    },
                    write: |settings_content, value, _| {
                        settings_content
                            .editor
                            .sticky_scroll
                            .get_or_insert_default()
                            .enabled = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
        ]
    }

    fn signature_help_section() -> [SettingsPageItem; 4] {
        [
            SettingsPageItem::SectionHeader(localization::localized_str!("Signature Help")),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Auto Signature Help"),
                description: localization::localized_str!("Automatically show a signature help pop-up."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("auto_signature_help"),
                    pick: |settings_content| settings_content.editor.auto_signature_help.as_ref(),
                    write: |settings_content, value, _| {
                        settings_content.editor.auto_signature_help = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Show Signature Help After Edits"),
                description: localization::localized_str!("Show the signature help pop-up after completions or bracket pairs are inserted."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("show_signature_help_after_edits"),
                    pick: |settings_content| {
                        settings_content
                            .editor
                            .show_signature_help_after_edits
                            .as_ref()
                    },
                    write: |settings_content, value, _| {
                        settings_content.editor.show_signature_help_after_edits = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Snippet Sort Order"),
                description: localization::localized_str!("Determines how snippets are sorted relative to other completion items."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("snippet_sort_order"),
                    pick: |settings_content| settings_content.editor.snippet_sort_order.as_ref(),
                    write: |settings_content, value, _| {
                        settings_content.editor.snippet_sort_order = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
        ]
    }

    fn hover_popover_section() -> [SettingsPageItem; 5] {
        [
            SettingsPageItem::SectionHeader(localization::localized_str!("Hover Popover")),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Enabled"),
                description: localization::localized_str!("Show the informational hover box when moving the mouse over symbols in the editor."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("hover_popover_enabled"),
                    pick: |settings_content| settings_content.editor.hover_popover_enabled.as_ref(),
                    write: |settings_content, value, _| {
                        settings_content.editor.hover_popover_enabled = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
            // todo(settings ui): add units to this number input
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Delay"),
                description: localization::localized_str!("Time to wait in milliseconds before showing the informational hover box."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("hover_popover_delay"),
                    pick: |settings_content| settings_content.editor.hover_popover_delay.as_ref(),
                    write: |settings_content, value, _| {
                        settings_content.editor.hover_popover_delay = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Sticky"),
                description: localization::localized_str!("Whether the hover popover sticks when the mouse moves toward it, allowing interaction with its contents."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("hover_popover_sticky"),
                    pick: |settings_content| settings_content.editor.hover_popover_sticky.as_ref(),
                    write: |settings_content, value, _| {
                        settings_content.editor.hover_popover_sticky = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
            // todo(settings ui): add units to this number input
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Hiding Delay"),
                description: localization::localized_str!("Time to wait in milliseconds before hiding the hover popover after the mouse moves away."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("hover_popover_hiding_delay"),
                    pick: |settings_content| {
                        settings_content.editor.hover_popover_hiding_delay.as_ref()
                    },
                    write: |settings_content, value, _| {
                        settings_content.editor.hover_popover_hiding_delay = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
        ]
    }

    fn drag_and_drop_selection_section() -> [SettingsPageItem; 3] {
        [
            SettingsPageItem::SectionHeader(localization::localized_str!("Drag And Drop Selection")),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Enabled"),
                description: localization::localized_str!("Enable drag and drop selection."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("drag_and_drop_selection.enabled"),
                    pick: |settings_content| {
                        settings_content
                            .editor
                            .drag_and_drop_selection
                            .as_ref()
                            .and_then(|drag_and_drop| drag_and_drop.enabled.as_ref())
                    },
                    write: |settings_content, value, _| {
                        settings_content
                            .editor
                            .drag_and_drop_selection
                            .get_or_insert_default()
                            .enabled = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Delay"),
                description: localization::localized_str!("Delay in milliseconds before drag and drop selection starts."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("drag_and_drop_selection.delay"),
                    pick: |settings_content| {
                        settings_content
                            .editor
                            .drag_and_drop_selection
                            .as_ref()
                            .and_then(|drag_and_drop| drag_and_drop.delay.as_ref())
                    },
                    write: |settings_content, value, _| {
                        settings_content
                            .editor
                            .drag_and_drop_selection
                            .get_or_insert_default()
                            .delay = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
        ]
    }

    fn gutter_section() -> [SettingsPageItem; 10] {
        [
            SettingsPageItem::SectionHeader(localization::localized_str!("Gutter")),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Show Line Numbers"),
                description: localization::localized_str!("Show line numbers in the gutter."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("gutter.line_numbers"),
                    pick: |settings_content| {
                        settings_content
                            .editor
                            .gutter
                            .as_ref()
                            .and_then(|gutter| gutter.line_numbers.as_ref())
                    },
                    write: |settings_content, value, _| {
                        settings_content
                            .editor
                            .gutter
                            .get_or_insert_default()
                            .line_numbers = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Relative Line Numbers"),
                description: localization::localized_str!("Controls line number display in the editor's gutter. \"disabled\" shows absolute line numbers, \"enabled\" shows relative line numbers for each absolute line, and \"wrapped\" shows relative line numbers for every line, absolute or wrapped."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("relative_line_numbers"),
                    pick: |settings_content| settings_content.editor.relative_line_numbers.as_ref(),
                    write: |settings_content, value, _| {
                        settings_content.editor.relative_line_numbers = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Show Runnables"),
                description: localization::localized_str!("Show runnable buttons in the gutter."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("gutter.runnables"),
                    pick: |settings_content| {
                        settings_content
                            .editor
                            .gutter
                            .as_ref()
                            .and_then(|gutter| gutter.runnables.as_ref())
                    },
                    write: |settings_content, value, _| {
                        settings_content
                            .editor
                            .gutter
                            .get_or_insert_default()
                            .runnables = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Show Breakpoints"),
                description: localization::localized_str!("Show breakpoints in the gutter."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("gutter.breakpoints"),
                    pick: |settings_content| {
                        settings_content
                            .editor
                            .gutter
                            .as_ref()
                            .and_then(|gutter| gutter.breakpoints.as_ref())
                    },
                    write: |settings_content, value, _| {
                        settings_content
                            .editor
                            .gutter
                            .get_or_insert_default()
                            .breakpoints = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Show Bookmarks"),
                description: localization::localized_str!("Show bookmarks in the gutter."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("gutter.bookmarks"),
                    pick: |settings_content| {
                        settings_content
                            .editor
                            .gutter
                            .as_ref()
                            .and_then(|gutter| gutter.bookmarks.as_ref())
                    },
                    write: |settings_content, value, _| {
                        settings_content
                            .editor
                            .gutter
                            .get_or_insert_default()
                            .bookmarks = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Show Folds"),
                description: localization::localized_str!("Show code folding controls in the gutter."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("gutter.folds"),
                    pick: |settings_content| {
                        settings_content
                            .editor
                            .gutter
                            .as_ref()
                            .and_then(|gutter| gutter.folds.as_ref())
                    },
                    write: |settings_content, value, _| {
                        settings_content.editor.gutter.get_or_insert_default().folds = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Min Line Number Digits"),
                description: localization::localized_str!("Minimum number of characters to reserve space for in the gutter."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("gutter.min_line_number_digits"),
                    pick: |settings_content| {
                        settings_content
                            .editor
                            .gutter
                            .as_ref()
                            .and_then(|gutter| gutter.min_line_number_digits.as_ref())
                    },
                    write: |settings_content, value, _| {
                        settings_content
                            .editor
                            .gutter
                            .get_or_insert_default()
                            .min_line_number_digits = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
            SettingsPageItem::DynamicItem(DynamicItem {
                discriminant: SettingItem {
                    title: localization::localized_str!("Git Gutter Width"),
                    description: localization::localized_str!("Width of the git diff indicators in the gutter. Default scales with the buffer font size."),
                    field: Box::new(SettingField {
                        organization_override: None,
                        json_path: Some("gutter.git_gutter_width$"),
                        pick: |settings_content| {
                            Some(
                                &dynamic_variants::<settings::GitGutterWidth>()[settings_content
                                    .editor
                                    .gutter
                                    .as_ref()?
                                    .git_gutter_width
                                    .as_ref()?
                                    .discriminant()
                                    as usize],
                            )
                        },
                        write: |settings_content, value, _| {
                            let gutter = settings_content.editor.gutter.get_or_insert_default();
                            gutter.git_gutter_width = value.map(|value| match value {
                                settings::GitGutterWidthDiscriminants::Default => {
                                    settings::GitGutterWidth::Default
                                }
                                settings::GitGutterWidthDiscriminants::Custom => {
                                    let width = match gutter.git_gutter_width {
                                        Some(settings::GitGutterWidth::Custom(width)) => {
                                            settings::PixelSetting(*width)
                                        }
                                        _ => settings::PixelSetting(3.0),
                                    };
                                    settings::GitGutterWidth::Custom(width)
                                }
                            });
                        },
                    }),
                    metadata: None,
                    files: USER,
                },
                pick_discriminant: |settings_content| {
                    Some(
                        settings_content
                            .editor
                            .gutter
                            .as_ref()?
                            .git_gutter_width
                            .as_ref()?
                            .discriminant() as usize,
                    )
                },
                fields: dynamic_variants::<settings::GitGutterWidth>()
                    .into_iter()
                    .map(|variant| match variant {
                        settings::GitGutterWidthDiscriminants::Default => vec![],
                        settings::GitGutterWidthDiscriminants::Custom => vec![SettingItem {
                            files: USER,
                            title: localization::localized_str!("Custom Width"),
                            description: localization::localized_str!("Width in pixels of the git diff indicators."),
                            field: Box::new(SettingField {
                                organization_override: None,
                                json_path: Some("gutter.git_gutter_width"),
                                pick: |settings_content| match settings_content
                                    .editor
                                    .gutter
                                    .as_ref()
                                    .and_then(|gutter| gutter.git_gutter_width.as_ref())
                                {
                                    Some(settings::GitGutterWidth::Custom(value)) => Some(value),
                                    _ => None,
                                },
                                write: |settings_content, value, _| {
                                    let Some(value) = value else {
                                        return;
                                    };
                                    if let Some(settings::GitGutterWidth::Custom(width)) =
                                        settings_content
                                            .editor
                                            .gutter
                                            .as_mut()
                                            .and_then(|gutter| gutter.git_gutter_width.as_mut())
                                    {
                                        *width = value;
                                    }
                                },
                            }),
                            metadata: None,
                        }],
                    })
                    .collect(),
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Inline Code Actions"),
                description: localization::localized_str!("Show code action button at start of buffer line."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("inline_code_actions"),
                    pick: |settings_content| settings_content.editor.inline_code_actions.as_ref(),
                    write: |settings_content, value, _| {
                        settings_content.editor.inline_code_actions = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
        ]
    }

    fn scrollbar_section() -> [SettingsPageItem; 10] {
        [
            SettingsPageItem::SectionHeader(localization::localized_str!("Scrollbar")),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Show"),
                description: localization::localized_str!("When to show the scrollbar in the editor."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("scrollbar"),
                    pick: |settings_content| {
                        settings_content.editor.scrollbar.as_ref()?.show.as_ref()
                    },
                    write: |settings_content, value, _| {
                        settings_content
                            .editor
                            .scrollbar
                            .get_or_insert_default()
                            .show = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Cursors"),
                description: localization::localized_str!("Show cursor positions in the scrollbar."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("scrollbar.cursors"),
                    pick: |settings_content| {
                        settings_content.editor.scrollbar.as_ref()?.cursors.as_ref()
                    },
                    write: |settings_content, value, _| {
                        settings_content
                            .editor
                            .scrollbar
                            .get_or_insert_default()
                            .cursors = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Git Diff"),
                description: localization::localized_str!("Show Git diff indicators in the scrollbar."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("scrollbar.git_diff"),
                    pick: |settings_content| {
                        settings_content
                            .editor
                            .scrollbar
                            .as_ref()?
                            .git_diff
                            .as_ref()
                    },
                    write: |settings_content, value, _| {
                        settings_content
                            .editor
                            .scrollbar
                            .get_or_insert_default()
                            .git_diff = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Search Results"),
                description: localization::localized_str!("Show buffer search result indicators in the scrollbar."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("scrollbar.search_results"),
                    pick: |settings_content| {
                        settings_content
                            .editor
                            .scrollbar
                            .as_ref()?
                            .search_results
                            .as_ref()
                    },
                    write: |settings_content, value, _| {
                        settings_content
                            .editor
                            .scrollbar
                            .get_or_insert_default()
                            .search_results = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Selected Text"),
                description: localization::localized_str!("Show selected text occurrences in the scrollbar."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("scrollbar.selected_text"),
                    pick: |settings_content| {
                        settings_content
                            .editor
                            .scrollbar
                            .as_ref()?
                            .selected_text
                            .as_ref()
                    },
                    write: |settings_content, value, _| {
                        settings_content
                            .editor
                            .scrollbar
                            .get_or_insert_default()
                            .selected_text = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Selected Symbol"),
                description: localization::localized_str!("Show selected symbol occurrences in the scrollbar."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("scrollbar.selected_symbol"),
                    pick: |settings_content| {
                        settings_content
                            .editor
                            .scrollbar
                            .as_ref()?
                            .selected_symbol
                            .as_ref()
                    },
                    write: |settings_content, value, _| {
                        settings_content
                            .editor
                            .scrollbar
                            .get_or_insert_default()
                            .selected_symbol = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Diagnostics"),
                description: localization::localized_str!("Which diagnostic indicators to show in the scrollbar."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("scrollbar.diagnostics"),
                    pick: |settings_content| {
                        settings_content
                            .editor
                            .scrollbar
                            .as_ref()?
                            .diagnostics
                            .as_ref()
                    },
                    write: |settings_content, value, _| {
                        settings_content
                            .editor
                            .scrollbar
                            .get_or_insert_default()
                            .diagnostics = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Horizontal Scrollbar"),
                description: localization::localized_str!("When false, forcefully disables the horizontal scrollbar."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("scrollbar.axes.horizontal"),
                    pick: |settings_content| {
                        settings_content
                            .editor
                            .scrollbar
                            .as_ref()?
                            .axes
                            .as_ref()?
                            .horizontal
                            .as_ref()
                    },
                    write: |settings_content, value, _| {
                        settings_content
                            .editor
                            .scrollbar
                            .get_or_insert_default()
                            .axes
                            .get_or_insert_default()
                            .horizontal = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Vertical Scrollbar"),
                description: localization::localized_str!("When false, forcefully disables the vertical scrollbar."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("scrollbar.axes.vertical"),
                    pick: |settings_content| {
                        settings_content
                            .editor
                            .scrollbar
                            .as_ref()?
                            .axes
                            .as_ref()?
                            .vertical
                            .as_ref()
                    },
                    write: |settings_content, value, _| {
                        settings_content
                            .editor
                            .scrollbar
                            .get_or_insert_default()
                            .axes
                            .get_or_insert_default()
                            .vertical = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
        ]
    }

    fn minimap_section() -> [SettingsPageItem; 7] {
        [
            SettingsPageItem::SectionHeader(localization::localized_str!("Minimap")),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Show"),
                description: localization::localized_str!("When to show the minimap in the editor."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("minimap.show"),
                    pick: |settings_content| {
                        settings_content.editor.minimap.as_ref()?.show.as_ref()
                    },
                    write: |settings_content, value, _| {
                        settings_content.editor.minimap.get_or_insert_default().show = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Display In"),
                description: localization::localized_str!("Where to show the minimap in the editor."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("minimap.display_in"),
                    pick: |settings_content| {
                        settings_content
                            .editor
                            .minimap
                            .as_ref()?
                            .display_in
                            .as_ref()
                    },
                    write: |settings_content, value, _| {
                        settings_content
                            .editor
                            .minimap
                            .get_or_insert_default()
                            .display_in = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Thumb"),
                description: localization::localized_str!("When to show the minimap thumb."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("minimap.thumb"),
                    pick: |settings_content| {
                        settings_content.editor.minimap.as_ref()?.thumb.as_ref()
                    },
                    write: |settings_content, value, _| {
                        settings_content
                            .editor
                            .minimap
                            .get_or_insert_default()
                            .thumb = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Thumb Border"),
                description: localization::localized_str!("Border style for the minimap's scrollbar thumb."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("minimap.thumb_border"),
                    pick: |settings_content| {
                        settings_content
                            .editor
                            .minimap
                            .as_ref()?
                            .thumb_border
                            .as_ref()
                    },
                    write: |settings_content, value, _| {
                        settings_content
                            .editor
                            .minimap
                            .get_or_insert_default()
                            .thumb_border = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Current Line Highlight"),
                description: localization::localized_str!("How to highlight the current line in the minimap."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("minimap.current_line_highlight"),
                    pick: |settings_content| {
                        settings_content
                            .editor
                            .minimap
                            .as_ref()
                            .and_then(|minimap| minimap.current_line_highlight.as_ref())
                            .or(settings_content.editor.current_line_highlight.as_ref())
                    },
                    write: |settings_content, value, _| {
                        settings_content
                            .editor
                            .minimap
                            .get_or_insert_default()
                            .current_line_highlight = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Max Width Columns"),
                description: localization::localized_str!("Maximum number of columns to display in the minimap."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("minimap.max_width_columns"),
                    pick: |settings_content| {
                        settings_content
                            .editor
                            .minimap
                            .as_ref()?
                            .max_width_columns
                            .as_ref()
                    },
                    write: |settings_content, value, _| {
                        settings_content
                            .editor
                            .minimap
                            .get_or_insert_default()
                            .max_width_columns = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
        ]
    }

    // [OHOS PORT BEGIN] This build has no editor toolbar, so none of its
    // toggles apply.
    #[cfg(not(target_env = "ohos"))]
    fn toolbar_section() -> [SettingsPageItem; 5] {
        [
            SettingsPageItem::SectionHeader(localization::localized_str!("Toolbar")),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Breadcrumbs"),
                description: localization::localized_str!("Show breadcrumbs."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("toolbar.breadcrumbs"),
                    pick: |settings_content| {
                        settings_content
                            .editor
                            .toolbar
                            .as_ref()?
                            .breadcrumbs
                            .as_ref()
                    },
                    write: |settings_content, value, _| {
                        settings_content
                            .editor
                            .toolbar
                            .get_or_insert_default()
                            .breadcrumbs = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Quick Actions"),
                description: localization::localized_str!("Show quick action buttons (e.g., search, selection, editor controls, etc.)."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("toolbar.quick_actions"),
                    pick: |settings_content| {
                        settings_content
                            .editor
                            .toolbar
                            .as_ref()?
                            .quick_actions
                            .as_ref()
                    },
                    write: |settings_content, value, _| {
                        settings_content
                            .editor
                            .toolbar
                            .get_or_insert_default()
                            .quick_actions = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Selections Menu"),
                description: localization::localized_str!("Show the selections menu in the editor toolbar."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("toolbar.selections_menu"),
                    pick: |settings_content| {
                        settings_content
                            .editor
                            .toolbar
                            .as_ref()?
                            .selections_menu
                            .as_ref()
                    },
                    write: |settings_content, value, _| {
                        settings_content
                            .editor
                            .toolbar
                            .get_or_insert_default()
                            .selections_menu = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Code Actions"),
                description: localization::localized_str!("Show code action buttons in the editor toolbar."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("toolbar.code_actions"),
                    pick: |settings_content| {
                        settings_content
                            .editor
                            .toolbar
                            .as_ref()?
                            .code_actions
                            .as_ref()
                    },
                    write: |settings_content, value, _| {
                        settings_content
                            .editor
                            .toolbar
                            .get_or_insert_default()
                            .code_actions = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
        ]
    }

    fn vim_settings_section() -> [SettingsPageItem; 13] {
        [
            SettingsPageItem::SectionHeader(localization::localized_str!("Vim")),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Default Mode"),
                description: localization::localized_str!("The default mode when Vim starts."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("vim.default_mode"),
                    pick: |settings_content| settings_content.vim.as_ref()?.default_mode.as_ref(),
                    write: |settings_content, value, _| {
                        settings_content.vim.get_or_insert_default().default_mode = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Toggle Relative Line Numbers"),
                description: localization::localized_str!("Toggle relative line numbers in Vim mode."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("vim.toggle_relative_line_numbers"),
                    pick: |settings_content| {
                        settings_content
                            .vim
                            .as_ref()?
                            .toggle_relative_line_numbers
                            .as_ref()
                    },
                    write: |settings_content, value, _| {
                        settings_content
                            .vim
                            .get_or_insert_default()
                            .toggle_relative_line_numbers = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Use System Clipboard"),
                description: localization::localized_str!("Controls when to use system clipboard in Vim mode."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("vim.use_system_clipboard"),
                    pick: |settings_content| {
                        settings_content.vim.as_ref()?.use_system_clipboard.as_ref()
                    },
                    write: |settings_content, value, _| {
                        settings_content
                            .vim
                            .get_or_insert_default()
                            .use_system_clipboard = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Use Smartcase Find"),
                description: localization::localized_str!("Enable smartcase searching in Vim mode."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("vim.use_smartcase_find"),
                    pick: |settings_content| {
                        settings_content.vim.as_ref()?.use_smartcase_find.as_ref()
                    },
                    write: |settings_content, value, _| {
                        settings_content
                            .vim
                            .get_or_insert_default()
                            .use_smartcase_find = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Global Substitution Default"),
                description: localization::localized_str!("When enabled, the :substitute command replaces all matches in a line by default. The 'g' flag then toggles this behavior."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("vim.gdefault"),
                    pick: |settings_content| settings_content.vim.as_ref()?.gdefault.as_ref(),
                    write: |settings_content, value, _| {
                        settings_content.vim.get_or_insert_default().gdefault = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Highlight on Yank Duration"),
                description: localization::localized_str!("Duration in milliseconds to highlight yanked text in Vim mode."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("vim.highlight_on_yank_duration"),
                    pick: |settings_content| {
                        settings_content
                            .vim
                            .as_ref()?
                            .highlight_on_yank_duration
                            .as_ref()
                    },
                    write: |settings_content, value, _| {
                        settings_content
                            .vim
                            .get_or_insert_default()
                            .highlight_on_yank_duration = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Regex Search"),
                description: localization::localized_str!("Use regex search by default in Vim search."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("vim.use_regex_search"),
                    pick: |settings_content| {
                        settings_content.vim.as_ref()?.use_regex_search.as_ref()
                    },
                    write: |settings_content, value, _| {
                        settings_content
                            .vim
                            .get_or_insert_default()
                            .use_regex_search = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Cursor Shape - Normal Mode"),
                description: localization::localized_str!("Cursor shape for normal mode."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("vim.cursor_shape.normal"),
                    pick: |settings_content| {
                        settings_content
                            .vim
                            .as_ref()?
                            .cursor_shape
                            .as_ref()?
                            .normal
                            .as_ref()
                    },
                    write: |settings_content, value, _| {
                        settings_content
                            .vim
                            .get_or_insert_default()
                            .cursor_shape
                            .get_or_insert_default()
                            .normal = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Cursor Shape - Insert Mode"),
                description: localization::localized_str!("Cursor shape for insert mode. Inherit uses the editor's cursor shape."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("vim.cursor_shape.insert"),
                    pick: |settings_content| {
                        settings_content
                            .vim
                            .as_ref()?
                            .cursor_shape
                            .as_ref()?
                            .insert
                            .as_ref()
                    },
                    write: |settings_content, value, _| {
                        settings_content
                            .vim
                            .get_or_insert_default()
                            .cursor_shape
                            .get_or_insert_default()
                            .insert = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Cursor Shape - Replace Mode"),
                description: localization::localized_str!("Cursor shape for replace mode."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("vim.cursor_shape.replace"),
                    pick: |settings_content| {
                        settings_content
                            .vim
                            .as_ref()?
                            .cursor_shape
                            .as_ref()?
                            .replace
                            .as_ref()
                    },
                    write: |settings_content, value, _| {
                        settings_content
                            .vim
                            .get_or_insert_default()
                            .cursor_shape
                            .get_or_insert_default()
                            .replace = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Cursor Shape - Visual Mode"),
                description: localization::localized_str!("Cursor shape for visual mode."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("vim.cursor_shape.visual"),
                    pick: |settings_content| {
                        settings_content
                            .vim
                            .as_ref()?
                            .cursor_shape
                            .as_ref()?
                            .visual
                            .as_ref()
                    },
                    write: |settings_content, value, _| {
                        settings_content
                            .vim
                            .get_or_insert_default()
                            .cursor_shape
                            .get_or_insert_default()
                            .visual = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Custom Digraphs"),
                description: localization::localized_str!("Custom digraph mappings for Vim mode."),
                field: Box::new(
                    SettingField {
                        organization_override: None,
                        json_path: Some("vim.custom_digraphs"),
                        pick: |settings_content| {
                            settings_content.vim.as_ref()?.custom_digraphs.as_ref()
                        },
                        write: |settings_content, value, _| {
                            settings_content.vim.get_or_insert_default().custom_digraphs = value;
                        },
                    }
                    .unimplemented(),
                ),
                metadata: None,
                files: USER,
            }),
        ]
    }

    let mut items: Vec<SettingsPageItem> = concat_sections!(
        auto_save_section(),
        which_key_section(),
        multibuffer_section(),
        scrolling_section(),
        signature_help_section(),
        hover_popover_section(),
        drag_and_drop_selection_section(),
        gutter_section(),
        scrollbar_section(),
        minimap_section(),
    )
    .into_vec();
    // [OHOS PORT BEGIN] This build has no editor toolbar, so its toggles are
    // not offered.
    #[cfg(not(target_env = "ohos"))]
    items.extend(toolbar_section());
    // [OHOS PORT END]
    items.extend(concat_sections!(
        vim_settings_section(),
        language_settings_data(),
    ));
    let items: Box<[SettingsPageItem]> = items.into();

    SettingsPage {
        title: localization::localized_str!("Editor"),
        items: items,
    }
}

fn languages_and_tools_page(cx: &App) -> SettingsPage {
    fn file_types_section() -> [SettingsPageItem; 2] {
        [
            SettingsPageItem::SectionHeader(localization::localized_str!("File Types")),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("File Type Associations"),
                description: localization::localized_str!("A mapping from languages to files and file extensions that should be treated as that language."),
                field: Box::new(
                    SettingField {
                        organization_override: None,
                        json_path: Some("file_type_associations"),
                        pick: |settings_content| {
                            settings_content.project.all_languages.file_types.as_ref()
                        },
                        write: |settings_content, value, _| {
                            settings_content.project.all_languages.file_types = value;
                        },
                    }
                    .unimplemented(),
                ),
                metadata: None,
                files: USER | PROJECT,
            }),
        ]
    }

    fn diagnostics_section() -> [SettingsPageItem; 3] {
        [
            SettingsPageItem::SectionHeader(localization::localized_str!("Diagnostics")),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Max Severity"),
                description: localization::localized_str!("Which level to use to filter out diagnostics displayed in the editor."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("diagnostics_max_severity"),
                    pick: |settings_content| {
                        settings_content.editor.diagnostics_max_severity.as_ref()
                    },
                    write: |settings_content, value, _| {
                        settings_content.editor.diagnostics_max_severity = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Include Warnings"),
                description: localization::localized_str!("Whether to show warnings or not by default."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("diagnostics.include_warnings"),
                    pick: |settings_content| {
                        settings_content
                            .diagnostics
                            .as_ref()?
                            .include_warnings
                            .as_ref()
                    },
                    write: |settings_content, value, _| {
                        settings_content
                            .diagnostics
                            .get_or_insert_default()
                            .include_warnings = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
        ]
    }

    fn inline_diagnostics_section() -> [SettingsPageItem; 5] {
        [
            SettingsPageItem::SectionHeader(localization::localized_str!("Inline Diagnostics")),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Enabled"),
                description: localization::localized_str!("Whether to show diagnostics inline or not."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("diagnostics.inline.enabled"),
                    pick: |settings_content| {
                        settings_content
                            .diagnostics
                            .as_ref()?
                            .inline
                            .as_ref()?
                            .enabled
                            .as_ref()
                    },
                    write: |settings_content, value, _| {
                        settings_content
                            .diagnostics
                            .get_or_insert_default()
                            .inline
                            .get_or_insert_default()
                            .enabled = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Update Debounce"),
                description: localization::localized_str!("The delay in milliseconds to show inline diagnostics after the last diagnostic update."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("diagnostics.inline.update_debounce_ms"),
                    pick: |settings_content| {
                        settings_content
                            .diagnostics
                            .as_ref()?
                            .inline
                            .as_ref()?
                            .update_debounce_ms
                            .as_ref()
                    },
                    write: |settings_content, value, _| {
                        settings_content
                            .diagnostics
                            .get_or_insert_default()
                            .inline
                            .get_or_insert_default()
                            .update_debounce_ms = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Padding"),
                description: localization::localized_str!("The amount of padding between the end of the source line and the start of the inline diagnostic."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("diagnostics.inline.padding"),
                    pick: |settings_content| {
                        settings_content
                            .diagnostics
                            .as_ref()?
                            .inline
                            .as_ref()?
                            .padding
                            .as_ref()
                    },
                    write: |settings_content, value, _| {
                        settings_content
                            .diagnostics
                            .get_or_insert_default()
                            .inline
                            .get_or_insert_default()
                            .padding = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Minimum Column"),
                description: localization::localized_str!("The minimum column at which to display inline diagnostics."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("diagnostics.inline.min_column"),
                    pick: |settings_content| {
                        settings_content
                            .diagnostics
                            .as_ref()?
                            .inline
                            .as_ref()?
                            .min_column
                            .as_ref()
                    },
                    write: |settings_content, value, _| {
                        settings_content
                            .diagnostics
                            .get_or_insert_default()
                            .inline
                            .get_or_insert_default()
                            .min_column = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
        ]
    }

    fn lsp_pull_diagnostics_section() -> [SettingsPageItem; 3] {
        [
            SettingsPageItem::SectionHeader(localization::localized_str!("LSP Pull Diagnostics")),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Enabled"),
                description: localization::localized_str!("Whether to pull for language server-powered diagnostics or not."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("diagnostics.lsp_pull_diagnostics.enabled"),
                    pick: |settings_content| {
                        settings_content
                            .diagnostics
                            .as_ref()?
                            .lsp_pull_diagnostics
                            .as_ref()?
                            .enabled
                            .as_ref()
                    },
                    write: |settings_content, value, _| {
                        settings_content
                            .diagnostics
                            .get_or_insert_default()
                            .lsp_pull_diagnostics
                            .get_or_insert_default()
                            .enabled = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
            // todo(settings_ui): Needs unit
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Debounce"),
                description: localization::localized_str!("Minimum time to wait before pulling diagnostics from the language server(s)."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("diagnostics.lsp_pull_diagnostics.debounce_ms"),
                    pick: |settings_content| {
                        settings_content
                            .diagnostics
                            .as_ref()?
                            .lsp_pull_diagnostics
                            .as_ref()?
                            .debounce_ms
                            .as_ref()
                    },
                    write: |settings_content, value, _| {
                        settings_content
                            .diagnostics
                            .get_or_insert_default()
                            .lsp_pull_diagnostics
                            .get_or_insert_default()
                            .debounce_ms = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
        ]
    }

    fn lsp_highlights_section() -> [SettingsPageItem; 2] {
        [
            SettingsPageItem::SectionHeader(localization::localized_str!("LSP Highlights")),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Debounce"),
                description: localization::localized_str!("The debounce delay before querying highlights from the language."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("lsp_highlight_debounce"),
                    pick: |settings_content| {
                        settings_content.editor.lsp_highlight_debounce.as_ref()
                    },
                    write: |settings_content, value, _| {
                        settings_content.editor.lsp_highlight_debounce = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
        ]
    }

    fn languages_list_section(cx: &App) -> Box<[SettingsPageItem]> {
        // todo(settings_ui): Refresh on extension (un)/installed
        // Note that `crates/json_schema_store` solves the same problem, there is probably a way to unify the two
        std::iter::once(SettingsPageItem::SectionHeader(localization::localized_str!("Languages")))
            .chain(all_language_names(cx).into_iter().map(|language_name| {
                let link = format!("languages.{language_name}");
                SettingsPageItem::SubPageLink(SubPageLink {
                    title: language_name,
                    r#type: crate::SubPageType::Language,
                    description: None,
                    search_aliases: &[],
                    json_path: Some(link.leak()),
                    in_json: true,
                    files: USER | PROJECT,
                    render: |this, scroll_handle, window, cx| {
                        let items: Box<[SettingsPageItem]> = concat_sections!(
                            language_settings_data(),
                            non_editor_language_settings_data()
                        );
                        this.render_sub_page_items(
                            items.iter().enumerate(),
                            scroll_handle,
                            window,
                            cx,
                        )
                        .into_any_element()
                    },
                })
            }))
            .collect()
    }

    SettingsPage {
        title: localization::localized_str!("Languages & Tools"),
        items: {
            concat_sections!(
                non_editor_language_settings_data(),
                file_types_section(),
                diagnostics_section(),
                inline_diagnostics_section(),
                lsp_pull_diagnostics_section(),
                lsp_highlights_section(),
                languages_list_section(cx),
            )
        },
    }
}

fn search_and_files_page() -> SettingsPage {
    fn search_section() -> [SettingsPageItem; 10] {
        [
            SettingsPageItem::SectionHeader(localization::localized_str!("Search")),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Whole Word"),
                description: localization::localized_str!("Search for whole words by default."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("search.whole_word"),
                    pick: |settings_content| {
                        settings_content.editor.search.as_ref()?.whole_word.as_ref()
                    },
                    write: |settings_content, value, _| {
                        settings_content
                            .editor
                            .search
                            .get_or_insert_default()
                            .whole_word = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Case Sensitive"),
                description: localization::localized_str!("Search case-sensitively by default."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("search.case_sensitive"),
                    pick: |settings_content| {
                        settings_content
                            .editor
                            .search
                            .as_ref()?
                            .case_sensitive
                            .as_ref()
                    },
                    write: |settings_content, value, _| {
                        settings_content
                            .editor
                            .search
                            .get_or_insert_default()
                            .case_sensitive = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Use Smartcase Search"),
                description: localization::localized_str!("Whether to automatically enable case-sensitive search based on the search query."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("use_smartcase_search"),
                    pick: |settings_content| settings_content.editor.use_smartcase_search.as_ref(),
                    write: |settings_content, value, _| {
                        settings_content.editor.use_smartcase_search = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Include Ignored"),
                description: localization::localized_str!("Include ignored files in search results by default."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("search.include_ignored"),
                    pick: |settings_content| {
                        settings_content
                            .editor
                            .search
                            .as_ref()?
                            .include_ignored
                            .as_ref()
                    },
                    write: |settings_content, value, _| {
                        settings_content
                            .editor
                            .search
                            .get_or_insert_default()
                            .include_ignored = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Regex"),
                description: localization::localized_str!("Use regex search by default."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("search.regex"),
                    pick: |settings_content| {
                        settings_content.editor.search.as_ref()?.regex.as_ref()
                    },
                    write: |settings_content, value, _| {
                        settings_content.editor.search.get_or_insert_default().regex = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Search Wrap"),
                description: localization::localized_str!("Whether the editor search results will loop."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("search_wrap"),
                    pick: |settings_content| settings_content.editor.search_wrap.as_ref(),
                    write: |settings_content, value, _| {
                        settings_content.editor.search_wrap = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Center on Match"),
                description: localization::localized_str!("Whether to center the current match in the editor"),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("editor.search.center_on_match"),
                    pick: |settings_content| {
                        settings_content
                            .editor
                            .search
                            .as_ref()
                            .and_then(|search| search.center_on_match.as_ref())
                    },
                    write: |settings_content, value, _| {
                        settings_content
                            .editor
                            .search
                            .get_or_insert_default()
                            .center_on_match = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Search on Type"),
                description: localization::localized_str!("Start searching as you type in project search, without pressing Enter."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("editor.search.search_on_type"),
                    pick: |settings_content| {
                        settings_content
                            .editor
                            .search
                            .as_ref()
                            .and_then(|search| search.search_on_type.as_ref())
                    },
                    write: |settings_content, value, _| {
                        settings_content
                            .editor
                            .search
                            .get_or_insert_default()
                            .search_on_type = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Seed Search Query From Cursor"),
                description: localization::localized_str!("When to populate a new search's query based on the text under the cursor."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("seed_search_query_from_cursor"),
                    pick: |settings_content| {
                        settings_content
                            .editor
                            .seed_search_query_from_cursor
                            .as_ref()
                    },
                    write: |settings_content, value, _| {
                        settings_content.editor.seed_search_query_from_cursor = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
        ]
    }

    fn command_palette_section() -> [SettingsPageItem; 2] {
        [
            SettingsPageItem::SectionHeader(localization::localized_str!("Command Palette")),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Use Command History"),
                description: localization::localized_str!("Whether to use command history ranking for sorting in the command palette."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("command_palette.use_command_history"),
                    pick: |settings_content| {
                        settings_content
                            .command_palette
                            .as_ref()?
                            .use_command_history
                            .as_ref()
                    },
                    write: |settings_content, value, _| {
                        settings_content
                            .command_palette
                            .get_or_insert_default()
                            .use_command_history = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
        ]
    }

    fn file_finder_section() -> [SettingsPageItem; 5] {
        [
            SettingsPageItem::SectionHeader(localization::localized_str!("File Finder")),
            // todo: null by default
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Include Ignored in Search"),
                description: localization::localized_str!("Use gitignored files when searching."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("file_finder.include_ignored"),
                    pick: |settings_content| {
                        settings_content
                            .file_finder
                            .as_ref()?
                            .include_ignored
                            .as_ref()
                    },
                    write: |settings_content, value, _| {
                        settings_content
                            .file_finder
                            .get_or_insert_default()
                            .include_ignored = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("File Icons"),
                description: localization::localized_str!("Show file icons in the file finder."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("file_finder.file_icons"),
                    pick: |settings_content| {
                        settings_content.file_finder.as_ref()?.file_icons.as_ref()
                    },
                    write: |settings_content, value, _| {
                        settings_content
                            .file_finder
                            .get_or_insert_default()
                            .file_icons = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Skip Focus For Active In Search"),
                description: localization::localized_str!("Whether the file finder should skip focus for the active file in search results."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("file_finder.skip_focus_for_active_in_search"),
                    pick: |settings_content| {
                        settings_content
                            .file_finder
                            .as_ref()?
                            .skip_focus_for_active_in_search
                            .as_ref()
                    },
                    write: |settings_content, value, _| {
                        settings_content
                            .file_finder
                            .get_or_insert_default()
                            .skip_focus_for_active_in_search = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: "Prefill Query From Selection",
                description: "Whether to pre-fill the file finder's query with the text selected in the focused item (e.g. an editor or a terminal).",
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("file_finder.prefill_query_from_selection"),
                    pick: |settings_content| {
                        settings_content
                            .file_finder
                            .as_ref()?
                            .prefill_query_from_selection
                            .as_ref()
                    },
                    write: |settings_content, value, _| {
                        settings_content
                            .file_finder
                            .get_or_insert_default()
                            .prefill_query_from_selection = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
        ]
    }

    fn file_scan_section() -> [SettingsPageItem; 7] {
        [
            SettingsPageItem::SectionHeader(localization::localized_str!("File Scan")),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("File Scan Exclusions"),
                description: localization::localized_str!("Files or globs of files that will be excluded by HiCodeer entirely. They will be skipped during file scans, file searches, and not be displayed in the project file tree. Takes precedence over \"File Scan Inclusions\""),
                field: Box::new(
                    SettingField {
                        organization_override: None,
                        json_path: Some("file_scan_exclusions"),
                        pick: |settings_content| {
                            settings_content
                                .project
                                .worktree
                                .file_scan_exclusions
                                .as_ref()
                        },
                        write: |settings_content, value, _| {
                            settings_content.project.worktree.file_scan_exclusions = value;
                        },
                    }
                    .unimplemented(),
                ),
                metadata: None,
                files: USER,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("File Scan Inclusions"),
                description: localization::localized_str!("Files or globs of files that will be included by HiCodeer, even when ignored by git. This is useful for files that are not tracked by git, but are still important to your project. Note that globs that are overly broad can slow down HiCodeer's file scanning. \"File Scan Exclusions\" takes precedence over these inclusions"),
                field: Box::new(
                    SettingField {
                        organization_override: None,
                        json_path: Some("file_scan_inclusions"),
                        pick: |settings_content| {
                            settings_content
                                .project
                                .worktree
                                .file_scan_inclusions
                                .as_ref()
                        },
                        write: |settings_content, value, _| {
                            settings_content.project.worktree.file_scan_inclusions = value;
                        },
                    }
                    .unimplemented(),
                ),
                metadata: None,
                files: USER,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("File Scan Depth"),
                description: localization::localized_str!("Maximum directory depth to eagerly index outside of git repositories; contents of directories at this depth or deeper are indexed on demand. Repositories rooted shallower than this depth are always indexed fully. In projects that are not rooted at a git repository, repositories directly inside a root folder activate their git features immediately; deeper ones activate on first use. 0 means no limit and activates all git repositories immediately"),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("file_scan_depth"),
                    pick: |settings_content| {
                        settings_content.project.worktree.file_scan_depth.as_ref()
                    },
                    write: |settings_content, value, _| {
                        settings_content.project.worktree.file_scan_depth = value;
                    },
                }),
                metadata: None,
                files: USER | PROJECT,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Scan Symbolic Links"),
                description: localization::localized_str!("When to scan content of linked directories"),
                field: Box::new(SettingField {
                    json_path: Some("scan_symlinks"),
                    organization_override: None,
                    pick: |settings_content| {
                        settings_content.project.worktree.scan_symlinks.as_ref()
                    },
                    write: |settings_content, value, _| {
                        settings_content.project.worktree.scan_symlinks = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Restore File State"),
                description: localization::localized_str!("Restore previous file state when reopening."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("restore_on_file_reopen"),
                    pick: |settings_content| {
                        settings_content.workspace.restore_on_file_reopen.as_ref()
                    },
                    write: |settings_content, value, _| {
                        settings_content.workspace.restore_on_file_reopen = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Close on File Delete"),
                description: localization::localized_str!("Automatically close files that have been deleted."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("close_on_file_delete"),
                    pick: |settings_content| {
                        settings_content.workspace.close_on_file_delete.as_ref()
                    },
                    write: |settings_content, value, _| {
                        settings_content.workspace.close_on_file_delete = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
        ]
    }

    SettingsPage {
        title: localization::localized_str!("Search & Files"),
        items: concat_sections![
            search_section(),
            command_palette_section(),
            file_finder_section(),
            file_scan_section(),
        ],
    }
}

fn window_and_layout_page() -> SettingsPage {
    fn status_bar_section() -> [SettingsPageItem; 11] {
        [
            SettingsPageItem::SectionHeader(localization::localized_str!("Status Bar")),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Project Panel Button"),
                description: localization::localized_str!("Show the project panel button in the status bar."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("project_panel.button"),
                    pick: |settings_content| {
                        settings_content.project_panel.as_ref()?.button.as_ref()
                    },
                    write: |settings_content, value, _| {
                        settings_content
                            .project_panel
                            .get_or_insert_default()
                            .button = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Active Language Button"),
                description: localization::localized_str!("Show the active language button in the status bar."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("status_bar.active_language_button"),
                    pick: |settings_content| {
                        settings_content
                            .status_bar
                            .as_ref()?
                            .active_language_button
                            .as_ref()
                    },
                    write: |settings_content, value, _| {
                        settings_content
                            .status_bar
                            .get_or_insert_default()
                            .active_language_button = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Active Encoding Button"),
                description: localization::localized_str!("Control when to show the active encoding in the status bar."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("status_bar.active_encoding_button"),
                    pick: |settings_content| {
                        settings_content
                            .status_bar
                            .as_ref()?
                            .active_encoding_button
                            .as_ref()
                    },
                    write: |settings_content, value, _| {
                        settings_content
                            .status_bar
                            .get_or_insert_default()
                            .active_encoding_button = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Cursor Position Button"),
                description: localization::localized_str!("Show the cursor position button in the status bar."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("status_bar.cursor_position_button"),
                    pick: |settings_content| {
                        settings_content
                            .status_bar
                            .as_ref()?
                            .cursor_position_button
                            .as_ref()
                    },
                    write: |settings_content, value, _| {
                        settings_content
                            .status_bar
                            .get_or_insert_default()
                            .cursor_position_button = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Line Endings Button"),
                description: localization::localized_str!("Show the active line endings button in the status bar."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("status_bar.line_endings_button"),
                    pick: |settings_content| {
                        settings_content
                            .status_bar
                            .as_ref()?
                            .line_endings_button
                            .as_ref()
                    },
                    write: |settings_content, value, _| {
                        settings_content
                            .status_bar
                            .get_or_insert_default()
                            .line_endings_button = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Pending Keystrokes Indicator"),
                description: localization::localized_str!("Show an indicator while a multi-stroke key binding is pending. If the input has a timeout, a countdown is shown and hovering pauses it. Its binding preview popover is disabled when the which-key menu is enabled."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("status_bar.pending_keystrokes_indicator"),
                    pick: |settings_content| {
                        settings_content
                            .status_bar
                            .as_ref()?
                            .pending_keystrokes_indicator
                            .as_ref()
                    },
                    write: |settings_content, value, _| {
                        settings_content
                            .status_bar
                            .get_or_insert_default()
                            .pending_keystrokes_indicator = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Terminal Button"),
                description: localization::localized_str!("Show the terminal button in the status bar."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("terminal.button"),
                    pick: |settings_content| settings_content.terminal.as_ref()?.button.as_ref(),
                    write: |settings_content, value, _| {
                        settings_content.terminal.get_or_insert_default().button = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Diagnostics Button"),
                description: localization::localized_str!("Show the project diagnostics button in the status bar."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("diagnostics.button"),
                    pick: |settings_content| settings_content.diagnostics.as_ref()?.button.as_ref(),
                    write: |settings_content, value, _| {
                        settings_content.diagnostics.get_or_insert_default().button = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Project Search Button"),
                description: localization::localized_str!("Show the project search button in the status bar."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("search.button"),
                    pick: |settings_content| {
                        settings_content.editor.search.as_ref()?.button.as_ref()
                    },
                    write: |settings_content, value, _| {
                        settings_content
                            .editor
                            .search
                            .get_or_insert_default()
                            .button = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Active File Name"),
                description: localization::localized_str!("Show the name of the active file in the status bar."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("status_bar.show_active_file"),
                    pick: |settings_content| {
                        settings_content
                            .status_bar
                            .as_ref()?
                            .show_active_file
                            .as_ref()
                    },
                    write: |settings_content, value, _| {
                        settings_content
                            .status_bar
                            .get_or_insert_default()
                            .show_active_file = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
        ]
    }

    // [OHOS PORT BEGIN] This build has no title bar, so none of its options
    // apply.
    #[cfg(not(target_env = "ohos"))]
    fn title_bar_section() -> Vec<SettingsPageItem> {
        vec![
            SettingsPageItem::SectionHeader(localization::localized_str!("Title Bar")),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Show Branch Status Icon"),
                description: localization::localized_str!("Show git status indicators on the branch icon in the titlebar."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("title_bar.show_branch_status_icon"),
                    pick: |settings_content| {
                        settings_content
                            .title_bar
                            .as_ref()?
                            .show_branch_status_icon
                            .as_ref()
                    },
                    write: |settings_content, value, _| {
                        settings_content
                            .title_bar
                            .get_or_insert_default()
                            .show_branch_status_icon = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Show Branch Name"),
                description: localization::localized_str!("Show the branch name button in the titlebar."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("title_bar.show_branch_name"),
                    pick: |settings_content| {
                        settings_content
                            .title_bar
                            .as_ref()?
                            .show_branch_name
                            .as_ref()
                    },
                    write: |settings_content, value, _| {
                        settings_content
                            .title_bar
                            .get_or_insert_default()
                            .show_branch_name = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Show Worktree Name"),
                description: localization::localized_str!("Show the worktree name button in the titlebar."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("title_bar.show_worktree_name"),
                    pick: |settings_content| {
                        settings_content
                            .title_bar
                            .as_ref()?
                            .show_worktree_name
                            .as_ref()
                    },
                    write: |settings_content, value, _| {
                        settings_content
                            .title_bar
                            .get_or_insert_default()
                            .show_worktree_name = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Show Project Items"),
                description: localization::localized_str!("Show the project host and name in the titlebar."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("title_bar.show_project_items"),
                    pick: |settings_content| {
                        settings_content
                            .title_bar
                            .as_ref()?
                            .show_project_items
                            .as_ref()
                    },
                    write: |settings_content, value, _| {
                        settings_content
                            .title_bar
                            .get_or_insert_default()
                            .show_project_items = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Show Onboarding Banner"),
                description: localization::localized_str!("Show banners announcing new features in the titlebar."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("title_bar.show_onboarding_banner"),
                    pick: |settings_content| {
                        settings_content
                            .title_bar
                            .as_ref()?
                            .show_onboarding_banner
                            .as_ref()
                    },
                    write: |settings_content, value, _| {
                        settings_content
                            .title_bar
                            .get_or_insert_default()
                            .show_onboarding_banner = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Show User Menu"),
                description: localization::localized_str!("Show the user menu button in the titlebar."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("title_bar.show_user_menu"),
                    pick: |settings_content| {
                        settings_content.title_bar.as_ref()?.show_user_menu.as_ref()
                    },
                    write: |settings_content, value, _| {
                        settings_content
                            .title_bar
                            .get_or_insert_default()
                            .show_user_menu = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
            SettingsPageItem::DynamicItem(DynamicItem {
                discriminant: SettingItem {
                    files: USER,
                    title: localization::localized_str!("Button Layout"),
                    description:
                        localization::localized_str!("(Linux only) choose how window control buttons are laid out in the titlebar."),
                    field: Box::new(SettingField {
                        organization_override: None,
                        json_path: Some("title_bar.button_layout$"),
                        pick: |settings_content| {
                            Some(
                                &dynamic_variants::<settings::WindowButtonLayoutContent>()[settings_content
                                    .title_bar
                                    .as_ref()?
                                    .button_layout
                                    .as_ref()?
                                    .discriminant()
                                    as usize],
                            )
                        },
                        write: |settings_content, value, _| {
                            let Some(value) = value else {
                                settings_content
                                    .title_bar
                                    .get_or_insert_default()
                                    .button_layout = None;
                                return;
                            };

                            let current_custom_layout = settings_content
                                .title_bar
                                .as_ref()
                                .and_then(|title_bar| title_bar.button_layout.as_ref())
                                .and_then(|button_layout| match button_layout {
                                    settings::WindowButtonLayoutContent::Custom(layout) => {
                                        Some(layout.clone())
                                    }
                                    _ => None,
                                });

                            let button_layout = match value {
                                settings::WindowButtonLayoutContentDiscriminants::PlatformDefault => {
                                    settings::WindowButtonLayoutContent::PlatformDefault
                                }
                                settings::WindowButtonLayoutContentDiscriminants::Standard => {
                                    settings::WindowButtonLayoutContent::Standard
                                }
                                settings::WindowButtonLayoutContentDiscriminants::Custom => {
                                    settings::WindowButtonLayoutContent::Custom(
                                        current_custom_layout.unwrap_or_else(|| {
                                            "close:minimize,maximize".to_string()
                                        }),
                                    )
                                }
                            };

                            settings_content
                                .title_bar
                                .get_or_insert_default()
                                .button_layout = Some(button_layout);
                        },
                    }),
                    metadata: None,
                },
                pick_discriminant: |settings_content| {
                    Some(
                        settings_content
                            .title_bar
                            .as_ref()?
                            .button_layout
                            .as_ref()?
                            .discriminant() as usize,
                    )
                },
                fields: dynamic_variants::<settings::WindowButtonLayoutContent>()
                    .into_iter()
                    .map(|variant| match variant {
                        settings::WindowButtonLayoutContentDiscriminants::PlatformDefault => {
                            vec![]
                        }
                        settings::WindowButtonLayoutContentDiscriminants::Standard => vec![],
                        settings::WindowButtonLayoutContentDiscriminants::Custom => vec![
                            SettingItem {
                                files: USER,
                                title: localization::localized_str!("Custom Button Layout"),
                                description:
                                    "GNOME-style layout string such as \"close:minimize,maximize\".",
                                field: Box::new(SettingField {
                                    organization_override: None,
                                    json_path: Some("title_bar.button_layout"),
                                    pick: |settings_content| match settings_content
                                        .title_bar
                                        .as_ref()?
                                        .button_layout
                                        .as_ref()?
                                    {
                                        settings::WindowButtonLayoutContent::Custom(layout) => {
                                            Some(layout)
                                        }
                                        _ => DEFAULT_EMPTY_STRING,
                                    },
                                    write: |settings_content, value, _| {
                                        settings_content
                                            .title_bar
                                            .get_or_insert_default()
                                            .button_layout = value
                                            .map(settings::WindowButtonLayoutContent::Custom);
                                    },
                                }),
                                metadata: Some(Box::new(SettingsFieldMetadata {
                                    placeholder: Some("close:minimize,maximize"),
                                    ..Default::default()
                                })),
                            },
                        ],
                    })
                    .collect(),
            }),
        ]
    }

    fn tab_bar_section() -> [SettingsPageItem; 9] {
        [
            SettingsPageItem::SectionHeader(localization::localized_str!("Tab Bar")),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Show Tab Bar"),
                description: localization::localized_str!("Show the tab bar in the editor."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("tab_bar.show"),
                    pick: |settings_content| settings_content.tab_bar.as_ref()?.show.as_ref(),
                    write: |settings_content, value, _| {
                        settings_content.tab_bar.get_or_insert_default().show = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Show Git Status In Tabs"),
                description: localization::localized_str!("Show the Git file status on a tab item."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("tabs.git_status"),
                    pick: |settings_content| settings_content.tabs.as_ref()?.git_status.as_ref(),
                    write: |settings_content, value, _| {
                        settings_content.tabs.get_or_insert_default().git_status = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Show File Icons In Tabs"),
                description: localization::localized_str!("Show the file icon for a tab."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("tabs.file_icons"),
                    pick: |settings_content| settings_content.tabs.as_ref()?.file_icons.as_ref(),
                    write: |settings_content, value, _| {
                        settings_content.tabs.get_or_insert_default().file_icons = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Tab Close Position"),
                description: localization::localized_str!("Position of the close button in a tab."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("tabs.close_position"),
                    pick: |settings_content| {
                        settings_content.tabs.as_ref()?.close_position.as_ref()
                    },
                    write: |settings_content, value, _| {
                        settings_content.tabs.get_or_insert_default().close_position = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                files: USER,
                title: localization::localized_str!("Maximum Tabs"),
                description: localization::localized_str!("Maximum open tabs in a pane. Will not close an unsaved tab."),
                // todo(settings_ui): The default for this value is null and it's use in code
                // is complex, so I'm going to come back to this later
                field: Box::new(
                    SettingField {
                        organization_override: None,
                        json_path: Some("max_tabs"),
                        pick: |settings_content| settings_content.workspace.max_tabs.as_ref(),
                        write: |settings_content, value, _| {
                            settings_content.workspace.max_tabs = value;
                        },
                    }
                    .unimplemented(),
                ),
                metadata: None,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Show Navigation History Buttons"),
                description: localization::localized_str!("Show the navigation history buttons in the tab bar."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("tab_bar.show_nav_history_buttons"),
                    pick: |settings_content| {
                        settings_content
                            .tab_bar
                            .as_ref()?
                            .show_nav_history_buttons
                            .as_ref()
                    },
                    write: |settings_content, value, _| {
                        settings_content
                            .tab_bar
                            .get_or_insert_default()
                            .show_nav_history_buttons = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Show Tab Bar Buttons"),
                description: localization::localized_str!("Show the tab bar buttons (New, Split Pane, Zoom)."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("tab_bar.show_tab_bar_buttons"),
                    pick: |settings_content| {
                        settings_content
                            .tab_bar
                            .as_ref()?
                            .show_tab_bar_buttons
                            .as_ref()
                    },
                    write: |settings_content, value, _| {
                        settings_content
                            .tab_bar
                            .get_or_insert_default()
                            .show_tab_bar_buttons = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Pinned Tabs Layout"),
                description: localization::localized_str!("Show pinned tabs in a separate row above unpinned tabs."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("tab_bar.show_pinned_tabs_in_separate_row"),
                    pick: |settings_content| {
                        settings_content
                            .tab_bar
                            .as_ref()?
                            .show_pinned_tabs_in_separate_row
                            .as_ref()
                    },
                    write: |settings_content, value, _| {
                        settings_content
                            .tab_bar
                            .get_or_insert_default()
                            .show_pinned_tabs_in_separate_row = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
        ]
    }

    fn tab_settings_section() -> [SettingsPageItem; 4] {
        [
            SettingsPageItem::SectionHeader(localization::localized_str!("Tab Settings")),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Activate On Close"),
                description: localization::localized_str!("What to do after closing the current tab."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("tabs.activate_on_close"),
                    pick: |settings_content| {
                        settings_content.tabs.as_ref()?.activate_on_close.as_ref()
                    },
                    write: |settings_content, value, _| {
                        settings_content
                            .tabs
                            .get_or_insert_default()
                            .activate_on_close = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Tab Show Diagnostics"),
                description: localization::localized_str!("Which files containing diagnostic errors/warnings to mark in the tabs."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("tabs.show_diagnostics"),
                    pick: |settings_content| {
                        settings_content.tabs.as_ref()?.show_diagnostics.as_ref()
                    },
                    write: |settings_content, value, _| {
                        settings_content
                            .tabs
                            .get_or_insert_default()
                            .show_diagnostics = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Show Close Button"),
                description: localization::localized_str!("Controls the appearance behavior of the tab's close button."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("tabs.show_close_button"),
                    pick: |settings_content| {
                        settings_content.tabs.as_ref()?.show_close_button.as_ref()
                    },
                    write: |settings_content, value, _| {
                        settings_content
                            .tabs
                            .get_or_insert_default()
                            .show_close_button = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
        ]
    }

    fn preview_tabs_section() -> [SettingsPageItem; 8] {
        [
            SettingsPageItem::SectionHeader(localization::localized_str!("Preview Tabs")),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Preview Tabs Enabled"),
                description: localization::localized_str!("Show opened editors as preview tabs."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("preview_tabs.enabled"),
                    pick: |settings_content| {
                        settings_content.preview_tabs.as_ref()?.enabled.as_ref()
                    },
                    write: |settings_content, value, _| {
                        settings_content
                            .preview_tabs
                            .get_or_insert_default()
                            .enabled = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Enable Preview From Project Panel"),
                description: localization::localized_str!("Whether to open tabs in preview mode when opened from the project panel with a single click or the Open action."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("preview_tabs.enable_preview_from_project_panel"),
                    pick: |settings_content| {
                        settings_content
                            .preview_tabs
                            .as_ref()?
                            .enable_preview_from_project_panel
                            .as_ref()
                    },
                    write: |settings_content, value, _| {
                        settings_content
                            .preview_tabs
                            .get_or_insert_default()
                            .enable_preview_from_project_panel = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Enable Preview From File Finder"),
                description: localization::localized_str!("Whether to open tabs in preview mode when selected from the file finder."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("preview_tabs.enable_preview_from_file_finder"),
                    pick: |settings_content| {
                        settings_content
                            .preview_tabs
                            .as_ref()?
                            .enable_preview_from_file_finder
                            .as_ref()
                    },
                    write: |settings_content, value, _| {
                        settings_content
                            .preview_tabs
                            .get_or_insert_default()
                            .enable_preview_from_file_finder = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Enable Preview From Multibuffer"),
                description: localization::localized_str!("Whether to open tabs in preview mode when opened from a multibuffer."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("preview_tabs.enable_preview_from_multibuffer"),
                    pick: |settings_content| {
                        settings_content
                            .preview_tabs
                            .as_ref()?
                            .enable_preview_from_multibuffer
                            .as_ref()
                    },
                    write: |settings_content, value, _| {
                        settings_content
                            .preview_tabs
                            .get_or_insert_default()
                            .enable_preview_from_multibuffer = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Enable Preview Multibuffer From Code Navigation"),
                description: localization::localized_str!("Whether to open tabs in preview mode when code navigation is used to open a multibuffer."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("preview_tabs.enable_preview_multibuffer_from_code_navigation"),
                    pick: |settings_content| {
                        settings_content
                            .preview_tabs
                            .as_ref()?
                            .enable_preview_multibuffer_from_code_navigation
                            .as_ref()
                    },
                    write: |settings_content, value, _| {
                        settings_content
                            .preview_tabs
                            .get_or_insert_default()
                            .enable_preview_multibuffer_from_code_navigation = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Enable Preview File From Code Navigation"),
                description: localization::localized_str!("Whether to open tabs in preview mode when code navigation is used to open a single file."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("preview_tabs.enable_preview_file_from_code_navigation"),
                    pick: |settings_content| {
                        settings_content
                            .preview_tabs
                            .as_ref()?
                            .enable_preview_file_from_code_navigation
                            .as_ref()
                    },
                    write: |settings_content, value, _| {
                        settings_content
                            .preview_tabs
                            .get_or_insert_default()
                            .enable_preview_file_from_code_navigation = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Enable Keep Preview On Code Navigation"),
                description: localization::localized_str!("Whether to keep tabs in preview mode when code navigation is used to navigate away from them. If `enable_preview_file_from_code_navigation` or `enable_preview_multibuffer_from_code_navigation` is also true, the new tab may replace the existing one."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("preview_tabs.enable_keep_preview_on_code_navigation"),
                    pick: |settings_content| {
                        settings_content
                            .preview_tabs
                            .as_ref()?
                            .enable_keep_preview_on_code_navigation
                            .as_ref()
                    },
                    write: |settings_content, value, _| {
                        settings_content
                            .preview_tabs
                            .get_or_insert_default()
                            .enable_keep_preview_on_code_navigation = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
        ]
    }

    fn layout_section() -> [SettingsPageItem; 6] {
        [
            SettingsPageItem::SectionHeader(localization::localized_str!("Layout")),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Bottom Dock Layout"),
                description: localization::localized_str!("Layout mode for the bottom dock."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("bottom_dock_layout"),
                    pick: |settings_content| settings_content.workspace.bottom_dock_layout.as_ref(),
                    write: |settings_content, value, _| {
                        settings_content.workspace.bottom_dock_layout = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                files: USER,
                title: localization::localized_str!("Centered Layout Left Padding"),
                description: localization::localized_str!("Left padding for centered layout."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("centered_layout.left_padding"),
                    pick: |settings_content| {
                        settings_content
                            .workspace
                            .centered_layout
                            .as_ref()?
                            .left_padding
                            .as_ref()
                    },
                    write: |settings_content, value, _| {
                        settings_content
                            .workspace
                            .centered_layout
                            .get_or_insert_default()
                            .left_padding = value;
                    },
                }),
                metadata: None,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                files: USER,
                title: localization::localized_str!("Centered Layout Right Padding"),
                description: localization::localized_str!("Right padding for centered layout."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("centered_layout.right_padding"),
                    pick: |settings_content| {
                        settings_content
                            .workspace
                            .centered_layout
                            .as_ref()?
                            .right_padding
                            .as_ref()
                    },
                    write: |settings_content, value, _| {
                        settings_content
                            .workspace
                            .centered_layout
                            .get_or_insert_default()
                            .right_padding = value;
                    },
                }),
                metadata: None,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Focus Follows Mouse"),
                description: localization::localized_str!("Whether to change focus to a pane when the mouse hovers over it."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("focus_follows_mouse.enabled"),
                    pick: |settings_content| {
                        settings_content
                            .workspace
                            .focus_follows_mouse
                            .as_ref()
                            .and_then(|s| s.enabled.as_ref())
                    },
                    write: |settings_content, value, _| {
                        settings_content
                            .workspace
                            .focus_follows_mouse
                            .get_or_insert_default()
                            .enabled = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Focus Follows Mouse Debounce ms"),
                description: localization::localized_str!("Amount of time to wait before changing focus."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("focus_follows_mouse.debounce_ms"),
                    pick: |settings_content| {
                        settings_content
                            .workspace
                            .focus_follows_mouse
                            .as_ref()
                            .and_then(|s| s.debounce_ms.as_ref())
                    },
                    write: |settings_content, value, _| {
                        settings_content
                            .workspace
                            .focus_follows_mouse
                            .get_or_insert_default()
                            .debounce_ms = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
        ]
    }

    fn window_section() -> Vec<SettingsPageItem> {
        vec![
            SettingsPageItem::SectionHeader(localization::localized_str!("Window")),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Title Format"),
                description: localization::localized_str!("Window title template. Available variables are `${projectName}`, `${fileName}`, `${filePath}`, `${relativePath}`, `${fileStem}`, `${remoteName}`, `${remoteHost}`, `${appName}`, `${branch}`, and `${separator}`. `${separator}` is omitted when adjacent variables are empty, but literal text is preserved. The collaboration indicator, when present, is appended after the rendered template. If the template renders to nothing, the default template is used instead."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("window_title_format"),
                    pick: |settings_content| {
                        settings_content.workspace.window_title_format.as_ref()
                    },
                    write: |settings_content, value, _| {
                        settings_content.workspace.window_title_format =
                            value.filter(|format| !format.is_empty());
                    },
                }),
                metadata: Some(Box::new(SettingsFieldMetadata {
                    placeholder: Some("${projectName}${separator}${fileName}"),
                    ..Default::default()
                })),
                files: USER,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Title Separator"),
                description: localization::localized_str!("String substituted for `${separator}` in the window title format. Include any surrounding whitespace in the value."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("window_title_separator"),
                    pick: |settings_content| {
                        settings_content.workspace.window_title_separator.as_ref()
                    },
                    write: |settings_content, value, _| {
                        settings_content.workspace.window_title_separator = value;
                    },
                }),
                metadata: Some(Box::new(SettingsFieldMetadata {
                    placeholder: Some(" — "),
                    ..Default::default()
                })),
                files: USER,
            }),
            // todo(settings_ui): Should we filter by platform.as_ref()?
            // [OHOS PORT BEGIN] macOS window tabbing and fullscreen modes, and
            // compositor-drawn decorations on Linux, all have no equivalent here.
            #[cfg(not(target_env = "ohos"))]
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Use System Window Tabs"),
                description: localization::localized_str!("(macOS only) whether to allow Windows to tab together."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("use_system_window_tabs"),
                    pick: |settings_content| {
                        settings_content.workspace.use_system_window_tabs.as_ref()
                    },
                    write: |settings_content, value, _| {
                        settings_content.workspace.use_system_window_tabs = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
            #[cfg(not(target_env = "ohos"))]
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Fullscreen Mode"),
                description: localization::localized_str!("(macOS only) which fullscreen mode the toggle fullscreen action enters."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("fullscreen_mode"),
                    pick: |settings_content| settings_content.workspace.fullscreen_mode.as_ref(),
                    write: |settings_content, value, _| {
                        settings_content.workspace.fullscreen_mode = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
            #[cfg(not(target_env = "ohos"))]
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Window Decorations"),
                description: localization::localized_str!("(Linux only) whether HiCodeer or your compositor should draw window decorations."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("window_decorations"),
                    pick: |settings_content| settings_content.workspace.window_decorations.as_ref(),
                    write: |settings_content, value, _| {
                        settings_content.workspace.window_decorations = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
            // [OHOS PORT END]
        ]
    }

    fn pane_modifiers_section() -> [SettingsPageItem; 5] {
        [
            SettingsPageItem::SectionHeader(localization::localized_str!("Pane Modifiers")),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Inactive Opacity"),
                description: localization::localized_str!("Opacity of inactive panels (0.0 - 1.0)."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("active_pane_modifiers.inactive_opacity"),
                    pick: |settings_content| {
                        settings_content
                            .workspace
                            .active_pane_modifiers
                            .as_ref()?
                            .inactive_opacity
                            .as_ref()
                    },
                    write: |settings_content, value, _| {
                        settings_content
                            .workspace
                            .active_pane_modifiers
                            .get_or_insert_default()
                            .inactive_opacity = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Border Size"),
                description: localization::localized_str!("Size of the border surrounding the active pane."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("active_pane_modifiers.border_size"),
                    pick: |settings_content| {
                        settings_content
                            .workspace
                            .active_pane_modifiers
                            .as_ref()?
                            .border_size
                            .as_ref()
                    },
                    write: |settings_content, value, _| {
                        settings_content
                            .workspace
                            .active_pane_modifiers
                            .get_or_insert_default()
                            .border_size = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Zoomed Padding"),
                description: localization::localized_str!("Show padding for zoomed panes."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("zoomed_padding"),
                    pick: |settings_content| settings_content.workspace.zoomed_padding.as_ref(),
                    write: |settings_content, value, _| {
                        settings_content.workspace.zoomed_padding = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Close Panel on Toggle"),
                description: localization::localized_str!("Whether invoking a panel's ToggleFocus action while it's already focused closes the panel, instead of just moving focus back to the editor."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("close_panel_on_toggle"),
                    pick: |settings_content| {
                        settings_content.workspace.close_panel_on_toggle.as_ref()
                    },
                    write: |settings_content, value, _| {
                        settings_content.workspace.close_panel_on_toggle = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
        ]
    }

    fn pane_split_direction_section() -> [SettingsPageItem; 3] {
        [
            SettingsPageItem::SectionHeader(localization::localized_str!("Pane Split Direction")),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Vertical Split Direction"),
                description: localization::localized_str!("Direction to split vertically."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("pane_split_direction_vertical"),
                    pick: |settings_content| {
                        settings_content
                            .workspace
                            .pane_split_direction_vertical
                            .as_ref()
                    },
                    write: |settings_content, value, _| {
                        settings_content.workspace.pane_split_direction_vertical = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Horizontal Split Direction"),
                description: localization::localized_str!("Direction to split horizontally."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("pane_split_direction_horizontal"),
                    pick: |settings_content| {
                        settings_content
                            .workspace
                            .pane_split_direction_horizontal
                            .as_ref()
                    },
                    write: |settings_content, value, _| {
                        settings_content.workspace.pane_split_direction_horizontal = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
        ]
    }

    let mut items: Vec<SettingsPageItem> = concat_sections![status_bar_section()].into_vec();
    // [OHOS PORT BEGIN] This build has no title bar, so its options are not
    // offered.
    #[cfg(not(target_env = "ohos"))]
    items.extend(title_bar_section());
    // [OHOS PORT END]
    items.extend(concat_sections![
        tab_bar_section(),
        tab_settings_section(),
        preview_tabs_section(),
        layout_section(),
        window_section(),
        pane_modifiers_section(),
        pane_split_direction_section(),
    ]);

    SettingsPage {
        title: localization::localized_str!("Window & Layout"),
        items: items.into(),
    }
}

fn panels_page() -> SettingsPage {
    fn project_panel_section() -> [SettingsPageItem; 30] {
        [
            SettingsPageItem::SectionHeader(localization::localized_str!("Project Panel")),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Project Panel Dock"),
                description: localization::localized_str!("Where to dock the project panel."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("project_panel.dock"),
                    pick: |settings_content| settings_content.project_panel.as_ref()?.dock.as_ref(),
                    write: |settings_content, value, _| {
                        settings_content.project_panel.get_or_insert_default().dock = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Project Panel Default Width"),
                description: localization::localized_str!("Default width of the project panel in pixels."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("project_panel.default_width"),
                    pick: |settings_content| {
                        settings_content
                            .project_panel
                            .as_ref()?
                            .default_width
                            .as_ref()
                    },
                    write: |settings_content, value, _| {
                        settings_content
                            .project_panel
                            .get_or_insert_default()
                            .default_width = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
            SettingsPageItem::DynamicItem(DynamicItem {
                discriminant: SettingItem {
                    title: localization::localized_str!("Project Panel Title Tooltips Delay"),
                    description: localization::localized_str!("Delay in milliseconds before tooltips appear for project panel titles."),
                    field: Box::new(SettingField {
                        organization_override: None,
                        json_path: Some("project_panel.title_tooltip_delay$"),
                        pick: |settings_content| {
                            Some(
                                &dynamic_variants::<settings::ProjectPanelTitleTooltipDelay>()
                                    [settings_content
                                        .project_panel
                                        .as_ref()?
                                        .title_tooltip_delay
                                        .as_ref()?
                                        .discriminant()
                                        as usize],
                            )
                        },
                        write: |settings_content, value, _| {
                            let project_panel =
                                settings_content.project_panel.get_or_insert_default();
                            project_panel.title_tooltip_delay = value.map(|value| match value {
                                settings::ProjectPanelTitleTooltipDelayDiscriminants::Default => {
                                    settings::ProjectPanelTitleTooltipDelay::Default
                                }
                                settings::ProjectPanelTitleTooltipDelayDiscriminants::Disabled => {
                                    settings::ProjectPanelTitleTooltipDelay::Disabled
                                }
                                settings::ProjectPanelTitleTooltipDelayDiscriminants::Custom => {
                                    let delay = match project_panel.title_tooltip_delay {
                                        Some(settings::ProjectPanelTitleTooltipDelay::Custom(
                                            delay,
                                        )) => settings::DelayMs(delay.0),
                                        _ => settings::DelayMs(1500),
                                    };
                                    settings::ProjectPanelTitleTooltipDelay::Custom(delay)
                                }
                            });
                        },
                    }),
                    metadata: None,
                    files: USER,
                },
                pick_discriminant: |settings_content| {
                    Some(
                        settings_content
                            .project_panel
                            .as_ref()?
                            .title_tooltip_delay
                            .as_ref()?
                            .discriminant() as usize,
                    )
                },
                fields: dynamic_variants::<settings::ProjectPanelTitleTooltipDelay>()
                    .into_iter()
                    .map(|variant| match variant {
                        settings::ProjectPanelTitleTooltipDelayDiscriminants::Default => vec![],
                        settings::ProjectPanelTitleTooltipDelayDiscriminants::Disabled => vec![],
                        settings::ProjectPanelTitleTooltipDelayDiscriminants::Custom => {
                            vec![SettingItem {
                                files: USER,
                                title: localization::localized_str!("Custom Delay"),
                                description: localization::localized_str!("Delay in milliseconds of the project panel title tooltips."),
                                field: Box::new(SettingField {
                                    organization_override: None,
                                    json_path: Some("project_panel.title_tooltip_delay"),
                                    pick: |settings_content| match settings_content
                                        .project_panel
                                        .as_ref()
                                        .and_then(|project_panel| {
                                            project_panel.title_tooltip_delay.as_ref()
                                        }) {
                                        Some(settings::ProjectPanelTitleTooltipDelay::Custom(
                                            value,
                                        )) => Some(value),
                                        _ => None,
                                    },
                                    write: |settings_content, value, _| {
                                        let Some(value) = value else {
                                            return;
                                        };
                                        if let Some(
                                            settings::ProjectPanelTitleTooltipDelay::Custom(width),
                                        ) = settings_content.project_panel.as_mut().and_then(
                                            |project_panel| {
                                                project_panel.title_tooltip_delay.as_mut()
                                            },
                                        ) {
                                            *width = value;
                                        }
                                    },
                                }),
                                metadata: None,
                            }]
                        }
                    })
                    .collect(),
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Hide .gitignore"),
                description: localization::localized_str!("Whether to hide the gitignore entries in the project panel."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("project_panel.hide_gitignore"),
                    pick: |settings_content| {
                        settings_content
                            .project_panel
                            .as_ref()?
                            .hide_gitignore
                            .as_ref()
                    },
                    write: |settings_content, value, _| {
                        settings_content
                            .project_panel
                            .get_or_insert_default()
                            .hide_gitignore = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Entry Spacing"),
                description: localization::localized_str!("Spacing between worktree entries in the project panel."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("project_panel.entry_spacing"),
                    pick: |settings_content| {
                        settings_content
                            .project_panel
                            .as_ref()?
                            .entry_spacing
                            .as_ref()
                    },
                    write: |settings_content, value, _| {
                        settings_content
                            .project_panel
                            .get_or_insert_default()
                            .entry_spacing = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("File Icons"),
                description: localization::localized_str!("Show file icons in the project panel."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("project_panel.file_icons"),
                    pick: |settings_content| {
                        settings_content.project_panel.as_ref()?.file_icons.as_ref()
                    },
                    write: |settings_content, value, _| {
                        settings_content
                            .project_panel
                            .get_or_insert_default()
                            .file_icons = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Folder Indicator"),
                description: localization::localized_str!("What to show for directories in the project panel."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("project_panel.folder_indicator"),
                    pick: |settings_content| {
                        settings_content
                            .project_panel
                            .as_ref()?
                            .folder_indicator
                            .as_ref()
                    },
                    write: |settings_content, value, _| {
                        settings_content
                            .project_panel
                            .get_or_insert_default()
                            .folder_indicator = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Git Status"),
                description: localization::localized_str!("Show the Git status in the project panel."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("project_panel.git_status"),
                    pick: |settings_content| {
                        settings_content.project_panel.as_ref()?.git_status.as_ref()
                    },
                    write: |settings_content, value, _| {
                        settings_content
                            .project_panel
                            .get_or_insert_default()
                            .git_status = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Indent Size"),
                description: localization::localized_str!("Amount of indentation for nested items."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("project_panel.indent_size"),
                    pick: |settings_content| {
                        settings_content
                            .project_panel
                            .as_ref()?
                            .indent_size
                            .as_ref()
                    },
                    write: |settings_content, value, _| {
                        settings_content
                            .project_panel
                            .get_or_insert_default()
                            .indent_size = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Auto Reveal Entries"),
                description: localization::localized_str!("Whether to reveal entries in the project panel automatically when a corresponding project entry becomes active."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("project_panel.auto_reveal_entries"),
                    pick: |settings_content| {
                        settings_content
                            .project_panel
                            .as_ref()?
                            .auto_reveal_entries
                            .as_ref()
                    },
                    write: |settings_content, value, _| {
                        settings_content
                            .project_panel
                            .get_or_insert_default()
                            .auto_reveal_entries = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Starts Open"),
                description: localization::localized_str!("Whether the project panel should open on startup."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("project_panel.starts_open"),
                    pick: |settings_content| {
                        settings_content
                            .project_panel
                            .as_ref()?
                            .starts_open
                            .as_ref()
                    },
                    write: |settings_content, value, _| {
                        settings_content
                            .project_panel
                            .get_or_insert_default()
                            .starts_open = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Auto Fold Directories"),
                description: localization::localized_str!("Whether to fold directories automatically and show compact folders when a directory has only one subdirectory inside."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("project_panel.auto_fold_dirs"),
                    pick: |settings_content| {
                        settings_content
                            .project_panel
                            .as_ref()?
                            .auto_fold_dirs
                            .as_ref()
                    },
                    write: |settings_content, value, _| {
                        settings_content
                            .project_panel
                            .get_or_insert_default()
                            .auto_fold_dirs = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Bold Folder Labels"),
                description: localization::localized_str!("Whether to show folder names with bold text in the project panel."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("project_panel.bold_folder_labels"),
                    pick: |settings_content| {
                        settings_content
                            .project_panel
                            .as_ref()?
                            .bold_folder_labels
                            .as_ref()
                    },
                    write: |settings_content, value, _| {
                        settings_content
                            .project_panel
                            .get_or_insert_default()
                            .bold_folder_labels = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Show Scrollbar"),
                description: localization::localized_str!("Show the scrollbar in the project panel."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("project_panel.scrollbar.show"),
                    pick: |settings_content| {
                        show_scrollbar_or_editor(settings_content, |settings_content| {
                            settings_content
                                .project_panel
                                .as_ref()?
                                .scrollbar
                                .as_ref()?
                                .show
                                .as_ref()
                        })
                    },
                    write: |settings_content, value, _| {
                        settings_content
                            .project_panel
                            .get_or_insert_default()
                            .scrollbar
                            .get_or_insert_default()
                            .show = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Horizontal Scroll"),
                description: localization::localized_str!("Whether to allow horizontal scrolling in the project panel. When disabled, the view is always locked to the leftmost position and long file names are clipped."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("project_panel.scrollbar.horizontal_scroll"),
                    pick: |settings_content| {
                        settings_content
                            .project_panel
                            .as_ref()?
                            .scrollbar
                            .as_ref()?
                            .horizontal_scroll
                            .as_ref()
                    },
                    write: |settings_content, value, _| {
                        settings_content
                            .project_panel
                            .get_or_insert_default()
                            .scrollbar
                            .get_or_insert_default()
                            .horizontal_scroll = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Show Diagnostics"),
                description: localization::localized_str!("Which files containing diagnostic errors/warnings to mark in the project panel."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("project_panel.show_diagnostics"),
                    pick: |settings_content| {
                        settings_content
                            .project_panel
                            .as_ref()?
                            .show_diagnostics
                            .as_ref()
                    },
                    write: |settings_content, value, _| {
                        settings_content
                            .project_panel
                            .get_or_insert_default()
                            .show_diagnostics = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Diagnostic Badges"),
                description: localization::localized_str!("Show error and warning count badges next to file names in the project panel."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("project_panel.diagnostic_badges"),
                    pick: |settings_content| {
                        settings_content
                            .project_panel
                            .as_ref()?
                            .diagnostic_badges
                            .as_ref()
                    },
                    write: |settings_content, value, _| {
                        settings_content
                            .project_panel
                            .get_or_insert_default()
                            .diagnostic_badges = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Git Status Indicator"),
                description: localization::localized_str!("Show a git status indicator next to file names in the project panel."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("project_panel.git_status_indicator"),
                    pick: |settings_content| {
                        settings_content
                            .project_panel
                            .as_ref()?
                            .git_status_indicator
                            .as_ref()
                    },
                    write: |settings_content, value, _| {
                        settings_content
                            .project_panel
                            .get_or_insert_default()
                            .git_status_indicator = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Sticky Scroll"),
                description: localization::localized_str!("Whether to stick parent directories at top of the project panel."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("project_panel.sticky_scroll"),
                    pick: |settings_content| {
                        settings_content
                            .project_panel
                            .as_ref()?
                            .sticky_scroll
                            .as_ref()
                    },
                    write: |settings_content, value, _| {
                        settings_content
                            .project_panel
                            .get_or_insert_default()
                            .sticky_scroll = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                files: USER,
                title: localization::localized_str!("Show Indent Guides"),
                description: localization::localized_str!("Show indent guides in the project panel."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("project_panel.indent_guides.show"),
                    pick: |settings_content| {
                        settings_content
                            .project_panel
                            .as_ref()?
                            .indent_guides
                            .as_ref()?
                            .show
                            .as_ref()
                    },
                    write: |settings_content, value, _| {
                        settings_content
                            .project_panel
                            .get_or_insert_default()
                            .indent_guides
                            .get_or_insert_default()
                            .show = value;
                    },
                }),
                metadata: None,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Drag and Drop"),
                description: localization::localized_str!("Whether to enable drag-and-drop operations in the project panel."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("project_panel.drag_and_drop"),
                    pick: |settings_content| {
                        settings_content
                            .project_panel
                            .as_ref()?
                            .drag_and_drop
                            .as_ref()
                    },
                    write: |settings_content, value, _| {
                        settings_content
                            .project_panel
                            .get_or_insert_default()
                            .drag_and_drop = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Hide Root"),
                description: localization::localized_str!("Whether to hide the root entry when only one folder is open in the window."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("project_panel.hide_root"),
                    pick: |settings_content| {
                        settings_content.project_panel.as_ref()?.hide_root.as_ref()
                    },
                    write: |settings_content, value, _| {
                        settings_content
                            .project_panel
                            .get_or_insert_default()
                            .hide_root = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Hide Hidden"),
                description: localization::localized_str!("Whether to hide the hidden entries in the project panel."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("project_panel.hide_hidden"),
                    pick: |settings_content| {
                        settings_content
                            .project_panel
                            .as_ref()?
                            .hide_hidden
                            .as_ref()
                    },
                    write: |settings_content, value, _| {
                        settings_content
                            .project_panel
                            .get_or_insert_default()
                            .hide_hidden = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Sort Mode"),
                description: localization::localized_str!("Sort order for entries in the project panel."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("project_panel.sort_mode"),
                    pick: |settings_content| {
                        settings_content.project_panel.as_ref()?.sort_mode.as_ref()
                    },
                    write: |settings_content, value, _| {
                        settings_content
                            .project_panel
                            .get_or_insert_default()
                            .sort_mode = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Sort Order"),
                description: localization::localized_str!("Whether to sort file and folder names case-sensitively in the project panel."),
                field: Box::new(SettingField {
                    organization_override: None,
                    pick: |settings_content| {
                        settings_content.project_panel.as_ref()?.sort_order.as_ref()
                    },
                    write: |settings_content, value, _| {
                        settings_content
                            .project_panel
                            .get_or_insert_default()
                            .sort_order = value;
                    },
                    json_path: Some("project_panel.sort_order"),
                }),
                metadata: None,
                files: USER,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Auto Open Files On Create"),
                description: localization::localized_str!("Whether to automatically open newly created files in the editor."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("project_panel.auto_open.on_create"),
                    pick: |settings_content| {
                        settings_content
                            .project_panel
                            .as_ref()?
                            .auto_open
                            .as_ref()?
                            .on_create
                            .as_ref()
                    },
                    write: |settings_content, value, _| {
                        settings_content
                            .project_panel
                            .get_or_insert_default()
                            .auto_open
                            .get_or_insert_default()
                            .on_create = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Auto Open Files On Paste"),
                description: localization::localized_str!("Whether to automatically open files after pasting or duplicating them."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("project_panel.auto_open.on_paste"),
                    pick: |settings_content| {
                        settings_content
                            .project_panel
                            .as_ref()?
                            .auto_open
                            .as_ref()?
                            .on_paste
                            .as_ref()
                    },
                    write: |settings_content, value, _| {
                        settings_content
                            .project_panel
                            .get_or_insert_default()
                            .auto_open
                            .get_or_insert_default()
                            .on_paste = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Auto Open Files On Drop"),
                description: localization::localized_str!("Whether to automatically open files dropped from external sources."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("project_panel.auto_open.on_drop"),
                    pick: |settings_content| {
                        settings_content
                            .project_panel
                            .as_ref()?
                            .auto_open
                            .as_ref()?
                            .on_drop
                            .as_ref()
                    },
                    write: |settings_content, value, _| {
                        settings_content
                            .project_panel
                            .get_or_insert_default()
                            .auto_open
                            .get_or_insert_default()
                            .on_drop = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Hidden Files"),
                description: localization::localized_str!("Globs to match files that will be considered \"hidden\" and can be hidden from the project panel."),
                field: Box::new(
                    SettingField {
                        organization_override: None,
                        json_path: Some("hidden_files"),
                        pick: |settings_content| {
                            settings_content.project.worktree.hidden_files.as_ref()
                        },
                        write: |settings_content, value, _| {
                            settings_content.project.worktree.hidden_files = value;
                        },
                    }
                    .unimplemented(),
                ),
                metadata: None,
                files: USER,
            }),
        ]
    }

    fn terminal_panel_section() -> [SettingsPageItem; 5] {
        [
            SettingsPageItem::SectionHeader(localization::localized_str!("Terminal Panel")),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Terminal Dock"),
                description: localization::localized_str!("Where to dock the terminal panel."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("terminal.dock"),
                    pick: |settings_content| settings_content.terminal.as_ref()?.dock.as_ref(),
                    write: |settings_content, value, _| {
                        settings_content.terminal.get_or_insert_default().dock = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Starts Open"),
                description: localization::localized_str!("Whether the terminal panel should open on startup."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("terminal.starts_open"),
                    pick: |settings_content| {
                        settings_content.terminal.as_ref()?.starts_open.as_ref()
                    },
                    write: |settings_content, value, _| {
                        settings_content
                            .terminal
                            .get_or_insert_default()
                            .starts_open = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Terminal Panel Flexible Sizing"),
                description: localization::localized_str!("Whether the terminal panel should use flexible (proportional) sizing when docked to the left or right."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("terminal.flexible"),
                    pick: |settings_content| settings_content.terminal.as_ref()?.flexible.as_ref(),
                    write: |settings_content, value, _| {
                        settings_content.terminal.get_or_insert_default().flexible = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Show Count Badge"),
                description: localization::localized_str!("Show a badge on the terminal panel icon with the count of open terminals."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("terminal.show_count_badge"),
                    pick: |settings_content| {
                        settings_content
                            .terminal
                            .as_ref()?
                            .show_count_badge
                            .as_ref()
                    },
                    write: |settings_content, value, _| {
                        settings_content
                            .terminal
                            .get_or_insert_default()
                            .show_count_badge = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
        ]
    }

    fn outline_panel_section() -> [SettingsPageItem; 12] {
        [
            SettingsPageItem::SectionHeader(localization::localized_str!("Outline Panel")),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Outline Panel Button"),
                description: localization::localized_str!("Show the outline panel button in the status bar."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("outline_panel.button"),
                    pick: |settings_content| {
                        settings_content.outline_panel.as_ref()?.button.as_ref()
                    },
                    write: |settings_content, value, _| {
                        settings_content
                            .outline_panel
                            .get_or_insert_default()
                            .button = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Outline Panel Dock"),
                description: localization::localized_str!("Where to dock the outline panel."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("outline_panel.dock"),
                    pick: |settings_content| settings_content.outline_panel.as_ref()?.dock.as_ref(),
                    write: |settings_content, value, _| {
                        settings_content.outline_panel.get_or_insert_default().dock = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Outline Panel Default Width"),
                description: localization::localized_str!("Default width of the outline panel in pixels."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("outline_panel.default_width"),
                    pick: |settings_content| {
                        settings_content
                            .outline_panel
                            .as_ref()?
                            .default_width
                            .as_ref()
                    },
                    write: |settings_content, value, _| {
                        settings_content
                            .outline_panel
                            .get_or_insert_default()
                            .default_width = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("File Icons"),
                description: localization::localized_str!("Show file icons in the outline panel."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("outline_panel.file_icons"),
                    pick: |settings_content| {
                        settings_content.outline_panel.as_ref()?.file_icons.as_ref()
                    },
                    write: |settings_content, value, _| {
                        settings_content
                            .outline_panel
                            .get_or_insert_default()
                            .file_icons = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Folder Indicator"),
                description: localization::localized_str!("What to show for directories in the outline panel."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("outline_panel.folder_indicator"),
                    pick: |settings_content| {
                        settings_content
                            .outline_panel
                            .as_ref()?
                            .folder_indicator
                            .as_ref()
                    },
                    write: |settings_content, value, _| {
                        settings_content
                            .outline_panel
                            .get_or_insert_default()
                            .folder_indicator = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Git Status"),
                description: localization::localized_str!("Show the Git status in the outline panel."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("outline_panel.git_status"),
                    pick: |settings_content| {
                        settings_content.outline_panel.as_ref()?.git_status.as_ref()
                    },
                    write: |settings_content, value, _| {
                        settings_content
                            .outline_panel
                            .get_or_insert_default()
                            .git_status = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Indent Size"),
                description: localization::localized_str!("Amount of indentation for nested items."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("outline_panel.indent_size"),
                    pick: |settings_content| {
                        settings_content
                            .outline_panel
                            .as_ref()?
                            .indent_size
                            .as_ref()
                    },
                    write: |settings_content, value, _| {
                        settings_content
                            .outline_panel
                            .get_or_insert_default()
                            .indent_size = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Auto Reveal Entries"),
                description: localization::localized_str!("Whether to reveal when a corresponding outline entry becomes active."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("outline_panel.auto_reveal_entries"),
                    pick: |settings_content| {
                        settings_content
                            .outline_panel
                            .as_ref()?
                            .auto_reveal_entries
                            .as_ref()
                    },
                    write: |settings_content, value, _| {
                        settings_content
                            .outline_panel
                            .get_or_insert_default()
                            .auto_reveal_entries = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Auto Fold Directories"),
                description: localization::localized_str!("Whether to fold directories automatically when a directory contains only one subdirectory."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("outline_panel.auto_fold_dirs"),
                    pick: |settings_content| {
                        settings_content
                            .outline_panel
                            .as_ref()?
                            .auto_fold_dirs
                            .as_ref()
                    },
                    write: |settings_content, value, _| {
                        settings_content
                            .outline_panel
                            .get_or_insert_default()
                            .auto_fold_dirs = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                files: USER,
                title: localization::localized_str!("Show Indent Guides"),
                description: localization::localized_str!("When to show indent guides in the outline panel."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("outline_panel.indent_guides.show"),
                    pick: |settings_content| {
                        settings_content
                            .outline_panel
                            .as_ref()?
                            .indent_guides
                            .as_ref()?
                            .show
                            .as_ref()
                    },
                    write: |settings_content, value, _| {
                        settings_content
                            .outline_panel
                            .get_or_insert_default()
                            .indent_guides
                            .get_or_insert_default()
                            .show = value;
                    },
                }),
                metadata: None,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Hide Symbols in Multi-Buffers"),
                description: localization::localized_str!("Whether to hide symbols, excerpts and search matches in the outline panel when a multi-buffer view is active."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("outline_panel.multi_buffer_hide_symbols"),
                    pick: |settings_content| {
                        settings_content
                            .outline_panel
                            .as_ref()?
                            .multi_buffer_hide_symbols
                            .as_ref()
                    },
                    write: |settings_content, value, _| {
                        settings_content
                            .outline_panel
                            .get_or_insert_default()
                            .multi_buffer_hide_symbols = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
        ]
    }

    fn git_panel_section() -> [SettingsPageItem; 19] {
        [
            SettingsPageItem::SectionHeader(localization::localized_str!("Git Panel")),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Git Panel Button"),
                description: localization::localized_str!("Show the Git panel button in the status bar."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("git_panel.button"),
                    pick: |settings_content| settings_content.git_panel.as_ref()?.button.as_ref(),
                    write: |settings_content, value, _| {
                        settings_content.git_panel.get_or_insert_default().button = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Git Panel Dock"),
                description: localization::localized_str!("Where to dock the Git panel."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("git_panel.dock"),
                    pick: |settings_content| settings_content.git_panel.as_ref()?.dock.as_ref(),
                    write: |settings_content, value, _| {
                        settings_content.git_panel.get_or_insert_default().dock = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Starts Open"),
                description: localization::localized_str!("Whether the git panel should open on startup."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("git_panel.starts_open"),
                    pick: |settings_content| {
                        settings_content.git_panel.as_ref()?.starts_open.as_ref()
                    },
                    write: |settings_content, value, _| {
                        settings_content
                            .git_panel
                            .get_or_insert_default()
                            .starts_open = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Git Panel Default Width"),
                description: localization::localized_str!("Default width of the Git panel in pixels."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("git_panel.default_width"),
                    pick: |settings_content| {
                        settings_content.git_panel.as_ref()?.default_width.as_ref()
                    },
                    write: |settings_content, value, _| {
                        settings_content
                            .git_panel
                            .get_or_insert_default()
                            .default_width = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Git Panel Status Style"),
                description: localization::localized_str!("How entry statuses are displayed."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("git_panel.status_style"),
                    pick: |settings_content| {
                        settings_content.git_panel.as_ref()?.status_style.as_ref()
                    },
                    write: |settings_content, value, _| {
                        settings_content
                            .git_panel
                            .get_or_insert_default()
                            .status_style = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Fallback Branch Name"),
                description: localization::localized_str!("Default branch name will be when init.defaultbranch is not set in Git."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("git_panel.fallback_branch_name"),
                    pick: |settings_content| {
                        settings_content
                            .git_panel
                            .as_ref()?
                            .fallback_branch_name
                            .as_ref()
                    },
                    write: |settings_content, value, _| {
                        settings_content
                            .git_panel
                            .get_or_insert_default()
                            .fallback_branch_name = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Sort By"),
                description: localization::localized_str!("How to sort entries in the git panel."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("git_panel.sort_by"),
                    pick: |settings_content| settings_content.git_panel.as_ref()?.sort_by.as_ref(),
                    write: |settings_content, value, _| {
                        settings_content.git_panel.get_or_insert_default().sort_by = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Group By"),
                description: localization::localized_str!("How to group entries in the git panel."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("git_panel.group_by"),
                    pick: |settings_content| settings_content.git_panel.as_ref()?.group_by.as_ref(),
                    write: |settings_content, value, _| {
                        settings_content.git_panel.get_or_insert_default().group_by = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Collapse Untracked Diff"),
                description: localization::localized_str!("Whether to collapse untracked files in the diff panel."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("git_panel.collapse_untracked_diff"),
                    pick: |settings_content| {
                        settings_content
                            .git_panel
                            .as_ref()?
                            .collapse_untracked_diff
                            .as_ref()
                    },
                    write: |settings_content, value, _| {
                        settings_content
                            .git_panel
                            .get_or_insert_default()
                            .collapse_untracked_diff = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Tree View"),
                description: localization::localized_str!("Enable to show entries in tree view list, disable to show in flat view list."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("git_panel.tree_view"),
                    pick: |settings_content| {
                        settings_content.git_panel.as_ref()?.tree_view.as_ref()
                    },
                    write: |settings_content, value, _| {
                        settings_content.git_panel.get_or_insert_default().tree_view = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("File Icons"),
                description: localization::localized_str!("Show file icons next to the Git status icon."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("git_panel.file_icons"),
                    pick: |settings_content| {
                        settings_content.git_panel.as_ref()?.file_icons.as_ref()
                    },
                    write: |settings_content, value, _| {
                        settings_content
                            .git_panel
                            .get_or_insert_default()
                            .file_icons = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Folder Indicator"),
                description: localization::localized_str!("What to show for directories in the git panel."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("git_panel.folder_indicator"),
                    pick: |settings_content| {
                        settings_content
                            .git_panel
                            .as_ref()?
                            .folder_indicator
                            .as_ref()
                    },
                    write: |settings_content, value, _| {
                        settings_content
                            .git_panel
                            .get_or_insert_default()
                            .folder_indicator = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Diff Stats"),
                description: localization::localized_str!("Whether to show the addition/deletion change count next to each file in the Git panel."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("git_panel.diff_stats"),
                    pick: |settings_content| {
                        settings_content.git_panel.as_ref()?.diff_stats.as_ref()
                    },
                    write: |settings_content, value, _| {
                        settings_content
                            .git_panel
                            .get_or_insert_default()
                            .diff_stats = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Primary Click Behavior"),
                description: localization::localized_str!("Default action when clicking a changed file in the Git panel."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("git_panel.entry_primary_click_action"),
                    pick: |settings_content| {
                        settings_content
                            .git_panel
                            .as_ref()?
                            .entry_primary_click_action
                            .as_ref()
                    },
                    write: |settings_content, value, _| {
                        settings_content
                            .git_panel
                            .get_or_insert_default()
                            .entry_primary_click_action = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Commit Editor"),
                description: localization::localized_str!("Whether the commit message editor is shown in the Git panel by default."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("git_panel.commit_editor"),
                    pick: |settings_content| {
                        settings_content.git_panel.as_ref()?.commit_editor.as_ref()
                    },
                    write: |settings_content, value, _| {
                        settings_content
                            .git_panel
                            .get_or_insert_default()
                            .commit_editor = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Show Count Badge"),
                description: localization::localized_str!("Whether to show a badge on the git panel icon with the count of uncommitted changes."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("git_panel.show_count_badge"),
                    pick: |settings_content| {
                        settings_content
                            .git_panel
                            .as_ref()?
                            .show_count_badge
                            .as_ref()
                    },
                    write: |settings_content, value, _| {
                        settings_content
                            .git_panel
                            .get_or_insert_default()
                            .show_count_badge = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Commit Title Max Length"),
                description: localization::localized_str!("Maximum length of the commit message title before a warning is shown. Set to 0 to disable."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("git_panel.commit_title_max_length"),
                    pick: |settings_content| {
                        settings_content
                            .git_panel
                            .as_ref()?
                            .commit_title_max_length
                            .as_ref()
                    },
                    write: |settings_content, value, _| {
                        settings_content
                            .git_panel
                            .get_or_insert_default()
                            .commit_title_max_length = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Scroll Bar"),
                description: localization::localized_str!("How and when the scrollbar should be displayed."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("git_panel.scrollbar.show"),
                    pick: |settings_content| {
                        show_scrollbar_or_editor(settings_content, |settings_content| {
                            settings_content
                                .git_panel
                                .as_ref()?
                                .scrollbar
                                .as_ref()?
                                .show
                                .as_ref()
                        })
                    },
                    write: |settings_content, value, _| {
                        settings_content
                            .git_panel
                            .get_or_insert_default()
                            .scrollbar
                            .get_or_insert_default()
                            .show = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
        ]
    }




    SettingsPage {
        title: localization::localized_str!("Panels"),
        items: concat_sections![
            project_panel_section(),
            terminal_panel_section(),
            outline_panel_section(),
            git_panel_section(),
        ],
    }
}


fn terminal_page() -> SettingsPage {
    fn environment_section() -> [SettingsPageItem; 5] {
        [
                SettingsPageItem::SectionHeader(localization::localized_str!("Environment")),
                SettingsPageItem::DynamicItem(DynamicItem {
                    discriminant: SettingItem {
                        files: USER | PROJECT,
                        title: localization::localized_str!("Shell"),
                        description: localization::localized_str!("What shell to use when opening a terminal."),
                        field: Box::new(SettingField {
                            organization_override: None,
                            json_path: Some("terminal.shell$"),
                            pick: |settings_content| {
                                Some(&dynamic_variants::<settings::Shell>()[
                                    settings_content
                                        .terminal
                                        .as_ref()?
                                        .project
                                        .shell
                                        .as_ref()?
                                        .discriminant() as usize
                                ])
                            },
                            write: |settings_content, value, _| {
                                let Some(value) = value else {
                                    if let Some(terminal) = settings_content.terminal.as_mut() {
                                        terminal.project.shell = None;
                                    }
                                    return;
                                };
                                let settings_value = settings_content
                                    .terminal
                                    .get_or_insert_default()
                                    .project
                                    .shell
                                    .get_or_insert_with(|| settings::Shell::default());
                                let default_shell = if cfg!(target_os = "windows") {
                                    "powershell.exe"
                                } else {
                                    "sh"
                                };
                                *settings_value = match value {
                                    settings::ShellDiscriminants::System => settings::Shell::System,
                                    settings::ShellDiscriminants::Program => {
                                        let program = match settings_value {
                                            settings::Shell::Program(program) => program.clone(),
                                            settings::Shell::WithArguments { program, .. } => program.clone(),
                                            _ => String::from(default_shell),
                                        };
                                        settings::Shell::Program(program)
                                    }
                                    settings::ShellDiscriminants::WithArguments => {
                                        let (program, args, title_override) = match settings_value {
                                            settings::Shell::Program(program) => (program.clone(), vec![], None),
                                            settings::Shell::WithArguments {
                                                program,
                                                args,
                                                title_override,
                                            } => (program.clone(), args.clone(), title_override.clone()),
                                            _ => (String::from(default_shell), vec![], None),
                                        };
                                        settings::Shell::WithArguments {
                                            program,
                                            args,
                                            title_override,
                                        }
                                    }
                                };
                            },
                        }),
                        metadata: None,
                    },
                    pick_discriminant: |settings_content| {
                        Some(
                            settings_content
                                .terminal
                                .as_ref()?
                                .project
                                .shell
                                .as_ref()?
                                .discriminant() as usize,
                        )
                    },
                    fields: dynamic_variants::<settings::Shell>()
                        .into_iter()
                        .map(|variant| match variant {
                            settings::ShellDiscriminants::System => vec![],
                            settings::ShellDiscriminants::Program => vec![SettingItem {
                                files: USER | PROJECT,
                                title: localization::localized_str!("Program"),
                                description: localization::localized_str!("The shell program to use."),
                                field: Box::new(SettingField {
                                    organization_override: None,
                                    json_path: Some("terminal.shell"),
                                    pick: |settings_content| match settings_content.terminal.as_ref()?.project.shell.as_ref()
                                    {
                                        Some(settings::Shell::Program(program)) => Some(program),
                                        _ => None,
                                    },
                                    write: |settings_content, value, _| {
                                        let Some(value) = value else {
                                            return;
                                        };
                                        match settings_content
                                            .terminal
                                            .get_or_insert_default()
                                            .project
                                            .shell
                                            .as_mut()
                                        {
                                            Some(settings::Shell::Program(program)) => *program = value,
                                            _ => return,
                                        }
                                    },
                                }),
                                metadata: None,
                            }],
                            settings::ShellDiscriminants::WithArguments => vec![
                                SettingItem {
                                    files: USER | PROJECT,
                                    title: localization::localized_str!("Program"),
                                    description: localization::localized_str!("The shell program to run."),
                                    field: Box::new(SettingField {
                                        organization_override: None,
                                        json_path: Some("terminal.shell.program"),
                                        pick: |settings_content| {
                                            match settings_content.terminal.as_ref()?.project.shell.as_ref() {
                                                Some(settings::Shell::WithArguments { program, .. }) => Some(program),
                                                _ => None,
                                            }
                                        },
                                        write: |settings_content, value, _| {
                                            let Some(value) = value else {
                                                return;
                                            };
                                            match settings_content
                                                .terminal
                                                .get_or_insert_default()
                                                .project
                                                .shell
                                                .as_mut()
                                            {
                                                Some(settings::Shell::WithArguments { program, .. }) => {
                                                    *program = value
                                                }
                                                _ => return,
                                            }
                                        },
                                    }),
                                    metadata: None,
                                },
                                SettingItem {
                                    files: USER | PROJECT,
                                    title: localization::localized_str!("Arguments"),
                                    description: localization::localized_str!("The arguments to pass to the shell program."),
                                    field: Box::new(
                                        SettingField {
                                            organization_override: None,
                                            json_path: Some("terminal.shell.args"),
                                            pick: |settings_content| {
                                                match settings_content.terminal.as_ref()?.project.shell.as_ref() {
                                                    Some(settings::Shell::WithArguments { args, .. }) => Some(args),
                                                    _ => None,
                                                }
                                            },
                                            write: |settings_content, value, _| {
                                                let Some(value) = value else {
                                                    return;
                                                };
                                                match settings_content
                                                    .terminal
                                                    .get_or_insert_default()
                                                    .project
                                                    .shell
                                                    .as_mut()
                                                {
                                                    Some(settings::Shell::WithArguments { args, .. }) => *args = value,
                                                    _ => return,
                                                }
                                            },
                                        }
                                        .unimplemented(),
                                    ),
                                    metadata: None,
                                },
                                SettingItem {
                                    files: USER | PROJECT,
                                    title: localization::localized_str!("Title Override"),
                                    description: localization::localized_str!("An optional string to override the title of the terminal tab."),
                                    field: Box::new(SettingField {
                                        organization_override: None,
                                        json_path: Some("terminal.shell.title_override"),
                                        pick: |settings_content| {
                                            match settings_content.terminal.as_ref()?.project.shell.as_ref() {
                                                Some(settings::Shell::WithArguments { title_override, .. }) => {
                                                    title_override.as_ref().or(DEFAULT_EMPTY_STRING)
                                                }
                                                _ => None,
                                            }
                                        },
                                        write: |settings_content, value, _| {
                                            match settings_content
                                                .terminal
                                                .get_or_insert_default()
                                                .project
                                                .shell
                                                .as_mut()
                                            {
                                                Some(settings::Shell::WithArguments { title_override, .. }) => {
                                                    *title_override = value.filter(|s| !s.is_empty())
                                                }
                                                _ => return,
                                            }
                                        },
                                    }),
                                    metadata: None,
                                },
                            ],
                        })
                        .collect(),
                }),
                SettingsPageItem::DynamicItem(DynamicItem {
                    discriminant: SettingItem {
                        files: USER | PROJECT,
                        title: localization::localized_str!("Working Directory"),
                        description: localization::localized_str!("What working directory to use when launching the terminal."),
                        field: Box::new(SettingField {
                            organization_override: None,
                            json_path: Some("terminal.working_directory$"),
                            pick: |settings_content| {
                                Some(&dynamic_variants::<settings::WorkingDirectory>()[
                                    settings_content
                                        .terminal
                                        .as_ref()?
                                        .project
                                        .working_directory
                                        .as_ref()?
                                        .discriminant() as usize
                                ])
                            },
                            write: |settings_content, value, _| {
                                let Some(value) = value else {
                                    if let Some(terminal) = settings_content.terminal.as_mut() {
                                        terminal.project.working_directory = None;
                                    }
                                    return;
                                };
                                let settings_value = settings_content
                                    .terminal
                                    .get_or_insert_default()
                                    .project
                                    .working_directory
                                    .get_or_insert_with(|| settings::WorkingDirectory::CurrentProjectDirectory);
                                *settings_value = match value {
                                    settings::WorkingDirectoryDiscriminants::CurrentFileDirectory => {
                                        settings::WorkingDirectory::CurrentFileDirectory
                                    },
                                    settings::WorkingDirectoryDiscriminants::CurrentProjectDirectory => {
                                        settings::WorkingDirectory::CurrentProjectDirectory
                                    }
                                    settings::WorkingDirectoryDiscriminants::FirstProjectDirectory => {
                                        settings::WorkingDirectory::FirstProjectDirectory
                                    }
                                    settings::WorkingDirectoryDiscriminants::AlwaysHome => {
                                        settings::WorkingDirectory::AlwaysHome
                                    }
                                    settings::WorkingDirectoryDiscriminants::Always => {
                                        let directory = match settings_value {
                                            settings::WorkingDirectory::Always { .. } => return,
                                            _ => String::new(),
                                        };
                                        settings::WorkingDirectory::Always { directory }
                                    }
                                };
                            },
                        }),
                        metadata: None,
                    },
                    pick_discriminant: |settings_content| {
                        Some(
                            settings_content
                                .terminal
                                .as_ref()?
                                .project
                                .working_directory
                                .as_ref()?
                                .discriminant() as usize,
                        )
                    },
                    fields: dynamic_variants::<settings::WorkingDirectory>()
                        .into_iter()
                        .map(|variant| match variant {
                            settings::WorkingDirectoryDiscriminants::CurrentFileDirectory => vec![],
                            settings::WorkingDirectoryDiscriminants::CurrentProjectDirectory => vec![],
                            settings::WorkingDirectoryDiscriminants::FirstProjectDirectory => vec![],
                            settings::WorkingDirectoryDiscriminants::AlwaysHome => vec![],
                            settings::WorkingDirectoryDiscriminants::Always => vec![SettingItem {
                                files: USER | PROJECT,
                                title: localization::localized_str!("Directory"),
                                description: localization::localized_str!("The directory path to use (will be shell expanded)."),
                                field: Box::new(SettingField {
                                    organization_override: None,
                                    json_path: Some("terminal.working_directory.always"),
                                    pick: |settings_content| {
                                        match settings_content.terminal.as_ref()?.project.working_directory.as_ref() {
                                            Some(settings::WorkingDirectory::Always { directory }) => Some(directory),
                                            _ => None,
                                        }
                                    },
                                    write: |settings_content, value, _| {
                                        let value = value.unwrap_or_default();
                                        match settings_content
                                            .terminal
                                            .get_or_insert_default()
                                            .project
                                            .working_directory
                                            .as_mut()
                                        {
                                            Some(settings::WorkingDirectory::Always { directory }) => *directory = value,
                                            _ => return,
                                        }
                                    },
                                }),
                                metadata: None,
                            }],
                        })
                        .collect(),
                }),
                SettingsPageItem::SettingItem(SettingItem {
                    title: localization::localized_str!("Environment Variables"),
                    description: localization::localized_str!("Key-value pairs to add to the terminal's environment."),
                    field: Box::new(
                        SettingField {
                            organization_override: None,
                            json_path: Some("terminal.env"),
                            pick: |settings_content| settings_content.terminal.as_ref()?.project.env.as_ref(),
                            write: |settings_content, value, _| {
                                settings_content.terminal.get_or_insert_default().project.env = value;
                            },
                        }
                        .unimplemented(),
                    ),
                    metadata: None,
                    files: USER | PROJECT,
                }),
                SettingsPageItem::SettingItem(SettingItem {
                    title: localization::localized_str!("Detect Virtual Environment"),
                    description: localization::localized_str!("Activates the Python virtual environment, if one is found, in the terminal's working directory."),
                    field: Box::new(
                        SettingField {
                            organization_override: None,
                            json_path: Some("terminal.detect_venv"),
                            pick: |settings_content| settings_content.terminal.as_ref()?.project.detect_venv.as_ref(),
                            write: |settings_content, value, _| {
                                settings_content
                                    .terminal
                                    .get_or_insert_default()
                                    .project
                                    .detect_venv = value;
                            },
                        }
                        .unimplemented(),
                    ),
                    metadata: None,
                    files: USER | PROJECT,
                }),
            ]
    }

    fn font_section() -> [SettingsPageItem; 6] {
        [
            SettingsPageItem::SectionHeader(localization::localized_str!("Font")),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Font Size"),
                description: localization::localized_str!("Font size for terminal text. If not set, defaults to buffer font size."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("terminal.font_size"),
                    pick: |settings_content| {
                        settings_content
                            .terminal
                            .as_ref()
                            .and_then(|terminal| terminal.font_size.as_ref())
                            .or(settings_content.theme.buffer_font_size.as_ref())
                    },
                    write: |settings_content, value, _| {
                        settings_content.terminal.get_or_insert_default().font_size = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Font Family"),
                description: localization::localized_str!("Font family for terminal text. If not set, defaults to buffer font family."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("terminal.font_family"),
                    pick: |settings_content| {
                        settings_content
                            .terminal
                            .as_ref()
                            .and_then(|terminal| terminal.font_family.as_ref())
                            .or(settings_content.theme.buffer_font_family.as_ref())
                    },
                    write: |settings_content, value, _| {
                        settings_content
                            .terminal
                            .get_or_insert_default()
                            .font_family = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Font Fallbacks"),
                description: localization::localized_str!("Font fallbacks for terminal text. If not set, defaults to buffer font fallbacks."),
                field: Box::new(
                    SettingField {
                        organization_override: None,
                        json_path: Some("terminal.font_fallbacks"),
                        pick: |settings_content| {
                            settings_content
                                .terminal
                                .as_ref()
                                .and_then(|terminal| terminal.font_fallbacks.as_ref())
                                .or(settings_content.theme.buffer_font_fallbacks.as_ref())
                        },
                        write: |settings_content, value, _| {
                            settings_content
                                .terminal
                                .get_or_insert_default()
                                .font_fallbacks = value;
                        },
                    }
                    .unimplemented(),
                ),
                metadata: None,
                files: USER,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Font Weight"),
                description: localization::localized_str!("Font weight for terminal text in CSS weight units (100-900)."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("terminal.font_weight"),
                    pick: |settings_content| {
                        settings_content.terminal.as_ref()?.font_weight.as_ref()
                    },
                    write: |settings_content, value, _| {
                        settings_content
                            .terminal
                            .get_or_insert_default()
                            .font_weight = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Font Features"),
                description: localization::localized_str!("Font features for terminal text."),
                field: Box::new(
                    SettingField {
                        organization_override: None,
                        json_path: Some("terminal.font_features"),
                        pick: |settings_content| {
                            settings_content
                                .terminal
                                .as_ref()
                                .and_then(|terminal| terminal.font_features.as_ref())
                                .or(settings_content.theme.buffer_font_features.as_ref())
                        },
                        write: |settings_content, value, _| {
                            settings_content
                                .terminal
                                .get_or_insert_default()
                                .font_features = value;
                        },
                    }
                    .unimplemented(),
                ),
                metadata: None,
                files: USER,
            }),
        ]
    }

    fn display_settings_section() -> [SettingsPageItem; 6] {
        [
            SettingsPageItem::SectionHeader(localization::localized_str!("Display Settings")),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Line Height"),
                description: localization::localized_str!("Line height for terminal text."),
                field: Box::new(
                    SettingField {
                        organization_override: None,
                        json_path: Some("terminal.line_height"),
                        pick: |settings_content| {
                            settings_content.terminal.as_ref()?.line_height.as_ref()
                        },
                        write: |settings_content, value, _| {
                            settings_content
                                .terminal
                                .get_or_insert_default()
                                .line_height = value;
                        },
                    }
                    .unimplemented(),
                ),
                metadata: None,
                files: USER,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Cursor Shape"),
                description: localization::localized_str!("Default cursor shape for the terminal (bar, block, underline, or hollow)."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("terminal.cursor_shape"),
                    pick: |settings_content| {
                        settings_content.terminal.as_ref()?.cursor_shape.as_ref()
                    },
                    write: |settings_content, value, _| {
                        settings_content
                            .terminal
                            .get_or_insert_default()
                            .cursor_shape = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Cursor Blinking"),
                description: localization::localized_str!("Sets the cursor blinking behavior in the terminal."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("terminal.blinking"),
                    pick: |settings_content| settings_content.terminal.as_ref()?.blinking.as_ref(),
                    write: |settings_content, value, _| {
                        settings_content.terminal.get_or_insert_default().blinking = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Alternate Scroll"),
                description: localization::localized_str!("Whether alternate scroll mode is active by default (converts mouse scroll to arrow keys in apps like Vim)."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("terminal.alternate_scroll"),
                    pick: |settings_content| {
                        settings_content
                            .terminal
                            .as_ref()?
                            .alternate_scroll
                            .as_ref()
                    },
                    write: |settings_content, value, _| {
                        settings_content
                            .terminal
                            .get_or_insert_default()
                            .alternate_scroll = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Minimum Contrast"),
                description: localization::localized_str!("The minimum APCA perceptual contrast between foreground and background colors (0-106)."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("terminal.minimum_contrast"),
                    pick: |settings_content| {
                        settings_content
                            .terminal
                            .as_ref()?
                            .minimum_contrast
                            .as_ref()
                    },
                    write: |settings_content, value, _| {
                        settings_content
                            .terminal
                            .get_or_insert_default()
                            .minimum_contrast = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
        ]
    }

    fn behavior_settings_section() -> Vec<SettingsPageItem> {
        vec![
            SettingsPageItem::SectionHeader(localization::localized_str!("Behavior Settings")),
            // [OHOS PORT BEGIN] There is no Option key on this platform's
            // keyboard, so the mapping has nothing to configure.
            #[cfg(not(target_env = "ohos"))]
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Option As Meta"),
                description: localization::localized_str!("Whether the option key behaves as the meta key."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("terminal.option_as_meta"),
                    pick: |settings_content| {
                        settings_content.terminal.as_ref()?.option_as_meta.as_ref()
                    },
                    write: |settings_content, value, _| {
                        settings_content
                            .terminal
                            .get_or_insert_default()
                            .option_as_meta = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Copy On Select"),
                description: localization::localized_str!("Whether selecting text in the terminal automatically copies to the system clipboard."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("terminal.copy_on_select"),
                    pick: |settings_content| {
                        settings_content.terminal.as_ref()?.copy_on_select.as_ref()
                    },
                    write: |settings_content, value, _| {
                        settings_content
                            .terminal
                            .get_or_insert_default()
                            .copy_on_select = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Keep Selection On Copy"),
                description: localization::localized_str!("Whether to keep the text selection after copying it to the clipboard."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("terminal.keep_selection_on_copy"),
                    pick: |settings_content| {
                        settings_content
                            .terminal
                            .as_ref()?
                            .keep_selection_on_copy
                            .as_ref()
                    },
                    write: |settings_content, value, _| {
                        settings_content
                            .terminal
                            .get_or_insert_default()
                            .keep_selection_on_copy = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Open Links In Mouse Mode"),
                description: localization::localized_str!("Whether cmd-click (ctrl-click on Linux and Windows) opens hyperlinks even when the terminal application has enabled mouse reporting. When disabled, these clicks are forwarded to the application; links can still be opened with shift-cmd-click."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("terminal.open_links_in_mouse_mode"),
                    pick: |settings_content| {
                        settings_content
                            .terminal
                            .as_ref()?
                            .open_links_in_mouse_mode
                            .as_ref()
                    },
                    write: |settings_content, value, _| {
                        settings_content
                            .terminal
                            .get_or_insert_default()
                            .open_links_in_mouse_mode = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Audible Bell"),
                description: localization::localized_str!("Whether to play a sound when the BEL character (`\\a`, `0x07`) is printed"),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("terminal.bell"),
                    pick: |settings_content| settings_content.terminal.as_ref()?.bell.as_ref(),
                    write: |settings_content, value, _| {
                        settings_content.terminal.get_or_insert_default().bell = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
        ]
    }

    fn layout_settings_section() -> [SettingsPageItem; 3] {
        [
            SettingsPageItem::SectionHeader(localization::localized_str!("Layout Settings")),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Default Width"),
                description: localization::localized_str!("Default width when the terminal is docked to the left or right (in pixels)."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("terminal.default_width"),
                    pick: |settings_content| {
                        settings_content.terminal.as_ref()?.default_width.as_ref()
                    },
                    write: |settings_content, value, _| {
                        settings_content
                            .terminal
                            .get_or_insert_default()
                            .default_width = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Default Height"),
                description: localization::localized_str!("Default height when the terminal is docked to the bottom (in pixels)."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("terminal.default_height"),
                    pick: |settings_content| {
                        settings_content.terminal.as_ref()?.default_height.as_ref()
                    },
                    write: |settings_content, value, _| {
                        settings_content
                            .terminal
                            .get_or_insert_default()
                            .default_height = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
        ]
    }

    fn advanced_settings_section() -> [SettingsPageItem; 3] {
        [
            SettingsPageItem::SectionHeader(localization::localized_str!("Advanced Settings")),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Max Scroll History Lines"),
                description: localization::localized_str!("Maximum number of lines to keep in scrollback history (max: 100,000; 0 disables scrolling)."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("terminal.max_scroll_history_lines"),
                    pick: |settings_content| {
                        settings_content
                            .terminal
                            .as_ref()?
                            .max_scroll_history_lines
                            .as_ref()
                    },
                    write: |settings_content, value, _| {
                        settings_content
                            .terminal
                            .get_or_insert_default()
                            .max_scroll_history_lines = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Scroll Multiplier"),
                description: localization::localized_str!("The multiplier for scrolling in the terminal with the mouse wheel"),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("terminal.scroll_multiplier"),
                    pick: |settings_content| {
                        settings_content
                            .terminal
                            .as_ref()?
                            .scroll_multiplier
                            .as_ref()
                    },
                    write: |settings_content, value, _| {
                        settings_content
                            .terminal
                            .get_or_insert_default()
                            .scroll_multiplier = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
        ]
    }

    // [OHOS PORT BEGIN] This build has no terminal toolbar, so its options do
    // not apply.
    #[cfg(not(target_env = "ohos"))]
    fn toolbar_section() -> [SettingsPageItem; 2] {
        [
            SettingsPageItem::SectionHeader(localization::localized_str!("Toolbar")),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Breadcrumbs"),
                description: localization::localized_str!("Display the terminal title in breadcrumbs inside the terminal pane."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("terminal.toolbar.breadcrumbs"),
                    pick: |settings_content| {
                        settings_content
                            .terminal
                            .as_ref()?
                            .toolbar
                            .as_ref()?
                            .breadcrumbs
                            .as_ref()
                    },
                    write: |settings_content, value, _| {
                        settings_content
                            .terminal
                            .get_or_insert_default()
                            .toolbar
                            .get_or_insert_default()
                            .breadcrumbs = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
        ]
    }

    fn scrollbar_section() -> [SettingsPageItem; 2] {
        [
            SettingsPageItem::SectionHeader(localization::localized_str!("Scrollbar")),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Show Scrollbar"),
                description: localization::localized_str!("When to show the scrollbar in the terminal."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("terminal.scrollbar.show"),
                    pick: |settings_content| {
                        show_scrollbar_or_editor(settings_content, |settings_content| {
                            settings_content
                                .terminal
                                .as_ref()?
                                .scrollbar
                                .as_ref()?
                                .show
                                .as_ref()
                        })
                    },
                    write: |settings_content, value, _| {
                        settings_content
                            .terminal
                            .get_or_insert_default()
                            .scrollbar
                            .get_or_insert_default()
                            .show = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
        ]
    }

    let mut items: Vec<SettingsPageItem> = concat_sections![
        environment_section(),
        font_section(),
        display_settings_section(),
        behavior_settings_section(),
        layout_settings_section(),
        advanced_settings_section(),
    ]
    .into_vec();
    // [OHOS PORT BEGIN] This build has no terminal toolbar, so its toggles are
    // not offered.
    #[cfg(not(target_env = "ohos"))]
    items.extend(toolbar_section());
    // [OHOS PORT END]
    items.extend(concat_sections![scrollbar_section()]);

    SettingsPage {
        title: localization::localized_str!("Terminal"),
        items: items.into(),
    }
}

fn version_control_page() -> SettingsPage {
    fn git_integration_section() -> [SettingsPageItem; 2] {
        [
            SettingsPageItem::SectionHeader(localization::localized_str!("Git Integration")),
            SettingsPageItem::DynamicItem(DynamicItem {
                discriminant: SettingItem {
                    files: USER,
                    title: localization::localized_str!("Disable Git Integration"),
                    description: localization::localized_str!("Disable all Git integration features in HiCodeer."),
                    field: Box::new(SettingField::<bool> {
                        organization_override: None,
                        json_path: Some("git.disable_git"),
                        pick: |settings_content| {
                            settings_content
                                .git
                                .as_ref()?
                                .enabled
                                .as_ref()?
                                .disable_git
                                .as_ref()
                        },
                        write: |settings_content, value, _| {
                            settings_content
                                .git
                                .get_or_insert_default()
                                .enabled
                                .get_or_insert_default()
                                .disable_git = value;
                        },
                    }),
                    metadata: None,
                },
                pick_discriminant: |settings_content| {
                    let disabled = settings_content
                        .git
                        .as_ref()?
                        .enabled
                        .as_ref()?
                        .disable_git
                        .unwrap_or(false);
                    Some(if disabled { 0 } else { 1 })
                },
                fields: vec![
                    vec![],
                    vec![
                        SettingItem {
                            files: USER,
                            title: localization::localized_str!("Enable Git Status"),
                            description: localization::localized_str!("Show Git status information in the editor."),
                            field: Box::new(SettingField::<bool> {
                                organization_override: None,
                                json_path: Some("git.enable_status"),
                                pick: |settings_content| {
                                    settings_content
                                        .git
                                        .as_ref()?
                                        .enabled
                                        .as_ref()?
                                        .enable_status
                                        .as_ref()
                                },
                                write: |settings_content, value, _| {
                                    settings_content
                                        .git
                                        .get_or_insert_default()
                                        .enabled
                                        .get_or_insert_default()
                                        .enable_status = value;
                                },
                            }),
                            metadata: None,
                        },
                        SettingItem {
                            files: USER,
                            title: localization::localized_str!("Enable Git Diff"),
                            description: localization::localized_str!("Show Git diff information in the editor."),
                            field: Box::new(SettingField::<bool> {
                                organization_override: None,
                                json_path: Some("git.enable_diff"),
                                pick: |settings_content| {
                                    settings_content
                                        .git
                                        .as_ref()?
                                        .enabled
                                        .as_ref()?
                                        .enable_diff
                                        .as_ref()
                                },
                                write: |settings_content, value, _| {
                                    settings_content
                                        .git
                                        .get_or_insert_default()
                                        .enabled
                                        .get_or_insert_default()
                                        .enable_diff = value;
                                },
                            }),
                            metadata: None,
                        },
                    ],
                ],
            }),
        ]
    }

    fn git_gutter_section() -> [SettingsPageItem; 3] {
        [
            SettingsPageItem::SectionHeader(localization::localized_str!("Git Gutter")),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Visibility"),
                description: localization::localized_str!("Control whether Git status is shown in the editor's gutter."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("git.git_gutter"),
                    pick: |settings_content| settings_content.git.as_ref()?.git_gutter.as_ref(),
                    write: |settings_content, value, _| {
                        settings_content.git.get_or_insert_default().git_gutter = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
            // todo(settings_ui): Figure out the right default for this value in default.json
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Debounce"),
                description: localization::localized_str!("Debounce threshold in milliseconds after which changes are reflected in the Git gutter."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("git.gutter_debounce"),
                    pick: |settings_content| {
                        settings_content.git.as_ref()?.gutter_debounce.as_ref()
                    },
                    write: |settings_content, value, _| {
                        settings_content.git.get_or_insert_default().gutter_debounce = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
        ]
    }

    fn inline_git_blame_section() -> [SettingsPageItem; 6] {
        [
            SettingsPageItem::SectionHeader(localization::localized_str!("Inline Git Blame")),
            SettingsPageItem::DynamicItem(DynamicItem {
                discriminant: SettingItem {
                    title: localization::localized_str!("Enabled"),
                    description: localization::localized_str!("Whether or not to show Git blame data for the currently focused line."),
                    field: Box::new(SettingField {
                        organization_override: None,
                        json_path: Some("git.inline_blame.enabled"),
                        pick: |settings_content| {
                            settings_content
                                .git
                                .as_ref()?
                                .inline_blame
                                .as_ref()?
                                .enabled
                                .as_ref()
                        },
                        write: |settings_content, value, _| {
                            settings_content
                                .git
                                .get_or_insert_default()
                                .inline_blame
                                .get_or_insert_default()
                                .enabled = value;
                        },
                    }),
                    metadata: None,
                    files: USER,
                },
                pick_discriminant: |settings_content| {
                    Some(
                        *settings_content
                            .git
                            .as_ref()?
                            .inline_blame
                            .as_ref()?
                            .enabled
                            .as_ref()? as usize,
                    )
                },
                fields: vec![
                    vec![],
                    vec![SettingItem {
                        title: localization::localized_str!("Location"),
                        description: localization::localized_str!("Where to render Git blame when it is enabled."),
                        field: Box::new(SettingField {
                            organization_override: None,
                            json_path: Some("git.inline_blame.location"),
                            pick: |settings_content| {
                                settings_content
                                    .git
                                    .as_ref()?
                                    .inline_blame
                                    .as_ref()?
                                    .location
                                    .as_ref()
                            },
                            write: |settings_content, value, _| {
                                settings_content
                                    .git
                                    .get_or_insert_default()
                                    .inline_blame
                                    .get_or_insert_default()
                                    .location = value;
                            },
                        }),
                        metadata: None,
                        files: USER,
                    }],
                ],
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Delay"),
                description: localization::localized_str!("The delay after which the inline blame information is shown."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("git.inline_blame.delay_ms"),
                    pick: |settings_content| {
                        settings_content
                            .git
                            .as_ref()?
                            .inline_blame
                            .as_ref()?
                            .delay_ms
                            .as_ref()
                    },
                    write: |settings_content, value, _| {
                        settings_content
                            .git
                            .get_or_insert_default()
                            .inline_blame
                            .get_or_insert_default()
                            .delay_ms = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Padding"),
                description: localization::localized_str!("Padding between the end of the source line and the start of the inline blame in columns."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("git.inline_blame.padding"),
                    pick: |settings_content| {
                        settings_content
                            .git
                            .as_ref()?
                            .inline_blame
                            .as_ref()?
                            .padding
                            .as_ref()
                    },
                    write: |settings_content, value, _| {
                        settings_content
                            .git
                            .get_or_insert_default()
                            .inline_blame
                            .get_or_insert_default()
                            .padding = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Minimum Column"),
                description: localization::localized_str!("The minimum column number at which to show the inline blame information."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("git.inline_blame.min_column"),
                    pick: |settings_content| {
                        settings_content
                            .git
                            .as_ref()?
                            .inline_blame
                            .as_ref()?
                            .min_column
                            .as_ref()
                    },
                    write: |settings_content, value, _| {
                        settings_content
                            .git
                            .get_or_insert_default()
                            .inline_blame
                            .get_or_insert_default()
                            .min_column = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Show Commit Summary"),
                description: localization::localized_str!("Show commit summary as part of the inline blame."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("git.inline_blame.show_commit_summary"),
                    pick: |settings_content| {
                        settings_content
                            .git
                            .as_ref()?
                            .inline_blame
                            .as_ref()?
                            .show_commit_summary
                            .as_ref()
                    },
                    write: |settings_content, value, _| {
                        settings_content
                            .git
                            .get_or_insert_default()
                            .inline_blame
                            .get_or_insert_default()
                            .show_commit_summary = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
        ]
    }

    fn git_blame_view_section() -> [SettingsPageItem; 2] {
        [
            SettingsPageItem::SectionHeader(localization::localized_str!("Git Blame View")),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Show Avatar"),
                description: localization::localized_str!("Show the avatar of the author of the commit."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("git.blame.show_avatar"),
                    pick: |settings_content| {
                        settings_content
                            .git
                            .as_ref()?
                            .blame
                            .as_ref()?
                            .show_avatar
                            .as_ref()
                    },
                    write: |settings_content, value, _| {
                        settings_content
                            .git
                            .get_or_insert_default()
                            .blame
                            .get_or_insert_default()
                            .show_avatar = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
        ]
    }

    fn branch_picker_section() -> [SettingsPageItem; 2] {
        [
            SettingsPageItem::SectionHeader(localization::localized_str!("Branch Picker")),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Show Author Name"),
                description: localization::localized_str!("Show author name as part of the commit information in branch picker."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("git.branch_picker.show_author_name"),
                    pick: |settings_content| {
                        settings_content
                            .git
                            .as_ref()?
                            .branch_picker
                            .as_ref()?
                            .show_author_name
                            .as_ref()
                    },
                    write: |settings_content, value, _| {
                        settings_content
                            .git
                            .get_or_insert_default()
                            .branch_picker
                            .get_or_insert_default()
                            .show_author_name = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
        ]
    }

    fn git_hunks_section() -> [SettingsPageItem; 5] {
        [
            SettingsPageItem::SectionHeader(localization::localized_str!("Git Hunks")),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Hunk Style"),
                description: localization::localized_str!("How Git hunks are displayed visually in the editor."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("git.hunk_style"),
                    pick: |settings_content| settings_content.git.as_ref()?.hunk_style.as_ref(),
                    write: |settings_content, value, _| {
                        settings_content.git.get_or_insert_default().hunk_style = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Diff Base"),
                description: localization::localized_str!("Whether git features show changes relative to HEAD (uncommitted changes) or to the default branch (all changes on the current branch)."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("git.diff_base"),
                    pick: |settings_content| settings_content.git.as_ref()?.diff_base.as_ref(),
                    write: |settings_content, value, _| {
                        settings_content.git.get_or_insert_default().diff_base = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Path Style"),
                description: localization::localized_str!("Should the name or path be displayed first in the git view."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("git.path_style"),
                    pick: |settings_content| settings_content.git.as_ref()?.path_style.as_ref(),
                    write: |settings_content, value, _| {
                        settings_content.git.get_or_insert_default().path_style = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Show Stage/Restore Buttons"),
                description: localization::localized_str!("Whether to show the stage and restore buttons on diff hunks."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("git.show_stage_restore_buttons"),
                    pick: |settings_content| {
                        settings_content
                            .git
                            .as_ref()?
                            .show_stage_restore_buttons
                            .as_ref()
                    },
                    write: |settings_content, value, _| {
                        settings_content
                            .git
                            .get_or_insert_default()
                            .show_stage_restore_buttons = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
        ]
    }

    fn file_diff_section() -> [SettingsPageItem; 2] {
        [
            SettingsPageItem::SectionHeader(localization::localized_str!("File Diff")),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Show Full File by Default"),
                description: localization::localized_str!("Whether newly opened file diffs show the full file instead of changes only."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("git.file_diff.show_full_file"),
                    pick: |settings_content| {
                        settings_content
                            .git
                            .as_ref()?
                            .file_diff
                            .as_ref()?
                            .show_full_file
                            .as_ref()
                    },
                    write: |settings_content, value, _| {
                        settings_content
                            .git
                            .get_or_insert_default()
                            .file_diff
                            .get_or_insert_default()
                            .show_full_file = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
        ]
    }

    SettingsPage {
        title: localization::localized_str!("Version Control"),
        items: concat_sections![
            git_integration_section(),
            git_gutter_section(),
            inline_git_blame_section(),
            git_blame_view_section(),
            branch_picker_section(),
            file_diff_section(),
            git_hunks_section(),
        ],
    }
}



fn network_page() -> SettingsPage {
    fn network_section() -> [SettingsPageItem; 3] {
        [
            SettingsPageItem::SectionHeader(localization::localized_str!("Network")),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Proxy"),
                description: localization::localized_str!("The proxy to use for network requests."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("proxy"),
                    pick: |settings_content| settings_content.proxy.as_ref(),
                    write: |settings_content, value, _| {
                        settings_content.proxy = value;
                    },
                }),
                metadata: Some(Box::new(SettingsFieldMetadata {
                    placeholder: Some("socks5h://localhost:10808"),
                    ..Default::default()
                })),
                files: USER,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Server URL"),
                description: localization::localized_str!("The URL of the HiCodeer server to connect to."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("server_url"),
                    pick: |settings_content| settings_content.server_url.as_ref(),
                    write: |settings_content, value, _| {
                        settings_content.server_url = value;
                    },
                }),
                metadata: Some(Box::new(SettingsFieldMetadata {
                    placeholder: Some("https://zed.dev"),
                    ..Default::default()
                })),
                files: USER,
            }),
        ]
    }

    SettingsPage {
        title: localization::localized_str!("Network"),
        items: concat_sections![network_section()],
    }
}

fn language_settings_field<T>(
    settings_content: &SettingsContent,
    get_language_setting_field: fn(&LanguageSettingsContent) -> Option<&T>,
) -> Option<&T> {
    let all_languages = &settings_content.project.all_languages;

    active_language()
        .and_then(|current_language_name| {
            all_languages
                .languages
                .0
                .get(current_language_name.as_ref())
        })
        .and_then(get_language_setting_field)
        .or_else(|| get_language_setting_field(&all_languages.defaults))
}

fn language_settings_field_mut<T>(
    settings_content: &mut SettingsContent,
    value: Option<T>,
    write: fn(&mut LanguageSettingsContent, Option<T>),
) {
    let all_languages = &mut settings_content.project.all_languages;
    let language_content = if let Some(current_language) = active_language() {
        all_languages
            .languages
            .0
            .entry(current_language.to_string())
            .or_default()
    } else {
        &mut all_languages.defaults
    };
    write(language_content, value);
}

fn language_settings_data() -> Box<[SettingsPageItem]> {
    fn indentation_section() -> [SettingsPageItem; 5] {
        [
            SettingsPageItem::SectionHeader(localization::localized_str!("Indentation")),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Tab Size"),
                description: localization::localized_str!("How many columns a tab should occupy."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("languages.$(language).tab_size"), // TODO(cameron): not JQ syntax because not URL-safe
                    pick: |settings_content| {
                        language_settings_field(settings_content, |language| {
                            language.tab_size.as_ref()
                        })
                    },
                    write: |settings_content, value, _| {
                        language_settings_field_mut(settings_content, value, |language, value| {
                            language.tab_size = value;
                        })
                    },
                }),
                metadata: None,
                files: USER | PROJECT,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Hard Tabs"),
                description: localization::localized_str!("Whether to indent lines using tab characters, as opposed to multiple spaces."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("languages.$(language).hard_tabs"),
                    pick: |settings_content| {
                        language_settings_field(settings_content, |language| {
                            language.hard_tabs.as_ref()
                        })
                    },
                    write: |settings_content, value, _| {
                        language_settings_field_mut(settings_content, value, |language, value| {
                            language.hard_tabs = value;
                        })
                    },
                }),
                metadata: None,
                files: USER | PROJECT,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Auto Indent"),
                description: localization::localized_str!("Controls automatic indentation behavior when typing."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("languages.$(language).auto_indent"),
                    pick: |settings_content| {
                        language_settings_field(settings_content, |language| {
                            language.auto_indent.as_ref()
                        })
                    },
                    write: |settings_content, value, _| {
                        language_settings_field_mut(settings_content, value, |language, value| {
                            language.auto_indent = value;
                        })
                    },
                }),
                metadata: None,
                files: USER | PROJECT,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Auto Indent On Paste"),
                description: localization::localized_str!("Whether indentation of pasted content should be adjusted based on the context."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("languages.$(language).auto_indent_on_paste"),
                    pick: |settings_content| {
                        language_settings_field(settings_content, |language| {
                            language.auto_indent_on_paste.as_ref()
                        })
                    },
                    write: |settings_content, value, _| {
                        language_settings_field_mut(settings_content, value, |language, value| {
                            language.auto_indent_on_paste = value;
                        })
                    },
                }),
                metadata: None,
                files: USER | PROJECT,
            }),
        ]
    }

    fn wrapping_section() -> [SettingsPageItem; 7] {
        [
            SettingsPageItem::SectionHeader(localization::localized_str!("Wrapping")),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Soft Wrap"),
                description: localization::localized_str!("How to soft-wrap long lines of text."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("languages.$(language).soft_wrap"),
                    pick: |settings_content| {
                        language_settings_field(settings_content, |language| {
                            language.soft_wrap.as_ref()
                        })
                    },
                    write: |settings_content, value, _| {
                        language_settings_field_mut(settings_content, value, |language, value| {
                            language.soft_wrap = value;
                        })
                    },
                }),
                metadata: None,
                files: USER | PROJECT,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Soft Wrap Indent"),
                description: localization::localized_str!("How to indent soft-wrapped lines."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("languages.$(language).soft_wrap_indent"),
                    pick: |settings_content| {
                        language_settings_field(settings_content, |language| {
                            language.soft_wrap_indent.as_ref()
                        })
                    },
                    write: |settings_content, value, _| {
                        language_settings_field_mut(settings_content, value, |language, value| {
                            language.soft_wrap_indent = value;
                        })
                    },
                }),
                metadata: None,
                files: USER | PROJECT,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Show Wrap Guides"),
                description: localization::localized_str!("Show wrap guides in the editor."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("languages.$(language).show_wrap_guides"),
                    pick: |settings_content| {
                        language_settings_field(settings_content, |language| {
                            language.show_wrap_guides.as_ref()
                        })
                    },
                    write: |settings_content, value, _| {
                        language_settings_field_mut(settings_content, value, |language, value| {
                            language.show_wrap_guides = value;
                        })
                    },
                }),
                metadata: None,
                files: USER | PROJECT,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Preferred Line Length"),
                description: localization::localized_str!("The column at which to soft-wrap lines, for buffers where soft-wrap is enabled."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("languages.$(language).preferred_line_length"),
                    pick: |settings_content| {
                        language_settings_field(settings_content, |language| {
                            language.preferred_line_length.as_ref()
                        })
                    },
                    write: |settings_content, value, _| {
                        language_settings_field_mut(settings_content, value, |language, value| {
                            language.preferred_line_length = value;
                        })
                    },
                }),
                metadata: None,
                files: USER | PROJECT,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Wrap Guides"),
                description: localization::localized_str!("Character counts at which to show wrap guides in the editor."),
                field: Box::new(
                    SettingField {
                        organization_override: None,
                        json_path: Some("languages.$(language).wrap_guides"),
                        pick: |settings_content| {
                            language_settings_field(settings_content, |language| {
                                language.wrap_guides.as_ref()
                            })
                        },
                        write: |settings_content, value, _| {
                            language_settings_field_mut(
                                settings_content,
                                value,
                                |language, value| {
                                    language.wrap_guides = value;
                                },
                            )
                        },
                    }
                    .unimplemented(),
                ),
                metadata: None,
                files: USER | PROJECT,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Allow Rewrap"),
                description: localization::localized_str!("Controls where the `editor::rewrap` action is allowed for this language."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("languages.$(language).allow_rewrap"),
                    pick: |settings_content| {
                        language_settings_field(settings_content, |language| {
                            language.allow_rewrap.as_ref()
                        })
                    },
                    write: |settings_content, value, _| {
                        language_settings_field_mut(settings_content, value, |language, value| {
                            language.allow_rewrap = value;
                        })
                    },
                }),
                metadata: None,
                files: USER | PROJECT,
            }),
        ]
    }

    fn indent_guides_section() -> [SettingsPageItem; 6] {
        [
            SettingsPageItem::SectionHeader(localization::localized_str!("Indent Guides")),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Enabled"),
                description: localization::localized_str!("Display indent guides in the editor."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("languages.$(language).indent_guides.enabled"),
                    pick: |settings_content| {
                        language_settings_field(settings_content, |language| {
                            language
                                .indent_guides
                                .as_ref()
                                .and_then(|indent_guides| indent_guides.enabled.as_ref())
                        })
                    },
                    write: |settings_content, value, _| {
                        language_settings_field_mut(settings_content, value, |language, value| {
                            language.indent_guides.get_or_insert_default().enabled = value;
                        })
                    },
                }),
                metadata: None,
                files: USER | PROJECT,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Line Width"),
                description: localization::localized_str!("The width of the indent guides in pixels, between 1 and 10."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("languages.$(language).indent_guides.line_width"),
                    pick: |settings_content| {
                        language_settings_field(settings_content, |language| {
                            language
                                .indent_guides
                                .as_ref()
                                .and_then(|indent_guides| indent_guides.line_width.as_ref())
                        })
                    },
                    write: |settings_content, value, _| {
                        language_settings_field_mut(settings_content, value, |language, value| {
                            language.indent_guides.get_or_insert_default().line_width = value;
                        })
                    },
                }),
                metadata: None,
                files: USER | PROJECT,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Active Line Width"),
                description: localization::localized_str!("The width of the active indent guide in pixels, between 1 and 10."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("languages.$(language).indent_guides.active_line_width"),
                    pick: |settings_content| {
                        language_settings_field(settings_content, |language| {
                            language
                                .indent_guides
                                .as_ref()
                                .and_then(|indent_guides| indent_guides.active_line_width.as_ref())
                        })
                    },
                    write: |settings_content, value, _| {
                        language_settings_field_mut(settings_content, value, |language, value| {
                            language
                                .indent_guides
                                .get_or_insert_default()
                                .active_line_width = value;
                        })
                    },
                }),
                metadata: None,
                files: USER | PROJECT,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Coloring"),
                description: localization::localized_str!("Determines how indent guides are colored."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("languages.$(language).indent_guides.coloring"),
                    pick: |settings_content| {
                        language_settings_field(settings_content, |language| {
                            language
                                .indent_guides
                                .as_ref()
                                .and_then(|indent_guides| indent_guides.coloring.as_ref())
                        })
                    },
                    write: |settings_content, value, _| {
                        language_settings_field_mut(settings_content, value, |language, value| {
                            language.indent_guides.get_or_insert_default().coloring = value;
                        })
                    },
                }),
                metadata: None,
                files: USER | PROJECT,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Background Coloring"),
                description: localization::localized_str!("Determines how indent guide backgrounds are colored."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("languages.$(language).indent_guides.background_coloring"),
                    pick: |settings_content| {
                        language_settings_field(settings_content, |language| {
                            language.indent_guides.as_ref().and_then(|indent_guides| {
                                indent_guides.background_coloring.as_ref()
                            })
                        })
                    },
                    write: |settings_content, value, _| {
                        language_settings_field_mut(settings_content, value, |language, value| {
                            language
                                .indent_guides
                                .get_or_insert_default()
                                .background_coloring = value;
                        })
                    },
                }),
                metadata: None,
                files: USER | PROJECT,
            }),
        ]
    }

    fn formatting_section() -> [SettingsPageItem; 8] {
        [
            SettingsPageItem::SectionHeader(localization::localized_str!("Formatting")),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Format On Save"),
                description: localization::localized_str!("On: format the whole buffer.\nOff: do not format.\nModifications: format only lines with unstaged changes; skips formatting when a git diff or LSP range formatting is unavailable.\nModifications If Available: same, but falls back to formatting the whole buffer."),
                field: Box::new(
                    // TODO(settings_ui): this setting should just be a bool
                    SettingField {
                        organization_override: None,
                        json_path: Some("languages.$(language).format_on_save"),
                        pick: |settings_content| {
                            language_settings_field(settings_content, |language| {
                                language.format_on_save.as_ref()
                            })
                        },
                        write: |settings_content, value, _| {
                            language_settings_field_mut(
                                settings_content,
                                value,
                                |language, value| {
                                    language.format_on_save = value;
                                },
                            )
                        },
                    },
                ),
                metadata: None,
                files: USER | PROJECT,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Remove Trailing Whitespace On Save"),
                description: localization::localized_str!("Whether or not to remove any trailing whitespace from lines of a buffer before saving it."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("languages.$(language).remove_trailing_whitespace_on_save"),
                    pick: |settings_content| {
                        language_settings_field(settings_content, |language| {
                            language.remove_trailing_whitespace_on_save.as_ref()
                        })
                    },
                    write: |settings_content, value, _| {
                        language_settings_field_mut(settings_content, value, |language, value| {
                            language.remove_trailing_whitespace_on_save = value;
                        })
                    },
                }),
                metadata: None,
                files: USER | PROJECT,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Ensure Final Newline On Save"),
                description: localization::localized_str!("Whether or not to ensure there's a single newline at the end of a buffer when saving it."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("languages.$(language).ensure_final_newline_on_save"),
                    pick: |settings_content| {
                        language_settings_field(settings_content, |language| {
                            language.ensure_final_newline_on_save.as_ref()
                        })
                    },
                    write: |settings_content, value, _| {
                        language_settings_field_mut(settings_content, value, |language, value| {
                            language.ensure_final_newline_on_save = value;
                        })
                    },
                }),
                metadata: None,
                files: USER | PROJECT,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Line Ending"),
                description: localization::localized_str!("How line endings should be handled for new files and during format and save operations."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("languages.$(language).line_ending"),
                    pick: |settings_content| {
                        language_settings_field(settings_content, |language| {
                            language.line_ending.as_ref()
                        })
                    },
                    write: |settings_content, value, _| {
                        language_settings_field_mut(settings_content, value, |language, value| {
                            language.line_ending = value;
                        })
                    },
                }),
                metadata: Some(Box::new(SettingsFieldMetadata {
                    should_do_titlecase: Some(false),
                    ..Default::default()
                })),
                files: USER | PROJECT,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Formatter"),
                description: localization::localized_str!("How to perform a buffer format."),
                field: Box::new(
                    SettingField {
                        organization_override: None,
                        json_path: Some("languages.$(language).formatter"),
                        pick: |settings_content| {
                            language_settings_field(settings_content, |language| {
                                language.formatter.as_ref()
                            })
                        },
                        write: |settings_content, value, _| {
                            language_settings_field_mut(
                                settings_content,
                                value,
                                |language, value| {
                                    language.formatter = value;
                                },
                            )
                        },
                    }
                    .unimplemented(),
                ),
                metadata: None,
                files: USER | PROJECT,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Use On Type Format"),
                description: localization::localized_str!("Whether to use additional LSP queries to format (and amend) the code after every \"trigger\" symbol input, defined by LSP server capabilities"),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("languages.$(language).use_on_type_format"),
                    pick: |settings_content| {
                        language_settings_field(settings_content, |language| {
                            language.use_on_type_format.as_ref()
                        })
                    },
                    write: |settings_content, value, _| {
                        language_settings_field_mut(settings_content, value, |language, value| {
                            language.use_on_type_format = value;
                        })
                    },
                }),
                metadata: None,
                files: USER | PROJECT,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Code Actions On Format"),
                description: localization::localized_str!("Additional code actions to run when formatting."),
                field: Box::new(
                    SettingField {
                        organization_override: None,
                        json_path: Some("languages.$(language).code_actions_on_format"),
                        pick: |settings_content| {
                            language_settings_field(settings_content, |language| {
                                language.code_actions_on_format.as_ref()
                            })
                        },
                        write: |settings_content, value, _| {
                            language_settings_field_mut(
                                settings_content,
                                value,
                                |language, value| {
                                    language.code_actions_on_format = value;
                                },
                            )
                        },
                    }
                    .unimplemented(),
                ),
                metadata: None,
                files: USER | PROJECT,
            }),
        ]
    }

    fn autoclose_section() -> [SettingsPageItem; 5] {
        [
            SettingsPageItem::SectionHeader(localization::localized_str!("Autoclose")),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Use Autoclose"),
                description: localization::localized_str!("Whether to automatically type closing characters for you. For example, when you type '(', HiCodeer will automatically add a closing ')' at the correct position."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("languages.$(language).use_autoclose"),
                    pick: |settings_content| {
                        language_settings_field(settings_content, |language| {
                            language.use_autoclose.as_ref()
                        })
                    },
                    write: |settings_content, value, _| {
                        language_settings_field_mut(settings_content, value, |language, value| {
                            language.use_autoclose = value;
                        })
                    },
                }),
                metadata: None,
                files: USER | PROJECT,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Use Auto Surround"),
                description: localization::localized_str!("Whether to automatically surround text with characters for you. For example, when you select text and type '(', HiCodeer will automatically surround text with ()."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("languages.$(language).use_auto_surround"),
                    pick: |settings_content| {
                        language_settings_field(settings_content, |language| {
                            language.use_auto_surround.as_ref()
                        })
                    },
                    write: |settings_content, value, _| {
                        language_settings_field_mut(settings_content, value, |language, value| {
                            language.use_auto_surround = value;
                        })
                    },
                }),
                metadata: None,
                files: USER | PROJECT,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Always Treat Brackets As Autoclosed"),
                description: localization::localized_str!("Controls whether the closing characters are always skipped over and auto-removed no matter how they were inserted."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("languages.$(language).always_treat_brackets_as_autoclosed"),
                    pick: |settings_content| {
                        language_settings_field(settings_content, |language| {
                            language.always_treat_brackets_as_autoclosed.as_ref()
                        })
                    },
                    write: |settings_content, value, _| {
                        language_settings_field_mut(settings_content, value, |language, value| {
                            language.always_treat_brackets_as_autoclosed = value;
                        })
                    },
                }),
                metadata: None,
                files: USER | PROJECT,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("JSX Tag Auto Close"),
                description: localization::localized_str!("Whether to automatically close JSX tags."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("languages.$(language).jsx_tag_auto_close"),
                    // TODO(settings_ui): this setting should just be a bool
                    pick: |settings_content| {
                        language_settings_field(settings_content, |language| {
                            language.jsx_tag_auto_close.as_ref()?.enabled.as_ref()
                        })
                    },
                    write: |settings_content, value, _| {
                        language_settings_field_mut(settings_content, value, |language, value| {
                            language.jsx_tag_auto_close.get_or_insert_default().enabled = value;
                        })
                    },
                }),
                metadata: None,
                files: USER | PROJECT,
            }),
        ]
    }

    fn whitespace_section() -> [SettingsPageItem; 4] {
        [
            SettingsPageItem::SectionHeader(localization::localized_str!("Whitespace")),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Show Whitespaces"),
                description: localization::localized_str!("Whether to show tabs and spaces in the editor."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("languages.$(language).show_whitespaces"),
                    pick: |settings_content| {
                        language_settings_field(settings_content, |language| {
                            language.show_whitespaces.as_ref()
                        })
                    },
                    write: |settings_content, value, _| {
                        language_settings_field_mut(settings_content, value, |language, value| {
                            language.show_whitespaces = value;
                        })
                    },
                }),
                metadata: None,
                files: USER | PROJECT,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Space Whitespace Indicator"),
                description: localization::localized_str!("Visible character used to render space characters when show_whitespaces is enabled (default: \"•\")"),
                field: Box::new(
                    SettingField {
                        organization_override: None,
                        json_path: Some("languages.$(language).whitespace_map.space"),
                        pick: |settings_content| {
                            language_settings_field(settings_content, |language| {
                                language.whitespace_map.as_ref()?.space.as_ref()
                            })
                        },
                        write: |settings_content, value, _| {
                            language_settings_field_mut(
                                settings_content,
                                value,
                                |language, value| {
                                    language.whitespace_map.get_or_insert_default().space = value;
                                },
                            )
                        },
                    }
                    .unimplemented(),
                ),
                metadata: None,
                files: USER | PROJECT,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Tab Whitespace Indicator"),
                description: localization::localized_str!("Visible character used to render tab characters when show_whitespaces is enabled (default: \"→\")"),
                field: Box::new(
                    SettingField {
                        organization_override: None,
                        json_path: Some("languages.$(language).whitespace_map.tab"),
                        pick: |settings_content| {
                            language_settings_field(settings_content, |language| {
                                language.whitespace_map.as_ref()?.tab.as_ref()
                            })
                        },
                        write: |settings_content, value, _| {
                            language_settings_field_mut(
                                settings_content,
                                value,
                                |language, value| {
                                    language.whitespace_map.get_or_insert_default().tab = value;
                                },
                            )
                        },
                    }
                    .unimplemented(),
                ),
                metadata: None,
                files: USER | PROJECT,
            }),
        ]
    }

    fn completions_section() -> [SettingsPageItem; 8] {
        [
            SettingsPageItem::SectionHeader(localization::localized_str!("Completions")),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Show Completions On Input"),
                description: localization::localized_str!("Whether to pop the completions menu while typing in an editor without explicitly requesting it."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("languages.$(language).show_completions_on_input"),
                    pick: |settings_content| {
                        language_settings_field(settings_content, |language| {
                            language.show_completions_on_input.as_ref()
                        })
                    },
                    write: |settings_content, value, _| {
                        language_settings_field_mut(settings_content, value, |language, value| {
                            language.show_completions_on_input = value;
                        })
                    },
                }),
                metadata: None,
                files: USER | PROJECT,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Show Completion Documentation"),
                description: localization::localized_str!("Whether to display inline and alongside documentation for items in the completions menu."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("languages.$(language).show_completion_documentation"),
                    pick: |settings_content| {
                        language_settings_field(settings_content, |language| {
                            language.show_completion_documentation.as_ref()
                        })
                    },
                    write: |settings_content, value, _| {
                        language_settings_field_mut(settings_content, value, |language, value| {
                            language.show_completion_documentation = value;
                        })
                    },
                }),
                metadata: None,
                files: USER | PROJECT,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Words"),
                description: localization::localized_str!("Controls how words are completed."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("languages.$(language).completions.words"),
                    pick: |settings_content| {
                        language_settings_field(settings_content, |language| {
                            language.completions.as_ref()?.words.as_ref()
                        })
                    },
                    write: |settings_content, value, _| {
                        language_settings_field_mut(settings_content, value, |language, value| {
                            language.completions.get_or_insert_default().words = value;
                        })
                    },
                }),
                metadata: None,
                files: USER | PROJECT,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Words Min Length"),
                description: localization::localized_str!("How many characters has to be in the completions query to automatically show the words-based completions."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("languages.$(language).completions.words_min_length"),
                    pick: |settings_content| {
                        language_settings_field(settings_content, |language| {
                            language.completions.as_ref()?.words_min_length.as_ref()
                        })
                    },
                    write: |settings_content, value, _| {
                        language_settings_field_mut(settings_content, value, |language, value| {
                            language
                                .completions
                                .get_or_insert_default()
                                .words_min_length = value;
                        })
                    },
                }),
                metadata: None,
                files: USER | PROJECT,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Completion Menu Scrollbar"),
                description: localization::localized_str!("When to show the scrollbar in the completion menu."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("editor.completion_menu_scrollbar"),
                    pick: |settings_content| {
                        settings_content.editor.completion_menu_scrollbar.as_ref()
                    },
                    write: |settings_content, value, _| {
                        settings_content.editor.completion_menu_scrollbar = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Completion Detail Alignment"),
                description: localization::localized_str!("Whether to align detail text in code completions context menus left or right."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("editor.completion_detail_alignment"),
                    pick: |settings_content| {
                        settings_content.editor.completion_detail_alignment.as_ref()
                    },
                    write: |settings_content, value, _| {
                        settings_content.editor.completion_detail_alignment = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Completion Menu Item Kind"),
                description: localization::localized_str!("How to display the LSP item kind (function, method, variable, etc.) of each entry in the completions menu."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("editor.completion_menu_item_kind"),
                    pick: |settings_content| {
                        settings_content.editor.completion_menu_item_kind.as_ref()
                    },
                    write: |settings_content, value, _| {
                        settings_content.editor.completion_menu_item_kind = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
        ]
    }

    fn inlay_hints_section() -> [SettingsPageItem; 10] {
        [
            SettingsPageItem::SectionHeader(localization::localized_str!("Inlay Hints")),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Enabled"),
                description: localization::localized_str!("Global switch to toggle hints on and off."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("languages.$(language).inlay_hints.enabled"),
                    pick: |settings_content| {
                        language_settings_field(settings_content, |language| {
                            language.inlay_hints.as_ref()?.enabled.as_ref()
                        })
                    },
                    write: |settings_content, value, _| {
                        language_settings_field_mut(settings_content, value, |language, value| {
                            language.inlay_hints.get_or_insert_default().enabled = value;
                        })
                    },
                }),
                metadata: None,
                files: USER | PROJECT,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Show Value Hints"),
                description: localization::localized_str!("Global switch to toggle inline values on and off when debugging."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("languages.$(language).inlay_hints.show_value_hints"),
                    pick: |settings_content| {
                        language_settings_field(settings_content, |language| {
                            language.inlay_hints.as_ref()?.show_value_hints.as_ref()
                        })
                    },
                    write: |settings_content, value, _| {
                        language_settings_field_mut(settings_content, value, |language, value| {
                            language
                                .inlay_hints
                                .get_or_insert_default()
                                .show_value_hints = value;
                        })
                    },
                }),
                metadata: None,
                files: USER | PROJECT,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Show Type Hints"),
                description: localization::localized_str!("Whether type hints should be shown."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("languages.$(language).inlay_hints.show_type_hints"),
                    pick: |settings_content| {
                        language_settings_field(settings_content, |language| {
                            language.inlay_hints.as_ref()?.show_type_hints.as_ref()
                        })
                    },
                    write: |settings_content, value, _| {
                        language_settings_field_mut(settings_content, value, |language, value| {
                            language.inlay_hints.get_or_insert_default().show_type_hints = value;
                        })
                    },
                }),
                metadata: None,
                files: USER | PROJECT,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Show Parameter Hints"),
                description: localization::localized_str!("Whether parameter hints should be shown."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("languages.$(language).inlay_hints.show_parameter_hints"),
                    pick: |settings_content| {
                        language_settings_field(settings_content, |language| {
                            language.inlay_hints.as_ref()?.show_parameter_hints.as_ref()
                        })
                    },
                    write: |settings_content, value, _| {
                        language_settings_field_mut(settings_content, value, |language, value| {
                            language
                                .inlay_hints
                                .get_or_insert_default()
                                .show_parameter_hints = value;
                        })
                    },
                }),
                metadata: None,
                files: USER | PROJECT,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Show Other Hints"),
                description: localization::localized_str!("Whether other hints should be shown."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("languages.$(language).inlay_hints.show_other_hints"),
                    pick: |settings_content| {
                        language_settings_field(settings_content, |language| {
                            language.inlay_hints.as_ref()?.show_other_hints.as_ref()
                        })
                    },
                    write: |settings_content, value, _| {
                        language_settings_field_mut(settings_content, value, |language, value| {
                            language
                                .inlay_hints
                                .get_or_insert_default()
                                .show_other_hints = value;
                        })
                    },
                }),
                metadata: None,
                files: USER | PROJECT,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Show Background"),
                description: localization::localized_str!("Show a background for inlay hints."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("languages.$(language).inlay_hints.show_background"),
                    pick: |settings_content| {
                        language_settings_field(settings_content, |language| {
                            language.inlay_hints.as_ref()?.show_background.as_ref()
                        })
                    },
                    write: |settings_content, value, _| {
                        language_settings_field_mut(settings_content, value, |language, value| {
                            language.inlay_hints.get_or_insert_default().show_background = value;
                        })
                    },
                }),
                metadata: None,
                files: USER | PROJECT,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Edit Debounce Ms"),
                description: localization::localized_str!("Whether or not to debounce inlay hints updates after buffer edits (set to 0 to disable debouncing)."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("languages.$(language).inlay_hints.edit_debounce_ms"),
                    pick: |settings_content| {
                        language_settings_field(settings_content, |language| {
                            language.inlay_hints.as_ref()?.edit_debounce_ms.as_ref()
                        })
                    },
                    write: |settings_content, value, _| {
                        language_settings_field_mut(settings_content, value, |language, value| {
                            language
                                .inlay_hints
                                .get_or_insert_default()
                                .edit_debounce_ms = value;
                        })
                    },
                }),
                metadata: None,
                files: USER | PROJECT,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Scroll Debounce Ms"),
                description: localization::localized_str!("Whether or not to debounce inlay hints updates after buffer scrolls (set to 0 to disable debouncing)."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("languages.$(language).inlay_hints.scroll_debounce_ms"),
                    pick: |settings_content| {
                        language_settings_field(settings_content, |language| {
                            language.inlay_hints.as_ref()?.scroll_debounce_ms.as_ref()
                        })
                    },
                    write: |settings_content, value, _| {
                        language_settings_field_mut(settings_content, value, |language, value| {
                            language
                                .inlay_hints
                                .get_or_insert_default()
                                .scroll_debounce_ms = value;
                        })
                    },
                }),
                metadata: None,
                files: USER | PROJECT,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Toggle On Modifiers Press"),
                description: localization::localized_str!("Toggles inlay hints (hides or shows) when the user presses the modifiers specified."),
                field: Box::new(
                    SettingField {
                        organization_override: None,
                        json_path: Some(
                            "languages.$(language).inlay_hints.toggle_on_modifiers_press",
                        ),
                        pick: |settings_content| {
                            language_settings_field(settings_content, |language| {
                                language
                                    .inlay_hints
                                    .as_ref()?
                                    .toggle_on_modifiers_press
                                    .as_ref()
                            })
                        },
                        write: |settings_content, value, _| {
                            language_settings_field_mut(
                                settings_content,
                                value,
                                |language, value| {
                                    language
                                        .inlay_hints
                                        .get_or_insert_default()
                                        .toggle_on_modifiers_press = value;
                                },
                            )
                        },
                    }
                    .unimplemented(),
                ),
                metadata: None,
                files: USER | PROJECT,
            }),
        ]
    }

    fn tasks_section() -> [SettingsPageItem; 4] {
        [
            SettingsPageItem::SectionHeader(localization::localized_str!("Tasks")),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Enabled"),
                description: localization::localized_str!("Whether tasks are enabled for this language."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("languages.$(language).tasks.enabled"),
                    pick: |settings_content| {
                        language_settings_field(settings_content, |language| {
                            language.tasks.as_ref()?.enabled.as_ref()
                        })
                    },
                    write: |settings_content, value, _| {
                        language_settings_field_mut(settings_content, value, |language, value| {
                            language.tasks.get_or_insert_default().enabled = value;
                        })
                    },
                }),
                metadata: None,
                files: USER | PROJECT,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Variables"),
                description: localization::localized_str!("Extra task variables to set for a particular language."),
                field: Box::new(
                    SettingField {
                        organization_override: None,
                        json_path: Some("languages.$(language).tasks.variables"),
                        pick: |settings_content| {
                            language_settings_field(settings_content, |language| {
                                language.tasks.as_ref()?.variables.as_ref()
                            })
                        },
                        write: |settings_content, value, _| {
                            language_settings_field_mut(
                                settings_content,
                                value,
                                |language, value| {
                                    language.tasks.get_or_insert_default().variables = value;
                                },
                            )
                        },
                    }
                    .unimplemented(),
                ),
                metadata: None,
                files: USER | PROJECT,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Prefer LSP"),
                description: localization::localized_str!("Use LSP tasks over HiCodeer language extension tasks."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("languages.$(language).tasks.prefer_lsp"),
                    pick: |settings_content| {
                        language_settings_field(settings_content, |language| {
                            language.tasks.as_ref()?.prefer_lsp.as_ref()
                        })
                    },
                    write: |settings_content, value, _| {
                        language_settings_field_mut(settings_content, value, |language, value| {
                            language.tasks.get_or_insert_default().prefer_lsp = value;
                        })
                    },
                }),
                metadata: None,
                files: USER | PROJECT,
            }),
        ]
    }

    fn miscellaneous_section() -> Vec<SettingsPageItem> {
        vec![
            SettingsPageItem::SectionHeader(localization::localized_str!("Miscellaneous")),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Language Detection"),
                description: localization::localized_str!("Whether to enable automatic language detection in unsaved buffers."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("language_detection"),
                    pick: |settings_content| settings_content.editor.language_detection.as_ref(),
                    write: |settings_content, value, _| {
                        settings_content.editor.language_detection = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Word Diff Enabled"),
                description: localization::localized_str!("Whether to enable word diff highlighting in the editor. When enabled, changed words within modified lines are highlighted to show exactly what changed."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("languages.$(language).word_diff_enabled"),
                    pick: |settings_content| {
                        language_settings_field(settings_content, |language| {
                            language.word_diff_enabled.as_ref()
                        })
                    },
                    write: |settings_content, value, _| {
                        language_settings_field_mut(settings_content, value, |language, value| {
                            language.word_diff_enabled = value;
                        })
                    },
                }),
                metadata: None,
                files: USER | PROJECT,
            }),
            // [OHOS PORT BEGIN] HiCodeer ships without the debugger stack, so a
            // preferred-debugger list has nothing to drive.
            #[cfg(not(target_env = "ohos"))]
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Debuggers"),
                description: localization::localized_str!("Preferred debuggers for this language."),
                field: Box::new(
                    SettingField {
                        organization_override: None,
                        json_path: Some("languages.$(language).debuggers"),
                        pick: |settings_content| {
                            language_settings_field(settings_content, |language| {
                                language.debuggers.as_ref()
                            })
                        },
                        write: |settings_content, value, _| {
                            language_settings_field_mut(
                                settings_content,
                                value,
                                |language, value| {
                                    language.debuggers = value;
                                },
                            )
                        },
                    }
                    .unimplemented(),
                ),
                metadata: None,
                files: USER | PROJECT,
            }),
            // [OHOS PORT BEGIN] Middle-click paste is an X11 convention; this
            // build has no pointer-driven paste path that honors it.
            #[cfg(not(target_env = "ohos"))]
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Middle Click Paste"),
                description: localization::localized_str!("Enable middle-click paste on Linux."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("languages.$(language).editor.middle_click_paste"),
                    pick: |settings_content| settings_content.editor.middle_click_paste.as_ref(),
                    write: |settings_content, value, _| {
                        settings_content.editor.middle_click_paste = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Extend Comment On Newline"),
                description: localization::localized_str!("Whether to start a new line with a comment when a previous line is a comment as well."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("languages.$(language).extend_comment_on_newline"),
                    pick: |settings_content| {
                        language_settings_field(settings_content, |language| {
                            language.extend_comment_on_newline.as_ref()
                        })
                    },
                    write: |settings_content, value, _| {
                        language_settings_field_mut(settings_content, value, |language, value| {
                            language.extend_comment_on_newline = value;
                        })
                    },
                }),
                metadata: None,
                files: USER | PROJECT,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Colorize Brackets"),
                description: localization::localized_str!("Whether to colorize brackets in the editor."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("languages.$(language).colorize_brackets"),
                    pick: |settings_content| {
                        language_settings_field(settings_content, |language| {
                            language.colorize_brackets.as_ref()
                        })
                    },
                    write: |settings_content, value, _| {
                        language_settings_field_mut(settings_content, value, |language, value| {
                            language.colorize_brackets = value;
                        })
                    },
                }),
                metadata: None,
                files: USER | PROJECT,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Vim/Emacs Modeline Support"),
                description: localization::localized_str!("Number of lines to search for modelines (set to 0 to disable)."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("modeline_lines"),
                    pick: |settings_content| settings_content.modeline_lines.as_ref(),
                    write: |settings_content, value, _| {
                        settings_content.modeline_lines = value;
                    },
                }),
                metadata: None,
                files: USER | PROJECT,
            }),
        ]
    }

    fn global_only_miscellaneous_sub_section() -> [SettingsPageItem; 4] {
        [
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Image Viewer"),
                description: localization::localized_str!("The unit for image file sizes."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("image_viewer.unit"),
                    pick: |settings_content| {
                        settings_content
                            .image_viewer
                            .as_ref()
                            .and_then(|image_viewer| image_viewer.unit.as_ref())
                    },
                    write: |settings_content, value, _| {
                        settings_content.image_viewer.get_or_insert_default().unit = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Open Markdown Files in Preview"),
                description: localization::localized_str!("Whether to automatically open Markdown files in the preview."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("markdown_preview.open_markdown_files_in_preview"),
                    pick: |settings_content| {
                        settings_content
                            .markdown_preview
                            .as_ref()?
                            .open_markdown_files_in_preview
                            .as_ref()
                    },
                    write: |settings_content, value, _| {
                        settings_content
                            .markdown_preview
                            .get_or_insert_default()
                            .open_markdown_files_in_preview = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
            SettingsPageItem::DynamicItem(DynamicItem {
                discriminant: SettingItem {
                    files: USER,
                    title: localization::localized_str!("Limit Markdown Preview Width"),
                    description: localization::localized_str!("Whether to constrain the markdown preview content to a maximum width, centering it when the pane is wider, for optimal readability."),
                    field: Box::new(SettingField::<bool> {
                        organization_override: None,
                        json_path: Some("markdown_preview.limit_content_width"),
                        pick: |settings_content| {
                            settings_content
                                .markdown_preview
                                .as_ref()?
                                .limit_content_width
                                .as_ref()
                        },
                        write: |settings_content, value, _| {
                            settings_content
                                .markdown_preview
                                .get_or_insert_default()
                                .limit_content_width = value;
                        },
                    }),
                    metadata: None,
                },
                pick_discriminant: |settings_content| {
                    let enabled = settings_content
                        .markdown_preview
                        .as_ref()?
                        .limit_content_width
                        .unwrap_or(true);
                    Some(if enabled { 1 } else { 0 })
                },
                fields: vec![
                    vec![],
                    vec![SettingItem {
                        files: USER,
                        title: localization::localized_str!("Max Width"),
                        description: localization::localized_str!("Maximum content width in pixels. Content will be centered when the pane is wider than this value."),
                        field: Box::new(SettingField {
                            organization_override: None,
                            json_path: Some("markdown_preview.max_width"),
                            pick: |settings_content| {
                                settings_content
                                    .markdown_preview
                                    .as_ref()?
                                    .max_width
                                    .as_ref()
                            },
                            write: |settings_content, value, _| {
                                settings_content
                                    .markdown_preview
                                    .get_or_insert_default()
                                    .max_width = value;
                            },
                        }),
                        metadata: None,
                    }],
                ],
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Drop Size Target"),
                description: localization::localized_str!("Relative size of the drop target in the editor that will open dropped file as a split pane."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("drop_target_size"),
                    pick: |settings_content| settings_content.workspace.drop_target_size.as_ref(),
                    write: |settings_content, value, _| {
                        settings_content.workspace.drop_target_size = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
        ]
    }

    let is_global = active_language().is_none();

    let code_lens_item = [SettingsPageItem::SettingItem(SettingItem {
        title: localization::localized_str!("Code Lens"),
        description: localization::localized_str!("Whether and how to display code lenses from language servers."),
        field: Box::new(SettingField {
            organization_override: None,
            json_path: Some("code_lens"),
            pick: |settings_content| settings_content.editor.code_lens.as_ref(),
            write: |settings_content, value, _| {
                settings_content.editor.code_lens = value;
            },
        }),
        metadata: None,
        files: USER,
    })];

    let lsp_document_colors_item = [SettingsPageItem::SettingItem(SettingItem {
        title: localization::localized_str!("LSP Document Colors"),
        description: localization::localized_str!("How to render LSP color previews in the editor."),
        field: Box::new(SettingField {
            organization_override: None,
            json_path: Some("lsp_document_colors"),
            pick: |settings_content| settings_content.editor.lsp_document_colors.as_ref(),
            write: |settings_content, value, _| {
                settings_content.editor.lsp_document_colors = value;
            },
        }),
        metadata: None,
        files: USER,
    })];

    if is_global {
        concat_sections!(
            indentation_section(),
            wrapping_section(),
            indent_guides_section(),
            formatting_section(),
            autoclose_section(),
            whitespace_section(),
            completions_section(),
            inlay_hints_section(),
            code_lens_item,
            lsp_document_colors_item,
            tasks_section(),
            miscellaneous_section(),
            global_only_miscellaneous_sub_section(),
        )
    } else {
        concat_sections!(
            indentation_section(),
            wrapping_section(),
            indent_guides_section(),
            formatting_section(),
            autoclose_section(),
            whitespace_section(),
            completions_section(),
            inlay_hints_section(),
            code_lens_item,
            tasks_section(),
            miscellaneous_section(),
        )
    }
}

/// LanguageSettings items that should be included in the "Languages & Tools" page
/// not the "Editor" page
fn non_editor_language_settings_data() -> Box<[SettingsPageItem]> {
    fn lsp_section() -> [SettingsPageItem; 10] {
        [
            SettingsPageItem::SectionHeader(localization::localized_str!("LSP")),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Enable Language Server"),
                description: localization::localized_str!("Whether to use language servers to provide code intelligence."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("languages.$(language).enable_language_server"),
                    pick: |settings_content| {
                        language_settings_field(settings_content, |language| {
                            language.enable_language_server.as_ref()
                        })
                    },
                    write: |settings_content, value, _| {
                        language_settings_field_mut(settings_content, value, |language, value| {
                            language.enable_language_server = value;
                        })
                    },
                }),
                metadata: None,
                files: USER | PROJECT,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Language Servers"),
                description: localization::localized_str!("The list of language servers to use (or disable) for this language."),
                field: Box::new(
                    SettingField {
                        organization_override: None,
                        json_path: Some("languages.$(language).language_servers"),
                        pick: |settings_content| {
                            language_settings_field(settings_content, |language| {
                                language.language_servers.as_ref()
                            })
                        },
                        write: |settings_content, value, _| {
                            language_settings_field_mut(
                                settings_content,
                                value,
                                |language, value| {
                                    language.language_servers = value;
                                },
                            )
                        },
                    }
                    .unimplemented(),
                ),
                metadata: None,
                files: USER | PROJECT,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Linked Edits"),
                description: localization::localized_str!("Whether to perform linked edits of associated ranges, if the LS supports it. For example, when editing opening <html> tag, the contents of the closing </html> tag will be edited as well."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("languages.$(language).linked_edits"),
                    pick: |settings_content| {
                        language_settings_field(settings_content, |language| {
                            language.linked_edits.as_ref()
                        })
                    },
                    write: |settings_content, value, _| {
                        language_settings_field_mut(settings_content, value, |language, value| {
                            language.linked_edits = value;
                        })
                    },
                }),
                metadata: None,
                files: USER | PROJECT,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Go To Definition Fallback"),
                description: localization::localized_str!("Whether to follow-up empty Go to definition responses from the language server."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("go_to_definition_fallback"),
                    pick: |settings_content| {
                        settings_content.editor.go_to_definition_fallback.as_ref()
                    },
                    write: |settings_content, value, _| {
                        settings_content.editor.go_to_definition_fallback = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Go To Definition Scroll Strategy"),
                description: localization::localized_str!("How to scroll the target into view when navigating to a definition or reference."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("go_to_definition_scroll_strategy"),
                    pick: |settings_content| {
                        settings_content
                            .editor
                            .go_to_definition_scroll_strategy
                            .as_ref()
                    },
                    write: |settings_content, value, _| {
                        settings_content.editor.go_to_definition_scroll_strategy = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("LSP Results Location"),
                description: localization::localized_str!("Where to show LSP results that can contain multiple locations (Go to Definition, Go to Implementation, Find All References)."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("lsp_results_location"),
                    pick: |settings_content| settings_content.editor.lsp_results_location.as_ref(),
                    write: |settings_content, value, _| {
                        settings_content.editor.lsp_results_location = value;
                    },
                }),
                metadata: None,
                files: USER,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Semantic Tokens"),
                description: {
                    static DESCRIPTION: OnceLock<&'static str> = OnceLock::new();
                    DESCRIPTION.get_or_init(|| {
                        SemanticTokens::VARIANTS
                            .iter()
                            .filter_map(|v| {
                                v.get_documentation().map(|doc| format!("{v:?}: {doc}"))
                            })
                            .join("\n")
                            .leak()
                    })
                },
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("languages.$(language).semantic_tokens"),
                    pick: |settings_content| {
                        settings_content
                            .project
                            .all_languages
                            .defaults
                            .semantic_tokens
                            .as_ref()
                    },
                    write: |settings_content, value, _| {
                        settings_content
                            .project
                            .all_languages
                            .defaults
                            .semantic_tokens = value;
                    },
                }),
                metadata: None,
                files: USER | PROJECT,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("LSP Folding Ranges"),
                description: localization::localized_str!("When enabled, use folding ranges from the language server instead of indent-based folding."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("languages.$(language).document_folding_ranges"),
                    pick: |settings_content| {
                        language_settings_field(settings_content, |language| {
                            language.document_folding_ranges.as_ref()
                        })
                    },
                    write: |settings_content, value, _| {
                        language_settings_field_mut(settings_content, value, |language, value| {
                            language.document_folding_ranges = value;
                        })
                    },
                }),
                metadata: None,
                files: USER | PROJECT,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("LSP Document Symbols"),
                description: localization::localized_str!("When enabled, use the language server's document symbols for outlines and breadcrumbs instead of tree-sitter."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("languages.$(language).document_symbols"),
                    pick: |settings_content| {
                        language_settings_field(settings_content, |language| {
                            language.document_symbols.as_ref()
                        })
                    },
                    write: |settings_content, value, _| {
                        language_settings_field_mut(settings_content, value, |language, value| {
                            language.document_symbols = value;
                        })
                    },
                }),
                metadata: None,
                files: USER | PROJECT,
            }),
        ]
    }

    fn lsp_completions_section() -> [SettingsPageItem; 4] {
        [
            SettingsPageItem::SectionHeader(localization::localized_str!("LSP Completions")),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Enabled"),
                description: localization::localized_str!("Whether to fetch LSP completions or not."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("languages.$(language).completions.lsp"),
                    pick: |settings_content| {
                        language_settings_field(settings_content, |language| {
                            language.completions.as_ref()?.lsp.as_ref()
                        })
                    },
                    write: |settings_content, value, _| {
                        language_settings_field_mut(settings_content, value, |language, value| {
                            language.completions.get_or_insert_default().lsp = value;
                        })
                    },
                }),
                metadata: None,
                files: USER | PROJECT,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Fetch Timeout (milliseconds)"),
                description: localization::localized_str!("When fetching LSP completions, determines how long to wait for a response of a particular server (set to 0 to wait indefinitely)."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("languages.$(language).completions.lsp_fetch_timeout_ms"),
                    pick: |settings_content| {
                        language_settings_field(settings_content, |language| {
                            language.completions.as_ref()?.lsp_fetch_timeout_ms.as_ref()
                        })
                    },
                    write: |settings_content, value, _| {
                        language_settings_field_mut(settings_content, value, |language, value| {
                            language
                                .completions
                                .get_or_insert_default()
                                .lsp_fetch_timeout_ms = value;
                        })
                    },
                }),
                metadata: None,
                files: USER | PROJECT,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Insert Mode"),
                description: localization::localized_str!("Controls how LSP completions are inserted."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("languages.$(language).completions.lsp_insert_mode"),
                    pick: |settings_content| {
                        language_settings_field(settings_content, |language| {
                            language.completions.as_ref()?.lsp_insert_mode.as_ref()
                        })
                    },
                    write: |settings_content, value, _| {
                        language_settings_field_mut(settings_content, value, |language, value| {
                            language.completions.get_or_insert_default().lsp_insert_mode = value;
                        })
                    },
                }),
                metadata: None,
                files: USER | PROJECT,
            }),
        ]
    }

    fn prettier_section() -> [SettingsPageItem; 5] {
        [
            SettingsPageItem::SectionHeader(localization::localized_str!("Prettier")),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Allowed"),
                description: localization::localized_str!("Enables or disables formatting with Prettier for a given language."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("languages.$(language).prettier.allowed"),
                    pick: |settings_content| {
                        language_settings_field(settings_content, |language| {
                            language.prettier.as_ref()?.allowed.as_ref()
                        })
                    },
                    write: |settings_content, value, _| {
                        language_settings_field_mut(settings_content, value, |language, value| {
                            language.prettier.get_or_insert_default().allowed = value;
                        })
                    },
                }),
                metadata: None,
                files: USER | PROJECT,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Parser"),
                description: localization::localized_str!("Forces Prettier integration to use a specific parser name when formatting files with the language."),
                field: Box::new(SettingField {
                    organization_override: None,
                    json_path: Some("languages.$(language).prettier.parser"),
                    pick: |settings_content| {
                        language_settings_field(settings_content, |language| {
                            language.prettier.as_ref()?.parser.as_ref()
                        })
                    },
                    write: |settings_content, value, _| {
                        language_settings_field_mut(settings_content, value, |language, value| {
                            language.prettier.get_or_insert_default().parser = value;
                        })
                    },
                }),
                metadata: None,
                files: USER | PROJECT,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Plugins"),
                description: localization::localized_str!("Forces Prettier integration to use specific plugins when formatting files with the language."),
                field: Box::new(
                    SettingField {
                        organization_override: None,
                        json_path: Some("languages.$(language).prettier.plugins"),
                        pick: |settings_content| {
                            language_settings_field(settings_content, |language| {
                                language.prettier.as_ref()?.plugins.as_ref()
                            })
                        },
                        write: |settings_content, value, _| {
                            language_settings_field_mut(
                                settings_content,
                                value,
                                |language, value| {
                                    language.prettier.get_or_insert_default().plugins = value;
                                },
                            )
                        },
                    }
                    .unimplemented(),
                ),
                metadata: None,
                files: USER | PROJECT,
            }),
            SettingsPageItem::SettingItem(SettingItem {
                title: localization::localized_str!("Options"),
                description: localization::localized_str!("Default Prettier options, in the format as in package.json section for Prettier."),
                field: Box::new(
                    SettingField {
                        organization_override: None,
                        json_path: Some("languages.$(language).prettier.options"),
                        pick: |settings_content| {
                            language_settings_field(settings_content, |language| {
                                language.prettier.as_ref()?.options.as_ref()
                            })
                        },
                        write: |settings_content, value, _| {
                            language_settings_field_mut(
                                settings_content,
                                value,
                                |language, value| {
                                    language.prettier.get_or_insert_default().options = value;
                                },
                            )
                        },
                    }
                    .unimplemented(),
                ),
                metadata: None,
                files: USER | PROJECT,
            }),
        ]
    }

    concat_sections!(
        lsp_section(),
        lsp_completions_section(),
        prettier_section(),
    )
}


fn show_scrollbar_or_editor(
    settings_content: &SettingsContent,
    show: fn(&SettingsContent) -> Option<&settings::ShowScrollbar>,
) -> Option<&settings::ShowScrollbar> {
    show(settings_content).or(settings_content
        .editor
        .scrollbar
        .as_ref()
        .and_then(|scrollbar| scrollbar.show.as_ref()))
}

fn dynamic_variants<T>() -> &'static [T::Discriminant]
where
    T: strum::IntoDiscriminant,
    T::Discriminant: strum::VariantArray,
{
    <<T as strum::IntoDiscriminant>::Discriminant as strum::VariantArray>::VARIANTS
}

/// Updates the `vim_mode` setting, disabling `helix_mode` if present and
/// `vim_mode` is being enabled.
fn write_vim_mode(settings: &mut SettingsContent, value: Option<bool>, _: &App) {
    write_vim_mode_inner(settings, value);
}

fn write_vim_mode_inner(settings: &mut SettingsContent, value: Option<bool>) {
    if value == Some(true) && settings.helix_mode == Some(true) {
        settings.helix_mode = Some(false);
    }
    settings.vim_mode = value;
}

/// Updates the `helix_mode` setting, disabling `vim_mode` if present and
/// `helix_mode` is being enabled.
fn write_helix_mode(settings: &mut SettingsContent, value: Option<bool>, _: &App) {
    write_helix_mode_inner(settings, value);
}

fn write_helix_mode_inner(settings: &mut SettingsContent, value: Option<bool>) {
    if value == Some(true) && settings.vim_mode == Some(true) {
        settings.vim_mode = Some(false);
    }
    settings.helix_mode = value;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_write_vim_helix_mode() {
        // Enabling vim mode while `vim_mode` and `helix_mode` are not yet set
        // should only update the `vim_mode` setting.
        let mut settings = SettingsContent::default();
        write_vim_mode_inner(&mut settings, Some(true));
        assert_eq!(settings.vim_mode, Some(true));
        assert_eq!(settings.helix_mode, None);

        // Enabling helix mode while `vim_mode` and `helix_mode` are not yet set
        // should only update the `helix_mode` setting.
        let mut settings = SettingsContent::default();
        write_helix_mode_inner(&mut settings, Some(true));
        assert_eq!(settings.helix_mode, Some(true));
        assert_eq!(settings.vim_mode, None);

        // Disabling helix mode should only touch `helix_mode` setting when
        // `vim_mode` is not set.
        write_helix_mode_inner(&mut settings, Some(false));
        assert_eq!(settings.helix_mode, Some(false));
        assert_eq!(settings.vim_mode, None);

        // Enabling vim mode should update `vim_mode` but leave `helix_mode`
        // untouched.
        write_vim_mode_inner(&mut settings, Some(true));
        assert_eq!(settings.vim_mode, Some(true));
        assert_eq!(settings.helix_mode, Some(false));

        // Enabling helix mode should update `helix_mode` and disable
        // `vim_mode`.
        write_helix_mode_inner(&mut settings, Some(true));
        assert_eq!(settings.helix_mode, Some(true));
        assert_eq!(settings.vim_mode, Some(false));

        // Enabling vim mode should update `vim_mode` and disable
        // `helix_mode`.
        write_vim_mode_inner(&mut settings, Some(true));
        assert_eq!(settings.vim_mode, Some(true));
        assert_eq!(settings.helix_mode, Some(false));
    }
}
