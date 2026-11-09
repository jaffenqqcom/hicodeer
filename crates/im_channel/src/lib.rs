//! Bridging one bound IM chat to its host.
//!
//! The crate owns the platforms, the stored channel configuration, the bridge
//! thread and the settings-facing manager. What a message means is not its
//! concern: text goes in and out through [`transport`], and the host decides
//! who should see it.

pub mod binding;
pub mod config;
pub mod provider;

#[cfg(target_env = "ohos")]
pub mod bridge;
#[cfg(target_env = "ohos")]
pub mod dingtalk;
#[cfg(target_env = "ohos")]
pub mod feishu;
#[cfg(target_env = "ohos")]
pub mod http;
#[cfg(target_env = "ohos")]
pub mod manager;
#[cfg(target_env = "ohos")]
pub mod platforms;
#[cfg(target_env = "ohos")]
pub mod qq;
#[cfg(target_env = "ohos")]
pub mod qr;
#[cfg(target_env = "ohos")]
pub mod registration;
#[cfg(target_env = "ohos")]
pub mod store;
#[cfg(target_env = "ohos")]
pub mod transport;
#[cfg(target_env = "ohos")]
pub mod ws;
