//! The ADI store browser: list, read, and write files under `~/.adi/mono`, confined to it by
//! the [`adi_fs`] jail.
//!
//! This is deliberately the *store* root and not the machine root. The project browser
//! ([`super::files`]) jails to one project's directory; this one widens the window to the whole
//! ADI store — projects, tasks, agents, dashboards, hive — and no further. Everything the app
//! owns is reachable; the user's home, their keys, and the rest of the disk are not.

use adi_fs::Jail;
use adi_projects::Projects;

use crate::types::{FsContent, FsCreate, FsListing, FsRef, FsWrite};

use super::files::{MAX_TEXT_BYTES, normalize_rel, parent_rel};
use super::response::{Response, error, ok_json};

/// `POST /api/fs/list` — list a directory inside the ADI store. `path` is relative to the store
/// root (`""` is the root).
#[must_use]
pub fn fs_list(store: &Projects, body: &[u8]) -> Response {
    let Some(req) = parse_ref(body) else {
        return bad_ref();
    };
    let jail = store_jail(store);
    match jail.list(&req.path) {
        Ok(entries) => {
            let path = normalize_rel(&req.path);
            let parent = parent_rel(&path);
            ok_json(&FsListing {
                path,
                parent,
                entries: entries.into_iter().map(super::files::file_entry).collect(),
            })
        }
        Err(e) => Response::from(&e),
    }
}

/// `POST /api/fs/read` — read one text file inside the ADI store. Binary files and files over
/// [`MAX_TEXT_BYTES`] are refused rather than returned.
#[must_use]
pub fn fs_read(store: &Projects, body: &[u8]) -> Response {
    let Some(req) = parse_ref(body) else {
        return bad_ref();
    };
    if req.path.trim().is_empty() {
        return error(400, "a file path is required");
    }
    read_content(&store_jail(store), &req.path)
}

/// `POST /api/fs/write` — atomically save one text file inside the ADI store, creating any
/// missing parents within it. Returns the fresh [`FsContent`] (re-read from disk) so the client
/// updates its size/modified in one round-trip.
#[must_use]
pub fn fs_write(store: &Projects, body: &[u8]) -> Response {
    let Some(req) = parse_write(body) else {
        return error(
            400,
            "expected JSON body { \"path\": \"…\", \"content\": \"…\" }",
        );
    };
    if req.content.len() as u64 > MAX_TEXT_BYTES {
        return error(
            413,
            &format!("file too large to save (max {MAX_TEXT_BYTES} bytes)"),
        );
    }
    let jail = store_jail(store);
    if let Err(e) = jail.write(&req.path, req.content.as_bytes()) {
        return Response::from(&e);
    }
    // Re-read so the response carries the authoritative size/modified after the write.
    read_content(&jail, &req.path)
}

/// `POST /api/fs/create` — create one empty file (`kind: "file"`) or directory (`kind: "dir"`)
/// inside the ADI store. Returns the fresh listing of the *parent* directory, so the tree that
/// asked for the create can redraw the folder it landed in without a second round-trip.
///
/// Creates never overwrite: a path that is already taken comes back 409.
#[must_use]
pub fn fs_create(store: &Projects, body: &[u8]) -> Response {
    let Some(req) = parse_create(body) else {
        return error(
            400,
            "expected JSON body { \"path\": \"…\", \"kind\": \"file\" | \"dir\" }",
        );
    };
    // Normalizing a path of only `.`/slashes empties it out, which would name the store root.
    let path = normalize_rel(&req.path);
    if path.is_empty() {
        return error(400, "a name is required");
    }
    let jail = store_jail(store);
    // The *raw* path goes to the jail, never the normalized one: `normalize_rel` strips `..`
    // components, so normalizing first would turn a climb the jail is meant to refuse into a
    // silent create somewhere else inside the store.
    let created = if req.kind == "dir" {
        jail.create_dir(&req.path)
    } else {
        jail.create_file(&req.path)
    };
    if let Err(e) = created {
        return Response::from(&e);
    }
    // The parent of a normalized non-empty path is `Some`, or the root — either way listable.
    let parent = parent_rel(&path).unwrap_or_default();
    match jail.list(&parent) {
        Ok(entries) => ok_json(&FsListing {
            path: parent.clone(),
            parent: parent_rel(&parent),
            entries: entries.into_iter().map(super::files::file_entry).collect(),
        }),
        Err(e) => Response::from(&e),
    }
}

/// Read `rel` as text and shape an [`FsContent`], enforcing the [`MAX_TEXT_BYTES`] cap.
fn read_content(jail: &Jail, rel: &str) -> Response {
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
        Ok(content) => ok_json(&FsContent {
            path: normalize_rel(rel),
            content,
            size: meta.size,
            modified: meta.modified,
        }),
        Err(e) => Response::from(&e),
    }
}

/// The jail rooted at the ADI store (`~/.adi/mono`, honoring `$ADI_DIR`). Taken from the same
/// [`adi_config`] store the rest of the API reads, so an alternate install stays consistent.
fn store_jail(store: &Projects) -> Jail {
    Jail::new(store.config().root().to_path_buf())
}

fn parse_ref(body: &[u8]) -> Option<FsRef> {
    // An empty body means the root — the panel's first load sends nothing to browse yet.
    if body.is_empty() {
        return Some(FsRef::default());
    }
    serde_json::from_slice(body).ok()
}

fn bad_ref() -> Response {
    error(400, "expected JSON body { \"path\"?: \"…\" }")
}

fn parse_write(body: &[u8]) -> Option<FsWrite> {
    let req: FsWrite = serde_json::from_slice(body).ok()?;
    (!req.path.trim().is_empty()).then_some(req)
}

fn parse_create(body: &[u8]) -> Option<FsCreate> {
    let req: FsCreate = serde_json::from_slice(body).ok()?;
    (!req.path.trim().is_empty()).then_some(req)
}
