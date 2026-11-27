-- Iamb server, migration 12: `push` after the security review of task 4.3
-- (docs/reviews/4.3-security-review.md), findings 3, 6, 7, 10, 11 and the scratch tables of
-- findings 14 and 16.
--
--  3. A create inserts and nothing else, and an update names the profile it expects. Every op's
--     row id is locked (an advisory lock on entity and id, after the profile locks, in one order),
--     and the row is read again under that lock: two requests that name one new id in two
--     profiles are serial, and the second is answered `blocked: invalid`. `backed` is written at
--     an attachment row's creation and never by an update.
--  6. A request locks only the profiles its caller has a grant in. An op on any other profile is
--     answered like one on a profile that does not exist, before any lock.
--  7. An op whose `base_rev`, `repush_rev`, `follows`, `cascade` or numbers have the wrong type or
--     size is `blocked: invalid` (a cascade list over 20,000 rows `blocked: too_large`), as data
--     and before any lock. The cascade answer is built in one statement and stays within 4 MiB:
--     rows that no longer fit come in the short form with `fetch`. An SQL error of the classes a
--     caller's data can cause is caught, so that the request is counted by the rate limiter.
-- 10. A delete of a row the caller cannot read answers `unchanged`, like one of an unknown id; an
--     op that names a profile or a group the caller has no grant in answers `parent_gone`.
-- 11. The base version of a write (C3, C17) is taken from the history only when the caller could
--     read that version; otherwise it "cannot be found".
-- 14, 16. No temporary table and no statement without a `WHERE`.

do $$
begin
    if current_user <> 'postgres' then
        raise exception 'iamb migrations must run as postgres, not %', current_user;
    end if;
end
$$;

-- Earlier forms of functions whose arguments change below (they come back when the earlier
-- migrations are applied again).
drop function if exists iamb_private.store_row(text, jsonb);
drop function if exists iamb_private.base_version(text, jsonb, bigint, uuid[]);
drop function if exists iamb_private.push_op(uuid, uuid, jsonb, uuid, integer);

-- Whether the user has any grant in the profile: its owner (a personal profile), a live member (a
-- live group), or an access entry for one of its folders. A request locks no other profile.
create or replace function iamb_private.has_grant(u uuid, p_profile uuid) returns boolean
language sql stable set search_path = pg_catalog, pg_temp as $$
    select exists (select 1 from iamb.profile p where p.id = p_profile and p.kind = 'personal' and p.owner_user_id = u)
        or exists (select 1 from iamb.membership m join iamb.profile g on g.id = m.profile_id
                   where m.profile_id = p_profile and m.user_id = u and m.deleted_at is null
                     and g.deleted_at is null and g.kind <> 'personal')
        or exists (select 1 from iamb.folder_access fa where fa.user_id = u and fa.profile_id = p_profile)
$$;

-- Whether the user could read a replaced version of a row while it was the row (C3, C17, C20):
-- they wrote that version; or they have an access entry now for the folder that version was in
-- (for a folder: for the folder itself) that began before the version was replaced
-- (`since_rev`, §5.4); for a tag, they own the personal profile or have been a member of the group
-- since before the version was replaced. `hrow` is the history row's content and `p_replaced_by`
-- the revision that replaced it.
create or replace function iamb_private.version_readable(u uuid, p_entity text, hrow jsonb, p_replaced_by bigint)
returns boolean
language sql stable set search_path = pg_catalog, pg_temp as $$
    select coalesce(
        iamb_private.try_uuid(hrow ->> 'updated_by') = u
        or case p_entity
            when 'snippet' then exists (
                select 1 from iamb.folder_access fa
                where fa.user_id = u and fa.folder_id = iamb_private.try_uuid(hrow ->> 'folder_id')
                  and fa.since_rev < p_replaced_by)
            when 'folder' then exists (
                select 1 from iamb.folder_access fa
                where fa.user_id = u and fa.folder_id = iamb_private.try_uuid(hrow ->> 'id')
                  and fa.since_rev < p_replaced_by)
            when 'tag' then exists (
                select 1 from iamb.profile p
                where p.id = iamb_private.try_uuid(hrow ->> 'profile_id') and p.deleted_at is null
                  and ((p.kind = 'personal' and p.owner_user_id = u)
                       or (p.kind <> 'personal' and exists (
                             select 1 from iamb.membership m
                             where m.profile_id = p.id and m.user_id = u and m.deleted_at is null
                               and m.created_rev < p_replaced_by))))
            else false
        end, false)
$$;

-- Writes a whole row of a synced content table from its jsonb form. A create inserts, and fails
-- when the id is in use; an update changes the row with that id in the profile the caller read it
-- in, never `backed` (set at an attachment row's creation and by the upload's commit only), and
-- fails when no such row is there.
create or replace function iamb_private.store_row(p_entity text, j jsonb, p_create boolean) returns void
language plpgsql volatile set search_path = pg_catalog, pg_temp as $$
declare
    n integer;
begin
    if p_create then
        case p_entity
        when 'folder' then insert into iamb.folder select * from jsonb_populate_record(null::iamb.folder, j);
        when 'folder_share' then insert into iamb.folder_share select * from jsonb_populate_record(null::iamb.folder_share, j);
        when 'snippet' then insert into iamb.snippet select * from jsonb_populate_record(null::iamb.snippet, j);
        when 'tag' then insert into iamb.tag select * from jsonb_populate_record(null::iamb.tag, j);
        when 'snippet_tag' then insert into iamb.snippet_tag select * from jsonb_populate_record(null::iamb.snippet_tag, j);
        when 'attachment' then insert into iamb.attachment select * from jsonb_populate_record(null::iamb.attachment, j);
        when 'snippet_attachment' then insert into iamb.snippet_attachment select * from jsonb_populate_record(null::iamb.snippet_attachment, j);
        else raise exception 'store_row: unknown entity';
        end case;
        return;
    end if;
    case p_entity
    when 'folder' then
        update iamb.folder t set
            updated_at = x.updated_at, updated_by = x.updated_by, rev = x.rev,
            deleted_at = x.deleted_at, updated_device = x.updated_device,
            updated_op = x.updated_op, lost_rev = x.lost_rev,
            parent_id = x.parent_id, name = x.name, position = x.position,
            member_permission = x.member_permission, default_trigger = x.default_trigger,
            default_case_mode = x.default_case_mode, abbreviation_prefix = x.abbreviation_prefix
        from jsonb_populate_record(null::iamb.folder, j) x
        where t.id = x.id and t.profile_id = x.profile_id;
    when 'folder_share' then
        update iamb.folder_share t set
            updated_at = x.updated_at, updated_by = x.updated_by, rev = x.rev,
            deleted_at = x.deleted_at, updated_device = x.updated_device,
            updated_op = x.updated_op, lost_rev = x.lost_rev, permission = x.permission
        from jsonb_populate_record(null::iamb.folder_share, j) x
        where t.id = x.id and t.folder_id = x.folder_id and t.profile_id = x.profile_id;
    when 'snippet' then
        update iamb.snippet t set
            updated_at = x.updated_at, updated_by = x.updated_by, rev = x.rev,
            deleted_at = x.deleted_at, updated_device = x.updated_device,
            updated_op = x.updated_op, lost_rev = x.lost_rev,
            folder_id = x.folder_id, abbreviation = x.abbreviation, label = x.label,
            kind = x.kind, body = x.body, trigger_mode = x.trigger_mode,
            case_mode = x.case_mode, word_start = x.word_start, enabled = x.enabled,
            content_by = x.content_by, content_at = x.content_at
        from jsonb_populate_record(null::iamb.snippet, j) x
        where t.id = x.id and t.profile_id = x.profile_id;
    when 'tag' then
        update iamb.tag t set
            updated_at = x.updated_at, updated_by = x.updated_by, rev = x.rev,
            deleted_at = x.deleted_at, updated_device = x.updated_device,
            updated_op = x.updated_op, lost_rev = x.lost_rev, name = x.name, color = x.color
        from jsonb_populate_record(null::iamb.tag, j) x
        where t.id = x.id and t.profile_id = x.profile_id;
    when 'snippet_tag' then
        update iamb.snippet_tag t set
            updated_at = x.updated_at, updated_by = x.updated_by, rev = x.rev,
            deleted_at = x.deleted_at, updated_device = x.updated_device,
            updated_op = x.updated_op, lost_rev = x.lost_rev
        from jsonb_populate_record(null::iamb.snippet_tag, j) x
        where t.id = x.id and t.snippet_id = x.snippet_id and t.tag_id = x.tag_id;
    when 'attachment' then
        update iamb.attachment t set
            updated_at = x.updated_at, updated_by = x.updated_by, rev = x.rev,
            deleted_at = x.deleted_at, updated_device = x.updated_device,
            updated_op = x.updated_op, lost_rev = x.lost_rev, unlinked_at = x.unlinked_at
        from jsonb_populate_record(null::iamb.attachment, j) x
        where t.id = x.id and t.profile_id = x.profile_id and t.sha256 = x.sha256;
    when 'snippet_attachment' then
        update iamb.snippet_attachment t set
            updated_at = x.updated_at, updated_by = x.updated_by, rev = x.rev,
            deleted_at = x.deleted_at, updated_device = x.updated_device,
            updated_op = x.updated_op, lost_rev = x.lost_rev
        from jsonb_populate_record(null::iamb.snippet_attachment, j) x
        where t.id = x.id and t.snippet_id = x.snippet_id and t.attachment_id = x.attachment_id;
    else
        raise exception 'store_row: unknown entity';
    end case;
    get diagnostics n = row_count;
    if n <> 1 then
        raise exception using errcode = 'integrity_constraint_violation',
            message = 'store_row: the row to update is not where it was read';
    end if;
end
$$;

-- Writes a new revision of a content row: the replaced version to the history, the row, the feed
-- entries for everyone who could read it before or can read it after, and the access entries
-- when the folder tree changed. `before` are the readers computed before the write.
create or replace function iamb_private.write_row(u uuid, dev uuid, op uuid, p_entity text, cur jsonb, newrow jsonb,
                                                  p_rev bigint, before uuid[], p_structural boolean,
                                                  p_profile uuid) returns void
language plpgsql volatile set search_path = pg_catalog, pg_temp as $$
declare
    after uuid[];
    gained uuid[];
    l record;
begin
    if cur is not null then
        perform iamb_private.keep_history(p_entity, cur, p_rev);
    end if;
    perform iamb_private.store_row(p_entity, newrow, cur is null);
    perform iamb_private.mark_written(p_entity, (newrow ->> 'id')::uuid, cur is null);
    if p_structural then
        perform iamb_private.refresh_access(array[p_profile]);
    end if;
    after := iamb_private.readers(p_entity, (newrow ->> 'id')::uuid);
    perform iamb_private.queue(before || after, 'row', p_entity, (newrow ->> 'id')::uuid, null, p_profile, null, null);
    -- A snippet that becomes readable for a user through a move brings its links (§5.5).
    if p_entity = 'snippet' and cur is not null then
        gained := array(select x from unnest(after) x where not x = any (before));
        if coalesce(array_length(gained, 1), 0) > 0 then
            for l in select 'snippet_tag' as e, id from iamb.snippet_tag where snippet_id = (newrow ->> 'id')::uuid and deleted_at is null
                     union all
                     select 'snippet_attachment', id from iamb.snippet_attachment where snippet_id = (newrow ->> 'id')::uuid and deleted_at is null
            loop
                perform iamb_private.queue(gained, 'row', l.e, l.id, null, p_profile, null, null);
            end loop;
        end if;
    end if;
end
$$;

-- The version a write was based on (C3, C17): the row or a history version at `base_rev`; for a
-- base of 0 on a row that exists, the version written by the latest op of this device the request
-- names in `follows`. Null when it cannot be found, and a history version the caller could not
-- read while it was the row cannot be found (finding 11).
create or replace function iamb_private.base_version(p_entity text, cur jsonb, p_base bigint, p_follows uuid[],
                                                     u uuid, dev uuid)
returns jsonb
language sql stable set search_path = pg_catalog, pg_temp as $$
    select case
        when p_base > 0 and (cur ->> 'rev')::bigint = p_base then cur
        when p_base > 0 then (select h.row from iamb.history h
                              where h.entity = p_entity and h.row_id = (cur ->> 'id')::uuid and h.rev = p_base
                                and iamb_private.version_readable(u, p_entity, h.row, h.replaced_by_rev))
        when coalesce(array_length(p_follows, 1), 0) = 0 then null
        else (select v from (
                  select cur as v, (cur ->> 'rev')::bigint as rv
                  where iamb_private.try_uuid(cur ->> 'updated_op') = any (p_follows)
                    and iamb_private.try_uuid(cur ->> 'updated_device') = dev
                  union all
                  select h.row, h.rev from iamb.history h
                  where h.entity = p_entity and h.row_id = (cur ->> 'id')::uuid
                    and iamb_private.try_uuid(h.row ->> 'updated_op') = any (p_follows)
                    and iamb_private.try_uuid(h.row ->> 'updated_device') = dev
              ) x order by rv desc limit 1)
    end
$$;

-- Records a conflict for the write `winner_rev` over the version `replaced` (C3, C6, C7). A row has
-- one open conflict: a new one takes over every version the open one offered, with their authors
-- as parties, and the old one is closed as `replaced`. Who of the parties is offered which version
-- is decided when the conflict is read (C20); the feed entry goes to the parties who can read the
-- row now, and to nobody else (findings 1 and 2).
create or replace function iamb_private.record_conflict(p_entity text, p_id uuid, p_profile uuid, p_kind text,
                                                        p_winner_rev bigint, u uuid, dev uuid, replaced jsonb)
returns uuid
language plpgsql volatile set search_path = pg_catalog, pg_temp as $$
declare
    cid    uuid := iamb_private.uuid7();
    old    iamb.conflict;
    v_parties uuid[];
    rd     uuid[];
begin
    select * into old from iamb.conflict where entity = p_entity and row_id = p_id and state = 'open' for update;
    if old.id is not null then
        update iamb.conflict set state = 'replaced', resolved_at = iamb_private.now_ms() where id = old.id;
    end if;
    insert into iamb.conflict (id, entity, row_id, profile_id, kind, winner_rev, winner_user, winner_device,
                               created_at, state, parties)
    values (cid, p_entity, p_id, p_profile, p_kind, p_winner_rev, u, dev, iamb_private.now_ms(), 'open', '{}');
    insert into iamb.conflict_version (conflict_id, rev, user_id, device_id, taken_over)
    values (cid, (replaced ->> 'rev')::bigint, iamb_private.try_uuid(replaced ->> 'updated_by'),
            iamb_private.try_uuid(replaced ->> 'updated_device'), false);
    if old.id is not null then
        insert into iamb.conflict_version (conflict_id, rev, user_id, device_id, taken_over)
        select cid, v.rev, v.user_id, v.device_id, true from iamb.conflict_version v
        where v.conflict_id = old.id
        on conflict (conflict_id, rev) do nothing;
    end if;
    select array_agg(distinct x) into v_parties from (
        select u as x union all select v.user_id from iamb.conflict_version v where v.conflict_id = cid
    ) y where x is not null;
    update iamb.conflict set parties = coalesce(v_parties, '{}') where id = cid;
    rd := iamb_private.readers(p_entity, p_id);
    perform iamb_private.queue(array(select x from unnest(coalesce(v_parties, '{}') || coalesce(old.parties, '{}')) x
                                     where x = any (rd)),
                               'conflict', p_entity, p_id, null, p_profile, null, jsonb_build_object('conflict', cid));
    return cid;
end
$$;

-- The most rows a folder delete's cascade list may name (§6.3).
create or replace function iamb_private.cascade_list_max() returns integer
language sql immutable set search_path = pg_catalog, pg_temp as $$ select 20000 $$;

-- The answer for every row of a folder delete's cascade list (C6): the row's revision and whether
-- it is deleted, the whole row where the device does not hold that content, or `gone`. One
-- statement, whatever the length of the list; whole rows as long as the answer stays within
-- 4 MiB, the others in the short form with `fetch` (§6.3), which the device reads with `rows`.
create or replace function iamb_private.cascade_answer(u uuid, p_list jsonb) returns jsonb
language sql stable set search_path = pg_catalog, pg_temp as $$
    with items as (
        select x.i, x.v ->> 'entity' as ent, x.v -> 'id' as idj, iamb_private.try_uuid(x.v ->> 'id') as rid,
               case when jsonb_typeof(x.v -> 'base_rev') = 'number' and length(x.v ->> 'base_rev') <= 20
                    then (x.v ->> 'base_rev')::numeric end as base
        from jsonb_array_elements(case when jsonb_typeof(p_list) = 'array' then p_list else '[]'::jsonb end)
             with ordinality as x(v, i)
    ),
    found as (
        select it.i, it.ent, it.idj, it.rid, it.base,
               coalesce(f.rev, s.rev) as rev, coalesce(f.deleted_at, s.deleted_at) as del,
               (f.id is not null or s.id is not null) as readable
        from items it
        left join iamb.folder f on it.ent = 'folder' and f.id = it.rid
             and exists (select 1 from iamb.folder_access fa where fa.user_id = u and fa.folder_id = f.id)
        left join iamb.snippet s on it.ent = 'snippet' and s.id = it.rid
             and exists (select 1 from iamb.folder_access fa where fa.user_id = u and fa.folder_id = s.folder_id)
    ),
    whole as (
        select fd.*, w.row, w.origin,
               sum(case when w.row is not null then octet_length(w.row::text) + 300 else 0 end) over (order by fd.i) as bytes
        from found fd
        left join iamb.wire_rows w
               on fd.readable and fd.del is null and fd.rev::numeric is distinct from fd.base
              and w.entity = fd.ent and w.id = fd.rid
    )
    select coalesce(jsonb_agg(
        case when not wh.readable then jsonb_build_object('entity', wh.ent, 'id', wh.idj, 'gone', true)
             when wh.row is not null and wh.bytes <= greatest(4000000 - 160 * (select count(*) from items), 0)
                 then jsonb_build_object('entity', wh.ent, 'id', wh.idj, 'rev', wh.rev, 'deleted', false,
                                         'row', wh.row, 'origin', wh.origin)
             when wh.row is not null
                 then jsonb_build_object('entity', wh.ent, 'id', wh.idj, 'rev', wh.rev, 'deleted', false, 'fetch', true)
             else jsonb_build_object('entity', wh.ent, 'id', wh.idj, 'rev', wh.rev, 'deleted', wh.del is not null)
        end order by wh.i), '[]'::jsonb)
    from whole wh
$$;

-- ---------------------------------------------------------------------------------------------
-- A folder's delete (C6): the subtree as the server has it, bounded by `cascade_max` per request
-- (answers `partial` while rows remain), deepest first; a share is ended only by who may end it.
-- ---------------------------------------------------------------------------------------------

create or replace function iamb_private.push_folder_delete(u uuid, dev uuid, op jsonb, p_budget integer)
returns jsonb
language plpgsql volatile set search_path = pg_catalog, pg_temp as $$
declare
    fid     uuid := iamb_private.try_uuid(op ->> 'id');
    cur     jsonb := iamb_private.get_row('folder', fid);
    p       uuid := (cur ->> 'profile_id')::uuid;
    list    jsonb := case when jsonb_typeof(op -> 'cascade') = 'array' then op -> 'cascade' else '[]'::jsonb end;
    base    bigint := coalesce((op ->> 'base_rev')::numeric::bigint, 0);
    follows uuid[] := array(select iamb_private.try_uuid(x) from jsonb_array_elements_text(coalesce(op -> 'follows', '[]')) x);
    opid    uuid := iamb_private.try_uuid(op ->> 'op_id');
    bases   jsonb;
    n       integer;
    ents    text[];
    ids     uuid[];
    i       integer;
    xr      jsonb;
    lb      bigint;
    bv      jsonb;
    r       bigint;
    before  uuid[];
    nr      jsonb;
    now_ms  bigint := iamb_private.now_ms();
    used    integer := 0;
    cid     uuid;
    snips   uuid[] := '{}';
    res     jsonb;
begin
    -- A folder the server does not know, or one the caller cannot read: the same answer.
    if cur is null or not iamb_private.can_read(u, 'folder', fid) then
        return iamb_private.op_result(u, op, 'unchanged', null, null, null, false)
               || jsonb_build_object('cascade', iamb_private.cascade_answer(u, list), '_used', 0);
    end if;
    -- Permission (§4.3, C12): the folder itself, then every root of a live share below it.
    if not ((cur ->> 'parent_id' is null and iamb_private.manages_profile(u, p))
            or (cur ->> 'parent_id' is not null and iamb_private.can_edit(u, (cur ->> 'parent_id')::uuid)))
       or (iamb_private.is_share_root(fid) and not iamb_private.can_manage(u, fid))
       or exists (
            with recursive sub as (
                select f.id from iamb.folder f where f.parent_id = fid
                union all
                select f.id from iamb.folder f join sub on f.parent_id = sub.id
            )
            select 1 from sub join iamb.folder_share s on s.folder_id = sub.id and s.deleted_at is null
            where not iamb_private.can_manage(u, sub.id)) then
        return iamb_private.op_result(u, op, 'rejected', 'forbidden', null, null, false)
               || jsonb_build_object('cascade', iamb_private.cascade_answer(u, list), '_used', 0);
    end if;
    if (cur ->> 'deleted_at') is not null then
        return iamb_private.op_result(u, op, 'unchanged', null, null, null, false)
               || jsonb_build_object('cascade', iamb_private.cascade_answer(u, list), '_used', 0);
    end if;

    -- The bases the list names, by row id (read once, not once per row).
    select coalesce(jsonb_object_agg(y.k, y.b), '{}') into bases from (
        select iamb_private.try_uuid(it ->> 'id')::text as k,
               case when jsonb_typeof(it -> 'base_rev') = 'number' and length(it ->> 'base_rev') <= 20
                         and (it ->> 'base_rev')::numeric between 0 and 9007199254740991
                    then trunc((it ->> 'base_rev')::numeric) end as b
        from jsonb_array_elements(list) it
    ) y where y.k is not null;

    -- The live rows below, deepest first: a snippet counts one level below its folder.
    with recursive sub as (
        select f.id, 1 as depth, array[f.id] as path from iamb.folder f where f.id = fid
        union all
        select f.id, sub.depth + 1, sub.path || f.id from iamb.folder f join sub on f.parent_id = sub.id
        where f.deleted_at is null and not f.id = any (sub.path)
    ),
    below as (
        select 'folder'::text as entity, sub.id, sub.depth from sub where sub.id <> fid
        union all
        select 'snippet', s.id, sub.depth + 1 from iamb.snippet s join sub on s.folder_id = sub.id
        where s.deleted_at is null
    )
    select count(*)::integer,
           coalesce(array_agg(b.entity order by b.depth desc, (b.entity = 'folder'), b.id), '{}'),
           coalesce(array_agg(b.id order by b.depth desc, (b.entity = 'folder'), b.id), '{}')
    into n, ents, ids from below b;

    for i in 1..least(n, greatest(p_budget, 0)) loop
        xr := iamb_private.get_row(ents[i], ids[i]);
        before := iamb_private.readers(ents[i], ids[i]);
        if ents[i] = 'folder' then
            for cid in select s.id from iamb.folder_share s where s.folder_id = ids[i] and s.deleted_at is null loop
                perform iamb_private.end_share(u, dev, opid, cid, array[p], 'folder_deleted');
            end loop;
        end if;
        r := iamb_private.next_rev(array[p]);
        nr := xr || jsonb_build_object('deleted_at', now_ms, 'rev', r, 'updated_at', now_ms, 'updated_by', u,
                                       'updated_device', dev, 'updated_op', opid);
        perform iamb_private.write_row(u, dev, opid, ents[i], xr, nr, r, before, false, p);
        if ents[i] = 'snippet' then snips := snips || ids[i]; end if;
        -- A row another device wrote that the list does not name, or names with a base below its
        -- revision after a change to a recorded field, gets a `deleted_with_folder` conflict.
        if iamb_private.try_uuid(xr ->> 'updated_device') is distinct from dev then
            lb := (bases ->> ids[i]::text)::numeric::bigint;
            if lb is null then
                perform iamb_private.record_conflict(ents[i], ids[i], p, 'deleted_with_folder', r, u, dev, xr);
            elsif lb < (xr ->> 'rev')::bigint then
                bv := iamb_private.base_version(ents[i], xr, lb, '{}', u, dev);
                if bv is null or iamb_private.pick(bv, iamb_private.recorded_fields(ents[i]))
                                 <> iamb_private.pick(xr, iamb_private.recorded_fields(ents[i])) then
                    perform iamb_private.record_conflict(ents[i], ids[i], p, 'deleted_with_folder', r, u, dev, xr);
                end if;
            end if;
        end if;
        used := used + 1;
    end loop;
    if coalesce(array_length(snips, 1), 0) > 0 then
        perform iamb_private.touch_attachments(array(select attachment_id from iamb.snippet_attachment where snippet_id = any (snips)));
    end if;
    if n > p_budget then
        perform iamb_private.refresh_access(array[p]);
        return jsonb_build_object('op_id', op -> 'op_id', 'status', 'partial', 'reason', null, 'conflict', null,
                                  'adjusted', '[]'::jsonb, 'replayed', false, 'blob', null, 'row', null,
                                  'origin', null, '_used', used);
    end if;

    -- Nothing left below: the folder itself, with its own shares.
    before := iamb_private.readers('folder', fid);
    for cid in select s.id from iamb.folder_share s where s.folder_id = fid and s.deleted_at is null loop
        perform iamb_private.end_share(u, dev, opid, cid, array[p], 'folder_deleted');
    end loop;
    r := iamb_private.next_rev(array[p]);
    nr := cur || jsonb_build_object('deleted_at', now_ms, 'rev', r, 'updated_at', now_ms, 'updated_by', u,
                                    'updated_device', dev, 'updated_op', opid);
    perform iamb_private.write_row(u, dev, opid, 'folder', cur, nr, r, before, true, p);
    cid := null;
    if base < (cur ->> 'rev')::bigint
       and not (iamb_private.try_uuid(cur ->> 'updated_device') = dev
                and iamb_private.try_uuid(cur ->> 'updated_op') = any (follows)) then
        bv := iamb_private.base_version('folder', cur, base, follows, u, dev);
        if bv is null or iamb_private.pick(bv, iamb_private.recorded_fields('folder'))
                         <> iamb_private.pick(cur, iamb_private.recorded_fields('folder'))
           or (bv ->> 'deleted_at' is null) <> (cur ->> 'deleted_at' is null) then
            cid := iamb_private.record_conflict('folder', fid, p, 'delete_over_edit', r, u, dev, cur);
        end if;
    end if;
    res := iamb_private.op_result(u, op, 'applied', null, cid, null, false);
    return res || jsonb_build_object('cascade', iamb_private.cascade_answer(u, list), '_used', used);
end
$$;

-- ---------------------------------------------------------------------------------------------

-- ---------------------------------------------------------------------------------------------
-- One op (§6.3, §8). Answers its result; `_used` is the number of rows it deleted by cascade.
-- `p_profile` is null for an op whose profile is unknown or one the caller has no grant in.
-- `p_blocked` are the ids of rows blocked earlier in the same request (C14). `p_origin` is set
-- only by "Restore this version" (C20): the author the restored content had, which the restored
-- row keeps; no device can pass it.
-- ---------------------------------------------------------------------------------------------

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

-- ---------------------------------------------------------------------------------------------
-- The request (§6.3)
-- ---------------------------------------------------------------------------------------------

-- Whether an op can be judged at all (finding 7): null, or the reason it is blocked. Checked for
-- every op before any lock, so that nothing a device sends ends a request in an SQL error.
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
    blocked  uuid[] := '{}';
    lk       record;
    rp       uuid;
begin
    select * into st from iamb.server_state;
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

    -- C24: the calling device's row is locked (by `enter`); now the profiles the ops touch, groups
    -- first, then personal profiles, each set in the order of its ids.
    perform iamb_private.lock_profiles(array(
        select distinct x from (select unnest(v_prof) as x union all select unnest(v_grp)) y where x is not null));
    -- Finding 3: then the id of every row an op names, in one order. An id is unique across all
    -- profiles, and the profile locks do not cover a new id that two requests place in two
    -- profiles; this lock makes "is the row there" and the write one step for both.
    for lk in
        select distinct hashtext('iamb.row') as k1, hashtext((x.op ->> 'entity') || ':' || iamb_private.try_uuid(x.op ->> 'id')::text) as k2
        from unnest(v_ops, v_err, v_prof) as x(op, err, prof)
        where x.err is null and x.prof is not null and iamb_private.try_uuid(x.op ->> 'id') is not null
          and x.op ->> 'entity' is not null
        order by 1, 2
    loop
        perform pg_advisory_xact_lock(lk.k1, lk.k2);
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

-- Finding 7: an SQL error of a class that a caller's data can cause (a value that does not fit, a
-- broken constraint) undoes the ops of the request and is answered `invalid` as data, so that the
-- call stays counted by the rate limiter (§6.1). The function log gets the error's code, never its
-- text, which could carry what the caller sent (G20, finding 13).
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
    end;
    if r ->> 'error' is null then
        r := r || jsonb_build_object('head', coalesce((select fh.head from iamb.feed_head fh where fh.user_id = auth.uid()), 0));
    end if;
    return jsonb_build_object('epoch', e -> 'epoch') || r;
end $$;

revoke execute on all functions in schema iamb_private from public, anon, authenticated, service_role;
grant execute on function
    iamb_private.session_ok(),
    iamb_private.member_of(uuid),
    iamb_private.role_in(uuid),
    iamb_private.shares_group_with(uuid),
    iamb_private.own_address(),
    iamb_private.enter(uuid, integer, text),
    iamb_private.position(uuid, text, text, bigint)
to authenticated;
revoke execute on function api.push(uuid, integer, text, jsonb) from public, anon, service_role;
grant execute on function api.push(uuid, integer, text, jsonb) to authenticated;
