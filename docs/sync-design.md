# Iamb sync design

Status: proposed for the owner's approval · Task 4.1 · Builds on
[`architecture.md`](architecture.md) §6, §7, §8 and §9 and on ADR-0014, ADR-0016, ADR-0020 and
ADR-0022 · Decisions recorded in [ADR-0027](decisions/0027-sync-protocol-and-conflict-rules.md),
[ADR-0028](decisions/0028-data-model-for-sync.md) and
[ADR-0029](decisions/0029-server-readable-content.md) · Scenarios:
[`spec/sync/scenarios.toml`](../spec/sync/scenarios.toml), checked by
`cargo test -p iamb-testkit --test sync_scenarios`

The architecture fixes the frame of sync: a Postgres database with row-level security on Supabase,
one change feed per user, last write wins per row with history, an outbox on the device. This
document fills the frame in so that the tasks that build on it have nothing left to design: the
server schema and its policies (4.2), their security review (4.3), the sync client and its
two-device simulation (4.4), the account screens (4.5), media sync (4.6) and plans (4.7).
§14 says what each of them takes from here. What needs the owner's decision is in §13 and is not
decided silently; every such point has a recommendation, and the design is written for the
recommendation.

How to read it: §3 to §5 say what exists (identities, permissions, tables); §6 and §7 how devices
and the server talk and what a device stores; §8 and §9 are the rules, numbered `C1` to `C23` for
the server and `D1` to `D14` for the device, and they are what the scenarios of §12 cite; §10
answers the conflict questions one by one; §11 is privacy and security.

## 1. What the design must achieve

| Goal | Target | Where |
| --- | --- | --- |
| Two online devices of one account show the same library | within 5 seconds of a change (p95) | §6.6, §6.7 |
| A newly shared folder appears for the people it is shared with | within 10 seconds while they are online | §6.4, D6 |
| No edit is lost | 0 in 1,000 simulated runs with random edits, offline periods and clock skew | below, §8, §9 |
| A group gets exactly the folder that was shared and nothing else | every query as a member returns only that folder's content | §4.4, §4.5, §11.5 |
| Content whose access ended disappears from devices | at the next sync; at the latest at the daily access check | D6, D7 |
| Editing works offline | every local change is queued in the transaction that makes it | §7, D1 |
| The service stays without network code | only `iamb-sync` talks to the server; it never sees keys | §11.2 |
| Typed text, form values and usage never reach the server | not part of any synced row | §11.2 |

**What "no lost edit" means.** A local change is a save, a delete, a move or a new row on some
device. When every device has been online long enough to finish syncing, each such change is in
exactly one of these places:

1. it is the row's current version on the server;
2. it is in the server's history because a later write replaced it knowingly: that write was based
   on it or on a later version;
3. it is in the server's history because a later write replaced it without having seen it, and a
   conflict record that names both versions is shown to the people concerned, who can restore it
   (§8, C3);
4. the server refused it, and the device that made it keeps its content in the list of changes
   that were not synced, where the user can copy it into a snippet of their own (§9, D5);
5. the server cannot store it yet (too large, over a plan's limit): the row stays on the device
   that made it, works there, and is marked as not synced (D5);
6. it was a position in a list (the order of folders), which is arrangement and not content: the
   last write wins without a record (C3).

Never: nowhere. The simulation of task 4.4 checks this together with the other invariants:

| | Invariant |
| --- | --- |
| I1 | **Convergence.** When all devices are online and idle, every device of a user holds exactly the rows the server shows that user, in the same revisions, except rows blocked on that device (place 5) |
| I2 | **No lost edit**, as defined above |
| I3 | **Revisions only grow.** No device replaces a row by an older revision of it |
| I4 | **No access leak.** After a device has synced past the end of an access, it holds none of the rows that access gave it |
| I5 | **Exactly once.** A change that is sent several times makes one revision |
| I6 | **Same answer everywhere.** Which snippet holds an abbreviation, and what a user may do in a folder, is the same on all devices of that user |

## 2. The main choices

| Question | Choice | Why |
| --- | --- | --- |
| Which write is "last"? | The one the server accepts last. The server gives every accepted write a revision from one growing counter and its own time. Device clocks decide nothing (C1) | ADR-0016 rejects client clocks as the authority; skewed or wrong clocks then cannot make devices disagree or let an old edit win by its date |
| Row or field? | Row. A write carries the whole row and replaces it (C2). Fields are not merged | A snippet's fields are one behaviour (abbreviation, trigger and content together), a merged row would be a revision nobody wrote and nobody approved (ADR-0022), and ADR-0016 already says so. Conflicts are rare for one person's snippets and every one is visible and reversible |
| What about the version that lost? | It goes to a history table on the server. If the winner had not seen it, a conflict is recorded and shown to both authors, who can restore it (C3, C20) | "A stale offline edit still wins, but visibly and reversibly" (ADR-0016) |
| Deletes? | A delete is a write like any other: whichever of an edit and a delete arrives last wins, and the other is offered back for 30 days (C5). A folder's delete takes the whole subtree as it is on the server (C6); a later write into a deleted folder brings the folder back (C7) | One rule in both orders is easier to hold in the head and to test than "edits always beat deletes"; nothing disappears without a record |
| Does the server enforce unique abbreviations? | No. The server never reads `abbreviation`, `label`, `body` or names (C9). Devices refuse duplicates in their own edits, and what sync brings together is ordered by a fixed precedence | Uniqueness is per user's view (personal plus shared folders), which no server constraint can express; refusing a row would leave an edit that exists on one device only; and a server that does not interpret content leaves room for end-to-end encryption later (§13) |
| How does a device send changes? | One request to a database function with up to 200 changed rows, each with an id that makes a repeat harmless (C15) and the revision it was based on. Each row gets its own answer (§6.3) | Partial failures stay partial; retries after a lost answer cannot write twice |
| How does a device learn of changes? | It reads its user's feed from its position, in pages. A live channel only says "there is something new"; it carries no content (§6.4, §6.6) | One code path applies changes whether they come after a poke, a poll or a month offline; a dropped channel costs seconds, never correctness |
| Who may write tables? | Nobody directly. Every table has row-level security with read rules only; every write goes through a function that checks the rules of §4 against the current state (§5.13) | The write rules need the previous version, the history, the feed and the access entries in one transaction; policies alone cannot do that, and the review of task 4.3 can test each function as each kind of user |
| How is "may this user read this folder" answered fast? | A table of access entries, one per user and folder, kept in step with shares and memberships by the functions that change them. Policies look it up; `folder_visible` and `folder_revoked` are exactly its additions and removals (§5.4, C21) | Sharing and unsharing reach devices from the same fact the policies use, so they cannot disagree |
| Sharing changes that race? | A share has one row per folder and group, with an id derived from the two. A stale change to it never wins; a delete always applies (C11) | Access must not widen because a device was late; "stop sharing" must stick |
| Who wrote a snippet's content? | The server records it (`content_by`), separately from who last touched the row. A device holds a snippet for review by that author, and remembers what it approved as content, not as a revision number (C17, D10) | Otherwise the owner's own label change would make a member's key presses count as the owner's (scenario S40) |
| Is snippet content hidden from the server? | Not in version 1: it is protected in transit and at rest, and readable by the server (§11.1) | End-to-end encryption needs key distribution, recovery without a password, and a member online to hand a key to a new member, against the 10-second target. It is the first open question (§13) |
| What does signing in do to the library on a device? | It adopts it into the account and syncs all of it (D9) | The gate is two devices of one account in sync. A choice of what stays local is a later option; the sign-in dialog must say what happens (§13) |

## 3. Identities

### 3.1 Accounts

An account is one person: a row in the authentication service's user table with a verified email
address. Sign-in is the browser flow of ADR-0020 (authorisation code with PKCE; magic link or an
identity provider); tokens live in the operating system's keychain and only `iamb-sync` reads
them. The server keeps a `user_profile` row per account with the display name people in the same
group see ("Ines Marchetti"). The email address is shown to the owner and admins of a group the
person belongs to or is invited to, and to nobody else.

### 3.2 Devices

A device is one installation of Iamb signed in to an account. It makes its own id (UUID version 7)
the first time it signs in and keeps it in `sync_state`. It registers with `hello` (§6.2), which
binds the device row to the authentication session of the token used. The device row holds a name
the user can change (default "Mac", "Windows PC"; the host name is not sent), the platform, the
app version and the protocol version. Devices exist for four reasons: the receipts that make a
repeated push harmless are kept per device (C15); a conflict can say on which device the other
version was written; the user can see the list and revoke one (§11.3); and a write is never in
conflict with the same device's own earlier write (C3).

### 3.3 Profiles, memberships, roles

As in architecture §7 and ADR-0014:

- Every account has exactly one **personal** profile. The server creates it with the account. Its
  name is always `Personal`; people see the owner's display name instead.
- A **group** is a profile of kind `family` or `company` (the interface says "family" and "team").
  The rules are the same for both. A group has members through `membership` rows with the role
  `owner`, `admin` or `member`. It has exactly one owner at every moment, and the profile's
  `owner_user_id` names the same person.
- There are no groups inside groups and no sharing with single addresses or by link.

A folder belongs to exactly one profile and never changes it. Moving content from a personal
profile to a group, or between groups, is a copy made by the device (new rows) followed by a
delete; the server refuses a row whose profile changes (C8).

### 3.4 Ids

| Row | Id | Made by |
| --- | --- | --- |
| Folder, snippet, tag, attachment, the two kinds of link | UUID version 7 | The device that creates the row |
| Folder share | Derived: the first 16 bytes of SHA-256 over `iamb:folder_share:` followed by the 16 bytes of the folder id and the 16 bytes of the group's profile id, with the version set to 8 and the variant to `10` | Any device; the same pair gives the same id everywhere (C11) |
| Group profile, invitation | UUID version 7 | The device that makes the call, so that a repeated call is harmless |
| Personal profile, membership, conflict | UUID version 7 | The server |
| Device | UUID version 7 | The device |
| Change sent in a push (`op_id`) | UUID | The store, when it writes the outbox entry |

## 4. Who may do what

### 4.1 Roles in a group

| Call | Owner | Admin | Member |
| --- | --- | --- | --- |
| See the group, its members' display names and roles | yes | yes | yes |
| See members' and invited people's email addresses | yes | yes | no |
| Invite (as member or admin), withdraw an invitation | yes | yes | no |
| Remove a member or an admin; change a role between member and admin | yes | yes | no |
| Remove the owner, or change the owner's role | no | no | no |
| Rename the group | yes | yes | no |
| Transfer ownership to a member or admin (the old owner becomes admin) | yes | no | no |
| Delete the group | yes | no | no |
| Leave the group | no (transfer first) | yes | yes |
| Share one of their own folders with the group | yes | yes | yes |

These calls need the server and are not queued offline: an invitation sends an email, a membership
is what gives access, and both are subject to the plan (§5.10). The device creates none of these
rows itself; it receives them through the feed (C16).

### 4.2 Permission on a folder

For a user and a folder the server derives one permission, the highest that any rule gives:

| Permission | Who has it | It allows |
| --- | --- | --- |
| `manage` | The owner of the personal profile the folder belongs to. For a folder that a group owns: the group's owner and admins | Everything below, and: rename, move, delete, restore and (personal folders) share or unshare the folder itself; set `member_permission` (group folders); create top-level folders in the profile |
| `edit` | Members of a group that a share of the folder, or of a folder above it, names with `edit`. For a group's folder: members, when the `member_permission` of the top-level folder of its tree is `edit` | Create, edit, delete and restore snippets and sub-folders inside; move them within the same shared tree; put tags of the folder's profile on snippets and create new tags there (renaming and deleting a tag stays with `manage`, because the owner's other snippets may carry it); add attachments |
| `view` | Members of a group that a share names with `view`; members of the owning group otherwise | Read |
| none | Everyone else | Nothing; the rows do not exist for them |

- A share covers the folder and everything below it, now and later.
- The **access root** of a user for a folder is the highest folder above it (or the folder itself)
  that the user can read: the shared folder, or the top-level folder of a group's tree. A user
  without `manage` can move snippets and sub-folders only within one access root; out of it they
  can only copy (architecture §7).
- `member_permission` is read from the top-level folder of a group's tree and applies to the
  whole tree; on other folders it is ignored. Absent means `view`.
- The device does not compute permissions. The server sends each user their access roots with the
  permission and the time access began (§6.5, `access`), and the store keeps them in
  `folder_access` (§7.1), so the interface and the server cannot disagree.

### 4.3 Per row

What a push may do, checked against the state on the server when it arrives (C12). "Inside" means
in a folder the permission applies to.

| Row | Create | Change | Delete or restore |
| --- | --- | --- | --- |
| Folder at the top of a profile | `manage` in that profile | `manage` | `manage` |
| Sub-folder | `edit` on the parent | Name, defaults, position, moving within the access root: `edit` on it and on the new parent. Anything else: `manage` | `edit` on its parent |
| Snippet | `edit` on the folder | `edit` on the folder it is in and on the folder it moves to; both in one access root unless `manage` | `edit` on the folder |
| Tag (belongs to a profile) | `edit` on at least one folder of the profile | `manage` in the profile | `manage` in the profile |
| Snippet–tag link | `edit` on the snippet's folder; the tag is in the folder's profile | — (a link has no fields) | `edit` on the snippet's folder |
| Attachment (belongs to a profile) | `edit` on at least one folder of the profile | — (never changes) | `manage` in the profile; the server removes attachments nothing uses (C19) |
| Snippet–attachment link | `edit` on the snippet's folder; the attachment is in the folder's profile | — | `edit` on the snippet's folder |
| Folder share | `manage` on the folder, the folder is in a personal profile, the writer is a member of the group | The same | `manage` on the folder |
| Profile, membership, invitation | Not by push (C16) | Not by push | Not by push |

### 4.4 What a share carries

Sharing the folder `F` with a group makes these rows readable for every member, and nothing else
of the owner's:

| Readable | Because |
| --- | --- |
| `F` and every folder below it | The share |
| Every snippet in those folders, with who wrote its content and when | It is in a readable folder |
| The links between those snippets and tags, and the tag rows those links name (name and colour) | A tag is visible with the snippets that carry it (architecture §7) |
| The links between those snippets and attachments, the attachment rows they name, and the stored files of exactly those attachments | A rich snippet is useless without its images |
| The share row itself, and other shares of those folders with groups the member is in | So that the device can show "Shared by Ines · you can use" |
| The owner's personal profile row (id, kind, the fixed name, the owner's id) and the owner's display name | A folder names its profile |

Not readable: the owner's other folders and their snippets; tags no visible snippet carries; links
from other snippets, even to a visible tag; attachments no visible snippet uses, and their files;
the history of any row (§5.6); the owner's other shares, devices, entitlements; and, as always,
anything that never leaves a device (§11.2). A snippet in `F` that includes a snippet outside `F`
with `{snippet}` does not resolve for the members (`E_SNIPPET_NOT_FOUND`); the share dialog should
say how many such snippets the folder has, which the service can count from the snapshot.

A folder below `F` whose parent a user cannot read is delivered with an empty parent and shown at
the top level (architecture §7). That happens only for a sub-folder shared by itself.

### 4.5 "Exactly one folder and nothing else"

In data it is one row, `folder_share(folder_id = F, profile_id = G, permission)`, and the access
entries the server derives from it: one `folder_access` row per member of `G` and folder of `F`'s
tree. In queries it means, for a member `T` of `G` who is in no other group with the owner, with
`P` the owner's personal profile (scenario S28 is this, row by row):

| Query as `T` | Returns |
| --- | --- |
| Folders of `P` | Exactly `F` and the folders below it |
| Snippets in folders of `P` | Exactly the snippets in those folders |
| Tags of `P` | Exactly the tags carried by those snippets |
| Snippet–tag links whose tag is in `P` | Exactly the links of those snippets; not the link from a private snippet to a visible tag |
| Attachments of `P`, and files under `P/` in storage | Exactly those linked from those snippets; a request for any other file answers as for a file that does not exist |
| Shares of folders of `P` | Exactly the share to `G` |
| History rows of `P`'s rows | None |
| The same queries after the share row is deleted | Nothing |
| Any insert, update or delete on any table | Refused |

Task 4.3 tests every line (§11.5).

## 5. Server data model

### 5.1 Conventions

- **Schemas.** `iamb` holds the tables and is not exposed through Supabase's REST interface.
  `api` holds the functions devices call and is the only schema exposed. `iamb_private` holds
  helper functions that no client can call. Supabase's own `auth` (users, sessions), `storage`
  (objects) and `realtime` are used as they are.
- **Roles.** `anon` has no privilege on anything of Iamb. `authenticated` may read tables through
  their policies and execute the functions of `api`. Scheduled jobs run as the tables' owner; no
  device ever holds a service key.
- **Row-level security** is enabled and forced on every table of `iamb`. Policies are read rules
  only (§5.13). There is no insert, update or delete policy and no such grant: every write goes
  through a function of `api`. Those functions are `security definer` with a fixed `search_path`,
  take the caller from the token (`auth.uid()`, the session id claim) and never from an argument,
  and check §4 themselves. Functions that only read (`pull`, `fetch_folder`, `snapshot`) are
  `security invoker`, so what they return passes through the policies: a mistake in the feed
  cannot show a row its reader may not read.
- **Types.** Ids are `uuid`. Times are `bigint` UTC milliseconds, as in the local store and the
  model's wire form, so a row is the same value on both sides. Enumerations are `text` with a
  `CHECK`, in the spelling the model serialises (`personal`, `edit`, `delimiter`).
- **Common columns** of the nine synced tables: `id uuid primary key`, `created_at bigint`,
  `updated_at bigint`, `updated_by uuid`, `rev bigint`, `deleted_at bigint` (null while live), and
  `updated_device uuid` (the device that wrote the revision; null for a write the server made;
  never sent to devices of other users).
- **Revisions.** `rev` comes from one sequence, `iamb.rev_seq`, taken after the row is locked, so
  the revisions of a row only grow, also across a delete, a purge and a restore of the row.
  `updated_at` is the server's clock at the write. `created_at` is what the creating device sent,
  but never later than the server's clock; it never changes afterwards. `updated_by` is the
  caller.
- **Content columns** (`abbreviation`, `label`, `body`, names, `file_name`) are stored and compared
  for equality, and never parsed, searched or used in a constraint (C9).

### 5.2 Synced tables

The columns beyond the common ones are those of the local store
(`crates/iamb-store/migrations/0001_initial.sql`), with these server-side additions and rules.

| Table | Columns | Server-side rules |
| --- | --- | --- |
| `profile` | `kind`, `name`, `owner_user_id` | One personal profile per account (unique on `owner_user_id` where `kind = 'personal'`), created by a trigger on the account's creation with the name `Personal`. Written only by account calls |
| `membership` | `profile_id`, `user_id`, `role` | Group profiles only. Unique live `(profile_id, user_id)`; exactly one live `owner` per profile (unique on `profile_id` where `role = 'owner'`). Written only by account calls |
| `folder` | `profile_id`, `parent_id`, `name`, `position`, `member_permission`, `default_trigger`, `default_case_mode`, `abbreviation_prefix` | `profile_id` never changes. The parent is in the same profile. At most four levels and no cycle, kept by C8 |
| `folder_share` | `folder_id`, `profile_id`, `permission` | The id is derived from `folder_id` and `profile_id` (§3.4) and checked. The folder is in a personal profile; `profile_id` is a group |
| `snippet` | `folder_id`, `abbreviation`, `label`, `kind`, `body`, `trigger_mode`, `case_mode`, `word_start`, `enabled`; added: `profile_id`, `content_by uuid`, `content_at bigint` | `profile_id` is the folder's profile, kept by the server for the policies. `content_by` and `content_at` say who last changed what the snippet does and when (C17). No unique index on the abbreviation |
| `tag` | `profile_id`, `name`, `color` | No unique index on the name |
| `snippet_tag` | `snippet_id`, `tag_id` | Snippet and tag are in the same profile. Hard-deleted with either end at the purge |
| `attachment` | `profile_id`, `sha256`, `mime`, `size`, `file_name` | Never changes after its creation. Several rows may name the same `(profile_id, sha256)` (C19) |
| `snippet_attachment` | `snippet_id`, `attachment_id` | As `snippet_tag` |

Size limits, checked by `push` (C14): `body` at most 400,000 bytes (the engine types at most
100,000 characters); `label`, a folder's or tag's `name` and `file_name` at most 1,024 bytes;
`abbreviation` at most 64 characters; `abbreviation_prefix` at most 64 characters; `color` at
most 32 bytes; `mime` at most 127 bytes.

### 5.3 Accounts and devices

| Table | Columns | Notes |
| --- | --- | --- |
| `user_profile` | `user_id` (primary key, the authentication user), `display_name`, `created_at`, `updated_at` | Created with the account; the display name starts as the part of the email address before `@` and the user can change it |
| `device` | `id` (primary key), `user_id`, `session_id`, `name`, `platform`, `app_version`, `protocol`, `created_at`, `last_seen_at`, `revoked_at` | `session_id` is the authentication session of the token that registered the device. A call is refused unless the token's session belongs to a device row that is not revoked (§11.3) |

### 5.4 Access entries

`folder_access (user_id, folder_id, permission, via, granted_at, root_folder_id)`, primary key
`(user_id, folder_id)`.

One row for every user and folder such that the user may read the folder under §4.2:
`permission` is the highest permission, `via` is `own`, `group` or `share`, `root_folder_id` is the
user's access root for the folder, and `granted_at` is the server's time at which the user's
access to that root began (for `share`, the later of the share's creation and the membership's;
for `group`, the membership's), the earliest when several grants apply. A folder keeps its
entries while it is a tombstone, so that its delete can be read; the purge removes them.

`iamb_private.refresh_access` recomputes the entries for the folders, groups or users a change
touches and is called, in the same transaction, by everything that can change them: a folder's
creation, move, delete or restore, a share's creation, change or delete, every membership change
and a group's deletion. **Invariant:** after every commit the table equals the rule of §4.2
computed from `folder`, `folder_share`, `membership` and `profile`. Task 4.2 ships a slow function
that computes the rule from those tables alone, and task 4.3 compares the two after every step of
its tests.

Its changes are the visibility entries of the feed (C21): for a user, the top of each newly
readable tree gets `folder_visible`; the top of each tree that stops being readable while the
folder still exists gets `folder_revoked`.

### 5.5 The change feed

`feed (user_id, seq, kind, entity, row_id, folder_id, profile_id, sha256, created_at)`, primary key
`(user_id, seq)`, and `feed_head (user_id, head, floor)`.

| `kind` | Meaning | The device |
| --- | --- | --- |
| `row` | The row `entity`/`row_id` changed, or became readable or unreadable for this user by a move | Applies the row as `pull` delivers it (D3) |
| `folder_visible` | A tree with the top `folder_id` became readable through a share or a membership | Fetches the tree whole (D6) |
| `folder_revoked` | The tree stopped being readable although it still exists | Purges it (D6) |
| `conflict` | A conflict the user is party to was recorded, or its state changed | Tells the UI host (`ConflictRecorded`) |
| `invitations` | An invitation for or by the user changed | Tells the UI host (`AccountChanged`) |
| `entitlements` | The plan of the user or of one of their groups changed | Reads the entitlements again (§6.12) |
| `blob` | The bytes of an attachment (`profile_id`, `sha256`) were verified | Asks the service what is missing (§6.11) |
| `account` | The display name, a device or the account itself changed | Reads the account again; a revoked device signs out |

Rules for task 4.2:

- **Order.** An entry gets `seq = head + 1` while the transaction holds the row lock on the
  user's `feed_head`; a transaction that writes to several users' feeds locks them in the order
  of the user ids. Entries therefore become visible in `seq` order: a reader that has seen `n`
  will never later find an entry below `n`.
- **Who gets a `row` entry.** Everyone who could read the row before the write or can read it
  after it. The writer's own feed gets it too: that is how the other devices of the same account
  hear of the change.
- **One entry per row.** A new `row` entry for the same user and row replaces the older one. The
  feed of a user is therefore never longer than the rows they can read plus the other kinds of
  entry of the last 30 days.
- **Content is not in the feed.** `pull` joins the entry to the row as it is at that moment,
  through the reader's policies.
- **Retention.** Entries older than 30 days are deleted and `floor` is set to the highest `seq`
  deleted. A position below `floor` cannot be continued (`reset_required`, D7).
- **Poke.** After the entries are written the function sends `{ "epoch": …, "head": … }` on the
  user's live channel (§6.6).

### 5.6 History and conflicts

`history (entity, row_id, rev, row jsonb, deleted, written_at, written_by, written_device,
replaced_at, replaced_by_rev)`, primary key `(entity, row_id, rev)`: every version a write
replaced, as it was, tombstones included.

`conflict (id, entity, row_id, profile_id, kind, winner_rev, winner_user, winner_device,
loser_rev, loser_user, loser_device, created_at, state, resolved_at, resolved_by, resolution)`.
`kind` is `edit_edit`, `edit_over_delete`, `delete_over_edit`, `deleted_with_folder` or
`folder_restored`; `state` is `open`, `resolved`, `replaced` (a newer conflict on the same row took
its place) or `expired`. At most one conflict per row is open. The loser's content is the history
row `(entity, row_id, loser_rev)`; the winner's is the row itself until it changes again.

What a conflict gives the user (through `Conflicts` and `GetConflict` on the account endpoint):
the row, both versions with their author's display name, device name and time, which fields
differ (computed by the UI from the two versions), and two actions, "Keep this version" and
"Restore the other version" (C20). The interface shows it in the editor ("Changed on another
device" · [Show the other version], flow 6) and in a list.

History is not a feature of its own in version 1: a client reads a history row only through a
conflict it is party to. There is no read policy on `history`.

### 5.7 Receipts

`push_receipt (device_id, op_id, status, reason, rev, conflict_id, created_at)`, primary key
`(device_id, op_id)`: what the server answered to a pushed change, kept 30 days (C15).

### 5.8 Invitations

`invitation (id, profile_id, email, role, invited_by, created_at, expires_at, status, answered_at,
accepted_by)`. `email` is stored in lower case; `role` is `admin` or `member`; `status` is
`pending`, `accepted`, `declined`, `withdrawn` or `expired`; `expires_at` is 14 days after
creation. At most one pending invitation per group and address. An invitation is matched to an
account by the verified email address of the token, at the time of the call. The email that is
sent says who invited the person and to open Iamb, and carries no link that accepts anything
(ADR-0020).

### 5.9 Attachment files

Bytes live in one private storage bucket, `attachments`, as `<profile id>/<sha256>`.
`attachment_blob (profile_id, sha256, size, mime, state, uploaded_by, created_at, verified_at)`,
primary key `(profile_id, sha256)`, says whether the file is there: `pending` from the moment an
upload is allowed, `verified` once the server has computed the SHA-256 of the uploaded object
and found it equal to the name. Only verified files can be read. §6.11 has the calls.

### 5.10 Plans, entitlements and flags

For task 4.7; there is no payment provider and every account is on the default plan until one is
added.

| Table | Columns | Notes |
| --- | --- | --- |
| `plan` | `id` (text, primary key), `scope` (`user` or `group`), `name`, `limits jsonb`, `features jsonb`, `is_default`, `active` | One default plan per scope |
| `subscription` | `id`, `user_id` or `profile_id` (exactly one), `plan_id`, `status` (`active`, `past_due`, `cancelled`), `started_at`, `ends_at`, `source` (`manual` until a provider exists), `external_ref` | At most one active subscription per account and per group. A group's plan is paid by its owner |
| `feature_flag` | `key` (primary key), `value jsonb`, `note` | Server-wide switches |
| `feature_override` | `key`, `user_id`, `value jsonb` | A flag's value for one account |

An entitlement is a name and a value. `api.entitlements()` returns the caller's as a flat map of
strings, which the agent hands to the service (`SetEntitlements`) for the interface to read: the
user plan's limits and features under their names, each group's under `<name>@<profile id>` for
groups the caller owns or administers, and the flags under `flag.<key>`. Enforcement is on the
server; the device's copy only decides what the interface offers.

| Name | Scope | Enforced by | When it is reached |
| --- | --- | --- | --- |
| `sync` | user | `hello` | `error: plan` (the device stays local) |
| `devices_max` | user | `hello` for a new device | `error: plan_limit` |
| `snippets_max`, `folders_max` | user (personal profile), group (group's rows) | `push`, on a create | `blocked: plan_limit` (C14) |
| `shared_folders_max` | user | `push`, on a share | `rejected: plan_limit` (C11) |
| `groups_owned_max` | user | `create_group` | `error: plan_limit` |
| `members_max` | group | `invite`, `accept_invitation` | `error: plan_limit` |
| `storage_bytes_max` | user, group | `attachment_upload_begin` | `error: plan_limit` |
| `history_days` | user, group | The retention job | — |

A limit counts live rows. Lowering a plan never deletes anything: existing rows stay, and only
new ones are refused.

### 5.11 Server state and rate limits

`server_state` has one row: `epoch uuid`, `epoch_started_at`, `epoch_base_rev` (the value of
`rev_seq` when the epoch began), `protocol_min`, `protocol_max`, `retention_days` (30) and
`history_days` (90). `rate_bucket (subject, bucket, window_start, count)` holds the counters of
§6.8.

### 5.12 Retention

One scheduled database job a day. "Purge" is a hard delete.

| What | Rule |
| --- | --- |
| Tombstones of synced rows | Purged 30 days after `deleted_at`, children before parents, with their history, their conflicts, their links and their access entries. After that the row id is unknown to the server (C13) |
| Attachments nothing uses | An attachment row that no live snippet has been linked to for 30 days is deleted (a tombstone, purged 30 days later). When the last row for a `(profile, sha256)` is purged, the stored file and its `attachment_blob` row are removed |
| Feed entries | Deleted after 30 days; `floor` raised |
| History | A version is deleted 90 days after it was replaced (`history_days`), but not while an open conflict names it |
| Conflicts | An open conflict expires after 90 days; a closed one is deleted 30 days after it was closed |
| Receipts | Deleted after 30 days |
| Invitations | A pending invitation expires after 14 days; an answered or expired one is deleted after 90 days |
| Devices | A device that has not called for 180 days is removed with its session; it registers again, and resyncs, when it comes back |
| Deleted groups | The profile row and its memberships are tombstones for 30 days, then purged |
| Deleted accounts | §11.1 |

The 30 days are one number, `retention_days`, for tombstones, the feed and receipts, so there is
one line to remember: **a device that was offline for more than 30 days resyncs** (D7).

### 5.13 Row-level security, in words

Read rules. "Reads the folder" means a `folder_access` row for the caller and the folder exists.
Every rule also requires that the token's session belongs to a device that is not revoked.

| Table | A user may read a row when |
| --- | --- |
| `profile` | It is their personal profile; or they have a live membership in it; or they read a folder that belongs to it |
| `membership` | It is their own (also as a tombstone); or it is live and they have a live membership in the same group |
| `folder` | They read the folder (tombstones included, until the purge) |
| `folder_share` | They have `manage` on the folder; or they have a live membership in the group it names |
| `snippet` | They read its folder |
| `tag` | The tag's profile is their personal profile or a group they are a member of; or a snippet they can read has a live link to it; or they wrote it last |
| `snippet_tag` | They can read its snippet |
| `attachment` | As `tag`, with the links to attachments |
| `snippet_attachment` | They can read its snippet |
| `user_profile` | It is their own; or they share a live group with that person; or that person owns a profile they can read |
| `device` | It is their own |
| `folder_access` | It is their own |
| `feed`, `feed_head` | It is their own |
| `conflict` | They are `winner_user` or `loser_user` |
| `invitation` | They are owner or admin of the group; or it is pending and its address is the verified address of their token |
| `attachment_blob` | It is verified and they can read an attachment row with its profile and hash |
| `history`, `push_receipt`, `plan`, `subscription`, `feature_flag`, `feature_override`, `server_state`, `rate_bucket` | Never; functions read them for the caller |
| Storage objects in `attachments` | The object's name is `<profile>/<sha256>` of a verified `attachment_blob` row they may read. Writing: only the name of a `pending` row that names them as uploader, and only once |
| Live channel messages | The topic is `feed:<their user id>`. Nobody may send |

Write rules are §4.1 and §4.3, enforced by the functions.

### 5.14 Functions of `api`

Every function takes the device id and the protocol version and returns one JSON object with an
`error` member that is null or a code (§6.1).

| Function | Does |
| --- | --- |
| `hello` | Registers or refreshes the device; agrees the protocol version; returns the server's state (§6.2) |
| `push` | Applies changed rows under §8 (§6.3) |
| `pull` | Returns a page of the caller's feed with the rows (§6.4) |
| `fetch_folder`, `snapshot`, `visible_folders`, `access`, `users` | Whole-tree and whole-library reads, the list of readable folders, the caller's access roots, display names (§6.5) |
| `create_group`, `rename_group`, `delete_group`, `transfer_ownership` | Group calls (C16) |
| `invite`, `withdraw_invitation`, `accept_invitation`, `decline_invitation`, `invitations` | Invitations (C16) |
| `set_member_role`, `remove_member`, `leave_group` | Memberships (C16) |
| `conflicts`, `get_conflict`, `resolve_conflict` | Conflicts (C20) |
| `attachment_upload_begin`, `attachment_upload_commit` | Attachment files (§6.11) |
| `entitlements` | The caller's entitlements (§5.10) |
| `devices`, `rename_device`, `revoke_device`, `sign_out_everywhere`, `set_display_name`, `delete_account` | The account (§11.3) |

## 6. Protocol

### 6.1 Transport and answers

`iamb-sync` calls the functions of `api` over HTTPS through Supabase's REST interface
(`POST …/rest/v1/rpc/<function>` with the `api` schema selected), with the project's public key
and the account's access token. Every call names the calling `device` and the `protocol` version
agreed in `hello`. A request body is at most 4 MiB.

A function always answers with one JSON object. `error` is null, or one of the codes below, or a
code of the call itself (C16). Refusals are data, not HTTP errors, so that what the function
already wrote (a rate counter, a receipt) is committed.

| `error` | Meaning | The agent |
| --- | --- | --- |
| `upgrade_required` | The device's protocol is older than the server accepts | Stops syncing and says so (D12) |
| `server_too_old` | The device speaks only versions newer than the server's | Tries `hello` again every hour |
| `device_unknown` | No device row for this id and session | Calls `hello`, then repeats |
| `device_revoked` | The device was revoked | Signs out (D14) |
| `rate_limited` | Too many calls; `retry_after_ms` says when | Waits that long, then goes on (§6.8) |
| `reset_required` | `pull` only: the position cannot be continued; `reason` is `expired`, `epoch` or `ahead` | Resyncs (D7) |
| `too_large` | The request is over a size or count limit | Halves the batch and repeats |
| `plan` | The plan does not include sync | Stops syncing; the library stays local |

HTTP status 401 means the access token is no longer good: the agent refreshes it once and repeats;
a second 401 means the session ended, and the agent reports "signed out" without touching the
library or the outbox (D14). A network error, a time-out (30 seconds) or a 5xx status is retried
(§6.8). A function that fails unexpectedly rolls its transaction back and is retried the same way;
receipts make that safe (C15).

### 6.2 `hello`: registering the device and agreeing the version

Called after sign-in, at every start of the agent and after `device_unknown`.

```json
{ "device": { "id": "…", "name": "Mac", "platform": "macos", "app_version": "0.4.0" },
  "protocols": [1, 2] }
```

`protocols` is the oldest and the newest protocol version the build speaks. The server picks the
highest version both sides speak, creates or updates the device row for the token's session, and
answers:

```json
{ "error": null, "protocol": 2, "user": "…", "personal_profile": "…", "epoch": "…",
  "head": 1841, "retention_days": 30, "server_time": 1795190400000,
  "limits": { "push_ops": 200, "push_bytes": 4194304, "pull_entries": 500 } }
```

No common version gives `upgrade_required` (the device is too old) or `server_too_old`. A device
id that belongs to another account is refused with `device_revoked`: the device then makes a new
id (it was copied with its data directory, or the account changed).

### 6.3 `push`

```json
{ "device": "…", "protocol": 2, "ops": [
  { "op_id": "…", "entity": "snippet", "id": "…", "kind": "put", "base_rev": 5,
    "row": { "folder_id": "…", "abbreviation": ";rty", "label": "Thanks", "kind": "plain",
             "body": "Thanks for your order.", "trigger": "delimiter",
             "case_mode": "insensitive", "word_start": true, "enabled": true,
             "created_at": 1795190000000 } },
  { "op_id": "…", "entity": "folder", "id": "…", "kind": "delete", "base_rev": 3,
    "cascade": [ { "entity": "snippet", "id": "…", "base_rev": 5 } ] } ] }
```

- An op is one row as it is now on the device (`put`, with the model's fields and the creation
  time) or its deletion (`delete`). `base_rev` is the revision the device had when it made its
  first unsent change to the row; 0 for a row the server has never seen. The device sends
  nothing the server assigns (`rev`, `updated_at`, `updated_by`, `content_by`).
- `cascade` is on a folder's delete only: the sub-folders and snippets the device deleted with
  it, each with its base (C6). They are not sent as ops of their own.
- `repush` is on ops a device sends again after the server lost them (C18), with the revision the
  device holds.
- A request has at most 200 ops (a cascade list of up to 20,000 rows counts as one). The device
  builds it under D1, so a request never names a parent the server cannot know.
- The server applies the ops in dependency order whatever order they were sent in: folders with
  parents first, tags, attachments, snippets, links, shares; deletes last, children first.
- One request is one transaction. Each op is judged by itself under §8, and a refused op does not
  undo the others.

The answer has one result per op, in the order sent:

```json
{ "error": null, "head": 1843, "results": [
  { "op_id": "…", "status": "applied", "reason": null, "conflict": "…", "adjusted": [],
    "replayed": false, "blob": null,
    "row": { "id": "…", "rev": 9012, "created_at": 1795190000000, "updated_at": 1795190402113,
             "updated_by": "…", "deleted_at": null, "…": "…" },
    "origin": { "content_by": "…", "content_at": 1795190402113 } } ] }
```

| `status` | `reason` | Meaning | Written in the scenarios as |
| --- | --- | --- | --- |
| `applied` | — | A new revision was written. `conflict` is the id of the conflict it recorded, if any; `adjusted` lists fields the server set differently (C8) | `applied`, `applied+conflict`, `applied+adjusted` |
| `unchanged` | — | The row already is what was sent; nothing was written (C4) | `unchanged`, `unchanged+adjusted` |
| `superseded` | — | A share's stale change: the server's row stands (C11) | `superseded` |
| `rejected` | `forbidden`, `gone`, `parent_gone`, `plan_limit`, `not_pushable` | The change is refused for good (C11, C12, C13, C16) | `rejected:…` |
| `blocked` | `too_large`, `plan_limit`, `invalid`, `parent_blocked` | The change cannot be stored as it is; the row stays on the device (C14) | `blocked:…` |

`replayed` is true when the op id was seen before (C15; `replayed` in the scenarios). `row` is
the row as the server has it now and as the caller may read it, with `origin` for a snippet, or
null when the caller may not read it or it does not exist. `blob` is on an attachment's result:
`missing` when the server does not have the bytes yet (§6.11). D4 and D5 say what the store does
with each result.

### 6.4 `pull`

```json
{ "device": "…", "protocol": 2, "after": "<epoch>:1841", "limit": 500 }
```

```json
{ "error": null, "epoch": "…", "next": "<epoch>:1846", "head": 1846, "more": false,
  "entries": [
    { "seq": 1843, "kind": "row", "entity": "snippet", "id": "…", "row": { "…": "…" },
      "origin": { "content_by": "…", "content_at": 1795190402113 } },
    { "seq": 1844, "kind": "row", "entity": "snippet", "id": "…", "row": null },
    { "seq": 1845, "kind": "folder_revoked", "folder": "…" },
    { "seq": 1846, "kind": "folder_visible", "folder": "…" } ] }
```

- The **position** is the text `<epoch>:<seq>`, kept in `sync_state.position`. A device asks for
  what comes after its position and stores `next` in the same transaction as the page's effects
  (D11).
- A `row` entry carries the row as it is now, read through the caller's policies. `row` is
  **null** when the row is no longer there for the caller: purged, or no longer readable. The
  device then removes its copy (D3). A folder is delivered with an empty parent when the caller
  cannot read the parent.
- **A page is closed under references.** With each row the page carries every row it refers to
  (its folder and the folders above, the tag or attachment of a link, the profile) whose own
  entry is not in or before this page. A device can therefore apply every page by itself.
- **Pages.** At most `limit` entries (500 at most) and 4 MiB; `more` says whether to ask again at
  once. A page always ends at an entry boundary.
- **No gaps to detect.** Entries appear in `seq` order (§5.5), so a device never has to look
  back. Numbers missing from a page are entries that a newer one for the same row replaced.
- `reset_required` when the position cannot be continued: its epoch is not the server's
  (`epoch`), its `seq` is below the feed's `floor` (`expired`), or above its `head` (`ahead`: the
  server lost entries, and starts a new epoch at once, C18).

### 6.5 Whole reads

| Function | Returns | Used for |
| --- | --- | --- |
| `fetch_folder(folder, cursor)` | The live rows of the tree under `folder` that the caller can read: folders, snippets with their origin, links, the tags and attachments those links name (with whether the bytes are verified), the shares of those folders the caller may read, the owning profile; and `head`, the feed's head when the first page was read. Pages of 500 rows by `cursor` | `folder_visible` (D6) |
| `snapshot(cursor)` | The same for everything the caller can read, with the profiles and memberships | Resync (D7) |
| `visible_folders()` | The ids of all folders the caller can read | The access check (D7) |
| `access()` | The caller's access roots outside their personal profile: `root`, `profile`, `permission`, `via`, `granted_at`; and their role in each group | `SetAccess` (§7.1), after every page that held a visibility entry or a share, membership or profile row |
| `users(ids)` | The display names of those of the given accounts the caller may see (§5.13) | Names of authors and members |

A whole read is not one transaction. The device notes `head` from the first page, applies the
pages, and then pulls from that head, which brings whatever changed while it was reading; rows are
applied by revision, so applying one twice is harmless (D3).

### 6.6 The live channel

One private broadcast channel per user on Supabase's realtime service: topic `feed:<user id>`,
event `head`, payload `{ "epoch": "…", "head": 1846 }`, sent by the database whenever it writes to
that user's feed. It carries no content and nobody but the database can send on it. The device
joins with its access token, renews the token on the socket before it expires and sends the
service's heartbeat every 25 seconds.

- A poke whose head is above the device's position starts a pull at once.
- After the channel (re)connects the device pulls once, because pokes may have been missed.
- **When it drops** (no heartbeat answer within 10 seconds, or the socket closes) the device
  polls instead: a `pull` every 2 seconds while the user was active in the last 10 minutes (a
  local change, or the library window open), every 60 seconds otherwise. It tries to reconnect
  after 1, 2, 5, 10 and 30 seconds and then every 60 seconds, each with up to 20% added at
  random.
- While the channel is up the device still pulls every 5 minutes, which catches a channel that
  died without saying so.

### 6.7 The sync loop

The agent runs one loop per signed-in account; D11 is the rule.

1. `hello` (once per start).
2. **Pull** until `more` is false: apply rows, fetch newly visible folders, purge revoked ones.
3. **Push** until the outbox has nothing to send, 200 ops at a time; act on every result.
4. **Pull** again (the push's own changes come back through the feed, with whatever the server
   changed around them).
5. Read `access`, `entitlements`, missing attachments (§6.11) if step 2 or 4 said they changed.
6. Wait for: `OutboxChanged` from the service (then wait 250 milliseconds more, so that a burst
   of saves becomes one request), a poke, a poll timer, or the network coming back. Go to 2.

Pull comes before push so that a device learns of a revoked folder or a lowered permission before
it sends, and has the parents of what it will send. It changes no outcome: the server refuses
what is not allowed in any order, and a pulled row never replaces an unsent change (D2).

| Budget | Parts |
| --- | --- |
| A change on device A visible on device B within 5 seconds | 250 ms debounce on A, the push (one request), the poke, B's pull (one request), `ApplyRemote` and the snapshot on B. With B's channel down, B's next poll is at most 2 seconds away |
| A shared folder on a member's device within 10 seconds | The owner's push of the share row, the poke, the member's pull, `fetch_folder` for the tree (one request per 500 rows), `ApplyRemote`; attachment bytes follow and are not part of the 10 seconds |

### 6.8 Retry, backoff and rate limits

- **Retry.** A call that failed without an answer (network, time-out, 5xx) is repeated with the
  same arguments, pushes with the same op ids, after 1, 2, 4, 8 … seconds up to 5 minutes, each
  with up to 20% added at random; the delay starts again at 1 second after a call that
  succeeds. While offline (the system reports no network) the agent does not try at all and
  starts at once when the network returns.
- **Order is kept.** A push that fails blocks later pushes, never pulls.
- **Rate limits**, counted by the server per device in windows of one minute (`rate_bucket`),
  recommended values: `push` 60 calls and 6,000 ops; `pull` 120 calls; whole reads 300 pages;
  `hello` 10 calls; account calls 30 per account; invitations 20 an hour per group and 50 a day
  per account. Over the limit the call answers `rate_limited` with `retry_after_ms`, having
  written nothing else. The first upload of a library of 10,000 snippets takes about two minutes
  at these values. The values are settings of the server, not of the protocol (§13).

### 6.9 Partial failures

| What fails | What holds |
| --- | --- |
| One op of a push is refused or blocked | The other ops are applied; each result is acted on by itself (D4, D5) |
| A push is cut off before the answer | The outbox still holds the changes; the same request is sent again and the receipts return the first answers (C15) |
| A push is delivered twice | The second finds the receipts (C15) |
| The agent stops between the pages of a pull | Each page's effects and its position are one store transaction; the next start continues after the last complete page (D11) |
| The agent stops in the middle of a whole read | The position is unchanged; the read starts again and applies rows by revision |
| The service is not running | The agent cannot read the outbox or apply rows; it waits and tries the endpoint again. Nothing is acknowledged that was not stored |
| The store refuses a row of a page (it should not) | The position does not move past it; after three tries the agent resyncs (D7) |
| The server is restored from a backup | A new epoch; devices send back what the server lost (C18) |

### 6.10 Versions

- **The protocol version** is one number. It is raised when a build could misread what the other
  side sends or would lose something by not knowing it: a new entity, a new field of a synced
  row, a new value of an enumeration, a changed meaning. New functions, new feed kinds and new
  members of an answer that an older device can ignore do not raise it (a device ignores feed
  kinds and answer members it does not know).
- **The server speaks a range**, `protocol_min` to `protocol_max`. Each version's `push` writes
  exactly the columns that version knows and leaves the others as they are, so an old device
  that saves a row does not erase a field a newer device wrote; a new column must have a default
  that is right for rows written without it. `pull` gives each device the row in the shape of
  its version.
- **A value an older version cannot hold** (a new enumeration value) makes the row unreadable
  for it: the server raises `protocol_min` before any build can write such a value. Raising it
  is announced by the update channel first (ADR-0023); devices below it get `upgrade_required`.
- **After a build that speaks a newer version starts for the first time**, the device resyncs
  once (D7), which fills in what the older build could not hold.
- The local database's schema version and the IPC protocol version (architecture §6.1) are
  separate numbers; neither is sent to the server.

### 6.11 Attachment files (the frame for task 4.6)

Rows first, bytes after (architecture §9).

- **Up.** The push result of an attachment row says `blob: missing` when the server has no
  verified file for its profile and hash. The agent then calls
  `attachment_upload_begin(profile, sha256, size, mime)`, which checks that the caller may write
  in the profile (§4.3), the size limit of task 4.6 and `storage_bytes_max`, and creates the
  `pending` row; uploads the file to `attachments/<profile>/<sha256>`; and calls
  `attachment_upload_commit(profile, sha256)`, which computes the SHA-256 of the stored object on
  the server, marks the row `verified` and writes a `blob` entry to the feed of everyone who can
  read the attachment, or deletes the object when the hash is wrong. `begin` on a verified file
  answers `exists`, and nothing is uploaded. The device records in `attachment_state` what it has
  uploaded; a file that is not on the device (it only has the row) is not uploaded by it.
- **Down.** `MissingAttachments` lists the attachments of the effective library whose file the
  device lacks and whose bytes are verified on the server. The agent downloads each, checks the
  hash, writes it to `attachments/` (temporary file, rename) and tells the service
  (`AttachmentAvailable`). Until then the snippet falls back to plain text
  (`attachment_missing`).
- The server-side hash check exists because a file is named by its hash and written once: without
  it a member with `edit` could put wrong bytes under a name and block the right ones for
  everyone.

### 6.12 Entitlements

After `hello`, and whenever the feed holds an `entitlements` entry, the agent calls
`entitlements()` and hands the map to the service (`SetEntitlements`). The interface reads it to
decide what to offer; the server enforces the same limits whatever the device believes (§5.10).
