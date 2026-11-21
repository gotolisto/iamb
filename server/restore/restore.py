#!/usr/bin/env python3
"""The operator's restore script (docs/sync-design.md C18; procedure in server/README.md).

Run it on a database that was just restored from a backup, while the REST interface is closed,
and before anything else touches the database. It does, in this order:

  1. switches on `server_state.maintenance`, so that every function answers `maintenance`;
  2. begins the new epoch with `iamb_private.begin_epoch(not_before)`, where `not_before` is the
     time of the last line of the function log from the database that was lost;
  3. makes again, through `iamb_private.restore_replay`, every logged call after the backup's
     moment that took something away (a removal, a withdrawn invitation, a lowered role, a
     withdrawn offer, a deleted group, a revoked device, an ended share, a deleted account);
  4. puts the stored files and their rows in step on the database side
     (`iamb_private.restore_files`), from the names the storage interface lists in the
     `attachments` bucket now (`--objects`, one name per line; the rows of `storage.objects` come
     back with the backup and cannot say which bytes survived), and lists the objects the edge
     function must hash and the upload objects to remove;
  5. prints the report. The server stays in maintenance until the operator, having read the
     report, runs this script again with `--open`.

The function log is read from a file: either Postgres's own log (lines with `iamb_log {...}`
after a `%m` timestamp prefix) or a JSON export of the platform's log explorer (a list of objects
with `event_message` and `timestamp`). It needs `psql` and a connection string for the database
owner; it never prints the connection string.
"""

import argparse
import datetime as dt
import json
import os
import re
import subprocess
import sys
import tempfile

LINE = re.compile(r"^(\d{4}-\d{2}-\d{2} \d{2}:\d{2}:\d{2}(?:\.\d+)?) (\w+).*?iamb_log (\{.*\})\s*$")


def parse_log(path):
    """Answers the logged lines as dicts, each with `logged_at` (milliseconds) from the log's own time."""
    with open(path, encoding="utf-8") as f:
        text = f.read()
    lines = []
    stripped = text.lstrip()
    if stripped.startswith("["):
        for e in json.loads(stripped):
            msg = e.get("event_message") or ""
            i = msg.find("iamb_log ")
            if i < 0:
                continue
            obj = json.loads(msg[i + len("iamb_log "):])
            ts = e.get("timestamp")
            if isinstance(ts, (int, float)):
                # The platform's log explorer stamps lines in microseconds.
                obj["logged_at"] = int(ts // 1000) if ts > 10**14 else int(ts)
            lines.append(obj)
        return lines
    for raw in text.splitlines():
        m = LINE.match(raw)
        if not m:
            continue
        stamp, zone, payload = m.groups()
        obj = json.loads(payload)
        t = dt.datetime.fromisoformat(stamp)
        if zone.upper() in ("UTC", "GMT", "Z"):
            t = t.replace(tzinfo=dt.timezone.utc)
        obj["logged_at"] = int(t.timestamp() * 1000)
        lines.append(obj)
    return lines


def run_sql(psql, dsn, sql):
    with tempfile.NamedTemporaryFile("w", suffix=".sql", delete=False, encoding="utf-8") as f:
        f.write(sql)
        name = f.name
    try:
        out = subprocess.run([psql, "-X", "-q", "-At", "-v", "ON_ERROR_STOP=1", "-d", dsn, "-f", name],
                             check=True, capture_output=True, text=True)
        return out.stdout.strip()
    except subprocess.CalledProcessError as e:
        sys.stderr.write(e.stderr)
        raise SystemExit(1)
    finally:
        os.unlink(name)


def literal(value):
    """A dollar-quoted SQL literal that cannot be closed by its content."""
    tag = "iamb"
    while f"${tag}$" in value:
        tag += "x"
    return f"${tag}${value}${tag}$"


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--dsn", default=os.environ.get("IAMB_RESTORE_DSN"),
                    help="libpq connection string of the restored database (or IAMB_RESTORE_DSN)")
    ap.add_argument("--log", help="the function log: a Postgres log file or a JSON export of the log explorer")
    ap.add_argument("--backup-at", type=int, help="the backup's moment, in UTC milliseconds")
    ap.add_argument("--not-before", type=int,
                    help="floor for the new epoch, in UTC milliseconds; default: the last line of the log")
    ap.add_argument("--objects", help="the object names the storage interface lists in the bucket, one per line")
    ap.add_argument("--psql", default=os.environ.get("PSQL", "psql"))
    ap.add_argument("--open", action="store_true", help="after reading the report: switch maintenance off")
    args = ap.parse_args()
    if not args.dsn:
        ap.error("--dsn or IAMB_RESTORE_DSN is required")

    if args.open:
        run_sql(args.psql, args.dsn, "update iamb.server_state set maintenance = false;")
        print("maintenance is off; expose the api schema again in the REST settings")
        return
    if args.log is None or args.backup_at is None:
        ap.error("--log and --backup-at are required")

    lines = parse_log(args.log)
    not_before = args.not_before
    if not_before is None:
        not_before = max((l.get("logged_at") or l.get("at") or 0 for l in lines), default=0)
    report = {"log_lines": len(lines), "backup_at": args.backup_at, "not_before": not_before}

    run_sql(args.psql, args.dsn, "update iamb.server_state set maintenance = true;")
    report["epoch"] = json.loads(run_sql(args.psql, args.dsn,
                                         f"select iamb_private.begin_epoch({int(not_before)})::text;"))
    report["calls"] = json.loads(run_sql(args.psql, args.dsn,
        f"select iamb_private.restore_replay({literal(json.dumps(lines))}::jsonb, {int(args.backup_at)})::text;"))
    if args.objects:
        with open(args.objects, encoding="utf-8") as f:
            names = [n.strip() for n in f if n.strip()]
        report["files"] = json.loads(run_sql(args.psql, args.dsn,
            "select iamb_private.restore_files(array(select jsonb_array_elements_text("
            f"{literal(json.dumps(names))}::jsonb)))::text;"))
    else:
        report["files"] = {"not_done": "no bucket listing given (--objects); step 3 is still to do"}
    json.dump(report, sys.stdout, indent=2)
    sys.stdout.write("\n")
    made = sum(1 for c in report["calls"] if c["result"] == "made_again")
    skipped = len(report["calls"]) - made
    to_hash = len(report["files"].get("objects_to_hash", []))
    sys.stderr.write(f"epoch {report['epoch']['epoch']}: {made} calls made again, {skipped} skipped; "
                     f"{to_hash} objects to hash; maintenance stays on\n")


if __name__ == "__main__":
    main()
