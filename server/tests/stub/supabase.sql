-- A minimal stand-in for what a Supabase project provides around Iamb's schemas, so that the
-- migrations and the policy tests run against a plain Postgres (server/README.md, "What the
-- stand-in cannot model"). Run once per cluster as the bootstrap superuser, which the test runner
-- names `supabase_admin` as on the platform.
--
-- Modelled: the roles (`postgres` as a non-superuser with BYPASSRLS, `anon`, `authenticated`,
-- `service_role`, `authenticator`, `supabase_auth_admin`), `auth.users` and `auth.sessions` with
-- the columns Iamb reads, `auth.uid()`, `auth.jwt()`, `auth.role()` exactly as the platform
-- defines them, the storage bucket and object tables with row-level security, and the realtime
-- message table with `realtime.topic()` and `realtime.send()` as the platform defines them.
-- Not modelled: the HTTP services (REST interface, authentication service, storage service,
-- realtime service) that set the claims and call these tables on a client's behalf.

do $$
declare
    r text;
begin
    foreach r in array array['anon', 'authenticated', 'service_role', 'supabase_auth_admin',
                             'supabase_storage_admin', 'supabase_realtime_admin'] loop
        if not exists (select 1 from pg_roles where rolname = r) then
            execute format('create role %I nologin noinherit', r);
        end if;
    end loop;
    if not exists (select 1 from pg_roles where rolname = 'postgres') then
        create role postgres login createrole bypassrls inherit;
    end if;
    if not exists (select 1 from pg_roles where rolname = 'authenticator') then
        create role authenticator login noinherit;
    end if;
end
$$;

alter role anon inherit;
alter role authenticated inherit;
alter role service_role inherit bypassrls;
grant anon, authenticated, service_role to authenticator;
grant anon, authenticated, service_role to postgres;

-- The platform lets `postgres` create schemas and extensions in the project database.
do $$ begin execute format('grant create, temporary on database %I to postgres', current_database()); end $$;

create schema if not exists extensions;
grant usage on schema extensions to postgres, anon, authenticated, service_role;
create extension if not exists dblink schema extensions;
grant execute on all functions in schema extensions to postgres;

-- ---------------------------------------------------------------------------------------------
-- auth
-- ---------------------------------------------------------------------------------------------
create schema if not exists auth authorization supabase_auth_admin;
grant usage on schema auth to anon, authenticated, service_role, postgres;

create table if not exists auth.users (
    id                 uuid primary key,
    aud                text,
    role               text,
    email              text,
    encrypted_password text,
    email_confirmed_at timestamptz,
    created_at         timestamptz not null default now(),
    updated_at         timestamptz not null default now(),
    is_anonymous       boolean not null default false
);
create table if not exists auth.sessions (
    id         uuid primary key,
    user_id    uuid not null references auth.users (id) on delete cascade,
    created_at timestamptz not null default now(),
    updated_at timestamptz not null default now(),
    aal        text,
    not_after  timestamptz
);
alter table auth.users owner to supabase_auth_admin;
alter table auth.sessions owner to supabase_auth_admin;
grant select, insert, update, delete on auth.users, auth.sessions to postgres;

create or replace function auth.uid() returns uuid language sql stable as $$
  select
  coalesce(
    nullif(current_setting('request.jwt.claim.sub', true), ''),
    (nullif(current_setting('request.jwt.claims', true), '')::jsonb ->> 'sub')
  )::uuid
$$;
create or replace function auth.role() returns text language sql stable as $$
  select
  coalesce(
    nullif(current_setting('request.jwt.claim.role', true), ''),
    (nullif(current_setting('request.jwt.claims', true), '')::jsonb ->> 'role')
  )::text
$$;
create or replace function auth.jwt() returns jsonb language sql stable as $$
  select
    coalesce(
        nullif(current_setting('request.jwt.claim', true), ''),
        nullif(current_setting('request.jwt.claims', true), '')
    )::jsonb
$$;
alter function auth.uid() owner to supabase_auth_admin;
alter function auth.role() owner to supabase_auth_admin;
alter function auth.jwt() owner to supabase_auth_admin;
grant execute on function auth.uid(), auth.role(), auth.jwt() to anon, authenticated, service_role, postgres;

-- ---------------------------------------------------------------------------------------------
-- storage (the tables the storage service queries with the caller's claims)
-- ---------------------------------------------------------------------------------------------
create schema if not exists storage;
grant usage on schema storage to anon, authenticated, service_role, postgres;
create table if not exists storage.buckets (
    id                 text primary key,
    name               text not null unique,
    owner              uuid,
    public             boolean default false,
    file_size_limit    bigint,
    allowed_mime_types text[],
    created_at         timestamptz default now(),
    updated_at         timestamptz default now()
);
create table if not exists storage.objects (
    id         uuid primary key default gen_random_uuid(),
    bucket_id  text references storage.buckets (id),
    name       text,
    owner      uuid,
    created_at timestamptz default now(),
    updated_at timestamptz default now(),
    metadata   jsonb,
    unique (bucket_id, name)
);
alter table storage.buckets enable row level security;
alter table storage.objects enable row level security;
-- On the platform the storage service's admin role owns these tables and `postgres` may still
-- create policies on them; here `postgres` owns them so that it can.
alter table storage.buckets owner to postgres;
alter table storage.objects owner to postgres;
grant select, insert, update, delete on storage.buckets, storage.objects to anon, authenticated, service_role;

-- ---------------------------------------------------------------------------------------------
-- realtime (the message table that authorises private broadcast channels)
-- ---------------------------------------------------------------------------------------------
create schema if not exists realtime;
grant usage on schema realtime to anon, authenticated, service_role, postgres;
create table if not exists realtime.messages (
    id          uuid not null default gen_random_uuid(),
    topic       text not null,
    extension   text not null,
    payload     jsonb,
    event       text,
    private     boolean default false,
    updated_at  timestamp not null default now(),
    inserted_at timestamp not null default now(),
    primary key (id, inserted_at)
);
alter table realtime.messages enable row level security;
alter table realtime.messages owner to postgres;
grant select, insert, update on realtime.messages to anon, authenticated, service_role;

create or replace function realtime.topic() returns text language sql stable as $$
select nullif(current_setting('realtime.topic', true), '')::text;
$$;
create or replace function realtime.send(payload jsonb, event text, topic text, private boolean default true)
returns void language plpgsql as $$
declare
  generated_id uuid;
  final_payload jsonb;
begin
  begin
    generated_id := gen_random_uuid();
    if payload ? 'id' then
      final_payload := payload;
    else
      final_payload := jsonb_set(payload, '{id}', to_jsonb(generated_id));
    end if;
    execute format('set local realtime.topic to %L', topic);
    insert into realtime.messages (id, payload, event, topic, private, extension)
    values (generated_id, final_payload, event, topic, private, 'broadcast');
  exception
    when others then
      raise warning 'WarnSendingBroadcastMessage: %', sqlerrm;
  end;
end;
$$;
grant execute on function realtime.topic() to anon, authenticated, service_role, postgres;
grant execute on function realtime.send(jsonb, text, text, boolean) to postgres;
