#!/usr/bin/env bash
# tgrep installer — installs tgrep and creates an rg wrapper.
#
# Usage:
#   ./install.sh [--prefix DIR] [--uninstall] [--index] [--no-index]
#                [--zfs] [--no-zfs]
#
# Default prefix: ~/.local
#
# What it does:
#   1. Checks destination ownership and locates genuine ripgrep
#   2. Builds tgrep in release mode using compile.sh
#   3. Installs tgrep to $PREFIX/bin/tgrep
#   4. Creates an rg wrapper at $PREFIX/bin/rg that routes to tgrep
#   5. Stores the real rg path for delegation
#   6. Offers a compressed ZFS dataset for the index (when ZFS is present)
#   7. Offers to build a systemwide index
#
# After installation, `rg` commands are transparently routed:
#   - Indexed searches → tgrep (faster)
#   - Unsupported flags / no index → real ripgrep (identical behavior)
#
# Uninstall:
#   ./install.sh --uninstall
#   This removes the wrapper and restores direct rg access.

set -euo pipefail

PREFIX="$HOME/.local"
UNINSTALL=false
WANT_INDEX=""    # "" = ask (TTY only), "yes", "no"
WANT_ZFS=""      # "" = ask (TTY only), "yes", "no"

while [[ $# -gt 0 ]]; do
    case "$1" in
        --prefix)
            if [[ "$#" -lt 2 || -z "$2" ]]; then
                echo 'Error: --prefix requires a nonempty directory.' >&2
                exit 2
            fi
            PREFIX="$2"
            shift 2
            ;;
        --uninstall)
            UNINSTALL=true
            shift
            ;;
        --index)
            WANT_INDEX="yes"
            shift
            ;;
        --no-index)
            WANT_INDEX="no"
            shift
            ;;
        --zfs)
            WANT_ZFS="yes"
            shift
            ;;
        --no-zfs)
            WANT_ZFS="no"
            shift
            ;;
        --help|-h)
            echo "Usage: $0 [--prefix DIR] [--uninstall] [--index|--no-index] [--zfs|--no-zfs]"
            echo ""
            echo "  --prefix DIR    Install prefix (default: ~/.local)"
            echo "  --index         Build a systemwide index without asking"
            echo "  --no-index      Skip the index-build offer"
            echo "  --zfs           Place the index on a ZFS dataset without asking"
            echo "  --no-zfs        Skip the ZFS offer"
            echo "  --uninstall     Remove tgrep and restore direct rg access"
            exit 0
            ;;
        *)
            echo "Unknown option: $1" >&2
            exit 1
            ;;
    esac
done

if ! IFS= read -r -d '' PREFIX < <(readlink -mz -- "$PREFIX"); then
    echo 'Error: could not resolve install prefix; files preserved.' >&2
    exit 2
fi
BIN_DIR="$PREFIX/bin"
CONFIG_DIR="$HOME/.config/tgrep"
RECEIPT="$BIN_DIR/.tgrep-install.sha256"
FILES=("$BIN_DIR/tgrep" "$BIN_DIR/rg")

file_hash() {
    local result
    result=$(sha256sum < "$1") || return 1
    printf '%s\n' "${result%% *}"
}

verify_installation() {
    local i actual_hash
    local -a installed_hashes
    if [[ ! -f "$RECEIPT" || -L "$RECEIPT" ]]; then
        echo "Error: no verified installation receipt at $RECEIPT; files preserved." >&2
        return 1
    fi
    mapfile -t installed_hashes < "$RECEIPT"
    if [[ "${#installed_hashes[@]}" -ne 2 ]]; then
        echo 'Error: malformed installation receipt; files preserved.' >&2
        return 1
    fi
    for i in 0 1; do
        if [[ ! "${installed_hashes[$i]}" =~ ^[a-f0-9]{64}$ || ! -f "${FILES[$i]}" || -L "${FILES[$i]}" ]]; then
            echo "Error: unverified installed file ${FILES[$i]}; files preserved." >&2
            return 1
        fi
        if ! actual_hash=$(file_hash "${FILES[$i]}"); then
            echo "Error: could not verify ${FILES[$i]}; files preserved." >&2
            return 1
        fi
        if [[ "$actual_hash" != "${installed_hashes[$i]}" ]]; then
            echo "Error: installed file changed: ${FILES[$i]}; files preserved." >&2
            return 1
        fi
    done
}

has_destination_files() {
    [[ -e "${FILES[0]}" || -L "${FILES[0]}" || -e "${FILES[1]}" || -L "${FILES[1]}" || -e "$RECEIPT" || -L "$RECEIPT" ]]
}

check_directory_path() {
    local path="$1"
    while [[ ! -e "$path" && ! -L "$path" ]]; do
        path="${path%/*}"
        [[ -n "$path" ]] || path=/
    done
    if [[ ! -d "$path" ]]; then
        echo "Error: directory path is blocked by $path; files preserved." >&2
        return 1
    fi
}

# Legacy installs and changed files require manual review; never infer ownership from names.
if $UNINSTALL; then
    verify_installation || exit 2
    if rm -f -- "${FILES[@]}"; then
        rm -f -- "$RECEIPT"
    else
        status=$?
        printf 'Error: uninstall partially completed; manual review is required. Ownership receipt retained: %s\n' "$RECEIPT" >&2
        exit "$status"
    fi
    echo 'Removed verified tgrep binaries. Configuration and index data are preserved.'
    exit 0
fi

check_directory_path "$BIN_DIR" || exit 2
check_directory_path "$CONFIG_DIR" || exit 2
OWNED_INSTALL=false
if has_destination_files; then
    verify_installation || exit 2
    OWNED_INSTALL=true
fi
if ! IFS= read -r -d '' SCRIPT_PATH < <(readlink -fz -- "${BASH_SOURCE[0]}"); then
    echo 'Error: could not resolve installer script path; files preserved.' >&2
    exit 2
fi
SCRIPT_DIR="${SCRIPT_PATH%/*}"
[[ -n "$SCRIPT_DIR" ]] || SCRIPT_DIR=/
REAL_RG=""
RG_VERSION=""

try_ripgrep() {
    local candidate version_output first_line marker_status=0
    IFS= read -r -d '' candidate < <(readlink -fz -- "$1") || return 1
    [[ -f "$candidate" && -x "$candidate" && -r "$candidate" ]] || return 1
    # Exclude the destination by canonical path and inode, including hard-link aliases.
    if [[ "$candidate" == "${FILES[1]}" || "$candidate" -ef "${FILES[1]}" ||
          "$candidate" == "${FILES[0]}" || "$candidate" -ef "${FILES[0]}" ||
          "$candidate" -ef "$SCRIPT_DIR/target/release/tgrep" ]]; then
        return 1
    fi
    grep -aqF -- 'TGREP_REAL_RG' "$candidate" || marker_status=$?
    [[ "$marker_status" -eq 1 ]] || return 1
    if ! version_output=$("$candidate" --version 2>/dev/null); then
        return 1
    fi
    first_line="${version_output%%$'\n'*}"
    if [[ ! "$first_line" =~ ^ripgrep[[:space:]]+([0-9]+\.[0-9]+\.[0-9]+)([[:space:]-].*)?$ ]]; then
        return 1
    fi
    RG_VERSION="${BASH_REMATCH[1]}"
    REAL_RG="$candidate"
}

# Check all candidates before building or creating destination/configuration files.
while IFS= read -r candidate; do
    if try_ripgrep "$candidate"; then break; fi
done < <(which -a rg 2>/dev/null || true)
if [[ -z "$REAL_RG" ]]; then
    for candidate in /usr/bin/rg /usr/local/bin/rg /opt/homebrew/bin/rg /snap/bin/rg; do
        if try_ripgrep "$candidate"; then break; fi
    done
fi
if [[ -z "$REAL_RG" ]]; then
    echo 'Error: no independent ripgrep binary passed --version validation; files preserved.' >&2
    exit 2
fi
printf 'Found ripgrep %s: %s\n' "$RG_VERSION" "$REAL_RG"

WRAPPER_TMP=""
RECEIPT_TMP=""
RECEIPT_READY=false
PUBLICATION_PHASE=prepublish
finish_install() {
    local status="$1" keep_receipt=false
    if [[ "$status" -ne 0 ]]; then
        case "$PUBLICATION_PHASE" in
            prepublish)
                printf 'Error: installation failed before binary publication (status %d).\n' "$status" >&2
                ;;
            tgrep)
                printf 'Error: publication failed while installing tgrep; destination may be partially updated (status %d).\n' "$status" >&2
                ;;
            rg)
                printf 'Error: tgrep was installed; rg and ownership receipt may be incomplete (status %d).\n' "$status" >&2
                ;;
            receipt)
                printf 'Error: both binaries were installed; ownership receipt publication failed (status %d).\n' "$status" >&2
                ;;
            config)
                printf 'Error: binaries and ownership receipt are installed; configuration failed (status %d).\n' "$status" >&2
                ;;
            complete)
                printf 'Error: a post-install action failed; binary installation completed (status %d).\n' "$status" >&2
                ;;
        esac
        if $RECEIPT_READY && [[ "$PUBLICATION_PHASE" != complete ]]; then
            printf 'Staged ownership receipt retained: %s\n' "$RECEIPT_TMP" >&2
            keep_receipt=true
        fi
    fi
    if [[ -n "$WRAPPER_TMP" ]] && ! rm -f -- "$WRAPPER_TMP"; then
        printf 'Warning: could not remove installer temporary file: %s\n' "$WRAPPER_TMP" >&2
    fi
    if ! $keep_receipt && [[ -n "$RECEIPT_TMP" ]]; then
        if ! rm -f -- "$RECEIPT_TMP"; then
            printf 'Warning: could not remove installer temporary receipt: %s\n' "$RECEIPT_TMP" >&2
        fi
    fi
    return "$status"
}
trap 'finish_install "$?"' EXIT

cd "$SCRIPT_DIR"
echo 'Building tgrep (release mode, auto-detecting CPU features)...'
./compile.sh --release
if [[ ! -f target/release/tgrep ]]; then
    echo 'Error: tgrep binary not found after build.' >&2
    exit 1
fi

# A build can take time; reject destinations that changed since the initial checks.
if $OWNED_INSTALL; then
    verify_installation || exit 2
elif has_destination_files; then
    echo 'Error: destination files appeared during the build; files preserved.' >&2
    exit 2
fi

shell_quote() {
    printf "'%s'" "${1//\'/\'\\\'\'}"
}
WRAPPER_TMP=$(mktemp)
{
    printf '%s\n' '#!/bin/sh' \
        '# tgrep rg wrapper — auto-generated by tgrep install.sh' \
        '# Routes rg commands to tgrep (indexed) or real ripgrep (fallback).' \
        '# To uninstall: run install.sh --uninstall'
    printf 'export TGREP_REAL_RG=%s\n' "$(shell_quote "$REAL_RG")"
    printf 'exec %s "$@"\n' "$(shell_quote "${FILES[0]}")"
} > "$WRAPPER_TMP"
RECEIPT_TMP=$(mktemp)
file_hash target/release/tgrep > "$RECEIPT_TMP"
file_hash "$WRAPPER_TMP" >> "$RECEIPT_TMP"
RECEIPT_READY=true

mkdir -p "$BIN_DIR"
PUBLICATION_PHASE=tgrep
install -m 0755 target/release/tgrep "${FILES[0]}"
PUBLICATION_PHASE=rg
install -m 0755 "$WRAPPER_TMP" "${FILES[1]}"
PUBLICATION_PHASE=receipt
install -m 0600 "$RECEIPT_TMP" "$RECEIPT"
PUBLICATION_PHASE=config

# Existing configuration can belong to another installation; do not replace it.
if [[ -e "$CONFIG_DIR/rg_path" || -L "$CONFIG_DIR/rg_path" ]]; then
    echo "Preserved existing configuration: $CONFIG_DIR/rg_path"
else
    mkdir -p "$CONFIG_DIR"
    (set -C; printf '%s\n' "$REAL_RG" > "$CONFIG_DIR/rg_path")
fi

if [[ ":$PATH:" != *":$BIN_DIR:"* ]]; then
    printf 'Add %s to your PATH to use the rg wrapper.\n' "$BIN_DIR"
fi
PUBLICATION_PHASE=complete
echo 'Installation complete. Existing configuration and index data are preserved.'

# ── 6. Optional: ZFS-backed index dataset ────────────────────────────

STATE_DIR="$HOME/.local/state/tgrep"

ask() {  # ask "question" -> 0 if yes; only prompts when stdin is a TTY
    local q="$1"
    if [[ -n "$WANT_ZFS" && "$q" == *ZFS* ]]; then
        [[ "$WANT_ZFS" == "yes" ]]
        return
    fi
    if [[ -n "$WANT_INDEX" && "$q" == *index* ]]; then
        [[ "$WANT_INDEX" == "yes" ]]
        return
    fi
    [[ -t 0 ]] || return 1
    local a
    read -r -p "$q [y/N] " a
    [[ "$a" =~ ^[Yy] ]]
}

if command -v zfs >/dev/null 2>&1 && command -v zpool >/dev/null 2>&1; then
    POOL="$(zpool list -H -o name 2>/dev/null | head -1 || true)"
    if [[ -n "$POOL" ]]; then
        echo "ZFS detected (pool: $POOL)."
        echo "  A native ZFS dataset is the best home for the tgrep index:"
        echo "  transparent zstd compression shrinks it ~2-3x, the ARC keeps"
        echo "  hot trigram blocks in RAM, and snapshots give you free rollback."
        if ask "Create ZFS dataset $POOL/tgrep for the index (uses sudo)?"; then
            if ! zfs list "$POOL/tgrep" >/dev/null 2>&1; then
                if sudo zfs create -o compression=zstd -o atime=off \
                    -o mountpoint="$STATE_DIR" "$POOL/tgrep"; then
                    sudo chown "$(id -u):$(id -g)" "$STATE_DIR"
                    echo "      Created: $POOL/tgrep mounted at $STATE_DIR"
                else
                    echo "      zfs create failed — keeping the plain directory." >&2
                fi
            else
                echo "      Dataset $POOL/tgrep already exists — leaving it alone."
                echo "      To use it here: sudo zfs set mountpoint=$STATE_DIR $POOL/tgrep"
            fi
        else
            echo "      Skipping ZFS setup (index will live in a plain directory)."
        fi
        echo ""
    fi
fi

# ── 7. Optional: build a systemwide index ────────────────────────────

echo "The rg wrapper only accelerates searches over indexed paths."
if ask "Build a systemwide index now?"; then
    echo ""
    echo "  What should tgrep index?"
    echo "    [1] Your home directory ($HOME)  — recommended"
    echo "    [2] The entire system (/)        — large; needs sudo for some paths"
    echo "    [3] Custom paths"
    CHOICE=""
    if [[ -n "$WANT_INDEX" && "$WANT_INDEX" == "yes" && ! -t 0 ]]; then
        CHOICE="1"
    else
        read -r -p "  Choice [1]: " CHOICE
    fi
    CHOICE="${CHOICE:-1}"
    case "$CHOICE" in
        1)
            echo "      Indexing $HOME (this can take a few minutes)..."
            "$BIN_DIR/tgrep" index build "$HOME" || true
            ;;
        2)
            echo "      WARNING: indexing / reads every readable file and can"
            echo "      produce a multi-GB index. Ctrl-C to abort."
            sleep 3
            sudo "$BIN_DIR/tgrep" index build / || true
            ;;
        3)
            read -r -p "  Paths to index (space-separated): " INDEX_PATHS
            if [[ -n "${INDEX_PATHS:-}" ]]; then
                # shellcheck disable=SC2086
                "$BIN_DIR/tgrep" index build $INDEX_PATHS || true
            fi
            ;;
        *)
            echo "      Unknown choice — skipping."
            ;;
    esac
else
    echo ""
    echo "Skipped. Build one any time:"
    echo "  tgrep index build /path/to/code"
fi
echo ""

echo "Useful commands:"
echo "  tgrep index status     — indexed paths, segments, cache stats"
echo "  tgrep index build DIR  — (re)index a path"
echo "  tgrep cache clear      — drop cached search results"
echo ""
echo "To uninstall:"
echo "  $0 --uninstall"
