-- Re-verification of the security review (docs/reviews/4.3-security-reverify.md), N5, option 1 of
-- the review's section 12.3: a deleted row is read by those who could read it before it was
-- deleted. A reader whose access began after the delete gets neither the tombstone nor its
-- content, from no read; a write of theirs does not bring the row back; and a device that held
-- the row is told to remove it.

-- A folder is shared after rows in it were deleted: a snippet, a link, a sub-folder with a
-- snippet. The member reads the folder and none of them.
do $$
declare
    ids jsonb := iamb_test.family('edit'); g uuid := (ids ->> 'g')::uuid; p uuid := (ids ->> 'p_ines')::uuid;
    f2 uuid := (ids ->> 'f2')::uuid; s2 uuid := (ids ->> 's2')::uuid;
    s3 uuid := gen_random_uuid(); s5 uuid := gen_random_uuid(); f3 uuid := gen_random_uuid(); s4 uuid := gen_random_uuid();
    t uuid := gen_random_uuid(); l2 uuid := gen_random_uuid(); l3 uuid := gen_random_uuid();
    r jsonb; snap jsonb; own jsonb; pl jsonb;
begin
    perform iamb_test.pull('tom');
    r := iamb_test.push('ines', jsonb_build_array(
        iamb_test.op_snippet(s3, f2, ';keep', 'Stays.'), iamb_test.op_snippet(s5, f2, ';later', 'Deleted after the share.'),
        iamb_test.op_folder(f3, p, f2, 'Letters about the arrears'), iamb_test.op_snippet(s4, f3, ';arr', 'The arrears are 2,300.'),
        iamb_test.op_tag(t, p, 'debtors'), iamb_test.op_snippet_tag(l2, s2, t), iamb_test.op_snippet_tag(l3, s3, t)));
    perform iamb_test.ok(iamb_test.st(r, 7) = 'applied', 'setup: ' || left(r::text, 300));
    -- Before she shares the folder Ines deletes the private snippet, takes the tag off the one
    -- that stays, and deletes the sub-folder.
    r := iamb_test.push('ines', jsonb_build_array(
        iamb_test.op_delete('snippet', s2, iamb_test.rev('snippet', s2)),
        iamb_test.op_delete('snippet_tag', l3, iamb_test.rev('snippet_tag', l3)),
        iamb_test.op_delete('folder', f3, iamb_test.rev('folder', f3),
                            jsonb_build_object('cascade', jsonb_build_array(jsonb_build_object('entity', 'snippet', 'id', s4, 'base_rev', iamb_test.rev('snippet', s4)))))));
    perform iamb_test.ok(iamb_test.st(r, 1) = 'applied' and iamb_test.st(r, 2) = 'applied' and iamb_test.st(r, 3) = 'applied', 'the deletes: ' || left(r::text, 300));
    r := iamb_test.push('ines', jsonb_build_array(iamb_test.op_share(f2, g, 'edit')));
    perform iamb_test.eq(iamb_test.st(r), 'applied', 'the folder is shared');

    -- The feed: the folder became visible; no entry names a deleted row.
    pl := iamb_test.pull('tom');
    perform iamb_test.ok(exists (select 1 from jsonb_array_elements(pl -> 'entries') e where e ->> 'kind' = 'folder_visible' and e ->> 'folder' = f2::text),
                         'the folder is visible to Tom');
    perform iamb_test.ok(pl::text not like '%Private text.%' and pl::text not like '%arrears%', 'nothing deleted is in his feed');
    -- The snapshot (what a device reads at a sign-in): the folder and the live snippets, and
    -- nothing that was deleted before the share.
    snap := iamb_test.snapshot('tom');
    perform iamb_test.ok(exists (select 1 from jsonb_array_elements(snap) x where x ->> 'id' = s3::text)
                         and exists (select 1 from jsonb_array_elements(snap) x where x ->> 'id' = f2::text), 'the snapshot has the folder and what is live in it');
    perform iamb_test.eq((select coalesce(jsonb_agg(x ->> 'entity' order by x ->> 'entity'), '[]') from jsonb_array_elements(snap) x
                          where (x ->> 'id')::uuid in (s2, s4, f3, l2, l3)), '[]'::jsonb,
                         'no row deleted before the share is in the member''s snapshot');
    perform iamb_test.ok(snap::text not like '%Private text.%' and snap::text not like '%arrears%' and snap::text not like '%2,300%',
                         'nor any of its content');
    -- `rows` (how a device catches up on a row by id) answers null for each.
    r := iamb_test.call('tom', format('api.rows(iamb_test.dev(''tom''), 1, %L::jsonb)', jsonb_build_array(
        jsonb_build_object('entity', 'snippet', 'id', s2), jsonb_build_object('entity', 'snippet', 'id', s4),
        jsonb_build_object('entity', 'folder', 'id', f3), jsonb_build_object('entity', 'snippet_tag', 'id', l3),
        jsonb_build_object('entity', 'snippet_tag', 'id', l2), jsonb_build_object('entity', 'snippet', 'id', s3))));
    perform iamb_test.eq((select jsonb_agg(jsonb_typeof(x -> 'row') order by i) from jsonb_array_elements(r -> 'rows') with ordinality as y(x, i)),
                         '["null", "null", "null", "null", "null", "object"]'::jsonb, 'rows() gives null for each of them');
    -- The folders he is told he can read do not name the deleted sub-folder.
    r := iamb_test.call('tom', 'api.visible_folders(iamb_test.dev(''tom''), 1)');
    perform iamb_test.ok(r -> 'folders' @> to_jsonb(array[f2]) and not r -> 'folders' @> to_jsonb(array[f3]), 'visible_folders: ' || r::text);
    -- A cascade list that names them is answered as for rows that are not there.
    r := iamb_test.push('tom', jsonb_build_array(iamb_test.op_delete('folder', gen_random_uuid(), 0, jsonb_build_object('cascade', jsonb_build_array(
        jsonb_build_object('entity', 'snippet', 'id', s2, 'base_rev', 0), jsonb_build_object('entity', 'folder', 'id', f3, 'base_rev', 0),
        jsonb_build_object('entity', 'snippet', 'id', gen_random_uuid(), 'base_rev', 0))))));
    perform iamb_test.eq((select jsonb_agg(e - 'id' - 'entity') from jsonb_array_elements(r -> 'results' -> 0 -> 'cascade') e),
                         '[{"gone": true}, {"gone": true}, {"gone": true}]'::jsonb, 'a cascade answer says gone, as for an unknown row');
    -- The owner reads her tombstones as before.
    own := iamb_test.snapshot('ines');
    perform iamb_test.eq((select count(*)::integer from jsonb_array_elements(own) x
                          where (x ->> 'id')::uuid in (s2, s4, f3, l3) and x -> 'row' ->> 'deleted_at' is not null), 4,
                         'the owner''s snapshot has the four tombstones');
    -- A row deleted after the share reaches the member as a tombstone, as before.
    perform iamb_test.pull('tom');
    r := iamb_test.push('ines', jsonb_build_array(iamb_test.op_delete('snippet', s5, iamb_test.rev('snippet', s5))));
    pl := iamb_test.pull('tom');
    perform iamb_test.ok(exists (select 1 from jsonb_array_elements(pl -> 'entries') e
                                 where e ->> 'id' = s5::text and e -> 'row' ->> 'deleted_at' is not null), 'a later delete arrives as a tombstone');
    perform iamb_test.check_all();
end $$;

-- A write does not bring such a row back for the member, and with it nothing that hung on it.
do $$
declare
    ids jsonb := iamb_test.family('edit'); g uuid := (ids ->> 'g')::uuid; p uuid := (ids ->> 'p_ines')::uuid;
    f2 uuid := (ids ->> 'f2')::uuid; s2 uuid := (ids ->> 's2')::uuid; f3 uuid := gen_random_uuid();
    s4 uuid := gen_random_uuid(); t uuid := gen_random_uuid(); l2 uuid := gen_random_uuid();
    r jsonb; pl jsonb; n integer;
begin
    -- As above: a private snippet that carries a tag of the owner's, and a sub-folder with a
    -- snippet, are deleted; then the folder is shared, to change.
    r := iamb_test.push('ines', jsonb_build_array(
        iamb_test.op_folder(f3, p, f2, 'Letters about the arrears'), iamb_test.op_snippet(s4, f3, ';arr', 'The arrears are 2,300.'),
        iamb_test.op_tag(t, p, 'debtors'), iamb_test.op_snippet_tag(l2, s2, t)));
    perform iamb_test.ok(iamb_test.st(r, 4) = 'applied', 'setup: ' || left(r::text, 300));
    r := iamb_test.push('ines', jsonb_build_array(
        iamb_test.op_delete('snippet', s2, iamb_test.rev('snippet', s2)),
        iamb_test.op_delete('folder', f3, iamb_test.rev('folder', f3),
                            jsonb_build_object('cascade', jsonb_build_array(jsonb_build_object('entity', 'snippet', 'id', s4, 'base_rev', iamb_test.rev('snippet', s4)))))));
    perform iamb_test.ok(iamb_test.st(r, 1) = 'applied' and iamb_test.st(r, 2) = 'applied', 'the deletes: ' || left(r::text, 300));
    r := iamb_test.push('ines', jsonb_build_array(iamb_test.op_share(f2, g, 'edit')));
    perform iamb_test.eq(iamb_test.st(r), 'applied', 'the folder is shared');
    perform iamb_test.pull('tom');
    -- Tom may change the folder. A put with the deleted snippet's id: before, it made the row
    -- live again, and its link to the owner's tag with it.
    r := iamb_test.push('tom', jsonb_build_array(
        iamb_test.op_snippet(s2, f2, ';mine', 'Tom''s text', 0),
        iamb_test.op_snippet(s2, f2, ';mine', 'Tom''s text', 7)));
    perform iamb_test.ok(iamb_test.st(r, 1) = 'rejected:gone' and iamb_test.st(r, 2) = 'rejected:gone',
                         'a put on the deleted snippet: ' || iamb_test.st(r, 1) || ', ' || iamb_test.st(r, 2));
    perform iamb_test.ok(r -> 'results' -> 0 -> 'row' = 'null'::jsonb and r -> 'results' -> 0 -> 'conflict' = 'null'::jsonb, 'with no row and no conflict in the answer');
    perform iamb_test.ok((select deleted_at is not null and body = 'Private text.' from iamb.snippet where id = s2), 'the snippet is deleted, as it was');
    perform iamb_test.act('tom');
    select count(*) into n from iamb.tag where id = t;
    perform iamb_test.act_owner();
    perform iamb_test.eq(n, 0, 'the owner''s tag, which only the deleted snippet still carries, is not his to read');
    -- A row put into the deleted sub-folder, or a folder put under it (C7 would restore it, with
    -- its name): the sub-folder is not there for him.
    r := iamb_test.push('tom', jsonb_build_array(
        iamb_test.op_snippet(gen_random_uuid(), f3, ';x', 'x'),
        iamb_test.op_folder(gen_random_uuid(), p, f3, 'Below'),
        iamb_test.op_snippet(s4, f3, ';arr', 'guess', 0),
        iamb_test.op_folder(f3, p, f2, 'Mine now', 0)));
    perform iamb_test.eq((select array_agg(iamb_test.st(r, i) order by i) from generate_series(1, 4) i),
                         array['rejected:parent_gone', 'rejected:parent_gone', 'rejected:gone', 'rejected:gone'],
                         'writes into, below and onto the deleted sub-folder');
    perform iamb_test.ok((select deleted_at is not null and name = 'Letters about the arrears' from iamb.folder where id = f3), 'the sub-folder is deleted, as it was');
    perform iamb_test.ok(r::text not like '%arrears%', 'and its name is in no answer');
    -- A link to the deleted snippet, and deletes of what he cannot read.
    r := iamb_test.push('tom', jsonb_build_array(
        iamb_test.op_snippet_tag(gen_random_uuid(), s2, t),
        iamb_test.op_delete('snippet', s2, 0),
        iamb_test.op_delete('folder', f3, 0, '{"cascade": []}')));
    perform iamb_test.eq((select array_agg(iamb_test.st(r, i) order by i) from generate_series(1, 3) i),
                         array['rejected:parent_gone', 'unchanged', 'unchanged'], 'a link to it, and deletes of it');
    perform iamb_test.ok(r -> 'results' -> 1 -> 'row' = 'null'::jsonb, 'no row in those answers');
    -- The owner restores the snippet: it is live again and reaches the member as a new revision,
    -- with its link and the tag the link names.
    perform iamb_test.pull('tom');
    r := iamb_test.push('ines', jsonb_build_array(iamb_test.op_snippet(s2, f2, ';priv', 'Back, and meant to be shared.', iamb_test.rev('snippet', s2))));
    perform iamb_test.eq(iamb_test.st(r), 'applied', 'the owner restores it');
    pl := iamb_test.pull('tom');
    perform iamb_test.ok(exists (select 1 from jsonb_array_elements(pl -> 'entries') e where e ->> 'id' = s2::text and e -> 'row' ->> 'body' = 'Back, and meant to be shared.')
                         and exists (select 1 from jsonb_array_elements(pl -> 'entries') e where e ->> 'id' = l2::text and jsonb_typeof(e -> 'row') = 'object'),
                         'the restored snippet arrives with its link');
    perform iamb_test.ok(pl::text not like '%Private text.%', 'and without the content it had before');
    perform iamb_test.check_all();
end $$;

-- The same for a group: a member who joins after a folder, a snippet and a tag of the group were
-- deleted reads none of them; a member who was there reads them.
do $$
declare
    ids jsonb := iamb_test.family('edit'); g uuid := (ids ->> 'g')::uuid;
    gf uuid := gen_random_uuid(); gf2 uuid := gen_random_uuid(); gs uuid := gen_random_uuid(); gs2 uuid := gen_random_uuid();
    gt uuid := gen_random_uuid(); inv uuid := gen_random_uuid(); r jsonb; snap jsonb; n integer;
begin
    perform iamb_test.new_user('nora'); perform iamb_test.hello('nora', 'Nora');
    r := iamb_test.push('ines', jsonb_build_array(
        iamb_test.op_folder(gf, g, null, 'Family'), iamb_test.op_folder(gf2, g, null, 'Old plans'),
        iamb_test.op_snippet(gs, gf, ';code', 'The alarm code is 5512.'), iamb_test.op_snippet(gs2, gf2, ';plan', 'Sell the flat.'),
        iamb_test.op_tag(gt, g, 'money worries')));
    perform iamb_test.ok(iamb_test.st(r, 5) = 'applied', 'group rows: ' || left(r::text, 300));
    r := iamb_test.push('ines', jsonb_build_array(
        iamb_test.op_delete('snippet', gs, iamb_test.rev('snippet', gs)),
        iamb_test.op_delete('tag', gt, iamb_test.rev('tag', gt)),
        iamb_test.op_delete('folder', gf2, iamb_test.rev('folder', gf2),
                            jsonb_build_object('cascade', jsonb_build_array(jsonb_build_object('entity', 'snippet', 'id', gs2, 'base_rev', iamb_test.rev('snippet', gs2)))))));
    perform iamb_test.ok(iamb_test.st(r, 1) = 'applied' and iamb_test.st(r, 2) = 'applied' and iamb_test.st(r, 3) = 'applied', 'deleted: ' || left(r::text, 300));
    r := iamb_test.call('ines', format('api.invite(iamb_test.dev(''ines''), 1, %L, %L, %L, ''member'')', inv, g, iamb_test.email('nora')));
    r := iamb_test.call('nora', format('api.accept_invitation(iamb_test.dev(''nora''), 1, %L)', inv));
    perform iamb_test.ok(r ->> 'error' is null, 'Nora joins: ' || r::text);
    snap := iamb_test.snapshot('nora');
    perform iamb_test.ok(exists (select 1 from jsonb_array_elements(snap) x where x ->> 'id' = gf::text), 'the new member''s snapshot has the group''s folder');
    perform iamb_test.eq((select coalesce(jsonb_agg(x ->> 'entity' order by x ->> 'entity'), '[]') from jsonb_array_elements(snap) x
                          where (x ->> 'id')::uuid in (gs, gs2, gf2, gt)), '[]'::jsonb, 'and no row that was deleted before she joined');
    perform iamb_test.ok(snap::text not like '%5512%' and snap::text not like '%Sell the flat%' and snap::text not like '%money worries%'
                         and snap::text not like '%Old plans%', 'nor its content');
    r := iamb_test.call('nora', 'api.visible_folders(iamb_test.dev(''nora''), 1)');
    perform iamb_test.ok(r -> 'folders' @> to_jsonb(array[gf]) and not r -> 'folders' @> to_jsonb(array[gf2]), 'visible_folders: ' || r::text);
    r := iamb_test.call('nora', 'api.access(iamb_test.dev(''nora''), 1)');
    perform iamb_test.ok(not exists (select 1 from jsonb_array_elements(r -> 'roots') x where x ->> 'root' = gf2::text), 'the deleted folder is no root of hers');
    -- Tom was a member when they were deleted.
    snap := iamb_test.snapshot('tom');
    perform iamb_test.eq((select count(*)::integer from jsonb_array_elements(snap) x
                          where (x ->> 'id')::uuid in (gs, gs2, gf2, gt) and x -> 'row' ->> 'deleted_at' is not null), 4,
                         'a member of that time has the four tombstones');
    -- An admin who joined later cannot restore the deleted tag by its id.
    r := iamb_test.call('ines', format('api.set_member_role(iamb_test.dev(''ines''), 1, %L, %L, ''admin'')', g, iamb_test.uid('nora')));
    perform iamb_test.ok(r ->> 'error' is null, 'Nora is made admin: ' || r::text);
    r := iamb_test.push('nora', jsonb_build_array(iamb_test.op_tag(gt, g, 'guess', 0)));
    perform iamb_test.eq(iamb_test.st(r), 'rejected:gone', 'her put on the deleted tag');
    perform iamb_test.check_all();
end $$;

-- A device that held the row. Tom's device holds the shared folder with its snippet and is
-- offline. The share ends; Ines rewrites the snippet and deletes it; she shares the folder again.
-- What the server then tells the device removes the snippet from it, and nothing hands it the
-- text written while the folder was not shared.
do $$
declare
    ids jsonb := iamb_test.family('edit'); g uuid := (ids ->> 'g')::uuid;
    f1 uuid := (ids ->> 'f1')::uuid; s1 uuid := (ids ->> 's1')::uuid; sx uuid := gen_random_uuid();
    held bigint; r jsonb; pl jsonb; snap jsonb; kinds text[];
begin
    r := iamb_test.push('ines', jsonb_build_array(iamb_test.op_snippet(sx, f1, ';two', 'Deleted while it is shared.')));
    perform iamb_test.pull('tom');
    held := iamb_test.rev('snippet', s1);
    -- While Tom reads the folder, one snippet is deleted: his feed gets its entry.
    perform iamb_test.push('ines', jsonb_build_array(iamb_test.op_delete('snippet', sx, iamb_test.rev('snippet', sx))));
    -- The share ends, the other snippet is rewritten and deleted, the share begins again.
    perform iamb_test.push('ines', jsonb_build_array(iamb_test.op_delete('folder_share', iamb_private.share_id(f1, g), iamb_test.rev('folder_share', iamb_private.share_id(f1, g)))));
    perform iamb_test.push('ines', jsonb_build_array(iamb_test.op_snippet(s1, f1, ';hrs', 'The safe is behind the mirror.', held)));
    perform iamb_test.push('ines', jsonb_build_array(iamb_test.op_delete('snippet', s1, iamb_test.rev('snippet', s1))));
    r := iamb_test.push('ines', jsonb_build_array(iamb_test.op_share(f1, g, 'edit', iamb_test.rev('folder_share', iamb_private.share_id(f1, g)))));
    perform iamb_test.eq(iamb_test.st(r), 'applied', 'shared again');
    -- The device comes back. The entry of the first delete is delivered as null (the device
    -- removes the row); then the folder is revoked (the device purges the tree, the other snippet
    -- with it) and visible again (the device fetches what is live).
    pl := iamb_test.pull('tom');
    perform iamb_test.ok(exists (select 1 from jsonb_array_elements(pl -> 'entries') e where e ->> 'id' = sx::text and e -> 'row' = 'null'::jsonb),
                         'the delete he could have seen is delivered as null now that his access began again');
    select array_agg(e ->> 'kind' order by (e ->> 'seq')::bigint) into kinds from jsonb_array_elements(pl -> 'entries') e
    where e ->> 'kind' in ('folder_revoked', 'folder_visible') and e ->> 'folder' = f1::text;
    perform iamb_test.eq(kinds, array['folder_revoked', 'folder_visible'], 'the folder is revoked, then visible');
    perform iamb_test.ok(pl::text not like '%behind the mirror%' and pl::text not like '%Deleted while it is shared%', 'no deleted content in the page');
    r := iamb_test.call('tom', format('api.fetch_folder(iamb_test.dev(''tom''), 1, %L)', f1));
    perform iamb_test.ok(r ->> 'error' is null and r::text not like '%behind the mirror%'
                         and not exists (select 1 from jsonb_array_elements(r -> 'rows') x where (x ->> 'id')::uuid in (s1, sx)),
                         'the fetch of the tree brings neither snippet');
    -- A device that missed all of that (its position expired) reads a snapshot, which has
    -- neither; `rows` answers null; and a change it still held is refused for good.
    snap := iamb_test.snapshot('tom');
    perform iamb_test.ok(not exists (select 1 from jsonb_array_elements(snap) x where (x ->> 'id')::uuid in (s1, sx))
                         and snap::text not like '%behind the mirror%', 'the snapshot has neither snippet');
    r := iamb_test.call('tom', format('api.rows(iamb_test.dev(''tom''), 1, %L::jsonb)', jsonb_build_array(jsonb_build_object('entity', 'snippet', 'id', s1))));
    perform iamb_test.ok(r -> 'rows' -> 0 -> 'row' = 'null'::jsonb, 'rows() gives null');
    r := iamb_test.push('tom', jsonb_build_array(iamb_test.op_snippet(s1, f1, ';hrs', 'Open 9 to 6.', held)));
    perform iamb_test.eq(iamb_test.st(r), 'rejected:gone', 'the edit his device still held, based on the version it had');
    perform iamb_test.ok(r::text not like '%behind the mirror%' and r -> 'results' -> 0 -> 'row' = 'null'::jsonb, 'answered without a row');
    perform iamb_test.ok((select deleted_at is not null and body = 'The safe is behind the mirror.' from iamb.snippet where id = s1), 'the snippet is as the owner left it');
    perform iamb_test.check_all();
end $$;

-- A deleted version in the history. The owner restores a snippet she had deleted before she
-- shared the folder, from a second computer that had not seen the delete: her conflict offers her
-- the tombstone. A member's stale write takes that conflict over. He is offered the version his
-- write replaced, and not the tombstone, whose content he could never read.
do $$
declare
    ids jsonb := iamb_test.family('edit'); g uuid := (ids ->> 'g')::uuid;
    f2 uuid := (ids ->> 'f2')::uuid; s2 uuid := (ids ->> 's2')::uuid;
    r1 bigint; rdel bigint; r jsonb; c uuid; first uuid; second uuid;
begin
    perform iamb_test.new_device('ines2', 'ines'); perform iamb_test.hello('ines2');
    r1 := iamb_test.rev('snippet', s2);
    perform iamb_test.push('ines', jsonb_build_array(iamb_test.op_snippet(s2, f2, ';priv', 'The PIN is 4711, zebra-quartz.', r1)));
    perform iamb_test.push('ines', jsonb_build_array(iamb_test.op_delete('snippet', s2, iamb_test.rev('snippet', s2))));
    rdel := iamb_test.rev('snippet', s2);
    perform iamb_test.push('ines', jsonb_build_array(iamb_test.op_share(f2, g, 'edit')));
    perform iamb_test.pull('tom');
    r := iamb_test.push('ines2', jsonb_build_array(iamb_test.op_snippet(s2, f2, ';priv', 'Ask me.', r1)));
    perform iamb_test.ok(iamb_test.st(r) = 'applied' and r -> 'results' -> 0 ->> 'conflict' is not null, 'her stale put restores the snippet, with a conflict: ' || iamb_test.st(r));
    r := iamb_test.push('tom', jsonb_build_array(iamb_test.op_snippet(s2, f2, ';priv', 'Tom was here', 0)));
    c := (r -> 'results' -> 0 ->> 'conflict')::uuid;
    perform iamb_test.ok(iamb_test.st(r) = 'applied' and c is not null, 'Tom''s stale write records a conflict: ' || iamb_test.st(r));
    perform iamb_test.ok(exists (select 1 from iamb.conflict_version v where v.conflict_id = c and v.rev = rdel), 'which holds the tombstone, taken over');
    r := iamb_test.call('tom', format('api.get_conflict(iamb_test.dev(''tom''), 1, %L)', c));
    perform iamb_test.eq((select jsonb_agg(v -> 'row' -> 'body' order by (v ->> 'rev')::bigint) from jsonb_array_elements(r -> 'versions') v),
                         '["Ask me."]'::jsonb, 'what Tom is offered');
    perform iamb_test.ok(r::text not like '%zebra-quartz%', 'the deleted content is not in his answer');
    r := iamb_test.call('tom', format('api.resolve_conflict(iamb_test.dev(''tom''), 1, %L, ''restore'', %s)', c, rdel));
    perform iamb_test.ok(r ->> 'error' = 'invalid' and (select body from iamb.snippet where id = s2) = 'Tom was here', 'and he cannot restore it: ' || r::text);
    -- A guess of the deleted content, sent with the tombstone's revision as its base, is answered
    -- like any other write (C17): the server does not name the owner as its author.
    first := (iamb_test.push('tom', jsonb_build_array(iamb_test.op_snippet(s2, f2, ';priv', 'The PIN is 4710.', rdel))) -> 'results' -> 0 -> 'origin' ->> 'content_by')::uuid;
    second := (iamb_test.push('tom', jsonb_build_array(iamb_test.op_snippet(s2, f2, ';priv', 'The PIN is 4711, zebra-quartz.', rdel))) -> 'results' -> 0 -> 'origin' ->> 'content_by')::uuid;
    perform iamb_test.ok(first is not distinct from second, 'the right guess is answered like the wrong one: ' || coalesce(first::text, 'nobody') || ', ' || coalesce(second::text, 'nobody'));
    -- The owner is offered all of it.
    r := iamb_test.call('ines', format('api.get_conflict(iamb_test.dev(''ines''), 1, %L)', (select id from iamb.conflict where row_id = s2 and state = 'open')));
    perform iamb_test.ok(r::text like '%zebra-quartz%', 'the owner is offered the deleted version');
    perform iamb_test.check_all();
end $$;

-- The delete and the share in one push: the member's access begins when the request has ended,
-- so the row deleted in it was never his to read (as S88 for a replaced version).
do $$
declare
    ids jsonb := iamb_test.family('edit'); g uuid := (ids ->> 'g')::uuid;
    f2 uuid := (ids ->> 'f2')::uuid; s2 uuid := (ids ->> 's2')::uuid; r jsonb; snap jsonb;
begin
    perform iamb_test.pull('tom');
    r := iamb_test.push('ines', jsonb_build_array(
        iamb_test.op_delete('snippet', s2, iamb_test.rev('snippet', s2)), iamb_test.op_share(f2, g, 'view')));
    perform iamb_test.ok(iamb_test.st(r, 1) = 'applied' and iamb_test.st(r, 2) = 'applied', 'one push: ' || iamb_test.st(r, 1) || ', ' || iamb_test.st(r, 2));
    r := iamb_test.pull('tom');
    snap := iamb_test.snapshot('tom');
    perform iamb_test.ok(r::text not like '%Private text.%' and snap::text not like '%Private text.%'
                         and not exists (select 1 from jsonb_array_elements(snap) x where x ->> 'id' = s2::text),
                         'the snippet deleted in the push that shared its folder reaches the member by no read');
    perform iamb_test.check_all();
end $$;

-- Who deleted a row reads its tombstone, whenever their access to it began. A device that creates
-- a folder and then deletes a folder above it, offline, sends both in one push: the folder it
-- made is created and deleted in one request, so no access entry for it is older than its delete.
-- The device must still get the tombstones of what it made, or it would keep them live.
do $$
declare
    ids jsonb := iamb_test.family('edit'); p uuid := (ids ->> 'p_ines')::uuid; f1 uuid := (ids ->> 'f1')::uuid;
    top uuid := gen_random_uuid(); x uuid := gen_random_uuid(); sx uuid := gen_random_uuid();
    r jsonb; snap jsonb; pl jsonb;
begin
    perform iamb_test.new_device('ines2', 'ines'); perform iamb_test.hello('ines2');
    perform iamb_test.push('ines', jsonb_build_array(iamb_test.op_folder(top, p, f1, 'Returns')));
    perform iamb_test.pull('tom'); perform iamb_test.pull('ines2');
    r := iamb_test.push('ines', jsonb_build_array(
        iamb_test.op_folder(x, p, top, 'Made offline'), iamb_test.op_snippet(sx, x, ';off', 'Made offline too.'),
        iamb_test.op_delete('folder', top, iamb_test.rev('folder', top), jsonb_build_object('cascade', jsonb_build_array(
            jsonb_build_object('entity', 'folder', 'id', x, 'base_rev', 0), jsonb_build_object('entity', 'snippet', 'id', sx, 'base_rev', 0))))));
    perform iamb_test.ok(iamb_test.st(r, 1) = 'applied' and iamb_test.st(r, 2) = 'applied' and iamb_test.st(r, 3) = 'applied', 'one push: ' || left(r::text, 200));
    perform iamb_test.ok((select deleted_at is not null from iamb.folder where id = x) and (select deleted_at is not null from iamb.snippet where id = sx),
                         'the folder made in the push is deleted by the same push, with its snippet');
    perform iamb_test.eq((select jsonb_agg(e -> 'deleted' order by e ->> 'entity') from jsonb_array_elements(r -> 'results' -> 2 -> 'cascade') e),
                         '[true, true]'::jsonb, 'the answer tells the device that both are deleted: ' || (r -> 'results' -> 2 -> 'cascade')::text);
    snap := iamb_test.snapshot('ines');
    perform iamb_test.eq((select count(*)::integer from jsonb_array_elements(snap) e where (e ->> 'id')::uuid in (x, sx) and e -> 'row' ->> 'deleted_at' is not null), 2,
                         'who deleted them reads both tombstones');
    -- The member read the folder above all along. The folder made and deleted in one request was
    -- never his to read; the one that was there before reaches him as a tombstone.
    pl := iamb_test.pull('tom');
    snap := iamb_test.snapshot('tom');
    perform iamb_test.ok(exists (select 1 from jsonb_array_elements(snap) e where e ->> 'id' = top::text and e -> 'row' ->> 'deleted_at' is not null),
                         'the member has the tombstone of the folder he could read');
    perform iamb_test.ok(not exists (select 1 from jsonb_array_elements(snap) e where (e ->> 'id')::uuid in (x, sx))
                         and pl::text not like '%Made offline%' and snap::text not like '%Made offline%',
                         'and nothing of the folder that was made and deleted in one request');
    perform iamb_test.check_all();
end $$;

-- A link hangs on a snippet, and a snippet can change its folder. A snippet that moves leaves
-- its deleted links behind, so the entry for the folder it is in says who could read the ones
-- that are left.
do $$
declare
    ids jsonb := iamb_test.family('edit'); p uuid := (ids ->> 'p_ines')::uuid;
    f1 uuid := (ids ->> 'f1')::uuid; f2 uuid := (ids ->> 'f2')::uuid; s1 uuid := (ids ->> 's1')::uuid; s2 uuid := (ids ->> 's2')::uuid;
    t uuid := gen_random_uuid(); t2 uuid := gen_random_uuid(); l1 uuid := gen_random_uuid(); l2 uuid := gen_random_uuid(); l2b uuid := gen_random_uuid();
    sub uuid := gen_random_uuid(); r jsonb; snap jsonb; pl jsonb;
begin
    r := iamb_test.push('ines', jsonb_build_array(
        iamb_test.op_tag(t, p, 'shop'), iamb_test.op_tag(t2, p, 'debtors'),
        iamb_test.op_snippet_tag(l1, s1, t), iamb_test.op_snippet_tag(l2, s2, t2), iamb_test.op_snippet_tag(l2b, s2, t)));
    perform iamb_test.ok(iamb_test.st(r, 5) = 'applied', 'tags and links: ' || left(r::text, 200));
    perform iamb_test.pull('tom');
    -- While the snippet is private its owner takes the tag `debtors` off it. Then she moves it
    -- into the shared folder, where the member's access is older than that.
    perform iamb_test.push('ines', jsonb_build_array(iamb_test.op_delete('snippet_tag', l2, iamb_test.rev('snippet_tag', l2))));
    r := iamb_test.push('ines', jsonb_build_array(iamb_test.op_snippet(s2, f1, ';priv', 'Private text.', iamb_test.rev('snippet', s2))));
    perform iamb_test.eq(iamb_test.st(r), 'applied', 'the snippet moves into the shared folder');
    pl := iamb_test.pull('tom');
    snap := iamb_test.snapshot('tom');
    perform iamb_test.ok(exists (select 1 from jsonb_array_elements(snap) e where e ->> 'id' = s2::text)
                         and exists (select 1 from jsonb_array_elements(snap) e where e ->> 'id' = l2b::text),
                         'the member has the snippet and its live link');
    perform iamb_test.ok(not exists (select 1 from jsonb_array_elements(snap) e where (e ->> 'id')::uuid in (l2, t2))
                         and not exists (select 1 from jsonb_array_elements(pl -> 'entries' || pl -> 'refs') e where (e ->> 'id')::uuid in (l2, t2))
                         and snap::text not like '%debtors%' and pl::text not like '%debtors%',
                         'and neither the link that was deleted while it was private nor the tag it named');
    perform iamb_test.eq((select count(*)::integer from iamb.snippet_tag where id = l2), 0, 'the server left the deleted link behind at the move');
    -- A link deleted while the member reads the snippet reaches him as a tombstone; when the
    -- snippet then moves to a sub-folder made afterwards, the server forgets it, and the device
    -- keeps what it has until its own purge.
    perform iamb_test.push('ines', jsonb_build_array(iamb_test.op_delete('snippet_tag', l1, iamb_test.rev('snippet_tag', l1))));
    pl := iamb_test.pull('tom');
    perform iamb_test.ok(exists (select 1 from jsonb_array_elements(pl -> 'entries') e where e ->> 'id' = l1::text and e -> 'row' ->> 'deleted_at' is not null),
                         'a link deleted while he reads the snippet arrives as a tombstone');
    r := iamb_test.push('ines', jsonb_build_array(iamb_test.op_folder(sub, p, f1, 'Sub'), iamb_test.op_snippet(s1, sub, ';hrs', 'Open 9 to 5.', iamb_test.rev('snippet', s1))));
    perform iamb_test.ok(iamb_test.st(r, 1) = 'applied' and iamb_test.st(r, 2) = 'applied', 'a new sub-folder, and the snippet moved into it');
    perform iamb_test.eq((select count(*)::integer from iamb.snippet_tag where id = l1), 0, 'its deleted link is gone from the server');
    r := iamb_test.call('ines', format('api.rows(iamb_test.dev(''ines''), 1, %L::jsonb)', jsonb_build_array(jsonb_build_object('entity', 'snippet_tag', 'id', l1))));
    perform iamb_test.ok(r -> 'rows' -> 0 -> 'row' = 'null'::jsonb, 'for everybody, as after a purge');
    perform iamb_test.check_all();
end $$;

-- A tag of someone else's profile is read through a link only while the tag is live. A deleted
-- tag that a shared snippet still carries (links are not deleted with a tag, C10) is not handed
-- to the snippet's readers; if it comes back, it brings its links.
do $$
declare
    ids jsonb := iamb_test.family('view'); g uuid := (ids ->> 'g')::uuid; p uuid := (ids ->> 'p_ines')::uuid;
    f1 uuid := (ids ->> 'f1')::uuid; f2 uuid := (ids ->> 'f2')::uuid; s1 uuid := (ids ->> 's1')::uuid; s2 uuid := (ids ->> 's2')::uuid;
    t uuid := gen_random_uuid(); told uuid := gen_random_uuid(); l uuid := gen_random_uuid(); lold uuid := gen_random_uuid();
    r jsonb; snap jsonb; pl jsonb;
begin
    r := iamb_test.push('ines', jsonb_build_array(
        iamb_test.op_tag(t, p, 'shop'), iamb_test.op_tag(told, p, 'people who owe me'),
        iamb_test.op_snippet_tag(l, s1, t), iamb_test.op_snippet_tag(lold, s2, told)));
    perform iamb_test.ok(iamb_test.st(r, 4) = 'applied', 'tags and links: ' || left(r::text, 200));
    -- A tag deleted before the folder that carries it is shared.
    perform iamb_test.push('ines', jsonb_build_array(iamb_test.op_delete('tag', told, iamb_test.rev('tag', told))));
    perform iamb_test.push('ines', jsonb_build_array(iamb_test.op_share(f2, g, 'view')));
    pl := iamb_test.pull('tom');
    snap := iamb_test.snapshot('tom');
    perform iamb_test.ok(exists (select 1 from jsonb_array_elements(snap) e where e ->> 'id' = s2::text), 'the member has the snippet of the newly shared folder');
    perform iamb_test.ok(not exists (select 1 from jsonb_array_elements(snap) e where e ->> 'id' = told::text)
                         and snap::text not like '%people who owe me%' and pl::text not like '%people who owe me%',
                         'and not the deleted tag it still carries');
    -- A tag deleted while he reads the snippet: the entry is delivered as null, and his device
    -- drops the tag.
    perform iamb_test.pull('tom');
    perform iamb_test.push('ines', jsonb_build_array(iamb_test.op_delete('tag', t, iamb_test.rev('tag', t))));
    pl := iamb_test.pull('tom');
    perform iamb_test.ok(exists (select 1 from jsonb_array_elements(pl -> 'entries') e where e ->> 'id' = t::text and e -> 'row' = 'null'::jsonb)
                         and pl::text not like '%shop%', 'the delete of a tag he read through a link is delivered as null');
    -- The owner restores it: the tag is live again, and it brings the link.
    r := iamb_test.push('ines', jsonb_build_array(iamb_test.op_tag(t, p, 'shop', iamb_test.rev('tag', t))));
    perform iamb_test.eq(iamb_test.st(r), 'applied', 'the tag is restored');
    pl := iamb_test.pull('tom');
    perform iamb_test.ok(exists (select 1 from jsonb_array_elements(pl -> 'entries') e where e ->> 'id' = t::text and e -> 'row' ->> 'name' = 'shop')
                         and exists (select 1 from jsonb_array_elements(pl -> 'entries') e where e ->> 'id' = l::text and jsonb_typeof(e -> 'row') = 'object'),
                         'the restored tag arrives with its link');
    -- The owner reads her deleted tag as before.
    snap := iamb_test.snapshot('ines');
    perform iamb_test.ok(exists (select 1 from jsonb_array_elements(snap) e where e ->> 'id' = told::text and e -> 'row' ->> 'deleted_at' is not null), 'the owner has the tombstone of her tag');
    perform iamb_test.check_all();
end $$;
