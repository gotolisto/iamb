-- Iamb server, migration 6: `push` (docs/sync-design.md §6.3) and the rules C1 to C15, C17 to
-- C19 and C21 it applies, under the locks of C24. One request is one transaction; each op is
-- judged by itself and a refused op does not undo the others.

do $$
begin
    if current_user <> 'postgres' then
        raise exception 'iamb migrations must run as postgres, not %', current_user;
    end if;
end
$$;

-- The synced fields of each entity under their column names, the fields whose change records a
-- conflict (C3) and the fields that make up a snippet's content (C17).
create or replace function iamb_private.fields(p_entity text) returns text[]
language sql immutable set search_path = '' as $$
    select case p_entity
        when 'folder' then array['profile_id', 'parent_id', 'name', 'position', 'member_permission',
                                 'default_trigger', 'default_case_mode', 'abbreviation_prefix']
        when 'snippet' then array['folder_id', 'abbreviation', 'label', 'kind', 'body', 'trigger_mode',
                                  'case_mode', 'word_start', 'enabled']
        when 'tag' then array['profile_id', 'name', 'color']
        when 'snippet_tag' then array['snippet_id', 'tag_id']
        when 'attachment' then array['profile_id', 'sha256', 'mime', 'size', 'file_name']
        when 'snippet_attachment' then array['snippet_id', 'attachment_id']
        when 'folder_share' then array['folder_id', 'profile_id', 'permission']
    end
$$;

create or replace function iamb_private.recorded_fields(p_entity text) returns text[]
language sql immutable set search_path = '' as $$
    select case p_entity
        when 'folder' then array['parent_id', 'name', 'member_permission', 'default_trigger',
                                 'default_case_mode', 'abbreviation_prefix']
        when 'snippet' then array['folder_id', 'abbreviation', 'label', 'kind', 'body', 'trigger_mode',
                                  'case_mode', 'word_start', 'enabled']
        when 'tag' then array['name', 'color']
        else '{}'::text[]
    end
$$;

create or replace function iamb_private.content_fields() returns text[]
language sql immutable set search_path = '' as $$
    select array['abbreviation', 'kind', 'body', 'trigger_mode', 'case_mode', 'word_start']
$$;

-- The subset of `j` under the given keys (a missing key counts as null).
create or replace function iamb_private.pick(j jsonb, keys text[]) returns jsonb
language sql immutable set search_path = '' as $$
    select coalesce(jsonb_object_agg(k, coalesce(j -> k, 'null'::jsonb)), '{}'::jsonb) from unnest(keys) k
$$;

-- An op's row in the wire form (§6.3) turned into column names and checked (C14): answers the
-- normalised fields, or {"_error": "invalid" | "too_large"}.
create or replace function iamb_private.normalize(p_entity text, r jsonb) returns jsonb
language plpgsql immutable set search_path = '' as $$
declare
    n   jsonb;
    d   jsonb;
    s   text;
    bad boolean := false;
begin
    if r is null or jsonb_typeof(r) <> 'object' then
        return '{"_error": "invalid"}';
    end if;
    case p_entity
    when 'folder' then
        d := coalesce(r -> 'defaults', '{}');
        if jsonb_typeof(d) <> 'object' then return '{"_error": "invalid"}'; end if;
        n := jsonb_build_object(
            'profile_id', iamb_private.try_uuid(r ->> 'profile_id'),
            'parent_id', iamb_private.try_uuid(r ->> 'parent_id'),
            'name', r -> 'name', 'position', r -> 'position',
            'member_permission', r -> 'member_permission',
            'default_trigger', d -> 'trigger', 'default_case_mode', d -> 'case_mode',
            'abbreviation_prefix', d -> 'abbreviation_prefix');
        bad := n ->> 'profile_id' is null
            or (r ->> 'parent_id' is not null and n ->> 'parent_id' is null)
            or jsonb_typeof(r -> 'name') is distinct from 'string'
            or jsonb_typeof(r -> 'position') is distinct from 'number'
            or coalesce(r ->> 'member_permission', 'view') not in ('view', 'edit')
            or coalesce(d ->> 'trigger', 'delimiter') not in ('immediate', 'delimiter')
            or coalesce(d ->> 'case_mode', 'exact') not in ('exact', 'insensitive', 'adaptive')
            or jsonb_typeof(coalesce(d -> 'abbreviation_prefix', '""')) not in ('string', 'null');
        if not bad and (r ->> 'position')::numeric <> trunc((r ->> 'position')::numeric) then bad := true; end if;
        if bad then return '{"_error": "invalid"}'; end if;
        if octet_length(r ->> 'name') > 1024 or char_length(coalesce(d ->> 'abbreviation_prefix', '')) > 64 then
            return '{"_error": "too_large"}';
        end if;
    when 'snippet' then
        n := jsonb_build_object(
            'folder_id', iamb_private.try_uuid(r ->> 'folder_id'),
            'abbreviation', r -> 'abbreviation', 'label', r -> 'label', 'kind', r -> 'kind',
            'body', r -> 'body', 'trigger_mode', r -> 'trigger', 'case_mode', r -> 'case_mode',
            'word_start', r -> 'word_start', 'enabled', r -> 'enabled');
        bad := n ->> 'folder_id' is null
            or jsonb_typeof(r -> 'abbreviation') is distinct from 'string'
            or jsonb_typeof(r -> 'label') is distinct from 'string'
            or jsonb_typeof(r -> 'body') is distinct from 'string'
            or coalesce(r ->> 'kind', '') not in ('plain', 'rich')
            or coalesce(r ->> 'trigger', '') not in ('immediate', 'delimiter')
            or coalesce(r ->> 'case_mode', '') not in ('exact', 'insensitive', 'adaptive')
            or jsonb_typeof(r -> 'word_start') is distinct from 'boolean'
            or jsonb_typeof(r -> 'enabled') is distinct from 'boolean'
            or char_length(coalesce(r ->> 'abbreviation', '')) < 1;
        if bad then return '{"_error": "invalid"}'; end if;
        if char_length(r ->> 'abbreviation') > 64 or octet_length(r ->> 'label') > 1024
           or octet_length(r ->> 'body') > 400000 then
            return '{"_error": "too_large"}';
        end if;
    when 'tag' then
        n := jsonb_build_object('profile_id', iamb_private.try_uuid(r ->> 'profile_id'),
                                'name', r -> 'name', 'color', r -> 'color');
        bad := n ->> 'profile_id' is null or jsonb_typeof(r -> 'name') is distinct from 'string'
            or jsonb_typeof(coalesce(r -> 'color', 'null')) not in ('string', 'null');
        if bad then return '{"_error": "invalid"}'; end if;
        if octet_length(r ->> 'name') > 1024 or octet_length(coalesce(r ->> 'color', '')) > 32 then
            return '{"_error": "too_large"}';
        end if;
    when 'snippet_tag' then
        n := jsonb_build_object('snippet_id', iamb_private.try_uuid(r ->> 'snippet_id'),
                                'tag_id', iamb_private.try_uuid(r ->> 'tag_id'));
        if n ->> 'snippet_id' is null or n ->> 'tag_id' is null then return '{"_error": "invalid"}'; end if;
    when 'attachment' then
        n := jsonb_build_object('profile_id', iamb_private.try_uuid(r ->> 'profile_id'),
                                'sha256', r -> 'sha256', 'mime', r -> 'mime', 'size', r -> 'size',
                                'file_name', r -> 'file_name');
        bad := n ->> 'profile_id' is null or coalesce(r ->> 'sha256', '') !~ '^[0-9a-f]{64}$'
            or jsonb_typeof(r -> 'mime') is distinct from 'string'
            or jsonb_typeof(r -> 'file_name') is distinct from 'string'
            or jsonb_typeof(r -> 'size') is distinct from 'number';
        if not bad and ((r ->> 'size')::numeric < 0 or (r ->> 'size')::numeric <> trunc((r ->> 'size')::numeric)) then
            bad := true;
        end if;
        if bad then return '{"_error": "invalid"}'; end if;
        if octet_length(r ->> 'mime') > 127 or octet_length(r ->> 'file_name') > 1024 then
            return '{"_error": "too_large"}';
        end if;
    when 'snippet_attachment' then
        n := jsonb_build_object('snippet_id', iamb_private.try_uuid(r ->> 'snippet_id'),
                                'attachment_id', iamb_private.try_uuid(r ->> 'attachment_id'));
        if n ->> 'snippet_id' is null or n ->> 'attachment_id' is null then return '{"_error": "invalid"}'; end if;
    when 'folder_share' then
        n := jsonb_build_object('folder_id', iamb_private.try_uuid(r ->> 'folder_id'),
                                'profile_id', iamb_private.try_uuid(r ->> 'profile_id'),
                                'permission', r -> 'permission');
        if n ->> 'folder_id' is null or n ->> 'profile_id' is null
           or coalesce(r ->> 'permission', '') not in ('view', 'edit') then
            return '{"_error": "invalid"}';
        end if;
    else
        return '{"_error": "invalid"}';
    end case;
    -- Normalise JSON nulls and uuid spellings so that the fields compare equal to stored rows.
    s := null;
    select jsonb_object_agg(k, case when v = 'null'::jsonb then 'null'::jsonb else v end) into n
    from jsonb_each(n) as x(k, v);
    if jsonb_typeof(r -> 'created_at') = 'number' then
        n := n || jsonb_build_object('created_at', (r ->> 'created_at')::numeric::bigint);
    end if;
    return n;
end
$$;

-- The row as the caller may read it now, in its wire form, with its origin; null when the caller
-- may not read it or it does not exist.
create or replace function iamb_private.readable_row(u uuid, p_entity text, p_id uuid) returns jsonb
language sql stable set search_path = '' as $$
    select case when iamb_private.can_read(u, p_entity, p_id) then
               (select jsonb_build_object('row', w.row, 'origin', w.origin)
                from iamb.wire_rows w where w.entity = p_entity and w.id = p_id)
           end
$$;

-- Keeps `attachment.unlinked_at` in step with the links (C19, §5.12).
create or replace function iamb_private.touch_attachments(p_ids uuid[]) returns void
language plpgsql volatile set search_path = '' as $$
declare
    now_ms bigint := iamb_private.now_ms();
begin
    update iamb.attachment a set unlinked_at = case
        when exists (select 1 from iamb.snippet_attachment l join iamb.snippet s on s.id = l.snippet_id
                     where l.attachment_id = a.id and l.deleted_at is null and s.deleted_at is null) then null
        else coalesce(a.unlinked_at, now_ms) end
    where a.id = any (p_ids);
end
$$;

-- The version a write was based on (C3, C17): the row or a history version at `base_rev`; for a
-- base of 0 on a row that exists, the version written by the latest op the request names in
-- `follows`. Null when it cannot be found.
create or replace function iamb_private.base_version(p_entity text, cur jsonb, p_base bigint, p_follows uuid[])
returns jsonb
language sql stable set search_path = '' as $$
    select case
        when p_base > 0 and (cur ->> 'rev')::bigint = p_base then cur
        when p_base > 0 then (select h.row from iamb.history h
                              where h.entity = p_entity and h.row_id = (cur ->> 'id')::uuid and h.rev = p_base)
        when coalesce(array_length(p_follows, 1), 0) = 0 then null
        else (select v from (
                  select cur as v, (cur ->> 'rev')::bigint as rv
                  where iamb_private.try_uuid(cur ->> 'updated_op') = any (p_follows)
                  union all
                  select h.row, h.rev from iamb.history h
                  where h.entity = p_entity and h.row_id = (cur ->> 'id')::uuid
                    and iamb_private.try_uuid(h.row ->> 'updated_op') = any (p_follows)
              ) x order by rv desc limit 1)
    end
$$;

-- Records a conflict for the write `winner_rev` over the version `replaced` (C3, C6, C7). A row has
-- one open conflict: a new one takes over every version the open one offered, with their authors as
-- parties, and the old one is closed as `replaced`.
create or replace function iamb_private.record_conflict(p_entity text, p_id uuid, p_profile uuid, p_kind text,
                                                        p_winner_rev bigint, u uuid, dev uuid, replaced jsonb)
returns uuid
language plpgsql volatile set search_path = '' as $$
declare
    cid    uuid := iamb_private.uuid7();
    old    iamb.conflict;
    v_parties uuid[];
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
    perform iamb_private.queue(coalesce(v_parties, '{}') || coalesce(old.parties, '{}'), 'conflict', p_entity, p_id, null, p_profile, null,
                               jsonb_build_object('conflict', cid));
    return cid;
end
$$;

-- Writes a new revision of a content row: the replaced version to the history, the row, the feed
-- entries for everyone who could read it before or can read it after, and the access entries
-- when the folder tree changed. `before` are the readers computed before the write.
create or replace function iamb_private.write_row(u uuid, dev uuid, op uuid, p_entity text, cur jsonb, newrow jsonb,
                                                  p_rev bigint, before uuid[], p_structural boolean,
                                                  p_profile uuid) returns void
language plpgsql volatile set search_path = '' as $$
declare
    after uuid[];
    gained uuid[];
    l record;
begin
    if cur is not null then
        perform iamb_private.keep_history(p_entity, cur, p_rev);
    end if;
    perform iamb_private.store_row(p_entity, newrow);
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

-- C7: restores every deleted folder from `p_folder` up, if the writer may (manage for a top-level
-- folder, edit on its parent for a sub-folder), each with a new revision and a `folder_restored`
-- conflict between the writer and whoever deleted it. Answers false when one may not be restored.
create or replace function iamb_private.restore_chain(u uuid, dev uuid, op uuid, p_folder uuid) returns boolean
language plpgsql volatile set search_path = '' as $$
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
    -- Top-down, so that no live folder is ever under a deleted one.
    for i in reverse coalesce(array_length(chain, 1), 0)..1 loop
        cur := iamb_private.get_row('folder', chain[i]);
        before := iamb_private.readers('folder', chain[i]);
        r := iamb_private.next_rev(array[(cur ->> 'profile_id')::uuid]);
        nr := cur || jsonb_build_object('deleted_at', null, 'rev', r, 'updated_at', now_ms, 'updated_by', u,
                                        'updated_device', dev, 'updated_op', op);
        perform iamb_private.write_row(u, dev, op, 'folder', cur, nr, r, before, true, (cur ->> 'profile_id')::uuid);
        perform iamb_private.record_conflict('folder', chain[i], (cur ->> 'profile_id')::uuid, 'folder_restored',
                                             r, u, dev, cur);
    end loop;
    return true;
end
$$;

-- A whole-row answer for an op.
create or replace function iamb_private.op_result(u uuid, op jsonb, p_status text, p_reason text, p_conflict uuid,
                                                  p_adjusted text[], p_replayed boolean) returns jsonb
language plpgsql stable set search_path = '' as $$
declare
    rr jsonb := iamb_private.readable_row(u, op ->> 'entity', iamb_private.try_uuid(op ->> 'id'));
begin
    return jsonb_build_object('op_id', op -> 'op_id', 'status', p_status, 'reason', p_reason,
                              'conflict', p_conflict, 'adjusted', to_jsonb(coalesce(p_adjusted, '{}')),
                              'replayed', p_replayed, 'blob', null,
                              'row', rr -> 'row', 'origin', rr -> 'origin');
end
$$;

-- The answer for every row of a folder delete's cascade list (C6): the row's revision and whether
-- it is deleted, the whole row where the device does not hold that content, or `gone`.
create or replace function iamb_private.cascade_answer(u uuid, p_list jsonb) returns jsonb
language plpgsql stable set search_path = '' as $$
declare
    it  jsonb;
    res jsonb := '[]';
    ent text;
    rid uuid;
    cur jsonb;
    rr  jsonb;
begin
    for it in select * from jsonb_array_elements(coalesce(p_list, '[]')) loop
        ent := it ->> 'entity';
        rid := iamb_private.try_uuid(it ->> 'id');
        cur := case when ent in ('folder', 'snippet') then iamb_private.get_row(ent, rid) end;
        if cur is null or not iamb_private.can_read(u, ent, rid) then
            res := res || jsonb_build_array(jsonb_build_object('entity', ent, 'id', it -> 'id', 'gone', true));
        elsif (cur ->> 'deleted_at') is null and (cur ->> 'rev')::bigint is distinct from (it ->> 'base_rev')::bigint then
            rr := iamb_private.readable_row(u, ent, rid);
            res := res || jsonb_build_array(jsonb_build_object('entity', ent, 'id', it -> 'id', 'rev', (cur ->> 'rev')::bigint,
                                                               'deleted', false, 'row', rr -> 'row', 'origin', rr -> 'origin'));
        else
            res := res || jsonb_build_array(jsonb_build_object('entity', ent, 'id', it -> 'id', 'rev', (cur ->> 'rev')::bigint,
                                                               'deleted', (cur ->> 'deleted_at') is not null));
        end if;
    end loop;
    return res;
end
$$;

-- ---------------------------------------------------------------------------------------------
-- A folder's delete (C6): the subtree as the server has it, bounded by `cascade_max` per request
-- (answers `partial` while rows remain), deepest first; a share is ended only by who may end it.
-- ---------------------------------------------------------------------------------------------

create or replace function iamb_private.push_folder_delete(u uuid, dev uuid, op jsonb, p_budget integer)
returns jsonb
language plpgsql volatile set search_path = '' set client_min_messages = 'warning' as $$
declare
    fid     uuid := iamb_private.try_uuid(op ->> 'id');
    cur     jsonb := iamb_private.get_row('folder', fid);
    p       uuid := (cur ->> 'profile_id')::uuid;
    list    jsonb := coalesce(op -> 'cascade', '[]');
    base    bigint := coalesce((op ->> 'base_rev')::bigint, 0);
    follows uuid[] := array(select iamb_private.try_uuid(x) from jsonb_array_elements_text(coalesce(op -> 'follows', '[]')) x);
    opid    uuid := iamb_private.try_uuid(op ->> 'op_id');
    n       integer;
    x       record;
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
    shares  uuid[];
    res     jsonb;
begin
    if cur is null then
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

    -- The live rows below, deepest first: a snippet counts one level below its folder.
    create temp table if not exists iamb_cascade (entity text, id uuid, depth integer, folder uuid) on commit drop;
    delete from pg_temp.iamb_cascade;
    insert into pg_temp.iamb_cascade
    with recursive sub as (
        select f.id, 1 as depth, array[f.id] as path from iamb.folder f where f.id = fid
        union all
        select f.id, sub.depth + 1, sub.path || f.id from iamb.folder f join sub on f.parent_id = sub.id
        where f.deleted_at is null and not f.id = any (sub.path)
    )
    select 'folder', sub.id, sub.depth, sub.id from sub where sub.id <> fid
    union all
    select 'snippet', s.id, sub.depth + 1, s.folder_id from iamb.snippet s join sub on s.folder_id = sub.id
    where s.deleted_at is null;
    select count(*) into n from pg_temp.iamb_cascade;

    for x in select * from pg_temp.iamb_cascade order by depth desc, (entity = 'folder'), id limit p_budget loop
        xr := iamb_private.get_row(x.entity, x.id);
        before := iamb_private.readers(x.entity, x.id);
        if x.entity = 'folder' then
            for cid in select s.id from iamb.folder_share s where s.folder_id = x.id and s.deleted_at is null loop
                perform iamb_private.end_share(u, dev, opid, cid, array[p], 'folder_deleted');
            end loop;
        end if;
        r := iamb_private.next_rev(array[p]);
        nr := xr || jsonb_build_object('deleted_at', now_ms, 'rev', r, 'updated_at', now_ms, 'updated_by', u,
                                       'updated_device', dev, 'updated_op', opid);
        perform iamb_private.write_row(u, dev, opid, x.entity, xr, nr, r, before, false, p);
        if x.entity = 'snippet' then snips := snips || x.id; end if;
        -- A row another device wrote that the list does not name, or names with a base below its
        -- revision after a change to a recorded field, gets a `deleted_with_folder` conflict.
        if iamb_private.try_uuid(xr ->> 'updated_device') is distinct from dev then
            select (it ->> 'base_rev')::bigint into lb from jsonb_array_elements(list) it
            where iamb_private.try_uuid(it ->> 'id') = x.id limit 1;
            if lb is null then
                perform iamb_private.record_conflict(x.entity, x.id, p, 'deleted_with_folder', r, u, dev, xr);
            elsif lb < (xr ->> 'rev')::bigint then
                bv := iamb_private.base_version(x.entity, xr, lb, '{}');
                if bv is null or iamb_private.pick(bv, iamb_private.recorded_fields(x.entity))
                                 <> iamb_private.pick(xr, iamb_private.recorded_fields(x.entity)) then
                    perform iamb_private.record_conflict(x.entity, x.id, p, 'deleted_with_folder', r, u, dev, xr);
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
        bv := iamb_private.base_version('folder', cur, base, follows);
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
-- One op (§6.3, §8). Answers its result; `_used` is the number of rows it deleted by cascade.
-- ---------------------------------------------------------------------------------------------

create or replace function iamb_private.push_op(u uuid, dev uuid, op jsonb, p_profile uuid, p_budget integer)
returns jsonb
language plpgsql volatile set search_path = '' as $$
declare
    ent      text := op ->> 'entity';
    rid      uuid := iamb_private.try_uuid(op ->> 'id');
    v_kind   text := op ->> 'kind';
    opid     uuid := iamb_private.try_uuid(op ->> 'op_id');
    base     bigint := coalesce((op ->> 'base_rev')::bigint, 0);
    follows  uuid[] := array(select iamb_private.try_uuid(x) from jsonb_array_elements_text(coalesce(op -> 'follows', '[]')) x);
    repush   text := op ->> 'repush';
    rp_rev   bigint := (op ->> 'repush_rev')::bigint;
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
        if del and cur is null then
            return iamb_private.op_result(u, op, 'unchanged', null, null, null, false);
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
        if cur is null then
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
            bv := iamb_private.base_version(ent, cur, base, follows);
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
    if exists (select 1 from pg_temp.iamb_push_blocked b
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
                          and g.kind <> 'personal' and g.deleted_at is null) then
            return iamb_private.op_result(u, op, 'rejected', 'parent_gone', null, null, false);
        end if;
        if not iamb_private.can_manage(u, cf.id)
           or not exists (select 1 from iamb.profile pp where pp.id = cf.profile_id and pp.kind = 'personal')
           or iamb_private.group_role(u, (n ->> 'profile_id')::uuid) is null
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
        if cur is not null and iamb_private.pick(n, iamb_private.content_fields()) = iamb_private.pick(cur, iamb_private.content_fields()) then
            nr := nr || jsonb_build_object('content_by', cur -> 'content_by', 'content_at', cur -> 'content_at');
        elsif honoured then
            nr := nr || jsonb_build_object('content_by', case when self then u end, 'content_at', now_ms);
        elsif cur is null then
            nr := nr || jsonb_build_object('content_by', u, 'content_at', now_ms);
        else
            bv := iamb_private.base_version(ent, cur, base, follows);
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
            bv := iamb_private.base_version(ent, cur, base, follows);
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

-- The owning profile of an op's row, read before any lock is held (C24): from the row if the
-- server has it, else from the row it names, else from another op of the same request.
create or replace function iamb_private.op_profile(op jsonb, ops jsonb) returns uuid
language plpgsql stable set search_path = '' as $$
declare
    ent text := op ->> 'entity';
    rid uuid := iamb_private.try_uuid(op ->> 'id');
    r   jsonb := op -> 'row';
    p   uuid;
    ref uuid;
    o   jsonb;
    i   integer;
begin
    p := iamb_private.row_profile(ent, rid);
    if p is not null or op ->> 'kind' = 'delete' then
        return p;
    end if;
    case ent
    when 'folder', 'tag', 'attachment' then
        return iamb_private.try_uuid(r ->> 'profile_id');
    when 'snippet', 'folder_share' then
        ref := iamb_private.try_uuid(r ->> 'folder_id');
        p := (select profile_id from iamb.folder where id = ref);
        if p is null then
            select iamb_private.try_uuid(x -> 'row' ->> 'profile_id') into p
            from jsonb_array_elements(ops) x
            where x ->> 'entity' = 'folder' and iamb_private.try_uuid(x ->> 'id') = ref limit 1;
        end if;
        return p;
    when 'snippet_tag', 'snippet_attachment' then
        ref := iamb_private.try_uuid(r ->> 'snippet_id');
        p := (select profile_id from iamb.snippet where id = ref);
        if p is null then
            select x into o from jsonb_array_elements(ops) x
            where x ->> 'entity' = 'snippet' and iamb_private.try_uuid(x ->> 'id') = ref limit 1;
            if o is not null then
                return iamb_private.op_profile(o, ops);
            end if;
        end if;
        return p;
    else
        return null;
    end case;
end
$$;

create or replace function iamb_private.push_core(u uuid, dev uuid, p_position text, ops jsonb) returns jsonb
language plpgsql volatile set search_path = '' set client_min_messages = 'warning' as $$
declare
    st      iamb.server_state;
    pc      jsonb;
    nops    integer;
    wait    bigint;
    budget  integer;
    o       record;
    res     jsonb;
    rc      iamb.push_receipt;
    results jsonb[];
    groups  uuid[];
    profiles uuid[];
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
    perform iamb_private.tx_begin();
    create temp table if not exists iamb_push_ops (idx integer, op jsonb, profile uuid, grp uuid, sortkey integer) on commit drop;
    create temp table if not exists iamb_push_blocked (id uuid) on commit drop;
    delete from pg_temp.iamb_push_ops;
    delete from pg_temp.iamb_push_blocked;
    insert into pg_temp.iamb_push_ops (idx, op)
    select (x.i - 1)::integer, x.v from jsonb_array_elements(ops) with ordinality as x(v, i);
    update pg_temp.iamb_push_ops set profile = iamb_private.op_profile(op, ops),
        grp = case when op ->> 'entity' = 'folder_share' then
                   coalesce((select s.profile_id from iamb.folder_share s where s.id = iamb_private.try_uuid(op ->> 'id')),
                            iamb_private.try_uuid(op -> 'row' ->> 'profile_id')) end;
    -- Dependency order (§6.3): folders with parents first, tags, attachments, snippets, links,
    -- shares; deletes last, children first.
    update pg_temp.iamb_push_ops set sortkey = case
        when op ->> 'kind' = 'put' then case op ->> 'entity'
            when 'folder' then 100 + coalesce((
                with recursive chain as (
                    select iamb_private.try_uuid(op -> 'row' ->> 'parent_id') as pid, 0 as n
                    union all
                    select iamb_private.try_uuid(x.op -> 'row' ->> 'parent_id'), chain.n + 1
                    from chain join pg_temp.iamb_push_ops x
                      on x.op ->> 'entity' = 'folder' and x.op ->> 'kind' = 'put'
                     and iamb_private.try_uuid(x.op ->> 'id') = chain.pid
                    where chain.n < 10
                )
                select count(*) - 1 from chain), 0)::integer
            when 'tag' then 200 when 'attachment' then 210 when 'snippet' then 300
            when 'snippet_tag' then 400 when 'snippet_attachment' then 410 when 'folder_share' then 500
            else 50 end
        else case op ->> 'entity'
            when 'snippet_tag' then 600 when 'snippet_attachment' then 610 when 'snippet' then 700
            when 'folder_share' then 750
            when 'folder' then 800 + (20 - coalesce(iamb_private.folder_depth(iamb_private.try_uuid(op ->> 'id')), 0))
            when 'tag' then 900 when 'attachment' then 910
            else 50 end
        end;

    -- C24: the calling device's row is locked (by `enter`); now the profiles every op touches,
    -- groups first, then personal profiles, each set in the order of its ids.
    select array_agg(distinct x) into profiles from (
        select profile as x from pg_temp.iamb_push_ops union all select grp from pg_temp.iamb_push_ops
    ) y where x is not null;
    perform iamb_private.lock_profiles(coalesce(profiles, '{}'));

    results := array_fill(null::jsonb, array[greatest(nops, 1)]);
    budget := st.cascade_max;
    for o in select * from pg_temp.iamb_push_ops order by sortkey, idx loop
        -- C15: an op whose id the server has seen writes nothing.
        select * into rc from iamb.push_receipt
        where device_id = dev and op_id = iamb_private.try_uuid(o.op ->> 'op_id');
        if rc.op_id is not null then
            res := iamb_private.op_result(u, o.op, rc.status, rc.reason, rc.conflict_id, rc.adjusted, true);
            if o.op ->> 'entity' = 'folder' and o.op ->> 'kind' = 'delete' then
                res := res || jsonb_build_object('cascade', iamb_private.cascade_answer(u, o.op -> 'cascade'));
            end if;
        else
            res := iamb_private.push_op(u, dev, o.op, o.profile, budget);
            budget := budget - coalesce((res ->> '_used')::integer, 0);
            res := res - '_used';
            if res ->> 'status' = 'blocked' then
                insert into pg_temp.iamb_push_blocked values (iamb_private.try_uuid(o.op ->> 'id'));
            end if;
            -- Final answers are kept for 30 days (C15); `partial` and `blocked` are not final.
            if res ->> 'status' in ('applied', 'unchanged', 'superseded', 'rejected')
               and iamb_private.try_uuid(o.op ->> 'op_id') is not null and iamb_private.try_uuid(o.op ->> 'id') is not null then
                insert into iamb.push_receipt (device_id, op_id, entity, row_id, status, reason, rev, conflict_id,
                                               adjusted, created_at)
                values (dev, iamb_private.try_uuid(o.op ->> 'op_id'), coalesce(o.op ->> 'entity', ''),
                        iamb_private.try_uuid(o.op ->> 'id'), res ->> 'status', res ->> 'reason',
                        (res -> 'row' ->> 'rev')::bigint, iamb_private.try_uuid(res ->> 'conflict'),
                        array(select jsonb_array_elements_text(coalesce(res -> 'adjusted', '[]'))),
                        iamb_private.now_ms())
                on conflict (device_id, op_id) do nothing;
            end if;
        end if;
        results[o.idx + 1] := res;
    end loop;

    return jsonb_build_object('error', null,
        'results', coalesce((select jsonb_agg(x order by i) from unnest(results) with ordinality as t(x, i) where i <= nops), '[]'::jsonb));
end
$$;

create or replace function api.push(device uuid, protocol integer, "position" text, ops jsonb)
returns jsonb language plpgsql volatile security definer set search_path = '' set statement_timeout = '20s' as $$
declare
    e jsonb := iamb_private.enter(device, protocol, 'push');
    r jsonb;
begin
    if e ->> 'error' is not null then return e; end if;
    r := iamb_private.push_core(auth.uid(), device, "position", ops);
    perform iamb_private.feed_flush();
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
