-- Re-verification of the security review (docs/reviews/4.3-security-reverify.md), N1: an access
-- entry begins when the request that made it has ended. A push applies folder ops before snippet
-- ops; when the owner replaces private content and moves its folder into the shared tree in ONE
-- push, the replaced version was never the row in a state the member could have seen, and he is
-- not offered it: not in a conflict, not through a restore, and not through the author rule.

-- The conflict form. Her first computer writes something private into s2, in the private folder
-- f2. Her second computer, which was offline, empties the snippet and then moves the folder under
-- the shared folder f1; it sends both in one push, in that order.
do $$
declare
    ids jsonb := iamb_test.family('edit');
    f1 uuid := (ids ->> 'f1')::uuid; f2 uuid := (ids ->> 'f2')::uuid; s2 uuid := (ids ->> 's2')::uuid;
    r0 bigint; rv bigint; r jsonb; c uuid; since bigint;
begin
    perform iamb_test.new_device('ines2', 'ines'); perform iamb_test.hello('ines2');
    r0 := iamb_test.rev('snippet', s2);
    perform iamb_test.push('ines', jsonb_build_array(iamb_test.op_snippet(s2, f2, ';priv', 'PIN of the safe: 4711', r0)));
    rv := iamb_test.rev('snippet', s2);
    r := iamb_test.push('ines2', jsonb_build_array(
        iamb_test.op_snippet(s2, f2, ';priv', 'Nothing to see', r0),
        iamb_test.op_folder(f2, iamb_test.personal('ines'), f1, 'Private', iamb_test.rev('folder', f2))));
    perform iamb_test.ok(iamb_test.st(r, 1) = 'applied' and iamb_test.st(r, 2) = 'applied'
                         and (r -> 'results' -> 0 ->> 'conflict') is not null,
                         'the owner''s push: the edit (with a conflict of her own) and the move: ' || iamb_test.st(r, 1) || ' / ' || iamb_test.st(r, 2));
    select since_rev into since from iamb.folder_access where user_id = iamb_test.uid('tom') and folder_id = f2;
    perform iamb_test.ok(since >= (select replaced_by_rev from iamb.history where entity = 'snippet' and row_id = s2 and rev = rv),
                         'Tom''s entry for the folder begins at or above the revision that replaced the private version: '
                         || since || ' against ' || (select replaced_by_rev from iamb.history where entity = 'snippet' and row_id = s2 and rev = rv));
    perform iamb_test.ok(since >= (select max(rev) from iamb.wire_rows), 'and at or above every revision of the request');
    -- Tom (edit on f1, and now on f2 below it) sends a stale edit and reads the conflict it records.
    perform iamb_test.pull('tom');
    r := iamb_test.push('tom', jsonb_build_array(iamb_test.op_snippet(s2, f2, ';priv', 'Tom was here', 0)));
    c := (r -> 'results' -> 0 ->> 'conflict')::uuid;
    perform iamb_test.ok(c is not null, 'Tom''s stale write records a conflict: ' || r::text);
    r := iamb_test.call('tom', format('api.get_conflict(iamb_test.dev(''tom''), 1, %L)', c));
    perform iamb_test.ok(r ->> 'error' is null and r::text not like '%4711%',
                         'the version she replaced in the push that shared the folder is not offered to Tom: ' || (r -> 'versions')::text);
    perform iamb_test.eq((select jsonb_agg(v -> 'row' -> 'body') from jsonb_array_elements(r -> 'versions') v),
                         '["Nothing to see"]'::jsonb, 'he is offered the version his own write replaced, and only that');
    r := iamb_test.call('tom', format('api.resolve_conflict(iamb_test.dev(''tom''), 1, %L, ''restore'', %s)', c, rv));
    perform iamb_test.eq(r ->> 'error', 'invalid', 'and he cannot restore it');
    perform iamb_test.eq((select body from iamb.snippet where id = s2), 'Tom was here', 'the row is what it was');
    -- Ines wrote both versions and is offered both.
    r := iamb_test.call('ines', format('api.get_conflict(iamb_test.dev(''ines''), 1, %L)', c));
    perform iamb_test.ok(jsonb_array_length(r -> 'versions') = 2 and r::text like '%4711%', 'the owner is offered both');
    -- A version replaced in a LATER request is on offer to Tom as before (S57).
    r0 := iamb_test.rev('snippet', s2);
    perform iamb_test.push('ines', jsonb_build_array(iamb_test.op_snippet(s2, f2, ';priv', 'written after the share', r0)));
    r := iamb_test.push('tom', jsonb_build_array(iamb_test.op_snippet(s2, f2, ';priv', 'Tom again', r0)));
    c := (r -> 'results' -> 0 ->> 'conflict')::uuid;
    perform iamb_test.ok(c is not null, 'a second conflict: ' || r::text);
    r := iamb_test.call('tom', format('api.get_conflict(iamb_test.dev(''tom''), 1, %L)', c));
    perform iamb_test.eq((select jsonb_agg(v -> 'row' -> 'body' order by (v ->> 'rev')::bigint) from jsonb_array_elements(r -> 'versions') v),
                         '["Nothing to see", "written after the share"]'::jsonb,
                         'versions replaced in later requests are on offer to Tom, and the private one still is not');
    perform iamb_test.check_all();
end $$;

-- The same for a folder's name: a private folder with a sub-folder; the second computer renames
-- the sub-folder and then moves the folder above it under the shared folder, in one push.
do $$
declare
    ids jsonb := iamb_test.family('edit');
    f1 uuid := (ids ->> 'f1')::uuid; f2 uuid := (ids ->> 'f2')::uuid; y uuid := gen_random_uuid();
    ry bigint; rn bigint; r jsonb; c uuid;
begin
    perform iamb_test.new_device('ines2', 'ines'); perform iamb_test.hello('ines2');
    perform iamb_test.push('ines', jsonb_build_array(iamb_test.op_folder(y, iamb_test.personal('ines'), f2, 'Sub')));
    ry := iamb_test.rev('folder', y);
    perform iamb_test.push('ines', jsonb_build_array(iamb_test.op_folder(y, iamb_test.personal('ines'), f2, 'A private name', ry)));
    rn := iamb_test.rev('folder', y);
    r := iamb_test.push('ines2', jsonb_build_array(
        iamb_test.op_folder(y, iamb_test.personal('ines'), f2, 'Misc', ry),
        iamb_test.op_folder(f2, iamb_test.personal('ines'), f1, 'Private', iamb_test.rev('folder', f2))));
    perform iamb_test.ok(iamb_test.st(r, 1) = 'applied' and iamb_test.st(r, 2) = 'applied', 'the owner''s push: ' || r::text);
    perform iamb_test.pull('tom');
    r := iamb_test.push('tom', jsonb_build_array(iamb_test.op_folder(y, iamb_test.personal('ines'), f2, 'Tom renamed it', 0)));
    c := (r -> 'results' -> 0 ->> 'conflict')::uuid;
    perform iamb_test.ok(c is not null, 'Tom''s stale rename records a conflict: ' || r::text);
    r := iamb_test.call('tom', format('api.get_conflict(iamb_test.dev(''tom''), 1, %L)', c));
    perform iamb_test.eq((select jsonb_agg(v -> 'row' -> 'name') from jsonb_array_elements(r -> 'versions') v),
                         '["Misc"]'::jsonb, 'the name the sub-folder had before the folder was shared is not offered to Tom');
    r := iamb_test.call('tom', format('api.resolve_conflict(iamb_test.dev(''tom''), 1, %L, ''restore'', %s)', c, rn));
    perform iamb_test.eq(r ->> 'error', 'invalid', 'and cannot be restored by him');
    perform iamb_test.check_all();
end $$;

-- The author rule (review finding 11) by the same route: one computer of the owner is enough.
-- She replaces a PIN and then moves the folder under the shared folder, in one push; the member
-- sends guesses with the old version's revision as their base.
do $$
declare
    ids jsonb := iamb_test.family('edit');
    f1 uuid := (ids ->> 'f1')::uuid; f2 uuid := (ids ->> 'f2')::uuid; s2 uuid := (ids ->> 's2')::uuid;
    rv bigint; r jsonb; guess text; answers text[] := '{}';
begin
    perform iamb_test.push('ines', jsonb_build_array(iamb_test.op_snippet(s2, f2, ';pin', 'PIN 4711', iamb_test.rev('snippet', s2))));
    rv := iamb_test.rev('snippet', s2);
    r := iamb_test.push('ines', jsonb_build_array(
        iamb_test.op_snippet(s2, f2, ';pin', 'ask me', rv),
        iamb_test.op_folder(f2, iamb_test.personal('ines'), f1, 'Private', iamb_test.rev('folder', f2))));
    perform iamb_test.ok(iamb_test.st(r, 1) = 'applied' and iamb_test.st(r, 2) = 'applied', 'the owner''s push: ' || r::text);
    perform iamb_test.pull('tom');
    foreach guess in array array['PIN 4710', 'PIN 4711', 'PIN 4712'] loop
        r := iamb_test.push('tom', jsonb_build_array(iamb_test.op_snippet(s2, f2, ';pin', guess, rv)));
        perform iamb_test.eq(iamb_test.st(r), 'applied', 'the guess is a write like any other');
        answers := answers || coalesce(r -> 'results' -> 0 -> 'origin' ->> 'content_by', 'nobody');
    end loop;
    perform iamb_test.ok(answers[1] = answers[2] and answers[2] = answers[3],
                         'the right guess is answered like the wrong ones: ' || answers::text);
    perform iamb_test.ok(not iamb_test.uid('ines')::text = any (answers), 'the owner is named for none of them');
    perform iamb_test.check_all();
end $$;

-- A version that was written and replaced within one request was never the row for anybody but
-- its writer. One push names the shared snippet twice: the first op puts something in that the
-- second takes out again. The folder has been shared all along, so Tom's entry is old.
do $$
declare
    ids jsonb := iamb_test.family('edit');
    f1 uuid := (ids ->> 'f1')::uuid; s1 uuid := (ids ->> 's1')::uuid;
    r0 bigint; mid bigint; r jsonb; guess text; answers text[] := '{}'; c uuid;
begin
    perform iamb_test.new_device('ines2', 'ines'); perform iamb_test.hello('ines2');
    perform iamb_test.pull('tom'); perform iamb_test.pull('ines2');
    r0 := iamb_test.rev('snippet', s1);
    r := iamb_test.push('ines', jsonb_build_array(
        iamb_test.op_snippet(s1, f1, ';hrs', 'pasted by mistake: 4711', r0),
        iamb_test.op_snippet(s1, f1, ';hrs', 'Open 9 to 5, as before.', r0)));
    perform iamb_test.ok(iamb_test.st(r, 1) = 'applied' and iamb_test.st(r, 2) = 'applied', 'one push, the row twice: ' || r::text);
    select h.rev into mid from iamb.history h where h.entity = 'snippet' and h.row_id = s1 and h.row ->> 'body' like 'pasted by mistake%';
    -- The author rule does not compare with it for Tom.
    perform iamb_test.pull('tom');
    foreach guess in array array['pasted by mistake: 4710', 'pasted by mistake: 4711'] loop
        r := iamb_test.push('tom', jsonb_build_array(iamb_test.op_snippet(s1, f1, ';hrs', guess, mid)));
        answers := answers || coalesce(r -> 'results' -> 0 -> 'origin' ->> 'content_by', 'nobody');
    end loop;
    perform iamb_test.ok(answers[1] = answers[2] and not iamb_test.uid('ines')::text = any (answers),
                         'a guess of the version in between is answered like any other: ' || answers::text);
    -- And a conflict that holds it does not offer it to him: her second computer's stale write
    -- takes over the conflict her double op recorded.
    r := iamb_test.push('ines2', jsonb_build_array(iamb_test.op_snippet(s1, f1, ';hrs', 'Open 10 to 4.', r0)));
    c := (r -> 'results' -> 0 ->> 'conflict')::uuid;
    perform iamb_test.ok(c is not null, 'a conflict: ' || r::text);
    insert into iamb.conflict_version (conflict_id, rev, user_id, device_id, taken_over)
    values (c, mid, iamb_test.uid('ines'), null, true) on conflict do nothing;
    r := iamb_test.call('tom', format('api.get_conflict(iamb_test.dev(''tom''), 1, %L)', c));
    perform iamb_test.ok(r ->> 'error' is null and jsonb_array_length(r -> 'versions') > 0
                         and not exists (select 1 from jsonb_array_elements(r -> 'versions') v where (v ->> 'rev')::bigint = mid),
                         'the version in between is not offered to Tom: ' || (select jsonb_agg(v -> 'rev') from jsonb_array_elements(r -> 'versions') v)::text);
    r := iamb_test.call('ines', format('api.get_conflict(iamb_test.dev(''ines''), 1, %L)', c));
    perform iamb_test.ok(exists (select 1 from jsonb_array_elements(r -> 'versions') v where (v ->> 'rev')::bigint = mid),
                         'its writer is offered it');
    perform iamb_test.ok((select (to_jsonb(h) ->> 'within_request')::boolean from iamb.history h where h.entity = 'snippet' and h.row_id = s1 and h.rev = mid),
                         'the version in between is marked as never having been the row');
    perform iamb_test.ok(not (select (to_jsonb(h) ->> 'within_request')::boolean from iamb.history h where h.entity = 'snippet' and h.row_id = s1 and h.rev = r0),
                         'the version from before the request is not');
    perform iamb_test.check_all();
end $$;
