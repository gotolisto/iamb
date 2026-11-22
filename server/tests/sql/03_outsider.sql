-- The plan's first test: an outsider reads nothing. A stranger with a valid session and device,
-- in no group with Ines, sees none of her rows through any table or function, and cannot write
-- into her profile (§4.5, G2, G3, G6, G14, G16).
do $$
declare
    ids  jsonb := iamb_test.family('edit');
    t    text;
    n    integer;
    r    jsonb;
    p    jsonb;
    sha  text := repeat('ab', 32);
    a1   uuid := iamb_test.id();
    l1   uuid := iamb_test.id();
    t1   uuid := iamb_test.id();
    lt1  uuid := iamb_test.id();
begin
    -- Ines adds a tag, an attachment with its verified file, and links.
    r := iamb_test.push('ines', jsonb_build_array(
        iamb_test.op_tag(t1, (ids ->> 'p_ines')::uuid, 'shop'),
        iamb_test.op_attachment(a1, (ids ->> 'p_ines')::uuid, sha),
        iamb_test.op_snippet_tag(lt1, (ids ->> 's1')::uuid, t1),
        iamb_test.op_snippet_attachment(l1, (ids ->> 's1')::uuid, a1)));
    perform iamb_test.ok(r ->> 'error' is null and iamb_test.st(r, 4) = 'applied', 'ines push ' || r::text);
    insert into iamb.attachment_blob values ((ids ->> 'p_ines')::uuid, sha, 10, 'image/png', 1);
    insert into storage.objects (bucket_id, name) values ('attachments', (ids ->> 'p_ines') || '/' || sha);
    perform iamb_test.check_all();

    -- Every table: nothing of Ines's (the stranger's own rows only).
    foreach t in array array['profile', 'membership', 'folder', 'folder_share', 'snippet', 'tag', 'snippet_tag',
                             'attachment', 'snippet_attachment', 'folder_access', 'conflict', 'conflict_version',
                             'invitation', 'attachment_blob', 'attachment_upload'] loop
        perform iamb_test.act('stranger');
        execute format('select count(*) from iamb.%I where %s', t, case t
            when 'profile' then 'owner_user_id <> auth.uid()'
            when 'membership' then 'true'
            when 'folder_access' then 'user_id <> auth.uid()'
            else 'true' end) into n;
        perform iamb_test.act_owner();
        perform iamb_test.eq(n, 0, 'stranger reads rows of ' || t);
    end loop;
    perform iamb_test.act('stranger');
    select count(*) into n from iamb.user_profile where user_id <> auth.uid();
    perform iamb_test.act_owner();
    perform iamb_test.eq(n, 0, 'stranger reads other display names');
    perform iamb_test.act('stranger');
    select count(*) into n from iamb.feed f where f.user_id <> auth.uid();
    perform iamb_test.act_owner();
    perform iamb_test.eq(n, 0, 'stranger reads other feeds');
    perform iamb_test.act('stranger');
    select count(*) into n from iamb.device d where d.user_id <> auth.uid();
    perform iamb_test.act_owner();
    perform iamb_test.eq(n, 0, 'stranger reads other devices');
    perform iamb_test.act('stranger');
    select count(*) into n from iamb.wire_rows w where w.entity not in ('profile') or w.id <> iamb_test.personal('stranger');
    perform iamb_test.act_owner();
    perform iamb_test.eq(n, 0, 'stranger reads wire rows');

    -- Every reading function.
    p := iamb_test.pull('stranger');
    perform iamb_test.ok(not exists (select 1 from jsonb_array_elements(p -> 'entries') e
                                     where e ->> 'kind' <> 'row' or e ->> 'id' <> iamb_test.personal('stranger')::text),
                         'stranger''s feed holds only their own profile: ' || p::text);
    r := iamb_test.call('stranger', format('api.rows(iamb_test.dev(''stranger''), 1, %L::jsonb)', jsonb_build_array(
        jsonb_build_object('entity', 'folder', 'id', ids -> 'f1'), jsonb_build_object('entity', 'snippet', 'id', ids -> 's1'),
        jsonb_build_object('entity', 'tag', 'id', t1), jsonb_build_object('entity', 'attachment', 'id', a1),
        jsonb_build_object('entity', 'profile', 'id', ids -> 'p_ines'), jsonb_build_object('entity', 'profile', 'id', ids -> 'g'))));
    perform iamb_test.ok(not exists (select 1 from jsonb_array_elements(r -> 'rows') x where jsonb_typeof(x -> 'row') = 'object'),
                         'rows() gives the stranger nothing: ' || r::text);
    r := iamb_test.call('stranger', format('api.fetch_folder(iamb_test.dev(''stranger''), 1, %L)', ids ->> 'f1'));
    perform iamb_test.ok(r ->> 'error' = 'not_found' or jsonb_array_length(r -> 'rows') = 0, 'fetch_folder: ' || r::text);
    r := iamb_test.call('stranger', 'api.snapshot(iamb_test.dev(''stranger''), 1, iamb_test.pos(''stranger''), null)');
    perform iamb_test.ok(r ->> 'error' is null and jsonb_array_length(r -> 'rows') = 1, 'snapshot: own profile only: ' || r::text);
    r := iamb_test.call('stranger', 'api.visible_folders(iamb_test.dev(''stranger''), 1)');
    perform iamb_test.eq(jsonb_array_length(r -> 'folders'), 0, 'visible_folders');
    r := iamb_test.call('stranger', 'api.access(iamb_test.dev(''stranger''), 1)');
    perform iamb_test.eq(jsonb_array_length(r -> 'roots'), 0, 'access roots');
    r := iamb_test.call('stranger', format('api.members(iamb_test.dev(''stranger''), 1, %L)', ids ->> 'g'));
    perform iamb_test.eq(r ->> 'error', 'not_found', 'members of a group not one''s own');
    r := iamb_test.call('stranger', format('api.users(iamb_test.dev(''stranger''), 1, array[%L, %L]::uuid[])',
                                           iamb_test.uid('ines'), iamb_test.uid('tom')));
    perform iamb_test.eq(jsonb_array_length(r -> 'users'), 0, 'display names');
    r := iamb_test.call('stranger', 'api.invitations(iamb_test.dev(''stranger''), 1)');
    perform iamb_test.eq(jsonb_array_length(r -> 'invitations'), 0, 'invitations');
    r := iamb_test.call('stranger', format('api.attachment_upload_begin(iamb_test.dev(''stranger''), 1, %L, %L, 10, ''image/png'')',
                                           ids ->> 'p_ines', sha));
    perform iamb_test.eq(r ->> 'error', 'forbidden', 'upload into another''s profile');

    -- Writes into Ines's profile.
    r := iamb_test.push('stranger', jsonb_build_array(
        iamb_test.op_snippet(iamb_test.id(), (ids ->> 'f1')::uuid, ';x', 'x'),
        iamb_test.op_snippet((ids ->> 's1')::uuid, (ids ->> 'f1')::uuid, ';x', 'x', 5),
        iamb_test.op_delete('snippet', (ids ->> 's1')::uuid, 5),
        iamb_test.op_folder(iamb_test.id(), (ids ->> 'p_ines')::uuid, null, 'Mine'),
        iamb_test.op_tag(iamb_test.id(), (ids ->> 'p_ines')::uuid, 'x'),
        iamb_test.op_delete('folder', (ids ->> 'f1')::uuid, 3),
        iamb_test.op_share((ids ->> 'f1')::uuid, (ids ->> 'g')::uuid, 'edit', 7)));
    perform iamb_test.eq(iamb_test.st(r, 1), 'rejected:parent_gone', 'create in an unreadable folder');
    -- C13: the folder the row names is unreadable for the writer.
    perform iamb_test.eq(iamb_test.st(r, 2), 'rejected:parent_gone', 'edit an unreadable snippet');
    perform iamb_test.ok(r -> 'results' -> 1 -> 'row' = 'null'::jsonb, 'no row in the answer');
    -- A delete of a row the caller cannot read answers like one of an id the server does not
    -- know (security review 4.3, finding 10).
    perform iamb_test.eq(iamb_test.st(r, 3), 'unchanged', 'delete an unreadable snippet');
    -- A profile the caller has no grant in: the answers of a profile and a row that do not exist.
    perform iamb_test.eq(iamb_test.st(r, 4), 'rejected:parent_gone', 'folder in another''s profile');
    perform iamb_test.eq(iamb_test.st(r, 5), 'rejected:parent_gone', 'tag in another''s profile');
    perform iamb_test.eq(iamb_test.st(r, 6), 'unchanged', 'delete another''s folder');
    perform iamb_test.eq(iamb_test.st(r, 7), 'rejected:parent_gone', 'share another''s folder');
    perform iamb_test.ok((select deleted_at from iamb.snippet where id = (ids ->> 's1')::uuid) is null
                         and (select body from iamb.snippet where id = (ids ->> 's1')::uuid) = 'Open 9 to 5.',
                         'Ines''s snippet untouched');

    -- Stored files and the live channel.
    perform iamb_test.act('stranger');
    select count(*) into n from storage.objects;
    perform iamb_test.act_owner();
    perform iamb_test.eq(n, 0, 'stranger reads a stored file');
    if not iamb_test.realtime_ready() then
        raise warning 'live channel checks skipped: no partition of realtime.messages for today (no client has connected yet)';
        perform iamb_test.check_all();
        return;
    end if;
    perform set_config('realtime.topic', 'feed:' || iamb_test.uid('ines'), false);
    insert into realtime.messages (topic, extension, payload, event, private)
    values ('feed:' || iamb_test.uid('ines'), 'broadcast', '{}', 'head', true);
    perform iamb_test.act('stranger');
    select count(*) into n from realtime.messages;
    perform iamb_test.act_owner();
    perform iamb_test.eq(n, 0, 'stranger receives on another user''s topic');
    perform set_config('realtime.topic', 'feed:' || iamb_test.uid('stranger'), false);
    perform iamb_test.act('stranger');
    select count(*) into n from realtime.messages;
    perform iamb_test.act_owner();
    perform iamb_test.ok(n > 0, 'stranger receives on their own topic');
    -- G16: no client may send, on any topic, its own included.
    perform iamb_test.act('stranger');
    begin
        insert into realtime.messages (topic, extension, payload, event, private)
        values ('feed:' || iamb_test.uid('stranger'), 'broadcast', '{}', 'head', true);
        n := 1;
    exception when insufficient_privilege then n := 0;
    end;
    perform iamb_test.act_owner();
    perform iamb_test.eq(n, 0, 'a client sends on the live channel');
    perform iamb_test.check_all();
end $$;

-- A caller with no valid session reads nothing even of their own: a signed-out session.
do $$
declare
    n integer;
begin
    perform iamb_test.new_user('gone');
    perform iamb_test.hello('gone', 'Gone');
    delete from auth.sessions where id = (select session_id from iamb_test.actor where name = 'gone');
    perform iamb_test.act('gone');
    select count(*) into n from iamb.profile;
    perform iamb_test.act_owner();
    perform iamb_test.eq(n, 0, 'a session that no longer exists reads its own profile');
end $$;
