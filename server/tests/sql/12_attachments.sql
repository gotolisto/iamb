-- G14 and C19: stored files are readable only through a backed attachment row that a readable
-- snippet links to; knowing a hash gives nothing; uploads are written once by their uploader and
-- read by nobody; a row given back after a restore by someone else has no creator (S73, S81, S84).
do $$
declare
    ids jsonb := iamb_test.family('edit');
    pi uuid; f1 uuid; f2 uuid; s1 uuid; s2 uuid;
    sha text := repeat('1f', 32); sha_new text := repeat('2e', 32);
    a_own uuid := iamb_test.id(); l_own uuid := iamb_test.id(); a_tom uuid := iamb_test.id(); l_tom uuid := iamb_test.id();
    a_rp uuid := iamb_test.id();
    r jsonb; up uuid; n integer; ok boolean; old_ep uuid; backup bigint;
begin
    pi := (ids ->> 'p_ines')::uuid; f1 := (ids ->> 'f1')::uuid; f2 := (ids ->> 'f2')::uuid;
    s1 := (ids ->> 's1')::uuid; s2 := (ids ->> 's2')::uuid;
    -- Ines attaches a file to her private snippet s2: the row first, then the bytes.
    r := iamb_test.push('ines', jsonb_build_array(iamb_test.op_attachment(a_own, pi, sha), iamb_test.op_snippet_attachment(l_own, s2, a_own)));
    perform iamb_test.ok(iamb_test.st(r, 1) = 'applied' and r -> 'results' -> 0 ->> 'blob' = 'missing', 'blob missing: ' || r::text);
    perform iamb_test.ok(not (select backed from iamb.attachment where id = a_own), 'not backed before the bytes');
    r := iamb_test.call('ines', format('api.attachment_upload_begin(iamb_test.dev(''ines''), 1, %L, %L, 10, ''image/png'')', pi, sha));
    up := (r ->> 'upload')::uuid;
    perform iamb_test.ok(up is not null and r ->> 'object' = pi || '/upload/' || up, 'upload begun: ' || r::text);
    -- The upload object: written once, by its uploader, under its own name; read by nobody.
    perform iamb_test.act('tom');
    begin
        insert into storage.objects (bucket_id, name) values ('attachments', pi || '/upload/' || up);
        ok := false;
    exception when insufficient_privilege then ok := true;
    end;
    perform iamb_test.act_owner();
    perform iamb_test.ok(ok, 'another member writes the upload object');
    perform iamb_test.act('ines');
    insert into storage.objects (bucket_id, name) values ('attachments', pi || '/upload/' || up);
    begin
        insert into storage.objects (bucket_id, name) values ('attachments', pi || '/' || sha);
        ok := false;
    exception when insufficient_privilege then ok := true;
    end;
    select count(*) into n from storage.objects where name like '%/upload/%';
    perform iamb_test.act_owner();
    perform iamb_test.ok(ok, 'the uploader writes a verified name directly');
    perform iamb_test.eq(n, 0, 'nobody reads an upload object');
    perform iamb_test.act('ines');
    begin
        update storage.objects set name = name where bucket_id = 'attachments';
        get diagnostics n = row_count;
    exception when insufficient_privilege then n := 0;
    end;
    perform iamb_test.act_owner();
    perform iamb_test.eq(n, 0, 'no object is overwritten');
    -- The commit (the edge function's database part, with the service key).
    insert into storage.objects (bucket_id, name) values ('attachments', pi || '/' || sha);
    perform set_config('role', 'service_role', false);
    r := api_service.attachment_verified(up, repeat('00', 32), 10);
    perform iamb_test.act_owner();
    perform iamb_test.eq(r ->> 'error', 'hash_mismatch', 'a hash that is not the upload''s');
    r := iamb_test.call('ines', format('api.attachment_upload_begin(iamb_test.dev(''ines''), 1, %L, %L, 10, ''image/png'')', pi, sha));
    up := (r ->> 'upload')::uuid;
    perform set_config('role', 'service_role', false);
    r := api_service.attachment_verified(up, sha, 10);
    perform iamb_test.act_owner();
    perform iamb_test.ok(r ->> 'error' is null and (select backed from iamb.attachment where id = a_own), 'verified and backed: ' || r::text);
    perform iamb_test.eq(iamb_test.visible('ines', 'attachment_blob'), 1, 'the owner reads her file');
    r := iamb_test.call('ines', format('api.attachment_upload_begin(iamb_test.dev(''ines''), 1, %L, %L, 10, ''image/png'')', pi, sha));
    perform iamb_test.eq(r ->> 'error', 'exists', 'exists for someone who can read the file');

    -- S73: Tom knows the hash. His own row and a link from the shared snippet give him nothing,
    -- and every answer is the one for a file that does not exist.
    perform iamb_test.pull('tom');
    r := iamb_test.push('tom', jsonb_build_array(iamb_test.op_attachment(a_tom, pi, sha), iamb_test.op_snippet_attachment(l_tom, s1, a_tom)));
    perform iamb_test.ok(iamb_test.st(r, 1) = 'applied' and r -> 'results' -> 0 ->> 'blob' = 'missing'
                         and iamb_test.st(r, 2) = 'applied', 'tom''s row and link: ' || r::text);
    perform iamb_test.ok(not (select backed from iamb.attachment where id = a_tom), 'his row is not backed');
    perform iamb_test.eq(iamb_test.visible('tom', 'attachment_blob'), 0, 'tom reads no blob row');
    perform iamb_test.act('tom');
    select count(*) into n from storage.objects;
    perform iamb_test.act_owner();
    perform iamb_test.eq(n, 0, 'tom reads no file');
    r := iamb_test.call('tom', format('api.attachment_upload_begin(iamb_test.dev(''tom''), 1, %L, %L, 10, ''image/png'')', pi, sha));
    perform iamb_test.ok(r ->> 'error' is null and r ->> 'upload' is not null, 'an existing hash he cannot read: an upload, not exists');
    up := (r ->> 'upload')::uuid;
    r := iamb_test.call('tom', format('api.attachment_upload_begin(iamb_test.dev(''tom''), 1, %L, %L, 10, ''image/png'')', pi, sha_new));
    perform iamb_test.ok(r ->> 'error' is null and r ->> 'upload' is not null, 'a hash nobody has: the same answer');
    -- The ines's private snippet s2 links the file through her backed row; Tom cannot read s2.
    perform iamb_test.eq(iamb_test.visible('tom', 'snippet_attachment'), 1, 'only the link from the shared snippet');
    -- Tom uploads the bytes: the commit keeps the verified file and backs his row.
    perform set_config('role', 'service_role', false);
    r := api_service.attachment_verified(up, sha, 10);
    perform iamb_test.act_owner();
    perform iamb_test.ok((select backed from iamb.attachment where id = a_tom), 'his upload backs his row');
    perform iamb_test.eq(iamb_test.visible('tom', 'attachment_blob'), 1, 'and now he reads it');
    -- unlinked_at follows the links.
    perform iamb_test.ok((select unlinked_at from iamb.attachment where id = a_tom) is null, 'linked');
    r := iamb_test.push('tom', jsonb_build_array(iamb_test.op_delete('snippet_attachment', l_tom, iamb_test.rev('snippet_attachment', l_tom))));
    perform iamb_test.ok((select unlinked_at from iamb.attachment where id = a_tom) is not null, 'unlinked_at set when the last link goes');
    perform iamb_test.eq(iamb_test.visible('tom', 'attachment_blob'), 0, 'no link, no file');

    -- C19, S84: a re-pushed row without author "self" has no creator and is never backed; with
    -- "self" its writer gets it back.
    -- (A restore: a new epoch begins; Ines resyncs and learns her profile's line.) Not on a hosted
    -- project: the move of `rev_seq` would outlive the rolled-back test.
    if current_setting('iamb_test.hosted', true) = 'on' then
        perform iamb_test.check_all();
        return;
    end if;
    old_ep := (select epoch from iamb.server_state);
    perform iamb_private.begin_epoch(0);
    r := iamb_test.call('ines', format('api.snapshot(iamb_test.dev(''ines''), 1, %L)', iamb_test.pos('ines')));
    update iamb_test.actor set position = r ->> 'start' where name = 'ines';
    backup := (select (x ->> 'backup_rev')::bigint from jsonb_array_elements(r -> 'lost' -> 'profiles') x where x ->> 'profile' = pi::text);
    perform iamb_test.ok(backup is not null, 'the snapshot names what the restore lost: ' || (r -> 'lost')::text);
    r := iamb_test.push('ines', jsonb_build_array(iamb_test.op_attachment(a_rp, pi, sha, 0, jsonb_build_object(
        'repush', 'row', 'repush_rev', backup + 1, 'repush_epoch', old_ep))));
    perform iamb_test.ok(iamb_test.st(r) = 'applied', 'a re-pushed row is created: ' || r::text);
    perform iamb_test.ok((select updated_by from iamb.attachment where id = a_rp) is null
                         and not (select backed from iamb.attachment where id = a_rp), 'no creator, not backed, even for the owner');
    perform iamb_test.check_all();
end $$;
