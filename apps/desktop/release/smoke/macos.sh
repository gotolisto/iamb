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
# The installed helper serves the account's default data root only. The test starts it there,
# so it must not find a library there, and it removes only what it created (the strict review
# of finding 11, finding 3).
default_root="$HOME/Library/Application Support/Iamb"
[ ! -e "$default_root" ] || fail "$default_root exists before the test: not a clean machine, and the test must not touch a real library"

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

# The installed UI host reaches the installed service (architecture §6.1, the installed-binary
# check; finding 11 and its strict review, finding 2). The helper's supervisor is started as the
# login item starts it, but --supervise --no-ui-host, for the account's default data root; the
# UI host then connects as it does at login, headless (--check-service). A copy of the UI host
# outside Iamb.app connects without checking the service (--skip-server-check), so that the
# refusal can only be the service's, and the worker's log must record it.
scratch="$(mktemp -d)"
worker_log="$default_root/logs/worker.log"
rejected="rejected a client: the peer is not the installed Iamb binary"
show_logs() {
  cat "$scratch/service.err" "$scratch"/*.out 2> /dev/null || true
  tail -n 40 "$default_root"/logs/*.log 2> /dev/null || true
}
start_service() {
  "$helper/Contents/MacOS/iamb-service" --supervise --no-ui-host 2>> "$scratch/service.err" &
  service_pid=$!
}
stop_service() {
  kill "$service_pid" 2> /dev/null || true
  wait "$service_pid" 2> /dev/null || true
}
rejections() {
  local n
  n="$(grep -c "$rejected" "$worker_log" 2> /dev/null || true)"
  echo "${n:-0}"
}
# Waits until the worker's log has recorded more rejections than $1.
await_rejection() {
  local deadline=$((SECONDS + 10))
  while [ "$(rejections)" -le "$1" ]; do
    [ "$SECONDS" -lt "$deadline" ] || return 1
    sleep 0.2
  done
}
start_service
if ! limited 120 "$app/Contents/MacOS/iamb-desktop" --check-service > "$scratch/check.out" 2>&1; then
  show_logs
  fail "the installed service refused the installed UI host, or never answered (--check-service)"
fi
grep -q "^service ok: iamb-service $version" "$scratch/check.out" \
  || { show_logs; fail "--check-service said: $(cat "$scratch/check.out")"; }
[ "$(rejections)" -eq 0 ] || { show_logs; fail "the worker logged a rejection for the installed UI host"; }
ok "the installed UI host connected to the installed service as the UI host ($(head -n 1 "$scratch/check.out"))"
cp "$app/Contents/MacOS/iamb-desktop" "$scratch/iamb-desktop"
before="$(rejections)"
if limited 120 "$scratch/iamb-desktop" --check-service --skip-server-check > "$scratch/copy.out" 2>&1; then
  show_logs
  fail "a copy of the UI host outside Iamb.app was accepted by the installed service"
fi
await_rejection "$before" \
  || { show_logs; fail "the service did not log the copy's rejection (\"$rejected\"): its own check was not what refused it"; }
ok "the installed service refused a copy of the UI host outside Iamb.app, and logged it"
stop_service

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

# A signed service must refuse an unsigned UI host in place of the real one (the strict review
# of finding 11, finding 2): an ad hoc re-signed copy put at the member's own path. Last, since
# it breaks the bundle's seal. Unexercised on unsigned runs, where the path is the check.
if [ "$signed" = true ]; then
  start_service
  cp "$app/Contents/MacOS/iamb-desktop" "$scratch/unsigned"
  codesign --remove-signature "$scratch/unsigned"
  codesign -f -s - "$scratch/unsigned"
  rm -f "$app/Contents/MacOS/iamb-desktop"
  cp "$scratch/unsigned" "$app/Contents/MacOS/iamb-desktop"
  before="$(rejections)"
  if limited 120 "$app/Contents/MacOS/iamb-desktop" --check-service --skip-server-check > "$scratch/unsigned.out" 2>&1; then
    show_logs
    fail "the signed service accepted an unsigned UI host at the member's own path"
  fi
  await_rejection "$before" || { show_logs; fail "the signed service did not log the unsigned UI host's rejection"; }
  ok "signed: the service refused an unsigned UI host in place of the real one, and logged it"
  stop_service
else
  ok "unsigned build: the refusal of an unsigned UI host by a signed service is not exercised"
fi
# Only what the test created: the data root did not exist before it.
rm -rf "$default_root" "$scratch"

# Uninstall: a user drags the app to the Bin; nothing else was installed.
rm -rf "$app"
[ ! -e "$app" ] || fail "Iamb.app is still there"
ok "uninstalled: /Applications/Iamb.app removed"
