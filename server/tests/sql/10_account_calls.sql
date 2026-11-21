-- G8: every account call of §4.1 succeeds for the roles that table allows and fails for the
-- others with the code of §5.14; a group always has exactly one owner, and nobody becomes one
-- without accepting. G11: invitations. Repeated calls answer as the first (§5.14).
do $$
declare
    g uuid := iamb_test.id(); g2 uuid := iamb_test.id();
    i_tom uuid := iamb_test.id(); i_ana uuid := iamb_test.id(); i_bo uuid := iamb_test.id(); i_x uuid := iamb_test.id();
    i_un uuid := iamb_test.id(); i_w uuid := iamb_test.id();
    r jsonb; m1 bigint; lim jsonb;
begin
    perform iamb_test.new_user('ines'); perform iamb_test.new_user('tom'); perform iamb_test.new_user('ana');
    perform iamb_test.new_user('bo'); perform iamb_test.new_user('unconfirmed', false); perform iamb_test.new_user('noname');
    perform iamb_test.hello('ines', 'Ines'); perform iamb_test.hello('tom', 'Tom'); perform iamb_test.hello('ana', 'Ana');
    perform iamb_test.hello('bo', 'Bo'); perform iamb_test.hello('unconfirmed', 'Un'); perform iamb_test.hello('noname');

    -- A display name is needed before a person becomes visible to others.
    r := iamb_test.call('noname', format('api.create_group(iamb_test.dev(''noname''), 1, %L, ''family'', ''X'')', iamb_test.id()));
    perform iamb_test.eq(r ->> 'error', 'name_required', 'create_group without a name');
    r := iamb_test.call('ines', format('api.create_group(iamb_test.dev(''ines''), 1, %L, ''club'', ''X'')', iamb_test.id()));
    perform iamb_test.eq(r ->> 'error', 'invalid', 'an unknown kind');
    r := iamb_test.call('ines', format('api.create_group(iamb_test.dev(''ines''), 1, %L, ''family'', ''Fam'')', g));
    m1 := (r ->> 'mark')::bigint;
    perform iamb_test.ok(r ->> 'error' is null and m1 is not null, 'create_group: ' || r::text);
    r := iamb_test.call('ines', format('api.create_group(iamb_test.dev(''ines''), 1, %L, ''family'', ''Fam'')', g));
    perform iamb_test.ok(r ->> 'error' is null and (r ->> 'mark')::bigint >= m1, 'a repeated create_group answers as the first: ' || r::text);
    r := iamb_test.call('tom', format('api.create_group(iamb_test.dev(''tom''), 1, %L, ''family'', ''Fam'')', g));
    perform iamb_test.eq(r ->> 'error', 'invalid', 'an id taken by someone else''s group');

    -- Invitations: owner invites; repeated invite answers as the first; duplicates refused.
    r := iamb_test.call('ines', format('api.invite(iamb_test.dev(''ines''), 1, %L, %L, %L, ''member'')', i_tom, g, iamb_test.email('tom')));
    perform iamb_test.ok(r ->> 'error' is null, 'invite tom ' || r::text);
    r := iamb_test.call('ines', format('api.invite(iamb_test.dev(''ines''), 1, %L, %L, %L, ''member'')', i_tom, g, iamb_test.email('tom')));
    perform iamb_test.ok(r ->> 'error' is null, 'repeated invite');
    r := iamb_test.call('ines', format('api.invite(iamb_test.dev(''ines''), 1, %L, %L, %L, ''member'')', iamb_test.id(), g, upper(iamb_test.email('tom'))));
    perform iamb_test.eq(r ->> 'error', 'already_invited', 'a second invitation to the same address');
    r := iamb_test.call('ines', format('api.invite(iamb_test.dev(''ines''), 1, %L, %L, %L, ''owner'')', iamb_test.id(), g, iamb_test.email('bo')));
    perform iamb_test.eq(r ->> 'error', 'invalid', 'nobody is invited as owner');
    r := iamb_test.call('ines', format('api.invite(iamb_test.dev(''ines''), 1, %L, %L, %L, ''admin'')', i_ana, g, iamb_test.email('ana')));
    r := iamb_test.call('ines', format('api.invite(iamb_test.dev(''ines''), 1, %L, %L, %L, ''member'')', i_un, g, iamb_test.email('unconfirmed')));
    -- G11: the invited account sees its invitation; nobody else but owner and admins.
    r := iamb_test.call('tom', 'api.invitations(iamb_test.dev(''tom''), 1)');
    perform iamb_test.ok(jsonb_array_length(r -> 'invitations') = 1 and r -> 'invitations' -> 0 ->> 'id' = i_tom::text, 'tom sees his: ' || r::text);
    r := iamb_test.call('bo', 'api.invitations(iamb_test.dev(''bo''), 1)');
    perform iamb_test.eq(jsonb_array_length(r -> 'invitations'), 0, 'bo sees none');
    r := iamb_test.call('unconfirmed', 'api.invitations(iamb_test.dev(''unconfirmed''), 1)');
    perform iamb_test.eq(jsonb_array_length(r -> 'invitations'), 0, 'an unconfirmed address sees none');
    r := iamb_test.call('unconfirmed', format('api.accept_invitation(iamb_test.dev(''unconfirmed''), 1, %L)', i_un));
    perform iamb_test.eq(r ->> 'error', 'invitation_invalid', 'an unconfirmed address accepts');
    r := iamb_test.call('bo', format('api.accept_invitation(iamb_test.dev(''bo''), 1, %L)', i_tom));
    perform iamb_test.eq(r ->> 'error', 'invitation_invalid', 'another account accepts');
    r := iamb_test.call('ines', 'api.invitations(iamb_test.dev(''ines''), 1)');
    perform iamb_test.eq(jsonb_array_length(r -> 'invitations'), 3, 'the owner sees the group''s invitations with addresses');
    r := iamb_test.call('tom', format('api.accept_invitation(iamb_test.dev(''tom''), 1, %L)', i_tom));
    perform iamb_test.ok(r ->> 'error' is null, 'tom accepts');
    r := iamb_test.call('tom', format('api.accept_invitation(iamb_test.dev(''tom''), 1, %L)', i_tom));
    perform iamb_test.ok(r ->> 'error' is null, 'a repeated accept');
    r := iamb_test.call('ana', format('api.accept_invitation(iamb_test.dev(''ana''), 1, %L)', i_ana));
    r := iamb_test.call('tom', 'api.invitations(iamb_test.dev(''tom''), 1)');
    perform iamb_test.eq(jsonb_array_length(r -> 'invitations'), 0, 'a member does not see the group''s invitations');
    r := iamb_test.call('ines', format('api.invite(iamb_test.dev(''ines''), 1, %L, %L, %L, ''member'')', iamb_test.id(), g, iamb_test.email('tom')));
    perform iamb_test.eq(r ->> 'error', 'already_member', 'inviting a member');
    -- An admin invites, and withdraws an invitation the owner sent.
    r := iamb_test.call('ana', format('api.invite(iamb_test.dev(''ana''), 1, %L, %L, %L, ''member'')', i_bo, g, iamb_test.email('bo')));
    perform iamb_test.ok(r ->> 'error' is null, 'an admin invites');
    r := iamb_test.call('ana', format('api.withdraw_invitation(iamb_test.dev(''ana''), 1, %L)', i_un));
    perform iamb_test.ok(r ->> 'error' is null, 'an admin withdraws the owner''s invitation');
    r := iamb_test.call('tom', format('api.withdraw_invitation(iamb_test.dev(''tom''), 1, %L)', i_bo));
    perform iamb_test.eq(r ->> 'error', 'forbidden', 'a member withdraws');
    -- S32, S33: accepted then withdrawn; withdrawn then accepted.
    r := iamb_test.call('ines', format('api.withdraw_invitation(iamb_test.dev(''ines''), 1, %L)', i_tom));
    perform iamb_test.eq(r ->> 'error', 'already_accepted', 'withdraw after accept');
    r := iamb_test.call('ines', format('api.withdraw_invitation(iamb_test.dev(''ines''), 1, %L)', i_bo));
    r := iamb_test.call('bo', format('api.accept_invitation(iamb_test.dev(''bo''), 1, %L)', i_bo));
    perform iamb_test.eq(r ->> 'error', 'invitation_invalid', 'accept after withdraw');
    -- members(): addresses for owner and admins only.
    r := iamb_test.call('ana', format('api.members(iamb_test.dev(''ana''), 1, %L)', g));
    perform iamb_test.ok(jsonb_array_length(r -> 'members') = 3 and not exists (select 1 from jsonb_array_elements(r -> 'members') m where m ->> 'email' is null),
                         'an admin sees addresses: ' || r::text);

    -- Roles: an admin changes member <-> admin, never the owner's role, never removes the owner.
    r := iamb_test.call('ana', format('api.set_member_role(iamb_test.dev(''ana''), 1, %L, %L, ''admin'')', g, iamb_test.uid('tom')));
    perform iamb_test.ok(r ->> 'error' is null, 'admin raises a member');
    r := iamb_test.call('ana', format('api.set_member_role(iamb_test.dev(''ana''), 1, %L, %L, ''member'')', g, iamb_test.uid('tom')));
    r := iamb_test.call('ana', format('api.set_member_role(iamb_test.dev(''ana''), 1, %L, %L, ''member'')', g, iamb_test.uid('ines')));
    perform iamb_test.eq(r ->> 'error', 'owner_role', 'the owner''s role');
    r := iamb_test.call('ana', format('api.set_member_role(iamb_test.dev(''ana''), 1, %L, %L, ''owner'')', g, iamb_test.uid('tom')));
    perform iamb_test.eq(r ->> 'error', 'invalid', 'nobody is made owner by a role change');
    r := iamb_test.call('ana', format('api.remove_member(iamb_test.dev(''ana''), 1, %L, %L)', g, iamb_test.uid('ines')));
    perform iamb_test.eq(r ->> 'error', 'owner_role', 'remove the owner');
    r := iamb_test.call('ana', format('api.remove_member(iamb_test.dev(''ana''), 1, %L, %L)', g, iamb_test.uid('bo')));
    perform iamb_test.eq(r ->> 'error', 'not_a_member', 'remove a non-member');
    r := iamb_test.call('ana', format('api.rename_group(iamb_test.dev(''ana''), 1, %L, ''Family'')', g));
    perform iamb_test.ok(r ->> 'error' is null, 'an admin renames');
    r := iamb_test.call('ana', format('api.transfer_ownership(iamb_test.dev(''ana''), 1, %L, %L)', g, iamb_test.uid('ana')));
    perform iamb_test.eq(r ->> 'error', 'not_owner', 'an admin offers the ownership');
    r := iamb_test.call('ana', format('api.delete_group(iamb_test.dev(''ana''), 1, %L)', g));
    perform iamb_test.eq(r ->> 'error', 'not_owner', 'an admin deletes the group');
    r := iamb_test.call('ines', format('api.leave_group(iamb_test.dev(''ines''), 1, %L)', g));
    perform iamb_test.eq(r ->> 'error', 'owner_cannot_leave', 'the owner leaves');
    r := iamb_test.call('bo', format('api.leave_group(iamb_test.dev(''bo''), 1, %L)', g));
    perform iamb_test.eq(r ->> 'error', 'not_a_member', 'a non-member leaves');
    r := iamb_test.call('bo', format('api.rename_group(iamb_test.dev(''bo''), 1, %L, ''Mine'')', g));
    perform iamb_test.eq(r ->> 'error', 'not_found', 'a non-member names a group');

    -- Ownership by offer and acceptance (C16, S34).
    r := iamb_test.call('ines', format('api.transfer_ownership(iamb_test.dev(''ines''), 1, %L, %L)', g, iamb_test.uid('bo')));
    perform iamb_test.eq(r ->> 'error', 'not_a_member', 'an offer to a non-member');
    r := iamb_test.call('ines', format('api.transfer_ownership(iamb_test.dev(''ines''), 1, %L, %L)', g, iamb_test.uid('tom')));
    perform iamb_test.ok(r ->> 'error' is null, 'offer to tom');
    perform iamb_test.eq((select role from iamb.membership where profile_id = g and user_id = iamb_test.uid('tom') and deleted_at is null),
                         'member', 'an offer changes no role');
    r := iamb_test.call('ana', format('api.accept_ownership(iamb_test.dev(''ana''), 1, %L)', g));
    perform iamb_test.eq(r ->> 'error', 'no_offer', 'someone else accepts the offer');
    select limits into lim from iamb.plan where id = 'default-user';
    update iamb.plan set limits = limits || '{"groups_owned_max": 0}' where id = 'default-user';
    r := iamb_test.call('tom', format('api.accept_ownership(iamb_test.dev(''tom''), 1, %L)', g));
    perform iamb_test.eq(r ->> 'error', 'plan_limit', 'his own plan must allow one more group');
    update iamb.plan set limits = lim where id = 'default-user';
    r := iamb_test.call('tom', format('api.accept_ownership(iamb_test.dev(''tom''), 1, %L)', g));
    perform iamb_test.ok(r ->> 'error' is null, 'tom accepts the ownership ' || r::text);
    perform iamb_test.eq((select owner_user_id from iamb.profile where id = g), iamb_test.uid('tom'), 'the profile names the new owner');
    perform iamb_test.eq((select role from iamb.membership where profile_id = g and user_id = iamb_test.uid('ines') and deleted_at is null),
                         'admin', 'the old owner becomes admin');
    perform iamb_test.eq((select count(*)::integer from iamb.membership where profile_id = g and role = 'owner' and deleted_at is null), 1,
                         'exactly one owner');
    -- Decline, and withdraw an offer.
    r := iamb_test.call('tom', format('api.transfer_ownership(iamb_test.dev(''tom''), 1, %L, %L)', g, iamb_test.uid('ana')));
    r := iamb_test.call('ana', format('api.decline_ownership(iamb_test.dev(''ana''), 1, %L)', g));
    perform iamb_test.ok(r ->> 'error' is null and (select offered_owner from iamb.profile where id = g) is null, 'declined');
    r := iamb_test.call('tom', format('api.delete_group(iamb_test.dev(''tom''), 1, %L)', g));
    perform iamb_test.ok(r ->> 'error' is null, 'the owner deletes the group');
    r := iamb_test.call('tom', format('api.rename_group(iamb_test.dev(''tom''), 1, %L, ''Back'')', g));
    perform iamb_test.eq(r ->> 'error', 'not_found', 'no call brings a deleted group back');
    perform iamb_test.eq(iamb_test.visible('ines', 'profile'), 1, 'nobody reads a deleted group');
    perform iamb_test.check_all();
end $$;
