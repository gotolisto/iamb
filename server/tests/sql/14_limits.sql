-- G18: size, count and rate limits whatever the client sends; nothing is written past a limit.
do $$
declare
    pi uuid; f uuid := iamb_test.id(); r jsonb; ops jsonb; n integer; st jsonb;
begin
    perform iamb_test.new_user('ines'); perform iamb_test.hello('ines', 'Ines');
    pi := iamb_test.personal('ines');
    r := iamb_test.push('ines', jsonb_build_array(iamb_test.op_folder(f, pi, null, 'F')));
    select jsonb_agg(iamb_test.op_snippet(iamb_test.id(), f, ';' || i, 'x')) into ops from generate_series(1, 201) i;
    r := iamb_test.push('ines', ops);
    perform iamb_test.eq(r ->> 'error', 'too_large', '201 ops');
    perform iamb_test.eq((select count(*)::integer from iamb.snippet where folder_id = f), 0, 'nothing written');
    r := iamb_test.push('ines', jsonb_build_array(iamb_test.op_snippet(iamb_test.id(), f, ';big', repeat('y', 4200000))));
    perform iamb_test.eq(r ->> 'error', 'too_large', 'a request over 4 MiB');
    r := iamb_test.call('ines', format('api.rows(iamb_test.dev(''ines''), 1, %L::jsonb)',
        (select jsonb_agg(jsonb_build_object('entity', 'snippet', 'id', gen_random_uuid())) from generate_series(1, 501))));
    perform iamb_test.eq(r ->> 'error', 'too_large', 'rows over 500');
    -- Rate limits, per device and minute; over the limit nothing else is written.
    select rate_limits into st from iamb.server_state;
    update iamb.server_state set rate_limits = rate_limits || '{"push_calls": 2, "push_ops": 3}' where id;
    delete from iamb.rate_bucket where true;
    r := iamb_test.push('ines', jsonb_build_array(iamb_test.op_snippet(iamb_test.id(), f, ';1', '1'), iamb_test.op_snippet(iamb_test.id(), f, ';2', '2')));
    perform iamb_test.eq(iamb_test.st(r, 2), 'applied', 'within the limits');
    r := iamb_test.push('ines', jsonb_build_array(iamb_test.op_snippet(iamb_test.id(), f, ';3', '3'), iamb_test.op_snippet(iamb_test.id(), f, ';4', '4')));
    perform iamb_test.ok(r ->> 'error' = 'rate_limited' and (r ->> 'retry_after_ms')::bigint > 0, 'ops per minute: ' || r::text);
    r := iamb_test.push('ines', jsonb_build_array());
    perform iamb_test.ok(r ->> 'error' = 'rate_limited', 'calls per minute: ' || r::text);
    perform iamb_test.eq((select count(*)::integer from iamb.snippet where folder_id = f), 2, 'nothing written past the limit');
    update iamb.server_state set rate_limits = st where id;
end $$;
