-- C20 and G5: a conflict is read by its parties only, with history versions only while the
-- reader may read the row; "keep" closes it; "restore" writes the version back with its author
-- and records no conflict.
do $$
declare
    ids jsonb := iamb_test.family('edit');
    f1 uuid; s1 uuid; r jsonb; base bigint; cid uuid; v_rev bigint; before integer;
begin
    f1 := (ids ->> 'f1')::uuid; s1 := (ids ->> 's1')::uuid;
    base := iamb_test.rev('snippet', s1);
    r := iamb_test.push('tom', jsonb_build_array(iamb_test.op_snippet(s1, f1, ';hrs', 'Tom''s hours', base)));
    r := iamb_test.push('ines', jsonb_build_array(iamb_test.op_snippet(s1, f1, ';hrs', 'Ines''s hours', base)));
    cid := (r -> 'results' -> 0 ->> 'conflict')::uuid;
    perform iamb_test.ok(cid is not null, 'conflict');
    r := iamb_test.call('tom', format('api.get_conflict(iamb_test.dev(''tom''), 1, %L)', cid));
    v_rev := (r -> 'versions' -> 0 ->> 'rev')::bigint;
    perform iamb_test.ok(r ->> 'error' is null and r -> 'versions' -> 0 -> 'row' ->> 'body' = 'Tom''s hours'
                         and r -> 'versions' -> 0 ->> 'device' = iamb_test.dev('tom')::text
                         and r -> 'conflict' -> 'winner_device' = 'null'::jsonb, 'a party reads it; his own version names his device: ' || r::text);
    r := iamb_test.call('ines', format('api.get_conflict(iamb_test.dev(''ines''), 1, %L)', cid));
    perform iamb_test.ok(r -> 'versions' -> 0 -> 'device' = 'null'::jsonb and r -> 'conflict' ->> 'winner_device' = iamb_test.dev('ines')::text,
                         'another person''s version names the person only: ' || r::text);
    r := iamb_test.call('stranger', format('api.get_conflict(iamb_test.dev(''stranger''), 1, %L)', cid));
    perform iamb_test.eq(r ->> 'error', 'not_found', 'not a party');
    r := iamb_test.call('stranger', format('api.resolve_conflict(iamb_test.dev(''stranger''), 1, %L, ''keep'')', cid));
    perform iamb_test.eq(r ->> 'error', 'not_found', 'not a party resolves');
    perform iamb_test.eq(iamb_test.visible('tom', 'history'), -1, 'history is not readable directly');
    r := iamb_test.call('tom', 'api.conflicts(iamb_test.dev(''tom''), 1)');
    perform iamb_test.eq(jsonb_array_length(r -> 'conflicts'), 1, 'the list');
    -- Restore Tom's version: a new revision by Tom, Tom's content authorship, no new conflict.
    select count(*) into before from iamb.conflict;
    r := iamb_test.call('tom', format('api.resolve_conflict(iamb_test.dev(''tom''), 1, %L, ''restore'', %s)', cid, v_rev));
    perform iamb_test.ok(r ->> 'error' is null, 'restore: ' || r::text);
    perform iamb_test.ok((select body from iamb.snippet where id = s1) = 'Tom''s hours'
                         and (select content_by from iamb.snippet where id = s1) = iamb_test.uid('tom')
                         and (select updated_by from iamb.snippet where id = s1) = iamb_test.uid('tom'), 'the version is back');
    perform iamb_test.eq((select count(*)::integer from iamb.conflict), before, 'no conflict for that write');
    perform iamb_test.eq((select state from iamb.conflict where id = cid), 'resolved', 'resolved');
    -- Keep, and a viewer cannot restore.
    base := iamb_test.rev('snippet', s1);
    r := iamb_test.push('tom', jsonb_build_array(iamb_test.op_snippet(s1, f1, ';hrs', 'again', base)));
    r := iamb_test.push('ines', jsonb_build_array(iamb_test.op_snippet(s1, f1, ';hrs', 'and again', base)));
    cid := (r -> 'results' -> 0 ->> 'conflict')::uuid;
    r := iamb_test.push('ines', jsonb_build_array(iamb_test.op_share(f1, (ids ->> 'g')::uuid, 'view',
                                                   iamb_test.rev('folder_share', iamb_private.share_id(f1, (ids ->> 'g')::uuid)))));
    r := iamb_test.call('tom', format('api.resolve_conflict(iamb_test.dev(''tom''), 1, %L, ''restore'', %s)', cid,
                                      (select rev from iamb.conflict_version where conflict_id = cid)));
    perform iamb_test.eq(r ->> 'error', 'forbidden', 'restoring needs the right to write the row now');
    r := iamb_test.call('tom', format('api.resolve_conflict(iamb_test.dev(''tom''), 1, %L, ''keep'')', cid));
    perform iamb_test.ok(r ->> 'error' is null and (select state from iamb.conflict where id = cid) = 'resolved', 'keep');
    perform iamb_test.check_all();
end $$;
