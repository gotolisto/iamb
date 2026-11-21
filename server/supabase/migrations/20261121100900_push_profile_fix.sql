-- Iamb server, migration 10: the owning profile of a delete whose row the server did not know
-- before the request, but which another op of the same request creates (C24: "else from another op
-- of the same request"). Migration 6 answered such a delete `rejected: parent_gone`.

do $$
begin
    if current_user <> 'postgres' then
        raise exception 'iamb migrations must run as postgres, not %', current_user;
    end if;
end
$$;

create or replace function iamb_private.op_profile(op jsonb, ops jsonb) returns uuid
language plpgsql stable set search_path = '' as $$
declare
    ent text := op ->> 'entity';
    rid uuid := iamb_private.try_uuid(op ->> 'id');
    r   jsonb := op -> 'row';
    p   uuid;
    ref uuid;
    o   jsonb;
begin
    p := iamb_private.row_profile(ent, rid);
    if p is not null then
        return p;
    end if;
    if op ->> 'kind' = 'delete' then
        -- A row this request creates and deletes again: the put names its profile.
        select x into o from jsonb_array_elements(ops) x
        where x ->> 'kind' = 'put' and x ->> 'entity' = ent and iamb_private.try_uuid(x ->> 'id') = rid limit 1;
        return case when o is null then null else iamb_private.op_profile(o, ops) end;
    end if;
    case ent
    when 'folder', 'tag', 'attachment' then
        return iamb_private.try_uuid(r ->> 'profile_id');
    when 'snippet', 'folder_share' then
        ref := iamb_private.try_uuid(r ->> 'folder_id');
        p := (select profile_id from iamb.folder where id = ref);
        if p is null then
            select iamb_private.try_uuid(x -> 'row' ->> 'profile_id') into p
            from jsonb_array_elements(ops) x
            where x ->> 'entity' = 'folder' and iamb_private.try_uuid(x ->> 'id') = ref limit 1;
        end if;
        return p;
    when 'snippet_tag', 'snippet_attachment' then
        ref := iamb_private.try_uuid(r ->> 'snippet_id');
        p := (select profile_id from iamb.snippet where id = ref);
        if p is null then
            select x into o from jsonb_array_elements(ops) x
            where x ->> 'entity' = 'snippet' and iamb_private.try_uuid(x ->> 'id') = ref limit 1;
            if o is not null then
                return iamb_private.op_profile(o, ops);
            end if;
        end if;
        return p;
    else
        return null;
    end case;
end
$$;

revoke execute on function iamb_private.op_profile(jsonb, jsonb) from public, anon, authenticated, service_role;
