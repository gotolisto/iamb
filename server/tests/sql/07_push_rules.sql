-- The rules of push: C1 to C5, C8 to C10, C13 to C17, and G9, G10 (§8, §11.5).
do $$
declare
    pi uuid; f uuid := iamb_test.id(); s uuid := iamb_test.id(); s2 uuid := iamb_test.id();
    r jsonb; r1 bigint; r2 bigint; n integer; c1 uuid; c2 uuid; op1 jsonb; row1 jsonb;
    now_ms bigint;
begin
    perform iamb_test.new_user('ines'); perform iamb_test.new_device('ines_b', 'ines');
    perform iamb_test.hello('ines', 'Ines'); perform iamb_test.hello('ines_b');
    pi := iamb_test.personal('ines');

    -- C1, G9: the server's counter and clock; a creation time in the future is cut back; fields
    -- the server assigns are not taken from the client.
    now_ms := iamb_private.now_ms();
    r := iamb_test.push('ines', jsonb_build_array(
        iamb_test.op_folder(f, pi, null, 'F'),
        iamb_test.op_snippet(s, f, ';ty', 'Thanks.', 0, jsonb_build_object('row', jsonb_build_object(
            'created_at', 99999999999999, 'rev', 7, 'updated_by', gen_random_uuid(), 'updated_at', 1,
            'deleted_at', 5, 'content_by', gen_random_uuid())))));
    perform iamb_test.eq(iamb_test.st(r, 2), 'applied', 'create');
    row1 := r -> 'results' -> 1 -> 'row';
    r1 := (row1 ->> 'rev')::bigint;
    perform iamb_test.ok(r1 > iamb_test.rev('folder', f), 'revisions grow in the order of acceptance');
    perform iamb_test.ok((row1 ->> 'created_at')::bigint <= iamb_private.now_ms(), 'created_at cut back to the server''s clock');
    perform iamb_test.ok((row1 ->> 'updated_at')::bigint >= now_ms, 'updated_at is the server''s clock');
    perform iamb_test.eq((row1 ->> 'updated_by')::uuid, iamb_test.uid('ines'), 'updated_by is the caller');
    perform iamb_test.ok(row1 -> 'deleted_at' = 'null'::jsonb, 'deleted_at not taken from the client');
    perform iamb_test.eq((r -> 'results' -> 1 -> 'origin' ->> 'content_by')::uuid, iamb_test.uid('ines'), 'content_by is the caller');
    perform iamb_test.eq((r -> 'results' -> 1 -> 'origin' ->> 'created_rev')::bigint, r1, 'created_rev');
    perform iamb_test.ok(not (row1 ? 'updated_device') and not (row1 ? 'updated_op') and not (row1 ? 'lost_rev'),
                         'never sent: updated_device, updated_op, lost_rev');
    perform iamb_test.eq((select updated_device from iamb.snippet where id = s), iamb_test.dev('ines'), 'updated_device recorded');

    -- C2, C4: B replaces the whole row; the same row again is `unchanged`.
    r := iamb_test.push('ines_b', jsonb_build_array(iamb_test.op_snippet(s, f, ';ty', 'Thank you.', r1)));
    perform iamb_test.eq(iamb_test.st(r), 'applied', 'B edits');
    r2 := (r -> 'results' -> 0 -> 'row' ->> 'rev')::bigint;
    perform iamb_test.ok(exists (select 1 from iamb.history where entity = 'snippet' and row_id = s and rev = r1 and row ->> 'body' = 'Thanks.'),
                         'the replaced version is in the history');
    select count(*) into n from iamb.history where row_id = s;
    r := iamb_test.push('ines_b', jsonb_build_array(iamb_test.op_snippet(s, f, ';ty', 'Thank you.', 1)));
    perform iamb_test.eq(iamb_test.st(r), 'unchanged', 'the same row again, whatever the base');
    perform iamb_test.eq(iamb_test.rev('snippet', s), r2, 'no revision for unchanged');
    perform iamb_test.eq((select count(*)::integer from iamb.history where row_id = s), n, 'no history for unchanged');

    -- C3: A, stale (base r1), changes the body: applied, with a conflict offering B's version.
    op1 := iamb_test.op_snippet(s, f, ';ty', 'Many thanks.', r1);
    r := iamb_test.push('ines', jsonb_build_array(op1));
    perform iamb_test.eq(iamb_test.st(r), 'applied', 'stale write still wins');
    c1 := (r -> 'results' -> 0 ->> 'conflict')::uuid;
    perform iamb_test.ok(c1 is not null, 'a conflict is recorded');
    perform iamb_test.eq((select kind from iamb.conflict where id = c1), 'edit_edit', 'kind');
    perform iamb_test.eq((select rev from iamb.conflict_version where conflict_id = c1), r2, 'it offers the replaced version');
    -- A again, before it saw its push answered (follows): no conflict with itself.
    r := iamb_test.push('ines', jsonb_build_array(iamb_test.op_snippet(s, f, ';ty', 'Many, many thanks.', r1,
        jsonb_build_object('follows', jsonb_build_array(op1 -> 'op_id')))));
    perform iamb_test.ok(iamb_test.st(r) = 'applied' and r -> 'results' -> 0 -> 'conflict' = 'null'::jsonb,
                         'no conflict with the same device''s own earlier change: ' || r::text);
    perform iamb_test.eq((select state from iamb.conflict where id = c1), 'open', 'a write that records no conflict leaves the open one');
    -- A stale write whose base had the same content as the row: nothing changed since the base,
    -- so no conflict (only the position, which is not a recorded field, for a folder).
    r := iamb_test.push('ines_b', jsonb_build_array(iamb_test.op_folder(f, pi, null, 'F', iamb_test.rev('folder', f),
        jsonb_build_object('row', jsonb_build_object('position', 3)))));
    r := iamb_test.push('ines', jsonb_build_array(iamb_test.op_folder(f, pi, null, 'F2', 0 + (select rev from iamb.history where row_id = f order by rev limit 1))));
    perform iamb_test.ok(iamb_test.st(r) = 'applied' and r -> 'results' -> 0 -> 'conflict' = 'null'::jsonb,
                         'a folder''s position records no conflict: ' || r::text);

    -- C5: a stale delete over an edit records delete_over_edit; a put on that tombstone, stale,
    -- records edit_over_delete; a delete of a tombstone or of an unknown row is unchanged.
    r := iamb_test.push('ines', jsonb_build_array(iamb_test.op_snippet(s2, f, ';two', 'Two')));
    r1 := iamb_test.rev('snippet', s2);
    r := iamb_test.push('ines_b', jsonb_build_array(iamb_test.op_snippet(s2, f, ';two', 'Two!', r1)));
    r := iamb_test.push('ines', jsonb_build_array(iamb_test.op_delete('snippet', s2, r1)));
    perform iamb_test.ok(iamb_test.st(r) = 'applied' and (select kind from iamb.conflict where id = (r -> 'results' -> 0 ->> 'conflict')::uuid) = 'delete_over_edit',
                         'delete_over_edit: ' || r::text);
    r := iamb_test.push('ines_b', jsonb_build_array(iamb_test.op_snippet(s2, f, ';two', 'Two!!', r1 + 1)));
    c2 := (r -> 'results' -> 0 ->> 'conflict')::uuid;
    perform iamb_test.ok(iamb_test.st(r) = 'applied' and (select kind from iamb.conflict where id = c2) = 'edit_over_delete',
                         'edit_over_delete: ' || r::text);
    perform iamb_test.ok((select deleted_at from iamb.snippet where id = s2) is null, 'the put made the row live again');
    perform iamb_test.ok((select state from iamb.conflict where row_id = s2 and kind = 'delete_over_edit') = 'replaced',
                         'the older conflict is replaced');
    perform iamb_test.eq((select count(*)::integer from iamb.conflict_version where conflict_id = c2), 2, 'the new conflict took over its version');
    r := iamb_test.push('ines', jsonb_build_array(iamb_test.op_delete('snippet', s2, iamb_test.rev('snippet', s2))));
    r := iamb_test.push('ines_b', jsonb_build_array(iamb_test.op_delete('snippet', s2, 1), iamb_test.op_delete('snippet', iamb_test.id(), 0)));
    perform iamb_test.eq(iamb_test.st(r, 1), 'unchanged', 'delete of a tombstone');
    perform iamb_test.ok(iamb_test.st(r, 2) = 'unchanged' and r -> 'results' -> 1 -> 'row' = 'null'::jsonb, 'delete of an unknown row');

    -- C9: duplicate abbreviations and names are accepted.
    r := iamb_test.push('ines', jsonb_build_array(iamb_test.op_snippet(iamb_test.id(), f, ';ty', 'Dup'),
                                                   iamb_test.op_folder(iamb_test.id(), pi, null, 'F2')));
    perform iamb_test.ok(iamb_test.st(r, 1) = 'applied' and iamb_test.st(r, 2) = 'applied', 'duplicates accepted');

    -- C13: a put with a base above 0 for a row the server does not know.
    r := iamb_test.push('ines', jsonb_build_array(iamb_test.op_snippet(iamb_test.id(), f, ';gone', 'x', 5)));
    perform iamb_test.eq(iamb_test.st(r), 'rejected:gone', 'unknown row with a base');

    -- C15: the same op twice writes once.
    op1 := iamb_test.op_snippet(iamb_test.id(), f, ';once', 'Once');
    r := iamb_test.push('ines', jsonb_build_array(op1));
    r1 := (r -> 'results' -> 0 -> 'row' ->> 'rev')::bigint;
    r := iamb_test.push('ines', jsonb_build_array(op1));
    perform iamb_test.ok(iamb_test.st(r) = 'applied' and (r -> 'results' -> 0 ->> 'replayed')::boolean
                         and (r -> 'results' -> 0 -> 'row' ->> 'rev')::bigint = r1, 'replayed: ' || r::text);
    perform iamb_test.eq((select count(*)::integer from iamb.history where row_id = (op1 ->> 'id')::uuid), 0, 'written once');

    -- C16, G10: profiles and memberships are not pushable.
    r := iamb_test.push('ines', jsonb_build_array(
        iamb_test.op('profile', pi, jsonb_build_object('kind', 'personal', 'name', 'Mine', 'owner_user_id', iamb_test.uid('ines')), 1),
        iamb_test.op('membership', iamb_test.id(), jsonb_build_object('profile_id', pi, 'user_id', iamb_test.uid('ines'), 'role', 'owner')),
        iamb_test.op_delete('profile', pi, 1)));
    perform iamb_test.ok(iamb_test.st(r, 1) = 'rejected:not_pushable' and iamb_test.st(r, 2) = 'rejected:not_pushable'
                         and iamb_test.st(r, 3) = 'rejected:not_pushable', 'not pushable: ' || r::text);
    perform iamb_test.check_all();
end $$;

-- C8: structure is put right, not refused; C10: links; C14: what cannot be stored.
do $$
declare
    pi uuid; pt uuid; l1 uuid := iamb_test.id(); l2 uuid := iamb_test.id(); l3 uuid := iamb_test.id(); l4 uuid := iamb_test.id();
    x uuid := iamb_test.id(); y uuid := iamb_test.id(); z uuid := iamb_test.id();
    m uuid := iamb_test.id(); m2 uuid := iamb_test.id(); s uuid := iamb_test.id(); sd uuid := iamb_test.id();
    t uuid := iamb_test.id(); ft uuid := iamb_test.id(); bf uuid := iamb_test.id();
    r jsonb; lim jsonb;
begin
    delete from iamb_test.actor where true;
    perform iamb_test.new_user('ines'); perform iamb_test.new_user('tom');
    perform iamb_test.hello('ines', 'Ines'); perform iamb_test.hello('tom', 'Tom');
    pi := iamb_test.personal('ines'); pt := iamb_test.personal('tom');
    r := iamb_test.push('ines', jsonb_build_array(
        iamb_test.op_folder(l1, pi, null, 'L1'), iamb_test.op_folder(l2, pi, l1, 'L2'),
        iamb_test.op_folder(l3, pi, l2, 'L3'), iamb_test.op_folder(l4, pi, l3, 'L4'),
        iamb_test.op_folder(x, pi, null, 'X'), iamb_test.op_folder(y, pi, x, 'Y'),
        iamb_test.op_folder(z, pi, l4, 'Z, a fifth level')));
    perform iamb_test.ok(iamb_test.st(r, 7) = 'applied' and r -> 'results' -> 6 -> 'adjusted' = '["parent_id"]'
                         and (r -> 'results' -> 6 -> 'row' ->> 'parent_id')::uuid = l3, 'a new fifth level goes under the nearest folder above: ' || (r -> 'results' -> 6)::text);
    -- A cycle keeps the current parent; everything else of the row is applied.
    r := iamb_test.push('ines', jsonb_build_array(iamb_test.op_folder(x, pi, y, 'X renamed', iamb_test.rev('folder', x))));
    perform iamb_test.ok(iamb_test.st(r) = 'applied' and r -> 'results' -> 0 -> 'adjusted' = '["parent_id"]'
                         and r -> 'results' -> 0 -> 'row' -> 'parent_id' = 'null'::jsonb and r -> 'results' -> 0 -> 'row' ->> 'name' = 'X renamed',
                         'cycle: ' || (r -> 'results' -> 0)::text);
    -- A move that would make a tree deeper than four levels keeps the current parent.
    r := iamb_test.push('ines', jsonb_build_array(iamb_test.op_folder(x, pi, l3, 'X renamed', iamb_test.rev('folder', x))));
    perform iamb_test.ok(iamb_test.st(r) like 'unchanged%' and r -> 'results' -> 0 -> 'adjusted' = '["parent_id"]',
                         'depth: ' || (r -> 'results' -> 0)::text);
    -- A row whose profile would change is blocked.
    r := iamb_test.push('tom', jsonb_build_array(iamb_test.op_folder(m, pt, null, 'Toms')));
    r := iamb_test.push('ines', jsonb_build_array(
        iamb_test.op_folder(m2, pi, m, 'under another profile'),
        iamb_test.op_folder(iamb_test.id(), pt, null, 'in another profile')));
    perform iamb_test.eq(iamb_test.st(r, 1), 'rejected:parent_gone', 'a parent the writer cannot read');
    -- A profile the caller has no grant in answers like one that does not exist, and is not
    -- locked (security review 4.3, findings 6 and 10).
    perform iamb_test.eq(iamb_test.st(r, 2), 'rejected:parent_gone', 'a folder in another profile');
    r := iamb_test.push('ines', jsonb_build_array(
        iamb_test.op_snippet(s, l1, ';s', 's'), iamb_test.op_snippet(sd, l1, ';d', 'd'),
        iamb_test.op_tag(t, pi, 'tag')));
    r := iamb_test.push('ines', jsonb_build_array(iamb_test.op_folder(l2, pt, l1, 'L2', iamb_test.rev('folder', l2))));
    perform iamb_test.eq(iamb_test.st(r), 'blocked:invalid', 'a folder whose profile would change');
    perform iamb_test.check_tree();

    -- C10: a link to a tombstone is accepted and waits; to an unknown row, parent_gone; two links
    -- for one pair may exist.
    r := iamb_test.push('ines', jsonb_build_array(iamb_test.op_delete('snippet', sd, iamb_test.rev('snippet', sd))));
    r := iamb_test.push('ines', jsonb_build_array(
        iamb_test.op_snippet_tag(iamb_test.id(), sd, t), iamb_test.op_snippet_tag(iamb_test.id(), s, t),
        iamb_test.op_snippet_tag(iamb_test.id(), s, t), iamb_test.op_snippet_tag(iamb_test.id(), s, iamb_test.id())));
    perform iamb_test.ok(iamb_test.st(r, 1) = 'applied' and iamb_test.st(r, 2) = 'applied' and iamb_test.st(r, 3) = 'applied',
                         'links: ' || r::text);
    perform iamb_test.eq(iamb_test.st(r, 4), 'rejected:parent_gone', 'a link to an unknown tag');

    -- C14: too large, invalid, parent blocked; plan limits.
    r := iamb_test.push('ines', jsonb_build_array(
        iamb_test.op_snippet(iamb_test.id(), l1, ';big', repeat('x', 400001)),
        iamb_test.op_snippet(iamb_test.id(), l1, repeat('a', 65), 'x'),
        iamb_test.op_snippet(iamb_test.id(), l1, ';k', 'x', 0, jsonb_build_object('row', jsonb_build_object('kind', 'weird'))),
        iamb_test.op_folder(bf, pi, null, repeat('n', 1025)),
        iamb_test.op_snippet(iamb_test.id(), bf, ';in', 'in a blocked folder'),
        iamb_test.op_snippet(iamb_test.id(), l1, ';ok', repeat('x', 400000))));
    perform iamb_test.eq(iamb_test.st(r, 1), 'blocked:too_large', 'body over 400,000 bytes');
    perform iamb_test.eq(iamb_test.st(r, 2), 'blocked:too_large', 'abbreviation over 64 characters');
    perform iamb_test.eq(iamb_test.st(r, 3), 'blocked:invalid', 'a value the protocol does not know');
    perform iamb_test.eq(iamb_test.st(r, 4), 'blocked:too_large', 'a name over 1,024 bytes');
    perform iamb_test.eq(iamb_test.st(r, 5), 'blocked:parent_blocked', 'a row whose parent was blocked in the same request');
    perform iamb_test.eq(iamb_test.st(r, 6), 'applied', 'a body of exactly 400,000 bytes');
    select limits into lim from iamb.plan where id = 'default-user';
    update iamb.plan set limits = limits || jsonb_build_object('snippets_max', (select count(*) from iamb.snippet where profile_id = pi and deleted_at is null),
                                                               'folders_max', (select count(*) from iamb.folder where profile_id = pi and deleted_at is null))
    where id = 'default-user';
    r := iamb_test.push('ines', jsonb_build_array(iamb_test.op_snippet(iamb_test.id(), l1, ';more', 'more'),
                                                   iamb_test.op_folder(iamb_test.id(), pi, null, 'more'),
                                                   iamb_test.op_snippet(s, l1, ';s', 'an edit is not a create', iamb_test.rev('snippet', s))));
    perform iamb_test.eq(iamb_test.st(r, 1), 'blocked:plan_limit', 'snippets_max');
    perform iamb_test.eq(iamb_test.st(r, 2), 'blocked:plan_limit', 'folders_max');
    perform iamb_test.eq(iamb_test.st(r, 3), 'applied', 'an edit over the limit');
    update iamb.plan set limits = lim where id = 'default-user';
    perform iamb_test.check_all();
end $$;

-- C17, G9: who wrote a snippet's content (S40, S55, and the four cases).
do $$
declare
    ids jsonb := iamb_test.family('edit');
    f1 uuid; s1 uuid; r jsonb; r0 bigint; rt bigint; c jsonb;
begin
    f1 := (ids ->> 'f1')::uuid; s1 := (ids ->> 's1')::uuid;
    perform iamb_test.new_device('ines_b', 'ines'); perform iamb_test.hello('ines_b');
    r0 := iamb_test.rev('snippet', s1);
    -- Case 3: Tom changes the content: he is its author.
    r := iamb_test.push('tom', jsonb_build_array(iamb_test.op_snippet(s1, f1, ';hrs', 'Open 9 to 5. Ring twice.', r0)));
    rt := iamb_test.rev('snippet', s1);
    perform iamb_test.eq((select content_by from iamb.snippet where id = s1), iamb_test.uid('tom'), 'case 3: the writer of new content');
    -- Case 1 (S40): Ines changes the label from a device that holds Tom's content: Tom stays the author.
    r := iamb_test.push('ines', jsonb_build_array(iamb_test.op_snippet(s1, f1, ';hrs', 'Open 9 to 5. Ring twice.', rt,
        jsonb_build_object('row', jsonb_build_object('label', 'Hours')))));
    perform iamb_test.eq((select content_by from iamb.snippet where id = s1), iamb_test.uid('tom'), 'case 1: content equal to the row');
    -- Case 2 (S55): Ines's other device, a moment behind (base r0), changes the label and carries
    -- her old content: equal to the base, so the base's author (Ines) is named, not Ines-the-writer
    -- of Tom's content.
    r := iamb_test.push('ines_b', jsonb_build_array(iamb_test.op_snippet(s1, f1, ';hrs', 'Open 9 to 5.', r0,
        jsonb_build_object('row', jsonb_build_object('label', 'Opening hours')))));
    c := (select to_jsonb(x) from iamb.snippet x where x.id = s1);
    perform iamb_test.eq((c ->> 'content_by')::uuid, (select (row ->> 'content_by')::uuid from iamb.history where row_id = s1 and rev = r0),
                         'case 2: the base version''s author');
    perform iamb_test.ok(r -> 'results' -> 0 ->> 'conflict' is not null, 'and a conflict, since it was stale');
    -- Case 4: base 0 on a row that exists, no follows: the server cannot tell.
    r := iamb_test.push('tom', jsonb_build_array(iamb_test.op_snippet(s1, f1, ';hrs', 'Something else', 0)));
    perform iamb_test.ok((select content_by from iamb.snippet where id = s1) is null, 'case 4: nobody');
    perform iamb_test.check_all();
end $$;
