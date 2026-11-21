-- Iamb server, migration 5: `hello` and the account calls of docs/sync-design.md §5.14 (C16,
-- C25). Each call of `api` is a thin wrapper: `iamb_private.enter` first, then the call's core in
-- `iamb_private.acct_*`, which takes the caller as an argument so that the operator's restore
-- script can make a logged call again as its caller (C18, `replay_call`). Only the wrappers are
-- reachable by devices, and they take the caller from the token alone (G17).

do $$
begin
    if current_user <> 'postgres' then
        raise exception 'iamb migrations must run as postgres, not %', current_user;
    end if;
end
$$;

-- A UUID of version 7 for the rows the server makes (§3.4).
create or replace function iamb_private.uuid7() returns uuid
language plpgsql volatile set search_path = '' as $$
declare
    ms bigint := iamb_private.now_ms();
    b  bytea := uuid_send(gen_random_uuid());
    i  integer;
begin
    for i in 0..5 loop
        b := set_byte(b, i, ((ms >> (40 - 8 * i)) & 255)::integer);
    end loop;
    b := set_byte(b, 6, (get_byte(b, 6) & 15) | 112);
    b := set_byte(b, 8, (get_byte(b, 8) & 63) | 128);
    return encode(b, 'hex')::uuid;
end
$$;

create or replace function iamb_private.personal_profile(u uuid) returns uuid
language sql stable set search_path = '' as $$
    select id from iamb.profile where owner_user_id = u and kind = 'personal'
$$;

-- The personal profiles that hold a folder shared with the group by a live share.
create or replace function iamb_private.sharing_profiles(g uuid) returns uuid[]
language sql stable set search_path = '' as $$
    select coalesce(array_agg(distinct f.profile_id), '{}')
    from iamb.folder_share s join iamb.folder f on f.id = s.folder_id
    where s.profile_id = g and s.deleted_at is null
$$;

-- The caller's role in a live group, or null.
create or replace function iamb_private.group_role(u uuid, g uuid) returns text
language sql stable set search_path = '' as $$
    select m.role from iamb.membership m join iamb.profile p on p.id = m.profile_id
    where m.profile_id = g and m.user_id = u and m.deleted_at is null
      and p.deleted_at is null and p.kind <> 'personal'
$$;

create or replace function iamb_private.err(p_code text) returns jsonb
language sql immutable set search_path = '' as $$
    select jsonb_build_object('error', p_code)
$$;

-- Ends a share as a write by `u` (C11, C16): history, a new revision, feed entries for everyone
-- who could read it, and a line in the function log for the restore script (C18).
create or replace function iamb_private.end_share(u uuid, dev uuid, op uuid, p_share uuid, p_profiles uuid[], p_reason text)
returns bigint
language plpgsql volatile set search_path = '' as $$
declare
    old    jsonb := iamb_private.get_row('folder_share', p_share);
    before uuid[];
    r      bigint;
    now_ms bigint := iamb_private.now_ms();
begin
    if old is null or (old ->> 'deleted_at') is not null then
        return null;
    end if;
    before := iamb_private.readers('folder_share', p_share);
    r := iamb_private.next_rev(p_profiles);
    perform iamb_private.keep_history('folder_share', old, r);
    update iamb.folder_share set deleted_at = now_ms, updated_at = now_ms, updated_by = u,
        updated_device = dev, updated_op = op, rev = r
    where id = p_share;
    perform iamb_private.mark_written('folder_share', p_share, false);
    perform iamb_private.queue(before || iamb_private.readers('folder_share', p_share), 'row', 'folder_share',
                               p_share, null, null, null, null);
    perform iamb_private.log('share_deleted', jsonb_build_object(
        'user', u, 'device', dev, 'share', p_share, 'folder', old ->> 'folder_id',
        'group', old ->> 'profile_id', 'rev', r, 'reason', p_reason));
    return r;
end
$$;

-- Writes a membership's new state (role or delete) with history and feed entries.
create or replace function iamb_private.write_membership(u uuid, dev uuid, p_id uuid, p_role text, p_delete boolean,
                                                         p_profiles uuid[]) returns bigint
language plpgsql volatile set search_path = '' as $$
declare
    old    jsonb := iamb_private.get_row('membership', p_id);
    before uuid[] := iamb_private.readers('membership', p_id);
    r      bigint := iamb_private.next_rev(p_profiles);
    now_ms bigint := iamb_private.now_ms();
begin
    perform iamb_private.keep_history('membership', old, r);
    update iamb.membership set role = coalesce(p_role, role),
        deleted_at = case when p_delete then now_ms else deleted_at end,
        updated_at = now_ms, updated_by = u, updated_device = dev, updated_op = null, rev = r
    where id = p_id;
    perform iamb_private.mark_written('membership', p_id, false);
    perform iamb_private.queue(before || iamb_private.readers('membership', p_id), 'row', 'membership',
                               p_id, null, null, null, null);
    return r;
end
$$;

-- ---------------------------------------------------------------------------------------------
-- hello (§6.2, C25)
-- ---------------------------------------------------------------------------------------------

create or replace function iamb_private.acct_hello(u uuid, sid uuid, p_device jsonb, p_protocols integer[],
                                                   p_max_rev bigint) returns jsonb
language plpgsql volatile set search_path = '' as $$
declare
    st       iamb.server_state;
    dev_id   uuid := iamb_private.try_uuid(p_device ->> 'id');
    lo       integer := p_protocols[1];
    hi       integer := coalesce(p_protocols[2], p_protocols[1]);
    proto    integer;
    d_id     iamb.device;
    d_sess   iamb.device;
    pp       uuid;
    r        bigint;
    now_ms   bigint := iamb_private.now_ms();
    issued   bigint;
    dn       text;
    ep_start bigint;
    lim      bigint;
    n        bigint;
    plan     iamb.plan;
    v_name   text := left(coalesce(p_device ->> 'name', ''), 200);
    v_plat   text := left(coalesce(p_device ->> 'platform', ''), 50);
    v_app    text := left(coalesce(p_device ->> 'app_version', ''), 50);
begin
    select * into st from iamb.server_state;
    if dev_id is null or lo is null or hi is null or lo > hi then
        return iamb_private.err('invalid');
    end if;
    proto := least(hi, st.protocol_max);
    if proto < greatest(lo, st.protocol_min) then
        if hi < st.protocol_min then
            return iamb_private.err('upgrade_required');
        end if;
        return iamb_private.err('server_too_old');
    end if;

    -- A device that holds a revision this server never gave out (C18): stop it, tell the
    -- operator, change nothing.
    select last_value into issued from iamb.rev_seq;
    if coalesce(p_max_rev, 0) > issued then
        perform iamb_private.log('server_behind', jsonb_build_object(
            'user', u, 'device', dev_id, 'claimed_rev', p_max_rev, 'issued', issued));
        return iamb_private.err('server_behind');
    end if;

    plan := iamb_private.user_plan(u);
    if coalesce((plan.features ->> 'sync')::boolean, true) is not true then
        return iamb_private.err('plan');
    end if;

    -- C24 step 1: the device rows this call may write, in the order of their ids.
    perform 1 from iamb.device where id = dev_id or session_id = sid order by id for update;
    select * into d_id from iamb.device where id = dev_id;
    select * into d_sess from iamb.device where session_id = sid;

    -- The personal profile and the account's rows, created when missing (§5.3).
    pp := iamb_private.personal_profile(u);
    if pp is null then
        pp := iamb_private.uuid7();
        r := nextval('iamb.rev_seq');
        insert into iamb.profile (id, created_at, updated_at, updated_by, rev, created_rev, kind, name,
                                  owner_user_id, head_rev)
        values (pp, now_ms, now_ms, u, r, r, 'personal', 'Personal', u, r)
        on conflict do nothing;
        if found then
            perform iamb_private.mark_written('profile', pp, true);
            perform iamb_private.queue(array[u], 'row', 'profile', pp, null, null, null, null);
        end if;
        pp := iamb_private.personal_profile(u);
    end if;
    insert into iamb.user_profile (user_id, display_name, created_at, updated_at)
    values (u, '', now_ms, now_ms) on conflict (user_id) do nothing;
    insert into iamb.feed_head (user_id) values (u) on conflict (user_id) do nothing;
    -- C24 step 3: the personal profile, under which the device limit is counted (§5.10).
    perform iamb_private.lock_profiles(array[pp]);

    if d_sess.id is not null then
        if d_sess.revoked_at is not null then
            return iamb_private.err('device_revoked');
        elsif d_sess.id <> dev_id then
            return iamb_private.err('device_conflict');
        end if;
        update iamb.device set name = v_name, platform = v_plat, app_version = v_app, protocol = proto,
            last_seen_at = now_ms
        where id = dev_id;
    elsif d_id.id is not null then
        if d_id.revoked_at is not null or d_id.user_id <> u
           or exists (select 1 from auth.sessions x where x.id = d_id.session_id) then
            return iamb_private.err('device_conflict');
        end if;
        -- The same installation signed in again: the row moves to the new session.
        update iamb.device set session_id = sid, name = v_name, platform = v_plat, app_version = v_app,
            protocol = proto, last_seen_at = now_ms
        where id = dev_id;
    else
        select started_at into ep_start from iamb.epoch where id = st.epoch;
        lim := (plan.limits ->> 'devices_max')::bigint;
        select count(*) into n from iamb.device d
        where d.user_id = u and d.revoked_at is null
          and exists (select 1 from auth.sessions x where x.id = d.session_id)
          and d.last_seen_at is not null and d.last_seen_at > now_ms - 30 * 86400000::bigint
          and d.last_seen_at >= coalesce(ep_start, 0);
        if lim is not null and n >= lim then
            return iamb_private.err('plan_limit');
        end if;
        insert into iamb.device (id, user_id, session_id, name, platform, app_version, protocol,
                                 created_at, last_seen_at)
        values (dev_id, u, sid, v_name, v_plat, v_app, proto, now_ms, now_ms);
        perform iamb_private.queue(array[u], 'account', null, null, null, null, null, null);
    end if;

    select display_name into dn from iamb.user_profile where user_id = u;
    return jsonb_build_object(
        'error', null, 'protocol', proto, 'user', u, 'personal_profile', pp, 'epoch', st.epoch,
        'head', coalesce((select head from iamb.feed_head where user_id = u), 0),
        'retention_days', st.retention_days, 'server_time', now_ms, 'display_name', coalesce(dn, ''),
        'limits', jsonb_build_object('push_ops', st.push_ops_max, 'push_bytes', st.push_bytes_max,
                                     'pull_entries', st.pull_entries_max, 'cascade_max', st.cascade_max,
                                     'attachment_bytes_max', st.attachment_bytes_max));
end
$$;

create or replace function api.hello(device jsonb, protocols integer[], max_rev bigint default 0)
returns jsonb
language plpgsql volatile security definer set search_path = '' set statement_timeout = '8s' as $$
declare
    e jsonb;
    r jsonb;
begin
    e := iamb_private.enter(iamb_private.try_uuid(device ->> 'id'), protocols[2], 'hello');
    if e ->> 'error' is not null then
        return e;
    end if;
    r := iamb_private.acct_hello(auth.uid(), iamb_private.token_session(), device, protocols, max_rev);
    perform iamb_private.feed_flush();
    return r || jsonb_build_object('epoch', e -> 'epoch');
end
$$;

-- `check`: nothing but `enter`, for the edge functions before they use the service key (§5.14).
create or replace function api.check(device uuid, protocol integer)
returns jsonb
language plpgsql volatile security invoker set search_path = '' set statement_timeout = '8s' as $$
begin
    return iamb_private.enter(device, protocol, 'check');
end
$$;

-- ---------------------------------------------------------------------------------------------
-- Groups (§4.1, C16)
-- ---------------------------------------------------------------------------------------------

create or replace function iamb_private.acct_create_group(u uuid, dev uuid, p_id uuid, p_kind text, p_name text)
returns jsonb
language plpgsql volatile set search_path = '' as $$
declare
    pp     uuid := iamb_private.personal_profile(u);
    ex     iamb.profile;
    lim    bigint;
    n      bigint;
    r      bigint;
    mid    uuid;
    now_ms bigint := iamb_private.now_ms();
begin
    if p_id is null or p_kind is null or p_kind not in ('family', 'company')
       or coalesce(btrim(p_name), '') = '' or octet_length(p_name) > 1024 then
        return iamb_private.err('invalid');
    end if;
    if pp is null then
        return iamb_private.err('invalid');
    end if;
    select * into ex from iamb.profile where id = p_id;
    if ex.id is not null then
        -- A repeated call (its answer was lost) answers as the first did.
        if ex.kind = p_kind and exists (select 1 from iamb.membership m
                                        where m.profile_id = ex.id and m.user_id = u and m.created_rev = (
                                            select min(m2.created_rev) from iamb.membership m2 where m2.profile_id = ex.id)) then
            return jsonb_build_object('error', null, 'group', ex.id, 'mark', ex.head_rev);
        end if;
        return iamb_private.err('invalid');
    end if;
    if coalesce((select display_name from iamb.user_profile where user_id = u), '') = '' then
        return iamb_private.err('name_required');
    end if;
    perform iamb_private.lock_profiles(array[pp]);
    lim := iamb_private.user_limit(u, 'groups_owned_max');
    select count(*) into n from iamb.profile where owner_user_id = u and kind <> 'personal' and deleted_at is null;
    if lim is not null and n >= lim then
        return iamb_private.err('plan_limit');
    end if;
    r := nextval('iamb.rev_seq');
    insert into iamb.profile (id, created_at, updated_at, updated_by, rev, created_rev, updated_device,
                              kind, name, owner_user_id, head_rev)
    values (p_id, now_ms, now_ms, u, r, r, dev, p_kind, p_name, u, r);
    mid := iamb_private.uuid7();
    r := iamb_private.next_rev(array[p_id, pp]);
    insert into iamb.membership (id, created_at, updated_at, updated_by, rev, created_rev, updated_device,
                                 profile_id, user_id, role)
    values (mid, now_ms, now_ms, u, r, r, dev, p_id, u, 'owner');
    perform iamb_private.mark_written('profile', p_id, true);
    perform iamb_private.mark_written('membership', mid, true);
    perform iamb_private.queue(array[u], 'row', 'profile', p_id, null, null, null, null);
    perform iamb_private.queue(array[u], 'row', 'membership', mid, null, null, null, null);
    perform iamb_private.queue(array[u], 'entitlements', null, null, null, null, null, null);
    perform iamb_private.log('create_group', jsonb_build_object('user', u, 'group', p_id, 'mark', r, 'answer', 'ok'));
    return jsonb_build_object('error', null, 'group', p_id, 'mark', r);
end
$$;

create or replace function iamb_private.acct_rename_group(u uuid, dev uuid, g uuid, p_name text)
returns jsonb
language plpgsql volatile set search_path = '' as $$
declare
    role   text;
    old    jsonb;
    before uuid[];
    r      bigint;
    now_ms bigint := iamb_private.now_ms();
begin
    if coalesce(btrim(p_name), '') = '' or octet_length(p_name) > 1024 then
        return iamb_private.err('invalid');
    end if;
    perform iamb_private.lock_profiles(array[g]);
    role := iamb_private.group_role(u, g);
    if role is null then
        return iamb_private.err('not_found');
    elsif role not in ('owner', 'admin') then
        return iamb_private.err('forbidden');
    end if;
    old := iamb_private.get_row('profile', g);
    if old ->> 'name' = p_name then
        return jsonb_build_object('error', null, 'mark', (old ->> 'head_rev')::bigint);
    end if;
    before := iamb_private.readers('profile', g);
    r := iamb_private.next_rev(array[g]);
    perform iamb_private.keep_history('profile', old, r);
    update iamb.profile set name = p_name, rev = r, updated_at = now_ms, updated_by = u, updated_device = dev,
        updated_op = null
    where id = g;
    perform iamb_private.mark_written('profile', g, false);
    perform iamb_private.queue(before || iamb_private.readers('profile', g), 'row', 'profile', g, null, null, null, null);
    perform iamb_private.log('rename_group', jsonb_build_object('user', u, 'group', g, 'mark', r, 'answer', 'ok'));
    return jsonb_build_object('error', null, 'mark', r);
end
$$;

-- Deletes a group (C16): its memberships, the shares into it and the folders it owns with their
-- content. Every member gets `folder_revoked` for the group's folders and for the folders others
-- shared with it.
create or replace function iamb_private.acct_delete_group(u uuid, dev uuid, g uuid)
returns jsonb
language plpgsql volatile set search_path = '' as $$
declare
    role    text;
    sharers uuid[];
    locks   uuid[];
    r       bigint;
    s       record;
    m       record;
    before  uuid[];
    now_ms  bigint := iamb_private.now_ms();
begin
    perform iamb_private.lock_profiles(array[g]);
    if exists (select 1 from iamb.profile where id = g and kind <> 'personal' and deleted_at is not null
               and owner_user_id = u) then
        return jsonb_build_object('error', null, 'mark', (select head_rev from iamb.profile where id = g));
    end if;
    role := iamb_private.group_role(u, g);
    if role is null then
        return iamb_private.err('not_found');
    elsif role <> 'owner' then
        return iamb_private.err('not_owner');
    end if;
    -- The profiles that share into the group, read after the group is locked, locked, and the
    -- shares read again under their locks (C24).
    sharers := iamb_private.sharing_profiles(g);
    perform iamb_private.lock_profiles(sharers);
    locks := array[g] || sharers;
    before := iamb_private.readers('profile', g);

    for s in select sh.id from iamb.folder_share sh where sh.profile_id = g and sh.deleted_at is null order by sh.id loop
        perform iamb_private.end_share(u, dev, null, s.id, locks, 'delete_group');
    end loop;
    r := iamb_private.next_rev(locks);
    -- The group's own rows: tombstones in two statements, no feed entries (nobody can read them).
    update iamb.snippet set deleted_at = now_ms, updated_at = now_ms, updated_by = u, updated_device = dev,
        updated_op = null, rev = r
    where profile_id = g and deleted_at is null;
    update iamb.folder set deleted_at = now_ms, updated_at = now_ms, updated_by = u, updated_device = dev,
        updated_op = null, rev = r
    where profile_id = g and deleted_at is null;
    for m in select id from iamb.membership where profile_id = g and deleted_at is null order by id loop
        perform iamb_private.write_membership(u, dev, m.id, null, true, locks);
    end loop;
    r := iamb_private.next_rev(locks);
    perform iamb_private.keep_history('profile', iamb_private.get_row('profile', g), r);
    update iamb.profile set deleted_at = now_ms, offered_owner = null, offered_at = null, rev = r,
        updated_at = now_ms, updated_by = u, updated_device = dev, updated_op = null
    where id = g;
    perform iamb_private.mark_written('profile', g, false);
    perform iamb_private.queue(before, 'row', 'profile', g, null, null, null, null);
    perform iamb_private.refresh_access(locks);
    perform iamb_private.queue(before, 'entitlements', null, null, null, null, null, null);
    perform iamb_private.log('delete_group', jsonb_build_object('user', u, 'group', g, 'mark', r, 'answer', 'ok'));
    return jsonb_build_object('error', null, 'mark', r);
end
$$;

create or replace function iamb_private.acct_transfer_ownership(u uuid, dev uuid, g uuid, target uuid)
returns jsonb
language plpgsql volatile set search_path = '' as $$
declare
    role  text;
    prev  uuid;
    r     bigint;
begin
    perform iamb_private.lock_profiles(array[g]);
    role := iamb_private.group_role(u, g);
    if role is null then
        return iamb_private.err('not_found');
    elsif role <> 'owner' then
        return iamb_private.err('not_owner');
    end if;
    select offered_owner into prev from iamb.profile where id = g;
    if target is null then
        if prev is null then
            return jsonb_build_object('error', null, 'mark', (select head_rev from iamb.profile where id = g));
        end if;
        r := iamb_private.next_rev(array[g]);
        update iamb.profile set offered_owner = null, offered_at = null where id = g;
        perform iamb_private.queue(array[u, prev], 'invitations', null, null, null, null, null, null);
        perform iamb_private.log('withdraw_offer', jsonb_build_object('user', u, 'group', g, 'person', prev, 'mark', r, 'answer', 'ok'));
        return jsonb_build_object('error', null, 'mark', r);
    end if;
    if target = u or iamb_private.group_role(target, g) is null then
        return iamb_private.err('not_a_member');
    end if;
    if prev = target then
        return jsonb_build_object('error', null, 'mark', (select head_rev from iamb.profile where id = g));
    end if;
    r := iamb_private.next_rev(array[g]);
    update iamb.profile set offered_owner = target, offered_at = iamb_private.now_ms() where id = g;
    perform iamb_private.queue(array[u, target, prev], 'invitations', null, null, null, null, null, null);
    perform iamb_private.log('transfer_ownership', jsonb_build_object('user', u, 'group', g, 'person', target, 'mark', r, 'answer', 'ok'));
    return jsonb_build_object('error', null, 'mark', r);
end
$$;

create or replace function iamb_private.acct_accept_ownership(u uuid, dev uuid, g uuid)
returns jsonb
language plpgsql volatile set search_path = '' as $$
declare
    pg_    iamb.profile;
    pp     uuid := iamb_private.personal_profile(u);
    lim    bigint;
    n      bigint;
    r      bigint;
    old    jsonb;
    before uuid[];
    m_old  uuid;
    m_new  uuid;
    now_ms bigint := iamb_private.now_ms();
begin
    perform iamb_private.lock_profiles(array[g, pp]);
    select * into pg_ from iamb.profile where id = g and kind <> 'personal' and deleted_at is null;
    if pg_.id is not null and pg_.owner_user_id = u then
        return jsonb_build_object('error', null, 'mark', pg_.head_rev);
    end if;
    if pg_.id is null or pg_.offered_owner is distinct from u or pg_.offered_at < now_ms - 14 * 86400000::bigint
       or iamb_private.group_role(u, g) is null then
        return iamb_private.err('no_offer');
    end if;
    lim := iamb_private.user_limit(u, 'groups_owned_max');
    select count(*) into n from iamb.profile where owner_user_id = u and kind <> 'personal' and deleted_at is null;
    if lim is not null and n >= lim then
        return iamb_private.err('plan_limit');
    end if;
    select id into m_old from iamb.membership where profile_id = g and role = 'owner' and deleted_at is null;
    select id into m_new from iamb.membership where profile_id = g and user_id = u and deleted_at is null;
    perform iamb_private.write_membership(u, dev, m_old, 'admin', false, array[g, pp]);
    perform iamb_private.write_membership(u, dev, m_new, 'owner', false, array[g, pp]);
    old := iamb_private.get_row('profile', g);
    before := iamb_private.readers('profile', g);
    r := iamb_private.next_rev(array[g, pp]);
    perform iamb_private.keep_history('profile', old, r);
    update iamb.profile set owner_user_id = u, offered_owner = null, offered_at = null, rev = r,
        updated_at = now_ms, updated_by = u, updated_device = dev, updated_op = null
    where id = g;
    perform iamb_private.mark_written('profile', g, false);
    perform iamb_private.queue(before || iamb_private.readers('profile', g), 'row', 'profile', g, null, null, null, null);
    perform iamb_private.queue(array[u, pg_.owner_user_id], 'entitlements', null, null, null, null, null, null);
    perform iamb_private.log('accept_ownership', jsonb_build_object('user', u, 'group', g, 'mark', r, 'answer', 'ok'));
    return jsonb_build_object('error', null, 'mark', r);
end
$$;

create or replace function iamb_private.acct_decline_ownership(u uuid, dev uuid, g uuid)
returns jsonb
language plpgsql volatile set search_path = '' as $$
declare
    pg_ iamb.profile;
    r   bigint;
begin
    perform iamb_private.lock_profiles(array[g]);
    select * into pg_ from iamb.profile where id = g and kind <> 'personal' and deleted_at is null;
    if pg_.id is null or pg_.offered_owner is distinct from u then
        return iamb_private.err('no_offer');
    end if;
    r := iamb_private.next_rev(array[g]);
    update iamb.profile set offered_owner = null, offered_at = null where id = g;
    perform iamb_private.queue(array[u, pg_.owner_user_id], 'invitations', null, null, null, null, null, null);
    perform iamb_private.log('decline_ownership', jsonb_build_object('user', u, 'group', g, 'mark', r, 'answer', 'ok'));
    return jsonb_build_object('error', null, 'mark', r);
end
$$;

-- ---------------------------------------------------------------------------------------------
-- Invitations and memberships (§5.8, C16)
-- ---------------------------------------------------------------------------------------------

create or replace function iamb_private.acct_invite(u uuid, dev uuid, p_id uuid, g uuid, p_email text, p_role text)
returns jsonb
language plpgsql volatile set search_path = '' as $$
declare
    role   text;
    em     text := lower(btrim(coalesce(p_email, '')));
    ex     iamb.invitation;
    lim    bigint;
    n      bigint;
    wait   bigint;
    st     iamb.server_state;
    r      bigint;
    now_ms bigint := iamb_private.now_ms();
    invitee uuid;
begin
    select * into st from iamb.server_state;
    if p_id is null or g is null or em !~ '^[^@\s]+@[^@\s]+$' or length(em) > 320
       or p_role not in ('admin', 'member') then
        return iamb_private.err('invalid');
    end if;
    perform iamb_private.lock_profiles(array[g]);
    select * into ex from iamb.invitation where id = p_id;
    if ex.id is not null then
        if ex.profile_id = g and ex.email = em and ex.invited_by = u then
            return jsonb_build_object('error', null, 'mark', (select head_rev from iamb.profile where id = g));
        end if;
        return iamb_private.err('invalid');
    end if;
    role := iamb_private.group_role(u, g);
    if role is null then
        return iamb_private.err('not_found');
    elsif role not in ('owner', 'admin') then
        return iamb_private.err('forbidden');
    end if;
    if coalesce((select display_name from iamb.user_profile where user_id = u), '') = '' then
        return iamb_private.err('name_required');
    end if;
    if exists (select 1 from iamb.membership m join auth.users au on au.id = m.user_id
               where m.profile_id = g and m.deleted_at is null and lower(au.email) = em
                 and au.email_confirmed_at is not null) then
        return iamb_private.err('already_member');
    end if;
    if exists (select 1 from iamb.invitation where profile_id = g and email = em and status = 'pending'
               and expires_at > now_ms) then
        return iamb_private.err('already_invited');
    end if;
    lim := iamb_private.profile_limit(g, 'members_max');
    select count(*) into n from iamb.membership where profile_id = g and deleted_at is null;
    if lim is not null and n >= lim then
        return iamb_private.err('plan_limit');
    end if;
    wait := iamb_private.rate_take(g, 'invites_group_hour', 3600000, coalesce((st.rate_limits ->> 'invites_group_hour')::integer, 20), 1);
    if wait is null then
        wait := iamb_private.rate_take(u, 'invites_account_day', 86400000, coalesce((st.rate_limits ->> 'invites_account_day')::integer, 50), 1);
    end if;
    if wait is not null then
        return jsonb_build_object('error', 'rate_limited', 'retry_after_ms', wait);
    end if;
    -- An expired pending invitation for the same address is closed first.
    update iamb.invitation set status = 'expired', answered_at = now_ms
    where profile_id = g and email = em and status = 'pending';
    insert into iamb.invitation (id, profile_id, email, role, invited_by, created_at, expires_at, status)
    values (p_id, g, em, p_role, u, now_ms, now_ms + 14 * 86400000::bigint, 'pending');
    r := iamb_private.next_rev(array[g]);
    select au.id into invitee from auth.users au where lower(au.email) = em and au.email_confirmed_at is not null limit 1;
    perform iamb_private.queue(
        array(select m.user_id from iamb.membership m where m.profile_id = g and m.deleted_at is null
              and m.role in ('owner', 'admin')) || invitee,
        'invitations', null, null, null, null, null, null);
    perform iamb_private.log('invite', jsonb_build_object('user', u, 'group', g, 'invitation', p_id, 'mark', r, 'answer', 'ok'));
    return jsonb_build_object('error', null, 'mark', r);
end
$$;

create or replace function iamb_private.acct_withdraw_invitation(u uuid, dev uuid, p_id uuid)
returns jsonb
language plpgsql volatile set search_path = '' as $$
declare
    inv  iamb.invitation;
    role text;
    r    bigint;
    invitee uuid;
begin
    select * into inv from iamb.invitation where id = p_id;
    if inv.id is null then
        return iamb_private.err('not_found');
    end if;
    perform iamb_private.lock_profiles(array[inv.profile_id]);
    select * into inv from iamb.invitation where id = p_id;
    role := iamb_private.group_role(u, inv.profile_id);
    if role is null then
        return iamb_private.err('not_found');
    elsif role not in ('owner', 'admin') then
        return iamb_private.err('forbidden');
    end if;
    if inv.status = 'accepted' then
        return iamb_private.err('already_accepted');
    elsif inv.status = 'withdrawn' then
        return jsonb_build_object('error', null, 'mark', (select head_rev from iamb.profile where id = inv.profile_id));
    elsif inv.status <> 'pending' then
        return iamb_private.err('invitation_invalid');
    end if;
    update iamb.invitation set status = 'withdrawn', answered_at = iamb_private.now_ms() where id = p_id;
    r := iamb_private.next_rev(array[inv.profile_id]);
    select au.id into invitee from auth.users au where lower(au.email) = inv.email and au.email_confirmed_at is not null limit 1;
    perform iamb_private.queue(
        array(select m.user_id from iamb.membership m where m.profile_id = inv.profile_id and m.deleted_at is null
              and m.role in ('owner', 'admin')) || invitee,
        'invitations', null, null, null, null, null, null);
    perform iamb_private.log('withdraw_invitation', jsonb_build_object('user', u, 'group', inv.profile_id, 'invitation', p_id, 'mark', r, 'answer', 'ok'));
    return jsonb_build_object('error', null, 'mark', r);
end
$$;

create or replace function iamb_private.acct_accept_invitation(u uuid, dev uuid, p_id uuid)
returns jsonb
language plpgsql volatile set search_path = '' as $$
declare
    inv     iamb.invitation;
    addr    text := (select lower(au.email) from auth.users au where au.id = u and au.email_confirmed_at is not null);
    sharers uuid[];
    locks   uuid[];
    lim     bigint;
    n       bigint;
    mid     uuid;
    r       bigint;
    now_ms  bigint := iamb_private.now_ms();
    mm      record;
begin
    select * into inv from iamb.invitation where id = p_id;
    if inv.id is null or addr is null or inv.email <> addr then
        return iamb_private.err('invitation_invalid');
    end if;
    perform iamb_private.lock_profiles(array[inv.profile_id]);
    sharers := iamb_private.sharing_profiles(inv.profile_id);
    perform iamb_private.lock_profiles(sharers);
    locks := array[inv.profile_id] || sharers;
    select * into inv from iamb.invitation where id = p_id;
    if inv.status = 'accepted' and inv.accepted_by = u then
        return jsonb_build_object('error', null, 'mark', (select head_rev from iamb.profile where id = inv.profile_id));
    end if;
    if inv.status <> 'pending' or inv.expires_at <= now_ms
       or not exists (select 1 from iamb.profile where id = inv.profile_id and deleted_at is null) then
        return iamb_private.err('invitation_invalid');
    end if;
    if coalesce((select display_name from iamb.user_profile where user_id = u), '') = '' then
        return iamb_private.err('name_required');
    end if;
    if iamb_private.group_role(u, inv.profile_id) is not null then
        update iamb.invitation set status = 'accepted', answered_at = now_ms, accepted_by = u where id = p_id;
        r := iamb_private.next_rev(locks);
        perform iamb_private.log('accept_invitation', jsonb_build_object('user', u, 'group', inv.profile_id, 'invitation', p_id, 'mark', r, 'answer', 'ok'));
        return jsonb_build_object('error', null, 'mark', r);
    end if;
    lim := iamb_private.profile_limit(inv.profile_id, 'members_max');
    select count(*) into n from iamb.membership where profile_id = inv.profile_id and deleted_at is null;
    if lim is not null and n >= lim then
        return iamb_private.err('plan_limit');
    end if;
    update iamb.invitation set status = 'accepted', answered_at = now_ms, accepted_by = u where id = p_id;
    mid := iamb_private.uuid7();
    r := iamb_private.next_rev(locks);
    insert into iamb.membership (id, created_at, updated_at, updated_by, rev, created_rev, updated_device,
                                 profile_id, user_id, role)
    values (mid, now_ms, now_ms, u, r, r, dev, inv.profile_id, u, inv.role);
    perform iamb_private.mark_written('membership', mid, true);
    perform iamb_private.refresh_access(locks);
    -- The new member's feed gets the group's profile and every live membership (§5.5); the
    -- others get the new membership.
    perform iamb_private.queue(array[u], 'row', 'profile', inv.profile_id, null, null, null, null);
    for mm in select id from iamb.membership where profile_id = inv.profile_id and deleted_at is null order by id loop
        perform iamb_private.queue(array[u], 'row', 'membership', mm.id, null, null, null, null);
    end loop;
    perform iamb_private.queue(iamb_private.readers('membership', mid), 'row', 'membership', mid, null, null, null, null);
    perform iamb_private.queue(
        array(select m.user_id from iamb.membership m where m.profile_id = inv.profile_id and m.deleted_at is null
              and m.role in ('owner', 'admin')) || u,
        'invitations', null, null, null, null, null, null);
    perform iamb_private.log('accept_invitation', jsonb_build_object('user', u, 'group', inv.profile_id, 'invitation', p_id, 'mark', r, 'answer', 'ok'));
    return jsonb_build_object('error', null, 'mark', r);
end
$$;

create or replace function iamb_private.acct_decline_invitation(u uuid, dev uuid, p_id uuid)
returns jsonb
language plpgsql volatile set search_path = '' as $$
declare
    inv  iamb.invitation;
    addr text := (select lower(au.email) from auth.users au where au.id = u and au.email_confirmed_at is not null);
    r    bigint;
begin
    select * into inv from iamb.invitation where id = p_id;
    if inv.id is null or addr is null or inv.email <> addr then
        return iamb_private.err('invitation_invalid');
    end if;
    perform iamb_private.lock_profiles(array[inv.profile_id]);
    select * into inv from iamb.invitation where id = p_id;
    if inv.status = 'declined' then
        return jsonb_build_object('error', null, 'mark', (select head_rev from iamb.profile where id = inv.profile_id));
    elsif inv.status <> 'pending' then
        return iamb_private.err('invitation_invalid');
    end if;
    update iamb.invitation set status = 'declined', answered_at = iamb_private.now_ms() where id = p_id;
    r := iamb_private.next_rev(array[inv.profile_id]);
    perform iamb_private.queue(
        array(select m.user_id from iamb.membership m where m.profile_id = inv.profile_id and m.deleted_at is null
              and m.role in ('owner', 'admin')) || u,
        'invitations', null, null, null, null, null, null);
    return jsonb_build_object('error', null, 'mark', r);
end
$$;

create or replace function iamb_private.acct_set_member_role(u uuid, dev uuid, g uuid, target uuid, p_role text)
returns jsonb
language plpgsql volatile set search_path = '' as $$
declare
    role  text;
    trole text;
    mid   uuid;
    r     bigint;
begin
    if p_role not in ('member', 'admin') then
        return iamb_private.err('invalid');
    end if;
    perform iamb_private.lock_profiles(array[g]);
    role := iamb_private.group_role(u, g);
    if role is null then
        return iamb_private.err('not_found');
    elsif role not in ('owner', 'admin') then
        return iamb_private.err('forbidden');
    end if;
    trole := iamb_private.group_role(target, g);
    if trole is null then
        return iamb_private.err('not_a_member');
    elsif trole = 'owner' then
        return iamb_private.err('owner_role');
    elsif trole = p_role then
        return jsonb_build_object('error', null, 'mark', (select head_rev from iamb.profile where id = g));
    end if;
    select id into mid from iamb.membership where profile_id = g and user_id = target and deleted_at is null;
    r := iamb_private.write_membership(u, dev, mid, p_role, false, array[g]);
    perform iamb_private.refresh_access(array[g]);
    perform iamb_private.log('set_member_role', jsonb_build_object('user', u, 'group', g, 'person', target,
                             'role', p_role, 'mark', r, 'answer', 'ok'));
    return jsonb_build_object('error', null, 'mark', r);
end
$$;

-- Ends a membership (remove, leave): the membership's tombstone, the shares from that person's
-- folders into the group, and every access entry the membership gave (C16, G13).
create or replace function iamb_private.end_membership(u uuid, dev uuid, g uuid, target uuid, p_call text)
returns jsonb
language plpgsql volatile set search_path = '' as $$
declare
    sharers uuid[];
    locks   uuid[];
    mid     uuid;
    r       bigint;
    s       record;
begin
    sharers := iamb_private.sharing_profiles(g);
    perform iamb_private.lock_profiles(sharers);
    locks := array[g] || sharers;
    select id into mid from iamb.membership where profile_id = g and user_id = target and deleted_at is null;
    for s in select sh.id from iamb.folder_share sh join iamb.folder f on f.id = sh.folder_id
             join iamb.profile p on p.id = f.profile_id
             where sh.profile_id = g and sh.deleted_at is null and p.kind = 'personal' and p.owner_user_id = target
             order by sh.id loop
        perform iamb_private.end_share(u, dev, null, s.id, locks, p_call);
    end loop;
    r := iamb_private.write_membership(u, dev, mid, null, true, locks);
    perform iamb_private.refresh_access(locks);
    perform iamb_private.queue(array[target], 'entitlements', null, null, null, null, null, null);
    perform iamb_private.log(p_call, jsonb_build_object('user', u, 'group', g, 'person', target, 'mark', r, 'answer', 'ok'));
    return jsonb_build_object('error', null, 'mark', r);
end
$$;

create or replace function iamb_private.acct_remove_member(u uuid, dev uuid, g uuid, target uuid)
returns jsonb
language plpgsql volatile set search_path = '' as $$
declare
    role  text;
    trole text;
begin
    perform iamb_private.lock_profiles(array[g]);
    role := iamb_private.group_role(u, g);
    if role is null then
        return iamb_private.err('not_found');
    elsif role not in ('owner', 'admin') then
        return iamb_private.err('forbidden');
    end if;
    trole := iamb_private.group_role(target, g);
    if trole is null then
        if exists (select 1 from iamb.membership where profile_id = g and user_id = target and deleted_at is not null) then
            return jsonb_build_object('error', null, 'mark', (select head_rev from iamb.profile where id = g));
        end if;
        return iamb_private.err('not_a_member');
    elsif trole = 'owner' then
        return iamb_private.err('owner_role');
    end if;
    return iamb_private.end_membership(u, dev, g, target, 'remove_member');
end
$$;

create or replace function iamb_private.acct_leave_group(u uuid, dev uuid, g uuid)
returns jsonb
language plpgsql volatile set search_path = '' as $$
declare
    role text;
begin
    perform iamb_private.lock_profiles(array[g]);
    role := iamb_private.group_role(u, g);
    if role is null then
        return iamb_private.err('not_a_member');
    elsif role = 'owner' then
        return iamb_private.err('owner_cannot_leave');
    end if;
    return iamb_private.end_membership(u, dev, g, u, 'leave_group');
end
$$;

-- ---------------------------------------------------------------------------------------------
-- The account (§11.3, C25)
-- ---------------------------------------------------------------------------------------------

create or replace function iamb_private.acct_set_display_name(u uuid, p_name text)
returns jsonb
language plpgsql volatile set search_path = '' as $$
begin
    if p_name is null or btrim(p_name) = '' or octet_length(p_name) > 200 then
        return iamb_private.err('invalid');
    end if;
    update iamb.user_profile set display_name = btrim(p_name), updated_at = iamb_private.now_ms() where user_id = u;
    perform iamb_private.queue(array[u], 'account', null, null, null, null, null, null);
    return jsonb_build_object('error', null);
end
$$;

create or replace function iamb_private.acct_rename_device(u uuid, p_target uuid, p_name text)
returns jsonb
language plpgsql volatile set search_path = '' as $$
begin
    if p_name is null or btrim(p_name) = '' or octet_length(p_name) > 200 then
        return iamb_private.err('invalid');
    end if;
    update iamb.device set name = btrim(p_name) where id = p_target and user_id = u;
    if not found then
        return iamb_private.err('not_found');
    end if;
    perform iamb_private.queue(array[u], 'account', null, null, null, null, null, null);
    return jsonb_build_object('error', null);
end
$$;

-- Marks one device row of the account, or all of them (`p_target` null), as revoked (C25). Locks
-- the device rows it writes and the caller's in the order of their ids (C24 step 1).
create or replace function iamb_private.acct_revoke(u uuid, dev uuid, p_target uuid, p_call text)
returns jsonb
language plpgsql volatile set search_path = '' as $$
declare
    ids    uuid[];
    now_ms bigint := iamb_private.now_ms();
begin
    if p_target is null then
        perform 1 from iamb.device where user_id = u order by id for update;
    else
        perform 1 from iamb.device where (id = p_target and user_id = u) or id = dev order by id for update;
        if not exists (select 1 from iamb.device where id = p_target and user_id = u) then
            return iamb_private.err('not_found');
        end if;
    end if;
    -- The caller's own row, checked once its lock is held.
    if dev is not null and not exists (select 1 from iamb.device where id = dev and user_id = u and revoked_at is null) then
        return iamb_private.err('device_revoked');
    end if;
    update iamb.device set revoked_at = coalesce(revoked_at, now_ms)
    where user_id = u and (p_target is null or id = p_target);
    select coalesce(array_agg(id order by id), '{}') into ids from iamb.device
    where user_id = u and revoked_at is not null and (p_target is null or id = p_target);
    perform iamb_private.queue(array[u], 'account', null, null, null, null, null, null);
    perform iamb_private.log(p_call, jsonb_build_object('user', u, 'device', dev, 'devices', to_jsonb(ids), 'answer', 'ok'));
    return jsonb_build_object('error', null, 'devices', to_jsonb(ids));
end
$$;

-- ---------------------------------------------------------------------------------------------
-- Wrappers in `api`. Each: `enter`, the core, the feed written last, the epoch in the answer.
-- ---------------------------------------------------------------------------------------------

create or replace function iamb_private.finish(e jsonb, r jsonb) returns jsonb
language plpgsql volatile set search_path = '' as $$
begin
    perform iamb_private.feed_flush();
    return jsonb_build_object('epoch', e -> 'epoch') || r;
end
$$;

create or replace function api.create_group(device uuid, protocol integer, id uuid, kind text, name text)
returns jsonb language plpgsql volatile security definer set search_path = '' set statement_timeout = '8s' as $$
declare e jsonb := iamb_private.enter(device, protocol, 'create_group');
begin
    if e ->> 'error' is not null then return e; end if;
    return iamb_private.finish(e, iamb_private.acct_create_group(auth.uid(), device, id, kind, name));
end $$;

create or replace function api.rename_group(device uuid, protocol integer, "group" uuid, name text)
returns jsonb language plpgsql volatile security definer set search_path = '' set statement_timeout = '8s' as $$
declare e jsonb := iamb_private.enter(device, protocol, 'rename_group');
begin
    if e ->> 'error' is not null then return e; end if;
    return iamb_private.finish(e, iamb_private.acct_rename_group(auth.uid(), device, "group", name));
end $$;

create or replace function api.delete_group(device uuid, protocol integer, "group" uuid)
returns jsonb language plpgsql volatile security definer set search_path = '' set statement_timeout = '30s' as $$
declare e jsonb := iamb_private.enter(device, protocol, 'delete_group');
begin
    if e ->> 'error' is not null then return e; end if;
    return iamb_private.finish(e, iamb_private.acct_delete_group(auth.uid(), device, "group"));
end $$;

create or replace function api.transfer_ownership(device uuid, protocol integer, "group" uuid, "user" uuid default null)
returns jsonb language plpgsql volatile security definer set search_path = '' set statement_timeout = '8s' as $$
declare e jsonb := iamb_private.enter(device, protocol, 'transfer_ownership');
begin
    if e ->> 'error' is not null then return e; end if;
    return iamb_private.finish(e, iamb_private.acct_transfer_ownership(auth.uid(), device, "group", "user"));
end $$;

create or replace function api.accept_ownership(device uuid, protocol integer, "group" uuid)
returns jsonb language plpgsql volatile security definer set search_path = '' set statement_timeout = '8s' as $$
declare e jsonb := iamb_private.enter(device, protocol, 'accept_ownership');
begin
    if e ->> 'error' is not null then return e; end if;
    return iamb_private.finish(e, iamb_private.acct_accept_ownership(auth.uid(), device, "group"));
end $$;

create or replace function api.decline_ownership(device uuid, protocol integer, "group" uuid)
returns jsonb language plpgsql volatile security definer set search_path = '' set statement_timeout = '8s' as $$
declare e jsonb := iamb_private.enter(device, protocol, 'decline_ownership');
begin
    if e ->> 'error' is not null then return e; end if;
    return iamb_private.finish(e, iamb_private.acct_decline_ownership(auth.uid(), device, "group"));
end $$;

create or replace function api.invite(device uuid, protocol integer, id uuid, "group" uuid, email text, role text)
returns jsonb language plpgsql volatile security definer set search_path = '' set statement_timeout = '8s' as $$
declare e jsonb := iamb_private.enter(device, protocol, 'invite');
begin
    if e ->> 'error' is not null then return e; end if;
    return iamb_private.finish(e, iamb_private.acct_invite(auth.uid(), device, id, "group", email, role));
end $$;

create or replace function api.withdraw_invitation(device uuid, protocol integer, id uuid)
returns jsonb language plpgsql volatile security definer set search_path = '' set statement_timeout = '8s' as $$
declare e jsonb := iamb_private.enter(device, protocol, 'withdraw_invitation');
begin
    if e ->> 'error' is not null then return e; end if;
    return iamb_private.finish(e, iamb_private.acct_withdraw_invitation(auth.uid(), device, id));
end $$;

create or replace function api.accept_invitation(device uuid, protocol integer, id uuid)
returns jsonb language plpgsql volatile security definer set search_path = '' set statement_timeout = '8s' as $$
declare e jsonb := iamb_private.enter(device, protocol, 'accept_invitation');
begin
    if e ->> 'error' is not null then return e; end if;
    return iamb_private.finish(e, iamb_private.acct_accept_invitation(auth.uid(), device, id));
end $$;

create or replace function api.decline_invitation(device uuid, protocol integer, id uuid)
returns jsonb language plpgsql volatile security definer set search_path = '' set statement_timeout = '8s' as $$
declare e jsonb := iamb_private.enter(device, protocol, 'decline_invitation');
begin
    if e ->> 'error' is not null then return e; end if;
    return iamb_private.finish(e, iamb_private.acct_decline_invitation(auth.uid(), device, id));
end $$;

create or replace function api.set_member_role(device uuid, protocol integer, "group" uuid, "user" uuid, role text)
returns jsonb language plpgsql volatile security definer set search_path = '' set statement_timeout = '8s' as $$
declare e jsonb := iamb_private.enter(device, protocol, 'set_member_role');
begin
    if e ->> 'error' is not null then return e; end if;
    return iamb_private.finish(e, iamb_private.acct_set_member_role(auth.uid(), device, "group", "user", role));
end $$;

create or replace function api.remove_member(device uuid, protocol integer, "group" uuid, "user" uuid)
returns jsonb language plpgsql volatile security definer set search_path = '' set statement_timeout = '30s' as $$
declare e jsonb := iamb_private.enter(device, protocol, 'remove_member');
begin
    if e ->> 'error' is not null then return e; end if;
    return iamb_private.finish(e, iamb_private.acct_remove_member(auth.uid(), device, "group", "user"));
end $$;

create or replace function api.leave_group(device uuid, protocol integer, "group" uuid)
returns jsonb language plpgsql volatile security definer set search_path = '' set statement_timeout = '30s' as $$
declare e jsonb := iamb_private.enter(device, protocol, 'leave_group');
begin
    if e ->> 'error' is not null then return e; end if;
    return iamb_private.finish(e, iamb_private.acct_leave_group(auth.uid(), device, "group"));
end $$;

create or replace function api.set_display_name(device uuid, protocol integer, name text)
returns jsonb language plpgsql volatile security definer set search_path = '' set statement_timeout = '8s' as $$
declare e jsonb := iamb_private.enter(device, protocol, 'set_display_name');
begin
    if e ->> 'error' is not null then return e; end if;
    return iamb_private.finish(e, iamb_private.acct_set_display_name(auth.uid(), name));
end $$;

create or replace function api.rename_device(device uuid, protocol integer, target uuid, name text)
returns jsonb language plpgsql volatile security definer set search_path = '' set statement_timeout = '8s' as $$
declare e jsonb := iamb_private.enter(device, protocol, 'rename_device');
begin
    if e ->> 'error' is not null then return e; end if;
    return iamb_private.finish(e, iamb_private.acct_rename_device(auth.uid(), target, name));
end $$;

create or replace function api.revoke_device(device uuid, protocol integer, target uuid)
returns jsonb language plpgsql volatile security definer set search_path = '' set statement_timeout = '8s' as $$
declare e jsonb := iamb_private.enter(device, protocol, 'revoke_device');
begin
    if e ->> 'error' is not null then return e; end if;
    if target is null then return jsonb_build_object('epoch', e -> 'epoch', 'error', 'not_found'); end if;
    return iamb_private.finish(e, iamb_private.acct_revoke(auth.uid(), device, target, 'revoke_device'));
end $$;

create or replace function api.sign_out_everywhere(device uuid, protocol integer)
returns jsonb language plpgsql volatile security definer set search_path = '' set statement_timeout = '8s' as $$
declare e jsonb := iamb_private.enter(device, protocol, 'sign_out_everywhere');
begin
    if e ->> 'error' is not null then return e; end if;
    return iamb_private.finish(e, iamb_private.acct_revoke(auth.uid(), device, null, 'sign_out_everywhere'));
end $$;

-- ---------------------------------------------------------------------------------------------
-- Account reads
-- ---------------------------------------------------------------------------------------------

-- Members of a group: display names and roles; owner and admins also get each address. A definer
-- function, because the addresses are in the authentication service's table (a correction to
-- §5.1's list of invoker functions, server/README.md).
create or replace function api.members(device uuid, protocol integer, "group" uuid)
returns jsonb language plpgsql volatile security definer set search_path = '' set statement_timeout = '8s' as $$
declare
    e      jsonb := iamb_private.enter(device, protocol, 'members');
    v_role text;
begin
    if e ->> 'error' is not null then return e; end if;
    v_role := iamb_private.group_role(auth.uid(), "group");
    if v_role is null then
        return jsonb_build_object('epoch', e -> 'epoch', 'error', 'not_found');
    end if;
    return jsonb_build_object('epoch', e -> 'epoch', 'error', null, 'members', coalesce((
        select jsonb_agg(jsonb_build_object(
                   'user', m.user_id, 'role', m.role,
                   'display_name', coalesce(up.display_name, ''),
                   'email', case when v_role in ('owner', 'admin') then au.email end)
               order by m.created_rev)
        from iamb.membership m
        left join iamb.user_profile up on up.user_id = m.user_id
        left join auth.users au on au.id = m.user_id
        where m.profile_id = "group" and m.deleted_at is null), '[]'::jsonb));
end $$;

-- Invitations for the caller's confirmed address, and those of groups the caller owns or
-- administers (with the address). Read through the policy on `invitation`.
create or replace function api.invitations(device uuid, protocol integer)
returns jsonb language plpgsql volatile security invoker set search_path = '' set statement_timeout = '8s' as $$
declare
    e jsonb := iamb_private.enter(device, protocol, 'invitations');
begin
    if e ->> 'error' is not null then return e; end if;
    return jsonb_build_object('epoch', e -> 'epoch', 'error', null, 'invitations', coalesce((
        select jsonb_agg(jsonb_build_object(
                   'id', i.id, 'group', i.profile_id, 'email', i.email, 'role', i.role,
                   'invited_by', i.invited_by, 'created_at', i.created_at, 'expires_at', i.expires_at,
                   'status', i.status) order by i.created_at)
        from iamb.invitation i where i.status = 'pending'), '[]'::jsonb));
end $$;

create or replace function api.devices(device uuid, protocol integer)
returns jsonb language plpgsql volatile security invoker set search_path = '' set statement_timeout = '8s' as $$
declare
    e jsonb := iamb_private.enter(device, protocol, 'devices');
begin
    if e ->> 'error' is not null then return e; end if;
    return jsonb_build_object('epoch', e -> 'epoch', 'error', null, 'devices', coalesce((
        select jsonb_agg(jsonb_build_object(
                   'id', d.id, 'name', d.name, 'platform', d.platform, 'app_version', d.app_version,
                   'created_at', d.created_at, 'last_seen_at', d.last_seen_at, 'revoked_at', d.revoked_at,
                   'current', d.id = device) order by d.created_at)
        from iamb.device d), '[]'::jsonb));
end $$;

create or replace function api.users(device uuid, protocol integer, ids uuid[])
returns jsonb language plpgsql volatile security invoker set search_path = '' set statement_timeout = '8s' as $$
declare
    e jsonb := iamb_private.enter(device, protocol, 'users');
begin
    if e ->> 'error' is not null then return e; end if;
    if coalesce(array_length(ids, 1), 0) > 500 then
        return jsonb_build_object('epoch', e -> 'epoch', 'error', 'too_large');
    end if;
    return jsonb_build_object('epoch', e -> 'epoch', 'error', null, 'users', coalesce((
        select jsonb_agg(jsonb_build_object('user', up.user_id, 'display_name', up.display_name))
        from iamb.user_profile up where up.user_id = any (ids)), '[]'::jsonb));
end $$;

-- Entitlements (§5.10, §6.12): the user plan's limits and features under their names, each
-- group's under `<name>@<profile id>` for groups the caller owns or administers, and the flags
-- under `flag.<key>`.
create or replace function api.entitlements(device uuid, protocol integer)
returns jsonb language plpgsql volatile security definer set search_path = '' set statement_timeout = '8s' as $$
declare
    e   jsonb := iamb_private.enter(device, protocol, 'entitlements');
    u   uuid := auth.uid();
    up  iamb.plan;
    res jsonb := '{}';
    g   record;
    gp  iamb.plan;
begin
    if e ->> 'error' is not null then return e; end if;
    up := iamb_private.user_plan(u);
    select res || coalesce(jsonb_object_agg(k, v #>> '{}'), '{}') into res
    from jsonb_each(coalesce(up.limits, '{}') || coalesce(up.features, '{}')) as x(k, v);
    for g in select m.profile_id from iamb.membership m join iamb.profile p on p.id = m.profile_id
             where m.user_id = u and m.deleted_at is null and p.deleted_at is null and m.role in ('owner', 'admin') loop
        gp := iamb_private.group_plan(g.profile_id);
        select res || coalesce(jsonb_object_agg(k || '@' || g.profile_id::text, v #>> '{}'), '{}') into res
        from jsonb_each(coalesce(gp.limits, '{}') || coalesce(gp.features, '{}')) as x(k, v);
    end loop;
    select res || coalesce(jsonb_object_agg('flag.' || f.key,
                                            coalesce((select o.value from iamb.feature_override o
                                                      where o.key = f.key and o.user_id = u), f.value) #>> '{}'), '{}')
    into res from iamb.feature_flag f;
    return jsonb_build_object('epoch', e -> 'epoch', 'error', null, 'entitlements', res);
end $$;
