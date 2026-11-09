//! Registration table for the IM platforms.
//!
//! Everything the bridge and the settings page need to know about a platform is
//! in one [`PlatformSpec`]: the labels it shows, the credential fields it asks
//! for, how it is bound, and how its provider is built. Adding a platform means
//! adding its module, its enum variant, and one entry in [`platforms`] — no new
//! match arms in the bridge, the manager or the settings page.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use futures::future::BoxFuture;
use http_client::HttpClient;

use crate::binding::{
    ConversationKind, DingTalkCredentials, FeishuCredentials, Platform, QqCredentials,
};
use crate::dingtalk::DingTalkProvider;
use crate::feishu::FeishuProvider;
use crate::provider::InboundProvider;
use crate::qq::QqProvider;
use crate::registration::{self, DeviceFlow, PollOutcome};

/// Credential fields keyed by name, as stored.
pub type CredentialValues = BTreeMap<String, String>;

/// One field the settings page asks for. The names are the keys the stored
/// binding uses, so a spec and its provider always agree.
pub struct CredentialField {
    pub key: &'static str,
    pub label: &'static str,
    pub description: &'static str,
    pub placeholder: &'static str,
    /// Whether the provider can derive the value, so leaving it empty still
    /// lets the channel connect.
    pub optional: bool,
}

impl CredentialField {
    const fn new(
        key: &'static str,
        label: &'static str,
        description: &'static str,
        placeholder: &'static str,
    ) -> Self {
        Self {
            key,
            label,
            description,
            placeholder,
            optional: false,
        }
    }

    /// A field the provider fills in itself when it is left empty.
    const fn new_optional(
        key: &'static str,
        label: &'static str,
        description: &'static str,
        placeholder: &'static str,
    ) -> Self {
        Self {
            key,
            label,
            description,
            placeholder,
            optional: true,
        }
    }
}

/// How a platform is bound.
pub enum BindingRoute {
    /// The platform's device flow returns the credentials after a QR scan.
    DeviceFlow(ScanFlow),
    /// No device flow: the page opens this URL and the user copies the
    /// credentials from the page it lands on.
    ExternalPage(&'static str),
}

/// The two calls a device flow needs. Boxed so a spec can hold them while the
/// registration module keeps writing plain async functions.
pub struct ScanFlow {
    pub begin: fn(Arc<dyn HttpClient>) -> BoxFuture<'static, anyhow::Result<DeviceFlow>>,
    pub poll:
        fn(Arc<dyn HttpClient>, String) -> BoxFuture<'static, anyhow::Result<PollOutcome>>,
}

/// Everything platform-specific, in one place.
pub struct PlatformSpec {
    pub platform: Platform,
    pub display_name: &'static str,
    pub route: BindingRoute,
    pub fields: &'static [CredentialField],
    build: fn(
        Arc<dyn HttpClient>,
        &CredentialValues,
        Option<String>,
        ConversationKind,
        Arc<AtomicBool>,
    ) -> Arc<dyn InboundProvider>,
}

impl PlatformSpec {
    /// The provider for this platform, built from the stored credentials.
    pub fn make_provider(
        &self,
        http: Arc<dyn HttpClient>,
        values: &CredentialValues,
        conversation: Option<String>,
        kind: ConversationKind,
        connection: Arc<AtomicBool>,
    ) -> Arc<dyn InboundProvider> {
        (self.build)(http, values, conversation, kind, connection)
    }

    /// True when the platform can bind by scanning a QR code.
    pub fn is_device_flow(&self) -> bool {
        matches!(self.route, BindingRoute::DeviceFlow(_))
    }

    /// The device flow, when the platform has one.
    pub fn scan(&self) -> Option<&ScanFlow> {
        match &self.route {
            BindingRoute::DeviceFlow(flow) => Some(flow),
            BindingRoute::ExternalPage(_) => None,
        }
    }
}

/// The platforms the build ships, in the order the settings table lists them.
pub fn platforms() -> &'static [PlatformSpec] {
    &PLATFORMS
}

/// The spec for one platform.
pub fn spec(platform: Platform) -> &'static PlatformSpec {
    PLATFORMS
        .iter()
        .find(|spec| spec.platform == platform)
        .expect("every platform has a spec")
}

static PLATFORMS: [PlatformSpec; 3] = [
    PlatformSpec {
        platform: Platform::Feishu,
        display_name: "Feishu",
        route: BindingRoute::DeviceFlow(ScanFlow {
            begin: begin_feishu_scan,
            poll: poll_feishu_scan,
        }),
        fields: &FEISHU_FIELDS,
        build: build_feishu,
    },
    PlatformSpec {
        platform: Platform::DingTalk,
        display_name: "DingTalk",
        route: BindingRoute::DeviceFlow(ScanFlow {
            begin: begin_dingtalk_scan,
            poll: poll_dingtalk_scan,
        }),
        fields: &DINGTALK_FIELDS,
        build: build_dingtalk,
    },
    PlatformSpec {
        platform: Platform::Qq,
        display_name: "QQ",
        route: BindingRoute::ExternalPage(crate::registration::QQ_CREATE_URL),
        fields: &QQ_FIELDS,
        build: build_qq,
    },
];

static FEISHU_FIELDS: [CredentialField; 2] = [
    CredentialField::new(
        "app_id",
        "App ID",
        "Open platform app identifier",
        "cli_xxx",
    ),
    CredentialField::new(
        "app_secret",
        "App Secret",
        "Open platform app secret",
        "App Secret",
    ),
];

static DINGTALK_FIELDS: [CredentialField; 3] = [
    CredentialField::new(
        "client_id",
        "Client ID",
        "App key from the DingTalk console",
        "Client ID",
    ),
    CredentialField::new(
        "client_secret",
        "Client Secret",
        "App secret from the DingTalk console",
        "Client Secret",
    ),
    CredentialField::new_optional(
        "robot_code",
        "Robot Code",
        "Defaults to the Client ID, which is the robot code for this app",
        "Robot Code",
    ),
];

static QQ_FIELDS: [CredentialField; 2] = [
    CredentialField::new("app_id", "AppID", "Bot AppID from q.qq.com", "AppID"),
    CredentialField::new(
        "app_secret",
        "AppSecret",
        "Bot AppSecret from q.qq.com",
        "AppSecret",
    ),
];

fn begin_feishu_scan(http: Arc<dyn HttpClient>) -> BoxFuture<'static, anyhow::Result<DeviceFlow>> {
    Box::pin(async move { registration::begin_feishu(http.as_ref()).await })
}

fn poll_feishu_scan(
    http: Arc<dyn HttpClient>,
    device_code: String,
) -> BoxFuture<'static, anyhow::Result<PollOutcome>> {
    Box::pin(async move { registration::poll_feishu(http.as_ref(), &device_code).await })
}

fn begin_dingtalk_scan(
    http: Arc<dyn HttpClient>,
) -> BoxFuture<'static, anyhow::Result<DeviceFlow>> {
    Box::pin(async move { registration::begin_dingtalk(http.as_ref()).await })
}

fn poll_dingtalk_scan(
    http: Arc<dyn HttpClient>,
    device_code: String,
) -> BoxFuture<'static, anyhow::Result<PollOutcome>> {
    Box::pin(async move { registration::poll_dingtalk(http.as_ref(), &device_code).await })
}

fn value(values: &CredentialValues, key: &str) -> String {
    values.get(key).cloned().unwrap_or_default()
}

fn build_feishu(
    http: Arc<dyn HttpClient>,
    values: &CredentialValues,
    chat_id: Option<String>,
    _kind: ConversationKind,
    connection: Arc<AtomicBool>,
) -> Arc<dyn InboundProvider> {
    Arc::new(FeishuProvider::new(
        http,
        FeishuCredentials {
            app_id: value(values, "app_id"),
            app_secret: value(values, "app_secret"),
        },
        chat_id,
        connection,
    ))
}

fn build_dingtalk(
    http: Arc<dyn HttpClient>,
    values: &CredentialValues,
    conversation_id: Option<String>,
    _kind: ConversationKind,
    connection: Arc<AtomicBool>,
) -> Arc<dyn InboundProvider> {
    // A robot app addresses itself by its app key, which the registration
    // flow returns as the client id; the robot code only needs an explicit
    // value when the app's robot differs from that key.
    let robot_code = match value(values, "robot_code") {
        explicit if !explicit.is_empty() => explicit,
        _ => value(values, "client_id"),
    };
    Arc::new(DingTalkProvider::new(
        http,
        DingTalkCredentials {
            client_id: value(values, "client_id"),
            client_secret: value(values, "client_secret"),
            robot_code,
        },
        conversation_id,
        connection,
    ))
}

fn build_qq(
    http: Arc<dyn HttpClient>,
    values: &CredentialValues,
    conversation_id: Option<String>,
    _kind: ConversationKind,
    connection: Arc<AtomicBool>,
) -> Arc<dyn InboundProvider> {
    Arc::new(QqProvider::new(
        http,
        QqCredentials {
            app_id: value(values, "app_id"),
            app_secret: value(values, "app_secret"),
        },
        conversation_id,
        connection,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every platform in the enum has exactly one spec, and the table covers
    /// all of them.
    #[test]
    fn every_platform_has_one_spec() {
        assert_eq!(platforms().len(), Platform::ALL.len());
        for platform in Platform::ALL {
            assert_eq!(spec(platform).platform, platform);
        }
    }

    /// A spec's fields are the keys its provider reads, so a binding made from
    /// the settings page always has everything the provider needs.
    #[test]
    fn specs_declare_their_credential_fields() {
        assert_eq!(
            spec(Platform::Feishu)
                .fields
                .iter()
                .map(|field| field.key)
                .collect::<Vec<_>>(),
            ["app_id", "app_secret"]
        );
        assert_eq!(spec(Platform::DingTalk).fields.len(), 3);
        assert!(!spec(Platform::Qq).is_device_flow());
        assert!(spec(Platform::Feishu).is_device_flow());
    }
}
