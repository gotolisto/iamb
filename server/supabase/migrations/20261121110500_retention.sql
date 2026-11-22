-- Iamb server, migration 16: the retention job after the security review of task 4.3
-- (docs/reviews/4.3-security-review.md), finding 9.
--
-- Before: one budget of `cascade_max` rows a run, spent in the order of the profiles' ids, groups
-- first, and the run stopped where the budget ended. A group's id is chosen by its creator, so one
-- account that made and deleted rows fast enough in a group with a low id kept the job from
-- everybody else's deleted rows, and from the expiry of their invitations, offers and conflicts.
--
-- Now:
--  - each profile gets a share of a run (a quarter of `cascade_max` rows), so no profile can take
--    the whole budget;
--  - a run goes on where the last one stopped (`server_state.job_after`), in the lock order of C24
--    (groups by id, then personal profiles by id), and begins again at the start once it has
--    reached the end; a run visits at most 1,000 profiles;
--  - what is not a purge of tombstones (invitations, offers, conflicts) is done for every profile
--    a run visits, whatever is left of the budget;
--  - the log line says how many profiles used up their share (`capped`), so that a job that falls
--    behind is seen; `more` is true until a whole pass has ended with no share used up.
-- The scratch rows of migration 11 that a committed transaction left behind, if a function ever
-- forgot to clear them, go here too.

do $$
begin
    if current_user <> 'postgres' then
        raise exception 'iamb migrations must run as postgres, not %', current_user;
    end if;
end
$$;

-- Where the last run stopped: the profile it visited last (its kind and id), or null at the start
-- of a pass. Written by the job alone.
alter table iamb.server_state add column if not exists job_after_personal boolean not null default false;
alter table iamb.server_state add column if not exists job_after uuid;

create or replace function iamb_private.daily_job_step() returns jsonb
language plpgsql volatile set search_path = pg_catalog, pg_temp as $$
declare
    st       iamb.server_state;
    v_tx     bigint := iamb_private.tx();
    now_ms   bigint := iamb_private.now_ms();
    cut      bigint;
    hcut     bigint;
    budget   integer;
    share    integer;
    take     integer;
    used     integer;
    p        record;
    n        integer;
    a        record;
    r        bigint;
    ep_start bigint;
    devices  integer;
    feed     integer;
    visited  integer := 0;
    capped   integer := 0;
    whole    boolean;
    at_end   boolean := true;
    more     boolean;
    users    uuid[];
begin
    select * into st from iamb.server_state;
    cut := now_ms - st.retention_days * 86400000::bigint;
    hcut := now_ms - st.history_days * 86400000::bigint;
    budget := st.cascade_max;
    share := greatest(st.cascade_max / 4, 1);
    whole := st.job_after is null;

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
    -- Scratch rows a committed transaction left behind (there should be none).
    delete from iamb_private.tx_feed where tx < v_tx and pg_xact_status(tx::text::xid8) is distinct from 'in progress';
    delete from iamb_private.tx_rows where tx < v_tx and pg_xact_status(tx::text::xid8) is distinct from 'in progress';

    -- C24 steps 2 and 3: profile by profile from where the last run stopped, groups first, each
    -- under its lock, each within its share of the budget.
    for p in
        select id, kind, deleted_at, (kind = 'personal') as personal from iamb.profile
        where st.job_after is null
           or ((kind = 'personal'), id) > (st.job_after_personal, st.job_after)
        order by (kind = 'personal'), id
    loop
        if budget <= 0 or visited >= 1000 then
            at_end := false;
            exit;
        end if;
        visited := visited + 1;
        take := least(share, budget);
        used := 0;
        if p.kind <> 'personal' and p.deleted_at is not null and p.deleted_at < cut then
            -- A deleted group, 30 days on: its rows are purged with it, then the profile row.
            loop
                n := iamb_private.purge_group(p.id, take - used);
                used := used + n;
                exit when n = 0 or used >= take or not exists (select 1 from iamb.profile where id = p.id);
            end loop;
        else
            used := iamb_private.purge_step(p.id, cut, take);
            if p.kind <> 'personal' then
                -- Invitations and offers of ownership belong to the group.
                update iamb.invitation set status = 'expired', answered_at = now_ms
                where profile_id = p.id and status = 'pending' and expires_at < now_ms;
                delete from iamb.invitation where profile_id = p.id and status <> 'pending' and answered_at < hcut;
                update iamb.profile set offered_owner = null, offered_at = null
                where id = p.id and offered_owner is not null and offered_at < now_ms - 14 * 86400000::bigint;
            end if;
            -- Conflicts of the profile: an open one expires after 90 days; a closed one goes 30
            -- days after it was closed.
            update iamb.conflict set state = 'expired', resolved_at = now_ms
            where profile_id = p.id and state = 'open' and created_at < hcut;
            delete from iamb.conflict where profile_id = p.id and state <> 'open' and resolved_at < cut;
            -- Attachments nothing has used for `history_days` become tombstones (a write by the
            -- server), within what is left of the profile's share.
            for a in select at.id from iamb.attachment at
                     where at.profile_id = p.id and at.deleted_at is null and at.unlinked_at is not null and at.unlinked_at < hcut
                     order by at.id limit greatest(take - used, 0) loop
                perform iamb_private.lock_profiles(array[p.id]);
                r := iamb_private.next_rev(array[p.id]);
                perform iamb_private.write_row(null, null, null, 'attachment', iamb_private.get_row('attachment', a.id),
                    iamb_private.get_row('attachment', a.id) || jsonb_build_object('deleted_at', now_ms, 'rev', r,
                        'updated_at', now_ms, 'updated_by', null, 'updated_device', null, 'updated_op', null),
                    r, iamb_private.readers('attachment', a.id), false, p.id);
                used := used + 1;
            end loop;
        end if;
        budget := budget - used;
        if used >= take then
            capped := capped + 1;
        end if;
        st.job_after_personal := p.personal;
        st.job_after := p.id;
    end loop;
    -- The next run goes on after the last profile visited; after the last profile of all it
    -- begins again at the start.
    if at_end then
        update iamb.server_state set job_after = null, job_after_personal = false where id;
    else
        update iamb.server_state set job_after = st.job_after, job_after_personal = st.job_after_personal where id;
    end if;
    more := capped > 0 or not at_end or not whole;

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
                                                             'feed', feed, 'more', more, 'profiles', visited,
                                                             'capped', capped, 'answer', 'ok'));
    return jsonb_build_object('rows', st.cascade_max - budget, 'devices', devices, 'feed', feed, 'more', more,
                              'profiles', visited, 'capped', capped);
end
$$;

revoke execute on function iamb_private.daily_job_step() from public, anon, authenticated, service_role;
