-- Security review 4.3, findings 1, 2, 4 and 11: what a conflict offers, to whom, and what
-- "Restore this version" may write (C3, C17, C20 as changed by ADR-0031).

-- Finding 1, first form: a version from the owner's private folder. Ines's two computers disagree
-- about a snippet in her private folder; she then moves it into the shared folder. Tom's stale
-- write records a conflict that takes the old versions over. He is offered the version his write
-- replaced and nothing from the private folder; Ines is offered all of them.
do $$
declare
    ids jsonb := iamb_test.family('edit');
    f1 uuid := (ids ->> 'f1')::uuid; f2 uuid := (ids ->> 'f2')::uuid; s2 uuid := (ids ->> 's2')::uuid;
    r1 bigint; r jsonb; c uuid; priv bigint;
begin
    perform iamb_test.new_device('ines2', 'ines'); perform iamb_test.hello('ines2');
    r1 := iamb_test.rev('snippet', s2);
    perform iamb_test.push('ines',  jsonb_build_array(iamb_test.op_snippet(s2, f2, ';priv', 'PIN of the safe: 4711', r1)));
    priv := iamb_test.rev('snippet', s2);
    perform iamb_test.push('ines2', jsonb_build_array(iamb_test.op_snippet(s2, f2, ';priv', 'Nothing to see', r1)));
    perform iamb_test.push('ines2', jsonb_build_array(iamb_test.op_snippet(s2, f1, ';priv', 'Nothing to see', iamb_test.rev('snippet', s2))));
    perform iamb_test.pull('tom');
    r := iamb_test.push('tom', jsonb_build_array(iamb_test.op_snippet(s2, f1, ';priv', 'Tom was here', 0)));
    c := (r -> 'results' -> 0 ->> 'conflict')::uuid;
    perform iamb_test.ok(c is not null, 'Tom''s stale write records a conflict: ' || r::text);
    r := iamb_test.call('tom', format('api.get_conflict(iamb_test.dev(''tom''), 1, %L)', c));
    perform iamb_test.ok(r ->> 'error' is null, 'Tom reads the conflict: ' || r::text);
    perform iamb_test.ok(r::text not like '%4711%', 'no version from the private folder reaches Tom: ' || (r -> 'versions')::text);
    perform iamb_test.ok(not exists (select 1 from jsonb_array_elements(r -> 'versions') v where v -> 'row' ->> 'folder_id' = f2::text),
                         'no version that was in the private folder');
    perform iamb_test.eq(jsonb_array_length(r -> 'versions'), 1, 'Tom is offered the version his write replaced, and only that');
    perform iamb_test.eq(r -> 'versions' -> 0 -> 'row' ->> 'body', 'Nothing to see', 'the version his write replaced');
    perform iamb_test.eq(r -> 'current' -> 'row' ->> 'body', 'Tom was here', 'the current row');
    -- He cannot restore a version he is not offered either.
    r := iamb_test.call('tom', format('api.resolve_conflict(iamb_test.dev(''tom''), 1, %L, ''restore'', %s)', c, priv));
    perform iamb_test.eq(r ->> 'error', 'invalid', 'restoring a version that is not on offer to him');
    perform iamb_test.ok((select body from iamb.snippet where id = s2) = 'Tom was here', 'nothing was restored');
    -- Ines wrote them all and is offered them all, the taken-over ones included.
    r := iamb_test.call('ines', format('api.get_conflict(iamb_test.dev(''ines''), 1, %L)', c));
    perform iamb_test.eq(jsonb_array_length(r -> 'versions'), 2, 'the owner is offered every version: ' || (r -> 'versions')::text);
    perform iamb_test.ok(r::text like '%4711%', 'her own private version among them');
    perform iamb_test.check_all();
end $$;

-- Finding 1, second form: a version from before the folder was shared. No move, and Tom's device
-- sends the base it really holds.
do $$
declare
    f uuid := gen_random_uuid(); s uuid := gen_random_uuid(); g uuid := gen_random_uuid(); inv uuid := gen_random_uuid();
    r1 bigint; r jsonb; c uuid; held bigint;
begin
    delete from iamb_test.actor where true;
    perform iamb_test.new_user('ines'); perform iamb_test.new_device('ines2', 'ines'); perform iamb_test.new_user('tom');
    perform iamb_test.hello('ines', 'Ines'); perform iamb_test.hello('ines2'); perform iamb_test.hello('tom', 'Tom');
    perform iamb_test.push('ines', jsonb_build_array(iamb_test.op_folder(f, iamb_test.personal('ines'), null, 'Bank'), iamb_test.op_snippet(s, f, ';iban', 'start')));
    -- Her own open conflict, from before the share.
    r1 := iamb_test.rev('snippet', s);
    perform iamb_test.push('ines',  jsonb_build_array(iamb_test.op_snippet(s, f, ';iban', 'my bank: IBAN XX00 1234', r1)));
    perform iamb_test.push('ines2', jsonb_build_array(iamb_test.op_snippet(s, f, ';iban', 'ask me', r1)));
    -- Now she shares the folder.
    perform iamb_test.call('ines', format('api.create_group(iamb_test.dev(''ines''), 1, %L, ''family'', ''Family'')', g));
    perform iamb_test.call('ines', format('api.invite(iamb_test.dev(''ines''), 1, %L, %L, %L, ''member'')', inv, g, iamb_test.email('tom')));
    perform iamb_test.call('tom', format('api.accept_invitation(iamb_test.dev(''tom''), 1, %L)', inv));
    perform iamb_test.push('ines', jsonb_build_array(iamb_test.op_share(f, g, 'edit')));
    perform iamb_test.pull('tom');
    held := iamb_test.rev('snippet', s);                       -- what Tom's device holds
    perform iamb_test.push('ines', jsonb_build_array(iamb_test.op_snippet(s, f, ';iban', 'ask me, really', held)));
    r := iamb_test.push('tom', jsonb_build_array(iamb_test.op_snippet(s, f, ';iban', 'Tom''s offline edit', held)));
    c := (r -> 'results' -> 0 ->> 'conflict')::uuid;
    perform iamb_test.ok(c is not null, 'conflict: ' || r::text);
    r := iamb_test.call('tom', format('api.get_conflict(iamb_test.dev(''tom''), 1, %L)', c));
    perform iamb_test.ok(r ->> 'error' is null and r::text not like '%IBAN%', 'no version from before the share reaches Tom: ' || (r -> 'versions')::text);
    perform iamb_test.eq(jsonb_array_length(r -> 'versions'), 1, 'one version on offer to Tom');
    perform iamb_test.eq(r -> 'versions' -> 0 -> 'row' ->> 'body', 'ask me, really', 'the one his write replaced');
    -- S57 still holds: versions that were replaced while the reader already had the folder are on
    -- offer to him, also as taken-over ones; the one from before the share still is not.
    held := iamb_test.rev('snippet', s);
    perform iamb_test.push('ines',  jsonb_build_array(iamb_test.op_snippet(s, f, ';iban', 'third', held)));
    perform iamb_test.push('ines2', jsonb_build_array(iamb_test.op_snippet(s, f, ';iban', 'fourth', held)));
    r := iamb_test.push('tom', jsonb_build_array(iamb_test.op_snippet(s, f, ';iban', 'Tom again', held)));
    c := (r -> 'results' -> 0 ->> 'conflict')::uuid;
    perform iamb_test.ok(c is not null, 'conflict: ' || r::text);
    r := iamb_test.call('tom', format('api.get_conflict(iamb_test.dev(''tom''), 1, %L)', c));
    perform iamb_test.eq((select array_agg(v -> 'row' ->> 'body' order by (v ->> 'rev')::bigint) from jsonb_array_elements(r -> 'versions') v),
                         array['ask me, really', 'third', 'fourth'],
                         'the versions replaced since the share began are on offer to Tom, taken-over ones included');
    r := iamb_test.call('ines', format('api.get_conflict(iamb_test.dev(''ines''), 1, %L)', c));
    perform iamb_test.eq(jsonb_array_length(r -> 'versions'), 4, 'the owner is offered those and the one from before the share');
    perform iamb_test.check_all();
end $$;

-- Finding 2: a removed member is told of no conflict, lists none, reads none and closes none.
do $$
declare
    ids jsonb := iamb_test.family('edit');
    f1 uuid := (ids ->> 'f1')::uuid; s1 uuid := (ids ->> 's1')::uuid; g uuid := (ids ->> 'g')::uuid;
    base bigint; r jsonb; c uuid; p jsonb; tom_rev bigint;
begin
    perform iamb_test.new_device('ines2', 'ines'); perform iamb_test.hello('ines2');
    perform iamb_test.pull('tom');
    base := iamb_test.rev('snippet', s1);
    perform iamb_test.push('tom',  jsonb_build_array(iamb_test.op_snippet(s1, f1, ';hrs', 'Tom''s hours', base)));
    tom_rev := iamb_test.rev('snippet', s1);
    r := iamb_test.push('ines', jsonb_build_array(iamb_test.op_snippet(s1, f1, ';hrs', 'Ines''s hours', base)));
    -- While he is a member he is a party like any other.
    r := iamb_test.call('tom', 'api.conflicts(iamb_test.dev(''tom''), 1)');
    perform iamb_test.eq(jsonb_array_length(r -> 'conflicts'), 1, 'a member lists the conflict');
    perform iamb_test.call('ines', format('api.remove_member(iamb_test.dev(''ines''), 1, %L, %L)', g, iamb_test.uid('tom')));
    perform iamb_test.pull('tom');
    -- After the removal her two computers conflict on s1; the new conflict takes Tom's version over.
    base := iamb_test.rev('snippet', s1);
    perform iamb_test.push('ines',  jsonb_build_array(iamb_test.op_snippet(s1, f1, ';hrs', 'After Tom left, v1', base)));
    r := iamb_test.push('ines2', jsonb_build_array(iamb_test.op_snippet(s1, f1, ';hrs', 'After Tom left, v2', base)));
    c := (r -> 'results' -> 0 ->> 'conflict')::uuid;
    perform iamb_test.ok(c is not null, 'conflict after the removal');
    p := iamb_test.pull('tom');
    perform iamb_test.eq(jsonb_array_length(p -> 'entries'), 0, 'the removed member''s feed gets nothing: ' || (p -> 'entries')::text);
    r := iamb_test.call('tom', 'api.conflicts(iamb_test.dev(''tom''), 1)');
    perform iamb_test.eq(jsonb_array_length(r -> 'conflicts'), 0, 'conflicts() lists nothing for him: ' || r::text);
    perform iamb_test.eq(iamb_test.visible('tom', 'conflict'), 0, 'the policy on conflict shows him no row');
    perform iamb_test.eq(iamb_test.visible('tom', 'conflict_version'), 0, 'nor a version row');
    r := iamb_test.call('tom', format('api.get_conflict(iamb_test.dev(''tom''), 1, %L)', c));
    perform iamb_test.eq(r ->> 'error', 'not_found', 'get_conflict');
    r := iamb_test.call('tom', format('api.resolve_conflict(iamb_test.dev(''tom''), 1, %L, ''keep'')', c));
    perform iamb_test.eq(r ->> 'error', 'not_found', 'he cannot close it');
    perform iamb_test.eq((select state from iamb.conflict where id = c), 'open', 'the conflict is still open');
    -- The people who are left still have every version, his included, and can restore it.
    r := iamb_test.call('ines', format('api.get_conflict(iamb_test.dev(''ines''), 1, %L)', c));
    perform iamb_test.ok(exists (select 1 from jsonb_array_elements(r -> 'versions') v where (v ->> 'rev')::bigint = tom_rev
                                 and v -> 'row' ->> 'body' = 'Tom''s hours'), 'his version is still on offer to the owner');
    r := iamb_test.call('ines', format('api.resolve_conflict(iamb_test.dev(''ines''), 1, %L, ''restore'', %s)', c, tom_rev));
    perform iamb_test.ok(r ->> 'error' is null and (select body from iamb.snippet where id = s1) = 'Tom''s hours', 'and she restores it: ' || r::text);
    p := iamb_test.pull('tom');
    perform iamb_test.eq(jsonb_array_length(p -> 'entries'), 0, 'still nothing in his feed');
    perform iamb_test.check_all();
end $$;

-- Finding 4: "Restore this version" is judged as a push of that version. A member with `edit`
-- cannot put a folder of the group at the top level with it, nor move a snippet out of the shared
-- tree; the owner can.
do $$
declare
    ids jsonb := iamb_test.family('edit'); g uuid := (ids ->> 'g')::uuid;
    t1 uuid := gen_random_uuid(); x uuid := gen_random_uuid(); r1 bigint; r jsonb; c uuid; top_rev bigint;
begin
    perform iamb_test.new_device('ines2', 'ines'); perform iamb_test.hello('ines2');
    perform iamb_test.push('ines', jsonb_build_array(
        iamb_test.op_folder(t1, g, null, 'T1', 0, '{"row": {"member_permission": "edit"}}'),
        iamb_test.op_folder(x,  g, null, 'X',  0, '{"row": {"member_permission": "edit"}}')));
    perform iamb_test.pull('ines2'); perform iamb_test.pull('tom');
    r1 := iamb_test.rev('folder', x);
    perform iamb_test.push('ines',  jsonb_build_array(iamb_test.op_folder(x, g, null, 'X renamed', r1, '{"row": {"member_permission": "edit"}}')));
    top_rev := iamb_test.rev('folder', x);                      -- X at the top level, written by the owner
    perform iamb_test.push('ines2', jsonb_build_array(iamb_test.op_folder(x, g, t1, 'X under T1', r1)));
    perform iamb_test.pull('tom');
    perform iamb_test.eq(iamb_test.st(iamb_test.push('tom', jsonb_build_array(iamb_test.op_folder(x, g, null, 'X', iamb_test.rev('folder', x))))),
                         'rejected:forbidden', 'a member may not move a folder to the top by push');
    -- Tom's stale rename takes the owner's versions over. He was a member all along, so they are
    -- on offer to him; restoring the one from the top level is another matter.
    r := iamb_test.push('tom', jsonb_build_array(iamb_test.op_folder(x, g, t1, 'X renamed by Tom', 0)));
    c := (r -> 'results' -> 0 ->> 'conflict')::uuid;
    perform iamb_test.ok(c is not null, 'conflict: ' || r::text);
    r := iamb_test.call('tom', format('api.resolve_conflict(iamb_test.dev(''tom''), 1, %L, ''restore'', %s)', c, top_rev));
    perform iamb_test.eq(r ->> 'error', 'forbidden', 'nor by restoring a version: ' || r::text);
    perform iamb_test.ok((select parent_id from iamb.folder where id = x) = t1 and (select name from iamb.folder where id = x) = 'X renamed by Tom'
                         and (select state from iamb.conflict where id = c) = 'open', 'the folder is where it was and the conflict stays open');
    -- The owner may: the same call, by her.
    r := iamb_test.call('ines', format('api.resolve_conflict(iamb_test.dev(''ines''), 1, %L, ''restore'', %s)', c, top_rev));
    perform iamb_test.ok(r ->> 'error' is null and (select parent_id from iamb.folder where id = x) is null
                         and (select name from iamb.folder where id = x) = 'X renamed', 'the owner restores it: ' || r::text);
    perform iamb_test.check_all();
end $$;

-- Finding 4, a snippet: a restore that would move it into another access root needs `manage`.
do $$
declare
    ids jsonb := iamb_test.family('edit'); g uuid := (ids ->> 'g')::uuid; f1 uuid := (ids ->> 'f1')::uuid;
    f3 uuid := gen_random_uuid(); s uuid := gen_random_uuid(); r jsonb; c uuid; in_f3 bigint; base bigint;
begin
    perform iamb_test.new_device('ines2', 'ines'); perform iamb_test.hello('ines2');
    -- A second folder shared with the family to change; the snippet starts there.
    perform iamb_test.push('ines', jsonb_build_array(iamb_test.op_folder(f3, iamb_test.personal('ines'), null, 'Other shared'),
                                                     iamb_test.op_snippet(s, f3, ';mv', 'in the other shared folder'),
                                                     iamb_test.op_share(f3, g, 'edit')));
    perform iamb_test.pull('tom');
    in_f3 := iamb_test.rev('snippet', s);
    -- The owner moves it to f1 (she may); Tom's stale edit makes the conflict that offers the version in f3.
    perform iamb_test.push('ines', jsonb_build_array(iamb_test.op_snippet(s, f1, ';mv', 'moved by the owner', in_f3)));
    base := iamb_test.rev('snippet', s);
    r := iamb_test.push('tom', jsonb_build_array(iamb_test.op_snippet(s, f1, ';mv', 'Tom''s edit', in_f3)));
    c := (r -> 'results' -> 0 ->> 'conflict')::uuid;
    perform iamb_test.ok(c is not null, 'conflict: ' || r::text);
    perform iamb_test.eq(iamb_test.st(iamb_test.push('tom', jsonb_build_array(iamb_test.op_snippet(s, f3, ';mv', 'x', iamb_test.rev('snippet', s))))),
                         'rejected:forbidden', 'a member may not move a snippet to another access root by push');
    -- The conflict offers Tom the version his write replaced (in f1). The version in f3 is not in
    -- this conflict; put it there as a later conflict would (taken over), and try to restore it.
    insert into iamb.conflict_version (conflict_id, rev, user_id, device_id, taken_over) values (c, in_f3, iamb_test.uid('ines'), null, true);
    r := iamb_test.call('tom', format('api.resolve_conflict(iamb_test.dev(''tom''), 1, %L, ''restore'', %s)', c, in_f3));
    perform iamb_test.eq(r ->> 'error', 'forbidden', 'nor by restoring a version: ' || r::text);
    perform iamb_test.eq((select folder_id from iamb.snippet where id = s), f1, 'the snippet is where it was');
    r := iamb_test.call('tom', format('api.resolve_conflict(iamb_test.dev(''tom''), 1, %L, ''restore'', %s)', c, base));
    perform iamb_test.ok(r ->> 'error' is null and (select body from iamb.snippet where id = s) = 'moved by the owner'
                         and (select content_by from iamb.snippet where id = s) = iamb_test.uid('ines')
                         and (select updated_by from iamb.snippet where id = s) = iamb_test.uid('tom'),
                         'a restore within the tree works and keeps the content''s author: ' || r::text);
    perform iamb_test.check_all();
end $$;

-- Finding 11: the author rule compares with a base version only when the writer could read it.
-- A member who sends guesses with a base from before the share is named as the author of none of
-- them and of all of them alike: the answers do not differ.
do $$
declare
    f uuid := gen_random_uuid(); s uuid := gen_random_uuid(); g uuid := gen_random_uuid(); inv uuid := gen_random_uuid();
    old_rev bigint; r jsonb; guess text; answers text[] := '{}';
begin
    delete from iamb_test.actor where true;
    perform iamb_test.new_user('ines'); perform iamb_test.new_user('tom');
    perform iamb_test.hello('ines', 'Ines'); perform iamb_test.hello('tom', 'Tom');
    perform iamb_test.push('ines', jsonb_build_array(iamb_test.op_folder(f, iamb_test.personal('ines'), null, 'Door codes'), iamb_test.op_snippet(s, f, ';pin', 'PIN 4711')));
    old_rev := iamb_test.rev('snippet', s);
    perform iamb_test.push('ines', jsonb_build_array(iamb_test.op_snippet(s, f, ';pin', 'ask me', old_rev)));
    perform iamb_test.call('ines', format('api.create_group(iamb_test.dev(''ines''), 1, %L, ''family'', ''Family'')', g));
    perform iamb_test.call('ines', format('api.invite(iamb_test.dev(''ines''), 1, %L, %L, %L, ''member'')', inv, g, iamb_test.email('tom')));
    perform iamb_test.call('tom', format('api.accept_invitation(iamb_test.dev(''tom''), 1, %L)', inv));
    perform iamb_test.push('ines', jsonb_build_array(iamb_test.op_share(f, g, 'edit')));
    perform iamb_test.pull('tom');
    foreach guess in array array['PIN 4710', 'PIN 4711', 'PIN 4712'] loop
        r := iamb_test.push('tom', jsonb_build_array(iamb_test.op_snippet(s, f, ';pin', guess, old_rev)));
        perform iamb_test.eq(iamb_test.st(r), 'applied', 'the guess is a write like any other');
        answers := answers || coalesce(r -> 'results' -> 0 -> 'origin' ->> 'content_by', 'nobody');
    end loop;
    perform iamb_test.ok(answers[1] = answers[2] and answers[2] = answers[3],
                         'the right guess is answered like the wrong ones: ' || answers::text);
    perform iamb_test.ok(not iamb_test.uid('ines')::text = any (answers), 'the owner is named for none of them');
    -- C17 case 2 as before for a base the writer could read (S55): Ines's device, a moment behind,
    -- sends Tom's content with a new label; the author stays Tom.
    r := iamb_test.push('tom', jsonb_build_array(iamb_test.op_snippet(s, f, ';pin', 'Tom''s content', iamb_test.rev('snippet', s))));
    old_rev := iamb_test.rev('snippet', s);
    perform iamb_test.push('tom', jsonb_build_array(iamb_test.op_snippet(s, f, ';pin', 'Tom''s newer content', old_rev)));
    r := iamb_test.push('ines', jsonb_build_array(iamb_test.op_snippet(s, f, ';pin', 'Tom''s content', old_rev, '{"row": {"label": "New label"}}')));
    perform iamb_test.eq(r -> 'results' -> 0 -> 'origin' ->> 'content_by', iamb_test.uid('tom')::text, 'a readable base still gives the base''s author');
    perform iamb_test.check_all();
end $$;
