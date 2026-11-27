-- C25, G15, G22: one session, one device, one position; revoked devices and sessions Iamb does not
-- accept are refused by every function and read nothing through any policy.
do $$
declare
    r jsonb; d_old uuid; p0 text; p1 text; p2 text; n integer; lim jsonb; f uuid := iamb_test.id();
begin
    perform iamb_test.new_user('ines'); perform iamb_test.hello('ines', 'Ines');
    -- hello again with the same session and id refreshes the row.
    r := iamb_test.call('ines', 'api.hello(jsonb_build_object(''id'', iamb_test.dev(''ines''), ''name'', ''Mac'', ''platform'', ''macos'', ''app_version'', ''0.5''), array[1,1], 0)');
    perform iamb_test.ok(r ->> 'error' is null and (select name from iamb.device where id = iamb_test.dev('ines')) = 'Mac', 'refresh: ' || r::text);
    -- The same session under another id: device_conflict.
    d_old := iamb_test.dev('ines');
    update iamb_test.actor set device_id = gen_random_uuid() where name = 'ines';
    r := iamb_test.call('ines', 'api.hello(jsonb_build_object(''id'', iamb_test.dev(''ines''), ''name'', ''x'', ''platform'', ''linux'', ''app_version'', ''1''), array[1,1], 0)');
    perform iamb_test.eq(r ->> 'error', 'device_conflict', 'a session that already has a device');
    update iamb_test.actor set device_id = d_old where name = 'ines';
    -- A copied data directory: another session presents the same device id while the first lives.
    perform iamb_test.new_device('copy', 'ines');
    update iamb_test.actor set device_id = d_old where name = 'copy';
    r := iamb_test.hello('copy');
    perform iamb_test.eq(r ->> 'error', 'device_conflict', 'the id is in use by a live session');
    -- The user signs out on the first installation and in again: the row moves to the new session.
    delete from auth.sessions where id = (select session_id from iamb_test.actor where name = 'ines');
    r := iamb_test.hello('copy');
    perform iamb_test.ok(r ->> 'error' is null and (select session_id from iamb.device where id = d_old) = (select session_id from iamb_test.actor where name = 'copy'),
                         'moved to the new session: ' || r::text);
    delete from iamb_test.actor where name = 'ines';
    update iamb_test.actor set name = 'ines' where name = 'copy';

    -- G22: positions.
    p0 := iamb_test.pos('ines');
    r := iamb_test.call('ines', format('api.pull(iamb_test.dev(''ines''), 1, %L)', p0));
    p1 := r ->> 'next';
    perform iamb_test.ok(r ->> 'error' is null and p1 is not null, 'pull with the position issued last: ' || r::text);
    -- The answer was lost: the same position again, any number of times.
    for n in 1..3 loop
        r := iamb_test.call('ines', format('api.pull(iamb_test.dev(''ines''), 1, %L)', p0));
        perform iamb_test.ok(r ->> 'error' is null, 'the presented position again: ' || r::text);
        p2 := r ->> 'next';
    end loop;
    -- p1 is now neither the last issued (p2) nor the presented (p0): another installation.
    r := iamb_test.call('ines', format('api.pull(iamb_test.dev(''ines''), 1, %L)', p1));
    perform iamb_test.eq(r ->> 'error', 'device_conflict', 'a position the server no longer keeps');
    r := iamb_test.call('ines', format('api.push(iamb_test.dev(''ines''), 1, %L, %L::jsonb)', p1,
        jsonb_build_array(iamb_test.op_folder(f, iamb_test.personal('ines'), null, 'F'))));
    perform iamb_test.ok(r ->> 'error' = 'device_conflict' and not exists (select 1 from iamb.folder where id = f), 'and such a push writes nothing');
    -- An older epoch, or below the floor: reset_required before the token is looked at.
    r := iamb_test.call('ines', format('api.pull(iamb_test.dev(''ines''), 1, %L)', gen_random_uuid() || ':1:nonsense'));
    perform iamb_test.ok(r ->> 'error' = 'reset_required' and r ->> 'reason' = 'epoch', 'older epoch: ' || r::text);
    r := iamb_test.call('ines', format('api.push(iamb_test.dev(''ines''), 1, %L, %L::jsonb)', gen_random_uuid() || ':1:x',
        jsonb_build_array(iamb_test.op_folder(f, iamb_test.personal('ines'), null, 'F'))));
    perform iamb_test.ok(r ->> 'error' = 'reset_required' and not exists (select 1 from iamb.folder where id = f), 'push of an older epoch writes nothing');
    update iamb.feed_head set floor = 2 where user_id = iamb_test.uid('ines');
    r := iamb_test.call('ines', format('api.pull(iamb_test.dev(''ines''), 1, %L)', (select epoch from iamb.server_state) || ':1:x'));
    perform iamb_test.ok(r ->> 'error' = 'reset_required' and r ->> 'reason' = 'expired', 'below the floor: ' || r::text);
    update iamb.feed_head set floor = 0 where user_id = iamb_test.uid('ines');
    r := iamb_test.call('ines', format('api.pull(iamb_test.dev(''ines''), 1, %L)', (select epoch from iamb.server_state) || ':99999:x'));
    perform iamb_test.eq(r ->> 'error', 'server_behind', 'a position ahead of the feed');
    -- A device to which no position was issued: pull and push are reset; the first snapshot is accepted.
    perform iamb_test.new_device('ines_b', 'ines');
    perform iamb_test.call('ines_b', 'api.hello(jsonb_build_object(''id'', iamb_test.dev(''ines_b''), ''name'', ''B'', ''platform'', ''linux'', ''app_version'', ''1''), array[1,1], 0)');
    r := iamb_test.call('ines_b', format('api.pull(iamb_test.dev(''ines_b''), 1, %L)', p2));
    perform iamb_test.ok(r ->> 'error' = 'reset_required' and r ->> 'reason' = 'expired', 'no position issued yet: ' || r::text);
    r := iamb_test.call('ines_b', format('api.snapshot(iamb_test.dev(''ines_b''), 1, %L)', p2));
    perform iamb_test.ok(r ->> 'error' is null and r ->> 'start' is not null, 'its first snapshot is accepted whatever it presents');
    -- devices_max.
    select limits into lim from iamb.plan where id = 'default-user';
    update iamb.plan set limits = limits || '{"devices_max": 2}' where id = 'default-user';
    perform iamb_test.new_device('ines_c', 'ines');
    r := iamb_test.call('ines_c', 'api.hello(jsonb_build_object(''id'', iamb_test.dev(''ines_c''), ''name'', ''C'', ''platform'', ''linux'', ''app_version'', ''1''), array[1,1], 0)');
    perform iamb_test.eq(r ->> 'error', 'plan_limit', 'devices_max');
    update iamb.plan set limits = lim where id = 'default-user';
    r := iamb_test.call('ines_c', 'api.pull(iamb_test.dev(''ines_c''), 1, null)');
    perform iamb_test.eq(r ->> 'error', 'device_unknown', 'a session without a device row');
    perform iamb_test.eq(iamb_test.visible('ines_c', 'profile'), 0, 'a session without a device row reads nothing');
end $$;

-- Revocation (G15, S66, S74): from the commit of revoke_device, and for as long as its session
-- exists, also after the daily job.
do $$
declare
    r jsonb; c text; n integer;
begin
    delete from iamb_test.actor where true;
    perform iamb_test.new_user('ines'); perform iamb_test.new_device('lost', 'ines');
    perform iamb_test.hello('ines', 'Ines'); perform iamb_test.hello('lost');
    r := iamb_test.call('ines', 'api.revoke_device(iamb_test.dev(''ines''), 1, iamb_test.dev(''lost''))');
    perform iamb_test.ok(r ->> 'error' is null, 'revoke ' || r::text);
    foreach c in array array[
        'api.pull(iamb_test.dev(''lost''), 1, iamb_test.pos(''lost''))',
        'api.push(iamb_test.dev(''lost''), 1, iamb_test.pos(''lost''), ''[]'')',
        'api.snapshot(iamb_test.dev(''lost''), 1, null)',
        'api.rows(iamb_test.dev(''lost''), 1, ''[]'')',
        'api.visible_folders(iamb_test.dev(''lost''), 1)',
        'api.access(iamb_test.dev(''lost''), 1)',
        'api.devices(iamb_test.dev(''lost''), 1)',
        'api.check(iamb_test.dev(''lost''), 1)',
        'api.entitlements(iamb_test.dev(''lost''), 1)',
        'api.set_display_name(iamb_test.dev(''lost''), 1, ''x'')',
        'api.sign_out_everywhere(iamb_test.dev(''lost''), 1)',
        'api.revoke_device(iamb_test.dev(''lost''), 1, iamb_test.dev(''ines''))',
        'api.create_group(iamb_test.dev(''lost''), 1, gen_random_uuid(), ''family'', ''x'')',
        'api.hello(jsonb_build_object(''id'', iamb_test.dev(''lost''), ''name'', ''x'', ''platform'', ''linux'', ''app_version'', ''1''), array[1,1], 0)',
        'api.hello(jsonb_build_object(''id'', gen_random_uuid(), ''name'', ''x'', ''platform'', ''linux'', ''app_version'', ''1''), array[1,1], 0)'] loop
        r := iamb_test.call('lost', c);
        perform iamb_test.eq(r ->> 'error', 'device_revoked', 'revoked device: ' || c);
    end loop;
    perform iamb_test.eq(iamb_test.visible('lost', 'profile'), 0, 'a revoked device reads nothing');
    perform iamb_test.ok((select revoked_at from iamb.device where id = iamb_test.dev('ines')) is null, 'the other device is not revoked');
    -- (What the daily job does with the revoked row is in 16_daily_job.)
    -- sign_out_everywhere revokes every device row, the caller's included.
    perform iamb_test.new_device('ines_b', 'ines'); perform iamb_test.hello('ines_b');
    r := iamb_test.call('ines', 'api.sign_out_everywhere(iamb_test.dev(''ines''), 1)');
    perform iamb_test.ok(r ->> 'error' is null and not exists (select 1 from iamb.device where user_id = iamb_test.uid('ines') and revoked_at is null),
                         'sign_out_everywhere: ' || r::text);
    r := iamb_test.call('ines', 'api.pull(iamb_test.dev(''ines''), 1, iamb_test.pos(''ines''))');
    perform iamb_test.eq(r ->> 'error', 'device_revoked', 'the caller''s own device too');
end $$;

-- Only sessions opened by one of Iamb's ways of signing in (S83): a password, a token without
-- `amr`, or one with any method outside `sign_in_methods` is refused by every function and policy,
-- and cannot register a device.
do $$
declare
    r jsonb;
begin
    delete from iamb_test.actor where true;
    perform iamb_test.new_user('ines'); perform iamb_test.hello('ines', 'Ines');
    perform iamb_test.new_device('pw', 'ines', 'password');
    r := iamb_test.hello('pw');
    perform iamb_test.eq(r ->> 'error', 'session_refused', 'a session opened with a password registers no device');
    -- The holder of the password session uses the id of a registered device: refused all the same.
    update iamb_test.actor set device_id = iamb_test.dev('ines') where name = 'pw';
    r := iamb_test.call('pw', 'api.pull(iamb_test.dev(''pw''), 1, iamb_test.pos(''ines''))');
    perform iamb_test.eq(r ->> 'error', 'session_refused', 'a password session');
    update iamb_test.actor set amr = 'otp' where name = 'ines';
    perform iamb_test.act('ines');
    perform set_config('request.jwt.claims', (auth.jwt() - 'amr')::text, false);
    r := api.pull(iamb_test.dev('ines'), 1, iamb_test.pos('ines'));
    perform iamb_test.act_owner();
    perform iamb_test.eq(r ->> 'error', 'session_refused', 'a token without amr');
    perform iamb_test.act('ines');
    perform set_config('request.jwt.claims', (auth.jwt() || '{"amr": [{"method": "otp"}, {"method": "password"}]}')::text, false);
    r := api.pull(iamb_test.dev('ines'), 1, iamb_test.pos('ines'));
    perform iamb_test.ok(r ->> 'error' = 'session_refused' and not iamb_private.session_ok(), 'any method outside the list: ' || r::text);
    perform iamb_test.act_owner();
    perform iamb_test.ok(iamb_test.visible('pw', 'profile') = 0, 'a password session reads nothing');
    perform iamb_test.ok(iamb_test.visible('ines', 'profile') = 1, 'the link session reads its own');
end $$;
