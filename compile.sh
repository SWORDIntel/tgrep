#!/usr/bin/env bash
# tgrep compile.sh — auto-detects CPU capabilities and builds accordingly.
#
# Detects SSE4.2, AVX2, AVX-512 at runtime via /proc/cpuinfo and selects
# the highest available SIMD level for KEYSTONE's C code paths.
#
# Usage:
#   ./compile.sh              # auto-detect + build
#   ./compile.sh --release    # auto-detect + release build
#   ./compile.sh --simd sse4  # force SSE4.2 only
#   ./compile.sh --simd avx2  # force AVX2
#   ./compile.sh --simd avx512 # force AVX-512
#   ./compile.sh --help

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"

# ── Help ─────────────────────────────────────────────────────────────

show_help() {
    cat << 'EOF'
tgrep compile.sh — hardware-aware build script

Usage:
  ./compile.sh [OPTIONS]

Options:
  --release       Build in release mode (optimized, LTO)
  --simd LEVEL    Force SIMD level: sse4, avx2, avx512 (default: auto-detect)
  --jobs N        Number of parallel build jobs (default: nproc)
  --clean         Clean build artifacts before compiling
  --help          Show this help message

The script detects CPU features from /proc/cpuinfo and selects the
highest available SIMD level. KEYSTONE's runtime dispatch will then
use the appropriate code paths at search time.

Environment:
  TGREP_SIMD_FLAGS  Override SIMD flags passed to cc (advanced)

Examples:
  ./compile.sh --release              # optimal for this CPU
  ./compile.sh --release --simd avx2 # force AVX2 even if AVX-512 available
  ./compile.sh --simd sse4            # portable build, SSE4.2 minimum
EOF
}

# ── Parse args ───────────────────────────────────────────────────────

RELEASE=false
SIMD_LEVEL="auto"
JOBS=$(nproc 2>/dev/null || echo 4)
CLEAN=false

while [[ $# -gt 0 ]]; do
    case "$1" in
        --release)  RELEASE=true; shift ;;
        --simd)     SIMD_LEVEL="$2"; shift 2 ;;
        --jobs)     JOBS="$2"; shift 2 ;;
        --clean)    CLEAN=true; shift ;;
        --help|-h)  show_help; exit 0 ;;
        *)          echo "Unknown option: $1" >&2; show_help; exit 1 ;;
    esac
done

# ── CPU feature detection ────────────────────────────────────────────

detect_cpu_features() {
    local cpuinfo
    cpuinfo=$(cat /proc/cpuinfo 2>/dev/null || echo "")
    if [ -z "$cpuinfo" ]; then
        echo "sse4"
        return
    fi

    local flags
    flags=$(echo "$cpuinfo" | grep -m1 '^flags' | cut -d: -f2)

    # Check in order of preference (highest first)
    if echo "$flags" | grep -qw 'avx512f'; then
        echo "avx512"
    elif echo "$flags" | grep -qw 'avx2'; then
        echo "avx2"
    elif echo "$flags" | grep -qw 'sse4_2'; then
        echo "sse4"
    else
        echo "scalar"
    fi
}

# ── Select SIMD flags ────────────────────────────────────────────────

if [ "$SIMD_LEVEL" = "auto" ]; then
    SIMD_LEVEL=$(detect_cpu_features)
fi

case "$SIMD_LEVEL" in
    avx512)
        SIMD_FLAGS="-msse4.2 -mavx2 -mavx512f"
        echo "[compile.sh] SIMD: AVX-512 (8x64-bit parallel search)"
        ;;
    avx2)
        SIMD_FLAGS="-msse4.2 -mavx2"
        echo "[compile.sh] SIMD: AVX2 (4x64-bit parallel search)"
        ;;
    sse4)
        SIMD_FLAGS="-msse4.2"
        echo "[compile.sh] SIMD: SSE4.2 (16-byte shuffle extraction)"
        ;;
    scalar)
        SIMD_FLAGS=""
        echo "[compile.sh] SIMD: scalar (no SIMD available)"
        ;;
    *)
        echo "Error: unknown SIMD level '$SIMD_LEVEL'" >&2
        echo "Use: sse4, avx2, avx512, or auto" >&2
        exit 1
        ;;
esac

# ── Clean ────────────────────────────────────────────────────────────

if $CLEAN; then
    echo "[compile.sh] Cleaning build artifacts..."
    cargo clean
fi

# ── Build ────────────────────────────────────────────────────────────

BUILD_ARGS=""
if $RELEASE; then
    BUILD_ARGS="--release"
    echo "[compile.sh] Building release (LTO, opt-level=3)..."
else
    echo "[compile.sh] Building debug..."
fi

export TGREP_SIMD_FLAGS="$SIMD_FLAGS"

cd "$SCRIPT_DIR"
cargo build $BUILD_ARGS --jobs "$JOBS"

# ── Verify ───────────────────────────────────────────────────────────

BINARY="target/debug/tgrep"
if $RELEASE; then
    BINARY="target/release/tgrep"
fi

if [ -f "$BINARY" ]; then
    echo "[compile.sh] Build complete: $BINARY"
    echo "[compile.sh] SIMD flags: $SIMD_FLAGS"

    # Show detected CPU features via tgrep's own detection
    if $RELEASE; then
        "$BINARY" --explain "test" /dev/null 2>&1 | grep -E "cpu_features|backend" || true
    fi
else
    echo "[compile.sh] Error: binary not found at $BINARY" >&2
    exit 1
fi
