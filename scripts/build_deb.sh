#!/usr/bin/env bash
# Package a current native release build; never install or execute it.
# Set SOURCE_DATE_EPOCH for deterministic package-layer timestamps.
# TGREP_DIST_DIR optionally selects a review output directory.
set -euo pipefail
if ! IFS= read -r -d '' SCRIPT_PATH < <(readlink -fz -- "${BASH_SOURCE[0]}"); then
    echo 'Error: could not resolve packaging script path.' >&2
    exit 2
fi
SCRIPT_DIR="${SCRIPT_PATH%/*}"
SCRIPT_DIR="${SCRIPT_DIR%/*}"
[[ -n "$SCRIPT_DIR" ]] || SCRIPT_DIR=/
cd "$SCRIPT_DIR"
if [[ -n "${CARGO_BUILD_TARGET:-}" ]]; then
    echo 'Error: this script packages native builds; CARGO_BUILD_TARGET is unsupported.' >&2
    exit 2
fi
STAGE_TMP=$(mktemp -d)
trap 'rm -rf -- "$STAGE_TMP"' EXIT
cargo metadata --locked --offline --no-deps --format-version 1 > "$STAGE_TMP/metadata.json"
mapfile -d '' -t INFO < <(python3 - "$STAGE_TMP/metadata.json" <<'PY'
import json, sys
m = json.load(open(sys.argv[1]))
p = next(p for p in m['packages'] if p['name'] == 'tgrep')
sys.stdout.write('\0'.join((p['version'], p.get('license') or '', m['target_directory'])) + '\0')
PY
)
[[ "${#INFO[@]}" -eq 3 && -n "${INFO[0]}" && -n "${INFO[2]}" ]] || exit 2
VERSION="${INFO[0]}"
LICENSE_ID="${INFO[1]}"
BINARY="${INFO[2]}/release/tgrep"
read -r LICENSE_HEADING < LICENSE
case "$LICENSE_ID" in
    AGPL-3.0-only)
        EXPECTED_LICENSE_HEADING='GNU AFFERO GENERAL PUBLIC LICENSE'
        ;;
    LicenseRef-SRSL-1.1)
        EXPECTED_LICENSE_HEADING='SWORD RESTRICTED SOURCE LICENCE (SRSL-1.1)'
        ;;
    *)
        echo "Error: unsupported or missing package license metadata: ${LICENSE_ID:-<empty>}." >&2
        exit 2
        ;;
esac
if [[ "$LICENSE_HEADING" != "$EXPECTED_LICENSE_HEADING" ]]; then
    echo "Error: Cargo license metadata ($LICENSE_ID) disagrees with LICENSE; resolve release licensing first." >&2
    exit 2
fi
cargo build --release --locked --bin tgrep
[[ -f "$BINARY" ]] || { echo "Error: release binary missing: $BINARY" >&2; exit 2; }
ARCH=$(dpkg --print-architecture)
ROOT="$STAGE_TMP/root"
mkdir -p "$ROOT/DEBIAN" "$ROOT/usr/bin" "$ROOT/usr/share/doc/tgrep" "$STAGE_TMP/debian"
install -m 0644 packaging/deb/root/DEBIAN/control "$ROOT/DEBIAN/control"
install -m 0755 packaging/deb/root/DEBIAN/postinst "$ROOT/DEBIAN/postinst"
install -m 0755 "$BINARY" "$ROOT/usr/bin/tgrep"
for doc in README.md LICENSE SECURITY_REVIEW.md; do
    if [[ -f "$doc" ]]; then
        install -m 0644 "$doc" "$ROOT/usr/share/doc/tgrep/$doc"
    fi
done
if [[ -f LICENSES/LicenseRef-SRSL-1.0.txt ]]; then
    mkdir -p "$ROOT/usr/share/doc/tgrep/LICENSES"
    install -m 0644 LICENSES/LicenseRef-SRSL-1.0.txt \
        "$ROOT/usr/share/doc/tgrep/LICENSES/LicenseRef-SRSL-1.0.txt"
fi
if [[ -f CHANGELOG.md ]]; then
    gzip -n -9 -c CHANGELOG.md > "$ROOT/usr/share/doc/tgrep/changelog.gz"
fi
printf 'Source: tgrep\n\nPackage: tgrep\nArchitecture: any\nDescription: tgrep\n' > "$STAGE_TMP/debian/control"
MULTIARCH=$(dpkg-architecture -qDEB_BUILD_MULTIARCH)
SHLIBS=$(cd "$STAGE_TMP" && dpkg-shlibdeps -O -l"/usr/lib/$MULTIARCH" -e "$ROOT/usr/bin/tgrep")
[[ "$SHLIBS" == shlibs:Depends=* && "$SHLIBS" != *$'\n'* ]] || {
    echo 'Error: could not derive runtime dependencies from the staged ELF.' >&2
    exit 2
}
DEPENDS="${SHLIBS#shlibs:Depends=}"
[[ -n "$DEPENDS" ]] || exit 2
python3 - "$ROOT/DEBIAN/control" "$VERSION" "$ARCH" "$DEPENDS" <<'PY'
from pathlib import Path
import sys
p = Path(sys.argv[1])
lines = p.read_text().splitlines()
for field, value in zip(('Version', 'Architecture', 'Depends'), sys.argv[2:]):
    matches = [i for i, line in enumerate(lines) if line.startswith(field + ':')]
    if len(matches) != 1:
        raise SystemExit('Expected exactly one ' + field + ' field')
    lines[matches[0]] = field + ': ' + value
p.write_text('\n'.join(lines) + '\n')
PY
if [[ -n "${SOURCE_DATE_EPOCH:-}" ]]; then
    [[ "$SOURCE_DATE_EPOCH" =~ ^[0-9]+$ ]] || exit 2
    find "$ROOT" -exec touch -h -d "@$SOURCE_DATE_EPOCH" {} +
fi
OUT_DIR="${TGREP_DIST_DIR:-dist}"
mkdir -p "$OUT_DIR"
DEB="$OUT_DIR/tgrep_${VERSION}_${ARCH}.deb"
dpkg-deb --root-owner-group --build "$ROOT" "$STAGE_TMP/tgrep.deb"
install -m 0644 "$STAGE_TMP/tgrep.deb" "$DEB"
echo "Built: $DEB"
