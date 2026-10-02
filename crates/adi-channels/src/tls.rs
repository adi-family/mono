//! `wss://` for the router socket — docs/channels.md's deployed router is https-only, so this is
//! required before the ADI-MONO-124 go-live, not merely available (`ws.rs`'s own crate-level note
//! flagged the gap this closes). Plain `ws://` (`wrangler dev`, and every one of this crate's own
//! tests outside this file) never touches this module at all — see [`wants_tls`].
//!
//! Trust anchors come from a compiled-in Mozilla root set (`webpki-roots`), not the platform's own
//! store — unlike `reqwest`'s choice of `rustls-platform-verifier` (right for a browser-facing
//! OAuth redirect, which wants whatever the OS already trusts), this socket dials one name we
//! control and gains nothing from the OS store's extra surface (platform APIs, JNI on Android).

use std::sync::{Arc, OnceLock};

use rustls::pki_types::ServerName;
use rustls::{ClientConfig, RootCertStore};
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;
use tokio_rustls::client::TlsStream;

use crate::error::{Error, Result};

/// Install `ring` as the process-wide rustls provider, once — the same call `router_api.rs`/
/// `node_api.rs` make before building their own client. Idempotent, and there is no start-up here
/// to rely on instead: a reconnect loop may build its first socket from any task.
fn ensure_provider() {
    rustls::crypto::ring::default_provider()
        .install_default()
        .ok();
}

/// Whether `url`'s scheme wants TLS — `wss://`/`https://` do, `ws://`/`http://` (the default, and
/// what every other test in this crate dials) don't.
pub(crate) fn wants_tls(url: &str) -> bool {
    let scheme = url.split_once("://").map_or("", |(s, _)| s);
    scheme.eq_ignore_ascii_case("wss") || scheme.eq_ignore_ascii_case("https")
}

fn default_config() -> Arc<ClientConfig> {
    static CONFIG: OnceLock<Arc<ClientConfig>> = OnceLock::new();
    CONFIG
        .get_or_init(|| {
            ensure_provider();
            let roots = RootCertStore {
                roots: webpki_roots::TLS_SERVER_ROOTS.to_vec(),
            };
            Arc::new(
                ClientConfig::builder()
                    .with_root_certificates(roots)
                    .with_no_client_auth(),
            )
        })
        .clone()
}

/// Wrap an already-connected TCP socket in TLS, verifying it under `host`. The one call
/// [`crate::client::RouterClient`] makes once [`wants_tls`] says the router's own URL needs it.
///
/// # Errors
/// [`Error::Protocol`] if `host` isn't a valid DNS name/IP for [`ServerName`], or the handshake —
/// including certificate verification — fails.
pub(crate) async fn connect(stream: TcpStream, host: &str) -> Result<TlsStream<TcpStream>> {
    connect_with(default_config(), stream, host).await
}

/// The testable seam: a test builds its own [`ClientConfig`] trusting a throwaway root, so this
/// runs a real rustls handshake rather than a connect-succeeds smoke test.
async fn connect_with(
    config: Arc<ClientConfig>,
    stream: TcpStream,
    host: &str,
) -> Result<TlsStream<TcpStream>> {
    let name = ServerName::try_from(host.to_string())
        .map_err(|_| Error::Protocol(format!("{host} is not a valid TLS server name")))?;
    Ok(TlsConnector::from(config).connect(name, stream).await?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rcgen::{CertifiedKey, generate_simple_self_signed};
    use rustls::ServerConfig;
    use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
    use tokio::net::TcpListener;
    use tokio_rustls::TlsAcceptor;

    #[test]
    fn wants_tls_reads_the_scheme_not_the_rest_of_the_url() {
        assert!(wants_tls("wss://hooks.withadi.dev/subscribe"));
        assert!(wants_tls("https://hooks.withadi.dev"));
        assert!(!wants_tls("ws://127.0.0.1:8787"));
        assert!(!wants_tls("http://localhost:8787/subscribe"));
    }

    fn self_signed(host: &str) -> (CertificateDer<'static>, PrivateKeyDer<'static>) {
        let CertifiedKey { cert, signing_key } =
            generate_simple_self_signed(vec![host.to_string()]).unwrap();
        let key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(signing_key.serialize_der()));
        (cert.der().clone(), key)
    }

    /// A real rustls handshake end to end: the client trusts the server's self-signed leaf as its
    /// sole root, connects, and plaintext actually flows both ways once the handshake settles —
    /// proving this is a working TLS client, not a type that merely compiles against one.
    #[tokio::test]
    async fn connect_succeeds_against_a_server_whose_certificate_is_trusted() {
        ensure_provider();
        let (cert, key) = self_signed("example.adi.test");
        let server_config = Arc::new(
            ServerConfig::builder()
                .with_no_client_auth()
                .with_single_cert(vec![cert.clone()], key)
                .unwrap(),
        );
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (sock, _) = listener.accept().await.unwrap();
            let mut tls = TlsAcceptor::from(server_config).accept(sock).await.unwrap();
            let mut buf = [0u8; 5];
            tls.read_exact(&mut buf).await.unwrap();
            assert_eq!(&buf, b"hello");
            tls.write_all(b"world").await.unwrap();
        });

        let mut roots = RootCertStore::empty();
        roots.add(cert).unwrap();
        let client_config = Arc::new(
            ClientConfig::builder()
                .with_root_certificates(roots)
                .with_no_client_auth(),
        );

        let tcp = TcpStream::connect(addr).await.unwrap();
        let mut tls = connect_with(client_config, tcp, "example.adi.test")
            .await
            .expect("handshake against a trusted root");
        tls.write_all(b"hello").await.unwrap();
        let mut buf = [0u8; 5];
        tls.read_exact(&mut buf).await.unwrap();
        assert_eq!(&buf, b"world");
        server.await.unwrap();
    }

    /// The control for the test above: the same server, a client that trusts a *different*
    /// self-signed root instead. Without this, "the handshake above succeeded" could mean
    /// verification is a no-op just as easily as it meaning verification actually passed.
    #[tokio::test]
    async fn connect_fails_against_an_untrusted_certificate() {
        ensure_provider();
        let (server_cert, server_key) = self_signed("example.adi.test");
        let server_config = Arc::new(
            ServerConfig::builder()
                .with_no_client_auth()
                .with_single_cert(vec![server_cert], server_key)
                .unwrap(),
        );
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (sock, _) = listener.accept().await.unwrap();
            // The client is expected to abort mid-handshake, so an error (or a dropped socket)
            // here is the success case for this test, not a panic.
            let _ = TlsAcceptor::from(server_config).accept(sock).await;
        });

        let (other_cert, _unused_key) = self_signed("example.adi.test");
        let mut roots = RootCertStore::empty();
        roots.add(other_cert).unwrap();
        let client_config = Arc::new(
            ClientConfig::builder()
                .with_root_certificates(roots)
                .with_no_client_auth(),
        );

        let tcp = TcpStream::connect(addr).await.unwrap();
        let err = connect_with(client_config, tcp, "example.adi.test")
            .await
            .expect_err("a certificate signed by an untrusted root must be refused");
        assert!(matches!(err, Error::Protocol(_)));
        let _ = server.await;
    }
}
