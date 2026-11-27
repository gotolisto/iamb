-- Iamb server, migration 19: an access entry begins when the request that made it has ended
-- (docs/reviews/4.3-security-reverify.md, N1).
--
-- Migration 11 gave every access entry `since_rev`, the server's revision counter when the entry
-- began, and migration 12 offers a replaced version of a row to a reader whose entry for the
-- version's folder began before the version was replaced. The entry was stamped when the op that
-- made it was applied, in the middle of its request. A push applies folder ops before snippet
-- ops, so "she emptied the snippet, then moved its folder into the shared tree", sent as one push,
-- was judged as "moved, then emptied": the member's entry began before the private version was
-- replaced, and the version counted as one he could read, although no committed state of the
-- database ever showed it to him.
--
-- Now:
--  - an entry is written as "not begun" (`since_rev` at the highest bigint, below which every
--    revision lies) and gets its number when the request ends: the counter's value then, which
--    is at or above every revision the request drew. `feed_flush`, the last step of every writing
--    request, does it. Until then the entry offers no version to anyone, so a path that forgot the
--    step would withhold versions, never hand one over (the tests check after every step that no
--    entry is left that way);
--  - a version that was written and replaced within one request was never the row in a committed
--    state. `history.within_request` marks it, and only its writer is offered it, whatever access
--    entries exist. (A request can name one row twice.)
-- "Could read it while it was the row" now means what it says: in a state the reader could have
-- seen. Versions replaced in a later request are judged as before.

do $$
begin
    if current_user <> 'postgres' then
        raise exception 'iamb migrations must run as postgres, not %', current_user;
    end if;
end
$$;

-- The entries a database already holds got their number in the middle of their request, and
-- which revisions that request went on to draw is not known any more. They begin now, once (when
-- this migration is first applied): a version replaced before today is then offered to its writer
-- and to nobody else. That withholds some versions from readers who could have seen them; it
-- hands none to a reader who could not.
do $do$
begin
    if not exists (select 1 from information_schema.columns
                   where table_schema = 'iamb' and table_name = 'history' and column_name = 'within_request') then
        update iamb.folder_access set since_rev = (select last_value from iamb.rev_seq)
        where since_rev < (select last_value from iamb.rev_seq);
    end if;
end
$do$;

alter table iamb.history add column if not exists within_request boolean not null default false;
create index if not exists folder_access_not_begun on iamb.folder_access (user_id, folder_id)
    where since_rev = 9223372036854775807;

-- Gives the entries made in this request their starting revision: the counter as it stands now,
-- after everything the request drew. Called by `feed_flush`.
create or replace function iamb_private.access_begin() returns void
language plpgsql volatile set search_path = pg_catalog, pg_temp set enable_seqscan = off as $$
declare
    v bigint;
begin
    if exists (select 1 from iamb.folder_access where since_rev = 9223372036854775807) then
        select last_value into v from iamb.rev_seq;
        update iamb.folder_access set since_rev = v where since_rev = 9223372036854775807;
    end if;
end
$$;

-- Keeps a replaced version. One that this same request wrote (the row is among the rows the
-- request has written, `tx_rows`) is marked: it was never the row in a committed state.
create or replace function iamb_private.keep_history(p_entity text, old jsonb, p_replaced_by bigint) returns void
language plpgsql volatile set search_path = pg_catalog, pg_temp as $$
begin
    insert into iamb.history (entity, row_id, rev, row, deleted, written_at, written_by, written_device,
                              replaced_at, replaced_by_rev, within_request)
    values (p_entity, (old ->> 'id')::uuid, (old ->> 'rev')::bigint, old, (old ->> 'deleted_at') is not null,
            (old ->> 'updated_at')::bigint, (old ->> 'updated_by')::uuid, (old ->> 'updated_device')::uuid,
            iamb_private.now_ms(), p_replaced_by,
            exists (select 1 from iamb_private.tx_rows t
                    where t.tx = iamb_private.tx() and t.entity = p_entity and t.id = (old ->> 'id')::uuid))
    on conflict (entity, row_id, rev) do nothing;
end
$$;

-- Whether the user could read a replaced version of a row while it was the row (C3, C17, C20), in
-- a state of the database the user could have seen: they wrote that version; or the version was
-- not written and replaced within one request, and they have an access entry now for the folder
-- that version was in (for a folder: for the folder itself) that began before the version was
-- replaced (`since_rev`, §5.4: the counter's value at the end of the request that made the entry,
-- so a version replaced in that same request is not below it); for a tag, they own the personal
-- profile or have been a member of the group since before the version was replaced.
create or replace function iamb_private.version_readable(u uuid, p_entity text, hrow jsonb, p_replaced_by bigint)
returns boolean
language sql stable set search_path = pg_catalog, pg_temp as $$
    select coalesce(
        iamb_private.try_uuid(hrow ->> 'updated_by') = u
        or not exists (select 1 from iamb.history h
                       where h.entity = p_entity and h.row_id = iamb_private.try_uuid(hrow ->> 'id')
                         and h.rev = (hrow ->> 'rev')::bigint and h.within_request)
        and case p_entity
            when 'snippet' then exists (
                select 1 from iamb.folder_access fa
                where fa.user_id = u and fa.folder_id = iamb_private.try_uuid(hrow ->> 'folder_id')
                  and fa.since_rev < p_replaced_by)
            when 'folder' then exists (
                select 1 from iamb.folder_access fa
                where fa.user_id = u and fa.folder_id = iamb_private.try_uuid(hrow ->> 'id')
                  and fa.since_rev < p_replaced_by)
            when 'tag' then exists (
                select 1 from iamb.profile p
                where p.id = iamb_private.try_uuid(hrow ->> 'profile_id') and p.deleted_at is null
                  and ((p.kind = 'personal' and p.owner_user_id = u)
                       or (p.kind <> 'personal' and exists (
                             select 1 from iamb.membership m
                             where m.profile_id = p.id and m.user_id = u and m.deleted_at is null
                               and m.created_rev < p_replaced_by))))
            else false
        end, false)
$$;

-- Recomputes the access entries of the folders of the given profiles and writes the visibility
-- entries of the feed (C21); see migrations 11 and 18. A new entry is written as not begun;
-- `access_begin` gives it its starting revision when the request ends.
create or replace function iamb_private.refresh_access(p_profiles uuid[]) returns void
language plpgsql volatile set search_path = pg_catalog, pg_temp as $$
declare
    now_ms  bigint := iamb_private.now_ms();
    gone_u  uuid[];
    gone_f  uuid[];
    came_u  uuid[];
    came_f  uuid[];
    n_changed bigint;
    r       record;
begin
    with n as materialized (
        select * from iamb_private.compute_access(p_profiles)
    ),
    gone as (
        delete from iamb.folder_access fa
        where fa.profile_id = any (p_profiles)
          and not exists (select 1 from n where n.user_id = fa.user_id and n.folder_id = fa.folder_id)
        returning fa.user_id, fa.folder_id
    ),
    changed as (
        update iamb.folder_access fa
        set permission = n.permission, via = n.via, root_folder_id = n.root_folder_id
        from n
        where n.user_id = fa.user_id and n.folder_id = fa.folder_id
          and (fa.permission, fa.via, fa.root_folder_id) is distinct from (n.permission, n.via, n.root_folder_id)
        returning 1
    ),
    came as (
        insert into iamb.folder_access (user_id, folder_id, profile_id, permission, via, granted_at, root_folder_id, since_rev)
        select n.user_id, n.folder_id, n.profile_id, n.permission, n.via,
               coalesce((select o.granted_at from iamb.folder_access o
                         where o.user_id = n.user_id and o.folder_id = n.root_folder_id
                           and o.root_folder_id = n.root_folder_id), now_ms),
               n.root_folder_id, 9223372036854775807
        from n
        where not exists (select 1 from iamb.folder_access o where o.user_id = n.user_id and o.folder_id = n.folder_id)
        returning user_id, folder_id
    )
    select (select coalesce(array_agg(g.user_id), '{}') from gone g),
           (select coalesce(array_agg(g.folder_id), '{}') from gone g),
           (select coalesce(array_agg(c.user_id), '{}') from came c),
           (select coalesce(array_agg(c.folder_id), '{}') from came c),
           (select count(*) from changed)
    into gone_u, gone_f, came_u, came_f, n_changed;

    -- Revoked: the tops of the lost trees, with what stays readable inside them.
    for r in
        select g.user_id, g.folder_id
        from unnest(gone_u, gone_f) as g(user_id, folder_id)
        join iamb.folder f on f.id = g.folder_id
        where f.parent_id is null
           or (g.user_id, f.parent_id) not in (select g2.user_id, g2.folder_id
                                               from unnest(gone_u, gone_f) as g2(user_id, folder_id))
        order by g.user_id, g.folder_id
    loop
        perform iamb_private.queue(array[r.user_id], 'folder_revoked', 'folder', null, r.folder_id, null, null,
            jsonb_build_object('except', coalesce((
                with recursive sub as (
                    select c.id, c.parent_id from iamb.folder c where c.parent_id = r.folder_id
                    union all
                    select c.id, c.parent_id from iamb.folder c join sub on c.parent_id = sub.id
                )
                select jsonb_agg(sub.id order by sub.id) from sub
                where exists (select 1 from iamb.folder_access n where n.user_id = r.user_id and n.folder_id = sub.id)
                  and not exists (select 1 from iamb.folder_access n where n.user_id = r.user_id and n.folder_id = sub.parent_id)
            ), '[]'::jsonb)));
    end loop;
    -- Visible: the tops of the gained trees.
    for r in
        select c.user_id, c.folder_id
        from unnest(came_u, came_f) as c(user_id, folder_id)
        join iamb.folder f on f.id = c.folder_id
        where f.deleted_at is null
          and (f.parent_id is null
               or (c.user_id, f.parent_id) not in (select c2.user_id, c2.folder_id
                                                   from unnest(came_u, came_f) as c2(user_id, folder_id)))
        order by c.user_id, c.folder_id
    loop
        perform iamb_private.queue(array[r.user_id], 'folder_visible', 'folder', null, r.folder_id, null, null, null);
    end loop;
end
$$;

-- Writes the collected entries last (§5.5, C24 step 4); see migrations 11 and 18. Before that it
-- lets the access entries of this request begin.
create or replace function iamb_private.feed_flush() returns void
language plpgsql volatile set search_path = pg_catalog, pg_temp
set enable_seqscan = off set plan_cache_mode = force_custom_plan as $$
declare
    v_tx   bigint := iamb_private.tx();
    users  uuid[];
    ep     uuid;
    now_ms bigint := iamb_private.now_ms();
    h      record;
    made   uuid[];
    older  bigint[];
begin
    -- The request's last step: the access entries it made begin now (N1).
    perform iamb_private.access_begin();
    select array_agg(distinct user_id order by user_id) into users from iamb_private.tx_feed where tx = v_tx;
    if users is null then
        delete from iamb_private.tx_rows where tx = v_tx;
        return;
    end if;
    select epoch into ep from iamb.server_state;
    insert into iamb.feed_head (user_id) select u from unnest(users) u order by u on conflict (user_id) do nothing;
    perform 1 from iamb.feed_head where user_id = any (users) order by user_id for update;

    -- A folder created in this transaction whose own `row` entry a user gets needs no
    -- `folder_visible` for that user as well.
    select array_agg(t.id) into made from iamb_private.tx_rows t
    where t.tx = v_tx and t.entity = 'folder' and t.created;
    if made is not null then
        delete from iamb_private.tx_feed p
        where p.tx = v_tx and p.kind = 'folder_visible' and p.folder_id = any (made)
          and exists (select 1 from iamb_private.tx_feed q
                      where q.tx = v_tx and q.user_id = p.user_id and q.entity = 'folder'
                        and q.row_id = p.folder_id and q.kind = 'row');
    end if;
    -- Within this transaction, the last `row` entry for a user and row stands for the others.
    -- (Read in one pass and deleted by a list: a join of the table with itself can be planned so
    -- that one side is read once per row of the other.)
    select array_agg(d.ord) into older
    from (select q.ord, row_number() over (partition by q.user_id, q.entity, q.row_id order by q.ord desc) as rn
          from iamb_private.tx_feed q where q.tx = v_tx and q.kind = 'row') d
    where d.rn > 1;
    if older is not null then
        delete from iamb_private.tx_feed p where p.tx = v_tx and p.ord = any (older);
    end if;
    -- One entry per row (§5.5): a new `row` entry takes the place of the older one for the same
    -- user and row, found through the unique index made for it (`feed_one_per_row`), which the
    -- `on conflict` names; entries of the other kinds never meet it.
    insert into iamb.feed as f (user_id, seq, kind, entity, row_id, folder_id, profile_id, sha256, detail, created_at)
    select p.user_id,
           fh.head + row_number() over (partition by p.user_id order by p.ord),
           p.kind, p.entity, p.row_id, p.folder_id, p.profile_id, p.sha256, p.detail, now_ms
    from iamb_private.tx_feed p join iamb.feed_head fh on fh.user_id = p.user_id
    where p.tx = v_tx and fh.user_id = any (users)
    on conflict (user_id, entity, row_id) where kind = 'row' do update
        set seq = excluded.seq, folder_id = excluded.folder_id, profile_id = excluded.profile_id,
            sha256 = excluded.sha256, detail = excluded.detail, created_at = excluded.created_at;
    for h in
        update iamb.feed_head fh set head = fh.head + c.n
        from (select user_id, count(*) as n from iamb_private.tx_feed where tx = v_tx group by user_id) c
        where fh.user_id = c.user_id
        returning fh.user_id, fh.head
    loop
        perform realtime.send(jsonb_build_object('epoch', ep, 'head', h.head), 'head',
                              'feed:' || h.user_id::text, true);
    end loop;
    delete from iamb_private.tx_feed where tx = v_tx;
    delete from iamb_private.tx_rows where tx = v_tx;
end
$$;

revoke execute on function iamb_private.access_begin(), iamb_private.keep_history(text, jsonb, bigint),
                           iamb_private.version_readable(uuid, text, jsonb, bigint),
                           iamb_private.refresh_access(uuid[]), iamb_private.feed_flush()
    from public, anon, authenticated, service_role;
