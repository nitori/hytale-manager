#!/bin/sh
# Install `hy` into ~/.local/bin.
#
#   curl -LsSf https://raw.githubusercontent.com/nitori/hytale-manager/master/install.sh | sh
#
# Linux only. Windows binaries are published on the releases page but are not installed by
# this script. Set HY_INSTALL_DIR to install somewhere else, e.g. /usr/local/bin under sudo.
set -eu

REPO="nitori/hytale-manager"
BASE="https://github.com/$REPO/releases/latest/download"
BIN_DIR="${HY_INSTALL_DIR:-$HOME/.local/bin}"

err() {
    echo "install.sh: $1" >&2
    exit 1
}

[ "$(uname -s)" = "Linux" ] || err "only Linux is supported; see https://github.com/$REPO/releases/latest"

case "$(uname -m)" in
    x86_64 | amd64) TARGET="x86_64-unknown-linux-musl" ;;
    *) err "no release is published for $(uname -m)" ;;
esac

command -v curl >/dev/null 2>&1 || err "curl is required"
command -v sha256sum >/dev/null 2>&1 || err "sha256sum is required"

ASSET="hy-$TARGET"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

echo "Downloading $ASSET"
curl -LsSf -o "$tmp/$ASSET" "$BASE/$ASSET" || err "could not download $BASE/$ASSET"
curl -LsSf -o "$tmp/SHA256SUMS" "$BASE/SHA256SUMS" || err "could not download $BASE/SHA256SUMS"

# SHA256SUMS covers every platform's asset, so the entry for this one is looked up by name
# rather than checked wholesale: a `-c` run would have to be told to ignore the others, and
# "ignore what is absent" cannot distinguish a foreign entry from a missing one.
expected="$(grep -E "[[:space:]][*]?${ASSET}\$" "$tmp/SHA256SUMS" | cut -d' ' -f1)"
[ -n "$expected" ] || err "SHA256SUMS has no entry for $ASSET"

actual="$(sha256sum "$tmp/$ASSET" | cut -d' ' -f1)"
[ "$actual" = "$expected" ] || err "checksum mismatch for $ASSET: expected $expected, got $actual"

mkdir -p "$BIN_DIR" || err "could not create $BIN_DIR"
install -m 755 "$tmp/$ASSET" "$BIN_DIR/hy" || err "could not install into $BIN_DIR"

echo "Installed $("$BIN_DIR/hy" --version) to $BIN_DIR/hy"

case ":$PATH:" in
    *":$BIN_DIR:"*) ;;
    *)
        echo
        echo "$BIN_DIR is not on your PATH."
        echo "Debian and Ubuntu add it from ~/.profile, but only when it already exists at"
        echo "login — so log out and back in, or run:"
        echo
        echo "    export PATH=\"$BIN_DIR:\$PATH\""
        ;;
esac
