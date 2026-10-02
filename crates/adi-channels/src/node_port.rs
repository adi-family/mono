//! Where `adi-app` is actually listening, written at start-up so a standalone process — the
//! `channel-reply` tool script, above all — can find its own node's local API without guessing a
//! port. `~/.adi/mono/channels/node_port` (honoring `$ADI_DIR`, the same way the tool's own script
//! reads it; see `src/tool.rs`).
//!
//! A narrow, single-purpose mechanism rather than a general "where is adi-app" registry: nothing
//! else in this crate needs one, and a general one is more than this task asks for.

use crate::error::Result;

const FILE_NAME: &str = "node_port";

/// Record this node's own listening port.
///
/// # Errors
/// [`Error::Config`] on a write failure.
pub fn write(config: &adi_config::Config, port: u16) -> Result<()> {
    config
        .module("channels")
        .write_raw(FILE_NAME, port.to_string().as_bytes())?;
    Ok(())
}

/// The port last recorded, if any.
///
/// # Errors
/// [`Error::Config`] on a read failure.
pub fn read(config: &adi_config::Config) -> Result<Option<u16>> {
    let Some(bytes) = config.module("channels").read_raw(FILE_NAME)? else {
        return Ok(None);
    };
    Ok(String::from_utf8_lossy(&bytes).trim().parse().ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_then_read_round_trips() {
        let root = std::env::temp_dir().join(format!(
            "adi-channels-node-port-{}-{:?}",
            std::process::id(),
            std::thread::current().id(),
        ));
        let _ = std::fs::remove_dir_all(&root);
        let config = adi_config::Config::with_root(root);

        assert_eq!(read(&config).unwrap(), None);
        write(&config, 8090).unwrap();
        assert_eq!(read(&config).unwrap(), Some(8090));
    }
}
