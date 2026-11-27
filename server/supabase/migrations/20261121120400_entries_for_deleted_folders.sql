-- Iamb server, migration 23: an access entry for a deleted folder that its reader may not read
-- gives nothing, also to a direct read of the tables (docs/reviews/4.3-final-attack-pass.md, O3).
--
-- A deleted folder keeps the access entries it would have if it were live (section 5.4), so that
-- its tombstone reaches those who could read it and so that it can come back. Since migration 22
-- such an entry gives no tombstone to a reader whose access began after the delete. The entry
-- itself was still readable by its user: read as `authenticated` by direct SQL, `folder_access`
-- named the ids of folders deleted before the reader's access began. No device can make that
-- read, because the REST interface exposes the functions of `api` and no table; the functions
-- filtered such folders out one by one. The protection belongs in the policy, where a function
-- written later cannot forget it. Looking for every other place that could name such a folder
-- found four that a device can reach:
--
--  - a share put on a deleted folder was applied (the share waited for the folder to come back),
--    and its row, which names the folder, was delivered to the members of the group by `pull` and
--    `snapshot`, with the owner's profile row; and an entry that gives nothing counted as a grant
--    in the profile, so a push that named the profile took its lock (as N3) without being able
--    to write there;
--  - a folder created and deleted in one request (a device makes a folder and deletes a folder
--    above it, offline) got, for the other readers of the tree, a `row` entry in their feed: null,
--    with the id of a folder they could never read;
--  - a member who left a group, or was removed, got `folder_revoked` for each of the group's
--    top-level folders, also for one deleted before they joined;
--  - `get_conflict` named, as the parent of a folder's version, a folder for which the caller
--    holds such an entry, and the first page of a snapshot after a restore named, among the
--    profiles the restore lost revisions of, one reached through such an entry alone.
--
-- Now:
--  - **the read policy of `folder_access`** shows an entry to its user only while it gives the
--    folder's row: the folder is live, or the user deleted it, or it was deleted after the entry
--    began. A policy on `folder_access` cannot read `folder` (whose own policy reads
--    `folder_access`), and a helper called for every entry made the reads two and a half times
--    slower, so the entry carries what the rule needs: `folder_deleted_rev` and
--    `folder_deleted_by`, empty while the folder is live. Two triggers keep them equal to the
--    folder's row, whichever function writes it; the tests compare them after every step. Every
--    other policy reads `folder_access` under that policy, so the same holds for what an entry
--    gives: a snippet, a link or a share of such a folder, and the profile it belongs to, are
--    not read through it;
--  - `readers` says the same for the functions that read for a caller, and so do the cascade
--    answer, the parent of a version in `get_conflict`, the profiles a snapshot names after a
--    restore, and the two questions a push asks before it locks (`has_grant`, `writes_in`);
--  - a row that a request created and that a user cannot read when the request ends gets no feed
--    entry for that user, and an entry that gave nothing ends without `folder_revoked`;
--  - a share of a deleted folder is `rejected: parent_gone`, as the model of the server in
--    `crates/iamb-sync` has had it since task 4.4: the folder's delete ended its shares (C6), and
--    access does not come back by itself when a folder does (C7).
--
-- What every legitimate reader gets is what it was: who deleted the folder, a reader whose access
-- began before the delete, and so every device that holds the folder under an access that still
-- stands. One thing is narrower than migration 22 left it: a reader who deleted a snippet, lost
-- the access, and came back after somebody else had deleted the snippet's folder read their own
-- old tombstone; now the folder is not there for them, and nothing in it is. No device holds such
-- a row (it left when the access ended). The permission helpers (`perm`, `can_edit`,
-- `can_manage`, `access_root`) still read every entry: a folder that is restored needs the
-- entries, and no write reaches into a deleted folder its writer may not read (migration 22).

do $$
begin
    if current_user <> 'postgres' then
        raise exception 'iamb migrations must run as postgres, not %', current_user;
    end if;
end
$$;

-- What the rule needs of the folder, on the entry: the revision of the folder's delete and who
-- deleted it; both empty while the folder is live.
alter table iamb.folder_access add column if not exists folder_deleted_rev bigint;
alter table iamb.folder_access add column if not exists folder_deleted_by uuid;

-- A new entry takes them from its folder.
create or replace function iamb_private.entry_takes_folder_state() returns trigger
language plpgsql set search_path = pg_catalog, pg_temp as $$
declare
    f record;
begin
    select t.deleted_at, t.rev, t.updated_by into f from iamb.folder t where t.id = new.folder_id;
    if found then
        new.folder_deleted_rev := case when f.deleted_at is not null then f.rev end;
        new.folder_deleted_by := case when f.deleted_at is not null then f.updated_by end;
    end if;
    return new;
end
$$;

-- A folder that is deleted or comes back tells its entries.
create or replace function iamb_private.folder_state_to_entries() returns trigger
language plpgsql set search_path = pg_catalog, pg_temp as $$
begin
    update iamb.folder_access
    set folder_deleted_rev = case when new.deleted_at is not null then new.rev end,
        folder_deleted_by = case when new.deleted_at is not null then new.updated_by end
    where folder_id = new.id;
    return null;
end
$$;

drop trigger if exists entry_takes_folder_state on iamb.folder_access;
create trigger entry_takes_folder_state before insert on iamb.folder_access
    for each row execute function iamb_private.entry_takes_folder_state();

drop trigger if exists folder_state_to_entries on iamb.folder;
create trigger folder_state_to_entries after update of deleted_at, rev, updated_by on iamb.folder
    for each row
    when (old.deleted_at is distinct from new.deleted_at
          or (new.deleted_at is not null
              and (old.rev is distinct from new.rev or old.updated_by is distinct from new.updated_by)))
    execute function iamb_private.folder_state_to_entries();

-- The entries a database already holds.
update iamb.folder_access fa
set folder_deleted_rev = case when f.deleted_at is not null then f.rev end,
    folder_deleted_by = case when f.deleted_at is not null then f.updated_by end
from iamb.folder f
where f.id = fa.folder_id
  and (fa.folder_deleted_rev, fa.folder_deleted_by) is distinct from
      (case when f.deleted_at is not null then f.rev end, case when f.deleted_at is not null then f.updated_by end);

drop policy if exists folder_access_read on iamb.folder_access;
create policy folder_access_read on iamb.folder_access for select to authenticated using (
    (select iamb_private.session_ok()) and user_id = (select auth.uid())
    and (folder_deleted_rev is null or folder_deleted_rev > since_rev
         or folder_deleted_by = (select auth.uid()))
);

-- ---------------------------------------------------------------------------------------------
-- The same for the functions that read for a caller. Who can read a row now; see migration 22.
-- A row is read through an entry only when the entry gives its folder's row.
-- ---------------------------------------------------------------------------------------------

create or replace function iamb_private.readers(p_entity text, p_id uuid) returns uuid[]
language plpgsql stable set search_path = pg_catalog, pg_temp as $$
declare
    res uuid[];
begin
    case p_entity
    when 'folder' then
        select array_agg(fa.user_id) into res
        from iamb.folder f join iamb.folder_access fa on fa.folder_id = f.id
        where f.id = p_id and (f.deleted_at is null or f.rev > fa.since_rev or f.updated_by = fa.user_id);
    when 'snippet' then
        select array_agg(fa.user_id) into res
        from iamb.snippet s join iamb.folder_access fa on fa.folder_id = s.folder_id
        where s.id = p_id and (s.deleted_at is null or s.rev > fa.since_rev or s.updated_by = fa.user_id)
          and (fa.folder_deleted_rev is null or fa.folder_deleted_rev > fa.since_rev or fa.folder_deleted_by = fa.user_id);
    when 'snippet_tag' then
        select array_agg(fa.user_id) into res
        from iamb.snippet_tag l join iamb.snippet s on s.id = l.snippet_id
        join iamb.folder_access fa on fa.folder_id = s.folder_id
        where l.id = p_id and (s.deleted_at is null or s.rev > fa.since_rev or s.updated_by = fa.user_id)
          and (l.deleted_at is null or l.rev > fa.since_rev or l.updated_by = fa.user_id)
          and (fa.folder_deleted_rev is null or fa.folder_deleted_rev > fa.since_rev or fa.folder_deleted_by = fa.user_id);
    when 'snippet_attachment' then
        select array_agg(fa.user_id) into res
        from iamb.snippet_attachment l join iamb.snippet s on s.id = l.snippet_id
        join iamb.folder_access fa on fa.folder_id = s.folder_id
        where l.id = p_id and (s.deleted_at is null or s.rev > fa.since_rev or s.updated_by = fa.user_id)
          and (l.deleted_at is null or l.rev > fa.since_rev or l.updated_by = fa.user_id)
          and (fa.folder_deleted_rev is null or fa.folder_deleted_rev > fa.since_rev or fa.folder_deleted_by = fa.user_id);
    when 'folder_share' then
        select array_agg(distinct fa.user_id) into res
        from iamb.folder_share sh join iamb.folder_access fa on fa.folder_id = sh.folder_id
        where sh.id = p_id
          and (fa.folder_deleted_rev is null or fa.folder_deleted_rev > fa.since_rev or fa.folder_deleted_by = fa.user_id)
          and (sh.deleted_at is null or sh.rev > fa.since_rev or sh.updated_by = fa.user_id)
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
              and (t.deleted_at is null or m.created_rev < t.rev)
            union all
            select fa.user_id from iamb.tag t join iamb.snippet_tag l on l.tag_id = t.id
            join iamb.snippet s on s.id = l.snippet_id
            join iamb.folder_access fa on fa.folder_id = s.folder_id
            where t.id = p_id and t.deleted_at is null and l.deleted_at is null and s.deleted_at is null
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
              and (t.deleted_at is null or m.created_rev < t.rev)
            union all
            select fa.user_id from iamb.attachment t join iamb.snippet_attachment l on l.attachment_id = t.id
            join iamb.snippet s on s.id = l.snippet_id
            join iamb.folder_access fa on fa.folder_id = s.folder_id
            where t.id = p_id and t.deleted_at is null and l.deleted_at is null and s.deleted_at is null
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
            select fa.user_id from iamb.folder_access fa
            where fa.profile_id = p_id and (fa.folder_deleted_rev is null or fa.folder_deleted_rev > fa.since_rev or fa.folder_deleted_by = fa.user_id)
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

-- The answer for every row of a folder delete's cascade list (C6); see migrations 20 and 22. A
-- snippet in a deleted folder the caller may not read is `gone`.
create or replace function iamb_private.cascade_answer(u uuid, p_list jsonb) returns jsonb
language plpgsql volatile set search_path = pg_catalog, pg_temp as $$
declare
    budget bigint := coalesce(nullif(current_setting('iamb.cascade_bytes_left', true), '')::bigint, 4000000);
    res    jsonb;
    used   bigint;
begin
    with items as materialized (
        select x.i, x.v ->> 'entity' as ent, x.v -> 'id' as idj, iamb_private.try_uuid(x.v ->> 'id') as rid,
               case when jsonb_typeof(x.v -> 'base_rev') = 'number' and length(x.v ->> 'base_rev') <= 20
                    then (x.v ->> 'base_rev')::numeric end as base
        from jsonb_array_elements(case when jsonb_typeof(p_list) = 'array' then p_list else '[]'::jsonb end)
             with ordinality as x(v, i)
    ),
    found as materialized (
        select it.i, it.ent, it.idj, it.rid, it.base,
               coalesce(f.rev, s.rev) as rev, coalesce(f.deleted_at, s.deleted_at) as del,
               (f.id is not null or s.id is not null) as readable,
               case when s.id is not null
                        then 700 + octet_length(s.body) + octet_length(s.label) + octet_length(s.abbreviation)
                    when f.id is not null then 600 + octet_length(f.name) end as sz
        from items it
        left join iamb.folder f on it.ent = 'folder' and f.id = it.rid
             and exists (select 1 from iamb.folder_access fa where fa.user_id = u and fa.folder_id = f.id
                           and (f.deleted_at is null or f.rev > fa.since_rev or f.updated_by = u))
        left join iamb.snippet s on it.ent = 'snippet' and s.id = it.rid
             and exists (select 1 from iamb.folder_access fa
                         where fa.user_id = u and fa.folder_id = s.folder_id
                           and (s.deleted_at is null or s.rev > fa.since_rev or s.updated_by = u)
                           and (fa.folder_deleted_rev is null or fa.folder_deleted_rev > fa.since_rev
                                or fa.folder_deleted_by = u))
    ),
    -- The first entry of each row that is live and differs from the base the device named.
    cand as (
        select c.i, c.ent, c.rid, sum(c.sz) over (order by c.i) as est
        from (select fd.i, fd.ent, fd.rid, fd.sz,
                     row_number() over (partition by fd.ent, fd.rid order by fd.i) as nth
              from found fd
              where fd.readable and fd.del is null and fd.rev::numeric is distinct from fd.base) c
        where c.nth = 1
    ),
    -- Only now is content read, and only of rows whose stored sizes fit the allowance; the exact
    -- sizes then decide.
    whole as materialized (
        select c.i, w.row, w.origin, sum(octet_length(w.row::text) + 300) over (order by c.i) as bytes
        from cand c join iamb.wire_rows w on w.entity = c.ent and w.id = c.rid
        where c.est <= budget
    )
    select coalesce(jsonb_agg(
        case when not fd.readable then jsonb_build_object('entity', fd.ent, 'id', fd.idj, 'gone', true)
             when wh.row is not null and wh.bytes <= budget
                 then jsonb_build_object('entity', fd.ent, 'id', fd.idj, 'rev', fd.rev, 'deleted', false,
                                         'row', wh.row, 'origin', wh.origin)
             when fd.del is null and fd.rev::numeric is distinct from fd.base
                 then jsonb_build_object('entity', fd.ent, 'id', fd.idj, 'rev', fd.rev, 'deleted', false, 'fetch', true)
             else jsonb_build_object('entity', fd.ent, 'id', fd.idj, 'rev', fd.rev, 'deleted', fd.del is not null)
        end order by fd.i), '[]'::jsonb),
        coalesce(max(wh.bytes) filter (where wh.bytes <= budget), 0)
    into res, used
    from found fd left join whole wh on wh.i = fd.i;
    perform set_config('iamb.cascade_bytes_left', greatest(budget - used, 0)::text, true);
    return res;
end
$$;

-- The history rows of the versions a conflict offers to the caller (G5); see migration 13. The
-- parent of a folder's version is left out when the caller cannot read it, and a deleted folder
-- the caller may not read is such a parent.
create or replace function iamb_private.conflict_history(p_conflict uuid)
returns table (rev bigint, written_at bigint, deleted boolean, "row" jsonb, origin jsonb)
language sql stable security definer set search_path = pg_catalog, pg_temp as $$
    select h.rev, h.written_at, h.deleted,
           case when c.entity = 'folder' and h.row ->> 'parent_id' is not null
                     and not exists (select 1 from iamb.folder_access fa
                                     where fa.user_id = auth.uid()
                                       and fa.folder_id = iamb_private.try_uuid(h.row ->> 'parent_id')
                                       and (fa.folder_deleted_rev is null or fa.folder_deleted_rev > fa.since_rev
                                            or fa.folder_deleted_by = fa.user_id))
                then iamb_private.wire_of(c.entity, h.row) || jsonb_build_object('parent_id', null)
                else iamb_private.wire_of(c.entity, h.row) end,
           case when c.entity = 'snippet' then
               jsonb_build_object('content_by', h.row -> 'content_by', 'content_at', h.row -> 'content_at',
                                  'created_rev', h.row -> 'created_rev') end
    from iamb.conflict c
    join iamb.conflict_version v on v.conflict_id = c.id
    join iamb.history h on h.entity = c.entity and h.row_id = c.row_id and h.rev = v.rev
    where c.id = p_conflict
      and iamb_private.session_ok()
      and auth.uid() = any (c.parties)
      and iamb_private.can_read(auth.uid(), c.entity, c.row_id)
      and iamb_private.version_readable(auth.uid(), c.entity, h.row, h.replaced_by_rev)
$$;

-- Whether the user has any grant in the profile: its owner (a personal profile), a live member (a
-- live group), or an access entry for one of its folders that gives that folder. A request locks
-- no other profile.
create or replace function iamb_private.has_grant(u uuid, p_profile uuid) returns boolean
language sql stable set search_path = pg_catalog, pg_temp as $$
    select exists (select 1 from iamb.profile p where p.id = p_profile and p.kind = 'personal' and p.owner_user_id = u)
        or exists (select 1 from iamb.membership m join iamb.profile g on g.id = m.profile_id
                   where m.profile_id = p_profile and m.user_id = u and m.deleted_at is null
                     and g.deleted_at is null and g.kind <> 'personal')
        or exists (select 1 from iamb.folder_access fa
                   where fa.user_id = u and fa.profile_id = p_profile
                     and (fa.folder_deleted_rev is null or fa.folder_deleted_rev > fa.since_rev or fa.folder_deleted_by = fa.user_id))
$$;

-- Whether the user can write anything in the profile: they manage it (the owner of a personal
-- profile, owner or admin of a group), or have `edit` or `manage` on one of its folders through
-- an entry that gives that folder.
create or replace function iamb_private.writes_in(u uuid, p_profile uuid) returns boolean
language sql stable set search_path = pg_catalog, pg_temp as $$
    select iamb_private.manages_profile(u, p_profile)
        or exists (select 1 from iamb.folder_access fa
                   where fa.user_id = u and fa.profile_id = p_profile and fa.permission in ('edit', 'manage')
                     and (fa.folder_deleted_rev is null or fa.folder_deleted_rev > fa.since_rev or fa.folder_deleted_by = fa.user_id))
$$;

-- One op of a push, or the restore of a conflict's version (C1 to C21); see migrations 12, 20 and
-- 22. Since then: a share of a deleted folder is `rejected: parent_gone`.
create or replace function iamb_private.push_op(u uuid, dev uuid, op jsonb, p_profile uuid, p_budget integer,
                                                p_blocked uuid[] default '{}', p_origin jsonb default null)
returns jsonb
language plpgsql volatile set search_path = pg_catalog, pg_temp as $$
declare
    ent      text := op ->> 'entity';
    rid      uuid := iamb_private.try_uuid(op ->> 'id');
    v_kind   text := op ->> 'kind';
    opid     uuid := iamb_private.try_uuid(op ->> 'op_id');
    base     bigint := coalesce((op ->> 'base_rev')::numeric::bigint, 0);
    follows  uuid[] := array(select iamb_private.try_uuid(x) from jsonb_array_elements_text(coalesce(op -> 'follows', '[]')) x);
    repush   text := op ->> 'repush';
    rp_rev   bigint := (op ->> 'repush_rev')::numeric::bigint;
    rp_ep    uuid := iamb_private.try_uuid(op ->> 'repush_epoch');
    self     boolean := coalesce(op ->> 'author', '') = 'self';
    cur      jsonb := iamb_private.get_row(ent, rid);
    n        jsonb;
    nr       jsonb;
    p        uuid := p_profile;
    st       iamb.server_state;
    now_ms   bigint := iamb_private.now_ms();
    r        bigint;
    before   uuid[];
    cid      uuid;
    adjusted text[] := '{}';
    stale    boolean;
    own      boolean;
    bv       jsonb;
    changed  boolean;
    ckind    text;
    honoured boolean := false;
    ep       iamb.epoch;
    backup   bigint;
    target   uuid;
    parent   iamb.folder;
    tf       iamb.folder;
    cf       iamb.folder;
    d        integer;
    lim      bigint;
    cnt      bigint;
    lockp    uuid[];
    structural boolean := false;
    sn       iamb.snippet;
    other    jsonb;
    blob     text;
    del      boolean := (op ->> 'kind') = 'delete';
    res      jsonb;
begin
    select * into st from iamb.server_state;
    if ent in ('profile', 'membership') then
        return iamb_private.op_result(u, op, 'rejected', 'not_pushable', null, null, false);
    end if;
    if rid is null or opid is null or v_kind not in ('put', 'delete') or iamb_private.fields(ent) is null then
        return iamb_private.op_result(u, op, 'blocked', 'invalid', null, null, false);
    end if;
    if p is null then
        -- No profile, or one the caller has no grant in (it was not locked): the answer of an
        -- unknown row, whatever the server holds under that id.
        if del then
            res := iamb_private.op_result(u, op, 'unchanged', null, null, null, false);
            if ent = 'folder' then
                res := res || jsonb_build_object('cascade', iamb_private.cascade_answer(u, op -> 'cascade'));
            end if;
            return res;
        end if;
        return iamb_private.op_result(u, op, 'rejected', 'parent_gone', null, null, false);
    end if;
    lockp := array[p];
    if not exists (select 1 from iamb.profile where id = p) then
        return iamb_private.op_result(u, op, 'rejected', 'parent_gone', null, null, false);
    end if;

    -- A re-push after a restore (C18) is honoured only for a revision in the span the restore
    -- lost; outside it the op is an ordinary write based on that revision.
    if repush in ('row', 'base') and rp_rev is not null then
        select e2.* into ep from iamb.epoch e1 join iamb.epoch e2 on e2.started_at > e1.started_at
        where e1.id = rp_ep order by e2.started_at limit 1;
        if ep.id is null and not exists (select 1 from iamb.epoch where id = rp_ep) then
            select e.* into ep from iamb.epoch e join iamb.server_state s on s.epoch = e.id;
        end if;
        if ep.id is not null then
            select backup_rev into backup from iamb.epoch_profile where epoch_id = ep.id and profile_id = p;
            backup := coalesce(backup, 0);
            honoured := rp_rev > backup and rp_rev < ep.first_rev;
        end if;
        if not honoured then
            base := rp_rev;
        end if;
    end if;

    -- ----------------------------------------------------------------------------- deletes
    if del then
        if ent = 'folder' then
            return iamb_private.push_folder_delete(u, dev, op, p_budget);
        end if;
        if cur is null or not iamb_private.can_read(u, ent, rid) then
            return iamb_private.op_result(u, op, 'unchanged', null, null, null, false);
        end if;
        -- Permission (§4.3).
        if (ent = 'snippet' and not iamb_private.can_edit(u, (cur ->> 'folder_id')::uuid))
           or (ent in ('snippet_tag', 'snippet_attachment') and not iamb_private.can_edit(u,
                   (select s.folder_id from iamb.snippet s where s.id = (cur ->> 'snippet_id')::uuid)))
           or (ent in ('tag', 'attachment') and not iamb_private.manages_profile(u, p))
           or (ent = 'folder_share' and not iamb_private.can_manage(u, (cur ->> 'folder_id')::uuid)) then
            return iamb_private.op_result(u, op, 'rejected', 'forbidden', null, null, false);
        end if;
        if (cur ->> 'deleted_at') is not null then
            return iamb_private.op_result(u, op, 'unchanged', null, null, null, false);
        end if;
        if ent = 'folder_share' then
            -- A share's delete always applies (C11); it is logged for the restore script.
            r := iamb_private.end_share(u, dev, opid, rid, lockp, 'push');
            perform iamb_private.refresh_access(lockp);
            return iamb_private.op_result(u, op, 'applied', null, null, null, false);
        end if;
        before := iamb_private.readers(ent, rid);
        r := iamb_private.next_rev(lockp);
        nr := cur || jsonb_build_object('deleted_at', now_ms, 'rev', r, 'updated_at', now_ms, 'updated_by', u,
                                        'updated_device', dev, 'updated_op', opid,
                                        'lost_rev', case when honoured then greatest(coalesce((cur ->> 'lost_rev')::bigint, 0), rp_rev)
                                                         else (cur ->> 'lost_rev')::bigint end);
        perform iamb_private.write_row(u, dev, opid, ent, cur, nr, r, before, false, p);
        if ent = 'snippet' then
            perform iamb_private.touch_attachments(array(select attachment_id from iamb.snippet_attachment where snippet_id = rid));
        elsif ent = 'snippet_attachment' then
            perform iamb_private.touch_attachments(array[(cur ->> 'attachment_id')::uuid]);
        end if;
        -- A stale delete records `delete_over_edit` (C3, C5).
        if ent in ('snippet', 'tag') and base < (cur ->> 'rev')::bigint
           and not (iamb_private.try_uuid(cur ->> 'updated_device') = dev
                    and iamb_private.try_uuid(cur ->> 'updated_op') = any (follows)) then
            bv := iamb_private.base_version(ent, cur, base, follows, u, dev);
            if bv is null or iamb_private.pick(bv, iamb_private.recorded_fields(ent))
                             <> iamb_private.pick(cur, iamb_private.recorded_fields(ent))
               or (bv ->> 'deleted_at' is null) <> (cur ->> 'deleted_at' is null) then
                cid := iamb_private.record_conflict(ent, rid, p, 'delete_over_edit', r, u, dev, cur);
            end if;
        end if;
        return iamb_private.op_result(u, op, 'applied', null, cid, null, false);
    end if;

    -- ----------------------------------------------------------------------------- puts
    -- A tombstone the writer may not read is, for the writer, a row that is gone (C13): a put
    -- does not bring it back. (Restored, it would bring back its live links, and with them tags
    -- and files the writer never had.) A share is judged by its own rule (C11).
    if cur is not null and (cur ->> 'deleted_at') is not null and ent <> 'folder_share'
       and not iamb_private.can_read(u, ent, rid) then
        return iamb_private.op_result(u, op, 'rejected', 'gone', null, null, false);
    end if;
    n := iamb_private.normalize(ent, op -> 'row');
    if n ? '_error' then
        return iamb_private.op_result(u, op, 'blocked', n ->> '_error', null, null, false);
    end if;
    -- A row whose parent was blocked in the same request (C14).
    if exists (select 1 from unnest(p_blocked) as b(id)
               where b.id in (iamb_private.try_uuid(n ->> 'parent_id'), iamb_private.try_uuid(n ->> 'folder_id'),
                              iamb_private.try_uuid(n ->> 'snippet_id'), iamb_private.try_uuid(n ->> 'tag_id'),
                              iamb_private.try_uuid(n ->> 'attachment_id'))) then
        return iamb_private.op_result(u, op, 'blocked', 'parent_blocked', null, null, false);
    end if;

    -- A row whose profile would change is blocked (C8); its other rows must be known and readable
    -- (C13). Then permission (§4.3, C12) against the state found under the locks.
    case ent
    when 'folder' then
        if (n ->> 'profile_id')::uuid <> p then
            return iamb_private.op_result(u, op, 'blocked', 'invalid', null, null, false);
        end if;
        target := iamb_private.try_uuid(n ->> 'parent_id');
        if target is not null then
            select * into parent from iamb.folder where id = target;
            if parent.id is null or iamb_private.perm(u, target) is null then
                return iamb_private.op_result(u, op, 'rejected', 'parent_gone', null, null, false);
            elsif parent.profile_id <> p then
                return iamb_private.op_result(u, op, 'blocked', 'invalid', null, null, false);
            end if;
        end if;
        if cur is null then
            if (target is null and not iamb_private.manages_profile(u, p))
               or (target is not null and not iamb_private.can_edit(u, target)) then
                return iamb_private.op_result(u, op, 'rejected', 'forbidden', null, null, false);
            end if;
            -- C8: a new folder that would be a fifth level goes under the nearest folder above.
            if target is not null then
                d := iamb_private.folder_depth(target);
                while d >= 4 loop
                    select f.* into parent from iamb.folder f where f.id = parent.parent_id;
                    target := parent.id;
                    d := d - 1;
                end loop;
                if target is distinct from iamb_private.try_uuid(n ->> 'parent_id') then
                    adjusted := array['parent_id'];
                    n := n || jsonb_build_object('parent_id', target);
                    if target is not null and not iamb_private.can_edit(u, target) then
                        return iamb_private.op_result(u, op, 'rejected', 'forbidden', null, null, false);
                    end if;
                end if;
            end if;
        else
            if (cur ->> 'parent_id' is null) or iamb_private.is_share_root(rid) or target is null
               or (n -> 'member_permission') is distinct from (cur -> 'member_permission') then
                -- A top-level folder, the root of a live share, a move to the top, or the members'
                -- permission: `manage` for every field.
                if not iamb_private.can_manage(u, rid)
                   or (target is null and not iamb_private.manages_profile(u, p))
                   or (target is not null and target is distinct from iamb_private.try_uuid(cur ->> 'parent_id')
                       and not iamb_private.can_edit(u, target)) then
                    return iamb_private.op_result(u, op, 'rejected', 'forbidden', null, null, false);
                end if;
            else
                if not iamb_private.can_edit(u, rid)
                   or (target is distinct from iamb_private.try_uuid(cur ->> 'parent_id')
                       and (not iamb_private.can_edit(u, target)
                            or (iamb_private.access_root(u, target) is distinct from iamb_private.access_root(u, rid)
                                and not iamb_private.can_manage(u, rid)))) then
                    return iamb_private.op_result(u, op, 'rejected', 'forbidden', null, null, false);
                end if;
            end if;
            -- C8: a move that would make a cycle or a fifth level keeps the current parent.
            if target is not null and target is distinct from iamb_private.try_uuid(cur ->> 'parent_id') then
                if iamb_private.is_in_subtree(target, rid)
                   or iamb_private.folder_depth(target) + iamb_private.subtree_height(rid) > 4 then
                    adjusted := array['parent_id'];
                    n := n || jsonb_build_object('parent_id', cur -> 'parent_id');
                    target := iamb_private.try_uuid(cur ->> 'parent_id');
                end if;
            end if;
            -- C8 for a folder that comes back (N4): a deleted folder can lie deeper than four
            -- levels, after a folder above it was moved; nothing live is below it. Restored, it
            -- goes under the nearest folder above that keeps four levels, as a new folder does.
            if (cur ->> 'deleted_at') is not null and target is not null then
                d := iamb_private.folder_depth(target);
                if d >= 4 then
                    select f.* into parent from iamb.folder f where f.id = target;
                    while d >= 4 loop
                        select f.* into parent from iamb.folder f where f.id = parent.parent_id;
                        target := parent.id;
                        d := d - 1;
                    end loop;
                    adjusted := array['parent_id'];
                    n := n || jsonb_build_object('parent_id', target);
                    if not iamb_private.can_edit(u, target) then
                        return iamb_private.op_result(u, op, 'rejected', 'forbidden', null, null, false);
                    end if;
                end if;
            end if;
        end if;
        structural := cur is null or (n -> 'parent_id') is distinct from (cur -> 'parent_id')
                      or (n -> 'member_permission') is distinct from (cur -> 'member_permission')
                      or (cur ->> 'deleted_at') is not null;
        -- C7: a live folder needs live folders above it.
        if target is not null and exists (select 1 from iamb.folder where id = target and deleted_at is not null) then
            if not iamb_private.restore_chain(u, dev, opid, target) then
                return iamb_private.op_result(u, op, 'rejected', 'parent_gone', null, null, false);
            end if;
        end if;
        if cur is null then
            lim := iamb_private.profile_limit(p, 'folders_max');
            select count(*) into cnt from iamb.folder where profile_id = p and deleted_at is null;
            if lim is not null and cnt >= lim then
                return iamb_private.op_result(u, op, 'blocked', 'plan_limit', null, null, false);
            end if;
        end if;
    when 'snippet' then
        target := (n ->> 'folder_id')::uuid;
        select * into tf from iamb.folder where id = target;
        if tf.id is null or iamb_private.perm(u, target) is null then
            return iamb_private.op_result(u, op, 'rejected', 'parent_gone', null, null, false);
        elsif tf.profile_id <> p then
            return iamb_private.op_result(u, op, 'blocked', 'invalid', null, null, false);
        end if;
        if cur is null then
            if not iamb_private.can_edit(u, target) then
                return iamb_private.op_result(u, op, 'rejected', 'forbidden', null, null, false);
            end if;
        else
            if not iamb_private.can_edit(u, (cur ->> 'folder_id')::uuid) or not iamb_private.can_edit(u, target)
               or (target <> (cur ->> 'folder_id')::uuid
                   and iamb_private.access_root(u, target) is distinct from iamb_private.access_root(u, (cur ->> 'folder_id')::uuid)
                   and not (iamb_private.can_manage(u, target) and iamb_private.can_manage(u, (cur ->> 'folder_id')::uuid))) then
                return iamb_private.op_result(u, op, 'rejected', 'forbidden', null, null, false);
            end if;
        end if;
        if tf.deleted_at is not null then
            if not iamb_private.restore_chain(u, dev, opid, target) then
                return iamb_private.op_result(u, op, 'rejected', 'parent_gone', null, null, false);
            end if;
        end if;
        if cur is null then
            lim := iamb_private.profile_limit(p, 'snippets_max');
            select count(*) into cnt from iamb.snippet where profile_id = p and deleted_at is null;
            if lim is not null and cnt >= lim then
                return iamb_private.op_result(u, op, 'blocked', 'plan_limit', null, null, false);
            end if;
        end if;
    when 'tag', 'attachment' then
        if (n ->> 'profile_id')::uuid <> p then
            return iamb_private.op_result(u, op, 'blocked', 'invalid', null, null, false);
        end if;
        if not exists (select 1 from iamb.profile where id = p and deleted_at is null) then
            return iamb_private.op_result(u, op, 'rejected', 'parent_gone', null, null, false);
        end if;
        if ent = 'attachment' and cur is not null
           and (iamb_private.pick(n, iamb_private.fields(ent)) <> iamb_private.pick(cur, iamb_private.fields(ent))
                or (cur ->> 'deleted_at') is null) then
            -- An attachment row never changes after its creation (C19).
            res := iamb_private.op_result(u, op, 'unchanged', null, null, null, false);
            if not iamb_private.can_read_blob(u, p, cur ->> 'sha256') then
                res := res || jsonb_build_object('blob', 'missing');
            end if;
            return res;
        end if;
        -- Create: `edit` on a folder of the profile; change, delete or restore: `manage` (§4.3).
        if (cur is null and not iamb_private.edits_in_profile(u, p))
           or (cur is not null and not iamb_private.manages_profile(u, p)) then
            return iamb_private.op_result(u, op, 'rejected', 'forbidden', null, null, false);
        end if;
    when 'snippet_tag', 'snippet_attachment' then
        select * into sn from iamb.snippet where id = (n ->> 'snippet_id')::uuid;
        other := case ent when 'snippet_tag' then iamb_private.get_row('tag', (n ->> 'tag_id')::uuid)
                          else iamb_private.get_row('attachment', (n ->> 'attachment_id')::uuid) end;
        if sn.id is null or other is null or not iamb_private.can_read(u, 'snippet', sn.id)
           or not iamb_private.can_read(u, case ent when 'snippet_tag' then 'tag' else 'attachment' end,
                                        (other ->> 'id')::uuid) then
            return iamb_private.op_result(u, op, 'rejected', 'parent_gone', null, null, false);
        end if;
        if (other ->> 'profile_id')::uuid <> sn.profile_id or sn.profile_id <> p
           or (cur is not null and iamb_private.pick(n, iamb_private.fields(ent)) <> iamb_private.pick(cur, iamb_private.fields(ent))) then
            return iamb_private.op_result(u, op, 'blocked', 'invalid', null, null, false);
        end if;
        if not iamb_private.can_edit(u, sn.folder_id) then
            return iamb_private.op_result(u, op, 'rejected', 'forbidden', null, null, false);
        end if;
    when 'folder_share' then
        -- C11: one row per folder and group; `manage` on a personal folder, a live membership in
        -- the group and, in a team, the role of owner or admin; a stale put is not applied.
        if rid <> iamb_private.share_id((n ->> 'folder_id')::uuid, (n ->> 'profile_id')::uuid) then
            return iamb_private.op_result(u, op, 'blocked', 'invalid', null, null, false);
        end if;
        select * into cf from iamb.folder where id = (n ->> 'folder_id')::uuid;
        if cf.id is null or iamb_private.perm(u, cf.id) is null
           or not exists (select 1 from iamb.profile g where g.id = (n ->> 'profile_id')::uuid
                          and g.kind <> 'personal' and g.deleted_at is null)
           or iamb_private.group_role(u, (n ->> 'profile_id')::uuid) is null then
            return iamb_private.op_result(u, op, 'rejected', 'parent_gone', null, null, false);
        end if;
        if not iamb_private.can_manage(u, cf.id)
           or not exists (select 1 from iamb.profile pp where pp.id = cf.profile_id and pp.kind = 'personal')
           or ((select kind from iamb.profile where id = (n ->> 'profile_id')::uuid) = 'company'
               and iamb_private.group_role(u, (n ->> 'profile_id')::uuid) not in ('owner', 'admin')) then
            return iamb_private.op_result(u, op, 'rejected', 'forbidden', null, null, false);
        end if;
        -- A share of a deleted folder is refused: the delete ended the folder's shares (C6), and
        -- access does not come back by itself when the folder does (C7). A share that waited on
        -- a deleted folder named it to the members of the group (O3).
        if cf.deleted_at is not null then
            return iamb_private.op_result(u, op, 'rejected', 'parent_gone', null, null, false);
        end if;
        if cur is null and base > 0 and not honoured then
            return iamb_private.op_result(u, op, 'rejected', 'gone', null, null, false);
        end if;
        if cur is not null and base < (cur ->> 'rev')::bigint and not honoured then
            return iamb_private.op_result(u, op, 'superseded', null, null, null, false);
        end if;
        if not exists (select 1 from iamb.folder_share s where s.folder_id = cf.id and s.deleted_at is null) then
            lim := iamb_private.user_limit(u, 'shared_folders_max');
            select count(distinct s.folder_id) into cnt from iamb.folder_share s join iamb.folder f on f.id = s.folder_id
            where f.profile_id = cf.profile_id and s.deleted_at is null;
            if lim is not null and cnt >= lim then
                return iamb_private.op_result(u, op, 'rejected', 'plan_limit', null, null, false);
            end if;
        end if;
        lockp := array[(n ->> 'profile_id')::uuid, p];
        structural := true;
    end case;

    -- C13: a put with a base above 0 for a row the server does not know is gone, unless it gives
    -- back a revision a restore lost (C18 case 4, within `retention_days` of the epoch).
    if cur is null and base > 0 and not honoured then
        return iamb_private.op_result(u, op, 'rejected', 'gone', null, null, false);
    end if;
    if cur is null and honoured and ep.started_at < now_ms - st.retention_days * 86400000::bigint then
        return iamb_private.op_result(u, op, 'rejected', 'gone', null, null, false);
    end if;

    -- C4: nothing new, nothing written.
    if cur is not null and (cur ->> 'deleted_at') is null
       and iamb_private.pick(n, iamb_private.fields(ent)) = iamb_private.pick(cur, iamb_private.fields(ent)) then
        if honoured and coalesce((cur ->> 'lost_rev')::bigint, 0) < rp_rev and (cur ->> 'rev')::bigint <= backup then
            update iamb.folder set lost_rev = rp_rev where ent = 'folder' and id = rid;
            update iamb.snippet set lost_rev = rp_rev where ent = 'snippet' and id = rid;
            update iamb.tag set lost_rev = rp_rev where ent = 'tag' and id = rid;
        end if;
        res := iamb_private.op_result(u, op, 'unchanged', null, null, adjusted, false);
        if ent = 'attachment' and not iamb_private.can_read_blob(u, p, cur ->> 'sha256') then
            res := res || jsonb_build_object('blob', 'missing');
        end if;
        return res;
    end if;

    -- C18 case 3: the row has taken in a lost revision at or above this one.
    if honoured and cur is not null and coalesce((cur ->> 'lost_rev')::bigint, 0) >= rp_rev then
        nr := cur || n || jsonb_build_object('rev', rp_rev, 'updated_by', case when self then u end,
                                             'updated_device', case when self then dev end, 'updated_op', opid,
                                             'deleted_at', null);
        perform iamb_private.keep_history(ent, nr, (cur ->> 'rev')::bigint);
        if (cur ->> 'lost_rev')::bigint > rp_rev
           and iamb_private.pick(n, iamb_private.recorded_fields(ent)) <> iamb_private.pick(cur, iamb_private.recorded_fields(ent))
           and ent in ('folder', 'snippet', 'tag') then
            cid := iamb_private.record_conflict(ent, rid, p, 'edit_edit', (cur ->> 'rev')::bigint,
                                                iamb_private.try_uuid(cur ->> 'updated_by'),
                                                iamb_private.try_uuid(cur ->> 'updated_device'), nr);
        end if;
        return iamb_private.op_result(u, op, 'superseded', null, cid, null, false);
    end if;

    -- The write.
    before := case when cur is null then '{}'::uuid[] else iamb_private.readers(ent, rid) end;
    r := iamb_private.next_rev(lockp);
    if cur is null then
        nr := jsonb_build_object('id', rid, 'created_at', least(coalesce((n ->> 'created_at')::bigint, now_ms), now_ms),
                                 'created_rev', r);
        if ent = 'snippet' then
            nr := nr || jsonb_build_object('profile_id', p);
        end if;
    else
        nr := cur;
    end if;
    nr := nr || (n - 'created_at') || jsonb_build_object(
        'rev', r, 'updated_at', now_ms, 'updated_by', u, 'updated_device', dev, 'updated_op', opid,
        'deleted_at', null,
        'lost_rev', case when honoured then greatest(coalesce((cur ->> 'lost_rev')::bigint, 0), rp_rev)
                         else (cur ->> 'lost_rev')::bigint end);

    -- C17: who wrote the snippet's content.
    if ent = 'snippet' then
        if p_origin is not null then
            nr := nr || jsonb_build_object('content_by', p_origin -> 'content_by', 'content_at', p_origin -> 'content_at');
        elsif cur is not null and iamb_private.pick(n, iamb_private.content_fields()) = iamb_private.pick(cur, iamb_private.content_fields()) then
            nr := nr || jsonb_build_object('content_by', cur -> 'content_by', 'content_at', cur -> 'content_at');
        elsif honoured then
            nr := nr || jsonb_build_object('content_by', case when self then u end, 'content_at', now_ms);
        elsif cur is null then
            nr := nr || jsonb_build_object('content_by', u, 'content_at', now_ms);
        else
            bv := iamb_private.base_version(ent, cur, base, follows, u, dev);
            if bv is null then
                nr := nr || jsonb_build_object('content_by', null, 'content_at', now_ms);
            elsif iamb_private.pick(n, iamb_private.content_fields()) = iamb_private.pick(bv, iamb_private.content_fields()) then
                nr := nr || jsonb_build_object('content_by', bv -> 'content_by', 'content_at', bv -> 'content_at');
            else
                nr := nr || jsonb_build_object('content_by', u, 'content_at', now_ms);
            end if;
        end if;
    end if;
    -- C19: an attachment row is backed when its creator can read the verified file; a re-pushed
    -- row without `author: "self"` has no creator and is not backed.
    if ent = 'attachment' then
        if cur is null then
            if honoured and not self then
                nr := nr || jsonb_build_object('updated_by', null, 'updated_device', null, 'backed', false,
                                               'unlinked_at', now_ms);
            else
                nr := nr || jsonb_build_object('backed', iamb_private.can_read_blob(u, p, n ->> 'sha256'),
                                               'unlinked_at', now_ms);
            end if;
        end if;
    end if;
    perform iamb_private.write_row(u, dev, opid, ent, cur, nr, r, before, structural, p);
    if ent = 'folder_share' then
        -- Logged so that the restore script can tell a share that was given back (C18).
        perform iamb_private.log('share_created', jsonb_build_object(
            'user', u, 'device', dev, 'share', rid, 'folder', n ->> 'folder_id', 'group', n ->> 'profile_id', 'rev', r));
    end if;
    if ent = 'snippet_attachment' then
        perform iamb_private.touch_attachments(array[(n ->> 'attachment_id')::uuid]);
    elsif ent = 'snippet' and cur is not null and (cur ->> 'deleted_at') is not null then
        perform iamb_private.touch_attachments(array(select attachment_id from iamb.snippet_attachment where snippet_id = rid));
    end if;

    -- C3: a stale write still wins, on the record. C18 cases 1 and 2 decide the same way, except
    -- that a re-push over a row not written since the restore records nothing.
    if cur is not null and ent in ('folder', 'snippet', 'tag') then
        stale := base < (cur ->> 'rev')::bigint;
        own := iamb_private.try_uuid(cur ->> 'updated_device') = dev
               and iamb_private.try_uuid(cur ->> 'updated_op') = any (follows);
        if honoured then
            stale := (cur ->> 'rev')::bigint > backup;
            changed := iamb_private.pick(n, iamb_private.recorded_fields(ent)) <> iamb_private.pick(cur, iamb_private.recorded_fields(ent))
                       or (cur ->> 'deleted_at') is not null;
        elsif stale then
            bv := iamb_private.base_version(ent, cur, base, follows, u, dev);
            if bv is null then
                changed := iamb_private.pick(n, iamb_private.recorded_fields(ent)) <> iamb_private.pick(cur, iamb_private.recorded_fields(ent))
                           or (cur ->> 'deleted_at') is not null;
            else
                changed := iamb_private.pick(bv, iamb_private.recorded_fields(ent)) <> iamb_private.pick(cur, iamb_private.recorded_fields(ent))
                           or (bv ->> 'deleted_at' is null) <> (cur ->> 'deleted_at' is null);
            end if;
        end if;
        if stale and not own and changed then
            ckind := case when (cur ->> 'deleted_at') is not null then 'edit_over_delete' else 'edit_edit' end;
            cid := iamb_private.record_conflict(ent, rid, p, ckind, r, u, dev, cur);
        end if;
    end if;

    res := iamb_private.op_result(u, op, 'applied', null, cid, adjusted, false);
    if ent = 'attachment' and not iamb_private.can_read_blob(u, p, n ->> 'sha256') then
        res := res || jsonb_build_object('blob', 'missing');
    end if;
    return res;
end
$$;

-- Whether the presented position is one the server keeps for the device, and a new one (C25);
-- see migration 3. The profiles a snapshot names after a restore are those the caller can read.
create or replace function iamb_private.position(p_device uuid, p_presented text, p_call text,
                                                 p_new_seq bigint default null)
returns jsonb
language plpgsql volatile security definer set search_path = pg_catalog, pg_temp as $$
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
            from (select fa.profile_id as pid from iamb.folder_access fa
                  where fa.user_id = d.user_id and (fa.folder_deleted_rev is null or fa.folder_deleted_rev > fa.since_rev or fa.folder_deleted_by = fa.user_id)
                  union select p.id from iamb.profile p where p.owner_user_id = d.user_id and p.kind = 'personal'
                  union select m.profile_id from iamb.membership m where m.user_id = d.user_id and m.deleted_at is null) x
            left join iamb.epoch_profile epr on epr.epoch_id = ep.id and epr.profile_id = x.pid), '[]'::jsonb));
        return jsonb_build_object('error', null, 'position', newpos, 'epoch', p_ep, 'seq', p_seq, 'lost', lost);
    end if;
    return jsonb_build_object('error', null, 'position', newpos, 'epoch', p_ep, 'seq', p_seq);
end
$$;

-- Recomputes the access entries of the folders of the given profiles and writes the visibility
-- entries of the feed (C21); see migrations 11, 18 and 19. An entry that gave nothing ends without
-- `folder_revoked`, and is no part of a revoked tree that "stays readable".
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
        returning fa.user_id, fa.folder_id,
                  (fa.folder_deleted_rev is null or fa.folder_deleted_rev > fa.since_rev
                   or fa.folder_deleted_by = fa.user_id) as gave
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
    -- An entry that gave nothing (its folder was deleted before it began) ends unheard: its user
    -- never read the folder, and `folder_revoked` would name it to them.
    select (select coalesce(array_agg(g.user_id), '{}') from gone g where g.gave),
           (select coalesce(array_agg(g.folder_id), '{}') from gone g where g.gave),
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
                where exists (select 1 from iamb.folder_access n where n.user_id = r.user_id and n.folder_id = sub.id
                                and (n.folder_deleted_rev is null or n.folder_deleted_rev > n.since_rev
                                     or n.folder_deleted_by = n.user_id))
                  and not exists (select 1 from iamb.folder_access n where n.user_id = r.user_id and n.folder_id = sub.parent_id
                                    and (n.folder_deleted_rev is null or n.folder_deleted_rev > n.since_rev
                                         or n.folder_deleted_by = n.user_id))
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

-- Writes the collected entries last (section 5.5, C24 step 4); see migrations 11, 18 and 19.
-- Before that it drops the entries for rows the request created that their user cannot read.
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
    c_ent  text[];
    c_id   uuid[];
begin
    -- The request's last step: the access entries it made begin now (N1).
    perform iamb_private.access_begin();
    -- A row this request created that a user cannot read now that it ends was never theirs to
    -- read: a folder made and deleted, with a folder above it, in one push. No entry names it for
    -- them (O3). Their devices never held it, so nothing has to be taken off them.
    select array_agg(t.entity), array_agg(t.id) into c_ent, c_id
    from iamb_private.tx_rows t where t.tx = v_tx and t.created;
    if c_id is not null then
        delete from iamb_private.tx_feed p
        using (select x.e, x.id, iamb_private.readers(x.e, x.id) as rd from unnest(c_ent, c_id) as x(e, id)) c
        where p.tx = v_tx and not p.user_id = any (c.rd)
          and ((p.kind = 'row' and p.entity = c.e and p.row_id = c.id)
               or (p.kind in ('folder_visible', 'folder_revoked') and c.e = 'folder' and p.folder_id = c.id));
    end if;
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

revoke execute on function iamb_private.entry_takes_folder_state(), iamb_private.folder_state_to_entries(),
                           iamb_private.readers(text, uuid),
                           iamb_private.cascade_answer(uuid, jsonb),
                           iamb_private.has_grant(uuid, uuid),
                           iamb_private.writes_in(uuid, uuid),
                           iamb_private.push_op(uuid, uuid, jsonb, uuid, integer, uuid[], jsonb),
                           iamb_private.refresh_access(uuid[]),
                           iamb_private.feed_flush()
    from public, anon, authenticated, service_role;
revoke execute on function iamb_private.conflict_history(uuid), iamb_private.position(uuid, text, text, bigint)
    from public, anon, service_role;
grant execute on function iamb_private.conflict_history(uuid), iamb_private.position(uuid, text, text, bigint)
    to authenticated;
