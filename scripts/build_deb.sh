#!/usr/bin/env bash
# build_deb.sh — package tgrep as a .deb (amd64).
#
# Usage:
#   ./scripts/build_deb.sh          # build (uses existing release binary)
#
# Output: dist/tgrep_<version>_<arch>.deb
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
cd "$SCRIPT_DIR"

VERSION="$(grep -m1 '^version' Cargo.toml | cut -d'"' -f2)"
ARCH="$(dpkg --print-architecture)"
OUT_DIR="dist"
DEB="tgrep_${VERSION}_${ARCH}.deb"

# Build if the release binary is missing
if [ ! -f target/release/tgrep ]; then
    echo "[*] Building tgrep (release)..."
    cargo build --release
fi

echo "[*] Staging tgrep ${VERSION} (${ARCH})..."
ROOT="packaging/deb/root"
rm -rf "$ROOT/usr"
mkdir -p "$ROOT/usr/bin" "$ROOT/usr/share/doc/tgrep"

install -m 0755 target/release/tgrep "$ROOT/usr/bin/tgrep"
install -m 0644 README.md "$ROOT/usr/share/doc/tgrep/README.md"
install -m 0644 LICENSE "$ROOT/usr/share/doc/tgrep/LICENSE"
[ -f SECURITY_REVIEW.md ] && install -m 0644 SECURITY_REVIEW.md "$ROOT/usr/share/doc/tgrep/SECURITY_REVIEW.md"
gzip -n -9 -c README.md > "$ROOT/usr/share/doc/tgrep/changelog.gz" 2>/dev/null || rm -f "$ROOT/usr/share/doc/tgrep/changelog.gz"

# Version + arch land in the control file at package time so the tree stays
# in sync with Cargo.toml without manual edits.
sed -i "s/^Version: .*/Version: ${VERSION}/" "$ROOT/DEBIAN/control"

mkdir -p "$OUT_DIR"
echo "[*] Building $OUT_DIR/$DEB..."
dpkg-deb --root-owner-group --build "$ROOT" "$OUT_DIR/$DEB"

echo "[*] Done:"
dpkg-deb -I "$OUT_DIR/$DEB" | head -8
ls -la "$OUT_DIR/$DEB"
