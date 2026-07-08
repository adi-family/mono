//! The `files` feature: jailed file access inside a *registered* project's directory. Every
//! path is confined to `projects/<id>/` by an [`adi_fs::Jail`] — no `..`, no absolute paths,
//! no symlink escape — and only projects that exist in the registry get a jail at all
//! (mirroring the app's `store.get` gate). This is the same isolation the app's file editor
//! uses, exposed to agents.

use adi_fs::Jail;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::{ErrorData as McpError, tool, tool_router};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::server::{AdiMcp, json_result, text_result};

/// Cap on text file size read/written through the MCP (mirrors the app's 512 KiB limit).
const MAX_TEXT_BYTES: usize = 512 * 1024;

/// A serializable view of a directory entry. `adi_fs::Entry` is a pure-std type with no serde
/// impl, so — like the app's `FileEntry` DTO — we map it to this before returning JSON.
#[derive(Debug, Serialize)]
struct FileEntry {
    /// The entry's file name (no path).
    name: String,
    /// Whether it is a directory.
    is_dir: bool,
    /// Whether it is a symbolic link.
    is_symlink: bool,
    /// Size in bytes (0 for directories).
    size: u64,
    /// Last-modified time as Unix epoch seconds, when the platform reports it.
    #[serde(skip_serializing_if = "Option::is_none")]
    modified: Option<u64>,
}

impl From<adi_fs::Entry> for FileEntry {
    fn from(e: adi_fs::Entry) -> Self {
        Self {
            name: e.name,
            is_dir: e.is_dir,
            is_symlink: e.is_symlink,
            size: e.size,
            modified: e.modified,
        }
    }
}

/// Arguments for `files_list`.
#[derive(Debug, Deserialize, JsonSchema)]
struct ListFilesArgs {
    /// The project id whose directory to browse.
    project: String,
    /// Directory path relative to the project root (default: the project root).
    #[serde(default)]
    path: String,
}

/// Arguments for `files_read`.
#[derive(Debug, Deserialize, JsonSchema)]
struct ReadFileArgs {
    /// The project id.
    project: String,
    /// File path relative to the project root.
    path: String,
}

/// Arguments for `files_write`.
#[derive(Debug, Deserialize, JsonSchema)]
struct WriteFileArgs {
    /// The project id.
    project: String,
    /// File path relative to the project root.
    path: String,
    /// The full new text contents of the file (overwrites any existing contents).
    content: String,
}

#[tool_router(router = files_router, vis = "pub")]
impl AdiMcp {
    #[tool(
        description = "List files and directories at a path inside a registered project (jailed to the project directory)"
    )]
    async fn files_list(
        &self,
        Parameters(args): Parameters<ListFilesArgs>,
    ) -> Result<CallToolResult, McpError> {
        let jail = project_jail(&args.project)?;
        let entries: Vec<FileEntry> = jail
            .list(&args.path)
            .map_err(fs_err)?
            .into_iter()
            .map(FileEntry::from)
            .collect();
        json_result(&entries)
    }

    #[tool(description = "Read a UTF-8 text file from inside a registered project")]
    async fn files_read(
        &self,
        Parameters(args): Parameters<ReadFileArgs>,
    ) -> Result<CallToolResult, McpError> {
        let jail = project_jail(&args.project)?;
        let meta = jail.metadata(&args.path).map_err(fs_err)?;
        if meta.size > MAX_TEXT_BYTES as u64 {
            return Err(McpError::invalid_params(
                format!("file is too large to read ({} bytes; max {MAX_TEXT_BYTES})", meta.size),
                None,
            ));
        }
        let content = jail.read_to_string(&args.path).map_err(fs_err)?;
        Ok(text_result(content))
    }

    #[tool(description = "Overwrite (or create) a text file inside a registered project")]
    async fn files_write(
        &self,
        Parameters(args): Parameters<WriteFileArgs>,
    ) -> Result<CallToolResult, McpError> {
        if args.content.len() > MAX_TEXT_BYTES {
            return Err(McpError::invalid_params(
                format!("content is too large ({} bytes; max {MAX_TEXT_BYTES})", args.content.len()),
                None,
            ));
        }
        let jail = project_jail(&args.project)?;
        jail.write(&args.path, args.content.as_bytes()).map_err(fs_err)?;
        Ok(text_result(format!(
            "wrote {} bytes to {}",
            args.content.len(),
            args.path
        )))
    }
}

/// Build a [`Jail`] rooted at a *registered* project's directory. Returns a client error if
/// the id is unsafe or the project isn't registered, so files can only be touched inside
/// projects the platform actually knows about.
fn project_jail(id: &str) -> Result<Jail, McpError> {
    let projects = adi_core::Adi::new().projects();
    let registered = projects
        .get(id)
        .map_err(|e| McpError::invalid_params(e.to_string(), None))?;
    if registered.is_none() {
        return Err(McpError::invalid_params(
            format!("no project with id {id:?}"),
            None,
        ));
    }
    let base = projects
        .project_dir(id)
        .map_err(|e| McpError::invalid_params(e.to_string(), None))?;
    Ok(Jail::new(base))
}

/// Map an [`adi_fs::Error`] to a fitting MCP error: an escape attempt, wrong file type, or
/// non-text file is a client (`invalid_params`) error; a missing path is too; raw I/O is
/// internal.
// Consumed by value because it is used as a `.map_err(fs_err)` adapter, which hands the error
// over by value.
#[allow(clippy::needless_pass_by_value)]
fn fs_err(e: adi_fs::Error) -> McpError {
    use adi_fs::Error as E;
    match e {
        E::Escape(_) | E::NotAFile(_) | E::NotText(_) | E::NotFound(_) => {
            McpError::invalid_params(e.to_string(), None)
        }
        E::Io { .. } => McpError::internal_error(format!("filesystem error: {e}"), None),
    }
}
