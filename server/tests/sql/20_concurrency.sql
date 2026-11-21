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

select extensions.dblink_disconnect('c2');
