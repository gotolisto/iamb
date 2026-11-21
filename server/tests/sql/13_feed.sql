-- §5.5: the feed's order, one entry per row, who gets a row entry, null rows, a page closed under
-- references, pages and `more`, and the poke on the live channel (§6.6, G16).
do $$
declare
    ids jsonb := iamb_test.family('edit');
    f1 uuid; f2 uuid; s1 uuid; s2 uuid; pi uuid; t2 uuid := iamb_test.id(); l2 uuid := iamb_test.id();
    r jsonb; p jsonb; seqs bigint[]; n integer; msg jsonb;
begin
    f1 := (ids ->> 'f1')::uuid; f2 := (ids ->> 'f2')::uuid; s1 := (ids ->> 's1')::uuid; s2 := (ids ->> 's2')::uuid;
    pi := (ids ->> 'p_ines')::uuid;
    perform iamb_test.pull('tom');
    perform iamb_test.pull('ines');
    -- Ines edits s1 twice: one entry per row in each feed; the writer's own feed gets it too.
    r := iamb_test.push('ines', jsonb_build_array(iamb_test.op_snippet(s1, f1, ';hrs', 'a', iamb_test.rev('snippet', s1))));
    r := iamb_test.push('ines', jsonb_build_array(iamb_test.op_snippet(s1, f1, ';hrs', 'b', iamb_test.rev('snippet', s1))));
    perform iamb_test.eq((select count(*)::integer from iamb.feed where user_id = iamb_test.uid('tom') and kind = 'row' and row_id = s1), 1,
                         'one entry per row');
    perform iamb_test.eq((select count(*)::integer from iamb.feed where user_id = iamb_test.uid('ines') and kind = 'row' and row_id = s1), 1,
                         'the writer''s own feed');
    p := iamb_test.pull('tom');
    perform iamb_test.ok(jsonb_array_length(p -> 'entries') = 1 and p -> 'entries' -> 0 -> 'row' ->> 'body' = 'b'
                         and p -> 'entries' -> 0 -> 'origin' ->> 'content_by' = iamb_test.uid('ines')::text,
                         'the row as it is now, with its origin: ' || p::text);
    -- The poke: {epoch, head} on feed:<user>, after the entries.
    if not iamb_test.realtime_ready() then
        raise warning 'poke checks skipped: no partition of realtime.messages for today';
    else
    select payload into msg from realtime.messages where topic = 'feed:' || iamb_test.uid('tom') order by (payload ->> 'head')::bigint desc limit 1;
    perform iamb_test.ok(msg ->> 'epoch' = (select epoch::text from iamb.server_state)
                         and (msg ->> 'head')::bigint = (select head from iamb.feed_head where user_id = iamb_test.uid('tom')),
                         'poke: ' || msg::text);
    perform iamb_test.ok(not exists (select 1 from realtime.messages where topic like 'feed:%' and not private), 'pokes are private');
    perform iamb_test.ok(not exists (select 1 from realtime.messages where payload ? 'row' or payload ? 'body'), 'pokes carry no content');
    end if;

    -- S62: s2, which carries t2, moves into the shared folder: Tom gets the snippet and its link,
    -- and the page brings the tag the link names. s1 moves out: delivered as null.
    r := iamb_test.push('ines', jsonb_build_array(iamb_test.op_tag(t2, pi, 'seasonal'), iamb_test.op_snippet_tag(l2, s2, t2)));
    r := iamb_test.push('ines', jsonb_build_array(
        iamb_test.op_snippet(s2, f1, ';priv', 'Private text.', iamb_test.rev('snippet', s2)),
        iamb_test.op_snippet(s1, f2, ';hrs', 'b', iamb_test.rev('snippet', s1))));
    p := iamb_test.pull('tom');
    perform iamb_test.ok(exists (select 1 from jsonb_array_elements(p -> 'entries') e where e ->> 'id' = s2::text and jsonb_typeof(e -> 'row') = 'object')
                         and exists (select 1 from jsonb_array_elements(p -> 'entries') e where e ->> 'id' = l2::text and jsonb_typeof(e -> 'row') = 'object')
                         and exists (select 1 from jsonb_array_elements(p -> 'refs') e where e ->> 'id' = t2::text),
                         'the snippet, its link, and the tag in refs: ' || p::text);
    perform iamb_test.ok(exists (select 1 from jsonb_array_elements(p -> 'entries') e where e ->> 'id' = s1::text and e -> 'row' = 'null'::jsonb),
                         's1 delivered as null');
    select array_agg((e ->> 'seq')::bigint) into seqs from jsonb_array_elements(p -> 'entries') e;
    perform iamb_test.ok(seqs = (select array_agg(x order by x) from unnest(seqs) x), 'entries in seq order');
    -- Pages: limit 1 gives one entry and more = true.
    r := iamb_test.push('ines', jsonb_build_array(iamb_test.op_snippet(s2, f1, ';priv', 'x', iamb_test.rev('snippet', s2)),
                                                   iamb_test.op_folder(iamb_test.id(), pi, f1, 'sub')));
    r := iamb_test.call('tom', 'api.pull(iamb_test.dev(''tom''), 1, iamb_test.pos(''tom''), 1)');
    perform iamb_test.ok(jsonb_array_length(r -> 'entries') = 1 and (r ->> 'more')::boolean, 'a page of one: ' || r::text);
    -- A folder below f1 comes with f1 in refs when f1's entry is not in the page.
    perform iamb_test.check_all();
end $$;
