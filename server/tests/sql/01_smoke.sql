-- Smoke: an account registers, creates a folder and a snippet, and reads them back.
do $$
declare
    h jsonb;
    r jsonb;
    f uuid := iamb_test.id();
    s uuid := iamb_test.id();
    p jsonb;
begin
    perform iamb_test.new_user('ines');
    h := iamb_test.hello('ines', 'Ines');
    perform iamb_test.ok(h ->> 'error' is null, 'hello: ' || h::text);
    perform iamb_test.ok(iamb_test.pos('ines') is not null, 'snapshot gave a position');
    r := iamb_test.push('ines', jsonb_build_array(
        iamb_test.op_folder(f, iamb_test.personal('ines'), null, 'Replies'),
        iamb_test.op_snippet(s, f, ';ty', 'Thank you.')));
    perform iamb_test.ok(r ->> 'error' is null, 'push: ' || r::text);
    perform iamb_test.eq(iamb_test.st(r, 1), 'applied', 'folder applied');
    perform iamb_test.eq(iamb_test.st(r, 2), 'applied', 'snippet applied');
    p := iamb_test.pull('ines');
    perform iamb_test.ok(p ->> 'error' is null, 'pull: ' || p::text);
    perform iamb_test.ok(jsonb_array_length(p -> 'entries') >= 2, 'pull has the rows: ' || p::text);
    perform iamb_test.check_all();
end $$;
