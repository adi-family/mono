//! This node's own opaque id, as the router sees it — `docs/channels.md` §1: a node token is
//! scoped to `(node id, provider)`.
//!
//! Deliberately **not** the fleet node name (`docs/fleet.md`): the design doc is explicit that
//! "node" here means "an ADI install" and draws no analogy to a fleet peer beyond the shared
//! English word, so reusing the fleet name would wire a coupling the design never asked for.
//! Instead this is a random id, minted once and kept forever at `channels/node_id`.

use crate::error::Result;

const FILE_NAME: &str = "node_id";

/// This install's id, creating one on first use.
///
/// # Errors
/// [`Error::Config`] if the id can't be read or written.
pub fn get_or_create(config: &adi_config::Config) -> Result<String> {
    let module = config.module("channels");
    if let Some(bytes) = module.read_raw(FILE_NAME)? {
        let id = String::from_utf8_lossy(&bytes).trim().to_string();
        if !id.is_empty() {
            return Ok(id);
        }
    }
    let id = uuid::Uuid::new_v4().to_string();
    module.write_raw(FILE_NAME, id.as_bytes())?;
    Ok(id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn minted_once_and_then_stable() {
        let root = std::env::temp_dir().join(format!(
            "adi-channels-node-id-{}-{:?}",
            std::process::id(),
            std::thread::current().id(),
        ));
        let _ = std::fs::remove_dir_all(&root);
        let config = adi_config::Config::with_root(root);

        let first = get_or_create(&config).expect("mint");
        assert!(!first.is_empty());
        let second = get_or_create(&config).expect("reread");
        assert_eq!(first, second);
    }
}
