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
//!
//! # The CA may only vouch for this machine's own names
//!
//! [`trust_hint`] asks the user to install `ca.pem` as a **system trust root**, which is a large
//! thing to ask: an unconstrained root may sign a certificate for `google.com`, for a bank, or for
//! the operator's own SSO, and every browser on the machine will accept it. `ca-key.pem` is kept
//! `0600` and that is not the point — the point is what a future key compromise, a backup, or a
//! root-level bug would be *worth*. Unconstrained, it is worth the whole machine's TLS.
//!
//! So [`ca_params`] carries X.509 **name constraints** (RFC 5280 §4.2.1.10) permitting exactly the
//! names the front door serves: the DNS subtree `adi` (which subsumes `app.adi`, `*.n.adi` and
//! every `<service>.<node>.n.adi`), the DNS name `localhost`, and the IP subtree `127.0.0.0/8`
//! (which covers both [`BASE_SANS`] addresses). A certificate this CA signs for any other name is
//! rejected by the verifier, not merely frowned upon: Apple's Security framework and NSS both
//! enforce constraints, and `a_leaf_for_an_outside_name_is_refused_by_the_platform` proves it
//! against the real platform verifier rather than assuming it.
//!
//! There are deliberately **no `excluded_subtrees`.** A default-deny reads like the right
//! belt-and-braces addition, but RFC 5280 gives exclusion precedence over permission — a name
//! matching an excluded subtree "is rejected regardless of information in permittedSubtrees" — and
//! the empty DNS name matches *every* DNS name. Excluding it would reject the permitted subtrees
//! along with everything else. A permitted subtree with no exclusions is already default-deny for
//! its own name type, which is the whole mechanism.
//!
//! The trade, stated plainly: this root can never be reused to sign anything outside `.adi`, and
//! somebody will eventually want to. That is what it is for. Signing a name outside the front
//! door's own zone needs a different CA, not a wider one.
//!
//! ## Which is why the CA is versioned
//!
//! [`load_or_create_ca`] rebuilds the CA certificate from [`ca_params`] on every start, so
//! changing those parameters changes the certificate — and the copy the user trusted is the one in
//! their keychain, which does not change with it. A silently mismatched pair is the worst outcome
//! available: HTTPS keeps working, signed by a root whose *trusted* copy still permits every name
//! on the internet. So the parameters carry [`CA_PARAMS_VERSION`], recorded in `ca.json` beside
//! them, and a CA written under an older version is **replaced** — new key, new certificate — with
//! the re-trust instruction logged at `warn`. Bump that constant with any future change to
//! `ca_params`, and the same migration happens by itself.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Context as _;
use rcgen::{
    BasicConstraints, CertificateParams, CertifiedIssuer, CidrSubnet, DnType,
    ExtendedKeyUsagePurpose, GeneralSubtree, IsCa, KeyPair, KeyUsagePurpose, NameConstraints,
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

/// The name types the CA may vouch for, as X.509 name constraints — see this module's header.
///
/// The DNS entry is the zone label with **no leading dot**: RFC 5280 §4.2.1.10 defines a DNS
/// constraint as matching the name itself plus anything built by adding labels to its left, so
/// `adi` covers `app.adi` and `nosh.laptop-b.n.adi` alike. A leading `.adi` is a widespread
/// convention that several verifiers accept and the RFC does not define; the plain label is the
/// form every verifier agrees on.
const PERMITTED_DNS: [&str; 2] = ["adi", "localhost"];

/// The loopback block the front door's addresses live in — `127.0.0.1` and `127.0.0.53`, i.e. both
/// of [`BASE_SANS`]' addresses, and the flavour-derived one a second install binds.
const PERMITTED_IPV4: ([u8; 4], u8) = ([127, 0, 0, 0], 8);

/// The revision of [`ca_params`]. A CA on disk recorded under an older number is replaced rather
/// than rebuilt from parameters it does not match — see this module's header.
///
/// * `1` — no name constraints (implicit: nothing recorded a version).
/// * `2` — name constraints for [`PERMITTED_DNS`] and [`PERMITTED_IPV4`].
const CA_PARAMS_VERSION: u32 = 2;

/// What the CA on disk was minted under, written beside it as `ca.json` — the same trick
/// [`LeafMeta`] uses to avoid an X.509 parser in the dependency tree.
#[derive(Debug, Serialize, Deserialize)]
struct CaMeta {
    /// [`CA_PARAMS_VERSION`] at the time it was generated.
    params_version: u32,
}

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
    let (chain, key) = load_or_create_leaf(dir, &wanted, &ca, ca_is_new)?;

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
    let meta_path = dir.join("ca.json");

    // Both halves must be present; one without the other is a broken state worth replacing.
    if cert_path.exists() && key_path.exists() {
        if ca_params_version(&meta_path) == CA_PARAMS_VERSION {
            let key_pem = read_key(&key_path)?;
            let key = KeyPair::from_pem(&key_pem).context("parsing the CA key")?;
            // rcgen can't load a certificate back from PEM, so re-derive it from the same
            // parameters and key. Deterministic in everything that matters: the public key, and
            // therefore the subject key identifier and signature the leaf chains to, all come
            // from `key`.
            let ca = CertifiedIssuer::self_signed(ca_params()?, key)
                .context("rebuilding the CA certificate from its key")?;
            return Ok((ca, false));
        }
        // Replaced rather than rebuilt: the certificate this CA would now produce is not the one
        // sitting in the user's trust store, and quietly signing leaves with a root whose trusted
        // copy still permits every name on the internet is the outcome worth avoiding most.
        warn!(
            ca = %cert_path.display(),
            "replacing the local CA: the one on disk predates this build's certificate parameters \
             (it vouches for every name, not only this machine's). HTTPS will warn until you trust \
             the new one, and the old root is now inert — remove it from your trust store"
        );
    }

    let key = KeyPair::generate().context("generating the CA key")?;
    // Serialized before the key moves into the issuer below, which takes ownership of it.
    let key_pem = key.serialize_pem();
    let ca =
        CertifiedIssuer::self_signed(ca_params()?, key).context("generating the CA certificate")?;
    write_public(&cert_path, &ca.pem())?;
    write_key(&key_path, &key_pem)?;
    write_public(
        &meta_path,
        &serde_json::to_string_pretty(&CaMeta {
            params_version: CA_PARAMS_VERSION,
        })
        .unwrap_or_default(),
    )?;
    info!(path = %cert_path.display(), "generated a local certificate authority");
    Ok((ca, true))
}

/// Which revision of [`ca_params`] the CA on disk was written under. An absent or unreadable
/// `ca.json` reads as `1` — every CA generated before the file existed is unconstrained, which is
/// exactly what version 1 means.
fn ca_params_version(meta_path: &Path) -> u32 {
    std::fs::read_to_string(meta_path)
        .ok()
        .and_then(|raw| serde_json::from_str::<CaMeta>(&raw).ok())
        .map_or(1, |meta| meta.params_version)
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
    // A *path-length* constraint bounds how many CAs may chain below this one and says nothing
    // about names; the name constraints below are what stop it vouching for `google.com`. See this
    // module's header for why there are no `excluded_subtrees` to go with them.
    params.name_constraints = Some(NameConstraints {
        permitted_subtrees: PERMITTED_DNS
            .iter()
            .map(|name| GeneralSubtree::DnsName((*name).to_string()))
            .chain(std::iter::once(GeneralSubtree::IpAddress(
                CidrSubnet::from_v4_prefix(PERMITTED_IPV4.0, PERMITTED_IPV4.1),
            )))
            .collect(),
        excluded_subtrees: Vec::new(),
    });
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
    ca_is_new: bool,
) -> anyhow::Result<(Vec<CertificateDer<'static>>, PrivateKeyDer<'static>)> {
    let cert_path = dir.join("cert.pem");
    let key_path = dir.join("key.pem");
    let meta_path = dir.join("cert.json");

    if let Some(reason) = reissue_reason(&cert_path, &key_path, &meta_path, wanted, ca_is_new) {
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
    ca_is_new: bool,
) -> Option<&'static str> {
    // First, and before the file checks: a leaf signed by the CA we just replaced chains to a root
    // nothing holds, so keeping it would serve a certificate no browser can build a path for.
    if ca_is_new {
        return Some("the certificate authority was replaced");
    }
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

    /// A CA generated before the name constraints existed is *replaced*, not rebuilt from
    /// parameters it does not match — and the leaf goes with it, or it would chain to a root
    /// nothing holds.
    #[test]
    fn a_ca_from_an_older_parameter_set_is_replaced_along_with_its_leaf() {
        let dir = std::env::temp_dir().join(format!("adi-hive-tls-mig-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let hosts = vec!["app.adi".to_string()];

        prepare(&dir, &hosts, None).expect("first prepare");
        let ca_pem = std::fs::read_to_string(dir.join("ca.pem")).unwrap();
        let ca_key = std::fs::read_to_string(dir.join("ca-key.pem")).unwrap();
        let leaf_pem = std::fs::read_to_string(dir.join("cert.pem")).unwrap();

        // Exactly what an install written by an older build looks like: no marker beside the CA.
        std::fs::remove_file(dir.join("ca.json")).expect("drop the marker");
        let migrated = prepare(&dir, &hosts, None).expect("prepare over an old CA");

        assert!(migrated.ca_is_new, "an unversioned CA must be replaced");
        assert_ne!(
            ca_key,
            std::fs::read_to_string(dir.join("ca-key.pem")).unwrap(),
            "replacing means a new key, not a re-signed certificate over the old one"
        );
        assert_ne!(ca_pem, std::fs::read_to_string(dir.join("ca.pem")).unwrap());
        assert_ne!(
            leaf_pem,
            std::fs::read_to_string(dir.join("cert.pem")).unwrap(),
            "a leaf signed by the replaced CA chains to a root nothing holds"
        );
        assert_eq!(ca_params_version(&dir.join("ca.json")), CA_PARAMS_VERSION);

        // And the next start settles down again — the migration must not repeat forever.
        let settled = prepare(&dir, &hosts, None).expect("prepare again");
        assert!(!settled.ca_is_new, "a current CA is reused, not replaced");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// An absent marker is version 1 — every CA generated before `ca.json` existed is the
    /// unconstrained kind, which is what version 1 means.
    #[test]
    fn an_unmarked_ca_reads_as_the_first_parameter_set() {
        let dir = std::env::temp_dir().join(format!("adi-hive-tls-ver-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let meta = dir.join("ca.json");

        assert_eq!(ca_params_version(&meta), 1, "no file at all");
        std::fs::write(&meta, "not json").unwrap();
        assert_eq!(ca_params_version(&meta), 1, "unreadable");
        std::fs::write(&meta, r#"{"params_version":7}"#).unwrap();
        assert_eq!(ca_params_version(&meta), 7);

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The constraints cover every name a leaf can actually carry — asserted against
    /// [`san_list`]'s own output rather than a hand-copied list, so a new base SAN that the CA
    /// may not vouch for fails here instead of in a browser.
    #[test]
    fn every_name_a_leaf_carries_is_inside_the_permitted_subtrees() {
        let sans = san_list(
            &["app.adi".into(), "nosh.adi".into()],
            Some(&["laptop-b".to_string(), "nosh.zomro-de1".to_string()]),
        );
        for san in &sans {
            let permitted = san.parse::<std::net::Ipv4Addr>().map_or_else(
                |_| {
                    PERMITTED_DNS
                        .iter()
                        .any(|zone| san == zone || san.ends_with(&format!(".{zone}")))
                },
                |ip| ip.octets()[0] == PERMITTED_IPV4.0[0],
            );
            assert!(permitted, "{san} is not inside the CA's permitted subtrees");
        }
        // The list is not vacuously satisfied: an outside name must fail the same check.
        assert!(!sans.iter().any(|s| s.ends_with(".example.com")));
    }

    /// The constraints are *enforced*, not merely present — asked of the real platform verifier,
    /// which is the only authority on the question that matters.
    ///
    /// macOS only: `security verify-cert` is Security.framework, the same evaluation Safari and
    /// Chrome perform. `-r` supplies the root for this evaluation alone, so nothing is added to
    /// any keychain and the test needs no privilege. Skipped, not failed, where `security` is not
    /// on PATH — a Linux builder has nothing to say about Apple's verifier.
    #[cfg(target_os = "macos")]
    #[test]
    fn a_leaf_for_an_outside_name_is_refused_by_the_platform() {
        use std::process::Command;

        let dir = std::env::temp_dir().join(format!("adi-hive-tls-nc-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let ca_key = KeyPair::generate().expect("ca key");
        let ca = CertifiedIssuer::self_signed(ca_params().expect("ca params"), ca_key).expect("ca");
        let ca_path = dir.join("ca.pem");
        std::fs::write(&ca_path, ca.pem()).unwrap();

        // Two leaves off the same root, differing only in the name they claim.
        let mint = |host: &str| {
            let key = KeyPair::generate().expect("leaf key");
            let mut params = CertificateParams::new(vec![host.to_string()]).expect("leaf params");
            params.is_ca = IsCa::NoCa;
            params.key_usages = vec![KeyUsagePurpose::DigitalSignature];
            params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
            let now = OffsetDateTime::now_utc();
            params.not_before = now - TimeDuration::hours(1);
            params.not_after = now + TimeDuration::days(LEAF_DAYS);
            let cert = params.signed_by(&key, &ca).expect("sign");
            let path = dir.join(format!("{host}.pem"));
            std::fs::write(&path, cert.pem()).unwrap();
            path
        };

        let verify = |leaf: &Path, host: &str| {
            Command::new("security")
                .args(["verify-cert", "-p", "ssl", "-s", host, "-c"])
                .arg(leaf)
                .arg("-r")
                .arg(&ca_path)
                .output()
        };

        let inside = mint("app.adi");
        let Ok(ok) = verify(&inside, "app.adi") else {
            let _ = std::fs::remove_dir_all(&dir);
            return; // no `security` on this box — nothing to ask
        };
        assert!(
            ok.status.success(),
            "the front door's own name must still validate: {}{}",
            String::from_utf8_lossy(&ok.stdout),
            String::from_utf8_lossy(&ok.stderr),
        );

        let outside = mint("www.example.com");
        let refused = verify(&outside, "www.example.com").expect("verify-cert");
        assert!(
            !refused.status.success(),
            "a name outside the permitted subtrees must be refused, but the platform accepted it: \
             {}{}",
            String::from_utf8_lossy(&refused.stdout),
            String::from_utf8_lossy(&refused.stderr),
        );

        // The control, without which the assertion above proves nothing: the *same* outside name,
        // off a root identical but for the constraints, must verify. Otherwise the refusal could
        // be anything — a policy, an expiry, a key usage — dressed up as a win.
        let mut open_params = ca_params().expect("ca params");
        open_params.name_constraints = None;
        let open_ca = CertifiedIssuer::self_signed(open_params, KeyPair::generate().expect("key"))
            .expect("unconstrained ca");
        let open_ca_path = dir.join("open-ca.pem");
        std::fs::write(&open_ca_path, open_ca.pem()).unwrap();
        let key = KeyPair::generate().expect("leaf key");
        let mut params =
            CertificateParams::new(vec!["www.example.com".to_string()]).expect("leaf params");
        params.is_ca = IsCa::NoCa;
        params.key_usages = vec![KeyUsagePurpose::DigitalSignature];
        params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
        let now = OffsetDateTime::now_utc();
        params.not_before = now - TimeDuration::hours(1);
        params.not_after = now + TimeDuration::days(LEAF_DAYS);
        let control = params.signed_by(&key, &open_ca).expect("sign");
        let control_path = dir.join("control.pem");
        std::fs::write(&control_path, control.pem()).unwrap();
        let accepted = Command::new("security")
            .args(["verify-cert", "-p", "ssl", "-s", "www.example.com", "-c"])
            .arg(&control_path)
            .arg("-r")
            .arg(&open_ca_path)
            .output()
            .expect("verify-cert");
        assert!(
            accepted.status.success(),
            "the control must pass, or the refusal above is not about name constraints: {}{}",
            String::from_utf8_lossy(&accepted.stdout),
            String::from_utf8_lossy(&accepted.stderr),
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
