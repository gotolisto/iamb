#!/usr/bin/env bash
# The restore test (docs/sync-design.md C18, G23, §11.6): a real backup (pg_dump, one snapshot)
# is taken in the middle of activity, writes and calls that take access away follow it, the
# backup is restored into a new database, the operator's script (server/restore/restore.py) runs
# with the function log, and the restored database is checked. Called by run.sh with the name of
# a fresh database; PGHOST, PGPORT, PG_BIN and SERVER_LOG come from run.sh.
set -euo pipefail

db=$1
here=$(cd "$(dirname "$0")" && pwd)
server=$(cd "$here/.." && pwd)
work=$(mktemp -d "${TMPDIR:-/tmp}/iamb-restore.XXXXXX")
trap 'rm -rf "$work"' EXIT
psql() { "$PG_BIN/psql" -X -q -v ON_ERROR_STOP=1 "$@"; }
q() { "$PG_BIN/psql" -X -q -At -v ON_ERROR_STOP=1 -U postgres -d "$1" -c "$2"; }
now_ms() { python3 -c 'import time; print(int(time.time() * 1000))'; }

# ------------------------------------------------------------------------------- before the backup
psql -U postgres -d "$db" <<'SQL' >/dev/null
do $$
declare
    ids jsonb := iamb_test.family('edit');
    h uuid := gen_random_uuid(); inv uuid := gen_random_uuid(); r jsonb; pi uuid;
    sha_kept text := repeat('a1', 32); sha_gone text := repeat('b2', 32);
begin
    pi := (ids ->> 'p_ines')::uuid;
    perform iamb_test.new_user('ana'); perform iamb_test.hello('ana', 'Ana');
    perform iamb_test.new_device('ines_b', 'ines'); perform iamb_test.hello('ines_b');
    perform iamb_test.call('ines', format('api.create_group(iamb_test.dev(''ines''), 1, %L, ''family'', ''Second'')', h));
    perform iamb_test.call('ines', format('api.invite(iamb_test.dev(''ines''), 1, %L, %L, %L, ''member'')', inv, ids ->> 'g', iamb_test.email('ana')));
    perform iamb_test.call('ana', format('api.accept_invitation(iamb_test.dev(''ana''), 1, %L)', inv));
    r := iamb_test.push('ines', jsonb_build_array(iamb_test.op_share((ids ->> 'f2')::uuid, h, 'view')));
    -- Two verified files: one whose object will be removed after the backup.
    insert into iamb.attachment_blob values (pi, sha_gone, 1, 'image/png', 1);
    insert into storage.objects (bucket_id, name) values ('attachments', pi || '/' || sha_gone);
    perform iamb_test.pull('tom');
    perform iamb_test.pull('ana');
    -- w1: committed just before the backup.
    r := iamb_test.push('tom', jsonb_build_array(iamb_test.op_snippet((ids ->> 's1')::uuid, (ids ->> 'f1')::uuid, ';hrs', 'w1',
                                                   iamb_test.rev('snippet', (ids ->> 's1')::uuid))));
    perform iamb_test.put('ids', ids || jsonb_build_object('h', h, 'w1', iamb_test.rev('snippet', (ids ->> 's1')::uuid),
                                                           'sha_kept', sha_kept, 'sha_gone', sha_gone,
                                                           'old_epoch', (select epoch from iamb.server_state)));
end $$;
SQL

backup_at=$(now_ms)
"$PG_BIN/pg_dump" -U supabase_admin -Fc -d "$db" -f "$work/backup.dump"

# --------------------------------------------------- after the backup: what the restore will lose
psql -U postgres -d "$db" <<'SQL' >/dev/null
do $$
declare
    ids jsonb := iamb_test.get('ids'); r jsonb; lost_folder uuid := gen_random_uuid(); pi uuid;
begin
    pi := (ids ->> 'p_ines')::uuid;
    -- w2: Tom's edit, acknowledged, then lost.
    r := iamb_test.push('tom', jsonb_build_array(iamb_test.op_snippet((ids ->> 's1')::uuid, (ids ->> 'f1')::uuid, ';hrs', 'w2',
                                                   iamb_test.rev('snippet', (ids ->> 's1')::uuid))));
    -- A folder created after the backup.
    r := iamb_test.push('ines', jsonb_build_array(iamb_test.op_folder(lost_folder, pi, null, 'Made after the backup')));
    -- What took access away: Ana removed, the second share ended, a device revoked.
    r := iamb_test.call('ines', format('api.remove_member(iamb_test.dev(''ines''), 1, %L, %L)', ids ->> 'g', iamb_test.uid('ana')));
    r := iamb_test.push('ines', jsonb_build_array(iamb_test.op_delete('folder_share', iamb_private.share_id((ids ->> 'f2')::uuid, (ids ->> 'h')::uuid))));
    r := iamb_test.call('ines', 'api.revoke_device(iamb_test.dev(''ines''), 1, iamb_test.dev(''ines_b''))');
    -- A file committed after the backup; the other file's object removed.
    insert into iamb.attachment_blob values (pi, ids ->> 'sha_kept', 1, 'image/png', 2);
    insert into storage.objects (bucket_id, name) values ('attachments', pi || '/' || (ids ->> 'sha_kept'));
    delete from storage.objects where name = pi || '/' || (ids ->> 'sha_gone');
    -- A session opened after the backup.
    perform iamb_test.new_device('late', 'ines');
    perform iamb_test.put('after', jsonb_build_object(
        'w2', iamb_test.rev('snippet', (ids ->> 's1')::uuid), 'lost_folder', lost_folder,
        'max_rev', (select last_value from iamb.rev_seq),
        'late_session', (select session_id from iamb_test.actor where name = 'late')));
end $$;
SQL
after=$(q "$db" "select iamb_test.get('after')::text")

# The function log of this database: its lines in the server's log (the prefix names the database).
sleep 0.3
grep -F " $db " "$SERVER_LOG" | grep -F "iamb_log " > "$work/function.log" || true

# ------------------------------------------------------------------------------------ the restore
rdb="r_${db}"
"$PG_BIN/psql" -X -q -U supabase_admin -d postgres -c "create database $rdb template template0" >/dev/null
"$PG_BIN/pg_restore" -U supabase_admin -d "$rdb" "$work/backup.dump"
trap '"$PG_BIN/psql" -X -q -U supabase_admin -d postgres -c "drop database if exists $rdb with (force)" >/dev/null 2>&1; rm -rf "$work"' EXIT

# Right after the restore, before the script: no row of a profile has a revision above its
# head_rev (§11.6), and w1 is in the backup at or below it.
psql -U postgres -d "$rdb" <<'SQL' >/dev/null
do $$
declare ids jsonb := iamb_test.get('ids');
begin
    perform iamb_test.ok(not exists (
        select 1 from iamb.wire_rows w
        where w.rev > coalesce((select p.head_rev from iamb.profile p
                                where p.id = coalesce(w.profile_id, iamb_private.row_profile(w.entity, w.id))), 0)),
        'right after the restore no row is above its profile''s head_rev');
    perform iamb_test.ok((select rev from iamb.snippet where id = (ids ->> 's1')::uuid) = (ids ->> 'w1')::bigint
                         and (ids ->> 'w1')::bigint <= (select head_rev from iamb.profile where id = (ids ->> 'p_ines')::uuid),
                         'a write committed just before the backup is in it, at or below head_rev');
end $$;
SQL

# The bucket as the storage interface lists it now: the original database's objects stand in for it.
q "$db" "select name from storage.objects where bucket_id = 'attachments' order by name" > "$work/objects.txt"
# The listing lacks the one verified file the restored database knows (its object was removed
# after the backup). A listing that lacks more than half of them could as well be a listing that
# went wrong, and every file it leaves out would be unbacked: the script refuses it before it
# changes anything, and takes it with --force (security review 4.3, §7 on correction 7).
if PSQL="$PG_BIN/psql" python3 -I "$server/restore/restore.py" --dsn "dbname=$rdb user=postgres" \
    --log "$work/function.log" --backup-at "$backup_at" --objects "$work/objects.txt" > "$work/refused.json" 2>"$work/refused.err"; then
    echo "restore.py took a bucket listing that lacks every verified file, without --force" >&2; exit 1
fi
grep -q -- "--force" "$work/refused.err" || { cat "$work/refused.err" >&2; exit 1; }
[[ "$(q "$rdb" "select maintenance::text || ' ' || (select count(*) from iamb.epoch) from iamb.server_state")" == "false 1" ]] \
    || { echo "the refused run changed the database" >&2; exit 1; }
PSQL="$PG_BIN/psql" python3 -I "$server/restore/restore.py" --dsn "dbname=$rdb user=postgres" --force \
    --log "$work/function.log" --backup-at "$backup_at" --objects "$work/objects.txt" > "$work/report.json" 2>"$work/report.err" \
    || { cat "$work/report.err" >&2; exit 1; }

# ------------------------------------------------------------------------------------- the checks
psql -U postgres -d "$rdb" -v after="$after" -v report="$(cat "$work/report.json")" <<'SQL' >/dev/null
select iamb_test.put('after', :'after'::jsonb);
select iamb_test.put('report', :'report'::jsonb);
do $$
declare
    ids jsonb := iamb_test.get('ids'); after jsonb := iamb_test.get('after'); report jsonb := iamb_test.get('report');
    ep iamb.epoch; backup bigint; r jsonb; pi uuid; s1 uuid; f1 uuid;
begin
    pi := (ids ->> 'p_ines')::uuid; s1 := (ids ->> 's1')::uuid; f1 := (ids ->> 'f1')::uuid;
    select e.* into ep from iamb.epoch e join iamb.server_state s on s.epoch = e.id;
    backup := (select backup_rev from iamb.epoch_profile where epoch_id = ep.id and profile_id = pi);
    perform iamb_test.ok(ep.id <> (ids ->> 'old_epoch')::uuid, 'a new epoch');
    -- G23: the first revision lies above every revision given out before, and above the floor.
    perform iamb_test.ok(ep.first_rev > (after ->> 'max_rev')::bigint, 'first_rev above every revision the lost time gave out');
    perform iamb_test.ok(ep.first_rev >= ((report ->> 'not_before')::bigint) * 1024, 'first_rev from the operator''s floor');
    perform iamb_test.ok(not exists (select 1 from iamb.wire_rows w
        where w.rev < ep.first_rev
          and w.rev > coalesce((select x.backup_rev from iamb.epoch_profile x where x.epoch_id = ep.id
                                and x.profile_id = coalesce(w.profile_id, iamb_private.row_profile(w.entity, w.id))), 0)),
        'every row below the new epoch is at or below its profile''s backup_rev');
    perform iamb_test.ok((select rev from iamb.snippet where id = s1) = (ids ->> 'w1')::bigint, 'w2 was lost');
    perform iamb_test.ok((after ->> 'w2')::bigint > backup, 'a write committed just after the backup is above the profile''s line');
    -- The script made again what took access away, and nothing that gives access.
    perform iamb_test.ok(exists (select 1 from jsonb_array_elements(report -> 'calls') c
                                 where c -> 'line' ->> 'event' = 'remove_member' and c ->> 'result' = 'made_again'), 'remove_member made again');
    perform iamb_test.ok(exists (select 1 from jsonb_array_elements(report -> 'calls') c
                                 where c -> 'line' ->> 'event' = 'share_deleted' and c ->> 'result' = 'made_again'), 'share delete made again');
    perform iamb_test.ok(exists (select 1 from jsonb_array_elements(report -> 'calls') c
                                 where c -> 'line' ->> 'event' = 'revoke_device' and c ->> 'result' = 'made_again'), 'revoke made again');
    perform iamb_test.ok(not exists (select 1 from jsonb_array_elements(report -> 'calls') c
                                     where c -> 'line' ->> 'event' in ('invite', 'accept_invitation', 'share_created', 'create_group')),
                         'no call that gives access');
    perform iamb_test.ok(not exists (select 1 from iamb.membership where profile_id = (ids ->> 'g')::uuid
                                     and user_id = iamb_test.uid('ana') and deleted_at is null), 'ana is removed again');
    perform iamb_test.ok((select deleted_at from iamb.folder_share where id = iamb_private.share_id((ids ->> 'f2')::uuid, (ids ->> 'h')::uuid)) is not null,
                         'the share is ended again');
    perform iamb_test.ok((select revoked_at from iamb.device where id = iamb_test.dev('ines_b')) is not null, 'the device is revoked again');
    perform iamb_test.ok(not exists (select 1 from iamb.device where position_token is not null), 'positions forgotten');
    -- Files: the blob row whose object is gone is removed; the object without a row is listed.
    perform iamb_test.ok(not exists (select 1 from iamb.attachment_blob where sha256 = ids ->> 'sha_gone'), 'blob row without object removed');
    perform iamb_test.ok(report -> 'files' -> 'objects_to_hash' ? (pi || '/' || (ids ->> 'sha_kept')), 'object without row listed to hash');
    -- Maintenance until the operator opens.
    r := iamb_test.call('tom', format('api.pull(iamb_test.dev(''tom''), 1, %L)', iamb_test.pos('tom')));
    perform iamb_test.eq(r ->> 'error', 'maintenance', 'closed until the operator opens');
    perform iamb_test.check_access();
    perform iamb_test.check_tree();
end $$;
SQL

PSQL="$PG_BIN/psql" python3 -I "$server/restore/restore.py" --dsn "dbname=$rdb user=postgres" --open >/dev/null

psql -U postgres -d "$rdb" <<'SQL' >/dev/null
do $$
declare
    ids jsonb := iamb_test.get('ids'); after jsonb := iamb_test.get('after');
    ep iamb.epoch; backup bigint; r jsonb; pi uuid; s1 uuid; f1 uuid; w2 bigint; lost_pos text; snap jsonb;
begin
    pi := (ids ->> 'p_ines')::uuid; s1 := (ids ->> 's1')::uuid; f1 := (ids ->> 'f1')::uuid; w2 := (after ->> 'w2')::bigint;
    select e.* into ep from iamb.epoch e join iamb.server_state s on s.epoch = e.id;
    backup := (select backup_rev from iamb.epoch_profile where epoch_id = ep.id and profile_id = pi);
    -- A session opened after the backup does not exist in the restored database.
    insert into iamb_test.actor (name, user_id, session_id, device_id) values ('late', iamb_test.uid('ines'), (after ->> 'late_session')::uuid, gen_random_uuid());
    perform iamb_test.eq(iamb_test.hello('late') ->> 'error', 'session_refused', 'a session opened after the backup');
    -- A device that holds a lost revision is not "behind": the new epoch lies above it.
    r := iamb_test.call('tom', format('api.hello(jsonb_build_object(''id'', iamb_test.dev(''tom''), ''name'', ''T'', ''platform'', ''linux'', ''app_version'', ''1''), array[1,1], %s)', w2));
    perform iamb_test.ok(r ->> 'error' is null and r ->> 'epoch' = ep.id::text, 'hello names the new epoch: ' || r::text);
    -- Every position of the older epoch is reset; the snapshot says what the restore lost.
    lost_pos := iamb_test.pos('tom');
    r := iamb_test.call('tom', format('api.pull(iamb_test.dev(''tom''), 1, %L)', lost_pos));
    perform iamb_test.ok(r ->> 'error' = 'reset_required' and r ->> 'reason' = 'epoch', 'reset_required: epoch');
    snap := iamb_test.call('tom', format('api.snapshot(iamb_test.dev(''tom''), 1, %L)', lost_pos));
    perform iamb_test.ok(snap -> 'lost' ->> 'epoch' = ep.id::text
                         and exists (select 1 from jsonb_array_elements(snap -> 'lost' -> 'profiles') x
                                     where x ->> 'profile' = pi::text and (x ->> 'backup_rev')::bigint = backup),
                         'lost names the line: ' || (snap -> 'lost')::text);
    update iamb_test.actor set position = snap ->> 'start' where name = 'tom';
    -- C18 case 1: Tom gives back w2, which the server lost; nobody wrote the row since.
    r := iamb_test.push('tom', jsonb_build_array(iamb_test.op_snippet(s1, f1, ';hrs', 'w2', w2, jsonb_build_object(
        'repush', 'row', 'repush_rev', w2, 'repush_epoch', ids ->> 'old_epoch', 'author', 'self'))));
    perform iamb_test.ok(iamb_test.st(r) = 'applied' and r -> 'results' -> 0 -> 'conflict' = 'null'::jsonb, 'case 1: ' || r::text);
    perform iamb_test.ok((select lost_rev from iamb.snippet where id = s1) = w2 and (select body from iamb.snippet where id = s1) = 'w2'
                         and (select content_by from iamb.snippet where id = s1) = iamb_test.uid('tom'), 'lost_rev and the author');
    -- C18 case 3: the same lost revision given back again by Ines's device: superseded.
    r := iamb_test.call('ines', format('api.snapshot(iamb_test.dev(''ines''), 1, %L)', iamb_test.pos('ines')));
    update iamb_test.actor set position = r ->> 'start' where name = 'ines';
    r := iamb_test.push('ines', jsonb_build_array(iamb_test.op_snippet(s1, f1, ';hrs', 'w2 as Ines held it', w2, jsonb_build_object(
        'repush', 'row', 'repush_rev', w2, 'repush_epoch', ids ->> 'old_epoch'))));
    perform iamb_test.eq(iamb_test.st(r), 'superseded', 'case 3');
    perform iamb_test.eq((select body from iamb.snippet where id = s1), 'w2', 'the row stands');
    -- Outside the span: an ordinary write based on that revision (stale: a conflict).
    r := iamb_test.push('ines', jsonb_build_array(iamb_test.op_snippet(s1, f1, ';hrs', 'old', backup, jsonb_build_object(
        'repush', 'row', 'repush_rev', backup, 'repush_epoch', ids ->> 'old_epoch'))));
    perform iamb_test.ok(iamb_test.st(r) = 'applied' and r -> 'results' -> 0 ->> 'conflict' is not null, 'outside the span: ' || r::text);
    -- C18 case 4: a row the server does not know comes back, within retention_days.
    r := iamb_test.push('ines', jsonb_build_array(iamb_test.op_folder((after ->> 'lost_folder')::uuid, pi, null, 'Made after the backup',
        (after ->> 'max_rev')::bigint, jsonb_build_object('repush', 'row', 'repush_rev', (after ->> 'max_rev')::bigint, 'repush_epoch', ids ->> 'old_epoch',
                                                           'author', 'self'))));
    perform iamb_test.eq(iamb_test.st(r), 'applied', 'case 4: the lost folder comes back');
    -- Permission is judged on arrival (C12): Ana, removed again, gives nothing back.
    r := iamb_test.call('ana', format('api.snapshot(iamb_test.dev(''ana''), 1, %L)', iamb_test.pos('ana')));
    update iamb_test.actor set position = r ->> 'start' where name = 'ana';
    r := iamb_test.push('ana', jsonb_build_array(iamb_test.op_snippet(s1, f1, ';hrs', 'ana', w2, jsonb_build_object(
        'repush', 'row', 'repush_rev', w2, 'repush_epoch', ids ->> 'old_epoch', 'author', 'self'))));
    perform iamb_test.ok(iamb_test.st(r) like 'rejected:%', 'a removed member''s re-push: ' || r::text);
    -- The floor holds when the server's clock is behind it.
    r := iamb_private.begin_epoch(iamb_private.now_ms() + 86400000);
    perform iamb_test.ok((r ->> 'first_rev')::bigint >= (iamb_private.now_ms() + 86400000 - 1000) * 1024, 'the floor wins over the clock');
    perform iamb_test.check_access();
    perform iamb_test.check_tree();
end $$;
SQL
