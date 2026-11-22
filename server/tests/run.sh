#!/usr/bin/env bash
# Runs the Iamb server's policy tests against a throwaway local Postgres cluster
# (server/README.md). Needs the Postgres server binaries (16 or newer); no Docker.
#
#   server/tests/run.sh                 # every test
#   server/tests/run.sh 03_outsider     # the tests whose file name contains the text
#
# PG_BIN may name the directory of `initdb`, `pg_ctl`, `psql` and `pg_dump`.
set -euo pipefail

here=$(cd "$(dirname "$0")" && pwd)
server=$(cd "$here/.." && pwd)
filter=${1:-}

# Postgres refuses to run as root; in a root shell the tests run as an unprivileged user.
if [[ $(id -u) -eq 0 ]]; then
    runas=${IAMB_TEST_USER:-postgres}
    id "$runas" >/dev/null 2>&1 || runas=nobody
    exec runuser -u "$runas" -- env PG_BIN="${PG_BIN:-}" KEEP="${KEEP:-}" SKIP_REAPPLY="${SKIP_REAPPLY:-}" \
        IAMB_TEST_PORT="${IAMB_TEST_PORT:-}" IAMB_SAFEUPDATE="${IAMB_SAFEUPDATE:-}" TMPDIR=/tmp "$0" "$@"
fi

if [[ -z "${PG_BIN:-}" ]]; then
    if command -v pg_config >/dev/null 2>&1 && [[ -x "$(pg_config --bindir)/initdb" ]]; then
        PG_BIN=$(pg_config --bindir)
    else
        PG_BIN=$(ls -d /usr/lib/postgresql/*/bin 2>/dev/null | sort -V | tail -1 || true)
    fi
fi
if [[ -z "${PG_BIN:-}" || ! -x "$PG_BIN/initdb" ]]; then
    echo "error: Postgres server binaries not found; set PG_BIN" >&2
    exit 2
fi

work=$(mktemp -d "${TMPDIR:-/tmp}/iamb-server-tests.XXXXXX")
port=${IAMB_TEST_PORT:-54329}
export PGHOST="$work" PGPORT="$port" PGOPTIONS="-c client_min_messages=warning"
cleanup() {
    "$PG_BIN/pg_ctl" -D "$work/data" -m immediate stop >/dev/null 2>&1 || true
    if [[ "${KEEP:-}" != "1" ]]; then rm -rf "$work"; else echo "kept $work"; fi
}
trap cleanup EXIT

"$PG_BIN/initdb" -D "$work/data" -U supabase_admin --auth=trust -E UTF8 --locale=C >/dev/null
cat >> "$work/data/postgresql.conf" <<EOF
port = $port
listen_addresses = ''
unix_socket_directories = '$work'
log_line_prefix = '%m [%p] %d '
log_timezone = 'UTC'
max_connections = 40
fsync = off
synchronous_commit = off
full_page_writes = off
temp_file_limit = '256MB'
EOF
# `temp_file_limit`: no statement of the suite needs more than a few MB of temporary files. With
# the limit, a function that starts to write them without bound (re-verification N2: one cascade
# list wrote gigabytes) fails its test here instead of filling the disk of whoever runs the suite.
# On the platform the role the REST interface connects with loads `safeupdate`, which refuses an
# UPDATE or DELETE without a WHERE clause, also inside a definer function (security review 4.3,
# finding 14). Where the library is installed (`make install` of pg-safeupdate, or the file
# `safeupdate.so` in the package library directory) every session of the test cluster loads it, so
# a statement without a WHERE fails here as it would there. Without it, the catalog check of
# `21_where_clauses.sql` is what guards against such a statement. IAMB_SAFEUPDATE=0 switches it off.
safeupdate="not installed; only the catalog check of 21_where_clauses guards"
pkglib=$("$PG_BIN/pg_config" --pkglibdir 2>/dev/null || true)
[[ -z "$pkglib" ]] && pkglib=$(cd "$PG_BIN/../lib" 2>/dev/null && pwd || true)
if [[ "${IAMB_SAFEUPDATE:-1}" != "0" && -n "$pkglib" && -f "$pkglib/safeupdate.so" ]]; then
    echo "session_preload_libraries = 'safeupdate'" >> "$work/data/postgresql.conf"
    safeupdate="loaded in every session"
fi
"$PG_BIN/pg_ctl" -D "$work/data" -l "$work/server.log" -w start >/dev/null

psql() { "$PG_BIN/psql" -X -q -v ON_ERROR_STOP=1 "$@"; }

# The template: the stand-in for the platform, the migrations in order (as `postgres`, as on
# the platform), then the test helpers.
psql -U supabase_admin -d postgres -c "create database iamb_template"
psql -U supabase_admin -d iamb_template -f "$here/stub/supabase.sql" >/dev/null
for m in "$server"/supabase/migrations/*.sql; do
    psql -U postgres -d iamb_template -f "$m" >/dev/null
done
# Applying the migrations a second time must work and change nothing that matters.
if [[ "${SKIP_REAPPLY:-}" != "1" ]]; then
    for m in "$server"/supabase/migrations/*.sql; do
        psql -U postgres -d iamb_template -f "$m" >/dev/null
    done
fi
psql -U postgres -d iamb_template -f "$here/lib.sql" >/dev/null

pass=0
fail=0
failed=()
run_one() {
    local name=$1; shift
    local db="t_$(echo "$name" | tr -c 'a-z0-9_' '_')"
    psql -U supabase_admin -d postgres -c "create database $db template iamb_template" >/dev/null
    local out
    if out=$("$@" "$db" 2>&1); then
        pass=$((pass + 1))
        printf 'ok    %s\n' "$name"
    else
        fail=$((fail + 1))
        failed+=("$name")
        printf 'FAIL  %s\n%s\n' "$name" "$(echo "$out" | tail -40 | sed 's/^/      /')"
    fi
    psql -U supabase_admin -d postgres -c "drop database $db with (force)" >/dev/null || true
}
sql_test() {
    local file=$1 db=$2 user=postgres
    # The concurrency tests open a second connection with dblink, which needs a superuser; the
    # API calls in them still run as `authenticated`.
    [[ "$file" == *concurrency* ]] && user=supabase_admin
    "$PG_BIN/psql" -X -q -v ON_ERROR_STOP=1 -v testdb="$db" -U "$user" -d "$db" -f "$file"
}

for t in "$here"/sql/*.sql; do
    name=$(basename "$t" .sql)
    [[ -n "$filter" && "$name" != *"$filter"* ]] && continue
    run_one "$name" sql_test "$t"
done
if [[ -z "$filter" || "restore" == *"$filter"* ]]; then
    restore_test() { PG_BIN="$PG_BIN" SERVER_LOG="$work/server.log" "$here/restore_test.sh" "$1"; }
    run_one "restore" restore_test
fi

# The restore script's reading of the function log (security review 4.3, finding 13).
if [[ -z "$filter" || "restore_parse" == *"$filter"* ]]; then
    parse_test() { python3 -I "$here/restore_parse_test.py"; }
    run_one "restore_parse" parse_test
fi

# §11.6: a push of 200 ops and a cascade of 2,000 rows against push's 20 s time-out.
if [[ -z "$filter" || "perf" == *"$filter"* ]]; then
    perf_test() { psql -U postgres -d "$1" -f "$here/perf.sql" -At | tail -1 > "$work/perf.json"; }
    run_one "perf" perf_test
    [[ -s "$work/perf.json" ]] && echo "      $(cat "$work/perf.json")"
fi

# G20: the function log holds ids, counts and codes, never row content. The tests write bodies,
# labels, names and addresses that would show here if a log line carried them.
if [[ -z "$filter" ]]; then
    if grep -F "iamb_log" "$work/server.log" | grep -E -i "Open (8|9|10)|Thank|Private text|example\.test|Shop replies|Ring twice|seasonal" >/dev/null; then
        fail=$((fail + 1)); failed+=("log_content"); echo "FAIL  log_content (a function log line carries row content)"
    elif ! grep -qF "iamb_log" "$work/server.log"; then
        fail=$((fail + 1)); failed+=("log_content"); echo "FAIL  log_content (no function log line at all)"
    else
        pass=$((pass + 1)); echo "ok    log_content ($(grep -cF "iamb_log" "$work/server.log") function log lines, none with row content)"
    fi
fi

echo
echo "safeupdate: $safeupdate"
echo "server tests: $pass passed, $fail failed"
if (( fail > 0 )); then
    printf '  failed: %s\n' "${failed[@]}"
    exit 1
fi
