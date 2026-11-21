-- Iamb server, migration 9: the daily job as bounded steps (a correction to migration 8, recorded
-- in server/README.md, "Design corrections").
--
-- Migration 8 made the daily job of §5.12 a procedure that commits between batches. Postgres
-- forbids transaction control in a procedure that has a `SET` clause, so that procedure could not
-- pin its `search_path`, and the security advisor reports it (lint 0011). The job is now a function
-- with a fixed `search_path` that does one bounded step: at most `cascade_max` rows of tombstones
-- per run, one transaction per run, so it never holds a lock for long, as §5.12 requires. Supabase
-- Cron runs it every ten minutes; any run can be repeated, and `more` says whether rows are left.
--
-- One step takes its locks in the order of C24: the device rows it removes first, then the
-- profiles it works on (groups by id, then personal profiles by id), and the feed heads last.

do $$
begin
    if current_user <> 'postgres' then
        raise exception 'iamb migrations must run as postgres, not %', current_user;
    end if;
end
$$;

do $$
begin
    if exists (select 1 from pg_extension where extname = 'pg_cron') then
        perform cron.unschedule(jobid) from cron.job where jobname = 'iamb-daily';
    end if;
end
$$;
drop procedure if exists iamb_private.daily_job();

-- A deleted group, 30 days on: every row of the group goes with it, within the budget; answers the
-- number of rows purged, and removes the profile row itself once nothing else is left. The tags,
-- attachments and links of a deleted group get their tombstones here, without revisions or feed
-- entries: nobody can read them since the group was deleted (C16).
drop function if exists iamb_private.purge_group_step(uuid, integer);
create or replace function iamb_private.purge_group(p_profile uuid, p_max integer) returns integer
language plpgsql volatile set search_path = '' as $$
declare
    n      integer;
    now_ms bigint := iamb_private.now_ms();
begin
    perform iamb_private.lock_profiles(array[p_profile]);
    update iamb.snippet_tag l set deleted_at = now_ms from iamb.snippet s
    where s.id = l.snippet_id and s.profile_id = p_profile and l.deleted_at is null;
    update iamb.snippet_attachment l set deleted_at = now_ms from iamb.snippet s
    where s.id = l.snippet_id and s.profile_id = p_profile and l.deleted_at is null;
    update iamb.tag set deleted_at = now_ms where profile_id = p_profile and deleted_at is null;
    update iamb.attachment set deleted_at = now_ms where profile_id = p_profile and deleted_at is null;
    update iamb.snippet set deleted_at = now_ms where profile_id = p_profile and deleted_at is null;
    update iamb.folder set deleted_at = now_ms where profile_id = p_profile and deleted_at is null;
    n := iamb_private.purge_step(p_profile, 9223372036854775807, p_max);
    if n > 0 then
        return n;
    end if;
    if exists (select 1 from iamb.folder where profile_id = p_profile)
       or exists (select 1 from iamb.snippet where profile_id = p_profile)
       or exists (select 1 from iamb.tag where profile_id = p_profile)
       or exists (select 1 from iamb.attachment where profile_id = p_profile) then
        return 0;
    end if;
    delete from iamb.history where entity = 'membership' and row_id in (select id from iamb.membership where profile_id = p_profile);
    delete from iamb.membership where profile_id = p_profile;
    delete from iamb.folder_share where profile_id = p_profile;
    delete from iamb.invitation where profile_id = p_profile;
    delete from iamb.attachment_upload where profile_id = p_profile;
    delete from iamb.attachment_blob where profile_id = p_profile;
    delete from iamb.subscription where profile_id = p_profile;
    delete from iamb.conflict where profile_id = p_profile;
    delete from iamb.history where entity = 'profile' and row_id = p_profile;
    delete from iamb.profile where id = p_profile;
    return 1;
end
$$;
revoke execute on function iamb_private.purge_group(uuid, integer) from public, anon, authenticated, service_role;

create or replace function iamb_private.daily_job_step() returns jsonb
language plpgsql volatile set search_path = '' set client_min_messages = 'warning' as $$
declare
    st       iamb.server_state;
    now_ms   bigint := iamb_private.now_ms();
    cut      bigint;
    hcut     bigint;
    budget   integer;
    p        record;
    n        integer;
    a        record;
    r        bigint;
    ep_start bigint;
    devices  integer;
    feed     integer;
    more     boolean := false;
    users    uuid[];
begin
    select * into st from iamb.server_state;
    cut := now_ms - st.retention_days * 86400000::bigint;
    hcut := now_ms - st.history_days * 86400000::bigint;
    budget := st.cascade_max;
    perform iamb_private.tx_begin();

    -- C24 step 1: device rows. One that is not revoked and has not called for 180 days (since the
    -- epoch began, for a row that has not called in this epoch); a revoked one once its session
    -- is gone (C25). Then the rate counters of past windows.
    select started_at into ep_start from iamb.epoch where id = st.epoch;
    perform 1 from iamb.device d
    where (d.revoked_at is null
           and coalesce(d.last_seen_at, greatest(d.created_at, coalesce(ep_start, 0))) < now_ms - 180 * 86400000::bigint)
       or (d.revoked_at is not null and not exists (select 1 from auth.sessions s where s.id = d.session_id))
    order by d.id for update skip locked;
    with gone as (
        delete from iamb.device d
        where (d.revoked_at is null
               and coalesce(d.last_seen_at, greatest(d.created_at, coalesce(ep_start, 0))) < now_ms - 180 * 86400000::bigint)
           or (d.revoked_at is not null and not exists (select 1 from auth.sessions s where s.id = d.session_id))
        returning 1
    ) select count(*) into devices from gone;
    delete from iamb.rate_bucket where window_start < now_ms - 2 * 86400000::bigint;
    delete from iamb.push_receipt where created_at < cut;

    -- C24 steps 2 and 3: profile by profile, groups first, each under its lock, within the budget.
    for p in select id, kind, deleted_at from iamb.profile order by (kind = 'personal'), id loop
        exit when budget <= 0;
        if p.kind <> 'personal' and p.deleted_at is not null and p.deleted_at < cut then
            -- A deleted group, 30 days on: its rows are purged with it, then the profile row.
            loop
                n := iamb_private.purge_group(p.id, budget);
                budget := budget - n;
                exit when n = 0 or budget <= 0 or not exists (select 1 from iamb.profile where id = p.id);
            end loop;
            if budget <= 0 then
                more := true;
                exit;
            end if;
            continue;
        end if;
        n := iamb_private.purge_step(p.id, cut, budget);
        budget := budget - n;
        if budget <= 0 then
            more := true;
            exit;
        end if;
        if p.kind <> 'personal' then
            -- Invitations and offers of ownership belong to the group.
            update iamb.invitation set status = 'expired', answered_at = now_ms
            where profile_id = p.id and status = 'pending' and expires_at < now_ms;
            delete from iamb.invitation where profile_id = p.id and status <> 'pending' and answered_at < hcut;
            update iamb.profile set offered_owner = null, offered_at = null
            where id = p.id and offered_owner is not null and offered_at < now_ms - 14 * 86400000::bigint;
        end if;
        -- Conflicts of the profile: an open one expires after 90 days; a closed one goes 30 days
        -- after it was closed.
        update iamb.conflict set state = 'expired', resolved_at = now_ms
        where profile_id = p.id and state = 'open' and created_at < hcut;
        delete from iamb.conflict where profile_id = p.id and state <> 'open' and resolved_at < cut;
        -- Attachments nothing has used for `history_days` become tombstones (a write by the server).
        for a in select at.id from iamb.attachment at
                 where at.profile_id = p.id and at.deleted_at is null and at.unlinked_at is not null and at.unlinked_at < hcut
                 order by at.id limit greatest(budget, 0) loop
            perform iamb_private.lock_profiles(array[p.id]);
            r := iamb_private.next_rev(array[p.id]);
            perform iamb_private.write_row(null, null, null, 'attachment', iamb_private.get_row('attachment', a.id),
                iamb_private.get_row('attachment', a.id) || jsonb_build_object('deleted_at', now_ms, 'rev', r,
                    'updated_at', now_ms, 'updated_by', null, 'updated_device', null, 'updated_op', null),
                r, iamb_private.readers('attachment', a.id), false, p.id);
            budget := budget - 1;
        end loop;
    end loop;

    -- Uploads under way past their time (their objects are removed through the storage interface
    -- by task 4.6's job, which reads these lines).
    perform iamb_private.log('upload_expired', jsonb_build_object('upload', u.id, 'profile', u.profile_id))
    from iamb.attachment_upload u where u.expires_at < now_ms;
    delete from iamb.attachment_upload where expires_at < now_ms;
    -- History: 90 days after a version was replaced, unless an open conflict offers it.
    delete from iamb.history h where h.replaced_at < hcut
      and not exists (select 1 from iamb.conflict c join iamb.conflict_version v on v.conflict_id = c.id
                      where c.state = 'open' and c.entity = h.entity and c.row_id = h.row_id and v.rev = h.rev);

    -- C24 step 4, last: the feed. Entries older than 30 days go, and the floor is raised to the
    -- highest one deleted; the heads are locked in the order of the user ids.
    select coalesce(array_agg(distinct f.user_id order by f.user_id), '{}') into users from iamb.feed f where f.created_at < cut;
    perform 1 from iamb.feed_head where user_id = any (users) order by user_id for update;
    with gone as (
        delete from iamb.feed where created_at < cut returning user_id, seq
    ), top as (
        select user_id, max(seq) as m, count(*) as c from gone group by user_id
    ), upd as (
        update iamb.feed_head fh set floor = greatest(fh.floor, top.m) from top where fh.user_id = top.user_id returning top.c
    ) select coalesce(sum(c), 0) into feed from upd;
    -- Entries the attachment tombstones above wrote.
    perform iamb_private.feed_flush();
    perform iamb_private.log('daily_job', jsonb_build_object('rows', st.cascade_max - budget, 'devices', devices,
                                                             'feed', feed, 'more', more, 'answer', 'ok'));
    return jsonb_build_object('rows', st.cascade_max - budget, 'devices', devices, 'feed', feed, 'more', more);
end
$$;

revoke execute on function iamb_private.daily_job_step() from public, anon, authenticated, service_role;

do $$
begin
    if exists (select 1 from pg_extension where extname = 'pg_cron') then
        perform cron.schedule('iamb-retention', '*/10 * * * *', 'select iamb_private.daily_job_step()');
    end if;
end
$$;
