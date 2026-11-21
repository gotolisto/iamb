-- The plan's second test: a viewer cannot write. Tom may only use Ines's shared folder; every
-- write there and every account call his role does not allow is refused, judged at arrival
-- (§4.3, C12, G7, G8). He reads exactly the shared folder (§4.5).
do $$
declare
    ids jsonb := iamb_test.family('view');
    f1  uuid;
    s1  uuid;
    g   uuid;
    r   jsonb;
    sub uuid := iamb_test.id();
    t1  uuid := iamb_test.id();
    n   integer;
begin
    f1 := (ids ->> 'f1')::uuid; s1 := (ids ->> 's1')::uuid; g := (ids ->> 'g')::uuid;
    r := iamb_test.push('ines', jsonb_build_array(iamb_test.op_tag(t1, (ids ->> 'p_ines')::uuid, 'shop')));
    perform iamb_test.check_all();

    -- He reads exactly f1, s1, the share; not f2 or s2.
    perform iamb_test.act('tom');
    select count(*) into n from iamb.folder where profile_id = (ids ->> 'p_ines')::uuid;
    perform iamb_test.act_owner();
    perform iamb_test.eq(n, 1, 'folders of Ines that Tom reads');
    perform iamb_test.act('tom');
    select count(*) into n from iamb.snippet where profile_id = (ids ->> 'p_ines')::uuid;
    perform iamb_test.act_owner();
    perform iamb_test.eq(n, 1, 'snippets of Ines that Tom reads');
    perform iamb_test.act('tom');
    select count(*) into n from iamb.tag where profile_id = (ids ->> 'p_ines')::uuid;
    perform iamb_test.act_owner();
    perform iamb_test.eq(n, 0, 'a tag no visible snippet carries');

    r := iamb_test.push('tom', jsonb_build_array(
        iamb_test.op_snippet(iamb_test.id(), f1, ';new', 'new'),
        iamb_test.op_snippet(s1, f1, ';hrs', 'Open 9 to 6.', iamb_test.rev('snippet', s1)),
        iamb_test.op_delete('snippet', s1, iamb_test.rev('snippet', s1)),
        iamb_test.op_folder(sub, (ids ->> 'p_ines')::uuid, f1, 'Sub'),
        iamb_test.op_folder(f1, (ids ->> 'p_ines')::uuid, null, 'Renamed', iamb_test.rev('folder', f1)),
        iamb_test.op_delete('folder', f1, iamb_test.rev('folder', f1)),
        iamb_test.op_tag(iamb_test.id(), (ids ->> 'p_ines')::uuid, 'mine'),
        iamb_test.op_snippet_tag(iamb_test.id(), s1, t1),
        iamb_test.op_share(f1, g, 'edit', iamb_test.rev('folder_share', iamb_private.share_id(f1, g))),
        iamb_test.op_delete('folder_share', iamb_private.share_id(f1, g), 99)));
    perform iamb_test.eq(iamb_test.st(r, 1), 'rejected:forbidden', 'create a snippet');
    perform iamb_test.eq(iamb_test.st(r, 2), 'rejected:forbidden', 'edit a snippet');
    perform iamb_test.ok((r -> 'results' -> 1 -> 'row' ->> 'body') = 'Open 9 to 5.', 'the answer carries the server''s row');
    perform iamb_test.eq(iamb_test.st(r, 3), 'rejected:forbidden', 'delete a snippet');
    perform iamb_test.eq(iamb_test.st(r, 4), 'rejected:forbidden', 'create a sub-folder');
    perform iamb_test.eq(iamb_test.st(r, 5), 'rejected:forbidden', 'rename the shared folder');
    perform iamb_test.eq(iamb_test.st(r, 6), 'rejected:forbidden', 'delete the shared folder');
    perform iamb_test.eq(iamb_test.st(r, 7), 'rejected:forbidden', 'create a tag in the owner''s profile');
    perform iamb_test.eq(iamb_test.st(r, 8), 'rejected:parent_gone', 'link a tag he cannot read');
    perform iamb_test.eq(iamb_test.st(r, 9), 'rejected:forbidden', 'change the share');
    perform iamb_test.eq(iamb_test.st(r, 10), 'rejected:forbidden', 'end the share');
    perform iamb_test.eq((select body from iamb.snippet where id = s1), 'Open 9 to 5.', 'snippet unchanged');
    perform iamb_test.ok((select deleted_at from iamb.folder_share where id = iamb_private.share_id(f1, g)) is null, 'share live');
    perform iamb_test.eq((select count(*)::integer from iamb.folder where profile_id = (ids ->> 'p_ines')::uuid), 2, 'no new folder');

    -- Account calls a member may not make.
    r := iamb_test.call('tom', format('api.invite(iamb_test.dev(''tom''), 1, %L, %L, ''x@example.test'', ''member'')', iamb_test.id(), g));
    perform iamb_test.eq(r ->> 'error', 'forbidden', 'a member invites');
    r := iamb_test.call('tom', format('api.rename_group(iamb_test.dev(''tom''), 1, %L, ''Mine'')', g));
    perform iamb_test.eq(r ->> 'error', 'forbidden', 'a member renames the group');
    r := iamb_test.call('tom', format('api.remove_member(iamb_test.dev(''tom''), 1, %L, %L)', g, iamb_test.uid('ines')));
    perform iamb_test.eq(r ->> 'error', 'forbidden', 'a member removes the owner');
    r := iamb_test.call('tom', format('api.set_member_role(iamb_test.dev(''tom''), 1, %L, %L, ''admin'')', g, iamb_test.uid('tom')));
    perform iamb_test.eq(r ->> 'error', 'forbidden', 'a member makes himself admin');
    r := iamb_test.call('tom', format('api.delete_group(iamb_test.dev(''tom''), 1, %L)', g));
    perform iamb_test.eq(r ->> 'error', 'not_owner', 'a member deletes the group');
    r := iamb_test.call('tom', format('api.transfer_ownership(iamb_test.dev(''tom''), 1, %L, %L)', g, iamb_test.uid('tom')));
    perform iamb_test.eq(r ->> 'error', 'not_owner', 'a member offers the ownership');
    r := iamb_test.call('tom', format('api.members(iamb_test.dev(''tom''), 1, %L)', g));
    perform iamb_test.ok(r ->> 'error' is null and not exists (select 1 from jsonb_array_elements(r -> 'members') m
                                                               where m ->> 'email' is not null), 'a member sees no addresses');
    -- No direct write on any table.
    perform iamb_test.act('tom');
    begin
        update iamb.snippet set body = 'x' where id = s1;
        raise exception 'update allowed';
    exception when insufficient_privilege then null;
    end;
    perform iamb_test.act_owner();
    perform iamb_test.check_all();
end $$;
