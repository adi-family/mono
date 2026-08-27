//! What this browser knows: its own identity, and one record per paired node.
//!
//! All of it lives in `IndexedDB` on this origin (`js/store.js`) and none of it is ever sent
//! anywhere. That is the whole security model, and it is worth being plain about what it is not:
//! a Mac keeps its mesh identity in `~/.adi/mono` under the user's own permissions and a phone
//! keeps it in the Keychain, but a browser has nothing of that kind. **Clearing site data destroys
//! the key and every pairing with it** — the node still holds a record for a key that no longer
//! exists anywhere, and the fix is to pair again.
//!
//! One consequence follows and is `docs/fleet.md` §5's second layer meeting a browser: the node's
//! password is kept here, so a reader is not asked for it on every request. It is stored beside
//! the key it authenticates and reachable by anything running on this origin — which is why this
//! client renders exactly one service, the node's own panel, and not arbitrary dashboards (ADI-13).

use serde::{Deserialize, Serialize};
use wasm_bindgen::prelude::*;

use crate::invite::{secret_from_hex, secret_to_hex};
use crate::mesh::Result;

#[wasm_bindgen(module = "/js/store.js")]
extern "C" {
    #[wasm_bindgen(catch)]
    async fn load(key: &str) -> std::result::Result<JsValue, JsValue>;
    #[wasm_bindgen(catch)]
    async fn save(key: &str, value: &str) -> std::result::Result<(), JsValue>;
}

/// Where the secret key lives.
const IDENTITY_KEY: &str = "identity";

/// Where the node list lives — one JSON array, written whole.
///
/// A record per row would be tidier and is not worth it: the list is a handful of entries a person
/// typed, it is read entirely on every render, and one value means a change is one atomic write
/// rather than a set of them that can half-fail.
const NODES_KEY: &str = "nodes";

/// One node this browser has paired with.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeRecord {
    /// What **this browser** calls the node. Local, renameable, and never sent anywhere
    /// (`docs/fleet.md` §2 rule 5) — it is also the `/n/<petname>/` path its panel is served under.
    pub petname: String,
    /// The node's `EndpointId`: its true name, and the only thing here that authorises anything.
    pub key: String,
    /// The relay the node calls home. Carried per node rather than assumed, because a node this
    /// client can reach need not be on the relay this client calls home (`docs/fleet.md` §9).
    pub relay: String,
    /// The username its Basic gate wants — `adi` from a pairing.
    pub username: String,
    /// The password it minted for this browser. See the module header for where this sits.
    pub password: String,
    /// Unix seconds at pairing, for the list's ordering and for a human to recognise a stale row.
    pub paired_at: u64,
    /// What the node said this browser may reach — `["http:app"]` from a fresh pairing. Recorded
    /// to show, never to enforce: the node is the only side whose opinion of a grant counts.
    #[serde(default)]
    pub grants: Vec<String>,
}

impl NodeRecord {
    /// The address to dial for this node.
    ///
    /// # Errors
    /// If the stored key or relay does not parse — which means the record was hand-edited, since
    /// nothing writes one that does not.
    pub fn addr(&self) -> Result<iroh::EndpointAddr> {
        crate::invite::addr_from(&self.key, &self.relay)
    }

    /// The key in the short form a person compares against what a node printed.
    #[must_use]
    pub fn short_key(&self) -> String {
        self.key.chars().take(10).collect()
    }
}

/// This browser's iroh secret key, minted on first use and kept forever after.
///
/// # Errors
/// If `IndexedDB` cannot be read or written — which in a browser means private-mode storage refusing
/// a write, and is worth surfacing rather than papering over with an in-memory key that would pair
/// once and be gone on reload.
pub async fn identity() -> Result<iroh::SecretKey> {
    if let Some(hex) = read(IDENTITY_KEY).await?
        && let Ok(secret) = secret_from_hex(&hex)
    {
        return Ok(secret);
    }
    // `SecretKey::generate` draws from `crypto.getRandomValues` here, the same source the CLI's
    // `identity::load_or_create` reaches through the OS.
    let secret = iroh::SecretKey::generate();
    write(IDENTITY_KEY, &secret_to_hex(&secret)).await?;
    Ok(secret)
}

/// Every paired node, oldest pairing first.
///
/// # Errors
/// If the store cannot be read. A value that does not parse is treated as an empty list rather
/// than an error: a half-written record must not lock a reader out of the ones that are fine.
pub async fn nodes() -> Result<Vec<NodeRecord>> {
    let Some(json) = read(NODES_KEY).await? else {
        return Ok(Vec::new());
    };
    Ok(serde_json::from_str(&json).unwrap_or_default())
}

/// Replace the node list.
///
/// # Errors
/// If the store cannot be written.
pub async fn save_nodes(nodes: &[NodeRecord]) -> Result<()> {
    let json = serde_json::to_string(nodes)
        .map_err(|e| format!("the node list did not serialise: {e}"))?;
    write(NODES_KEY, &json).await
}

/// Add `node`, replacing any record with the same key.
///
/// Keyed on the **key** and not the petname, because re-pairing a node you already have is what an
/// operator does when the password is lost (`docs/fleet.md` §8) — and it must update that node, not
/// add a second row for the same machine under a name the reader has to disambiguate.
///
/// # Errors
/// If the store cannot be read or written.
pub async fn add_node(node: NodeRecord) -> Result<Vec<NodeRecord>> {
    let mut nodes = nodes().await?;
    match nodes.iter().position(|n| n.key == node.key) {
        // The reader's own name for it survives a re-pair; only what the node told us is refreshed.
        Some(at) => {
            let petname = nodes[at].petname.clone();
            nodes[at] = NodeRecord { petname, ..node };
        }
        None => nodes.push(node),
    }
    save_nodes(&nodes).await?;
    Ok(nodes)
}

/// A petname free in `nodes`, starting from `wanted` and suffixing until one is.
///
/// §2 rule 3 in one function: a clash resolves to a suggestion, it never refuses. Here the clash is
/// with another node *this browser* has, which the far side knows nothing about.
#[must_use]
pub fn free_petname(nodes: &[NodeRecord], wanted: &str) -> String {
    let taken = |name: &str| nodes.iter().any(|n| n.petname == name);
    if !taken(wanted) {
        return wanted.to_string();
    }
    // Bounded by the list itself: with `n` names taken, one of the first `n + 2` suffixes is free.
    // Clippy is right that a bare `(2..)` cannot be proved to terminate, and a reader deserves the
    // same proof.
    (2..nodes.len() + 3)
        .map(|n| format!("{wanted}-{n}"))
        .find(|name| !taken(name))
        .unwrap_or_else(|| wanted.to_string())
}

async fn read(key: &str) -> Result<Option<String>> {
    let value = load(key)
        .await
        .map_err(|e| format!("the browser's storage could not be read: {}", describe(&e)))?;
    Ok(value.as_string())
}

async fn write(key: &str, value: &str) -> Result<()> {
    save(key, value).await.map_err(|e| {
        format!(
            "the browser's storage could not be written: {}",
            describe(&e)
        )
    })
}

/// A JS exception as a sentence. `Error.message` when there is one, the value's own string form
/// otherwise — a `DOMException` from a private-mode write is the case that matters, and it has one.
fn describe(error: &JsValue) -> String {
    js_sys::Reflect::get(error, &JsValue::from_str("message"))
        .ok()
        .and_then(|m| m.as_string())
        .or_else(|| error.as_string())
        .unwrap_or_else(|| "unknown error".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(petname: &str, key: &str) -> NodeRecord {
        NodeRecord {
            petname: petname.into(),
            key: key.into(),
            relay: "https://mad.mono-relay.withadi.dev".into(),
            username: "adi".into(),
            password: "s3cret".into(),
            paired_at: 1,
            grants: vec!["http:app".into()],
        }
    }

    #[test]
    fn a_petname_clash_resolves_rather_than_refusing() {
        let nodes = vec![node("laptop", "aa"), node("laptop-2", "bb")];
        assert_eq!(free_petname(&nodes, "desktop"), "desktop");
        assert_eq!(free_petname(&nodes, "laptop"), "laptop-3");
        assert_eq!(free_petname(&[], "laptop"), "laptop");
    }

    #[test]
    fn a_record_round_trips_through_its_json() {
        let record = node("laptop", "aa");
        let json = serde_json::to_string(&record).expect("encode");
        assert_eq!(
            serde_json::from_str::<NodeRecord>(&json).expect("decode"),
            record
        );

        // A record written before `grants` existed must still load — the list is shown, not acted
        // on, so an absent one is an honest empty rather than a reason to refuse the row.
        let older = r#"{"petname":"a","key":"b","relay":"","username":"adi",
                        "password":"p","paired_at":1}"#;
        assert!(
            serde_json::from_str::<NodeRecord>(older)
                .expect("decode")
                .grants
                .is_empty()
        );
    }
}
