-- Security review 4.3, finding 9: one account cannot keep the retention job from everybody else's
-- deleted rows. Mallory's group has the lowest id there is and more due tombstones than a run's
-- whole budget; Ines's deleted snippet is purged all the same, in the first pass.
do $$
declare
    ids jsonb := iamb_test.family('edit'); mf uuid := gen_random_uuid(); s2 uuid := (ids ->> 's2')::uuid; batch uuid[]; r jsonb;
    low uuid := '00000000-0000-4000-8000-000000000000'; i integer; left_m integer; inv uuid := gen_random_uuid(); cid uuid;
begin
    update iamb.server_state set cascade_max = 40, rate_limits = rate_limits || '{"push_calls": 1000, "push_ops": 100000}' where id;
    perform iamb_test.new_user('mallory'); perform iamb_test.hello('mallory', 'Mallory');
    perform iamb_test.call('mallory', format('api.create_group(iamb_test.dev(''mallory''), 1, %L, ''family'', ''first'')', low));
    perform iamb_test.push('mallory', jsonb_build_array(iamb_test.op_folder(mf, low, null, 'churn')));
    for i in 1..2 loop                      -- 200 snippets made and deleted: five runs' whole budgets
        batch := array(select gen_random_uuid() from generate_series(1, 100));
        perform iamb_test.push('mallory', (select jsonb_agg(iamb_test.op_snippet(x, mf, ';c', 'c')) from unnest(batch) x));
        perform iamb_test.push('mallory', (select jsonb_agg(iamb_test.op_delete('snippet', x, 0)) from unnest(batch) x));
    end loop;
    -- Ines deletes her private snippet; an invitation of her group is past its time. 31 days pass.
    perform iamb_test.push('ines', jsonb_build_array(iamb_test.op_delete('snippet', s2, iamb_test.rev('snippet', s2))));
    perform iamb_test.call('ines', format('api.invite(iamb_test.dev(''ines''), 1, %L, %L, ''late@example.test'', ''member'')', inv, ids ->> 'g'));
    update iamb.invitation set expires_at = expires_at - 31 * 86400000::bigint where id = inv;
    update iamb.snippet set deleted_at = deleted_at - 31 * 86400000::bigint where deleted_at is not null;
    r := iamb_private.daily_job_step();
    select count(*) into left_m from iamb.snippet where profile_id = low;
    perform iamb_test.ok((r ->> 'more')::boolean and (r ->> 'capped')::integer >= 1, 'run 1 says that a profile used up its share: ' || r::text);
    perform iamb_test.ok(left_m between 150 and 199, 'Mallory''s group got a share of the run, not all of it: ' || left_m || ' of 200 rows left');
    perform iamb_test.ok(not exists (select 1 from iamb.snippet where id = s2), 'Ines''s deleted snippet is purged in the same run');
    perform iamb_test.eq((select status from iamb.invitation where id = inv), 'expired', 'and her group''s invitation has expired');
    -- The runs go on until nothing is left, in a bounded number of them.
    r := iamb_test.daily();
    perform iamb_test.ok((r ->> 'steps')::integer between 2 and 60 and not (r ->> 'more')::boolean, 'the job ends: ' || r::text);
    perform iamb_test.eq((select count(*)::integer from iamb.snippet where profile_id = low), 0, 'Mallory''s rows are purged too');
    perform iamb_test.ok((select job_after is null from iamb.server_state), 'the next pass begins at the start');
    update iamb.server_state set cascade_max = 2000, rate_limits = rate_limits || '{"push_calls": 60, "push_ops": 6000}' where id;
    perform iamb_test.check_all();
end $$;
