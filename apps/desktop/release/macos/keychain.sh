#!/usr/bin/env bash
# Imports the Developer ID Application certificate (APPLE_CERTIFICATE: a base64 .p12;
# APPLE_CERTIFICATE_PASSWORD: its password) into a temporary keychain for build-helper.sh, and
# exports its path as IAMB_KEYCHAIN. Prints nothing secret. The bundler imports the same
# certificate into a keychain of its own for the main app.
set -euo pipefail

: "${APPLE_CERTIFICATE:?}" "${APPLE_CERTIFICATE_PASSWORD:?}" "${RUNNER_TEMP:?}"
keychain="$RUNNER_TEMP/iamb-signing.keychain-db"
cert="$RUNNER_TEMP/iamb-signing.p12"
password="$(openssl rand -base64 24)"

printf '%s' "$APPLE_CERTIFICATE" | base64 --decode > "$cert"
security create-keychain -p "$password" "$keychain"
security set-keychain-settings -lut 21600 "$keychain"
security unlock-keychain -p "$password" "$keychain"
security import "$cert" -k "$keychain" -P "$APPLE_CERTIFICATE_PASSWORD" -f pkcs12 \
  -T /usr/bin/codesign > /dev/null
security set-key-partition-list -S apple-tool:,apple:,codesign: -s -k "$password" "$keychain" > /dev/null
# Keep the login keychain in the search list, and add this one.
existing=$(security list-keychains -d user | tr -d '"')
# shellcheck disable=SC2086
security list-keychains -d user -s "$keychain" $existing
rm -f "$cert"

if ! security find-identity -v -p codesigning "$keychain" | grep -q "Developer ID Application"; then
  echo "The certificate in APPLE_CERTIFICATE is not a Developer ID Application identity." >&2
  exit 1
fi
echo "IAMB_KEYCHAIN=$keychain" >> "$GITHUB_ENV"
echo "Imported the signing certificate into a temporary keychain."
