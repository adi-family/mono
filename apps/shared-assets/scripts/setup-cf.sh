#!/usr/bin/env bash
#
# Stand the shared-assets R2 bucket up on a Cloudflare account from nothing. Modeled closely on
# apps/oauth-router/scripts/setup-cf.sh and apps/docs/scripts/setup-cf.sh — see those for the
# fuller "why" on the parts this shares with them (credential resolution, the zone-on-this-account
# check, why a proxied CNAME can't be verified with `dig`).
#
# Three things have to be true for a published build to be reachable, and only the first two are
# in git:
#
#   1. the bucket exists, with a CORS policy that lets any origin fetch from it — every ADI
#      instance is a different origin (app.adi, a fleet node's <service>.<node>.n.adi, a bare
#      IP:port during setup), and none of that may be baked into the bucket, so the rule has to
#      admit all of them rather than list them
#   2. cdn.withadi.dev is attached to the bucket as a custom domain, AND a CNAME for it exists —
#      R2 manages this DNS record itself once the zone is confirmed on this account, unlike a
#      Pages custom domain, which leaves it to this script (see apps/docs's version for that path)
#   3. the outer router that maps requests to cdn.withadi.dev through to Cloudflare (i.e. that
#      nothing shadows this hostname) — there is nothing to do for this one; a subdomain with its
#      own DNS record needs no entry anywhere else in this repo
#
# This does 1-2 and is idempotent, so it also doubles as the "did anything drift?" check.
#
# Auth: CLOUDFLARE_API_TOKEN, read from this machine's secret store — never a `wrangler login`,
# which needs a browser this shell doesn't have. The token needs Account > Workers R2 Storage >
# Edit, plus Zone > Zone > Read and Zone > DNS > Edit on withadi.dev (R2 writes the custom
# domain's CNAME itself, but attaching it still has to confirm the zone).
#
# Usage:
#   ./scripts/setup-cf.sh
#   CLOUDFLARE_ACCOUNT_ID=… ./scripts/setup-cf.sh    # only needed if the token sees >1 account
#
# The R2 CORS and custom-domain request shapes below are Cloudflare's documented API as of this
# writing (API reference, "R2 Bucket CORS" / "R2 Bucket Custom Domains"). Neither has been
# exercised against a live account by this script — that's the one thing left for whoever runs it
# to confirm; a shape mismatch shows up as a `cloudflare: [<code>] <message>` from `cf()` below,
# not a silent no-op.
set -euo pipefail

BUCKET="adi-shared-assets"
DOMAIN="cdn.withadi.dev"
API="https://api.cloudflare.com/client/v4"

cd "$(dirname "$0")/.."

while [ $# -gt 0 ]; do
  case "$1" in
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
  Account > Workers R2 Storage > Edit
  Zone > Zone > Read        (on withadi.dev)
  Zone > DNS > Edit         (on withadi.dev)

Re-run this script once it's set.
MSG
  exit 0
fi
export CLOUDFLARE_API_TOKEN="$TOKEN"

# `cf <method> <path> [json-body]` -> the API result, or a fatal error with the API's messages.
cf() {
  local method="$1" path="$2" body="${3:-}"
  local args=(-sS -X "$method" "$API$path" -H "authorization: Bearer $TOKEN")
  [ -n "$body" ] && args+=(-H "content-type: application/json" -d "$body")
  local out; out="$(curl "${args[@]}")" || die "$method $path: curl failed"
  if [ "$(jq -r '.success' <<<"$out")" != "true" ]; then
    echo "$out" | jq -r '.errors[]? | "  cloudflare: [\(.code)] \(.message)"' >&2
    die "$method $path failed"
  fi
  jq -c '.result' <<<"$out"
}

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

# A custom domain's CNAME is only written for free (by R2 itself, below) if the zone is on this
# same account — same caveat as apps/docs's Pages domain.
zone="$(cf GET "/zones?name=${DOMAIN#*.}")"
ZONE_ID="$(jq -r '.[0].id // ""' <<<"$zone")"
if [ "$(jq -r '.[0].account.id // ""' <<<"$zone")" != "$CLOUDFLARE_ACCOUNT_ID" ]; then
  echo "  warning: the ${DOMAIN#*.} zone is not on this account — $DOMAIN cannot be attached" >&2
  echo "  as a custom domain from here; it has to be done from wherever that zone lives." >&2
else
  echo "zone ${DOMAIN#*.} is on this account ($(jq -r '.[0].status' <<<"$zone")), id $ZONE_ID"
fi

step "Bucket: $BUCKET"
if cf_exists "/accounts/$CLOUDFLARE_ACCOUNT_ID/r2/buckets/$BUCKET"; then
  echo "already exists"
else
  cf POST "/accounts/$CLOUDFLARE_ACCOUNT_ID/r2/buckets" \
    "$(jq -nc --arg name "$BUCKET" '{name: $name}')" >/dev/null
  echo "created"
fi

step "CORS"
# Every origin, because every origin is one of ours and none of them is knowable in advance: an
# ADI panel may be app.adi, https://app.adi, a bare loopback port during setup, or a fleet node's
# own <service>.<node>.n.adi — the whole reason this bucket exists is to be reachable from all of
# them without a single one of their identities appearing anywhere in the URL (see
# crates/adi-app/src/shared_assets.rs). GET/HEAD only: nothing here is ever written by a browser.
# `headers: ["*"]` is what lets the `crossorigin="anonymous"` fetch (needed for the SRI check on
# the stylesheet links) send its default request headers without a preflight rejection.
cf PUT "/accounts/$CLOUDFLARE_ACCOUNT_ID/r2/buckets/$BUCKET/cors" "$(jq -nc '{
  rules: [{
    id: "adi-shared-assets: readable from any origin",
    allowed: { methods: ["GET", "HEAD"], origins: ["*"], headers: ["*"] },
    exposeHeaders: ["ETag", "Content-Type"],
    maxAgeSeconds: 86400
  }]
}')" >/dev/null
echo "set: GET/HEAD from any origin"

step "Custom domain: $DOMAIN"
domains_path="/accounts/$CLOUDFLARE_ACCOUNT_ID/r2/buckets/$BUCKET/domains/custom"
# `(.domains // .)`: tolerates the list coming back either as a bare array or wrapped in a
# `domains` key (bucket-listing responses elsewhere in this API use the wrapped form) — whichever
# it is here, this reads it either way rather than guessing wrong and always reporting "not found".
if cf GET "$domains_path" | jq -e --arg d "$DOMAIN" 'any((.domains // .)[]?; .domain == $d)' >/dev/null 2>&1; then
  echo "already attached"
elif [ -z "$ZONE_ID" ]; then
  echo "  skipped — the zone isn't on this account (see the warning above)" >&2
else
  cf POST "$domains_path" "$(jq -nc --arg domain "$DOMAIN" --arg zone "$ZONE_ID" \
    '{domain: $domain, zoneId: $zone, enabled: true, minTLS: "1.2"}')" >/dev/null
  echo "attached — R2 writes its own CNAME once this settles; give it a few minutes"
fi

step "Verify"
echo "  https://$DOMAIN/ -> $(curl -s -o /dev/null -w '%{http_code}' --max-time 15 "https://$DOMAIN/" 2>/dev/null || true)"
echo "  (404 here is fine and expected — a bare bucket root, or before anything is published;"
echo "   what matters is that it isn't a connection failure or a certificate error)"

echo
echo "Next: ./scripts/publish.sh publishes a build."
