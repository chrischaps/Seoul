#!/usr/bin/env bash
# Build a universal (Apple Silicon + Intel) Seoul.app for distribution:
#   dist/seoul-<version>-macos-universal.zip
# containing Seoul.app and README.txt. Runs on macOS with the Xcode command
# line tools and both Rust targets:
#   rustup target add aarch64-apple-darwin x86_64-apple-darwin
#   ./packaging/package-macos.sh
set -euo pipefail
cd "$(dirname "$0")/.."

version=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
name="seoul-$version-macos-universal"

for target in aarch64-apple-darwin x86_64-apple-darwin; do
    cargo build --release --target "$target" --target-dir target/package
done

stage="dist/$name"
app="$stage/Seoul.app"
rm -rf "$stage" "dist/$name.zip"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"

lipo -create -output "$app/Contents/MacOS/seoul" \
    target/package/aarch64-apple-darwin/release/seoul \
    target/package/x86_64-apple-darwin/release/seoul
cp -R presets seoul.toml "$app/Contents/Resources/"
sed "s/__VERSION__/$version/g" packaging/Info.plist > "$app/Contents/Info.plist"
cp packaging/README-macos.txt "$stage/README.txt"

# Ad-hoc signature. Gatekeeper still won't trust it (that takes an Apple
# Developer ID and notarization), but Apple Silicon won't run unsigned code
# at all, and a whole-bundle signature keeps macOS from calling it damaged.
codesign --force --deep --sign - "$app"

# ditto keeps the bundle's symlinks and extended attributes intact.
(cd dist && ditto -c -k --keepParent "$name" "$name.zip")
echo "Packaged dist/$name.zip ($(du -h "dist/$name.zip" | cut -f1))"
