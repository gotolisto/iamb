# Iamb sync server

Task 4.2: the Postgres schema, row-level security, database functions and migrations of the sync
server, on Supabase. The design is [`docs/sync-design.md`](../docs/sync-design.md) (§4, §5, §6.1 to
§6.5, §6.11, §8, §11.5, §11.6) with ADR-0027, ADR-0028 and ADR-0029; this directory implements it
and tests it. Where the implementation had to differ, the difference is a proposal in
[Design corrections](#design-corrections), not a silent change.

The security review of task 4.3 ([`docs/reviews/4.3-security-review.md`](../docs/reviews/4.3-security-review.md))
found 20 points; its follow-up changed the design where the design itself was the cause (C3, C20,
C24, §5.4, §5.6; ADR-0031) and the functions in migrations 11 to 18. The review's last section,
"Resolution", says for each finding what was done, which test holds it and what is left.

## Layout

| Path | What |
| --- | --- |
| `supabase/migrations/` | Eighteen timestamped migrations, applied in order, each safe to apply again |
| `supabase/apply.sh` | Applies the migrations to a hosted project through the Management API |
| `restore/restore.py` | The operator's restore script (C18) |
| `tests/run.sh` | The local test runner: a throwaway Postgres cluster, the stand-in, the migrations (applied twice), every test |
| `tests/stub/supabase.sql` | The stand-in for what the platform provides around Iamb's schemas |
| `tests/lib.sql` | Test helpers: accounts, acting as one, ops in the wire form, the invariant checks |
| `tests/sql/*.sql` | The policy and function tests, one file per area |
| `tests/restore_test.sh` | The restore test: a real dump in the middle of activity, then the script |
| `tests/restore_parse_test.py` | The restore script's reading of the function log, with the lines a caller can cause |
| `tests/perf.sql` | A push of 200 ops and a cascade of 2,000 rows against push's 20 s |
| `tests/hosted.sh` | The policy tests against a hosted project, each in a transaction that is rolled back |
| `tests/hosted_auth_probe.py` | The authentication checks of §11.6 on a hosted project, with throwaway accounts |
| `tests/hosted_rest_writes.py` | Real writing calls through the REST interface of a hosted project, with throwaway accounts and real sessions |

The migrations:

| File | Holds |
| --- | --- |
| `…100000_schemas_and_tables` | Schemas `iamb`, `api`, `api_service`, `iamb_private`; 29 tables with constraints and indexes; `rev_seq` without a cache; row-level security on every table; read grants |
| `…100100_seed` | The `server_state` row, the first epoch, the default plans (open question 4), the private `attachments` bucket |
| `…100200_private_helpers` | Gate helpers (`session_ok`, `member_of`, `role_in`, `shares_group_with`, `own_address`, `enter`, `position`), the C24 locks, revisions and `head_rev`, the feed writer, `compute_access` / `refresh_access` and the slow reference `access_reference`, `readers`, plans and limits |
| `…100300_policies` | The read policies of §5.13, the storage and live-channel policies, the wire-form view |
| `…100400_api_account` | `hello`, `check`, the account calls of §5.14, `members`, `invitations`, `devices`, `users`, `entitlements` |
| `…100500_api_push` | `push` and the rules C1 to C17, C19, C21 |
| `…100600_api_read` | `pull`, `rows`, `fetch_folder`, `snapshot`, `visible_folders`, `access`, conflicts, `attachment_upload_begin`, `api_service.attachment_verified`, `api_service.delete_account_data` |
| `…100700_maintenance` | The retention job (first form), `begin_epoch`, `replay_call`, `restore_replay`, `restore_files`, the token hook, the schedule, the final grants |
| `…100800_daily_job_steps` | The retention job as bounded steps every ten minutes (correction 6) |
| `…100900_push_profile_fix` | A delete of a row the same request creates (found by the tests) |
| `…110000_scratch_tables` | Review 14, 16: no statement without a `WHERE`; what a call collects is in `iamb_private.tx_feed` and `tx_rows`, keyed by the transaction, not in temporary tables; `folder_access.since_rev` |
| `…110100_push` | Review 3, 6, 7, 10, 11: a create inserts or fails, every op's row id is locked, an update names its profile; no lock on a profile the caller has no grant in; ops of the wrong shape refused as data; the cascade list and its answer bounded; answers that do not tell "not yours" from "not there"; a base version only when the writer could read it |
| `…110200_conflicts` | Review 1, 2, 4: what a conflict offers and to whom (`version_readable`, `conflict_history`), taking part only while the row is readable, "Restore this version" through `push_op`; `get_conflict` as an invoker function |
| `…110300_account` | Review 5, 6, 10, 12, 15, 20: an offer ends with its membership; account calls check the membership before they lock; repeated calls give no `mark` to somebody who has left; `revoke_device` and `sign_out_everywhere` in a counter of their own; `accept_ownership` recomputes access; a session past `not_after` is refused; `members` as an invoker function |
| `…110400_uploads` | Review 8: uploads under way count against their uploader; `attachment_upload_cancel`; the profile's allowance checked at the commit for an uploader who does not belong to the profile |
| `…110500_retention` | Review 9: a share of each run per profile, a run goes on where the last stopped (`server_state.job_after`) |
| `…110600_reads_and_grants` | Review 18, 16: a page of `fetch_folder` and `snapshot` ends at about 4 MiB; `search_path = pg_catalog, pg_temp` on every function; the final grants |
| `…110700_steady_plans` | Found while measuring on the hosted project: `feed_flush`, `store_row` and `refresh_access` written so that their cost does not depend on a plan a pooled session made while a table was empty |

## The schema in brief

- **`iamb`** holds the tables and is not exposed through the REST interface: the nine synced tables
  (`profile`, `membership`, `folder`, `folder_share`, `snippet`, `tag`, `snippet_tag`,
  `attachment`, `snippet_attachment`) with the common columns of §5.1 (`rev`, `created_rev`,
  `updated_device`, `updated_op`, `lost_rev`, …); `user_profile`, `device`; `folder_access`;
  `feed`, `feed_head`; `history`, `conflict`, `conflict_version`, `push_receipt`; `invitation`;
  `attachment_blob`, `attachment_upload`; `plan`, `subscription`, `feature_flag`,
  `feature_override`; `server_state`, `epoch`, `epoch_profile`, `rate_bucket`. Times are bigint
  milliseconds, ids uuid, enumerations text with a check.
- **`api`** (exposed; `authenticated` only) holds the 36 functions devices call. Every one calls
  `iamb_private.enter` first and answers one JSON object with `error` and `epoch`. The writing ones
  are `security definer`, owned by `postgres`, with `search_path` pinned to `pg_catalog, pg_temp`
  (every name in them is qualified, and the caller's temporary schema comes last) and
  their own `statement_timeout`; each core is an `iamb_private.acct_*` / `push_*` function that
  takes the caller as an argument, so that the restore script can make a logged call again as its
  caller, while the wrappers take the caller from the token alone. The readers named by §5.1
  (`pull`, `rows`, `fetch_folder`, `snapshot`, `visible_folders`, `access`, `users`, and since the
  review `members`, `conflicts` and `get_conflict`) are
  `security invoker` and return rows through the view `iamb.wire_rows` under the caller's policies.
- **`api_service`** (exposed; `service_role` only, by grants) holds the two functions the edge
  functions call with the service key.
- **`iamb_private`** (not exposed) holds the helpers and two scratch tables (`tx_feed`, `tx_rows`:
  what a call collects, keyed by the transaction, with row-level security on, no policy and no
  grant). `authenticated` may execute exactly the seven
  gate helpers and the two reader helpers (`member_addresses`, `conflict_history`, §5.1); the token
  hook is executable by `supabase_auth_admin` and nobody else; everything
  else by the tables' owner only.
- **Locks (C24).** Every writing function locks, in this order: the device rows (the caller's by
  `enter`; `revoke_device` and `sign_out_everywhere` lock all theirs together by id), the group
  profiles by id, the personal profiles by id (`lock_profiles` sorts groups first), and last the
  feed heads by user id (`feed_flush`). Revisions are drawn under the profile locks and recorded
  as `head_rev`. A `push` locks only the profiles its caller has a grant in, and after them the id
  of every row its ops name (an advisory lock on entity and id), then reads the rows again; an
  account call checks the caller's membership before it locks and again under the lock.
- **Feed.** Entries are collected during the call (in `iamb_private.tx_feed`) and written last by `feed_flush`, which gives
  each user's entries `head + 1, …`, replaces an older `row` entry for the same row, and pokes
  `feed:<user>` with `{epoch, head}` through `realtime.send`.

## How each guarantee is enforced and where it is tested

| | Enforced by | Tested in |
| --- | --- | --- |
| G1 | `refresh_access` recomputes the entries of every profile a change touches, inside the changing transaction under the C24 locks; `access_reference` computes §4.2 independently, folder by folder | `check_access` after every step of every test, including each concurrency case; `06_access_rule`; `23_review_account` (after `accept_ownership`, which the earlier tests ran without a folder in the group) |
| G2 | One read policy per table (§5.13), each requiring `session_ok()`; the wire view masks an unreadable parent | `03_outsider`, `04_viewer`, `05_removed_member`; `check_readers` compares the policies with `iamb_private.readers` for every actor and row after every step |
| G3 | The `tag` and `attachment` policies (readable snippet with a live link, or writer with `edit` now) | `04_viewer`, `05_removed_member`, `12_attachments` |
| G4 | No write privilege anywhere; `anon` has no schema; exactly the gate helpers granted; helpers check the caller's own session and device | `02_grants` (catalog and by trying; a device feeding other devices' ids to the helpers changes nothing) |
| G5 | `history` has no grant and a false policy; `get_conflict` reads it through `conflict_history`, which returns a version only to a party who can read the row now and who wrote the version or could read it while it was the row (`version_readable`: an access entry for the version's folder whose `since_rev` is below the revision that replaced it). `push` finds a base version under the same rule | `05_removed_member`, `15_conflicts`, `22_review_conflicts` |
| G6 | Own-rows policies on `feed`, `feed_head`, `device`, `folder_access`; `conflict` and `conflict_version` by a party who can read the conflict's row; `attachment_upload` by its uploader and by who manages its profile | `03_outsider`, `22_review_conflicts`, `25_review_uploads` |
| G7 | `push_op` checks §4.3 against `folder_access` after the locks; share roots and the roots of shares below need `manage`. `resolve_conflict` makes a restore through `push_op`, so it allows what a push allows | `04_viewer`, `08_folder_delete`, `09_shares`, `22_review_conflicts` |
| G8 | `acct_*` check the role after locking the group; one live owner by a unique index; ownership only by `accept_ownership`, for an offer made to the caller's present membership; `end_membership` withdraws an offer | `10_account_calls`, `23_review_account` |
| G9 | `push` builds the stored row from the op's fields only; C17's four cases | `07_push_rules` |
| G10 | `profile` and `membership` ops answer `rejected: not_pushable` | `07_push_rules` |
| G11 | The `invitation` policy and `own_address()` (confirmed address read from `auth.users`) | `10_account_calls` |
| G12 | A stale share put is `superseded`; a delete always applies | `09_shares`, `05_removed_member` |
| G13 | `end_membership`, `end_share`, `acct_delete_group` and the folder cascade call `refresh_access` in the same transaction; conflicts follow access: the policy on `conflict` and every conflict function require that the caller can read the row now (`can_read`); answers about what the caller cannot see are the answers for what does not exist | `05_removed_member`, `06_access_rule`, `22_review_conflicts`, `23_review_account`, `24_review_push` |
| G14 | `backed` set only by the server, at an attachment row's creation and by the commit, never by an update; storage read policy through `attachment_blob`; upload objects insert-only by their uploader | `12_attachments`, `03_outsider`, `05_removed_member`, `20_concurrency` (f) |
| G15 | `enter` and `session_ok()` require a live `auth.sessions` row that is not past its `not_after`, an accepted `amr` and an unrevoked device row; revoked rows are kept while their sessions exist | `11_sessions`, `16_daily_job`, `23_review_account`; on the hosted project `hosted_auth_probe.py` and `hosted_rest_writes.py` |
| G16 | One select policy on `realtime.messages` for `feed:<own id>`; no insert policy; private channels only. The realtime service applies the policy when a socket joins (review 17) | `03_outsider`, `05_removed_member`, `13_feed` |
| G17 | Definer functions with `search_path` pinned to `pg_catalog, pg_temp`; no dynamic SQL; caller from the token; no temporary table; no `UPDATE` or `DELETE` without a `WHERE` | `02_grants` (catalog), `21_where_clauses` (catalog, and a caller's own temporary table with a trigger) |
| G18 | Sizes in `normalize`, counts in `push_core` and `rows`, rate limits in `enter` and `rate_take`, `cascade_max` in the cascade; `op_shape` before any lock; the cascade list and its answer, and the pages of whole reads, bounded; a counter of their own for the two revoking calls; a share of each retention run per profile | `07_push_rules`, `08_folder_delete`, `14_limits`, `24_review_push`, `23_review_account`, `26_review_retention`, `27_review_reads` |
| G19 | Receipts under the device row's lock | `07_push_rules`, `20_concurrency` (e) |
| G20 | `iamb_private.log` writes ids, counts and codes only | `run.sh` scans every function log line for the bodies, names and addresses the tests wrote |
| G21 | The lock order of C24 | `20_concurrency`: folders moved into each other, a create in a folder being deleted (both orders), a sub-folder created while its share ends, a push into a group's folder while a member is removed; each second request is seen waiting on the first's lock; I7 and G1 after each. Since the review: one id created in two profiles by two requests (an attachment row, a folder), where the second is refused; and an owner's write that is answered while a stranger's request naming her profile is still open |
| G22 | `iamb_private.position` | `11_sessions` |
| G23 | `begin_epoch` (first revision from the operator's floor and above every known revision), `epoch_profile`, the re-push span of C18, `replay_call` and `restore_replay` | `restore_test.sh`, `12_attachments` |

I7 (`check_tree`: no cycle, at most four levels, no live row under a deleted folder) runs after every
step too.

## Running the tests

**Locally**: `server/tests/run.sh` (about a minute). It needs the Postgres server binaries, 16 or
newer (`PG_BIN` may name their directory; otherwise `pg_config` or `/usr/lib/postgresql/*/bin`), and
Python 3. No Docker. In a root shell it runs itself as the `postgres` user, because Postgres refuses
to run as root. `server/tests/run.sh 07` runs the tests whose name contains `07`; `KEEP=1` keeps the
cluster directory. Where the library `safeupdate` is installed for the server (`safeupdate.so` in
its library directory; it is built from the `pg-safeupdate` sources with `make install`), every
session of the test cluster loads it, as the REST interface's role does on the platform, and the
runner's last lines say so; without it the catalog check of `21_where_clauses` is the guard
against an `UPDATE` or `DELETE` without a `WHERE`. The CI job does not install the library today.

**In CI**: the `server` job of `.github/workflows/ci.yml` (Linux) installs Postgres and runs the same
script; `ci-ok` requires it.

**Against the hosted project**: `SUPABASE_PROJECT_REF=<ref> server/tests/hosted.sh` sends each test
file with the helpers through the Management API inside `begin; … rollback;`, so the throwaway
accounts (rows of `auth.users` and `auth.sessions`) and every row the tests write are gone when the
file ends. It needs a Management API token (`SUPABASE_ACCESS_TOKEN`, or a proxy that adds it). Two
things a rollback does not undo: values drawn from `rev_seq` (so the tests skip `begin_epoch` there)
and the `iamb_log` lines the calls wrote to the project's Postgres log, which name throwaway ids; a
restore that read them would skip them as `not_found`. `20_concurrency` and the restore test run
only locally (dblink needs a superuser; a restore needs a dump). The authentication checks:
`SUPABASE_PROJECT_REF=<ref> python3 -I server/tests/hosted_auth_probe.py [--enable-hook]`.

**`hosted.sh` does not go through the REST interface**, and what goes through the Management API
runs as a role that does not load `safeupdate`: all its files passed while no writing call of a
device could succeed on the project (review 14). The test that does go the device's way is
`SUPABASE_PROJECT_REF=<ref> python3 -I server/tests/hosted_rest_writes.py`: it makes two throwaway
accounts by SQL, has the authentication service open real sessions for them with a one-time code
(`amr: otp`, no mailbox and no mail), and calls `hello`, `snapshot`, `push`, `pull`, `fetch_folder`,
the account calls, the conflict calls, the upload calls, `revoke_device` and `sign_out_everywhere`
through `POST /rest/v1/rpc/…` with those tokens. It commits, so it removes every row it made
afterwards, and compares the row counts of every table of `iamb` and `iamb_private`, of the
authentication service's tables and of `storage.objects` with those taken before. It cannot take
back the values drawn from `rev_seq` and its function log lines.

### What the stand-in cannot model

`tests/stub/supabase.sql` gives a plain Postgres the roles (`postgres` as a non-superuser with
BYPASSRLS, `anon`, `authenticated`, `service_role`, `authenticator`, `supabase_auth_admin`),
`auth.users` and `auth.sessions` with the columns Iamb reads, `auth.uid()`, `auth.jwt()` and
`auth.role()` exactly as the platform defines them, `storage.buckets` and `storage.objects` with
row-level security, and `realtime.messages`, `realtime.topic()` and `realtime.send()` as the platform
defines them. It does not model:

- the HTTP services: the REST interface (it applies a function's own `statement_timeout`, which the
  hosted probe checked instead), the authentication service (tokens, refresh, sign-out, the `amr`
  claim, the token hook), the storage service (uploads, upserts, the bytes) and the realtime service
  (joins, delivery, the private-channel switch). The tests set the claims as the REST interface would;
- the platform's ownership of the storage and realtime tables (here `postgres` owns them so that it
  can create policies; on the platform it may create them anyway), its event triggers and default
  privileges;
- the partitioning of `realtime.messages` and the realtime service's daily partitions (see platform
  check 2);
- Supabase Cron (absent here; the tests call the step function directly);
- physical backups and point-in-time recovery: the restore test uses `pg_dump` and `pg_restore`, one
  snapshot as well; nor the authentication service's refresh tokens after a restore;
- the platform's log pipeline: the tests read the cluster's own log file.

## Applying the migrations

`SUPABASE_PROJECT_REF=<ref> server/supabase/apply.sh` applies `supabase/migrations/*.sql` in order
through `POST /v1/projects/<ref>/database/migrations`, one named migration per file, skipping names
already applied, with a pause between files (the API names each version after the current second).
The migrations must run as `postgres` and say so if they do not. With the Supabase command-line tool,
`supabase db push` from `server/` does the same. A new change is a new migration file: the hosted
project already holds these eighteen.

Besides the migrations, a project needs these settings (the hosted project has them; see below): the
REST interface exposes `api,api_service` and nothing else; Realtime allows private channels only; the
custom access token hook is `pg-functions://postgres/iamb_private/token_hook`; and the authentication
settings of §11.3.

## The restore procedure (C18)

For an operator who restores the database from a backup, or finds that it was restored.

1. **Close the REST interface** before the restore: set `server_state.maintenance` in the old database
   if it still runs, and remove `api` and `api_service` from the exposed schemas
   (`PATCH /v1/projects/<ref>/postgrest` with `{"db_schema": "public"}`, or the dashboard's API
   settings; it takes effect within seconds). **`GET` and `PATCH` on that address answer with the
   project's signing secret (`jwt_secret`) in clear, next to `db_schema`: send the answer nowhere,
   do not write it to a file or a terminal that is logged, and keep only the status code.** The
   project's network restrictions cover direct
   database connections only, not the REST interface. Pausing the project also closes it.
2. **Restore** the backup.
3. **Export the function log** from the old database's time: the `iamb_log` lines of the Postgres log
   as JSON, a list of objects with `event_message` and `timestamp`. Through the Management API that
   is `GET /v1/projects/<ref>/analytics/endpoints/logs` with the parameters `sql`
   (`select event_message, timestamp from logs where event_message like 'iamb_log %' order by
   timestamp limit 1000`; the table is `logs`), `iso_timestamp_start` and `iso_timestamp_end` (as
   `2026-11-21T14:00:00.000Z`; at most 24 hours per query, at most 1,000 rows per answer, so page by
   the time window and join the lists; the endpoint answers 429 when asked more than a few times a
   minute). The script takes as a function log line only a message that begins with `iamb_log ` and
   holds a JSON object; anything else that carries the marker (a failed cast is logged with the
   text a caller sent) is skipped and counted in the report as `log_lines_skipped`. And **list the bucket**: the object names in `attachments`
   through the storage interface, one per line. The rows of `storage.objects` come back with the
   backup and cannot say which bytes survived.
4. **Run the script** with the restored database's owner connection, the log, the backup's moment and
   the listing:

       python3 -I server/restore/restore.py --dsn "<connection>" --log log.json \
           --backup-at <utc ms> --objects objects.txt > report.json

   It switches maintenance on, begins the epoch (`begin_epoch`, with the time of the log's last line
   as the floor unless `--not-before` says otherwise), makes again every logged call after the
   backup's moment that took something away (`restore_replay`; skipping one the log shows given back
   afterwards, or one the restored state no longer allows), removes the `attachment_blob` rows whose
   objects are gone and unbacks their attachment rows (`restore_files`), and prints the report:
   what it made again, what it skipped and why, and the objects the edge function must hash.
   Before it changes anything it compares the listing with the verified files the database knows:
   a listing that lacks more than half of them is refused, because every file it leaves out would
   be unbacked; if the listing is right, run again with `--force`.
5. **Read the report**, have the listed objects hashed by `attachment-commit`'s logic (task 4.6), then
   open: `restore.py --dsn … --open`, and expose `api,api_service` again.
6. **Tell the users** that every computer may have to sign in again and that owners should check their
   groups (open question 3).

Never run the script on a database that was not restored: a new epoch costs every device a whole read.
A device that reports more than the server holds is answered `server_behind`, and the line in the log
(`event: server_behind`, with the account and the number it claimed) is the alert. Any account can
write such lines by asking with numbers of its own, a few accounts together too (review 10, 20): the
lines are a reason to look (was the database restored? what does `rev_seq` hold against the highest
`head_rev`?), never by themselves the proof that it was.

## The hosted project

Project `gemyckdldyzcvrvwuivk` (`iamb`, Postgres 17, us-east-1), set up on 2026-11-21 through the
Management API only.

**Migrations applied**: the eighteen files above, in order (`supabase_migrations.schema_migrations`;
the first ten as versions 20261121131746 to 20261121133446, the eight of the review's follow-up on
2026-11-21 and 22). The last one was changed twice while its effect was measured there, and its
statements were run again each time; the project holds the file as it is here.

**Row-level security**: on for 29 of the 29 tables of `iamb` and for the two scratch tables of `iamb_private`; the exposed schemas `api` and `api_service`
hold no table, and `public` holds none of Iamb's:

    select n.nspname, count(*) as tables, count(*) filter (where c.relrowsecurity) as rls_on
    from pg_class c join pg_namespace n on n.oid = c.relnamespace
    where c.relkind in ('r', 'p') and n.nspname in ('iamb', 'iamb_private', 'api', 'api_service')
    group by 1;
    -- iamb | 29 | 29
    -- iamb_private | 2 | 2

**Advisors** (after the last migration):

- Security: 23 warnings, all lint 0029 "Signed-In Users Can Execute SECURITY DEFINER Function", one
  per definer function of `api` (`hello`, `push`, the account calls, `entitlements`,
  `resolve_conflict`, `attachment_upload_begin`, `attachment_upload_cancel`), and 2 at INFO
  ("RLS enabled, no policy") for the two scratch tables of `iamb_private`, which nobody but the
  tables' owner may touch. `members` and `get_conflict` are invoker functions since the review;
  `entitlements` stays a definer function (it reads `plan`, `subscription` and the flag tables for
  the caller alone and takes no argument but the device). The warnings are not noise: these
  functions are the whole write boundary, and the review's findings were in eleven of them. None
  is suppressed. They are the design's own pattern
  (§5.1: every write goes through a definer function that checks §4 itself), and Supabase's page for
  the lint lists this case as intentional: "Option 3: Keep both SECURITY DEFINER and the EXECUTE grant
  — intentional … a 'submit feedback' function that writes to a table they cannot otherwise write to
  … the finding is intentional and can be suppressed for that object", and under False Positives:
  "The function is intentionally a per-user privileged operation … Confirm the function validates input
  and limits what it does, then suppress." Each validates its input, takes the caller from the token
  and checks §4.1 or §4.3 after its locks (tested above). The one other warning the advisor showed
  (lint 0011, the retention job's mutable `search_path`) was fixed by migration 9. No other security
  finding. Correction 10 says how the count itself could be brought to zero; the review advises
  against it (its §6), and it was not done.
- Performance: 3 findings at INFO (two `unused_index`, the project has no traffic yet; one
  `no_primary_key` for the scratch table `tx_feed`). None at WARN.

**Settings changed** (each in the design's scope; nothing else was touched):

| Setting | Before | After | Why |
| --- | --- | --- | --- |
| REST exposed schemas (`db_schema`) | `public,graphql_public` | `api,api_service` | §5.1 |
| Realtime `private_only` | unset | `true` | §11.3 |
| Auth `hook_custom_access_token_enabled`, `_uri` | off | on, `pg-functions://postgres/iamb_private/token_hook` | C25, §11.6 |
| Auth `security_update_password_require_reauthentication` | false | true | §11.3 |
| Auth `security_update_password_require_current_password` | false | true | §11.3 |
| Extension `pg_cron` and the job `iamb-retention` (`*/10 * * * *`) | absent | present | §5.12 |
| Storage bucket `attachments` (private, 25 MiB) | absent | present | §5.9 |

Already as §11.3 requires, left unchanged: anonymous sign-in off, phone off, manual identity linking
off, secure email change on, email confirmation required (`mailer_autoconfirm` off), refresh-token
rotation on with a reuse interval of 10 s, access tokens of 3,600 s, no identity provider enabled.
Not changed: TOTP enrolment, which is on by default (correction 11); the mail sender (open question 8).

**For the owner to switch, in the project's authentication settings** (the review's follow-up changed
no setting): **TOTP enrolment off** (Authentication, Multi-Factor, TOTP). Iamb never asks for a second factor;
an account that enrols and verifies one through the authentication service gets sessions whose `amr`
holds `totp`, which Iamb refuses, until it signs in again without the factor (review 19).

**Tests on the hosted project**: `hosted.sh`, 24 of 24 files passed (all but `20_concurrency`), each in a
rolled-back transaction, through the Management API. Through the REST interface, as a device calls:
`hosted_rest_writes.py`, 52 of 52 checks, after migrations 11 to 18; before them the same script
failed in its first call (`hello`: HTTP 400, "DELETE requires a WHERE clause") and in all that
depend on it (2 of 49 passed). `hosted_auth_probe.py --enable-hook` as below; `perf.sql` rolled
back: a push of 200 ops 0.42 to 0.55 s, a cascade of 2,000 rows 3.4 to 6.5 s over sixteen runs
(before migration 18 the same delete took between 3 and 21 s, depending on the plan the pooled
session had kept). Throwaway accounts left afterwards: 0; the row counts of every table equal
those before the runs (2 plans, 1 epoch, 1 `server_state`). The rolled-back
runs advanced `rev_seq` (one early run moved it to the clock times 1,024 through `begin_epoch`; with no
committed revision on the project it was set back to 1, and `begin_epoch` is no longer run there).

### Platform checks (§11.6)

Done on the hosted project:

1. **A policy cannot read its own table; the gate helpers work** — every table has a policy test, and
   all pass on the platform (`hosted.sh`).
2. **The database can send on a private channel** — `realtime.send` from inside the writing
   transaction stores the poke, and clients cannot insert (`03`, `05`, `13` on the platform), but only
   once the realtime service has created the day's partitions of `realtime.messages`: on the new project
   there were none, every poke was dropped with a warning, and one WebSocket connection created them.
   Supabase's broadcast guide says so: "A client connecting over WebSocket creates those daily
   partitions. `realtime.send` does not." Harmless for correctness (nobody is listening on such a day,
   and a device pulls once after it connects, §6.6); correction 9. Delivery to a joined client was not
   checked: a private join needs a session Iamb accepts (below).
3. **A function's own time-out applies through the REST interface** — a probe function with
   `statement_timeout = 15s` sleeping 9.5 s returned after 9.7 s (HTTP 200); the same without the setting
   was stopped at 8.2 s (`57014`). Push of 200 ops 0.44 s, cascade of 2,000 rows 4.6 s, both against 20 s.
4. **`rev_seq` has no cache** — `pg_sequence.seqcache = 1` on the platform (`02_grants`).
5. **Sessions** — a password session's token carries `session_id`, the same after a refresh; signing out
   with scope `local` removed one `auth.sessions` row, `others` all but the caller's, `global` all;
   deleting the user removed its sessions; the access token of an ended session is refused by the
   authentication service's own `/user` endpoint (the REST interface checks only the signature, which is
   why `session_ok()` reads `auth.sessions`). The tables' owner may read `auth.sessions` and
   `auth.users`. A revoked device row is kept while its session exists and keeps refusing it after the
   retention job (`11`, `16` on the platform, with sessions made by SQL).
6. **`amr`** — a password session's `amr` is `[password]`, also after a refresh. Iamb's functions refuse
   it through the REST interface: `hello` and `check` answered `session_refused` (S83 on the platform).
   `api_service` refused to a signed-in user (403), `api` to the public key alone (401). A session
   opened by a one-time code carries `amr` `[otp]`, also after a refresh, passes the hook and is
   accepted (the review, and `hosted_rest_writes.py` at every run).
7. **The token hook** — with the hook on, a password sign-in is refused at the authentication service
   (HTTP 403, "Iamb does not sign in with a password."), and a refresh of an earlier session passes. The
   grants it needs are in migration 8.
8. **Password change** — with the settings above, changing the password without the current one answers
   `current_password_required` **for a session that was opened with a password, which Iamb never
   has**. A session opened by a code set a first password, and changed it, without the current one
   within 24 hours of its sign-in (review 19); the setting does not protect Iamb's kind of session.
   A password still opens nothing: the hook refuses the sign-in.
9. **Supabase Cron** — the plan has it: `pg_cron` installed, the job ran at 13:30 and succeeded.
10. **The function log** — the `iamb_log` lines reach the platform's log stream, stamped
    by the log system; 443 lines were there after the test runs. The export of restore step 3 was
    run as written there and read by the restore script's parser: 315 lines of the last eight
    hours, each with the log system's time, none skipped.
11. **Writing calls through the REST interface** — the role that interface connects with loads
    `safeupdate`, which refuses an `UPDATE` or `DELETE` without a `WHERE`, also inside a definer
    function. Since migration 11 no function holds one; `hosted_rest_writes.py` makes the calls.
12. **The live channel checks the right to receive at the join** — a socket that joined before its
    device was revoked received the next poke; a join after the revocation was refused (review 17;
    §6.6 and G16 now say so).

Left, with what each needs:

- **`amr` of Iamb's own sign-ins by a link and by an identity provider** and that the hook lets them
  through (the code form is checked, above):
  a sign-in to a real mailbox or provider. `sign_in_methods` is set to
  `{otp, magiclink, email/signup, invite, oauth}` from the authentication service's method names
  and must be confirmed then (correction 11).
- ~~Whether requiring the current password keeps an account without a password from setting one~~:
  it does not (check 8).
- **That a request cut off by its time-out is not counted by the rate limiter**: inferred from how
  Postgres undoes a cancelled statement, not run; §6.3 and §6.8 are written on that assumption.
- **A refresh token issued after a backup, after a restore**: a restore of the hosted project, which is
  destructive and needs a plan with backups; the procedure already assumes the worst (every computer
  may have to sign in again).
- **Stored files across a restore**: the storage service and `attachment-commit` (task 4.6). The restore
  test checks the database side with a listing; correction 7.
- **How long the platform keeps the function log**: the project's plan, which the token cannot read
  (`organizations_read` is missing). Supabase's documented retention is one day on the free plan and
  seven days on the first paid one, the same seven days as its daily backups; the log must be kept at
  least as long as the oldest backup the operator would restore from (open question 3).
- **The live channel's limit of connections**: the plan, as above.

## Design corrections

Proposals for `docs/sync-design.md`, as task 4.2 made them; the implementation follows each. The
follow-up to the security review edited the design itself where the review's findings were in it
(ADR-0031), and took up the proposals below as noted with each.

1. **`members`, `entitlements` and `get_conflict` are definer functions.** §5.1 lists `members` among the
   invoker readers, but owners and admins get each member's address, which is in `auth.users` and
   unreadable to an invoker function; `entitlements` reads `plan` and `subscription`, and `get_conflict`
   the `history`, which §5.13 makes unreadable. Proposal: name them with the definer functions in §5.1;
   each checks the caller's role or party itself. *Since the review:* `members` and `get_conflict`
   are invoker functions again, each with one small definer helper for the one thing no policy can
   give (`member_addresses`, `conflict_history`; §5.1 lists them as reader helpers). `entitlements`
   was left a definer function: it has nothing to check but the caller, and a helper would be the
   whole function.
2. **`conflict.parties`.** A policy on `conflict` that looks at `conflict_version`, and one on
   `conflict_version` that looks at `conflict`, recurse. The parties are kept on the conflict row
   (`uuid[]`, maintained by the functions). Proposal: add the column to §5.6. *Since the review:* a
   party stays in the list when their access ends, and takes part only while they can read the row
   (C20, §5.6); the policy and every function check that.
3. **`folder_access.profile_id`**, copied from the folder, so that "edit on a folder of this profile"
   (tags, attachments, profiles) needs no join. Proposal: add it to §5.4.
4. **`push_receipt.entity`, `row_id`, `adjusted`**, so that a replayed op can answer with the row as it is
   now and the fields the server set (C15). Proposal: add them to §5.7.
5. **`position(device, presented, call, new_seq)`** also answers `lost` for a snapshot that presents a
   position of an older epoch: `snapshot` is an invoker function and `epoch_profile` is unreadable.
   Proposal: change the helper's row in §5.1.
6. **The retention job runs as bounded steps every ten minutes.** A procedure that commits between
   batches cannot pin its `search_path` (Postgres forbids transaction control in a procedure with a `SET`
   clause), and the security advisor reports it. `iamb_private.daily_job_step()` does at most
   `cascade_max` rows per run, one transaction per run, with the C24 lock order inside the step.
   Proposal: §5.12 "one scheduled database job a day" becomes "a bounded step every ten minutes".
   *Since the review:* a share of each run per profile, and a run goes on where the last stopped (§5.12).
7. **Restore step 3 needs the storage interface.** On the platform the rows of `storage.objects` are in
   the database and come back with the backup; the bytes do not. The database cannot tell which objects
   survived, so `restore_files` takes the bucket's listing from the storage interface. Proposal: say so
   in C18's step 3 and §11.6. *Since the review:* the script refuses a listing that lacks more than
   half of the verified files, without `--force`.
8. **Removing stored objects.** An object must be removed through the storage interface, not by deleting
   its row. The retention job removes the `attachment_blob` and `attachment_upload` rows and writes
   `blob_removed` and `upload_expired` lines for task 4.6's job, which removes the objects. Proposal: say
   so in §5.12. *The review prefers a table* that the job reads and empties, written in the same
   transaction, to commands that travel through a log the platform keeps for a day on its smallest
   plan. Not done here: the reader of those lines does not exist yet, and the table belongs to the
   task that writes it (4.6).
9. **The live channel on a day no client has connected.** `realtime.send` drops the poke until a client's
   connection has created the day's partition (platform check 2). Proposal: add the caveat to the §11.6
   row on the live channel; nothing else changes, since §6.6 already pulls after a connection.
10. **Lint 0029.** The 24 warnings are intentional (above). If the owner wants the advisor's count itself
    at zero: `api` keeps `security invoker` wrappers and the definer implementations move to a schema
    that is not exposed (granted to `authenticated` for execution, as the gate helpers are). That is
    equally safe and hides nothing from a reviewer, but adds a schema to §5.1. Not done: the review
    advises against it, because the count is the one outside signal that says where the write
    boundary is.
11. **`sign_in_methods` and second factors.** The design names "the link or code sent to the account's
    address, and the identity providers". The authentication service's method names include `otp`,
    `magiclink`, `email/signup`, `invite` and `oauth`; `recovery` (a password reset link) is left out.
    Second factors: TOTP enrolment is on by default, and a session that has verified a second factor
    carries `totp` in `amr`, which `session_ok()` would refuse. Proposal: either add `totp` (a second
    factor never opens a session by itself) or switch TOTP enrolment off in §11.3's settings; and confirm
    the names with a real sign-in (platform checks, left). *The review confirmed both and advises to
    switch enrolment off*; that is the owner's step above. A code from a recovery mail, verified as
    a code, gives `otp` like any other code: it proves the mailbox, which is the design's test.
12. **Logging share creation.** The restore script must skip a share's delete when the log shows the
    share given back, so `push` logs share creations too (`share_created`), not only deletes (§11.1).
13. **`parent_gone` or `forbidden` for a row in a folder that became unreadable.** A removed member's write
    to his own snippet answers `rejected: parent_gone` (C13: the folder it names is unreadable for the
    writer); S30 does not name the reason. Either way the device keeps the change (D5). Proposal: say in
    C12 that C13's reason wins when the folder is unreadable. *Since the review:* C12 and C13 say so,
    and the same holds for the op's own row and for a profile or group the caller has no grant in.
14. **Invitation emails.** `invite` records the invitation and tells the feeds; sending the email needs the
    mail service of open question 8 and is not done by the database.

## Left for later tasks

- The edge functions `attachment-commit` and `account-delete` (tasks 4.6 and 4.5); their database parts
  (`api_service.attachment_verified`, `api_service.delete_account_data`) and `api.check` are here.
- Removing stored objects through the storage interface after the retention job's log lines (4.6).
- The checks of §11.6 listed as left above.
- From the security review, with the reasons in its Resolution: an allowance in bytes per profile
  and a bound in bytes on `rows` and on the rows in a push's results (review 18; both need a
  decision on the plans or a short form in the protocol); a table instead of log lines for the
  objects to remove (correction 8, task 4.6); the size and the bounding of an uploaded object
  (review 8, task 4.6); a limit in front of the database for requests that run into their time-out
  (review 7, the hosting plan); showing the pending invitations when an admin is removed (review 20,
  the interface).
- A protocol version 2: only version 1 exists, so "each version's push writes the columns that version
  knows" has nothing to choose between yet.
