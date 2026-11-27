-- The final attack pass before the beta (docs/reviews/4.3-final-attack-pass.md), O3: an access
-- entry for a deleted folder that its reader may not read gives nothing. The reads here are made
-- directly, as a device's role and with no function of `api` in between (`iamb_test.named`): the
-- REST interface exposes no table, and this is what the policies alone hold.

-- A sub-folder and a folder below it are deleted before the folder above is shared. Read
-- directly, no table and no view names either to the member; who deleted them, and a member who
-- could read them before, read what they read.
do $$
declare
    ids jsonb := iamb_test.family('edit'); g uuid := (ids ->> 'g')::uuid; p uuid := (ids ->> 'p_ines')::uuid;
    f1 uuid := (ids ->> 'f1')::uuid; f2 uuid := (ids ->> 'f2')::uuid;
    f3 uuid := gen_random_uuid(); f4 uuid := gen_random_uuid(); s4 uuid := gen_random_uuid();
    k1 uuid := gen_random_uuid(); ks uuid := gen_random_uuid(); inv uuid := gen_random_uuid();
    r jsonb; n integer; got jsonb;
begin
    perform iamb_test.new_user('nora'); perform iamb_test.hello('nora', 'Nora');
    perform iamb_test.push('ines', jsonb_build_array(
        iamb_test.op_folder(f3, p, f2, 'Letters about the arrears'), iamb_test.op_folder(f4, p, f3, 'Deeper'),
        iamb_test.op_snippet(s4, f4, ';arr', 'The arrears are 2,300.'),
        iamb_test.op_folder(k1, p, f1, 'Deleted while shared'), iamb_test.op_snippet(ks, k1, ';k', 'Seen by the member.')));
    perform iamb_test.pull('tom');
    r := iamb_test.push('ines', jsonb_build_array(iamb_test.op_delete('folder', f3, iamb_test.rev('folder', f3),
        jsonb_build_object('cascade', jsonb_build_array(jsonb_build_object('entity', 'folder', 'id', f4, 'base_rev', iamb_test.rev('folder', f4)),
                                                         jsonb_build_object('entity', 'snippet', 'id', s4, 'base_rev', iamb_test.rev('snippet', s4)))))));
    perform iamb_test.eq(iamb_test.st(r), 'applied', 'the sub-folder is deleted, with what is in it');
    perform iamb_test.push('ines', jsonb_build_array(iamb_test.op_share(f2, g, 'edit')));
    -- The server keeps an access entry for each deleted folder, as for any folder below a share.
    select count(*) into n from iamb.folder_access where user_id = iamb_test.uid('tom') and folder_id in (f3, f4);
    perform iamb_test.eq(n, 2, 'the entries are there');
    -- Read directly as the member, nothing names the two folders or the snippet.
    got := iamb_test.named('tom', array[f3, f4, s4]);
    perform iamb_test.eq(got, '{}'::jsonb, 'tables and views that name a folder deleted before the member''s access began');
    perform iamb_test.act('tom');
    select count(*) into n from iamb.folder_access;
    perform iamb_test.act_owner();
    perform iamb_test.eq(n, 3, 'his own entries, read directly: the two shared folders and the live one below the first');
    -- A folder deleted while the member reads it: the entry stays his to read, with the tombstone.
    perform iamb_test.push('ines', jsonb_build_array(iamb_test.op_delete('folder', k1, iamb_test.rev('folder', k1),
        jsonb_build_object('cascade', jsonb_build_array(jsonb_build_object('entity', 'snippet', 'id', ks, 'base_rev', iamb_test.rev('snippet', ks)))))));
    got := iamb_test.named('tom', array[k1]);
    perform iamb_test.ok(got ? 'folder_access' and got ? 'folder' and got ? 'snippet' and got ? 'wire_rows' and got ? 'feed',
                         'a folder deleted while he could read it is his to read: ' || got::text);
    -- Who deleted them reads everything, as before.
    got := iamb_test.named('ines', array[f3, f4]);
    perform iamb_test.ok((got ->> 'folder_access')::int = 2 and (got ->> 'folder')::int = 2 and got ? 'snippet' and got ? 'wire_rows',
                         'who deleted the folders: ' || got::text);
    -- Somebody who joins the group later reads the shared folders and none of the deleted ones.
    r := iamb_test.call('ines', format('api.invite(iamb_test.dev(''ines''), 1, %L, %L, %L, ''member'')', inv, g, iamb_test.email('nora')));
    r := iamb_test.call('nora', format('api.accept_invitation(iamb_test.dev(''nora''), 1, %L)', inv));
    perform iamb_test.ok(r ->> 'error' is null, 'Nora joins: ' || r::text);
    perform iamb_test.eq(iamb_test.named('nora', array[f3, f4, s4, k1, ks]), '{}'::jsonb, 'what names any of them for a member who came later');
    perform iamb_test.act('nora');
    select count(*) into n from iamb.folder_access;
    perform iamb_test.act_owner();
    perform iamb_test.eq(n, 2, 'her entries, read directly: the two live folders');
    -- A deleted folder that comes back is there for everybody who reads the folder above it.
    r := iamb_test.push('ines', jsonb_build_array(iamb_test.op_folder(f3, p, f2, 'Letters', iamb_test.rev('folder', f3))));
    perform iamb_test.eq(iamb_test.st(r), 'applied', 'the owner restores the sub-folder');
    got := iamb_test.named('nora', array[f3]);
    perform iamb_test.ok(got ? 'folder_access' and got ? 'folder' and got ? 'feed', 'restored, it is hers to read, with its entry: ' || got::text);
    perform iamb_test.eq(iamb_test.named('nora', array[f4, s4]), '{}'::jsonb, 'and what was deleted with it stays away');
    perform iamb_test.check_all();
end $$;

-- An entry that gives nothing gives no share, no profile and no grant. A share that waits on a
-- deleted folder cannot be made any more; one that an older server accepted is put here by hand.
do $$
declare
    ids jsonb := iamb_test.family('edit'); g uuid := (ids ->> 'g')::uuid; p uuid := (ids ->> 'p_ines')::uuid;
    f1 uuid := (ids ->> 'f1')::uuid; f2 uuid := (ids ->> 'f2')::uuid; s2 uuid := (ids ->> 's2')::uuid;
    sh uuid := iamb_private.share_id((ids ->> 'f2')::uuid, (ids ->> 'g')::uuid); r jsonb; got jsonb; snap jsonb; pl jsonb;
begin
    -- Nothing else of Ines's is shared with the member.
    perform iamb_test.push('ines', jsonb_build_array(iamb_test.op_delete('folder_share', iamb_private.share_id(f1, g), 0)));
    perform iamb_test.push('ines', jsonb_build_array(iamb_test.op_share(f2, g, 'edit')));
    perform iamb_test.push('ines', jsonb_build_array(iamb_test.op_delete('folder', f2, iamb_test.rev('folder', f2),
        jsonb_build_object('cascade', jsonb_build_array(jsonb_build_object('entity', 'snippet', 'id', s2, 'base_rev', iamb_test.rev('snippet', s2)))))));
    perform iamb_test.pull('tom');
    -- The share is put again, on the deleted folder: refused, as a row put into a deleted folder
    -- its writer could not bring back.
    r := iamb_test.push('ines', jsonb_build_array(iamb_test.op_share(f2, g, 'edit', iamb_test.rev('folder_share', sh))));
    perform iamb_test.eq(iamb_test.st(r), 'rejected:parent_gone', 'a share of a deleted folder');
    perform iamb_test.ok((select deleted_at is not null from iamb.folder_share where id = sh), 'the share stays deleted');
    -- What an older server left: the share live on the deleted folder, and the entries it gives.
    update iamb.folder_share set deleted_at = null where id = sh;
    perform iamb_private.refresh_access(array[p]);
    perform iamb_private.feed_flush();
    perform iamb_test.ok(exists (select 1 from iamb.folder_access where user_id = iamb_test.uid('tom') and folder_id = f2), 'the member has an entry for the deleted folder');
    -- (His feed keeps the entries of the time when he read the folder: that it became visible,
    -- and that it was revoked. They are his, and a device needs them.)
    got := iamb_test.named('tom', array[f2, p]) - 'feed';
    perform iamb_test.eq(got, '{}'::jsonb, 'read directly, no row names the deleted folder or its owner''s profile to the member');
    pl := iamb_test.pull('tom');
    snap := iamb_test.snapshot('tom');
    perform iamb_test.ok(pl::text not like '%' || f2 || '%' and snap::text not like '%' || f2 || '%'
                         and not exists (select 1 from jsonb_array_elements(snap) e where e ->> 'id' = p::text),
                         'and neither does a pull or a snapshot');
    -- Such an entry is no grant in the profile: a push that names the profile is answered as for
    -- one that is not there, and takes no lock on it.
    perform iamb_test.ok(not iamb_private.has_grant(iamb_test.uid('tom'), p) and not iamb_private.writes_in(iamb_test.uid('tom'), p),
                         'no grant, and nothing to lock the profile for');
    r := iamb_test.push('tom', jsonb_build_array(iamb_test.op_snippet(gen_random_uuid(), f2, ';x', 'x'), iamb_test.op_tag(gen_random_uuid(), p, 'x')));
    perform iamb_test.ok(iamb_test.st(r, 1) = 'rejected:parent_gone' and iamb_test.st(r, 2) = 'rejected:parent_gone', 'his writes there: ' || iamb_test.st(r, 1) || ', ' || iamb_test.st(r, 2));
    -- The owner reads her share and her folder.
    got := iamb_test.named('ines', array[f2]);
    perform iamb_test.ok(got ? 'folder_access' and got ? 'folder' and got ? 'folder_share', 'the owner: ' || got::text);
    perform iamb_test.check_all();
end $$;

-- A folder made and deleted in one request. A device that creates a folder and deletes a folder
-- above it, offline, sends both in one push. The others who read the tree could never read the
-- new folder: no entry of their feed names it. Who made and deleted it reads its tombstone.
do $$
declare
    ids jsonb := iamb_test.family('edit'); p uuid := (ids ->> 'p_ines')::uuid; f1 uuid := (ids ->> 'f1')::uuid;
    top uuid := gen_random_uuid(); x uuid := gen_random_uuid(); sx uuid := gen_random_uuid(); r jsonb; pl jsonb; got jsonb;
begin
    perform iamb_test.new_device('ines2', 'ines'); perform iamb_test.hello('ines2');
    perform iamb_test.push('ines', jsonb_build_array(iamb_test.op_folder(top, p, f1, 'Returns')));
    perform iamb_test.pull('tom'); perform iamb_test.pull('ines2');
    r := iamb_test.push('ines', jsonb_build_array(
        iamb_test.op_folder(x, p, top, 'Made offline'), iamb_test.op_snippet(sx, x, ';off', 'Made offline too.'),
        iamb_test.op_delete('folder', top, iamb_test.rev('folder', top), jsonb_build_object('cascade', jsonb_build_array(
            jsonb_build_object('entity', 'folder', 'id', x, 'base_rev', 0), jsonb_build_object('entity', 'snippet', 'id', sx, 'base_rev', 0))))));
    perform iamb_test.ok(iamb_test.st(r, 1) = 'applied' and iamb_test.st(r, 3) = 'applied', 'one push: ' || left(r::text, 200));
    got := iamb_test.named('tom', array[x, sx]);
    perform iamb_test.eq(got, '{}'::jsonb, 'read directly, what names the folder made and deleted in one request to the member');
    pl := iamb_test.pull('tom');
    perform iamb_test.ok(not exists (select 1 from jsonb_array_elements(pl -> 'entries') e where (e ->> 'id')::uuid in (x, sx) or e ->> 'folder' = x::text),
                         'no entry of his feed names it: ' || left((pl -> 'entries')::text, 300));
    perform iamb_test.ok(exists (select 1 from jsonb_array_elements(pl -> 'entries') e where e ->> 'id' = top::text and e -> 'row' ->> 'deleted_at' is not null),
                         'the folder he could read arrives as a tombstone');
    -- Her other device gets the tombstones of what she made.
    pl := iamb_test.pull('ines2');
    perform iamb_test.eq((select count(*)::integer from jsonb_array_elements(pl -> 'entries') e where (e ->> 'id')::uuid in (x, sx) and e -> 'row' ->> 'deleted_at' is not null), 2,
                         'her other device is told of both');
    perform iamb_test.check_all();
end $$;

-- The parent of a folder's version in a conflict. A member renames a sub-folder of a shared
-- folder and a conflict keeps the version his write replaced, which names the shared folder as
-- its parent. The sub-folder is then shared by itself and moved out, the folder it was in is
-- deleted, and a folder above both is shared: the member holds an entry for the deleted one, which
-- gives nothing, and the version he is offered names no parent, as for any folder he cannot read.
do $$
declare
    ids jsonb := iamb_test.family('edit'); g uuid := (ids ->> 'g')::uuid; p uuid := (ids ->> 'p_ines')::uuid;
    a uuid := gen_random_uuid(); rt uuid := gen_random_uuid(); c uuid := gen_random_uuid(); base bigint; r jsonb; cf uuid;
begin
    perform iamb_test.push('ines', jsonb_build_array(
        iamb_test.op_folder(a, p, null, 'Above'), iamb_test.op_folder(rt, p, a, 'Shared'), iamb_test.op_folder(c, p, rt, 'Sub-folder'),
        iamb_test.op_share(rt, g, 'edit')));
    perform iamb_test.pull('tom');
    base := iamb_test.rev('folder', c);
    perform iamb_test.push('ines', jsonb_build_array(iamb_test.op_folder(c, p, rt, 'Renamed by the owner', base)));
    r := iamb_test.push('tom', jsonb_build_array(iamb_test.op_folder(c, p, rt, 'Renamed by the member', base)));
    cf := (r -> 'results' -> 0 ->> 'conflict')::uuid;
    perform iamb_test.ok(cf is not null, 'the member''s stale rename records a conflict: ' || iamb_test.st(r));
    r := iamb_test.call('tom', format('api.get_conflict(iamb_test.dev(''tom''), 1, %L)', cf));
    perform iamb_test.ok(jsonb_array_length(r -> 'versions') = 1 and r -> 'versions' -> 0 -> 'row' ->> 'parent_id' = rt::text,
                         'the version he is offered names its parent, which he reads');
    -- The sub-folder is shared by itself and moves to the top; the folder it was in is deleted
    -- (his access to it ends); the folder above is shared (an entry for the deleted one begins).
    r := iamb_test.push('ines', jsonb_build_array(iamb_test.op_share(c, g, 'edit')));
    r := iamb_test.push('ines', jsonb_build_array(iamb_test.op_folder(c, p, null, 'Renamed by the member', iamb_test.rev('folder', c))));
    perform iamb_test.eq(iamb_test.st(r), 'applied', 'the sub-folder moves to the top');
    r := iamb_test.push('ines', jsonb_build_array(iamb_test.op_delete('folder', rt, iamb_test.rev('folder', rt), '{"cascade": []}')));
    perform iamb_test.eq(iamb_test.st(r), 'applied', 'the folder it was in is deleted');
    perform iamb_test.pull('tom');
    r := iamb_test.push('ines', jsonb_build_array(iamb_test.op_share(a, g, 'view')));
    perform iamb_test.eq(iamb_test.st(r), 'applied', 'the folder above is shared');
    perform iamb_test.ok(exists (select 1 from iamb.folder_access where user_id = iamb_test.uid('tom') and folder_id = rt), 'he has an entry for the deleted folder');
    r := iamb_test.call('tom', format('api.get_conflict(iamb_test.dev(''tom''), 1, %L)', cf));
    perform iamb_test.ok(r ->> 'error' is null and jsonb_array_length(r -> 'versions') = 1 and r -> 'versions' -> 0 -> 'row' -> 'parent_id' = 'null'::jsonb,
                         'the version names no parent now: ' || left((r -> 'versions')::text, 300));
    perform iamb_test.eq(iamb_test.named('tom', array[rt]) - 'feed', '{}'::jsonb, 'and no row he can read directly names the deleted folder');
    perform iamb_test.check_all();
end $$;

-- A group's folder that was deleted before a member joined. When that member leaves, nothing is
-- revoked that she never read; a member who read the folder is told that it is gone.
do $$
declare
    ids jsonb := iamb_test.family('edit'); g uuid := (ids ->> 'g')::uuid;
    gf uuid := gen_random_uuid(); gd uuid := gen_random_uuid(); inv uuid := gen_random_uuid(); r jsonb; pl jsonb; n integer;
begin
    perform iamb_test.new_user('nora'); perform iamb_test.hello('nora', 'Nora');
    perform iamb_test.push('ines', jsonb_build_array(
        iamb_test.op_folder(gf, g, null, 'Family', 0, jsonb_build_object('row', jsonb_build_object('member_permission', 'edit'))),
        iamb_test.op_folder(gd, g, null, 'Old plans', 0, jsonb_build_object('row', jsonb_build_object('member_permission', 'edit')))));
    perform iamb_test.pull('tom');
    perform iamb_test.push('ines', jsonb_build_array(iamb_test.op_delete('folder', gd, iamb_test.rev('folder', gd), '{"cascade": []}')));
    r := iamb_test.call('ines', format('api.invite(iamb_test.dev(''ines''), 1, %L, %L, %L, ''member'')', inv, g, iamb_test.email('nora')));
    r := iamb_test.call('nora', format('api.accept_invitation(iamb_test.dev(''nora''), 1, %L)', inv));
    perform iamb_test.ok(r ->> 'error' is null, 'Nora joins: ' || r::text);
    perform iamb_test.pull('nora'); perform iamb_test.pull('tom');
    perform iamb_test.eq(iamb_test.named('nora', array[gd]), '{}'::jsonb, 'what names the folder deleted before she joined');
    -- With `edit` on a deleted folder alone she could write nothing in a profile: such an entry
    -- is nothing to take the profile's lock for. (Here the group has a live folder too.)
    perform iamb_test.ok(iamb_private.writes_in(iamb_test.uid('nora'), g), 'she can write in the group, through its live folder');
    -- Both leave. Nora's feed names the live folder; Tom's names both, and he held both.
    r := iamb_test.call('nora', format('api.leave_group(iamb_test.dev(''nora''), 1, %L)', g));
    perform iamb_test.ok(r ->> 'error' is null, 'Nora leaves: ' || r::text);
    pl := iamb_test.pull('nora');
    perform iamb_test.eq((select array_agg((e ->> 'folder')::uuid order by e ->> 'folder') from jsonb_array_elements(pl -> 'entries') e where e ->> 'kind' = 'folder_revoked'),
                         (select array_agg(x order by x::text) from unnest(array[gf, (ids ->> 'f1')::uuid]) x),
                         'what is revoked for the member who came later: the group''s live folder and the folder shared with the group');
    perform iamb_test.eq(iamb_test.named('nora', array[gd]), '{}'::jsonb, 'read directly, nothing names the deleted folder to her afterwards either');
    r := iamb_test.call('ines', format('api.remove_member(iamb_test.dev(''ines''), 1, %L, %L)', g, iamb_test.uid('tom')));
    perform iamb_test.ok(r ->> 'error' is null, 'Tom is removed: ' || r::text);
    pl := iamb_test.pull('tom');
    select count(*) into n from jsonb_array_elements(pl -> 'entries') e where e ->> 'kind' = 'folder_revoked' and (e ->> 'folder')::uuid in (gf, gd);
    perform iamb_test.eq(n, 2, 'a member who read the deleted folder is told that it is revoked, with the live one');
    perform iamb_test.ok(pl::text like '%' || gd || '%', 'his feed names it, as it must');
    perform iamb_test.check_all();
end $$;
