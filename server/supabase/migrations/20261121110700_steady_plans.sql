-- Iamb server, migration 18: plans that do not depend on how large a table looked when a session
-- first planned a statement.
--
-- Found on the hosted project while measuring the fixes of the security review of task 4.3: the
-- folder delete of `tests/perf.sql` (2,000 rows by cascade) took between 3 and 21 seconds there,
-- against push's 20 s. The REST interface and the Management API keep database sessions open, and
-- a session keeps the plan of a function's statement. A plan made while a table was empty (a new
-- project; the scratch tables, which are empty between requests) can scan that table once per row
-- of the other side of a join, which is harmless for a handful of rows and quadratic for the
-- thousands of one large request, and so can a plan that finds a row by a less exact index than
-- the one made for it (a profile's rows instead of the row's id). Three places could be planned
-- that way:
--
--  - `feed_flush` joins the scratch table with itself and with `iamb.feed`. It is now planned anew
--    at every call (it runs once per request), from the tables as they are then, and with
--    sequential scans switched off: every statement in it has an index to go by. Its two clean-up
--    steps no longer join the scratch table with itself: each reads it in one pass and deletes by
--    a list. And a new `row` entry replaces the older one for the same user and row through the
--    unique index made for that, by `on conflict`, instead of a delete that had to find it.
--  - `store_row` updated the row found by a join with the new values. It now names the row by its
--    id, like every other statement that reads one row, and compares the profile it expects in a
--    way no other index can serve.
--  - `refresh_access` looked up, for every entry that ended or began, whether its parent's entry
--    ended or began too, by a join of two lists. It now asks a hashed list.
-- Nothing else changes: the same rows are written.

do $$
begin
    if current_user <> 'postgres' then
        raise exception 'iamb migrations must run as postgres, not %', current_user;
    end if;
end
$$;


-- Writes the collected entries last (§5.5, C24 step 4); see migration 11. Planned anew at every
-- call and without sequential scans; the two clean-up steps read the scratch table in one pass.
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

-- Writes a whole row of a synced content table from its jsonb form. A create inserts, and fails
-- when the id is in use; an update changes the row with that id in the profile the caller read it
-- in, never `backed` (set at an attachment row's creation and by the upload's commit only), and
-- fails when no such row is there.
create or replace function iamb_private.store_row(p_entity text, j jsonb, p_create boolean) returns void
language plpgsql volatile set search_path = pg_catalog, pg_temp as $$
declare
    n integer;
begin
    if p_create then
        case p_entity
        when 'folder' then insert into iamb.folder select * from jsonb_populate_record(null::iamb.folder, j);
        when 'folder_share' then insert into iamb.folder_share select * from jsonb_populate_record(null::iamb.folder_share, j);
        when 'snippet' then insert into iamb.snippet select * from jsonb_populate_record(null::iamb.snippet, j);
        when 'tag' then insert into iamb.tag select * from jsonb_populate_record(null::iamb.tag, j);
        when 'snippet_tag' then insert into iamb.snippet_tag select * from jsonb_populate_record(null::iamb.snippet_tag, j);
        when 'attachment' then insert into iamb.attachment select * from jsonb_populate_record(null::iamb.attachment, j);
        when 'snippet_attachment' then insert into iamb.snippet_attachment select * from jsonb_populate_record(null::iamb.snippet_attachment, j);
        else raise exception 'store_row: unknown entity';
        end case;
        return;
    end if;
    case p_entity
    when 'folder' then
        update iamb.folder t set
            updated_at = x.updated_at, updated_by = x.updated_by, rev = x.rev,
            deleted_at = x.deleted_at, updated_device = x.updated_device,
            updated_op = x.updated_op, lost_rev = x.lost_rev,
            parent_id = x.parent_id, name = x.name, position = x.position,
            member_permission = x.member_permission, default_trigger = x.default_trigger,
            default_case_mode = x.default_case_mode, abbreviation_prefix = x.abbreviation_prefix
        from jsonb_populate_record(null::iamb.folder, j) x
        where t.id = (j ->> 'id')::uuid and t.profile_id is not distinct from x.profile_id;
    when 'folder_share' then
        update iamb.folder_share t set
            updated_at = x.updated_at, updated_by = x.updated_by, rev = x.rev,
            deleted_at = x.deleted_at, updated_device = x.updated_device,
            updated_op = x.updated_op, lost_rev = x.lost_rev, permission = x.permission
        from jsonb_populate_record(null::iamb.folder_share, j) x
        where t.id = (j ->> 'id')::uuid and t.folder_id is not distinct from x.folder_id and t.profile_id is not distinct from x.profile_id;
    when 'snippet' then
        update iamb.snippet t set
            updated_at = x.updated_at, updated_by = x.updated_by, rev = x.rev,
            deleted_at = x.deleted_at, updated_device = x.updated_device,
            updated_op = x.updated_op, lost_rev = x.lost_rev,
            folder_id = x.folder_id, abbreviation = x.abbreviation, label = x.label,
            kind = x.kind, body = x.body, trigger_mode = x.trigger_mode,
            case_mode = x.case_mode, word_start = x.word_start, enabled = x.enabled,
            content_by = x.content_by, content_at = x.content_at
        from jsonb_populate_record(null::iamb.snippet, j) x
        where t.id = (j ->> 'id')::uuid and t.profile_id is not distinct from x.profile_id;
    when 'tag' then
        update iamb.tag t set
            updated_at = x.updated_at, updated_by = x.updated_by, rev = x.rev,
            deleted_at = x.deleted_at, updated_device = x.updated_device,
            updated_op = x.updated_op, lost_rev = x.lost_rev, name = x.name, color = x.color
        from jsonb_populate_record(null::iamb.tag, j) x
        where t.id = (j ->> 'id')::uuid and t.profile_id is not distinct from x.profile_id;
    when 'snippet_tag' then
        update iamb.snippet_tag t set
            updated_at = x.updated_at, updated_by = x.updated_by, rev = x.rev,
            deleted_at = x.deleted_at, updated_device = x.updated_device,
            updated_op = x.updated_op, lost_rev = x.lost_rev
        from jsonb_populate_record(null::iamb.snippet_tag, j) x
        where t.id = (j ->> 'id')::uuid and t.snippet_id is not distinct from x.snippet_id and t.tag_id is not distinct from x.tag_id;
    when 'attachment' then
        update iamb.attachment t set
            updated_at = x.updated_at, updated_by = x.updated_by, rev = x.rev,
            deleted_at = x.deleted_at, updated_device = x.updated_device,
            updated_op = x.updated_op, lost_rev = x.lost_rev, unlinked_at = x.unlinked_at
        from jsonb_populate_record(null::iamb.attachment, j) x
        where t.id = (j ->> 'id')::uuid and t.profile_id is not distinct from x.profile_id and t.sha256 is not distinct from x.sha256;
    when 'snippet_attachment' then
        update iamb.snippet_attachment t set
            updated_at = x.updated_at, updated_by = x.updated_by, rev = x.rev,
            deleted_at = x.deleted_at, updated_device = x.updated_device,
            updated_op = x.updated_op, lost_rev = x.lost_rev
        from jsonb_populate_record(null::iamb.snippet_attachment, j) x
        where t.id = (j ->> 'id')::uuid and t.snippet_id is not distinct from x.snippet_id and t.attachment_id is not distinct from x.attachment_id;
    else
        raise exception 'store_row: unknown entity';
    end case;
    get diagnostics n = row_count;
    if n <> 1 then
        raise exception using errcode = 'integrity_constraint_violation',
            message = 'store_row: the row to update is not where it was read';
    end if;
end
$$;

-- Recomputes the access entries of the folders of the given profiles and writes the visibility
-- entries of the feed (C21); see migration 11. The two lists of ended and begun entries are asked
-- as hashed lists.
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

revoke execute on function iamb_private.store_row(text, jsonb, boolean), iamb_private.refresh_access(uuid[]),
                           iamb_private.feed_flush() from public, anon, authenticated, service_role;
