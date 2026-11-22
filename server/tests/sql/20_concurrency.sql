-- G21 and G19: two requests at the same moment end as some order of the two (C24). Each case
-- starts the second request on another connection while the first holds its locks, checks that
-- the second waits, then lets the first commit; afterwards I7 and G1 hold. Run as a superuser
-- because the second connection is opened with dblink.
\set ON_ERROR_STOP on
set client_min_messages = warning;

do $$
declare
    ids jsonb := iamb_test.family('edit');
    pi uuid; g uuid; x uuid := gen_random_uuid(); y uuid := gen_random_uuid(); f uuid := gen_random_uuid();
    sub uuid := gen_random_uuid(); gf uuid := gen_random_uuid(); r jsonb; inv uuid := gen_random_uuid();
begin
    pi := (ids ->> 'p_ines')::uuid; g := (ids ->> 'g')::uuid;
    perform iamb_test.new_device('ines_b', 'ines'); perform iamb_test.hello('ines_b');
    r := iamb_test.push('ines', jsonb_build_array(
        iamb_test.op_folder(x, pi, null, 'X'), iamb_test.op_folder(y, pi, null, 'Y'), iamb_test.op_folder(f, pi, null, 'F'),
        iamb_test.op_folder(gf, g, null, 'Group folder', 0, jsonb_build_object('row', jsonb_build_object('member_permission', 'edit')))));
    perform iamb_test.ok(iamb_test.st(r, 4) = 'applied', 'setup ' || r::text);
    perform iamb_test.put('ids', ids || jsonb_build_object('x', x, 'y', y, 'f', f, 'sub', sub, 'gf', gf));
end $$;

select extensions.dblink_connect('c2', format('host=%s port=%s dbname=%s user=supabase_admin application_name=iamb_c2',
       current_setting('unix_socket_directories'), current_setting('port'), current_database()));
select extensions.dblink_exec('c2', 'set client_min_messages = warning');

-- The second connection is waiting on a lock the first holds.
create or replace function pg_temp.c2_waits() returns boolean language sql as $$
    select exists (select 1 from pg_stat_activity where application_name = 'iamb_c2' and wait_event_type = 'Lock')
$$;
-- The answer of the second connection; dblink wants one more call, which gives an empty set,
-- before the connection takes the next query.
create or replace function pg_temp.c2_result() returns jsonb language plpgsql as $$
declare
    res jsonb;
begin
    select r::jsonb into res from extensions.dblink_get_result('c2') as t(r text);
    perform 1 from extensions.dblink_get_result('c2') as t(r text);
    return res;
end $$;

-- (a) Folders moved into each other (S13): the second finds the first and keeps its parent.
begin;
select iamb_test.st(iamb_test.push('ines', jsonb_build_array(iamb_test.op_folder((iamb_test.get('ids') ->> 'x')::uuid,
       (iamb_test.get('ids') ->> 'p_ines')::uuid, (iamb_test.get('ids') ->> 'y')::uuid, 'X',
       iamb_test.rev('folder', (iamb_test.get('ids') ->> 'x')::uuid))))) as a1;
select extensions.dblink_send_query('c2', format('select iamb_test.push(%L, %L::jsonb)::text', 'ines_b',
       jsonb_build_array(iamb_test.op_folder((iamb_test.get('ids') ->> 'y')::uuid, (iamb_test.get('ids') ->> 'p_ines')::uuid,
       (iamb_test.get('ids') ->> 'x')::uuid, 'Y', iamb_test.rev('folder', (iamb_test.get('ids') ->> 'y')::uuid)))));
select pg_sleep(0.5);
select iamb_test.ok(pg_temp.c2_waits(), '(a) the second request waits for the first');
commit;
select iamb_test.put('a', pg_temp.c2_result());
do $$
begin
    -- The move was the op's only change, so the answer is "unchanged+adjusted" (§6.3).
    perform iamb_test.ok(iamb_test.st(iamb_test.get('a')) = 'unchanged' and iamb_test.get('a') -> 'results' -> 0 -> 'adjusted' = '["parent_id"]',
                         '(a) the second move keeps its parent: ' || iamb_test.get('a')::text);
    perform iamb_test.check_tree();
    perform iamb_test.check_access();
end $$;

-- (b) A create in a folder being deleted (S16, S53): the delete commits first; the create finds
-- the tombstone and restores the folder (C7).
begin;
select iamb_test.st(iamb_test.push('ines', jsonb_build_array(iamb_test.op_delete('folder', (iamb_test.get('ids') ->> 'f')::uuid,
       iamb_test.rev('folder', (iamb_test.get('ids') ->> 'f')::uuid))))) as b1;
select extensions.dblink_send_query('c2', format('select iamb_test.push(%L, %L::jsonb)::text', 'ines_b',
       jsonb_build_array(iamb_test.op_snippet(gen_random_uuid(), (iamb_test.get('ids') ->> 'f')::uuid, ';new', 'new'))));
select pg_sleep(0.5);
select iamb_test.ok(pg_temp.c2_waits(), '(b) the second request waits for the first');
commit;
select iamb_test.put('b', pg_temp.c2_result());
do $$
begin
    perform iamb_test.ok(iamb_test.st(iamb_test.get('b')) = 'applied', '(b) the create is applied: ' || iamb_test.get('b')::text);
    perform iamb_test.ok((select deleted_at from iamb.folder where id = (iamb_test.get('ids') ->> 'f')::uuid) is null, '(b) the folder is restored');
    perform iamb_test.check_tree();
    perform iamb_test.check_access();
end $$;
-- ... and the other order: the create commits first, then the delete takes it (deleted_with_folder).
begin;
select iamb_test.st(iamb_test.push('ines_b', jsonb_build_array(iamb_test.op_snippet(gen_random_uuid(), (iamb_test.get('ids') ->> 'f')::uuid, ';two', 'two')))) as b2;
select extensions.dblink_send_query('c2', format('select iamb_test.push(%L, %L::jsonb)::text', 'ines',
       jsonb_build_array(iamb_test.op_delete('folder', (iamb_test.get('ids') ->> 'f')::uuid, iamb_test.rev('folder', (iamb_test.get('ids') ->> 'f')::uuid)))));
select pg_sleep(0.5);
select iamb_test.ok(pg_temp.c2_waits(), '(b2) the second request waits for the first');
commit;
select iamb_test.put('b2', pg_temp.c2_result());
do $$
begin
    perform iamb_test.ok(iamb_test.st(iamb_test.get('b2')) = 'applied', '(b2) the delete is applied: ' || iamb_test.get('b2')::text);
    perform iamb_test.ok(not exists (select 1 from iamb.snippet where folder_id = (iamb_test.get('ids') ->> 'f')::uuid and deleted_at is null),
                         '(b2) the create is deleted with the folder');
    perform iamb_test.ok(exists (select 1 from iamb.conflict c join iamb.snippet s on s.id = c.row_id
                                 where s.abbreviation = ';two' and c.kind = 'deleted_with_folder'), '(b2) deleted_with_folder');
    perform iamb_test.check_tree();
    perform iamb_test.check_access();
end $$;

-- (c) A sub-folder created while its share ends: the share's end commits first; the create is
-- refused, and no access entry survives (S37, G13).
select iamb_test.pull('tom') is not null;
begin;
select iamb_test.st(iamb_test.push('ines', jsonb_build_array(iamb_test.op_delete('folder_share',
       iamb_private.share_id((iamb_test.get('ids') ->> 'f1')::uuid, (iamb_test.get('ids') ->> 'g')::uuid))))) as c1;
select extensions.dblink_send_query('c2', format('select iamb_test.push(%L, %L::jsonb)::text', 'tom',
       jsonb_build_array(iamb_test.op_folder((iamb_test.get('ids') ->> 'sub')::uuid, (iamb_test.get('ids') ->> 'p_ines')::uuid,
       (iamb_test.get('ids') ->> 'f1')::uuid, 'Sub'))));
select pg_sleep(0.5);
select iamb_test.ok(pg_temp.c2_waits(), '(c) the second request waits for the first');
commit;
select iamb_test.put('c', pg_temp.c2_result());
do $$
begin
    perform iamb_test.eq(iamb_test.st(iamb_test.get('c')), 'rejected:parent_gone', '(c) the sub-folder is refused');
    perform iamb_test.ok(not exists (select 1 from iamb.folder where id = (iamb_test.get('ids') ->> 'sub')::uuid), '(c) not written');
    perform iamb_test.ok(not exists (select 1 from iamb.folder_access where user_id = iamb_test.uid('tom')
                                     and profile_id = (iamb_test.get('ids') ->> 'p_ines')::uuid), '(c) no access left');
    perform iamb_test.check_tree();
    perform iamb_test.check_access();
end $$;

-- (d) A push into a group's folder while a member is removed: the removal commits first; the
-- push is refused.
begin;
select iamb_test.call('ines', format('api.remove_member(iamb_test.dev(''ines''), 1, %L, %L)', iamb_test.get('ids') ->> 'g', iamb_test.uid('tom'))) ->> 'error' as d1;
select extensions.dblink_send_query('c2', format('select iamb_test.push(%L, %L::jsonb)::text', 'tom',
       jsonb_build_array(iamb_test.op_snippet(gen_random_uuid(), (iamb_test.get('ids') ->> 'gf')::uuid, ';g', 'into the group folder'))));
select pg_sleep(0.5);
select iamb_test.ok(pg_temp.c2_waits(), '(d) the second request waits for the first');
commit;
select iamb_test.put('d', pg_temp.c2_result());
do $$
begin
    perform iamb_test.eq(iamb_test.st(iamb_test.get('d')), 'rejected:parent_gone', '(d) the push is refused');
    perform iamb_test.ok(not exists (select 1 from iamb.snippet where folder_id = (iamb_test.get('ids') ->> 'gf')::uuid), '(d) nothing written');
    perform iamb_test.check_tree();
    perform iamb_test.check_access();
end $$;

-- (e) G19: the same push delivered twice at the same moment writes once.
select iamb_test.put('op', iamb_test.op_snippet(gen_random_uuid(), (iamb_test.get('ids') ->> 'x')::uuid, ';twice', 'twice'));
begin;
select iamb_test.st(iamb_test.push('ines', jsonb_build_array(iamb_test.get('op')))) as e1;
select extensions.dblink_send_query('c2', format('select iamb_test.push(%L, %L::jsonb)::text', 'ines', jsonb_build_array(iamb_test.get('op'))));
select pg_sleep(0.5);
select iamb_test.ok(pg_temp.c2_waits(), '(e) the copy waits on the device''s lock');
commit;
select iamb_test.put('e', pg_temp.c2_result());
do $$
begin
    perform iamb_test.ok(iamb_test.st(iamb_test.get('e')) = 'applied' and (iamb_test.get('e') -> 'results' -> 0 ->> 'replayed')::boolean,
                         '(e) the copy is answered from the receipt: ' || iamb_test.get('e')::text);
    perform iamb_test.eq((select count(*)::integer from iamb.snippet where abbreviation = ';twice'), 1, '(e) one row');
    perform iamb_test.eq((select count(*)::integer from iamb.history where row_id = (iamb_test.get('op') ->> 'id')::uuid), 0, '(e) one revision');
    perform iamb_test.check_all();
end $$;

-- Security review 4.3, finding 3: two requests that create one id in two profiles. An id is unique
-- across all profiles and the profile locks are per profile, so the second request holds another
-- lock than the first; it waits on the lock of the row's id, finds the first one's row when it
-- gets it, and is answered `blocked: invalid`. It never becomes an update of the first one's row.
-- (f) An attachment row: request 1 creates row A in Ines's profile with the hash of her private
-- file (allowed, not backed); request 2 creates "the same" row A in Tom's own profile with the hash
-- of a file of his own, for which the server computes `backed`.
do $$
declare
    ids jsonb := iamb_test.family('edit');
begin
    perform iamb_test.new_device('tom2', 'tom'); perform iamb_test.hello('tom2'); perform iamb_test.pull('tom');
    insert into iamb.attachment_blob values ((ids ->> 'p_ines')::uuid, repeat('a1', 32), 1000, 'image/png', 1),
                                            ((ids ->> 'p_tom')::uuid, repeat('70', 32), 10, 'image/png', 1);
    insert into storage.objects (bucket_id, name) values ('attachments', (ids ->> 'p_ines') || '/' || repeat('a1', 32));
    perform iamb_test.put('ids', ids || jsonb_build_object('A', gen_random_uuid(), 'X', gen_random_uuid()));
end $$;
begin;
select iamb_test.put('f1', iamb_test.push('tom', jsonb_build_array(iamb_test.op_attachment((iamb_test.get('ids') ->> 'A')::uuid,
       (iamb_test.get('ids') ->> 'p_ines')::uuid, repeat('a1', 32))))) is not null;
select extensions.dblink_send_query('c2', format('select iamb_test.push(%L, %L::jsonb)::text', 'tom2',
       jsonb_build_array(iamb_test.op_attachment((iamb_test.get('ids') ->> 'A')::uuid, (iamb_test.get('ids') ->> 'p_tom')::uuid, repeat('70', 32)))));
select pg_sleep(0.5);
select iamb_test.ok(pg_temp.c2_waits(), '(f) the second request waits for the first');
commit;
select iamb_test.put('f', pg_temp.c2_result());
do $$
declare
    a uuid := (iamb_test.get('ids') ->> 'A')::uuid; r jsonb; n integer;
begin
    perform iamb_test.ok(iamb_test.st(iamb_test.get('f1')) = 'applied' and iamb_test.get('f1') -> 'results' -> 0 ->> 'blob' = 'missing',
                         '(f) the first request creates the row, not backed: ' || iamb_test.get('f1')::text);
    perform iamb_test.eq(iamb_test.st(iamb_test.get('f')), 'blocked:invalid', '(f) the second request is refused');
    perform iamb_test.ok((select profile_id = (iamb_test.get('ids') ->> 'p_ines')::uuid and sha256 = repeat('a1', 32) and not backed
                          from iamb.attachment where id = a), '(f) the row is the first request''s, and not backed');
    perform iamb_test.eq((select count(*)::integer from iamb.attachment where id = a), 1, '(f) one row');
    -- Linked to the shared snippet, it gives Tom no file.
    perform iamb_test.pull('tom');
    r := iamb_test.push('tom', jsonb_build_array(iamb_test.op_snippet_attachment(gen_random_uuid(), (iamb_test.get('ids') ->> 's1')::uuid, a)));
    perform iamb_test.act('tom');
    select count(*) into n from storage.objects;
    perform iamb_test.act_owner();
    perform iamb_test.eq(n, 0, '(f) the private file is not readable for Tom');
    r := iamb_test.call('tom', format('api.attachment_upload_begin(iamb_test.dev(''tom''), 1, %L, %L, 1000, ''image/png'')',
                                      iamb_test.get('ids') ->> 'p_ines', repeat('a1', 32)));
    perform iamb_test.ok(r ->> 'error' is null, '(f) and upload_begin answers him as for a file that does not exist: ' || r::text);
    perform iamb_test.check_tree();
    perform iamb_test.check_access();
end $$;
-- (g) A folder: request 1 creates X under the shared folder; request 2 creates "the same" X at the
-- top level of Tom's own profile.
begin;
select iamb_test.st(iamb_test.push('tom', jsonb_build_array(iamb_test.op_folder((iamb_test.get('ids') ->> 'X')::uuid,
       (iamb_test.get('ids') ->> 'p_ines')::uuid, (iamb_test.get('ids') ->> 'f1')::uuid, 'X under the shared folder')))) as g1;
select extensions.dblink_send_query('c2', format('select iamb_test.push(%L, %L::jsonb)::text', 'tom2',
       jsonb_build_array(iamb_test.op_folder((iamb_test.get('ids') ->> 'X')::uuid, (iamb_test.get('ids') ->> 'p_tom')::uuid, null, 'X at the top of my own'))));
select pg_sleep(0.5);
select iamb_test.ok(pg_temp.c2_waits(), '(g) the second request waits for the first');
commit;
select iamb_test.put('g', pg_temp.c2_result());
do $$
declare
    x uuid := (iamb_test.get('ids') ->> 'X')::uuid;
begin
    perform iamb_test.eq(iamb_test.st(iamb_test.get('g')), 'blocked:invalid', '(g) the second request is refused');
    perform iamb_test.ok((select profile_id = (iamb_test.get('ids') ->> 'p_ines')::uuid and parent_id = (iamb_test.get('ids') ->> 'f1')::uuid
                                 and name = 'X under the shared folder' from iamb.folder where id = x),
                         '(g) the folder is where the first request put it');
    perform iamb_test.check_tree();
    perform iamb_test.check_access();
    perform iamb_test.check_all();
end $$;

-- Security review 4.3, finding 6: a request takes no lock on a profile its caller has no grant in.
-- (h) A stranger's push names Ines's personal profile and her group and stays open; Ines's own
-- push does not wait for it.
begin;
select iamb_test.st(iamb_test.push('stranger', jsonb_build_array(
       iamb_test.op_tag(gen_random_uuid(), (iamb_test.get('ids') ->> 'p_ines')::uuid, 'x'),
       iamb_test.op_tag(gen_random_uuid(), (iamb_test.get('ids') ->> 'g')::uuid, 'x'),
       iamb_test.op_snippet(gen_random_uuid(), (iamb_test.get('ids') ->> 'f2')::uuid, ';x', 'x')))) as h1;
select extensions.dblink_send_query('c2', format('select iamb_test.push(%L, %L::jsonb)::text', 'ines',
       jsonb_build_array(iamb_test.op_snippet((iamb_test.get('ids') ->> 's2')::uuid, (iamb_test.get('ids') ->> 'f2')::uuid, ';priv', 'edited while the stranger''s request is open',
                         iamb_test.rev('snippet', (iamb_test.get('ids') ->> 's2')::uuid)))));
select pg_sleep(0.7);
select iamb_test.ok(not pg_temp.c2_waits(), '(h) the owner''s request does not wait for the stranger''s');
select iamb_test.ok(not exists (select 1 from pg_stat_activity where application_name = 'iamb_c2' and state = 'active'),
                    '(h) and has been answered while the stranger''s request is still open');
commit;
select iamb_test.put('h', pg_temp.c2_result());
do $$
begin
    perform iamb_test.eq(iamb_test.st(iamb_test.get('h')), 'applied', '(h) the owner''s write is applied');
    perform iamb_test.check_all();
end $$;
-- (i) The same for an account call and for an upload: a stranger's `attachment_upload_begin` and
-- `rename_group` for a profile he has no right in hold no lock.
begin;
select iamb_test.call('stranger', format('api.attachment_upload_begin(iamb_test.dev(''stranger''), 1, %L, %L, 10, ''image/png'')',
       iamb_test.get('ids') ->> 'p_ines', repeat('cd', 32))) ->> 'error' as i1;
select iamb_test.call('stranger', format('api.rename_group(iamb_test.dev(''stranger''), 1, %L, ''x'')', iamb_test.get('ids') ->> 'g')) ->> 'error' as i2;
select iamb_test.call('stranger', format('api.withdraw_invitation(iamb_test.dev(''stranger''), 1, %L)',
       (select id from iamb.invitation where profile_id = (iamb_test.get('ids') ->> 'g')::uuid limit 1))) ->> 'error' as i3;
select extensions.dblink_send_query('c2', format('select iamb_test.push(%L, %L::jsonb)::text', 'ines',
       jsonb_build_array(iamb_test.op_folder(gen_random_uuid(), (iamb_test.get('ids') ->> 'g')::uuid, null, 'A folder of the group'),
                         iamb_test.op_folder(gen_random_uuid(), (iamb_test.get('ids') ->> 'p_ines')::uuid, null, 'A folder of her own'))));
select pg_sleep(0.7);
select iamb_test.ok(not pg_temp.c2_waits(), '(i) the owner''s request does not wait for the stranger''s calls');
commit;
select iamb_test.put('i', pg_temp.c2_result());
do $$
begin
    perform iamb_test.ok(iamb_test.st(iamb_test.get('i'), 1) = 'applied' and iamb_test.st(iamb_test.get('i'), 2) = 'applied', '(i) the owner''s writes are applied');
    perform iamb_test.check_all();
end $$;

-- Re-verification N3 (docs/reviews/4.3-security-reverify.md): two ways that were left to stall
-- somebody else's writes.
-- (j) A reader holds no lock. Tom may only read the shared folder; his push into it is refused,
-- and while his request is still open Ines saves a snippet in her private folder without waiting.
do $$
declare
    ids jsonb := iamb_test.family('view');
begin
    perform iamb_test.pull('tom'); perform iamb_test.pull('ines'); perform iamb_test.pull('stranger');
    perform iamb_test.put('ids', ids);
end $$;
begin;
select iamb_test.put('j1', iamb_test.push('tom', jsonb_build_array(
       iamb_test.op_snippet(gen_random_uuid(), (iamb_test.get('ids') ->> 'f1')::uuid, ';x', 'x'),
       iamb_test.op_delete('snippet', (iamb_test.get('ids') ->> 's1')::uuid, iamb_test.rev('snippet', (iamb_test.get('ids') ->> 's1')::uuid))))) is not null;
select extensions.dblink_send_query('c2', format('select iamb_test.push(%L, %L::jsonb)::text', 'ines',
       jsonb_build_array(iamb_test.op_snippet((iamb_test.get('ids') ->> 's2')::uuid, (iamb_test.get('ids') ->> 'f2')::uuid, ';priv', 'edited while the reader''s request is open',
                         iamb_test.rev('snippet', (iamb_test.get('ids') ->> 's2')::uuid)))));
select pg_sleep(0.7);
select iamb_test.ok(not pg_temp.c2_waits(), '(j) the owner''s request does not wait for the reader''s');
select iamb_test.ok(not exists (select 1 from pg_stat_activity where application_name = 'iamb_c2' and state = 'active'),
                    '(j) and has been answered while the reader''s request is still open');
commit;
select iamb_test.put('j', pg_temp.c2_result());
do $$
begin
    perform iamb_test.ok(iamb_test.st(iamb_test.get('j1'), 1) = 'rejected:forbidden' and iamb_test.st(iamb_test.get('j1'), 2) = 'rejected:forbidden',
                         '(j) the reader''s ops are refused as before: ' || iamb_test.st(iamb_test.get('j1'), 1) || ', ' || iamb_test.st(iamb_test.get('j1'), 2));
    perform iamb_test.eq(iamb_test.st(iamb_test.get('j')), 'applied', '(j) the owner''s write is applied');
    perform iamb_test.check_all();
end $$;
-- (k) The lock on a row's id cannot be shared by an id somebody else chose. Two ids that had one
-- key under the 32-bit hash of the first form (found among 400,000 random ids): Ines's private
-- snippet has the first; a stranger creates a snippet with the second in his own folder and his
-- request stays open; Ines's edit of her snippet does not wait.
do $$
declare
    a uuid; b uuid; sf uuid := gen_random_uuid(); r jsonb;
begin
    with c as (select gen_random_uuid() as id from generate_series(1, 400000)),
         h as (select id, hashtext('snippet:' || id::text) as k from c),
         d as (select k from h group by k having count(*) > 1 limit 1)
    select (array_agg(h.id order by h.id))[1], (array_agg(h.id order by h.id))[2] into a, b from h join d using (k);
    perform iamb_test.ok(a is not null and b is not null and a <> b, '(k) two ids with one 32-bit key');
    r := iamb_test.push('ines', jsonb_build_array(iamb_test.op_snippet(a, (iamb_test.get('ids') ->> 'f2')::uuid, ';a', 'private row A')));
    perform iamb_test.eq(iamb_test.st(r), 'applied', '(k) her row');
    r := iamb_test.push('stranger', jsonb_build_array(iamb_test.op_folder(sf, iamb_test.personal('stranger'), null, 'own')));
    perform iamb_test.eq(iamb_test.st(r), 'applied', '(k) his folder');
    perform iamb_test.put('ids', iamb_test.get('ids') || jsonb_build_object('ka', a, 'kb', b, 'sf', sf));
end $$;
begin;
select iamb_test.put('k1', iamb_test.push('stranger', jsonb_build_array(
       iamb_test.op_snippet((iamb_test.get('ids') ->> 'kb')::uuid, (iamb_test.get('ids') ->> 'sf')::uuid, ';b', 'b')))) is not null;
select extensions.dblink_send_query('c2', format('select iamb_test.push(%L, %L::jsonb)::text', 'ines',
       jsonb_build_array(iamb_test.op_snippet((iamb_test.get('ids') ->> 'ka')::uuid, (iamb_test.get('ids') ->> 'f2')::uuid, ';a', 'edited',
                         iamb_test.rev('snippet', (iamb_test.get('ids') ->> 'ka')::uuid)))));
select pg_sleep(0.7);
select iamb_test.ok(not pg_temp.c2_waits(), '(k) the owner''s request does not wait for a stranger''s create of another id');
commit;
select iamb_test.put('k', pg_temp.c2_result());
do $$
begin
    perform iamb_test.eq(iamb_test.st(iamb_test.get('k1')), 'applied', '(k) the stranger''s create in his own folder');
    perform iamb_test.eq(iamb_test.st(iamb_test.get('k')), 'applied', '(k) the owner''s edit');
    perform iamb_test.check_all();
end $$;

select extensions.dblink_disconnect('c2');
