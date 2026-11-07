#!/bin/bash
# SPDX-License-Identifier: LicenseRef-SRSL-1.1
# Fast external boundaries for private shell fixtures, never a product installation.
set -eu
tool="$1"
invoked="$2"
shift 2

owned() {
    local path="$1"
    case "$path" in /*) ;; *) path="$PWD/$path" ;; esac
    case "$path" in
        "$TGREP_FAKE_ROOT"/*) ;;
        *) printf 'Fixture refused path: %s\n' "$1" >&2; exit 125 ;;
    esac
}

{
    printf '%s\000%s\000%s\000' "$tool" "$PWD" "$invoked"
    printf '%s\000' "$@"
    printf '\000'
} >> "$TGREP_FAKE_QUERY_LOG"

case "$tool" in
    readlink)
        arguments=("$@")
        last=$((${#arguments[@]} - 1))
        case "${arguments[$last]}" in
            /usr/bin/rg|/usr/local/bin/rg|/opt/homebrew/bin/rg|/snap/bin/rg)
                arguments[$last]="$TGREP_FAKE_FALLBACK" ;;
        esac
        owned "${arguments[$last]}"
        exec /usr/bin/readlink "${arguments[@]}"
        ;;
    which) printf '%s\n' "$TGREP_FAKE_CANDIDATES" ;;
    probe-valid) printf 'ripgrep 15.1.0\n' ;;
    probe-failed) printf 'ripgrep 15.1.0\n'; exit 3 ;;
    probe-invalid) printf 'synthetic-other-tool 15.1.0\n' ;;
    dpkg) printf 'amd64\n' ;;
    dpkg-architecture) printf 'x86_64-linux-gnu\n' ;;
    dpkg-shlibdeps) printf 'shlibs:Depends=libc6 (>= 2.39)\n' ;;
    dpkg-deb)
        source="${@: -2:1}"
        destination="${@: -1}"
        owned "$source"
        owned "$destination"
        /bin/cp "$source/DEBIAN/control" "$TGREP_FAKE_PACKAGE_CONTROL"
        printf 'synthetic archive, never installed\n' > "$destination"
        ;;
    grep|rg)
        if [ "$1" = --version ]; then printf 'synthetic version\n'; else printf 'file:1\n'; fi
        ;;
    find) : ;;
    seq)
        if [ "$1" = 0 ]; then printf '0\n'
        elif [ "$2" = 5 ]; then printf '1\n2\n3\n4\n5\n'
        else printf '1\n'; fi
        ;;
    zfs|zpool) exit 1 ;;
    sudo|sync) printf 'Forbidden fixture boundary: %s\n' "$tool" >&2; exit 125 ;;
    compile)
        [ "$TGREP_FAKE_BUILD_FAILURE" != true ] || exit 7
        if [ "$TGREP_FAKE_ARRIVE" = true ]; then
            /bin/mkdir -p "$TGREP_FAKE_BIN"
            printf 'external file appeared during build\n' > "$TGREP_FAKE_BIN/rg"
        fi
        /bin/mkdir -p "$TGREP_FAKE_REPO/target/release"
        /bin/cp "$TGREP_FAKE_BACKEND" "$TGREP_FAKE_REPO/target/release/tgrep"
        /bin/chmod 755 "$TGREP_FAKE_REPO/target/release/tgrep"
        ;;
    wrong-compile)
        /bin/rm -f -- "$TGREP_FAKE_SIBLING_SENTINEL"
        exit 9
        ;;
    mkdir)
        path="${!#}"
        owned "$path"
        if [ "$path" = "$TGREP_FAKE_FAIL_MKDIR" ]; then
            [ "$path" != "$TGREP_FAKE_CONFIG_DIR" ] || exit 44
            exit 40
        fi
        exec /bin/mkdir "$@"
        ;;
    install)
        counter="$TGREP_FAKE_ROOT/install-count"
        number=0
        if [ -f "$counter" ]; then read -r number < "$counter"; fi
        number=$((number + 1))
        printf '%s\n' "$number" > "$counter"
        [ "$number" != "$TGREP_FAKE_FAIL_INSTALL" ] || exit "$((40 + number))"
        source="${@: -2:1}"
        destination="${@: -1}"
        owned "$source"
        owned "$destination"
        /bin/rm -f -- "$destination"
        /bin/cp -- "$source" "$destination"
        /bin/chmod "$2" "$destination"
        ;;
    rm)
        status=0
        for path in "$@"; do
            case "$path" in --|-*) continue ;; esac
            owned "$path"
            if [ "${path##*/}" = "$TGREP_FAKE_FAIL_REMOVE" ]; then
                status=1
            else
                /bin/rm -rf -- "$path" || status=$?
            fi
        done
        exit "$status"
        ;;
    touch)
        for path in "$@"; do owned "$path"; /usr/bin/touch -- "$path"; done
        ;;
    *) printf 'Unknown fake tool: %s\n' "$tool" >&2; exit 125 ;;
esac
