-- §11.6: a push of 200 ops and a folder delete that takes 2,000 rows, against push's 20 s. Run by
-- run.sh's "perf" step locally, and inside a rolled-back transaction on the hosted project.
-- Answers {push_200_ms, cascade_2000_ms} in iamb_test.get('perf').
do $$
declare
    pi uuid; f uuid := gen_random_uuid(); sub uuid := gen_random_uuid(); r jsonb; ops jsonb; t0 timestamptz;
    push_ms numeric; cascade_ms numeric; i integer; n integer;
begin
    delete from iamb_test.actor where true;
    perform iamb_test.new_user('perf'); perform iamb_test.hello('perf', 'Perf');
    pi := iamb_test.personal('perf');
    update iamb.server_state set rate_limits = rate_limits || '{"push_calls": 1000, "push_ops": 100000}' where id;
    update iamb.plan set limits = limits || '{"snippets_max": 100000}' where id = 'default-user';
    r := iamb_test.push('perf', jsonb_build_array(iamb_test.op_folder(f, pi, null, 'Perf'), iamb_test.op_folder(sub, pi, f, 'Sub')));
    select jsonb_agg(iamb_test.op_snippet(gen_random_uuid(), f, ';p' || g, repeat('x', 200))) into ops from generate_series(1, 200) g;
    t0 := clock_timestamp();
    r := iamb_test.push('perf', ops);
    push_ms := extract(epoch from clock_timestamp() - t0) * 1000;
    perform iamb_test.ok(iamb_test.st(r, 200) = 'applied', 'push of 200: ' || left(r::text, 300));
    for i in 1..9 loop
        select jsonb_agg(iamb_test.op_snippet(gen_random_uuid(), sub, ';q' || i || '_' || g, 'x')) into ops from generate_series(1, 200) g;
        r := iamb_test.push('perf', ops);
    end loop;
    select count(*) into n from iamb.snippet where profile_id = pi and deleted_at is null;
    perform iamb_test.eq(n, 2000, 'rows below the folder');
    -- 1,999 rows below the folder plus the sub-folder: 2,000 by cascade, then the folder itself.
    delete from iamb.snippet where id = (select id from iamb.snippet where folder_id = sub limit 1);
    t0 := clock_timestamp();
    r := iamb_test.push('perf', jsonb_build_array(iamb_test.op_delete('folder', f, iamb_test.rev('folder', f))));
    cascade_ms := extract(epoch from clock_timestamp() - t0) * 1000;
    perform iamb_test.eq(iamb_test.st(r), 'applied', 'cascade of 2,000');
    perform iamb_test.put('perf', jsonb_build_object('push_200_ms', round(push_ms), 'cascade_2000_ms', round(cascade_ms)));
end $$;
select iamb_test.get('perf')::text as perf;
