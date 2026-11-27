-- Iamb server, migration 15: uploads under way after the security review of task 4.3
-- (docs/reviews/4.3-security-review.md), finding 8, and finding 6 for `attachment_upload_begin`.
--
-- Before: every open upload counted against the profile's storage allowance with the size its
-- caller declared, whoever opened it; a member with `edit` on one folder could reserve the owner's
-- whole allowance with uploads he never sent, the owner could neither see nor end them, and the
-- answers told him the total size of her files.
--
-- Now:
--  - an upload under way counts against its uploader: at most `uploads_open_max` (20) open uploads
--    per account, and their declared sizes together within the uploader's own storage allowance;
--  - it counts against the profile's allowance only when the uploader belongs to the profile (the
--    owner of a personal profile, a member of a group). For anybody else the profile's allowance
--    is checked when the upload is committed, so `attachment_upload_begin` tells such a caller
--    nothing about the size of the files the profile holds;
--  - whoever manages the profile sees its open uploads (the policy) and can end them, as can the
--    uploader: `api.attachment_upload_cancel`;
--  - a declared size of 0 is refused;
--  - the caller's right in the profile is checked before the profile is locked (finding 6).
--
-- Left for task 4.6 (the edge function `attachment-commit` and the storage service): comparing the
-- object's real size with the declared one before it calls `attachment_verified` (the function
-- refuses a size other than the declared one), bounding the object at upload, and removing the
-- object of an upload that was ended (the function log line `upload_expired` names it).

do $$
begin
    if current_user <> 'postgres' then
        raise exception 'iamb migrations must run as postgres, not %', current_user;
    end if;
end
$$;

update iamb.server_state set rate_limits = rate_limits || '{"uploads_open_max": 20}'
where not rate_limits ? 'uploads_open_max';

-- An upload under way is read by its uploader and by whoever manages the profile it is for.
drop policy if exists attachment_upload_read on iamb.attachment_upload;
create policy attachment_upload_read on iamb.attachment_upload for select to authenticated using (
    (select iamb_private.session_ok())
    and (uploaded_by = (select auth.uid())
         or exists (select 1 from iamb.profile p
                    where p.id = attachment_upload.profile_id and p.kind = 'personal'
                      and p.owner_user_id = (select auth.uid()))
         or (select iamb_private.role_in(attachment_upload.profile_id)) in ('owner', 'admin'))
);

-- Whether the user belongs to the profile: the owner of a personal profile, a live member of a
-- live group.
create or replace function iamb_private.belongs_to(u uuid, p_profile uuid) returns boolean
language sql stable set search_path = pg_catalog, pg_temp as $$
    select exists (select 1 from iamb.profile p where p.id = p_profile and p.kind = 'personal' and p.owner_user_id = u)
        or iamb_private.group_role(u, p_profile) is not null
$$;

create or replace function iamb_private.acct_attachment_upload_begin(u uuid, p_profile uuid, p_sha256 text, p_size bigint, p_mime text)
returns jsonb
language plpgsql volatile set search_path = pg_catalog, pg_temp as $$
declare
    st       iamb.server_state;
    lim      bigint;
    used     bigint;
    own_lim  bigint;
    own_open bigint;
    own_n    bigint;
    uid_     uuid;
    now_ms   bigint := iamb_private.now_ms();
begin
    select * into st from iamb.server_state;
    if p_profile is null or coalesce(p_sha256, '') !~ '^[0-9a-f]{64}$' or p_size is null or p_size <= 0
       or p_mime is null or octet_length(p_mime) > 127 then
        return iamb_private.err('invalid');
    end if;
    -- Before the lock (finding 6): a profile the caller may not write in, known or not.
    if not iamb_private.edits_in_profile(u, p_profile) then
        return iamb_private.err('forbidden');
    end if;
    perform iamb_private.lock_profiles(array[p_profile]);
    if not iamb_private.edits_in_profile(u, p_profile) then
        return iamb_private.err('forbidden');
    end if;
    if p_size > st.attachment_bytes_max then
        return iamb_private.err('too_large');
    end if;
    -- Judged by what the caller may read, so a member learns nothing of the owner's files.
    if iamb_private.can_read_blob(u, p_profile, p_sha256) then
        return jsonb_build_object('error', 'exists');
    end if;
    -- The uploader's own uploads under way: their number, and their sizes against the uploader's
    -- own allowance. Counted under the lock of the uploader's row in `rate_bucket`, so that two
    -- calls of one account for two profiles cannot both pass.
    perform iamb_private.rate_take(u, 'uploads_open', 86400000, 2147483647, 0);
    select count(*), coalesce(sum(x.size), 0) into own_n, own_open
    from iamb.attachment_upload x where x.uploaded_by = u and x.expires_at > now_ms;
    own_lim := iamb_private.user_limit(u, 'storage_bytes_max');
    if own_n >= coalesce((st.rate_limits ->> 'uploads_open_max')::integer, 20)
       or (own_lim is not null and own_open + p_size > own_lim) then
        return iamb_private.err('plan_limit');
    end if;
    -- The profile's allowance, for somebody who belongs to the profile: its verified files and the
    -- uploads under way of the people who belong to it. Anybody else's upload is counted against
    -- the profile when it is committed (`attachment_verified`).
    if iamb_private.belongs_to(u, p_profile) then
        lim := iamb_private.profile_limit(p_profile, 'storage_bytes_max');
        select coalesce((select sum(b.size) from iamb.attachment_blob b where b.profile_id = p_profile), 0)
             + coalesce((select sum(x.size) from iamb.attachment_upload x
                         where x.profile_id = p_profile and x.expires_at > now_ms
                           and iamb_private.belongs_to(x.uploaded_by, p_profile)), 0)
        into used;
        if lim is not null and used + p_size > lim then
            return iamb_private.err('plan_limit');
        end if;
    end if;
    uid_ := iamb_private.uuid7();
    insert into iamb.attachment_upload (id, profile_id, sha256, size, mime, uploaded_by, created_at, expires_at)
    values (uid_, p_profile, p_sha256, p_size, p_mime, u, now_ms, now_ms + 86400000);
    return jsonb_build_object('error', null, 'upload', uid_, 'object', p_profile::text || '/upload/' || uid_::text,
                              'expires_at', now_ms + 86400000);
end
$$;

-- Ends an upload under way: by its uploader, or by whoever manages the profile it is for. The
-- object, if one was sent, is removed by task 4.6's job, which reads the log line. An upload the
-- caller may not end answers like one that does not exist.
create or replace function iamb_private.acct_attachment_upload_cancel(u uuid, p_upload uuid)
returns jsonb
language plpgsql volatile set search_path = pg_catalog, pg_temp as $$
declare
    up iamb.attachment_upload;
begin
    select * into up from iamb.attachment_upload x where x.id = p_upload;
    if up.id is null or not (up.uploaded_by = u or iamb_private.manages_profile(u, up.profile_id)) then
        return iamb_private.err('not_found');
    end if;
    perform iamb_private.lock_profiles(array[up.profile_id]);
    delete from iamb.attachment_upload x
    where x.id = p_upload and (x.uploaded_by = u or iamb_private.manages_profile(u, x.profile_id));
    if not found then
        return iamb_private.err('not_found');
    end if;
    perform iamb_private.log('upload_expired', jsonb_build_object('upload', up.id, 'profile', up.profile_id,
                                                                  'user', u, 'reason', 'cancelled'));
    return jsonb_build_object('error', null);
end
$$;

create or replace function api.attachment_upload_cancel(device uuid, protocol integer, upload uuid)
returns jsonb language plpgsql volatile security definer set search_path = pg_catalog, pg_temp set statement_timeout = '8s' as $$
declare e jsonb := iamb_private.enter(device, protocol, 'attachment_upload_cancel');
begin
    if e ->> 'error' is not null then return e; end if;
    return iamb_private.finish(e, iamb_private.acct_attachment_upload_cancel(auth.uid(), upload));
end $$;

-- `api_service.attachment_verified`: called by the edge function `attachment-commit` with the
-- service key after it has hashed the uploaded object and copied it to `<profile>/<sha256>`. The
-- profile's storage allowance is checked here for every upload (for an uploader who does not
-- belong to the profile this is the only place): a file that does not fit is not recorded, the
-- answer is `plan_limit`, and the edge function removes the object.
create or replace function api_service.attachment_verified(id uuid, sha256 text, size bigint)
returns jsonb language plpgsql volatile security definer set search_path = pg_catalog, pg_temp set statement_timeout = '20s' as $$
#variable_conflict use_column
declare
    up      iamb.attachment_upload;
    now_ms  bigint := iamb_private.now_ms();
    readers uuid[];
    lim     bigint;
    used    bigint;
begin
    select * into up from iamb.attachment_upload x where x.id = attachment_verified.id;
    if up.id is null then
        -- A commit that was cut off and is called again finds what is already done.
        return jsonb_build_object('error', null, 'done', true);
    end if;
    perform iamb_private.lock_profiles(array[up.profile_id]);
    select * into up from iamb.attachment_upload x where x.id = attachment_verified.id;
    if up.id is null then
        return jsonb_build_object('error', null, 'done', true);
    end if;
    if up.sha256 <> attachment_verified.sha256 or up.size <> attachment_verified.size then
        delete from iamb.attachment_upload x where x.id = up.id;
        return jsonb_build_object('error', 'hash_mismatch');
    end if;
    if not exists (select 1 from iamb.attachment_blob b where b.profile_id = up.profile_id and b.sha256 = up.sha256) then
        lim := iamb_private.profile_limit(up.profile_id, 'storage_bytes_max');
        select coalesce(sum(b.size), 0) into used from iamb.attachment_blob b where b.profile_id = up.profile_id;
        if lim is not null and used + up.size > lim then
            delete from iamb.attachment_upload x where x.id = up.id;
            perform iamb_private.log('upload_expired', jsonb_build_object('upload', up.id, 'profile', up.profile_id,
                                                                          'reason', 'plan_limit'));
            return jsonb_build_object('error', 'plan_limit');
        end if;
    end if;
    insert into iamb.attachment_blob (profile_id, sha256, size, mime, verified_at)
    values (up.profile_id, up.sha256, up.size, up.mime, now_ms)
    on conflict (profile_id, sha256) do nothing;
    -- The attachment rows of that profile and hash that the uploader created become backed (C19).
    update iamb.attachment a set backed = true
    where a.profile_id = up.profile_id and a.sha256 = up.sha256 and a.updated_by = up.uploaded_by and not a.backed;
    delete from iamb.attachment_upload x where x.id = up.id;
    select coalesce(array_agg(distinct cand), '{}') into readers from (
        select p.owner_user_id as cand from iamb.profile p where p.id = up.profile_id and p.kind = 'personal'
        union all
        select m.user_id from iamb.membership m where m.profile_id = up.profile_id and m.deleted_at is null
        union all
        select fa.user_id from iamb.attachment a
        join iamb.snippet_attachment l on l.attachment_id = a.id and l.deleted_at is null
        join iamb.snippet s on s.id = l.snippet_id and s.deleted_at is null
        join iamb.folder_access fa on fa.folder_id = s.folder_id
        where a.profile_id = up.profile_id and a.sha256 = up.sha256
    ) x where iamb_private.can_read_blob(cand, up.profile_id, up.sha256);
    perform iamb_private.queue(readers, 'blob', null, null, null, up.profile_id, up.sha256, null);
    perform iamb_private.feed_flush();
    return jsonb_build_object('error', null, 'done', true);
end $$;

revoke execute on function api.attachment_upload_cancel(uuid, integer, uuid) from public, anon, service_role;
grant execute on function api.attachment_upload_cancel(uuid, integer, uuid) to authenticated;
revoke execute on function api_service.attachment_verified(uuid, text, bigint) from public, anon, authenticated;
grant execute on function api_service.attachment_verified(uuid, text, bigint) to service_role;
