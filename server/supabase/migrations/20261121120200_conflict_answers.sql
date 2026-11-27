-- Iamb server, migration 21: a party's answer to a conflict closes it for the people who are
-- offered no more than that party (docs/reviews/4.3-security-reverify.md, N6).
--
-- Since migration 13 a party can be offered fewer versions than a conflict holds (ADR-0031). But
-- "Keep the current version" by any party still closed the conflict for everybody: a member whose
-- stale write had taken over the owner's open conflict about a private version, which he is not
-- offered, closed it, and she could no longer restore that version.
--
-- Now a party's answer ("keep", or a restore) closes the conflict for everybody only when that
-- party is offered every version the conflict holds. Otherwise it closes the conflict for that
-- party alone (`conflict.closed_for`): it leaves their list, reads as resolved for them, and
-- stays open for the others with every version it offered them. Once nobody who takes part is
-- left for whom it is open, it is closed.

do $$
begin
    if current_user <> 'postgres' then
        raise exception 'iamb migrations must run as postgres, not %', current_user;
    end if;
end
$$;

alter table iamb.conflict add column if not exists closed_for uuid[] not null default '{}';

-- Whether the user is offered every version the conflict holds (a version whose history row is
-- gone is on offer to nobody).
create or replace function iamb_private.offered_all(u uuid, p_conflict uuid) returns boolean
language sql stable set search_path = pg_catalog, pg_temp as $$
    select not exists (
        select 1 from iamb.conflict c
        join iamb.conflict_version v on v.conflict_id = c.id
        join iamb.history h on h.entity = c.entity and h.row_id = c.row_id and h.rev = v.rev
        where c.id = p_conflict
          and not iamb_private.version_readable(u, c.entity, h.row, h.replaced_by_rev))
$$;

-- Records a party's answer: for everybody when the party is offered every version, and otherwise
-- for the party alone; closes the conflict when it is open for nobody who takes part any more.
-- Tells the feeds of those for whom something changed. `p_all` was decided before the answer's
-- own write, if it had one.
create or replace function iamb_private.conflict_answered(c iamb.conflict, u uuid, p_resolution text, p_all boolean)
returns void
language plpgsql volatile set search_path = pg_catalog, pg_temp as $$
declare
    rd     uuid[] := iamb_private.readers(c.entity, c.row_id);
    closed uuid[] := array(select distinct x from unnest(c.closed_for || u) x);
    now_ms bigint := iamb_private.now_ms();
begin
    if p_all or not exists (select 1 from unnest(c.parties) x where x = any (rd) and not x = any (closed)) then
        update iamb.conflict set state = 'resolved', resolved_at = now_ms, resolved_by = u, resolution = p_resolution
        where id = c.id;
        perform iamb_private.queue(array(select x from unnest(c.parties) x where x = any (rd)),
                                   'conflict', c.entity, c.row_id, null, c.profile_id, null, jsonb_build_object('conflict', c.id));
    else
        update iamb.conflict set closed_for = closed where id = c.id;
        perform iamb_private.queue(array[u], 'conflict', c.entity, c.row_id, null, c.profile_id, null,
                                   jsonb_build_object('conflict', c.id));
    end if;
end
$$;

-- Closes a conflict ("keep") or restores one of the versions it offers to the caller ("restore",
-- with `rev`); see migration 13. The answer closes the conflict for everybody when the caller is
-- offered every version it holds, and for the caller alone otherwise (N6).
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
    v_all  boolean;
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
    elsif u = any (c.closed_for) then
        -- Closed for this party by an earlier answer of theirs (N6).
        return jsonb_build_object('error', null, 'state', 'resolved');
    end if;
    -- An answer closes the conflict for everybody only when the party is offered every version
    -- it holds (N6).
    v_all := iamb_private.offered_all(u, p_id);
    if p_action = 'keep' then
        perform iamb_private.conflict_answered(c, u, 'keep', v_all);
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
    perform iamb_private.conflict_answered(c, u, 'restore:' || p_rev::text, v_all);
    return jsonb_build_object('error', null, 'state', 'resolved');
end
$$;

-- The open conflicts the caller takes part in (the policy decides that), without those the
-- caller has answered for themselves.
create or replace function api.conflicts(device uuid, protocol integer)
returns jsonb language plpgsql volatile security invoker set search_path = pg_catalog, pg_temp set statement_timeout = '8s' as $$
declare
    e jsonb := iamb_private.enter(device, protocol, 'conflicts');
begin
    if e ->> 'error' is not null then return e; end if;
    return jsonb_build_object('error', null, 'epoch', e -> 'epoch', 'conflicts', coalesce((
        select jsonb_agg(jsonb_build_object('id', c.id, 'entity', c.entity, 'row_id', c.row_id, 'kind', c.kind,
                                            'state', c.state, 'created_at', c.created_at, 'winner_rev', c.winner_rev)
                         order by c.created_at desc)
        from iamb.conflict c where c.state = 'open' and not auth.uid() = any (c.closed_for)), '[]'::jsonb));
end $$;

-- A conflict with the current row and the versions it offers to the caller; see migration 13. A
-- conflict the caller has answered for themselves reads as resolved.
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
                                       'state', case when c.state = 'open' and uid = any (c.closed_for) then 'resolved' else c.state end,
                                       'created_at', c.created_at, 'winner_rev', c.winner_rev,
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

revoke execute on function iamb_private.offered_all(uuid, uuid),
                           iamb_private.conflict_answered(iamb.conflict, uuid, text, boolean),
                           iamb_private.acct_resolve_conflict(uuid, uuid, uuid, text, bigint)
    from public, anon, authenticated, service_role;
revoke execute on function api.conflicts(uuid, integer), api.get_conflict(uuid, integer, uuid) from public, anon, service_role;
grant execute on function api.conflicts(uuid, integer), api.get_conflict(uuid, integer, uuid) to authenticated;
