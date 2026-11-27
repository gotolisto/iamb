-- Iamb server, migration 22: a tombstone is read by those who could read the row before it was
-- deleted (docs/reviews/4.3-security-reverify.md, N5; the owner chose option 1 of the review's
-- section 12.3).
--
-- A deleted row keeps its content for 30 days (the retention of tombstones), and until now every
-- read handed it to whoever could read its folder at that moment: also to a member with whom the
-- folder was shared after the delete, or who joined the group after it. ADR-0031 already says, for
-- the versions a write replaced, that a reader gets what they wrote or could read while it was
-- the row. The same now holds for a row that was deleted:
--
--  - a deleted folder, snippet, link or share is readable through an access entry only when the
--    reader deleted it, or the entry began before the delete: `rev > since_rev`. A tombstone's
--    revision is the revision of its delete (no write changes a deleted row without making it
--    live), and `since_rev` is the counter's value when the request that made the entry had ended
--    (migration 19), so a row deleted in that same request is not delivered either, except to
--    who deleted it: a device that creates a folder and deletes a folder above it sends both in
--    one push, and must get the tombstones of what it made;
--  - a link hangs on a snippet, and a snippet can change its folder. So that the entry for the
--    snippet's folder says the right thing about its deleted links, a snippet that changes its
--    folder leaves them behind: the server removes them at the move, as the purge would later.
--    Every deleted link the server holds was deleted while its snippet was where it is now;
--  - a deleted tag or attachment row: its personal profile's owner as before; a member of its
--    group only when the membership began before the delete; and nobody through a link. A link
--    from a shared snippet gives the tag or the attachment row while that row is live;
--  - to everybody else the row is not there: the policies do not show it, `readers` does not name
--    them, a `row` entry of the feed and `rows` deliver null, `snapshot` leaves it out, a cascade
--    list answers `gone`, `visible_folders` and `access` do not list a folder they may not read;
--  - a write does not bring it back for them either. A `put` on a tombstone the writer may not read
--    is `rejected: gone`, as for a row that was purged (C13): had it restored the row, the row's
--    live links would have come back with it, and with them tags and files the writer never had.
--    A row put into, or a folder put under, a deleted folder the writer may not read is
--    `rejected: parent_gone` (C7 restores nothing for them). A share is the exception: its id is
--    derived from its folder and group, and who may write it is decided by `manage` alone (C11);
--  - a deleted version in the history (the tombstone itself, replaced when the row was restored)
--    is offered in a conflict, and counts as the base of a write, only for who wrote it and for
--    a reader whose access began before that delete.
--
-- Nothing changes for a reader whose access began before the delete, except that a tag or an
-- attachment row of someone else's profile now leaves their device when it is deleted (a null
-- row) instead of staying as a tombstone. A device that held a row and whose access ended and
-- began again has removed it by then, or removes it now, on one of these words of the server:
-- `folder_revoked` or a null folder (the tree is purged), the null `row` entry of the delete, the
-- whole fetch that follows `folder_visible` (rows of the tree that it does not bring are removed),
-- a resync, `gone` in a cascade answer or `rejected: gone` to a change it still held
-- (docs/sync-design.md, section 5.4 and D3, D5, D6, D7). And the server never withholds a
-- tombstone that a device holds under an access that still stands: what a reader could read once
-- stays readable until that access ends or the row is purged.
--
-- Nothing is converted. (On a database that held rows when migration 19 was first applied, every
-- access entry began at that moment, its owner's included: a tombstone older than that is then
-- read by who deleted it and by nobody else until its purge, and devices that hold the row are
-- told to remove it, as they would be by the tombstone. The hosted project held no row.)

do $$
begin
    if current_user <> 'postgres' then
        raise exception 'iamb migrations must run as postgres, not %', current_user;
    end if;
end
$$;

-- Whether the caller has a live membership in the live group `p` that began before the revision
-- `r` (a gate helper like `member_of`: the policies never query `membership` themselves).
create or replace function iamb_private.member_before(p uuid, r bigint) returns boolean
language sql stable security definer set search_path = pg_catalog, pg_temp as $$
    select exists (
        select 1 from iamb.membership m join iamb.profile g on g.id = m.profile_id
        where m.profile_id = p and m.user_id = auth.uid() and m.deleted_at is null
          and g.deleted_at is null and g.kind <> 'personal' and m.created_rev < r)
$$;

-- ---------------------------------------------------------------------------------------------
-- The read policies of the rows that can be tombstones a reader must not get (section 5.13).
-- ---------------------------------------------------------------------------------------------

drop policy if exists folder_read on iamb.folder;
create policy folder_read on iamb.folder for select to authenticated using (
    (select iamb_private.session_ok())
    and exists (select 1 from iamb.folder_access fa
                where fa.user_id = (select auth.uid()) and fa.folder_id = folder.id
                  and (folder.deleted_at is null or folder.rev > fa.since_rev
                       or folder.updated_by = (select auth.uid())))
);

drop policy if exists folder_share_read on iamb.folder_share;
create policy folder_share_read on iamb.folder_share for select to authenticated using (
    (select iamb_private.session_ok())
    and (
        exists (select 1 from iamb.folder_access fa
                where fa.user_id = (select auth.uid()) and fa.folder_id = folder_share.folder_id
                  and fa.permission = 'manage'
                  and (folder_share.deleted_at is null or folder_share.rev > fa.since_rev
                       or folder_share.updated_by = (select auth.uid())))
        or ((select iamb_private.member_of(folder_share.profile_id))
            and exists (select 1 from iamb.folder_access fa
                        where fa.user_id = (select auth.uid()) and fa.folder_id = folder_share.folder_id
                          and (folder_share.deleted_at is null or folder_share.rev > fa.since_rev
                               or folder_share.updated_by = (select auth.uid()))))
    ));

drop policy if exists snippet_read on iamb.snippet;
create policy snippet_read on iamb.snippet for select to authenticated using (
    (select iamb_private.session_ok())
    and exists (select 1 from iamb.folder_access fa
                where fa.user_id = (select auth.uid()) and fa.folder_id = snippet.folder_id
                  and (snippet.deleted_at is null or snippet.rev > fa.since_rev
                       or snippet.updated_by = (select auth.uid())))
);

-- A link: its snippet is readable (the snippet's own policy applies to the subquery), and the
-- link is live, or the reader deleted it, or it was deleted after the reader's entry for the
-- snippet's folder began.
drop policy if exists snippet_tag_read on iamb.snippet_tag;
create policy snippet_tag_read on iamb.snippet_tag for select to authenticated using (
    (select iamb_private.session_ok())
    and exists (select 1 from iamb.snippet s
                join iamb.folder_access fa on fa.user_id = (select auth.uid()) and fa.folder_id = s.folder_id
                where s.id = snippet_tag.snippet_id
                  and (snippet_tag.deleted_at is null or snippet_tag.rev > fa.since_rev
                       or snippet_tag.updated_by = (select auth.uid())))
);

drop policy if exists snippet_attachment_read on iamb.snippet_attachment;
create policy snippet_attachment_read on iamb.snippet_attachment for select to authenticated using (
    (select iamb_private.session_ok())
    and exists (select 1 from iamb.snippet s
                join iamb.folder_access fa on fa.user_id = (select auth.uid()) and fa.folder_id = s.folder_id
                where s.id = snippet_attachment.snippet_id
                  and (snippet_attachment.deleted_at is null or snippet_attachment.rev > fa.since_rev
                       or snippet_attachment.updated_by = (select auth.uid())))
);

drop policy if exists tag_read on iamb.tag;
create policy tag_read on iamb.tag for select to authenticated using (
    (select iamb_private.session_ok())
    and (
        exists (select 1 from iamb.profile p
                where p.id = tag.profile_id and p.kind = 'personal' and p.owner_user_id = (select auth.uid()))
        or ((select iamb_private.member_of(tag.profile_id))
            and (tag.deleted_at is null or iamb_private.member_before(tag.profile_id, tag.rev)))
        or (tag.deleted_at is null
            and exists (select 1 from iamb.snippet_tag l join iamb.snippet s on s.id = l.snippet_id
                        where l.tag_id = tag.id and l.deleted_at is null and s.deleted_at is null))
        or (updated_by = (select auth.uid())
            and exists (select 1 from iamb.folder_access fa join iamb.folder f on f.id = fa.folder_id
                        where fa.user_id = (select auth.uid()) and fa.profile_id = tag.profile_id
                          and fa.permission in ('edit', 'manage') and f.deleted_at is null))
    ));

drop policy if exists attachment_read on iamb.attachment;
create policy attachment_read on iamb.attachment for select to authenticated using (
    (select iamb_private.session_ok())
    and (
        exists (select 1 from iamb.profile p
                where p.id = attachment.profile_id and p.kind = 'personal' and p.owner_user_id = (select auth.uid()))
        or ((select iamb_private.member_of(attachment.profile_id))
            and (attachment.deleted_at is null or iamb_private.member_before(attachment.profile_id, attachment.rev)))
        or (attachment.deleted_at is null
            and exists (select 1 from iamb.snippet_attachment l join iamb.snippet s on s.id = l.snippet_id
                        where l.attachment_id = attachment.id and l.deleted_at is null and s.deleted_at is null))
        or (updated_by = (select auth.uid())
            and exists (select 1 from iamb.folder_access fa join iamb.folder f on f.id = fa.folder_id
                        where fa.user_id = (select auth.uid()) and fa.profile_id = attachment.profile_id
                          and fa.permission in ('edit', 'manage') and f.deleted_at is null))
    ));

-- ---------------------------------------------------------------------------------------------
-- The same rule for the functions that read for a caller: who can read a row now.
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
        where s.id = p_id and (s.deleted_at is null or s.rev > fa.since_rev or s.updated_by = fa.user_id);
    when 'snippet_tag' then
        select array_agg(fa.user_id) into res
        from iamb.snippet_tag l join iamb.snippet s on s.id = l.snippet_id
        join iamb.folder_access fa on fa.folder_id = s.folder_id
        where l.id = p_id and (s.deleted_at is null or s.rev > fa.since_rev or s.updated_by = fa.user_id)
          and (l.deleted_at is null or l.rev > fa.since_rev or l.updated_by = fa.user_id);
    when 'snippet_attachment' then
        select array_agg(fa.user_id) into res
        from iamb.snippet_attachment l join iamb.snippet s on s.id = l.snippet_id
        join iamb.folder_access fa on fa.folder_id = s.folder_id
        where l.id = p_id and (s.deleted_at is null or s.rev > fa.since_rev or s.updated_by = fa.user_id)
          and (l.deleted_at is null or l.rev > fa.since_rev or l.updated_by = fa.user_id);
    when 'folder_share' then
        select array_agg(distinct fa.user_id) into res
        from iamb.folder_share sh join iamb.folder_access fa on fa.folder_id = sh.folder_id
        where sh.id = p_id
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

-- Rows in their wire form; see migration 4. A folder is delivered with an empty parent when the
-- reader cannot read the parent, and a deleted parent the reader may not read is such a parent.
create or replace view iamb.wire_rows with (security_invoker = true) as
select 'profile'::text as entity, t.id, t.id as profile_id, t.rev, t.deleted_at,
       jsonb_build_object('id', t.id, 'created_at', t.created_at, 'updated_at', t.updated_at,
                          'updated_by', t.updated_by, 'rev', t.rev, 'deleted_at', t.deleted_at,
                          'kind', t.kind, 'name', t.name, 'owner_user_id', t.owner_user_id) as row,
       null::jsonb as origin
from iamb.profile t
union all
select 'membership', t.id, t.profile_id, t.rev, t.deleted_at,
       jsonb_build_object('id', t.id, 'created_at', t.created_at, 'updated_at', t.updated_at,
                          'updated_by', t.updated_by, 'rev', t.rev, 'deleted_at', t.deleted_at,
                          'profile_id', t.profile_id, 'user_id', t.user_id, 'role', t.role),
       null
from iamb.membership t
union all
select 'folder', t.id, t.profile_id, t.rev, t.deleted_at,
       jsonb_build_object('id', t.id, 'created_at', t.created_at, 'updated_at', t.updated_at,
                          'updated_by', t.updated_by, 'rev', t.rev, 'deleted_at', t.deleted_at,
                          'profile_id', t.profile_id,
                          'parent_id', case when t.parent_id is not null and exists (
                                                select 1 from iamb.folder_access fa
                                                join iamb.folder pf on pf.id = fa.folder_id
                                                where fa.folder_id = t.parent_id and fa.user_id = auth.uid()
                                                  and (pf.deleted_at is null or pf.rev > fa.since_rev
                                                       or pf.updated_by = auth.uid()))
                                            then t.parent_id end,
                          'name', t.name, 'position', t.position, 'member_permission', t.member_permission,
                          'defaults', jsonb_build_object('trigger', t.default_trigger,
                                                         'case_mode', t.default_case_mode,
                                                         'abbreviation_prefix', t.abbreviation_prefix)),
       null
from iamb.folder t
union all
select 'folder_share', t.id, null, t.rev, t.deleted_at,
       jsonb_build_object('id', t.id, 'created_at', t.created_at, 'updated_at', t.updated_at,
                          'updated_by', t.updated_by, 'rev', t.rev, 'deleted_at', t.deleted_at,
                          'folder_id', t.folder_id, 'profile_id', t.profile_id, 'permission', t.permission),
       null
from iamb.folder_share t
union all
select 'snippet', t.id, t.profile_id, t.rev, t.deleted_at,
       jsonb_build_object('id', t.id, 'created_at', t.created_at, 'updated_at', t.updated_at,
                          'updated_by', t.updated_by, 'rev', t.rev, 'deleted_at', t.deleted_at,
                          'folder_id', t.folder_id, 'abbreviation', t.abbreviation, 'label', t.label,
                          'kind', t.kind, 'body', t.body, 'trigger', t.trigger_mode,
                          'case_mode', t.case_mode, 'word_start', t.word_start, 'enabled', t.enabled),
       jsonb_build_object('content_by', t.content_by, 'content_at', t.content_at, 'created_rev', t.created_rev)
from iamb.snippet t
union all
select 'tag', t.id, t.profile_id, t.rev, t.deleted_at,
       jsonb_build_object('id', t.id, 'created_at', t.created_at, 'updated_at', t.updated_at,
                          'updated_by', t.updated_by, 'rev', t.rev, 'deleted_at', t.deleted_at,
                          'profile_id', t.profile_id, 'name', t.name, 'color', t.color),
       null
from iamb.tag t
union all
select 'snippet_tag', t.id, null, t.rev, t.deleted_at,
       jsonb_build_object('id', t.id, 'created_at', t.created_at, 'updated_at', t.updated_at,
                          'updated_by', t.updated_by, 'rev', t.rev, 'deleted_at', t.deleted_at,
                          'snippet_id', t.snippet_id, 'tag_id', t.tag_id),
       null
from iamb.snippet_tag t
union all
select 'attachment', t.id, t.profile_id, t.rev, t.deleted_at,
       jsonb_build_object('id', t.id, 'created_at', t.created_at, 'updated_at', t.updated_at,
                          'updated_by', t.updated_by, 'rev', t.rev, 'deleted_at', t.deleted_at,
                          'profile_id', t.profile_id, 'sha256', t.sha256, 'mime', t.mime,
                          'size', t.size, 'file_name', t.file_name),
       null
from iamb.attachment t
union all
select 'snippet_attachment', t.id, null, t.rev, t.deleted_at,
       jsonb_build_object('id', t.id, 'created_at', t.created_at, 'updated_at', t.updated_at,
                          'updated_by', t.updated_by, 'rev', t.rev, 'deleted_at', t.deleted_at,
                          'snippet_id', t.snippet_id, 'attachment_id', t.attachment_id),
       null
from iamb.snippet_attachment t;

revoke all on iamb.wire_rows from public, anon, authenticated, service_role;
grant select on iamb.wire_rows to authenticated;

-- Whether the user could read a replaced version of a row while it was the row (C3, C17, C20); see
-- migration 19. A deleted version (the tombstone that a restore of the row replaced) is judged as
-- the tombstone is: readable for an access that began before the delete.
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
                  and fa.since_rev < lim.r)
            when 'folder' then exists (
                select 1 from iamb.folder_access fa
                where fa.user_id = u and fa.folder_id = iamb_private.try_uuid(hrow ->> 'id')
                  and fa.since_rev < lim.r)
            when 'tag' then exists (
                select 1 from iamb.profile p
                where p.id = iamb_private.try_uuid(hrow ->> 'profile_id') and p.deleted_at is null
                  and ((p.kind = 'personal' and p.owner_user_id = u)
                       or (p.kind <> 'personal' and exists (
                             select 1 from iamb.membership m
                             where m.profile_id = p.id and m.user_id = u and m.deleted_at is null
                               and m.created_rev < lim.r))))
            else false
        end, false)
    -- A live version was the row until it was replaced; a deleted one could be read only by those
    -- who could read the row before that delete, which is the version's own revision.
    from (select case when (hrow ->> 'deleted_at') is null then p_replaced_by
                      else least(p_replaced_by, (hrow ->> 'rev')::bigint) end as r) lim
$$;

-- The answer for every row of a folder delete's cascade list (C6); see migration 20. A row that
-- is a tombstone the caller may not read is `gone`, like a row the caller cannot read at all.
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
             and exists (select 1 from iamb.folder_access fa where fa.user_id = u and fa.folder_id = s.folder_id
                           and (s.deleted_at is null or s.rev > fa.since_rev or s.updated_by = u))
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

-- Writes a new revision of a content row; see migration 12. Since then: a snippet that changes
-- its folder leaves its deleted links behind, and a tag or an attachment row that comes back
-- brings its links.
create or replace function iamb_private.write_row(u uuid, dev uuid, op uuid, p_entity text, cur jsonb, newrow jsonb,
                                                  p_rev bigint, before uuid[], p_structural boolean,
                                                  p_profile uuid) returns void
language plpgsql volatile set search_path = pg_catalog, pg_temp as $$
declare
    after uuid[];
    gained uuid[];
    l record;
begin
    if cur is not null then
        perform iamb_private.keep_history(p_entity, cur, p_rev);
    end if;
    perform iamb_private.store_row(p_entity, newrow, cur is null);
    -- A snippet that changes its folder leaves its deleted links behind: they are removed here,
    -- with their history, as the purge would remove them later. What the server holds of a
    -- snippet's deleted links was therefore deleted while the snippet was where it is now, and
    -- the entry for that folder says who could read them (section 5.4).
    if p_entity = 'snippet' and cur is not null and (cur ->> 'folder_id') is distinct from (newrow ->> 'folder_id') then
        with gone as (
            delete from iamb.snippet_tag where snippet_id = (newrow ->> 'id')::uuid and deleted_at is not null
            returning id
        )
        delete from iamb.history h using gone where h.entity = 'snippet_tag' and h.row_id = gone.id;
        with gone as (
            delete from iamb.snippet_attachment where snippet_id = (newrow ->> 'id')::uuid and deleted_at is not null
            returning id
        )
        delete from iamb.history h using gone where h.entity = 'snippet_attachment' and h.row_id = gone.id;
    end if;
    perform iamb_private.mark_written(p_entity, (newrow ->> 'id')::uuid, cur is null);
    if p_structural then
        perform iamb_private.refresh_access(array[p_profile]);
    end if;
    after := iamb_private.readers(p_entity, (newrow ->> 'id')::uuid);
    perform iamb_private.queue(before || after, 'row', p_entity, (newrow ->> 'id')::uuid, null, p_profile, null, null);
    -- A tag or an attachment row that comes back brings its links: who reads it through a link
    -- lost it when it was deleted, and a device drops the links of a row it loses (section 5.13).
    if p_entity in ('tag', 'attachment') and cur is not null and (cur ->> 'deleted_at') is not null
       and (newrow ->> 'deleted_at') is null then
        gained := array(select x from unnest(after) x where not x = any (before));
        if coalesce(array_length(gained, 1), 0) > 0 then
            for l in select 'snippet_tag' as e, t.id from iamb.snippet_tag t
                     where p_entity = 'tag' and t.tag_id = (newrow ->> 'id')::uuid and t.deleted_at is null
                     union all
                     select 'snippet_attachment', t.id from iamb.snippet_attachment t
                     where p_entity = 'attachment' and t.attachment_id = (newrow ->> 'id')::uuid and t.deleted_at is null
            loop
                perform iamb_private.queue(gained, 'row', l.e, l.id, null, p_profile, null, null);
            end loop;
        end if;
    end if;
    -- A snippet that becomes readable for a user through a move brings its links (§5.5).
    if p_entity = 'snippet' and cur is not null then
        gained := array(select x from unnest(after) x where not x = any (before));
        if coalesce(array_length(gained, 1), 0) > 0 then
            for l in select 'snippet_tag' as e, id from iamb.snippet_tag where snippet_id = (newrow ->> 'id')::uuid and deleted_at is null
                     union all
                     select 'snippet_attachment', id from iamb.snippet_attachment where snippet_id = (newrow ->> 'id')::uuid and deleted_at is null
            loop
                perform iamb_private.queue(gained, 'row', l.e, l.id, null, p_profile, null, null);
            end loop;
        end if;
    end if;
end
$$;

-- C7: restores every deleted folder from the given one upwards, if the writer may; false, with
-- nothing written, when not; see migration 20. The writer may not when one of them is a tombstone
-- the writer cannot read.
create or replace function iamb_private.restore_chain(u uuid, dev uuid, op uuid, p_folder uuid) returns boolean
language plpgsql volatile set search_path = pg_catalog, pg_temp as $$
declare
    chain  uuid[] := '{}';
    a      uuid := p_folder;
    f      iamb.folder;
    cur    jsonb;
    nr     jsonb;
    r      bigint;
    before uuid[];
    now_ms bigint := iamb_private.now_ms();
    i      integer;
    n      integer;
    lvl    integer;
    a3     uuid;
    newp   uuid[];
begin
    while a is not null loop
        select * into f from iamb.folder where id = a;
        exit when f.id is null or a = any (chain);
        if f.deleted_at is not null then
            -- A deleted folder the writer may not read is not there for the writer: restored, it
            -- would hand over a name the writer never had.
            if not iamb_private.can_read(u, 'folder', a) then return false; end if;
            if f.parent_id is null then
                if not iamb_private.manages_profile(u, f.profile_id) then return false; end if;
            elsif not iamb_private.can_edit(u, f.parent_id) then
                return false;
            end if;
            chain := chain || a;
        end if;
        a := f.parent_id;
    end loop;
    -- C8 (N4): a deleted folder can lie deeper than four levels, after a folder above it was moved.
    -- Restored, it goes under the folder on the third level of its path, if the writer may put it
    -- there. Decided for the whole chain before anything is written.
    n := coalesce(array_length(chain, 1), 0);
    if n > 0 then
        newp := array_fill(null::uuid, array[n]);
        select * into f from iamb.folder where id = chain[n];
        lvl := coalesce(iamb_private.folder_depth(f.parent_id), 0);
        if lvl >= 3 then
            a3 := f.parent_id;
            for i in 1..(lvl - 3) loop
                select g.parent_id into a3 from iamb.folder g where g.id = a3;
            end loop;
        end if;
        for i in reverse n..1 loop
            lvl := lvl + 1;
            if lvl > 4 then
                if not iamb_private.can_edit(u, a3) then
                    return false;
                end if;
                newp[i] := a3;
                lvl := 4;
            elsif lvl = 3 then
                a3 := chain[i];
            end if;
        end loop;
    end if;
    -- Top-down, so that no live folder is ever under a deleted one.
    for i in reverse n..1 loop
        cur := iamb_private.get_row('folder', chain[i]);
        before := iamb_private.readers('folder', chain[i]);
        r := iamb_private.next_rev(array[(cur ->> 'profile_id')::uuid]);
        nr := cur || jsonb_build_object('deleted_at', null, 'rev', r, 'updated_at', now_ms, 'updated_by', u,
                                        'updated_device', dev, 'updated_op', op)
                  || case when newp[i] is not null then jsonb_build_object('parent_id', newp[i]) else '{}'::jsonb end;
        perform iamb_private.write_row(u, dev, op, 'folder', cur, nr, r, before, true, (cur ->> 'profile_id')::uuid);
        perform iamb_private.record_conflict('folder', chain[i], (cur ->> 'profile_id')::uuid, 'folder_restored',
                                             r, u, dev, cur);
    end loop;
    return true;
end
$$;

-- One op of a push, or the restore of a conflict's version (C1 to C21); see migrations 12 and 20.
-- Since then: a put on a tombstone the writer may not read is `rejected: gone`.
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

-- The ids of all folders the caller can read; answered from `protocol_oldest` on and for accounts
-- whose plan has no sync (the access check, D12). A deleted folder the caller may not read is not
-- among them.
create or replace function api.visible_folders(device uuid, protocol integer)
returns jsonb language plpgsql volatile security invoker set search_path = pg_catalog, pg_temp set statement_timeout = '8s' as $$
declare
    e jsonb := iamb_private.enter(device, protocol, 'visible_folders');
begin
    if e ->> 'error' is not null then return e; end if;
    return jsonb_build_object('error', null, 'epoch', e -> 'epoch', 'folders', coalesce((
        select jsonb_agg(fa.folder_id order by fa.folder_id)
        from iamb.folder_access fa join iamb.folder f on f.id = fa.folder_id
        where fa.user_id = auth.uid()
          and (f.deleted_at is null or f.rev > fa.since_rev or f.updated_by = auth.uid())), '[]'::jsonb));
end $$;

-- The caller's access roots outside their personal profile, and their role in each group. A
-- deleted folder the caller may not read is no root.
create or replace function api.access(device uuid, protocol integer)
returns jsonb language plpgsql volatile security invoker set search_path = pg_catalog, pg_temp set statement_timeout = '8s' as $$
declare
    e   jsonb := iamb_private.enter(device, protocol, 'access');
    uid uuid := auth.uid();
begin
    if e ->> 'error' is not null then return e; end if;
    return jsonb_build_object('error', null, 'epoch', e -> 'epoch',
        'roots', coalesce((
            select jsonb_agg(jsonb_build_object('root', fa.folder_id, 'profile', fa.profile_id,
                                                'permission', fa.permission, 'via', fa.via,
                                                'granted_at', fa.granted_at) order by fa.granted_at, fa.folder_id)
            from iamb.folder_access fa join iamb.folder f on f.id = fa.folder_id
            where fa.user_id = uid and fa.root_folder_id = fa.folder_id and fa.via <> 'own'
              and (f.deleted_at is null or f.rev > fa.since_rev or f.updated_by = uid)), '[]'::jsonb),
        'groups', coalesce((
            select jsonb_agg(jsonb_build_object('group', m.profile_id, 'role', m.role) order by m.profile_id)
            from iamb.membership m where m.user_id = uid and m.deleted_at is null), '[]'::jsonb));
end $$;

revoke execute on function iamb_private.member_before(uuid, bigint) from public, anon, service_role;
grant execute on function iamb_private.member_before(uuid, bigint) to authenticated;
revoke execute on function iamb_private.readers(text, uuid),
                           iamb_private.version_readable(uuid, text, jsonb, bigint),
                           iamb_private.cascade_answer(uuid, jsonb),
                           iamb_private.write_row(uuid, uuid, uuid, text, jsonb, jsonb, bigint, uuid[], boolean, uuid),
                           iamb_private.restore_chain(uuid, uuid, uuid, uuid),
                           iamb_private.push_op(uuid, uuid, jsonb, uuid, integer, uuid[], jsonb)
    from public, anon, authenticated, service_role;
revoke execute on function api.visible_folders(uuid, integer), api.access(uuid, integer) from public, anon, service_role;
grant execute on function api.visible_folders(uuid, integer), api.access(uuid, integer) to authenticated;
