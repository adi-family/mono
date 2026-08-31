# `demo-node/` — what runs on `adi-demo`

`scripts/deploy-demo-node.sh` builds the box. This directory is what goes *on* it, kept here
because the node is otherwise the only copy: `adi-demo` is a plain GCE instance with no autohealing
and no backups, and the dashboard App Review is told to open would go with it.

## `hello-reviewer/`

One dashboard, one panel: *"Hello reviewer, server time is 2026-08-31 06:55:18 UTC"*, ticking once
a second, plus a line saying which machine served it.

It is deliberately the smallest thing that proves the two claims a reviewer cannot otherwise check
(`apps/ios/APPSTORE.md` §2.2). That the fleet is **live** — the clock moves. And that it is
**remote** — the time is the *node's*, so it cannot have been rendered by the phone on its own.
That is also why the page keeps a clock *offset* rather than repainting a string from the server:
it ticks smoothly on a phone whose own clock is wrong, without asking a relayed connection for a
request per second.

## Putting it on a node

The scaffold comes from the node's own panel; only these two files are ours. From this Mac, with
the node paired and its panel reachable at `app.<petname>.n.adi`:

```bash
PW=…   # the Basic-auth password the pairing minted; see APPSTORE.md §2.2

# 1. scaffold (gives an id and the host hello-reviewer.adi)
curl -u "adi:$PW" -X POST http://app.adi-demo.n.adi/api/dashboards/create \
  -H 'Content-Type: application/json' \
  -d '{"name":"Hello Reviewer","description":"A one-panel dashboard for App Review."}'

# 2. copy these two in, and drop the scaffold's example pair
gcloud compute scp hello-reviewer/frontend/modules/hello.ts \
  hello-reviewer/backend/routes/time.ts adi-demo:/tmp/ \
  --zone=europe-southwest1-a --project=mono-504617
D=/home/adi/.adi/mono/dashboards/<id>
gcloud compute ssh adi-demo --zone=europe-southwest1-a --project=mono-504617 --command "
  sudo install -o adi -g adi -m 644 /tmp/hello.ts $D/frontend/modules/hello.ts &&
  sudo install -o adi -g adi -m 644 /tmp/time.ts  $D/backend/routes/time.ts  &&
  sudo rm -f $D/frontend/modules/status.ts $D/backend/routes/status.ts"
```

**Then restart the backend, or you will debug correct code.** `guides/dashboards.md` warns that
routes do not reliably hot-reload, and this hit exactly that: with the new `time.ts` on disk the
process still served the *deleted* `/status` and answered `/time` with `not found`. Kill it by its
port and the supervisor brings it straight back:

```bash
gcloud compute ssh adi-demo --zone=… --command \
  'sudo -u adi bash -c "kill \$(ss -lntpH \"sport = :<backend_port>\" | grep -oE \"pid=[0-9]+\" | cut -d= -f2)"'
```

## Checking it the way a reviewer will see it

A screenshot behind Basic auth cannot be taken with `--screenshot http://user:pass@host/`: Chrome
loads the document and then refuses every same-origin `fetch` from it ("Request cannot be
constructed from a URL that includes credentials"), so the page renders showing a failure that is
not real. Inject the header over CDP instead — set `Network.setExtraHTTPHeaders`, and drive
`Emulation.setDeviceMetricsOverride` with `mobile: true` at 390px, because that is the width the
reviewer holds. Verified at both 900px and 390px on 2026-08-31; no overflow at either.
