//! The one HTTP call: GET an HTTPS URL, bounded in time and size.
//!
//! Blocking reqwest over rustls, exactly as `adi-facts`' ollama client is, so the crate stays
//! synchronous and an async host reaches it through its blocking pool. No tokio here.

/// How long one fetch may take, connect and body combined. A manifest or bundle that cannot
/// arrive in half a minute is not arriving.
const TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// The most bytes one fetch may return. Manifests are kilobytes and bundles are base64 of a few
/// MiB, so this bounds a misbehaving host without touching a real artifact.
const MAX_BYTES: usize = 16 * 1024 * 1024;

/// GET `url` over HTTPS and return its body.
///
/// # Errors
/// A one-line reason for every way this can fail: a client that cannot be built, a transport
/// error, a non-success status, or a body past [`MAX_BYTES`].
pub fn get(url: &str) -> std::result::Result<Vec<u8>, String> {
    // reqwest 0.13 is taken workspace-wide with `rustls-no-provider`, so nobody installs a
    // crypto provider for us and `Client::build` fails until somebody does. Every client site
    // in this tree opens with this line; it is idempotent — a second install returns `Err`,
    // which is why the result is dropped.
    rustls::crypto::ring::default_provider()
        .install_default()
        .ok();
    let client = reqwest::blocking::Client::builder()
        .timeout(TIMEOUT)
        .build()
        .map_err(|e| format!("building the http client: {e}"))?;
    let response = client.get(url).send().map_err(|e| format!("{url}: {e}"))?;
    let status = response.status();
    if !status.is_success() {
        let text = response.text().unwrap_or_default();
        let text = text.trim();
        return Err(if text.is_empty() {
            format!("{url}: {status}")
        } else {
            format!(
                "{url}: {status}: {}",
                text.chars().take(200).collect::<String>()
            )
        });
    }
    let bytes = response
        .bytes()
        .map_err(|e| format!("{url}: reading the body: {e}"))?;
    if bytes.len() > MAX_BYTES {
        return Err(format!(
            "{url}: {} bytes is past the {} byte fetch limit",
            bytes.len(),
            MAX_BYTES
        ));
    }
    Ok(bytes.to_vec())
}
