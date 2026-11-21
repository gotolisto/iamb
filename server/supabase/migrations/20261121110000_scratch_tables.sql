-- Iamb server, migration 11: no statement without a `WHERE`, and no scratch table in the caller's
-- session (docs/reviews/4.3-security-review.md, findings 14 and 16).
--
-- Finding 14: the role the REST interface connects with loads `safeupdate` on the platform, which
-- refuses an `UPDATE` or `DELETE` without a `WHERE` clause, also inside a definer function. The
-- functions emptied their scratch tables that way, so every writing call failed through that
-- interface. Finding 16: those scratch tables were temporary tables, which live in the session of
-- the caller, who could have made a table of the same name first.
--
-- From here on the rows collected during a call (pending feed entries, the rows written) are in
-- two ordinary tables of `iamb_private`, keyed by the transaction, readable and writable by the
-- tables' owner alone, and deleted with a `WHERE` at the end of the call. `refresh_access` needs
-- no scratch table any more: it changes `folder_access` in one statement and works from what that
-- statement returns. The scratch tables of `push` go in the next migration.
--
-- Also here, because `refresh_access` writes it: `folder_access.since_rev`, the server's revision
-- counter when the entry began (finding 1; the rule is in migration 13).

do $$
begin
    if current_user <> 'postgres' then
        raise exception 'iamb migrations must run as postgres, not %', current_user;
    end if;
end
$$;

-- ---------------------------------------------------------------------------------------------
-- The scratch tables. Unlogged: their rows never outlive the transaction that wrote them.
-- ---------------------------------------------------------------------------------------------

create unlogged table if not exists iamb_private.tx_feed (
    tx         bigint not null,
    ord        bigint generated always as identity,
    user_id    uuid   not null,
    kind       text   not null,
    entity     text,
    row_id     uuid,
    folder_id  uuid,
    profile_id uuid,
    sha256     text,
    detail     jsonb
);
create index if not exists tx_feed_tx on iamb_private.tx_feed (tx, user_id, entity, row_id, ord);

create unlogged table if not exists iamb_private.tx_rows (
    tx      bigint  not null,
    entity  text    not null,
    id      uuid    not null,
    created boolean not null,
    primary key (tx, entity, id)
);

alter table iamb_private.tx_feed enable row level security;
alter table iamb_private.tx_rows enable row level security;
revoke all on iamb_private.tx_feed, iamb_private.tx_rows from public, anon, authenticated, service_role;

-- The current transaction as a number. Rows of the scratch tables carry it, so that a row another
-- transaction left behind is never taken for one of this call's.
create or replace function iamb_private.tx() returns bigint
language sql stable set search_path = pg_catalog, pg_temp as $$
    select (pg_current_xact_id()::text)::bigint
$$;

-- Kept for the callers of earlier migrations; there is nothing to prepare any more.
create or replace function iamb_private.tx_begin() returns void
language plpgsql volatile set search_path = pg_catalog, pg_temp as $$
begin
    return;
end
$$;

create or replace function iamb_private.queue(p_users uuid[], p_kind text, p_entity text, p_row uuid,
                                              p_folder uuid, p_profile uuid, p_sha256 text,
                                              p_detail jsonb) returns void
language plpgsql volatile set search_path = pg_catalog, pg_temp as $$
begin
    insert into iamb_private.tx_feed (tx, user_id, kind, entity, row_id, folder_id, profile_id, sha256, detail)
    select distinct iamb_private.tx(), u, p_kind, p_entity, p_row, p_folder, p_profile, p_sha256, p_detail
    from unnest(p_users) u where u is not null;
end
$$;

create or replace function iamb_private.mark_written(p_entity text, p_id uuid, p_created boolean) returns void
language plpgsql volatile set search_path = pg_catalog, pg_temp as $$
begin
    insert into iamb_private.tx_rows as t (tx, entity, id, created)
    values (iamb_private.tx(), p_entity, p_id, p_created)
    on conflict (tx, entity, id) do update set created = t.created or excluded.created;
end
$$;

create or replace function iamb_private.created_in_tx(p_entity text, p_id uuid) returns boolean
language sql stable set search_path = pg_catalog, pg_temp as $$
    select exists (select 1 from iamb_private.tx_rows
                   where tx = iamb_private.tx() and entity = p_entity and id = p_id and created)
$$;

-- Writes the collected entries last (§5.5, C24 step 4): locks the `feed_head` rows of every user
-- concerned in the order of the user ids, gives each entry `seq = head + 1`, replaces an older
-- `row` entry for the same user and row, and pokes each user's live channel. The locks are held to
-- the commit, so entries become visible in `seq` order.
create or replace function iamb_private.feed_flush() returns void
language plpgsql volatile set search_path = pg_catalog, pg_temp as $$
declare
    v_tx   bigint := iamb_private.tx();
    users  uuid[];
    ep     uuid;
    now_ms bigint := iamb_private.now_ms();
    h      record;
begin
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
    delete from iamb_private.tx_feed p
    where p.tx = v_tx and p.kind = 'folder_visible'
      and exists (select 1 from iamb_private.tx_rows t
                  where t.tx = v_tx and t.entity = 'folder' and t.id = p.folder_id and t.created)
      and exists (select 1 from iamb_private.tx_feed q
                  where q.tx = v_tx and q.user_id = p.user_id and q.kind = 'row' and q.entity = 'folder'
                    and q.row_id = p.folder_id);
    -- Within this transaction, the last `row` entry for a user and row stands for the others.
    delete from iamb_private.tx_feed p
    using (select q.ord, row_number() over (partition by q.user_id, q.entity, q.row_id order by q.ord desc) as rn
           from iamb_private.tx_feed q where q.tx = v_tx and q.kind = 'row') d
    where p.tx = v_tx and p.ord = d.ord and d.rn > 1;
    delete from iamb.feed f using iamb_private.tx_feed p
    where p.tx = v_tx and p.kind = 'row' and f.user_id = p.user_id and f.kind = 'row'
      and f.entity = p.entity and f.row_id = p.row_id;
    insert into iamb.feed (user_id, seq, kind, entity, row_id, folder_id, profile_id, sha256, detail, created_at)
    select p.user_id,
           fh.head + row_number() over (partition by p.user_id order by p.ord),
           p.kind, p.entity, p.row_id, p.folder_id, p.profile_id, p.sha256, p.detail, now_ms
    from iamb_private.tx_feed p join iamb.feed_head fh on fh.user_id = p.user_id
    where p.tx = v_tx;
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

-- ---------------------------------------------------------------------------------------------
-- Access entries (§5.4, C21)
-- ---------------------------------------------------------------------------------------------

-- `since_rev`: the value of the server's revision counter when this entry began. Every write of a
-- profile draws its revision under the profile's lock, and so does the change that makes an
-- entry: a version of a row that a later write replaced with a revision above `since_rev` was
-- there to be read while the entry existed; one replaced at or below it was not. An entry that
-- ends and begins again begins with a new value. Entries from before this column count from now.
do $$
begin
    if not exists (select 1 from information_schema.columns
                   where table_schema = 'iamb' and table_name = 'folder_access' and column_name = 'since_rev') then
        alter table iamb.folder_access add column since_rev bigint;
        update iamb.folder_access set since_rev = (select last_value from iamb.rev_seq) where since_rev is null;
        alter table iamb.folder_access alter column since_rev set not null;
    end if;
end
$$;

-- Recomputes the access entries of the folders of the given profiles and writes the visibility
-- entries of the feed (C21): for a user, `folder_visible` for the top of each newly readable tree
-- (the feed writer drops it for a folder created in this transaction whose own `row` entry the
-- user gets) and `folder_revoked` for the top of each tree that stopped being readable, with the
-- parts inside it that stay readable in `except`. Called in the same transaction as every change
-- that can change the entries, under the locks of C24.
--
-- One statement changes the table and answers which entries ended and which began; every part of
-- it reads the table as it was before the statement, so `granted_at` of a new entry is still the
-- time access to its root began: that of the root's own entry when it was already there, else now
-- (D13 orders shared folders by it).
create or replace function iamb_private.refresh_access(p_profiles uuid[]) returns void
language plpgsql volatile set search_path = pg_catalog, pg_temp as $$
declare
    now_ms  bigint := iamb_private.now_ms();
    v_rev   bigint;
    gone_u  uuid[];
    gone_f  uuid[];
    came_u  uuid[];
    came_f  uuid[];
    n_changed bigint;
    r       record;
begin
    select last_value into v_rev from iamb.rev_seq;
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
               n.root_folder_id, v_rev
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
           or not exists (select 1 from unnest(gone_u, gone_f) as g2(user_id, folder_id)
                          where g2.user_id = g.user_id and g2.folder_id = f.parent_id)
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
               or not exists (select 1 from unnest(came_u, came_f) as c2(user_id, folder_id)
                              where c2.user_id = c.user_id and c2.folder_id = f.parent_id))
        order by c.user_id, c.folder_id
    loop
        perform iamb_private.queue(array[r.user_id], 'folder_visible', 'folder', null, r.folder_id, null, null, null);
    end loop;
end
$$;

-- ---------------------------------------------------------------------------------------------
-- `begin_epoch` (C18), with a `WHERE` on every statement.
-- ---------------------------------------------------------------------------------------------

create or replace function iamb_private.begin_epoch(p_not_before bigint) returns jsonb
language plpgsql volatile set search_path = pg_catalog, pg_temp as $$
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
    update iamb.server_state set epoch = e where id;
    delete from iamb.feed where true;
    update iamb.feed_head set floor = head where true;
    update iamb.device set position_token = null, position_token_before = null, last_seen_at = null where true;
    perform iamb_private.log('begin_epoch', jsonb_build_object('epoch', e, 'first_rev', first, 'not_before', p_not_before));
    return jsonb_build_object('epoch', e, 'first_rev', first, 'started_at', now_ms);
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
    iamb_private.position(uuid, text, text, bigint)
to authenticated;
