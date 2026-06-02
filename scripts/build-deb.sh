#!/usr/bin/env bash
set -euo pipefail

if ! command -v cargo-deb &>/dev/null; then
    cargo install cargo-deb
fi

DISTRIB="target/distrib"
TARGETS=(
    "x86_64-unknown-linux-gnu"
    "aarch64-unknown-linux-gnu"
)

for TARGET in "${TARGETS[@]}"; do
    TARBALL=$(ls "$DISTRIB"/ito-*"$TARGET"*.tar.* 2>/dev/null | grep -v '\.sha256$' | head -1)
    if [[ -z "$TARBALL" ]]; then
        echo "No tarball found for $TARGET, skipping"
        continue
    fi

    DEST="target/$TARGET/dist"
    mkdir -p "$DEST"

    tar -xf "$TARBALL" -C "$DEST" --strip-components=1

    # cargo deb expects the binary at target/<target>/release/<bin>
    RELEASE_DIR="target/$TARGET/release"
    mkdir -p "$RELEASE_DIR"
    cp "$DEST/ito" "$RELEASE_DIR/ito"

    cargo deb --no-build --target "$TARGET" -p ito

    # map Rust target triple to Debian arch name for unambiguous copy
    case "$TARGET" in
        x86_64-*)  DEB_ARCH="amd64" ;;
        aarch64-*) DEB_ARCH="arm64" ;;
        *)         DEB_ARCH="$TARGET" ;;
    esac
    cp target/debian/ito_*_"$DEB_ARCH".deb target/distrib/ito-"$TARGET".deb
done
