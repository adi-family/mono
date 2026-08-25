//! HTTPS for the front door: a locally-trusted certificate, minted and renewed in-process.
//!
//! **Why this exists.** A service worker — and therefore installing the control panel as an app —
//! requires a *secure context*, and `http://app.adi` is not one. Loopback earns that exemption only
//! under the literal name `localhost`, never a hostname that merely resolves there. So the front
//! door terminates TLS with a certificate the user trusts once, and `https://app.adi` becomes a
//! first-class origin.
//!
//! **Two pairs of files** under `<config dir>/tls/`:
//!
//! * `ca.pem` / `ca-key.pem` — the certificate authority. Generated once and then left alone: it is
//!   what the system trust store pins, so re-generating it would silently break every browser on the
//!   machine until the user re-trusted the new one.
//! * `cert.pem` / `key.pem` — the leaf, carrying every proxied host as a SAN. Re-minted whenever
//!   that host set changes (a new `.adi` service appeared) or it nears expiry. Cheap and safe to
//!   redo precisely *because* the CA above stays put — the trust anchor never moves.
//!
//! Nothing here shells out: no `openssl`, no `mkcert`. A fresh machine gets working TLS from the
//! binary alone, and the only manual step left is trusting `ca.pem` once.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Context as _;
use rcgen::{
    BasicConstraints, CertificateParams, CertifiedIssuer, DnType, ExtendedKeyUsagePurpose, IsCa,
    KeyPair, KeyUsagePurpose,
};
use rustls::ServerConfig;
use rustls::pki_types::pem::PemObject as _;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use serde::{Deserialize, Serialize};
use time::{Duration as TimeDuration, OffsetDateTime};
use tracing::{info, warn};

/// How long a freshly minted leaf is valid. Kept close to the public-CA maximum (397 days) and
/// well inside Apple's 825-day ceiling — a longer-lived leaf is rejected outright by the platform
/// verifier that Chrome and Safari both use on macOS.
const LEAF_DAYS: i64 = 365;

/// Re-mint once the leaf has less than this left, so it never expires under a running daemon.
const RENEW_WITHIN_DAYS: i64 = 30;

/// The CA outlives many leaves; re-trusting is the one step that costs the user a password, so it
/// should be rare. Roots are exempt from the leaf lifetime limits above.
const CA_DAYS: i64 = 3650;

/// Hosts every leaf carries regardless of config, so the front door is reachable by name and by
/// address over TLS even before any service is routed.
const BASE_SANS: [&str; 3] = ["localhost", "127.0.0.1", "127.0.0.53"];

/// The mesh zone's own wildcard, added whenever a mesh gateway is configured. It covers the
/// three-label `<node>.n.adi` — the node itself — and **not** the four-label service names below;
/// see [`mesh_sans`] for why those need one SAN apiece.
const MESH_ZONE_SAN: &str = "*.n.adi";

/// What the leaf on disk was minted for, so we can tell whether it still fits the config without
/// parsing X.509. Written beside it as `cert.json`.
#[derive(Debug, Serialize, Deserialize)]
struct LeafMeta {
    /// The SAN list, sorted — compared against what the current config wants.
    hosts: Vec<String>,
    /// Unix seconds; drives renewal without an X.509 parser in the dependency tree.
    issued_unix: i64,
}

/// A ready TLS front door.
#[derive(Debug)]
pub struct Tls {
    pub config: Arc<ServerConfig>,
    /// Where the CA lives — surfaced in the log line that tells the user what to trust.
    pub ca_path: PathBuf,
    /// True when this run generated the CA, i.e. nothing can trust it yet.
    pub ca_is_new: bool,
}

/// Load the TLS identity for `hosts`, generating or renewing whatever is missing or stale.
///
/// `mesh` is `Some(petnames)` when a mesh gateway is configured — the leaf then also covers the
/// reserved `n.adi` zone (see [`san_list`]) — and `None` when this machine has no mesh at all.
///
/// # Errors
/// Fails if `dir` can't be created, a key is unreadable (a root-owned key when running
/// unprivileged, say), or certificate generation fails. Callers treat that as "no HTTPS" rather
/// than fatal — the plain-HTTP front door should survive a broken cert.
pub fn prepare(dir: &Path, hosts: &[String], mesh: Option<&[String]>) -> anyhow::Result<Tls> {
    std::fs::create_dir_all(dir)
        .with_context(|| format!("creating TLS directory {}", dir.display()))?;

    let (ca, ca_is_new) = load_or_create_ca(dir)?;
    let wanted = san_list(hosts, mesh);
    let (chain, key) = load_or_create_leaf(dir, &wanted, &ca)?;

    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let mut config = ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .context("selecting TLS protocol versions")?
        .with_no_client_auth()
        .with_single_cert(chain, key)
        .context("installing the leaf certificate")?;
    // The proxy speaks HTTP/1.1 only — it parses a request head and splices bytes. Advertising just
    // h1 keeps a browser from negotiating h2 over ALPN and framing everything in a way the proxy
    // would forward as garbage.
    config.alpn_protocols = vec![b"http/1.1".to_vec()];

    Ok(Tls {
        config: Arc::new(config),
        ca_path: dir.join("ca.pem"),
        ca_is_new,
    })
}

/// Every SAN the leaf should carry: the configured hosts plus [`BASE_SANS`], plus the mesh names
/// from [`mesh_sans`] when a gateway is configured. Deduped and sorted so the comparison against
/// `cert.json` is order-insensitive.
fn san_list(hosts: &[String], mesh: Option<&[String]>) -> Vec<String> {
    let mut all: Vec<String> = BASE_SANS
        .iter()
        .map(|s| (*s).to_string())
        .chain(hosts.iter().map(|h| h.trim().to_ascii_lowercase()))
        .chain(mesh.map(mesh_sans).unwrap_or_default())
        .filter(|h| !h.is_empty())
        .collect();
    all.sort();
    all.dedup();
    all
}

/// The SANs that cover the reserved mesh zone for the nodes this machine has paired with.
///
/// **Why one SAN per node.** A wildcard label matches exactly one label, and only the leftmost
/// label may be a wildcard (RFC 6125 §6.4.3, and every browser enforces it). A remote service is
/// four labels — `<service>.<node>.n.adi` — so:
///
/// * `*.n.adi` matches `laptop-b.n.adi`, never `nosh.laptop-b.n.adi`. One label short.
/// * `*.*.n.adi` would be the shape that "fits", and is worthless: a second wildcard is rejected
///   outright, so the SAN matches nothing at all. It is deliberately not emitted — a certificate
///   that looks like it covers the fleet but silently doesn't is worse than one that admits it.
/// * `*.<node>.n.adi` puts the single wildcard leftmost, over the service label, which is the one
///   part that genuinely varies without limit. That works, and it costs one SAN per **node** — a
///   number bounded by how many machines you have paired, not by how many services they run.
///
/// So the leaf can only cover nodes it knows about, which is why `proxy.mesh_nodes` exists: pairing
/// records the petname, and the next start mints a leaf that covers it. The zone wildcard
/// [`MESH_ZONE_SAN`] is added alongside so a node's own apex is covered even before any service on
/// it is named.
///
/// **A service name deeper than one label needs an entry of its own.** `docs/fleet.md` §1 allows
/// `app.nosh.<node>.n.adi`, and the same one-label rule that defeats `*.*.n.adi` defeats
/// `*.<node>.n.adi` there. An entry may therefore carry dots — `nosh.laptop-b` mints
/// `*.nosh.laptop-b.n.adi`, covering every service under that name — which is the manual escape
/// hatch for https to a deep name. Nothing populates those automatically: pairing learns a
/// petname, never the shape of the hosts the node happens to serve. Over plain http a deep name
/// needs none of this and works as soon as it is granted.
fn mesh_sans(nodes: &[String]) -> Vec<String> {
    let mut out = vec![MESH_ZONE_SAN.to_string()];
    out.extend(
        nodes
            .iter()
            .map(|n| n.trim().to_ascii_lowercase())
            .filter(|n| !n.is_empty())
            .map(|n| format!("*.{n}.{}", crate::config::MESH_ZONE)),
    );
    out
}

/// Read the CA from disk, or generate and persist one. Returns whether it was just created.
///
/// The CA comes back as a [`CertifiedIssuer`] — rcgen 0.14 signs against an `Issuer` (the subject,
/// key-id method, key usages and signing key bundled together) rather than a certificate plus a
/// loose key pair, and `CertifiedIssuer` is the variant that also keeps the certificate itself, so
/// `ca.pem` can still be written from it.
fn load_or_create_ca(dir: &Path) -> anyhow::Result<(CertifiedIssuer<'static, KeyPair>, bool)> {
    let cert_path = dir.join("ca.pem");
    let key_path = dir.join("ca-key.pem");

    // Both halves must be present; one without the other is a broken state worth replacing.
    if cert_path.exists() && key_path.exists() {
        let key_pem = read_key(&key_path)?;
        let key = KeyPair::from_pem(&key_pem).context("parsing the CA key")?;
        // rcgen can't load a certificate back from PEM, so re-derive it from the same parameters
        // and key. Deterministic in everything that matters: the public key, and therefore the
        // subject key identifier and signature the leaf chains to, all come from `key`.
        let ca = CertifiedIssuer::self_signed(ca_params()?, key)
            .context("rebuilding the CA certificate from its key")?;
        return Ok((ca, false));
    }

    let key = KeyPair::generate().context("generating the CA key")?;
    // Serialized before the key moves into the issuer below, which takes ownership of it.
    let key_pem = key.serialize_pem();
    let ca =
        CertifiedIssuer::self_signed(ca_params()?, key).context("generating the CA certificate")?;
    write_public(&cert_path, &ca.pem())?;
    write_key(&key_path, &key_pem)?;
    info!(path = %cert_path.display(), "generated a local certificate authority");
    Ok((ca, true))
}

/// The CA's parameters. Must stay byte-stable across releases: [`load_or_create_ca`] rebuilds the
/// certificate from these on every start, and a changed subject would no longer match what the
/// user trusted.
fn ca_params() -> anyhow::Result<CertificateParams> {
    let mut params = CertificateParams::new(Vec::new()).context("building CA parameters")?;
    params.distinguished_name = {
        let mut dn = rcgen::DistinguishedName::new();
        dn.push(DnType::CommonName, "adi local CA");
        dn.push(DnType::OrganizationName, "adi-family");
        dn
    };
    params.is_ca = IsCa::Ca(BasicConstraints::Constrained(0));
    params.key_usages = vec![
        KeyUsagePurpose::KeyCertSign,
        KeyUsagePurpose::CrlSign,
        KeyUsagePurpose::DigitalSignature,
    ];
    // Fixed dates, not `now`: this function is re-run on every start to rebuild the certificate
    // that signs the leaf, and a moving validity window would make it a different certificate each
    // time. Spans [`CA_DAYS`] from a date safely in the past.
    params.not_before = rcgen::date_time_ymd(2025, 1, 1);
    params.not_after = rcgen::date_time_ymd(2025, 1, 1) + TimeDuration::days(CA_DAYS);
    Ok(params)
}

/// Read the leaf if it still covers `wanted` and isn't near expiry; otherwise mint a new one.
fn load_or_create_leaf(
    dir: &Path,
    wanted: &[String],
    ca: &CertifiedIssuer<'_, KeyPair>,
) -> anyhow::Result<(Vec<CertificateDer<'static>>, PrivateKeyDer<'static>)> {
    let cert_path = dir.join("cert.pem");
    let key_path = dir.join("key.pem");
    let meta_path = dir.join("cert.json");

    if let Some(reason) = reissue_reason(&cert_path, &key_path, &meta_path, wanted) {
        info!(hosts = ?wanted, %reason, "issuing a front-door certificate");
        let key = KeyPair::generate().context("generating the leaf key")?;
        let mut params =
            CertificateParams::new(wanted.to_vec()).context("building leaf parameters")?;
        params.distinguished_name = {
            let mut dn = rcgen::DistinguishedName::new();
            dn.push(DnType::CommonName, "adi front door");
            dn
        };
        params.is_ca = IsCa::NoCa;
        params.key_usages = vec![KeyUsagePurpose::DigitalSignature];
        params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
        let now = OffsetDateTime::now_utc();
        // Backdate slightly: a client whose clock trails ours shouldn't see a not-yet-valid cert.
        params.not_before = now - TimeDuration::hours(1);
        params.not_after = now + TimeDuration::days(LEAF_DAYS);
        let cert = params
            .signed_by(&key, ca)
            .context("signing the leaf certificate")?;

        write_public(&cert_path, &cert.pem())?;
        write_key(&key_path, &key.serialize_pem())?;
        let meta = LeafMeta {
            hosts: wanted.to_vec(),
            issued_unix: now.unix_timestamp(),
        };
        write_public(
            &meta_path,
            &serde_json::to_string_pretty(&meta).unwrap_or_default(),
        )?;
    }

    let chain = read_chain(&cert_path)?;
    let key = read_leaf_key(&key_path)?;
    Ok((chain, key))
}

/// Why the leaf needs re-issuing, or `None` when the one on disk will do.
fn reissue_reason(
    cert_path: &Path,
    key_path: &Path,
    meta_path: &Path,
    wanted: &[String],
) -> Option<&'static str> {
    if !cert_path.exists() || !key_path.exists() {
        return Some("no certificate yet");
    }
    let Some(meta) = std::fs::read_to_string(meta_path)
        .ok()
        .and_then(|raw| serde_json::from_str::<LeafMeta>(&raw).ok())
    else {
        // Can't tell what it covers, so don't trust that it covers the right thing.
        return Some("certificate metadata missing or unreadable");
    };
    if meta.hosts != wanted {
        return Some("proxied host set changed");
    }
    let age_days = (OffsetDateTime::now_utc().unix_timestamp() - meta.issued_unix) / 86_400;
    if age_days >= LEAF_DAYS - RENEW_WITHIN_DAYS {
        return Some("certificate nearing expiry");
    }
    None
}

fn read_chain(path: &Path) -> anyhow::Result<Vec<CertificateDer<'static>>> {
    let pem = std::fs::read(path)
        .with_context(|| format!("reading certificate {}", path.display()))?;
    let chain = CertificateDer::pem_slice_iter(&pem)
        .collect::<Result<Vec<_>, _>>()
        .with_context(|| format!("parsing certificate {}", path.display()))?;
    anyhow::ensure!(!chain.is_empty(), "{} held no certificate", path.display());
    Ok(chain)
}

fn read_leaf_key(path: &Path) -> anyhow::Result<PrivateKeyDer<'static>> {
    let pem = read_key(path)?;
    // An empty file is not a distinct case here the way it is for the chain above: pki-types
    // reports "no PEM section of the wanted kind" as a parse error, so one context covers both
    // a malformed key and a file that held none.
    PrivateKeyDer::from_pem_slice(pem.as_bytes())
        .with_context(|| format!("parsing private key {}", path.display()))
}

/// Read a private key, naming the likely cause when the daemon simply isn't allowed to.
fn read_key(path: &Path) -> anyhow::Result<String> {
    std::fs::read_to_string(path).map_err(|e| {
        if e.kind() == std::io::ErrorKind::PermissionDenied {
            anyhow::anyhow!(
                "cannot read {} ({e}) — it is 0600 and owned by another user; \
                 the front door and this process disagree on who runs TLS",
                path.display()
            )
        } else {
            anyhow::anyhow!("reading {}: {e}", path.display())
        }
    })
}

fn write_public(path: &Path, contents: &str) -> anyhow::Result<()> {
    std::fs::write(path, contents)
        .with_context(|| format!("writing {}", path.display()))?;
    set_mode(path, 0o644);
    Ok(())
}

/// Write a private key and take away group/other access before it can be read.
fn write_key(path: &Path, contents: &str) -> anyhow::Result<()> {
    std::fs::write(path, contents).with_context(|| format!("writing {}", path.display()))?;
    set_mode(path, 0o600);
    Ok(())
}

fn set_mode(path: &Path, mode: u32) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        if let Err(e) = std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)) {
            warn!(path = %path.display(), error = %e, "could not set file mode");
        }
    }
    #[cfg(not(unix))]
    let _ = (path, mode);
}

/// The one-time instruction the user has to run for a browser to accept the front door. Logged on
/// every start while the CA is untrusted-looking (freshly generated), because a front door serving
/// HTTPS nobody trusts is worse than one that says so.
#[must_use]
pub fn trust_hint(ca_path: &Path) -> String {
    if cfg!(target_os = "macos") {
        format!(
            "trust it once with:  sudo security add-trusted-cert -d -r trustRoot \
             -k /Library/Keychains/System.keychain {}",
            ca_path.display()
        )
    } else {
        format!("trust {} in your browser/system trust store", ca_path.display())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn san_list_adds_base_hosts_and_dedupes() {
        let sans = san_list(&["app.adi".into(), "APP.adi".into(), "api.adi".into()], None);
        assert_eq!(
            sans,
            vec!["127.0.0.1", "127.0.0.53", "api.adi", "app.adi", "localhost"]
        );
    }

    #[test]
    fn san_list_ignores_blank_hosts() {
        let sans = san_list(&[String::new(), "  ".into()], None);
        assert_eq!(sans, vec!["127.0.0.1", "127.0.0.53", "localhost"]);
    }

    /// The wildcard decision, written down: a four-label `<service>.<node>.n.adi` needs the single
    /// permitted wildcard in the **leftmost** label with the node spelled out, so the leaf carries
    /// one SAN per paired node — never a two-wildcard `*.*.n.adi`, which no client accepts.
    #[test]
    fn mesh_sans_use_one_leftmost_wildcard_per_node() {
        let sans = mesh_sans(&["laptop-b".into(), "Tower".into(), "  ".into()]);
        assert_eq!(sans, vec!["*.n.adi", "*.laptop-b.n.adi", "*.tower.n.adi"]);

        // `*.n.adi` is the node apex only — it is one label short of a service name, which is the
        // whole reason the per-node entries exist.
        assert!(matches_wildcard("*.n.adi", "laptop-b.n.adi"));
        assert!(!matches_wildcard("*.n.adi", "nosh.laptop-b.n.adi"));
        assert!(matches_wildcard("*.laptop-b.n.adi", "nosh.laptop-b.n.adi"));

        // …and one label short again for a deep service name, which is why a dotted entry is
        // allowed: it moves the same single wildcard one level further down.
        assert!(!matches_wildcard("*.laptop-b.n.adi", "app.nosh.laptop-b.n.adi"));
        let deep = mesh_sans(&["nosh.laptop-b".into()]);
        assert_eq!(deep, vec!["*.n.adi", "*.nosh.laptop-b.n.adi"]);
        assert!(matches_wildcard(
            "*.nosh.laptop-b.n.adi",
            "app.nosh.laptop-b.n.adi"
        ));

        assert!(
            !sans.iter().any(|s| s.matches('*').count() > 1),
            "a second wildcard label is rejected by every client; it must never be emitted",
        );
    }

    /// A single wildcard label, matched the way RFC 6125 §6.4.3 says clients do: it stands for
    /// exactly one label, and only the leftmost one.
    fn matches_wildcard(san: &str, host: &str) -> bool {
        let Some(suffix) = san.strip_prefix("*.") else {
            return san == host;
        };
        host.strip_suffix(suffix)
            .is_some_and(|label| label.ends_with('.') && !label[..label.len() - 1].contains('.'))
    }

    #[test]
    fn a_configured_mesh_gateway_puts_the_zone_wildcard_on_the_leaf() {
        // Gateway configured, nothing paired yet: the zone wildcard is still carried, so a node's
        // own apex is covered the moment it appears.
        let bare = san_list(&["app.adi".into()], Some(&[]));
        assert_eq!(
            bare,
            vec!["*.n.adi", "127.0.0.1", "127.0.0.53", "app.adi", "localhost"]
        );

        let paired = san_list(&["app.adi".into()], Some(&["laptop-b".to_string()]));
        assert!(paired.contains(&"*.laptop-b.n.adi".to_string()));

        // No mesh gateway: not one mesh name on the leaf.
        let none = san_list(&["app.adi".into()], None);
        assert!(none.iter().all(|s| !s.contains("n.adi")), "{none:?}");
    }

    #[test]
    fn a_leaf_carrying_mesh_wildcards_still_mints() {
        // `*` is legal in a DNS SAN but not in a hostname, so prove rcgen actually accepts the
        // shape before a front door depends on it at start-up.
        let dir = std::env::temp_dir().join(format!("adi-hive-tls-mesh-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let ready = prepare(
            &dir,
            &["app.adi".to_string()],
            Some(&["laptop-b".to_string()]),
        )
        .expect("a leaf with wildcard mesh SANs");
        assert!(ready.ca_path.ends_with("ca.pem"));
        let meta = std::fs::read_to_string(dir.join("cert.json")).unwrap();
        assert!(meta.contains("*.laptop-b.n.adi"), "{meta}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_generated_identity_is_stable_and_reloads_without_reissuing() {
        let dir = std::env::temp_dir().join(format!("adi-hive-tls-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let hosts = vec!["app.adi".to_string()];

        let first = prepare(&dir, &hosts, None).expect("first prepare");
        assert!(first.ca_is_new, "the CA should be generated on a cold start");
        let ca_pem = std::fs::read_to_string(dir.join("ca.pem")).unwrap();
        let leaf_pem = std::fs::read_to_string(dir.join("cert.pem")).unwrap();

        // Same hosts: nothing should be re-issued, and the CA must not move.
        let second = prepare(&dir, &hosts, None).expect("second prepare");
        assert!(!second.ca_is_new);
        assert_eq!(ca_pem, std::fs::read_to_string(dir.join("ca.pem")).unwrap());
        assert_eq!(
            leaf_pem,
            std::fs::read_to_string(dir.join("cert.pem")).unwrap(),
            "an unchanged host set must reuse the existing leaf"
        );

        // A new host re-mints the leaf but must leave the trusted CA alone.
        prepare(&dir, &["app.adi".into(), "api.adi".into()], None).expect("third prepare");
        assert_eq!(
            ca_pem,
            std::fs::read_to_string(dir.join("ca.pem")).unwrap(),
            "the CA is the trust anchor; it must survive a leaf re-issue"
        );
        assert_ne!(
            leaf_pem,
            std::fs::read_to_string(dir.join("cert.pem")).unwrap(),
            "a changed host set must produce a new leaf"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn the_private_keys_are_not_world_readable() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = std::env::temp_dir().join(format!("adi-hive-tls-mode-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        prepare(&dir, &["app.adi".to_string()], None).expect("prepare");
        for name in ["ca-key.pem", "key.pem"] {
            let mode = std::fs::metadata(dir.join(name)).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600, "{name} should be 0600, was {mode:o}");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
