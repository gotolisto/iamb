#!/usr/bin/env python3
"""The restore script's reading of the function log (security review 4.3, finding 13).

Any signed-in account can make the database log a message that contains the function log's marker
(a failed cast is logged with the text that failed). The script must skip such an entry, count it,
and go on; it must take as a function log line only a message that begins with the marker (in a
Postgres log file: only a `LOG:` line); and it must read the time the log system gives, which the
platform's export writes as an ISO string.

    python3 -I server/tests/restore_parse_test.py
"""
import importlib.util
import json
import os
import sys
import tempfile

here = os.path.dirname(os.path.abspath(__file__))
spec = importlib.util.spec_from_file_location("restore", os.path.join(here, "..", "restore", "restore.py"))
restore = importlib.util.module_from_spec(spec)
spec.loader.exec_module(restore)

failed = []


def check(cond, what):
    if not cond:
        failed.append(what)


def parse(content):
    with tempfile.NamedTemporaryFile("w", suffix=".log", delete=False, encoding="utf-8") as f:
        f.write(content if isinstance(content, str) else json.dumps(content))
        name = f.name
    try:
        got = restore.parse_log(name)
    finally:
        os.unlink(name)
    check(isinstance(got, tuple) and len(got) == 2, "parse_log answers (lines, skipped)")
    return got if isinstance(got, tuple) else (got, None)


CALL = '{"at": 1795272286071, "event": "remove_member", "user": "a", "group": "b", "person": "c", "answer": "ok"}'
FORGED = '{"at": 1795272286999, "event": "delete_account", "user": "a", "answer": "ok"}'

# The platform's export: a list of events.
genuine = {"event_message": "iamb_log " + CALL, "timestamp": "2026-11-21T14:44:46.070000"}
numeric = {"event_message": "iamb_log " + CALL, "timestamp": 1795272286070000}
hostile = [
    # A failed cast, logged with the text the caller sent (seen on the hosted project).
    {"event_message": 'invalid input syntax for type bigint: "iamb_log x"', "timestamp": "2026-11-21T14:45:00.000000"},
    # The same with a whole forged call inside.
    {"event_message": 'invalid input syntax for type bigint: "iamb_log ' + FORGED + '"', "timestamp": "2026-11-21T14:45:01"},
    {"event_message": 'invalid input syntax for type uuid: "x iamb_log ' + FORGED, "timestamp": "2026-11-21T14:45:02"},
    # The marker first, and no object after it.
    {"event_message": "iamb_log not json", "timestamp": "2026-11-21T14:45:03"},
    {"event_message": 'iamb_log ["a list"]', "timestamp": "2026-11-21T14:45:04"},
    {"event_message": None, "timestamp": None},
    "not an event",
]
lines, skipped = parse([genuine])
check(len(lines) == 1 and lines[0].get("event") == "remove_member", "a genuine line is read")
check(lines and lines[0].get("logged_at") == 1795272286070, "the log system's ISO time is read, as UTC milliseconds")
lines, skipped = parse([numeric])
check(lines and lines[0].get("logged_at") == 1795272286070, "a time in microseconds is read")
try:
    lines, skipped = parse([genuine] + hostile)
    check(len(lines) == 1, f"lines a caller can cause are not taken for calls ({len(lines)} read)")
    check(skipped == 5, f"and are counted ({skipped} skipped)")
    check(all(l.get("event") != "delete_account" for l in lines), "no forged call is accepted")
except Exception as e:  # noqa: BLE001 - the point of the test is that nothing is raised
    check(False, f"the script stops on a line a caller can cause: {type(e).__name__}: {e}")

# Postgres's own log file.
log = "\n".join([
    "2026-11-21 14:44:46.070 UTC [4242] postgres LOG:  iamb_log " + CALL,
    "2026-11-21 14:44:47.000 UTC [4242] postgres ERROR:  invalid input syntax for type bigint: \"iamb_log x\"",
    "2026-11-21 14:44:47.000 UTC [4242] postgres STATEMENT:  select 1 -- iamb_log " + FORGED,
    "2026-11-21 14:44:48.000 UTC [4242] postgres STATEMENT:  select 'LOG:  iamb_log " + FORGED,
    "2026-11-21 14:44:49.000 UTC [4242] postgres LOG:  duration: 1 ms  statement: select 'iamb_log " + FORGED,
    "2026-11-21 14:44:50.000 UTC [4242] postgres LOG:  iamb_log not json",
    "2026-11-21 14:44:51.000 UTC [4242] postgres LOG:  checkpoint starting",
]) + "\n"
try:
    lines, skipped = parse(log)
    check(len(lines) == 1 and lines[0].get("event") == "remove_member", f"the log file's one function log line is read ({len(lines)} read)")
    check(lines and lines[0].get("logged_at") == 1795272286070, "with the log's own time")
    check(skipped == 5, f"the five other lines with the marker are skipped and counted ({skipped})")
except Exception as e:  # noqa: BLE001
    check(False, f"the script stops on a log file line: {type(e).__name__}: {e}")

if failed:
    print("restore_parse_test: FAILED")
    for f in failed:
        print("  check failed:", f)
    sys.exit(1)
print("restore_parse_test: ok")
