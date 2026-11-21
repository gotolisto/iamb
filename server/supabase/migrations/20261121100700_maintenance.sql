-- Iamb server, migration 8: the daily job (docs/sync-design.md §5.12), the restore procedure's
-- database part (C18: `begin_epoch`, `replay_call`, `restore_replay`, `restore_files`), the token
-- hook (C25), the schedule, and the final grants.

do $$
begin
    if current_user <> 'postgres' then
        raise exception 'iamb migrations must run as postgres, not %', current_user;
    end if;
end
$$;

-- ---------------------------------------------------------------------------------------------
-- The daily job (§5.12). A procedure: it works profile by profile, takes the profile's lock,
-- handles at most `cascade_max` rows per transaction and commits between them. It can be run any
-- number of times. Run as the tables' owner (Supabase Cron, or `call` by the operator).
-- ---------------------------------------------------------------------------------------------

-- One bounded step of tombstone purging for one profile; answers the number of rows purged.
create or replace function iamb_private.purge_step(p_profile uuid, p_cut bigint, p_max integer) returns integer
language plpgsql volatile set search_path = '' as $$
declare
    n     integer := 0;
    k     integer;
    ids   uuid[];
begin
    perform iamb_private.lock_profiles(array[p_profile]);
    -- Links first (old tombstones, or links of rows about to go), then snippets, shares, tags,
    -- attachments, and folders deepest first.
    select coalesce(array_agg(l.id), '{}') into ids from (
        select l.id from iamb.snippet_tag l join iamb.snippet s on s.id = l.snippet_id
        where s.profile_id = p_profile and (l.deleted_at < p_cut or s.deleted_at < p_cut)
        limit p_max) l;
    delete from iamb.history where entity = 'snippet_tag' and row_id = any (ids);
    delete from iamb.snippet_tag where id = any (ids);
    get diagnostics k = row_count; n := n + k;
    select coalesce(array_agg(l.id), '{}') into ids from (
        select l.id from iamb.snippet_attachment l join iamb.snippet s on s.id = l.snippet_id
        where s.profile_id = p_profile and (l.deleted_at < p_cut or s.deleted_at < p_cut)
        limit greatest(p_max - n, 0)) l;
    delete from iamb.history where entity = 'snippet_attachment' and row_id = any (ids);
    delete from iamb.snippet_attachment where id = any (ids);
    get diagnostics k = row_count; n := n + k;
    if n >= p_max then return n; end if;

    select coalesce(array_agg(s.id), '{}') into ids from (
        select s.id from iamb.snippet s where s.profile_id = p_profile and s.deleted_at < p_cut
          and not exists (select 1 from iamb.snippet_tag l where l.snippet_id = s.id)
          and not exists (select 1 from iamb.snippet_attachment l where l.snippet_id = s.id)
        limit p_max - n) s;
    delete from iamb.history where entity = 'snippet' and row_id = any (ids);
    delete from iamb.conflict where entity = 'snippet' and row_id = any (ids);
    delete from iamb.snippet where id = any (ids);
    get diagnostics k = row_count; n := n + k;
    if n >= p_max then return n; end if;

    select coalesce(array_agg(s.id), '{}') into ids from (
        select s.id from iamb.folder_share s join iamb.folder f on f.id = s.folder_id
        where f.profile_id = p_profile and (s.deleted_at < p_cut or f.deleted_at < p_cut)
        limit p_max - n) s;
    delete from iamb.history where entity = 'folder_share' and row_id = any (ids);
    delete from iamb.folder_share where id = any (ids);
    get diagnostics k = row_count; n := n + k;
    if n >= p_max then return n; end if;

    select coalesce(array_agg(t.id), '{}') into ids from (
        select t.id from iamb.tag t where t.profile_id = p_profile and t.deleted_at < p_cut
          and not exists (select 1 from iamb.snippet_tag l where l.tag_id = t.id and l.deleted_at is null)
        limit p_max - n) t;
    delete from iamb.history where entity = 'tag' and row_id = any (ids);
    delete from iamb.conflict where entity = 'tag' and row_id = any (ids);
    delete from iamb.tag where id = any (ids);
    get diagnostics k = row_count; n := n + k;
    if n >= p_max then return n; end if;

    select coalesce(array_agg(a.id), '{}') into ids from (
        select a.id from iamb.attachment a where a.profile_id = p_profile and a.deleted_at < p_cut
          and not exists (select 1 from iamb.snippet_attachment l where l.attachment_id = a.id and l.deleted_at is null)
        limit p_max - n) a;
    delete from iamb.history where entity = 'attachment' and row_id = any (ids);
    -- The stored file goes with the last row for its profile and hash; the object itself is
    -- removed through the storage interface by the edge function of task 4.6, which reads these
    -- log lines (a storage object must not be deleted by SQL).
    perform iamb_private.log('blob_removed', jsonb_build_object('profile', p_profile, 'sha256', b.sha256))
    from iamb.attachment_blob b
    where b.profile_id = p_profile
      and exists (select 1 from iamb.attachment a where a.id = any (ids) and a.sha256 = b.sha256)
      and not exists (select 1 from iamb.attachment a where a.profile_id = p_profile and a.sha256 = b.sha256 and not a.id = any (ids));
    delete from iamb.attachment_blob b
    where b.profile_id = p_profile
      and exists (select 1 from iamb.attachment a where a.id = any (ids) and a.sha256 = b.sha256)
      and not exists (select 1 from iamb.attachment a where a.profile_id = p_profile and a.sha256 = b.sha256 and not a.id = any (ids));
    delete from iamb.attachment where id = any (ids);
    get diagnostics k = row_count; n := n + k;
    if n >= p_max then return n; end if;

    -- Folders whose subtree holds nothing else: deepest first.
    select coalesce(array_agg(f.id), '{}') into ids from (
        select f.id from iamb.folder f where f.profile_id = p_profile and f.deleted_at < p_cut
          and not exists (select 1 from iamb.folder c where c.parent_id = f.id)
          and not exists (select 1 from iamb.snippet s where s.folder_id = f.id)
          and not exists (select 1 from iamb.folder_share s where s.folder_id = f.id)
        limit p_max - n) f;
    delete from iamb.history where entity = 'folder' and row_id = any (ids);
    delete from iamb.conflict where entity = 'folder' and row_id = any (ids);
    delete from iamb.folder where id = any (ids);
    get diagnostics k = row_count; n := n + k;
    return n;
end
$$;

-- A deleted group, 30 days on: its rows are purged with it. Answers whether the profile is gone.
create or replace function iamb_private.purge_group_step(p_profile uuid, p_max integer) returns boolean
language plpgsql volatile set search_path = '' as $$
declare
    n integer;
begin
    n := iamb_private.purge_step(p_profile, 9223372036854775807, p_max);
    if n > 0 then
        return false;
    end if;
    update iamb.folder set parent_id = null where profile_id = p_profile;
    delete from iamb.folder f where f.profile_id = p_profile
      and not exists (select 1 from iamb.snippet s where s.folder_id = f.id)
      and not exists (select 1 from iamb.folder_share s where s.folder_id = f.id);
    if exists (select 1 from iamb.folder where profile_id = p_profile)
       or exists (select 1 from iamb.snippet where profile_id = p_profile)
       or exists (select 1 from iamb.tag where profile_id = p_profile)
       or exists (select 1 from iamb.attachment where profile_id = p_profile) then
        return false;
    end if;
    delete from iamb.history where entity = 'membership' and row_id in (select id from iamb.membership where profile_id = p_profile);
    delete from iamb.membership where profile_id = p_profile;
    delete from iamb.folder_share where profile_id = p_profile;
    delete from iamb.invitation where profile_id = p_profile;
    delete from iamb.attachment_upload where profile_id = p_profile;
    delete from iamb.attachment_blob where profile_id = p_profile;
    delete from iamb.subscription where profile_id = p_profile;
    delete from iamb.history where entity = 'profile' and row_id = p_profile;
    delete from iamb.profile where id = p_profile;
    return true;
end
$$;

-- No `SET` clause: Postgres forbids transaction control in a procedure that has one. Every name
-- in the body is schema-qualified instead.
create or replace procedure iamb_private.daily_job()
language plpgsql as $$
declare
    st     iamb.server_state;
    now_ms bigint;
    cut    bigint;
    hcut   bigint;
    p      record;
    n      integer;
    done   boolean;
    ep_start bigint;
    a      record;
    r      bigint;
begin
    select * into st from iamb.server_state;
    now_ms := iamb_private.now_ms();
    cut := now_ms - st.retention_days * 86400000::bigint;
    hcut := now_ms - st.history_days * 86400000::bigint;

    -- Tombstones, profile by profile, at most `cascade_max` rows per transaction.
    for p in select id, kind, deleted_at from iamb.profile order by id loop
        if p.kind <> 'personal' and p.deleted_at is not null and p.deleted_at < cut then
            loop
                done := iamb_private.purge_group_step(p.id, st.cascade_max);
                commit;
                exit when done;
            end loop;
        else
            loop
                n := iamb_private.purge_step(p.id, cut, st.cascade_max);
                commit;
                exit when n < st.cascade_max;
            end loop;
        end if;
    end loop;

    -- Attachments nothing has used for `history_days` become tombstones (a write by the server).
    for a in select at.id, at.profile_id from iamb.attachment at
             where at.deleted_at is null and at.unlinked_at is not null and at.unlinked_at < hcut
             order by at.profile_id, at.id loop
        perform iamb_private.tx_begin();
        perform iamb_private.lock_profiles(array[a.profile_id]);
        if exists (select 1 from iamb.attachment where id = a.id and deleted_at is null and unlinked_at < hcut) then
            r := iamb_private.next_rev(array[a.profile_id]);
            perform iamb_private.write_row(null, null, null, 'attachment', iamb_private.get_row('attachment', a.id),
                iamb_private.get_row('attachment', a.id) || jsonb_build_object('deleted_at', now_ms, 'rev', r,
                    'updated_at', now_ms, 'updated_by', null, 'updated_device', null, 'updated_op', null),
                r, iamb_private.readers('attachment', a.id), false, a.profile_id);
            perform iamb_private.feed_flush();
        end if;
        commit;
    end loop;

    -- Uploads under way past their time (their objects are removed by task 4.6's job).
    perform iamb_private.log('upload_expired', jsonb_build_object('upload', u.id, 'profile', u.profile_id))
    from iamb.attachment_upload u where u.expires_at < now_ms;
    delete from iamb.attachment_upload where expires_at < now_ms;
    commit;

    -- The feed: entries older than 30 days, and the floor raised to the highest one deleted.
    with gone as (
        delete from iamb.feed where created_at < cut returning user_id, seq
    ), top as (
        select user_id, max(seq) as m from gone group by user_id
    )
    update iamb.feed_head fh set floor = greatest(fh.floor, top.m) from top where fh.user_id = top.user_id;
    commit;

    -- History: 90 days after a version was replaced, unless an open conflict offers it.
    delete from iamb.history h where h.replaced_at < hcut
      and not exists (select 1 from iamb.conflict c join iamb.conflict_version v on v.conflict_id = c.id
                      where c.state = 'open' and c.entity = h.entity and c.row_id = h.row_id and v.rev = h.rev);
    -- Conflicts: an open one expires after 90 days; a closed one goes 30 days after it was closed.
    update iamb.conflict set state = 'expired', resolved_at = now_ms where state = 'open' and created_at < hcut;
    delete from iamb.conflict where state <> 'open' and resolved_at < cut;
    -- Receipts.
    delete from iamb.push_receipt where created_at < cut;
    -- Invitations and offers of ownership.
    update iamb.invitation set status = 'expired', answered_at = now_ms where status = 'pending' and expires_at < now_ms;
    delete from iamb.invitation where status <> 'pending' and answered_at < hcut;
    update iamb.profile set offered_owner = null, offered_at = null
    where offered_owner is not null and offered_at < now_ms - 14 * 86400000::bigint;
    commit;

    -- Devices: one that is not revoked and has not called for 180 days (since the epoch began,
    -- for a row that has not called in this epoch); a revoked one once its session is gone (C25).
    select started_at into ep_start from iamb.epoch where id = st.epoch;
    delete from iamb.device d
    where d.revoked_at is null
      and coalesce(d.last_seen_at, greatest(d.created_at, coalesce(ep_start, 0))) < now_ms - 180 * 86400000::bigint;
    delete from iamb.device d
    where d.revoked_at is not null and not exists (select 1 from auth.sessions s where s.id = d.session_id);
    delete from iamb.rate_bucket where window_start < now_ms - 2 * 86400000::bigint;
    commit;
    perform iamb_private.log('daily_job', jsonb_build_object('answer', 'ok'));
end
$$;

-- ---------------------------------------------------------------------------------------------
-- The restore procedure, database part (C18). Run only by the operator's script, as the tables'
-- owner, on a restored database that is closed to devices. No call by a device reaches these.
-- ---------------------------------------------------------------------------------------------

-- Begins a new epoch: an `epoch` row whose first revision lies past every revision a device can
-- hold, an `epoch_profile` row with each profile's `head_rev`, `rev_seq` moved to the first
-- revision, the feeds emptied, and the devices' positions and last calls forgotten.
create or replace function iamb_private.begin_epoch(p_not_before bigint) returns jsonb
language plpgsql volatile set search_path = '' as $$
declare
    now_ms  bigint := iamb_private.now_ms();
    t       bigint := greatest(now_ms, coalesce(p_not_before, 0));
    seq_val bigint;
    max_head bigint;
    first   bigint;
    e       uuid := gen_random_uuid();
begin
    select last_value into seq_val from iamb.rev_seq;
    select coalesce(max(head_rev), 0) into max_head from iamb.profile;
    first := greatest(t * 1024, seq_val + 1, max_head + 1);
    insert into iamb.epoch (id, started_at, first_rev) values (e, now_ms, first);
    insert into iamb.epoch_profile (epoch_id, profile_id, backup_rev)
    select e, p.id, p.head_rev from iamb.profile p;
    perform setval('iamb.rev_seq', first, false);
    update iamb.server_state set epoch = e;
    delete from iamb.feed;
    update iamb.feed_head set floor = head;
    update iamb.device set position_token = null, position_token_before = null, last_seen_at = null;
    perform iamb_private.log('begin_epoch', jsonb_build_object('epoch', e, 'first_rev', first, 'not_before', p_not_before));
    return jsonb_build_object('epoch', e, 'first_rev', first, 'started_at', now_ms);
end
$$;

-- Makes one logged call again as its logged caller and under the call's own rules. Only calls
-- that took something away; never one that gives access.
create or replace function iamb_private.replay_call(c jsonb) returns jsonb
language plpgsql volatile set search_path = '' as $$
declare
    u   uuid := iamb_private.try_uuid(c ->> 'user');
    res jsonb;
    sh  iamb.folder_share;
    f   iamb.folder;
begin
    perform iamb_private.tx_begin();
    case c ->> 'event'
    when 'remove_member' then
        res := iamb_private.acct_remove_member(u, null, (c ->> 'group')::uuid, (c ->> 'person')::uuid);
    when 'leave_group' then
        res := iamb_private.acct_leave_group(u, null, (c ->> 'group')::uuid);
    when 'withdraw_invitation' then
        res := iamb_private.acct_withdraw_invitation(u, null, (c ->> 'invitation')::uuid);
    when 'set_member_role' then
        if c ->> 'role' <> 'member' then
            return jsonb_build_object('error', 'gives_access');
        end if;
        res := iamb_private.acct_set_member_role(u, null, (c ->> 'group')::uuid, (c ->> 'person')::uuid, 'member');
    when 'withdraw_offer' then
        res := iamb_private.acct_transfer_ownership(u, null, (c ->> 'group')::uuid, null);
    when 'delete_group' then
        res := iamb_private.acct_delete_group(u, null, (c ->> 'group')::uuid);
    when 'revoke_device', 'sign_out_everywhere' then
        perform 1 from iamb.device where user_id = u order by id for update;
        update iamb.device set revoked_at = coalesce(revoked_at, iamb_private.now_ms())
        where user_id = u and id in (select (jsonb_array_elements_text(coalesce(c -> 'devices', '[]')))::uuid);
        res := jsonb_build_object('error', null);
    when 'share_deleted' then
        select * into sh from iamb.folder_share where id = (c ->> 'share')::uuid;
        if sh.id is null then
            return jsonb_build_object('error', 'not_found');
        end if;
        select * into f from iamb.folder where id = sh.folder_id;
        perform iamb_private.lock_profiles(array[sh.profile_id, f.profile_id]);
        if not iamb_private.can_manage(u, f.id) then
            return jsonb_build_object('error', 'forbidden');
        end if;
        perform iamb_private.end_share(u, null, null, sh.id, array[f.profile_id], 'replay');
        perform iamb_private.refresh_access(array[f.profile_id]);
        res := jsonb_build_object('error', null);
    when 'delete_account' then
        res := api_service.delete_account_data(u);
    else
        return jsonb_build_object('error', 'not_replayable');
    end case;
    perform iamb_private.feed_flush();
    return res;
end
$$;

-- The script's step 2: from the function log lines (as logged, each with its `at`, or the log
-- system's own time in `logged_at`), makes again in log order every call answered `ok` after the
-- backup's moment that took something away, skipping one whose access the log shows given back
-- afterwards. Answers the report: what was made again and what was skipped, and why.
create or replace function iamb_private.restore_replay(p_lines jsonb, p_backup_at bigint) returns jsonb
language plpgsql volatile set search_path = '' as $$
declare
    l      record;
    res    jsonb;
    report jsonb := '[]';
    later  boolean;
begin
    for l in
        select x.v, x.i, coalesce((x.v ->> 'logged_at')::bigint, (x.v ->> 'at')::bigint) as t
        from jsonb_array_elements(p_lines) with ordinality as x(v, i)
        where coalesce((x.v ->> 'logged_at')::bigint, (x.v ->> 'at')::bigint) > p_backup_at
          and x.v ->> 'event' in ('remove_member', 'leave_group', 'withdraw_invitation', 'set_member_role',
                                  'withdraw_offer', 'delete_group', 'revoke_device', 'sign_out_everywhere',
                                  'share_deleted', 'delete_account')
          and coalesce(x.v ->> 'answer', 'ok') = 'ok'
        order by t, x.i
    loop
        if l.v ->> 'event' = 'set_member_role' and l.v ->> 'role' <> 'member' then
            continue;
        end if;
        if l.v ->> 'event' = 'share_deleted' and l.v ->> 'reason' in ('replay', 'delete_group', 'remove_member',
                                                                      'leave_group', 'delete_account') then
            -- Made again with the call that ended it.
            continue;
        end if;
        -- The same access given back afterwards, by the log.
        select exists (
            select 1 from jsonb_array_elements(p_lines) y
            where coalesce((y ->> 'logged_at')::bigint, (y ->> 'at')::bigint) > l.t
              and coalesce(y ->> 'answer', 'ok') = 'ok'
              and (
                   (l.v ->> 'event' in ('remove_member', 'leave_group') and y ->> 'event' = 'accept_invitation'
                    and y ->> 'group' = l.v ->> 'group'
                    and y ->> 'user' = coalesce(l.v ->> 'person', l.v ->> 'user'))
                or (l.v ->> 'event' = 'set_member_role' and y ->> 'event' = 'set_member_role'
                    and y ->> 'group' = l.v ->> 'group' and y ->> 'person' = l.v ->> 'person' and y ->> 'role' = 'admin')
                or (l.v ->> 'event' = 'withdraw_offer' and y ->> 'event' = 'transfer_ownership'
                    and y ->> 'group' = l.v ->> 'group' and y ->> 'person' = l.v ->> 'person')
                or (l.v ->> 'event' = 'share_deleted' and y ->> 'event' = 'share_created'
                    and y ->> 'share' = l.v ->> 'share'))
        ) into later;
        if later then
            report := report || jsonb_build_array(jsonb_build_object('line', l.v, 'result', 'skipped', 'why', 'given_back'));
            continue;
        end if;
        res := iamb_private.replay_call(l.v);
        report := report || jsonb_build_array(jsonb_build_object(
            'line', l.v, 'result', case when res ->> 'error' is null then 'made_again' else 'skipped' end,
            'why', res ->> 'error'));
    end loop;
    return report;
end
$$;

-- The script's step 3, database side. On the platform the rows of `storage.objects` are in the
-- database and come back with the backup, while the bytes are not: the database alone cannot
-- tell which objects survived. So the script passes the names the storage interface lists in the
-- bucket now (`p_present`). An `attachment_blob` row whose object is not listed is removed, and
-- the attachment rows of that profile and hash are no longer backed, so devices are asked for the
-- bytes again (§6.11); listed verified-file names without a row are answered for the edge function
-- to hash (a database function cannot read an object's bytes, §11.6), and listed upload objects
-- without an `attachment_upload` row are answered for removal.
create or replace function iamb_private.restore_files(p_present text[]) returns jsonb
language plpgsql volatile set search_path = '' as $$
declare
    removed jsonb;
    to_hash jsonb;
    stale   jsonb;
begin
    with gone as (
        delete from iamb.attachment_blob b
        where not (b.profile_id::text || '/' || b.sha256) = any (p_present)
        returning b.profile_id, b.sha256
    )
    select coalesce(jsonb_agg(jsonb_build_object('profile', profile_id, 'sha256', sha256)), '[]') into removed from gone;
    update iamb.attachment a set backed = false
    where a.backed
      and not exists (select 1 from iamb.attachment_blob b where b.profile_id = a.profile_id and b.sha256 = a.sha256);
    select coalesce(jsonb_agg(n order by n), '[]') into to_hash from unnest(p_present) n
    where n ~ '^[0-9a-f-]{36}/[0-9a-f]{64}$'
      and not exists (select 1 from iamb.attachment_blob b where n = b.profile_id::text || '/' || b.sha256);
    select coalesce(jsonb_agg(n order by n), '[]') into stale from unnest(p_present) n
    where n ~ '/upload/'
      and not exists (select 1 from iamb.attachment_upload u where n = u.profile_id::text || '/upload/' || u.id::text);
    return jsonb_build_object('blob_rows_removed', removed, 'objects_to_hash', to_hash, 'uploads_to_remove', stale);
end
$$;

-- ---------------------------------------------------------------------------------------------
-- The custom access token hook (C25, §11.6): refuses to issue a token asked for with a password.
-- Executed by the authentication service's role and by nobody else; reads no table.
-- ---------------------------------------------------------------------------------------------

create or replace function iamb_private.token_hook(event jsonb) returns jsonb
language plpgsql stable set search_path = '' as $$
begin
    if event ->> 'authentication_method' = 'password' then
        return jsonb_build_object('error', jsonb_build_object(
            'http_code', 403, 'message', 'Iamb does not sign in with a password.'));
    end if;
    return jsonb_build_object('claims', event -> 'claims');
end
$$;

-- ---------------------------------------------------------------------------------------------
-- The schedule: Supabase Cron where the project has it (§11.6); elsewhere any scheduler that
-- runs `call iamb_private.daily_job()` once a day will do.
-- ---------------------------------------------------------------------------------------------

do $$
begin
    if exists (select 1 from pg_available_extensions where name = 'pg_cron') then
        create extension if not exists pg_cron;
        perform cron.schedule('iamb-daily', '17 3 * * *', 'call iamb_private.daily_job()');
    end if;
end
$$;

-- ---------------------------------------------------------------------------------------------
-- Final grants (§5.1, G4). Functions of `api`: `authenticated` only. Functions of
-- `iamb_private`: the tables' owner, except the gate helpers for `authenticated` and the token
-- hook for the authentication service. Functions of `api_service`: `service_role` only.
-- ---------------------------------------------------------------------------------------------

revoke execute on all functions in schema api from public, anon, authenticated, service_role;
revoke execute on all functions in schema iamb_private from public, anon, authenticated, service_role;
revoke execute on all functions in schema api_service from public, anon, authenticated, service_role;
revoke execute on all procedures in schema iamb_private from public, anon, authenticated, service_role;
grant execute on all functions in schema api to authenticated;
grant execute on function
    iamb_private.session_ok(),
    iamb_private.member_of(uuid),
    iamb_private.role_in(uuid),
    iamb_private.shares_group_with(uuid),
    iamb_private.own_address(),
    iamb_private.enter(uuid, integer, text),
    iamb_private.position(uuid, text, text, bigint)
to authenticated;
grant execute on function api_service.attachment_verified(uuid, text, bigint),
                          api_service.delete_account_data(uuid) to service_role;

do $$
begin
    if exists (select 1 from pg_roles where rolname = 'supabase_auth_admin') then
        grant usage on schema iamb_private to supabase_auth_admin;
        grant execute on function iamb_private.token_hook(jsonb) to supabase_auth_admin;
    end if;
end
$$;
