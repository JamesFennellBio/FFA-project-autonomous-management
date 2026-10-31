#!/usr/bin/env bash
# Builds the iOS app with xtool (works on Linux) and runs it on a phone on USB.
#
#   ./scripts/ios-xtool.sh          # build, install and launch on the phone
#   ./scripts/ios-xtool.sh build    # just build (app/ios/xtool/FarmLab.app)
#
# One-time setup (app/README.md → Phones): Swift 6, xtool, `xtool setup`
# (an Xcode.xip from developer.apple.com and your Apple ID), Rust target
# aarch64-apple-ios, trunk.
#
# Steps: the UI (trunk) → the Rust app as a static library for iOS → Tauri's
# and the barcode plugin's Swift packages copied to app/ios/.deps → xtool
# links everything into FarmLab.app, signs it and installs it.
set -euo pipefail
cd "$(dirname "$0")/.."
ROOT=$PWD

# The iPhoneOS SDK that `xtool setup` unpacked from Xcode.xip.
SDK=${FARMLAB_IOS_SDK:-$(find ~/.swiftpm/swift-sdks ~/.config/swiftpm/swift-sdks ~/Library/org.swift.swiftpm/swift-sdks \
  -maxdepth 8 -type d -name 'iPhoneOS*.sdk' 2>/dev/null | head -1)}
if [[ -z $SDK || ! -d $SDK ]]; then
  echo "No iPhoneOS SDK found. Run \`xtool setup\` first (or set FARMLAB_IOS_SDK)." >&2
  exit 1
fi
echo "iOS SDK: $SDK"

(cd app/ui && trunk build --release)

# C / Objective-C inside Rust crates (objc2's exception helper) builds with
# clang against the iOS SDK instead of Xcode's xcrun.
export IPHONEOS_DEPLOYMENT_TARGET=15.0
export SDKROOT=$SDK
export CC_aarch64_apple_ios=clang
export CFLAGS_aarch64_apple_ios="--target=arm64-apple-ios$IPHONEOS_DEPLOYMENT_TARGET -isysroot $SDK"
export AR_aarch64_apple_ios=llvm-ar
# staticlib only: the cdylib in Cargo.toml would need Apple's linker.
cargo rustc --release --target aarch64-apple-ios -p farmlab-app --lib --crate-type staticlib

# Tauri's Swift runtime and the barcode-scanner plugin, from the exact crate
# versions in Cargo.lock. The plugin's Package.swift expects Tauri's package at
# ../.tauri/tauri-api, so both go under .deps with that layout.
crate_dir() {
  cargo metadata --format-version 1 --locked 2>/dev/null | python3 -c '
import json, os, sys
m = json.load(sys.stdin)
p = [p for p in m["packages"] if p["name"] == sys.argv[1]]
print(os.path.dirname(p[0]["manifest_path"]))' "$1"
}
DEPS=app/ios/.deps
rm -rf "$DEPS"
mkdir -p "$DEPS/.tauri"
cp -R "$(crate_dir tauri)/mobile/ios-api" "$DEPS/.tauri/tauri-api"
cp -R "$(crate_dir tauri-plugin-barcode-scanner)/ios" "$DEPS/barcode-scanner"
rm -rf "$DEPS"/.tauri/tauri-api/.build "$DEPS"/barcode-scanner/.build

cd "$ROOT/app/ios"
if [[ ${1:-run} == build ]]; then
  xtool dev build
else
  xtool dev run
fi
