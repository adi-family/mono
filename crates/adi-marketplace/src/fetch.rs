//! The one fetch: GET an HTTPS URL, or read a `file:///` path off disk — bounded in time and size
//! either way, because a source is one of those two shapes (`sources::add`) and both stand in for
//! the same act, "get me the bytes this source currently serves."
//!
//! Blocking reqwest over rustls, exactly as `adi-facts`' ollama client is, so the crate stays
//! synchronous and an async host reaches it through its blocking pool. No tokio here.

/// How long one fetch may take, connect and body combined. A manifest or bundle that cannot
/// arrive in half a minute is not arriving.
const TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// The most bytes one fetch may return. Manifests are kilobytes and bundles are base64 of a few
/// MiB, so this bounds a misbehaving host without touching a real artifact. Applied to a
/// `file://` read too, for the same reason — a local manifest is not exempt from the bound just
/// because nothing had to answer for it over the network.
const MAX_BYTES: usize = 16 * 1024 * 1024;

/// GET `url` and return its body: `https://` over the network, or `file:///` straight off disk.
///
/// # Errors
/// A one-line reason for every way this can fail: a client that cannot be built, a transport
/// error, a non-success status, a body past [`MAX_BYTES`], or — for `file://` — the read itself
/// failing (not found, permissions), which is a source's local counterpart to an unreachable
/// host and degrades to the stale cache exactly the same way (`sync::sync_one`).
pub fn get(url: &str) -> std::result::Result<Vec<u8>, String> {
    match url.strip_prefix("file://") {
        Some(path) => get_file(path),
        None => get_https(url),
    }
}

/// Read a `file:///` source's bytes off disk. `sources::add` only ever admits an absolute path
/// under this scheme, so `path` here already starts with `/`.
fn get_file(path: &str) -> std::result::Result<Vec<u8>, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("file://{path}: {e}"))?;
    if bytes.len() > MAX_BYTES {
        return Err(format!(
            "file://{path}: {} bytes is past the {} byte fetch limit",
            bytes.len(),
            MAX_BYTES
        ));
    }
    Ok(bytes)
}

/// GET `url` over HTTPS and return its body.
fn get_https(url: &str) -> std::result::Result<Vec<u8>, String> {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_file_url_reads_the_path_after_the_third_slash() {
        let dir = std::env::temp_dir().join(format!(
            "adi-marketplace-fetch-file-{}-{:?}",
            std::process::id(),
            std::thread::current().id(),
        ));
        std::fs::create_dir_all(&dir).expect("scratch dir");
        let path = dir.join("manifest.json");
        std::fs::write(&path, b"{}").expect("write");

        let url = format!("file://{}", path.display());
        assert_eq!(get(&url).expect("read"), b"{}");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_deleted_file_is_a_failed_fetch_not_a_panic() {
        let missing = std::env::temp_dir().join("adi-marketplace-fetch-file-does-not-exist.json");
        let url = format!("file://{}", missing.display());
        assert!(get(&url).is_err());
    }
}
