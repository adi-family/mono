//! Images a person attached to a message: the `attachments` table, and the bytes beside it.
//!
//! ```text
//! <sessions_dir>/attachments/<id>.<ext>    the bytes, exactly as uploaded
//! ```
//!
//! **The extension is load-bearing**, not decoration. An engine that cannot be handed a picture in
//! its request body is handed this *path* instead and opens the file itself — and a file-reading
//! tool decides whether it is looking at an image by the name. `<id>` alone reads as a text file
//! with unprintable bytes in it.
//!
//! # Why the bytes are a file and the rest is a row
//!
//! For the same reason a session's log is a file: a blob is read whole, streamed straight back out
//! to whoever asked, and never queried. What *is* queried — what it is called, what type it is, how
//! big it is, whose message it ended up on — is a row, so a transcript can say "three images" and a
//! sweep can find the ones nobody ever sent without opening a single file.
//!
//! # An attachment outlives no session it was never attached to
//!
//! Uploading happens *before* the message that carries it exists — you paste a screenshot into an
//! empty composer, and the conversation it belongs to may never be started. So a row is born
//! **unclaimed** (no agent, no session), and [`claim`] stamps it with the conversation the moment a
//! turn actually records it. That is what makes the two cleanups possible: a deleted conversation
//! takes its images with it ([`delete_for_session`]), and an upload that was never sent is swept
//! after [`UNCLAIMED_TTL_MS`] ([`sweep_unclaimed`]) rather than sitting on disk for ever.
//!
//! There is no foreign key to `sessions` for exactly that reason — an unclaimed row has no session
//! to point at, and a cascade would have to invent one.

use std::path::{Path, PathBuf};

use rusqlite::{Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

use super::db::{now_ms, sql_err};

/// The largest image the store will take, in bytes.
///
/// A screenshot of a 6K display in PNG is comfortably under this; a photograph straight off a phone
/// usually is too. Past it the answer is a refusal with a readable reason rather than a request that
/// costs a provider round trip and comes back rejected there — every model API caps image size, and
/// the strictest of the four here is well above this.
pub const MAX_BYTES: usize = 5 << 20;

/// How long an upload that was never sent is kept before a sweep removes it.
///
/// A day, because the composer is a draft: a screenshot pasted this morning, left in the box, and
/// sent after lunch is ordinary. Anything older than that was abandoned — the tab was closed, the
/// message was rewritten — and the bytes are nobody's.
pub const UNCLAIMED_TTL_MS: u64 = 24 * 60 * 60 * 1000;

/// The image types a message may carry, each with the extension its file is given.
///
/// A closed list rather than "anything that starts with `image/`", because it is the *providers'*
/// list: these four are what Anthropic, `OpenAI`, Gemini and a local Ollama all accept. Taking an
/// AVIF here would mean accepting bytes that the send is guaranteed to fail on later, in a place
/// where the failure is no longer attached to the file that caused it.
const TYPES: [(&str, &str); 4] = [
    ("image/png", "png"),
    ("image/jpeg", "jpg"),
    ("image/webp", "webp"),
    ("image/gif", "gif"),
];

/// The image types a message may carry — the wire names, for error messages and validation.
pub const MEDIA_TYPES: [&str; 4] = ["image/png", "image/jpeg", "image/webp", "image/gif"];

/// The directory attachments live in, under the store's root.
const DIR: &str = "attachments";

/// One attached image, as a message carries it.
///
/// The bytes are *not* here: a turn is read on every poll of an open chat, and a transcript that
/// inlined its images would send them again every second. What a reader gets is enough to draw a
/// thumbnail and ask for the bytes once, by id.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Attachment {
    /// Opaque, minted by the store — never a filename. What a reader fetches the bytes by.
    pub id: String,
    /// What the file was called where it came from, for the reader's benefit only. A pasted
    /// screenshot has no name of its own and gets one from whoever uploaded it.
    #[serde(default)]
    pub name: String,
    /// One of [`MEDIA_TYPES`], and the thing a provider is told when the bytes go out.
    #[serde(default)]
    pub media_type: String,
    /// Bytes on disk. Carried so a reader can show the size without a second request.
    #[serde(default)]
    pub size: u64,
}

/// Where one attachment's bytes are.
///
/// Derived from the row rather than stored, so there is one rule for the name and no column that
/// can disagree with the file it points at.
#[must_use]
pub fn path(dir: &Path, attachment: &Attachment) -> PathBuf {
    dir.join(DIR)
        .join(format!("{}.{}", attachment.id, extension(&attachment.media_type)))
}

/// The extension a media type's file is given. `bin` for anything unknown, which [`put`] refuses
/// before it can ever be written — so that arm exists only to keep this total.
#[must_use]
pub fn extension(media_type: &str) -> &'static str {
    TYPES
        .iter()
        .find(|(mime, _)| *mime == media_type)
        .map_or("bin", |(_, ext)| *ext)
}

/// Whether `media_type` is one this store takes.
#[must_use]
pub fn is_supported(media_type: &str) -> bool {
    MEDIA_TYPES.contains(&media_type)
}

/// Store `bytes` and return the attachment that now refers to them.
///
/// The file is written **before** the row, and the row is what makes it findable: a write that
/// fails leaves nothing behind, and a row that fails to insert takes its file with it. The reverse
/// order would leave a row pointing at bytes that were never written, which every later read has to
/// defend against.
///
/// # Errors
/// Returns [`Error::Arguments`] for an unsupported type or an over-[`MAX_BYTES`] body, and I/O or
/// database errors.
pub(super) fn put(
    conn: &Connection,
    dir: &Path,
    name: &str,
    media_type: &str,
    bytes: &[u8],
) -> Result<Attachment> {
    if !is_supported(media_type) {
        return Err(Error::Arguments(format!(
            "{media_type} isn't an image type a message can carry — {} are",
            MEDIA_TYPES.join(", ")
        )));
    }
    if bytes.is_empty() {
        return Err(Error::Arguments("that image is empty".to_string()));
    }
    if bytes.len() > MAX_BYTES {
        return Err(Error::Arguments(format!(
            "that image is {} bytes, over the {MAX_BYTES}-byte limit",
            bytes.len()
        )));
    }
    // Minted by SQLite rather than by this process: two processes uploading at the same instant
    // must not be able to agree on an id, and a counter or a timestamp lets them.
    let id: String = conn
        .query_row("SELECT lower(hex(randomblob(12)))", [], |row| row.get(0))
        .map_err(|e| sql_err("mint an attachment id in", e))?;

    let size = bytes.len() as u64;
    let stored = Attachment {
        id,
        name: name.to_string(),
        media_type: media_type.to_string(),
        size,
    };
    let file = path(dir, &stored);
    if let Some(parent) = file.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&file, bytes)?;

    if let Err(e) = conn.execute(
        "INSERT INTO attachments (id, name, media_type, size, created_at) \
         VALUES (?1, ?2, ?3, ?4, ?5)",
        rusqlite::params![stored.id, stored.name, stored.media_type, size, now_ms()],
    ) {
        let _ = std::fs::remove_file(&file);
        return Err(sql_err("record an attachment in", e));
    }
    Ok(stored)
}

/// One attachment by id, or `None` when there is no such row.
#[must_use]
pub(super) fn get(conn: &Connection, id: &str) -> Option<Attachment> {
    conn.query_row(
        "SELECT id, name, media_type, size FROM attachments WHERE id = ?1",
        [id],
        |row| {
            Ok(Attachment {
                id: row.get(0)?,
                name: row.get(1)?,
                media_type: row.get(2)?,
                size: row.get::<_, i64>(3)?.unsigned_abs(),
            })
        },
    )
    .optional()
    .ok()
    .flatten()
}

/// The rows for `ids`, in the order asked, silently dropping any that no longer exist.
///
/// Dropping rather than failing is deliberate: the ids come from a client that uploaded them a
/// moment ago, and one that was swept between the upload and the send is a message with one fewer
/// image — not a message that cannot be sent.
#[must_use]
pub(super) fn resolve(conn: &Connection, ids: &[String]) -> Vec<Attachment> {
    ids.iter().filter_map(|id| get(conn, id)).collect()
}

/// Stamp these attachments as belonging to one conversation — what recording the turn that carries
/// them does.
///
/// Best-effort by design: an id that names no row is one that was swept, and the turn holding it
/// renders as a missing image rather than failing to be recorded at all.
pub(super) fn claim(conn: &Connection, agent: &str, session: &str, ids: &[String]) {
    for id in ids {
        let _ = conn.execute(
            "UPDATE attachments SET agent = ?1, session = ?2 WHERE id = ?3",
            rusqlite::params![agent, session, id],
        );
    }
}

/// Remove every attachment claimed by one conversation, bytes and rows — what deleting it does.
///
/// Returns how many were removed.
pub(super) fn delete_for_session(conn: &Connection, dir: &Path, agent: &str, session: &str) -> usize {
    let rows = rows_where(
        conn,
        "SELECT id, media_type FROM attachments WHERE agent = ?1 AND session = ?2",
        rusqlite::params![agent, session],
    );
    remove(conn, dir, &rows)
}

/// Remove uploads nobody ever sent, older than [`UNCLAIMED_TTL_MS`]. Returns how many went.
///
/// Called on the way in to an upload rather than on a timer: the only thing that creates orphans is
/// uploading, so the cheapest place to notice them is the next upload. One indexed-ish scan over a
/// table that holds a handful of rows in the normal case.
pub(super) fn sweep_unclaimed(conn: &Connection, dir: &Path, now: u64) -> usize {
    let cutoff = now.saturating_sub(UNCLAIMED_TTL_MS);
    let rows = rows_where(
        conn,
        "SELECT id, media_type FROM attachments WHERE session = '' AND created_at < ?1",
        rusqlite::params![cutoff],
    );
    remove(conn, dir, &rows)
}

/// The rows one query selects, or nothing at all if it fails — every caller here is a cleanup, and a
/// cleanup that cannot read is a cleanup that does nothing.
///
/// Whole rows rather than ids, because the file's name is derived from the media type: an id alone
/// does not say what to unlink.
fn rows_where(conn: &Connection, sql: &str, params: &[&dyn rusqlite::ToSql]) -> Vec<Attachment> {
    let Ok(mut stmt) = conn.prepare(sql) else {
        return Vec::new();
    };
    stmt.query_map(params, |row| {
        Ok(Attachment {
            id: row.get(0)?,
            name: String::new(),
            media_type: row.get(1)?,
            size: 0,
        })
    })
    .map(|rows| rows.flatten().collect())
    .unwrap_or_default()
}

/// Delete these attachments' bytes and rows. The file goes first: a row left behind by a failed
/// unlink is findable and can be swept again, while a file left behind by a deleted row is not.
fn remove(conn: &Connection, dir: &Path, rows: &[Attachment]) -> usize {
    let mut gone = 0;
    for row in rows {
        let _ = std::fs::remove_file(path(dir, row));
        if conn
            .execute("DELETE FROM attachments WHERE id = ?1", [&row.id])
            .is_ok()
        {
            gone += 1;
        }
    }
    gone
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> (tempdir::Dir, rusqlite::Connection) {
        let dir = tempdir::Dir::new("attachments");
        let conn = Connection::open_in_memory().expect("memory db");
        conn.execute_batch(super::super::db::SCHEMA).expect("schema");
        (dir, conn)
    }

    /// A scratch directory that removes itself — the store's own tests use `temp_dir` by hand, and
    /// this is the same idea kept local to the module.
    mod tempdir {
        use std::path::{Path, PathBuf};

        pub struct Dir(PathBuf);

        impl Dir {
            pub fn new(tag: &str) -> Self {
                let path = std::env::temp_dir().join(format!(
                    "adi-attach-{tag}-{}-{:?}",
                    std::process::id(),
                    std::thread::current().id(),
                ));
                let _ = std::fs::remove_dir_all(&path);
                std::fs::create_dir_all(&path).expect("scratch dir");
                Self(path)
            }

            pub fn path(&self) -> &Path {
                &self.0
            }
        }

        impl Drop for Dir {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
    }

    #[test]
    fn stored_bytes_come_back_by_id() {
        let (dir, conn) = store();
        let saved = put(&conn, dir.path(), "shot.png", "image/png", b"PNG-ish").expect("put");
        assert_eq!(saved.size, 7);
        assert_eq!(saved.media_type, "image/png");
        assert_eq!(
            std::fs::read(path(dir.path(), &saved)).expect("bytes"),
            b"PNG-ish"
        );
        // The file carries the type's own extension, which is what lets an engine that opens it by
        // path recognise it as a picture rather than as bytes.
        assert!(
            path(dir.path(), &saved).ends_with(format!("{}.png", saved.id)),
            "{:?}",
            path(dir.path(), &saved),
        );
        assert_eq!(get(&conn, &saved.id).expect("row").name, "shot.png");
    }

    /// The two refusals that must happen *here*, before any bytes are written: a type no provider
    /// takes, and a body over the cap.
    #[test]
    fn an_unsupported_type_or_an_oversized_body_is_refused() {
        let (dir, conn) = store();
        assert!(put(&conn, dir.path(), "a.pdf", "application/pdf", b"%PDF").is_err());
        let huge = vec![0u8; MAX_BYTES + 1];
        assert!(put(&conn, dir.path(), "big.png", "image/png", &huge).is_err());
        assert!(put(&conn, dir.path(), "empty.png", "image/png", b"").is_err());
    }

    /// Claiming is what makes a conversation's delete take its images with it — and what keeps the
    /// sweep off the ones that were actually sent.
    #[test]
    fn a_claimed_attachment_belongs_to_its_conversation_and_an_unclaimed_one_is_swept() {
        let (dir, conn) = store();
        let sent = put(&conn, dir.path(), "sent.png", "image/png", b"a").expect("put");
        let abandoned = put(&conn, dir.path(), "draft.png", "image/png", b"b").expect("put");
        claim(&conn, "chatty", "conv-1", std::slice::from_ref(&sent.id));

        // Now, with nothing old: the sent one is claimed and the draft is too young to sweep.
        assert_eq!(sweep_unclaimed(&conn, dir.path(), now_ms()), 0);

        // A day and a second later, the draft is nobody's.
        assert_eq!(
            sweep_unclaimed(&conn, dir.path(), now_ms() + UNCLAIMED_TTL_MS + 1000),
            1
        );
        assert!(get(&conn, &abandoned.id).is_none());
        assert!(get(&conn, &sent.id).is_some());

        assert_eq!(delete_for_session(&conn, dir.path(), "chatty", "conv-1"), 1);
        assert!(get(&conn, &sent.id).is_none());
        assert!(!path(dir.path(), &sent).exists());
    }

    /// An id that no longer names a row is one fewer image, not a failure — see [`resolve`].
    #[test]
    fn resolving_drops_what_is_gone_and_keeps_the_order() {
        let (dir, conn) = store();
        let one = put(&conn, dir.path(), "1.png", "image/png", b"1").expect("put");
        let two = put(&conn, dir.path(), "2.png", "image/png", b"2").expect("put");
        let ids = vec![two.id.clone(), "nope".to_string(), one.id.clone()];
        let names: Vec<String> = resolve(&conn, &ids).into_iter().map(|a| a.name).collect();
        assert_eq!(names, ["2.png", "1.png"]);
    }
}
