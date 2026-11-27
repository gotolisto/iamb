-- Security review 4.3, findings 5, 10 (the account calls), 12, 15 and 20 (a session's time limit).

-- Finding 5: an offer of the ownership ends with the membership it was made to.
do $$
declare
    ids jsonb := iamb_test.family('edit'); g uuid := (ids ->> 'g')::uuid; i1 uuid := gen_random_uuid(); i2 uuid := gen_random_uuid();
    r jsonb; p jsonb;
begin
    perform iamb_test.new_user('adm'); perform iamb_test.hello('adm', 'Adm');
    perform iamb_test.call('ines', format('api.invite(iamb_test.dev(''ines''), 1, %L, %L, %L, ''admin'')', i1, g, iamb_test.email('adm')));
    perform iamb_test.call('adm', format('api.accept_invitation(iamb_test.dev(''adm''), 1, %L)', i1));
    -- The owner offers the group to Tom, then removes him.
    r := iamb_test.call('ines', format('api.transfer_ownership(iamb_test.dev(''ines''), 1, %L, %L)', g, iamb_test.uid('tom')));
    perform iamb_test.ok(r ->> 'error' is null, 'offer: ' || r::text);
    perform iamb_test.pull('ines'); perform iamb_test.pull('tom');
    perform iamb_test.call('ines', format('api.remove_member(iamb_test.dev(''ines''), 1, %L, %L)', g, iamb_test.uid('tom')));
    perform iamb_test.ok((select offered_owner is null and offered_at is null from iamb.profile where id = g), 'the offer ended with the membership');
    p := iamb_test.pull('ines');
    perform iamb_test.ok(exists (select 1 from jsonb_array_elements(p -> 'entries') e where e ->> 'kind' = 'invitations'), 'the owner''s feed says so');
    r := iamb_test.call('tom', format('api.accept_ownership(iamb_test.dev(''tom''), 1, %L)', g));
    perform iamb_test.eq(r ->> 'error', 'no_offer', 'removed, he cannot accept');
    -- An admin invites him back as a member.
    perform iamb_test.call('adm', format('api.invite(iamb_test.dev(''adm''), 1, %L, %L, %L, ''member'')', i2, g, iamb_test.email('tom')));
    r := iamb_test.call('tom', format('api.accept_invitation(iamb_test.dev(''tom''), 1, %L)', i2));
    perform iamb_test.ok(r ->> 'error' is null, 'back as a member: ' || r::text);
    r := iamb_test.call('tom', format('api.accept_ownership(iamb_test.dev(''tom''), 1, %L)', g));
    perform iamb_test.eq(r ->> 'error', 'no_offer', 'back as a member, the old offer is gone');
    perform iamb_test.eq((select owner_user_id from iamb.profile where id = g), iamb_test.uid('ines'), 'the owner is the owner');
    -- Even an offer row left over from before (as a database from before this fix could hold): it
    -- was made to another membership than the one he has now.
    update iamb.profile set offered_owner = iamb_test.uid('tom'), offered_at = (select created_at - 1 from iamb.membership
        where profile_id = g and user_id = iamb_test.uid('tom') and deleted_at is null) where id = g;
    r := iamb_test.call('tom', format('api.accept_ownership(iamb_test.dev(''tom''), 1, %L)', g));
    perform iamb_test.eq(r ->> 'error', 'no_offer', 'an offer made before this membership began');
    update iamb.profile set offered_owner = null, offered_at = null where id = g;
    -- A fresh offer to the membership he has now still works (S34).
    perform iamb_test.call('ines', format('api.transfer_ownership(iamb_test.dev(''ines''), 1, %L, %L)', g, iamb_test.uid('tom')));
    r := iamb_test.call('tom', format('api.accept_ownership(iamb_test.dev(''tom''), 1, %L)', g));
    perform iamb_test.ok(r ->> 'error' is null and (select owner_user_id from iamb.profile where id = g) = iamb_test.uid('tom'), 'a new offer is accepted: ' || r::text);
    perform iamb_test.check_all();
end $$;

-- Finding 15: `accept_ownership` recomputes the access entries (G1), with a folder in the group.
do $$
declare
    ids jsonb := iamb_test.family('edit'); g uuid := (ids ->> 'g')::uuid; gf uuid := gen_random_uuid(); r jsonb;
begin
    perform iamb_test.push('ines', jsonb_build_array(iamb_test.op_folder(gf, g, null, 'Group folder')));
    perform iamb_test.pull('tom');
    perform iamb_test.eq((select permission from iamb.folder_access where folder_id = gf and user_id = iamb_test.uid('tom')), 'view', 'a member reads the group''s folder');
    perform iamb_test.call('ines', format('api.transfer_ownership(iamb_test.dev(''ines''), 1, %L, %L)', g, iamb_test.uid('tom')));
    r := iamb_test.call('tom', format('api.accept_ownership(iamb_test.dev(''tom''), 1, %L)', g));
    perform iamb_test.ok(r ->> 'error' is null, 'accept: ' || r::text);
    perform iamb_test.check_access();
    perform iamb_test.eq((select permission from iamb.folder_access where folder_id = gf and user_id = iamb_test.uid('tom')), 'manage', 'the new owner manages it');
    r := iamb_test.push('tom', jsonb_build_array(iamb_test.op_folder(gf, g, null, 'Renamed by the new owner', iamb_test.rev('folder', gf))));
    perform iamb_test.eq(iamb_test.st(r), 'applied', 'and may rename a top-level folder');
    perform iamb_test.check_all();
end $$;

-- Finding 10: what the answers of the account calls tell somebody who is not in the group.
do $$
declare
    ids jsonb := iamb_test.family('edit'); g uuid := (ids ->> 'g')::uuid; inv uuid; r jsonb; r2 jsonb; own_inv uuid := gen_random_uuid();
begin
    select id into inv from iamb.invitation where profile_id = g and accepted_by = iamb_test.uid('tom');
    -- While he is a member, a repeated `accept_invitation` answers as the first did.
    r := iamb_test.call('tom', format('api.accept_invitation(iamb_test.dev(''tom''), 1, %L)', inv));
    perform iamb_test.ok(r ->> 'error' is null and r ? 'mark', 'a repeated call by a member: ' || r::text);
    perform iamb_test.call('ines', format('api.invite(iamb_test.dev(''ines''), 1, %L, %L, ''someone@example.test'', ''member'')', own_inv, g));
    perform iamb_test.call('ines', format('api.remove_member(iamb_test.dev(''ines''), 1, %L, %L)', g, iamb_test.uid('tom')));
    -- Removed: the invitation he once accepted gives him no word of the group's revisions.
    r := iamb_test.call('tom', format('api.accept_invitation(iamb_test.dev(''tom''), 1, %L)', inv));
    perform iamb_test.ok(r ->> 'error' = 'invitation_invalid' and not r ? 'mark', 'no revision of the group after the removal: ' || r::text);
    -- `invite` looks at the caller's role first: an invitation id in use and a fresh one answer the same.
    r := iamb_test.call('tom', format('api.invite(iamb_test.dev(''tom''), 1, %L, %L, ''x@example.test'', ''member'')', own_inv, g));
    r2 := iamb_test.call('tom', format('api.invite(iamb_test.dev(''tom''), 1, %L, %L, ''x@example.test'', ''member'')', gen_random_uuid(), g));
    perform iamb_test.ok(r ->> 'error' = 'not_found' and r2 ->> 'error' = 'not_found', 'invite by a non-member: ' || r::text || r2::text);
    -- The owner's repeated calls still answer as the first did.
    r := iamb_test.call('ines', format('api.invite(iamb_test.dev(''ines''), 1, %L, %L, ''someone@example.test'', ''member'')', own_inv, g));
    perform iamb_test.ok(r ->> 'error' is null and r ? 'mark', 'a repeated invite by the owner: ' || r::text);
    r := iamb_test.call('ines', format('api.create_group(iamb_test.dev(''ines''), 1, %L, ''family'', ''Family'')', g));
    perform iamb_test.ok(r ->> 'error' is null and r ? 'mark', 'a repeated create_group by the owner: ' || r::text);
    -- A group's creator who has left it (the group was handed over) gets no mark from `create_group`.
    perform iamb_test.new_user('ana'); perform iamb_test.hello('ana', 'Ana');
    perform iamb_test.call('ines', format('api.invite(iamb_test.dev(''ines''), 1, %L, %L, %L, ''member'')', inv, g, iamb_test.email('ana')));
    select id into inv from iamb.invitation where profile_id = g and email = iamb_test.email('ana');
    if inv is null then
        inv := gen_random_uuid();
        perform iamb_test.call('ines', format('api.invite(iamb_test.dev(''ines''), 1, %L, %L, %L, ''member'')', inv, g, iamb_test.email('ana')));
    end if;
    perform iamb_test.call('ana', format('api.accept_invitation(iamb_test.dev(''ana''), 1, %L)', inv));
    perform iamb_test.call('ines', format('api.transfer_ownership(iamb_test.dev(''ines''), 1, %L, %L)', g, iamb_test.uid('ana')));
    perform iamb_test.call('ana', format('api.accept_ownership(iamb_test.dev(''ana''), 1, %L)', g));
    perform iamb_test.call('ines', format('api.leave_group(iamb_test.dev(''ines''), 1, %L)', g));
    r := iamb_test.call('ines', format('api.create_group(iamb_test.dev(''ines''), 1, %L, ''family'', ''Family'')', g));
    perform iamb_test.ok(r ->> 'error' = 'invalid' and not r ? 'mark', 'create_group for a group its creator has left: ' || r::text);
    -- The calls that name a group answer a non-member without telling whether the group exists.
    r := iamb_test.call('stranger', format('api.rename_group(iamb_test.dev(''stranger''), 1, %L, ''x'')', g));
    r2 := iamb_test.call('stranger', format('api.rename_group(iamb_test.dev(''stranger''), 1, %L, ''x'')', gen_random_uuid()));
    perform iamb_test.ok(r - 'epoch' = r2 - 'epoch' and r ->> 'error' = 'not_found', 'rename_group: ' || r::text || r2::text);
    r := iamb_test.call('stranger', format('api.set_member_role(iamb_test.dev(''stranger''), 1, %L, %L, ''nonsense'')', g, iamb_test.uid('ana')));
    perform iamb_test.eq(r ->> 'error', 'not_found', 'set_member_role with a role that does not exist, by a non-member');
    perform iamb_test.check_all();
end $$;

-- Finding 12: the device to be revoked cannot use up the counter that revocation is counted in.
do $$
declare
    r jsonb; i integer;
begin
    delete from iamb_test.actor where true;
    perform iamb_test.new_user('owner'); perform iamb_test.new_device('thief', 'owner');
    perform iamb_test.hello('owner', 'Owner'); perform iamb_test.hello('thief');
    delete from iamb.rate_bucket where true;                                           -- a new minute
    for i in 1..40 loop                                                                -- the device to be revoked: cheap calls
        r := iamb_test.call('thief', format('api.withdraw_invitation(iamb_test.dev(''thief''), 1, %L)', gen_random_uuid()));
    end loop;
    perform iamb_test.eq(r ->> 'error', 'rate_limited', 'the thief has used up the account''s counter');
    -- ... and its own revocation counter as well.
    for i in 1..12 loop
        r := iamb_test.call('thief', 'api.revoke_device(iamb_test.dev(''thief''), 1, gen_random_uuid())');
    end loop;
    perform iamb_test.eq(r ->> 'error', 'rate_limited', 'the thief has used up its own revocation counter');
    r := iamb_test.call('owner', 'api.revoke_device(iamb_test.dev(''owner''), 1, iamb_test.dev(''thief''))');
    perform iamb_test.ok(r ->> 'error' is null, 'the owner revokes all the same: ' || r::text);
    r := iamb_test.call('thief', 'api.visible_folders(iamb_test.dev(''thief''), 1)');
    perform iamb_test.eq(r ->> 'error', 'device_revoked', 'the thief is out');
    r := iamb_test.call('owner', 'api.sign_out_everywhere(iamb_test.dev(''owner''), 1)');
    perform iamb_test.ok(r ->> 'error' is null, 'sign_out_everywhere too: ' || r::text);
end $$;

-- Finding 20: a session past a time limit set at the authentication service is refused by every
-- function and reads no row.
do $$
declare
    r jsonb;
begin
    delete from iamb_test.actor where true;
    perform iamb_test.new_user('ines'); perform iamb_test.hello('ines', 'Ines');
    update auth.sessions set not_after = now() + interval '1 hour' where id = (select session_id from iamb_test.actor where name = 'ines');
    r := iamb_test.call('ines', 'api.check(iamb_test.dev(''ines''), 1)');
    perform iamb_test.ok(r ->> 'error' is null, 'a session inside its time limit: ' || r::text);
    perform iamb_test.eq(iamb_test.visible('ines', 'profile'), 1, 'reads its profile');
    update auth.sessions set not_after = now() - interval '1 second' where id = (select session_id from iamb_test.actor where name = 'ines');
    r := iamb_test.call('ines', 'api.check(iamb_test.dev(''ines''), 1)');
    perform iamb_test.eq(r ->> 'error', 'session_refused', 'a session past its time limit');
    perform iamb_test.eq(iamb_test.visible('ines', 'profile'), 0, 'reads nothing');
end $$;
