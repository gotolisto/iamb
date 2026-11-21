#!/usr/bin/env bash
# Builds the helper app "Iamb Service.app" that the macOS bundle carries at
# Iamb.app/Contents/Library/LoginItems/ (ADR-0021, docs/release.md), into ../out/.
#
#   build-helper.sh <version>
#
# - iamb-service is built with `-p iamb-service` alone, as it ships (ADR-0008, check 4), for
#   both Apple architectures, and joined into one universal binary.
# - The helper is signed here, before `tauri build` copies it into the main bundle: the bundler
#   signs only the main app's own binaries, and the outer signature needs every nested bundle
#   signed first. With APPLE_SIGNING_IDENTITY set (and the keychain from keychain.sh), it is
#   signed with that identity, the hardened runtime and a secure timestamp; without it, ad hoc,
#   as the main app is then (docs/release.md, "Unsigned builds").
set -euo pipefail

version="${1:?usage: build-helper.sh <version>}"
here="$(cd "$(dirname "$0")" && pwd)"
repo="$(cd "$here/../../../.." && pwd)"
out="$here/../out"
app="$out/Iamb Service.app"

# The same floor as the main app (`minimumSystemVersion` in ../tauri.macos.json): the
# login-item API the main app registers the helper with needs macOS 13.
export MACOSX_DEPLOYMENT_TARGET=13.0

cd "$repo"
for target in aarch64-apple-darwin x86_64-apple-darwin; do
  cargo build -p iamb-service --release --locked --target "$target"
done

rm -rf "$app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
lipo -create -output "$app/Contents/MacOS/iamb-service" \
  target/aarch64-apple-darwin/release/iamb-service \
  target/x86_64-apple-darwin/release/iamb-service
build="${version%%-*}"
sed -e "s/@VERSION@/$version/g" -e "s/@BUILD@/$build/g" \
  "$here/IambService.Info.plist.in" > "$app/Contents/Info.plist"
plutil -lint "$app/Contents/Info.plist"
printf 'APPL????' > "$app/Contents/PkgInfo"
cp "$repo/apps/desktop/src-tauri/icons/icon.icns" "$app/Contents/Resources/icon.icns"

identity="${APPLE_SIGNING_IDENTITY:-}"
if [ -z "$identity" ] || [ "$identity" = "-" ]; then
  echo "Signing the helper ad hoc (no signing identity)."
  codesign --force --sign - --options runtime \
    --entitlements "$here/IambService.entitlements" "$app"
else
  echo "Signing the helper with the Developer ID identity."
  keychain=()
  if [ -n "${IAMB_KEYCHAIN:-}" ]; then keychain=(--keychain "$IAMB_KEYCHAIN"); fi
  codesign --force --sign "$identity" ${keychain[@]+"${keychain[@]}"} --options runtime --timestamp \
    --entitlements "$here/IambService.entitlements" "$app"
fi
codesign --verify --strict --verbose=2 "$app"
lipo -archs "$app/Contents/MacOS/iamb-service"
"$app/Contents/MacOS/iamb-service" --version
