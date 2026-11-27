-- The plan's third test: a removed member loses access, including through the feed, the
-- history, conflicts, attachments and the live channel, and including the rows he wrote there
-- (§4.5 last lines, G3, G5, G13, G14, G16; S30). The same for a share that ends.
do $$
declare
    ids  jsonb := iamb_test.family('edit');
    f1   uuid; s1 uuid; g uuid; p_ines uuid;
    s3   uuid := iamb_test.id();
    t3   uuid := iamb_test.id();
    a3   uuid := iamb_test.id();
    l3   uuid := iamb_test.id();
    sub  uuid := iamb_test.id();
    sha  text := repeat('cd', 32);
    r    jsonb;
    p    jsonb;
    cid  uuid;
    n    integer;
    t    text;
    msgs integer;
begin
    f1 := (ids ->> 'f1')::uuid; s1 := (ids ->> 's1')::uuid; g := (ids ->> 'g')::uuid; p_ines := (ids ->> 'p_ines')::uuid;
    -- Ines's verified file, which Tom may read through s1's link.
    insert into iamb.attachment_blob values (p_ines, sha, 10, 'image/png', 1);
    insert into storage.objects (bucket_id, name) values ('attachments', p_ines || '/' || sha);
    r := iamb_test.push('ines', jsonb_build_array(iamb_test.op_attachment(a3, p_ines, sha),
                                                   iamb_test.op_snippet_attachment(l3, s1, a3)));
    perform iamb_test.ok(iamb_test.st(r, 2) = 'applied', 'ines links ' || r::text);
    -- Tom writes there: a snippet, a sub-folder, a tag; and a conflict with Ines that he is party to.
    perform iamb_test.pull('tom');
    r := iamb_test.push('tom', jsonb_build_array(
        iamb_test.op_snippet(s3, f1, ';tom', 'Tom''s text'),
        iamb_test.op_folder(sub, p_ines, f1, 'Tom''s folder'),
        iamb_test.op_tag(t3, p_ines, 'toms')));
    perform iamb_test.ok(iamb_test.st(r, 1) = 'applied' and iamb_test.st(r, 2) = 'applied' and iamb_test.st(r, 3) = 'applied',
                         'tom writes ' || r::text);
    r := iamb_test.push('ines', jsonb_build_array(iamb_test.op_snippet(s1, f1, ';hrs', 'Open 9 to 6.', iamb_test.rev('snippet', s1))));
    r := iamb_test.push('tom', jsonb_build_array(iamb_test.op_snippet(s1, f1, ';hrs', 'Open 8 to 5.', iamb_test.rev('snippet', s1) - 1)));
    cid := (r -> 'results' -> 0 ->> 'conflict')::uuid;
    perform iamb_test.ok(cid is not null, 'a conflict between Tom and Ines: ' || r::text);
    perform iamb_test.check_all();
    -- Before: Tom reads all of it.
    perform iamb_test.ok(iamb_test.visible('tom', 'tag') >= 1 and iamb_test.visible('tom', 'attachment_blob') = 1, 'tom reads before');
    perform iamb_test.act('tom');
    select count(*) into n from storage.objects;
    perform iamb_test.act_owner();
    perform iamb_test.eq(n, 1, 'tom reads the file before');
    r := iamb_test.call('tom', format('api.get_conflict(iamb_test.dev(''tom''), 1, %L)', cid));
    perform iamb_test.ok(r ->> 'error' is null and jsonb_array_length(r -> 'versions') = 1, 'tom reads the conflict before: ' || r::text);
    perform iamb_test.pull('tom');

    -- Ines removes Tom.
    r := iamb_test.call('ines', format('api.remove_member(iamb_test.dev(''ines''), 1, %L, %L)', g, iamb_test.uid('tom')));
    perform iamb_test.ok(r ->> 'error' is null, 'remove_member ' || r::text);
    perform iamb_test.check_all();
    -- G13: the same transaction removed his access entries.
    perform iamb_test.eq((select count(*)::integer from iamb.folder_access where user_id = iamb_test.uid('tom')
                          and profile_id <> (ids ->> 'p_tom')::uuid), 0, 'tom''s access entries');

    -- Every table: nothing of Ines's profile or the group's, including what Tom wrote.
    foreach t in array array['folder', 'folder_share', 'snippet', 'tag', 'snippet_tag', 'attachment',
                             'snippet_attachment', 'attachment_blob'] loop
        perform iamb_test.act('tom');
        execute format('select count(*) from iamb.%I x where %s', t, case
            when t in ('folder', 'snippet', 'tag', 'attachment', 'attachment_blob') then 'x.profile_id = ' || quote_literal(p_ines)
            else 'true' end) into n;
        perform iamb_test.act_owner();
        perform iamb_test.eq(n, 0, 'removed member reads ' || t);
    end loop;
    perform iamb_test.act('tom');
    select count(*) into n from iamb.profile where id in (p_ines, g);
    perform iamb_test.act_owner();
    perform iamb_test.eq(n, 0, 'removed member reads the profiles');
    perform iamb_test.act('tom');
    select count(*) into n from iamb.membership where profile_id = g and user_id <> auth.uid();
    perform iamb_test.act_owner();
    perform iamb_test.eq(n, 0, 'removed member reads the other memberships');
    perform iamb_test.act('tom');
    select count(*) into n from storage.objects;
    perform iamb_test.act_owner();
    perform iamb_test.eq(n, 0, 'removed member reads the file');

    -- The feed: folder_revoked for f1, rows delivered as null.
    p := iamb_test.pull('tom');
    perform iamb_test.ok(exists (select 1 from jsonb_array_elements(p -> 'entries') e
                                 where e ->> 'kind' = 'folder_revoked' and e ->> 'folder' = f1::text),
                         'folder_revoked in tom''s feed: ' || p::text);
    perform iamb_test.ok(not exists (select 1 from jsonb_array_elements(p -> 'entries') e
                                     where e ->> 'kind' = 'row' and jsonb_typeof(e -> 'row') = 'object'
                                       and e ->> 'entity' in ('folder', 'snippet', 'tag', 'attachment', 'folder_share')
                                       and (e -> 'row' ->> 'profile_id' = p_ines::text or e -> 'row' ->> 'folder_id' = f1::text)),
                         'no readable row of Ines in the page');
    perform iamb_test.ok(not exists (select 1 from jsonb_array_elements(p -> 'refs') e
                                     where e -> 'row' ->> 'profile_id' = p_ines::text or e ->> 'id' in (p_ines::text, g::text)),
                         'no readable reference of Ines in the page');
    -- The whole reads.
    r := iamb_test.call('tom', format('api.rows(iamb_test.dev(''tom''), 1, %L::jsonb)', jsonb_build_array(
        jsonb_build_object('entity', 'snippet', 'id', s3), jsonb_build_object('entity', 'folder', 'id', sub),
        jsonb_build_object('entity', 'tag', 'id', t3), jsonb_build_object('entity', 'snippet', 'id', s1),
        jsonb_build_object('entity', 'attachment', 'id', a3))));
    perform iamb_test.ok(not exists (select 1 from jsonb_array_elements(r -> 'rows') x where jsonb_typeof(x -> 'row') = 'object'),
                         'rows(): not even his own: ' || r::text);
    r := iamb_test.call('tom', format('api.fetch_folder(iamb_test.dev(''tom''), 1, %L)', f1));
    perform iamb_test.ok(jsonb_array_length(coalesce(r -> 'rows', '[]')) = 0, 'fetch_folder: ' || r::text);
    r := iamb_test.call('tom', 'api.snapshot(iamb_test.dev(''tom''), 1, iamb_test.pos(''tom''), null)');
    perform iamb_test.ok(not exists (select 1 from jsonb_array_elements(r -> 'rows') x
                                     where x -> 'row' ->> 'profile_id' = p_ines::text or x ->> 'id' in (p_ines::text, g::text)
                                        or x -> 'row' ->> 'folder_id' = f1::text),
                         'snapshot: ' || r::text);
    r := iamb_test.call('tom', 'api.visible_folders(iamb_test.dev(''tom''), 1)');
    perform iamb_test.eq(jsonb_array_length(r -> 'folders'), 0, 'visible_folders after removal');
    -- History and conflicts: a conflict on a row he may no longer read is, for him, not there
    -- (G5, G13; security review 4.3, finding 2).
    r := iamb_test.call('tom', format('api.get_conflict(iamb_test.dev(''tom''), 1, %L)', cid));
    perform iamb_test.eq(r ->> 'error', 'not_found', 'get_conflict after removal');
    perform iamb_test.ok(r -> 'versions' is null, 'no version in the answer');
    perform iamb_test.eq(iamb_test.visible('tom', 'history'), -1, 'history is unreadable');
    -- Writes are refused.
    r := iamb_test.push('tom', jsonb_build_array(
        iamb_test.op_snippet(s3, f1, ';tom', 'Again', iamb_test.rev('snippet', s3)),
        iamb_test.op_snippet(iamb_test.id(), f1, ';n', 'n')));
    -- C12, C13: his folder is no longer readable for him.
    perform iamb_test.eq(iamb_test.st(r, 1), 'rejected:parent_gone', 'his own snippet');
    perform iamb_test.eq(iamb_test.st(r, 2), 'rejected:parent_gone', 'a new snippet');
    -- The live channel: Ines's next change pokes nobody but those who can read it.
    select count(*) into msgs from realtime.messages where topic = 'feed:' || iamb_test.uid('tom');
    r := iamb_test.push('ines', jsonb_build_array(iamb_test.op_snippet(s1, f1, ';hrs', 'Open 10 to 5.', iamb_test.rev('snippet', s1))));
    perform iamb_test.ok(iamb_test.st(r) like 'applied%', 'ines edits ' || r::text);
    perform iamb_test.eq((select count(*)::integer from realtime.messages where topic = 'feed:' || iamb_test.uid('tom')), msgs,
                         'no poke for the removed member');
    if iamb_test.realtime_ready() then
        perform iamb_test.ok((select count(*) from realtime.messages where topic = 'feed:' || iamb_test.uid('ines')) > 0, 'ines is poked');
    else
        raise warning 'poke check skipped: no partition of realtime.messages for today';
    end if;
    p := iamb_test.pull('tom');
    perform iamb_test.eq(jsonb_array_length(p -> 'entries'), 0, 'nothing new in his feed');
    perform iamb_test.check_all();
end $$;

-- The same when the share ends (G13) and when the shared folder is deleted (C21: the people the
-- share served get folder_revoked, not tombstones).
do $$
declare
    ids jsonb := iamb_test.family('edit');
    f1 uuid; g uuid; r jsonb; p jsonb;
begin
    f1 := (ids ->> 'f1')::uuid; g := (ids ->> 'g')::uuid;
    perform iamb_test.pull('tom');
    r := iamb_test.push('ines', jsonb_build_array(iamb_test.op_delete('folder_share', iamb_private.share_id(f1, g), 0)));
    perform iamb_test.eq(iamb_test.st(r), 'applied', 'a share''s delete applies whatever its base');
    perform iamb_test.check_all();
    perform iamb_test.eq(iamb_test.visible('tom', 'snippet'), 0, 'tom reads nothing after the share ended');
    p := iamb_test.pull('tom');
    perform iamb_test.ok(exists (select 1 from jsonb_array_elements(p -> 'entries') e
                                 where e ->> 'kind' = 'folder_revoked' and e ->> 'folder' = f1::text), 'folder_revoked: ' || p::text);
    -- Shared again, then the folder deleted.
    r := iamb_test.push('ines', jsonb_build_array(iamb_test.op_share(f1, g, 'view', iamb_test.rev('folder_share', iamb_private.share_id(f1, g)))));
    perform iamb_test.eq(iamb_test.st(r), 'applied', 'shared again');
    perform iamb_test.ok(iamb_test.visible('tom', 'snippet') = 1, 'tom reads again');
    p := iamb_test.pull('tom');
    perform iamb_test.ok(exists (select 1 from jsonb_array_elements(p -> 'entries') e
                                 where e ->> 'kind' = 'folder_visible' and e ->> 'folder' = f1::text), 'folder_visible: ' || p::text);
    r := iamb_test.push('ines', jsonb_build_array(iamb_test.op_delete('folder', f1, iamb_test.rev('folder', f1),
        jsonb_build_object('cascade', jsonb_build_array(jsonb_build_object('entity', 'snippet', 'id', ids -> 's1',
                                                                           'base_rev', iamb_test.rev('snippet', (ids ->> 's1')::uuid)))))));
    perform iamb_test.eq(iamb_test.st(r), 'applied', 'folder deleted');
    perform iamb_test.check_all();
    perform iamb_test.eq(iamb_test.visible('tom', 'folder') - (select count(*)::integer from iamb.folder where profile_id = (ids ->> 'p_tom')::uuid), 0,
                         'tom reads no tombstone of the deleted shared folder');
    perform iamb_test.ok(iamb_test.visible('ines', 'folder') >= 2, 'ines reads her tombstone');
    p := iamb_test.pull('tom');
    perform iamb_test.ok(exists (select 1 from jsonb_array_elements(p -> 'entries') e
                                 where e ->> 'kind' = 'folder_revoked' and e ->> 'folder' = f1::text), 'folder_revoked on delete: ' || p::text);
end $$;
