-- Iamb server, migration 3: the helpers of `iamb_private` (docs/sync-design.md §5.1, §5.4,
-- §5.5, C24). Only the gate helpers at the top may be executed by `authenticated`; everything
-- else is executable by the tables' owner alone and is reached from inside definer functions,
-- the daily job or the operator's restore script.

do $$
begin
    if current_user <> 'postgres' then
        raise exception 'iamb migrations must run as postgres, not %', current_user;
    end if;
end
$$;

-- ---------------------------------------------------------------------------------------------
-- Small pure helpers (owner only)
-- ---------------------------------------------------------------------------------------------

-- The server's clock in UTC milliseconds.
create or replace function iamb_private.now_ms() returns bigint
language sql volatile set search_path = '' as $$
    select (extract(epoch from clock_timestamp()) * 1000)::bigint
$$;

-- A uuid from text, or null when the text is not one (never an error).
create or replace function iamb_private.try_uuid(t text) returns uuid
language sql immutable set search_path = '' as $$
    select case when t ~* '^[0-9a-f]{8}-?[0-9a-f]{4}-?[0-9a-f]{4}-?[0-9a-f]{4}-?[0-9a-f]{4}-?[0-9a-f]{8}$'
                then t::uuid end
$$;

-- The caller's session id from the token, or null.
create or replace function iamb_private.token_session() returns uuid
language sql stable set search_path = '' as $$
    select iamb_private.try_uuid(auth.jwt() ->> 'session_id')
$$;

-- Whether every method in the token's `amr` claim is one of Iamb's ways of signing in (C25). A
-- token without the claim, or with an empty one, is refused.
create or replace function iamb_private.amr_ok(j jsonb) returns boolean
language sql stable set search_path = '' as $$
    select coalesce(
        jsonb_typeof(j -> 'amr') = 'array'
        and jsonb_array_length(j -> 'amr') > 0
        and not exists (
            select 1
            from jsonb_array_elements(j -> 'amr') e, iamb.server_state st
            where not (coalesce(case when jsonb_typeof(e) = 'object' then e ->> 'method' end,
                                case when jsonb_typeof(e) = 'string' then e #>> '{}' end, '')
                       = any (st.sign_in_methods))),
        false)
$$;

-- The share id derived from a folder and a group (§3.4): the first 16 bytes of SHA-256 over
-- `iamb:folder_share:` || folder id bytes || group id bytes, version 8, variant 10.
create or replace function iamb_private.share_id(p_folder uuid, p_group uuid) returns uuid
language plpgsql immutable set search_path = '' as $$
declare
    h bytea := substring(sha256(convert_to('iamb:folder_share:', 'UTF8') || uuid_send(p_folder) || uuid_send(p_group)) from 1 for 16);
begin
    h := set_byte(h, 6, (get_byte(h, 6) & 15) | 128);
    h := set_byte(h, 8, (get_byte(h, 8) & 63) | 128);
    return encode(h, 'hex')::uuid;
end
$$;

-- One line of the function log (§11.1): ids, counts and codes, never row content. It goes to the
-- Postgres server log, which the platform keeps outside the database with its own time.
create or replace function iamb_private.log(p_event text, p_detail jsonb) returns void
language plpgsql volatile set search_path = '' as $$
begin
    raise log 'iamb_log %', jsonb_build_object('event', p_event, 'at', iamb_private.now_ms()) || coalesce(p_detail, '{}');
end
$$;

-- ---------------------------------------------------------------------------------------------
-- Gate helpers (§5.1): `security definer`, take the caller from the token, return nothing about
-- anyone else, and write nothing but the caller's own device row and rate counters.
-- ---------------------------------------------------------------------------------------------

-- Whether the token's session still exists, was opened by one of Iamb's ways of signing in and has
-- a device row that is not revoked (C25). Every policy requires it.
create or replace function iamb_private.session_ok() returns boolean
language sql stable security definer set search_path = '' as $$
    select coalesce((
        select c.sid is not null
           and c.uid is not null
           and iamb_private.amr_ok(c.j)
           and exists (select 1 from auth.sessions s where s.id = c.sid and s.user_id = c.uid)
           and exists (select 1 from iamb.device d
                       where d.session_id = c.sid and d.user_id = c.uid and d.revoked_at is null)
        from (select auth.jwt() as j, auth.uid() as uid, iamb_private.token_session() as sid) c
    ), false)
$$;

-- Whether the caller has a live membership in the live group `p`.
create or replace function iamb_private.member_of(p uuid) returns boolean
language sql stable security definer set search_path = '' as $$
    select exists (
        select 1 from iamb.membership m join iamb.profile g on g.id = m.profile_id
        where m.profile_id = p and m.user_id = auth.uid() and m.deleted_at is null
          and g.deleted_at is null and g.kind <> 'personal')
$$;

-- The caller's role in the live group `p`, or null.
create or replace function iamb_private.role_in(p uuid) returns text
language sql stable security definer set search_path = '' as $$
    select m.role from iamb.membership m join iamb.profile g on g.id = m.profile_id
    where m.profile_id = p and m.user_id = auth.uid() and m.deleted_at is null
      and g.deleted_at is null and g.kind <> 'personal'
$$;

-- Whether the caller and account `u` have a live membership in one live group.
create or replace function iamb_private.shares_group_with(u uuid) returns boolean
language sql stable security definer set search_path = '' as $$
    select exists (
        select 1
        from iamb.membership a
        join iamb.membership b on b.profile_id = a.profile_id
        join iamb.profile g on g.id = a.profile_id
        where a.user_id = auth.uid() and b.user_id = u
          and a.deleted_at is null and b.deleted_at is null and g.deleted_at is null)
$$;

-- The caller's confirmed email address in lower case, or null when it is not confirmed (§5.8).
create or replace function iamb_private.own_address() returns text
language sql stable security definer set search_path = '' as $$
    select lower(u.email) from auth.users u
    where u.id = auth.uid() and u.email_confirmed_at is not null and u.email is not null
$$;

-- The first statement of every function of `api` (§5.1). Answers {error, epoch, user, ...}.
-- Calls that write or issue a position lock the calling device's row first and check it after
-- (C24). `hello` checks only the session, the protocol range and the rate; it registers the
-- device itself. Never changes `server_state`, an epoch, a feed or another device's row.
create or replace function iamb_private.enter(p_device uuid, p_protocol integer, p_call text)
returns jsonb
language plpgsql volatile security definer set search_path = '' as $$
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
       or not exists (select 1 from auth.sessions x where x.id = sid and x.user_id = uid) then
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
                        'attachment_upload_begin', 'rename_device', 'set_display_name',
                        'revoke_device', 'sign_out_everywhere') then 'account_calls'
        else 'other_calls' end;
    lim := coalesce((st.rate_limits ->> v_bucket)::integer, 300);
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

-- The position check of C25. `p_call` is `pull`, `push` or `snapshot`. With `p_new_seq` it also
-- issues a new position and sets the device's pair: presented becomes the position this call
-- presented, last the one it issues. Touches the caller's own device row only, and only after
-- `enter` has locked it.
create or replace function iamb_private.position(p_device uuid, p_presented text, p_call text,
                                                 p_new_seq bigint default null)
returns jsonb
language plpgsql volatile security definer set search_path = '' as $$
declare
    st     iamb.server_state;
    d      iamb.device;
    fh     iamb.feed_head;
    parts  text[];
    p_ep   uuid;
    p_seq  bigint;
    ok     boolean := false;
    newpos text;
    ep     iamb.epoch;
    lost   jsonb;
begin
    select * into st from iamb.server_state;
    select * into d from iamb.device
    where id = p_device and user_id = auth.uid() and session_id = iamb_private.token_session()
      and revoked_at is null;
    if d.id is null then
        return jsonb_build_object('error', 'device_unknown');
    end if;
    select * into fh from iamb.feed_head where user_id = d.user_id;
    if fh.user_id is null then
        fh.head := 0;
        fh.floor := 0;
    end if;
    parts := string_to_array(coalesce(p_presented, ''), ':');
    if array_length(parts, 1) = 3 and parts[2] ~ '^[0-9]{1,18}$' then
        p_ep := iamb_private.try_uuid(parts[1]);
        p_seq := parts[2]::bigint;
    end if;

    if p_call in ('pull', 'push') then
        if p_ep is null or p_seq is null then
            return jsonb_build_object('error', 'reset_required', 'reason', 'expired');
        elsif p_ep <> st.epoch then
            return jsonb_build_object('error', 'reset_required', 'reason', 'epoch');
        elsif p_seq < fh.floor then
            return jsonb_build_object('error', 'reset_required', 'reason', 'expired');
        elsif d.position_token is null then
            return jsonb_build_object('error', 'reset_required', 'reason', 'expired');
        elsif p_seq > fh.head then
            perform iamb_private.log('server_behind', jsonb_build_object(
                'user', d.user_id, 'device', d.id, 'claimed_seq', p_seq, 'head', fh.head));
            return jsonb_build_object('error', 'server_behind');
        end if;
        ok := p_presented = d.position_token or p_presented = d.position_token_before;
    elsif p_call = 'snapshot' then
        ok := d.position_token is null
              or p_ep is null or p_ep <> st.epoch or p_seq < fh.floor
              or p_presented = d.position_token or p_presented = d.position_token_before;
    end if;
    if not ok then
        return jsonb_build_object('error', 'device_conflict');
    end if;
    if p_new_seq is not null then
        newpos := st.epoch::text || ':' || p_new_seq::text || ':' || replace(gen_random_uuid()::text, '-', '');
        update iamb.device set position_token_before = p_presented, position_token = newpos
        where id = d.id;
    end if;
    -- A snapshot that presents a position of an older epoch learns what the restore lost (§6.5,
    -- C18): for every profile the caller can read, the last revision of it the restored database
    -- held, in the epoch that followed the device's (the current one when that epoch is unknown).
    if p_call = 'snapshot' and p_ep is not null and p_ep <> st.epoch then
        select e2.* into ep from iamb.epoch e1 join iamb.epoch e2 on e2.started_at > e1.started_at
        where e1.id = p_ep order by e2.started_at limit 1;
        if ep.id is null then
            select * into ep from iamb.epoch where id = st.epoch;
        end if;
        lost := jsonb_build_object('epoch', ep.id, 'started_at', ep.started_at, 'profiles', coalesce((
            select jsonb_agg(jsonb_build_object('profile', x.pid, 'backup_rev', coalesce(epr.backup_rev, 0)) order by x.pid)
            from (select fa.profile_id as pid from iamb.folder_access fa where fa.user_id = d.user_id
                  union select p.id from iamb.profile p where p.owner_user_id = d.user_id and p.kind = 'personal'
                  union select m.profile_id from iamb.membership m where m.user_id = d.user_id and m.deleted_at is null) x
            left join iamb.epoch_profile epr on epr.epoch_id = ep.id and epr.profile_id = x.pid), '[]'::jsonb));
        return jsonb_build_object('error', null, 'position', newpos, 'epoch', p_ep, 'seq', p_seq, 'lost', lost);
    end if;
    return jsonb_build_object('error', null, 'position', newpos, 'epoch', p_ep, 'seq', p_seq);
end
$$;

-- ---------------------------------------------------------------------------------------------
-- Locks and revisions (C24, §5.1)
-- ---------------------------------------------------------------------------------------------

-- Locks the given profile rows in the order of C24: groups by id, then personal profiles by id.
-- A caller holding locks already must only add profiles that sort after them.
create or replace function iamb_private.lock_profiles(p_ids uuid[]) returns void
language plpgsql volatile set search_path = '' as $$
begin
    perform 1 from iamb.profile where id = any (p_ids)
    order by (kind = 'personal'), id
    for update;
end
$$;

-- The next revision, drawn under the locks of the given profiles; records it as their `head_rev`.
create or replace function iamb_private.next_rev(p_profiles uuid[]) returns bigint
language plpgsql volatile set search_path = '' as $$
declare
    r bigint := nextval('iamb.rev_seq');
begin
    update iamb.profile set head_rev = greatest(head_rev, r) where id = any (p_profiles);
    return r;
end
$$;

-- ---------------------------------------------------------------------------------------------
-- Per-transaction scratch tables: pending feed entries and rows written in this transaction.
-- ---------------------------------------------------------------------------------------------

create or replace function iamb_private.tx_begin() returns void
language plpgsql volatile set search_path = '' set client_min_messages = 'warning' as $$
begin
    create temp table if not exists iamb_feed_pending (
        ord        bigserial,
        user_id    uuid not null,
        kind       text not null,
        entity     text,
        row_id     uuid,
        folder_id  uuid,
        profile_id uuid,
        sha256     text,
        detail     jsonb
    ) on commit drop;
    create temp table if not exists iamb_tx_rows (
        entity  text not null,
        id      uuid not null,
        created boolean not null,
        primary key (entity, id)
    ) on commit drop;
end
$$;

create or replace function iamb_private.queue(p_users uuid[], p_kind text, p_entity text, p_row uuid,
                                              p_folder uuid, p_profile uuid, p_sha256 text,
                                              p_detail jsonb) returns void
language plpgsql volatile set search_path = '' as $$
begin
    perform iamb_private.tx_begin();
    insert into pg_temp.iamb_feed_pending (user_id, kind, entity, row_id, folder_id, profile_id, sha256, detail)
    select distinct u, p_kind, p_entity, p_row, p_folder, p_profile, p_sha256, p_detail
    from unnest(p_users) u where u is not null;
end
$$;

create or replace function iamb_private.mark_written(p_entity text, p_id uuid, p_created boolean) returns void
language plpgsql volatile set search_path = '' as $$
begin
    perform iamb_private.tx_begin();
    insert into pg_temp.iamb_tx_rows (entity, id, created) values (p_entity, p_id, p_created)
    on conflict (entity, id) do update set created = pg_temp.iamb_tx_rows.created or excluded.created;
end
$$;

create or replace function iamb_private.created_in_tx(p_entity text, p_id uuid) returns boolean
language plpgsql volatile set search_path = '' as $$
begin
    perform iamb_private.tx_begin();
    return exists (select 1 from pg_temp.iamb_tx_rows where entity = p_entity and id = p_id and created);
end
$$;

-- Writes the collected entries last (§5.5, C24 step 4): locks the `feed_head` rows of every user
-- concerned in the order of the user ids, gives each entry `seq = head + 1`, replaces an older
-- `row` entry for the same user and row, and pokes each user's live channel. The locks are held to
-- the commit, so entries become visible in `seq` order.
create or replace function iamb_private.feed_flush() returns void
language plpgsql volatile set search_path = '' as $$
declare
    users  uuid[];
    ep     uuid;
    now_ms bigint := iamb_private.now_ms();
    h      record;
begin
    perform iamb_private.tx_begin();
    select array_agg(distinct user_id order by user_id) into users from pg_temp.iamb_feed_pending;
    if users is null then
        delete from pg_temp.iamb_tx_rows;
        return;
    end if;
    select epoch into ep from iamb.server_state;
    insert into iamb.feed_head (user_id) select u from unnest(users) u order by u on conflict (user_id) do nothing;
    perform 1 from iamb.feed_head where user_id = any (users) order by user_id for update;

    -- A folder created in this transaction whose own `row` entry a user gets needs no
    -- `folder_visible` for that user as well.
    delete from pg_temp.iamb_feed_pending p
    where p.kind = 'folder_visible'
      and exists (select 1 from pg_temp.iamb_tx_rows t where t.entity = 'folder' and t.id = p.folder_id and t.created)
      and exists (select 1 from pg_temp.iamb_feed_pending q
                  where q.user_id = p.user_id and q.kind = 'row' and q.entity = 'folder' and q.row_id = p.folder_id);
    -- Within this transaction, the last `row` entry for a user and row stands for the others.
    delete from pg_temp.iamb_feed_pending p
    where p.kind = 'row' and exists (
        select 1 from pg_temp.iamb_feed_pending q
        where q.user_id = p.user_id and q.kind = 'row' and q.entity = p.entity
          and q.row_id = p.row_id and q.ord > p.ord);
    delete from iamb.feed f using pg_temp.iamb_feed_pending p
    where p.kind = 'row' and f.user_id = p.user_id and f.kind = 'row'
      and f.entity = p.entity and f.row_id = p.row_id;
    insert into iamb.feed (user_id, seq, kind, entity, row_id, folder_id, profile_id, sha256, detail, created_at)
    select p.user_id,
           fh.head + row_number() over (partition by p.user_id order by p.ord),
           p.kind, p.entity, p.row_id, p.folder_id, p.profile_id, p.sha256, p.detail, now_ms
    from pg_temp.iamb_feed_pending p join iamb.feed_head fh on fh.user_id = p.user_id;
    for h in
        update iamb.feed_head fh set head = fh.head + c.n
        from (select user_id, count(*) as n from pg_temp.iamb_feed_pending group by user_id) c
        where fh.user_id = c.user_id
        returning fh.user_id, fh.head
    loop
        perform realtime.send(jsonb_build_object('epoch', ep, 'head', h.head), 'head',
                              'feed:' || h.user_id::text, true);
    end loop;
    delete from pg_temp.iamb_feed_pending;
    delete from pg_temp.iamb_tx_rows;
end
$$;

-- ---------------------------------------------------------------------------------------------
-- Access entries (§5.4, C21)
-- ---------------------------------------------------------------------------------------------

-- The rule of §4.2 for the folders of the given profiles, computed from `folder`,
-- `folder_share`, `membership` and `profile` alone. A deleted folder has the entries it would
-- have if it were live, from the shares and memberships that are live now.
create or replace function iamb_private.compute_access(p_profiles uuid[])
returns table (user_id uuid, folder_id uuid, profile_id uuid, permission text, via text, root_folder_id uuid)
language sql stable set search_path = '' as $$
    with recursive tree as (
        select f.id, f.profile_id, f.id as top, array[f.id] as path
        from iamb.folder f
        where f.profile_id = any (p_profiles) and f.parent_id is null
        union all
        select c.id, c.profile_id, t.top, t.path || c.id
        from iamb.folder c join tree t on c.parent_id = t.id
        where not c.id = any (t.path)
    ),
    prof as (
        select p.id, p.kind, p.owner_user_id from iamb.profile p
        where p.id = any (p_profiles) and p.deleted_at is null
    ),
    grants as (
        select pr.owner_user_id as user_id, t.id as folder_id, t.profile_id,
               3 as rank, 'own' as via, 1 as root_depth, t.top as root, 3 as via_rank
        from tree t join prof pr on pr.id = t.profile_id
        where pr.kind = 'personal'
        union all
        select m.user_id, t.id, t.profile_id,
               case when m.role in ('owner', 'admin') then 3
                    when tf.member_permission = 'edit' then 2 else 1 end,
               'group', 1, t.top, 2
        from tree t
        join prof pr on pr.id = t.profile_id and pr.kind <> 'personal'
        join iamb.membership m on m.profile_id = pr.id and m.deleted_at is null
        join iamb.folder tf on tf.id = t.top
        union all
        select m.user_id, t.id, t.profile_id,
               case when s.permission = 'edit' then 2 else 1 end,
               'share', array_position(t.path, s.folder_id), s.folder_id, 1
        from tree t
        join prof pr on pr.id = t.profile_id and pr.kind = 'personal'
        join iamb.folder_share s on s.folder_id = any (t.path) and s.deleted_at is null
        join iamb.profile g on g.id = s.profile_id and g.deleted_at is null and g.kind <> 'personal'
        join iamb.membership m on m.profile_id = g.id and m.deleted_at is null
    )
    select g.user_id, g.folder_id, g.profile_id,
           (array['view', 'edit', 'manage'])[max(g.rank)],
           (array_agg(g.via order by g.rank desc, g.via_rank desc))[1],
           (array_agg(g.root order by g.root_depth, g.via_rank desc))[1]
    from grants g
    group by g.user_id, g.folder_id, g.profile_id
$$;

-- Recomputes the access entries of the folders of the given profiles and writes the visibility
-- entries of the feed (C21): for a user, `folder_visible` for the top of each newly readable tree
-- (the feed writer drops it for a folder created in this transaction whose own `row` entry the
-- user gets) and
-- `folder_revoked` for the top of each tree that stopped being readable, with the parts inside it
-- that stay readable in `except`. Called in the same transaction as every change that can change
-- the entries, under the locks of C24.
create or replace function iamb_private.refresh_access(p_profiles uuid[]) returns void
language plpgsql volatile set search_path = '' set client_min_messages = 'warning' as $$
declare
    now_ms bigint := iamb_private.now_ms();
    r      record;
begin
    perform iamb_private.tx_begin();
    create temp table if not exists iamb_acc_new (
        user_id uuid, folder_id uuid, profile_id uuid, permission text, via text, root_folder_id uuid
    ) on commit drop;
    create temp table if not exists iamb_acc_old (
        user_id uuid, folder_id uuid, profile_id uuid, permission text, via text, root_folder_id uuid,
        granted_at bigint
    ) on commit drop;
    delete from pg_temp.iamb_acc_new;
    delete from pg_temp.iamb_acc_old;
    insert into pg_temp.iamb_acc_new select * from iamb_private.compute_access(p_profiles);
    insert into pg_temp.iamb_acc_old
        select fa.user_id, fa.folder_id, fa.profile_id, fa.permission, fa.via, fa.root_folder_id, fa.granted_at
        from iamb.folder_access fa where fa.profile_id = any (p_profiles);

    -- Visibility entries first, computed from the difference.
    -- Revoked: the tops of the lost trees, with what stays readable inside them.
    for r in
        select o.user_id, o.folder_id
        from pg_temp.iamb_acc_old o
        join iamb.folder f on f.id = o.folder_id
        where not exists (select 1 from pg_temp.iamb_acc_new n where n.user_id = o.user_id and n.folder_id = o.folder_id)
          and (f.parent_id is null or not exists (
                select 1 from pg_temp.iamb_acc_old o2
                where o2.user_id = o.user_id and o2.folder_id = f.parent_id
                  and not exists (select 1 from pg_temp.iamb_acc_new n2 where n2.user_id = o2.user_id and n2.folder_id = o2.folder_id)))
        order by o.user_id, o.folder_id
    loop
        perform iamb_private.queue(array[r.user_id], 'folder_revoked', 'folder', null, r.folder_id, null, null,
            jsonb_build_object('except', coalesce((
                with recursive sub as (
                    select c.id, c.parent_id from iamb.folder c where c.parent_id = r.folder_id
                    union all
                    select c.id, c.parent_id from iamb.folder c join sub on c.parent_id = sub.id
                )
                select jsonb_agg(sub.id order by sub.id) from sub
                where exists (select 1 from pg_temp.iamb_acc_new n where n.user_id = r.user_id and n.folder_id = sub.id)
                  and not exists (select 1 from pg_temp.iamb_acc_new n where n.user_id = r.user_id and n.folder_id = sub.parent_id)
            ), '[]'::jsonb)));
    end loop;
    -- Visible: the tops of the gained trees.
    for r in
        select n.user_id, n.folder_id
        from pg_temp.iamb_acc_new n
        join iamb.folder f on f.id = n.folder_id
        where f.deleted_at is null
          and not exists (select 1 from pg_temp.iamb_acc_old o where o.user_id = n.user_id and o.folder_id = n.folder_id)
          and (f.parent_id is null or not exists (
                select 1 from pg_temp.iamb_acc_new n2
                where n2.user_id = n.user_id and n2.folder_id = f.parent_id
                  and not exists (select 1 from pg_temp.iamb_acc_old o2 where o2.user_id = n2.user_id and o2.folder_id = n2.folder_id)))
        order by n.user_id, n.folder_id
    loop
        perform iamb_private.queue(array[r.user_id], 'folder_visible', 'folder', null, r.folder_id, null, null, null);
    end loop;

    -- Then the entries themselves.
    delete from iamb.folder_access fa
    where fa.profile_id = any (p_profiles)
      and not exists (select 1 from pg_temp.iamb_acc_new n where n.user_id = fa.user_id and n.folder_id = fa.folder_id);
    update iamb.folder_access fa
    set permission = n.permission, via = n.via, root_folder_id = n.root_folder_id
    from pg_temp.iamb_acc_new n
    where n.user_id = fa.user_id and n.folder_id = fa.folder_id
      and (fa.permission, fa.via, fa.root_folder_id) is distinct from (n.permission, n.via, n.root_folder_id);
    -- `granted_at` of a new entry is the time access to its root began: that of the root's own
    -- entry when it was already there, else now (D13 orders shared folders by it).
    insert into iamb.folder_access (user_id, folder_id, profile_id, permission, via, granted_at, root_folder_id)
    select n.user_id, n.folder_id, n.profile_id, n.permission, n.via,
           coalesce((select o.granted_at from pg_temp.iamb_acc_old o
                     where o.user_id = n.user_id and o.folder_id = n.root_folder_id
                       and o.root_folder_id = n.root_folder_id), now_ms),
           n.root_folder_id
    from pg_temp.iamb_acc_new n
    where not exists (select 1 from pg_temp.iamb_acc_old o where o.user_id = n.user_id and o.folder_id = n.folder_id);
end
$$;

-- The slow reference of §5.4: the whole table the rule of §4.2 gives, computed folder by folder
-- by walking up the tree, independently of `compute_access`. Task 4.3 compares it with
-- `folder_access` after every step (G1); the policy tests do too.
create or replace function iamb_private.access_reference()
returns table (user_id uuid, folder_id uuid, permission text, via text, root_folder_id uuid)
language plpgsql stable set search_path = '' as $$
declare
    f      record;
    anc    uuid[];
    a      uuid;
    pr     iamb.profile;
    topf   iamb.folder;
    cand   record;
begin
    for f in select * from iamb.folder loop
        -- Ancestors from the folder up to the top.
        anc := array[f.id];
        a := f.parent_id;
        while a is not null and not a = any (anc) loop
            anc := anc || a;
            select parent_id into a from iamb.folder where id = a;
        end loop;
        select * into topf from iamb.folder where id = anc[array_length(anc, 1)];
        select * into pr from iamb.profile where id = f.profile_id;
        if pr.deleted_at is not null then
            continue;
        end if;
        for cand in
            with g as (
                select pr.owner_user_id as u, 'manage' as perm, 'own' as via, topf.id as root, 99 as depth
                where pr.kind = 'personal'
                union all
                select m.user_id, case when m.role in ('owner', 'admin') then 'manage'
                                       when topf.member_permission = 'edit' then 'edit' else 'view' end,
                       'group', topf.id, 99
                from iamb.membership m
                where pr.kind <> 'personal' and m.profile_id = pr.id and m.deleted_at is null
                union all
                select m.user_id, s.permission, 'share', s.folder_id, array_position(anc, s.folder_id)
                from iamb.folder_share s
                join iamb.profile gp on gp.id = s.profile_id
                join iamb.membership m on m.profile_id = gp.id and m.deleted_at is null
                where pr.kind = 'personal' and s.folder_id = any (anc) and s.deleted_at is null
                  and gp.deleted_at is null and gp.kind <> 'personal'
            )
            select g.u,
                   case max(case g.perm when 'manage' then 3 when 'edit' then 2 else 1 end)
                       when 3 then 'manage' when 2 then 'edit' else 'view' end as perm,
                   (array_agg(g.via order by case g.perm when 'manage' then 3 when 'edit' then 2 else 1 end desc,
                                             case g.via when 'own' then 3 when 'group' then 2 else 1 end desc))[1] as via,
                   (array_agg(g.root order by g.depth desc))[1] as root
            from g group by g.u
        loop
            user_id := cand.u;
            folder_id := f.id;
            permission := cand.perm;
            via := cand.via;
            root_folder_id := cand.root;
            return next;
        end loop;
    end loop;
end
$$;

-- ---------------------------------------------------------------------------------------------
-- Who can read a row (the read rules of §5.13, evaluated for every user at once). Used for the
-- feed's `row` entries ("everyone who could read the row before the write or can read it after
-- it") and for the rows a definer function returns. The policy tests check that it agrees with
-- the policies for every user and row.
-- ---------------------------------------------------------------------------------------------

create or replace function iamb_private.readers(p_entity text, p_id uuid) returns uuid[]
language plpgsql stable set search_path = '' as $$
declare
    res uuid[];
begin
    case p_entity
    when 'folder' then
        select array_agg(fa.user_id) into res from iamb.folder_access fa where fa.folder_id = p_id;
    when 'snippet' then
        select array_agg(fa.user_id) into res
        from iamb.snippet s join iamb.folder_access fa on fa.folder_id = s.folder_id where s.id = p_id;
    when 'snippet_tag' then
        select array_agg(fa.user_id) into res
        from iamb.snippet_tag l join iamb.snippet s on s.id = l.snippet_id
        join iamb.folder_access fa on fa.folder_id = s.folder_id where l.id = p_id;
    when 'snippet_attachment' then
        select array_agg(fa.user_id) into res
        from iamb.snippet_attachment l join iamb.snippet s on s.id = l.snippet_id
        join iamb.folder_access fa on fa.folder_id = s.folder_id where l.id = p_id;
    when 'folder_share' then
        select array_agg(distinct fa.user_id) into res
        from iamb.folder_share sh join iamb.folder_access fa on fa.folder_id = sh.folder_id
        where sh.id = p_id
          and (fa.permission = 'manage' or exists (
                select 1 from iamb.membership m join iamb.profile g on g.id = m.profile_id
                where m.profile_id = sh.profile_id and m.user_id = fa.user_id
                  and m.deleted_at is null and g.deleted_at is null and g.kind <> 'personal'));
    when 'tag' then
        select array_agg(distinct u) into res from (
            select p.owner_user_id as u from iamb.tag t join iamb.profile p on p.id = t.profile_id
            where t.id = p_id and p.kind = 'personal'
            union all
            select m.user_id from iamb.tag t join iamb.profile p on p.id = t.profile_id
            join iamb.membership m on m.profile_id = p.id and m.deleted_at is null
            where t.id = p_id and p.kind <> 'personal' and p.deleted_at is null
            union all
            select fa.user_id from iamb.snippet_tag l join iamb.snippet s on s.id = l.snippet_id
            join iamb.folder_access fa on fa.folder_id = s.folder_id
            where l.tag_id = p_id and l.deleted_at is null and s.deleted_at is null
            union all
            select t.updated_by from iamb.tag t
            where t.id = p_id and t.updated_by is not null and exists (
                select 1 from iamb.folder_access fa join iamb.folder f on f.id = fa.folder_id
                where fa.user_id = t.updated_by and fa.profile_id = t.profile_id
                  and fa.permission in ('edit', 'manage') and f.deleted_at is null)
        ) x;
    when 'attachment' then
        select array_agg(distinct u) into res from (
            select p.owner_user_id as u from iamb.attachment t join iamb.profile p on p.id = t.profile_id
            where t.id = p_id and p.kind = 'personal'
            union all
            select m.user_id from iamb.attachment t join iamb.profile p on p.id = t.profile_id
            join iamb.membership m on m.profile_id = p.id and m.deleted_at is null
            where t.id = p_id and p.kind <> 'personal' and p.deleted_at is null
            union all
            select fa.user_id from iamb.snippet_attachment l join iamb.snippet s on s.id = l.snippet_id
            join iamb.folder_access fa on fa.folder_id = s.folder_id
            where l.attachment_id = p_id and l.deleted_at is null and s.deleted_at is null
            union all
            select t.updated_by from iamb.attachment t
            where t.id = p_id and t.updated_by is not null and exists (
                select 1 from iamb.folder_access fa join iamb.folder f on f.id = fa.folder_id
                where fa.user_id = t.updated_by and fa.profile_id = t.profile_id
                  and fa.permission in ('edit', 'manage') and f.deleted_at is null)
        ) x;
    when 'profile' then
        select array_agg(distinct u) into res from (
            select p.owner_user_id as u from iamb.profile p where p.id = p_id and p.kind = 'personal'
            union all
            select m.user_id from iamb.profile p join iamb.membership m on m.profile_id = p.id
            where p.id = p_id and p.kind <> 'personal' and p.deleted_at is null and m.deleted_at is null
            union all
            select fa.user_id from iamb.folder_access fa where fa.profile_id = p_id
        ) x;
    when 'membership' then
        select array_agg(distinct u) into res from (
            select m.user_id as u from iamb.membership m where m.id = p_id
            union all
            select o.user_id from iamb.membership m
            join iamb.profile g on g.id = m.profile_id
            join iamb.membership o on o.profile_id = m.profile_id and o.deleted_at is null
            where m.id = p_id and m.deleted_at is null and g.deleted_at is null and g.kind <> 'personal'
        ) x;
    else
        res := null;
    end case;
    return coalesce(res, '{}');
end
$$;

create or replace function iamb_private.can_read(p_user uuid, p_entity text, p_id uuid) returns boolean
language sql stable set search_path = '' as $$
    select p_user = any (iamb_private.readers(p_entity, p_id))
$$;

-- Whether user `u` can read the verified file (profile, sha256) under the rule of §5.13.
create or replace function iamb_private.can_read_blob(u uuid, p_profile uuid, p_sha256 text) returns boolean
language sql stable set search_path = '' as $$
    select exists (select 1 from iamb.attachment_blob b where b.profile_id = p_profile and b.sha256 = p_sha256)
       and (
            exists (select 1 from iamb.profile p where p.id = p_profile and p.kind = 'personal' and p.owner_user_id = u)
            or exists (select 1 from iamb.membership m join iamb.profile g on g.id = m.profile_id
                       where m.profile_id = p_profile and m.user_id = u and m.deleted_at is null
                         and g.deleted_at is null and g.kind <> 'personal')
            or exists (select 1 from iamb.snippet_attachment l
                       join iamb.attachment a on a.id = l.attachment_id
                       join iamb.snippet s on s.id = l.snippet_id
                       join iamb.folder_access fa on fa.folder_id = s.folder_id and fa.user_id = u
                       where a.profile_id = p_profile and a.sha256 = p_sha256 and a.backed
                         and a.deleted_at is null and l.deleted_at is null and s.deleted_at is null)
       )
$$;

-- ---------------------------------------------------------------------------------------------
-- Rows as jsonb (internal column names), written back with static statements per table.
-- ---------------------------------------------------------------------------------------------

create or replace function iamb_private.get_row(p_entity text, p_id uuid) returns jsonb
language plpgsql stable set search_path = '' as $$
declare
    j jsonb;
begin
    case p_entity
    when 'profile' then select to_jsonb(t) into j from iamb.profile t where t.id = p_id;
    when 'membership' then select to_jsonb(t) into j from iamb.membership t where t.id = p_id;
    when 'folder' then select to_jsonb(t) into j from iamb.folder t where t.id = p_id;
    when 'folder_share' then select to_jsonb(t) into j from iamb.folder_share t where t.id = p_id;
    when 'snippet' then select to_jsonb(t) into j from iamb.snippet t where t.id = p_id;
    when 'tag' then select to_jsonb(t) into j from iamb.tag t where t.id = p_id;
    when 'snippet_tag' then select to_jsonb(t) into j from iamb.snippet_tag t where t.id = p_id;
    when 'attachment' then select to_jsonb(t) into j from iamb.attachment t where t.id = p_id;
    when 'snippet_attachment' then select to_jsonb(t) into j from iamb.snippet_attachment t where t.id = p_id;
    else j := null;
    end case;
    return j;
end
$$;

-- Inserts or replaces a whole row of a synced content table from its jsonb form.
create or replace function iamb_private.store_row(p_entity text, j jsonb) returns void
language plpgsql volatile set search_path = '' as $$
begin
    case p_entity
    when 'folder' then
        insert into iamb.folder select * from jsonb_populate_record(null::iamb.folder, j)
        on conflict (id) do update set
            updated_at = excluded.updated_at, updated_by = excluded.updated_by, rev = excluded.rev,
            deleted_at = excluded.deleted_at, updated_device = excluded.updated_device,
            updated_op = excluded.updated_op, lost_rev = excluded.lost_rev,
            parent_id = excluded.parent_id, name = excluded.name, position = excluded.position,
            member_permission = excluded.member_permission, default_trigger = excluded.default_trigger,
            default_case_mode = excluded.default_case_mode, abbreviation_prefix = excluded.abbreviation_prefix;
    when 'folder_share' then
        insert into iamb.folder_share select * from jsonb_populate_record(null::iamb.folder_share, j)
        on conflict (id) do update set
            updated_at = excluded.updated_at, updated_by = excluded.updated_by, rev = excluded.rev,
            deleted_at = excluded.deleted_at, updated_device = excluded.updated_device,
            updated_op = excluded.updated_op, lost_rev = excluded.lost_rev,
            permission = excluded.permission;
    when 'snippet' then
        insert into iamb.snippet select * from jsonb_populate_record(null::iamb.snippet, j)
        on conflict (id) do update set
            updated_at = excluded.updated_at, updated_by = excluded.updated_by, rev = excluded.rev,
            deleted_at = excluded.deleted_at, updated_device = excluded.updated_device,
            updated_op = excluded.updated_op, lost_rev = excluded.lost_rev,
            folder_id = excluded.folder_id, abbreviation = excluded.abbreviation, label = excluded.label,
            kind = excluded.kind, body = excluded.body, trigger_mode = excluded.trigger_mode,
            case_mode = excluded.case_mode, word_start = excluded.word_start, enabled = excluded.enabled,
            content_by = excluded.content_by, content_at = excluded.content_at;
    when 'tag' then
        insert into iamb.tag select * from jsonb_populate_record(null::iamb.tag, j)
        on conflict (id) do update set
            updated_at = excluded.updated_at, updated_by = excluded.updated_by, rev = excluded.rev,
            deleted_at = excluded.deleted_at, updated_device = excluded.updated_device,
            updated_op = excluded.updated_op, lost_rev = excluded.lost_rev,
            name = excluded.name, color = excluded.color;
    when 'snippet_tag' then
        insert into iamb.snippet_tag select * from jsonb_populate_record(null::iamb.snippet_tag, j)
        on conflict (id) do update set
            updated_at = excluded.updated_at, updated_by = excluded.updated_by, rev = excluded.rev,
            deleted_at = excluded.deleted_at, updated_device = excluded.updated_device,
            updated_op = excluded.updated_op, lost_rev = excluded.lost_rev;
    when 'attachment' then
        insert into iamb.attachment select * from jsonb_populate_record(null::iamb.attachment, j)
        on conflict (id) do update set
            updated_at = excluded.updated_at, updated_by = excluded.updated_by, rev = excluded.rev,
            deleted_at = excluded.deleted_at, updated_device = excluded.updated_device,
            updated_op = excluded.updated_op, lost_rev = excluded.lost_rev,
            unlinked_at = excluded.unlinked_at, backed = excluded.backed;
    when 'snippet_attachment' then
        insert into iamb.snippet_attachment select * from jsonb_populate_record(null::iamb.snippet_attachment, j)
        on conflict (id) do update set
            updated_at = excluded.updated_at, updated_by = excluded.updated_by, rev = excluded.rev,
            deleted_at = excluded.deleted_at, updated_device = excluded.updated_device,
            updated_op = excluded.updated_op, lost_rev = excluded.lost_rev;
    else
        raise exception 'store_row: unknown entity';
    end case;
end
$$;

-- Keeps the replaced version (§5.6). A version already kept under its revision stays as it is.
create or replace function iamb_private.keep_history(p_entity text, old jsonb, p_replaced_by bigint) returns void
language plpgsql volatile set search_path = '' as $$
begin
    insert into iamb.history (entity, row_id, rev, row, deleted, written_at, written_by, written_device,
                              replaced_at, replaced_by_rev)
    values (p_entity, (old ->> 'id')::uuid, (old ->> 'rev')::bigint, old, (old ->> 'deleted_at') is not null,
            (old ->> 'updated_at')::bigint, (old ->> 'updated_by')::uuid, (old ->> 'updated_device')::uuid,
            iamb_private.now_ms(), p_replaced_by)
    on conflict (entity, row_id, rev) do nothing;
end
$$;

-- The profile that owns a row the server holds.
create or replace function iamb_private.row_profile(p_entity text, p_id uuid) returns uuid
language sql stable set search_path = '' as $$
    select case p_entity
        when 'profile' then p_id
        when 'membership' then (select profile_id from iamb.membership where id = p_id)
        when 'folder' then (select profile_id from iamb.folder where id = p_id)
        when 'folder_share' then (select f.profile_id from iamb.folder_share s join iamb.folder f on f.id = s.folder_id where s.id = p_id)
        when 'snippet' then (select profile_id from iamb.snippet where id = p_id)
        when 'tag' then (select profile_id from iamb.tag where id = p_id)
        when 'attachment' then (select profile_id from iamb.attachment where id = p_id)
        when 'snippet_tag' then (select s.profile_id from iamb.snippet_tag l join iamb.snippet s on s.id = l.snippet_id where l.id = p_id)
        when 'snippet_attachment' then (select s.profile_id from iamb.snippet_attachment l join iamb.snippet s on s.id = l.snippet_id where l.id = p_id)
    end
$$;

-- ---------------------------------------------------------------------------------------------
-- Permissions (§4.2, §4.3), judged from the access entries
-- ---------------------------------------------------------------------------------------------

create or replace function iamb_private.perm(u uuid, p_folder uuid) returns text
language sql stable set search_path = '' as $$
    select permission from iamb.folder_access where user_id = u and folder_id = p_folder
$$;

create or replace function iamb_private.can_edit(u uuid, p_folder uuid) returns boolean
language sql stable set search_path = '' as $$
    select coalesce(iamb_private.perm(u, p_folder) in ('edit', 'manage'), false)
$$;

create or replace function iamb_private.can_manage(u uuid, p_folder uuid) returns boolean
language sql stable set search_path = '' as $$
    select coalesce(iamb_private.perm(u, p_folder) = 'manage', false)
$$;

-- `manage` in a profile: the owner of a personal profile; the owner and admins of a live group.
create or replace function iamb_private.manages_profile(u uuid, p_profile uuid) returns boolean
language sql stable set search_path = '' as $$
    select exists (select 1 from iamb.profile p where p.id = p_profile and p.deleted_at is null
                   and ((p.kind = 'personal' and p.owner_user_id = u)
                        or (p.kind <> 'personal' and exists (
                              select 1 from iamb.membership m where m.profile_id = p.id and m.user_id = u
                                and m.deleted_at is null and m.role in ('owner', 'admin')))))
$$;

-- `edit` on at least one live folder of the profile (tags and attachments, §4.3).
create or replace function iamb_private.edits_in_profile(u uuid, p_profile uuid) returns boolean
language sql stable set search_path = '' as $$
    select exists (select 1 from iamb.folder_access fa join iamb.folder f on f.id = fa.folder_id
                   where fa.user_id = u and fa.profile_id = p_profile
                     and fa.permission in ('edit', 'manage') and f.deleted_at is null)
       or iamb_private.manages_profile(u, p_profile)
$$;

create or replace function iamb_private.access_root(u uuid, p_folder uuid) returns uuid
language sql stable set search_path = '' as $$
    select root_folder_id from iamb.folder_access where user_id = u and folder_id = p_folder
$$;

-- Whether the folder is the root of a live share.
create or replace function iamb_private.is_share_root(p_folder uuid) returns boolean
language sql stable set search_path = '' as $$
    select exists (select 1 from iamb.folder_share s where s.folder_id = p_folder and s.deleted_at is null)
$$;

-- Depth of a folder (a top-level folder is 1) and the height of its subtree (a leaf is 1).
create or replace function iamb_private.folder_depth(p_folder uuid) returns integer
language sql stable set search_path = '' as $$
    with recursive up as (
        select id, parent_id, 1 as d, array[id] as path from iamb.folder where id = p_folder
        union all
        select f.id, f.parent_id, up.d + 1, up.path || f.id from iamb.folder f join up on f.id = up.parent_id
        where not f.id = any (up.path)
    )
    select max(d) from up
$$;

create or replace function iamb_private.subtree_height(p_folder uuid) returns integer
language sql stable set search_path = '' as $$
    with recursive down as (
        select id, 1 as h, array[id] as path from iamb.folder where id = p_folder
        union all
        select f.id, down.h + 1, down.path || f.id from iamb.folder f join down on f.parent_id = down.id
        where f.deleted_at is null and not f.id = any (down.path)
    )
    select max(h) from down
$$;

-- Whether `p_candidate` is the folder itself or lies below it.
create or replace function iamb_private.is_in_subtree(p_candidate uuid, p_folder uuid) returns boolean
language sql stable set search_path = '' as $$
    with recursive up as (
        select id, parent_id, array[id] as path from iamb.folder where id = p_candidate
        union all
        select f.id, f.parent_id, up.path || f.id from iamb.folder f join up on f.id = up.parent_id
        where not f.id = any (up.path)
    )
    select exists (select 1 from up where id = p_folder)
$$;

-- ---------------------------------------------------------------------------------------------
-- Plans and limits (§5.10)
-- ---------------------------------------------------------------------------------------------

create or replace function iamb_private.user_plan(u uuid) returns iamb.plan
language sql stable set search_path = '' as $$
    select coalesce(
        (select p from iamb.subscription s join iamb.plan p on p.id = s.plan_id
         where s.user_id = u and s.status = 'active' and p.active limit 1),
        (select p from iamb.plan p where p.scope = 'user' and p.is_default limit 1))
$$;

create or replace function iamb_private.group_plan(g uuid) returns iamb.plan
language sql stable set search_path = '' as $$
    select coalesce(
        (select p from iamb.subscription s join iamb.plan p on p.id = s.plan_id
         where s.profile_id = g and s.status = 'active' and p.active limit 1),
        (select p from iamb.plan p where p.scope = 'group' and p.is_default limit 1))
$$;

-- A limit for the rows of a profile: the owner's plan for a personal profile, the group's plan
-- for a group. Null means no limit.
create or replace function iamb_private.profile_limit(p_profile uuid, p_name text) returns bigint
language sql stable set search_path = '' as $$
    select case when p.kind = 'personal' then ((iamb_private.user_plan(p.owner_user_id)).limits ->> p_name)::bigint
                else ((iamb_private.group_plan(p.id)).limits ->> p_name)::bigint end
    from iamb.profile p where p.id = p_profile
$$;

create or replace function iamb_private.user_limit(u uuid, p_name text) returns bigint
language sql stable set search_path = '' as $$
    select ((iamb_private.user_plan(u)).limits ->> p_name)::bigint
$$;

-- ---------------------------------------------------------------------------------------------
-- Rate counters used inside definer functions (push ops, invitations)
-- ---------------------------------------------------------------------------------------------

-- Adds `n` to a counter with a window of `p_window_ms`; answers the milliseconds to wait when the
-- counter would pass `p_limit` (and then adds nothing), or null.
create or replace function iamb_private.rate_take(p_subject uuid, p_bucket text, p_window_ms bigint,
                                                  p_limit integer, n integer) returns bigint
language plpgsql volatile set search_path = '' as $$
declare
    now_ms bigint := iamb_private.now_ms();
    win    bigint := now_ms - (now_ms % p_window_ms);
    rb     iamb.rate_bucket;
begin
    insert into iamb.rate_bucket (subject, bucket, window_start, count)
    values (p_subject, p_bucket, win, 0)
    on conflict (subject, bucket) do nothing;
    select * into rb from iamb.rate_bucket where subject = p_subject and bucket = p_bucket for update;
    if rb.window_start <> win then
        rb.count := 0;
        rb.window_start := win;
    end if;
    if rb.count + n > p_limit then
        return greatest(rb.window_start + p_window_ms - now_ms, 1);
    end if;
    update iamb.rate_bucket set count = rb.count + n, window_start = rb.window_start
    where subject = p_subject and bucket = p_bucket;
    return null;
end
$$;

-- ---------------------------------------------------------------------------------------------
-- Grants: exactly the gate helpers for `authenticated` (§5.1, G4).
-- ---------------------------------------------------------------------------------------------

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
