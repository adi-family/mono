-- The one table that lives outside any Durable Object: a webhook arrives holding a routing
-- key (chat id, team id, ...) and nothing else, so this is the single lookup that has to run
-- before a node is even known. Everything past that point (connections, allowlists, thread
-- maps) lives inside the owning NodeConnection Durable Object -- see docs/channels.md §1.
CREATE TABLE IF NOT EXISTS routing_keys (
  provider TEXT NOT NULL,
  routing_key TEXT NOT NULL,
  node_id TEXT NOT NULL,
  PRIMARY KEY (provider, routing_key)
);
