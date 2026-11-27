-- Iamb server, migration 14: sessions, the account calls and the groups after the security review
-- of task 4.3 (docs/reviews/4.3-security-review.md), findings 5, 6, 10, 12, 15 and two points of
-- finding 20.
--
--  5. An offer of the ownership ends with the membership it was made to, and `accept_ownership`
--     accepts an offer only for the membership it was made to.
--  6. An account call takes no lock for a caller who is not in the group it names: the wrappers of
--     `api` check the membership before the call's core locks anything (the core checks again
--     under the lock; the restore script calls the cores directly).
-- 10. The answers to a repeated `create_group`, `invite` and `accept_invitation` give the group's
--     revision only to somebody who is in the group now; `invite` looks at the caller's role before
--     it looks at the invitation's id.
-- 12. `revoke_device` and `sign_out_everywhere` are counted by themselves, per calling device.
-- 15. `accept_ownership` recomputes the access entries.
-- 20. A session past a time limit set at the authentication service is refused.
--
-- `members` becomes an invoker function (review §6): names and roles are read under the caller's
-- policies, and one small definer helper returns the addresses to an owner or admin.

do $$
begin
    if current_user <> 'postgres' then
        raise exception 'iamb migrations must run as postgres, not %', current_user;
    end if;
end
$$;

-- The limit of the new counter (calls per minute per calling device); an operator's value stays.
update iamb.server_state set rate_limits = rate_limits || '{"revoke_calls": 10}'
where not rate_limits ? 'revoke_calls';


-- Whether the token's session still exists, has not passed a time limit the authentication service
-- set for it, was opened by one of Iamb's ways of signing in and has a device row that is not
-- revoked (C25). Every policy requires it.
create or replace function iamb_private.session_ok() returns boolean
language sql stable security definer set search_path = pg_catalog, pg_temp as $$
    select coalesce((
        select c.sid is not null
           and c.uid is not null
           and iamb_private.amr_ok(c.j)
           and exists (select 1 from auth.sessions s where s.id = c.sid and s.user_id = c.uid
                       and (s.not_after is null or s.not_after > now()))
           and exists (select 1 from iamb.device d
                       where d.session_id = c.sid and d.user_id = c.uid and d.revoked_at is null)
        from (select auth.jwt() as j, auth.uid() as uid, iamb_private.token_session() as sid) c
    ), false)
$$;

-- The first statement of every function of `api` (§5.1). Answers {error, epoch, user, ...}.
-- Calls that write or issue a position lock the calling device's row first and check it after
-- (C24). `hello` checks only the session, the protocol range and the rate; it registers the
-- device itself. Never changes `server_state`, an epoch, a feed or another device's row.
create or replace function iamb_private.enter(p_device uuid, p_protocol integer, p_call text)
returns jsonb
language plpgsql volatile security definer set search_path = pg_catalog, pg_temp as $$
declare
    st       iamb.server_state;
    uid      uuid := auth.uid();
    claims   jsonb := auth.jwt();
    sid      uuid := iamb_private.token_session();
    d        iamb.device;
    s        iamb.device;
    now_ms   bigint := iamb_private.now_ms();
    lockit   boolean;
    v_bucket text;
    lim      integer;
    v_subject uuid;
    win      bigint := now_ms - (now_ms % 60000);
    cnt      integer;
    wstart   bigint;
begin
    select * into st from iamb.server_state;
    if st.maintenance then
        return jsonb_build_object('error', 'maintenance', 'epoch', st.epoch);
    end if;
    if uid is null or sid is null or not iamb_private.amr_ok(claims)
       or not exists (select 1 from auth.sessions x where x.id = sid and x.user_id = uid
                       and (x.not_after is null or x.not_after > now())) then
        return jsonb_build_object('error', 'session_refused', 'epoch', st.epoch);
    end if;

    -- Protocol range (C23); the access check is answered from `protocol_oldest` on (§6.10).
    if p_call = 'visible_folders' then
        if p_protocol is null or p_protocol < st.protocol_oldest then
            return jsonb_build_object('error', 'upgrade_required', 'epoch', st.epoch);
        end if;
    elsif p_call <> 'hello' then
        if p_protocol is null or p_protocol < st.protocol_min then
            return jsonb_build_object('error', 'upgrade_required', 'epoch', st.epoch);
        elsif p_protocol > st.protocol_max then
            return jsonb_build_object('error', 'server_too_old', 'epoch', st.epoch);
        end if;
    end if;

    lockit := p_call not in ('hello', 'rows', 'fetch_folder', 'visible_folders', 'access', 'users',
                             'members', 'entitlements', 'check', 'conflicts', 'get_conflict',
                             'invitations', 'devices', 'revoke_device', 'sign_out_everywhere');

    if p_call <> 'hello' then
        if lockit then
            select * into d from iamb.device where id = p_device for update;
        else
            select * into d from iamb.device where id = p_device;
        end if;
        if d.id is null or d.session_id <> sid or d.user_id <> uid then
            select * into s from iamb.device where session_id = sid;
            if s.id is not null and s.revoked_at is not null then
                return jsonb_build_object('error', 'device_revoked', 'epoch', st.epoch);
            elsif d.id is not null and d.user_id = uid and d.revoked_at is not null then
                return jsonb_build_object('error', 'device_revoked', 'epoch', st.epoch);
            elsif s.id is not null then
                return jsonb_build_object('error', 'device_conflict', 'epoch', st.epoch);
            end if;
            return jsonb_build_object('error', 'device_unknown', 'epoch', st.epoch);
        end if;
        if d.revoked_at is not null then
            return jsonb_build_object('error', 'device_revoked', 'epoch', st.epoch);
        end if;
    end if;

    -- Rate limits (§6.8), counted per device in windows of one minute (per account for account
    -- calls); keyed by what was verified above, so nobody can fill another device's counter.
    v_bucket := case
        when p_call = 'push' then 'push_calls'
        when p_call = 'pull' then 'pull_calls'
        when p_call in ('fetch_folder', 'snapshot', 'rows') then 'read_pages'
        when p_call = 'hello' then 'hello_calls'
        when p_call in ('create_group', 'rename_group', 'delete_group', 'transfer_ownership',
                        'accept_ownership', 'decline_ownership', 'invite', 'withdraw_invitation',
                        'accept_invitation', 'decline_invitation', 'set_member_role',
                        'remove_member', 'leave_group', 'resolve_conflict',
                        'attachment_upload_begin', 'attachment_upload_cancel', 'rename_device',
                        'set_display_name') then 'account_calls'
        -- Finding 12: revocation has a counter of its own, per calling device, so that no other
        -- device of the account (the one to be revoked among them) can use it up.
        when p_call in ('revoke_device', 'sign_out_everywhere') then 'revoke_calls'
        else 'other_calls' end;
    lim := coalesce((st.rate_limits ->> v_bucket)::integer, case when v_bucket = 'revoke_calls' then 10 else 300 end);
    v_subject := case when v_bucket = 'account_calls' then uid
                    when p_call = 'hello' then sid
                    else d.id end;
    insert into iamb.rate_bucket as rb (subject, bucket, window_start, count)
    values (v_subject, v_bucket, win, 1)
    on conflict (subject, bucket) do update
        set count = case when rb.window_start = win then rb.count + 1 else 1 end,
            window_start = win
    returning rb.count, rb.window_start into cnt, wstart;
    if cnt > lim then
        return jsonb_build_object('error', 'rate_limited', 'epoch', st.epoch,
                                  'retry_after_ms', greatest(wstart + 60000 - now_ms, 1));
    end if;

    if p_call <> 'hello' and (d.last_seen_at is null or d.last_seen_at < now_ms - 3600000) then
        update iamb.device set last_seen_at = now_ms where id = d.id;
    end if;
    return jsonb_build_object('error', null, 'epoch', st.epoch, 'user', uid, 'session', sid);
end
$$;

-- Ends a membership (remove, leave): the membership's tombstone, the shares from that person's
-- folders into the group, an offer of the ownership made to that person, and every access entry
-- the membership gave (C16, G13).
create or replace function iamb_private.end_membership(u uuid, dev uuid, g uuid, target uuid, p_call text)
returns jsonb
language plpgsql volatile set search_path = pg_catalog, pg_temp as $$
declare
    sharers uuid[];
    locks   uuid[];
    mid     uuid;
    r       bigint;
    s       record;
    offer_owner uuid;
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
    -- An offer of the ownership was made to this membership and ends with it (finding 5); both
    -- sides are told, as for a withdrawn offer.
    update iamb.profile set offered_owner = null, offered_at = null
    where id = g and offered_owner = target
    returning owner_user_id into offer_owner;
    if offer_owner is not null then
        perform iamb_private.queue(array[offer_owner, target], 'invitations', null, null, null, null, null, null);
    end if;
    perform iamb_private.refresh_access(locks);
    perform iamb_private.queue(array[target], 'entitlements', null, null, null, null, null, null);
    perform iamb_private.log(p_call, jsonb_build_object('user', u, 'group', g, 'person', target, 'mark', r, 'answer', 'ok'));
    return jsonb_build_object('error', null, 'mark', r);
end
$$;

create or replace function iamb_private.acct_accept_ownership(u uuid, dev uuid, g uuid)
returns jsonb
language plpgsql volatile set search_path = pg_catalog, pg_temp as $$
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
    -- The offer names the caller, is not older than 14 days, and was made to the membership the
    -- caller has now: one that began after the offer is another membership (finding 5).
    if pg_.id is null or pg_.offered_owner is distinct from u or pg_.offered_at < now_ms - 14 * 86400000::bigint
       or not exists (select 1 from iamb.membership m
                      where m.profile_id = g and m.user_id = u and m.deleted_at is null
                        and m.created_at <= pg_.offered_at) then
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
    -- The two roles changed, and with them the permission on the group's folders (finding 15).
    perform iamb_private.refresh_access(array[g]);
    perform iamb_private.queue(before || iamb_private.readers('profile', g), 'row', 'profile', g, null, null, null, null);
    perform iamb_private.queue(array[u, pg_.owner_user_id], 'entitlements', null, null, null, null, null, null);
    perform iamb_private.log('accept_ownership', jsonb_build_object('user', u, 'group', g, 'mark', r, 'answer', 'ok'));
    return jsonb_build_object('error', null, 'mark', r);
end
$$;

create or replace function iamb_private.acct_create_group(u uuid, dev uuid, p_id uuid, p_kind text, p_name text)
returns jsonb
language plpgsql volatile set search_path = pg_catalog, pg_temp as $$
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
        -- A repeated call (its answer was lost) answers as the first did, for as long as the
        -- caller is in the group; afterwards the id is one in use, like any other (finding 10).
        if ex.kind = p_kind and ex.deleted_at is null
           and exists (select 1 from iamb.membership m
                       where m.profile_id = ex.id and m.user_id = u and m.deleted_at is null and m.created_rev = (
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
    values (p_id, now_ms, now_ms, u, r, r, dev, p_kind, p_name, u, r)
    on conflict (id) do nothing;
    if not found then
        -- Another request took the id between the read above and here.
        return iamb_private.err('invalid');
    end if;
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

create or replace function iamb_private.acct_invite(u uuid, dev uuid, p_id uuid, g uuid, p_email text, p_role text)
returns jsonb
language plpgsql volatile set search_path = pg_catalog, pg_temp as $$
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
    -- The caller's role first, and before the lock (findings 6 and 10): somebody who is not in
    -- the group learns nothing about an invitation's id and holds no lock.
    if iamb_private.group_role(u, g) is null then
        return iamb_private.err('not_found');
    end if;
    perform iamb_private.lock_profiles(array[g]);
    role := iamb_private.group_role(u, g);
    if role is null then
        return iamb_private.err('not_found');
    elsif role not in ('owner', 'admin') then
        return iamb_private.err('forbidden');
    end if;
    select * into ex from iamb.invitation where id = p_id;
    if ex.id is not null then
        if ex.profile_id = g and ex.email = em and ex.invited_by = u then
            return jsonb_build_object('error', null, 'mark', (select head_rev from iamb.profile where id = g));
        end if;
        return iamb_private.err('invalid');
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

create or replace function iamb_private.acct_accept_invitation(u uuid, dev uuid, p_id uuid)
returns jsonb
language plpgsql volatile set search_path = pg_catalog, pg_temp as $$
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
    -- A repeated call answers as the first did while the caller is still in the group; somebody
    -- who has left it since gets no word of the group's revisions (finding 10).
    if inv.status = 'accepted' and inv.accepted_by = u and iamb_private.group_role(u, inv.profile_id) is not null then
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

create or replace function iamb_private.acct_withdraw_invitation(u uuid, dev uuid, p_id uuid)
returns jsonb
language plpgsql volatile set search_path = pg_catalog, pg_temp as $$
declare
    inv  iamb.invitation;
    role text;
    r    bigint;
    invitee uuid;
begin
    select * into inv from iamb.invitation where id = p_id;
    -- Checked before the lock (finding 6) and again under it.
    if inv.id is null or iamb_private.group_role(u, inv.profile_id) is null then
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

-- ---------------------------------------------------------------------------------------------
-- Wrappers that name a group: no lock for a caller who is not in it (finding 6). Each answers what
-- its core answers to such a caller.
-- ---------------------------------------------------------------------------------------------

create or replace function api.rename_group(device uuid, protocol integer, "group" uuid, name text)
returns jsonb language plpgsql volatile security definer set search_path = pg_catalog, pg_temp set statement_timeout = '8s' as $$
declare e jsonb := iamb_private.enter(device, protocol, 'rename_group');
begin
    if e ->> 'error' is not null then return e; end if;
    if iamb_private.group_role(auth.uid(), "group") is null then
        return iamb_private.finish(e, iamb_private.err('not_found'));
    end if;
    return iamb_private.finish(e, iamb_private.acct_rename_group(auth.uid(), device, "group", name));
end $$;

create or replace function api.delete_group(device uuid, protocol integer, "group" uuid)
returns jsonb language plpgsql volatile security definer set search_path = pg_catalog, pg_temp set statement_timeout = '30s' as $$
declare e jsonb := iamb_private.enter(device, protocol, 'delete_group');
begin
    if e ->> 'error' is not null then return e; end if;
    -- In the group, or its owner calling again for a group already deleted.
    if iamb_private.group_role(auth.uid(), "group") is null
       and not exists (select 1 from iamb.profile p where p.id = "group" and p.kind <> 'personal'
                       and p.deleted_at is not null and p.owner_user_id = auth.uid()) then
        return iamb_private.finish(e, iamb_private.err('not_found'));
    end if;
    return iamb_private.finish(e, iamb_private.acct_delete_group(auth.uid(), device, "group"));
end $$;

create or replace function api.transfer_ownership(device uuid, protocol integer, "group" uuid, "user" uuid default null)
returns jsonb language plpgsql volatile security definer set search_path = pg_catalog, pg_temp set statement_timeout = '8s' as $$
declare e jsonb := iamb_private.enter(device, protocol, 'transfer_ownership');
begin
    if e ->> 'error' is not null then return e; end if;
    if iamb_private.group_role(auth.uid(), "group") is null then
        return iamb_private.finish(e, iamb_private.err('not_found'));
    end if;
    return iamb_private.finish(e, iamb_private.acct_transfer_ownership(auth.uid(), device, "group", "user"));
end $$;

create or replace function api.accept_ownership(device uuid, protocol integer, "group" uuid)
returns jsonb language plpgsql volatile security definer set search_path = pg_catalog, pg_temp set statement_timeout = '8s' as $$
declare e jsonb := iamb_private.enter(device, protocol, 'accept_ownership');
begin
    if e ->> 'error' is not null then return e; end if;
    if iamb_private.group_role(auth.uid(), "group") is null then
        return iamb_private.finish(e, iamb_private.err('no_offer'));
    end if;
    return iamb_private.finish(e, iamb_private.acct_accept_ownership(auth.uid(), device, "group"));
end $$;

create or replace function api.decline_ownership(device uuid, protocol integer, "group" uuid)
returns jsonb language plpgsql volatile security definer set search_path = pg_catalog, pg_temp set statement_timeout = '8s' as $$
declare e jsonb := iamb_private.enter(device, protocol, 'decline_ownership');
begin
    if e ->> 'error' is not null then return e; end if;
    if iamb_private.group_role(auth.uid(), "group") is null then
        return iamb_private.finish(e, iamb_private.err('no_offer'));
    end if;
    return iamb_private.finish(e, iamb_private.acct_decline_ownership(auth.uid(), device, "group"));
end $$;

create or replace function api.set_member_role(device uuid, protocol integer, "group" uuid, "user" uuid, role text)
returns jsonb language plpgsql volatile security definer set search_path = pg_catalog, pg_temp set statement_timeout = '8s' as $$
declare e jsonb := iamb_private.enter(device, protocol, 'set_member_role');
begin
    if e ->> 'error' is not null then return e; end if;
    if iamb_private.group_role(auth.uid(), "group") is null then
        return iamb_private.finish(e, iamb_private.err('not_found'));
    end if;
    return iamb_private.finish(e, iamb_private.acct_set_member_role(auth.uid(), device, "group", "user", role));
end $$;

create or replace function api.remove_member(device uuid, protocol integer, "group" uuid, "user" uuid)
returns jsonb language plpgsql volatile security definer set search_path = pg_catalog, pg_temp set statement_timeout = '30s' as $$
declare e jsonb := iamb_private.enter(device, protocol, 'remove_member');
begin
    if e ->> 'error' is not null then return e; end if;
    if iamb_private.group_role(auth.uid(), "group") is null then
        return iamb_private.finish(e, iamb_private.err('not_found'));
    end if;
    return iamb_private.finish(e, iamb_private.acct_remove_member(auth.uid(), device, "group", "user"));
end $$;

create or replace function api.leave_group(device uuid, protocol integer, "group" uuid)
returns jsonb language plpgsql volatile security definer set search_path = pg_catalog, pg_temp set statement_timeout = '30s' as $$
declare e jsonb := iamb_private.enter(device, protocol, 'leave_group');
begin
    if e ->> 'error' is not null then return e; end if;
    if iamb_private.group_role(auth.uid(), "group") is null then
        return iamb_private.finish(e, iamb_private.err('not_a_member'));
    end if;
    return iamb_private.finish(e, iamb_private.acct_leave_group(auth.uid(), device, "group"));
end $$;

-- ---------------------------------------------------------------------------------------------
-- `members` (review §6): an invoker function. Names and roles come through the policies on
-- `membership` and `user_profile`; the addresses, which are in the authentication service's table,
-- through one definer helper that answers an owner or admin of the group and nobody else.
-- ---------------------------------------------------------------------------------------------

create or replace function iamb_private.member_addresses(p_group uuid)
returns table (user_id uuid, email text)
language sql stable security definer set search_path = pg_catalog, pg_temp as $$
    select m.user_id, au.email::text
    from iamb.membership m join auth.users au on au.id = m.user_id
    where m.profile_id = p_group and m.deleted_at is null
      and iamb_private.session_ok()
      and iamb_private.role_in(p_group) in ('owner', 'admin')
$$;

create or replace function api.members(device uuid, protocol integer, "group" uuid)
returns jsonb language plpgsql volatile security invoker set search_path = pg_catalog, pg_temp set statement_timeout = '8s' as $$
declare
    e jsonb := iamb_private.enter(device, protocol, 'members');
begin
    if e ->> 'error' is not null then return e; end if;
    if iamb_private.role_in("group") is null then
        return jsonb_build_object('epoch', e -> 'epoch', 'error', 'not_found');
    end if;
    return jsonb_build_object('epoch', e -> 'epoch', 'error', null, 'members', coalesce((
        select jsonb_agg(jsonb_build_object(
                   'user', m.user_id, 'role', m.role,
                   'display_name', coalesce(up.display_name, ''),
                   'email', a.email)
               order by m.created_rev)
        from iamb.membership m
        left join iamb.user_profile up on up.user_id = m.user_id
        left join iamb_private.member_addresses("group") a on a.user_id = m.user_id
        where m.profile_id = "group" and m.deleted_at is null), '[]'::jsonb));
end $$;

revoke execute on function iamb_private.member_addresses(uuid) from public, anon, service_role;
grant execute on function iamb_private.member_addresses(uuid) to authenticated;
