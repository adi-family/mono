#!/usr/bin/env bash
#
# Upload the release signing secrets to GitHub, from one local file you fill in once.
#
#   scripts/set-release-secrets.sh --check    # validate everything, upload nothing
#   scripts/set-release-secrets.sh            # validate, then set the secrets
#
# The file lives OUTSIDE the repo — `~/.adi/release-secrets.env` by default, override with
# $ADI_RELEASE_SECRETS. Outside on purpose: a secrets file inside a working tree is one
# `git add -A` away from being public, and this repo is public.
#
# Nothing here prints a secret value. Failures name the *field*, never its contents.
#
# The required secret names are read out of .github/workflows/release.yml rather than listed
# here, so renaming one in the workflow can't leave this script quietly setting the old name.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
FILE="${ADI_RELEASE_SECRETS:-$HOME/.adi/release-secrets.env}"
REPO="${ADI_UPDATE_REPO:-adi-family/mono}"
WORKFLOW="$ROOT/.github/workflows/release.yml"
CHECK_ONLY=false
TEMPLATE=false

case "${1:-}" in
    --check)    CHECK_ONLY=true ;;
    --template) TEMPLATE=true ;;
    "")         ;;
    *) echo "usage: $0 [--template | --check]" >&2; exit 2 ;;
esac

die()  { echo "✗ $*" >&2; exit 1; }
ok()   { echo "✓ $*"; }
warn() { echo "⚠ $*" >&2; }

# ── --template: write the file, pre-filling what this machine already knows ─────────────────
# Three of the five are already in apps/macos/.env (release.sh has always read them from
# there). Copying them across means the only things left to fill in by hand are the two that
# come from the certificate export.
if $TEMPLATE; then
    [ ! -f "$FILE" ] || die "$FILE already exists — edit it, or delete it to start over"
    mkdir -p "$(dirname "$FILE")"
    ( umask 077; : > "$FILE" )   # created 600 before a single secret is written into it

    TEAM_ID=""; AC_USER=""; AC_PASS=""
    if [ -f "$ROOT/apps/macos/.env" ]; then
        set -a; . "$ROOT/apps/macos/.env"; set +a
    fi

    {
        echo "# Release signing secrets for $REPO — read by scripts/set-release-secrets.sh."
        echo "# Kept outside the repo on purpose: that tree is public."
        echo "# Fill the two blanks, then run:  scripts/set-release-secrets.sh"
        echo
        echo "# The exported Developer ID certificate, as a PATH to the .p12 (not base64 —"
        echo "# the script encodes it). Keychain Access → My Certificates → the"
        echo "# 'Developer ID Application' row → right-click → Export → .p12, set a password."
        echo "MACOS_CERT_P12_FILE="
        echo
        echo "# The password you typed in that export dialog."
        echo "MACOS_CERT_PASSWORD="
        echo
        if [ -n "${AC_USER:-}" ]; then
            echo "# ↓ copied from apps/macos/.env"
        fi
        printf 'APPLE_ID=%s\n' "${AC_USER:-}"
        printf 'APPLE_APP_PASSWORD=%s\n' "${AC_PASS:-}"
        printf 'APPLE_TEAM_ID=%s\n' "${TEAM_ID:-}"
    } >> "$FILE"

    ok "wrote $FILE (mode 600)"
    [ -n "${AC_USER:-}" ] && ok "pre-filled APPLE_ID / APPLE_APP_PASSWORD / APPLE_TEAM_ID from apps/macos/.env"
    echo
    echo "Left to fill in: MACOS_CERT_P12_FILE and MACOS_CERT_PASSWORD."
    echo "Then check it with:  $0 --check"
    exit 0
fi

# ── the file ────────────────────────────────────────────────────────────────────────────────
[ -f "$FILE" ] || die "no secrets file at $FILE
   create it from the template:  scripts/set-release-secrets.sh --template"

if [ "$(stat -f '%OLp' "$FILE" 2>/dev/null || stat -c '%a' "$FILE")" != "600" ]; then
    warn "$FILE is readable by more than you — fixing with chmod 600"
    chmod 600 "$FILE"
fi

set -a
# shellcheck disable=SC1090
. "$FILE"
set +a

# ── what the workflow actually asks for ─────────────────────────────────────────────────────
[ -f "$WORKFLOW" ] || die "missing $WORKFLOW — nothing to read the secret names from"
REQUIRED="$(grep -o 'secrets\.[A-Z_][A-Z0-9_]*' "$WORKFLOW" | cut -d. -f2 | sort -u)"
[ -n "$REQUIRED" ] || die "found no secrets.* references in $WORKFLOW"

# ── the certificate: a path here, not a base64 blob ─────────────────────────────────────────
# Pasting a 4KB base64 string into a file by hand is how a truncated certificate gets shipped.
# Point at the .p12 and let this script encode it.
[ -n "${MACOS_CERT_P12_FILE:-}" ] || die "MACOS_CERT_P12_FILE is empty in $FILE"
P12="${MACOS_CERT_P12_FILE/#\~/$HOME}"
[ -f "$P12" ] || die "no such file: $P12  (MACOS_CERT_P12_FILE)"
[ -n "${MACOS_CERT_PASSWORD:-}" ] || die "MACOS_CERT_PASSWORD is empty in $FILE"

for field in APPLE_ID APPLE_APP_PASSWORD APPLE_TEAM_ID; do
    [ -n "${!field:-}" ] || die "$field is empty in $FILE"
done

# ── validate before uploading ───────────────────────────────────────────────────────────────
# The Team ID is the one that fails silently: adi-update refuses any bundle not signed by
# DEFAULT_TEAM_ID, so a mismatch here ships releases that every machine downloads and rejects.
EXPECTED_TEAM="$(sed -n 's/.*DEFAULT_TEAM_ID: &str = "\([^"]*\)".*/\1/p' "$ROOT/crates/adi-update/src/engine.rs")"
[ -n "$EXPECTED_TEAM" ] || die "could not read DEFAULT_TEAM_ID from crates/adi-update/src/engine.rs"
[ "$APPLE_TEAM_ID" = "$EXPECTED_TEAM" ] \
    || die "APPLE_TEAM_ID does not match adi-update's DEFAULT_TEAM_ID ($EXPECTED_TEAM)
   every machine would reject releases signed by any other team"
ok "APPLE_TEAM_ID matches DEFAULT_TEAM_ID ($EXPECTED_TEAM)"

case "$APPLE_ID" in
    *@*.*) ok "APPLE_ID looks like an Apple ID" ;;
    *) die "APPLE_ID does not look like an email address" ;;
esac

# Apple's app-specific passwords are always xxxx-xxxx-xxxx-xxxx. Catching the wrong password
# here beats discovering it after a 20-minute build, at the notarization step.
case "$APPLE_APP_PASSWORD" in
    ????-????-????-????) ok "APPLE_APP_PASSWORD has the app-specific-password shape" ;;
    *) die "APPLE_APP_PASSWORD is not in Apple's xxxx-xxxx-xxxx-xxxx form
   generate one at https://account.apple.com → Sign-In and Security → App-Specific Passwords
   (your Apple account password will NOT work for notarization)" ;;
esac

# Import into a throwaway keychain — the same act the CI job performs, so a certificate that
# fails here would have failed there, only 20 minutes later and with a worse error.
KEYCHAIN="$(mktemp -u "${TMPDIR:-/tmp}/adi-secret-check-XXXXXX").keychain-db"
KEYCHAIN_PW="$(uuidgen)"
cleanup() { security delete-keychain "$KEYCHAIN" 2>/dev/null || true; }
trap cleanup EXIT
security create-keychain -p "$KEYCHAIN_PW" "$KEYCHAIN" >/dev/null
security unlock-keychain -p "$KEYCHAIN_PW" "$KEYCHAIN" >/dev/null
if ! security import "$P12" -k "$KEYCHAIN" -P "$MACOS_CERT_PASSWORD" -T /usr/bin/codesign >/dev/null 2>&1; then
    die "the .p12 would not import — wrong MACOS_CERT_PASSWORD, or the file is not a PKCS#12 identity
   re-export from Keychain Access → My Certificates → the 'Developer ID Application' row → Export"
fi
IDENTITY_COUNT="$(security find-identity -v -p codesigning "$KEYCHAIN" 2>/dev/null \
    | grep -c "Developer ID Application.*($EXPECTED_TEAM)" || true)"
[ "$IDENTITY_COUNT" -ge 1 ] \
    || die "the .p12 imported, but holds no 'Developer ID Application' identity for team $EXPECTED_TEAM
   exporting the certificate alone is not enough — the export must include its private key"
ok "the .p12 imports and carries a Developer ID Application identity for $EXPECTED_TEAM"
cleanup
trap - EXIT

# `-b 0` keeps BSD base64 from wrapping; the workflow decodes either form, but an unwrapped
# blob is one less thing to wonder about when reading the secret back.
CERT_B64="$(base64 -b 0 -i "$P12" 2>/dev/null || base64 -i "$P12" | tr -d '\n')"
[ ${#CERT_B64} -gt 100 ] || die "the base64 of $P12 came out suspiciously short"
ok "certificate encoded (${#CERT_B64} base64 chars)"

# Every name the workflow references must be one we are about to set.
MISSING=""
for name in $REQUIRED; do
    case "$name" in
        MACOS_CERT_P12) continue ;;   # supplied as a file path, encoded above
    esac
    [ -n "${!name:-}" ] || MISSING="$MISSING $name"
done
[ -z "$MISSING" ] || die "the workflow needs secrets this file does not set:$MISSING"
ok "every secret $WORKFLOW references is accounted for"

if $CHECK_ONLY; then
    echo
    echo "--check: everything validates. Re-run without --check to set them on $REPO."
    exit 0
fi

# ── upload ──────────────────────────────────────────────────────────────────────────────────
command -v gh >/dev/null 2>&1 || die "the GitHub CLI (gh) is required — brew install gh && gh auth login"
gh repo view "$REPO" >/dev/null 2>&1 || die "cannot reach $REPO with the current gh login"

echo
echo "==> setting secrets on $REPO"
# Values go over stdin, never as an argument: an argument is visible to anyone who can run `ps`
# while this is running.
set_secret() { printf '%s' "$2" | gh secret set "$1" --repo "$REPO" >/dev/null && ok "set $1"; }

set_secret MACOS_CERT_P12      "$CERT_B64"
set_secret MACOS_CERT_PASSWORD "$MACOS_CERT_PASSWORD"
set_secret APPLE_ID            "$APPLE_ID"
set_secret APPLE_APP_PASSWORD  "$APPLE_APP_PASSWORD"
set_secret APPLE_TEAM_ID       "$APPLE_TEAM_ID"

echo
echo "==> now on $REPO:"
gh secret list --repo "$REPO"
echo
echo "Dry run (builds all three platforms, publishes nothing):"
echo "  gh workflow run Release --repo $REPO -f version=0.2.0"
echo "Real release:"
echo "  git tag v0.2.0 && git push --tags"
