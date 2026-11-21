#!/usr/bin/env bash
# Runs the policy tests against a hosted Supabase project through the Management API
# (server/README.md, "Against the hosted project"). Each test file is sent together with the test
# helpers inside `begin; ... rollback;`: the throwaway accounts (rows of auth.users and
# auth.sessions), and everything the test writes, are gone when it ends. What a rollback does not
# undo: values drawn from `iamb.rev_seq` (so the tests skip `begin_epoch`, which would move it to
# the clock times 1,024), and the function log lines (`iamb_log`) the calls wrote to the project's
# Postgres log.
#
#   SUPABASE_PROJECT_REF=<ref> server/tests/hosted.sh [filter]
#
# The access token is not handled here: set SUPABASE_ACCESS_TOKEN, or run where a proxy adds it.
# Tests that need a superuser connection (20_concurrency) or a dump and restore (restore) run only
# locally.
set -euo pipefail

here=$(cd "$(dirname "$0")" && pwd)
ref=${SUPABASE_PROJECT_REF:?set SUPABASE_PROJECT_REF}
filter=${1:-}
api="https://api.supabase.com/v1/projects/$ref/database/query"
auth=()
if [[ -n "${SUPABASE_ACCESS_TOKEN:-}" ]]; then
    auth=(-H @<(printf 'Authorization: Bearer %s\n' "$SUPABASE_ACCESS_TOKEN"))
fi

pass=0; fail=0; failed=()
for t in "$here"/sql/*.sql; do
    name=$(basename "$t" .sql)
    case "$name" in 20_concurrency) continue ;; esac
    [[ -n "$filter" && "$name" != *"$filter"* ]] && continue
    body=$(mktemp)
    { printf 'begin;\nset local client_min_messages = warning;\nset local iamb_test.hosted = on;\n'; cat "$here/lib.sql"; printf '\n'; cat "$t";
      printf '\nselect %s as passed;\nrollback;\n' "'$name'"; } |
        python3 -I -c 'import json, sys; print(json.dumps({"query": sys.stdin.read()}))' > "$body"
    out=$(curl -sS -X POST "$api" "${auth[@]}" -H 'Content-Type: application/json' --data-binary @"$body" -w '\n%{http_code}')
    rm -f "$body"
    code=${out##*$'\n'}
    if [[ "$code" == 2* ]]; then
        pass=$((pass + 1)); printf 'ok    %s (hosted)\n' "$name"
    else
        fail=$((fail + 1)); failed+=("$name")
        printf 'FAIL  %s (hosted)\n      %s\n' "$name" "$(echo "${out%$'\n'*}" | head -c 1500)"
    fi
done
echo
echo "hosted server tests: $pass passed, $fail failed"
(( fail == 0 ))
