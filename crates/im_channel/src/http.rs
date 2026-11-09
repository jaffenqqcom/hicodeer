//! Small HTTP helpers shared by the platform providers.
//!
//! Every platform talks JSON over HTTPS: one call to fetch an access token,
//! then one call to send a message. Keeping the shape here means a provider
//! only carries its own URLs and field names.

use futures::AsyncReadExt as _;
use http_client::{AsyncBody, HttpClient, Method, Request};

/// Posts `payload` as JSON and returns the decoded response body.
///
/// A non-success status is an error carrying the response text, because every
/// platform reports its real failure reason in the body rather than the status.
pub(crate) async fn post_json(
    client: &dyn HttpClient,
    uri: &str,
    headers: Vec<(String, String)>,
    payload: &serde_json::Value,
) -> anyhow::Result<serde_json::Value> {
    let body = serde_json::to_vec(payload)?;
    let mut builder = Request::builder()
        .method(Method::POST)
        .uri(uri)
        .header("Content-Type", "application/json");
    for (name, value) in headers {
        builder = builder.header(name, value);
    }
    let request = builder.body(AsyncBody::from(body))?;
    let mut response = client.send(request).await?;
    let status = response.status();
    let mut text = String::new();
    response.body_mut().read_to_string(&mut text).await?;
    if !status.is_success() {
        anyhow::bail!("request to {uri} failed with {status}: {text}");
    }
    serde_json::from_str(&text)
        .map_err(|error| anyhow::anyhow!("response from {uri} is not JSON: {error}: {text}"))
}

/// Performs a GET and returns the decoded response body.
pub(crate) async fn get_json(
    client: &dyn HttpClient,
    uri: &str,
    headers: Vec<(String, String)>,
) -> anyhow::Result<serde_json::Value> {
    let mut builder = Request::builder().method(Method::GET).uri(uri);
    for (name, value) in headers {
        builder = builder.header(name, value);
    }
    let request = builder.body(AsyncBody::empty())?;
    let mut response = client.send(request).await?;
    let status = response.status();
    let mut text = String::new();
    response.body_mut().read_to_string(&mut text).await?;
    if !status.is_success() {
        anyhow::bail!("request to {uri} failed with {status}: {text}");
    }
    serde_json::from_str(&text)
        .map_err(|error| anyhow::anyhow!("response from {uri} is not JSON: {error}: {text}"))
}

/// Reads a string field, reporting the whole body when it is missing.
pub(crate) fn string_field<'a>(
    response: &'a serde_json::Value,
    field: &str,
) -> anyhow::Result<&'a str> {
    response
        .get(field)
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("response has no string field `{field}`: {response}"))
}
