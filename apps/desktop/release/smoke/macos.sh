#!/usr/bin/env bash
# Install smoke test, macOS (docs/release.md, "Install smoke test"): on a clean runner, install
# the disk image the way a user does (attach, copy Iamb.app to /Applications), check what was
# installed, start it, check signatures when the build is signed, uninstall, check it is gone.
#
#   macos.sh <path to .dmg> <expected version> <signed: true|false>
set -euo pipefail

dmg="${1:?usage: macos.sh <dmg> <version> <signed>}"
version="${2:?}"
signed="${3:?}"
app=/Applications/Iamb.app
helper="$app/Contents/Library/LoginItems/Iamb Service.app"
summary="${GITHUB_STEP_SUMMARY:-/dev/null}"
fail() { echo "FAIL: $*" >&2; echo "- FAIL: $*" >> "$summary"; exit 1; }
ok() { echo "ok: $*"; echo "- ok: $*" >> "$summary"; }
plist() { /usr/libexec/PlistBuddy -c "Print :$2" "$1/Contents/Info.plist"; }
# Runs a command with a time limit (macOS has no `timeout`).
limited() {
  local seconds="$1"; shift
  "$@" & local pid=$!
  ( sleep "$seconds"; kill -9 "$pid" 2> /dev/null ) & local watchdog=$!
  local status=0
  wait "$pid" || status=$?
  kill "$watchdog" 2> /dev/null || true
  return "$status"
}

echo "### Install smoke test: macOS" >> "$summary"
[ ! -e "$app" ] || fail "$app exists before the install: not a clean machine"

# Install.
mount="$(mktemp -d)"
hdiutil attach -nobrowse -readonly -mountpoint "$mount" "$dmg" > /dev/null
[ -d "$mount/Iamb.app" ] || fail "the disk image has no Iamb.app"
[ -L "$mount/Applications" ] || fail "the disk image has no Applications link"
cp -R "$mount/Iamb.app" /Applications/
hdiutil detach "$mount" > /dev/null
ok "installed $(basename "$dmg") into /Applications"

# What was installed.
[ -x "$app/Contents/MacOS/iamb-desktop" ] || fail "the UI host is missing"
[ -x "$helper/Contents/MacOS/iamb-service" ] || fail "the service helper app is missing"
[ "$(plist "$app" CFBundleIdentifier)" = org.iamb.Iamb ] || fail "wrong bundle identifier"
[ "$(plist "$helper" CFBundleIdentifier)" = org.iamb.Iamb.Service ] \
  || fail "the helper's bundle identifier is not org.iamb.Iamb.Service (login_item.rs)"
[ "$(plist "$app" CFBundleShortVersionString)" = "$version" ] || fail "app version is not $version"
[ "$(plist "$helper" CFBundleShortVersionString)" = "$version" ] || fail "helper version is not $version"
for bin in "$app/Contents/MacOS/iamb-desktop" "$helper/Contents/MacOS/iamb-service"; do
  archs="$(lipo -archs "$bin")"
  case "$archs" in *x86_64*arm64* | *arm64*x86_64*) ;; *) fail "$(basename "$bin") is not universal: $archs" ;; esac
done
ok "app and helper present, bundle identifiers right, version $version, universal (arm64 + x86_64)"

out="$("$app/Contents/MacOS/iamb-desktop" --version)"
[ "$out" = "iamb-desktop $version" ] || fail "iamb-desktop --version said: $out"
out="$("$helper/Contents/MacOS/iamb-service" --version)"
[ "$out" = "iamb-service $version" ] || fail "iamb-service --version said: $out"
ok "both executables run and report $version"

# Start the installed UI host: one cold open of the library window in a scratch data root
# (the measurement mode of docs/test-strategy.md §8.1; it exits by itself).
data="$(mktemp -d)/data"
limited 120 "$app/Contents/MacOS/iamb-desktop" --data-dir "$data" --measure-open 1 \
  || fail "the installed UI host did not open its window and exit cleanly"
ok "the installed UI host opened its library window (--measure-open 1)"

# Signatures. The bundle is always sealed (ad hoc when unsigned), so the structural check runs
# either way: it fails if the helper was not signed before the main app.
codesign --verify --deep --strict --verbose=2 "$app" || fail "codesign --verify failed"
if [ "$signed" = true ]; then
  spctl -a -vv -t exec "$app" || fail "Gatekeeper rejects the app"
  xcrun stapler validate "$app" || fail "the app has no stapled notarization ticket"
  xcrun stapler validate "$dmg" || fail "the disk image has no stapled notarization ticket"
  spctl -a -vv -t open --context context:primary-signature "$dmg" || fail "Gatekeeper rejects the disk image"
  team_app="$(codesign -dv "$app" 2>&1 | sed -n 's/^TeamIdentifier=//p')"
  team_helper="$(codesign -dv "$helper" 2>&1 | sed -n 's/^TeamIdentifier=//p')"
  [ -n "$team_app" ] && [ "$team_app" = "$team_helper" ] || fail "app and helper are not signed by the same team"
  codesign -dv "$helper" 2>&1 | grep -q 'flags=.*runtime' || fail "the helper lacks the hardened runtime"
  codesign -dv "$app" 2>&1 | grep -q 'flags=.*runtime' || fail "the app lacks the hardened runtime"
  ok "signed: codesign strict, Gatekeeper accepts app and image, notarization stapled to both, same team, hardened runtime"
else
  ok "unsigned build (ad hoc signature only): Gatekeeper and notarization checks skipped"
fi

# Uninstall: a user drags the app to the Bin; nothing else was installed.
rm -rf "$app"
[ ! -e "$app" ] || fail "Iamb.app is still there"
ok "uninstalled: /Applications/Iamb.app removed"
