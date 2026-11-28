//! The relay's local SQLite store — one `rusqlite::Connection`, guarded by a `Mutex` (a
//! self-hostable relay's write volume never justifies a pool), with the same WAL pragmas
//! `adi-db`'s own doc gives for the same reason (readers never block the writer), duplicated
//! here rather than depending on that crate: a self-hosting customer has neither `adi-db` nor
//! `~/.adi/mono/` installed, and this store must not assume either.

use std::path::Path;
use std::sync::Mutex;
use std::time::Duration;

use adi_relay::seal::Sealed;
use rusqlite::{Connection, OptionalExtension as _, params};

const BUSY_TIMEOUT: Duration = Duration::from_millis(5000);

/// Same order as `adi-db`'s own `PRAGMAS`, and the same reason: `busy_timeout` first, because
/// setting `journal_mode` takes a lock that would otherwise fail instantly with none in effect.
const PRAGMAS: &str = "\
    pragma busy_timeout = 5000;\n\
    pragma journal_mode = WAL;\n\
    pragma foreign_keys = ON;\n\
    pragma synchronous = NORMAL;\n";

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS endpoints (
  id                 TEXT PRIMARY KEY,
  owner_fingerprint  TEXT NOT NULL,
  owner_key          TEXT NOT NULL,
  sealing_key        TEXT NOT NULL,
  retention_secs     INTEGER NOT NULL,
  paused             INTEGER NOT NULL DEFAULT 0,
  created_at         INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_endpoints_owner ON endpoints(owner_fingerprint);

CREATE TABLE IF NOT EXISTS records (
  id                INTEGER PRIMARY KEY AUTOINCREMENT,
  endpoint_id       TEXT NOT NULL REFERENCES endpoints(id),
  received_at       INTEGER NOT NULL,
  expires_at        INTEGER NOT NULL,
  byte_len          INTEGER NOT NULL,
  ephemeral_pubkey  BLOB NOT NULL,
  nonce             BLOB NOT NULL,
  ciphertext        BLOB NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_records_endpoint ON records(endpoint_id);
CREATE INDEX IF NOT EXISTS idx_records_expires  ON records(expires_at);

CREATE TABLE IF NOT EXISTS nonces (
  owner_fingerprint TEXT NOT NULL,
  nonce             TEXT NOT NULL,
  expires_at        INTEGER NOT NULL,
  PRIMARY KEY (owner_fingerprint, nonce)
);

-- `licence_code` is not in the task's own original schema sketch — added here, a deliberate
-- deviation: the daily recheck sweep has to beam the licence service again, and the beam's wire
-- contract requires the code on every call, so it has to be remembered somewhere between
-- register and the next recheck.
CREATE TABLE IF NOT EXISTS licence_status (
  owner_fingerprint TEXT PRIMARY KEY,
  licence_code      TEXT NOT NULL,
  valid             INTEGER NOT NULL,
  checked_at        INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS registrations_by_ip (
  ip    TEXT NOT NULL,
  day   TEXT NOT NULL,
  count INTEGER NOT NULL,
  PRIMARY KEY (ip, day)
);
";

#[derive(Debug)]
pub struct Store {
    conn: Mutex<Connection>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EndpointRow {
    pub id: String,
    pub owner_fingerprint: String,
    pub sealing_key_b64: String,
    pub retention_secs: u64,
    pub paused: bool,
    pub created_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EndpointSummary {
    pub endpoint_id: String,
    pub retention_secs: u64,
    pub paused: bool,
    pub created_at: i64,
    pub pending: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PulledRecord {
    /// The row id `insert_sealed_record` sealed the AAD against (`adi_relay::seal::aad`) — the
    /// opener needs it back to rebuild the same AAD, so it rides the wire too (a deliberate
    /// deviation from the pull response's first draft, which omitted it).
    pub id: i64,
    pub endpoint_id: String,
    pub received_at: i64,
    pub sealed: Sealed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeleteOutcome {
    Deleted,
    Forbidden,
    NotFound,
}

impl Store {
    /// Open (creating if absent) the database at `path`, applying the WAL pragmas and schema.
    ///
    /// # Errors
    /// Any `rusqlite` error opening the connection or running the schema.
    pub fn open(path: &Path) -> rusqlite::Result<Self> {
        let conn = Connection::open(path)?;
        conn.busy_timeout(BUSY_TIMEOUT)?;
        conn.execute_batch(PRAGMAS)?;
        conn.execute_batch(SCHEMA)?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    /// An in-memory store, for tests — same schema, no file.
    ///
    /// # Errors
    /// Any `rusqlite` error.
    pub fn open_in_memory() -> rusqlite::Result<Self> {
        let conn = Connection::open_in_memory()?;
        conn.execute_batch(SCHEMA)?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Connection> {
        self.conn
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    // ---- endpoints -------------------------------------------------------------------

    /// How many endpoints `owner_fingerprint` already owns on this relay — the `register`-time
    /// check against the mode's `max_endpoints_per_key`.
    ///
    /// # Errors
    /// Any `rusqlite` error.
    pub fn count_endpoints(&self, owner_fingerprint: &str) -> rusqlite::Result<i64> {
        self.lock().query_row(
            "SELECT COUNT(*) FROM endpoints WHERE owner_fingerprint = ?1",
            params![owner_fingerprint],
            |row| row.get(0),
        )
    }

    /// Insert a freshly minted endpoint. The caller mints `id` (unguessable, the relay's own
    /// namespace) and passes it in rather than this function generating one, so a retry on a
    /// rare id collision is the caller's loop to run, not this function's.
    ///
    /// # Errors
    /// Any `rusqlite` error, including a primary-key collision on `id`.
    pub fn insert_endpoint(
        &self,
        id: &str,
        owner_fingerprint: &str,
        owner_key: &str,
        sealing_key_b64: &str,
        retention_secs: u64,
        created_at: i64,
    ) -> rusqlite::Result<()> {
        self.lock().execute(
            "INSERT INTO endpoints (id, owner_fingerprint, owner_key, sealing_key, retention_secs, paused, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, 0, ?6)",
            params![id, owner_fingerprint, owner_key, sealing_key_b64, retention_secs, created_at],
        )?;
        Ok(())
    }

    /// Does `id` already exist? Used by `register`'s id-collision retry loop.
    ///
    /// # Errors
    /// Any `rusqlite` error.
    pub fn endpoint_exists(&self, id: &str) -> rusqlite::Result<bool> {
        self.lock()
            .query_row("SELECT 1 FROM endpoints WHERE id = ?1", params![id], |_| {
                Ok(())
            })
            .optional()
            .map(|r| r.is_some())
    }

    /// Look up one endpoint by id, for the sender-ingress path and `delete`'s ownership check.
    ///
    /// # Errors
    /// Any `rusqlite` error.
    pub fn get_endpoint(&self, id: &str) -> rusqlite::Result<Option<EndpointRow>> {
        self.lock()
            .query_row(
                "SELECT id, owner_fingerprint, sealing_key, retention_secs, paused, created_at
                 FROM endpoints WHERE id = ?1",
                params![id],
                |row| {
                    Ok(EndpointRow {
                        id: row.get(0)?,
                        owner_fingerprint: row.get(1)?,
                        sealing_key_b64: row.get(2)?,
                        retention_secs: row.get::<_, i64>(3)?.max(0).cast_unsigned(),
                        paused: row.get::<_, i64>(4)? != 0,
                        created_at: row.get(5)?,
                    })
                },
            )
            .optional()
    }

    /// Every endpoint `owner_fingerprint` owns, with its pending-record count — `GET
    /// /v1/endpoints`.
    ///
    /// # Errors
    /// Any `rusqlite` error.
    pub fn list_endpoints(
        &self,
        owner_fingerprint: &str,
    ) -> rusqlite::Result<Vec<EndpointSummary>> {
        let conn = self.lock();
        let mut stmt = conn.prepare(
            "SELECT e.id, e.retention_secs, e.paused, e.created_at,
                    (SELECT COUNT(*) FROM records r WHERE r.endpoint_id = e.id)
             FROM endpoints e WHERE e.owner_fingerprint = ?1
             ORDER BY e.created_at ASC",
        )?;
        let rows = stmt.query_map(params![owner_fingerprint], |row| {
            Ok(EndpointSummary {
                endpoint_id: row.get(0)?,
                retention_secs: row.get::<_, i64>(1)?.max(0).cast_unsigned(),
                paused: row.get::<_, i64>(2)? != 0,
                created_at: row.get(3)?,
                pending: row.get(4)?,
            })
        })?;
        rows.collect()
    }

    /// Delete an endpoint and every one of its queued records — `Forbidden` if
    /// `owner_fingerprint` doesn't match the recorded owner, `NotFound` if the id never existed.
    ///
    /// # Errors
    /// Any `rusqlite` error.
    pub fn delete_endpoint(
        &self,
        id: &str,
        owner_fingerprint: &str,
    ) -> rusqlite::Result<DeleteOutcome> {
        let conn = self.lock();
        let owner: Option<String> = conn
            .query_row(
                "SELECT owner_fingerprint FROM endpoints WHERE id = ?1",
                params![id],
                |row| row.get(0),
            )
            .optional()?;
        match owner {
            None => Ok(DeleteOutcome::NotFound),
            Some(owner) if owner != owner_fingerprint => Ok(DeleteOutcome::Forbidden),
            Some(_) => {
                conn.execute("DELETE FROM records WHERE endpoint_id = ?1", params![id])?;
                conn.execute("DELETE FROM endpoints WHERE id = ?1", params![id])?;
                Ok(DeleteOutcome::Deleted)
            }
        }
    }

    /// Pause or unpause every endpoint `owner_fingerprint` owns — what a licence-status flip
    /// does: a lapsed licence pauses every endpoint that fingerprint owns.
    ///
    /// # Errors
    /// Any `rusqlite` error.
    pub fn set_endpoints_paused(
        &self,
        owner_fingerprint: &str,
        paused: bool,
    ) -> rusqlite::Result<()> {
        self.lock().execute(
            "UPDATE endpoints SET paused = ?1 WHERE owner_fingerprint = ?2",
            params![i64::from(paused), owner_fingerprint],
        )?;
        Ok(())
    }

    // ---- records (ingress, pull) -----------------------------------------------------

    /// How many records are already queued for `endpoint_id` — the per-endpoint pending-record
    /// cap, checked at ingress before accepting one more.
    ///
    /// # Errors
    /// Any `rusqlite` error.
    pub fn pending_count(&self, endpoint_id: &str) -> rusqlite::Result<i64> {
        self.lock().query_row(
            "SELECT COUNT(*) FROM records WHERE endpoint_id = ?1",
            params![endpoint_id],
            |row| row.get(0),
        )
    }

    /// How many records `endpoint_id` has accepted since `since` (unix seconds) — the ingress
    /// rate limit (`max_requests_per_minute_per_endpoint`). The schema has no dedicated counter
    /// table for this; counting already-accepted records over the trailing
    /// window is free (no new table, no separate sweep) and is exactly the quantity the limit
    /// means: how many deliveries this endpoint has actually taken recently.
    ///
    /// # Errors
    /// Any `rusqlite` error.
    pub fn records_since(&self, endpoint_id: &str, since: i64) -> rusqlite::Result<i64> {
        self.lock().query_row(
            "SELECT COUNT(*) FROM records WHERE endpoint_id = ?1 AND received_at >= ?2",
            params![endpoint_id, since],
            |row| row.get(0),
        )
    }

    /// Seal `plaintext` to `sealing_public` and store it for `endpoint_id` — the sender-ingress
    /// path. Sealing happens inside the same transaction as the insert because the AAD binds the
    /// ciphertext to its own row id, which SQLite only assigns at insert time:
    /// a placeholder row is inserted first, then updated with the sealed bytes computed from the
    /// row id `last_insert_rowid()` just returned — both statements commit atomically, so there
    /// is no window where a half-sealed row is visible to a concurrent `pull`.
    ///
    /// # Errors
    /// Any `rusqlite` error.
    pub fn insert_sealed_record(
        &self,
        endpoint_id: &str,
        sealing_public: &[u8; 32],
        received_at: i64,
        expires_at: i64,
        byte_len: i64,
        plaintext: &[u8],
    ) -> rusqlite::Result<()> {
        let conn = self.lock();
        conn.execute(
            "INSERT INTO records (endpoint_id, received_at, expires_at, byte_len, ephemeral_pubkey, nonce, ciphertext)
             VALUES (?1, ?2, ?3, ?4, x'', x'', x'')",
            params![endpoint_id, received_at, expires_at, byte_len],
        )?;
        let record_id = conn.last_insert_rowid();
        let sealed = adi_relay::seal::seal(sealing_public, endpoint_id, record_id, plaintext);
        conn.execute(
            "UPDATE records SET ephemeral_pubkey = ?1, nonce = ?2, ciphertext = ?3 WHERE id = ?4",
            params![
                sealed.ephemeral_pubkey.as_slice(),
                sealed.nonce.as_slice(),
                sealed.ciphertext,
                record_id
            ],
        )?;
        Ok(())
    }

    /// Delete-at-handout: atomically take up to `batch_max` queued records across every
    /// *unpaused* endpoint `owner_fingerprint` owns, oldest first, and return them — gone from
    /// the store the instant this call returns, by construction (`DELETE … RETURNING`), not by a
    /// second step the caller has to remember to run.
    ///
    /// # Errors
    /// Any `rusqlite` error.
    pub fn pull(
        &self,
        owner_fingerprint: &str,
        batch_max: u32,
    ) -> rusqlite::Result<Vec<PulledRecord>> {
        let conn = self.lock();
        let mut stmt = conn.prepare(
            "DELETE FROM records WHERE id IN (
                SELECT r.id FROM records r JOIN endpoints e ON e.id = r.endpoint_id
                WHERE e.owner_fingerprint = ?1 AND e.paused = 0
                ORDER BY r.id ASC LIMIT ?2
             )
             RETURNING id, endpoint_id, received_at, ephemeral_pubkey, nonce, ciphertext",
        )?;
        let rows = stmt.query_map(params![owner_fingerprint, batch_max], |row| {
            Ok(PulledRecord {
                id: row.get(0)?,
                endpoint_id: row.get(1)?,
                received_at: row.get(2)?,
                sealed: Sealed {
                    ephemeral_pubkey: row.get::<_, Vec<u8>>(3)?.try_into().unwrap_or_default(),
                    nonce: row.get::<_, Vec<u8>>(4)?.try_into().unwrap_or_default(),
                    ciphertext: row.get(5)?,
                },
            })
        })?;
        rows.collect()
    }

    // ---- nonces ------------------------------------------------------------------------

    /// Remember `nonce` for `owner_fingerprint`, expiring at `expires_at` — `Ok(true)` if it was
    /// fresh (and is now remembered), `Ok(false)` if it had already been seen inside the window
    /// (the replay check).
    ///
    /// # Errors
    /// Any `rusqlite` error other than the expected unique-constraint violation on replay.
    pub fn remember_nonce(
        &self,
        owner_fingerprint: &str,
        nonce: &str,
        expires_at: i64,
    ) -> rusqlite::Result<bool> {
        match self.lock().execute(
            "INSERT INTO nonces (owner_fingerprint, nonce, expires_at) VALUES (?1, ?2, ?3)",
            params![owner_fingerprint, nonce, expires_at],
        ) {
            Ok(_) => Ok(true),
            Err(rusqlite::Error::SqliteFailure(e, _))
                if e.code == rusqlite::ErrorCode::ConstraintViolation =>
            {
                Ok(false)
            }
            Err(e) => Err(e),
        }
    }

    // ---- licence status (enterprise mode) ----------------------------------------------

    /// # Errors
    /// Any `rusqlite` error.
    pub fn get_licence_status(
        &self,
        owner_fingerprint: &str,
    ) -> rusqlite::Result<Option<(bool, i64)>> {
        self.lock()
            .query_row(
                "SELECT valid, checked_at FROM licence_status WHERE owner_fingerprint = ?1",
                params![owner_fingerprint],
                |row| Ok((row.get::<_, i64>(0)? != 0, row.get(1)?)),
            )
            .optional()
    }

    /// `licence_code` is remembered alongside the verdict so the next recheck sweep can beam the
    /// licence service again without asking the node for it a second time — see this module's
    /// own schema comment.
    ///
    /// # Errors
    /// Any `rusqlite` error.
    pub fn set_licence_status(
        &self,
        owner_fingerprint: &str,
        licence_code: &str,
        valid: bool,
        checked_at: i64,
    ) -> rusqlite::Result<()> {
        self.lock().execute(
            "INSERT INTO licence_status (owner_fingerprint, licence_code, valid, checked_at) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(owner_fingerprint) DO UPDATE SET licence_code = ?2, valid = ?3, checked_at = ?4",
            params![owner_fingerprint, licence_code, i64::from(valid), checked_at],
        )?;
        Ok(())
    }

    /// Every `(owner_fingerprint, licence_code)` with a cached verdict — what the daily recheck
    /// sweep iterates.
    ///
    /// # Errors
    /// Any `rusqlite` error.
    pub fn all_licence_rows(&self) -> rusqlite::Result<Vec<(String, String)>> {
        let conn = self.lock();
        let mut stmt =
            conn.prepare("SELECT owner_fingerprint, licence_code FROM licence_status")?;
        let rows = stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
        rows.collect()
    }

    // ---- registrations by ip (public mode) ---------------------------------------------

    /// # Errors
    /// Any `rusqlite` error.
    pub fn registrations_today(&self, ip: &str, day: &str) -> rusqlite::Result<i64> {
        self.lock()
            .query_row(
                "SELECT count FROM registrations_by_ip WHERE ip = ?1 AND day = ?2",
                params![ip, day],
                |row| row.get(0),
            )
            .optional()
            .map(|v| v.unwrap_or(0))
    }

    /// # Errors
    /// Any `rusqlite` error.
    pub fn record_registration(&self, ip: &str, day: &str) -> rusqlite::Result<()> {
        self.lock().execute(
            "INSERT INTO registrations_by_ip (ip, day, count) VALUES (?1, ?2, 1)
             ON CONFLICT(ip, day) DO UPDATE SET count = count + 1",
            params![ip, day],
        )?;
        Ok(())
    }

    // ---- retention / sweep ----------------------------------------------------------

    /// Purge every record whose `expires_at` has passed. Returns how many were removed.
    ///
    /// # Errors
    /// Any `rusqlite` error.
    pub fn purge_expired_records(&self, now: i64) -> rusqlite::Result<usize> {
        self.lock()
            .execute("DELETE FROM records WHERE expires_at < ?1", params![now])
    }

    /// Purge nonce rows past their skew window. Returns how many were removed.
    ///
    /// # Errors
    /// Any `rusqlite` error.
    pub fn purge_expired_nonces(&self, now: i64) -> rusqlite::Result<usize> {
        self.lock()
            .execute("DELETE FROM nonces WHERE expires_at < ?1", params![now])
    }

    /// Purge `registrations_by_ip` rows older than `before_day` (2 days, for the rolling
    /// per-IP daily cap).
    ///
    /// # Errors
    /// Any `rusqlite` error.
    pub fn purge_old_registrations(&self, before_day: &str) -> rusqlite::Result<usize> {
        self.lock().execute(
            "DELETE FROM registrations_by_ip WHERE day < ?1",
            params![before_day],
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> Store {
        Store::open_in_memory().expect("open")
    }

    fn node() -> ([u8; 32], [u8; 32]) {
        // (sealing_public, sealing_secret) for round-tripping a sealed record in tests.
        let secret = x25519_dalek::StaticSecret::from([4u8; 32]);
        let public = *x25519_dalek::PublicKey::from(&secret).as_bytes();
        (public, secret.to_bytes())
    }

    #[test]
    fn registering_and_listing_round_trips() {
        let s = store();
        s.insert_endpoint("wh_a", "fp1", "ssh-ed25519 AAA", "sealkey", 300, 1000)
            .expect("insert");
        assert_eq!(s.count_endpoints("fp1").expect("count"), 1);
        assert_eq!(s.count_endpoints("fp-other").expect("count"), 0);

        let got = s.get_endpoint("wh_a").expect("get").expect("present");
        assert_eq!(got.owner_fingerprint, "fp1");
        assert_eq!(got.retention_secs, 300);
        assert!(!got.paused);

        let list = s.list_endpoints("fp1").expect("list");
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].endpoint_id, "wh_a");
        assert_eq!(list[0].pending, 0);
    }

    #[test]
    fn delete_checks_ownership_before_removing_anything() {
        let s = store();
        s.insert_endpoint("wh_a", "fp1", "k", "seal", 300, 1000)
            .unwrap();
        assert_eq!(
            s.delete_endpoint("wh_a", "fp-other").unwrap(),
            DeleteOutcome::Forbidden
        );
        assert!(
            s.get_endpoint("wh_a").unwrap().is_some(),
            "forbidden delete must not remove the row"
        );
        assert_eq!(
            s.delete_endpoint("wh_missing", "fp1").unwrap(),
            DeleteOutcome::NotFound
        );
        assert_eq!(
            s.delete_endpoint("wh_a", "fp1").unwrap(),
            DeleteOutcome::Deleted
        );
        assert!(s.get_endpoint("wh_a").unwrap().is_none());
    }

    #[test]
    fn delete_removes_queued_records_too() {
        let (sealing_public, _secret) = node();
        let s = store();
        s.insert_endpoint("wh_a", "fp1", "k", "seal", 300, 1000)
            .unwrap();
        s.insert_sealed_record("wh_a", &sealing_public, 1000, 2000, 5, b"hi")
            .unwrap();
        s.delete_endpoint("wh_a", "fp1").unwrap();
        // The record's endpoint is gone; pulling for fp1 must come back empty, not error.
        assert!(s.pull("fp1", 10).unwrap().is_empty());
    }

    #[test]
    fn insert_then_pull_round_trips_through_the_seal() {
        let (sealing_public, sealing_secret) = node();
        let secret = x25519_dalek::StaticSecret::from(sealing_secret);
        let s = store();
        s.insert_endpoint("wh_a", "fp1", "k", "seal", 300, 1000)
            .unwrap();
        s.insert_sealed_record("wh_a", &sealing_public, 1_111, 9_999, 11, b"hello webhook")
            .unwrap();

        let pulled = s.pull("fp1", 10).expect("pull");
        assert_eq!(pulled.len(), 1);
        assert_eq!(pulled[0].endpoint_id, "wh_a");
        assert_eq!(pulled[0].received_at, 1_111);

        let opened =
            adi_relay::seal::open(&secret, "wh_a", pulled[0].id, &pulled[0].sealed).expect("open");
        assert_eq!(&*opened, b"hello webhook");
    }

    /// A deliberate deviation from the pull response's first draft: `id` has to ride the wire,
    /// distinctly per record, because the opener needs it back to rebuild the AAD. Two records
    /// in one batch must come back with two different ids, each opening only with its own.
    #[test]
    fn pulled_records_in_one_batch_carry_distinct_ids() {
        let (sealing_public, sealing_secret) = node();
        let secret = x25519_dalek::StaticSecret::from(sealing_secret);
        let s = store();
        s.insert_endpoint("wh_a", "fp1", "k", "seal", 300, 1000)
            .unwrap();
        s.insert_sealed_record("wh_a", &sealing_public, 1, 9_999, 5, b"first")
            .unwrap();
        s.insert_sealed_record("wh_a", &sealing_public, 2, 9_999, 6, b"second")
            .unwrap();

        let pulled = s.pull("fp1", 10).expect("pull");
        assert_eq!(pulled.len(), 2);
        assert_ne!(pulled[0].id, pulled[1].id);

        let first = adi_relay::seal::open(&secret, "wh_a", pulled[0].id, &pulled[0].sealed)
            .expect("open first");
        assert_eq!(&*first, b"first");
        let second = adi_relay::seal::open(&secret, "wh_a", pulled[1].id, &pulled[1].sealed)
            .expect("open second");
        assert_eq!(&*second, b"second");
        // The AAD is bound per-record: the wrong id for either never opens.
        assert!(adi_relay::seal::open(&secret, "wh_a", pulled[1].id, &pulled[0].sealed).is_err());
    }

    #[test]
    fn pull_deletes_at_handout_and_a_second_pull_sees_nothing() {
        let (sealing_public, _secret) = node();
        let s = store();
        s.insert_endpoint("wh_a", "fp1", "k", "seal", 300, 1000)
            .unwrap();
        s.insert_sealed_record("wh_a", &sealing_public, 1, 2, 1, b"a")
            .unwrap();
        assert_eq!(s.pull("fp1", 10).unwrap().len(), 1);
        assert!(s.pull("fp1", 10).unwrap().is_empty());
    }

    #[test]
    fn pull_is_scoped_to_the_keys_endpoints_across_all_of_them() {
        let (sealing_public, _secret) = node();
        let s = store();
        s.insert_endpoint("wh_a", "fp1", "k", "seal", 300, 1000)
            .unwrap();
        s.insert_endpoint("wh_b", "fp1", "k", "seal", 300, 1000)
            .unwrap();
        s.insert_endpoint("wh_c", "fp2", "k", "seal", 300, 1000)
            .unwrap();
        s.insert_sealed_record("wh_a", &sealing_public, 1, 2, 1, b"a")
            .unwrap();
        s.insert_sealed_record("wh_b", &sealing_public, 1, 2, 1, b"b")
            .unwrap();
        s.insert_sealed_record("wh_c", &sealing_public, 1, 2, 1, b"c")
            .unwrap();

        let fp1 = s.pull("fp1", 10).unwrap();
        assert_eq!(fp1.len(), 2, "both of fp1's endpoints, in one call");
        let fp2 = s.pull("fp2", 10).unwrap();
        assert_eq!(fp2.len(), 1);
    }

    #[test]
    fn a_paused_endpoints_records_never_come_back_from_pull() {
        let (sealing_public, _secret) = node();
        let s = store();
        s.insert_endpoint("wh_a", "fp1", "k", "seal", 300, 1000)
            .unwrap();
        s.insert_sealed_record("wh_a", &sealing_public, 1, 2, 1, b"a")
            .unwrap();
        s.set_endpoints_paused("fp1", true).unwrap();
        assert!(
            s.pull("fp1", 10).unwrap().is_empty(),
            "paused endpoints are withheld from pull"
        );

        // Ingress still queues, though — the record is still there once unpaused.
        s.set_endpoints_paused("fp1", false).unwrap();
        assert_eq!(s.pull("fp1", 10).unwrap().len(), 1);
    }

    #[test]
    fn pull_batch_is_capped_and_oldest_first() {
        let (sealing_public, _secret) = node();
        let s = store();
        s.insert_endpoint("wh_a", "fp1", "k", "seal", 300, 1000)
            .unwrap();
        for i in 0..5 {
            s.insert_sealed_record("wh_a", &sealing_public, i, i + 100, 1, b"x")
                .unwrap();
        }
        let first = s.pull("fp1", 2).expect("pull");
        assert_eq!(first.len(), 2);
        assert_eq!(first[0].received_at, 0);
        assert_eq!(first[1].received_at, 1);
        assert_eq!(
            s.pull("fp1", 10).unwrap().len(),
            3,
            "the rest is still queued"
        );
    }

    #[test]
    fn nonce_replay_is_refused_once_remembered() {
        let s = store();
        assert!(s.remember_nonce("fp1", "n1", 1_000).unwrap());
        assert!(
            !s.remember_nonce("fp1", "n1", 1_000).unwrap(),
            "same key, same nonce"
        );
        // A different key may reuse the same nonce string — it's scoped per key.
        assert!(s.remember_nonce("fp2", "n1", 1_000).unwrap());
    }

    #[test]
    fn licence_status_round_trips_and_updates() {
        let s = store();
        assert_eq!(s.get_licence_status("fp1").unwrap(), None);
        s.set_licence_status("fp1", "ADI-ENT-X", true, 1_000)
            .unwrap();
        assert_eq!(s.get_licence_status("fp1").unwrap(), Some((true, 1_000)));
        s.set_licence_status("fp1", "ADI-ENT-X", false, 2_000)
            .unwrap();
        assert_eq!(s.get_licence_status("fp1").unwrap(), Some((false, 2_000)));
        assert_eq!(
            s.all_licence_rows().unwrap(),
            vec![("fp1".to_string(), "ADI-ENT-X".to_string())]
        );
    }

    #[test]
    fn registrations_by_ip_counts_and_resets_per_day() {
        let s = store();
        assert_eq!(s.registrations_today("1.2.3.4", "2026-11-13").unwrap(), 0);
        s.record_registration("1.2.3.4", "2026-11-13").unwrap();
        s.record_registration("1.2.3.4", "2026-11-13").unwrap();
        assert_eq!(s.registrations_today("1.2.3.4", "2026-11-13").unwrap(), 2);
        assert_eq!(s.registrations_today("1.2.3.4", "2026-11-14").unwrap(), 0);
    }

    #[test]
    fn purge_removes_only_whats_actually_expired() {
        let (sealing_public, _secret) = node();
        let s = store();
        s.insert_endpoint("wh_a", "fp1", "k", "seal", 300, 1000)
            .unwrap();
        s.insert_sealed_record("wh_a", &sealing_public, 1, 50, 1, b"old")
            .unwrap();
        s.insert_sealed_record("wh_a", &sealing_public, 1, 500, 1, b"new")
            .unwrap();
        assert_eq!(s.purge_expired_records(100).unwrap(), 1);
        assert_eq!(
            s.pull("fp1", 10).unwrap().len(),
            1,
            "only the unexpired one is left"
        );

        s.remember_nonce("fp1", "a", 50).unwrap();
        s.remember_nonce("fp1", "b", 500).unwrap();
        assert_eq!(s.purge_expired_nonces(100).unwrap(), 1);

        s.record_registration("1.1.1.1", "2026-11-01").unwrap();
        s.record_registration("1.1.1.1", "2026-11-13").unwrap();
        assert_eq!(s.purge_old_registrations("2026-11-11").unwrap(), 1);
    }

    #[test]
    fn pending_count_reflects_queued_records() {
        let (sealing_public, _secret) = node();
        let s = store();
        s.insert_endpoint("wh_a", "fp1", "k", "seal", 300, 1000)
            .unwrap();
        assert_eq!(s.pending_count("wh_a").unwrap(), 0);
        s.insert_sealed_record("wh_a", &sealing_public, 1, 2, 1, b"x")
            .unwrap();
        assert_eq!(s.pending_count("wh_a").unwrap(), 1);
    }

    #[test]
    fn endpoint_exists_tells_apart_present_and_absent_ids() {
        let s = store();
        assert!(!s.endpoint_exists("wh_a").unwrap());
        s.insert_endpoint("wh_a", "fp1", "k", "seal", 300, 1000)
            .unwrap();
        assert!(s.endpoint_exists("wh_a").unwrap());
    }
}
