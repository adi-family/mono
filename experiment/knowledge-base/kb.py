"""Minimal knowledge base over the schema in graph.sql: flat facts plus a derivation graph."""
import sqlite3, time, json, sys

def now_ms(): return int(time.time() * 1000)   # display only; the mechanism uses nodes.version

class KB:
    def __init__(self, path=":memory:"):
        self.db = sqlite3.connect(path)
        self.db.row_factory = sqlite3.Row
        self.db.executescript(open("graph.sql").read())

    def add(self, id, fact, author, creator, sources=()):
        """Record a fact. `sources` are the ids it was derived from — empty for something a
        human simply said. Each source is stamped at its version as of right now."""
        t = now_ms()
        self.db.execute("INSERT INTO nodes(id,fact,author,creator,version,updated_at) VALUES(?,?,?,?,1,?)",
                        (id, fact, author, creator, t))
        for s in sources:
            row = self.db.execute("SELECT version FROM nodes WHERE id=?", (s,)).fetchone()
            if row is None: raise KeyError(f"unknown source {s}")
            self.db.execute("INSERT INTO edges(src,dst,src_version,created_at) VALUES(?,?,?,?)",
                            (s, id, row["version"], t))
        self.db.commit()
        return id

    def edit(self, id, fact):
        """Change a fact. Every edge into its dependents now carries a version that no longer
        matches — that is the whole invalidation mechanism, and it costs one UPDATE."""
        self.db.execute("UPDATE nodes SET fact=?, version=version+1, updated_at=? WHERE id=?",
                        (fact, now_ms(), id))
        self.db.commit()

    def stale(self):
        return [dict(r) for r in self.db.execute(
            "SELECT s.id, s.root_cause, s.depth, n.fact FROM stale s JOIN nodes n ON n.id=s.id "
            "ORDER BY s.depth, s.id")]

    def refresh(self, id):
        """Mark a derived node re-generated: bring its incoming edges up to the sources'
        current versions. Nothing else in the graph moves."""
        for e in self.db.execute("SELECT src FROM edges WHERE dst=?", (id,)).fetchall():
            u = self.db.execute("SELECT version FROM nodes WHERE id=?", (e["src"],)).fetchone()["version"]
            self.db.execute("UPDATE edges SET src_version=? WHERE src=? AND dst=?",
                            (u, e["src"], id))
        self.db.execute("UPDATE nodes SET version=version+1, updated_at=? WHERE id=?", (now_ms(), id))
        self.db.commit()
