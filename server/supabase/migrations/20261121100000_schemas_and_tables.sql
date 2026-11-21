-- Iamb server, migration 1: schemas, tables, constraints, indexes, the revision counter, and
-- row-level security switched on for every table (docs/sync-design.md §5).
--
-- Schemas (§5.1): `iamb` holds the tables and is not exposed through the REST interface; `api`
-- holds the functions devices call and is the only schema exposed to them; `api_service` holds
-- the functions the edge functions call with the service key; `iamb_private` holds helpers and
-- is not exposed. Times are bigint UTC milliseconds, ids uuid, enumerations text with a CHECK.

do $$
begin
    -- The functions that write are `security definer` and must be owned by the tables' owner,
    -- which on Supabase is `postgres` (not a superuser; it bypasses row-level security).
    if current_user <> 'postgres' then
        raise exception 'iamb migrations must run as postgres, not %', current_user;
    end if;
end
$$;

create schema if not exists iamb;
create schema if not exists iamb_private;
create schema if not exists api;
create schema if not exists api_service;

-- Nothing of Iamb is reachable by default: no schema usage, no function execution for PUBLIC.
revoke all on schema iamb, iamb_private, api, api_service from public;
alter default privileges in schema iamb revoke all on tables from public;
alter default privileges in schema iamb revoke all on sequences from public;
alter default privileges in schema iamb revoke execute on functions from public;
alter default privileges in schema iamb_private revoke execute on functions from public;
alter default privileges in schema api revoke execute on functions from public;
alter default privileges in schema api_service revoke execute on functions from public;

-- `authenticated` reads tables through their policies and runs the functions of `api` and the
-- gate helpers; `service_role` runs the functions of `api_service`; `anon` gets nothing.
grant usage on schema iamb to authenticated;
grant usage on schema iamb_private to authenticated;
grant usage on schema api to authenticated;
grant usage on schema api_service to service_role;

-- ---------------------------------------------------------------------------------------------
-- Server state, epochs, rate limits (§5.11)
-- ---------------------------------------------------------------------------------------------

-- One value for the whole server, never cached (§5.1, C18): a value fetched later is higher,
-- whichever connection fetches it.
create sequence if not exists iamb.rev_seq as bigint minvalue 1 start with 1 cache 1 no cycle;

create table if not exists iamb.server_state (
    id                   boolean primary key default true check (id),
    epoch                uuid    not null,
    protocol_min         integer not null,
    protocol_max         integer not null,
    protocol_oldest      integer not null,
    retention_days       integer not null default 30,
    history_days         integer not null default 90,
    cascade_max          integer not null default 2000,
    attachment_bytes_max bigint  not null default 26214400,
    push_ops_max         integer not null default 200,
    push_bytes_max       integer not null default 4194304,
    pull_entries_max     integer not null default 500,
    -- The ways a session may have been opened for Iamb to accept it, as the token's `amr`
    -- claim names them; never `password` (C25).
    sign_in_methods      text[]  not null,
    -- Recommended values of §6.8: calls per minute per device, ops per minute, account calls
    -- per minute per account, invitations per hour per group and per day per account.
    rate_limits          jsonb   not null,
    maintenance          boolean not null default false,
    check (protocol_oldest <= protocol_min and protocol_min <= protocol_max),
    check (not ('password' = any (sign_in_methods)))
);

create table if not exists iamb.epoch (
    id         uuid   primary key,
    started_at bigint not null,
    first_rev  bigint not null
);

create table if not exists iamb.rate_bucket (
    subject      uuid    not null,
    bucket       text    not null,
    window_start bigint  not null,
    count        integer not null,
    primary key (subject, bucket)
);

-- ---------------------------------------------------------------------------------------------
-- Synced tables (§5.2). Common columns: id, created_at, updated_at, updated_by, rev,
-- created_rev, deleted_at, updated_device, updated_op, lost_rev.
-- ---------------------------------------------------------------------------------------------

create table if not exists iamb.profile (
    id             uuid    primary key,
    created_at     bigint  not null,
    updated_at     bigint  not null,
    updated_by     uuid,
    rev            bigint  not null,
    created_rev    bigint  not null,
    deleted_at     bigint,
    updated_device uuid,
    updated_op     uuid,
    lost_rev       bigint,
    kind           text    not null check (kind in ('personal', 'family', 'company')),
    name           text    not null,
    owner_user_id  uuid    not null,
    offered_owner  uuid,
    offered_at     bigint,
    -- The last revision a transaction holding this row's lock drew (C18, C24); never sent.
    head_rev       bigint  not null default 0
);
create unique index if not exists profile_one_personal on iamb.profile (owner_user_id) where kind = 'personal';
create index if not exists profile_owner on iamb.profile (owner_user_id);

create table if not exists iamb.membership (
    id             uuid    primary key,
    created_at     bigint  not null,
    updated_at     bigint  not null,
    updated_by     uuid,
    rev            bigint  not null,
    created_rev    bigint  not null,
    deleted_at     bigint,
    updated_device uuid,
    updated_op     uuid,
    lost_rev       bigint,
    profile_id     uuid    not null references iamb.profile (id),
    user_id        uuid    not null,
    role           text    not null check (role in ('owner', 'admin', 'member'))
);
create unique index if not exists membership_live on iamb.membership (profile_id, user_id) where deleted_at is null;
create unique index if not exists membership_one_owner on iamb.membership (profile_id) where role = 'owner' and deleted_at is null;
create index if not exists membership_user on iamb.membership (user_id);
create index if not exists membership_profile on iamb.membership (profile_id);

create table if not exists iamb.folder (
    id                  uuid    primary key,
    created_at          bigint  not null,
    updated_at          bigint  not null,
    updated_by          uuid,
    rev                 bigint  not null,
    created_rev         bigint  not null,
    deleted_at          bigint,
    updated_device      uuid,
    updated_op          uuid,
    lost_rev            bigint,
    profile_id          uuid    not null references iamb.profile (id),
    parent_id           uuid    references iamb.folder (id),
    name                text    not null,
    position            bigint  not null,
    member_permission   text    check (member_permission in ('view', 'edit')),
    default_trigger     text    check (default_trigger in ('immediate', 'delimiter')),
    default_case_mode   text    check (default_case_mode in ('exact', 'insensitive', 'adaptive')),
    abbreviation_prefix text,
    check (parent_id is null or parent_id <> id)
);
create index if not exists folder_profile on iamb.folder (profile_id);
create index if not exists folder_parent on iamb.folder (parent_id);

create table if not exists iamb.folder_share (
    id             uuid    primary key,
    created_at     bigint  not null,
    updated_at     bigint  not null,
    updated_by     uuid,
    rev            bigint  not null,
    created_rev    bigint  not null,
    deleted_at     bigint,
    updated_device uuid,
    updated_op     uuid,
    lost_rev       bigint,
    folder_id      uuid    not null references iamb.folder (id),
    profile_id     uuid    not null references iamb.profile (id),
    permission     text    not null check (permission in ('view', 'edit')),
    unique (folder_id, profile_id)
);
create index if not exists folder_share_profile on iamb.folder_share (profile_id);

create table if not exists iamb.snippet (
    id             uuid    primary key,
    created_at     bigint  not null,
    updated_at     bigint  not null,
    updated_by     uuid,
    rev            bigint  not null,
    created_rev    bigint  not null,
    deleted_at     bigint,
    updated_device uuid,
    updated_op     uuid,
    lost_rev       bigint,
    folder_id      uuid    not null references iamb.folder (id),
    profile_id     uuid    not null references iamb.profile (id),
    abbreviation   text    not null,
    label          text    not null,
    kind           text    not null check (kind in ('plain', 'rich')),
    body           text    not null,
    trigger_mode   text    not null check (trigger_mode in ('immediate', 'delimiter')),
    case_mode      text    not null check (case_mode in ('exact', 'insensitive', 'adaptive')),
    word_start     boolean not null,
    enabled        boolean not null,
    content_by     uuid,
    content_at     bigint
);
create index if not exists snippet_folder on iamb.snippet (folder_id);
create index if not exists snippet_profile on iamb.snippet (profile_id);

create table if not exists iamb.tag (
    id             uuid    primary key,
    created_at     bigint  not null,
    updated_at     bigint  not null,
    updated_by     uuid,
    rev            bigint  not null,
    created_rev    bigint  not null,
    deleted_at     bigint,
    updated_device uuid,
    updated_op     uuid,
    lost_rev       bigint,
    profile_id     uuid    not null references iamb.profile (id),
    name           text    not null,
    color          text
);
create index if not exists tag_profile on iamb.tag (profile_id);

create table if not exists iamb.snippet_tag (
    id             uuid    primary key,
    created_at     bigint  not null,
    updated_at     bigint  not null,
    updated_by     uuid,
    rev            bigint  not null,
    created_rev    bigint  not null,
    deleted_at     bigint,
    updated_device uuid,
    updated_op     uuid,
    lost_rev       bigint,
    snippet_id     uuid    not null references iamb.snippet (id) on delete cascade,
    tag_id         uuid    not null references iamb.tag (id) on delete cascade
);
create index if not exists snippet_tag_snippet on iamb.snippet_tag (snippet_id);
create index if not exists snippet_tag_tag on iamb.snippet_tag (tag_id);

create table if not exists iamb.attachment (
    id             uuid    primary key,
    created_at     bigint  not null,
    updated_at     bigint  not null,
    updated_by     uuid,
    rev            bigint  not null,
    created_rev    bigint  not null,
    deleted_at     bigint,
    updated_device uuid,
    updated_op     uuid,
    lost_rev       bigint,
    profile_id     uuid    not null references iamb.profile (id),
    sha256         text    not null check (sha256 ~ '^[0-9a-f]{64}$'),
    mime           text    not null,
    size           bigint  not null check (size >= 0),
    file_name      text    not null,
    unlinked_at    bigint,
    backed         boolean not null default false
);
create index if not exists attachment_profile_hash on iamb.attachment (profile_id, sha256);

create table if not exists iamb.snippet_attachment (
    id             uuid    primary key,
    created_at     bigint  not null,
    updated_at     bigint  not null,
    updated_by     uuid,
    rev            bigint  not null,
    created_rev    bigint  not null,
    deleted_at     bigint,
    updated_device uuid,
    updated_op     uuid,
    lost_rev       bigint,
    snippet_id     uuid    not null references iamb.snippet (id) on delete cascade,
    attachment_id  uuid    not null references iamb.attachment (id) on delete cascade
);
create index if not exists snippet_attachment_snippet on iamb.snippet_attachment (snippet_id);
create index if not exists snippet_attachment_attachment on iamb.snippet_attachment (attachment_id);

-- ---------------------------------------------------------------------------------------------
-- Accounts and devices (§5.3)
-- ---------------------------------------------------------------------------------------------

create table if not exists iamb.user_profile (
    user_id      uuid   primary key,
    display_name text   not null default '',
    created_at   bigint not null,
    updated_at   bigint not null
);

create table if not exists iamb.device (
    id                    uuid    primary key,
    user_id               uuid    not null,
    session_id            uuid    not null unique,
    name                  text    not null,
    platform              text    not null,
    app_version           text    not null,
    protocol              integer not null,
    created_at            bigint  not null,
    last_seen_at          bigint,
    revoked_at            bigint,
    position_token        text,
    position_token_before text
);
create index if not exists device_user on iamb.device (user_id);

-- ---------------------------------------------------------------------------------------------
-- Access entries (§5.4). `profile_id` is the folder's profile, copied here so that the policies
-- that ask "edit on a folder of this profile" need no join (an addition to §5.4's columns).
-- ---------------------------------------------------------------------------------------------

create table if not exists iamb.folder_access (
    user_id        uuid   not null,
    folder_id      uuid   not null references iamb.folder (id) on delete cascade,
    profile_id     uuid   not null references iamb.profile (id),
    permission     text   not null check (permission in ('view', 'edit', 'manage')),
    via            text   not null check (via in ('own', 'group', 'share')),
    granted_at     bigint not null,
    root_folder_id uuid   not null,
    primary key (user_id, folder_id)
);
create index if not exists folder_access_folder on iamb.folder_access (folder_id);
create index if not exists folder_access_profile on iamb.folder_access (profile_id, user_id);

-- ---------------------------------------------------------------------------------------------
-- The change feed (§5.5)
-- ---------------------------------------------------------------------------------------------

create table if not exists iamb.feed (
    user_id    uuid   not null,
    seq        bigint not null,
    kind       text   not null check (kind in ('row', 'folder_visible', 'folder_revoked', 'conflict',
                                               'invitations', 'entitlements', 'blob', 'account')),
    entity     text   check (entity in ('profile', 'membership', 'folder', 'folder_share', 'snippet',
                                        'tag', 'snippet_tag', 'attachment', 'snippet_attachment')),
    row_id     uuid,
    folder_id  uuid,
    profile_id uuid,
    sha256     text,
    detail     jsonb,
    created_at bigint not null,
    primary key (user_id, seq)
);
-- One entry per row (§5.5): a new `row` entry for the same user and row replaces the older one.
create unique index if not exists feed_one_per_row on iamb.feed (user_id, entity, row_id) where kind = 'row';
create index if not exists feed_created on iamb.feed (created_at);

create table if not exists iamb.feed_head (
    user_id uuid   primary key,
    head    bigint not null default 0,
    floor   bigint not null default 0
);

-- ---------------------------------------------------------------------------------------------
-- History, conflicts, receipts (§5.6, §5.7)
-- ---------------------------------------------------------------------------------------------

create table if not exists iamb.history (
    entity          text    not null,
    row_id          uuid    not null,
    rev             bigint  not null,
    row             jsonb   not null,
    deleted         boolean not null,
    written_at      bigint  not null,
    written_by      uuid,
    written_device  uuid,
    replaced_at     bigint  not null,
    replaced_by_rev bigint,
    primary key (entity, row_id, rev)
);
create index if not exists history_replaced on iamb.history (replaced_at);

-- `parties` (the winner's author and the author of every offered version) is kept on the row so
-- that the read policies of `conflict` and `conflict_version` need no helper (an addition to the
-- columns of §5.6).
create table if not exists iamb.conflict (
    id            uuid    primary key,
    entity        text    not null,
    row_id        uuid    not null,
    profile_id    uuid    not null references iamb.profile (id) on delete cascade,
    kind          text    not null check (kind in ('edit_edit', 'edit_over_delete', 'delete_over_edit',
                                                   'deleted_with_folder', 'folder_restored')),
    winner_rev    bigint  not null,
    winner_user   uuid,
    winner_device uuid,
    created_at    bigint  not null,
    state         text    not null check (state in ('open', 'resolved', 'replaced', 'expired')),
    resolved_at   bigint,
    resolved_by   uuid,
    resolution    text,
    parties       uuid[]  not null
);
create unique index if not exists conflict_one_open on iamb.conflict (entity, row_id) where state = 'open';
create index if not exists conflict_row on iamb.conflict (entity, row_id);
create index if not exists conflict_profile on iamb.conflict (profile_id);
create index if not exists conflict_parties on iamb.conflict using gin (parties);

create table if not exists iamb.conflict_version (
    conflict_id uuid    not null references iamb.conflict (id) on delete cascade,
    rev         bigint  not null,
    user_id     uuid,
    device_id   uuid,
    taken_over  boolean not null default false,
    primary key (conflict_id, rev)
);

create table if not exists iamb.push_receipt (
    device_id   uuid   not null references iamb.device (id) on delete cascade,
    op_id       uuid   not null,
    entity      text   not null,
    row_id      uuid   not null,
    status      text   not null,
    reason      text,
    rev         bigint,
    conflict_id uuid,
    adjusted    text[] not null default '{}',
    created_at  bigint not null,
    primary key (device_id, op_id)
);
create index if not exists push_receipt_created on iamb.push_receipt (created_at);

-- ---------------------------------------------------------------------------------------------
-- Invitations (§5.8)
-- ---------------------------------------------------------------------------------------------

create table if not exists iamb.invitation (
    id          uuid   primary key,
    profile_id  uuid   not null references iamb.profile (id),
    email       text   not null check (email = lower(email)),
    role        text   not null check (role in ('admin', 'member')),
    invited_by  uuid   not null,
    created_at  bigint not null,
    expires_at  bigint not null,
    status      text   not null check (status in ('pending', 'accepted', 'declined', 'withdrawn', 'expired')),
    answered_at bigint,
    accepted_by uuid
);
create unique index if not exists invitation_one_pending on iamb.invitation (profile_id, email) where status = 'pending';
create index if not exists invitation_email on iamb.invitation (email) where status = 'pending';
create index if not exists invitation_profile on iamb.invitation (profile_id);

-- ---------------------------------------------------------------------------------------------
-- Attachment files (§5.9)
-- ---------------------------------------------------------------------------------------------

create table if not exists iamb.attachment_blob (
    profile_id  uuid   not null references iamb.profile (id),
    sha256      text   not null check (sha256 ~ '^[0-9a-f]{64}$'),
    size        bigint not null,
    mime        text   not null,
    verified_at bigint not null,
    primary key (profile_id, sha256)
);

create table if not exists iamb.attachment_upload (
    id          uuid   primary key,
    profile_id  uuid   not null references iamb.profile (id),
    sha256      text   not null check (sha256 ~ '^[0-9a-f]{64}$'),
    size        bigint not null,
    mime        text   not null,
    uploaded_by uuid   not null,
    created_at  bigint not null,
    expires_at  bigint not null
);
create index if not exists attachment_upload_profile on iamb.attachment_upload (profile_id);
create index if not exists attachment_upload_uploader on iamb.attachment_upload (uploaded_by);

-- ---------------------------------------------------------------------------------------------
-- Plans, entitlements and flags (§5.10)
-- ---------------------------------------------------------------------------------------------

create table if not exists iamb.plan (
    id         text    primary key,
    scope      text    not null check (scope in ('user', 'group')),
    name       text    not null,
    limits     jsonb   not null default '{}',
    features   jsonb   not null default '{}',
    is_default boolean not null default false,
    active     boolean not null default true
);
create unique index if not exists plan_one_default on iamb.plan (scope) where is_default;

create table if not exists iamb.subscription (
    id           uuid   primary key,
    user_id      uuid,
    profile_id   uuid   references iamb.profile (id),
    plan_id      text   not null references iamb.plan (id),
    status       text   not null check (status in ('active', 'past_due', 'cancelled')),
    started_at   bigint not null,
    ends_at      bigint,
    source       text   not null default 'manual',
    external_ref text,
    check ((user_id is null) <> (profile_id is null))
);
create unique index if not exists subscription_user_active on iamb.subscription (user_id) where status = 'active' and user_id is not null;
create unique index if not exists subscription_profile_active on iamb.subscription (profile_id) where status = 'active' and profile_id is not null;
create index if not exists subscription_plan on iamb.subscription (plan_id);
create index if not exists subscription_profile on iamb.subscription (profile_id);

create table if not exists iamb.feature_flag (
    key   text  primary key,
    value jsonb not null,
    note  text
);

create table if not exists iamb.feature_override (
    key     text  not null references iamb.feature_flag (key) on delete cascade,
    user_id uuid  not null,
    value   jsonb not null,
    primary key (key, user_id)
);
create index if not exists feature_override_user on iamb.feature_override (user_id);

create table if not exists iamb.epoch_profile (
    epoch_id   uuid   not null references iamb.epoch (id) on delete cascade,
    profile_id uuid   not null references iamb.profile (id) on delete cascade,
    backup_rev bigint not null,
    primary key (epoch_id, profile_id)
);
create index if not exists epoch_profile_profile on iamb.epoch_profile (profile_id);

-- ---------------------------------------------------------------------------------------------
-- Row-level security on every table of `iamb` (§5.1). The policies are in migration 3; there is
-- no insert, update or delete policy and no such grant anywhere.
-- ---------------------------------------------------------------------------------------------

do $$
declare
    t record;
begin
    for t in select c.relname from pg_class c join pg_namespace n on n.oid = c.relnamespace
             where n.nspname = 'iamb' and c.relkind in ('r', 'p') loop
        execute format('alter table iamb.%I enable row level security', t.relname);
        execute format('revoke all on iamb.%I from public, anon, authenticated, service_role', t.relname);
    end loop;
end
$$;
revoke all on sequence iamb.rev_seq from public, anon, authenticated, service_role;

-- Read grants for the tables that have read policies (§5.13). Every other table has none.
grant select on iamb.profile, iamb.membership, iamb.folder, iamb.folder_share, iamb.snippet,
                iamb.tag, iamb.snippet_tag, iamb.attachment, iamb.snippet_attachment,
                iamb.user_profile, iamb.device, iamb.folder_access, iamb.feed, iamb.feed_head,
                iamb.conflict, iamb.conflict_version, iamb.invitation, iamb.attachment_blob,
                iamb.attachment_upload
    to authenticated;
