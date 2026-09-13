#!/usr/bin/env bash
# Build the local marketplace the dev panel browses: five bundle repositories, one manifest
# listing them, and the source entry that points the dev store at it.
#
# It exists because the v2 marketplace (docs/marketplace-bundles.md) cannot be looked at without
# bundles to look at, and nothing is published yet — every real manifest out there still lists a
# v1 app. The `file://` carve-out both `sources.toml` and `bundles[].repo` already grant for
# exactly this case (docs/marketplace.md, "Sources") is what makes it work with no host involved.
#
# The five cover the shapes a reviewer needs to compare side by side:
#
#   changelog   two tools and nothing else
#   standup     one dashboard and nothing else — with a readme and a gallery, so the app page
#               has something on it
#   reviewer    one agent and nothing else — and it declares a secret nobody has set, which is
#               how the listing's missing-secret line gets something to say
#   crm-suite   the design document's own example: agent + tool + LLM backend + dashboard
#   ops-kit     all eight kinds at once, including the project scaffold and the parked hive
#               service — the widest the element grouping ever gets
#
# Everything lands in the ISOLATED dev store (.adi-dev), never ~/.adi/mono.
#
# Re-running is safe and does **nothing** when the fixture content has not changed: each
# repository is committed only when `git add -A` actually staged something, so the pins hold and
# an element already installed from one stays current. Which is also how to produce the
# update-waiting state deliberately — edit a file under one repository (or in this script) and
# re-run: that one bundle's pin moves past what is installed, and its row offers Update. To start
# over from nothing instead, `rm -rf .adi-dev/fixtures` first.
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

# Set, not defaulted: an inherited ADI_DIR points at whatever store the caller is working in —
# an agent run carries the live one — and this script is by definition about the dev store.
export ADI_DIR="${ADI_DEV_DIR:-$repo_root/.adi-dev}"
fixtures="${ADI_FIXTURES:-$ADI_DIR/fixtures}"
repos="$fixtures/repos"
manifest="$fixtures/marketplace.json"
source_name="${ADI_FIXTURE_SOURCE:-bundles}"
# The new `marketplace` verbs are not in the released binary yet (docs/marketplace-bundles.md
# says so out loud), so the debug build is the one that can add a file:// source at all.
cli="${ADI_MONO:-$repo_root/target/debug/adi-mono}"

[ -x "$cli" ] || { echo "error: no adi-mono at $cli — cargo build -p adi-cli" >&2; exit 1; }

mkdir -p "$repos"

# A publisher's repository, committed — but only when something really changed, because a commit
# here is a published release: it moves the manifest's pin, and every install standing at the old
# one starts reading as outdated. The identity is set per repo rather than inherited, so this
# works on a machine whose global git config has none.
commit_repo() {
  local dir=$1
  if [ ! -d "$dir/.git" ]; then
    git -C "$dir" init --quiet -b main
    git -C "$dir" config user.email "publisher@example"
    git -C "$dir" config user.name "The Publisher"
  fi
  git -C "$dir" add -A
  git -C "$dir" diff --cached --quiet || git -C "$dir" commit --quiet -m "publish"
}

# A dashboard element: the two entry points the store requires, plus the module and route a real
# one keeps its own work in (guides/dashboards.md — the entry points are rewritten on migration,
# these are not).
write_dashboard() {
  local dir=$1 title=$2 route=$3
  mkdir -p "$dir/frontend/modules" "$dir/backend/routes"
  cat > "$dir/frontend/index.html" <<HTML
<!doctype html>
<meta charset="utf-8">
<title>$title</title>
<div id="app"></div>
<script type="module" src="./index.ts"></script>
HTML
  cat > "$dir/frontend/index.ts" <<TS
import { mount } from "./modules/$route";

mount(document.getElementById("app")!);
TS
  cat > "$dir/frontend/modules/$route.ts" <<TS
export async function mount(root: HTMLElement) {
  const res = await fetch("/api/$route");
  const rows = await res.json();
  root.textContent = rows.length ? "" : "Nothing yet.";
}
TS
  cat > "$dir/backend/index.ts" <<TS
import { $route } from "./routes/$route";

Bun.serve({ port: Number(process.env.PORT), fetch: $route });
TS
  cat > "$dir/backend/routes/$route.ts" <<TS
export function $route(_req: Request): Response {
  return Response.json([]);
}
TS
}

# ---------------------------------------------------------------- changelog — tools only

mkdir -p "$repos/changelog/tools"
cat > "$repos/changelog/tools/changelog.sh" <<'SH'
#!/usr/bin/env bash
# Turn a commit range into release notes, one line per commit worth reading.
set -euo pipefail
range=${1:-$(git describe --tags --abbrev=0)..HEAD}
git log --no-merges --pretty='- %s' "$range"
SH
cat > "$repos/changelog/tools/commit-lint.sh" <<'SH'
#!/usr/bin/env bash
# Check a commit message against the rules this repository keeps: a subject that says what
# changed, under 72 characters, and no trailing period.
set -euo pipefail
subject=$(head -n1 "${1:-/dev/stdin}")
[ "${#subject}" -le 72 ] || { echo "subject is ${#subject} characters, over 72" >&2; exit 1; }
case "$subject" in *.) echo "subject ends in a period" >&2; exit 1 ;; esac
echo "ok"
SH
cat > "$repos/changelog/README.md" <<'MD'
# Changelog tools

Two shell tools an agent can reach once you tick them on: `changelog` reads a commit range and
writes the note, `commit-lint` refuses a subject line nobody will be able to read later.
MD
commit_repo "$repos/changelog"

# ---------------------------------------------------------------- standup — one dashboard

write_dashboard "$repos/standup/dashboards/standup" "Standup board" "standup"
cat > "$repos/standup/README.md" <<'MD'
# Standup board

What every agent finished yesterday, on one screen.
MD
commit_repo "$repos/standup"

# ---------------------------------------------------------------- reviewer — one agent

mkdir -p "$repos/reviewer/agents"
cat > "$repos/reviewer/agents/reviewer.toml" <<'TOML'
backend = "harness:adi"
tags = ["review", "git"]

[arguments]
system_prompt = """
You review a diff the way a senior engineer does: say what would break, name the line, and stop.
Praise nothing. If the change is fine, say it is fine in one sentence.
"""

[[secrets]]
name = "REVIEWER_GITHUB_TOKEN"
TOML
commit_repo "$repos/reviewer"

# ---------------------------------------------------------------- crm-suite — the four-kind combo

mkdir -p "$repos/crm-suite/agents" "$repos/crm-suite/tools" "$repos/crm-suite/llm"
# bin_tools and backends name this bundle's own siblings by their published ids, which is the
# only wiring a publisher can honestly write: those two kinds land verbatim or not at all
# (docs/marketplace-bundles.md decision #3), so the reference resolves or the element is simply
# absent — it never quietly points at a stranger's tool of the same name.
cat > "$repos/crm-suite/agents/sales-bot.toml" <<'TOML'
tags = ["sales", "follow-up"]
bin_tools = ["csv-import"]

[arguments]
system_prompt = """
You draft the follow-up nobody got round to sending. One paragraph, their words, no pitch.
"""

[[backends]]
backend = "gpt5"
TOML
cat > "$repos/crm-suite/tools/csv-import.sh" <<'SH'
#!/usr/bin/env bash
# Load a contacts export into the CRM's own table, skipping rows already there.
set -euo pipefail
file=${1:?usage: csv-import <contacts.csv>}
rows=$(awk -F, 'NR > 1 { print $1 "\t" $2 }' "$file" | tee /dev/stderr | wc -l)
printf 'imported %s contact(s)\n' "$rows"
SH
cat > "$repos/crm-suite/llm/gpt5.toml" <<'TOML'
label = "GPT-5"
runtime = "harness:adi"
model = "gpt-5"
provider = "openai"
api_key_env = "OPENAI_API_KEY"
TOML
write_dashboard "$repos/crm-suite/dashboards/crm" "CRM" "contacts"
commit_repo "$repos/crm-suite"

# ---------------------------------------------------------------- ops-kit — all eight kinds

mkdir -p "$repos/ops-kit"/{agents,tools,llm,embeddings,services,triggers,project}
cat > "$repos/ops-kit/agents/oncall.toml" <<'TOML'
tags = ["ops", "oncall"]
bin_tools = ["pager"]
unattended = true

[arguments]
system_prompt = """
You are the first responder. Read the alert, say what is broken and what you checked, and hand
over. Never restart anything you were not asked to restart.
"""

[[backends]]
backend = "local-qwen"
TOML
cat > "$repos/ops-kit/tools/pager.sh" <<'SH'
#!/usr/bin/env bash
# Page whoever is on call, through the webhook this machine holds the URL for.
set -euo pipefail
message=${1:?usage: pager <message>}
printf 'would page: %s\n' "$message"
SH
write_dashboard "$repos/ops-kit/dashboards/ops" "Ops" "incidents"
cat > "$repos/ops-kit/llm/local-qwen.toml" <<'TOML'
label = "Qwen (local)"
runtime = "harness:adi"
model = "qwen3-coder"
provider = "openai"
base_url = "http://127.0.0.1:11434/v1"
TOML
cat > "$repos/ops-kit/embeddings/bge-small.toml" <<'TOML'
label = "BGE small (local)"
runtime = "hash"
model = "hash-bow-256"
dimensions = 256
TOML
# A backing service with no proxy.host defaults to `always`, which is why an installed one is
# parked in the marketplace module rather than written into a live hive.yaml.
cat > "$repos/ops-kit/services/incident-queue.yaml" <<'YAML'
runner:
  docker:
    image: redis:7-alpine
YAML
cat > "$repos/ops-kit/triggers/nightly-sweep.toml" <<'TOML'
kind = "background"
description = "Closes incidents nobody touched for a week."
enabled = true
code = """
echo 'sweeping stale incidents'
"""
TOML
cat > "$repos/ops-kit/project/config.toml" <<'TOML'
name = "Ops"
description = "Where this bundle's own agents, tools and backends are filed."
TOML
commit_repo "$repos/ops-kit"

# ---------------------------------------------------------------- the manifest itself

python3 - "$repos" "$manifest" <<'PY'
"""Write the manifest the five repositories are published in.

Icons and gallery stills are `data:` URIs rather than links: the manifest has already been
fetched, so an icon carried inside it costs no further request and works with the network gone
(docs/marketplace.md). It is also the only honest way to publish an image from a repository that
has no host — the same reason the manifest itself is a file:// source here.
"""

import base64, json, os, shutil, subprocess, sys
from pathlib import Path

repos, manifest_path = Path(sys.argv[1]), Path(sys.argv[2])


def pin(slug):
    return subprocess.run(
        ["git", "-C", str(repos / slug), "rev-parse", "HEAD"],
        check=True, capture_output=True, text=True,
    ).stdout.strip()


def repo_url(slug):
    return f"file://{repos / slug}"


def data_uri(svg):
    return "data:image/svg+xml;base64," + base64.b64encode(svg.encode()).decode()


# Ink tones, not the accent: an icon fetched from a publisher sits beside the panel's own one
# orange, and a listing of five orange marks would be five screens' worth of it.
INK, DIM = "#A9A6A0", "#6F6C67"


def icon(body):
    return data_uri(
        '<svg xmlns="http://www.w3.org/2000/svg" width="64" height="64" viewBox="0 0 24 24" '
        f'fill="none" stroke="{INK}" stroke-width="1.5" stroke-linecap="round" '
        f'stroke-linejoin="round">{body}</svg>'
    )


ICONS = {
    "changelog": icon(
        '<path d="M14 3H7a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h10a2 2 0 0 0 2-2V8z"/>'
        '<path d="M14 3v5h5"/><path d="M9 12h6"/><path d="M9 16h4"/>'
    ),
    "standup": icon(
        '<rect x="3" y="4" width="7" height="7" rx="1"/><rect x="14" y="4" width="7" height="4" rx="1"/>'
        '<rect x="3" y="14" width="7" height="6" rx="1"/><rect x="14" y="11" width="7" height="9" rx="1"/>'
    ),
    "crm-suite": icon(
        '<path d="M16 20v-1a4 4 0 0 0-4-4H7a4 4 0 0 0-4 4v1"/><circle cx="9.5" cy="8" r="3"/>'
        '<path d="M21 20v-1a4 4 0 0 0-3-3.9"/><path d="M16 4.2a4 4 0 0 1 0 7.6"/>'
    ),
    "ops-kit": icon(
        '<rect x="3" y="4" width="18" height="7" rx="2"/><rect x="3" y="14" width="18" height="6" rx="2"/>'
        f'<circle cx="7" cy="7.5" r=".8" fill="{INK}"/><circle cx="7" cy="17" r=".8" fill="{INK}"/>'
    ),
}


# --- mock screens -------------------------------------------------------------------------------
# A publisher's screenshots, drawn rather than captured: these five repositories do not exist, so
# there is nothing to photograph. Four shapes, because a gallery of one shape tells you nothing
# about how the page handles a set — a list, a terminal, a transcript, a queue.
#
# Real text rather than grey bars: a row of placeholder rectangles reads as a loading state, and
# what is being reviewed here is whether a screenshot of a working thing sits well on the page.
# No orange anywhere in them, deliberately — a gallery image is drawn on the same screen as the
# panel's own one filled accent (§8), and a mock is not worth spending it on.

W, H = 1200, 750
BG, SIDE, RULE = "#161616", "#101010", "#242424"
INK_1, INK_2, INK_3, CODE = "#ECEAE6", "#A9A6A0", "#6F6C67", "#D6D3CD"
OK, WARN, ERR = "#4CB77A", "#E0A84B", "#E25C5C"
SANS = "system-ui,-apple-system,Segoe UI,sans-serif"
MONO = "ui-monospace,SFMono-Regular,Menlo,monospace"


def esc(s):
    return s.replace("&", "&amp;").replace("<", "&lt;").replace(">", "&gt;")


def label(x, y, s, fill=INK_2, size=15, family=SANS, weight=400, anchor="start"):
    return (
        f'<text x="{x}" y="{y}" font-family="{family}" font-size="{size}" font-weight="{weight}" '
        f'fill="{fill}" text-anchor="{anchor}">{esc(s)}</text>'
    )


def frame(title, body):
    """The window every mock is drawn in: a bar with the screen's own name, the page under it."""
    return data_uri(
        f'<svg xmlns="http://www.w3.org/2000/svg" width="{W}" height="{H}" viewBox="0 0 {W} {H}">'
        f'<rect width="{W}" height="{H}" fill="{BG}"/>'
        f'<rect width="{W}" height="76" fill="{SIDE}"/>'
        f'{label(40, 48, title, INK_1, 22, weight=500)}'
        f'<rect x="1040" y="24" width="120" height="28" rx="6" fill="#2A2A2A"/>'
        f'{body}</svg>'
    )


def screen_rows(title, rows):
    """A list screen: a name on the left, a value on the right, a hairline between."""
    body = ""
    for i, (left, right) in enumerate(rows):
        y = 130 + i * 54
        body += label(40, y, left, INK_1, 15)
        body += label(1160, y, right, INK_3, 14, anchor="end")
        body += f'<line x1="40" y1="{y + 20}" x2="1160" y2="{y + 20}" stroke="{RULE}"/>'
    return frame(title, body)


def screen_queue(title, rows):
    """The same, with a status dot in front — what an incident list or a run list looks like."""
    body = ""
    for i, (dot, left, meta, right) in enumerate(rows):
        y = 130 + i * 60
        body += f'<circle cx="46" cy="{y - 5}" r="4" fill="{dot}"/>'
        body += label(64, y, left, INK_1, 15)
        body += label(64, y + 20, meta, INK_3, 13)
        body += label(1160, y, right, INK_3, 14, anchor="end")
        body += f'<line x1="40" y1="{y + 34}" x2="1160" y2="{y + 34}" stroke="{RULE}"/>'
    return frame(title, body)


def screen_terminal(title, lines):
    """A shell: a prompt, its output, and whatever the tool had to say about it."""
    body = f'<rect x="40" y="108" width="1120" height="{H - 148}" rx="10" fill="#1E1E1E"/>'
    for i, (text, tone) in enumerate(lines):
        fill = {"prompt": CODE, "out": INK_2, "dim": INK_3, "ok": OK, "err": ERR}[tone]
        body += label(66, 146 + i * 30, text, fill, 15, family=MONO)
    return frame(title, body)


def screen_chat(title, turns):
    """A transcript: who spoke, then what they said, at the measure the real one reads at."""
    body, y = "", 132
    for who, said in turns:
        body += label(40, y, who, INK_3, 13)
        y += 26
        for line in said:
            body += label(40, y, line, INK_1, 16)
            y += 28
        y += 22
    return frame(title, body)


def build_clip(stills, out_dir):
    """A short clip for the one gallery that should have one, cross-faded from two of its stills.

    The video path is a real part of the page — a poster on the stage, a play glyph on the
    thumbnail, `<video controls>` that waits to be asked — and a manifest of pictures never
    exercises it. Built rather than shipped: an mp4 checked into this repository would be a binary
    nobody can review, and the two frames are already here.

    `shot` rasterises the SVG (no rasteriser in the standard library, and every mock is an SVG);
    ffmpeg does the fade. Missing either is a note, not a failure — a gallery of stills is still a
    gallery, and this script has to run on a machine that has neither.
    """
    if not (shutil.which("shot") and shutil.which("ffmpeg")):
        print("clip: no shot/ffmpeg on PATH — that gallery goes out as stills")
        return None
    # `shot` is `adi-mono tools run shot`, so it resolves the tool out of whatever store ADI_DIR
    # names — and this script has pointed that at the dev store, which has no tools in it at all
    # ("error: no such tool: shot"). Dropping the override for this one call puts it back on the
    # real store, where the tool lives. The fixtures it writes are still the dev store's.
    env = {k: v for k, v in os.environ.items() if k != "ADI_DIR"}
    out_dir.mkdir(parents=True, exist_ok=True)
    frames = []
    try:
        for i, still in enumerate(stills):
            html, png = out_dir / f"frame{i}.html", out_dir / f"frame{i}.png"
            html.write_text(
                f'<body style="margin:0;background:{BG}">'
                f'<img src="{still}" width="{W}" height="{H}">'
            )
            subprocess.run(
                ["shot", str(html), "--out", str(png),
                 "--width", str(W), "--height", str(H), "--dpr", "1"],
                check=True, capture_output=True, env=env,
            )
            frames.append(png)
        mp4 = out_dir / "tour.mp4"
        subprocess.run(
            ["ffmpeg", "-y", "-loop", "1", "-t", "2.2", "-i", str(frames[0]),
             "-loop", "1", "-t", "2.2", "-i", str(frames[1]),
             "-filter_complex",
             "[0][1]xfade=transition=fade:duration=0.7:offset=1.6,format=yuv420p",
             "-r", "20", "-c:v", "libx264", "-crf", "32", "-preset", "veryfast",
             "-movflags", "+faststart", str(mp4)],
            check=True, capture_output=True,
        )
    except subprocess.CalledProcessError as e:
        print(f"clip: {e.cmd[0]} failed — that gallery goes out as stills")
        return None
    print(f"clip: {mp4.stat().st_size // 1024} KiB of mp4, carried in the manifest")
    return "data:video/mp4;base64," + base64.b64encode(mp4.read_bytes()).decode()


STANDUP_README = """\
## What it is

One screen that answers the only question a standup ever asks: what moved since yesterday. Every
agent that finished a run in the last day gets a line — what it was asked for, what it shipped,
and how long it took.

## What it needs

Nothing but the store it is installed into. It reads the runs this machine already has:

```
GET /api/runs?since=24h
```

## What it does not do

It does not start anything, does not write to any run, and has no opinion about what should have
been finished. It is a list.
"""

CRM_README = """\
## What it is

The four pieces of a follow-up habit, published together: an agent that drafts the message, the
tool that loads your contacts, the backend it answers on, and the board that shows who has gone
quiet.

## Installing part of it

Each element installs on its own — take the tool without the agent, or the dashboard without
either:

```
adi-mono marketplace install bundles/crm-suite/tools/csv-import
```

The agent names `csv-import` in its `bin_tools`, so installing the agent alone leaves it with an
empty bin until the tool lands too. That is the ordinary state of an agent whose tools are not
ticked, not an error.
"""

# The stills, built before the manifest so the clip can be cross-faded from two of them.
CHANGELOG_STILLS = [
    screen_terminal("changelog", [
        ("$ changelog v1.1.0..HEAD", "prompt"),
        ("- marketplace: a shelf you browse, and a page per item you act from", "out"),
        ("- marketplace: five local bundles to look at", "out"),
        ("- chat rail: never filter out a run asking a person a question", "out"),
        ("- embedding backends: wire the three consumers", "out"),
        ("- hive: a service may arrive parked and start on your say-so", "out"),
        ("- indexer: bump the pipeline version after a schema change", "out"),
        ("", "out"),
        ("21 commits, 6 worth reading, 15 filtered", "dim"),
        ("", "out"),
        ("$ changelog v1.1.0..HEAD --since-tag", "prompt"),
        ("nothing new since v1.2.0", "dim"),
    ]),
    screen_terminal("commit-lint", [
        ("$ commit-lint .git/COMMIT_EDITMSG", "prompt"),
        ("subject is 94 characters, over 72", "err"),
        ("", "out"),
        ("$ commit-lint .git/COMMIT_EDITMSG", "prompt"),
        ("ok", "ok"),
    ]),
]

STANDUP_STILLS = [
    screen_rows("Standup · yesterday", [
        ("adi-ui — rebuilt the marketplace as a shelf and a page", "1h 12m"),
        ("adi-dev — embedding backends, the operator surface", "3h 04m"),
        ("adi-docs — comments audit over adi-agents", "22m"),
        ("adi-ui — five marketplace bundles to look at", "36m"),
        ("adi-mesh — relay ping timeout, reproduced", "2h 21m"),
        ("landing — hero copy, third pass", "12m"),
        ("adi-dev — clone lint across the workspace", "58m"),
        ("adi-ui — sentence case on the kind headings", "6m"),
        ("adi-db — vacuum on the events spool", "9m"),
        ("adi-dev — trigger that arrives disabled, tested", "41m"),
    ]),
    screen_chat("Standup · one run", [
        ("asked for", ["Make the marketplace look like a marketplace."]),
        ("shipped", [
            "The listing is a shelf now: rows are links, no buttons,",
            "and what an item carries is said in words.",
            "Every act moved to the item's own page — one orange Install,",
            "a gallery, and what is included with an action each.",
        ]),
        ("checked", [
            "Installed an element and took it back out again, in a browser.",
            "7 unit tests. Five screenshots read.",
        ]),
        ("took", ["1h 12m · 4 files · 1 commit"]),
    ]),
]

REVIEWER_STILLS = [
    screen_chat("reviewer · pull/318", [
        ("reviewer", [
            "install.rs:412 — the staging directory is removed before the",
            "rename, so a failed rename leaves nothing to retry from.",
            "Move the cleanup after it, or keep the path.",
        ]),
        ("reviewer", [
            "bundle.rs:1842 — land_tool mints an id and then undoes it on a",
            "collision. That is two writes where one check would do, and the",
            "undo is not on the error path.",
        ]),
        ("reviewer", ["Nothing else here would break."]),
    ]),
    screen_chat("reviewer · pull/319", [
        ("reviewer", ["This one is fine."]),
        ("you", ["Say why."]),
        ("reviewer", [
            "One function, one caller, the test covers the empty case and the",
            "two-element case. The name says what it returns.",
            "There is nothing here to be wrong about.",
        ]),
    ]),
]

CRM_STILLS = [
    screen_rows("CRM · gone quiet", [
        ("Marta Kaufmann — Northwind", "47 days"),
        ("Ben Okoro — Sable & Co", "31 days"),
        ("Priya Raman — Halter Logistics", "28 days"),
        ("Tom Whitfield — Gearbox", "22 days"),
        ("Ana Ferreira — Mistral Freight", "19 days"),
        ("Jonas Lind — Kestrel", "14 days"),
        ("Hana Sato — Orchard Lane", "12 days"),
        ("Dmitri Volkov — Ternary", "9 days"),
        ("Grace Mbeki — Fieldnote", "8 days"),
        ("Luis Ferrán — Costa Dorada", "6 days"),
    ]),
    screen_chat("sales-bot · draft for Marta", [
        ("last heard from her", ["12 March — \"circle back after the Q2 budget lands\""]),
        ("sales-bot", [
            "Hi Marta — you mentioned in March that the pilot was waiting",
            "on your Q2 budget. That is behind you now, so: still worth",
            "picking up, or shall I stop asking?",
        ]),
        ("waiting on you", ["Send · Edit · Never mind"]),
    ]),
]

OPS_STILLS = [
    screen_queue("Ops · open incidents", [
        (ERR, "api-gateway returning 502 on /v2/search", "opened 12m ago · nobody assigned", "sev 1"),
        (WARN, "queue depth over 10k for twenty minutes", "opened 1h ago · oncall", "sev 2"),
        (WARN, "nightly export ran twice", "opened 4h ago · oncall", "sev 3"),
        (WARN, "certificate on relay-2 expires in six days", "opened 9h ago · oncall", "sev 3"),
        (OK, "disk pressure on builder-2", "closed 6h ago · swept", "closed"),
        (OK, "webhook retries backed off correctly", "closed 11h ago · swept", "closed"),
        (OK, "index rebuild finished", "closed 1d ago · oncall", "closed"),
    ]),
    screen_terminal("pager", [
        ("$ pager 'api-gateway 502s on /v2/search'", "prompt"),
        ("paged: oncall (primary) — acknowledged in 41s", "ok"),
        ("", "out"),
        ("$ pager --who", "prompt"),
        ("primary: you, until 09:00", "out"),
        ("secondary: dmitri, until Thursday", "out"),
        ("", "out"),
        ("$ pager --dry-run 'queue depth over 10k'", "prompt"),
        ("would page: oncall (primary)", "dim"),
        ("would not page: secondary (sev 2 policy)", "dim"),
    ]),
]

# One clip, on the item with the most to show in motion. Everything else is stills — a gallery of
# clips is a page that asks to be watched, and these are tools.
CRM_CLIP = build_clip(CRM_STILLS, manifest_path.parent / "clip")

BUNDLES = [
    {
        "slug": "changelog",
        "name": "Changelog tools",
        "description": "Turn a range of commits into release notes, and refuse a subject nobody can read.",
        "keywords": ["git", "release", "notes"],
        "version": "1.2.0",
        "icon": ICONS["changelog"],
        "gallery": [
            {"url": CHANGELOG_STILLS[0], "caption": "One release, as the tool writes it"},
            {"url": CHANGELOG_STILLS[1], "caption": "…and the subject it refused first"},
        ],
        "elements": [
            {"kind": "tool", "name": "changelog", "description": "Reads a commit range and writes the note."},
            {"kind": "tool", "name": "commit-lint", "description": "Checks a subject line against the house rules."},
        ],
    },
    {
        "slug": "standup",
        "name": "Standup board",
        "description": "What every agent finished yesterday, on one screen.",
        "keywords": ["dashboard", "daily", "agents"],
        "version": "0.3.1",
        "icon": ICONS["standup"],
        "readme": STANDUP_README,
        "gallery": [
            {"url": STANDUP_STILLS[0], "caption": "The board itself, newest run first"},
            {"url": STANDUP_STILLS[1],
             "caption": "One run opened: what it was asked for, and what it shipped"},
        ],
        "elements": [
            {"kind": "dashboard", "name": "standup", "description": "The board itself."},
        ],
    },
    {
        "slug": "reviewer",
        "name": "Code reviewer",
        "description": "Reads a diff the way a senior reviewer does, and says what would break.",
        "keywords": ["review", "quality"],
        "version": "2.0.0",
        # No icon on purpose: this is the entry that draws the placeholder tile, so a listing
        # where only some publishers ship a mark can be looked at rather than guessed about.
        "gallery": [
            {"url": REVIEWER_STILLS[0], "caption": "What it says when something would break"},
            {"url": REVIEWER_STILLS[1], "caption": "…and when nothing would"},
        ],
        "elements": [
            {"kind": "agent", "name": "reviewer", "description": "The reviewer itself. Wants REVIEWER_GITHUB_TOKEN."},
        ],
    },
    {
        "slug": "crm-suite",
        "name": "CRM suite",
        "description": "A follow-up agent, its import tool, the backend it answers on, and the board that watches both.",
        "keywords": ["sales", "contacts", "follow-up"],
        "version": "0.4.2",
        "icon": ICONS["crm-suite"],
        "readme": CRM_README,
        # A clip in the middle of the strip, so the thumbnail that carries the play glyph is not
        # the one the stage opens on — which is the arrangement worth looking at.
        "gallery": [
            {"url": CRM_STILLS[0], "caption": "Who has gone quiet, oldest silence first"},
        ] + ([{"url": CRM_CLIP, "poster": CRM_STILLS[0], "kind": "video",
               "caption": "From a contacts export to the first draft"}] if CRM_CLIP else []) + [
            {"url": CRM_STILLS[1], "caption": "The draft sales-bot writes, before anybody sends it"},
        ],
        "elements": [
            {"kind": "agent", "name": "sales-bot", "description": "Drafts the follow-up."},
            {"kind": "tool", "name": "csv-import", "description": "Loads a contacts export."},
            {"kind": "llm", "name": "gpt5", "description": "The model sales-bot answers on."},
            {"kind": "dashboard", "name": "crm", "description": "Who has gone quiet, oldest silence first."},
        ],
    },
    {
        "slug": "ops-kit",
        "name": "On-call kit",
        "description": "Everything an on-call rotation needs, in one bundle: eight elements, its own project.",
        "keywords": ["ops", "on-call", "incidents"],
        "version": "1.0.0",
        "icon": ICONS["ops-kit"],
        "gallery": [
            {"url": OPS_STILLS[0], "caption": "Open incidents, oldest first"},
            {"url": OPS_STILLS[1], "caption": "pager, from the on-call agent's own bin"},
        ],
        "elements": [
            {"kind": "agent", "name": "oncall", "description": "First responder. Reads the alert, hands over."},
            {"kind": "tool", "name": "pager", "description": "Pages whoever is on call."},
            {"kind": "dashboard", "name": "ops", "description": "Open incidents, oldest first."},
            {"kind": "llm", "name": "local-qwen", "description": "A local model, so a page at 3am needs no network."},
            {"kind": "embedding", "name": "bge-small", "description": "Embeds past incidents for the search."},
            {"kind": "service", "name": "incident-queue", "description": "Redis, for the queue. Arrives parked."},
            {"kind": "trigger", "name": "nightly-sweep", "description": "Closes incidents nobody touched. Arrives disabled."},
            {"kind": "project", "name": "ops", "description": "The project the rest of this bundle is filed under."},
        ],
    },
]

for bundle in BUNDLES:
    bundle["repo"] = repo_url(bundle["slug"])
    bundle["commit"] = pin(bundle["slug"])
    bundle["branch"] = "main"

manifest_path.parent.mkdir(parents=True, exist_ok=True)
manifest_path.write_text(
    json.dumps({"name": "Local bundles", "bundles": BUNDLES}, indent=2) + "\n"
)
print(f"wrote {manifest_path} ({manifest_path.stat().st_size // 1024} KiB, {len(BUNDLES)} bundles)")
PY

# ---------------------------------------------------------------- point the dev store at it

"$cli" marketplace remove "$source_name" >/dev/null 2>&1 || true
"$cli" marketplace add "$source_name" "file://$manifest"
"$cli" marketplace sync
"$cli" marketplace apps
