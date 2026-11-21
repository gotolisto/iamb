#!/usr/bin/env bash
# Applies server/supabase/migrations/*.sql, in order, to a hosted Supabase project through the
# Management API (`POST /v1/projects/<ref>/database/migrations`), one named migration per file,
# skipping names the project has already applied. (With the Supabase command-line tool,
# `supabase db push` does the same from this directory.)
#
#   SUPABASE_PROJECT_REF=<ref> server/supabase/apply.sh
#
# The access token is not handled here: set SUPABASE_ACCESS_TOKEN, or run where a proxy adds it.
set -euo pipefail

here=$(cd "$(dirname "$0")" && pwd)
ref=${SUPABASE_PROJECT_REF:?set SUPABASE_PROJECT_REF}
base="https://api.supabase.com/v1/projects/$ref/database/migrations"
auth=()
if [[ -n "${SUPABASE_ACCESS_TOKEN:-}" ]]; then
    auth=(-H @<(printf 'Authorization: Bearer %s\n' "$SUPABASE_ACCESS_TOKEN"))
fi

applied=$(curl -sS --fail "${auth[@]}" "$base" | python3 -I -c 'import json, sys; print("\n".join(m.get("name") or "" for m in json.load(sys.stdin)))')
for f in "$here"/migrations/*.sql; do
    name=$(basename "$f" .sql)
    if grep -qxF "$name" <<<"$applied"; then
        echo "skip  $name (applied)"
        continue
    fi
    body=$(mktemp)
    python3 -I -c 'import json, sys; print(json.dumps({"name": sys.argv[1], "query": open(sys.argv[2]).read()}))' "$name" "$f" > "$body"
    code=$(curl -sS -o "$body.out" -w '%{http_code}' -X POST "$base" "${auth[@]}" -H 'Content-Type: application/json' \
           -H "Idempotency-Key: iamb-$name" --data-binary @"$body")
    if [[ "$code" != 2* ]]; then
        echo "FAIL  $name: HTTP $code $(head -c 1000 "$body.out")"
        rm -f "$body" "$body.out"
        exit 1
    fi
    rm -f "$body" "$body.out"
    echo "ok    $name"
    # The API names each migration's version after the current second; two in one second collide.
    sleep 1.5
done
