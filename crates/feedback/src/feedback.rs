use client::telemetry;
use extension_host::ExtensionStore;
use gpui::{App, ClipboardItem, PromptLevel, actions};
use system_specs::{CopySystemSpecsIntoClipboard, SystemSpecs};
use util::ResultExt;
use workspace::Workspace;
use zed_actions::feedback::{EmailZed, FileBugReport, RequestFeature};

actions!(
    zed,
    [
        /// Opens the Zed repository on GitHub.
        OpenZedRepo,
        /// Copies installed extensions to the clipboard for bug reports.
        CopyInstalledExtensionsIntoClipboard
    ]
);

// [OHOS PORT BEGIN] HiCodeer is not developed in the Zed repository, so bug
// reports and feature requests must not point at the upstream tracker. To
// restore the upstream behaviour, delete these blocks.
#[cfg(target_env = "ohos")]
const HICODEER_REPO_URL: &str = "https://github.com/jaffenqqcom/hicodeer";
#[cfg(target_env = "ohos")]
const REQUEST_FEATURE_URL: &str =
    "https://github.com/jaffenqqcom/hicodeer/discussions/new/choose";
// [OHOS PORT END]
// [OHOS PORT BEGIN] To restore the upstream addresses, delete these definitions.
#[cfg(not(target_env = "ohos"))]
const HICODEER_REPO_URL: &str = "https://github.com/zed-industries/zed";
#[cfg(not(target_env = "ohos"))]
const REQUEST_FEATURE_URL: &str = "https://github.com/zed-industries/zed/discussions/new/choose";
// [OHOS PORT END]

fn file_bug_report_url(specs: &SystemSpecs) -> String {
    // [OHOS PORT BEGIN] HiCodeer is not developed in the Zed repository, so bug
    // reports point at the HiCodeer tracker. The upstream repository's issue
    // template does not exist there, so only the environment is attached. To
    // restore the upstream behaviour, delete this block.
    #[cfg(target_env = "ohos")]
    let url = format!(
        "{HICODEER_REPO_URL}/issues/new?environment={}",
        urlencoding::encode(&specs.to_string())
    );
    // [OHOS PORT END]
    // [OHOS PORT BEGIN] To restore the upstream behaviour, delete this block.
    #[cfg(not(target_env = "ohos"))]
    // [OHOS PORT END]
    #[cfg(not(target_env = "ohos"))]
    let url = format!(
        concat!(
            "https://github.com/zed-industries/zed",
            "/issues/new",
            "?",
            "template=10_bug_report.yml",
            "&",
            "environment={}"
        ),
        urlencoding::encode(&specs.to_string())
    );
    // [OHOS PORT END]
    url
}

fn email_zed_url(specs: &SystemSpecs) -> String {
    // [OHOS PORT BEGIN] HiCodeer has no Zed support mailbox, so the feedback
    // mailto is dropped. To restore the upstream behaviour, delete this block.
    #[cfg(target_env = "ohos")]
    let mail_to = "mailto:";
    // [OHOS PORT END]
    // [OHOS PORT BEGIN] To restore the upstream mailbox, delete this block.
    #[cfg(not(target_env = "ohos"))]
    let mail_to = "mailto:hi@zed.dev";
    // [OHOS PORT END]
    format!("{mail_to}?body={}", email_body(specs))
}

fn email_body(specs: &SystemSpecs) -> String {
    let body = format!("\n\nSystem Information:\n\n{}", specs);
    urlencoding::encode(&body).to_string()
}

pub fn init(cx: &mut App) {
    cx.observe_new(|workspace: &mut Workspace, _, _| {
        workspace
            .register_action(|_, _: &CopySystemSpecsIntoClipboard, window, cx| {
                let specs =
                    SystemSpecs::new(window, cx, telemetry::os_name(), telemetry::os_version());

                cx.spawn_in(window, async move |_, cx| {
                    let specs = specs.await.to_string();

                    cx.update(|_, cx| {
                        cx.write_to_clipboard(ClipboardItem::new_string(specs.clone()))
                    })
                    .log_err();

                    cx.prompt(
                        PromptLevel::Info,
                        localization::localized_str!("Copied into clipboard"),
                        Some(&specs),
                        &[localization::localized_str!("OK")],
                    )
                    .await
                })
                .detach();
            })
            .register_action(|_, _: &CopyInstalledExtensionsIntoClipboard, window, cx| {
                let clipboard_text = format_installed_extensions_for_clipboard(cx);
                cx.write_to_clipboard(ClipboardItem::new_string(clipboard_text.clone()));
                drop(window.prompt(
                    PromptLevel::Info,
                    localization::localized_str!("Copied into clipboard"),
                    Some(&clipboard_text),
                    &[localization::localized_str!("OK")],
                    cx,
                ));
            })
            .register_action(|_, _: &RequestFeature, _, cx| {
                cx.open_url(REQUEST_FEATURE_URL);
            })
            .register_action(move |_, _: &FileBugReport, window, cx| {
                let specs =
                    SystemSpecs::new(window, cx, telemetry::os_name(), telemetry::os_version());
                cx.spawn_in(window, async move |_, cx| {
                    let specs = specs.await;
                    cx.update(|_, cx| {
                        cx.open_url(&file_bug_report_url(&specs));
                    })
                    .log_err();
                })
                .detach();
            })
            .register_action(move |_, _: &EmailZed, window, cx| {
                let specs =
                    SystemSpecs::new(window, cx, telemetry::os_name(), telemetry::os_version());
                cx.spawn_in(window, async move |_, cx| {
                    let specs = specs.await;
                    cx.update(|_, cx| {
                        cx.open_url(&email_zed_url(&specs));
                    })
                    .log_err();
                })
                .detach();
            })
            .register_action(move |_, _: &OpenZedRepo, _, cx| {
                cx.open_url(HICODEER_REPO_URL);
            });
    })
    .detach();
}

fn format_installed_extensions_for_clipboard(cx: &mut App) -> String {
    let store = ExtensionStore::global(cx);
    let store = store.read(cx);
    let mut lines = Vec::with_capacity(store.extension_index.extensions.len());

    for (extension_id, entry) in store.extension_index.extensions.iter() {
        let line = format!(
            "- {} ({}) v{}{}",
            entry.manifest.name,
            extension_id,
            entry.manifest.version,
            if entry.dev { " (dev)" } else { "" }
        );
        lines.push(line);
    }

    lines.sort();

    if lines.is_empty() {
        return "No extensions installed.".to_string();
    }

    format!(
        "Installed extensions ({}):\n{}",
        lines.len(),
        lines.join("\n")
    )
}
