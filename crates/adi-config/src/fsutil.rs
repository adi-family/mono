//! Small filesystem helpers shared by the config-file and raw-store code.

use std::io;
use std::path::Path;

/// Write `bytes` to `path` atomically: create parents, write a per-pid temp file,
/// then rename it into place so a reader never observes a half-written file.
pub(crate) fn atomic_write(path: &Path, bytes: &[u8]) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    // Per-pid temp name keeps concurrent writers from clobbering each other's temp file.
    let file_name = path
        .file_name()
        .map_or_else(|| "config".to_string(), |n| n.to_string_lossy().into_owned());
    let tmp = path.with_file_name(format!("{file_name}.{}.tmp", std::process::id()));

    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}
