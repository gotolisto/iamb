-- Re-verification of the security review (docs/reviews/4.3-security-reverify.md), N6: a party's
-- answer to a conflict closes it for everybody only when that party is offered every version the
-- conflict holds (C20, ADR-0031). Otherwise it closes it for that party alone.

-- Ines's two computers disagree about a snippet in her private folder; the conflict stays open.
-- She moves the snippet into the shared folder; Tom's stale edit takes her conflict over. He is
-- offered the version his write replaced and not her private one.
do $$
declare
    ids jsonb := iamb_test.family('edit');
    f1 uuid := (ids ->> 'f1')::uuid; f2 uuid := (ids ->> 'f2')::uuid; s2 uuid := (ids ->> 's2')::uuid;
    r1 bigint; priv bigint; r jsonb; c uuid; p jsonb;
begin
    perform iamb_test.new_device('ines2', 'ines'); perform iamb_test.hello('ines2');
    r1 := iamb_test.rev('snippet', s2);
    perform iamb_test.push('ines',  jsonb_build_array(iamb_test.op_snippet(s2, f2, ';priv', 'her lost private version', r1)));
    priv := iamb_test.rev('snippet', s2);
    perform iamb_test.push('ines2', jsonb_build_array(iamb_test.op_snippet(s2, f2, ';priv', 'Nothing to see', r1)));
    perform iamb_test.push('ines2', jsonb_build_array(iamb_test.op_snippet(s2, f1, ';priv', 'Nothing to see', iamb_test.rev('snippet', s2))));
    perform iamb_test.pull('tom'); perform iamb_test.pull('ines');
    r := iamb_test.push('tom', jsonb_build_array(iamb_test.op_snippet(s2, f1, ';priv', 'Tom was here', 0)));
    c := (r -> 'results' -> 0 ->> 'conflict')::uuid;
    perform iamb_test.ok(c is not null, 'Tom''s stale write records a conflict');
    r := iamb_test.call('tom', format('api.get_conflict(iamb_test.dev(''tom''), 1, %L)', c));
    perform iamb_test.eq((select jsonb_agg(v -> 'row' -> 'body') from jsonb_array_elements(r -> 'versions') v), '["Nothing to see"]'::jsonb, 'what Tom is offered');
    perform iamb_test.pull('tom'); perform iamb_test.pull('ines');
    -- Tom answers "Keep the current version".
    r := iamb_test.call('tom', format('api.resolve_conflict(iamb_test.dev(''tom''), 1, %L, ''keep'')', c));
    perform iamb_test.ok(r ->> 'error' is null and r ->> 'state' = 'resolved', 'Tom keeps: ' || r::text);
    -- For him it is closed ...
    r := iamb_test.call('tom', 'api.conflicts(iamb_test.dev(''tom''), 1)');
    perform iamb_test.eq(jsonb_array_length(r -> 'conflicts'), 0, 'it has left Tom''s list');
    r := iamb_test.call('tom', format('api.get_conflict(iamb_test.dev(''tom''), 1, %L)', c));
    perform iamb_test.eq(r -> 'conflict' ->> 'state', 'resolved', 'and reads as resolved for him');
    r := iamb_test.call('tom', format('api.resolve_conflict(iamb_test.dev(''tom''), 1, %L, ''keep'')', c));
    perform iamb_test.ok(r ->> 'error' is null and r ->> 'state' = 'resolved', 'his repeated answer finds what it asked for');
    p := iamb_test.pull('tom');
    perform iamb_test.ok(exists (select 1 from jsonb_array_elements(p -> 'entries') e where e ->> 'kind' = 'conflict'), 'his feed says so');
    -- ... and for Ines it is open, with her private version on offer; her feed got nothing.
    p := iamb_test.pull('ines');
    perform iamb_test.eq(jsonb_array_length(p -> 'entries'), 0, 'nothing in the owner''s feed');
    perform iamb_test.eq((select state from iamb.conflict where id = c), 'open', 'the conflict is open');
    r := iamb_test.call('ines', 'api.conflicts(iamb_test.dev(''ines''), 1)');
    perform iamb_test.ok(exists (select 1 from jsonb_array_elements(r -> 'conflicts') x where x ->> 'id' = c::text), 'it is in the owner''s list');
    r := iamb_test.call('ines', format('api.get_conflict(iamb_test.dev(''ines''), 1, %L)', c));
    perform iamb_test.ok(r -> 'conflict' ->> 'state' = 'open' and r::text like '%her lost private version%', 'with her private version on offer');
    r := iamb_test.call('ines', format('api.resolve_conflict(iamb_test.dev(''ines''), 1, %L, ''restore'', %s)', c, priv));
    perform iamb_test.ok(r ->> 'error' is null and r ->> 'state' = 'resolved', 'she restores it: ' || r::text);
    perform iamb_test.eq((select body from iamb.snippet where id = s2), 'her lost private version', 'the row has her version');
    perform iamb_test.eq((select state from iamb.conflict where id = c), 'resolved', 'and the conflict is closed for everybody: she is offered every version');
    perform iamb_test.check_all();
end $$;

-- The same when Tom restores the version he is offered: the row is written, the conflict leaves
-- his list, and it stays open for Ines with her private version.
do $$
declare
    ids jsonb := iamb_test.family('edit');
    f1 uuid := (ids ->> 'f1')::uuid; f2 uuid := (ids ->> 'f2')::uuid; s2 uuid := (ids ->> 's2')::uuid;
    r1 bigint; priv bigint; shown bigint; r jsonb; c uuid;
begin
    perform iamb_test.new_device('ines2', 'ines'); perform iamb_test.hello('ines2');
    r1 := iamb_test.rev('snippet', s2);
    perform iamb_test.push('ines',  jsonb_build_array(iamb_test.op_snippet(s2, f2, ';priv', 'her lost private version', r1)));
    priv := iamb_test.rev('snippet', s2);
    perform iamb_test.push('ines2', jsonb_build_array(iamb_test.op_snippet(s2, f2, ';priv', 'Nothing to see', r1)));
    perform iamb_test.push('ines2', jsonb_build_array(iamb_test.op_snippet(s2, f1, ';priv', 'Nothing to see', iamb_test.rev('snippet', s2))));
    shown := iamb_test.rev('snippet', s2);
    perform iamb_test.pull('tom');
    r := iamb_test.push('tom', jsonb_build_array(iamb_test.op_snippet(s2, f1, ';priv', 'Tom was here', 0)));
    c := (r -> 'results' -> 0 ->> 'conflict')::uuid;
    r := iamb_test.call('tom', format('api.resolve_conflict(iamb_test.dev(''tom''), 1, %L, ''restore'', %s)', c, shown));
    perform iamb_test.ok(r ->> 'error' is null and r ->> 'state' = 'resolved', 'Tom restores the version he is offered: ' || r::text);
    perform iamb_test.eq((select body from iamb.snippet where id = s2), 'Nothing to see', 'the row has it');
    perform iamb_test.eq(jsonb_array_length(iamb_test.call('tom', 'api.conflicts(iamb_test.dev(''tom''), 1)') -> 'conflicts'), 0, 'the conflict has left his list');
    r := iamb_test.call('ines', format('api.get_conflict(iamb_test.dev(''ines''), 1, %L)', c));
    perform iamb_test.ok(r -> 'conflict' ->> 'state' = 'open' and r::text like '%her lost private version%', 'it is open for the owner, with her private version');
    -- A party who is offered every version closes it for everybody, as before.
    r := iamb_test.call('ines', format('api.resolve_conflict(iamb_test.dev(''ines''), 1, %L, ''keep'')', c));
    perform iamb_test.ok(r ->> 'error' is null and (select state from iamb.conflict where id = c) = 'resolved', 'the owner''s "keep" closes it');
    perform iamb_test.check_all();
end $$;

-- Where every party is offered every version (the folder was shared throughout, S57), one answer
-- closes the conflict for all, as before.
do $$
declare
    ids jsonb := iamb_test.family('edit'); f1 uuid := (ids ->> 'f1')::uuid; s1 uuid := (ids ->> 's1')::uuid;
    base bigint; r jsonb; c uuid;
begin
    perform iamb_test.pull('tom');
    base := iamb_test.rev('snippet', s1);
    perform iamb_test.push('ines', jsonb_build_array(iamb_test.op_snippet(s1, f1, ';hrs', 'Ines''s hours', base)));
    r := iamb_test.push('tom', jsonb_build_array(iamb_test.op_snippet(s1, f1, ';hrs', 'Tom''s hours', base)));
    c := (r -> 'results' -> 0 ->> 'conflict')::uuid;
    r := iamb_test.call('tom', format('api.resolve_conflict(iamb_test.dev(''tom''), 1, %L, ''keep'')', c));
    perform iamb_test.ok(r ->> 'state' = 'resolved' and (select state from iamb.conflict where id = c) = 'resolved'
                         and (select closed_for from iamb.conflict where id = c) = '{}', 'a member''s "keep" closes a conflict he is offered all of');
    perform iamb_test.eq(jsonb_array_length(iamb_test.call('ines', 'api.conflicts(iamb_test.dev(''ines''), 1)') -> 'conflicts'), 0, 'it has left the owner''s list too');
    perform iamb_test.check_all();
end $$;
