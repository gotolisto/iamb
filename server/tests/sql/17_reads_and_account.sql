-- The reading functions' positive answers (§6.5): fetch_folder in pages, snapshot with
-- tombstones and memberships, rows, access, users, entitlements, devices; and the account calls
-- of §11.1 and §11.3: rename_device, set_display_name, delete_account_data.
do $$
declare
    ids jsonb := iamb_test.family('view');
    f1 uuid; s1 uuid; g uuid; pi uuid; r jsonb; ops jsonb; cur text; n integer := 0; pages integer := 0; i integer;
    del uuid := iamb_test.id();
begin
    f1 := (ids ->> 'f1')::uuid; s1 := (ids ->> 's1')::uuid; g := (ids ->> 'g')::uuid; pi := (ids ->> 'p_ines')::uuid;
    -- 600 more snippets in the shared folder, so that fetch_folder takes two pages.
    for i in 1..3 loop
        select jsonb_agg(iamb_test.op_snippet(gen_random_uuid(), f1, ';f' || i || '_' || x, 'x')) into ops from generate_series(1, 200) x;
        r := iamb_test.push('ines', ops);
    end loop;
    -- A row created and deleted again in one request (D1): the delete finds its profile in the put.
    r := iamb_test.push('ines', jsonb_build_array(iamb_test.op_snippet(del, f1, ';del', 'del'), iamb_test.op_delete('snippet', del, 0)));
    perform iamb_test.ok(iamb_test.st(r, 1) = 'applied' and iamb_test.st(r, 2) = 'applied', 'create and delete in one request: ' || r::text);
    loop
        r := iamb_test.call('tom', format('api.fetch_folder(iamb_test.dev(''tom''), 1, %L, %L)', f1, cur));
        perform iamb_test.ok(r ->> 'error' is null, 'fetch_folder: ' || left(r::text, 300));
        if pages = 0 then
            perform iamb_test.ok((r ->> 'head')::bigint > 0, 'the first page carries head');
            perform iamb_test.ok(r -> 'rows' -> 0 ->> 'entity' = 'folder' and r -> 'rows' -> 0 ->> 'id' = f1::text, 'folders first');
        else
            perform iamb_test.ok(r -> 'head' = 'null'::jsonb, 'later pages carry no head');
        end if;
        n := n + jsonb_array_length(r -> 'rows');
        pages := pages + 1;
        cur := r ->> 'cursor';
        exit when cur is null or pages > 5;
    end loop;
    -- f1, the share, the profile, s1 and 600 snippets; not the deleted one.
    perform iamb_test.eq(pages, 2, 'two pages');
    perform iamb_test.eq(n, 604, 'every live row of the tree once');
    -- rows(): readable rows, null for the rest, in the order asked.
    r := iamb_test.call('tom', format('api.rows(iamb_test.dev(''tom''), 1, %L::jsonb)', jsonb_build_array(
        jsonb_build_object('entity', 'snippet', 'id', s1), jsonb_build_object('entity', 'snippet', 'id', ids -> 's2'),
        jsonb_build_object('entity', 'snippet', 'id', del))));
    perform iamb_test.ok(r -> 'rows' -> 0 -> 'row' ->> 'body' = 'Open 9 to 5.' and r -> 'rows' -> 1 -> 'row' = 'null'::jsonb
                         and r -> 'rows' -> 2 -> 'row' ->> 'deleted_at' is not null, 'rows: ' || r::text);
    -- snapshot: everything readable, tombstones and memberships included, in pages.
    cur := null; n := 0; pages := 0;
    loop
        r := iamb_test.call('tom', format('api.snapshot(iamb_test.dev(''tom''), 1, %L, %L)', iamb_test.pos('tom'), cur));
        perform iamb_test.ok(r ->> 'error' is null, 'snapshot: ' || left(r::text, 300));
        if pages = 0 then
            update iamb_test.actor set position = r ->> 'start' where name = 'tom';
            perform iamb_test.ok(exists (select 1 from jsonb_array_elements(r -> 'rows') x where x ->> 'entity' = 'membership'), 'memberships');
        end if;
        n := n + (select count(*) from jsonb_array_elements(r -> 'rows') x where x ->> 'id' = del::text);
        pages := pages + 1;
        cur := r ->> 'cursor';
        exit when cur is null or pages > 5;
    end loop;
    perform iamb_test.eq(n, 1, 'the tombstone the server still holds');
    -- access(): the root outside the personal profile, with permission, via and granted_at.
    r := iamb_test.call('tom', 'api.access(iamb_test.dev(''tom''), 1)');
    perform iamb_test.ok(jsonb_array_length(r -> 'roots') = 1 and r -> 'roots' -> 0 ->> 'root' = f1::text
                         and r -> 'roots' -> 0 ->> 'permission' = 'view' and r -> 'roots' -> 0 ->> 'via' = 'share'
                         and r -> 'groups' -> 0 ->> 'role' = 'member', 'access: ' || r::text);
    -- users(): display names of people in a group with the caller; nobody else's.
    r := iamb_test.call('tom', format('api.users(iamb_test.dev(''tom''), 1, array[%L, %L]::uuid[])', iamb_test.uid('ines'), iamb_test.uid('stranger')));
    perform iamb_test.ok(jsonb_array_length(r -> 'users') = 1 and r -> 'users' -> 0 ->> 'display_name' = 'Ines Marchetti', 'users: ' || r::text);
    -- entitlements(): the user plan's names, and the group's for its owner.
    r := iamb_test.call('ines', 'api.entitlements(iamb_test.dev(''ines''), 1)');
    perform iamb_test.ok(r -> 'entitlements' ->> 'devices_max' = '5' and r -> 'entitlements' ->> 'sync' = 'true'
                         and r -> 'entitlements' ->> ('members_max@' || g) = '10', 'entitlements: ' || r::text);
    r := iamb_test.call('tom', 'api.entitlements(iamb_test.dev(''tom''), 1)');
    perform iamb_test.ok(not (r -> 'entitlements' ? ('members_max@' || g)), 'a member gets no group entitlements');
    -- devices(), rename_device(), set_display_name().
    r := iamb_test.call('tom', 'api.rename_device(iamb_test.dev(''tom''), 1, iamb_test.dev(''tom''), ''Laptop'')');
    perform iamb_test.ok(r ->> 'error' is null, 'rename_device');
    r := iamb_test.call('tom', 'api.rename_device(iamb_test.dev(''tom''), 1, iamb_test.dev(''ines''), ''Mine'')');
    perform iamb_test.eq(r ->> 'error', 'not_found', 'another account''s device');
    r := iamb_test.call('tom', 'api.devices(iamb_test.dev(''tom''), 1)');
    perform iamb_test.ok(jsonb_array_length(r -> 'devices') = 1 and r -> 'devices' -> 0 ->> 'name' = 'Laptop'
                         and (r -> 'devices' -> 0 ->> 'current')::boolean, 'devices: ' || r::text);
    r := iamb_test.call('tom', 'api.set_display_name(iamb_test.dev(''tom''), 1, ''   '')');
    perform iamb_test.eq(r ->> 'error', 'invalid', 'an empty display name');
    perform iamb_test.check_all();
end $$;

-- Deleting an account (§11.1): refused while the user owns a group with other members; otherwise
-- every device is revoked, the groups the user owns are deleted, the memberships end and the
-- personal rows go at once.
do $$
declare
    ids jsonb := iamb_test.family('edit');
    r jsonb; pt uuid; tf uuid := iamb_test.id(); tg uuid := iamb_test.id(); u_ines uuid; u_tom uuid;
begin
    pt := (ids ->> 'p_tom')::uuid; u_ines := iamb_test.uid('ines'); u_tom := iamb_test.uid('tom');
    r := iamb_test.push('tom', jsonb_build_array(iamb_test.op_folder(tf, pt, null, 'Tom''s'), iamb_test.op_snippet(iamb_test.id(), tf, ';t', 't')));
    perform iamb_test.call('tom', format('api.create_group(iamb_test.dev(''tom''), 1, %L, ''family'', ''Toms'')', tg));
    perform set_config('role', 'service_role', false);
    r := api_service.delete_account_data(u_ines);
    perform iamb_test.act_owner();
    perform iamb_test.eq(r ->> 'error', 'owns_group', 'refused while the user owns a group with members');
    perform set_config('role', 'service_role', false);
    r := api_service.delete_account_data(u_tom);
    perform iamb_test.act_owner();
    perform iamb_test.ok(r ->> 'error' is null, 'delete_account_data: ' || r::text);
    perform iamb_test.ok(not exists (select 1 from iamb.device where user_id = iamb_test.uid('tom') and revoked_at is null), 'devices revoked');
    perform iamb_test.ok(not exists (select 1 from iamb.profile where id = pt), 'personal profile gone');
    perform iamb_test.ok(not exists (select 1 from iamb.folder where id = tf) and not exists (select 1 from iamb.snippet where profile_id = pt),
                         'personal rows gone at once');
    perform iamb_test.ok((select deleted_at from iamb.profile where id = tg) is not null, 'the group he owned is deleted');
    perform iamb_test.ok(not exists (select 1 from iamb.membership where user_id = iamb_test.uid('tom') and deleted_at is null), 'memberships ended');
    perform iamb_test.ok(not exists (select 1 from iamb.user_profile where user_id = iamb_test.uid('tom')), 'display name gone');
    r := iamb_test.call('tom', 'api.pull(iamb_test.dev(''tom''), 1, iamb_test.pos(''tom''))');
    perform iamb_test.eq(r ->> 'error', 'device_revoked', 'his tokens are refused at once');
    perform iamb_test.check_access();
    perform iamb_test.check_tree();
end $$;
