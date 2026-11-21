-- §5.12: the daily job. Times are moved back on the rows instead of moving the clock.
do $$
declare
    ids jsonb := iamb_test.family('edit');
    pi uuid; f1 uuid; f2 uuid; s1 uuid; s2 uuid; g2 uuid := iamb_test.id(); gf uuid := iamb_test.id();
    a uuid := iamb_test.id(); inv uuid := iamb_test.id(); inv2 uuid := iamb_test.id(); old_pos text;
    r jsonb; day bigint := 86400000; c_open uuid; c_old uuid; base bigint;
begin
    pi := (ids ->> 'p_ines')::uuid; f1 := (ids ->> 'f1')::uuid; f2 := (ids ->> 'f2')::uuid;
    s1 := (ids ->> 's1')::uuid; s2 := (ids ->> 's2')::uuid;
    perform iamb_test.pull('tom');
    old_pos := iamb_test.pos('tom');
    -- A tombstone 31 days old, with its history and a conflict.
    base := iamb_test.rev('snippet', s2);
    perform iamb_test.new_device('ines_b', 'ines'); perform iamb_test.hello('ines_b');
    r := iamb_test.push('ines_b', jsonb_build_array(iamb_test.op_snippet(s2, f2, ';priv', 'changed', base)));
    r := iamb_test.push('ines', jsonb_build_array(iamb_test.op_delete('snippet', s2, base)));
    perform iamb_test.ok(r -> 'results' -> 0 ->> 'conflict' is not null, 'a conflict on s2');
    update iamb.snippet set deleted_at = deleted_at - 31 * day where id = s2;
    -- History 91 days old: one version offered by an open conflict (stays), one not (goes).
    base := iamb_test.rev('snippet', s1);
    r := iamb_test.push('tom', jsonb_build_array(iamb_test.op_snippet(s1, f1, ';hrs', 'tom', base)));
    r := iamb_test.push('ines', jsonb_build_array(iamb_test.op_snippet(s1, f1, ';hrs', 'ines', base)));
    c_open := (r -> 'results' -> 0 ->> 'conflict')::uuid;
    update iamb.history set replaced_at = replaced_at - 91 * day where row_id = s1;
    -- A conflict closed 31 days ago; an open one 91 days old.
    insert into iamb.conflict (id, entity, row_id, profile_id, kind, winner_rev, created_at, state, resolved_at, parties)
    values (gen_random_uuid(), 'snippet', s1, pi, 'edit_edit', 1, 0, 'resolved', iamb_private.now_ms() - 31 * day, '{}') returning id into c_old;
    -- Receipts and feed entries 31 days old.
    update iamb.push_receipt set created_at = created_at - 31 * day where true;
    update iamb.feed set created_at = created_at - 31 * day where user_id = iamb_test.uid('tom');
    -- Invitations: pending past its time; answered 91 days ago. An offer of ownership 15 days old.
    perform iamb_test.call('ines', format('api.invite(iamb_test.dev(''ines''), 1, %L, %L, %L, ''member'')', inv, ids ->> 'g', iamb_test.email('stranger')));
    update iamb.invitation set expires_at = iamb_private.now_ms() - 1 where id = inv;
    update iamb.invitation set answered_at = iamb_private.now_ms() - 91 * day where status = 'accepted';
    perform iamb_test.call('ines', format('api.transfer_ownership(iamb_test.dev(''ines''), 1, %L, %L)', ids ->> 'g', iamb_test.uid('tom')));
    update iamb.profile set offered_at = offered_at - 15 * day where id = (ids ->> 'g')::uuid;
    -- A group deleted 31 days ago, with a folder.
    perform iamb_test.call('ines', format('api.create_group(iamb_test.dev(''ines''), 1, %L, ''family'', ''Old'')', g2));
    r := iamb_test.push('ines', jsonb_build_array(iamb_test.op_folder(gf, g2, null, 'Old folder'), iamb_test.op_snippet(iamb_test.id(), gf, ';o', 'o'),
                                                   iamb_test.op_tag(iamb_test.id(), g2, 'old tag')));
    perform iamb_test.ok(iamb_test.st(r, 3) = 'applied', 'a tag in the group ' || r::text);
    perform iamb_test.call('ines', format('api.delete_group(iamb_test.dev(''ines''), 1, %L)', g2));
    update iamb.profile set deleted_at = deleted_at - 31 * day where id = g2;
    update iamb.folder set deleted_at = deleted_at - 31 * day where profile_id = g2;
    update iamb.snippet set deleted_at = deleted_at - 31 * day where profile_id = g2;
    -- A device idle for 181 days; an attachment nothing has used for 91 days.
    update iamb.device set last_seen_at = iamb_private.now_ms() - 181 * day where id = iamb_test.dev('ines_b');
    r := iamb_test.push('ines', jsonb_build_array(iamb_test.op_attachment(a, pi, repeat('3c', 32))));
    update iamb.attachment set unlinked_at = unlinked_at - 91 * day where id = a;
    perform iamb_test.put('ids', ids || jsonb_build_object('g2', g2, 'a', a, 'c_open', c_open, 'c_old', c_old, 'inv', inv, 'old_pos', old_pos,
                                                           'dev_b', iamb_test.dev('ines_b')));
end $$;

select iamb_test.daily();

do $$
declare
    ids jsonb := iamb_test.get('ids');
    r jsonb;
begin
    perform iamb_test.ok(not exists (select 1 from iamb.snippet where id = (ids ->> 's2')::uuid), 'the old tombstone is purged');
    perform iamb_test.ok(not exists (select 1 from iamb.history where row_id = (ids ->> 's2')::uuid)
                         and not exists (select 1 from iamb.conflict where row_id = (ids ->> 's2')::uuid), 'with its history and conflicts');
    r := iamb_test.push('ines', jsonb_build_array(iamb_test.op_snippet((ids ->> 's2')::uuid, (ids ->> 'f2')::uuid, ';priv', 'back', 9)));
    perform iamb_test.eq(iamb_test.st(r), 'rejected:gone', 'a purged row does not come back (C13)');
    perform iamb_test.ok(exists (select 1 from iamb.history h join iamb.conflict_version v on v.rev = h.rev
                                 where v.conflict_id = (ids ->> 'c_open')::uuid and h.row_id = (ids ->> 's1')::uuid),
                         'a version an open conflict offers stays');
    perform iamb_test.ok(not exists (select 1 from iamb.conflict where id = (ids ->> 'c_old')::uuid), 'a conflict closed 30 days ago goes');
    perform iamb_test.eq((select count(*)::integer from iamb.push_receipt where created_at < iamb_private.now_ms() - 30 * 86400000::bigint), 0,
                         'receipts older than 30 days go');
    perform iamb_test.ok(exists (select 1 from iamb.push_receipt), 'younger receipts stay');
    r := iamb_test.call('tom', format('api.pull(iamb_test.dev(''tom''), 1, %L)', ids ->> 'old_pos'));
    perform iamb_test.ok(r ->> 'error' = 'reset_required' and r ->> 'reason' = 'expired', 'a position below the raised floor: ' || r::text);
    perform iamb_test.eq((select status from iamb.invitation where id = (ids ->> 'inv')::uuid), 'expired', 'a pending invitation expires');
    perform iamb_test.eq((select count(*)::integer from iamb.invitation where status = 'accepted'), 0, 'an answered one goes after 90 days');
    perform iamb_test.ok((select offered_owner from iamb.profile where id = (ids ->> 'g')::uuid) is null, 'an offer lapses after 14 days');
    perform iamb_test.ok(not exists (select 1 from iamb.profile where id = (ids ->> 'g2')::uuid)
                         and not exists (select 1 from iamb.folder where profile_id = (ids ->> 'g2')::uuid)
                         and not exists (select 1 from iamb.tag where profile_id = (ids ->> 'g2')::uuid), 'a deleted group is purged, tags too');
    perform iamb_test.ok(not exists (select 1 from iamb.device where id = (ids ->> 'dev_b')::uuid), 'an idle device row is removed');
    perform iamb_test.ok((select deleted_at from iamb.attachment where id = (ids ->> 'a')::uuid) is not null, 'an unused attachment becomes a tombstone');
    perform iamb_test.check_all();
end $$;

-- The job can be run any number of times; with a small budget it works in several steps.
update iamb.server_state set cascade_max = 2 where id;
select iamb_test.daily();
select iamb_test.daily();
update iamb.server_state set cascade_max = 2000 where id;
do $$ begin perform iamb_test.check_all(); end $$;

-- G15 and §5.12: a revoked device row stays for as long as its session exists, also after the
-- daily job, and keeps refusing that session; it goes once the session is gone (S74).
do $$
declare r jsonb;
begin
    delete from iamb_test.actor where true;
    perform iamb_test.new_user('ines'); perform iamb_test.new_device('lost', 'ines');
    perform iamb_test.hello('ines', 'Ines'); perform iamb_test.hello('lost');
    r := iamb_test.call('ines', 'api.revoke_device(iamb_test.dev(''ines''), 1, iamb_test.dev(''lost''))');
    perform iamb_test.ok(r ->> 'error' is null, 'revoke ' || r::text);
    update iamb.device set last_seen_at = 0 where id = iamb_test.dev('lost');
end $$;
select iamb_test.daily();
do $$
declare r jsonb;
begin
    perform iamb_test.ok(exists (select 1 from iamb.device where id = iamb_test.dev('lost')), 'the revoked row stays while its session exists');
    r := iamb_test.call('lost', 'api.hello(jsonb_build_object(''id'', gen_random_uuid(), ''name'', ''x'', ''platform'', ''linux'', ''app_version'', ''1''), array[1,1], 0)');
    perform iamb_test.eq(r ->> 'error', 'device_revoked', 'still refused after the daily job');
    delete from auth.sessions where id = (select session_id from iamb_test.actor where name = 'lost');
end $$;
select iamb_test.daily();
do $$
begin
    perform iamb_test.ok(not exists (select 1 from iamb.device where id = iamb_test.dev('lost')), 'removed once its session is gone');
end $$;
