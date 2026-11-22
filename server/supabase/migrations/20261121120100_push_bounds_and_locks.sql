-- Iamb server, migration 20: what one push may cost, what it may lock, and where a restored folder
-- goes (docs/reviews/4.3-security-reverify.md, N2, N3 and N4).
--
-- N2. The answer to a cascade list was bounded, the work for it was not: the statement read the
--     whole wire form of every listed row before it decided which rows the answer would hold, so
--     a list that named a few large rows many times wrote temporary files without bound.
--     Now: a list that names a row twice is `blocked: invalid`; which rows come whole is decided
--     from the sizes of the stored columns alone, and only those rows are read; all the cascade
--     answers of one request share one allowance of 4 MiB of whole rows. A request that is cut
--     off all the same (its time-out, or the database out of temporary file space, disk or
--     memory) is caught: its writes are undone, it is answered `too_large` as data, and it stays
--     counted, as ten calls.
-- N3. (a) A profile is locked only for a caller who can write something in it. An op in a profile
--     the caller can only read is judged without the lock, by the same code, and can write
--     nothing: a revision is drawn only under a lock (`next_rev` refuses otherwise).
--     (b) The lock on a row's id has a 64-bit key made with a seed that only the server holds, so
--     nobody can make an id of their own that shares a lock with somebody else's row.
-- N4. A deleted folder can lie deeper than four levels after a folder above it was moved. When
--     it is restored (a put, a conflict's version, or a row put below it) it goes under the
--     nearest folder above that keeps four levels, as a new folder does (C8).

do $$
begin
    if current_user <> 'postgres' then
        raise exception 'iamb migrations must run as postgres, not %', current_user;
    end if;
end
$$;

-- The seed of the id locks. Nobody but the tables' owner reads `server_state`.
alter table iamb.server_state add column if not exists lock_seed bigint;
update iamb.server_state
set lock_seed = hashtextextended(gen_random_uuid()::text || gen_random_uuid()::text, 0)
where lock_seed is null;
alter table iamb.server_state alter column lock_seed set not null;

-- Whether the user can write anything in the profile: they manage it (the owner of a personal
-- profile, owner or admin of a group), or have `edit` or `manage` on one of its folders.
create or replace function iamb_private.writes_in(u uuid, p_profile uuid) returns boolean
language sql stable set search_path = pg_catalog, pg_temp as $$
    select iamb_private.manages_profile(u, p_profile)
        or exists (select 1 from iamb.folder_access fa
                   where fa.user_id = u and fa.profile_id = p_profile and fa.permission in ('edit', 'manage'))
$$;

-- The next revision, drawn under the locks of the given profiles; records it as their `head_rev`.
-- Every write of a synced row draws one first. While a push judges an op without its profile's
-- lock (N3), no revision can be drawn: the op is then answered as refused.
create or replace function iamb_private.next_rev(p_profiles uuid[]) returns bigint
language plpgsql volatile set search_path = pg_catalog, pg_temp as $$
declare
    r bigint;
begin
    if current_setting('iamb.unlocked_op', true) = 'on' then
        raise exception 'a write without its profile''s lock' using errcode = 'IB001';
    end if;
    r := nextval('iamb.rev_seq');
    update iamb.profile set head_rev = greatest(head_rev, r) where id = any (p_profiles);
    return r;
end
$$;

-- The answer for every row of a folder delete's cascade list (C6): the row's revision and whether
-- it is deleted, the whole row where the device does not hold that content, or `gone`. Which rows
-- come whole is decided from the sizes of the stored columns, for one entry per row, before any
-- content is read; the content of the others is never touched. Whole rows as long as the request's
-- allowance lasts (4 MiB for all the cascade answers of one request), the others in the short form
-- with `fetch` (§6.3), which the device reads with `rows`.
create or replace function iamb_private.cascade_answer(u uuid, p_list jsonb) returns jsonb
language plpgsql volatile set search_path = pg_catalog, pg_temp as $$
declare
    budget bigint := coalesce(nullif(current_setting('iamb.cascade_bytes_left', true), '')::bigint, 4000000);
    res    jsonb;
    used   bigint;
begin
    with items as materialized (
        select x.i, x.v ->> 'entity' as ent, x.v -> 'id' as idj, iamb_private.try_uuid(x.v ->> 'id') as rid,
               case when jsonb_typeof(x.v -> 'base_rev') = 'number' and length(x.v ->> 'base_rev') <= 20
                    then (x.v ->> 'base_rev')::numeric end as base
        from jsonb_array_elements(case when jsonb_typeof(p_list) = 'array' then p_list else '[]'::jsonb end)
             with ordinality as x(v, i)
    ),
    found as materialized (
        select it.i, it.ent, it.idj, it.rid, it.base,
               coalesce(f.rev, s.rev) as rev, coalesce(f.deleted_at, s.deleted_at) as del,
               (f.id is not null or s.id is not null) as readable,
               case when s.id is not null
                        then 700 + octet_length(s.body) + octet_length(s.label) + octet_length(s.abbreviation)
                    when f.id is not null then 600 + octet_length(f.name) end as sz
        from items it
        left join iamb.folder f on it.ent = 'folder' and f.id = it.rid
             and exists (select 1 from iamb.folder_access fa where fa.user_id = u and fa.folder_id = f.id)
        left join iamb.snippet s on it.ent = 'snippet' and s.id = it.rid
             and exists (select 1 from iamb.folder_access fa where fa.user_id = u and fa.folder_id = s.folder_id)
    ),
    -- The first entry of each row that is live and differs from the base the device named.
    cand as (
        select c.i, c.ent, c.rid, sum(c.sz) over (order by c.i) as est
        from (select fd.i, fd.ent, fd.rid, fd.sz,
                     row_number() over (partition by fd.ent, fd.rid order by fd.i) as nth
              from found fd
              where fd.readable and fd.del is null and fd.rev::numeric is distinct from fd.base) c
        where c.nth = 1
    ),
    -- Only now is content read, and only of rows whose stored sizes fit the allowance; the exact
    -- sizes then decide.
    whole as materialized (
        select c.i, w.row, w.origin, sum(octet_length(w.row::text) + 300) over (order by c.i) as bytes
        from cand c join iamb.wire_rows w on w.entity = c.ent and w.id = c.rid
        where c.est <= budget
    )
    select coalesce(jsonb_agg(
        case when not fd.readable then jsonb_build_object('entity', fd.ent, 'id', fd.idj, 'gone', true)
             when wh.row is not null and wh.bytes <= budget
                 then jsonb_build_object('entity', fd.ent, 'id', fd.idj, 'rev', fd.rev, 'deleted', false,
                                         'row', wh.row, 'origin', wh.origin)
             when fd.del is null and fd.rev::numeric is distinct from fd.base
                 then jsonb_build_object('entity', fd.ent, 'id', fd.idj, 'rev', fd.rev, 'deleted', false, 'fetch', true)
             else jsonb_build_object('entity', fd.ent, 'id', fd.idj, 'rev', fd.rev, 'deleted', fd.del is not null)
        end order by fd.i), '[]'::jsonb),
        coalesce(max(wh.bytes) filter (where wh.bytes <= budget), 0)
    into res, used
    from found fd left join whole wh on wh.i = fd.i;
    perform set_config('iamb.cascade_bytes_left', greatest(budget - used, 0)::text, true);
    return res;
end
$$;

-- Whether an op can be judged at all (review finding 7): null, or the reason it is blocked. A
-- cascade list that names a row twice is `invalid`.
create or replace function iamb_private.op_shape(op jsonb) returns text
language plpgsql immutable set search_path = pg_catalog, pg_temp as $$
declare
    k text;
    v jsonb;
begin
    if op is null or jsonb_typeof(op) <> 'object' then
        return 'invalid';
    end if;
    -- Revisions: whole numbers from 0 that a double holds exactly.
    foreach k in array array['base_rev', 'repush_rev'] loop
        v := op -> k;
        if v is not null and jsonb_typeof(v) <> 'null' then
            if jsonb_typeof(v) <> 'number' or length(v #>> '{}') > 20 then
                return 'invalid';
            elsif (v #>> '{}')::numeric < 0 or (v #>> '{}')::numeric > 9007199254740991
                  or (v #>> '{}')::numeric <> trunc((v #>> '{}')::numeric) then
                return 'invalid';
            end if;
        end if;
    end loop;
    v := op -> 'follows';
    if v is not null and jsonb_typeof(v) <> 'null' and (jsonb_typeof(v) <> 'array' or jsonb_array_length(v) > 1000) then
        return 'invalid';
    end if;
    v := op -> 'cascade';
    if v is not null and jsonb_typeof(v) <> 'null' then
        if jsonb_typeof(v) <> 'array' then
            return 'invalid';
        elsif jsonb_array_length(v) > iamb_private.cascade_list_max() then
            return 'too_large';
        elsif (select count(*) <> count(distinct (x ->> 'entity', x ->> 'id')) from jsonb_array_elements(v) x) then
            -- A device lists each row it deleted once (N2).
            return 'invalid';
        end if;
    end if;
    -- Numbers of the row that go into bigint columns.
    if jsonb_typeof(op -> 'row') = 'object' then
        foreach k in array array['created_at', 'position', 'size'] loop
            v := op -> 'row' -> k;
            if jsonb_typeof(v) = 'number'
               and (length(v #>> '{}') > 20 or abs((v #>> '{}')::numeric) > 9007199254740991) then
                return 'invalid';
            end if;
        end loop;
    end if;
    return null;
end
$$;

-- C7: restores every deleted folder from the given one upwards, if the writer may; false, with
-- nothing written, when not. A restored folder that would lie on a fifth level is put under the
-- nearest folder above that keeps four levels (C8).
create or replace function iamb_private.restore_chain(u uuid, dev uuid, op uuid, p_folder uuid) returns boolean
language plpgsql volatile set search_path = pg_catalog, pg_temp as $$
declare
    chain  uuid[] := '{}';
    a      uuid := p_folder;
    f      iamb.folder;
    cur    jsonb;
    nr     jsonb;
    r      bigint;
    before uuid[];
    now_ms bigint := iamb_private.now_ms();
    i      integer;
    n      integer;
    lvl    integer;
    a3     uuid;
    newp   uuid[];
begin
    while a is not null loop
        select * into f from iamb.folder where id = a;
        exit when f.id is null or a = any (chain);
        if f.deleted_at is not null then
            if f.parent_id is null then
                if not iamb_private.manages_profile(u, f.profile_id) then return false; end if;
            elsif not iamb_private.can_edit(u, f.parent_id) then
                return false;
            end if;
            chain := chain || a;
        end if;
        a := f.parent_id;
    end loop;
    -- C8 (N4): a deleted folder can lie deeper than four levels, after a folder above it was moved.
    -- Restored, it goes under the folder on the third level of its path, if the writer may put it
    -- there. Decided for the whole chain before anything is written.
    n := coalesce(array_length(chain, 1), 0);
    if n > 0 then
        newp := array_fill(null::uuid, array[n]);
        select * into f from iamb.folder where id = chain[n];
        lvl := coalesce(iamb_private.folder_depth(f.parent_id), 0);
        if lvl >= 3 then
            a3 := f.parent_id;
            for i in 1..(lvl - 3) loop
                select g.parent_id into a3 from iamb.folder g where g.id = a3;
            end loop;
        end if;
        for i in reverse n..1 loop
            lvl := lvl + 1;
            if lvl > 4 then
                if not iamb_private.can_edit(u, a3) then
                    return false;
                end if;
                newp[i] := a3;
                lvl := 4;
            elsif lvl = 3 then
                a3 := chain[i];
            end if;
        end loop;
    end if;
    -- Top-down, so that no live folder is ever under a deleted one.
    for i in reverse n..1 loop
        cur := iamb_private.get_row('folder', chain[i]);
        before := iamb_private.readers('folder', chain[i]);
        r := iamb_private.next_rev(array[(cur ->> 'profile_id')::uuid]);
        nr := cur || jsonb_build_object('deleted_at', null, 'rev', r, 'updated_at', now_ms, 'updated_by', u,
                                        'updated_device', dev, 'updated_op', op)
                  || case when newp[i] is not null then jsonb_build_object('parent_id', newp[i]) else '{}'::jsonb end;
        perform iamb_private.write_row(u, dev, op, 'folder', cur, nr, r, before, true, (cur ->> 'profile_id')::uuid);
        perform iamb_private.record_conflict('folder', chain[i], (cur ->> 'profile_id')::uuid, 'folder_restored',
                                             r, u, dev, cur);
    end loop;
    return true;
end
$$;

-- One op of a push, or the restore of a conflict's version (C1 to C21); see migration 12. Since
-- then: a deleted folder that comes back is held to four levels (N4).
create or replace function iamb_private.push_op(u uuid, dev uuid, op jsonb, p_profile uuid, p_budget integer,
                                                p_blocked uuid[] default '{}', p_origin jsonb default null)
returns jsonb
language plpgsql volatile set search_path = pg_catalog, pg_temp as $$
declare
    ent      text := op ->> 'entity';
    rid      uuid := iamb_private.try_uuid(op ->> 'id');
    v_kind   text := op ->> 'kind';
    opid     uuid := iamb_private.try_uuid(op ->> 'op_id');
    base     bigint := coalesce((op ->> 'base_rev')::numeric::bigint, 0);
    follows  uuid[] := array(select iamb_private.try_uuid(x) from jsonb_array_elements_text(coalesce(op -> 'follows', '[]')) x);
    repush   text := op ->> 'repush';
    rp_rev   bigint := (op ->> 'repush_rev')::numeric::bigint;
    rp_ep    uuid := iamb_private.try_uuid(op ->> 'repush_epoch');
    self     boolean := coalesce(op ->> 'author', '') = 'self';
    cur      jsonb := iamb_private.get_row(ent, rid);
    n        jsonb;
    nr       jsonb;
    p        uuid := p_profile;
    st       iamb.server_state;
    now_ms   bigint := iamb_private.now_ms();
    r        bigint;
    before   uuid[];
    cid      uuid;
    adjusted text[] := '{}';
    stale    boolean;
    own      boolean;
    bv       jsonb;
    changed  boolean;
    ckind    text;
    honoured boolean := false;
    ep       iamb.epoch;
    backup   bigint;
    target   uuid;
    parent   iamb.folder;
    tf       iamb.folder;
    cf       iamb.folder;
    d        integer;
    lim      bigint;
    cnt      bigint;
    lockp    uuid[];
    structural boolean := false;
    sn       iamb.snippet;
    other    jsonb;
    blob     text;
    del      boolean := (op ->> 'kind') = 'delete';
    res      jsonb;
begin
    select * into st from iamb.server_state;
    if ent in ('profile', 'membership') then
        return iamb_private.op_result(u, op, 'rejected', 'not_pushable', null, null, false);
    end if;
    if rid is null or opid is null or v_kind not in ('put', 'delete') or iamb_private.fields(ent) is null then
        return iamb_private.op_result(u, op, 'blocked', 'invalid', null, null, false);
    end if;
    if p is null then
        -- No profile, or one the caller has no grant in (it was not locked): the answer of an
        -- unknown row, whatever the server holds under that id.
        if del then
            res := iamb_private.op_result(u, op, 'unchanged', null, null, null, false);
            if ent = 'folder' then
                res := res || jsonb_build_object('cascade', iamb_private.cascade_answer(u, op -> 'cascade'));
            end if;
            return res;
        end if;
        return iamb_private.op_result(u, op, 'rejected', 'parent_gone', null, null, false);
    end if;
    lockp := array[p];
    if not exists (select 1 from iamb.profile where id = p) then
        return iamb_private.op_result(u, op, 'rejected', 'parent_gone', null, null, false);
    end if;

    -- A re-push after a restore (C18) is honoured only for a revision in the span the restore
    -- lost; outside it the op is an ordinary write based on that revision.
    if repush in ('row', 'base') and rp_rev is not null then
        select e2.* into ep from iamb.epoch e1 join iamb.epoch e2 on e2.started_at > e1.started_at
        where e1.id = rp_ep order by e2.started_at limit 1;
        if ep.id is null and not exists (select 1 from iamb.epoch where id = rp_ep) then
            select e.* into ep from iamb.epoch e join iamb.server_state s on s.epoch = e.id;
        end if;
        if ep.id is not null then
            select backup_rev into backup from iamb.epoch_profile where epoch_id = ep.id and profile_id = p;
            backup := coalesce(backup, 0);
            honoured := rp_rev > backup and rp_rev < ep.first_rev;
        end if;
        if not honoured then
            base := rp_rev;
        end if;
    end if;

    -- ----------------------------------------------------------------------------- deletes
    if del then
        if ent = 'folder' then
            return iamb_private.push_folder_delete(u, dev, op, p_budget);
        end if;
        if cur is null or not iamb_private.can_read(u, ent, rid) then
            return iamb_private.op_result(u, op, 'unchanged', null, null, null, false);
        end if;
        -- Permission (§4.3).
        if (ent = 'snippet' and not iamb_private.can_edit(u, (cur ->> 'folder_id')::uuid))
           or (ent in ('snippet_tag', 'snippet_attachment') and not iamb_private.can_edit(u,
                   (select s.folder_id from iamb.snippet s where s.id = (cur ->> 'snippet_id')::uuid)))
           or (ent in ('tag', 'attachment') and not iamb_private.manages_profile(u, p))
           or (ent = 'folder_share' and not iamb_private.can_manage(u, (cur ->> 'folder_id')::uuid)) then
            return iamb_private.op_result(u, op, 'rejected', 'forbidden', null, null, false);
        end if;
        if (cur ->> 'deleted_at') is not null then
            return iamb_private.op_result(u, op, 'unchanged', null, null, null, false);
        end if;
        if ent = 'folder_share' then
            -- A share's delete always applies (C11); it is logged for the restore script.
            r := iamb_private.end_share(u, dev, opid, rid, lockp, 'push');
            perform iamb_private.refresh_access(lockp);
            return iamb_private.op_result(u, op, 'applied', null, null, null, false);
        end if;
        before := iamb_private.readers(ent, rid);
        r := iamb_private.next_rev(lockp);
        nr := cur || jsonb_build_object('deleted_at', now_ms, 'rev', r, 'updated_at', now_ms, 'updated_by', u,
                                        'updated_device', dev, 'updated_op', opid,
                                        'lost_rev', case when honoured then greatest(coalesce((cur ->> 'lost_rev')::bigint, 0), rp_rev)
                                                         else (cur ->> 'lost_rev')::bigint end);
        perform iamb_private.write_row(u, dev, opid, ent, cur, nr, r, before, false, p);
        if ent = 'snippet' then
            perform iamb_private.touch_attachments(array(select attachment_id from iamb.snippet_attachment where snippet_id = rid));
        elsif ent = 'snippet_attachment' then
            perform iamb_private.touch_attachments(array[(cur ->> 'attachment_id')::uuid]);
        end if;
        -- A stale delete records `delete_over_edit` (C3, C5).
        if ent in ('snippet', 'tag') and base < (cur ->> 'rev')::bigint
           and not (iamb_private.try_uuid(cur ->> 'updated_device') = dev
                    and iamb_private.try_uuid(cur ->> 'updated_op') = any (follows)) then
            bv := iamb_private.base_version(ent, cur, base, follows, u, dev);
            if bv is null or iamb_private.pick(bv, iamb_private.recorded_fields(ent))
                             <> iamb_private.pick(cur, iamb_private.recorded_fields(ent))
               or (bv ->> 'deleted_at' is null) <> (cur ->> 'deleted_at' is null) then
                cid := iamb_private.record_conflict(ent, rid, p, 'delete_over_edit', r, u, dev, cur);
            end if;
        end if;
        return iamb_private.op_result(u, op, 'applied', null, cid, null, false);
    end if;

    -- ----------------------------------------------------------------------------- puts
    n := iamb_private.normalize(ent, op -> 'row');
    if n ? '_error' then
        return iamb_private.op_result(u, op, 'blocked', n ->> '_error', null, null, false);
    end if;
    -- A row whose parent was blocked in the same request (C14).
    if exists (select 1 from unnest(p_blocked) as b(id)
               where b.id in (iamb_private.try_uuid(n ->> 'parent_id'), iamb_private.try_uuid(n ->> 'folder_id'),
                              iamb_private.try_uuid(n ->> 'snippet_id'), iamb_private.try_uuid(n ->> 'tag_id'),
                              iamb_private.try_uuid(n ->> 'attachment_id'))) then
        return iamb_private.op_result(u, op, 'blocked', 'parent_blocked', null, null, false);
    end if;

    -- A row whose profile would change is blocked (C8); its other rows must be known and readable
    -- (C13). Then permission (§4.3, C12) against the state found under the locks.
    case ent
    when 'folder' then
        if (n ->> 'profile_id')::uuid <> p then
            return iamb_private.op_result(u, op, 'blocked', 'invalid', null, null, false);
        end if;
        target := iamb_private.try_uuid(n ->> 'parent_id');
        if target is not null then
            select * into parent from iamb.folder where id = target;
            if parent.id is null or iamb_private.perm(u, target) is null then
                return iamb_private.op_result(u, op, 'rejected', 'parent_gone', null, null, false);
            elsif parent.profile_id <> p then
                return iamb_private.op_result(u, op, 'blocked', 'invalid', null, null, false);
            end if;
        end if;
        if cur is null then
            if (target is null and not iamb_private.manages_profile(u, p))
               or (target is not null and not iamb_private.can_edit(u, target)) then
                return iamb_private.op_result(u, op, 'rejected', 'forbidden', null, null, false);
            end if;
            -- C8: a new folder that would be a fifth level goes under the nearest folder above.
            if target is not null then
                d := iamb_private.folder_depth(target);
                while d >= 4 loop
                    select f.* into parent from iamb.folder f where f.id = parent.parent_id;
                    target := parent.id;
                    d := d - 1;
                end loop;
                if target is distinct from iamb_private.try_uuid(n ->> 'parent_id') then
                    adjusted := array['parent_id'];
                    n := n || jsonb_build_object('parent_id', target);
                    if target is not null and not iamb_private.can_edit(u, target) then
                        return iamb_private.op_result(u, op, 'rejected', 'forbidden', null, null, false);
                    end if;
                end if;
            end if;
        else
            if (cur ->> 'parent_id' is null) or iamb_private.is_share_root(rid) or target is null
               or (n -> 'member_permission') is distinct from (cur -> 'member_permission') then
                -- A top-level folder, the root of a live share, a move to the top, or the members'
                -- permission: `manage` for every field.
                if not iamb_private.can_manage(u, rid)
                   or (target is null and not iamb_private.manages_profile(u, p))
                   or (target is not null and target is distinct from iamb_private.try_uuid(cur ->> 'parent_id')
                       and not iamb_private.can_edit(u, target)) then
                    return iamb_private.op_result(u, op, 'rejected', 'forbidden', null, null, false);
                end if;
            else
                if not iamb_private.can_edit(u, rid)
                   or (target is distinct from iamb_private.try_uuid(cur ->> 'parent_id')
                       and (not iamb_private.can_edit(u, target)
                            or (iamb_private.access_root(u, target) is distinct from iamb_private.access_root(u, rid)
                                and not iamb_private.can_manage(u, rid)))) then
                    return iamb_private.op_result(u, op, 'rejected', 'forbidden', null, null, false);
                end if;
            end if;
            -- C8: a move that would make a cycle or a fifth level keeps the current parent.
            if target is not null and target is distinct from iamb_private.try_uuid(cur ->> 'parent_id') then
                if iamb_private.is_in_subtree(target, rid)
                   or iamb_private.folder_depth(target) + iamb_private.subtree_height(rid) > 4 then
                    adjusted := array['parent_id'];
                    n := n || jsonb_build_object('parent_id', cur -> 'parent_id');
                    target := iamb_private.try_uuid(cur ->> 'parent_id');
                end if;
            end if;
            -- C8 for a folder that comes back (N4): a deleted folder can lie deeper than four
            -- levels, after a folder above it was moved; nothing live is below it. Restored, it
            -- goes under the nearest folder above that keeps four levels, as a new folder does.
            if (cur ->> 'deleted_at') is not null and target is not null then
                d := iamb_private.folder_depth(target);
                if d >= 4 then
                    select f.* into parent from iamb.folder f where f.id = target;
                    while d >= 4 loop
                        select f.* into parent from iamb.folder f where f.id = parent.parent_id;
                        target := parent.id;
                        d := d - 1;
                    end loop;
                    adjusted := array['parent_id'];
                    n := n || jsonb_build_object('parent_id', target);
                    if not iamb_private.can_edit(u, target) then
                        return iamb_private.op_result(u, op, 'rejected', 'forbidden', null, null, false);
                    end if;
                end if;
            end if;
        end if;
        structural := cur is null or (n -> 'parent_id') is distinct from (cur -> 'parent_id')
                      or (n -> 'member_permission') is distinct from (cur -> 'member_permission')
                      or (cur ->> 'deleted_at') is not null;
        -- C7: a live folder needs live folders above it.
        if target is not null and exists (select 1 from iamb.folder where id = target and deleted_at is not null) then
            if not iamb_private.restore_chain(u, dev, opid, target) then
                return iamb_private.op_result(u, op, 'rejected', 'parent_gone', null, null, false);
            end if;
        end if;
        if cur is null then
            lim := iamb_private.profile_limit(p, 'folders_max');
            select count(*) into cnt from iamb.folder where profile_id = p and deleted_at is null;
            if lim is not null and cnt >= lim then
                return iamb_private.op_result(u, op, 'blocked', 'plan_limit', null, null, false);
            end if;
        end if;
    when 'snippet' then
        target := (n ->> 'folder_id')::uuid;
        select * into tf from iamb.folder where id = target;
        if tf.id is null or iamb_private.perm(u, target) is null then
            return iamb_private.op_result(u, op, 'rejected', 'parent_gone', null, null, false);
        elsif tf.profile_id <> p then
            return iamb_private.op_result(u, op, 'blocked', 'invalid', null, null, false);
        end if;
        if cur is null then
            if not iamb_private.can_edit(u, target) then
                return iamb_private.op_result(u, op, 'rejected', 'forbidden', null, null, false);
            end if;
        else
            if not iamb_private.can_edit(u, (cur ->> 'folder_id')::uuid) or not iamb_private.can_edit(u, target)
               or (target <> (cur ->> 'folder_id')::uuid
                   and iamb_private.access_root(u, target) is distinct from iamb_private.access_root(u, (cur ->> 'folder_id')::uuid)
                   and not (iamb_private.can_manage(u, target) and iamb_private.can_manage(u, (cur ->> 'folder_id')::uuid))) then
                return iamb_private.op_result(u, op, 'rejected', 'forbidden', null, null, false);
            end if;
        end if;
        if tf.deleted_at is not null then
            if not iamb_private.restore_chain(u, dev, opid, target) then
                return iamb_private.op_result(u, op, 'rejected', 'parent_gone', null, null, false);
            end if;
        end if;
        if cur is null then
            lim := iamb_private.profile_limit(p, 'snippets_max');
            select count(*) into cnt from iamb.snippet where profile_id = p and deleted_at is null;
            if lim is not null and cnt >= lim then
                return iamb_private.op_result(u, op, 'blocked', 'plan_limit', null, null, false);
            end if;
        end if;
    when 'tag', 'attachment' then
        if (n ->> 'profile_id')::uuid <> p then
            return iamb_private.op_result(u, op, 'blocked', 'invalid', null, null, false);
        end if;
        if not exists (select 1 from iamb.profile where id = p and deleted_at is null) then
            return iamb_private.op_result(u, op, 'rejected', 'parent_gone', null, null, false);
        end if;
        if ent = 'attachment' and cur is not null
           and (iamb_private.pick(n, iamb_private.fields(ent)) <> iamb_private.pick(cur, iamb_private.fields(ent))
                or (cur ->> 'deleted_at') is null) then
            -- An attachment row never changes after its creation (C19).
            res := iamb_private.op_result(u, op, 'unchanged', null, null, null, false);
            if not iamb_private.can_read_blob(u, p, cur ->> 'sha256') then
                res := res || jsonb_build_object('blob', 'missing');
            end if;
            return res;
        end if;
        -- Create: `edit` on a folder of the profile; change, delete or restore: `manage` (§4.3).
        if (cur is null and not iamb_private.edits_in_profile(u, p))
           or (cur is not null and not iamb_private.manages_profile(u, p)) then
            return iamb_private.op_result(u, op, 'rejected', 'forbidden', null, null, false);
        end if;
    when 'snippet_tag', 'snippet_attachment' then
        select * into sn from iamb.snippet where id = (n ->> 'snippet_id')::uuid;
        other := case ent when 'snippet_tag' then iamb_private.get_row('tag', (n ->> 'tag_id')::uuid)
                          else iamb_private.get_row('attachment', (n ->> 'attachment_id')::uuid) end;
        if sn.id is null or other is null or not iamb_private.can_read(u, 'snippet', sn.id)
           or not iamb_private.can_read(u, case ent when 'snippet_tag' then 'tag' else 'attachment' end,
                                        (other ->> 'id')::uuid) then
            return iamb_private.op_result(u, op, 'rejected', 'parent_gone', null, null, false);
        end if;
        if (other ->> 'profile_id')::uuid <> sn.profile_id or sn.profile_id <> p
           or (cur is not null and iamb_private.pick(n, iamb_private.fields(ent)) <> iamb_private.pick(cur, iamb_private.fields(ent))) then
            return iamb_private.op_result(u, op, 'blocked', 'invalid', null, null, false);
        end if;
        if not iamb_private.can_edit(u, sn.folder_id) then
            return iamb_private.op_result(u, op, 'rejected', 'forbidden', null, null, false);
        end if;
    when 'folder_share' then
        -- C11: one row per folder and group; `manage` on a personal folder, a live membership in
        -- the group and, in a team, the role of owner or admin; a stale put is not applied.
        if rid <> iamb_private.share_id((n ->> 'folder_id')::uuid, (n ->> 'profile_id')::uuid) then
            return iamb_private.op_result(u, op, 'blocked', 'invalid', null, null, false);
        end if;
        select * into cf from iamb.folder where id = (n ->> 'folder_id')::uuid;
        if cf.id is null or iamb_private.perm(u, cf.id) is null
           or not exists (select 1 from iamb.profile g where g.id = (n ->> 'profile_id')::uuid
                          and g.kind <> 'personal' and g.deleted_at is null)
           or iamb_private.group_role(u, (n ->> 'profile_id')::uuid) is null then
            return iamb_private.op_result(u, op, 'rejected', 'parent_gone', null, null, false);
        end if;
        if not iamb_private.can_manage(u, cf.id)
           or not exists (select 1 from iamb.profile pp where pp.id = cf.profile_id and pp.kind = 'personal')
           or ((select kind from iamb.profile where id = (n ->> 'profile_id')::uuid) = 'company'
               and iamb_private.group_role(u, (n ->> 'profile_id')::uuid) not in ('owner', 'admin')) then
            return iamb_private.op_result(u, op, 'rejected', 'forbidden', null, null, false);
        end if;
        if cur is null and base > 0 and not honoured then
            return iamb_private.op_result(u, op, 'rejected', 'gone', null, null, false);
        end if;
        if cur is not null and base < (cur ->> 'rev')::bigint and not honoured then
            return iamb_private.op_result(u, op, 'superseded', null, null, null, false);
        end if;
        if not exists (select 1 from iamb.folder_share s where s.folder_id = cf.id and s.deleted_at is null) then
            lim := iamb_private.user_limit(u, 'shared_folders_max');
            select count(distinct s.folder_id) into cnt from iamb.folder_share s join iamb.folder f on f.id = s.folder_id
            where f.profile_id = cf.profile_id and s.deleted_at is null;
            if lim is not null and cnt >= lim then
                return iamb_private.op_result(u, op, 'rejected', 'plan_limit', null, null, false);
            end if;
        end if;
        lockp := array[(n ->> 'profile_id')::uuid, p];
        structural := true;
    end case;

    -- C13: a put with a base above 0 for a row the server does not know is gone, unless it gives
    -- back a revision a restore lost (C18 case 4, within `retention_days` of the epoch).
    if cur is null and base > 0 and not honoured then
        return iamb_private.op_result(u, op, 'rejected', 'gone', null, null, false);
    end if;
    if cur is null and honoured and ep.started_at < now_ms - st.retention_days * 86400000::bigint then
        return iamb_private.op_result(u, op, 'rejected', 'gone', null, null, false);
    end if;

    -- C4: nothing new, nothing written.
    if cur is not null and (cur ->> 'deleted_at') is null
       and iamb_private.pick(n, iamb_private.fields(ent)) = iamb_private.pick(cur, iamb_private.fields(ent)) then
        if honoured and coalesce((cur ->> 'lost_rev')::bigint, 0) < rp_rev and (cur ->> 'rev')::bigint <= backup then
            update iamb.folder set lost_rev = rp_rev where ent = 'folder' and id = rid;
            update iamb.snippet set lost_rev = rp_rev where ent = 'snippet' and id = rid;
            update iamb.tag set lost_rev = rp_rev where ent = 'tag' and id = rid;
        end if;
        res := iamb_private.op_result(u, op, 'unchanged', null, null, adjusted, false);
        if ent = 'attachment' and not iamb_private.can_read_blob(u, p, cur ->> 'sha256') then
            res := res || jsonb_build_object('blob', 'missing');
        end if;
        return res;
    end if;

    -- C18 case 3: the row has taken in a lost revision at or above this one.
    if honoured and cur is not null and coalesce((cur ->> 'lost_rev')::bigint, 0) >= rp_rev then
        nr := cur || n || jsonb_build_object('rev', rp_rev, 'updated_by', case when self then u end,
                                             'updated_device', case when self then dev end, 'updated_op', opid,
                                             'deleted_at', null);
        perform iamb_private.keep_history(ent, nr, (cur ->> 'rev')::bigint);
        if (cur ->> 'lost_rev')::bigint > rp_rev
           and iamb_private.pick(n, iamb_private.recorded_fields(ent)) <> iamb_private.pick(cur, iamb_private.recorded_fields(ent))
           and ent in ('folder', 'snippet', 'tag') then
            cid := iamb_private.record_conflict(ent, rid, p, 'edit_edit', (cur ->> 'rev')::bigint,
                                                iamb_private.try_uuid(cur ->> 'updated_by'),
                                                iamb_private.try_uuid(cur ->> 'updated_device'), nr);
        end if;
        return iamb_private.op_result(u, op, 'superseded', null, cid, null, false);
    end if;

    -- The write.
    before := case when cur is null then '{}'::uuid[] else iamb_private.readers(ent, rid) end;
    r := iamb_private.next_rev(lockp);
    if cur is null then
        nr := jsonb_build_object('id', rid, 'created_at', least(coalesce((n ->> 'created_at')::bigint, now_ms), now_ms),
                                 'created_rev', r);
        if ent = 'snippet' then
            nr := nr || jsonb_build_object('profile_id', p);
        end if;
    else
        nr := cur;
    end if;
    nr := nr || (n - 'created_at') || jsonb_build_object(
        'rev', r, 'updated_at', now_ms, 'updated_by', u, 'updated_device', dev, 'updated_op', opid,
        'deleted_at', null,
        'lost_rev', case when honoured then greatest(coalesce((cur ->> 'lost_rev')::bigint, 0), rp_rev)
                         else (cur ->> 'lost_rev')::bigint end);

    -- C17: who wrote the snippet's content.
    if ent = 'snippet' then
        if p_origin is not null then
            nr := nr || jsonb_build_object('content_by', p_origin -> 'content_by', 'content_at', p_origin -> 'content_at');
        elsif cur is not null and iamb_private.pick(n, iamb_private.content_fields()) = iamb_private.pick(cur, iamb_private.content_fields()) then
            nr := nr || jsonb_build_object('content_by', cur -> 'content_by', 'content_at', cur -> 'content_at');
        elsif honoured then
            nr := nr || jsonb_build_object('content_by', case when self then u end, 'content_at', now_ms);
        elsif cur is null then
            nr := nr || jsonb_build_object('content_by', u, 'content_at', now_ms);
        else
            bv := iamb_private.base_version(ent, cur, base, follows, u, dev);
            if bv is null then
                nr := nr || jsonb_build_object('content_by', null, 'content_at', now_ms);
            elsif iamb_private.pick(n, iamb_private.content_fields()) = iamb_private.pick(bv, iamb_private.content_fields()) then
                nr := nr || jsonb_build_object('content_by', bv -> 'content_by', 'content_at', bv -> 'content_at');
            else
                nr := nr || jsonb_build_object('content_by', u, 'content_at', now_ms);
            end if;
        end if;
    end if;
    -- C19: an attachment row is backed when its creator can read the verified file; a re-pushed
    -- row without `author: "self"` has no creator and is not backed.
    if ent = 'attachment' then
        if cur is null then
            if honoured and not self then
                nr := nr || jsonb_build_object('updated_by', null, 'updated_device', null, 'backed', false,
                                               'unlinked_at', now_ms);
            else
                nr := nr || jsonb_build_object('backed', iamb_private.can_read_blob(u, p, n ->> 'sha256'),
                                               'unlinked_at', now_ms);
            end if;
        end if;
    end if;
    perform iamb_private.write_row(u, dev, opid, ent, cur, nr, r, before, structural, p);
    if ent = 'folder_share' then
        -- Logged so that the restore script can tell a share that was given back (C18).
        perform iamb_private.log('share_created', jsonb_build_object(
            'user', u, 'device', dev, 'share', rid, 'folder', n ->> 'folder_id', 'group', n ->> 'profile_id', 'rev', r));
    end if;
    if ent = 'snippet_attachment' then
        perform iamb_private.touch_attachments(array[(n ->> 'attachment_id')::uuid]);
    elsif ent = 'snippet' and cur is not null and (cur ->> 'deleted_at') is not null then
        perform iamb_private.touch_attachments(array(select attachment_id from iamb.snippet_attachment where snippet_id = rid));
    end if;

    -- C3: a stale write still wins, on the record. C18 cases 1 and 2 decide the same way, except
    -- that a re-push over a row not written since the restore records nothing.
    if cur is not null and ent in ('folder', 'snippet', 'tag') then
        stale := base < (cur ->> 'rev')::bigint;
        own := iamb_private.try_uuid(cur ->> 'updated_device') = dev
               and iamb_private.try_uuid(cur ->> 'updated_op') = any (follows);
        if honoured then
            stale := (cur ->> 'rev')::bigint > backup;
            changed := iamb_private.pick(n, iamb_private.recorded_fields(ent)) <> iamb_private.pick(cur, iamb_private.recorded_fields(ent))
                       or (cur ->> 'deleted_at') is not null;
        elsif stale then
            bv := iamb_private.base_version(ent, cur, base, follows, u, dev);
            if bv is null then
                changed := iamb_private.pick(n, iamb_private.recorded_fields(ent)) <> iamb_private.pick(cur, iamb_private.recorded_fields(ent))
                           or (cur ->> 'deleted_at') is not null;
            else
                changed := iamb_private.pick(bv, iamb_private.recorded_fields(ent)) <> iamb_private.pick(cur, iamb_private.recorded_fields(ent))
                           or (bv ->> 'deleted_at' is null) <> (cur ->> 'deleted_at' is null);
            end if;
        end if;
        if stale and not own and changed then
            ckind := case when (cur ->> 'deleted_at') is not null then 'edit_over_delete' else 'edit_edit' end;
            cid := iamb_private.record_conflict(ent, rid, p, ckind, r, u, dev, cur);
        end if;
    end if;

    res := iamb_private.op_result(u, op, 'applied', null, cid, adjusted, false);
    if ent = 'attachment' and not iamb_private.can_read_blob(u, p, n ->> 'sha256') then
        res := res || jsonb_build_object('blob', 'missing');
    end if;
    return res;
end
$$;

-- The request (§6.3); see migration 12. Since then: the locks of N3 and the cascade allowance
-- of N2.
create or replace function iamb_private.push_core(u uuid, dev uuid, p_position text, ops jsonb) returns jsonb
language plpgsql volatile set search_path = pg_catalog, pg_temp as $$
declare
    st       iamb.server_state;
    pc       jsonb;
    nops     integer;
    wait     bigint;
    budget   integer;
    i        integer;
    o        jsonb;
    res      jsonb;
    rc       iamb.push_receipt;
    results  jsonb[];
    v_ops    jsonb[];
    v_err    text[];
    v_prof   uuid[];
    v_grp    uuid[];
    v_key    integer[];
    granted  uuid[];
    writable uuid[];
    v_ro     boolean[];
    blocked  uuid[] := '{}';
    lk       record;
    rp       uuid;
begin
    select * into st from iamb.server_state;
    -- The whole rows that the cascade answers of this request may hold between them (N2).
    perform set_config('iamb.cascade_bytes_left', '4000000', true);
    if ops is null or jsonb_typeof(ops) <> 'array' then
        ops := '[]';
    end if;
    nops := jsonb_array_length(ops);
    if nops > st.push_ops_max or octet_length(ops::text) > st.push_bytes_max then
        return iamb_private.err('too_large');
    end if;
    -- C25: a push whose position cannot be continued writes nothing.
    pc := iamb_private.position(dev, p_position, 'push');
    if pc ->> 'error' is not null then
        return jsonb_build_object('error', pc ->> 'error', 'reason', pc -> 'reason');
    end if;
    wait := iamb_private.rate_take(dev, 'push_ops', 60000, coalesce((st.rate_limits ->> 'push_ops')::integer, 6000), nops);
    if wait is not null then
        return jsonb_build_object('error', 'rate_limited', 'retry_after_ms', wait);
    end if;

    -- Each op, in the order sent: its shape (a key whose value is JSON null counts as absent),
    -- then, for an op that can be judged, its owning profile and, for a share, its group.
    select coalesce(array_agg(y.op order by y.pos), '{}'), coalesce(array_agg(y.err order by y.pos), '{}')
    into v_ops, v_err
    from (
        select x.pos,
               case when jsonb_typeof(x.v) = 'object'
                    then x.v - array(select e.key from jsonb_each(x.v) e where jsonb_typeof(e.value) = 'null')
                    else x.v end as op,
               iamb_private.op_shape(x.v) as err
        from jsonb_array_elements(ops) with ordinality as x(v, pos)
    ) y;
    ops := to_jsonb(v_ops);
    v_prof := array_fill(null::uuid, array[greatest(nops, 1)]);
    v_grp := array_fill(null::uuid, array[greatest(nops, 1)]);
    v_key := array_fill(50, array[greatest(nops, 1)]);
    for i in 1..nops loop
        continue when v_err[i] is not null;
        o := v_ops[i];
        v_prof[i] := iamb_private.op_profile(o, ops);
        if o ->> 'entity' = 'folder_share' then
            v_grp[i] := coalesce((select s.profile_id from iamb.folder_share s where s.id = iamb_private.try_uuid(o ->> 'id')),
                                 iamb_private.try_uuid(o -> 'row' ->> 'profile_id'));
        end if;
        -- Dependency order (§6.3): folders with parents first, tags, attachments, snippets,
        -- links, shares; deletes last, children first.
        v_key[i] := case
            when o ->> 'kind' = 'put' then case o ->> 'entity'
                when 'folder' then 100 + coalesce((
                    with recursive chain as (
                        select iamb_private.try_uuid(o -> 'row' ->> 'parent_id') as pid, 0 as n
                        union all
                        select iamb_private.try_uuid(x -> 'row' ->> 'parent_id'), chain.n + 1
                        from chain join jsonb_array_elements(ops) x
                          on x ->> 'entity' = 'folder' and x ->> 'kind' = 'put'
                         and iamb_private.try_uuid(x ->> 'id') = chain.pid
                        where chain.n < 10
                    )
                    select count(*) - 1 from chain), 0)::integer
                when 'tag' then 200 when 'attachment' then 210 when 'snippet' then 300
                when 'snippet_tag' then 400 when 'snippet_attachment' then 410 when 'folder_share' then 500
                else 50 end
            else case o ->> 'entity'
                when 'snippet_tag' then 600 when 'snippet_attachment' then 610 when 'snippet' then 700
                when 'folder_share' then 750
                when 'folder' then 800 + (20 - coalesce(iamb_private.folder_depth(iamb_private.try_uuid(o ->> 'id')), 0))
                when 'tag' then 900 when 'attachment' then 910
                else 50 end
            end;
    end loop;

    -- Finding 6: only the profiles the caller has a grant in are locked. An op on any other
    -- profile, and a share with a group the caller is not a member of, loses its profile here and
    -- is answered like an op on a profile the server does not know.
    select coalesce(array_agg(x), '{}') into granted from (
        select distinct x from (select unnest(v_prof) as x union all select unnest(v_grp)) y where x is not null
    ) z where iamb_private.has_grant(u, x);
    for i in 1..nops loop
        if v_prof[i] is not null
           and (not v_prof[i] = any (granted) or (v_grp[i] is not null and not v_grp[i] = any (granted))) then
            v_prof[i] := null;
            v_grp[i] := null;
        end if;
    end loop;
    -- N3: of the profiles the caller has a grant in, only those the caller can write something in
    -- are locked. An op in a profile the caller can only read is judged without the lock: it is
    -- refused by the same rules as before, and it cannot write (`next_rev`).
    select coalesce(array_agg(x), '{}') into writable from unnest(granted) x where iamb_private.writes_in(u, x);
    v_ro := array_fill(false, array[greatest(nops, 1)]);
    for i in 1..nops loop
        if v_prof[i] is not null and not v_prof[i] = any (writable) then
            v_ro[i] := true;
        end if;
    end loop;

    -- C24: the calling device's row is locked (by `enter`); now the profiles the ops write in,
    -- groups first, then personal profiles, each set in the order of its ids.
    perform iamb_private.lock_profiles(array(
        select distinct y.x from unnest(v_prof, v_grp, v_ro) as t(p, g, ro)
        cross join lateral (values (t.p), (t.g)) as y(x)
        where not t.ro and y.x is not null));
    -- Finding 3: then the id of every row an op may write, in one order. An id is unique across
    -- all profiles, and the profile locks do not cover a new id that two requests place in two
    -- profiles; this lock makes "is the row there" and the write one step for both. The key has 64
    -- bits and is made with the server's seed (N3): no id can be chosen to share another row's lock.
    for lk in
        select distinct hashtextextended((x.op ->> 'entity') || ':' || iamb_private.try_uuid(x.op ->> 'id')::text,
                                         st.lock_seed) as k
        from unnest(v_ops, v_err, v_prof, v_ro) as x(op, err, prof, ro)
        where x.err is null and x.prof is not null and not x.ro
          and iamb_private.try_uuid(x.op ->> 'id') is not null and x.op ->> 'entity' is not null
        order by 1
    loop
        perform pg_advisory_xact_lock(lk.k);
    end loop;

    results := array_fill(null::jsonb, array[greatest(nops, 1)]);
    budget := st.cascade_max;
    for i in select t.pos from unnest(v_key) with ordinality as t(k, pos) where t.pos <= nops order by t.k, t.pos loop
        o := v_ops[i];
        -- C15: an op whose id the server has seen writes nothing.
        select * into rc from iamb.push_receipt
        where device_id = dev and op_id = iamb_private.try_uuid(o ->> 'op_id');
        if v_err[i] is not null then
            res := iamb_private.op_result(u, case when jsonb_typeof(o) = 'object' then o else '{}'::jsonb end,
                                          'blocked', v_err[i], null, null, false);
        elsif rc.op_id is not null then
            res := iamb_private.op_result(u, o, rc.status, rc.reason, rc.conflict_id, rc.adjusted, true);
            if o ->> 'entity' = 'folder' and o ->> 'kind' = 'delete' then
                res := res || jsonb_build_object('cascade', iamb_private.cascade_answer(u, o -> 'cascade'));
            end if;
        else
            -- The row's profile as it is now, under the locks: a row that came to exist since the
            -- profile was read, in another profile than the one this op named, is not this op's
            -- to write (C8, finding 3).
            rp := iamb_private.row_profile(o ->> 'entity', iamb_private.try_uuid(o ->> 'id'));
            if v_prof[i] is not null and rp is not null and rp <> v_prof[i] then
                res := iamb_private.op_result(u, o, 'blocked', 'invalid', null, null, false);
            elsif v_ro[i] then
                -- A profile the caller can read and write nothing in: judged without its lock.
                -- Should the caller's rights have grown since they were read, the write is still
                -- refused, because nothing holds the lock it needs.
                begin
                    perform set_config('iamb.unlocked_op', 'on', true);
                    res := iamb_private.push_op(u, dev, o, v_prof[i], 0, blocked, null);
                    perform set_config('iamb.unlocked_op', '', true);
                exception when sqlstate 'IB001' then
                    res := iamb_private.op_result(u, o, 'rejected', 'forbidden', null, null, false);
                    if o ->> 'entity' = 'folder' and o ->> 'kind' = 'delete' then
                        res := res || jsonb_build_object('cascade', iamb_private.cascade_answer(u, o -> 'cascade'));
                    end if;
                end;
            else
                res := iamb_private.push_op(u, dev, o, v_prof[i], budget, blocked, null);
            end if;
            budget := budget - coalesce((res ->> '_used')::integer, 0);
            res := res - '_used';
            -- Final answers are kept for 30 days (C15); `partial` and `blocked` are not final.
            if res ->> 'status' in ('applied', 'unchanged', 'superseded', 'rejected')
               and iamb_private.try_uuid(o ->> 'op_id') is not null and iamb_private.try_uuid(o ->> 'id') is not null then
                insert into iamb.push_receipt (device_id, op_id, entity, row_id, status, reason, rev, conflict_id,
                                               adjusted, created_at)
                values (dev, iamb_private.try_uuid(o ->> 'op_id'), coalesce(o ->> 'entity', ''),
                        iamb_private.try_uuid(o ->> 'id'), res ->> 'status', res ->> 'reason',
                        (res -> 'row' ->> 'rev')::bigint, iamb_private.try_uuid(res ->> 'conflict'),
                        array(select jsonb_array_elements_text(coalesce(res -> 'adjusted', '[]'))),
                        iamb_private.now_ms())
                on conflict (device_id, op_id) do nothing;
            end if;
        end if;
        if res ->> 'status' = 'blocked' and jsonb_typeof(o) = 'object' and iamb_private.try_uuid(o ->> 'id') is not null then
            blocked := blocked || iamb_private.try_uuid(o ->> 'id');
        end if;
        results[i] := res;
    end loop;

    return jsonb_build_object('error', null,
        'results', coalesce((select jsonb_agg(t.x order by t.pos) from unnest(results) with ordinality as t(x, pos) where t.pos <= nops), '[]'::jsonb));
end
$$;

-- Review finding 7: an SQL error of a class that a caller's data can cause (a value that does not
-- fit, a broken constraint) undoes the ops of the request and is answered `invalid` as data, so
-- that the call stays counted by the rate limiter (§6.1). N2: so does a request that is cut off,
-- answered `too_large`: the device halves the batch (§6.1). The function log gets the error's
-- code, never its text, which could carry what the caller sent (G20).
create or replace function api.push(device uuid, protocol integer, "position" text, ops jsonb)
returns jsonb language plpgsql volatile security definer set search_path = pg_catalog, pg_temp set statement_timeout = '20s' as $$
declare
    e jsonb := iamb_private.enter(device, protocol, 'push');
    r jsonb;
begin
    if e ->> 'error' is not null then return e; end if;
    begin
        r := iamb_private.push_core(auth.uid(), device, "position", ops);
        perform iamb_private.feed_flush();
    exception when data_exception or integrity_constraint_violation then
        perform iamb_private.log('push_error', jsonb_build_object('user', auth.uid(), 'device', device, 'code', sqlstate));
        r := jsonb_build_object('error', 'invalid');
    when query_canceled or insufficient_resources then
        -- N2: the request ran into its time-out, or the database ran out of temporary file space,
        -- disk or memory for it. What it wrote is undone; the call stays counted, as ten.
        perform iamb_private.log('push_cut_off', jsonb_build_object('user', auth.uid(), 'device', device, 'code', sqlstate));
        update iamb.rate_bucket set count = count + 9 where subject = device and bucket = 'push_calls';
        r := jsonb_build_object('error', 'too_large');
    end;
    if r ->> 'error' is null then
        r := r || jsonb_build_object('head', coalesce((select fh.head from iamb.feed_head fh where fh.user_id = auth.uid()), 0));
    end if;
    return jsonb_build_object('epoch', e -> 'epoch') || r;
end $$;

revoke execute on function iamb_private.writes_in(uuid, uuid), iamb_private.next_rev(uuid[]),
                           iamb_private.cascade_answer(uuid, jsonb), iamb_private.op_shape(jsonb),
                           iamb_private.restore_chain(uuid, uuid, uuid, uuid),
                           iamb_private.push_op(uuid, uuid, jsonb, uuid, integer, uuid[], jsonb),
                           iamb_private.push_core(uuid, uuid, text, jsonb)
    from public, anon, authenticated, service_role;
revoke execute on function api.push(uuid, integer, text, jsonb) from public, anon, service_role;
grant execute on function api.push(uuid, integer, text, jsonb) to authenticated;
