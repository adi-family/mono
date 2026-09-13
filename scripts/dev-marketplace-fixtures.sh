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

import base64, json, subprocess, sys
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


def shot(title, rows):
    """A still for the gallery: what the publisher would have screenshotted, at 1200x750.

    No orange anywhere in it, deliberately. A gallery image is drawn on the same screen as the
    panel's own one filled accent (§8), and a mock screenshot is not worth spending it on.
    """
    lines = "".join(
        f'<rect x="40" y="{124 + i * 58}" width="{w}" height="11" rx="5" fill="{DIM}"/>'
        f'<rect x="{40 + w + 24}" y="{124 + i * 58}" width="90" height="11" rx="5" fill="#3A3A3A"/>'
        f'<line x1="40" y1="{158 + i * 58}" x2="1160" y2="{158 + i * 58}" stroke="#242424"/>'
        for i, w in enumerate(rows)
    )
    return data_uri(
        '<svg xmlns="http://www.w3.org/2000/svg" width="1200" height="750" viewBox="0 0 1200 750">'
        '<rect width="1200" height="750" fill="#161616"/>'
        '<rect width="1200" height="76" fill="#101010"/>'
        f'<text x="40" y="48" font-family="system-ui,sans-serif" font-size="24" fill="#ECEAE6">{title}</text>'
        '<rect x="1040" y="24" width="120" height="28" rx="6" fill="#2A2A2A"/>'
        f'{lines}</svg>'
    )


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

BUNDLES = [
    {
        "slug": "changelog",
        "name": "Changelog tools",
        "description": "Turn a range of commits into release notes, and refuse a subject nobody can read.",
        "keywords": ["git", "release", "notes"],
        "version": "1.2.0",
        "icon": ICONS["changelog"],
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
            {"url": shot("Standup · yesterday",
                         [420, 300, 360, 250, 390, 280, 330, 450, 270, 360, 240]),
             "caption": "The board itself, newest run first"},
            {"url": shot("Standup · one run",
                         [520, 340, 300, 470, 360, 280, 420, 250, 380, 310, 290]),
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
