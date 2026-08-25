#!/usr/bin/env bash
#
# Stand the OAuth router up on a Cloudflare account from nothing.
#
# Written because the account it used to live on is gone: the Pages project, its secrets and
# the oauth-router.withadi.dev custom domain were all account-scoped, and none of them came
# across the migration. Everything here is idempotent, so it is also the "did anything drift?"
# check — run it again any time and it will only do what is still missing.
#
# Four things have to be true for a login to work, and only the first is in git:
#
#   1. the Pages project exists and has this deployment uploaded to it
#   2. STATE_SECRET and each provider's client id/secret are set as Pages secrets
#   3. oauth-router.withadi.dev is attached to the project as a custom domain, AND a CNAME
#      for it points at adi-oauth-router.pages.dev (attaching does not create the record)
#   4. each provider has https://oauth-router.withadi.dev/callback/<provider> registered
#
# It does 1-3. Step 4 is on the provider's own console and is printed at the end.
#
# Auth: `npx wrangler login` is enough. Attaching a custom domain to a Pages project has no
# wrangler command and has to go through the REST API, but wrangler's stored OAuth token is
# accepted as a bearer token there, so this script borrows it rather than asking for a second
# credential. CLOUDFLARE_API_TOKEN overrides it if you'd rather use a scoped API token — it
# needs Account > Cloudflare Pages > Edit, plus Zone > Zone > Read and Zone > DNS > Edit on
# withadi.dev. CLOUDFLARE_ACCOUNT_ID is likewise optional while the login has exactly one
# account.
#
# Usage:
#   npx wrangler login && ./scripts/setup-cf.sh
#   ./scripts/setup-cf.sh --secrets-from .dev.vars      # take secret values from a file
#   CLOUDFLARE_ACCOUNT_ID=… CLOUDFLARE_API_TOKEN=… ./scripts/setup-cf.sh
#
set -euo pipefail

PROJECT="adi-oauth-router"
DOMAIN="oauth-router.withadi.dev"
PRODUCTION_BRANCH="main"
API="https://api.cloudflare.com/client/v4"

cd "$(dirname "$0")/.."

SECRETS_FILE=""
FORCE_SECRETS=0
while [ $# -gt 0 ]; do
  case "$1" in
    --secrets-from) SECRETS_FILE="${2:?--secrets-from needs a path}"; shift 2 ;;
    --force-secrets) FORCE_SECRETS=1; shift ;;
    # Print the header block: every comment line after the shebang, up to the first blank-ish
    # line of code. Beats a hardcoded line range, which goes stale the moment the header does.
    -h|--help) sed -n '2,/^set -euo/p' "$0" | grep '^#' | sed 's/^# \{0,1\}//'; exit 0 ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
  esac
done

die() { echo "error: $*" >&2; exit 1; }
step() { printf '\n\033[1m==> %s\033[0m\n' "$*"; }

# The bearer token for the REST calls. An explicit CLOUDFLARE_API_TOKEN wins; otherwise reuse
# the OAuth token `wrangler login` wrote, which the API accepts too. Deliberately kept in a
# local of its own and never exported: wrangler prefers CLOUDFLARE_API_TOKEN over its own
# login when it sees it in the environment, and an OAuth token in that slot is rejected.
WRANGLER_CONFIG="${WRANGLER_CONFIG:-$HOME/Library/Preferences/.wrangler/config/default.toml}"
[ -f "$WRANGLER_CONFIG" ] || WRANGLER_CONFIG="$HOME/.config/.wrangler/config/default.toml"
TOKEN="${CLOUDFLARE_API_TOKEN:-}"
if [ -z "$TOKEN" ] && [ -f "$WRANGLER_CONFIG" ]; then
  TOKEN="$(sed -n 's/^[[:space:]]*oauth_token[[:space:]]*=[[:space:]]*"\(.*\)"[[:space:]]*$/\1/p' \
    "$WRANGLER_CONFIG" | head -1)"
fi
[ -n "$TOKEN" ] || die "no credentials: run \`npx wrangler login\`, or set CLOUDFLARE_API_TOKEN"

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
# Not /user/tokens/verify — that endpoint only understands API tokens and rejects an OAuth
# one. Listing accounts works for both, and doubles as the account-id lookup.
if [ -z "${CLOUDFLARE_ACCOUNT_ID:-}" ]; then
  accounts="$(cf GET /accounts)"
  [ "$(jq 'length' <<<"$accounts")" = "1" ] ||
    die "this login sees $(jq 'length' <<<"$accounts") accounts; set CLOUDFLARE_ACCOUNT_ID to pick one:
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
  npx wrangler pages project create "$PROJECT" --production-branch "$PRODUCTION_BRANCH"
fi

step "Secrets"
# The provider credentials are optional in the sense that a provider with no client id is
# simply not enabled (see src/providers.ts) — so only STATE_SECRET is needed for a working,
# if empty, router, and it is generated below when nothing supplies one. Values come from
# --secrets-from, else the environment, else a prompt; a name that resolves to nothing is
# skipped and reported at the end.
#
# A secret already on the project is left alone. That is the important half: --secrets-from
# .dev.vars is the convenient way to run this, and .dev.vars is exactly where a placeholder
# like STATE_SECRET="dev-only-change-me" lives — overwriting a real secret with it would break
# every signed state in flight and be invisible until a login failed. --force-secrets to
# deliberately rotate.
SECRET_NAMES=(STATE_SECRET GOOGLE_CLIENT_ID GOOGLE_CLIENT_SECRET GITHUB_CLIENT_ID GITHUB_CLIENT_SECRET)
existing="$(cf GET "/accounts/$CLOUDFLARE_ACCOUNT_ID/pages/projects/$PROJECT" \
  | jq -r '[.deployment_configs.production.env_vars // {} | to_entries[]
            | select(.value.type == "secret_text") | .key] | join(" ")')"
missing=()
for name in "${SECRET_NAMES[@]}"; do
  if [ "$FORCE_SECRETS" = 0 ] && [[ " $existing " == *" $name "* ]]; then
    echo "  $name — already set, left alone"
    continue
  fi

  value=""
  if [ -n "$SECRETS_FILE" ] && [ -f "$SECRETS_FILE" ]; then
    # Accept both `NAME=value` and `NAME="value"`, ignoring comments.
    value="$(sed -n "s/^[[:space:]]*$name[[:space:]]*=[[:space:]]*//p" "$SECRETS_FILE" \
      | head -1 | sed 's/^"\(.*\)"$/\1/' | sed "s/^'\(.*\)'$/\1/")"
  fi
  [ -z "$value" ] && value="$(printenv "$name" || true)"
  if [ -z "$value" ] && [ -t 0 ]; then
    read -rsp "  $name (blank to skip): " value; echo
  fi
  # Nothing else has to know STATE_SECRET — it only has to be secret and stable — so mint one
  # rather than leave the router unable to sign a state at all.
  if [ -z "$value" ] && [ "$name" = STATE_SECRET ]; then
    value="$(openssl rand -hex 32)"
    echo "  $name — generated"
  fi
  if [ -z "$value" ]; then
    missing+=("$name"); echo "  $name — skipped"; continue
  fi
  printf '%s' "$value" | npx wrangler pages secret put "$name" --project-name "$PROJECT" >/dev/null
  echo "  $name — set"
done

step "Deploy"
# Reads pages_build_output_dir and [vars] from wrangler.toml. --commit-dirty because this is a
# hand-run deploy, not one driven off a clean git checkout.
npx wrangler pages deploy --project-name "$PROJECT" --branch "$PRODUCTION_BRANCH" --commit-dirty=true

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
  # A wrangler OAuth login cannot write it either — it carries zone:read and no DNS scope.
  if ! cf_exists "/zones/$ZONE_ID/dns_records?name=$DOMAIN"; then
    cat >&2 <<MSG
  cannot read DNS with these credentials (a wrangler login has zone:read but no DNS scope).
  Add one record and the domain verifies itself, either in the dashboard —
    ${DOMAIN#*.} -> DNS -> Add record: CNAME  ${DOMAIN%%.*}  ->  $PROJECT.pages.dev  (proxied)
  or by re-running this script with a token that has Zone > DNS > Edit on ${DOMAIN#*.}.
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
        comment: "adi-oauth-router Pages custom domain"}')" >/dev/null
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
echo "  https://$PROJECT.pages.dev/health -> $(curl -s -o /dev/null -w '%{http_code}' --max-time 15 "https://$PROJECT.pages.dev/health" 2>/dev/null || true)"
# The custom domain's certificate can take a few minutes and the CNAME is written by
# Cloudflare's own side, not by this token; a blank or 5xx here usually just means "not yet".
echo "  https://$DOMAIN/health -> $(curl -s -o /dev/null -w '%{http_code}' --max-time 15 "https://$DOMAIN/health" 2>/dev/null || true)"
curl -s --max-time 15 "https://$DOMAIN/health" | jq . 2>/dev/null || true

echo
echo "Still to do by hand:"
for name in "${missing[@]}"; do
  echo "  - secret $name was never set; a provider with no client id stays disabled"
done
echo "  - register the redirect URI at each provider you enabled:"
echo "      Google: console.cloud.google.com -> Credentials -> https://$DOMAIN/callback/google"
echo "      GitHub: github.com/settings/developers            -> https://$DOMAIN/callback/github"
echo "  - confirm /health lists the providers you expect (above); it is the enabled-provider check"
