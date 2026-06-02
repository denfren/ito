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
    TARBALL=$(ls "$DISTRIB"/ito-*-"$TARGET".tar.gz 2>/dev/null | head -1)
    if [[ -z "$TARBALL" ]]; then
        echo "No tarball found for $TARGET, skipping"
        continue
    fi

    DEST="target/$TARGET/dist"
    mkdir -p "$DEST"

    tar -xzf "$TARBALL" -C "$DEST" --strip-components=1

    # cargo deb expects the binary at target/<target>/release/<bin>
    RELEASE_DIR="target/$TARGET/release"
    mkdir -p "$RELEASE_DIR"
    cp "$DEST/ito" "$RELEASE_DIR/ito"

    cargo deb --no-build --target "$TARGET" -p ito

    # copy to a fixed name so extra-artifacts paths are stable
    cp target/"$TARGET"/debian/ito_*_*.deb target/distrib/ito-"$TARGET".deb
done
