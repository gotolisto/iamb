-- Iamb server, migration 4: the read policies of docs/sync-design.md §5.13, the policies on the
-- storage bucket and on the live channel, and the view that gives rows their wire form.
--
-- Every policy is a read rule for `authenticated` and requires `iamb_private.session_ok()`. There
-- is no insert, update or delete policy and no such grant: every write goes through a function.
-- Helpers are called as `(select ...)` so that each is evaluated once per query.

do $$
begin
    if current_user <> 'postgres' then
        raise exception 'iamb migrations must run as postgres, not %', current_user;
    end if;
end
$$;

-- Drop and recreate, so that the migration can be applied again.
do $$
declare
    p record;
begin
    for p in select schemaname, tablename, policyname from pg_policies where schemaname = 'iamb' loop
        execute format('drop policy %I on %I.%I', p.policyname, p.schemaname, p.tablename);
    end loop;
end
$$;

create policy profile_read on iamb.profile for select to authenticated using (
    (select iamb_private.session_ok())
    and (
        (kind = 'personal' and owner_user_id = (select auth.uid()))
        or (select iamb_private.member_of(profile.id))
        or exists (select 1 from iamb.folder_access fa
                   where fa.user_id = (select auth.uid()) and fa.profile_id = profile.id)
    ));

create policy membership_read on iamb.membership for select to authenticated using (
    (select iamb_private.session_ok())
    and (user_id = (select auth.uid())
         or (deleted_at is null and (select iamb_private.member_of(membership.profile_id))))
);

create policy folder_read on iamb.folder for select to authenticated using (
    (select iamb_private.session_ok())
    and exists (select 1 from iamb.folder_access fa
                where fa.user_id = (select auth.uid()) and fa.folder_id = folder.id)
);

create policy folder_share_read on iamb.folder_share for select to authenticated using (
    (select iamb_private.session_ok())
    and (
        exists (select 1 from iamb.folder_access fa
                where fa.user_id = (select auth.uid()) and fa.folder_id = folder_share.folder_id
                  and fa.permission = 'manage')
        or ((select iamb_private.member_of(folder_share.profile_id))
            and exists (select 1 from iamb.folder_access fa
                        where fa.user_id = (select auth.uid()) and fa.folder_id = folder_share.folder_id))
    ));

create policy snippet_read on iamb.snippet for select to authenticated using (
    (select iamb_private.session_ok())
    and exists (select 1 from iamb.folder_access fa
                where fa.user_id = (select auth.uid()) and fa.folder_id = snippet.folder_id)
);

create policy tag_read on iamb.tag for select to authenticated using (
    (select iamb_private.session_ok())
    and (
        exists (select 1 from iamb.profile p
                where p.id = tag.profile_id and p.kind = 'personal' and p.owner_user_id = (select auth.uid()))
        or (select iamb_private.member_of(tag.profile_id))
        or exists (select 1 from iamb.snippet_tag l join iamb.snippet s on s.id = l.snippet_id
                   where l.tag_id = tag.id and l.deleted_at is null and s.deleted_at is null)
        or (updated_by = (select auth.uid())
            and exists (select 1 from iamb.folder_access fa join iamb.folder f on f.id = fa.folder_id
                        where fa.user_id = (select auth.uid()) and fa.profile_id = tag.profile_id
                          and fa.permission in ('edit', 'manage') and f.deleted_at is null))
    ));

create policy snippet_tag_read on iamb.snippet_tag for select to authenticated using (
    (select iamb_private.session_ok())
    and exists (select 1 from iamb.snippet s where s.id = snippet_tag.snippet_id)
);

create policy attachment_read on iamb.attachment for select to authenticated using (
    (select iamb_private.session_ok())
    and (
        exists (select 1 from iamb.profile p
                where p.id = attachment.profile_id and p.kind = 'personal' and p.owner_user_id = (select auth.uid()))
        or (select iamb_private.member_of(attachment.profile_id))
        or exists (select 1 from iamb.snippet_attachment l join iamb.snippet s on s.id = l.snippet_id
                   where l.attachment_id = attachment.id and l.deleted_at is null and s.deleted_at is null)
        or (updated_by = (select auth.uid())
            and exists (select 1 from iamb.folder_access fa join iamb.folder f on f.id = fa.folder_id
                        where fa.user_id = (select auth.uid()) and fa.profile_id = attachment.profile_id
                          and fa.permission in ('edit', 'manage') and f.deleted_at is null))
    ));

create policy snippet_attachment_read on iamb.snippet_attachment for select to authenticated using (
    (select iamb_private.session_ok())
    and exists (select 1 from iamb.snippet s where s.id = snippet_attachment.snippet_id)
);

create policy user_profile_read on iamb.user_profile for select to authenticated using (
    (select iamb_private.session_ok())
    and (user_id = (select auth.uid())
         or (select iamb_private.shares_group_with(user_profile.user_id))
         or exists (select 1 from iamb.profile p where p.owner_user_id = user_profile.user_id))
);

create policy device_read on iamb.device for select to authenticated using (
    (select iamb_private.session_ok()) and user_id = (select auth.uid())
);

create policy folder_access_read on iamb.folder_access for select to authenticated using (
    (select iamb_private.session_ok()) and user_id = (select auth.uid())
);

create policy feed_read on iamb.feed for select to authenticated using (
    (select iamb_private.session_ok()) and user_id = (select auth.uid())
);

create policy feed_head_read on iamb.feed_head for select to authenticated using (
    (select iamb_private.session_ok()) and user_id = (select auth.uid())
);

create policy conflict_read on iamb.conflict for select to authenticated using (
    (select iamb_private.session_ok()) and (select auth.uid()) = any (parties)
);

create policy conflict_version_read on iamb.conflict_version for select to authenticated using (
    (select iamb_private.session_ok())
    and exists (select 1 from iamb.conflict c where c.id = conflict_version.conflict_id)
);

create policy invitation_read on iamb.invitation for select to authenticated using (
    (select iamb_private.session_ok())
    and ((select iamb_private.role_in(invitation.profile_id)) in ('owner', 'admin')
         or (status = 'pending' and email = (select iamb_private.own_address())))
);

create policy attachment_blob_read on iamb.attachment_blob for select to authenticated using (
    (select iamb_private.session_ok())
    and (
        exists (select 1 from iamb.profile p
                where p.id = attachment_blob.profile_id and p.kind = 'personal'
                  and p.owner_user_id = (select auth.uid()))
        or (select iamb_private.member_of(attachment_blob.profile_id))
        or exists (select 1 from iamb.snippet_attachment l
                   join iamb.attachment a on a.id = l.attachment_id
                   join iamb.snippet s on s.id = l.snippet_id
                   where a.profile_id = attachment_blob.profile_id and a.sha256 = attachment_blob.sha256
                     and a.backed and a.deleted_at is null and l.deleted_at is null and s.deleted_at is null)
    ));

create policy attachment_upload_read on iamb.attachment_upload for select to authenticated using (
    (select iamb_private.session_ok()) and uploaded_by = (select auth.uid())
);

-- Tables no device reads (§5.13: "Never; functions read them for the caller"). The explicit
-- refusal documents the intent; these tables have no grant to `authenticated` either.
do $$
declare
    t text;
begin
    foreach t in array array['history', 'push_receipt', 'plan', 'subscription', 'feature_flag',
                             'feature_override', 'server_state', 'epoch', 'epoch_profile', 'rate_bucket'] loop
        execute format('create policy %I on iamb.%I for select to authenticated using (false)', t || '_none', t);
    end loop;
end
$$;

-- ---------------------------------------------------------------------------------------------
-- Rows in their wire form (§6.3, §6.4: the model's fields and names; never `updated_device`,
-- `updated_op`, `lost_rev`, `head_rev` or an offer of ownership). A security-invoker view: read
-- by a device's function it applies that device's policies. A folder is delivered with an empty
-- parent when the reader cannot read the parent (§6.4).
-- ---------------------------------------------------------------------------------------------

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
                                                where fa.folder_id = t.parent_id and fa.user_id = auth.uid())
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

-- ---------------------------------------------------------------------------------------------
-- Storage objects in `attachments` (§5.13). Reading: `<profile>/<sha256>` of an
-- `attachment_blob` row the caller may read. Writing: only `<profile>/upload/<id>` of an
-- unexpired `attachment_upload` row that names the caller, once (no update policy, and the
-- object name is unique). Nobody reads an upload object.
-- ---------------------------------------------------------------------------------------------

drop policy if exists iamb_attachments_read on storage.objects;
drop policy if exists iamb_attachments_upload on storage.objects;

create policy iamb_attachments_read on storage.objects for select to authenticated using (
    bucket_id = 'attachments'
    and (select iamb_private.session_ok())
    and exists (select 1 from iamb.attachment_blob b
                where b.profile_id::text || '/' || b.sha256 = objects.name)
);

create policy iamb_attachments_upload on storage.objects for insert to authenticated with check (
    bucket_id = 'attachments'
    and (select iamb_private.session_ok())
    and exists (select 1 from iamb.attachment_upload u
                where u.profile_id::text || '/upload/' || u.id::text = objects.name
                  and u.uploaded_by = (select auth.uid())
                  and u.expires_at > (extract(epoch from now()) * 1000)::bigint)
);

-- ---------------------------------------------------------------------------------------------
-- The live channel (§6.6): one private broadcast topic per user, `feed:<user id>`. A user may
-- receive on their own topic; there is no insert policy, so no client may send.
-- ---------------------------------------------------------------------------------------------

drop policy if exists iamb_feed_receive on realtime.messages;

create policy iamb_feed_receive on realtime.messages for select to authenticated using (
    extension = 'broadcast'
    and (select realtime.topic()) = 'feed:' || (select auth.uid())::text
    and (select iamb_private.session_ok())
);
