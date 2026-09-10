# Shared config for setup-cf.sh and publish.sh — sourced, not run. Keeping the bucket and
# domain in one place is the point: a stray wrangler.toml used to be a third place these could
# drift apart, and it's gone (see git history) — this is the one place left.

BUCKET="adi-shared-assets"
DOMAIN="cdn.withadi.dev"
