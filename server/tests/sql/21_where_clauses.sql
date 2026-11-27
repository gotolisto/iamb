-- Security review 4.3, findings 14 and 16.
--
-- 14: the role the REST interface connects with loads `safeupdate` on the platform, which refuses
-- an UPDATE or DELETE without a WHERE clause, also inside a definer function. No function of Iamb
-- may hold such a statement. This is the catalog check, which runs everywhere (also on the hosted
-- project); where `safeupdate.so` is installed, `run.sh` also loads it in every session of the
-- test cluster, so that every statement the tests reach is checked by the library itself.
-- 16: no function keeps rows in a temporary table of the caller's session.
do $$
declare
    f    record;
    stmt text;
    bad  text := '';
    n    integer := 0;
begin
    for f in select s.nspname, p.proname, p.prosrc
             from pg_proc p join pg_namespace s on s.oid = p.pronamespace
             where s.nspname in ('iamb', 'iamb_private', 'api', 'api_service') and p.prokind in ('f', 'p') loop
        n := n + 1;
        -- Comments out, then statement by statement. A statement that deletes or updates must
        -- hold a WHERE (`on conflict ... do update set` and `for update` are not statements).
        for stmt in select x from regexp_split_to_table(regexp_replace(f.prosrc, '--[^\n]*', '', 'g'), ';') x loop
            if (stmt ~* '\mdelete\s+from\M'
                or stmt ~* '\mupdate\s+(only\s+)?[a-z_][a-z0-9_.]*(\s+(as\s+)?[a-z_][a-z0-9_]*)?\s+set\M')
               and stmt !~* '\mwhere\M' then
                bad := bad || format(E'\n  %s.%s: %s', f.nspname, f.proname, left(regexp_replace(btrim(stmt), '\s+', ' ', 'g'), 90));
            end if;
        end loop;
    end loop;
    perform iamb_test.ok(n > 100, 'the catalog check saw the functions (' || n || ')');
    if bad <> '' then
        raise exception 'check failed: a DELETE or UPDATE without a WHERE clause (safeupdate refuses it on the platform):%', bad;
    end if;
end $$;

-- 16, by trying: a caller who could send statements of his own makes, before a writing function
-- runs in his session, temporary tables with the names of the scratch tables of task 4.2 and a
-- trigger on one. Nothing of Iamb touches them: the trigger does not run, and his role is what it
-- was. (No device can do this: the REST interface runs one function call per request. `anon` and
-- `authenticated` hold the TEMPORARY privilege on the platform.)
do $$
declare
    ids  jsonb;
    r    jsonb;
    made boolean := true;
begin
    perform set_config('role', 'authenticated', false);
    begin
        execute 'create temp table iamb_feed_pending (ord bigserial, user_id uuid not null, kind text not null, entity text,
                     row_id uuid, folder_id uuid, profile_id uuid, sha256 text, detail jsonb)';
        execute 'create temp table iamb_tx_rows (entity text not null, id uuid not null, created boolean not null, primary key (entity, id))';
        execute $f$create function pg_temp.iamb_hijack() returns trigger language plpgsql as $t$
                   begin
                       update iamb.membership set role = 'admin' where user_id = auth.uid() and deleted_at is null and role = 'member';
                       return new;
                   end $t$ $f$;
        execute 'create trigger iamb_hijack before insert on pg_temp.iamb_feed_pending for each row execute function pg_temp.iamb_hijack()';
    exception when insufficient_privilege then
        made := false;      -- a cluster where the role may not make temporary tables at all
    end;
    perform iamb_test.act_owner();
    ids := iamb_test.family('edit');
    r := iamb_test.call('tom', 'api.set_display_name(iamb_test.dev(''tom''), 1, ''Tom'')');
    if made then
        execute 'drop table pg_temp.iamb_feed_pending, pg_temp.iamb_tx_rows';
        execute 'drop function pg_temp.iamb_hijack()';
    end if;
    perform iamb_test.ok(r ->> 'error' is null, 'the call works: ' || r::text);
    perform iamb_test.eq((select role from iamb.membership where user_id = iamb_test.uid('tom') and deleted_at is null),
                         'member', 'the caller''s temporary table and its trigger were not used');
    perform iamb_test.ok(exists (select 1 from iamb.feed f where f.user_id = iamb_test.uid('tom') and f.kind = 'account'),
                         'the feed entry of the call was written');
    perform iamb_test.check_all();
end $$;
