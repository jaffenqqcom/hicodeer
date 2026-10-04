use codestral::{CODESTRAL_API_URL, codestral_api_key_state, codestral_api_url};
use edit_prediction::{
    ApiKeyState,
    mercury::{MERCURY_CREDENTIALS_URL, mercury_api_token},
    open_ai_compatible::{open_ai_compatible_api_token, open_ai_compatible_api_url},
};
use edit_prediction_ui::{get_available_providers, set_completion_provider};
use gpui::{App, Entity, ScrollHandle, TaskExt, prelude::*};
use language::language_settings::AllLanguageSettings;

use settings::Settings as _;
use ui::{ButtonLink, ConfiguredApiCard, ContextMenu, DropdownMenu, DropdownStyle, prelude::*};
use workspace::AppState;

const OLLAMA_API_URL_PLACEHOLDER: &str = "http://localhost:11434";
const OLLAMA_MODEL_PLACEHOLDER: &str = "qwen2.5-coder:3b-base";

const OPEN_AI_COMPATIBLE_API_URL_PLACEHOLDER: &str = "http://localhost:8080/v1/completions";
const OPEN_AI_COMPATIBLE_MODEL_PLACEHOLDER: &str = "qwen2.5-coder:3b-base";

use crate::{
    SettingField, SettingItem, SettingsFieldMetadata, SettingsPageItem, SettingsWindow, USER,
    components::{SettingsInputField, SettingsSectionHeader},
};

pub(crate) fn render_edit_prediction_setup_page(
    settings_window: &SettingsWindow,
    scroll_handle: &ScrollHandle,
    window: &mut Window,
    cx: &mut Context<SettingsWindow>,
) -> AnyElement {
    // [OHOS PORT BEGIN] HiCodeer offers only the two providers a user can point
    // at their own server; the hosted ones have no account to connect to. The
    // dropdown above still comes from `get_available_providers`, which enforces
    // the same set. To restore the upstream providers, delete this block.
    #[cfg(target_env = "ohos")]
    let providers = [
        Some(render_ollama_provider(settings_window, window, cx).into_any_element()),
        Some(
            render_api_key_provider(
                IconName::AiOpenAiCompat,
                "OpenAI Compatible API",
                // [OHOS PORT BEGIN] This provider takes a key the user brings
                // themselves, so its label says what the key is for. To restore
                // the upstream label, delete this argument.
                "API Key for authentication",
                // [OHOS PORT END]
                // [OHOS PORT BEGIN] The message is not rendered on this
                // platform, so it is left empty. To restore the upstream
                // providers, delete this block.
                #[cfg(target_env = "ohos")]
                ApiKeyDocs::Custom {
                    message: SharedString::default(),
                },
                // [OHOS PORT END]
                // [OHOS PORT BEGIN] To restore the upstream providers, delete
                // this block.
                #[cfg(not(target_env = "ohos"))]
                ApiKeyDocs::Custom {
                    message: "The API key sent as Authorization: Bearer {key}.".into(),
                },
                // [OHOS PORT END]
                open_ai_compatible_api_token(cx),
                |cx| open_ai_compatible_api_url(cx),
                Some(
                    settings_window
                        .render_sub_page_items_section(
                            open_ai_compatible_settings().iter().enumerate(),
                            true,
                            window,
                            cx,
                        )
                        .into_any_element(),
                ),
                window,
                cx,
            )
            .into_any_element(),
        ),
    ];
    // [OHOS PORT END]
    // [OHOS PORT BEGIN] To restore the upstream providers, delete this block.
    #[cfg(not(target_env = "ohos"))]
    let providers = [
        Some(render_provider_dropdown(window, cx)),
        Some(render_zed_provider(settings_window, window, cx).into_any_element()),
        render_github_copilot_provider(settings_window, window, cx)
            .map(IntoElement::into_any_element),
        Some(
            render_api_key_provider(
                IconName::Inception,
                "Mercury",
                "API Key",
                ApiKeyDocs::Link {
                    dashboard_url: "https://platform.inceptionlabs.ai/dashboard/api-keys".into(),
                },
                mercury_api_token(cx),
                |_cx| MERCURY_CREDENTIALS_URL,
                Some(
                    settings_window
                        .render_sub_page_items_section(
                            mercury_settings().iter().enumerate(),
                            true,
                            window,
                            cx,
                        )
                        .into_any_element(),
                ),
                window,
                cx,
            )
            .into_any_element(),
        ),
        Some(
            render_api_key_provider(
                IconName::AiMistral,
                "Codestral",
                "API Key",
                ApiKeyDocs::Link {
                    dashboard_url: "https://console.mistral.ai/codestral".into(),
                },
                codestral_api_key_state(cx),
                |cx| codestral_api_url(cx),
                Some(
                    settings_window
                        .render_sub_page_items_section(
                            codestral_settings().iter().enumerate(),
                            true,
                            window,
                            cx,
                        )
                        .into_any_element(),
                ),
                window,
                cx,
            )
            .into_any_element(),
        ),
        Some(render_ollama_provider(settings_window, window, cx).into_any_element()),
        Some(
            render_api_key_provider(
                IconName::AiOpenAiCompat,
                "OpenAI Compatible API",
                "API Key",
                ApiKeyDocs::Custom {
                    message: "The API key sent as Authorization: Bearer {key}.".into(),
                },
                open_ai_compatible_api_token(cx),
                |cx| open_ai_compatible_api_url(cx),
                Some(
                    settings_window
                        .render_sub_page_items_section(
                            open_ai_compatible_settings().iter().enumerate(),
                            true,
                            window,
                            cx,
                        )
                        .into_any_element(),
                ),
                window,
                cx,
            )
            .into_any_element(),
        ),
    ];
    // [OHOS PORT END]

    div()
        .size_full()
        .child(
            v_flex()
                .id("ep-setup-page")
                .min_w_0()
                .size_full()
                .px_8()
                .pb_16()
                .overflow_y_scroll()
                .track_scroll(&scroll_handle)
                .children(providers.into_iter().flatten()),
        )
        .into_any_element()
}

fn render_provider_dropdown(window: &mut Window, cx: &mut App) -> AnyElement {
    let current_provider = AllLanguageSettings::get_global(cx)
        .edit_predictions
        .provider;
    let current_provider_name = current_provider.display_name().unwrap_or("No provider set");

    let menu = ContextMenu::build(window, cx, move |mut menu, _, cx| {
        let available_providers = get_available_providers(cx);
        let fs = <dyn fs::Fs>::global(cx);

        for provider in available_providers {
            let Some(name) = provider.display_name() else {
                continue;
            };
            let is_current = provider == current_provider;

            menu = menu.toggleable_entry(name, is_current, IconPosition::Start, None, {
                let fs = fs.clone();
                move |_, cx| {
                    set_completion_provider(fs.clone(), cx, provider);
                }
            });
        }
        menu
    });

    v_flex()
        .id("provider-selector")
        .min_w_0()
        .gap_1p5()
        .child(SettingsSectionHeader::new(localization::localized_str!("Active Provider")).no_padding(true))
        .child(
            h_flex()
                .pt_2p5()
                .w_full()
                .min_w_0()
                .justify_between()
                .child(
                    v_flex()
                        .w_full()
                        .min_w_0()
                        .max_w_1_2()
                        .child(Label::new(localization::localized_str!("Provider")))
                        .child(
                            Label::new(localization::localized_str!("Select which provider to use for edit predictions."))
                                .size(LabelSize::Small)
                                .color(Color::Muted),
                        ),
                )
                .child(
                    DropdownMenu::new("provider-dropdown", current_provider_name, menu)
                        .tab_index(0)
                        .style(DropdownStyle::Outlined),
                ),
        )
        .into_any_element()
}

enum ApiKeyDocs {
    Link { dashboard_url: SharedString },
    Custom { message: SharedString },
}
fn render_api_key_provider(
    icon: IconName,
    title: &'static str,
    // [OHOS PORT BEGIN] Each provider words its own key label, so it is passed
    // in rather than shared: the one offered on this platform says what the key
    // is for. To restore the upstream label, delete this parameter and the
    // `key_label` arguments at the call sites.
    key_label: &'static str,
    // [OHOS PORT END]
    // [OHOS PORT BEGIN] `docs` describes the second line, which is left out on
    // this platform, so the argument goes unnamed to keep it unused. To restore
    // the upstream providers, delete this block.
    #[cfg(target_env = "ohos")]
    _docs: ApiKeyDocs,
    // [OHOS PORT END]
    // [OHOS PORT BEGIN] To restore the upstream providers, delete this block.
    #[cfg(not(target_env = "ohos"))]
    docs: ApiKeyDocs,
    // [OHOS PORT END]
    api_key_state: Entity<ApiKeyState>,
    current_url: fn(&mut App) -> SharedString,
    additional_fields: Option<AnyElement>,
    window: &mut Window,
    cx: &mut Context<SettingsWindow>,
) -> impl IntoElement {
    let weak_page = cx.weak_entity();
    let credentials_provider = zed_credentials_provider::global(cx);
    _ = window.use_keyed_state(current_url(cx), cx, |_, cx| {
        let task = api_key_state.update(cx, |key_state, cx| {
            key_state.load_if_needed(
                current_url(cx),
                |state| state,
                credentials_provider.clone(),
                cx,
            )
        });
        cx.spawn(async move |_, cx| {
            task.await.ok();
            weak_page
                .update(cx, |_, cx| {
                    cx.notify();
                })
                .ok();
        })
    });

    let (has_key, env_var_name, is_from_env_var) = api_key_state.read_with(cx, |state, _| {
        (
            state.has_key(),
            Some(state.env_var_name().clone()),
            state.is_from_env_var(),
        )
    });

    let write_key = move |api_key: Option<String>, cx: &mut App| {
        let credentials_provider = zed_credentials_provider::global(cx);
        api_key_state
            .update(cx, |key_state, cx| {
                let url = current_url(cx);
                key_state.store(
                    url,
                    api_key,
                    |key_state| key_state,
                    credentials_provider,
                    cx,
                )
            })
            .detach_and_log_err(cx);
    };

    let base_container = v_flex().id(title).min_w_0().pt_8().gap_1p5();

    let header = SettingsSectionHeader::new(title)
        .icon(icon)
        .no_padding(true);

    // [OHOS PORT BEGIN] On this platform the second line is left out, so the
    // description is `None` and the label stands alone. To restore it, delete
    // this block.
    #[cfg(target_env = "ohos")]
    let description: Option<gpui::Div> = None;
    // [OHOS PORT END]
    // [OHOS PORT BEGIN] To restore the second line, delete this block.
    #[cfg(not(target_env = "ohos"))]
    let description: Option<gpui::Div> = Some(match docs {
        ApiKeyDocs::Custom { message } => div().min_w_0().w_full().child(
            Label::new(message)
                .size(LabelSize::Small)
                .color(Color::Muted),
        ),
        ApiKeyDocs::Link { dashboard_url } => h_flex()
            .w_full()
            .min_w_0()
            .flex_wrap()
            .gap_0p5()
            .child(
                Label::new(localization::localized_str!("Visit the"))
                    .size(LabelSize::Small)
                    .color(Color::Muted),
            )
            .child(
                ButtonLink::new({
        let __zed_i18n_arg_0 = format!("{}", title);
        localization::format_message(
            "{title} dashboard",
            &[
                ("title", __zed_i18n_arg_0)
            ],
        )
    }, dashboard_url)
                    .no_icon(true)
                    .label_size(LabelSize::Small)
                    .label_color(Color::Muted),
            )
            .child(
                Label::new(localization::localized_str!("to generate an API key."))
                    .size(LabelSize::Small)
                    .color(Color::Muted),
            ),
    });
    // [OHOS PORT END]

    let configured_card_label = if is_from_env_var {
        localization::localized_str!("API Key Set in Environment Variable")
    } else {
        localization::localized_str!("API Key Configured")
    };

    let container = if has_key {
        base_container.child(header).child(
            ConfiguredApiCard::new(format!("{title}-reset-key"), configured_card_label)
                .button_label(localization::localized_str!("Reset Key"))
                .button_tab_index(0)
                .disabled(is_from_env_var)
                .when_some(env_var_name, |this, env_var_name| {
                    this.when(is_from_env_var, |this| {
                        this.tooltip_label({
        let __zed_i18n_arg_0 = format!("{}", env_var_name);
        localization::format_message(
            "To reset your API key, unset the {} environment variable.",
            &[
                ("0", __zed_i18n_arg_0)
            ],
        )
    })
                    })
                })
                .on_click(move |_, _, cx| {
                    write_key(None, cx);
                }),
        )
    } else {
        base_container.child(header).child(
            h_flex()
                .pt_2p5()
                .w_full()
                .min_w_0()
                .justify_between()
                .child(
                    v_flex()
                        .w_full()
                        .min_w_0()
                        .max_w_1_2()
                        .gap_0p5()
                        // `localized_str!` only takes a literal, so the runtime
                        // key goes through the same translation entry point
                        // directly. Falls back to the key when the catalogue has
                        // no entry, which is what the macro does too.
                        .child(Label::new(localization::translate_static(key_label)))
                        .when_some(description, |this, description| this.child(description))
                        // [OHOS PORT BEGIN] The environment-variable hint names
                        // an upstream product, so it is not shown here. To
                        // restore it, delete this block.
                        .when(
                            {
                                #[cfg(target_env = "ohos")]
                                {
                                    false
                                }
                                #[cfg(not(target_env = "ohos"))]
                                {
                                    env_var_name.is_some()
                                }
                            },
                            |this| {
                                this.when_some(env_var_name, |this, env_var_name| {
                                    this.child({
                                        let label = {
        let __zed_i18n_arg_0 = format!("{}", env_var_name.as_ref());
        localization::format_message(
            "Or set the {} env var and restart Zed.",
            &[
                ("0", __zed_i18n_arg_0)
            ],
        )
    };
                                        Label::new(label)
                                            .size(LabelSize::Small)
                                            .color(Color::Muted)
                                    })
                                })
                            },
                        )
                        // [OHOS PORT END]
                )
                .child(
                    SettingsInputField::new(format!("{}-api-key-input", title))
                        .tab_index(0)
                        .with_placeholder("xxxxxxxxxxxxxxxxxxxx")
                        .aria_label({
        let __zed_i18n_arg_0 = format!("{}", title);
        localization::format_message(
            "{} API Key",
            &[
                ("0", __zed_i18n_arg_0)
            ],
        )
    })
                        .on_confirm(move |api_key, _window, cx| {
                            write_key(api_key.filter(|key| !key.is_empty()), cx);
                        }),
                ),
        )
    };

    container.when_some(additional_fields, |this, additional_fields| {
        this.child(
            div()
                .map(|this| if has_key { this.mt_1() } else { this.mt_4() })
                .px_neg_8()
                .border_t_1()
                .border_color(cx.theme().colors().border_variant)
                .child(additional_fields),
        )
    })
}

fn render_ollama_provider(
    settings_window: &SettingsWindow,
    window: &mut Window,
    cx: &mut Context<SettingsWindow>,
) -> impl IntoElement {
    let ollama_settings = ollama_settings();
    let additional_fields = settings_window
        .render_sub_page_items_section(ollama_settings.iter().enumerate(), true, window, cx)
        .into_any_element();

    v_flex()
        .id("ollama")
        .min_w_0()
        .pt_8()
        .gap_1p5()
        .child(
            SettingsSectionHeader::new(localization::localized_str!("Ollama"))
                .icon(IconName::AiOllama)
                .no_padding(true),
        )
        .child(div().px_neg_8().child(additional_fields))
}

fn ollama_settings() -> Box<[SettingsPageItem]> {
    Box::new([
        SettingsPageItem::SettingItem(SettingItem {
            title: localization::localized_str!("API URL"),
            description: localization::localized_str!("The base URL of your Ollama server."),
            field: Box::new(SettingField {
                organization_override: None,
                pick: |settings| {
                    settings
                        .project
                        .all_languages
                        .edit_predictions
                        .as_ref()?
                        .ollama
                        .as_ref()?
                        .api_url
                        .as_ref()
                },
                write: |settings, value, _app: &App| {
                    settings
                        .project
                        .all_languages
                        .edit_predictions
                        .get_or_insert_default()
                        .ollama
                        .get_or_insert_default()
                        .api_url = value;
                },
                json_path: Some("edit_predictions.ollama.api_url"),
            }),
            metadata: Some(Box::new(SettingsFieldMetadata {
                placeholder: Some(OLLAMA_API_URL_PLACEHOLDER),
                ..Default::default()
            })),
            files: USER,
        }),
        SettingsPageItem::SettingItem(SettingItem {
            title: localization::localized_str!("Model"),
            description: localization::localized_str!("The Ollama model to use for edit predictions."),
            field: Box::new(SettingField {
                organization_override: None,
                pick: |settings| {
                    settings
                        .project
                        .all_languages
                        .edit_predictions
                        .as_ref()?
                        .ollama
                        .as_ref()?
                        .model
                        .as_ref()
                },
                write: |settings, value, _app: &App| {
                    settings
                        .project
                        .all_languages
                        .edit_predictions
                        .get_or_insert_default()
                        .ollama
                        .get_or_insert_default()
                        .model = value;
                },
                json_path: Some("edit_predictions.ollama.model"),
            }),
            metadata: Some(Box::new(SettingsFieldMetadata {
                placeholder: Some(OLLAMA_MODEL_PLACEHOLDER),
                ..Default::default()
            })),
            files: USER,
        }),
        SettingsPageItem::SettingItem(SettingItem {
            title: localization::localized_str!("Prompt Format"),
            description: localization::localized_str!("The prompt format to use when requesting predictions. Set to Infer to have the format inferred based on the model name."),
            field: Box::new(SettingField {
                organization_override: None,
                pick: |settings| {
                    settings
                        .project
                        .all_languages
                        .edit_predictions
                        .as_ref()?
                        .ollama
                        .as_ref()?
                        .prompt_format
                        .as_ref()
                },
                write: |settings, value, _app: &App| {
                    settings
                        .project
                        .all_languages
                        .edit_predictions
                        .get_or_insert_default()
                        .ollama
                        .get_or_insert_default()
                        .prompt_format = value;
                },
                json_path: Some("edit_predictions.ollama.prompt_format"),
            }),
            files: USER,
            metadata: None,
        }),
        SettingsPageItem::SettingItem(SettingItem {
            title: localization::localized_str!("Max Output Tokens"),
            description: localization::localized_str!("The maximum number of tokens to generate."),
            field: Box::new(SettingField {
                organization_override: None,
                pick: |settings| {
                    settings
                        .project
                        .all_languages
                        .edit_predictions
                        .as_ref()?
                        .ollama
                        .as_ref()?
                        .max_output_tokens
                        .as_ref()
                },
                write: |settings, value, _app: &App| {
                    settings
                        .project
                        .all_languages
                        .edit_predictions
                        .get_or_insert_default()
                        .ollama
                        .get_or_insert_default()
                        .max_output_tokens = value;
                },
                json_path: Some("edit_predictions.ollama.max_output_tokens"),
            }),
            metadata: None,
            files: USER,
        }),
        SettingsPageItem::SettingItem(SettingItem {
            title: localization::localized_str!("Prediction Debounce"),
            description: localization::localized_str!("Delay in milliseconds before automatically requesting a prediction after typing stops. Set to 0 to request predictions immediately."),
            field: Box::new(SettingField {
                organization_override: None,
                pick: |settings| {
                    settings
                        .project
                        .all_languages
                        .edit_predictions
                        .as_ref()?
                        .ollama
                        .as_ref()?
                        .prediction_debounce
                        .as_ref()
                },
                write: |settings, value, _app: &App| {
                    settings
                        .project
                        .all_languages
                        .edit_predictions
                        .get_or_insert_default()
                        .ollama
                        .get_or_insert_default()
                        .prediction_debounce = value;
                },
                json_path: Some("edit_predictions.ollama.prediction_debounce"),
            }),
            metadata: None,
            files: USER,
        }),
    ])
}

fn open_ai_compatible_settings() -> Box<[SettingsPageItem]> {
    Box::new([
        SettingsPageItem::SettingItem(SettingItem {
            title: localization::localized_str!("API URL"),
            description: localization::localized_str!("The URL of your OpenAI-compatible server's completions API."),
            field: Box::new(SettingField {
                organization_override: None,
                pick: |settings| {
                    settings
                        .project
                        .all_languages
                        .edit_predictions
                        .as_ref()?
                        .open_ai_compatible_api
                        .as_ref()?
                        .api_url
                        .as_ref()
                },
                write: |settings, value, _app: &App| {
                    settings
                        .project
                        .all_languages
                        .edit_predictions
                        .get_or_insert_default()
                        .open_ai_compatible_api
                        .get_or_insert_default()
                        .api_url = value;
                },
                json_path: Some("edit_predictions.open_ai_compatible_api.api_url"),
            }),
            metadata: Some(Box::new(SettingsFieldMetadata {
                placeholder: Some(OPEN_AI_COMPATIBLE_API_URL_PLACEHOLDER),
                ..Default::default()
            })),
            files: USER,
        }),
        SettingsPageItem::SettingItem(SettingItem {
            title: localization::localized_str!("Model"),
            description: localization::localized_str!("The model string to pass to the OpenAI-compatible server."),
            field: Box::new(SettingField {
                organization_override: None,
                pick: |settings| {
                    settings
                        .project
                        .all_languages
                        .edit_predictions
                        .as_ref()?
                        .open_ai_compatible_api
                        .as_ref()?
                        .model
                        .as_ref()
                },
                write: |settings, value, _app: &App| {
                    settings
                        .project
                        .all_languages
                        .edit_predictions
                        .get_or_insert_default()
                        .open_ai_compatible_api
                        .get_or_insert_default()
                        .model = value;
                },
                json_path: Some("edit_predictions.open_ai_compatible_api.model"),
            }),
            metadata: Some(Box::new(SettingsFieldMetadata {
                placeholder: Some(OPEN_AI_COMPATIBLE_MODEL_PLACEHOLDER),
                ..Default::default()
            })),
            files: USER,
        }),
        SettingsPageItem::SettingItem(SettingItem {
            title: localization::localized_str!("Prompt Format"),
            description: localization::localized_str!("The prompt format to use when requesting predictions. Set to Infer to have the format inferred based on the model name."),
            field: Box::new(SettingField {
                organization_override: None,
                pick: |settings| {
                    settings
                        .project
                        .all_languages
                        .edit_predictions
                        .as_ref()?
                        .open_ai_compatible_api
                        .as_ref()?
                        .prompt_format
                        .as_ref()
                },
                write: |settings, value, _app: &App| {
                    settings
                        .project
                        .all_languages
                        .edit_predictions
                        .get_or_insert_default()
                        .open_ai_compatible_api
                        .get_or_insert_default()
                        .prompt_format = value;
                },
                json_path: Some("edit_predictions.open_ai_compatible_api.prompt_format"),
            }),
            files: USER,
            metadata: None,
        }),
        SettingsPageItem::SettingItem(SettingItem {
            title: localization::localized_str!("Max Output Tokens"),
            description: localization::localized_str!("The maximum number of tokens to generate."),
            field: Box::new(SettingField {
                organization_override: None,
                pick: |settings| {
                    settings
                        .project
                        .all_languages
                        .edit_predictions
                        .as_ref()?
                        .open_ai_compatible_api
                        .as_ref()?
                        .max_output_tokens
                        .as_ref()
                },
                write: |settings, value, _app: &App| {
                    settings
                        .project
                        .all_languages
                        .edit_predictions
                        .get_or_insert_default()
                        .open_ai_compatible_api
                        .get_or_insert_default()
                        .max_output_tokens = value;
                },
                json_path: Some("edit_predictions.open_ai_compatible_api.max_output_tokens"),
            }),
            metadata: None,
            files: USER,
        }),
        SettingsPageItem::SettingItem(SettingItem {
            title: localization::localized_str!("Prediction Debounce"),
            description: localization::localized_str!("Delay in milliseconds before automatically requesting a prediction after typing stops. Set to 0 to request predictions immediately."),
            field: Box::new(SettingField {
                organization_override: None,
                pick: |settings| {
                    settings
                        .project
                        .all_languages
                        .edit_predictions
                        .as_ref()?
                        .open_ai_compatible_api
                        .as_ref()?
                        .prediction_debounce
                        .as_ref()
                },
                write: |settings, value, _app: &App| {
                    settings
                        .project
                        .all_languages
                        .edit_predictions
                        .get_or_insert_default()
                        .open_ai_compatible_api
                        .get_or_insert_default()
                        .prediction_debounce = value;
                },
                json_path: Some("edit_predictions.open_ai_compatible_api.prediction_debounce"),
            }),
            metadata: None,
            files: USER,
        }),
    ])
}

fn codestral_settings() -> Box<[SettingsPageItem]> {
    Box::new([
        SettingsPageItem::SettingItem(SettingItem {
            title: localization::localized_str!("API URL"),
            description: localization::localized_str!("The API URL to use for Codestral."),
            field: Box::new(SettingField {
                organization_override: None,
                pick: |settings| {
                    settings
                        .project
                        .all_languages
                        .edit_predictions
                        .as_ref()?
                        .codestral
                        .as_ref()?
                        .api_url
                        .as_ref()
                },
                write: |settings, value, _app: &App| {
                    settings
                        .project
                        .all_languages
                        .edit_predictions
                        .get_or_insert_default()
                        .codestral
                        .get_or_insert_default()
                        .api_url = value;
                },
                json_path: Some("edit_predictions.codestral.api_url"),
            }),
            metadata: Some(Box::new(SettingsFieldMetadata {
                placeholder: Some(CODESTRAL_API_URL),
                ..Default::default()
            })),
            files: USER,
        }),
        SettingsPageItem::SettingItem(SettingItem {
            title: localization::localized_str!("Max Tokens"),
            description: localization::localized_str!("The maximum number of tokens to generate."),
            field: Box::new(SettingField {
                organization_override: None,
                pick: |settings| {
                    settings
                        .project
                        .all_languages
                        .edit_predictions
                        .as_ref()?
                        .codestral
                        .as_ref()?
                        .max_tokens
                        .as_ref()
                },
                write: |settings, value, _app: &App| {
                    settings
                        .project
                        .all_languages
                        .edit_predictions
                        .get_or_insert_default()
                        .codestral
                        .get_or_insert_default()
                        .max_tokens = value;
                },
                json_path: Some("edit_predictions.codestral.max_tokens"),
            }),
            metadata: None,
            files: USER,
        }),
        SettingsPageItem::SettingItem(SettingItem {
            title: localization::localized_str!("Model"),
            description: localization::localized_str!("The Codestral model id to use."),
            field: Box::new(SettingField {
                organization_override: None,
                pick: |settings| {
                    settings
                        .project
                        .all_languages
                        .edit_predictions
                        .as_ref()?
                        .codestral
                        .as_ref()?
                        .model
                        .as_ref()
                },
                write: |settings, value, _app: &App| {
                    settings
                        .project
                        .all_languages
                        .edit_predictions
                        .get_or_insert_default()
                        .codestral
                        .get_or_insert_default()
                        .model = value;
                },
                json_path: Some("edit_predictions.codestral.model"),
            }),
            metadata: Some(Box::new(SettingsFieldMetadata {
                placeholder: Some("codestral-latest"),
                ..Default::default()
            })),
            files: USER,
        }),
        SettingsPageItem::SettingItem(SettingItem {
            title: localization::localized_str!("Prediction Debounce"),
            description: localization::localized_str!("Delay in milliseconds before automatically requesting a prediction after typing stops. Set to 0 to request predictions immediately."),
            field: Box::new(SettingField {
                organization_override: None,
                pick: |settings| {
                    settings
                        .project
                        .all_languages
                        .edit_predictions
                        .as_ref()?
                        .codestral
                        .as_ref()?
                        .prediction_debounce
                        .as_ref()
                },
                write: |settings, value, _app: &App| {
                    settings
                        .project
                        .all_languages
                        .edit_predictions
                        .get_or_insert_default()
                        .codestral
                        .get_or_insert_default()
                        .prediction_debounce = value;
                },
                json_path: Some("edit_predictions.codestral.prediction_debounce"),
            }),
            metadata: None,
            files: USER,
        }),
    ])
}

fn mercury_settings() -> Box<[SettingsPageItem]> {
    Box::new([SettingsPageItem::SettingItem(SettingItem {
        title: localization::localized_str!("Prediction Debounce"),
        description: localization::localized_str!("Delay in milliseconds before automatically requesting a prediction after typing stops. Set to 0 to request predictions immediately."),
        field: Box::new(SettingField {
            organization_override: None,
            pick: |settings| {
                settings
                    .project
                    .all_languages
                    .edit_predictions
                    .as_ref()?
                    .mercury
                    .as_ref()?
                    .prediction_debounce
                    .as_ref()
            },
            write: |settings, value, _app: &App| {
                settings
                    .project
                    .all_languages
                    .edit_predictions
                    .get_or_insert_default()
                    .mercury
                    .get_or_insert_default()
                    .prediction_debounce = value;
            },
            json_path: Some("edit_predictions.mercury.prediction_debounce"),
        }),
        metadata: None,
        files: USER,
    })])
}

fn zed_settings() -> Box<[SettingsPageItem]> {
    Box::new([SettingsPageItem::SettingItem(SettingItem {
        title: localization::localized_str!("Prediction Debounce"),
        description: localization::localized_str!("Delay in milliseconds before automatically requesting a prediction after typing stops. Set to 0 to request predictions immediately."),
        field: Box::new(SettingField {
            organization_override: None,
            pick: |settings| {
                settings
                    .project
                    .all_languages
                    .edit_predictions
                    .as_ref()?
                    .zed
                    .as_ref()?
                    .prediction_debounce
                    .as_ref()
            },
            write: |settings, value, _app: &App| {
                settings
                    .project
                    .all_languages
                    .edit_predictions
                    .get_or_insert_default()
                    .zed
                    .get_or_insert_default()
                    .prediction_debounce = value;
            },
            json_path: Some("edit_predictions.zed.prediction_debounce"),
        }),
        metadata: None,
        files: USER,
    })])
}

fn render_zed_provider(
    settings_window: &SettingsWindow,
    window: &mut Window,
    cx: &mut Context<SettingsWindow>,
) -> impl IntoElement {
    let zed_settings = zed_settings();
    let additional_fields = settings_window
        .render_sub_page_items_section(zed_settings.iter().enumerate(), true, window, cx)
        .into_any_element();

    v_flex()
        .id("zed")
        .min_w_0()
        .pt_8()
        .gap_1p5()
        .child(
            SettingsSectionHeader::new(localization::localized_str!("Zed Predictions"))
                .icon(IconName::ZedPredict)
                .no_padding(true),
        )
        .child(div().px_neg_8().child(additional_fields))
}

fn copilot_settings() -> Box<[SettingsPageItem]> {
    Box::new([SettingsPageItem::SettingItem(SettingItem {
        title: localization::localized_str!("Prediction Debounce"),
        description: localization::localized_str!("Delay in milliseconds before automatically requesting a prediction after typing stops. Set to 0 to request predictions immediately."),
        field: Box::new(SettingField {
            organization_override: None,
            pick: |settings| {
                settings
                    .project
                    .all_languages
                    .edit_predictions
                    .as_ref()?
                    .copilot
                    .as_ref()?
                    .prediction_debounce
                    .as_ref()
            },
            write: |settings, value, _app: &App| {
                settings
                    .project
                    .all_languages
                    .edit_predictions
                    .get_or_insert_default()
                    .copilot
                    .get_or_insert_default()
                    .prediction_debounce = value;
            },
            json_path: Some("edit_predictions.copilot.prediction_debounce"),
        }),
        metadata: None,
        files: USER,
    })])
}

fn render_github_copilot_provider(
    settings_window: &SettingsWindow,
    window: &mut Window,
    cx: &mut Context<SettingsWindow>,
) -> Option<impl IntoElement> {
    let configuration_view = window.use_state(cx, |_, cx| {
        copilot_ui::ConfigurationView::new(
            move |cx| {
                let app_state = AppState::global(cx);
                copilot::GlobalCopilotAuth::try_get_or_init(app_state, cx)
                    .is_some_and(|copilot| copilot.0.read(cx).is_authenticated())
            },
            copilot_ui::ConfigurationMode::EditPrediction,
            cx,
        )
    });

    let additional_fields = settings_window
        .render_sub_page_items_section(copilot_settings().iter().enumerate(), true, window, cx)
        .into_any_element();

    Some(
        v_flex()
            .id("github-copilot")
            .min_w_0()
            .pt_8()
            .gap_1p5()
            .child(
                SettingsSectionHeader::new(localization::localized_str!("GitHub Copilot"))
                    .icon(IconName::Copilot)
                    .no_padding(true),
            )
            .child(configuration_view)
            .child(div().px_neg_8().child(additional_fields)),
    )
}
