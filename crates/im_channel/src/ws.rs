//! The WebSocket connection all three platforms use for inbound messages.
//!
//! Feishu, DingTalk and QQ all deliver events over an outbound WebSocket; none
//! of them can reach a device that has no public address. The TLS setup is the
//! same one the collab client uses, so certificate roots come from the system.

use std::sync::Arc;

use anyhow::Context as _;
use async_tungstenite::WebSocketStream;
use async_tungstenite::tungstenite::client::IntoClientRequest;
use async_tungstenite::tungstenite::http::{HeaderName, HeaderValue};
use tokio::net::TcpStream;

/// A connected socket, already past the TLS handshake.
pub(crate) type ImSocket = WebSocketStream<async_tungstenite::tokio::ConnectStream>;

/// Opens `url` with the given extra request headers.
pub(crate) async fn connect(url: &str, headers: &[(String, String)]) -> anyhow::Result<ImSocket> {
    let parsed =
        http_client::Url::parse(url).with_context(|| format!("invalid websocket url {url}"))?;
    let host = parsed
        .host_str()
        .context("websocket url has no host")?
        .to_string();
    let port = parsed
        .port_or_known_default()
        .context("websocket url has no port")?;
    let tcp = TcpStream::connect((host.as_str(), port)).await?;
    let mut request = url.into_client_request()?;
    for (name, value) in headers {
        request.headers_mut().insert(
            HeaderName::from_bytes(name.as_bytes())?,
            HeaderValue::from_str(value)?,
        );
    }
    let (socket, _) = async_tungstenite::tokio::client_async_tls_with_connector_and_config(
        request,
        tcp,
        Some(Arc::new(http_client_tls::tls_config()).into()),
        None,
    )
    .await?;
    log::info!(
        "im channel: websocket connected to {}",
        parsed.host_str().unwrap_or("")
    );
    Ok(socket)
}
