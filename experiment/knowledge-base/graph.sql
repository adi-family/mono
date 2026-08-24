-- Knowledge base: flat facts, plus a derivation graph over them.
--
-- The graph answers exactly one question: when a fact changes, what that was built on it is
-- now out of date? It answers mechanically — no model, no similarity, no judgement.

PRAGMA foreign_keys = ON;

-- Everyone who can say or write something: a person, or an agent at a given version.
-- Identities repeat on almost every row — one operator and a handful of agents produce a whole
-- base — so they are stored once and referenced by integer. `nodes` carries two of them and
-- `history` a third; inlining the strings would cost more than the facts do.
CREATE TABLE IF NOT EXISTS actors (
    id   INTEGER PRIMARY KEY AUTOINCREMENT,
    name TEXT NOT NULL UNIQUE,            -- "igor", "agent:extractor@1"
    kind TEXT NOT NULL DEFAULT 'human'    -- 'human' | 'agent'
);

CREATE TABLE IF NOT EXISTS nodes (
    id         TEXT PRIMARY KEY,
    fact       TEXT    NOT NULL,          -- one plain sentence
    author     INTEGER NOT NULL REFERENCES actors(id),   -- whose meaning this is
    creator    INTEGER NOT NULL REFERENCES actors(id),   -- who wrote the record
    version    INTEGER NOT NULL DEFAULT 1,-- bumped on every edit. THIS is what edges compare.
    updated_at INTEGER NOT NULL,          -- wall clock, for humans only. Never load-bearing.
    kind       TEXT    NOT NULL DEFAULT 'fact'   -- 'fact' | 'composed' | 'artifact'
);

-- Read facts through this, not through `nodes` — it puts the names back.
CREATE VIEW IF NOT EXISTS facts_v AS
SELECT n.id, n.fact, a.name AS author, c.name AS creator, n.version, n.updated_at, n.kind
FROM   nodes n JOIN actors a ON a.id = n.author JOIN actors c ON c.id = n.creator;

-- A COMPOSED node is the one place facts are read as a group.
--
-- "We support all countries", "We do not support the CIS", "We support Ukraine" are three facts
-- that qualify each other. None is wrong, none supersedes another, and none is independent —
-- read alone, each of the first two is actively misleading. So they get composed into a fourth
-- node, "We support all countries except the CIS, though we do support Ukraine", derived from
-- all three by ordinary edges.
--
-- The atomic facts stay atomic, and stay separately editable. The composition is derived, so
-- when any part changes the existing staleness machinery marks it out of date and it is
-- regenerated. Nothing new is needed to keep it honest.
--
-- This is why the parts are NOT merged into one sentence: a merge would make "we do not support
-- the CIS" unaddressable, and the next reversal would have to be applied by editing prose.

-- An edge says: `dst` was derived from `src`, and at the moment it was derived, `src` was at
-- version `src_version`.
--
-- Two earlier drafts were worse. The first stored a composite stamp "<src id>_<src updated_at>";
-- the id half was redundant (the edge already has `src`) and cost a string concatenation per row
-- on every check — 81 ms against 69 ms over 180k edges, and 9% more file. The second kept the
-- wall-clock timestamp, which can fail SILENTLY: two edits inside one millisecond leave it
-- unmoved and the edit becomes invisible to every dependent, with no error anywhere.
--
-- So it is a plain counter, bumped on each edit. Monotonic by construction, per node, unable to
-- collide with itself, and independent of any clock. Deliberately not a hash: nothing here is
-- adversarial, collisions are not a threat model, and an integer you can read is debuggable in a
-- way a digest is not.
CREATE TABLE IF NOT EXISTS edges (
    src         TEXT    NOT NULL REFERENCES nodes(id) ON DELETE CASCADE,
    dst         TEXT    NOT NULL REFERENCES nodes(id) ON DELETE CASCADE,
    src_version INTEGER NOT NULL,
    created_at  INTEGER NOT NULL,
    PRIMARY KEY (src, dst)
);

CREATE INDEX IF NOT EXISTS edges_by_src ON edges(src);
CREATE INDEX IF NOT EXISTS edges_by_dst ON edges(dst);

-- A node is DIRECTLY stale when any edge into it carries a stamp that no longer matches its
-- source's current version. One join, no recursion, no model.
CREATE VIEW IF NOT EXISTS stale_direct AS
SELECT e.dst          AS id,
       e.src          AS changed_source,
       e.src_version  AS version_at_derivation,
       s.version      AS version_now
FROM   edges e
JOIN   nodes s ON s.id = e.src
WHERE  e.src_version <> s.version;

-- ...and TRANSITIVELY stale when anything it was built on is stale, however deep.
CREATE VIEW IF NOT EXISTS stale AS
WITH RECURSIVE spread(id, root_cause, depth) AS (
    SELECT id, changed_source, 0 FROM stale_direct
    UNION
    SELECT e.dst, sp.root_cause, sp.depth + 1
    FROM   edges e
    JOIN   spread sp ON sp.id = e.src
    WHERE  sp.depth < 64                  -- a cycle would otherwise run forever
)
SELECT id, root_cause, MIN(depth) AS depth FROM spread GROUP BY id, root_cause;

-- `version` is the only field the mechanism reads. `updated_at` is there so a human can see when
-- something last moved; nothing compares it, and it may be as coarse as you like.

-- ---------------------------------------------------------------------------
-- Ingestion. Facts are staged in a transaction and are invisible to the base
-- until it commits, so a caller can be shown what needs deciding before
-- anything lands.
-- ---------------------------------------------------------------------------

CREATE TABLE IF NOT EXISTS notes (            -- the prose a fact came from, kept verbatim
    id TEXT PRIMARY KEY, text TEXT NOT NULL,
    author INTEGER NOT NULL REFERENCES actors(id), created_at INTEGER NOT NULL);

CREATE TABLE IF NOT EXISTS transactions (
    id TEXT PRIMARY KEY,
    state      TEXT    NOT NULL,              -- needs_review | ready | committed | aborted
    author     INTEGER NOT NULL REFERENCES actors(id),
    creator    INTEGER NOT NULL REFERENCES actors(id),
    note_id    TEXT,
    created_at INTEGER NOT NULL);

CREATE TABLE IF NOT EXISTS staged (
    tx TEXT NOT NULL REFERENCES transactions(id) ON DELETE CASCADE,
    seq INTEGER NOT NULL, fact TEXT NOT NULL,
    dropped INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (tx, seq));

-- One row per pair a human or an agent must rule on. `verdict` stays NULL until it does, and
-- `confirmer` records who did — a verdict with no owner is not a verdict.
CREATE TABLE IF NOT EXISTS pending (
    tx TEXT NOT NULL REFERENCES transactions(id) ON DELETE CASCADE,
    pair INTEGER NOT NULL,
    new_seq  INTEGER NOT NULL,
    base_id  TEXT,                            -- NULL when the pair is two staged facts
    base_seq INTEGER,
    strength REAL NOT NULL,
    kind     TEXT NOT NULL,                   -- controversy | duplicate | narrows
    why      TEXT NOT NULL DEFAULT '',
    verdict  TEXT, keep TEXT, confirmer INTEGER REFERENCES actors(id), resolved_at INTEGER,
    PRIMARY KEY (tx, pair));

CREATE TABLE IF NOT EXISTS vectors (          -- cached so a fact is embedded exactly once
    id TEXT PRIMARY KEY, model TEXT NOT NULL, vec TEXT NOT NULL);

-- ---------------------------------------------------------------------------
-- History. Append-only, and the reason it exists is not the reason you would guess.
--
-- Fact ids get referenced from outside the base — a marker in source, `FACT: proj#f_abc123`,
-- the way a TODO is. Those references outlive everything the base does to itself.
--
-- Because `merge` and `supersede` rewrite the winning node IN PLACE, a committed id is never
-- destroyed and an outside reference never dangles. What changes underneath it is the MEANING:
-- the same id can say the opposite of what it said when someone wrote it down. A dangling
-- pointer announces itself; a pointer that silently changed target does not.
--
-- So every change to a fact is logged with both texts, the verdict that caused it, and who
-- confirmed that verdict. `facts get <id>` replays it.
-- ---------------------------------------------------------------------------

CREATE TABLE IF NOT EXISTS history (
    seq       INTEGER PRIMARY KEY AUTOINCREMENT,
    id        TEXT    NOT NULL,          -- the fact this happened to
    at        INTEGER NOT NULL,
    version   INTEGER NOT NULL,          -- the version the fact reached
    event     TEXT    NOT NULL,          -- created | merged | superseded | absorbed | edited
    was       TEXT    NOT NULL DEFAULT '',  -- text before
    now       TEXT    NOT NULL DEFAULT '',  -- text after
    other     TEXT    NOT NULL DEFAULT '',  -- the fact on the other side of the decision
    confirmer INTEGER REFERENCES actors(id),
    tx        TEXT    NOT NULL DEFAULT '');

CREATE INDEX IF NOT EXISTS history_of ON history(id, seq);
