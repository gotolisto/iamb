-- Security review 4.3, finding 18: a page of `snapshot` and of `fetch_folder` ends at about 4 MiB,
-- not only at 500 rows, and the cursor goes on from there: every row arrives once.
do $$
declare
    f uuid := gen_random_uuid(); r jsonb; ids uuid[]; got uuid[] := '{}'; cur text; pages integer := 0; biggest integer := 0;
    d uuid; i integer;
begin
    delete from iamb_test.actor where true;
    perform iamb_test.new_user('ines'); perform iamb_test.hello('ines', 'Ines');
    d := iamb_test.dev('ines');
    update iamb.server_state set rate_limits = rate_limits || '{"push_calls": 1000, "push_ops": 100000}' where id;
    perform iamb_test.push('ines', jsonb_build_array(iamb_test.op_folder(f, iamb_test.personal('ines'), null, 'Big')));
    ids := array(select gen_random_uuid() from generate_series(1, 40));
    for i in 0..3 loop                       -- 40 snippets of 390 kB: 15.6 MB
        perform iamb_test.push('ines', (select jsonb_agg(iamb_test.op_snippet(x, f, ';b' || o, repeat('y', 390000)))
                                        from unnest(ids[i * 10 + 1 : i * 10 + 10]) with ordinality as t(x, o)));
    end loop;
    -- fetch_folder
    loop
        r := iamb_test.call('ines', format('api.fetch_folder(%L, 1, %L, %L)', d, f, cur));
        perform iamb_test.ok(r ->> 'error' is null, 'fetch_folder: ' || left(r::text, 200));
        pages := pages + 1; biggest := greatest(biggest, octet_length(r::text));
        got := got || array(select (x ->> 'id')::uuid from jsonb_array_elements(r -> 'rows') x where x ->> 'entity' = 'snippet');
        cur := r ->> 'cursor';
        exit when cur is null or pages > 50;
    end loop;
    perform iamb_test.ok(biggest < 4800000, 'the largest page of fetch_folder has ' || biggest || ' bytes');
    perform iamb_test.ok(pages between 4 and 8, pages || ' pages');
    perform iamb_test.ok((select array_agg(x order by x) from unnest(got) x) = (select array_agg(x order by x) from unnest(ids) x),
                         'every snippet arrives exactly once');
    -- snapshot
    got := '{}'; cur := null; pages := 0; biggest := 0;
    loop
        r := iamb_test.call('ines', format('api.snapshot(%L, 1, %L, %L)', d, case when cur is null then iamb_test.pos('ines') end, cur));
        perform iamb_test.ok(r ->> 'error' is null, 'snapshot: ' || left(r::text, 200));
        if cur is null then update iamb_test.actor set position = r ->> 'start' where name = 'ines'; end if;
        pages := pages + 1; biggest := greatest(biggest, octet_length(r::text));
        got := got || array(select (x ->> 'id')::uuid from jsonb_array_elements(r -> 'rows') x where x ->> 'entity' = 'snippet');
        cur := r ->> 'cursor';
        exit when cur is null or pages > 50;
    end loop;
    perform iamb_test.ok(biggest < 4800000, 'the largest page of snapshot has ' || biggest || ' bytes');
    perform iamb_test.ok((select array_agg(x order by x) from unnest(got) x) = (select array_agg(x order by x) from unnest(ids) x),
                         'every snippet arrives exactly once');
    -- A page of small rows still holds 500 of them.
    perform iamb_test.push('ines', jsonb_build_array(iamb_test.op_delete('folder', f, iamb_test.rev('folder', f))));
    update iamb.server_state set rate_limits = rate_limits || '{"push_calls": 60, "push_ops": 6000}' where id;
end $$;
