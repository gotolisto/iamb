-- Iamb server, migration 17: whole reads bounded in bytes (docs/reviews/4.3-security-review.md,
-- finding 18), the search path of every function (finding 16), and the final grants.
--
-- `fetch_folder` and `snapshot` ended a page at 500 rows whatever their size: 500 snippets of
-- 400,000 bytes were one answer of 200 MB. A page now ends at 500 rows or about 4 MiB, whichever
-- comes first, and always holds at least one row; the cursor goes on from there, so a device that
-- follows the cursor notices nothing. (`rows` and the rows in a push's results are not bounded in
-- bytes yet: that needs a short form in the protocol, as the cascade answer has; see the review's
-- resolution.)

do $$
begin
    if current_user <> 'postgres' then
        raise exception 'iamb migrations must run as postgres, not %', current_user;
    end if;
end
$$;


create or replace function api.fetch_folder(device uuid, protocol integer, folder uuid, cursor text default null)
returns jsonb language plpgsql volatile security invoker set search_path = pg_catalog, pg_temp set statement_timeout = '15s' as $$
declare
    e      jsonb := iamb_private.enter(device, protocol, 'fetch_folder');
    uid    uuid := auth.uid();
    v_rows jsonb;
    v_next text;
begin
    if e ->> 'error' is not null then return e; end if;
    if not exists (select 1 from iamb.folder f where f.id = folder and f.deleted_at is null) then
        return jsonb_build_object('error', 'not_found', 'epoch', e -> 'epoch');
    end if;
    with recursive tree as (
        select f.id, 1 as depth, array[f.id] as path from iamb.folder f where f.id = folder and f.deleted_at is null
        union all
        select f.id, tree.depth + 1, tree.path || f.id from iamb.folder f join tree on f.parent_id = tree.id
        where f.deleted_at is null and not f.id = any (tree.path)
    ),
    snips as (select s.id from iamb.snippet s join tree on s.folder_id = tree.id where s.deleted_at is null),
    tl as (select l.id, l.tag_id from iamb.snippet_tag l join snips on snips.id = l.snippet_id where l.deleted_at is null),
    al as (select l.id, l.attachment_id from iamb.snippet_attachment l join snips on snips.id = l.snippet_id where l.deleted_at is null),
    keyed as (
        select '1' || lpad(tree.depth::text, 3, '0') || tree.id::text as k, 'folder' as e, tree.id from tree
        union all select '2000' || x.id::text, 'folder_share', x.id from iamb.folder_share x join tree on x.folder_id = tree.id where x.deleted_at is null
        union all select '3000' || p.id::text, 'profile', p.id from iamb.profile p where p.id = (select f.profile_id from iamb.folder f where f.id = folder)
        union all select '4000' || t.id::text, 'tag', t.id from iamb.tag t where t.id in (select tl.tag_id from tl) and t.deleted_at is null
        union all select '5000' || a.id::text, 'attachment', a.id from iamb.attachment a where a.id in (select al.attachment_id from al) and a.deleted_at is null
        union all select '6000' || snips.id::text, 'snippet', snips.id from snips
        union all select '7000' || tl.id::text, 'snippet_tag', tl.id from tl
        union all select '8000' || al.id::text, 'snippet_attachment', al.id from al
    ),
    page as (
        select keyed.k, w.entity, w.id, w.row, w.origin, row_number() over (order by keyed.k) as rn
        from keyed join iamb.wire_rows w on w.entity = keyed.e and w.id = keyed.id
        where cursor is null or keyed.k > cursor
        order by keyed.k
        limit 501
    ),
    shown as (
        select page.*,
               case when page.entity = 'attachment' then jsonb_build_object('verified', exists (
                        select 1 from iamb.attachment_blob b
                        where b.profile_id = (page.row ->> 'profile_id')::uuid and b.sha256 = page.row ->> 'sha256'))
                    else page.origin end as org
        from page
    ),
    sized as (
        select shown.*,
               coalesce(sum(octet_length(shown.row::text)) over (order by shown.k rows between unbounded preceding and 1 preceding), 0) as bytes_before
        from shown
    ),
    taken as (
        -- At most 500 rows and about 4 MiB a page; always at least one row.
        select * from sized where sized.rn <= 500 and sized.bytes_before < 4000000
    )
    select (select jsonb_agg(jsonb_build_object('entity', taken.entity, 'id', taken.id, 'row', taken.row, 'origin', taken.org)
                             order by taken.k) from taken),
           case when (select count(*) from shown) > (select count(*) from taken) then (select max(taken.k) from taken) end
    into v_rows, v_next;
    return jsonb_build_object('error', null, 'epoch', e -> 'epoch', 'rows', coalesce(v_rows, '[]'),
                              'cursor', v_next,
                              'head', case when cursor is null then
                                  coalesce((select fh.head from iamb.feed_head fh where fh.user_id = uid), 0) end);
end $$;

create or replace function api.snapshot(device uuid, protocol integer, "position" text default null, cursor text default null)
returns jsonb language plpgsql volatile security invoker set search_path = pg_catalog, pg_temp set statement_timeout = '15s' as $$
declare
    e      jsonb := iamb_private.enter(device, protocol, 'snapshot');
    uid    uuid := auth.uid();
    pc     jsonb;
    v_head bigint;
    v_rows jsonb;
    v_next text;
    res    jsonb;
begin
    if e ->> 'error' is not null then return e; end if;
    res := jsonb_build_object('error', null, 'epoch', e -> 'epoch');
    if cursor is null then
        select fh.head into v_head from iamb.feed_head fh where fh.user_id = uid;
        pc := iamb_private.position(device, "position", 'snapshot', coalesce(v_head, 0));
        if pc ->> 'error' is not null then
            return jsonb_build_object('error', pc ->> 'error', 'epoch', e -> 'epoch');
        end if;
        res := res || jsonb_build_object('start', pc -> 'position', 'head', coalesce(v_head, 0));
        if pc ? 'lost' then
            res := res || jsonb_build_object('lost', pc -> 'lost');
        end if;
    end if;
    with fa as (
        select x.folder_id, x.profile_id from iamb.folder_access x where x.user_id = uid
    ),
    profs as (
        select fa.profile_id as id from fa
        union select p.id from iamb.profile p where p.owner_user_id = uid and p.kind = 'personal'
        union select m.profile_id from iamb.membership m where m.user_id = uid
    ),
    keyed as (
        select '1000' || p.id::text as k, 'profile' as e, p.id from iamb.profile p where p.id in (select id from profs)
        union all select '2000' || m.id::text, 'membership', m.id from iamb.membership m where m.profile_id in (select id from profs)
        union all select '3' || lpad(coalesce(dep.d, 1)::text, 3, '0') || f.id::text, 'folder', f.id
            from iamb.folder f
            left join lateral (
                with recursive up as (
                    select f.parent_id as pid, 1 as d
                    union all
                    select g.parent_id, up.d + 1 from iamb.folder g join up on g.id = up.pid where up.d < 8
                )
                select max(up.d) as d from up
            ) dep on true
            where f.id in (select fa.folder_id from fa)
        union all select '4000' || s.id::text, 'folder_share', s.id from iamb.folder_share s where s.folder_id in (select fa.folder_id from fa)
        union all select '5000' || t.id::text, 'tag', t.id from iamb.tag t where t.profile_id in (select id from profs)
        union all select '6000' || a.id::text, 'attachment', a.id from iamb.attachment a where a.profile_id in (select id from profs)
        union all select '7000' || s.id::text, 'snippet', s.id from iamb.snippet s where s.folder_id in (select fa.folder_id from fa)
        union all select '8000' || l.id::text, 'snippet_tag', l.id from iamb.snippet_tag l join iamb.snippet s on s.id = l.snippet_id
            where s.folder_id in (select fa.folder_id from fa)
        union all select '9000' || l.id::text, 'snippet_attachment', l.id from iamb.snippet_attachment l join iamb.snippet s on s.id = l.snippet_id
            where s.folder_id in (select fa.folder_id from fa)
    ),
    page as (
        select keyed.k, w.entity, w.id, w.row, w.origin, row_number() over (order by keyed.k) as rn
        from keyed join iamb.wire_rows w on w.entity = keyed.e and w.id = keyed.id
        where cursor is null or keyed.k > cursor
        order by keyed.k
        limit 501
    ),
    shown as (
        select page.*,
               case when page.entity = 'attachment' then jsonb_build_object('verified', exists (
                        select 1 from iamb.attachment_blob b
                        where b.profile_id = (page.row ->> 'profile_id')::uuid and b.sha256 = page.row ->> 'sha256'))
                    else page.origin end as org
        from page
    ),
    sized as (
        select shown.*,
               coalesce(sum(octet_length(shown.row::text)) over (order by shown.k rows between unbounded preceding and 1 preceding), 0) as bytes_before
        from shown
    ),
    taken as (
        -- At most 500 rows and about 4 MiB a page; always at least one row.
        select * from sized where sized.rn <= 500 and sized.bytes_before < 4000000
    )
    select (select jsonb_agg(jsonb_build_object('entity', taken.entity, 'id', taken.id, 'row', taken.row, 'origin', taken.org)
                             order by taken.k) from taken),
           case when (select count(*) from shown) > (select count(*) from taken) then (select max(taken.k) from taken) end
    into v_rows, v_next;
    return res || jsonb_build_object('rows', coalesce(v_rows, '[]'), 'cursor', v_next);
end $$;

-- ---------------------------------------------------------------------------------------------
-- The search path of every function of Iamb (finding 16). An empty search path still searches the
-- caller's temporary schema first for tables and types; `pg_catalog, pg_temp` puts it last, where
-- nothing a caller made there can stand in for a built-in name. Every other name in the functions
-- is written with its schema. This runs last: an earlier migration applied again sets the old
-- value, and this sets it back.
-- ---------------------------------------------------------------------------------------------

do $$
declare
    f record;
begin
    for f in select p.oid::regprocedure as sig
             from pg_proc p join pg_namespace s on s.oid = p.pronamespace
             where s.nspname in ('iamb', 'iamb_private', 'api', 'api_service') and p.prokind in ('f', 'p')
               and not exists (select 1 from unnest(coalesce(p.proconfig, '{}')) c where c = 'search_path=pg_catalog, pg_temp') loop
        execute format('alter function %s set search_path = pg_catalog, pg_temp', f.sig);
    end loop;
end
$$;

-- ---------------------------------------------------------------------------------------------
-- Final grants (§5.1, G4). Functions of `api`: `authenticated` only. Functions of `iamb_private`:
-- the tables' owner, except the gate helpers and the two reader helpers for `authenticated` and
-- the token hook for the authentication service. Functions of `api_service`: `service_role` only.
-- The scratch tables: the tables' owner only.
-- ---------------------------------------------------------------------------------------------

revoke execute on all functions in schema api from public, anon, authenticated, service_role;
revoke execute on all functions in schema iamb_private from public, anon, authenticated, service_role;
revoke execute on all functions in schema api_service from public, anon, authenticated, service_role;
grant execute on all functions in schema api to authenticated;
grant execute on function
    iamb_private.session_ok(),
    iamb_private.member_of(uuid),
    iamb_private.role_in(uuid),
    iamb_private.shares_group_with(uuid),
    iamb_private.own_address(),
    iamb_private.enter(uuid, integer, text),
    iamb_private.position(uuid, text, text, bigint),
    iamb_private.conflict_history(uuid),
    iamb_private.member_addresses(uuid)
to authenticated;
grant execute on function api_service.attachment_verified(uuid, text, bigint),
                          api_service.delete_account_data(uuid) to service_role;
revoke all on all tables in schema iamb_private from public, anon, authenticated, service_role;
