#!/usr/bin/env bash
# Install binaryen's wasm-opt user-locally (no sudo), pinned to the version Trunk.toml's
# `[tools] wasm-opt` names — the two have to agree or Trunk ignores this binary and downloads
# its own copy into ~/.cache/trunk instead (harmless, just a second copy and a network call).
#
# Usage: scripts/install-binaryen.sh
set -euo pipefail

version="version_133"
dest="$HOME/.local/share/binaryen-$version"
bin_dir="$HOME/.local/bin"

if "$bin_dir/wasm-opt" --version 2>/dev/null | grep -q "$version"; then
  echo "==> wasm-opt $version already installed at $bin_dir/wasm-opt"
  exit 0
fi

os="linux"
case "$(uname -s)" in
  Darwin) os="macos" ;;
  Linux) os="linux" ;;
  *) echo "error: unsupported OS $(uname -s)" >&2; exit 1 ;;
esac
arch="x86_64"
case "$(uname -m)" in
  arm64|aarch64) arch="aarch64" ;;
  x86_64) arch="x86_64" ;;
  *) echo "error: unsupported architecture $(uname -m)" >&2; exit 1 ;;
esac
# binaryen's own release asset naming is irregular: an arm64 macOS build is "arm64-macos",
# every other combination is "<arch>-<os>" (matching Trunk's own tools.rs url() table, which
# this mirrors on purpose — same releases, same install target).
asset="binaryen-$version-$arch-$os.tar.gz"
if [[ "$os" == "macos" && "$arch" == "aarch64" ]]; then
  asset="binaryen-$version-arm64-macos.tar.gz"
fi
url="https://github.com/WebAssembly/binaryen/releases/download/$version/$asset"

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
echo "==> downloading $url"
curl -fsSL -o "$tmp/binaryen.tar.gz" "$url"

rm -rf "$dest"
mkdir -p "$dest"
tar xzf "$tmp/binaryen.tar.gz" -C "$tmp"
mv "$tmp"/binaryen-"$version"/* "$dest/"

mkdir -p "$bin_dir"
ln -sf "$dest/bin/wasm-opt" "$bin_dir/wasm-opt"
echo "==> installed: $("$bin_dir/wasm-opt" --version)"
echo "==> make sure $bin_dir is on PATH"
