-- C6: a folder's delete takes the subtree as the server has it, in bounded parts; answers every
-- row of its list; records deleted_with_folder; and a share is ended only by who may end it (G7,
-- G18, S51, S52, S76).
do $$
declare
    pi uuid; f uuid := iamb_test.id(); g uuid := iamb_test.id(); h uuid := iamb_test.id(); o uuid := iamb_test.id();
    s1 uuid := iamb_test.id(); s2 uuid := iamb_test.id(); s3 uuid := iamb_test.id(); s4 uuid := iamb_test.id();
    s5 uuid := iamb_test.id(); s1_base bigint;
    r jsonb; del jsonb; e jsonb;
begin
    perform iamb_test.new_user('ines'); perform iamb_test.new_device('ines_b', 'ines');
    perform iamb_test.hello('ines', 'Ines'); perform iamb_test.hello('ines_b');
    pi := iamb_test.personal('ines');
    update iamb.server_state set cascade_max = 3 where id;
    r := iamb_test.push('ines', jsonb_build_array(
        iamb_test.op_folder(f, pi, null, 'F'), iamb_test.op_folder(g, pi, f, 'G'), iamb_test.op_folder(h, pi, g, 'H'),
        iamb_test.op_folder(o, pi, null, 'Other'),
        iamb_test.op_snippet(s1, f, ';1', '1'), iamb_test.op_snippet(s2, g, ';2', '2'),
        iamb_test.op_snippet(s3, h, ';3', '3'), iamb_test.op_snippet(s4, h, ';4', '4'), iamb_test.op_snippet(s5, f, ';5', '5')));
    s1_base := iamb_test.rev('snippet', s1);
    -- B, meanwhile: edits s1, moves s5 out of F, and creates s6 in G (which A does not know of).
    r := iamb_test.push('ines_b', jsonb_build_array(
        iamb_test.op_snippet(s1, f, ';1', 'one', s1_base),
        iamb_test.op_snippet(s5, o, ';5', '5', iamb_test.rev('snippet', s5)),
        iamb_test.op_snippet(iamb_test.id(), g, ';6', '6')));
    -- A deletes F, naming what it deleted with it, with the bases it had.
    del := iamb_test.op_delete('folder', f, iamb_test.rev('folder', f), jsonb_build_object('cascade', jsonb_build_array(
        jsonb_build_object('entity', 'folder', 'id', g, 'base_rev', iamb_test.rev('folder', g)),
        jsonb_build_object('entity', 'folder', 'id', h, 'base_rev', iamb_test.rev('folder', h)),
        jsonb_build_object('entity', 'snippet', 'id', s1, 'base_rev', s1_base),
        jsonb_build_object('entity', 'snippet', 'id', s2, 'base_rev', iamb_test.rev('snippet', s2)),
        jsonb_build_object('entity', 'snippet', 'id', s3, 'base_rev', iamb_test.rev('snippet', s3)),
        jsonb_build_object('entity', 'snippet', 'id', s4, 'base_rev', iamb_test.rev('snippet', s4)),
        jsonb_build_object('entity', 'snippet', 'id', s5, 'base_rev', 0))));
    -- Seven rows below F (G, H, s1, s2, s3, s4, s6): three per request.
    r := iamb_test.push('ines', jsonb_build_array(del));
    perform iamb_test.ok(iamb_test.st(r) = 'partial' and r -> 'results' -> 0 -> 'cascade' is null, 'first part: ' || r::text);
    perform iamb_test.eq((select count(*)::integer from iamb.snippet where deleted_at is not null), 3, 'three rows deleted, deepest first');
    perform iamb_test.ok((select deleted_at from iamb.folder where id = f) is null, 'the folder stays live meanwhile');
    perform iamb_test.check_tree();
    perform iamb_test.check_access();
    perform iamb_test.eq((select count(*)::integer from iamb.push_receipt where op_id = (del ->> 'op_id')::uuid), 0, 'partial is not kept');
    r := iamb_test.push('ines', jsonb_build_array(del));
    perform iamb_test.eq(iamb_test.st(r), 'partial', 'second part');
    perform iamb_test.check_tree();
    r := iamb_test.push('ines', jsonb_build_array(del));
    perform iamb_test.eq(iamb_test.st(r), 'applied', 'the last part deletes the folder');
    perform iamb_test.check_all();
    perform iamb_test.ok((select deleted_at from iamb.folder where id = f) is not null, 'F deleted');
    -- Every row of the list is answered: deleted with it; s5 live because it had left the folder,
    -- with its row since the device does not hold that revision.
    perform iamb_test.eq(jsonb_array_length(r -> 'results' -> 0 -> 'cascade'), 7, 'an entry per listed row');
    select x into e from jsonb_array_elements(r -> 'results' -> 0 -> 'cascade') x where x ->> 'id' = s5::text;
    perform iamb_test.ok((e ->> 'deleted')::boolean = false and e -> 'row' ->> 'folder_id' = o::text, 's5 stays live: ' || e::text);
    select x into e from jsonb_array_elements(r -> 'results' -> 0 -> 'cascade') x where x ->> 'id' = s2::text;
    perform iamb_test.ok((e ->> 'deleted')::boolean and not (e ? 'row'), 's2 deleted, short form: ' || e::text);
    perform iamb_test.ok((select deleted_at from iamb.snippet where id = s5) is null, 's5 live on the server');
    -- deleted_with_folder: s1 (B changed it after A's base) and s6 (B wrote it, not in the list).
    perform iamb_test.ok(exists (select 1 from iamb.conflict where row_id = s1 and kind = 'deleted_with_folder'), 's1 conflict');
    perform iamb_test.ok(exists (select 1 from iamb.conflict c join iamb.snippet s on s.id = c.row_id
                                 where s.abbreviation = ';6' and c.kind = 'deleted_with_folder'), 's6 conflict');
    perform iamb_test.ok(not exists (select 1 from iamb.conflict where row_id in (s2, s3, s4)), 'no conflict for rows A had seen');
    -- Each deleted row has its own revision and updated_op = the folder's op.
    perform iamb_test.ok(not exists (select 1 from iamb.snippet where id in (s2, s3, s4) and updated_op <> (del ->> 'op_id')::uuid),
                         'rows a folder''s delete took along carry its op');
    perform iamb_test.eq((select count(distinct rev)::integer from iamb.snippet where id in (s1, s2, s3, s4)), 4, 'own revisions');
    -- The repeated op finds the receipt.
    r := iamb_test.push('ines', jsonb_build_array(del));
    perform iamb_test.ok(iamb_test.st(r) = 'applied' and (r -> 'results' -> 0 ->> 'replayed')::boolean
                         and jsonb_array_length(r -> 'results' -> 0 -> 'cascade') = 7, 'replayed with entries');
    update iamb.server_state set cascade_max = 2000 where id;
end $$;

-- A share is ended only by who may end it (S76): a member with `edit` may not delete a folder
-- above the root of another live share, nor the root of the share that gives him access.
do $$
declare
    ids jsonb := iamb_test.family('edit');
    f1 uuid; pi uuid; g uuid; h uuid := iamb_test.id(); a uuid := iamb_test.id(); b uuid := iamb_test.id();
    s uuid := iamb_test.id(); r jsonb; inv uuid := iamb_test.id();
begin
    f1 := (ids ->> 'f1')::uuid; pi := (ids ->> 'p_ines')::uuid; g := (ids ->> 'g')::uuid;
    perform iamb_test.new_user('ana'); perform iamb_test.hello('ana', 'Ana');
    perform iamb_test.call('ines', format('api.create_group(iamb_test.dev(''ines''), 1, %L, ''family'', ''Other'')', h));
    perform iamb_test.call('ines', format('api.invite(iamb_test.dev(''ines''), 1, %L, %L, %L, ''member'')', inv, h, iamb_test.email('ana')));
    perform iamb_test.call('ana', format('api.accept_invitation(iamb_test.dev(''ana''), 1, %L)', inv));
    -- f1 / a / b; b is shared with the other group.
    r := iamb_test.push('ines', jsonb_build_array(iamb_test.op_folder(a, pi, f1, 'A'), iamb_test.op_folder(b, pi, a, 'B'),
                                                   iamb_test.op_snippet(s, b, ';b', 'b'), iamb_test.op_share(b, h, 'view')));
    perform iamb_test.ok(iamb_test.st(r, 4) = 'applied', 'share b ' || r::text);
    r := iamb_test.push('tom', jsonb_build_array(iamb_test.op_delete('folder', a, iamb_test.rev('folder', a))));
    perform iamb_test.eq(iamb_test.st(r), 'rejected:forbidden', 'a folder above the root of another share');
    perform iamb_test.ok((select deleted_at from iamb.folder where id = a) is null and (select deleted_at from iamb.snippet where id = s) is null,
                         'nothing deleted');
    r := iamb_test.push('tom', jsonb_build_array(iamb_test.op_delete('folder', f1, iamb_test.rev('folder', f1))));
    perform iamb_test.eq(iamb_test.st(r), 'rejected:forbidden', 'the root of his own share');
    -- Ines may: the share of b ends with it, and Ana gets folder_revoked.
    perform iamb_test.pull('ana');
    r := iamb_test.push('ines', jsonb_build_array(iamb_test.op_delete('folder', a, iamb_test.rev('folder', a))));
    perform iamb_test.eq(iamb_test.st(r), 'applied', 'the owner deletes it');
    perform iamb_test.ok((select deleted_at from iamb.folder_share where id = iamb_private.share_id(b, h)) is not null, 'the share ended with it');
    perform iamb_test.ok(exists (select 1 from jsonb_array_elements(iamb_test.pull('ana') -> 'entries') e
                                 where e ->> 'kind' = 'folder_revoked' and e ->> 'folder' = b::text), 'ana: folder_revoked');
    perform iamb_test.check_all();
end $$;
