-- Security review 4.3, findings 7 and 10 (and the answers of finding 6): what `push` does with
-- input a device should never send, what one request can cost, and what its answers tell about
-- rows and profiles that are not the caller's.

-- Finding 7 (a): input of the wrong type is refused as data, op by op; the request does not end in
-- an SQL error, the other ops are applied, and the call is counted by the rate limiter.
do $$
declare
    ids jsonb := iamb_test.family('edit'); f uuid := gen_random_uuid(); s uuid := gen_random_uuid(); r jsonb;
    counted integer; ops jsonb; bad jsonb; i integer; n integer;
begin
    perform iamb_test.pull('stranger');
    perform iamb_test.push('stranger', jsonb_build_array(iamb_test.op_folder(f, iamb_test.personal('stranger'), null, 'own')));
    select count into counted from iamb.rate_bucket where subject = iamb_test.dev('stranger') and bucket = 'push_calls';
    ops := jsonb_build_array(
        jsonb_build_object('op_id', gen_random_uuid(), 'entity', 'tag', 'id', gen_random_uuid(), 'kind', 'delete', 'base_rev', 'x'),
        jsonb_build_object('op_id', gen_random_uuid(), 'entity', 'tag', 'id', gen_random_uuid(), 'kind', 'delete', 'base_rev', 1.5),
        jsonb_build_object('op_id', gen_random_uuid(), 'entity', 'tag', 'id', gen_random_uuid(), 'kind', 'delete', 'base_rev', -1),
        jsonb_build_object('op_id', gen_random_uuid(), 'entity', 'tag', 'id', gen_random_uuid(), 'kind', 'delete', 'base_rev', 1e30),
        jsonb_build_object('op_id', gen_random_uuid(), 'entity', 'tag', 'id', gen_random_uuid(), 'kind', 'delete', 'follows', 'x'),
        jsonb_build_object('op_id', gen_random_uuid(), 'entity', 'tag', 'id', gen_random_uuid(), 'kind', 'delete', 'follows', jsonb_build_object('a', 1)),
        jsonb_build_object('op_id', gen_random_uuid(), 'entity', 'folder', 'id', gen_random_uuid(), 'kind', 'delete', 'cascade', 'x'),
        jsonb_build_object('op_id', gen_random_uuid(), 'entity', 'snippet', 'id', gen_random_uuid(), 'kind', 'put', 'repush', 'row', 'repush_rev', 'x',
                           'row', iamb_test.op_snippet(gen_random_uuid(), f, ';a', 'a') -> 'row'),
        iamb_test.op_snippet(gen_random_uuid(), f, ';a', 'a', 0, '{"row": {"created_at": 1e40}}'),
        iamb_test.op_folder(gen_random_uuid(), iamb_test.personal('stranger'), null, 'p', 0, '{"row": {"position": 1e25}}'),
        iamb_test.op_attachment(gen_random_uuid(), iamb_test.personal('stranger'), repeat('ab', 32), 0) #- '{row,size}' || '{}',
        '"not an object"'::jsonb, '7'::jsonb, 'null'::jsonb, '[]'::jsonb,
        -- JSON null counts as absent, as it does for a device that writes every key.
        iamb_test.op_snippet(s, f, ';ok', 'the one good op') || '{"follows": null, "cascade": null, "repush_rev": null}');
    n := jsonb_array_length(ops);
    r := iamb_test.push('stranger', ops);
    perform iamb_test.ok(r ->> 'error' is null, 'the request is answered, not ended in an error: ' || left(r::text, 300));
    for i in 1..n - 1 loop
        perform iamb_test.ok(iamb_test.st(r, i) in ('blocked:invalid'), 'op ' || i || ' is blocked: ' || iamb_test.st(r, i));
    end loop;
    perform iamb_test.eq(iamb_test.st(r, n), 'applied', 'the good op of the same request is applied');
    perform iamb_test.ok(exists (select 1 from iamb.snippet where id = s), 'and written');
    perform iamb_test.eq((select count from iamb.rate_bucket where subject = iamb_test.dev('stranger') and bucket = 'push_calls'),
                         counted + 1, 'the request was counted');
    -- A numeric position or size that is too large for the column.
    perform iamb_test.eq((select count(*)::integer from iamb.folder where profile_id = iamb_test.personal('stranger')), 1, 'nothing else written');
    perform iamb_test.check_all();
end $$;

-- Finding 7 (b): a request that does end in an SQL error is still counted: the error is caught, the
-- request's ops are undone, and the answer is data. (The error is planted here; no input is known
-- to cause one.)
do $$
declare
    f uuid := gen_random_uuid(); r jsonb; counted integer;
begin
    delete from iamb_test.actor where true;
    perform iamb_test.new_user('ines'); perform iamb_test.hello('ines', 'Ines');
    perform iamb_test.push('ines', jsonb_build_array(iamb_test.op_folder(f, iamb_test.personal('ines'), null, 'F')));
    alter table iamb.snippet add constraint iamb_test_planted check (body <> 'planted error');
    select count into counted from iamb.rate_bucket where subject = iamb_test.dev('ines') and bucket = 'push_calls';
    r := iamb_test.push('ines', jsonb_build_array(iamb_test.op_snippet(gen_random_uuid(), f, ';one', 'fine'),
                                                   iamb_test.op_snippet(gen_random_uuid(), f, ';two', 'planted error')));
    alter table iamb.snippet drop constraint iamb_test_planted;
    perform iamb_test.eq(r ->> 'error', 'invalid', 'an SQL error inside a push is answered as data');
    perform iamb_test.eq((select count(*)::integer from iamb.snippet where folder_id = f), 0, 'the request''s ops are undone');
    perform iamb_test.eq((select count from iamb.rate_bucket where subject = iamb_test.dev('ines') and bucket = 'push_calls'),
                         counted + 1, 'the failed request was counted');
    r := iamb_test.push('ines', jsonb_build_array(iamb_test.op_snippet(gen_random_uuid(), f, ';one', 'fine')));
    perform iamb_test.eq(iamb_test.st(r), 'applied', 'the device goes on');
    perform iamb_test.check_all();
end $$;

-- Finding 7 (c): a cascade list is bounded in number (20,000, §6.3), in time and in bytes.
do $$
declare
    df uuid := gen_random_uuid(); big uuid := gen_random_uuid(); r jsonb; t0 timestamptz; list jsonb; ms numeric; sn uuid[];
    whole integer; short integer;
begin
    delete from iamb_test.actor where true;
    perform iamb_test.new_user('ines'); perform iamb_test.hello('ines', 'Ines');
    update iamb.server_state set rate_limits = rate_limits || '{"push_calls": 1000, "push_ops": 100000}' where id;
    perform iamb_test.push('ines', jsonb_build_array(iamb_test.op_folder(df, iamb_test.personal('ines'), null, 'd'),
                                                     iamb_test.op_folder(big, iamb_test.personal('ines'), null, 'big')));
    perform iamb_test.push('ines', jsonb_build_array(iamb_test.op_delete('folder', df, 0)));
    -- 20,001 rows: refused before any work.
    select jsonb_agg(jsonb_build_object('entity', 'snippet', 'id', gen_random_uuid())) into list from generate_series(1, 20001);
    r := iamb_test.push('ines', jsonb_build_array(iamb_test.op_delete('folder', df, 0, jsonb_build_object('cascade', list))));
    perform iamb_test.eq(iamb_test.st(r), 'blocked:too_large', 'a cascade list over 20,000 rows');
    perform iamb_test.ok(not r -> 'results' -> 0 ? 'cascade', 'and no answer for its rows');
    -- 20,000 unknown ids, which the design allows: answered, and in far less than push's 20 s
    -- (it took 41 s before, and 11 s for 10,000).
    list := list - 0;
    t0 := clock_timestamp();
    r := iamb_test.push('ines', jsonb_build_array(iamb_test.op_delete('folder', df, 0, jsonb_build_object('cascade', list))));
    ms := extract(epoch from clock_timestamp() - t0) * 1000;
    perform iamb_test.eq(iamb_test.st(r), 'unchanged', 'a cascade list of 20,000 rows');
    perform iamb_test.eq(jsonb_array_length(r -> 'results' -> 0 -> 'cascade'), 20000, 'one answer for every row of the list');
    perform iamb_test.ok(r -> 'results' -> 0 -> 'cascade' -> 0 = jsonb_build_object('entity', 'snippet', 'id', list -> 0 -> 'id', 'gone', true), 'each one gone');
    perform iamb_test.ok(ms < 5000, 'in ' || round(ms) || ' ms');
    perform iamb_test.put('cascade_20000_ms', to_jsonb(round(ms)));
    -- The answer stays within the 4 MiB of a request: 30 of the caller's own snippets of 390 kB,
    -- named with a base they do not have, would be 11.7 MB of whole rows. The first ones come
    -- whole, the others in the short form with `fetch`. (A list that names a row twice is refused
    -- since the re-verification: `29_reverify_push`.)
    sn := array(select gen_random_uuid() from generate_series(1, 30));
    for i in 0..2 loop
        perform iamb_test.push('ines', (select jsonb_agg(iamb_test.op_snippet(x, big, ';b' || o, repeat('y', 390000)))
                                        from unnest(sn[i * 10 + 1 : i * 10 + 10]) with ordinality as t(x, o)));
    end loop;
    select jsonb_agg(jsonb_build_object('entity', 'snippet', 'id', x, 'base_rev', 1)) into list
    from unnest(sn) x;
    r := iamb_test.push('ines', jsonb_build_array(iamb_test.op_delete('folder', df, 0, jsonb_build_object('cascade', list))));
    perform iamb_test.eq(jsonb_array_length(r -> 'results' -> 0 -> 'cascade'), 30, '30 answers');
    select count(*) filter (where e ? 'row'), count(*) filter (where (e ->> 'fetch')::boolean and not e ? 'row')
    into whole, short from jsonb_array_elements(r -> 'results' -> 0 -> 'cascade') e;
    perform iamb_test.ok(whole between 1 and 10 and whole + short = 30, whole || ' whole rows and ' || short || ' short ones');
    perform iamb_test.ok(octet_length(r::text) < 4194304, 'the answer has ' || octet_length(r::text) || ' bytes');
    perform iamb_test.ok(not exists (select 1 from jsonb_array_elements(r -> 'results' -> 0 -> 'cascade') e
                                     where (e ->> 'fetch')::boolean and ((e ->> 'deleted')::boolean or not e ? 'rev')),
                         'a short entry still says the row''s revision and that it is live');
    update iamb.server_state set rate_limits = rate_limits || '{"push_calls": 60, "push_ops": 6000}' where id;
end $$;

-- Finding 10 (and 6): an op on a profile, a group or a row that is not the caller's is answered
-- like one on a profile, a group or a row the server does not know.
do $$
declare
    ids jsonb := iamb_test.family('edit'); g uuid := (ids ->> 'g')::uuid; p_ines uuid := (ids ->> 'p_ines')::uuid; rf uuid := gen_random_uuid();
    one jsonb; two jsonb;
begin
    perform iamb_test.pull('tom');
    perform iamb_test.call('ines', format('api.remove_member(iamb_test.dev(''ines''), 1, %L, %L)', g, iamb_test.uid('tom')));
    perform iamb_test.pull('tom');
    perform iamb_test.push('tom', jsonb_build_array(iamb_test.op_folder(rf, iamb_test.personal('tom'), null, 'own')));
    -- Tom was removed. What the answers still tell him about ids he remembers: nothing.
    one := iamb_test.push('tom', jsonb_build_array(iamb_test.op_tag(gen_random_uuid(), g, 'x'), iamb_test.op_tag(gen_random_uuid(), p_ines, 'x'),
                                                    iamb_test.op_attachment(gen_random_uuid(), p_ines, repeat('ab', 32))));
    two := iamb_test.push('tom', jsonb_build_array(iamb_test.op_tag(gen_random_uuid(), gen_random_uuid(), 'x')));
    perform iamb_test.ok(iamb_test.st(one, 1) = iamb_test.st(two) and iamb_test.st(one, 2) = iamb_test.st(two) and iamb_test.st(one, 3) = iamb_test.st(two)
                         and iamb_test.st(two) = 'rejected:parent_gone',
                         'a tag or attachment put that names a profile: the group ' || iamb_test.st(one, 1) || ', her profile ' || iamb_test.st(one, 2)
                         || ', an unknown id ' || iamb_test.st(two));
    one := iamb_test.push('tom', jsonb_build_array(iamb_test.op_delete('snippet', (ids ->> 's2')::uuid), iamb_test.op_delete('snippet', (ids ->> 's1')::uuid),
                                                    iamb_test.op_delete('folder', (ids ->> 'f2')::uuid, 0, '{"cascade": []}'),
                                                    iamb_test.op_delete('folder_share', iamb_private.share_id((ids ->> 'f1')::uuid, g))));
    two := iamb_test.push('tom', jsonb_build_array(iamb_test.op_delete('snippet', gen_random_uuid())));
    perform iamb_test.ok(iamb_test.st(one, 1) = 'unchanged' and iamb_test.st(one, 2) = 'unchanged' and iamb_test.st(one, 3) = 'unchanged'
                         and iamb_test.st(one, 4) = 'unchanged' and iamb_test.st(two) = 'unchanged',
                         'a delete: her private snippet ' || iamb_test.st(one, 1) || ', the snippet he once read ' || iamb_test.st(one, 2)
                         || ', her folder ' || iamb_test.st(one, 3) || ', her share ' || iamb_test.st(one, 4) || ', an unknown id ' || iamb_test.st(two));
    perform iamb_test.ok((one -> 'results' -> 0) - 'op_id' = (two -> 'results' -> 0) - 'op_id', 'the whole result is the same, field for field');
    perform iamb_test.ok((select deleted_at is null from iamb.snippet where id = (ids ->> 's1')::uuid)
                         and (select deleted_at is null from iamb.folder where id = (ids ->> 'f2')::uuid)
                         and (select deleted_at is null from iamb.folder_share where id = iamb_private.share_id((ids ->> 'f1')::uuid, g)), 'nothing was deleted');
    one := iamb_test.push('tom', jsonb_build_array(iamb_test.op_share(rf, g, 'view')));
    two := iamb_test.push('tom', jsonb_build_array(iamb_test.op_share(rf, gen_random_uuid(), 'view')));
    perform iamb_test.ok(iamb_test.st(one) = 'rejected:parent_gone' and iamb_test.st(two) = 'rejected:parent_gone',
                         'a share with a group: the one he left ' || iamb_test.st(one) || ', an unknown id ' || iamb_test.st(two));
    -- A member with `view` in a profile is told `forbidden`, as before: the profile is no secret to him.
    perform iamb_test.new_user('vera'); perform iamb_test.hello('vera', 'Vera');
    perform iamb_test.call('ines', format('api.invite(iamb_test.dev(''ines''), 1, %L, %L, %L, ''member'')', rf, g, iamb_test.email('vera')));
    perform iamb_test.call('vera', format('api.accept_invitation(iamb_test.dev(''vera''), 1, %L)', rf));
    perform iamb_test.push('ines', jsonb_build_array(iamb_test.op_share((ids ->> 'f1')::uuid, g, 'view', iamb_test.rev('folder_share', iamb_private.share_id((ids ->> 'f1')::uuid, g)))));
    perform iamb_test.pull('vera');
    one := iamb_test.push('vera', jsonb_build_array(iamb_test.op_tag(gen_random_uuid(), p_ines, 'x'), iamb_test.op_delete('snippet', (ids ->> 's1')::uuid, iamb_test.rev('snippet', (ids ->> 's1')::uuid))));
    perform iamb_test.ok(iamb_test.st(one, 1) = 'rejected:forbidden' and iamb_test.st(one, 2) = 'rejected:forbidden',
                         'a member who may only read: ' || iamb_test.st(one, 1) || ', ' || iamb_test.st(one, 2));
    perform iamb_test.check_all();
end $$;
