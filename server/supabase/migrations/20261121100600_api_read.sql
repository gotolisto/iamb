-- Iamb server, migration 7: the reading functions (docs/sync-design.md §6.4, §6.5), conflicts
-- (§5.6, C20), the upload call of §6.11 and the two functions of `api_service`.
--
-- `pull`, `rows`, `fetch_folder`, `snapshot`, `visible_folders` and `access` are `security
-- invoker`: they call `iamb_private.enter` first and return rows through queries under the
-- caller's policies, so a mistake in the feed cannot show a row its reader may not read.

do $$
begin
    if current_user <> 'postgres' then
        raise exception 'iamb migrations must run as postgres, not %', current_user;
    end if;
end
$$;

-- An internal row (as stored, or as kept in the history) in its wire form; owner only.
create or replace function iamb_private.wire_of(p_entity text, j jsonb) returns jsonb
language sql immutable set search_path = '' as $$
    select case p_entity
        when 'folder' then jsonb_build_object(
            'id', j -> 'id', 'created_at', j -> 'created_at', 'updated_at', j -> 'updated_at',
            'updated_by', j -> 'updated_by', 'rev', j -> 'rev', 'deleted_at', j -> 'deleted_at',
            'profile_id', j -> 'profile_id', 'parent_id', j -> 'parent_id', 'name', j -> 'name',
            'position', j -> 'position', 'member_permission', j -> 'member_permission',
            'defaults', jsonb_build_object('trigger', j -> 'default_trigger', 'case_mode', j -> 'default_case_mode',
                                           'abbreviation_prefix', j -> 'abbreviation_prefix'))
        when 'snippet' then jsonb_build_object(
            'id', j -> 'id', 'created_at', j -> 'created_at', 'updated_at', j -> 'updated_at',
            'updated_by', j -> 'updated_by', 'rev', j -> 'rev', 'deleted_at', j -> 'deleted_at',
            'folder_id', j -> 'folder_id', 'abbreviation', j -> 'abbreviation', 'label', j -> 'label',
            'kind', j -> 'kind', 'body', j -> 'body', 'trigger', j -> 'trigger_mode',
            'case_mode', j -> 'case_mode', 'word_start', j -> 'word_start', 'enabled', j -> 'enabled')
        else j - array['updated_device', 'updated_op', 'lost_rev', 'head_rev', 'offered_owner', 'offered_at',
                       'created_rev', 'content_by', 'content_at', 'unlinked_at', 'backed']
    end
$$;

-- ---------------------------------------------------------------------------------------------
-- pull (§6.4)
-- ---------------------------------------------------------------------------------------------

create or replace function api.pull(device uuid, protocol integer, after text, "limit" integer default 500)
returns jsonb language plpgsql volatile security invoker set search_path = '' set statement_timeout = '8s' as $$
declare
    e        jsonb := iamb_private.enter(device, protocol, 'pull');
    pc       jsonb;
    aseq     bigint;
    lim      integer := least(greatest(coalesce("limit", 500), 1), 500);
    uid      uuid := auth.uid();
    v_head   bigint;
    ent      record;
    entries  jsonb := '[]';
    v_size   bigint := 0;
    v_last   bigint;
    v_more   boolean := false;
    item     jsonb;
    known    text[] := '{}';
    frontier jsonb;
    refs     jsonb := '[]';
    newrefs  jsonb;
    i        integer;
begin
    if e ->> 'error' is not null then return e; end if;
    pc := iamb_private.position(device, after, 'pull');
    if pc ->> 'error' is not null then
        return jsonb_build_object('error', pc ->> 'error', 'reason', pc -> 'reason', 'epoch', e -> 'epoch');
    end if;
    aseq := (pc ->> 'seq')::bigint;
    v_last := aseq;
    select fh.head into v_head from iamb.feed_head fh where fh.user_id = uid;

    for ent in
        select f.seq, f.kind, f.entity, f.row_id, f.folder_id, f.profile_id, f.sha256, f.detail, w.row, w.origin
        from iamb.feed f
        left join iamb.wire_rows w on f.kind = 'row' and w.entity = f.entity and w.id = f.row_id
        where f.user_id = uid and f.seq > aseq
        order by f.seq
        limit lim + 1
    loop
        -- At most `limit` entries and about 4 MiB; a page always ends at an entry boundary.
        if jsonb_array_length(entries) >= lim or (v_size > 4000000 and jsonb_array_length(entries) > 0) then
            v_more := true;
            exit;
        end if;
        item := case ent.kind
            when 'row' then jsonb_build_object('seq', ent.seq, 'kind', 'row', 'entity', ent.entity, 'id', ent.row_id,
                                               'row', ent.row, 'origin', ent.origin)
            when 'folder_visible' then jsonb_build_object('seq', ent.seq, 'kind', ent.kind, 'folder', ent.folder_id)
            when 'folder_revoked' then jsonb_build_object('seq', ent.seq, 'kind', ent.kind, 'folder', ent.folder_id,
                                                          'except', coalesce(ent.detail -> 'except', '[]'))
            when 'conflict' then jsonb_build_object('seq', ent.seq, 'kind', ent.kind, 'conflict', ent.detail -> 'conflict',
                                                    'entity', ent.entity, 'id', ent.row_id)
            when 'blob' then jsonb_build_object('seq', ent.seq, 'kind', ent.kind, 'profile', ent.profile_id, 'sha256', ent.sha256)
            else jsonb_build_object('seq', ent.seq, 'kind', ent.kind)
        end;
        v_size := v_size + octet_length(item::text);
        entries := entries || jsonb_build_array(item);
        v_last := ent.seq;
        if ent.kind = 'row' then
            known := known || (ent.entity || ':' || ent.row_id::text);
        end if;
    end loop;

    -- A page is closed under references (§6.4): the rows its rows refer to (their folders and the
    -- folders above, the snippet, tag or attachment of a link, the profile) come with it in
    -- `refs`, as the caller may read them.
    frontier := coalesce((select jsonb_agg(jsonb_build_object('entity', x ->> 'entity', 'row', x -> 'row'))
                          from jsonb_array_elements(entries) x
                          where x ->> 'kind' = 'row' and jsonb_typeof(x -> 'row') = 'object'), '[]');
    for i in 1..8 loop
        exit when jsonb_array_length(frontier) = 0;
        with fr as (
            select x ->> 'entity' as e, x -> 'row' as r from jsonb_array_elements(frontier) x
        ),
        refd as (
            select 'folder' as e, fr.r ->> 'folder_id' as t from fr where fr.e in ('snippet', 'folder_share')
            union all select 'folder', fr.r ->> 'parent_id' from fr where fr.e = 'folder'
            union all select 'profile', fr.r ->> 'profile_id' from fr
                      where fr.e in ('folder', 'tag', 'attachment', 'membership', 'folder_share')
            union all select 'snippet', fr.r ->> 'snippet_id' from fr where fr.e in ('snippet_tag', 'snippet_attachment')
            union all select 'tag', fr.r ->> 'tag_id' from fr where fr.e = 'snippet_tag'
            union all select 'attachment', fr.r ->> 'attachment_id' from fr where fr.e = 'snippet_attachment'
        ),
        cast_ as (
            select distinct refd.e,
                   case when refd.t ~* '^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$'
                        then refd.t::uuid end as id
            from refd
        )
        select coalesce(jsonb_agg(jsonb_build_object('entity', w.entity, 'id', w.id, 'row', w.row, 'origin', w.origin)), '[]')
        into newrefs
        from cast_ join iamb.wire_rows w on w.entity = cast_.e and w.id = cast_.id
        where cast_.id is not null and not (cast_.e || ':' || cast_.id::text) = any (known);
        known := known || array(select (x ->> 'entity') || ':' || (x ->> 'id') from jsonb_array_elements(newrefs) x);
        refs := refs || newrefs;
        frontier := newrefs;
    end loop;

    pc := iamb_private.position(device, after, 'pull', v_last);
    return jsonb_build_object('error', null, 'epoch', e -> 'epoch', 'next', pc -> 'position',
                              'head', coalesce(v_head, 0), 'more', v_more, 'entries', entries, 'refs', refs);
end $$;

-- ---------------------------------------------------------------------------------------------
-- Whole reads (§6.5)
-- ---------------------------------------------------------------------------------------------

-- Up to 500 rows named by entity and id, each as the caller may read it now, or null (D15).
create or replace function api.rows(device uuid, protocol integer, refs jsonb)
returns jsonb language plpgsql volatile security invoker set search_path = '' set statement_timeout = '8s' as $$
declare
    e jsonb := iamb_private.enter(device, protocol, 'rows');
begin
    if e ->> 'error' is not null then return e; end if;
    if refs is null or jsonb_typeof(refs) <> 'array' or jsonb_array_length(refs) > 500 then
        return jsonb_build_object('error', 'too_large', 'epoch', e -> 'epoch');
    end if;
    return jsonb_build_object('error', null, 'epoch', e -> 'epoch', 'rows', coalesce((
        select jsonb_agg(jsonb_build_object('entity', x.v ->> 'entity', 'id', x.v -> 'id',
                                            'row', w.row, 'origin', w.origin) order by x.i)
        from jsonb_array_elements(refs) with ordinality as x(v, i)
        left join iamb.wire_rows w
          on w.entity = x.v ->> 'entity'
         and w.id = case when x.v ->> 'id' ~* '^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$'
                         then (x.v ->> 'id')::uuid end), '[]'::jsonb));
end $$;

-- The live rows of the tree under `folder` the caller can read, in pages of 500 by a keyset
-- cursor: folders (parents first), snippets with their origin, links, the tags and attachments
-- those links name (with whether the bytes are verified and readable), the readable shares of
-- those folders, and the owning profile; `head` on the first page.
create or replace function api.fetch_folder(device uuid, protocol integer, folder uuid, cursor text default null)
returns jsonb language plpgsql volatile security invoker set search_path = '' set statement_timeout = '15s' as $$
declare
    e      jsonb := iamb_private.enter(device, protocol, 'fetch_folder');
    uid    uuid := auth.uid();
    v_rows jsonb;
    v_next text;
begin
    if e ->> 'error' is not null then return e; end if;
    if not exists (select 1 from iamb.folder f where f.id = folder and f.deleted_at is null) then
        return jsonb_build_object('error', 'not_found', 'epoch', e -> 'epoch');
    end if;
    with recursive tree as (
        select f.id, 1 as depth, array[f.id] as path from iamb.folder f where f.id = folder and f.deleted_at is null
        union all
        select f.id, tree.depth + 1, tree.path || f.id from iamb.folder f join tree on f.parent_id = tree.id
        where f.deleted_at is null and not f.id = any (tree.path)
    ),
    snips as (select s.id from iamb.snippet s join tree on s.folder_id = tree.id where s.deleted_at is null),
    tl as (select l.id, l.tag_id from iamb.snippet_tag l join snips on snips.id = l.snippet_id where l.deleted_at is null),
    al as (select l.id, l.attachment_id from iamb.snippet_attachment l join snips on snips.id = l.snippet_id where l.deleted_at is null),
    keyed as (
        select '1' || lpad(tree.depth::text, 3, '0') || tree.id::text as k, 'folder' as e, tree.id from tree
        union all select '2000' || x.id::text, 'folder_share', x.id from iamb.folder_share x join tree on x.folder_id = tree.id where x.deleted_at is null
        union all select '3000' || p.id::text, 'profile', p.id from iamb.profile p where p.id = (select f.profile_id from iamb.folder f where f.id = folder)
        union all select '4000' || t.id::text, 'tag', t.id from iamb.tag t where t.id in (select tl.tag_id from tl) and t.deleted_at is null
        union all select '5000' || a.id::text, 'attachment', a.id from iamb.attachment a where a.id in (select al.attachment_id from al) and a.deleted_at is null
        union all select '6000' || snips.id::text, 'snippet', snips.id from snips
        union all select '7000' || tl.id::text, 'snippet_tag', tl.id from tl
        union all select '8000' || al.id::text, 'snippet_attachment', al.id from al
    ),
    page as (
        select keyed.k, w.entity, w.id, w.row, w.origin, row_number() over (order by keyed.k) as rn
        from keyed join iamb.wire_rows w on w.entity = keyed.e and w.id = keyed.id
        where cursor is null or keyed.k > cursor
        order by keyed.k
        limit 501
    ),
    shown as (
        select page.*,
               case when page.entity = 'attachment' then jsonb_build_object('verified', exists (
                        select 1 from iamb.attachment_blob b
                        where b.profile_id = (page.row ->> 'profile_id')::uuid and b.sha256 = page.row ->> 'sha256'))
                    else page.origin end as org
        from page
    )
    select jsonb_agg(jsonb_build_object('entity', shown.entity, 'id', shown.id, 'row', shown.row, 'origin', shown.org)
                     order by shown.k) filter (where shown.rn <= 500),
           case when count(*) > 500 then max(shown.k) filter (where shown.rn = 500) end
    into v_rows, v_next
    from shown;
    return jsonb_build_object('error', null, 'epoch', e -> 'epoch', 'rows', coalesce(v_rows, '[]'),
                              'cursor', v_next,
                              'head', case when cursor is null then
                                  coalesce((select fh.head from iamb.feed_head fh where fh.user_id = uid), 0) end);
end $$;

-- Everything the caller can read, tombstones included, with the profiles and memberships, in
-- pages of 500. The first page (no cursor) checks the position (C25), carries `start`, a position
-- at the feed's head, and, when the position presented is of an older epoch, `lost` (C18).
create or replace function api.snapshot(device uuid, protocol integer, "position" text default null, cursor text default null)
returns jsonb language plpgsql volatile security invoker set search_path = '' set statement_timeout = '15s' as $$
declare
    e      jsonb := iamb_private.enter(device, protocol, 'snapshot');
    uid    uuid := auth.uid();
    pc     jsonb;
    v_head bigint;
    v_rows jsonb;
    v_next text;
    res    jsonb;
begin
    if e ->> 'error' is not null then return e; end if;
    res := jsonb_build_object('error', null, 'epoch', e -> 'epoch');
    if cursor is null then
        select fh.head into v_head from iamb.feed_head fh where fh.user_id = uid;
        pc := iamb_private.position(device, "position", 'snapshot', coalesce(v_head, 0));
        if pc ->> 'error' is not null then
            return jsonb_build_object('error', pc ->> 'error', 'epoch', e -> 'epoch');
        end if;
        res := res || jsonb_build_object('start', pc -> 'position', 'head', coalesce(v_head, 0));
        if pc ? 'lost' then
            res := res || jsonb_build_object('lost', pc -> 'lost');
        end if;
    end if;
    with fa as (
        select x.folder_id, x.profile_id from iamb.folder_access x where x.user_id = uid
    ),
    profs as (
        select fa.profile_id as id from fa
        union select p.id from iamb.profile p where p.owner_user_id = uid and p.kind = 'personal'
        union select m.profile_id from iamb.membership m where m.user_id = uid
    ),
    keyed as (
        select '1000' || p.id::text as k, 'profile' as e, p.id from iamb.profile p where p.id in (select id from profs)
        union all select '2000' || m.id::text, 'membership', m.id from iamb.membership m where m.profile_id in (select id from profs)
        union all select '3' || lpad(coalesce(dep.d, 1)::text, 3, '0') || f.id::text, 'folder', f.id
            from iamb.folder f
            left join lateral (
                with recursive up as (
                    select f.parent_id as pid, 1 as d
                    union all
                    select g.parent_id, up.d + 1 from iamb.folder g join up on g.id = up.pid where up.d < 8
                )
                select max(up.d) as d from up
            ) dep on true
            where f.id in (select fa.folder_id from fa)
        union all select '4000' || s.id::text, 'folder_share', s.id from iamb.folder_share s where s.folder_id in (select fa.folder_id from fa)
        union all select '5000' || t.id::text, 'tag', t.id from iamb.tag t where t.profile_id in (select id from profs)
        union all select '6000' || a.id::text, 'attachment', a.id from iamb.attachment a where a.profile_id in (select id from profs)
        union all select '7000' || s.id::text, 'snippet', s.id from iamb.snippet s where s.folder_id in (select fa.folder_id from fa)
        union all select '8000' || l.id::text, 'snippet_tag', l.id from iamb.snippet_tag l join iamb.snippet s on s.id = l.snippet_id
            where s.folder_id in (select fa.folder_id from fa)
        union all select '9000' || l.id::text, 'snippet_attachment', l.id from iamb.snippet_attachment l join iamb.snippet s on s.id = l.snippet_id
            where s.folder_id in (select fa.folder_id from fa)
    ),
    page as (
        select keyed.k, w.entity, w.id, w.row, w.origin, row_number() over (order by keyed.k) as rn
        from keyed join iamb.wire_rows w on w.entity = keyed.e and w.id = keyed.id
        where cursor is null or keyed.k > cursor
        order by keyed.k
        limit 501
    ),
    shown as (
        select page.*,
               case when page.entity = 'attachment' then jsonb_build_object('verified', exists (
                        select 1 from iamb.attachment_blob b
                        where b.profile_id = (page.row ->> 'profile_id')::uuid and b.sha256 = page.row ->> 'sha256'))
                    else page.origin end as org
        from page
    )
    select jsonb_agg(jsonb_build_object('entity', shown.entity, 'id', shown.id, 'row', shown.row, 'origin', shown.org)
                     order by shown.k) filter (where shown.rn <= 500),
           case when count(*) > 500 then max(shown.k) filter (where shown.rn = 500) end
    into v_rows, v_next
    from shown;
    return res || jsonb_build_object('rows', coalesce(v_rows, '[]'), 'cursor', v_next);
end $$;

-- The ids of all folders the caller can read; answered from `protocol_oldest` on and for accounts
-- whose plan has no sync (the access check, D12).
create or replace function api.visible_folders(device uuid, protocol integer)
returns jsonb language plpgsql volatile security invoker set search_path = '' set statement_timeout = '8s' as $$
declare
    e jsonb := iamb_private.enter(device, protocol, 'visible_folders');
begin
    if e ->> 'error' is not null then return e; end if;
    return jsonb_build_object('error', null, 'epoch', e -> 'epoch', 'folders', coalesce((
        select jsonb_agg(fa.folder_id order by fa.folder_id) from iamb.folder_access fa
        where fa.user_id = auth.uid()), '[]'::jsonb));
end $$;

-- The caller's access roots outside their personal profile, and their role in each group.
create or replace function api.access(device uuid, protocol integer)
returns jsonb language plpgsql volatile security invoker set search_path = '' set statement_timeout = '8s' as $$
declare
    e   jsonb := iamb_private.enter(device, protocol, 'access');
    uid uuid := auth.uid();
begin
    if e ->> 'error' is not null then return e; end if;
    return jsonb_build_object('error', null, 'epoch', e -> 'epoch',
        'roots', coalesce((
            select jsonb_agg(jsonb_build_object('root', fa.folder_id, 'profile', fa.profile_id,
                                                'permission', fa.permission, 'via', fa.via,
                                                'granted_at', fa.granted_at) order by fa.granted_at, fa.folder_id)
            from iamb.folder_access fa
            where fa.user_id = uid and fa.root_folder_id = fa.folder_id and fa.via <> 'own'), '[]'::jsonb),
        'groups', coalesce((
            select jsonb_agg(jsonb_build_object('group', m.profile_id, 'role', m.role) order by m.profile_id)
            from iamb.membership m where m.user_id = uid and m.deleted_at is null), '[]'::jsonb));
end $$;

-- ---------------------------------------------------------------------------------------------
-- Conflicts (§5.6, C20)
-- ---------------------------------------------------------------------------------------------

create or replace function api.conflicts(device uuid, protocol integer)
returns jsonb language plpgsql volatile security invoker set search_path = '' set statement_timeout = '8s' as $$
declare
    e jsonb := iamb_private.enter(device, protocol, 'conflicts');
begin
    if e ->> 'error' is not null then return e; end if;
    return jsonb_build_object('error', null, 'epoch', e -> 'epoch', 'conflicts', coalesce((
        select jsonb_agg(jsonb_build_object('id', c.id, 'entity', c.entity, 'row_id', c.row_id, 'kind', c.kind,
                                            'state', c.state, 'created_at', c.created_at, 'winner_rev', c.winner_rev)
                         order by c.created_at desc)
        from iamb.conflict c where c.state = 'open'), '[]'::jsonb));
end $$;

-- A conflict with the current row and every version it offers (G5: a history version is returned
-- only to a party of the conflict who may still read the row). A version the reader wrote also
-- names its device; another person's names the person only.
create or replace function api.get_conflict(device uuid, protocol integer, id uuid)
returns jsonb language plpgsql volatile security definer set search_path = '' set statement_timeout = '8s' as $$
declare
    e   jsonb := iamb_private.enter(device, protocol, 'get_conflict');
    uid uuid := auth.uid();
    c   iamb.conflict;
begin
    if e ->> 'error' is not null then return e; end if;
    select * into c from iamb.conflict x where x.id = get_conflict.id;
    if c.id is null or not uid = any (c.parties) then
        return jsonb_build_object('error', 'not_found', 'epoch', e -> 'epoch');
    end if;
    if not iamb_private.can_read(uid, c.entity, c.row_id) then
        return jsonb_build_object('error', 'forbidden', 'epoch', e -> 'epoch');
    end if;
    return jsonb_build_object('error', null, 'epoch', e -> 'epoch',
        'conflict', jsonb_build_object('id', c.id, 'entity', c.entity, 'row_id', c.row_id, 'kind', c.kind,
                                       'state', c.state, 'created_at', c.created_at, 'winner_rev', c.winner_rev,
                                       'winner_user', c.winner_user,
                                       'winner_device', case when c.winner_user = uid then c.winner_device end),
        'current', iamb_private.readable_row(uid, c.entity, c.row_id),
        'versions', coalesce((
            select jsonb_agg(jsonb_build_object(
                       'rev', v.rev, 'user', v.user_id,
                       'display_name', (select up.display_name from iamb.user_profile up where up.user_id = v.user_id),
                       'device', case when v.user_id = uid then v.device_id end,
                       'taken_over', v.taken_over,
                       'written_at', h.written_at, 'deleted', h.deleted,
                       'row', case when h.row is not null then iamb_private.wire_of(c.entity, h.row) end,
                       'origin', case when c.entity = 'snippet' and h.row is not null then
                                    jsonb_build_object('content_by', h.row -> 'content_by', 'content_at', h.row -> 'content_at',
                                                       'created_rev', h.row -> 'created_rev') end)
                   order by v.rev desc)
            from iamb.conflict_version v
            left join iamb.history h on h.entity = c.entity and h.row_id = c.row_id and h.rev = v.rev
            where v.conflict_id = c.id), '[]'::jsonb));
end $$;

-- Closes a conflict ("keep") or restores one of the versions it offers ("restore", with `rev`):
-- the version is written as a new revision by the caller, keeps the author its content had,
-- records no conflict, and the conflict is resolved (C20). A deleted folder's version deletes at
-- most `cascade_max` rows per call and answers `partial` until the subtree is gone.
create or replace function iamb_private.acct_resolve_conflict(u uuid, dev uuid, p_id uuid, p_action text, p_rev bigint)
returns jsonb
language plpgsql volatile set search_path = '' as $$
declare
    c      iamb.conflict;
    h      iamb.history;
    cur    jsonb;
    nr     jsonb;
    r      bigint;
    before uuid[];
    now_ms bigint := iamb_private.now_ms();
    res    jsonb;
    target uuid;
    st     iamb.server_state;
begin
    select * into st from iamb.server_state;
    select * into c from iamb.conflict where id = p_id;
    if c.id is null or not u = any (c.parties) then
        return iamb_private.err('not_found');
    end if;
    perform iamb_private.lock_profiles(array[c.profile_id]);
    select * into c from iamb.conflict where id = p_id;
    if c.state <> 'open' then
        return jsonb_build_object('error', null, 'state', c.state);
    end if;
    if p_action = 'keep' then
        update iamb.conflict set state = 'resolved', resolved_at = now_ms, resolved_by = u, resolution = 'keep' where id = p_id;
        perform iamb_private.queue(c.parties, 'conflict', c.entity, c.row_id, null, c.profile_id, null, jsonb_build_object('conflict', c.id));
        return jsonb_build_object('error', null, 'state', 'resolved');
    elsif p_action <> 'restore' or not exists (select 1 from iamb.conflict_version v where v.conflict_id = p_id and v.rev = p_rev) then
        return iamb_private.err('invalid');
    end if;
    select * into h from iamb.history where entity = c.entity and row_id = c.row_id and rev = p_rev;
    if h.rev is null then
        return iamb_private.err('gone');
    end if;
    cur := iamb_private.get_row(c.entity, c.row_id);
    if cur is null then
        return iamb_private.err('gone');
    end if;
    -- The right to write the row now (C12).
    if (c.entity = 'snippet' and not (iamb_private.can_edit(u, (cur ->> 'folder_id')::uuid)
                                      and iamb_private.can_edit(u, (h.row ->> 'folder_id')::uuid)))
       or (c.entity = 'tag' and not iamb_private.manages_profile(u, c.profile_id))
       or (c.entity = 'folder' and not (
              (cur ->> 'parent_id' is null and iamb_private.manages_profile(u, c.profile_id))
              or (cur ->> 'parent_id' is not null and iamb_private.can_edit(u, (cur ->> 'parent_id')::uuid)
                  and (not iamb_private.is_share_root(c.row_id) or iamb_private.can_manage(u, c.row_id))))) then
        return iamb_private.err('forbidden');
    end if;
    if h.deleted then
        if c.entity = 'folder' then
            res := iamb_private.push_folder_delete(u, dev,
                       jsonb_build_object('op_id', null, 'entity', 'folder', 'id', c.row_id, 'kind', 'delete',
                                          'base_rev', (cur ->> 'rev')::bigint, 'cascade', '[]'::jsonb),
                       st.cascade_max);
            if res ->> 'status' = 'partial' then
                return iamb_private.err('partial');
            elsif res ->> 'status' = 'rejected' then
                return iamb_private.err('forbidden');
            end if;
        elsif (cur ->> 'deleted_at') is null then
            before := iamb_private.readers(c.entity, c.row_id);
            r := iamb_private.next_rev(array[c.profile_id]);
            nr := cur || jsonb_build_object('deleted_at', now_ms, 'rev', r, 'updated_at', now_ms, 'updated_by', u,
                                            'updated_device', dev, 'updated_op', null);
            perform iamb_private.write_row(u, dev, null, c.entity, cur, nr, r, before, false, c.profile_id);
        end if;
    else
        if c.entity = 'folder' then
            target := iamb_private.try_uuid(h.row ->> 'parent_id');
            if target is not null and (iamb_private.is_in_subtree(target, c.row_id)
                                       or iamb_private.folder_depth(target) + iamb_private.subtree_height(c.row_id) > 4) then
                target := iamb_private.try_uuid(cur ->> 'parent_id');
            end if;
        elsif c.entity = 'snippet' then
            target := (h.row ->> 'folder_id')::uuid;
        end if;
        if target is not null and exists (select 1 from iamb.folder where id = target and deleted_at is not null) then
            if not iamb_private.restore_chain(u, dev, null, target) then
                return iamb_private.err('forbidden');
            end if;
        end if;
        before := iamb_private.readers(c.entity, c.row_id);
        r := iamb_private.next_rev(array[c.profile_id]);
        nr := cur || iamb_private.pick(h.row, iamb_private.fields(c.entity) || case when c.entity = 'snippet'
                                                    then array['content_by', 'content_at'] else '{}'::text[] end)
              || jsonb_build_object('deleted_at', null, 'rev', r, 'updated_at', now_ms, 'updated_by', u,
                                    'updated_device', dev, 'updated_op', null);
        if c.entity = 'folder' then
            nr := nr || jsonb_build_object('parent_id', target);
        end if;
        perform iamb_private.write_row(u, dev, null, c.entity, cur, nr, r, before, c.entity = 'folder', c.profile_id);
    end if;
    update iamb.conflict set state = 'resolved', resolved_at = now_ms, resolved_by = u,
        resolution = 'restore:' || p_rev::text where id = p_id;
    perform iamb_private.queue(c.parties, 'conflict', c.entity, c.row_id, null, c.profile_id, null, jsonb_build_object('conflict', c.id));
    return jsonb_build_object('error', null, 'state', 'resolved');
end
$$;

create or replace function api.resolve_conflict(device uuid, protocol integer, id uuid, action text, rev bigint default null)
returns jsonb language plpgsql volatile security definer set search_path = '' set statement_timeout = '20s' as $$
declare e jsonb := iamb_private.enter(device, protocol, 'resolve_conflict');
begin
    if e ->> 'error' is not null then return e; end if;
    return iamb_private.finish(e, iamb_private.acct_resolve_conflict(auth.uid(), device, id, action, rev));
end $$;

-- ---------------------------------------------------------------------------------------------
-- Attachment files (§6.11)
-- ---------------------------------------------------------------------------------------------

create or replace function iamb_private.acct_attachment_upload_begin(u uuid, p_profile uuid, p_sha256 text, p_size bigint, p_mime text)
returns jsonb
language plpgsql volatile set search_path = '' as $$
declare
    st     iamb.server_state;
    lim    bigint;
    used   bigint;
    uid_   uuid;
    now_ms bigint := iamb_private.now_ms();
begin
    select * into st from iamb.server_state;
    if p_profile is null or coalesce(p_sha256, '') !~ '^[0-9a-f]{64}$' or p_size is null or p_size < 0
       or p_mime is null or octet_length(p_mime) > 127 then
        return iamb_private.err('invalid');
    end if;
    perform iamb_private.lock_profiles(array[p_profile]);
    if not iamb_private.edits_in_profile(u, p_profile) then
        return iamb_private.err('forbidden');
    end if;
    if p_size > st.attachment_bytes_max then
        return iamb_private.err('too_large');
    end if;
    -- Judged by what the caller may read, so a member learns nothing of the owner's files.
    if iamb_private.can_read_blob(u, p_profile, p_sha256) then
        return jsonb_build_object('error', 'exists');
    end if;
    lim := iamb_private.profile_limit(p_profile, 'storage_bytes_max');
    select coalesce((select sum(b.size) from iamb.attachment_blob b where b.profile_id = p_profile), 0)
         + coalesce((select sum(x.size) from iamb.attachment_upload x where x.profile_id = p_profile and x.expires_at > now_ms), 0)
    into used;
    if lim is not null and used + p_size > lim then
        return iamb_private.err('plan_limit');
    end if;
    uid_ := iamb_private.uuid7();
    insert into iamb.attachment_upload (id, profile_id, sha256, size, mime, uploaded_by, created_at, expires_at)
    values (uid_, p_profile, p_sha256, p_size, p_mime, u, now_ms, now_ms + 86400000);
    return jsonb_build_object('error', null, 'upload', uid_, 'object', p_profile::text || '/upload/' || uid_::text,
                              'expires_at', now_ms + 86400000);
end
$$;

create or replace function api.attachment_upload_begin(device uuid, protocol integer, profile uuid, sha256 text, size bigint, mime text)
returns jsonb language plpgsql volatile security definer set search_path = '' set statement_timeout = '8s' as $$
declare e jsonb := iamb_private.enter(device, protocol, 'attachment_upload_begin');
begin
    if e ->> 'error' is not null then return e; end if;
    return iamb_private.finish(e, iamb_private.acct_attachment_upload_begin(auth.uid(), profile, sha256, size, mime));
end $$;

-- `api_service.attachment_verified`: called by the edge function `attachment-commit` with the
-- service key after it has hashed the uploaded object and copied it to `<profile>/<sha256>`.
create or replace function api_service.attachment_verified(id uuid, sha256 text, size bigint)
returns jsonb language plpgsql volatile security definer set search_path = '' set statement_timeout = '20s' as $$
#variable_conflict use_column
declare
    up      iamb.attachment_upload;
    now_ms  bigint := iamb_private.now_ms();
    readers uuid[];
begin
    select * into up from iamb.attachment_upload x where x.id = attachment_verified.id;
    if up.id is null then
        -- A commit that was cut off and is called again finds what is already done.
        return jsonb_build_object('error', null, 'done', true);
    end if;
    perform iamb_private.lock_profiles(array[up.profile_id]);
    select * into up from iamb.attachment_upload x where x.id = attachment_verified.id;
    if up.id is null then
        return jsonb_build_object('error', null, 'done', true);
    end if;
    if up.sha256 <> attachment_verified.sha256 or up.size <> attachment_verified.size then
        delete from iamb.attachment_upload x where x.id = up.id;
        return jsonb_build_object('error', 'hash_mismatch');
    end if;
    insert into iamb.attachment_blob (profile_id, sha256, size, mime, verified_at)
    values (up.profile_id, up.sha256, up.size, up.mime, now_ms)
    on conflict (profile_id, sha256) do nothing;
    -- The attachment rows of that profile and hash that the uploader created become backed (C19).
    update iamb.attachment a set backed = true
    where a.profile_id = up.profile_id and a.sha256 = up.sha256 and a.updated_by = up.uploaded_by and not a.backed;
    delete from iamb.attachment_upload x where x.id = up.id;
    select coalesce(array_agg(distinct cand), '{}') into readers from (
        select p.owner_user_id as cand from iamb.profile p where p.id = up.profile_id and p.kind = 'personal'
        union all
        select m.user_id from iamb.membership m where m.profile_id = up.profile_id and m.deleted_at is null
        union all
        select fa.user_id from iamb.attachment a
        join iamb.snippet_attachment l on l.attachment_id = a.id and l.deleted_at is null
        join iamb.snippet s on s.id = l.snippet_id and s.deleted_at is null
        join iamb.folder_access fa on fa.folder_id = s.folder_id
        where a.profile_id = up.profile_id and a.sha256 = up.sha256
    ) x where iamb_private.can_read_blob(cand, up.profile_id, up.sha256);
    perform iamb_private.queue(readers, 'blob', null, null, null, up.profile_id, up.sha256, null);
    perform iamb_private.feed_flush();
    return jsonb_build_object('error', null, 'done', true);
end $$;

-- `api_service.delete_account_data`: called by the edge function `account-delete` with the
-- service key, after `api.check` with the caller's token (§11.1). Refused while the user owns a
-- group with other members. Marks every device revoked, deletes the groups the user owns, ends
-- their memberships, and removes their personal rows without the 30 days.
create or replace function api_service.delete_account_data("user" uuid)
returns jsonb language plpgsql volatile security definer set search_path = '' set statement_timeout = '60s' as $$
declare
    u       uuid := delete_account_data."user";
    pp      uuid := iamb_private.personal_profile(u);
    groups  uuid[];
    sharers uuid[];
    g       record;
    now_ms  bigint := iamb_private.now_ms();
begin
    if exists (select 1 from iamb.profile p where p.owner_user_id = u and p.kind <> 'personal' and p.deleted_at is null
               and exists (select 1 from iamb.membership m where m.profile_id = p.id and m.user_id <> u and m.deleted_at is null)) then
        return jsonb_build_object('error', 'owns_group');
    end if;
    -- C24: the account's device rows, its groups, then the personal profiles (its own and those
    -- that share into its groups).
    perform 1 from iamb.device where user_id = u order by id for update;
    update iamb.device set revoked_at = coalesce(revoked_at, now_ms) where user_id = u;
    select coalesce(array_agg(distinct m.profile_id), '{}') into groups from iamb.membership m
    where m.user_id = u and m.deleted_at is null;
    perform iamb_private.lock_profiles(groups);
    select coalesce(array_agg(distinct x), '{}') into sharers from (
        select unnest(iamb_private.sharing_profiles(gg)) as x from unnest(groups) gg
        union select pp
    ) y where x is not null;
    perform iamb_private.lock_profiles(sharers);
    for g in select p.id, p.owner_user_id from iamb.profile p where p.id = any (groups) order by p.id loop
        if g.owner_user_id = u then
            perform iamb_private.acct_delete_group(u, null, g.id);
        else
            perform iamb_private.end_membership(u, null, g.id, u, 'delete_account');
        end if;
    end loop;
    if pp is not null then
        delete from iamb.folder_access where profile_id = pp;
        delete from iamb.feed f where f.profile_id = pp;
        delete from iamb.conflict where profile_id = pp;
        delete from iamb.history h where h.row_id in (
            select id from iamb.folder where profile_id = pp union all select id from iamb.snippet where profile_id = pp
            union all select id from iamb.tag where profile_id = pp union all select id from iamb.attachment where profile_id = pp
            union all select l.id from iamb.snippet_tag l join iamb.snippet s on s.id = l.snippet_id where s.profile_id = pp
            union all select l.id from iamb.snippet_attachment l join iamb.snippet s on s.id = l.snippet_id where s.profile_id = pp
            union all select s.id from iamb.folder_share s join iamb.folder f on f.id = s.folder_id where f.profile_id = pp
            union all select pp);
        delete from iamb.snippet_tag l using iamb.snippet s where s.id = l.snippet_id and s.profile_id = pp;
        delete from iamb.snippet_attachment l using iamb.snippet s where s.id = l.snippet_id and s.profile_id = pp;
        delete from iamb.snippet where profile_id = pp;
        delete from iamb.folder_share s using iamb.folder f where f.id = s.folder_id and f.profile_id = pp;
        update iamb.folder set parent_id = null where profile_id = pp;
        delete from iamb.folder where profile_id = pp;
        delete from iamb.tag where profile_id = pp;
        delete from iamb.attachment where profile_id = pp;
        delete from iamb.attachment_upload where profile_id = pp;
        delete from iamb.attachment_blob where profile_id = pp;
        delete from iamb.epoch_profile where profile_id = pp;
        delete from iamb.profile where id = pp;
    end if;
    delete from iamb.feed where user_id = u;
    delete from iamb.feed_head where user_id = u;
    delete from iamb.user_profile where user_id = u;
    delete from iamb.feature_override where user_id = u;
    perform iamb_private.log('delete_account', jsonb_build_object('user', u, 'answer', 'ok'));
    perform iamb_private.feed_flush();
    return jsonb_build_object('error', null);
end $$;

revoke execute on all functions in schema api_service from public, anon, authenticated, service_role;
grant execute on function api_service.attachment_verified(uuid, text, bigint), api_service.delete_account_data(uuid) to service_role;
