-- Security review 4.3, finding 8: uploads under way. A member who may change one folder of the
-- owner's can no longer reserve her storage allowance with uploads he never sends, cannot read the
-- size of her files off the answers, and she sees his open uploads and can end them.
do $$
declare
    ids jsonb := iamb_test.family('edit'); p_ines uuid := (ids ->> 'p_ines')::uuid; i integer := 0; r jsonb;
    sz bigint := 26214400; n integer := 0; total bigint := 0; up uuid; first_refusal bigint; second_refusal bigint;
begin
    update iamb.server_state set rate_limits = rate_limits || '{"account_calls": 100000}' where id;
    -- Tom opens uploads in Ines's profile, as large as he can, and never sends a byte.
    while sz >= 1024 and i < 200 loop
        i := i + 1;
        r := iamb_test.call('tom', format('api.attachment_upload_begin(iamb_test.dev(''tom''), 1, %L, %L, %s, ''image/png'')', p_ines, md5(i::text) || md5(i::text), sz));
        if r ->> 'error' is null then n := n + 1; total := total + sz; up := (r ->> 'upload')::uuid;
        elsif r ->> 'error' = 'plan_limit' then sz := sz / 2;
        else raise exception 'unexpected answer %', r; end if;
    end loop;
    perform iamb_test.ok(n <= 20, 'Tom holds ' || n || ' open uploads (at most 20 per account)');
    -- The owner's own upload is not in his way.
    r := iamb_test.call('ines', format('api.attachment_upload_begin(iamb_test.dev(''ines''), 1, %L, %L, 26214400, ''image/png'')', p_ines, repeat('d4', 32)));
    perform iamb_test.ok(r ->> 'error' is null, 'Ines, a file of 25 MiB in her own profile: ' || r::text);
    -- She sees his open uploads, and ends one; he sees his own only.
    perform iamb_test.eq(iamb_test.visible('ines', 'attachment_upload'), n + 1, 'the owner sees the uploads under way in her profile');
    perform iamb_test.eq(iamb_test.visible('tom', 'attachment_upload'), n, 'the uploader sees his own');
    perform iamb_test.eq(iamb_test.visible('stranger', 'attachment_upload'), 0, 'a stranger none');
    r := iamb_test.call('stranger', format('api.attachment_upload_cancel(iamb_test.dev(''stranger''), 1, %L)', up));
    perform iamb_test.eq(r ->> 'error', 'not_found', 'a stranger cannot end it');
    r := iamb_test.call('ines', format('api.attachment_upload_cancel(iamb_test.dev(''ines''), 1, %L)', up));
    perform iamb_test.ok(r ->> 'error' is null and not exists (select 1 from iamb.attachment_upload where id = up), 'the owner ends it: ' || r::text);
    r := iamb_test.call('ines', format('api.attachment_upload_cancel(iamb_test.dev(''ines''), 1, %L)', up));
    perform iamb_test.eq(r ->> 'error', 'not_found', 'once');
    select id into up from iamb.attachment_upload where uploaded_by = iamb_test.uid('tom') limit 1;
    r := iamb_test.call('tom', format('api.attachment_upload_cancel(iamb_test.dev(''tom''), 1, %L)', up));
    perform iamb_test.ok(r ->> 'error' is null, 'the uploader ends his own: ' || r::text);
    -- A declared size of 0 reserves nothing and is refused.
    r := iamb_test.call('tom', format('api.attachment_upload_begin(iamb_test.dev(''tom''), 1, %L, %L, 0, ''image/png'')', p_ines, repeat('e5', 32)));
    perform iamb_test.eq(r ->> 'error', 'invalid', 'a declared size of 0');
    perform iamb_test.check_all();
end $$;

-- The answers tell a member nothing about the size of the owner's files: whatever she holds, the
-- same calls by him get the same answers.
do $$
declare
    ids jsonb; p_ines uuid; r jsonb; run integer; answers text[]; seen text[] := '{}'; i integer;
begin
    update iamb.server_state set rate_limits = rate_limits || '{"account_calls": 100000}' where id;
    for run in 1..2 loop
        ids := iamb_test.family('edit'); p_ines := (ids ->> 'p_ines')::uuid;
        if run = 2 then
            -- This time the owner's profile is almost full.
            insert into iamb.attachment_blob values (p_ines, repeat('a1', 32), 524288000 - 1000, 'image/png', 1);
        end if;
        answers := '{}';
        for i in 1..25 loop
            r := iamb_test.call('tom', format('api.attachment_upload_begin(iamb_test.dev(''tom''), 1, %L, %L, %s, ''image/png'')',
                                              p_ines, md5(i::text) || md5(i::text), 26214400));
            answers := answers || coalesce(r ->> 'error', 'ok');
        end loop;
        seen := seen || array_to_string(answers, ',');
    end loop;
    perform iamb_test.eq(seen[1], seen[2], 'the same answers whether her profile is empty or full');
    -- The profile's allowance is checked when his upload is committed: a file that does not fit is
    -- not recorded.
    r := api_service.attachment_verified((select id from iamb.attachment_upload where uploaded_by = iamb_test.uid('tom') limit 1),
                                         (select sha256 from iamb.attachment_upload where uploaded_by = iamb_test.uid('tom') limit 1), 26214400);
    perform iamb_test.eq(r ->> 'error', 'plan_limit', 'the commit of a file that does not fit the profile');
    perform iamb_test.eq((select count(*)::integer from iamb.attachment_blob where profile_id = p_ines), 1, 'no file recorded');
    -- And her own upload is refused at once when her profile is full, as before.
    r := iamb_test.call('ines', format('api.attachment_upload_begin(iamb_test.dev(''ines''), 1, %L, %L, 2000, ''image/png'')', p_ines, repeat('d4', 32)));
    perform iamb_test.eq(r ->> 'error', 'plan_limit', 'the owner over her own allowance');
    r := iamb_test.call('ines', format('api.attachment_upload_begin(iamb_test.dev(''ines''), 1, %L, %L, 900, ''image/png'')', p_ines, repeat('d4', 32)));
    perform iamb_test.ok(r ->> 'error' is null, 'the owner within it: ' || r::text);
    r := api_service.attachment_verified((r ->> 'upload')::uuid, repeat('d4', 32), 900);
    perform iamb_test.ok(r ->> 'error' is null, 'and its commit: ' || r::text);
end $$;
