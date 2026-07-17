#!/bin/sh
# claude-toolkit installer.
#
# Downloads the latest (or a pinned) release, verifies its SHA-256 checksum,
# and installs ccwatch / ccmap / ccstat into PREFIX, replacing older copies.
#
# Usage:
#   curl -fsSL https://raw.githubusercontent.com/vankichi/claude-toolkit/main/install.sh | sh
#
# Environment:
#   PREFIX   install destination (default: $HOME/.local/bin)
#   VERSION  release tag to install, e.g. v0.1.0 (default: latest)
#
# Supported: macOS (Apple Silicon) and Linux x86_64.
# Windows: download the *-x86_64-pc-windows-msvc.zip asset manually and verify
# it against SHA256SUMS.

set -eu

REPO="vankichi/claude-toolkit"
BINS="ccwatch ccmap ccstat"
PREFIX="${PREFIX:-$HOME/.local/bin}"
VERSION="${VERSION:-latest}"

err() {
	printf 'install.sh: %s\n' "$1" >&2
	exit 1
}

need() {
	command -v "$1" >/dev/null 2>&1 || err "required command not found: $1"
}

need curl
need tar

# --- resolve target triple from OS/arch -------------------------------------
os="$(uname -s)"
arch="$(uname -m)"
case "$os" in
Darwin)
	case "$arch" in
	arm64 | aarch64) target="aarch64-apple-darwin" ;;
	*) err "no prebuilt binary for macOS/$arch (only Apple Silicon is published)" ;;
	esac
	;;
Linux)
	case "$arch" in
	x86_64 | amd64) target="x86_64-unknown-linux-gnu" ;;
	*) err "no prebuilt binary for Linux/$arch (only x86_64 is published)" ;;
	esac
	;;
*)
	err "unsupported OS: $os (on Windows, download the .zip from the releases page)"
	;;
esac

# --- resolve version --------------------------------------------------------
if [ "$VERSION" = "latest" ]; then
	VERSION="$(curl -fsSL "https://api.github.com/repos/$REPO/releases/latest" |
		grep '"tag_name"' | head -n1 |
		sed -E 's/.*"tag_name"[[:space:]]*:[[:space:]]*"([^"]+)".*/\1/')"
	[ -n "$VERSION" ] || err "could not determine the latest release tag"
fi
ver="${VERSION#v}"

bundle="claude-toolkit-${ver}-${target}.tar.gz"
base="https://github.com/$REPO/releases/download/$VERSION"

# --- download bundle + checksums --------------------------------------------
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT INT TERM

printf 'downloading %s (%s)\n' "$bundle" "$VERSION"
curl -fsSL "$base/$bundle" -o "$tmp/$bundle" || err "failed to download $bundle"
curl -fsSL "$base/SHA256SUMS" -o "$tmp/SHA256SUMS" || err "failed to download SHA256SUMS"

# --- verify checksum --------------------------------------------------------
expected="$(awk -v f="$bundle" '$2 == f {print $1}' "$tmp/SHA256SUMS")"
[ -n "$expected" ] || err "no checksum for $bundle in SHA256SUMS"

if command -v sha256sum >/dev/null 2>&1; then
	actual="$(sha256sum "$tmp/$bundle" | awk '{print $1}')"
elif command -v shasum >/dev/null 2>&1; then
	actual="$(shasum -a 256 "$tmp/$bundle" | awk '{print $1}')"
else
	err "need sha256sum or shasum to verify the download"
fi

[ "$expected" = "$actual" ] || err "checksum mismatch for $bundle
  expected: $expected
  actual:   $actual"
printf 'checksum verified\n'

# --- extract & install ------------------------------------------------------
tar -xzf "$tmp/$bundle" -C "$tmp"
extracted="$tmp/claude-toolkit-${ver}-${target}"

mkdir -p "$PREFIX"
for bin in $BINS; do
	src="$extracted/$bin"
	dst="$PREFIX/$bin"
	[ -f "$src" ] || err "expected binary missing from archive: $bin"
	cp "$src" "$dst"
	chmod +x "$dst"
	# macOS 26+ SIGKILLs Mach-O binaries copied without a fresh signature.
	if [ "$os" = "Darwin" ]; then
		codesign --force --sign - "$dst" >/dev/null 2>&1 || true
	fi
	printf 'installed %s -> %s\n' "$bin" "$dst"
done

case ":${PATH}:" in
*":$PREFIX:"*) ;;
*) printf '\nnote: %s is not on your PATH. Add it, e.g.:\n  export PATH="%s:$PATH"\n' "$PREFIX" "$PREFIX" ;;
esac
