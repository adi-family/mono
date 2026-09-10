#!/usr/bin/env bash
#
# Publish the webapp bundle (crates/adi-webapp) to the shared-assets R2 bucket, under an
# immutable `monoapp/<version>/` prefix — the version comes from scripts/version.sh, the same
# resolver every packaging script and the compiled-in `adi-app::VERSION` use, so a build published
# here lands under exactly the prefix a matching running instance asks for
# (crates/adi-app/src/shared_assets.rs).
#
# `index.html` is never published: it stays served by each instance, and is what points a
# browser's copy at the URLs this script just published (see that same module).
#
# Idempotent in the sense that matters: a file already reachable at its published URL is skipped
# rather than re-uploaded. Safe for the files Trunk itself content-hashes (the wasm, its JS glue,
# both stylesheets, the snippets) because the key *is* the hash — two publishes of the same
# content produce the same key, and different content can never collide onto a key an older,
# already-cached instance is still using. It's *also* harmless for the handful Trunk does not hash
# (manifest.webmanifest, sw.js, assets/*): nothing in crates/adi-app/src/shared_assets.rs ever
# points a browser at this bucket for any of them — they publish here because the decided scope is
# "the whole dist/", not because anything currently reads them back from it.
#
# fonts/*, additionally, are mirrored **unversioned** at the bucket root (overwritten every
# publish) as a **compatibility shim for versions published before the `@font-face` urls in
# crates/adi-ui/fonts/fonts.css became relative** (`url("fonts/…")` rather than the old
# root-absolute `url("/fonts/…")`). A relative `url()` resolves against the *stylesheet's own*
# URL, so a build with the fix serves its fonts from right under its own version prefix — the
# fonts/ this script publishes a few lines above, alongside everything else in dist/ — and needs
# nothing at the bucket root. A version published before the fix still has the old absolute paths
# baked into its stylesheet's SRI hash (dist/index.html's `integrity=`), so those bytes cannot be
# moved or rewritten after the fact; the root mirror is what keeps such a version's fonts loading
# for as long as it stays live. Safe to drop once no published version predates the fix.
#
# Usage:
#   ./scripts/publish.sh                 # trunk build --release, then publish
#   ./scripts/publish.sh --no-build       # publish whatever is already in the private dist dir
set -euo pipefail

. "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/config.sh"

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
private_dist="$repo_root/target/webapp-dist-publish"

do_build=1
for arg in "$@"; do
  case "$arg" in
    --no-build) do_build=0 ;;
    -h|--help) sed -n '2,/^set -euo/p' "$0" | grep '^#' | sed 's/^# \{0,1\}//'; exit 0 ;;
    *) echo "unknown argument: $arg" >&2; exit 2 ;;
  esac
done

die() { echo "error: $*" >&2; exit 1; }
step() { printf '\n\033[1m==> %s\033[0m\n' "$*"; }

command -v wrangler >/dev/null 2>&1 || command -v bunx >/dev/null 2>&1 ||
  die "need 'wrangler' on PATH, or 'bunx' to fetch it on demand"
wrangler() { if command -v wrangler >/dev/null 2>&1; then command wrangler "$@"; else bunx wrangler "$@"; fi; }

# An already-set CLOUDFLARE_API_TOKEN wins outright — that's CI (release.yml passes the
# repository secret this way; there's no secret store on a GitHub runner for `adi-mono` to read
# from). Falling back to the secret store is what makes a hand-run publish from this machine work
# without exporting anything first.
TOKEN="${CLOUDFLARE_API_TOKEN:-}"
[ -n "$TOKEN" ] || TOKEN="$(adi-mono secrets read CLOUDFLARE_API_TOKEN 2>/dev/null || true)"
[ -n "$TOKEN" ] || die "CLOUDFLARE_API_TOKEN is not set — run adi-mono secrets set CLOUDFLARE_API_TOKEN first \
(see ./setup-cf.sh --help for the scopes it needs)"
export CLOUDFLARE_API_TOKEN="$TOKEN"

if [ -z "${CLOUDFLARE_ACCOUNT_ID:-}" ]; then
  accounts="$(curl -sS "https://api.cloudflare.com/client/v4/accounts" -H "authorization: Bearer $TOKEN" | jq -c '.result')"
  [ "$(jq 'length' <<<"$accounts")" = "1" ] ||
    die "this token sees $(jq 'length' <<<"$accounts") accounts; set CLOUDFLARE_ACCOUNT_ID to pick one"
  CLOUDFLARE_ACCOUNT_ID="$(jq -r '.[0].id' <<<"$accounts")"
  export CLOUDFLARE_ACCOUNT_ID
fi

VERSION="$("$repo_root/scripts/version.sh")"
echo "==> version: $VERSION"

if [ "$do_build" = "1" ]; then
  command -v trunk >/dev/null 2>&1 || die "'trunk' is not installed (cargo install trunk)"
  step "trunk build --release  (crates/adi-webapp -> target/webapp-dist-publish/)"
  rm -rf "$private_dist"
  ADI_VERSION="$VERSION" bash -c "cd '$repo_root/crates/adi-webapp' && trunk build --release --dist '$private_dist'"
fi
[ -f "$private_dist/index.html" ] || die "no build at $private_dist — run without --no-build first"

# The `Content-Type` Trunk's own dist needs; `adi-app`'s `content_type()` (src/main.rs) is the
# same mapping applied to the same tree, kept in sync by hand since one is shell and the other
# Rust. `application/wasm` matters most: served as `application/octet-stream` it can still be
# `fetch()`-ed, but `WebAssembly.instantiateStreaming` — the fast path wasm-bindgen's glue takes
# when it's offered — refuses anything else.
content_type_for() {
  case "$1" in
    *.js|*.mjs) echo "text/javascript; charset=utf-8" ;;
    *.wasm) echo "application/wasm" ;;
    *.css) echo "text/css; charset=utf-8" ;;
    *.json|*.map) echo "application/json; charset=utf-8" ;;
    *.webmanifest) echo "application/manifest+json; charset=utf-8" ;;
    *.svg) echo "image/svg+xml" ;;
    *.ico) echo "image/x-icon" ;;
    *.png) echo "image/png" ;;
    *.woff2) echo "font/woff2" ;;
    *.html) echo "text/html; charset=utf-8" ;;
    *) echo "application/octet-stream" ;;
  esac
}

# `key` -> already reachable at the public CDN URL. Used only to skip a redundant upload of a
# file a previous run already published; see the header above on why a false negative here (a
# transient network hiccup, DNS not yet live) is harmless rather than unsafe — the worst it costs
# is re-uploading bytes that were already there under the same, content-derived key.
already_published() {
  code="$(curl -s -o /dev/null -w '%{http_code}' --max-time 10 "https://$DOMAIN/$1" 2>/dev/null || echo 000)"
  [ "$code" = "200" ]
}

put() {
  local key="$1" file="$2" cache_control="$3"
  # `--remote`: wrangler's r2 object commands default to a local dev simulator, not the real
  # bucket — omitting this is the failure mode that looks like success (exits 0, nothing on R2).
  wrangler r2 object put "$BUCKET/$key" --file "$file" --remote \
    --content-type "$(content_type_for "$file")" --cache-control "$cache_control" >/dev/null
}

step "Publishing monoapp/$VERSION/ from $private_dist"
uploaded=0 skipped=0
while IFS= read -r -d '' file; do
  rel="${file#"$private_dist"/}"
  [ "$rel" = "index.html" ] && continue
  key="monoapp/$VERSION/$rel"
  if already_published "$key"; then
    skipped=$((skipped + 1))
    continue
  fi
  put "$key" "$file" "public, max-age=31536000, immutable"
  uploaded=$((uploaded + 1))
  echo "  $key"
done < <(find "$private_dist" -type f -print0)
echo "uploaded $uploaded, already present $skipped"

step "Mirroring fonts/ unversioned at the bucket root (compat shim for pre-relative-url versions — see the header above)"
if [ -d "$private_dist/fonts" ]; then
  while IFS= read -r -d '' file; do
    rel="${file#"$private_dist"/fonts/}"
    put "fonts/$rel" "$file" "public, max-age=86400"
    echo "  fonts/$rel"
  done < <(find "$private_dist/fonts" -type f -print0)
else
  echo "  no fonts/ in this build — nothing to mirror"
fi

echo
echo "Published. https://$DOMAIN/monoapp/$VERSION/ is what a shell built at this version now asks for."
