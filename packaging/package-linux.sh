#!/usr/bin/env bash
# Build Seoul for Linux and package it:
#   dist/seoul-<version>-linux-<arch>.tar.gz
# containing seoul/{seoul, presets/, seoul.toml, README.txt}. Needs
# pkg-config and the ALSA headers (libasound2-dev). With an argument it
# cross-compiles: `aarch64` builds for 64-bit ARM (the Raspberry Pi 5), as
# set up in packaging/linux.Dockerfile.
#   ./packaging/package-linux.sh            # this machine's architecture
#   ./packaging/package-linux.sh aarch64
set -euo pipefail
cd "$(dirname "$0")/.."

# tr: a Windows checkout mounted into the build box has CRLF line endings.
version=$(tr -d '\r' < Cargo.toml | sed -n 's/^version = "\(.*\)"/\1/p' | head -1)
arch=${1:-$(uname -m)}
if [ "$arch" = "$(uname -m)" ]; then
    cargo build --release --target-dir target/linux
    bin=target/linux/release/seoul
else
    cargo build --release --target "$arch-unknown-linux-gnu" --target-dir target/linux
    bin="target/linux/$arch-unknown-linux-gnu/release/seoul"
fi

name="seoul-$version-linux-$arch"
stage="dist/$name"
rm -rf "$stage" "dist/$name.tar.gz"
mkdir -p "$stage/seoul"
cp "$bin" "$stage/seoul/"
cp -R presets seoul.toml "$stage/seoul/"
cp packaging/README-linux.txt "$stage/seoul/README.txt"
tar -C "$stage" -czf "dist/$name.tar.gz" seoul
echo "Packaged dist/$name.tar.gz ($(du -h "dist/$name.tar.gz" | cut -f1))"
