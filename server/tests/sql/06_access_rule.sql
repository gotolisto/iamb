-- G1: `folder_access` equals the rule of §4.2 (the slow reference) after every commit, through
-- nested shares, group folders, member permissions, roles, moves across share edges, deletes,
-- restores, leaving and deleting groups; and the visibility entries of C21 follow it.
do $$
declare
    pi uuid; g uuid := iamb_test.id(); h uuid := iamb_test.id();
    a uuid := iamb_test.id(); a1 uuid := iamb_test.id(); a1a uuid := iamb_test.id(); a1a1 uuid := iamb_test.id();
    b uuid := iamb_test.id(); gf uuid := iamb_test.id(); gf1 uuid := iamb_test.id();
    s uuid := iamb_test.id();
    r jsonb; p jsonb;
    i1 uuid := iamb_test.id(); i2 uuid := iamb_test.id(); i3 uuid := iamb_test.id();
begin
    perform iamb_test.new_user('ines'); perform iamb_test.new_user('tom'); perform iamb_test.new_user('ana');
    perform iamb_test.hello('ines', 'Ines'); perform iamb_test.hello('tom', 'Tom'); perform iamb_test.hello('ana', 'Ana');
    pi := iamb_test.personal('ines');
    r := iamb_test.call('ines', format('api.create_group(iamb_test.dev(''ines''), 1, %L, ''family'', ''Fam'')', g));
    r := iamb_test.call('ines', format('api.create_group(iamb_test.dev(''ines''), 1, %L, ''company'', ''Co'')', h));
    r := iamb_test.call('ines', format('api.invite(iamb_test.dev(''ines''), 1, %L, %L, %L, ''member'')', i1, g, iamb_test.email('tom')));
    r := iamb_test.call('ines', format('api.invite(iamb_test.dev(''ines''), 1, %L, %L, %L, ''admin'')', i2, g, iamb_test.email('ana')));
    r := iamb_test.call('ines', format('api.invite(iamb_test.dev(''ines''), 1, %L, %L, %L, ''member'')', i3, h, iamb_test.email('tom')));
    perform iamb_test.call('tom', format('api.accept_invitation(iamb_test.dev(''tom''), 1, %L)', i1));
    perform iamb_test.call('ana', format('api.accept_invitation(iamb_test.dev(''ana''), 1, %L)', i2));
    r := iamb_test.call('tom', format('api.accept_invitation(iamb_test.dev(''tom''), 1, %L)', i3));
    perform iamb_test.ok(r ->> 'error' is null, 'accept ' || r::text);
    perform iamb_test.check_all();

    -- Ines's tree, four levels deep, and B; a group folder.
    r := iamb_test.push('ines', jsonb_build_array(
        iamb_test.op_folder(a, pi, null, 'A'), iamb_test.op_folder(a1, pi, a, 'A1'),
        iamb_test.op_folder(a1a, pi, a1, 'A1a'), iamb_test.op_folder(a1a1, pi, a1a, 'A1a1'),
        iamb_test.op_folder(b, pi, null, 'B'),
        iamb_test.op_snippet(s, a1a1, ';deep', 'deep'),
        iamb_test.op_folder(gf, g, null, 'Group folder', 0, jsonb_build_object('row', jsonb_build_object('member_permission', 'edit'))),
        iamb_test.op_folder(gf1, g, gf, 'Group sub')));
    perform iamb_test.ok(not exists (select 1 from jsonb_array_elements(r -> 'results') x where x ->> 'status' <> 'applied'), 'tree ' || r::text);
    perform iamb_test.check_all();
    perform iamb_test.eq(iamb_private.perm(iamb_test.uid('tom'), gf1), 'edit', 'member_permission edit reaches the whole group tree');
    perform iamb_test.eq(iamb_private.perm(iamb_test.uid('ana'), gf1), 'manage', 'an admin manages the group''s folders');

    -- Nested shares: A1 to the family (edit), A1a to the team (view).
    r := iamb_test.push('ines', jsonb_build_array(iamb_test.op_share(a1, g, 'edit'), iamb_test.op_share(a1a, h, 'view')));
    perform iamb_test.ok(iamb_test.st(r, 1) = 'applied' and iamb_test.st(r, 2) = 'applied', 'shares ' || r::text);
    perform iamb_test.check_all();
    perform iamb_test.eq(iamb_private.perm(iamb_test.uid('tom'), a1a1), 'edit', 'the highest permission wins');
    perform iamb_test.eq(iamb_private.access_root(iamb_test.uid('tom'), a1a1), a1, 'the access root is the highest readable folder');
    perform iamb_test.eq(iamb_private.perm(iamb_test.uid('tom'), a), null::text, 'not the folder above the share');
    p := iamb_test.pull('tom');
    perform iamb_test.ok(exists (select 1 from jsonb_array_elements(p -> 'entries') e where e ->> 'kind' = 'folder_visible' and e ->> 'folder' = a1::text)
                         and not exists (select 1 from jsonb_array_elements(p -> 'entries') e where e ->> 'kind' = 'folder_visible' and e ->> 'folder' = a1a::text),
                         'one folder_visible, for the top of the newly readable tree: ' || (p -> 'entries')::text);

    -- A1a moves out of A1 into B: Tom keeps it through the team (view), rooted at A1a; no revoke.
    r := iamb_test.push('ines', jsonb_build_array(iamb_test.op_folder(a1a, pi, b, 'A1a', iamb_test.rev('folder', a1a))));
    perform iamb_test.eq(iamb_test.st(r), 'applied', 'move');
    perform iamb_test.check_all();
    perform iamb_test.eq(iamb_private.perm(iamb_test.uid('tom'), a1a1), 'view', 'permission after the move');
    perform iamb_test.eq(iamb_private.access_root(iamb_test.uid('tom'), a1a1), a1a, 'root after the move');
    p := iamb_test.pull('tom');
    perform iamb_test.ok(not exists (select 1 from jsonb_array_elements(p -> 'entries') e where e ->> 'kind' = 'folder_revoked'),
                         'a folder still reachable through another grant is not revoked: ' || (p -> 'entries')::text);
    -- Ana (family only) loses A1a: folder_revoked for it, and its row as null.
    p := iamb_test.pull('ana');
    perform iamb_test.ok(exists (select 1 from jsonb_array_elements(p -> 'entries') e where e ->> 'kind' = 'folder_revoked' and e ->> 'folder' = a1a::text),
                         'ana: folder_revoked for the tree that left: ' || (p -> 'entries')::text);

    -- The team share ends: Tom loses A1a entirely.
    r := iamb_test.push('ines', jsonb_build_array(iamb_test.op_delete('folder_share', iamb_private.share_id(a1a, h))));
    perform iamb_test.check_all();
    perform iamb_test.eq(iamb_private.perm(iamb_test.uid('tom'), a1a1), null::text, 'after the share ended');

    -- Moved back under A1: the family reaches it again (folder_visible for the moved folder).
    r := iamb_test.push('ines', jsonb_build_array(iamb_test.op_folder(a1a, pi, a1, 'A1a', iamb_test.rev('folder', a1a))));
    perform iamb_test.check_all();
    p := iamb_test.pull('tom');
    perform iamb_test.ok(exists (select 1 from jsonb_array_elements(p -> 'entries') e where e ->> 'kind' = 'folder_visible' and e ->> 'folder' = a1a::text),
                         'folder_visible for a tree that came in by a move: ' || (p -> 'entries')::text);

    -- Roles: Tom becomes admin (manage on the group's folders), then member again.
    r := iamb_test.call('ines', format('api.set_member_role(iamb_test.dev(''ines''), 1, %L, %L, ''admin'')', g, iamb_test.uid('tom')));
    perform iamb_test.check_all();
    perform iamb_test.eq(iamb_private.perm(iamb_test.uid('tom'), gf1), 'manage', 'admin');
    r := iamb_test.call('ines', format('api.set_member_role(iamb_test.dev(''ines''), 1, %L, %L, ''member'')', g, iamb_test.uid('tom')));
    perform iamb_test.check_all();
    -- member_permission back to view (a change of the top-level folder by manage).
    r := iamb_test.push('ines', jsonb_build_array(iamb_test.op_folder(gf, g, null, 'Group folder', iamb_test.rev('folder', gf))));
    perform iamb_test.eq(iamb_test.st(r), 'applied', 'member_permission');
    perform iamb_test.check_all();
    perform iamb_test.eq(iamb_private.perm(iamb_test.uid('tom'), gf1), 'view', 'member_permission view');

    -- A deleted folder keeps the entries it would have if live; its restore keeps them.
    r := iamb_test.push('ines', jsonb_build_array(iamb_test.op_delete('folder', a1a, iamb_test.rev('folder', a1a))));
    perform iamb_test.eq(iamb_test.st(r), 'applied', 'delete A1a');
    perform iamb_test.check_all();
    perform iamb_test.eq(iamb_private.perm(iamb_test.uid('tom'), a1a1), 'edit', 'tombstones stay readable through the share above');
    r := iamb_test.push('tom', jsonb_build_array(iamb_test.op_snippet(iamb_test.id(), a1a1, ';back', 'back')));
    perform iamb_test.eq(iamb_test.st(r), 'applied', 'a write into a deleted folder restores it (C7)');
    perform iamb_test.check_all();
    perform iamb_test.ok((select deleted_at from iamb.folder where id = a1a) is null and (select deleted_at from iamb.folder where id = a1a1) is null,
                         'the folders above are live again');
    perform iamb_test.ok(exists (select 1 from iamb.conflict where row_id = a1a and kind = 'folder_restored'), 'folder_restored conflict');

    -- Leaving and deleting groups.
    r := iamb_test.call('tom', format('api.leave_group(iamb_test.dev(''tom''), 1, %L)', h));
    perform iamb_test.ok(r ->> 'error' is null, 'leave ' || r::text);
    perform iamb_test.check_all();
    r := iamb_test.call('ines', format('api.delete_group(iamb_test.dev(''ines''), 1, %L)', g));
    perform iamb_test.ok(r ->> 'error' is null, 'delete_group ' || r::text);
    perform iamb_test.check_all();
    perform iamb_test.eq((select count(*)::integer from iamb.folder_access where user_id in (iamb_test.uid('tom'), iamb_test.uid('ana'))
                          and profile_id in (pi, g)), 0, 'nothing left for the members');
    p := iamb_test.pull('tom');
    perform iamb_test.ok(exists (select 1 from jsonb_array_elements(p -> 'entries') e where e ->> 'kind' = 'folder_revoked' and e ->> 'folder' = gf::text)
                         and exists (select 1 from jsonb_array_elements(p -> 'entries') e where e ->> 'kind' = 'folder_revoked' and e ->> 'folder' = a1::text),
                         'folder_revoked for the group''s folders and for those shared with it: ' || (p -> 'entries')::text);
    perform iamb_test.eq(iamb_test.visible('tom', 'profile'), 1, 'only his own profile');
end $$;

-- A folder shared by itself inside a tree that is revoked stays readable and is listed in
-- `except` (C21, S65, S82).
do $$
declare
    pi uuid; g uuid := iamb_test.id(); h uuid := iamb_test.id();
    a uuid := iamb_test.id(); a1 uuid := iamb_test.id(); r jsonb; p jsonb; i1 uuid := iamb_test.id(); i2 uuid := iamb_test.id();
begin
    delete from iamb_test.actor;
    perform iamb_test.new_user('ines'); perform iamb_test.new_user('tom');
    perform iamb_test.hello('ines', 'Ines'); perform iamb_test.hello('tom', 'Tom');
    pi := iamb_test.personal('ines');
    perform iamb_test.call('ines', format('api.create_group(iamb_test.dev(''ines''), 1, %L, ''family'', ''Fam'')', g));
    perform iamb_test.call('ines', format('api.create_group(iamb_test.dev(''ines''), 1, %L, ''family'', ''Fam2'')', h));
    perform iamb_test.call('ines', format('api.invite(iamb_test.dev(''ines''), 1, %L, %L, %L, ''member'')', i1, g, iamb_test.email('tom')));
    perform iamb_test.call('ines', format('api.invite(iamb_test.dev(''ines''), 1, %L, %L, %L, ''member'')', i2, h, iamb_test.email('tom')));
    perform iamb_test.call('tom', format('api.accept_invitation(iamb_test.dev(''tom''), 1, %L)', i1));
    perform iamb_test.call('tom', format('api.accept_invitation(iamb_test.dev(''tom''), 1, %L)', i2));
    r := iamb_test.push('ines', jsonb_build_array(iamb_test.op_folder(a, pi, null, 'A'), iamb_test.op_folder(a1, pi, a, 'A1'),
                                                   iamb_test.op_share(a, g, 'view'), iamb_test.op_share(a1, h, 'view')));
    perform iamb_test.check_all();
    perform iamb_test.pull('tom');
    r := iamb_test.push('ines', jsonb_build_array(iamb_test.op_delete('folder_share', iamb_private.share_id(a, g))));
    perform iamb_test.check_all();
    p := iamb_test.pull('tom');
    perform iamb_test.ok(exists (select 1 from jsonb_array_elements(p -> 'entries') e
                                 where e ->> 'kind' = 'folder_revoked' and e ->> 'folder' = a::text and e -> 'except' = jsonb_build_array(a1)),
                         'except lists the part that stays: ' || (p -> 'entries')::text);
    perform iamb_test.eq(iamb_private.perm(iamb_test.uid('tom'), a1), 'view', 'A1 still readable');
end $$;
