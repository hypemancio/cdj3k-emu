#!/usr/bin/env bash
# SPDX-License-Identifier: MIT OR Apache-2.0
# [intel-port] install-guest-artifacts.sh
#
# Install a "guest-artifacts" bundle produced by the guest-artifacts GitHub
# Actions workflow (see .github/workflows/guest-artifacts.yml) into the
# places build.sh would have put them, so bundle.sh can run without a local
# Docker guest build:
#
#   build/Image            - aarch64 Linux 6.6 kernel
#   build/docker-out/      - modules/*.ko, dummy_drv.so, guest tools
#   guest/out/             - cfgd_aarch64, subucom_*, stemd_client, ep122_shim.so
#
# Usage: ./scripts/install-guest-artifacts.sh <artifact.zip | extracted-dir>

set -euo pipefail
REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"

SRC="${1:?Usage: $0 <guest-artifacts.zip | extracted-dir>}"

CLEANUP_DIR=""
if [[ -f "$SRC" ]]; then
    # A downloaded artifact zip: extract to a temp dir first.
    CLEANUP_DIR="$(mktemp -d)"
    trap 'rm -rf "$CLEANUP_DIR"' EXIT
    unzip -q "$SRC" -d "$CLEANUP_DIR"
    SRC="$CLEANUP_DIR"
fi

if [[ ! -d "$SRC" ]]; then
    echo "ERROR: not a file or directory: $SRC" >&2
    exit 1
fi

# The workflow uploads build/Image, build/docker-out/, guest/out/ with paths
# relative to the repo root; tolerate a zip that was extracted with or
# without that top-level layout.
if [[ -f "$SRC/build/Image" ]]; then
    ART_BUILD="$SRC/build"
    ART_GUEST_OUT="$SRC/guest/out"
elif [[ -f "$SRC/Image" ]]; then
    ART_BUILD="$SRC"
    ART_GUEST_OUT="$SRC/out"
else
    echo "ERROR: no Image found under $SRC (expected build/Image or Image)" >&2
    exit 1
fi

install_tree() {
    local from="$1" to="$2"
    mkdir -p "$to"
    cp -R "$from/." "$to/"
}

echo "==> Installing guest artifacts from: $SRC"

mkdir -p "$REPO_ROOT/build"
cp "$ART_BUILD/Image" "$REPO_ROOT/build/Image"
echo "  ✓  build/Image"

if [[ -d "$ART_BUILD/docker-out" ]]; then
    install_tree "$ART_BUILD/docker-out" "$REPO_ROOT/build/docker-out"
    echo "  ✓  build/docker-out/"
else
    echo "ERROR: docker-out/ missing from artifact" >&2
    exit 1
fi

if [[ -d "$ART_GUEST_OUT" ]]; then
    install_tree "$ART_GUEST_OUT" "$REPO_ROOT/guest/out"
    echo "  ✓  guest/out/"
else
    echo "ERROR: guest/out/ missing from artifact" >&2
    exit 1
fi

# The two files bundle.sh refuses to bundle without.
for required in "$REPO_ROOT/build/Image" "$REPO_ROOT/guest/out/cfgd_aarch64"; do
    if [[ ! -f "$required" ]]; then
        echo "ERROR: required file missing after install: $required" >&2
        exit 1
    fi
done

echo "==> Done. bundle.sh can now run without a local guest build."
