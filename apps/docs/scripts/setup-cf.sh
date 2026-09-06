#!/usr/bin/env bash
#
# Stand the docs site up on a Cloudflare account from nothing. Modeled closely on
# apps/oauth-router/scripts/setup-cf.sh — see that script for the fuller "why" on each step.
#
# Three things have to be true for the site to be reachable, and only the first is in git:
#
#   1. the Pages project exists and has this build uploaded to it
#   2. docs.withadi.dev is attached to the project as a custom domain, AND a CNAME for it
#      points at adi-docs.pages.dev (attaching does not create the record)
#   3. the outer router that maps docs.withadi.dev/mono/* (and, later, /cloud/*) to the right
#      Pages project actually forwards here — that piece lives outside this script and this repo
#
# This does 1-2 and is idempotent, so it also doubles as the "did anything drift?" check.
#
# Auth: CLOUDFLARE_API_TOKEN, read from this machine's secret store — never a `wrangler login`,
# which needs a browser this shell doesn't have. The token needs Account > Cloudflare Pages >
# Edit, plus Zone > Zone > Read and Zone > DNS > Edit on withadi.dev for the domain-attach step.
#
# Usage:
#   ./scripts/setup-cf.sh
#   CLOUDFLARE_ACCOUNT_ID=… ./scripts/setup-cf.sh    # only needed if the token sees >1 account
#
set -euo pipefail

PROJECT="adi-docs"
DOMAIN="docs.withadi.dev"
PRODUCTION_BRANCH="main"
API="https://api.cloudflare.com/client/v4"

cd "$(dirname "$0")/.."

while [ $# -gt 0 ]; do
  case "$1" in
    # Print the header block: every comment line after the shebang, up to the first blank-ish
    # line of code. Beats a hardcoded line range, which goes stale the moment the header does.
    -h|--help) sed -n '2,/^set -euo/p' "$0" | grep '^#' | sed 's/^# \{0,1\}//'; exit 0 ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
  esac
done

die() { echo "error: $*" >&2; exit 1; }
step() { printf '\n\033[1m==> %s\033[0m\n' "$*"; }

TOKEN="$(adi-mono secrets read CLOUDFLARE_API_TOKEN 2>/dev/null || true)"
if [ -z "$TOKEN" ]; then
  cat <<MSG
CLOUDFLARE_API_TOKEN is not set — nothing to do yet, and that's expected on a first run.

Set it with:
  adi-mono secrets set CLOUDFLARE_API_TOKEN

It needs, scoped globally:
  Account > Cloudflare Pages > Edit
  Zone > Zone > Read        (on withadi.dev)
  Zone > DNS > Edit         (on withadi.dev)

Re-run this script once it's set.
MSG
  exit 0
fi
# Exported so `bunx wrangler` picks it up too — it reads CLOUDFLARE_API_TOKEN from the
# environment on its own, with no `wrangler login` involved.
export CLOUDFLARE_API_TOKEN="$TOKEN"

# `cf <method> <path> [json-body]` -> the API result, or a fatal error with the API's messages.
cf() {
  local method="$1" path="$2" body="${3:-}"
  # Separate `local` on purpose: in one statement bash expands the array literal before the
  # scalars are bound, so "$method" reads as unset and `set -u` kills the script.
  local args=(-sS -X "$method" "$API$path" -H "authorization: Bearer $TOKEN")
  [ -n "$body" ] && args+=(-H "content-type: application/json" -d "$body")
  local out; out="$(curl "${args[@]}")" || die "$method $path: curl failed"
  if [ "$(jq -r '.success' <<<"$out")" != "true" ]; then
    echo "$out" | jq -r '.errors[]? | "  cloudflare: [\(.code)] \(.message)"' >&2
    die "$method $path failed"
  fi
  jq -c '.result' <<<"$out"
}

# The same call as a yes/no question. `cf` dies on a non-2xx, and `exit` from inside an `if`
# condition still exits the script — so an existence check needs its own non-fatal path.
cf_exists() {
  local out
  out="$(curl -sS -X GET "$API$1" -H "authorization: Bearer $TOKEN")" || return 1
  [ "$(jq -r '.success' <<<"$out")" = "true" ]
}

step "Checking the credentials"
if [ -z "${CLOUDFLARE_ACCOUNT_ID:-}" ]; then
  accounts="$(cf GET /accounts)"
  [ "$(jq 'length' <<<"$accounts")" = "1" ] ||
    die "this token sees $(jq 'length' <<<"$accounts") accounts; set CLOUDFLARE_ACCOUNT_ID to pick one:
$(jq -r '.[] | "  \(.id)  \(.name)"' <<<"$accounts")"
  CLOUDFLARE_ACCOUNT_ID="$(jq -r '.[0].id' <<<"$accounts")"
fi
cf GET "/accounts/$CLOUDFLARE_ACCOUNT_ID" >/dev/null
echo "credentials ok, account $CLOUDFLARE_ACCOUNT_ID"

# The custom domain only gets its CNAME and certificate for free if the zone is on this same
# account. Say so up front rather than leaving a domain stuck in "pending".
zone="$(cf GET "/zones?name=${DOMAIN#*.}")"
ZONE_ID="$(jq -r '.[0].id // ""' <<<"$zone")"
if [ "$(jq -r '.[0].account.id // ""' <<<"$zone")" != "$CLOUDFLARE_ACCOUNT_ID" ]; then
  echo "  warning: the ${DOMAIN#*.} zone is not on this account — the CNAME to" >&2
  echo "  $PROJECT.pages.dev has to be created wherever that zone actually lives." >&2
else
  echo "zone ${DOMAIN#*.} is on this account ($(jq -r '.[0].status' <<<"$zone")), id $ZONE_ID"
fi

step "Pages project: $PROJECT"
if cf_exists "/accounts/$CLOUDFLARE_ACCOUNT_ID/pages/projects/$PROJECT"; then
  echo "already exists"
else
  bunx wrangler pages project create "$PROJECT" --production-branch "$PRODUCTION_BRANCH"
fi

step "Build"
bun install
bun run build

step "Deploy"
# Reads pages_build_output_dir from wrangler.toml. --commit-dirty because this is a hand-run
# deploy, not one driven off a clean git checkout.
bunx wrangler pages deploy --project-name "$PROJECT" --branch "$PRODUCTION_BRANCH" --commit-dirty=true

step "Custom domain: $DOMAIN"
# API-only: there is no `wrangler pages domain` command.
DOMAINS_PATH="/accounts/$CLOUDFLARE_ACCOUNT_ID/pages/projects/$PROJECT/domains"
if cf GET "$DOMAINS_PATH" | jq -e --arg d "$DOMAIN" 'any(.[]; .name == $d)' >/dev/null; then
  echo "already attached"
else
  cf POST "$DOMAINS_PATH" "$(jq -nc --arg name "$DOMAIN" '{name: $name}')" >/dev/null
  echo "attached"
fi
domain_json="$(cf GET "$DOMAINS_PATH" | jq -c --arg d "$DOMAIN" '.[] | select(.name == $d)')"
domain_status="$(jq -r '.status' <<<"$domain_json")"
echo "  status $domain_status, cert $(jq -r '.certificate_authority // "-"' <<<"$domain_json")/$(jq -r '.validation_data.status // "-"' <<<"$domain_json")"

# Everything below is only needed while the domain is not yet serving. `active` already implies
# the DNS record is in place and the certificate issued, so there is nothing left to check —
# and notably the record cannot be checked with `dig CNAME`: it has to be *proxied*, and
# Cloudflare flattens a proxied CNAME to A records at the edge, so a CNAME query comes back
# empty however correct the record is.
if [ "$domain_status" != "active" ]; then
  step "DNS: $DOMAIN -> $PROJECT.pages.dev"
  # Attaching the domain does NOT create this record. The dashboard wizard offers to; the API
  # does not, and the domain then sits at `pending` with "CNAME record not set" indefinitely.
  if [ -z "$ZONE_ID" ]; then
    cat >&2 <<MSG
  the ${DOMAIN#*.} zone is not on this account, so this token cannot write the record either.
  Add one, wherever that zone lives —
    ${DOMAIN#*.} -> DNS -> Add record: CNAME  ${DOMAIN%%.*}  ->  $PROJECT.pages.dev  (proxied)
MSG
  elif cf GET "/zones/$ZONE_ID/dns_records?name=$DOMAIN" | jq -e 'length > 0' >/dev/null; then
    cf GET "/zones/$ZONE_ID/dns_records?name=$DOMAIN" \
      | jq -r '.[] | "  record exists: \(.type) \(.name) -> \(.content) (proxied: \(.proxied))"'
  else
    # Proxied on purpose: the edge is what terminates TLS for the custom domain and routes the
    # hostname to the Pages project.
    cf POST "/zones/$ZONE_ID/dns_records" "$(jq -nc \
      --arg name "$DOMAIN" --arg content "$PROJECT.pages.dev" \
      '{type: "CNAME", name: $name, content: $content, proxied: true,
        comment: "adi-docs Pages custom domain"}')" >/dev/null
    echo "  created CNAME $DOMAIN -> $PROJECT.pages.dev (proxied)"
  fi

  # An empty PATCH re-runs verification. Worth doing unconditionally here: a domain attached
  # before its record existed stays `pending` (or lands in an `error` state) until something
  # asks it to look again, and this is that something.
  echo "  re-triggering verification"
  cf PATCH "$DOMAINS_PATH/$DOMAIN" '{}' \
    | jq -r '"  now: \(.status) (\(.verification_data.error_message // "no error"))"'
  echo "  a 522 from here on is expected until verification completes — the edge is still"
  echo "  treating $PROJECT.pages.dev as an ordinary origin rather than routing to the project."
fi

step "Verify"
echo "  https://$PROJECT.pages.dev/ -> $(curl -s -o /dev/null -w '%{http_code}' --max-time 15 "https://$PROJECT.pages.dev/" 2>/dev/null || true)"
# The custom domain's certificate can take a few minutes and the CNAME is written by
# Cloudflare's own side, not by this token; a blank or 5xx here usually just means "not yet".
echo "  https://$DOMAIN/ -> $(curl -s -o /dev/null -w '%{http_code}' --max-time 15 "https://$DOMAIN/" 2>/dev/null || true)"

echo
echo "Still to do, outside this script:"
echo "  - the router that maps $DOMAIN/mono/* (and later /cloud/*) to the right Pages project"
echo "  - confirm https://$PROJECT.pages.dev/mono/ renders the site once that routing exists"
