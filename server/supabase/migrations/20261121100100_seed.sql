-- Iamb server, migration 2: the server's one state row, the first epoch, the default plans
-- (open question 4's recommended values) and the private storage bucket for attachment files.
-- Every statement keeps a value an operator has changed since.

do $$
begin
    if current_user <> 'postgres' then
        raise exception 'iamb migrations must run as postgres, not %', current_user;
    end if;
end
$$;

do $$
declare
    e uuid;
    now_ms bigint := (extract(epoch from clock_timestamp()) * 1000)::bigint;
begin
    if not exists (select 1 from iamb.server_state) then
        e := gen_random_uuid();
        insert into iamb.epoch (id, started_at, first_rev) values (e, now_ms, 1);
        insert into iamb.server_state (epoch, protocol_min, protocol_max, protocol_oldest,
                                       sign_in_methods, rate_limits)
        values (e, 1, 1, 1,
                -- The link or code sent to the account's address, and identity providers
                -- (C25). `password` is never accepted; see server/README.md for how the names
                -- were chosen and which still need a sign-in on the project to confirm.
                array['otp', 'magiclink', 'email/signup', 'invite', 'oauth'],
                jsonb_build_object(
                    'push_calls', 60, 'push_ops', 6000, 'pull_calls', 120, 'read_pages', 300,
                    'hello_calls', 10, 'account_calls', 30, 'other_calls', 300,
                    'invites_group_hour', 20, 'invites_account_day', 50));
    end if;
end
$$;

insert into iamb.plan (id, scope, name, limits, features, is_default, active) values
    ('default-user', 'user', 'Default',
     '{"devices_max": 5, "snippets_max": 10000, "folders_max": 10000, "shared_folders_max": 10,
       "groups_owned_max": 3, "storage_bytes_max": 524288000, "history_days": 90}',
     '{"sync": true}', true, true),
    ('default-group', 'group', 'Default group',
     '{"snippets_max": 10000, "folders_max": 10000, "members_max": 10,
       "storage_bytes_max": 524288000, "history_days": 90}',
     '{}', true, true)
on conflict (id) do nothing;

-- One private bucket; objects are `<profile id>/<sha256>` (verified) and
-- `<profile id>/upload/<upload id>` (under way). §5.9, §5.13.
insert into storage.buckets (id, name, public, file_size_limit)
values ('attachments', 'attachments', false, 26214400)
on conflict (id) do nothing;
