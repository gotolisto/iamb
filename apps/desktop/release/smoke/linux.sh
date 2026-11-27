#!/usr/bin/env bash
# Install smoke test, Linux (docs/release.md, "Install smoke test"): install the .deb on a clean
# runner, check what was installed, start it under a virtual display, remove the package, check
# it is gone. Linux packages are not signed by this pipeline.
#
#   linux.sh <path to .deb> <expected version>
set -euo pipefail

deb="${1:?usage: linux.sh <deb> <version>}"
version="${2:?}"
summary="${GITHUB_STEP_SUMMARY:-/dev/null}"
fail() { echo "FAIL: $*" >&2; echo "- FAIL: $*" >> "$summary"; exit 1; }
ok() { echo "ok: $*"; echo "- ok: $*" >> "$summary"; }

echo "### Install smoke test: Linux" >> "$summary"
! dpkg -s iamb > /dev/null 2>&1 || fail "the package iamb is installed already: not a clean machine"

# Install (apt installs the package's declared dependencies too; dpkg does the install).
sudo apt-get update -qq
sudo apt-get install -y --no-install-recommends "$(realpath "$deb")" xvfb
[ "$(dpkg-query -W -f='${Version}' iamb)" = "$version" ] || fail "package version is not $version"
ok "installed $(basename "$deb") (package iamb $version)"

for bin in /usr/bin/iamb-desktop /usr/bin/iamb-service; do
  [ -x "$bin" ] || fail "$bin is missing"
done
out="$(/usr/bin/iamb-desktop --version)"
[ "$out" = "iamb-desktop $version" ] || fail "iamb-desktop --version said: $out"
out="$(/usr/bin/iamb-service --version)"
[ "$out" = "iamb-service $version" ] || fail "iamb-service --version said: $out"
ok "UI host and service installed in /usr/bin and report $version"

data="$(mktemp -d)/data"
timeout 120 xvfb-run -a /usr/bin/iamb-desktop --data-dir "$data" --measure-open 1 \
  || fail "the installed UI host did not open its window and exit cleanly"
ok "the installed UI host opened its library window (--measure-open 1, Xvfb)"
ok "unsigned: Linux packages are not signed by this pipeline"

sudo apt-get purge -y iamb
! dpkg -s iamb > /dev/null 2>&1 || fail "the package is still installed"
for bin in /usr/bin/iamb-desktop /usr/bin/iamb-service; do
  [ ! -e "$bin" ] || fail "$bin is still there"
done
ok "uninstalled: package purged, binaries gone"
