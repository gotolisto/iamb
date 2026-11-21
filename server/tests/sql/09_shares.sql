-- C11 and G12: sharing never wins by being late; who may share; the share's id; the plan's limit.
-- G7: a member with edit cannot rename, move or delete the root of his share at any depth.
do $$
declare
    ids jsonb := iamb_test.family('edit');
    f1 uuid; f2 uuid; g uuid; pi uuid; sid uuid; r jsonb; base bigint; ga1 bigint; ga2 bigint;
    t uuid := iamb_test.id(); inv uuid := iamb_test.id(); tf uuid := iamb_test.id(); gf uuid := iamb_test.id();
    lim jsonb;
begin
    f1 := (ids ->> 'f1')::uuid; f2 := (ids ->> 'f2')::uuid; g := (ids ->> 'g')::uuid; pi := (ids ->> 'p_ines')::uuid;
    sid := iamb_private.share_id(f1, g);
    base := iamb_test.rev('folder_share', sid);
    ga1 := (select granted_at from iamb.folder_access where user_id = iamb_test.uid('tom') and folder_id = f1);
    -- A device that saw the current revision changes the permission; a stale one does not win.
    r := iamb_test.push('ines', jsonb_build_array(iamb_test.op_share(f1, g, 'view', base)));
    perform iamb_test.eq(iamb_test.st(r), 'applied', 'based on the current revision');
    r := iamb_test.push('ines', jsonb_build_array(iamb_test.op_share(f1, g, 'edit', base)));
    perform iamb_test.ok(iamb_test.st(r) = 'superseded' and r -> 'results' -> 0 -> 'row' ->> 'permission' = 'view',
                         'a stale put is superseded with the server''s row: ' || r::text);
    -- A delete always applies; a stale put cannot bring it back; one based on the tombstone can,
    -- and the access that begins is a new one with a new granted_at.
    r := iamb_test.push('ines', jsonb_build_array(iamb_test.op_delete('folder_share', sid, 1)));
    perform iamb_test.eq(iamb_test.st(r), 'applied', 'delete');
    r := iamb_test.push('ines', jsonb_build_array(iamb_test.op_share(f1, g, 'edit', base)));
    perform iamb_test.eq(iamb_test.st(r), 'superseded', 'stale put on the tombstone');
    perform iamb_test.ok(not exists (select 1 from iamb.folder_access where user_id = iamb_test.uid('tom') and folder_id = f1), 'no access');
    perform pg_sleep(0.01);
    r := iamb_test.push('ines', jsonb_build_array(iamb_test.op_share(f1, g, 'edit', iamb_test.rev('folder_share', sid))));
    perform iamb_test.eq(iamb_test.st(r), 'applied', 'shared again');
    ga2 := (select granted_at from iamb.folder_access where user_id = iamb_test.uid('tom') and folder_id = f1);
    perform iamb_test.ok(ga2 > ga1, 'a new granted_at');
    -- The id must be the derived one; the folder must be personal; the writer a member.
    r := iamb_test.push('ines', jsonb_build_array(
        iamb_test.op('folder_share', iamb_test.id(), jsonb_build_object('folder_id', f2, 'profile_id', g, 'permission', 'view'))));
    perform iamb_test.eq(iamb_test.st(r), 'blocked:invalid', 'an id that is not derived');
    r := iamb_test.push('ines', jsonb_build_array(iamb_test.op_folder(gf, g, null, 'Group folder'), iamb_test.op_share(gf, g, 'view')));
    perform iamb_test.eq(iamb_test.st(r, 2), 'rejected:forbidden', 'a group''s folder is not shared');
    r := iamb_test.push('tom', jsonb_build_array(iamb_test.op_folder(tf, iamb_test.personal('tom'), null, 'Tom''s'),
                                                  iamb_test.op_share(tf, g, 'view')));
    perform iamb_test.eq(iamb_test.st(r, 2), 'applied', 'in a family every member may share');
    -- In a team, a member may not (open question 7).
    perform iamb_test.call('ines', format('api.create_group(iamb_test.dev(''ines''), 1, %L, ''company'', ''Team'')', t));
    perform iamb_test.call('ines', format('api.invite(iamb_test.dev(''ines''), 1, %L, %L, %L, ''member'')', inv, t, iamb_test.email('tom')));
    perform iamb_test.call('tom', format('api.accept_invitation(iamb_test.dev(''tom''), 1, %L)', inv));
    r := iamb_test.push('tom', jsonb_build_array(iamb_test.op_share(tf, t, 'view')));
    perform iamb_test.eq(iamb_test.st(r), 'rejected:forbidden', 'a team member shares');
    r := iamb_test.push('stranger', jsonb_build_array(iamb_test.op_folder(iamb_test.id(), iamb_test.personal('stranger'), null, 'S')));
    r := iamb_test.push('stranger', jsonb_build_array(iamb_test.op_share(
        (select id from iamb.folder where profile_id = iamb_test.personal('stranger') limit 1), g, 'view')));
    perform iamb_test.eq(iamb_test.st(r), 'rejected:forbidden', 'a share into a group one is not in');
    -- shared_folders_max counts folders, not shares.
    select limits into lim from iamb.plan where id = 'default-user';
    update iamb.plan set limits = limits || '{"shared_folders_max": 1}' where id = 'default-user';
    r := iamb_test.push('ines', jsonb_build_array(iamb_test.op_share(f2, g, 'view'), iamb_test.op_share(f1, t, 'view')));
    perform iamb_test.eq(iamb_test.st(r, 1), 'rejected:plan_limit', 'a second shared folder');
    perform iamb_test.eq(iamb_test.st(r, 2), 'applied', 'a second share of the same folder');
    update iamb.plan set limits = lim where id = 'default-user';
    perform iamb_test.check_all();
end $$;

-- G7 at depth: f1 / a / b, with b shared to the family; Tom (edit) may work inside b and nowhere
-- above, and may not touch b itself.
do $$
declare
    pi uuid; g uuid := iamb_test.id(); a uuid := iamb_test.id(); b uuid := iamb_test.id(); c uuid := iamb_test.id();
    d uuid := iamb_test.id(); o uuid := iamb_test.id(); s uuid := iamb_test.id(); inv uuid := iamb_test.id(); r jsonb;
begin
    delete from iamb_test.actor;
    perform iamb_test.new_user('ines'); perform iamb_test.new_user('tom');
    perform iamb_test.hello('ines', 'Ines'); perform iamb_test.hello('tom', 'Tom');
    pi := iamb_test.personal('ines');
    perform iamb_test.call('ines', format('api.create_group(iamb_test.dev(''ines''), 1, %L, ''family'', ''Fam'')', g));
    perform iamb_test.call('ines', format('api.invite(iamb_test.dev(''ines''), 1, %L, %L, %L, ''member'')', inv, g, iamb_test.email('tom')));
    perform iamb_test.call('tom', format('api.accept_invitation(iamb_test.dev(''tom''), 1, %L)', inv));
    r := iamb_test.push('ines', jsonb_build_array(iamb_test.op_folder(a, pi, null, 'A'), iamb_test.op_folder(b, pi, a, 'B'),
        iamb_test.op_folder(o, pi, null, 'Other'), iamb_test.op_share(b, g, 'edit')));
    -- Inside: create, rename and move within the root are allowed.
    r := iamb_test.push('tom', jsonb_build_array(iamb_test.op_folder(c, pi, b, 'C'), iamb_test.op_folder(d, pi, b, 'D'),
                                                  iamb_test.op_snippet(s, c, ';s', 's')));
    perform iamb_test.ok(iamb_test.st(r, 1) = 'applied' and iamb_test.st(r, 3) = 'applied', 'inside: ' || r::text);
    r := iamb_test.push('tom', jsonb_build_array(iamb_test.op_folder(c, pi, d, 'C moved', iamb_test.rev('folder', c)),
                                                  iamb_test.op_snippet(s, d, ';s', 's', iamb_test.rev('snippet', s))));
    perform iamb_test.ok(iamb_test.st(r, 1) = 'applied' and iamb_test.st(r, 2) = 'applied', 'moves within the root: ' || r::text);
    -- The root itself: rename, move, delete are refused.
    r := iamb_test.push('tom', jsonb_build_array(
        -- His device holds b with an empty parent, which he cannot read (§6.4).
        iamb_test.op_folder(b, pi, null, 'B renamed', iamb_test.rev('folder', b)),
        iamb_test.op_folder(b, pi, d, 'B', iamb_test.rev('folder', b)),
        iamb_test.op_delete('folder', b, iamb_test.rev('folder', b)),
        iamb_test.op_folder(c, pi, a, 'C', iamb_test.rev('folder', c)),
        iamb_test.op_folder(c, pi, o, 'C', iamb_test.rev('folder', c))));
    perform iamb_test.eq(iamb_test.st(r, 1), 'rejected:forbidden', 'rename the root of the share');
    perform iamb_test.eq(iamb_test.st(r, 2), 'rejected:forbidden', 'move the root of the share');
    perform iamb_test.eq(iamb_test.st(r, 3), 'rejected:forbidden', 'delete the root of the share');
    perform iamb_test.eq(iamb_test.st(r, 4), 'rejected:parent_gone', 'move out of the access root, to a folder he cannot read');
    perform iamb_test.eq(iamb_test.st(r, 5), 'rejected:parent_gone', 'move to another folder of the owner');
    perform iamb_test.check_all();
end $$;
