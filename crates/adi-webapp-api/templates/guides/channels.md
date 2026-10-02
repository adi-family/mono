# Channels

One ADI agent, reachable from Telegram or Slack. A **connection** binds one chat/workspace to
one agent: every message that chat sends becomes a run (the first message) or a reply to the
run already open (every message after), and the run's final answer posts back to that chat
automatically. You don't call anything to make that happen — it's the whole point of connecting
a channel. The one thing you do call is `channel-reply`, for a message mid-turn.

## Where it lives
- `~/.adi/mono/channels/<id>.toml` — one connection: provider, routing key, target agent,
  allowlist, paused flag, and the provider-thread → run-id map.
- The router (`hooks.withadi.dev`, or a local `wrangler dev` while this is unreleased) holds the
  bot credentials and the live WebSocket. This store never has a bot token in it.

## Do it
Every verb below needs `adi-app` running on this machine — it's the process holding the live
socket to the router, and `connect`'s wait on `linked` is waiting on that socket, not on a file.

- Connect a service: `{{cli}} channels connect <telegram|slack> --agent <agent>` — registers the
  connection, prints the install/link URL (for Telegram, `t.me/<bot>?start=<code>`; Telegram also
  prints a second link, `?startgroup=<code>`, to add the bot to a group instead of messaging it
  directly), and blocks until the operator completes the link (or `--timeout-secs`, default 600,
  runs out — the connection is kept either way, so re-run to keep waiting). Panel:
  `/settings/channels` → the service's card → **Connect**.
- List connections: `{{cli}} channels list`, or `GET /api/channels`.
- Change which agent a connection targets: `{{cli}} channels route <id> --agent <agent>`.
- Pause or resume delivery without disconnecting: `{{cli}} channels route <id> --pause` /
  `--resume` (the same verb — pausing is a property of the connection, not a separate thing).
- Change who may talk: `{{cli}} channels allow <id> --add <sender-id>` (add one sender to the
  list) or `--mode open|owner_only` (replace the allowlist outright). Default, from the moment a
  chat links, is `owner_only` — only whoever completed the link.
- Disconnect: `{{cli}} channels disconnect <id>` — tells the router to drop it, then drops it
  here too. The provider's node token is revoked too, once nothing else on this node still uses
  that provider.

Every one of these is also a button on the connection's row in the panel (`/settings/channels`):
Change agent, Pause/Resume, Who may talk, Disconnect — same calls, same effect, either door.

## The `channel-reply` tool — a mid-run post
The run's own final answer posts back on its own; `channel-reply` is for everything **before**
that — a status update, a partial result, anything worth saying while the turn is still going.
Enable it on an agent (`agents.md` — tick it in `bin_tools`, same as any other tool) and call it
from inside a run:

```
channel-reply "still working on the second half, found 12 matches so far"
```

It never blocks the turn (unlike `Ask`) and reads which run it's posting from off its own
environment (`ADI_RUN_ID`), so it needs no connection id — it looks up which connection's thread
map names the run it's running inside of. Calling it from a run nothing opened over a channel is
a no-op, not an error: there's nothing to post back to.

## Allowlist, in one paragraph
A connection's allowlist is enforced on this node, never by the router — the router forwards
every webhook for a linked chat regardless of sender, so a bug there can leak a message but never
grant a reply from an agent a sender isn't allowed to talk to. `owner_only` (default) means
exactly the person who completed the link; `open` means anyone in that chat or workspace; a named
list is anyone on it. A disallowed sender gets silence, not a "you're not allowed" reply — telling
a stranger a bot exists here and is gated is worse than saying nothing.

## Notes
- One service thread is one ADI conversation for the lifetime of the connection — the second
  message on a Telegram chat (or a Slack channel/thread) replies to the run the first one opened,
  it never starts a second one.
- Reconnecting a provider that's already registered on this node reuses the same node token —
  there is one token per `(node, provider)`, shared across every connection on that provider, not
  one per connection. That's also why `disconnect` only revokes the token once every connection on
  that provider is gone.
- Slack's install is OAuth, not a deep link: `connect`'s printed URL is "Add to Slack" — completing
  it in a browser installs the app into that workspace and links it in one step, no `/start`
  message to send. The connection's routing key is the Slack **workspace** (`team_id`), so one link
  covers every channel the bot is invited into; who may talk still narrows per the usual allowlist.
- In a Telegram group, the bot runs in privacy mode: it only sees messages that `@mention` it (or
  reply to one of its own messages), not every message in the group — and it only replies to those.
- Linking a chat/workspace that's already connected to another ADI is refused, not reassigned —
  "first install wins". The person is told why (in the chat, or on Slack's OAuth callback page) and
  pointed at disconnecting it first if that's actually what they want.
