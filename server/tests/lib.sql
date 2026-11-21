-- Helpers for the Iamb server's policy tests (server/README.md). Not part of the migrations: the
-- test runner loads this file into its local database, and the hosted run loads it inside a
-- transaction that is rolled back. Accounts are rows of `auth.users` and `auth.sessions`; acting as
-- one sets the role `authenticated` and the token's claims as the REST interface would.

create schema if not exists iamb_test;
grant usage on schema iamb_test to authenticated;

create table if not exists iamb_test.actor (
    name       text primary key,
    user_id    uuid not null,
    session_id uuid not null,
    device_id  uuid not null,
    amr        text not null default 'otp',
    position   text
);
grant select on iamb_test.actor to authenticated;

-- Values handed from one top-level statement of a test to the next.
create table if not exists iamb_test.kv (k text primary key, v jsonb);
create or replace function iamb_test.put(p_k text, p_v jsonb) returns jsonb
language sql as $$
    insert into iamb_test.kv values (p_k, p_v) on conflict (k) do update set v = excluded.v returning v
$$;
create or replace function iamb_test.get(p_k text) returns jsonb
language sql stable as $$ select v from iamb_test.kv where k = p_k $$;

create or replace function iamb_test.ok(cond boolean, msg text) returns void
language plpgsql as $$
begin
    if cond is not true then
        raise exception 'check failed: %', msg;
    end if;
end $$;

create or replace function iamb_test.eq(a anyelement, b anyelement, msg text) returns void
language plpgsql as $$
begin
    if a is distinct from b then
        raise exception 'check failed: % (got %, expected %)', msg, a, b;
    end if;
end $$;

-- A new account with one signed-in installation: a user, a session and a device id.
create or replace function iamb_test.new_user(p_name text, p_confirmed boolean default true, p_amr text default 'otp')
returns uuid language plpgsql as $$
declare
    u uuid := gen_random_uuid();
    s uuid := gen_random_uuid();
begin
    insert into auth.users (id, aud, role, email, email_confirmed_at, created_at, updated_at)
    values (u, 'authenticated', 'authenticated', p_name || '-' || left(u::text, 8) || '@example.test',
            case when p_confirmed then now() end, now(), now());
    insert into auth.sessions (id, user_id, created_at, updated_at, aal)
    values (s, u, now(), now(), 'aal1');
    insert into iamb_test.actor (name, user_id, session_id, device_id, amr)
    values (p_name, u, s, gen_random_uuid(), p_amr);
    return u;
end $$;

-- Another installation of an existing account: its own session and device id.
create or replace function iamb_test.new_device(p_name text, p_of text, p_amr text default 'otp')
returns uuid language plpgsql as $$
declare
    u uuid := (select user_id from iamb_test.actor where name = p_of);
    s uuid := gen_random_uuid();
begin
    insert into auth.sessions (id, user_id, created_at, updated_at, aal) values (s, u, now(), now(), 'aal1');
    insert into iamb_test.actor (name, user_id, session_id, device_id, amr) values (p_name, u, s, gen_random_uuid(), p_amr);
    return u;
end $$;

create or replace function iamb_test.email(p_name text) returns text
language sql stable as $$
    select u.email from auth.users u join iamb_test.actor a on a.user_id = u.id where a.name = p_name
$$;

create or replace function iamb_test.uid(p_name text) returns uuid
language sql stable as $$ select user_id from iamb_test.actor where name = p_name $$;

create or replace function iamb_test.dev(p_name text) returns uuid
language sql stable as $$ select device_id from iamb_test.actor where name = p_name $$;

create or replace function iamb_test.pos(p_name text) returns text
language sql stable as $$ select position from iamb_test.actor where name = p_name $$;

-- Acts as the actor: the role and the claims the REST interface sets for its token.
create or replace function iamb_test.act(p_name text) returns void
language plpgsql as $$
declare
    a iamb_test.actor;
begin
    select * into a from iamb_test.actor where name = p_name;
    if a.name is null then
        raise exception 'no actor %', p_name;
    end if;
    perform set_config('request.jwt.claims', jsonb_build_object(
        'sub', a.user_id, 'role', 'authenticated', 'aud', 'authenticated', 'session_id', a.session_id,
        'amr', jsonb_build_array(jsonb_build_object('method', a.amr, 'timestamp', 1795190000)))::text, false);
    perform set_config('role', 'authenticated', false);
end $$;

-- Acts as an anonymous caller (the public key without a user token).
create or replace function iamb_test.act_anon() returns void
language plpgsql as $$
begin
    perform set_config('request.jwt.claims', '{"role": "anon"}', false);
    perform set_config('role', 'anon', false);
end $$;

create or replace function iamb_test.act_owner() returns void
language plpgsql as $$
begin
    execute 'reset role';
    perform set_config('request.jwt.claims', '', false);
end $$;

-- Runs `select <expr>` as the actor and answers its jsonb.
create or replace function iamb_test.call(p_name text, p_expr text) returns jsonb
language plpgsql as $$
declare
    r jsonb;
begin
    perform iamb_test.act(p_name);
    execute 'select (' || p_expr || ')::jsonb' into r;
    perform iamb_test.act_owner();
    return r;
exception when others then
    perform iamb_test.act_owner();
    raise;
end $$;

-- `hello`, then the first snapshot, whose `start` becomes the actor's position.
create or replace function iamb_test.hello(p_name text, p_display text default null) returns jsonb
language plpgsql as $$
declare
    h jsonb;
    s jsonb;
    d uuid := iamb_test.dev(p_name);
begin
    perform iamb_test.act(p_name);
    h := api.hello(jsonb_build_object('id', d, 'name', 'Test ' || p_name, 'platform', 'linux', 'app_version', '0.4.0'),
                   array[1, 1], 0);
    if h ->> 'error' is null then
        if p_display is not null then
            perform api.set_display_name(d, 1, p_display);
        end if;
        s := api.snapshot(d, 1, null, null);
    end if;
    perform iamb_test.act_owner();
    if s ->> 'start' is not null then
        update iamb_test.actor set position = s ->> 'start' where name = p_name;
    end if;
    return h;
end $$;

create or replace function iamb_test.push(p_name text, p_ops jsonb) returns jsonb
language plpgsql as $$
declare
    r jsonb;
begin
    perform iamb_test.act(p_name);
    r := api.push(iamb_test.dev(p_name), 1, iamb_test.pos(p_name), p_ops);
    perform iamb_test.act_owner();
    return r;
end $$;

-- Pulls until `more` is false; answers {error, entries, refs} and keeps the new position.
create or replace function iamb_test.pull(p_name text) returns jsonb
language plpgsql as $$
declare
    r       jsonb;
    entries jsonb := '[]';
    refs    jsonb := '[]';
    i       integer := 0;
begin
    loop
        perform iamb_test.act(p_name);
        r := api.pull(iamb_test.dev(p_name), 1, iamb_test.pos(p_name), 500);
        perform iamb_test.act_owner();
        if r ->> 'error' is not null then
            return r;
        end if;
        update iamb_test.actor set position = r ->> 'next' where name = p_name;
        entries := entries || (r -> 'entries');
        refs := refs || (r -> 'refs');
        i := i + 1;
        exit when not (r ->> 'more')::boolean or i > 100;
    end loop;
    return jsonb_build_object('error', null, 'entries', entries, 'refs', refs, 'head', r -> 'head');
end $$;

create or replace function iamb_test.personal(p_name text) returns uuid
language sql stable as $$
    select p.id from iamb.profile p join iamb_test.actor a on a.user_id = p.owner_user_id
    where a.name = p_name and p.kind = 'personal'
$$;

create or replace function iamb_test.id() returns uuid language sql volatile as $$ select gen_random_uuid() $$;

-- ---------------------------------------------------------------------------------------------
-- Ops in the wire form of §6.3
-- ---------------------------------------------------------------------------------------------

create or replace function iamb_test.op(p_entity text, p_id uuid, p_row jsonb, p_base bigint default 0,
                                        p_extra jsonb default '{}') returns jsonb
language sql volatile as $$
    select jsonb_build_object('op_id', gen_random_uuid(), 'entity', p_entity, 'id', p_id, 'kind', 'put',
                              'base_rev', p_base, 'row', p_row) || p_extra
$$;

create or replace function iamb_test.op_folder(p_id uuid, p_profile uuid, p_parent uuid, p_name text,
                                               p_base bigint default 0, p_extra jsonb default '{}') returns jsonb
language sql volatile as $$
    select iamb_test.op('folder', p_id, jsonb_build_object(
        'profile_id', p_profile, 'parent_id', p_parent, 'name', p_name, 'position', 0, 'member_permission', null,
        'defaults', jsonb_build_object('trigger', null, 'case_mode', null, 'abbreviation_prefix', null),
        'created_at', 1795190000000) || coalesce(p_extra -> 'row', '{}'), p_base, p_extra - 'row')
$$;

create or replace function iamb_test.op_snippet(p_id uuid, p_folder uuid, p_abbr text, p_body text,
                                                p_base bigint default 0, p_extra jsonb default '{}') returns jsonb
language sql volatile as $$
    select iamb_test.op('snippet', p_id, jsonb_build_object(
        'folder_id', p_folder, 'abbreviation', p_abbr, 'label', 'Label', 'kind', 'plain', 'body', p_body,
        'trigger', 'delimiter', 'case_mode', 'insensitive', 'word_start', true, 'enabled', true,
        'created_at', 1795190000000) || coalesce(p_extra -> 'row', '{}'), p_base, p_extra - 'row')
$$;

create or replace function iamb_test.op_tag(p_id uuid, p_profile uuid, p_name text, p_base bigint default 0) returns jsonb
language sql volatile as $$
    select iamb_test.op('tag', p_id, jsonb_build_object('profile_id', p_profile, 'name', p_name, 'color', null), p_base)
$$;

create or replace function iamb_test.op_snippet_tag(p_id uuid, p_snippet uuid, p_tag uuid, p_base bigint default 0) returns jsonb
language sql volatile as $$
    select iamb_test.op('snippet_tag', p_id, jsonb_build_object('snippet_id', p_snippet, 'tag_id', p_tag), p_base)
$$;

create or replace function iamb_test.op_attachment(p_id uuid, p_profile uuid, p_sha text, p_base bigint default 0,
                                                   p_extra jsonb default '{}') returns jsonb
language sql volatile as $$
    select iamb_test.op('attachment', p_id, jsonb_build_object('profile_id', p_profile, 'sha256', p_sha,
                        'mime', 'image/png', 'size', 10, 'file_name', 'a.png'), p_base, p_extra)
$$;

create or replace function iamb_test.op_snippet_attachment(p_id uuid, p_snippet uuid, p_att uuid, p_base bigint default 0) returns jsonb
language sql volatile as $$
    select iamb_test.op('snippet_attachment', p_id, jsonb_build_object('snippet_id', p_snippet, 'attachment_id', p_att), p_base)
$$;

create or replace function iamb_test.op_share(p_folder uuid, p_group uuid, p_perm text, p_base bigint default 0) returns jsonb
language sql volatile as $$
    select iamb_test.op('folder_share', iamb_private.share_id(p_folder, p_group),
                        jsonb_build_object('folder_id', p_folder, 'profile_id', p_group, 'permission', p_perm), p_base)
$$;

create or replace function iamb_test.op_delete(p_entity text, p_id uuid, p_base bigint default 0,
                                               p_extra jsonb default '{}') returns jsonb
language sql volatile as $$
    select jsonb_build_object('op_id', gen_random_uuid(), 'entity', p_entity, 'id', p_id, 'kind', 'delete',
                              'base_rev', p_base) || p_extra
$$;

-- The status of result `i` (1-based) of a push answer, with its reason.
create or replace function iamb_test.st(r jsonb, i integer default 1) returns text
language sql immutable as $$
    select coalesce(r -> 'results' -> (i - 1) ->> 'status', 'error:' || coalesce(r ->> 'error', '?'))
           || coalesce(':' || (r -> 'results' -> (i - 1) ->> 'reason'), '')
$$;

create or replace function iamb_test.rev(p_entity text, p_id uuid) returns bigint
language sql stable as $$ select (iamb_private.get_row(p_entity, p_id) ->> 'rev')::bigint $$;

-- ---------------------------------------------------------------------------------------------
-- Invariants checked after every step
-- ---------------------------------------------------------------------------------------------

-- G1: `folder_access` equals the rule of §4.2 computed by the slow reference.
create or replace function iamb_test.check_access() returns void
language plpgsql as $$
declare
    n integer;
    d text;
begin
    select count(*), string_agg(x::text, '; ') into n, d from (
        (select user_id, folder_id, permission, via, root_folder_id from iamb.folder_access
         except select * from iamb_private.access_reference())
        union all
        (select * from iamb_private.access_reference()
         except select user_id, folder_id, permission, via, root_folder_id from iamb.folder_access)
    ) x;
    if n > 0 then
        raise exception 'check failed: folder_access differs from the reference in % rows: %', n, left(d, 2000);
    end if;
end $$;

-- I7: no folder cycle, no tree deeper than four levels, no live row under a deleted folder.
create or replace function iamb_test.check_tree() returns void
language plpgsql as $$
begin
    perform iamb_test.ok(not exists (
        with recursive up as (
            select f.id as start, f.parent_id as cur, 1 as d from iamb.folder f
            union all
            select up.start, g.parent_id, up.d + 1 from up join iamb.folder g on g.id = up.cur where up.d < 10
        ) select 1 from up where up.cur = up.start or up.d > 4), 'no cycle and at most four levels');
    perform iamb_test.ok(not exists (
        select 1 from iamb.folder c join iamb.folder p on p.id = c.parent_id
        where c.deleted_at is null and p.deleted_at is not null), 'no live folder under a deleted folder');
    perform iamb_test.ok(not exists (
        select 1 from iamb.snippet s join iamb.folder f on f.id = s.folder_id
        where s.deleted_at is null and f.deleted_at is not null), 'no live snippet under a deleted folder');
    perform iamb_test.ok(not exists (
        select 1 from iamb.folder c join iamb.folder p on p.id = c.parent_id where c.profile_id <> p.profile_id),
        'a parent is in the same profile');
    perform iamb_test.ok(not exists (
        select 1 from iamb.snippet s join iamb.folder f on f.id = s.folder_id where s.profile_id <> f.profile_id),
        'a snippet is in its folder''s profile');
end $$;

-- The policies give the actor exactly the rows `iamb_private.readers` says they may read: the
-- functions that answer rows for the caller and the feed use the same rule as the policies.
create or replace function iamb_test.check_readers(p_name text) returns void
language plpgsql as $$
declare
    u   uuid := iamb_test.uid(p_name);
    ent text;
    exp uuid[];
    got uuid[];
    active boolean;
begin
    -- A session Iamb does not accept (no device row, revoked, ended, another way of signing in)
    -- reads nothing at all.
    active := exists (select 1 from iamb_test.actor a join iamb.device d on d.session_id = a.session_id and d.user_id = a.user_id
                      join auth.sessions s on s.id = a.session_id
                      where a.name = p_name and d.revoked_at is null
                        and exists (select 1 from iamb.server_state st where a.amr = any (st.sign_in_methods)));
    foreach ent in array array['profile', 'membership', 'folder', 'folder_share', 'snippet', 'tag',
                               'snippet_tag', 'attachment', 'snippet_attachment'] loop
        execute format('select coalesce(array_agg(id order by id), ''{}'') from iamb.%I t where %L and %L = any (iamb_private.readers(%L, t.id))',
                       ent, active, u, ent) into exp;
        perform iamb_test.act(p_name);
        execute format('select coalesce(array_agg(id order by id), ''{}'') from iamb.%I', ent) into got;
        perform iamb_test.act_owner();
        if exp is distinct from got then
            raise exception 'check failed: % may read % rows % by the policy but % by readers()', p_name, ent, got, exp;
        end if;
    end loop;
end $$;

-- Whether the live channel's message table takes a message now. On the platform the realtime
-- service creates its daily partitions when a client connects, and `realtime.send` does not; on a
-- project no client has connected to today, every poke is dropped (server/README.md).
create or replace function iamb_test.realtime_ready() returns boolean
language plpgsql as $$
begin
    begin
        insert into realtime.messages (topic, extension, payload, event, private)
        values ('iamb-probe', 'broadcast', '{}', 'probe', true);
        raise exception using errcode = 'IAMB0', message = 'probe';
    exception
        when check_violation then return false;
        when sqlstate 'IAMB0' then return true;
    end;
end $$;

-- Runs the daily job's steps until no rows are left (as Supabase Cron would, every ten minutes).
create or replace function iamb_test.daily() returns jsonb
language plpgsql as $$
declare
    r jsonb;
    i integer := 0;
begin
    loop
        r := iamb_private.daily_job_step();
        i := i + 1;
        exit when not (r ->> 'more')::boolean or i > 1000;
    end loop;
    return r || jsonb_build_object('steps', i);
end $$;

-- How many rows of a table the actor can read; -1 when the table is not readable at all.
create or replace function iamb_test.visible(p_name text, p_table text) returns integer
language plpgsql as $$
declare
    n integer;
begin
    perform iamb_test.act(p_name);
    begin
        execute format('select count(*) from iamb.%I', p_table) into n;
    exception when insufficient_privilege then
        n := -1;
    end;
    perform iamb_test.act_owner();
    return n;
end $$;

create or replace function iamb_test.check_all() returns void
language plpgsql as $$
declare
    a record;
begin
    perform iamb_test.check_access();
    perform iamb_test.check_tree();
    for a in select name from iamb_test.actor order by name loop
        perform iamb_test.check_readers(a.name);
    end loop;
end $$;

-- A family as in the scenarios of §12: Ines owns the group, Tom is a member, and Ines shares
-- `f1` (with snippet `s1`) with the family to change. Answers the ids.
create or replace function iamb_test.family(p_perm text default 'edit') returns jsonb
language plpgsql as $$
declare
    g   uuid := gen_random_uuid();
    f1  uuid := gen_random_uuid();
    f2  uuid := gen_random_uuid();
    s1  uuid := gen_random_uuid();
    s2  uuid := gen_random_uuid();
    inv uuid := gen_random_uuid();
    r   jsonb;
begin
    -- A new cast each time: earlier accounts stay in the database, under no name.
    delete from iamb_test.actor;
    perform iamb_test.new_user('ines');
    perform iamb_test.new_user('tom');
    perform iamb_test.new_user('stranger');
    perform iamb_test.hello('ines', 'Ines Marchetti');
    perform iamb_test.hello('tom', 'Tom');
    perform iamb_test.hello('stranger', 'Somebody');
    r := iamb_test.call('ines', format('api.create_group(iamb_test.dev(''ines''), 1, %L, ''family'', ''Family'')', g));
    perform iamb_test.ok(r ->> 'error' is null, 'create_group ' || r::text);
    r := iamb_test.call('ines', format('api.invite(iamb_test.dev(''ines''), 1, %L, %L, %L, ''member'')', inv, g, iamb_test.email('tom')));
    perform iamb_test.ok(r ->> 'error' is null, 'invite ' || r::text);
    r := iamb_test.call('tom', format('api.accept_invitation(iamb_test.dev(''tom''), 1, %L)', inv));
    perform iamb_test.ok(r ->> 'error' is null, 'accept ' || r::text);
    r := iamb_test.push('ines', jsonb_build_array(
        iamb_test.op_folder(f1, iamb_test.personal('ines'), null, 'Shop replies'),
        iamb_test.op_folder(f2, iamb_test.personal('ines'), null, 'Private'),
        iamb_test.op_snippet(s1, f1, ';hrs', 'Open 9 to 5.'),
        iamb_test.op_snippet(s2, f2, ';priv', 'Private text.'),
        iamb_test.op_share(f1, g, p_perm)));
    perform iamb_test.ok(r ->> 'error' is null and iamb_test.st(r, 5) = 'applied', 'family push ' || r::text);
    return jsonb_build_object('g', g, 'f1', f1, 'f2', f2, 's1', s1, 's2', s2,
                              'p_ines', iamb_test.personal('ines'), 'p_tom', iamb_test.personal('tom'));
end $$;

grant execute on all functions in schema iamb_test to authenticated;
grant usage on schema iamb_test to anon;
grant execute on function iamb_test.act_owner() to anon;
grant usage on schema iamb_test to service_role;
grant execute on function iamb_test.act_owner() to service_role;
