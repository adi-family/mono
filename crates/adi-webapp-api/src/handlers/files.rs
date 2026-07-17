use std::path::{Component, Path};

use adi_fs::Error as FsError;
use adi_fs::Jail;
use adi_projects::Projects;

use crate::types::{DirListing, FileContent, FileEntry, FilesRef, WriteFile};

use super::response::{error, ok_json, Response};

/// The largest text file we'll read into the editor or accept on a write. Keeps a single
/// response/request bounded (project files here are configs — small); a larger file is
/// refused rather than truncated. Comfortably under the server's 1 MiB request-body cap.
pub(crate) const MAX_TEXT_BYTES: u64 = 512 * 1024;

/// `POST /api/projects/files` — list a directory inside a project's own directory, confined to
/// it by the [`adi_fs`] jail (no `..`, no absolute paths, no symlink escape). `path` is relative
/// to the project root (`""` is the root).
#[must_use]
pub fn list_files(store: &Projects, body: &[u8]) -> Response {
    let Some(req) = parse_files_ref(body) else {
        return bad_files_ref();
    };
    let jail = match project_jail(store, &req.id) {
        Ok(jail) => jail,
        Err(resp) => return resp,
    };
    match jail.list(&req.path) {
        Ok(entries) => {
            let path = normalize_rel(&req.path);
            let parent = parent_rel(&path);
            ok_json(&DirListing {
                id: req.id,
                path,
                parent,
                entries: entries.into_iter().map(file_entry).collect(),
            })
        }
        Err(e) => Response::from(&e),
    }
}

/// `POST /api/projects/file/read` — read one text file inside a project's directory. Binary
/// files and files over [`MAX_TEXT_BYTES`] are refused rather than returned.
#[must_use]
pub fn read_file(store: &Projects, body: &[u8]) -> Response {
    let Some(req) = parse_files_ref(body) else {
        return bad_files_ref();
    };
    let jail = match project_jail(store, &req.id) {
        Ok(jail) => jail,
        Err(resp) => return resp,
    };
    read_file_content(&jail, &req.id, &req.path)
}

/// `POST /api/projects/file/write` — atomically save one text file inside a project's directory,
/// creating any missing parents within it. Returns the fresh [`FileContent`] (re-read from disk)
/// so the client updates its size/modified in one round-trip.
#[must_use]
pub fn write_file(store: &Projects, body: &[u8]) -> Response {
    let Some(req) = parse_write_file(body) else {
        return error(
            400,
            "expected JSON body { \"id\": \"…\", \"path\": \"…\", \"content\": \"…\" }",
        );
    };
    if req.content.len() as u64 > MAX_TEXT_BYTES {
        return error(
            413,
            &format!("file too large to save (max {MAX_TEXT_BYTES} bytes)"),
        );
    }
    let jail = match project_jail(store, &req.id) {
        Ok(jail) => jail,
        Err(resp) => return resp,
    };
    if let Err(e) = jail.write(&req.path, req.content.as_bytes()) {
        return Response::from(&e);
    }
    // Re-read so the response carries the authoritative size/modified after the write.
    read_file_content(&jail, &req.id, &req.path)
}

/// Read `rel` as text and shape a [`FileContent`], enforcing the [`MAX_TEXT_BYTES`] cap.
fn read_file_content(jail: &Jail, id: &str, rel: &str) -> Response {
    let meta = match jail.metadata(rel) {
        Ok(meta) => meta,
        Err(e) => return Response::from(&e),
    };
    if meta.is_dir {
        return error(400, &format!("not a file: {rel}"));
    }
    if meta.size > MAX_TEXT_BYTES {
        return error(
            413,
            &format!(
                "file too large to edit ({} bytes, max {MAX_TEXT_BYTES})",
                meta.size
            ),
        );
    }
    match jail.read_to_string(rel) {
        Ok(content) => ok_json(&FileContent {
            id: id.to_string(),
            path: normalize_rel(rel),
            content,
            size: meta.size,
            modified: meta.modified,
        }),
        Err(e) => Response::from(&e),
    }
}

/// Build a jail rooted at a *registered* project's directory. A path with an unsafe id is a
/// 400; an unregistered id is a 404 (mirroring [`project_detail`]); a store failure is a 500.
fn project_jail(store: &Projects, id: &str) -> Result<Jail, Response> {
    // Only registered projects are browsable — same existence gate as the detail view.
    match store.get(id) {
        Ok(Some(_)) => {}
        Ok(None) => return Err(error(404, &format!("no such project: {id}"))),
        Err(e) => return Err(Response::from(&e)),
    }
    let dir = store.project_dir(id).map_err(|e| Response::from(&e))?;
    Ok(Jail::new(dir))
}

// Map a jail [`FsError`] to an HTTP status: an escape/`not-a-file` is a 400, a missing path a
// 404, a non-UTF-8 (binary) file a 415, and any other I/O error a 500.
impl From<&FsError> for Response {
    fn from(e: &FsError) -> Self {
        let status = match e {
            FsError::Escape(_) | FsError::NotAFile(_) => 400,
            FsError::NotFound(_) => 404,
            FsError::NotText(_) => 415,
            FsError::Io { .. } => 500,
        };
        error(status, &e.to_string())
    }
}

/// Flatten an [`adi_fs::Entry`] into its wire [`FileEntry`] DTO.
fn file_entry(entry: adi_fs::Entry) -> FileEntry {
    FileEntry {
        name: entry.name,
        is_dir: entry.is_dir,
        is_symlink: entry.is_symlink,
        size: entry.size,
        modified: entry.modified,
    }
}

/// Normalize a jailed relative path to a clean display form: keep only real segments joined by
/// `/`, dropping `.` and redundant separators. `..`/absolute paths never reach here (the jail
/// rejects them first). The project root is the empty string.
fn normalize_rel(rel: &str) -> String {
    Path::new(rel)
        .components()
        .filter_map(|c| match c {
            Component::Normal(s) => Some(s.to_string_lossy().into_owned()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("/")
}

/// The parent of a normalized relative path: `None` at the root, else the path with its last
/// segment removed (a top-level entry's parent is the root, `""`).
fn parent_rel(norm: &str) -> Option<String> {
    if norm.is_empty() {
        return None;
    }
    match norm.rsplit_once('/') {
        Some((head, _)) => Some(head.to_string()),
        None => Some(String::new()),
    }
}

fn parse_files_ref(body: &[u8]) -> Option<FilesRef> {
    let req: FilesRef = serde_json::from_slice(body).ok()?;
    (!req.id.trim().is_empty()).then_some(req)
}

fn bad_files_ref() -> Response {
    error(
        400,
        "expected JSON body { \"id\": \"…\", \"path\"?: \"…\" }",
    )
}

fn parse_write_file(body: &[u8]) -> Option<WriteFile> {
    let req: WriteFile = serde_json::from_slice(body).ok()?;
    (!req.id.trim().is_empty() && !req.path.trim().is_empty()).then_some(req)
}

// MARK: workspaces & project hooks — working copies created by the script files under
// <project>/.adi/hooks, registered in <project>/.adi/workspaces.toml. All routes live
// under /api/projects/… (never /api/hooks/*, which is the triggers webhook URL space).
