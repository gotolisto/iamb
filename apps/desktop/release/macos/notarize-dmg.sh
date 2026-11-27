#!/usr/bin/env bash
# Notarizes and staples the disk image (the bundler has already notarized and stapled the app
# inside it, and signed the image). Apple checks the outermost container a user downloads, so the
# image gets its own ticket too. Reads APPLE_ID, APPLE_PASSWORD (an app-specific password) and
# APPLE_TEAM_ID from the environment; prints nothing secret.
#
#   notarize-dmg.sh <path to .dmg>
set -euo pipefail

dmg="${1:?usage: notarize-dmg.sh <dmg>}"
: "${APPLE_ID:?}" "${APPLE_PASSWORD:?}" "${APPLE_TEAM_ID:?}"

result="$(xcrun notarytool submit "$dmg" --apple-id "$APPLE_ID" --password "$APPLE_PASSWORD" \
  --team-id "$APPLE_TEAM_ID" --wait --timeout 45m --output-format json)"
status="$(printf '%s' "$result" | python3 -c 'import json,sys; print(json.load(sys.stdin).get("status",""))')"
id="$(printf '%s' "$result" | python3 -c 'import json,sys; print(json.load(sys.stdin).get("id",""))')"
echo "Notarization of $(basename "$dmg"): $status (submission $id)"
if [ "$status" != "Accepted" ]; then
  if [ -n "$id" ]; then
    xcrun notarytool log "$id" --apple-id "$APPLE_ID" --password "$APPLE_PASSWORD" \
      --team-id "$APPLE_TEAM_ID" || true
  fi
  exit 1
fi
xcrun stapler staple "$dmg"
xcrun stapler validate "$dmg"
