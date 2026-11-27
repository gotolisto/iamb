-- Re-verification of the security review (docs/reviews/4.3-security-reverify.md), N2, N3 (the part
-- one connection can show) and N4: what one push may cost, and where a restored folder goes.

-- N2: the work for a cascade answer is bounded, not only the answer. An account with ten large
-- snippets of her own (4 MB, inside every limit) and no group.
select iamb_test.put('temp0', to_jsonb((select temp_bytes from pg_stat_database where datname = current_database()))) is not null;
do $$
declare
    sf uuid := gen_random_uuid(); sn uuid[] := array(select gen_random_uuid() from generate_series(1, 10));
    r jsonb; list jsonb; whole integer; short integer; t0 timestamptz; counted integer;
begin
    delete from iamb_test.actor where true;
    perform iamb_test.new_user('mallory'); perform iamb_test.hello('mallory', 'Mallory');
    perform iamb_test.push('mallory', jsonb_build_array(iamb_test.op_folder(sf, iamb_test.personal('mallory'), null, 'own')));
    perform iamb_test.push('mallory', (select jsonb_agg(iamb_test.op_snippet(x, sf, ';b' || o, repeat('x', 390000)))
                                       from unnest(sn) with ordinality as t(x, o)));
    -- A list that names each of them a hundred times: 1,000 entries, 84 kB of request. Before, the
    -- server read a thousand whole rows for it (383 MB of temporary files; 20,000 entries took
    -- the disk). A device names each row it deleted once: the op is refused before any work.
    select jsonb_agg(jsonb_build_object('entity', 'snippet', 'id', x, 'base_rev', 0)) into list
    from unnest(sn) x, generate_series(1, 100);
    select count into counted from iamb.rate_bucket where subject = iamb_test.dev('mallory') and bucket = 'push_calls';
    t0 := clock_timestamp();
    r := iamb_test.push('mallory', jsonb_build_array(iamb_test.op_delete('folder', gen_random_uuid(), 0, jsonb_build_object('cascade', list))));
    perform iamb_test.eq(iamb_test.st(r), 'blocked:invalid', 'a cascade list that names a row twice');
    perform iamb_test.ok(not r -> 'results' -> 0 ? 'cascade', 'and no answer for its rows');
    perform iamb_test.ok(clock_timestamp() - t0 < interval '2 seconds', 'at once');
    perform iamb_test.eq((select count from iamb.rate_bucket where subject = iamb_test.dev('mallory') and bucket = 'push_calls'),
                         counted + 1, 'the request was counted');
    -- Three ops in one request, each with a list of the ten rows (each row once per list): the
    -- cascade answers of one request share one allowance of whole rows.
    select jsonb_agg(jsonb_build_object('entity', 'snippet', 'id', x, 'base_rev', 1)) into list from unnest(sn) x;
    r := iamb_test.push('mallory', jsonb_build_array(
        iamb_test.op_delete('folder', gen_random_uuid(), 0, jsonb_build_object('cascade', list)),
        iamb_test.op_delete('folder', gen_random_uuid(), 0, jsonb_build_object('cascade', list)),
        iamb_test.op_delete('folder', gen_random_uuid(), 0, jsonb_build_object('cascade', list))));
    perform iamb_test.ok(iamb_test.st(r, 1) = 'unchanged' and iamb_test.st(r, 2) = 'unchanged' and iamb_test.st(r, 3) = 'unchanged', 'three lists: ' || left(r::text, 200));
    select count(*) filter (where e ? 'row'), count(*) filter (where (e ->> 'fetch')::boolean and not e ? 'row')
    into whole, short from jsonb_array_elements(r -> 'results') x, jsonb_array_elements(x -> 'cascade') e;
    perform iamb_test.ok(whole between 1 and 10 and whole + short = 30, whole || ' whole rows and ' || short || ' short ones in the whole request');
    perform iamb_test.ok(octet_length(r::text) < 4194304, 'the answer of the request has ' || octet_length(r::text) || ' bytes');
    perform iamb_test.ok(not exists (select 1 from jsonb_array_elements(r -> 'results') x, jsonb_array_elements(x -> 'cascade') e
                                     where (e ->> 'fetch')::boolean and ((e ->> 'deleted')::boolean or not e ? 'rev')),
                         'a short entry says the row''s revision and that it is live');
    -- The next request has its own allowance.
    r := iamb_test.push('mallory', jsonb_build_array(iamb_test.op_delete('folder', gen_random_uuid(), 0, jsonb_build_object('cascade', list))));
    select count(*) filter (where e ? 'row') into whole from jsonb_array_elements(r -> 'results' -> 0 -> 'cascade') e;
    perform iamb_test.ok(whole >= 1, 'the next request gets whole rows again: ' || whole);
    perform iamb_test.check_all();
end $$;
-- The temporary files those requests wrote (locally; the statistics are not readable inside the
-- one transaction a hosted run is).
select pg_stat_force_next_flush();
select pg_stat_clear_snapshot();
do $$
declare used bigint;
begin
    if current_setting('iamb_test.hosted', true) = 'on' then return; end if;
    select temp_bytes - (iamb_test.get('temp0') #>> '{}')::bigint into used from pg_stat_database where datname = current_database();
    perform iamb_test.ok(used < 32 * 1024 * 1024, 'temporary files written by the cascade answers: ' || used || ' bytes');
end $$;

-- N2: a request that is cut off stays counted. The cut-offs are planted (a trigger that fails as
-- the database does when its temporary file space, or its time, is used up): nothing a device
-- sends is known to cause one any more.
do $$
declare
    f uuid := gen_random_uuid(); r jsonb; counted integer;
begin
    delete from iamb_test.actor where true;
    perform iamb_test.new_user('ines'); perform iamb_test.hello('ines', 'Ines');
    perform iamb_test.push('ines', jsonb_build_array(iamb_test.op_folder(f, iamb_test.personal('ines'), null, 'F')));
    create function iamb_test.planted() returns trigger language plpgsql as $t$
    begin
        if new.body = 'out of temporary file space' then
            raise exception 'temporary file size exceeds temp_file_limit' using errcode = '53400';
        elsif new.body = 'out of time' then
            raise exception 'canceling statement due to statement timeout' using errcode = '57014';
        end if;
        return new;
    end $t$;
    create trigger iamb_test_planted before insert on iamb.snippet for each row execute function iamb_test.planted();
    select count into counted from iamb.rate_bucket where subject = iamb_test.dev('ines') and bucket = 'push_calls';
    r := iamb_test.push('ines', jsonb_build_array(iamb_test.op_snippet(gen_random_uuid(), f, ';one', 'fine'),
                                                   iamb_test.op_snippet(gen_random_uuid(), f, ';two', 'out of temporary file space')));
    perform iamb_test.eq(r ->> 'error', 'too_large', 'a push that ran out of temporary file space is answered as data');
    perform iamb_test.eq((select count(*)::integer from iamb.snippet where folder_id = f), 0, 'its ops are undone');
    perform iamb_test.eq((select count from iamb.rate_bucket where subject = iamb_test.dev('ines') and bucket = 'push_calls'),
                         counted + 10, 'and it was counted, as ten calls');
    r := iamb_test.push('ines', jsonb_build_array(iamb_test.op_snippet(gen_random_uuid(), f, ';two', 'out of time')));
    perform iamb_test.eq(r ->> 'error', 'too_large', 'a push that was cut off by its time-out is answered as data');
    perform iamb_test.eq((select count from iamb.rate_bucket where subject = iamb_test.dev('ines') and bucket = 'push_calls'),
                         counted + 20, 'and counted');
    drop trigger iamb_test_planted on iamb.snippet;
    drop function iamb_test.planted();
    r := iamb_test.push('ines', jsonb_build_array(iamb_test.op_snippet(gen_random_uuid(), f, ';one', 'fine')));
    perform iamb_test.eq(iamb_test.st(r), 'applied', 'the device goes on');
    perform iamb_test.check_all();
end $$;

-- N3 (a), as far as one connection shows it: an op in a profile the caller can only read is
-- answered as before (the two-connection test that it holds no lock is `20_concurrency` (j)).
do $$
declare
    ids jsonb := iamb_test.family('view'); f1 uuid := (ids ->> 'f1')::uuid; f2 uuid := (ids ->> 'f2')::uuid;
    s1 uuid := (ids ->> 's1')::uuid; s2 uuid := (ids ->> 's2')::uuid; r jsonb; head bigint;
begin
    perform iamb_test.pull('tom');
    select head_rev into head from iamb.profile where id = (ids ->> 'p_ines')::uuid;
    r := iamb_test.push('tom', jsonb_build_array(
        iamb_test.op_snippet(gen_random_uuid(), f1, ';x', 'x'),                                   -- a create in the folder he may read
        iamb_test.op_snippet(s1, f1, ';hrs', 'changed', iamb_test.rev('snippet', s1)),            -- a change of a row he may read
        iamb_test.op_delete('snippet', s1, iamb_test.rev('snippet', s1)),                         -- its delete
        iamb_test.op_snippet(gen_random_uuid(), f2, ';x', 'x'),                                   -- a create in her private folder
        iamb_test.op_delete('snippet', s2),                                                       -- a delete of her private snippet
        iamb_test.op_delete('folder', f1, iamb_test.rev('folder', f1), '{"cascade": []}'),        -- the shared folder's delete
        iamb_test.op_tag(gen_random_uuid(), (ids ->> 'p_ines')::uuid, 'x')));                     -- a tag in her profile
    perform iamb_test.eq((select array_agg(iamb_test.st(r, i) order by i) from generate_series(1, 7) i),
                         array['rejected:forbidden', 'rejected:forbidden', 'rejected:forbidden', 'rejected:parent_gone',
                               'unchanged', 'rejected:forbidden', 'rejected:forbidden'],
                         'the answers to a reader''s ops are what they were');
    perform iamb_test.ok(r -> 'results' -> 1 -> 'row' ->> 'body' is not null and r -> 'results' -> 3 -> 'row' is null
                         or jsonb_typeof(r -> 'results' -> 3 -> 'row') = 'null',
                         'a refused op carries the row he may read, and no row he may not');
    perform iamb_test.eq((select head_rev from iamb.profile where id = (ids ->> 'p_ines')::uuid), head, 'nothing was written in her profile');
    perform iamb_test.check_all();
end $$;

-- N4: a deleted folder can lie deeper than four levels after a folder above it was moved. When
-- it is restored it goes under the nearest folder above that keeps four levels, as a new folder
-- does (C8): by a put, by a conflict's version, and when a row is put below it (C7).
do $$
declare
    p uuid; a uuid := gen_random_uuid(); b uuid := gen_random_uuid(); c uuid := gen_random_uuid();
    x uuid := gen_random_uuid(); y uuid := gen_random_uuid(); y2 uuid := gen_random_uuid(); z uuid := gen_random_uuid();
    s uuid := gen_random_uuid(); r jsonb; cf uuid := gen_random_uuid(); ylive bigint;
begin
    delete from iamb_test.actor where true;
    perform iamb_test.new_user('ines'); perform iamb_test.hello('ines', 'Ines');
    p := iamb_test.personal('ines');
    perform iamb_test.push('ines', jsonb_build_array(
        iamb_test.op_folder(a, p, null, 'A'), iamb_test.op_folder(b, p, a, 'B'), iamb_test.op_folder(c, p, b, 'C'),   -- three levels
        iamb_test.op_folder(x, p, null, 'X'), iamb_test.op_folder(y, p, x, 'Y'), iamb_test.op_folder(y2, p, x, 'Y2'),
        iamb_test.op_folder(z, p, x, 'Z'), iamb_test.op_snippet(s, z, ';s', 'in Z')));                                 -- X with Y, Y2 and Z
    ylive := iamb_test.rev('folder', y);
    perform iamb_test.push('ines', jsonb_build_array(
        iamb_test.op_delete('folder', y, iamb_test.rev('folder', y)), iamb_test.op_delete('folder', y2, iamb_test.rev('folder', y2)),
        iamb_test.op_delete('folder', z, iamb_test.rev('folder', z),
                            jsonb_build_object('cascade', jsonb_build_array(jsonb_build_object('entity', 'snippet', 'id', s, 'base_rev', iamb_test.rev('snippet', s)))))));
    r := iamb_test.push('ines', jsonb_build_array(iamb_test.op_folder(x, p, c, 'X', iamb_test.rev('folder', x))));
    perform iamb_test.ok(iamb_test.st(r) = 'applied' and (select parent_id from iamb.folder where id = x) = c,
                         'X, whose sub-folders are deleted, moves under C, to the fourth level: ' || iamb_test.st(r));
    perform iamb_test.check_tree();
    -- A put restores Y2 where it was: under X that would be a fifth level, so it goes under C.
    r := iamb_test.push('ines', jsonb_build_array(iamb_test.op_folder(y2, p, x, 'Y2', iamb_test.rev('folder', y2))));
    perform iamb_test.ok(r -> 'results' -> 0 ->> 'status' = 'applied' and r -> 'results' -> 0 -> 'adjusted' ? 'parent_id',
                         'the restore is applied and says that the parent was changed: ' || ((r -> 'results' -> 0) - 'row')::text);
    perform iamb_test.ok((select deleted_at is null and parent_id = c from iamb.folder where id = y2), 'Y2 is live, under C');
    perform iamb_test.eq(iamb_private.folder_depth(y2), 4, 'on the fourth level');
    perform iamb_test.check_tree();
    -- "Restore this version" of Y, from a conflict that offers the version it had before its delete.
    insert into iamb.conflict (id, entity, row_id, profile_id, kind, winner_rev, winner_user, winner_device, created_at, state, parties)
    values (cf, 'folder', y, p, 'delete_over_edit', iamb_test.rev('folder', y), iamb_test.uid('ines'), null, 1, 'open', array[iamb_test.uid('ines')]);
    insert into iamb.conflict_version (conflict_id, rev, user_id, device_id, taken_over) values (cf, ylive, iamb_test.uid('ines'), null, false);
    r := iamb_test.call('ines', format('api.resolve_conflict(iamb_test.dev(''ines''), 1, %L, ''restore'', %s)', cf, ylive));
    perform iamb_test.ok(r ->> 'error' is null and r ->> 'state' = 'resolved', 'the version is restored: ' || r::text);
    perform iamb_test.ok((select deleted_at is null and parent_id = c from iamb.folder where id = y), 'Y is live, under C');
    perform iamb_test.check_tree();
    -- A snippet put back into Z, which is deleted under X (C7): Z is restored under C as well.
    r := iamb_test.push('ines', jsonb_build_array(iamb_test.op_snippet(s, z, ';s', 'back in Z', iamb_test.rev('snippet', s))));
    perform iamb_test.eq(iamb_test.st(r), 'applied', 'a snippet put into the deleted folder');
    perform iamb_test.ok((select deleted_at is null and parent_id = c from iamb.folder where id = z)
                         and (select deleted_at is null and folder_id = z from iamb.snippet where id = s), 'Z is live under C, with the snippet in it');
    perform iamb_test.check_all();
end $$;

-- N4, the permission: a member with `edit` on a folder of the fourth level cannot have a deleted
-- sub-folder restored one level up, where he may not write.
do $$
declare
    ids jsonb := iamb_test.family('edit'); p uuid := (ids ->> 'p_ines')::uuid; g uuid := (ids ->> 'g')::uuid;
    a uuid := gen_random_uuid(); b uuid := gen_random_uuid(); c uuid := gen_random_uuid();
    x uuid := gen_random_uuid(); y uuid := gen_random_uuid(); s uuid := gen_random_uuid(); r jsonb;
begin
    perform iamb_test.push('ines', jsonb_build_array(
        iamb_test.op_folder(a, p, null, 'A'), iamb_test.op_folder(b, p, a, 'B'), iamb_test.op_folder(c, p, b, 'C'),
        iamb_test.op_folder(x, p, null, 'X'), iamb_test.op_folder(y, p, x, 'Y'), iamb_test.op_snippet(s, y, ';s', 'in Y'),
        iamb_test.op_share(x, g, 'edit')));
    perform iamb_test.pull('tom');
    perform iamb_test.push('tom', jsonb_build_array(iamb_test.op_delete('folder', y, iamb_test.rev('folder', y),
                                                                        jsonb_build_object('cascade', jsonb_build_array(jsonb_build_object('entity', 'snippet', 'id', s, 'base_rev', iamb_test.rev('snippet', s)))))));
    perform iamb_test.push('ines', jsonb_build_array(iamb_test.op_folder(x, p, c, 'X', iamb_test.rev('folder', x))));
    perform iamb_test.pull('tom');
    -- Tom has `edit` on X (now the fourth level) and no access to C.
    r := iamb_test.push('tom', jsonb_build_array(iamb_test.op_folder(y, p, x, 'Y', iamb_test.rev('folder', y))));
    perform iamb_test.eq(iamb_test.st(r), 'rejected:forbidden', 'the member''s restore of the sub-folder, which would land outside his tree');
    r := iamb_test.push('tom', jsonb_build_array(iamb_test.op_snippet(s, y, ';s', 'back', iamb_test.rev('snippet', s))));
    perform iamb_test.eq(iamb_test.st(r), 'rejected:parent_gone', 'and his put of a snippet into it (C7)');
    perform iamb_test.ok((select deleted_at is not null and parent_id = x from iamb.folder where id = y), 'the sub-folder is deleted, where it was');
    -- The owner may.
    r := iamb_test.push('ines', jsonb_build_array(iamb_test.op_snippet(s, y, ';s', 'back', iamb_test.rev('snippet', s))));
    perform iamb_test.ok(iamb_test.st(r) = 'applied' and (select deleted_at is null and parent_id = c from iamb.folder where id = y),
                         'the owner''s put restores it under C: ' || iamb_test.st(r));
    perform iamb_test.check_all();
end $$;
