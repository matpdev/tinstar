#!/bin/sh
# Installs tinstar from the GitHub releases (Linux and macOS, x86_64 and arm64).
#
#   curl -fsSL https://raw.githubusercontent.com/matpdev/tinstar/main/install.sh | sh
#
# TINSTAR_VERSION      a release tag such as v0.1.0 (default: the latest release)
# TINSTAR_INSTALL_DIR  where the binary goes (default: ~/.local/bin)
set -eu

repo="matpdev/tinstar"
version="${TINSTAR_VERSION:-latest}"
dir="${TINSTAR_INSTALL_DIR:-$HOME/.local/bin}"

fail() { echo "tinstar install: $*" >&2; exit 1; }

case "$(uname -s)" in
  Linux) os=unknown-linux-gnu ;;
  Darwin) os=apple-darwin ;;
  *) fail "no prebuilt binary for $(uname -s); build it with: cargo install --git https://github.com/$repo tinstar" ;;
esac
case "$(uname -m)" in
  x86_64 | amd64) arch=x86_64 ;;
  aarch64 | arm64) arch=aarch64 ;;
  *) fail "no prebuilt binary for $(uname -m); build it with: cargo install --git https://github.com/$repo tinstar" ;;
esac
asset="tinstar-$arch-$os.tar.gz"

if [ "$version" = latest ]; then
  url="https://github.com/$repo/releases/latest/download/$asset"
else
  url="https://github.com/$repo/releases/download/$version/$asset"
fi

if command -v curl >/dev/null 2>&1; then
  fetch() { curl -fsSL "$1" -o "$2"; }
elif command -v wget >/dev/null 2>&1; then
  fetch() { wget -qO "$2" "$1"; }
else
  fail "needs curl or wget"
fi

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

echo "Downloading $url"
fetch "$url" "$tmp/$asset" || fail "download failed ($url)"
fetch "$url.sha256" "$tmp/$asset.sha256" || fail "checksum download failed"

expected="$(cut -d' ' -f1 "$tmp/$asset.sha256")"
if command -v sha256sum >/dev/null 2>&1; then
  actual="$(sha256sum "$tmp/$asset" | cut -d' ' -f1)"
else
  actual="$(shasum -a 256 "$tmp/$asset" | cut -d' ' -f1)"
fi
[ "$expected" = "$actual" ] || fail "checksum mismatch for $asset"

tar xzf "$tmp/$asset" -C "$tmp"
mkdir -p "$dir"
# Replace rather than overwrite: an editor may be running the old binary.
cp "$tmp/tinstar-$arch-$os/tinstar" "$dir/tinstar.new"
chmod +x "$dir/tinstar.new"
mv -f "$dir/tinstar.new" "$dir/tinstar"

echo "Installed $("$dir/tinstar" --version) to $dir/tinstar"
case ":$PATH:" in
  *":$dir:"*) ;;
  *) echo "Add $dir to your PATH to run it as 'tinstar'." ;;
esac
