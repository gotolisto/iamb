-- Iamb server, migration 13: conflicts after the security review of task 4.3
-- (docs/reviews/4.3-security-review.md), findings 1, 2 and 4; the design change is in
-- docs/sync-design.md (C3, C20, §5.6) and ADR-0030.
--
--  1. A conflict offers a version only to a party who wrote it or could read it while it was the
--     row: with an access entry for the folder that version was in which began before the version
--     was replaced (`iamb_private.version_readable`, migration 12). A version from the owner's
--     private folder, or from before the folder was shared, is not offered to a member.
--  2. A party takes part only while they can read the row: the policy on `conflict`, the list,
--     `get_conflict`, the feed entries and `resolve_conflict` (also "keep") all require it. A
--     removed member learns of nothing and closes nothing.
--  4. "Restore this version" is judged as a push of that version by that user would be: it is
--     made through `push_op`, the one code path that knows §4.3, C7 and C8.
--
-- `get_conflict` becomes an invoker function (review §6): the conflict, its versions' authors and
-- the current row are read under the caller's policies, and one small definer helper,
-- `iamb_private.conflict_history`, returns the history rows of the versions on offer to the caller.

do $$
begin
    if current_user <> 'postgres' then
        raise exception 'iamb migrations must run as postgres, not %', current_user;
    end if;
end
$$;

-- A conflict is read by a party who can read its row (the policy of the row's own table decides,
-- as the caller). `conflict_version` follows through its own policy, which reads `conflict`.
drop policy if exists conflict_read on iamb.conflict;
create policy conflict_read on iamb.conflict for select to authenticated using (
    (select iamb_private.session_ok())
    and (select auth.uid()) = any (parties)
    and ((entity = 'snippet' and exists (select 1 from iamb.snippet s where s.id = conflict.row_id))
         or (entity = 'folder' and exists (select 1 from iamb.folder f where f.id = conflict.row_id))
         or (entity = 'tag' and exists (select 1 from iamb.tag t where t.id = conflict.row_id)))
);

-- The history rows of the versions a conflict offers to the caller (G5): the caller is a party,
-- can read the row now, and wrote the version or could read it while it was the row. In the wire
-- form; a folder's parent the caller cannot read is left out, as in every other read. The one
-- definer part of `get_conflict`; it takes the caller from the token and answers nothing to a
-- session Iamb does not accept.
create or replace function iamb_private.conflict_history(p_conflict uuid)
returns table (rev bigint, written_at bigint, deleted boolean, "row" jsonb, origin jsonb)
language sql stable security definer set search_path = pg_catalog, pg_temp as $$
    select h.rev, h.written_at, h.deleted,
           case when c.entity = 'folder' and h.row ->> 'parent_id' is not null
                     and not exists (select 1 from iamb.folder_access fa
                                     where fa.user_id = auth.uid()
                                       and fa.folder_id = iamb_private.try_uuid(h.row ->> 'parent_id'))
                then iamb_private.wire_of(c.entity, h.row) || jsonb_build_object('parent_id', null)
                else iamb_private.wire_of(c.entity, h.row) end,
           case when c.entity = 'snippet' then
               jsonb_build_object('content_by', h.row -> 'content_by', 'content_at', h.row -> 'content_at',
                                  'created_rev', h.row -> 'created_rev') end
    from iamb.conflict c
    join iamb.conflict_version v on v.conflict_id = c.id
    join iamb.history h on h.entity = c.entity and h.row_id = c.row_id and h.rev = v.rev
    where c.id = p_conflict
      and iamb_private.session_ok()
      and auth.uid() = any (c.parties)
      and iamb_private.can_read(auth.uid(), c.entity, c.row_id)
      and iamb_private.version_readable(auth.uid(), c.entity, h.row, h.replaced_by_rev)
$$;

-- A conflict with the current row and the versions it offers to the caller. A version the reader
-- wrote also names its device; another person's names the person only. A conflict the caller is
-- no party of, and one on a row the caller cannot read, answer the same: `not_found`.
create or replace function api.get_conflict(device uuid, protocol integer, id uuid)
returns jsonb language plpgsql volatile security invoker set search_path = pg_catalog, pg_temp set statement_timeout = '8s' as $$
declare
    e   jsonb := iamb_private.enter(device, protocol, 'get_conflict');
    uid uuid := auth.uid();
    c   iamb.conflict;
begin
    if e ->> 'error' is not null then return e; end if;
    select * into c from iamb.conflict x where x.id = get_conflict.id;
    if c.id is null then
        return jsonb_build_object('error', 'not_found', 'epoch', e -> 'epoch');
    end if;
    return jsonb_build_object('error', null, 'epoch', e -> 'epoch',
        'conflict', jsonb_build_object('id', c.id, 'entity', c.entity, 'row_id', c.row_id, 'kind', c.kind,
                                       'state', c.state, 'created_at', c.created_at, 'winner_rev', c.winner_rev,
                                       'winner_user', c.winner_user,
                                       'winner_device', case when c.winner_user = uid then c.winner_device end),
        'current', (select jsonb_build_object('row', w.row, 'origin', w.origin)
                    from iamb.wire_rows w where w.entity = c.entity and w.id = c.row_id),
        'versions', coalesce((
            select jsonb_agg(jsonb_build_object(
                       'rev', v.rev, 'user', v.user_id,
                       'display_name', (select up.display_name from iamb.user_profile up where up.user_id = v.user_id),
                       'device', case when v.user_id = uid then v.device_id end,
                       'taken_over', v.taken_over,
                       'written_at', h.written_at, 'deleted', h.deleted,
                       'row', h.row, 'origin', h.origin)
                   order by v.rev desc)
            from iamb.conflict_version v
            join iamb_private.conflict_history(c.id) h on h.rev = v.rev
            where v.conflict_id = c.id), '[]'::jsonb));
end $$;

-- Closes a conflict ("keep") or restores one of the versions it offers to the caller ("restore",
-- with `rev`). Both need a party who can read the row. A restore is a write of that version by the
-- caller, judged exactly as a push of it would be (§4.3, C7, C8, C12): it goes through `push_op`
-- with the row's current revision as its base, so it records no conflict; the restored content
-- keeps the author it had (C17). A deleted folder's version deletes at most `cascade_max` rows per
-- call and answers `partial` until the subtree is gone.
create or replace function iamb_private.acct_resolve_conflict(u uuid, dev uuid, p_id uuid, p_action text, p_rev bigint)
returns jsonb
language plpgsql volatile set search_path = pg_catalog, pg_temp as $$
declare
    c      iamb.conflict;
    h      iamb.history;
    cur    jsonb;
    op     jsonb;
    res    jsonb;
    rd     uuid[];
    now_ms bigint := iamb_private.now_ms();
    st     iamb.server_state;
begin
    select * into st from iamb.server_state;
    select * into c from iamb.conflict where id = p_id;
    -- Not a party, or the row is not the caller's to read: one answer, before any lock.
    if c.id is null or not u = any (c.parties) or not iamb_private.can_read(u, c.entity, c.row_id) then
        return iamb_private.err('not_found');
    end if;
    perform iamb_private.lock_profiles(array[c.profile_id]);
    select * into c from iamb.conflict where id = p_id;
    rd := iamb_private.readers(c.entity, c.row_id);
    if c.id is null or not u = any (c.parties) or not u = any (rd) then
        return iamb_private.err('not_found');
    end if;
    if c.state <> 'open' then
        return jsonb_build_object('error', null, 'state', c.state);
    end if;
    if p_action = 'keep' then
        update iamb.conflict set state = 'resolved', resolved_at = now_ms, resolved_by = u, resolution = 'keep' where id = p_id;
        perform iamb_private.queue(array(select x from unnest(c.parties) x where x = any (rd)),
                                   'conflict', c.entity, c.row_id, null, c.profile_id, null, jsonb_build_object('conflict', c.id));
        return jsonb_build_object('error', null, 'state', 'resolved');
    elsif p_action is distinct from 'restore' or p_rev is null
          or not exists (select 1 from iamb.conflict_version v where v.conflict_id = p_id and v.rev = p_rev) then
        return iamb_private.err('invalid');
    end if;
    select * into h from iamb.history where entity = c.entity and row_id = c.row_id and rev = p_rev;
    if h.rev is null then
        return iamb_private.err('gone');
    end if;
    -- A version that is not on offer to the caller is, for the caller, not a version of this
    -- conflict (finding 1).
    if not iamb_private.version_readable(u, c.entity, h.row, h.replaced_by_rev) then
        return iamb_private.err('invalid');
    end if;
    cur := iamb_private.get_row(c.entity, c.row_id);
    if cur is null then
        return iamb_private.err('gone');
    end if;

    -- The restore as an op: the version's fields in the wire form, based on the row as it is.
    op := jsonb_build_object('op_id', iamb_private.uuid7(), 'entity', c.entity, 'id', c.row_id,
                             'base_rev', (cur ->> 'rev')::bigint,
                             'kind', case when h.deleted then 'delete' else 'put' end);
    if h.deleted then
        op := op || jsonb_build_object('cascade', '[]'::jsonb);
    else
        op := op || jsonb_build_object('row', iamb_private.wire_of(c.entity, h.row));
    end if;
    res := iamb_private.push_op(u, dev, op, c.profile_id, st.cascade_max, '{}',
               case when c.entity = 'snippet' and not h.deleted then
                   jsonb_build_object('content_by', h.row -> 'content_by', 'content_at', h.row -> 'content_at') end);
    if res ->> 'status' = 'partial' then
        return iamb_private.err('partial');
    elsif res ->> 'status' = 'rejected' then
        return iamb_private.err('forbidden');
    elsif res ->> 'status' not in ('applied', 'unchanged') then
        return iamb_private.err('invalid');
    end if;
    update iamb.conflict set state = 'resolved', resolved_at = now_ms, resolved_by = u,
        resolution = 'restore:' || p_rev::text where id = p_id;
    rd := iamb_private.readers(c.entity, c.row_id);
    perform iamb_private.queue(array(select x from unnest(c.parties) x where x = any (rd)),
                               'conflict', c.entity, c.row_id, null, c.profile_id, null, jsonb_build_object('conflict', c.id));
    return jsonb_build_object('error', null, 'state', 'resolved');
end
$$;

revoke execute on all functions in schema iamb_private from public, anon, authenticated, service_role;
grant execute on function
    iamb_private.session_ok(),
    iamb_private.member_of(uuid),
    iamb_private.role_in(uuid),
    iamb_private.shares_group_with(uuid),
    iamb_private.own_address(),
    iamb_private.enter(uuid, integer, text),
    iamb_private.position(uuid, text, text, bigint),
    iamb_private.conflict_history(uuid)
to authenticated;
revoke execute on function api.get_conflict(uuid, integer, uuid) from public, anon, service_role;
grant execute on function api.get_conflict(uuid, integer, uuid) to authenticated;
