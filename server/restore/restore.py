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
after a `%m [%p] %d ` prefix and `LOG:`) or a JSON export of the platform's log (a list of objects
with `event_message`, which must begin with `iamb_log `, and `timestamp`, a number or an ISO time).
An entry that carries the marker and is not a function log line is skipped and counted in the
report (`log_lines_skipped`); no caller can make the script stop or accept a line. It needs `psql` and a connection string for the database
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

MARK = "iamb_log "
# A line of Postgres's own log, as `log_line_prefix = '%m [%p] %d '` writes it. Only a `LOG:` line
# whose message begins with the marker is a line of the function log: a `STATEMENT:` or `ERROR:`
# line that merely contains the marker (it can carry text a caller sent) is not.
LINE = re.compile(r"^(\d{4}-\d{2}-\d{2} \d{2}:\d{2}:\d{2}(?:\.\d+)?) (\w+) [^:]*LOG:\s+iamb_log (\{.*\})\s*$")


def stamp_ms(ts):
    """The log system's time of a line in UTC milliseconds, or None: a number (micro- or
    milliseconds) or an ISO 8601 string, as the platform's log export gives it."""
    if isinstance(ts, bool):
        return None
    if isinstance(ts, (int, float)):
        return int(ts // 1000) if ts > 10**14 else int(ts)
    if isinstance(ts, str) and ts.strip():
        text = ts.strip()
        if text.isdigit():
            return stamp_ms(int(text))
        try:
            t = dt.datetime.fromisoformat(text[:-1] + "+00:00" if text.endswith(("Z", "z")) else text)
        except ValueError:
            return None
        if t.tzinfo is None:
            t = t.replace(tzinfo=dt.timezone.utc)
        return int(t.timestamp() * 1000)
    return None


def payload(text):
    """The logged object of a message that begins with the marker, or None."""
    if not isinstance(text, str) or not text.startswith(MARK):
        return None
    try:
        obj = json.loads(text[len(MARK):])
    except ValueError:
        return None
    return obj if isinstance(obj, dict) and isinstance(obj.get("event"), str) else None


def parse_log(path):
    """Answers (lines, skipped): the lines of the function log as dicts, each with `logged_at`
    (milliseconds) from the log's own time where the log gives one, and the number of entries that
    carry the marker and are not such a line. Those are skipped, never an error: any signed-in
    account can make the database log a message that contains the marker (security review 4.3,
    finding 13), and the script must still run."""
    with open(path, encoding="utf-8") as f:
        text = f.read()
    lines = []
    skipped = 0
    stripped = text.lstrip()
    if stripped.startswith("["):
        for e in json.loads(stripped):
            msg = e.get("event_message") if isinstance(e, dict) else None
            if not isinstance(msg, str) or MARK not in msg:
                continue
            obj = payload(msg)
            if obj is None:
                skipped += 1
                continue
            at = stamp_ms(e.get("timestamp"))
            if at is not None:
                obj["logged_at"] = at
            lines.append(obj)
        return lines, skipped
    for raw in text.splitlines():
        if MARK not in raw:
            continue
        m = LINE.match(raw)
        obj = payload(MARK + m.group(3)) if m else None
        if obj is None:
            skipped += 1
            continue
        stamp, zone = m.group(1), m.group(2)
        t = dt.datetime.fromisoformat(stamp)
        if zone.upper() in ("UTC", "GMT", "Z"):
            t = t.replace(tzinfo=dt.timezone.utc)
        obj["logged_at"] = int(t.timestamp() * 1000)
        lines.append(obj)
    return lines, skipped


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
    ap.add_argument("--force", action="store_true",
                    help="accept a bucket listing that lacks more than half of the verified files")
    ap.add_argument("--psql", default=os.environ.get("PSQL", "psql"))
    ap.add_argument("--open", action="store_true", help="after reading the report: switch maintenance off")
    args = ap.parse_args()
    if not args.dsn:
        ap.error("--dsn or IAMB_RESTORE_DSN is required")

    if args.open:
        run_sql(args.psql, args.dsn, "update iamb.server_state set maintenance = false where id;")
        print("maintenance is off; expose the api schema again in the REST settings")
        return
    if args.log is None or args.backup_at is None:
        ap.error("--log and --backup-at are required")

    lines, skipped = parse_log(args.log)
    not_before = args.not_before
    if not_before is None:
        not_before = max((l.get("logged_at") or l.get("at") or 0 for l in lines), default=0)
    report = {"log_lines": len(lines), "log_lines_skipped": skipped, "backup_at": args.backup_at,
              "not_before": not_before}

    names = None
    if args.objects:
        with open(args.objects, encoding="utf-8") as f:
            names = [n.strip() for n in f if n.strip()]
        # A listing that is too short would unback the files it leaves out. Before anything is
        # changed: refuse one that lacks more than half of the verified files the database knows.
        known = [n for n in run_sql(args.psql, args.dsn,
                                    "select profile_id::text || '/' || sha256 from iamb.attachment_blob;").splitlines() if n]
        missing = len(set(known) - set(names))
        if known and missing * 2 > len(known) and not args.force:
            raise SystemExit(f"the bucket listing lacks {missing} of the {len(known)} verified files the database "
                             "knows; nothing was changed. If the listing is right, run again with --force")

    run_sql(args.psql, args.dsn, "update iamb.server_state set maintenance = true where id;")
    report["epoch"] = json.loads(run_sql(args.psql, args.dsn,
                                         f"select iamb_private.begin_epoch({int(not_before)})::text;"))
    report["calls"] = json.loads(run_sql(args.psql, args.dsn,
        f"select iamb_private.restore_replay({literal(json.dumps(lines))}::jsonb, {int(args.backup_at)})::text;"))
    if names is not None:
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
                     f"{to_hash} objects to hash; {report['log_lines_skipped']} log entries with the marker "
                     f"that are not function log lines; maintenance stays on\n")


if __name__ == "__main__":
    main()
