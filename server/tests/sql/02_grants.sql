-- G4, G17 and the KPI "row-level security on 100% of tables": what each role may do, from the
-- catalog and by trying.
do $$
declare
    t   record;
    f   record;
    n   integer;
    ok  boolean;
begin
    -- Row-level security is on for every table in Iamb's schemas and in every schema the REST
    -- interface exposes to devices (`api`, `api_service`, which hold no table).
    select count(*) into n from pg_class c join pg_namespace s on s.oid = c.relnamespace
    where s.nspname in ('iamb', 'iamb_private', 'api', 'api_service') and c.relkind in ('r', 'p') and not c.relrowsecurity;
    perform iamb_test.eq(n, 0, 'tables without row-level security');
    select count(*) into n from pg_class c join pg_namespace s on s.oid = c.relnamespace
    where s.nspname in ('iamb_private', 'api', 'api_service') and c.relkind in ('r', 'p', 'v', 'm', 'f');
    perform iamb_test.eq(n, 0, 'relations outside the schema iamb');

    -- Policies are read rules only (§5.13).
    select count(*) into n from pg_policies where schemaname = 'iamb' and cmd <> 'SELECT';
    perform iamb_test.eq(n, 0, 'insert, update or delete policies');

    -- No role a device can have holds any write privilege on a table of iamb (G4).
    select count(*) into n from pg_class c join pg_namespace s on s.oid = c.relnamespace,
         unnest(array['anon', 'authenticated', 'service_role']) r, unnest(array['INSERT', 'UPDATE', 'DELETE', 'TRUNCATE']) pr
    where s.nspname = 'iamb' and c.relkind in ('r', 'p', 'v') and has_table_privilege(r, c.oid, pr);
    perform iamb_test.eq(n, 0, 'write privileges on iamb tables');
    select count(*) into n from pg_class c join pg_namespace s on s.oid = c.relnamespace
    where s.nspname = 'iamb' and c.relkind in ('r', 'p', 'v') and has_table_privilege('anon', c.oid, 'SELECT');
    perform iamb_test.eq(n, 0, 'anon reads a table');
    perform iamb_test.ok(not has_schema_privilege('anon', 'iamb', 'USAGE') and not has_schema_privilege('anon', 'api', 'USAGE')
                         and not has_schema_privilege('anon', 'iamb_private', 'USAGE') and not has_schema_privilege('anon', 'api_service', 'USAGE'),
                         'anon has no schema of Iamb');
    perform iamb_test.ok(not has_schema_privilege('authenticated', 'api_service', 'USAGE'), 'devices cannot reach api_service');

    -- Functions: nothing for anon or PUBLIC; `api` for authenticated; in iamb_private exactly the
    -- gate helpers; api_service for service_role only.
    for f in select p.oid, s.nspname, p.proname, p.prokind, p.prosecdef, p.proconfig, p.prosrc,
                    pg_get_function_identity_arguments(p.oid) as args
             from pg_proc p join pg_namespace s on s.oid = p.pronamespace
             where s.nspname in ('api', 'iamb_private', 'api_service') loop
        perform iamb_test.ok(not has_function_privilege('anon', f.oid, 'EXECUTE'), 'anon executes ' || f.nspname || '.' || f.proname);
        perform iamb_test.ok(not exists (select 1 from aclexplode(coalesce((select proacl from pg_proc where oid = f.oid), acldefault('f', 10::oid))) a
                                         where a.grantee = 0 and a.privilege_type = 'EXECUTE'),
                             'PUBLIC executes ' || f.nspname || '.' || f.proname);
        ok := has_function_privilege('authenticated', f.oid, 'EXECUTE');
        if f.nspname = 'api' then
            perform iamb_test.ok(ok, 'authenticated executes api.' || f.proname);
        elsif f.nspname = 'iamb_private' then
            perform iamb_test.eq(ok, f.proname in ('session_ok', 'member_of', 'role_in', 'shares_group_with',
                                                   'own_address', 'enter', 'position'),
                                 'authenticated executes iamb_private.' || f.proname);
        else
            perform iamb_test.ok(not ok and has_function_privilege('service_role', f.oid, 'EXECUTE'),
                                 'api_service.' || f.proname || ' for service_role only');
        end if;
        -- G17: a fixed search_path everywhere, and no statement built at run time.
        perform iamb_test.ok(exists (select 1 from unnest(f.proconfig) c where c like 'search_path=%'),
                             f.nspname || '.' || f.proname || ' has a fixed search_path');
        perform iamb_test.ok(f.prosrc !~* '\mexecute\M\s+(format|''|\$)' , f.nspname || '.' || f.proname || ' builds no statement');
        -- Every function of api that writes is security definer; the readers of §5.1 are invoker.
        if f.nspname = 'api' then
            perform iamb_test.eq(f.prosecdef, f.proname not in ('pull', 'rows', 'fetch_folder', 'snapshot', 'visible_folders',
                                                                'access', 'users', 'check', 'invitations', 'devices', 'conflicts'),
                                 'api.' || f.proname || ' definer');
            perform iamb_test.ok(exists (select 1 from unnest(f.proconfig) c where c like 'statement_timeout=%'),
                                 'api.' || f.proname || ' has its own statement_timeout');
        end if;
        if f.nspname = 'iamb_private' and f.prosecdef then
            perform iamb_test.ok(f.proname in ('session_ok', 'member_of', 'role_in', 'shares_group_with',
                                               'own_address', 'enter', 'position'), 'definer helper ' || f.proname);
        end if;
    end loop;
    -- The token hook is executable by the authentication service's role and nobody else.
    perform iamb_test.ok(has_function_privilege('supabase_auth_admin', 'iamb_private.token_hook(jsonb)', 'EXECUTE')
                         and not has_function_privilege('authenticated', 'iamb_private.token_hook(jsonb)', 'EXECUTE'),
                         'token hook grants');

    -- The revision counter has no cache (§5.1, §11.6).
    perform iamb_test.eq((select seqcache from pg_sequence where seqrelid = 'iamb.rev_seq'::regclass), 1::bigint, 'rev_seq cache');
    -- Every function of iamb_private and api is owned by the tables' owner.
    select count(*) into n from pg_proc p join pg_namespace s on s.oid = p.pronamespace
    where s.nspname in ('api', 'iamb_private', 'api_service') and p.proowner <> (select relowner from pg_class where oid = 'iamb.snippet'::regclass);
    perform iamb_test.eq(n, 0, 'functions owned by someone else');
end $$;

-- Trying it: anon and authenticated are refused every write, and anon every read and call.
do $$
declare
    t     text;
    ok    boolean;
    r     jsonb;
begin
    perform iamb_test.new_user('ines');
    perform iamb_test.hello('ines', 'Ines');
    foreach t in array array['profile', 'membership', 'folder', 'folder_share', 'snippet', 'tag', 'snippet_tag',
                             'attachment', 'snippet_attachment', 'user_profile', 'device', 'folder_access', 'feed',
                             'feed_head', 'history', 'conflict', 'conflict_version', 'push_receipt', 'invitation',
                             'attachment_blob', 'attachment_upload', 'plan', 'subscription', 'feature_flag',
                             'feature_override', 'server_state', 'epoch', 'epoch_profile', 'rate_bucket'] loop
        foreach r in array array['"authenticated"'::jsonb, '"anon"'::jsonb] loop
            if r #>> '{}' = 'authenticated' then perform iamb_test.act('ines'); else perform iamb_test.act_anon(); end if;
            begin
                execute format('delete from iamb.%I', t);
                ok := false;
            exception when insufficient_privilege then ok := true;
            end;
            begin
                execute format('update iamb.%I set created_at = 0', t);
                ok := false;
            exception when insufficient_privilege or undefined_column then ok := ok;
            end;
            perform iamb_test.act_owner();
            perform iamb_test.ok(ok, (r #>> '{}') || ' may write ' || t);
        end loop;
        perform iamb_test.act_anon();
        begin
            execute format('select count(*) from iamb.%I', t);
            ok := false;
        exception when insufficient_privilege then ok := true;
        end;
        perform iamb_test.act_owner();
        perform iamb_test.ok(ok, 'anon may read ' || t);
    end loop;
    perform iamb_test.act_anon();
    begin
        perform api.hello('{"id": "00000000-0000-7000-8000-000000000001"}', array[1, 1], 0);
        ok := false;
    exception when insufficient_privilege then ok := true;
    end;
    begin
        perform iamb_private.session_ok();
        ok := false;
    exception when insufficient_privilege then ok := ok;
    end;
    perform iamb_test.act_owner();
    perform iamb_test.ok(ok, 'anon may call a function');
    perform iamb_test.act('ines');
    begin
        perform iamb_private.refresh_access(array[iamb_test.personal('ines')]);
        ok := false;
    exception when insufficient_privilege then ok := true;
    end;
    begin
        perform iamb_private.begin_epoch(0);
        ok := false;
    exception when insufficient_privilege then ok := ok;
    end;
    begin
        perform api_service.delete_account_data(iamb_test.uid('ines'));
        ok := false;
    exception when insufficient_privilege then ok := ok;
    end;
    perform iamb_test.act_owner();
    perform iamb_test.ok(ok, 'a device calls an owner-only function');
end $$;

-- Whatever a device sends to the gate helpers, server_state, epochs, other feeds and other
-- devices' rows are as they were (G4).
do $$
declare
    before_state jsonb;
    before_epochs bigint;
    before_dev jsonb;
    r jsonb;
    claim bigint;
begin
    perform iamb_test.new_user('mallory');
    perform iamb_test.new_user('victim');
    perform iamb_test.hello('mallory', 'M');
    perform iamb_test.hello('victim', 'V');
    select to_jsonb(s) into before_state from iamb.server_state s;
    select count(*) into before_epochs from iamb.epoch;
    select jsonb_agg(to_jsonb(d) order by d.id) into before_dev from iamb.device d where d.user_id = iamb_test.uid('victim');
    perform iamb_test.act('mallory');
    r := iamb_private.enter(iamb_test.dev('victim'), 1, 'push');
    perform iamb_test.ok(r ->> 'error' in ('device_conflict', 'device_unknown'), 'enter with another device: ' || r::text);
    r := iamb_private.position(iamb_test.dev('victim'), iamb_test.pos('victim'), 'pull', 99);
    perform iamb_test.eq(r ->> 'error', 'device_unknown', 'position of another device');
    r := iamb_private.enter(iamb_test.dev('mallory'), 99, 'pull');
    perform iamb_test.eq(r ->> 'error', 'server_too_old', 'protocol above the range');
    r := iamb_private.enter(iamb_test.dev('mallory'), 0, 'pull');
    perform iamb_test.eq(r ->> 'error', 'upgrade_required', 'protocol below the range');
    perform iamb_test.act_owner();
    claim := (select last_value from iamb.rev_seq) + 1000;
    perform iamb_test.act('mallory');
    r := api.hello(jsonb_build_object('id', iamb_test.dev('mallory'), 'name', 'x', 'platform', 'linux', 'app_version', '1'),
                   array[1, 1], claim);
    perform iamb_test.eq(r ->> 'error', 'server_behind', 'a device that claims more than the server');
    perform iamb_test.act_owner();
    perform iamb_test.eq((select to_jsonb(s) from iamb.server_state s), before_state, 'server_state unchanged');
    perform iamb_test.eq((select count(*) from iamb.epoch), before_epochs, 'no epoch begun');
    perform iamb_test.eq((select jsonb_agg(to_jsonb(d) order by d.id) from iamb.device d where d.user_id = iamb_test.uid('victim')),
                         before_dev, 'the other device row unchanged');
end $$;
