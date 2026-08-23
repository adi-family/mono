-- A base of flat facts, plus a derivation graph over them.
--
-- Ported from `experiment/knowledge-base/graph.sql`, which the Python prototype ran against.
-- The comments are the design record: they say why each column is shaped the way it is, and
-- most of them exist because an earlier shape failed a measurement.
--
-- The graph answers exactly one question: when a fact changes, what that was built on it is
-- now out of date? It answers mechanically — no model, no similarity, no judgement.

pragma foreign_keys = ON;

-- Everyone who can say or write something: a person, or an agent at a given version.
-- Identities repeat on almost every row — one operator and a handful of agents produce a whole
-- base — so they are stored once and referenced by integer. `nodes` carries two of them and
-- `history` a third; inlining the strings would cost more than the facts do. Measured on 200,000
-- facts with five distinct identities: 33.3 MB inlined against 28.4 MB by reference, 15% of the
-- file for a table with five rows in it.
create table if not exists actors (
    id   integer primary key autoincrement,
    name text not null unique,             -- "igor", "agent:extractor@1"
    kind text not null default 'human'     -- 'human' | 'agent'
);

create table if not exists nodes (
    id         text primary key,
    fact       text    not null,           -- one plain sentence
    author     integer not null references actors(id),  -- whose meaning this is
    creator    integer not null references actors(id),  -- who wrote the record
    version    integer not null default 1, -- bumped on every edit. THIS is what edges compare.
    updated_at integer not null,           -- wall clock, for humans only. Never load-bearing.
    kind       text    not null default 'fact'   -- 'fact' | 'note' | 'artifact'
);

-- Read facts through this, not through `nodes` — it puts the names back.
create view if not exists facts_v as
select n.id, n.fact, a.name as author, c.name as creator, n.version, n.updated_at, n.kind
from   nodes n join actors a on a.id = n.author join actors c on c.id = n.creator;

-- `kind` records what a node *is*, and every kind is compared against every other. A derived
-- artifact is a node, so a new fact is checked against it exactly like a fact — which is how
-- "we can support China" surfaced against a plan that said "skip China for now".
--
-- The prototype had a fourth kind, `composed`: a node deriving a single readable sentence from
-- several atomic facts that qualify each other. It was built, it worked, and it was removed —
-- it added a concept every caller has to understand for a payoff that only appears once the
-- base is systematically splitting facts into atoms, which is itself deferred. See DESIGN.md,
-- "Measured, and deliberately not built yet". Do not reintroduce it here.

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
create table if not exists edges (
    src         text    not null references nodes(id) on delete cascade,
    dst         text    not null references nodes(id) on delete cascade,
    src_version integer not null,
    created_at  integer not null,
    primary key (src, dst)
);

create index if not exists edges_by_src on edges(src);
create index if not exists edges_by_dst on edges(dst);

-- A node is DIRECTLY stale when any edge into it carries a stamp that no longer matches its
-- source's current version. One join, no recursion, no model.
create view if not exists stale_direct as
select e.dst          as id,
       e.src          as changed_source,
       e.src_version  as version_at_derivation,
       s.version      as version_now
from   edges e
join   nodes s on s.id = e.src
where  e.src_version <> s.version;

-- ...and TRANSITIVELY stale when anything it was built on is stale, however deep.
create view if not exists stale as
with recursive spread(id, root_cause, depth) as (
    select id, changed_source, 0 from stale_direct
    union
    select e.dst, sp.root_cause, sp.depth + 1
    from   edges e
    join   spread sp on sp.id = e.src
    where  sp.depth < 64                  -- a cycle would otherwise run forever
)
select id, root_cause, min(depth) as depth from spread group by id, root_cause;

-- ---------------------------------------------------------------------------
-- Ingestion. Facts are staged in a transaction and are invisible to the base
-- until it commits, so a caller can be shown what needs deciding before
-- anything lands.
-- ---------------------------------------------------------------------------

create table if not exists notes (            -- the prose a fact came from, kept verbatim
    id text primary key, text text not null,
    author integer not null references actors(id), created_at integer not null);

create table if not exists transactions (
    id text primary key,
    state      text    not null,              -- needs_review | ready | committed | aborted
    author     integer not null references actors(id),
    creator    integer not null references actors(id),
    note_id    text,
    created_at integer not null);

create table if not exists staged (
    tx text not null references transactions(id) on delete cascade,
    seq integer not null, fact text not null,
    dropped integer not null default 0,
    primary key (tx, seq));

-- One row per pair a human or an agent must rule on. `verdict` stays NULL until it does, and
-- `confirmer` records who did — a verdict with no owner is not a verdict.
create table if not exists pending (
    tx text not null references transactions(id) on delete cascade,
    pair integer not null,
    new_seq  integer not null,
    base_id  text,                            -- NULL when the pair is two staged facts
    base_seq integer,
    strength real not null,
    kind     text not null,                   -- controversy | duplicate | narrows | unclassified
    why      text not null default '',
    verdict  text, keep text, confirmer integer references actors(id), resolved_at integer,
    primary key (tx, pair));

-- Cached so a fact is embedded exactly once.
--
-- `model` is not decoration. Facts are embedded by a prose model and the code index by a code
-- model, and two vector spaces compared to each other produce plausible-looking rankings and no
-- error anywhere — the exact silent failure this design exists to avoid. A row whose `model` is
-- not the one asking is treated as absent and re-embedded. `dims` is stored with it so a blob of
-- the wrong width is caught rather than decoded into nonsense.
create table if not exists vectors (
    id text primary key references nodes(id) on delete cascade,
    model text not null, dims integer not null, vec blob not null);

-- ---------------------------------------------------------------------------
-- History. Append-only, and the reason it exists is not the reason you would guess.
--
-- Fact ids get referenced from outside the base — in a comment, in a plan, in another agent's
-- notes. Those references outlive everything the base does to itself.
--
-- Because `merge` and `supersede` rewrite the winning node IN PLACE, a committed id is never
-- destroyed and an outside reference never dangles. What changes underneath it is the MEANING:
-- the same id can say the opposite of what it said when someone wrote it down. A dangling
-- pointer announces itself; a pointer that silently changed target does not.
--
-- So every change to a fact is logged with both texts, the verdict that caused it, and who
-- confirmed that verdict. `facts get <id>@<version>` replays it.
-- ---------------------------------------------------------------------------

create table if not exists history (
    seq       integer primary key autoincrement,
    id        text    not null,          -- the fact this happened to
    at        integer not null,
    version   integer not null,          -- the version the fact reached
    event     text    not null,          -- created | merge | supersede | absorbed | derived
    was       text    not null default '',  -- text before
    now       text    not null default '',  -- text after
    other     text    not null default '',  -- the fact on the other side of the decision
    confirmer integer references actors(id),
    tx        text    not null default '');

create index if not exists history_of on history(id, seq);
