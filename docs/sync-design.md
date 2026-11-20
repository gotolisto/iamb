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
| Is snippet content hidden from the server? | Not in version 1: it is protected in transit and at rest, and readable by the server (§11.1) | End-to-end encryption needs key distribution, recovery without a password, and a member online to hand a key to a new member, against the 10-second target. It is open question 2 (§13) |
| What does signing in do to the library on a device? | It adopts it into the account and syncs all of it (D9) | The gate is two devices of one account in sync. A choice of what stays local is a later option; the sign-in dialog must say what happens (open question 1, §13) |

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
user's access root for the folder, and `granted_at` is the server's clock at the moment the
user's access to that root began: the time its entry was first written, kept for as long as the
access lasts without a break, whichever grants come and go meanwhile. It is never taken from a
row a device wrote, so no device clock can move it (D13 orders shared folders by it). A folder
keeps its entries while it is a tombstone, so that its delete can be read; the purge removes
them.

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
  hear of the change. A snippet that becomes readable for a user through a move brings its
  links: each gets a `row` entry for that user, and the tags and attachments they name come
  with the page (§6.4). A new member's feed gets a `row` entry for the group's profile and for
  each live membership of the group.
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

`server_state` has one row: the current `epoch uuid`, `protocol_min`, `protocol_max`,
`retention_days` (30) and `history_days` (90). `epoch (id, started_at, base_rev)` has one row
per epoch the database has had; `base_rev` is the value of `rev_seq` when the epoch began, that
is, the last revision the restored backup knew. `rate_bucket (subject, bucket, window_start,
count)` holds the counters of §6.8.

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
a second 401 means the session ended, and the agent signs out as D14 says, which keeps the
user's own library and its outbox. A network error, a time-out (30 seconds) or a 5xx status is retried
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
  server lost entries, and starts a new epoch at once, C18). With the reason `epoch` the answer
  carries `repush_above`: the base revision of the epoch that followed the one in the device's
  position (C18).

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
| A change on device A visible on device B within 5 seconds | 250 ms debounce on A, the push (one request), the poke, B's pull (one request), `ApplyRemote` and the snapshot on B. With B's channel down and B in use, B's next poll is at most 2 seconds away. A device that nobody has used for 10 minutes and that has no live channel polls once a minute, and is the one case outside the 5 seconds |
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
  read the attachment, or deletes the object when the hash is wrong. Because a database function
  cannot read a stored object, `attachment_upload_commit` is the one call that is a Supabase
  edge function: it reads the object, computes the hash and calls a function of `iamb_private`
  that does the rest. `begin` on a verified file answers `exists`, and nothing is uploaded. The
  device records in `attachment_state` what it has uploaded; a file that is not on the device
  (it only has the row) is not uploaded by it.
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

## 7. On the device

The service owns the store and has no network code (ADR-0008); the sync agent has the network
and no store. Everything below crosses between them as messages of the Sync group
(architecture §6.2). §9 holds the rules; this section holds what they need.

### 7.1 What the store gains (migration 3, task 4.4)

| Change | For |
| --- | --- |
| `outbox.op_id` (16 bytes, made when the entry is written) | The id of the change on the wire (C15) |
| `outbox.cascade_of` (the `seq` of a folder's delete entry, on the entries its cascade wrote) | The cascade list of a folder's delete (C6) |
| `outbox.blocked` (a reason, or null) | Changes the server cannot store yet (D5) |
| `outbox.repush_rev` (a revision, or null) | Rows a device sends again after the server was restored, with the revision it holds (C18, D7) |
| The unique index `snippet_abbreviation` becomes an ordinary index; the check stays in the store's local write path | Rows from the server are applied as they are; duplicates are ordered by precedence (C9, D13) |
| `sync_state` gains `account_user_id`, `device_id`, `personal_profile_id` | Which account the library belongs to (D9, D14) |
| `folder_access (folder_id, profile_id, permission, via, granted_at)` | The user's access roots as the server computed them (§4.2); read-only state in the interface, and precedence (D13) |
| `snippet_origin (snippet_id, content_by, content_at)` | Who wrote a snippet's content (C17, D10) |
| `approval` gains `approved_content` (the abbreviation, kind, body, trigger, case mode and word-start flag that were allowed, as JSON); `approved_rev` stays for the review dialog | "The last approved revision keeps expanding" without the server (D10) |
| `kept_change (id, entity, row_id, content, place, reason, made_at, kept_at)` | Changes the server refused, kept for the user (D5) |
| `known_user (user_id, display_name)` | Authors' and members' names for the review dialog and the folder header |
| `attachment_state (profile_id, sha256, uploaded)` | Which files this device has sent (§6.11) |

All of these are device-local and never synced, like `usage`, `setting`, `app_exclusion` and
`approval` (architecture §7). `outbox` no longer receives entries for `profile` and `membership`
rows: the server writes those (C16).

### 7.2 Messages to add

Sync group of the service endpoint (role `sync`; architecture §6.2). The four existing names
keep their meaning and gain members.

| Message | Change |
| --- | --- |
| `OutboxRead(limit)` | Returns changes ready to send, built under D1: for each, the op id, the entity, the row as it is now (or that it is deleted), the base revision, the cascade list, and the `seq` values it covers |
| `OutboxAck(results)` | Takes the server's result for each change (status, reason, the row and origin the server returned, the `seq` values) and acts on it under D4 and D5, in one transaction |
| `ApplyRemote(batch, position)` | Applies rows under D2 and D3; a null row removes the local row; stores `position` in the same transaction when given; answers per row (`applied`, `shadowed`, `stale`, `removed`, `refused`) |
| `PurgeFolder(id)` | Under D6: first moves unsent changes of the tree to `kept_change`, then removes the tree, its approvals, usage, origins and cached files, and rows of other people's profiles that nothing left uses |
| `SyncState` (new) | The account id, device id, position, the count of waiting, blocked and kept changes |
| `Adopt(user, personal_profile, mode)` (new) | D9 |
| `ReconcileBegin(scope, repush_above)`, `ReconcileEnd(token, position)` (new) | D7: rows applied between the two with the token are noted; `End` removes synced rows of the scope that were not among them and have no unsent change. With `repush_above` (from `reset_required`), `Begin` first writes an outbox entry, marked for re-push, for every synced row in a higher revision |
| `SetAccess(roots, roles)`, `SetKnownUsers(users)` (new) | Fill `folder_access` and `known_user` |
| `PutShare(share)`, `DeleteShare(id)` (new) | Write a share as a local change, with an outbox entry, for the account endpoint's `SetFolderShare` and `RemoveFolderShare`, so that sharing works offline (flow 6) and in order with the folder's own rows |
| `SignedOut(keep)` (new) | D14 |
| `UploadedAttachment(profile, sha256)`, `AttachmentsToUpload` (new) | §6.11 |

Library group (role `ui`): `ListKeptChanges`, `CopyKeptChange(id, folder)` (makes a new snippet
in a folder of the user's from a kept change), `DismissKeptChange(id)`; `ListFolders`,
`ListSnippets` and `GetSnippet` over the whole effective library with each folder's profile,
owner, access and permission (UX spec §11.2, entry 16); `ListPendingReviews` with the approved
content, the pending content, the author's name and the time of the change (entry 18); a
diagnostic `needs_review` when a held snippet is typed (entry 19).

Account endpoint (architecture §6.4), besides what is listed there: `CreateProfile(kind, name)`,
`RenameProfile`, `DeleteProfile`, `TransferOwnership`, `SetMemberRole`, `LeaveProfile`,
`WithdrawInvitation`, `DeclineInvitation` (UX spec §11.2, entry 17); `GetConflict(id)`,
`ResolveConflict(id, choice)`; `Devices`, `RenameDevice`, `RevokeDevice`, `SignOutEverywhere`,
`SetDisplayName`, `DeleteAccount`; `SyncNow`. `Invitations` answers both the invitations the user
received and those they sent.

These are additions an older peer can ignore, except the changed answers of `OutboxRead`,
`OutboxAck` and `ApplyRemote`; no build ever answered those (the service says `not_implemented`
for the Sync group today), so task 4.4 raises the IPC protocol to 4 when it implements them.

### 7.3 The answers to the four questions the store left open

`crates/iamb-store/src/ops.rs` lists four edges of soft delete "for the sync design" (review 1.9,
finding 22). Task 4.4 replaces that comment with a pointer to this table.

| Question | Rule |
| --- | --- |
| Deleting a snippet or a tag leaves its link rows live; deleting a profile leaves its shares and folders | Kept, on both sides, for links: a link counts only while it and both its ends are live, and is purged with either end (C10). That is what lets Undo bring a snippet back with its tags. A folder's delete now also deletes the folder's shares, on the device and on the server (C6). A profile is deleted only by the server, which deletes its memberships, the shares into it and the folders it owns in the same transaction (C16) |
| `put` on a deleted folder restores it but not what the delete cascaded to | Kept, on both sides (C7): a restored folder comes back empty of what was deleted with it, and unshared. Each cascaded row is restored by itself: Undo right after the delete (a `put` per row), or the conflict that the server recorded for a row the deleting device had not seen (C6) |
| A purge can remove rows that still have unsent outbox entries | Not any more (D8): the local purge skips every row with an entry in the outbox, waiting or blocked |
| One conflicting row fails a whole `apply_remote` batch | Not any more (D3): a row from the server cannot conflict on its abbreviation (C9), and each row of a batch is applied or reported by itself |

## 8. The rules on the server

How `push` and the account calls decide. Scenario numbers point to §12.

**C1.** *Arrival order.* The last write is the one the server accepts last. The server takes a
row lock, gives the write the next value of `rev_seq` and its own clock as `updated_at`. A
device's clock is used for nothing but `created_at` (and that is cut back to the server's clock
when it lies in the future) and for what the user is shown about a change. (S24, S25)

**C2.** *The whole row.* An accepted `put` replaces every field of the row with what was sent; a
`delete` sets `deleted_at`. The version that was there goes to `history` as it was. Fields are
never merged. (S01, S02, S06, S12)

**C3.** *A stale write still wins, on the record.* A write is stale when its `base_rev` is below
the row's `rev`. It is applied like any other. A conflict is recorded when both hold: the version
it replaces was written by another device; and since the base someone changed a recorded field
or whether the row is deleted. Recorded fields are, for a snippet: folder, abbreviation, label,
kind, body, trigger, case mode, word-start, enabled; for a folder: parent, name, the member
permission and the defaults for new snippets; for a tag: name and colour. A folder's `position`
is not recorded, and links, attachments and shares never get conflicts. The comparison is
between the base version, taken from the history, and the version being replaced; when the
history no longer holds the base, between what was sent and the version being replaced. The
conflict names the writer as winner and the author of the replaced version as loser, and is
shown to both; a newer conflict on the same row closes the older one as `replaced`. (S01, S08,
S22, S49)

**C4.** *Nothing new, nothing written.* When what was sent equals the row in every field and in
whether it is deleted, the answer is `unchanged` with the row's revision: no revision, no
history, no feed entry, no conflict, whatever the base. (S05, S42, S43)

**C5.** *A delete is a write.* Whichever of an edit and a delete arrives last wins. A stale
`delete` deletes the row and, under C3, records `delete_over_edit` with the live version as the
loser. A `put` on a tombstone makes the row live again and, when stale, records
`edit_over_delete` with the tombstone as the loser. A `delete` of a tombstone is `unchanged`; a
`delete` of a row the server does not know is `unchanged` too. (S03, S04, S05, S18, S19, S20,
S22)

**C6.** *A folder's delete takes the subtree.* Deleting a folder deletes, in the same
transaction, every live folder and snippet below it as they are on the server at that moment,
and every live share of those folders (C21 then ends the access they gave). Each gets its own
revision and feed entry. The op's `cascade` list says which of them the deleting device knew
and in which revision: a row that another device wrote and that the list does not name, or
names with a base below its revision after a change to a recorded field, gets a
`deleted_with_folder` conflict, so it can be restored. Restoring a row restores the folders
above it (C7). (S15, S16, S23)

**C7.** *A live row needs live folders.* A `put` that would leave a live snippet or folder under
a deleted folder restores every deleted folder on the way up, if the writer may: `manage` for a
top-level folder, `edit` on its parent for a sub-folder. Each restored folder gets a new
revision and a `folder_restored` conflict between the writer and whoever deleted it. What was
deleted with the folder stays deleted, and its shares stay deleted: access never comes back by
itself. If the writer may not restore a folder on the way, the `put` is `rejected: parent_gone`.
(S15)

**C8.** *Structure is put right, not refused.* A folder `put` that would make the folder its own
ancestor, or make any tree deeper than four levels, keeps the folder's current parent; a new
folder that would be a fifth level is put under the nearest folder above the requested parent
that keeps four levels, or at the top. Everything else of the row is applied, `adjusted` names
`parent_id`, and the device takes the server's row. A row whose profile would change (a folder
under a parent of another profile, a snippet moved to a folder of another profile, a link across
profiles) is `blocked: invalid`. (S13, S14)

**C9.** *The server does not read content.* No uniqueness, search or parsing applies to
abbreviations, labels, bodies or names; the server only stores them and compares them for
equality (C4, C17). Two snippets with the same abbreviation in one profile, two tags or two
sibling folders with the same name are all accepted. Which snippet holds an abbreviation is
decided on each device by the same precedence (D13). (S09, S10, S11, S25, S48)

**C10.** *Links are rows.* A link is created and deleted by itself; nothing cascades to it and it
cascades nothing. It counts only while it and both rows it joins are live, and it is purged with
either. A link to a tombstone is accepted and waits; a link to a row the server does not know is
`rejected: parent_gone`. Two links for the same pair may exist; devices treat them as one. (S17,
S18, S19, S20, S21)

**C11.** *Sharing never wins by being late.* A share's id is derived from its folder and group,
so a pair has one row. A `put` needs `manage` on the folder, a folder in a personal profile, and
the writer's live membership in the group; over the plan's `shared_folders_max` it is
`rejected: plan_limit`. A stale `put` is not applied: the answer is `superseded` with the
server's row. A `put` based on the row's current revision is applied, also on a tombstone
(sharing again). A `delete` is applied whatever its base. Shares get no conflicts; their
versions are in the history. (S31, S36, S37, S46)

**C12.** *Permission is judged on arrival.* Every op is checked against §4.3 with the memberships,
shares and folders as they are on the server at that moment, for the place the row is in and
the place it would be in. What the device believed, and what was true when the user made the
change, do not count. A refused op is `rejected: forbidden` and the answer carries the server's
row if the caller may still read it. (S30, S31, S34, S36, S39)

**C13.** *Rows the server does not know.* A `put` with a base above 0 for an id the server does
not have is `rejected: gone`: the row was purged more than 30 days after its delete, and it does
not come back. A `put` with base 0 creates the row. A row whose folder, or other row it must
refer to, is unknown to the server or unreadable for the writer is `rejected: parent_gone`.
(S23)

**C14.** *What cannot be stored stays local.* A row over a size limit is `blocked: too_large`; a
content row over the plan's limit is `blocked: plan_limit`; a row the server's version of the
protocol cannot accept is `blocked: invalid`; a row whose parent was blocked in the same request
is `blocked: parent_blocked`. Nothing is written for it and the other ops go on. (S45)

**C15.** *Once.* The server keeps, for 30 days, the answer it gave to each `(device, op_id)`,
and takes a lock on the device's receipts for the length of a push. An op whose id it has seen
writes nothing: the answer is the stored status with `replayed` set and the row as it is now.
(S41, S42)

**C16.** *Profiles, memberships and invitations belong to the server.* They change only through
account calls, never through `push` (`rejected: not_pushable`). Every call locks the group's
profile row first, checks §4.1 and its own conditions against the state it finds, and writes
everything in one transaction; of two calls that race, the one that commits first decides what
the other finds. `accept_invitation` needs a pending, unexpired invitation whose address is the
token's verified address (`invitation_invalid` otherwise; `plan_limit` when the group is full);
`withdraw_invitation` needs it pending (`already_accepted` otherwise); `leave_group` is refused
to the owner (`owner_cannot_leave`); `transfer_ownership` and `delete_group` to anyone else
(`not_owner`); `transfer_ownership` needs a live member as the new owner (`not_a_member`) and
changes the profile's owner and both memberships together. Ending a membership (remove, leave)
deletes the shares from that person's folders into the group. Deleting a group deletes its
memberships, the shares into it and the folders it owns with their content; every member gets
`folder_revoked` for the group's folders. A call that is repeated after it succeeded (the answer
was lost) answers as it did the first time: `create_group` and `invite` carry the id of the row
they create, and the others find the state they asked for. (S30, S32, S33, S34, S35, S39)

**C17.** *The server says who wrote it.* `updated_by` is the caller of the write and
`updated_device` their device, for every row; a client cannot set either. For a snippet the
server also keeps `content_by` and `content_at`: set to the caller and the time when the row is
created or when the write changes the abbreviation, kind, body, trigger, case mode or
word-start flag, and left alone otherwise. (S40)

**C18.** *After a restore, devices give back what the server lost.* A restored database starts a
new epoch: a new row in `epoch` with the restored value of `rev_seq` as its base, and `rev_seq`
moved 2^40 further, past any revision a device can hold (the restore procedure runs
`iamb_private.begin_epoch`; a `pull` whose position is ahead of the head runs it as well, so a
forgotten step heals at the first device that notices). Every position of an older epoch gets
`reset_required` with `repush_above`, the base of the epoch that followed the device's. A device
then resyncs (D7) and sends again, marked `repush` with the revision it holds, every row it has in
a revision above that base: those are the rows written after the backup. A re-push onto a row
nobody has written in the new epoch is applied without a conflict; of two re-pushes for one row
the higher old revision stays, the other is `superseded`; a re-push onto a row written in the new
epoch is an ordinary stale write (C3). (S43)

**C19.** *Attachments.* An attachment row never changes after its creation; a `put` that
differs from it is `unchanged`. Several rows may name the same profile and hash; the stored
file exists once per profile and hash and is readable when verified (§6.11). An attachment that
no live snippet is linked to for 30 days is deleted by the server. (S19, S20, S21)

**C20.** *Resolving a conflict.* Either party may close a conflict. "Keep" closes it. "Restore"
needs the right to write the row now; the server writes the loser's version as a new revision
by that user (a live loser makes the row live and restores the folders above it, C7; a
tombstone as loser deletes the row, a folder with its subtree), records no conflict for that
write, and closes the conflict. The version it replaces goes to the history like any other.
(S07)

**C21.** *Visibility.* What a user may read follows from `folder_access` (§5.4), which the server
keeps equal to §4.2 inside every transaction that changes a folder's place, a share or a
membership. When a tree becomes readable for a user, `folder_visible` for its top goes into
their feed; when it stops being readable while it still exists, `folder_revoked`. A tree that
is revoked while part of it stays readable through another grant gets `folder_revoked` for its
top followed by `folder_visible` for each part that stays. A folder that is still reachable
through another grant is not revoked; only its permission changes. A deleted folder is not
revoked: it is delivered as a tombstone. (S10, S11, S28, S29, S30, S35, S36, S37, S38)

**C22.** *Thirty days.* Tombstones, the feed and receipts are kept 30 days (§5.12). Inside that
time every rule above applies whatever the delay. After it a deleted row is unknown (C13) and a
device's position is too old to continue (D7). (S22, S23, S29)

**C23.** *Versions.* The server accepts the protocol versions from `protocol_min` to
`protocol_max` and answers every call of an older device with `upgrade_required`, having done
nothing. Each version's functions read and write the row shape of that version (§6.10). (S44)

## 9. The rules on the device

**D1.** *From outbox entries to changes.* The store writes one outbox entry per local change, in
the transaction that makes the change, as it does today. What is sent is one change per row:
the row as it is now, or its deletion; the base revision of its oldest waiting entry; the op id
of its newest. A row that was created and deleted without ever being sent is not sent at all:
its entries are dropped and the row removed. Rows are taken in the order of their oldest entry,
up to 200; then every row those refer to that the server has never seen is added, so that a
request is complete in itself. A row that refers to a blocked row is blocked with it. Changes
are sent in dependency order (§6.3). (S26, S27)

**D2.** *A pulled row never replaces an unsent change.* While a row has an entry in the outbox,
waiting or blocked, `ApplyRemote` leaves the row alone and answers `shadowed`. The device's own
push follows, wins on the server (C2), and the server keeps the version the device did not
apply, with a conflict (C3). (S01, S03, S04, S22)

**D3.** *Applying pulled rows.* Each row of a batch is handled by itself, in dependency order
with foreign keys checked at the end of the batch. A row is written only when its revision is
above the local one; an equal or lower revision is `stale` and ignored. A null row removes the
local row (`removed`) unless D2 holds. A tombstone for a row the device does not have is
ignored. No uniqueness check applies to rows from the server. A folder whose chain of parents
does not reach the top within eight steps is shown at the top level, so a cycle that exists for
a moment between a pull and the answer to a push cannot hide a folder. (S13, S15, S16, S18)

**D4.** *Acting on `applied` and `unchanged`.* The entries the change covered are removed. If
the row has no newer entry, the store replaces it with the row of the answer (revision, times,
author, origin, and any field the server adjusted). If the user changed the row again while the
push was under way, the store keeps the row, takes only the revision from the answer, and sets
it as the base of the entries that remain, so that the next push is based on the device's own
accepted version. (S05, S13, S14, S26, S41)

**D5.** *Acting on the other answers.* `rejected`: every entry of the row is removed, newer
ones included; for a `put` the device's version of the row, as it is now, is copied into
`kept_change` with the reason and the folder's name; then the row of the answer replaces the
local row, or the local row is removed when the answer has none. For a refused share nothing is
kept, the row is put back or removed, and the user is told. `superseded`: the entries are
removed and the server's row replaces the local one.
`blocked`: the entries stay, marked with the reason; the row stays as it is, works on this
device, and is offered again when it changes or when the entitlements change. The user sees
kept and blocked changes in the library, never as a notice over another application (UX spec
§4.11). (S23, S30, S31, S36, S37, S39, S45, S46)

**D6.** *Folders that appear and disappear.* On `folder_visible` the agent fetches the tree
(`fetch_folder`), applies it between `ReconcileBegin` and `ReconcileEnd` for that tree, and reads
`access` again. On `folder_revoked` the service purges the tree (`PurgeFolder`): unsent changes
in it go to `kept_change` first and leave the outbox; then the folders, snippets, links, shares,
approvals, usage and origins of the tree are removed, with cached files and rows of other
people's profiles that nothing the device still holds uses. When the tombstone of the user's own
membership arrives, the service also removes that group's profile row and its other
memberships; the group's folders go through `folder_revoked`, which the server writes in the
same transaction. (S10, S28, S29, S30, S35, S37, S38)

**D7.** *Resync.* On `reset_required`, after every sign-in, after the first start of a build
with a newer protocol, and when the store refuses a pulled row three times, the device fetches
everything it may read (`snapshot`) between `ReconcileBegin(all)` and `ReconcileEnd`, which
removes synced rows that were not in it. Rows with unsent changes, and the folders above them,
are left alone; the push that follows settles them (C13, D5). In a new epoch the device first
queues for re-push every row it holds in a revision above `repush_above` (C18). A resync ends
with the **access check**: the device asks for the folders it may read (`visible_folders`) and
purges every folder of another profile that is not among them. The access check also runs at
every start of the agent and once a day, so that no lost message can leave a revoked folder on
a device that is online. (S23, S29, S43, S44, S49)

**D8.** *The local purge.* Once a day and after each sync the store removes tombstones whose
`deleted_at` is more than 30 days old, with their links, usage and approvals, but never a row
that has an entry in the outbox. A tombstone the server never saw (revision 0, no account) is
removed after 30 days as well. (S23, S29)

**D9.** *Adoption.* The first time an account signs in on a device, the library that is there
becomes that account's. In one transaction the store inserts the account's personal profile
row, moves every folder, tag and attachment of the local personal profile to it (and the files
under `attachments/`), removes the local profile row and all tombstones, and records the
account. The agent resyncs (D7). Then the store merges, top down: a local folder that was never
synced and has the name and parent of a folder of the account gives its content to that folder
and is dropped; a local tag with the name of an account tag likewise; a local snippet that
equals a snippet of the same folder in abbreviation, label, kind, body and options is dropped.
Every remaining row that was never synced is queued as new, in dependency order, and pushed.
Two computers that sign in for the first time at the same moment cannot see each other's rows
in time to merge; the account then has two folders of the same name, which the user merges by
hand. A library that belongs to another account is never adopted silently: the interface asks whether
to add it to the new account (every row gets a new id) or to remove it from the device. (S48)

**D10.** *Review by content.* A snippet is held on a device when its content was written by
someone else (`content_by` is not the signed-in user), its template contains `{key}`, `{wait}`,
`{clipboard}` or a line break (ADR-0022; a template that does not parse counts), and its
abbreviation, kind, body, trigger, case mode and word-start flag are not exactly what the
device's `approved_content` holds for it. Whenever a revision is not held, or the user approves
one, those six fields are stored as `approved_content`. While a snippet is held, the engine's
reference set and matchable set use the stored content, or leave the snippet out when there is
none. Approvals are per device and are removed with the folder (D6). (S40)

**D11.** *One loop, one position.* The agent pulls, pushes, and pulls again (§6.7). The effects
of a page and its position are one store transaction, and a page that is read twice has the
same effect as once. A poke or a poll only starts the loop early; without the live channel the
device polls (§6.6). (S47)

**D12.** *Too old.* On `upgrade_required` the device stops calling the server, keeps expanding
and editing, keeps every change in its outbox, and says that Iamb must be updated to keep
syncing. (S44)

**D13.** *Who holds an abbreviation.* The service orders the effective library before it builds
the snapshot: first the snippets of the user's personal profile; then the other access roots by
the time access to them began (`granted_at`, from the server) and then by the root folder's id;
inside one of these by `created_at` and then id. The first snippet with a folded abbreviation
holds it; the others neither expand nor resolve, and library health lists them. A snippet in a
shared folder can therefore never take an abbreviation from one of the user's own. The store
still refuses a local edit that would create a duplicate inside a profile, and the editor still
warns about a shared one (UX spec §4.3). (S09, S10, S11, S25, S48)

**D14.** *Signing out.* The agent drops the tokens and ends the session on the server when it
can. The service removes everything of other people's profiles (D6, without keeping changes:
the user is asked first when unsent changes to shared folders exist) and keeps the user's own
library, with its revisions, its outbox and the account it belongs to. The library keeps
working. Signing in again with the same account resyncs (D7) and pushes what waited; a
different account is D9's last sentence. A device the server revoked behaves the same way.
(S49)

## 10. The conflict questions, one by one

| Question | Answer | Rules | Scenarios |
| --- | --- | --- | --- |
| Last write wins by which clock? | By the order in which the server accepts writes. The server's revision counter orders them and its own clock dates them. No device clock, no hybrid clock | C1 | S24, S25 |
| What is kept in the history? | Every version a write replaced, whole, tombstones included, for 90 days, and for as long as an open conflict names it | C2, §5.6, §5.12 | S01, S04 |
| What does a recorded conflict hold, and how does the user restore? | The row, the two versions (the current row and a history version), who wrote each, on which device and when, and a kind. Both authors see it. "Restore the other version" makes the server write that version as a new revision; "Keep this version" closes it | C3, C20, §5.6 | S01, S07 |
| Field by field, or the row? | The row. A stale write replaces fields it did not mean to change, and the conflict offers the other version back. The position of a folder is the one field that is replaced without a record | C2, C3 | S02, S06, S08 |
| Delete against edit | Whichever arrives last wins, in both orders; the other is offered back; a tombstone lasts 30 days | C5 | S03, S04, S22 |
| Delete against delete | One delete; the second changes nothing | C4, C5 | S05 |
| Move against edit | A snippet's folder is a field of its row, so it is an ordinary conflict of two edits | C2, C3 | S06, S12 |
| A folder's delete against changes inside it | The delete takes everything in the folder as the server has it, and records a conflict for what the deleting device had not seen; a write that arrives after the delete brings the folder back with that row only | C6, C7 | S15, S16 |
| Folders moved into each other, or too deep | The server keeps the tree valid by leaving the offending folder where it is, or one level up, and tells the device | C8 | S13, S14 |
| The same abbreviation on two devices | Both snippets are kept. The one created first holds the abbreviation on every device; the other is listed in library health | C9, D13 | S09, S25, S48 |
| The same abbreviation in a personal and a shared folder, or in two shared folders | The user's own always holds it. Between shared folders the one shared with the user first, by the server's time | C9, D13 | S10, S11 |
| Tag renamed while it is put on a snippet; folder renamed while moved | Tag and link are different rows: both apply. Name and parent of a folder are one row: the later write wins, with a conflict | C2, C3, C10 | S12, S17 |
| Tag deleted while it is put on a snippet | The link is kept and has no effect while the tag is deleted | C10 | S18 |
| Attachments added on two devices | Two attachment rows, one stored file; nothing to resolve | C19 | S21 |
| An attachment added to a snippet that is deleted elsewhere | The snippet's row decides (delete against edit); the attachment row and the link follow it, and an attachment nothing uses is removed after 30 days | C5, C10, C19 | S19, S20 |
| Membership or permission changes racing an edit | The permission at the moment the edit arrives counts. A refused edit is kept on the device for the user | C12, C16, D5 | S30, S31, S36, S39 |
| Shares racing each other | One row per folder and group; a stale change never wins; a delete always applies | C11 | S37 |
| An invitation accepted while it is withdrawn; ownership handed over while the old owner acts | The call that commits first decides; the other is refused with a reason | C16 | S32, S33, S34 |
| A group deleted with unsent changes on a member's device | The group's folders are revoked and purged; the unsent changes are kept on the device for the user | C16, D6 | S35 |
| Offline for less than 30 days | Nothing special: tombstones and the feed are still there | C22 | S22 |
| Offline for more than 30 days | The device resyncs. Its changes to rows that were deleted and purged meanwhile are refused and kept for the user; everything else is pushed | C13, C22, D7 | S23, S29 |
| Clock skew, ahead or behind | Does not affect which write wins. It can affect which of two duplicate abbreviations holds, the same way on every device | C1, D13 | S24, S25 |
| The same change sent twice | One revision | C15 | S41, S42 |
| The server restored from a backup | Devices send back what it lost | C18 | S43 |
| Device and server on different protocol versions | The older side stops syncing and keeps working locally until it is updated | C23, D12 | S44 |

## 11. Privacy and security

### 11.1 What the server holds and who can read it

The server stores, per account: the email address and a display name; the devices' names,
platforms and app versions; every synced row, which includes the abbreviations, labels and
content of snippets and the names of folders and tags; the files of attachments; memberships
and invitations (with the invited address); replaced versions for 90 days and conflicts; the
feed, receipts and rate counters, which hold ids and counts.

- **In transit** everything goes over TLS. **At rest** the database, the files and the backups
  are on volumes the hosting provider encrypts. That protects a stolen disk. It does not protect
  against whoever can query the running database: **the operator of the server, and Supabase as
  its processor, can read synced snippets.** Iamb's privacy text must say this in these words.
- **Not end-to-end encrypted in version 1**, and why this does not break architecture §8. Rule 1
  and the threat model of §8 are about typed text: it never leaves the machine, and nothing
  here changes that, because typed text is in no synced row. A snippet is text the user chose
  to save and to sync. Encrypting it so that the server cannot read it is possible in this
  design, since the server never interprets content (C9), but it is not free: sign-in has no
  password to derive a key from, so losing every device would lose the library unless the user
  kept a recovery key; a shared folder needs a key per folder, wrapped for each member, which
  only a device that already has it can hand to a new member, so a folder could not appear
  within 10 seconds unless such a device is online; removing a member means a new key and
  re-encrypting; and the server could no longer check an attachment's hash. It is open
  question 2 (§13). If it is added later it is a mode of a profile: the content columns
  hold ciphertext, C4 and C17 compare a hash the device sends, and nothing else in §8 changes.
- **Logs.** The functions log ids, counts and codes, never arguments or rows; statement logging
  of function arguments stays off on the server. The agent's log on the device records message
  names, counts, durations and codes, never content, names or addresses (architecture §6.1).
- **Deleting an account** (`delete_account`) is refused while the user owns a group that has
  other members. Otherwise it ends every session at once, deletes the groups the user owns,
  ends their memberships, and removes their personal rows, history and files from the database
  without the 30 days (§13). The devices keep their local library, which is a local library
  again (D14).

### 11.2 What never leaves the device

| Never sent | Where it lives |
| --- | --- |
| Typed characters | Three places in locked memory in the service, for moments (architecture §8 rule 1); the sync agent never sees a key event |
| Fill-in values, clipboard content, palette queries, dry-run input | The service's and the UI host's memory (§8 rule 2) |
| How often and when a snippet is used | `usage`, device-local (ADR-0015) |
| The log of why something did not expand | The service's memory (§8 rule 4) |
| Settings, shortcuts, per-app injection settings, excluded apps | `setting`, `app_exclusion`, device-local |
| Which received snippets the user approved | `approval`, device-local (ADR-0022) |
| Changes the server refused, and rows it cannot store yet | `kept_change`, the blocked outbox entries and their rows |
| The computer's host name, the list of applications, anything about the focused window | Not read by the agent at all |

`iamb-service` still has no network code (ADR-0008): the four checks of architecture §2 hold
unchanged. The agent connects to the service with the role `sync`, which may call the Sync
group only (architecture §6.2): it can read the outbox and write rows, and it cannot ask for
diagnostics, forms, settings or status. What it holds in memory is rows on their way and the
tokens; what it writes to disk is attachment files and its log.

### 11.3 Threats around sharing

| Who | What they can do | What limits it | What is left |
| --- | --- | --- | --- |
| A member who may only use a folder | Read the shared tree (§4.4) | No table accepts a direct write; `push` refuses them (C12); nothing outside the tree is readable, and a refused read looks like a row or file that does not exist | They can copy what they can read |
| A member who may change a folder | Add, change and delete snippets and sub-folders in the tree; add attachments | On every other member's device, content they wrote that presses keys, waits, reads the clipboard or has several lines is held until approved (D10), under the author the server recorded, which no client can set (C17); a shared snippet can never take over one of a member's own abbreviations (D13); deletes are tombstones for 30 days; wrong bytes cannot be stored under a hash (§6.11); they cannot move content out of the tree, share it onward or change who sees it (§4.3); the owner's plan limits and the rate limits bound how much they can add | Single-line text they write expands on the others' devices without review (ADR-0022 accepts this); they can delete the tree's content, which the owner must restore within 30 days |
| A removed member, or one whose share ended | Nothing new from the moment the removal commits: every policy and every function refuses them, and unsent changes are refused (C12). Their devices purge the content at the next sync, and at the latest at the daily access check (D6, D7) | — | What their devices already held they may have copied, and a device that never comes online again keeps its copy. The server cannot erase a device. The share dialog should say so |
| Someone who holds a member's unlocked device | Read the library on it, and act as that member | The owner of the account revokes the device from another device, or signs out everywhere: the device's session ends, and because every call and every policy checks the device row, access stops at once and not when a token expires. Then they change what must change | The library on that device stays readable to whoever holds it; malware running as the user is outside the model (architecture §8) |
| Someone who stole a refresh token | Read and write as the user | Tokens are in the keychain and only `iamb-sync` reads them; sign-in uses PKCE and a loopback address open for seconds (ADR-0020); refresh tokens are single-use, and the reuse of an old one ends the session for the thief and the user alike, which the user sees as being signed out; access tokens live one hour at most; the device list shows each device's last call; "sign out everywhere" ends all sessions | Until the user notices, the thief has the user's access. Required server settings: refresh-token rotation with reuse detection on, and access tokens of one hour. A device that has not called for 180 days loses its session through the daily job (§5.12); Supabase's own limits on a session's lifetime need a paid hosting plan and are not relied on |
| Someone on the network | See that a device talks to the server | TLS; the live channel carries a number, no content | — |
| Another user of the same computer | Nothing | The data root is the user's alone; the endpoints check the peer (architecture §3.3, §6.1) | — |
| The server's operator, or someone who breaks into the server | Read every synced row; change rows, including who is recorded as an author | Hosting controls, and task 4.3 | In version 1 the server is trusted, for reading and for the author it reports. End-to-end encryption with signed content would end that trust (§13) |
| Someone who sends invitations at random | Make people receive an email | An invitation needs the right to invite and is rate-limited; the email carries no link that accepts; accepting shares nothing of the person's own; only the verified address can accept | — |

### 11.4 Review before use

ADR-0022 holds a received snippet until the user approves it. Sync supplies what that needs: the
author of the content, set by the server (C17) and stored on the device in `snippet_origin`; the
time of the change (`content_at`); the author's display name (`known_user`); and the content the
device was last allowed to type (`approved_content`), which is what keeps expanding and what the
review dialog compares with. A snippet is judged by its content, not by its revision number
(D10): a move, a new label or a switched-off snippet does not ask again, and no write by anyone
else can turn a member's key presses into the reader's own (S40). The user's own content is
never held, on any of their devices. Restoring a version from a conflict makes the restorer its
author.

### 11.5 What the policies and functions must guarantee

The list task 4.3 tests, each as the kinds of user of §4 (owner, admin, member with `edit`,
member with `view`, former member, stranger, signed out).

| | Guarantee |
| --- | --- |
| G1 | `folder_access` equals §4.2 after every commit (compared with the slow reference function, §5.4) |
| G2 | A user reads a folder, a snippet, a link, a share only under §5.13; every table of §4.5 returns exactly the rows listed there, and nothing after the share ends |
| G3 | A tag or an attachment of someone else's profile is readable only while a readable snippet has a live link to it, or by the user who wrote the row last (so that the creator of a new tag or attachment can see it before it is linked) |
| G4 | No role a device can have may insert, update or delete in any table of `iamb`, or call anything in `iamb_private`. `anon` can read nothing and call nothing |
| G5 | `history` is unreadable directly; a history version is returned only by `get_conflict` to a party of the conflict who may still read the row |
| G6 | `feed`, `feed_head`, `device`, `folder_access` and `conflict` rows are readable by their user only |
| G7 | Every write of §4.3 succeeds for the users that table allows and is `rejected: forbidden` for all others, judged at arrival (C12): after a membership, role or share change commits, the next push sees it |
| G8 | Every account call of §4.1 succeeds for the roles that table allows and fails for all others; a group always has exactly one owner |
| G9 | `updated_by`, `updated_device`, `content_by`, `rev`, `updated_at` and `deleted_at` are set by the server; nothing a client sends changes them (C17) |
| G10 | A profile, membership or invitation cannot be written by `push` (C16) |
| G11 | An invitation is readable by the group's owner and admins and by the account whose verified address it names, and only that account can accept it |
| G12 | A stale change to a share is never applied, and a share's delete always is (C11) |
| G13 | After a member is removed, a share deleted or a group deleted, the same transaction has removed the access entries: the former reader's next query returns no row of that content, and their feed holds `folder_revoked` |
| G14 | A stored file is readable only as `<profile>/<sha256>` of a verified attachment the user may read; a request for any other name answers exactly like one for a file that does not exist; a file can be written once, by the uploader `attachment_upload_begin` named, and is readable only after the server verified its hash |
| G15 | A revoked device, and a session without a device row, is refused by every function and reads no row through any policy |
| G16 | The live channel delivers to a user only the topic of their own id and accepts no message from a client |
| G17 | Functions that write are `security definer` with a fixed `search_path`, take the user and the session from the token only, and build no statement from their arguments |
| G18 | Size, count and rate limits are enforced by the server whatever the client sends (§5.2, §6.8) |
| G19 | A repeated push writes once (C15), also when two copies arrive at the same time |
| G20 | No function's log line and no error text contains row content |

## 12. Conflict scenarios

Each scenario gives the starting state, the steps in order, and the defined outcome: the state
of every row on the server and on every device, the history and the conflicts the server holds,
and what each device shows its user. They are written out here from
[`spec/sync/scenarios.toml`](../spec/sync/scenarios.toml), which is the source
(`python3 spec/sync/render_scenarios.py` renders this section; `--check` fails when it is out of
date). The format is documented in `crates/iamb-testkit/src/sync_scenarios.rs`, and
`cargo test -p iamb-testkit --test sync_scenarios` fails when there are fewer than 30 scenarios,
when an id is used twice, when a scenario does not state every row for the server and for every
device, when a conflict, the history and the rows contradict each other, when a case the task
asks for is covered by no scenario, or when this section lacks a scenario or a rule it cites.

How to read one:

- Devices start signed in, online and in step with the server unless the start says otherwise.
  An online device syncs after every step that concerns it; `offline` and `online` bracket work
  done without the server; `manual` leaves the network up but lets the scenario say when the
  device pulls and pushes.
- A version is named after the step that wrote it (`a1`), and `start` is what a row holds at the
  start. In an outcome, `live · b1` means the row is live in the revision the step `b1` wrote;
  `deleted · b1` that `b1` deleted it; `absent` that no row with that id is there (never
  received, purged, or not visible to that user; for an invitation: not listed for that user).
  `=` in a device's column means "as on the server".
- "Shown" is what the device shows its user, as a code: `conflict` (a conflict is listed and
  marked on the row), `kept` (changes that were not synced are kept on this computer), `blocked`
  (a row cannot be synced yet), `shared` and `revoked` (a folder arrived or went), `duplicate`
  and `broken` (library health), `review` (a snippet waits for review), `adjusted` (a folder was
  placed elsewhere), `superseded` (a sharing change was not made), `plan_limit`, `upgrade`,
  `invitation_invalid`, `refused` (an account call was refused, with its reason). The wording is
  task 4.5's; none of them is a notice over another application (UX spec §4.11).
- The two-device simulation of task 4.4 replays every scenario against the real store and sync
  client and a model of the server, and compares each outcome field by field; its 1,000 random
  runs check the invariants of §1 instead of fixed outcomes.

<!-- scenarios:begin (rendered by spec/sync/render_scenarios.py; do not edit) -->

| Scenario | Title | Category | Covers |
| --- | --- | --- | --- |
| S01 | Two devices change the same field of one snippet | edit | edit-same-field |
| S02 | Two devices change different fields of one snippet | edit | edit-different-fields |
| S03 | An edit and a delete of one snippet, the delete arrives last | delete | edit-vs-delete-delete-last |
| S04 | An edit and a delete of one snippet, the edit arrives last | delete | edit-vs-delete-edit-last |
| S05 | Both devices delete the same snippet | delete | delete-vs-delete |
| S06 | One device moves a snippet, the other edits it | edit | move-vs-edit |
| S07 | The user restores the other version from a conflict | edit | resolve-conflict |
| S08 | One device reorders a folder, the other renames it | structure | reorder-vs-rename |
| S09 | Two devices create a snippet with the same abbreviation | abbreviation | same-abbreviation-personal |
| S10 | A personal snippet and a shared snippet have the same abbreviation | abbreviation | same-abbreviation-personal-vs-shared |
| S11 | Two shared folders hold the same abbreviation | abbreviation | same-abbreviation-across-shares |
| S12 | One device renames a folder, the other moves it | structure | folder-rename-vs-move |
| S13 | Two devices move two folders into each other | structure | folder-cycle |
| S14 | A new sub-folder would be a fifth level after a move elsewhere | structure | folder-depth |
| S15 | A folder is deleted, then a snippet created in it arrives | structure | folder-delete-vs-create |
| S16 | A snippet is created in a folder, then the folder's delete arrives | structure | folder-delete-cascade |
| S17 | One device renames a tag, the other puts it on a snippet | tags | tag-rename-vs-assignment |
| S18 | One device deletes a tag, the other puts it on a snippet | tags | tag-delete-vs-assignment |
| S19 | An image is added to a snippet that was deleted elsewhere, the edit arrives last | attachments | attachment-vs-snippet-delete |
| S20 | An image is added to a snippet that was deleted elsewhere, the delete arrives last | attachments | attachment-vs-snippet-delete |
| S21 | The same image is attached on two devices | attachments | attachments-two-devices |
| S22 | A device comes back after 20 days offline | offline | long-offline-inside-window |
| S23 | A device comes back after 45 days offline | offline | long-offline-beyond-window |
| S24 | The device whose clock runs ahead does not win by its timestamp | clock | clock-ahead |
| S25 | The device whose clock runs behind still wins by arriving last | clock | clock-behind |
| S26 | The user edits a snippet again while its push is in flight | transport | edit-during-push |
| S27 | Many offline changes to the same rows are sent as one change per row | offline | offline-burst |
| S28 | A family is given exactly one folder and nothing else | sharing | exactly-one-folder |
| S29 | A device was offline when a folder stopped being shared | sharing | missed-folder-revoked |
| S30 | A member is removed while their device holds unsent changes | membership | member-removed-with-pending |
| S31 | A member who may only use a folder tries to change it | membership | viewer-write |
| S32 | An invitation is accepted just before the inviter withdraws it | membership | invitation-accept-vs-withdraw |
| S33 | An invitation is withdrawn just before it is accepted | membership | invitation-accept-vs-withdraw |
| S34 | The owner of a team hands the team to another member | membership | ownership-transfer |
| S35 | A group is deleted while a member holds unsent changes | membership | profile-deletion |
| S36 | A folder is shared with two groups and one share ends | sharing | two-shares-same-folder |
| S37 | A stale device shares a folder that was unshared in the meantime | sharing | share-stale-put, two-shares-same-folder |
| S38 | A snippet includes a snippet from a folder the user loses | sharing | include-loses-access |
| S39 | An admin is made a member while their device holds an unsent edit | membership | membership-racing-edit |
| S40 | A member adds key presses to a shared snippet, and the owner's own edit does not hide that | sharing | review-hold |
| S41 | The answer to a push is lost and the push is sent again | transport | replayed-push |
| S42 | The network delivers the same push twice | transport | duplicated-push |
| S43 | The server is restored from a backup | server | server-restore |
| S44 | A device speaks a protocol version the server no longer accepts | server | version-mismatch |
| S45 | One row of a push is too large to store | transport | partial-failure |
| S46 | The plan does not allow another shared folder | sharing | plan-limit |
| S47 | The live channel is down | transport | channel-down |
| S48 | A second computer with its own snippets signs in for the first time | server | first-sign-in |
| S49 | The user signs out with an unsent change and signs in again | offline | sign-out |

#### S01 · Two devices change the same field of one snippet

Category: edit. Covers: edit-same-field. Rules: C1, C2, C3, D2, D4.

**Start.** Ines has two devices, both in step. The snippet s1 is in her personal folder f1.

Devices: A (ines), B (ines). Profiles: `p-ines` (personal, ines).

| Row | Kind | Refers to | Rev | Fields |
| --- | --- | --- | --- | --- |
| `f1` | folder | profile `p-ines` | 3 | name `Replies` |
| `s1` | snippet | folder `f1` | 5 | abbreviation `;rty`, label `Thanks`, body `Thanks for your order.` |

**Steps.**

1. A goes offline.
2. B goes offline.
3. `a1` · A edits `s1`: body `Thank you for your order.`.
4. `b1` · B edits `s1`: body `Thanks a lot for your order.`.
5. A comes back online and syncs. Answer: `s1` `applied`.
6. B comes back online and syncs. Answer: `s1` `applied+conflict`. *B pulls first. The remote a1 is
   not written over B's unsent change (D2). B's push is based on the revision before a1, so it is
   stale, and it still wins (C2, C3).*

**Outcome.** The change that reached the server last, b1, is the snippet on the server and on both
devices. a1 is kept in the history, and a conflict on both devices offers it back.

| Row | Server | A | B |
| --- | --- | --- | --- |
| `f1` | live · start | = | = |
| `s1` | live · b1 | = | = |

- History (replaced versions the server keeps): `s1` start, a1.
- Conflicts: `s1` edit_edit, winner b1, loser a1, shown to ines, open.
- A: shown conflict.
- B: shown conflict.

#### S02 · Two devices change different fields of one snippet

Category: edit. Covers: edit-different-fields. Rules: C2, C3, D2.

**Start.** As S01. Device A changes the label, device B the content.

Devices: A (ines), B (ines). Profiles: `p-ines` (personal, ines).

| Row | Kind | Refers to | Rev | Fields |
| --- | --- | --- | --- | --- |
| `f1` | folder | profile `p-ines` | 3 | name `Replies` |
| `s1` | snippet | folder `f1` | 5 | abbreviation `;rty`, label `Thanks`, body `Thanks for your order.` |

**Steps.**

1. A goes offline.
2. B goes offline.
3. `a1` · A edits `s1`: label `Thanks for the order`.
4. `b1` · B edits `s1`: body `Thanks a lot for your order.`.
5. A comes back online and syncs. Answer: `s1` `applied`.
6. B comes back online and syncs. Answer: `s1` `applied+conflict`. *A push carries the whole row.
   B's row still has the old label, so the label change of a1 is replaced with the rest (C2: no
   merging of fields).*

**Outcome.** The row is B's: the new content with the old label. A's label change is in the history
and a conflict offers A's version back. Nothing is merged field by field.

| Row | Server | A | B |
| --- | --- | --- | --- |
| `f1` | live · start | = | = |
| `s1` | live · b1 (label `Thanks`, body `Thanks a lot for your order.`) | = | = |

- History (replaced versions the server keeps): `s1` start, a1.
- Conflicts: `s1` edit_edit, winner b1, loser a1, shown to ines, open.
- A: shown conflict.
- B: shown conflict.

#### S03 · An edit and a delete of one snippet, the delete arrives last

Category: delete. Covers: edit-vs-delete-delete-last. Rules: C2, C3, C5, D2.

**Start.** As S01. Device A edits the snippet while device B deletes it.

Devices: A (ines), B (ines). Profiles: `p-ines` (personal, ines).

| Row | Kind | Refers to | Rev | Fields |
| --- | --- | --- | --- | --- |
| `f1` | folder | profile `p-ines` | 3 | name `Replies` |
| `s1` | snippet | folder `f1` | 5 | abbreviation `;rty`, body `Thanks for your order.` |

**Steps.**

1. A goes offline.
2. B goes offline.
3. `a1` · A edits `s1`: body `Thank you for your order.`.
4. `b1` · B deletes `s1`.
5. A comes back online and syncs. Answer: `s1` `applied`.
6. B comes back online and syncs. Answer: `s1` `applied+conflict`. *The delete was made without
   knowing a1. A delete is a write like any other: it arrives last, so it wins (C5).*

**Outcome.** The snippet is deleted everywhere. The edited version a1 is in the history, and a
conflict on both devices offers to bring it back; the tombstone stays for 30 days.

| Row | Server | A | B |
| --- | --- | --- | --- |
| `f1` | live · start | = | = |
| `s1` | deleted · b1 | = | = |

- History (replaced versions the server keeps): `s1` start, a1.
- Conflicts: `s1` delete_over_edit, winner b1, loser a1, shown to ines, open.
- A: shown conflict.
- B: shown conflict.

#### S04 · An edit and a delete of one snippet, the edit arrives last

Category: delete. Covers: edit-vs-delete-edit-last. Rules: C2, C3, C5, D2.

**Start.** As S03, but the deleting device reaches the server first.

Devices: A (ines), B (ines). Profiles: `p-ines` (personal, ines).

| Row | Kind | Refers to | Rev | Fields |
| --- | --- | --- | --- | --- |
| `f1` | folder | profile `p-ines` | 3 | name `Replies` |
| `s1` | snippet | folder `f1` | 5 | abbreviation `;rty`, body `Thanks for your order.` |

**Steps.**

1. A goes offline.
2. B goes offline.
3. `a1` · A edits `s1`: body `Thank you for your order.`.
4. `b1` · B deletes `s1`.
5. B comes back online and syncs. Answer: `s1` `applied`.
6. A comes back online and syncs. Answer: `s1` `applied+conflict`. *A pulls the tombstone but does
   not apply it over its unsent edit (D2). The edit arrives last and brings the snippet back (C5).*

**Outcome.** The snippet is live everywhere with A's content. The conflict tells the user that it
had been deleted on the other device, and offers to delete it after all.

| Row | Server | A | B |
| --- | --- | --- | --- |
| `f1` | live · start | = | = |
| `s1` | live · a1 | = | = |

- History (replaced versions the server keeps): `s1` start, b1.
- Conflicts: `s1` edit_over_delete, winner a1, loser b1, shown to ines, open.
- A: shown conflict.
- B: shown conflict.

#### S05 · Both devices delete the same snippet

Category: delete. Covers: delete-vs-delete. Rules: C4, C5, D4.

**Start.** As S01. Both devices delete the snippet while offline.

Devices: A (ines), B (ines). Profiles: `p-ines` (personal, ines).

| Row | Kind | Refers to | Rev | Fields |
| --- | --- | --- | --- | --- |
| `f1` | folder | profile `p-ines` | 3 | name `Replies` |
| `s1` | snippet | folder `f1` | 5 | abbreviation `;rty`, body `Thanks for your order.` |

**Steps.**

1. A goes offline.
2. B goes offline.
3. `a1` · A deletes `s1`.
4. `b1` · B deletes `s1`.
5. A comes back online and syncs. Answer: `s1` `applied`.
6. B comes back online and syncs. Answer: `s1` `unchanged`. *The row is already deleted: the second
   delete changes nothing, makes no revision and no conflict (C4). B takes the revision of a1.*

**Outcome.** The snippet is deleted once, by a1. No conflict is recorded and nothing is shown.

| Row | Server | A | B |
| --- | --- | --- | --- |
| `f1` | live · start | = | = |
| `s1` | deleted · a1 | = | = |

- History (replaced versions the server keeps): `s1` start.
- Conflicts: none.
- A: shown nothing.
- B: shown nothing.

#### S06 · One device moves a snippet, the other edits it

Category: edit. Covers: move-vs-edit. Rules: C2, C3, D2.

**Start.** Ines has two folders. Device A moves s1 from f1 to f2 while device B changes its content.

Devices: A (ines), B (ines). Profiles: `p-ines` (personal, ines).

| Row | Kind | Refers to | Rev | Fields |
| --- | --- | --- | --- | --- |
| `f1` | folder | profile `p-ines` | 3 | name `Replies` |
| `f2` | folder | profile `p-ines` | 4 | name `Orders` |
| `s1` | snippet | folder `f1` | 5 | abbreviation `;rty`, body `Thanks for your order.` |

**Steps.**

1. A goes offline.
2. B goes offline.
3. `a1` · A moves `s1` to `f2`.
4. `b1` · B edits `s1`: body `Thanks a lot for your order.`.
5. A comes back online and syncs. Answer: `s1` `applied`.
6. B comes back online and syncs. Answer: `s1` `applied+conflict`. *The folder is a field of the
   snippet's row. B's row still names f1, so the move is replaced with the rest.*

**Outcome.** The snippet has B's content and is back in f1. The moved version is in the history; the
conflict offers it back, and the user can also simply move the snippet again.

| Row | Server | A | B |
| --- | --- | --- | --- |
| `f1` | live · start | = | = |
| `f2` | live · start | = | = |
| `s1` | live · b1 (folder `f1`) | = | = |

- History (replaced versions the server keeps): `s1` start, a1.
- Conflicts: `s1` edit_edit, winner b1, loser a1, shown to ines, open.
- A: shown conflict.
- B: shown conflict.

#### S07 · The user restores the other version from a conflict

Category: edit. Covers: resolve-conflict. Rules: C2, C3, C20.

**Start.** As S01, and then Ines chooses the other version on device A.

Devices: A (ines), B (ines). Profiles: `p-ines` (personal, ines).

| Row | Kind | Refers to | Rev | Fields |
| --- | --- | --- | --- | --- |
| `f1` | folder | profile `p-ines` | 3 | name `Replies` |
| `s1` | snippet | folder `f1` | 5 | abbreviation `;rty`, body `Thanks for your order.` |

**Steps.**

1. A goes offline.
2. B goes offline.
3. `a1` · A edits `s1`: body `Thank you for your order.`.
4. `b1` · B edits `s1`: body `Thanks a lot for your order.`.
5. A comes back online and syncs. Answer: `s1` `applied`.
6. B comes back online and syncs. Answer: `s1` `applied+conflict`.
7. `r1` · A: the user restores the other version in the conflict on `s1`. Answer: `ok`. *The server
   writes the content of a1 as a new revision by Ines and closes the conflict. It is an ordinary
   write: it reaches every device through the feed.*

**Outcome.** The snippet has the content of a1 again, as a new revision r1. b1 joined the history.
The conflict is resolved and no longer listed.

| Row | Server | A | B |
| --- | --- | --- | --- |
| `f1` | live · start | = | = |
| `s1` | live · r1 (body `Thank you for your order.`) | = | = |

- History (replaced versions the server keeps): `s1` start, a1, b1.
- Conflicts: `s1` edit_edit, winner b1, loser a1, shown to ines, resolved.
- A: shown conflict.
- B: shown conflict.

#### S08 · One device reorders a folder, the other renames it

Category: structure. Covers: reorder-vs-rename. Rules: C2, C3.

**Start.** Ines has two top-level folders. Device A drags f2 above f1 (its position changes), device
B renames f2.

Devices: A (ines), B (ines). Profiles: `p-ines` (personal, ines).

| Row | Kind | Refers to | Rev | Fields |
| --- | --- | --- | --- | --- |
| `f1` | folder | profile `p-ines` | 3 | name `Replies`, position `0` |
| `f2` | folder | profile `p-ines` | 4 | name `Orders`, position `1` |

**Steps.**

1. A goes offline.
2. B goes offline.
3. `a1` · A edits `f2`: position `-1`.
4. `b1` · B edits `f2`: name `Order replies`.
5. A comes back online and syncs. Answer: `f2` `applied`.
6. B comes back online and syncs. Answer: `f2` `applied`. *The push is stale, but the only thing
   changed since its base is the position, which is arrangement and not a recorded field: no
   conflict is recorded (C3).*

**Outcome.** The folder has the new name and its old position: the rename wins the whole row and the
reordering of this folder is undone, without a conflict. Both versions are in the history.

| Row | Server | A | B |
| --- | --- | --- | --- |
| `f1` | live · start | = | = |
| `f2` | live · b1 (name `Order replies`, position `1`) | = | = |

- History (replaced versions the server keeps): `f2` start, a1.
- Conflicts: none.
- A: shown nothing.
- B: shown nothing.

#### S09 · Two devices create a snippet with the same abbreviation

Category: abbreviation. Covers: same-abbreviation-personal. Rules: C9, D13.

**Start.** Ines has two devices, both in step, and no snippet with the abbreviation ;sig. Each
device creates one while offline; A does it first.

Devices: A (ines), B (ines). Profiles: `p-ines` (personal, ines).

| Row | Kind | Refers to | Rev | Fields |
| --- | --- | --- | --- | --- |
| `f1` | folder | profile `p-ines` | 3 | name `Replies` |

**Steps.**

1. A goes offline.
2. B goes offline.
3. `a1` · A creates the snippet `s2` (folder `f1`): abbreviation `;sig`, body `Kind regards, Ines`.
4. `b1` · B creates the snippet `s3` (folder `f1`): abbreviation `;SIG`, body `Best, Ines
   Marchetti`.
5. A comes back online and syncs. Answer: `s2` `applied`.
6. B comes back online and syncs. Answer: `s3` `applied`. *They are two rows, so nothing is replaced
   and no conflict is recorded. The server does not compare abbreviations (C9).*

**Outcome.** Both snippets exist everywhere. On both devices the one created first, s2, holds the
abbreviation; s3 neither expands nor resolves and library health lists it under "abbreviation taken"
until the user renames or deletes one.

| Row | Server | A | B |
| --- | --- | --- | --- |
| `f1` | live · start | = | = |
| `s2` | live · a1 | = | = |
| `s3` | live · b1 | = | = |

- History (replaced versions the server keeps): none.
- Conflicts: none.
- A: shown duplicate; expands `;sig` as `s2`.
- B: shown duplicate; expands `;sig` as `s2`.

#### S10 · A personal snippet and a shared snippet have the same abbreviation

Category: abbreviation. Covers: same-abbreviation-personal-vs-shared. Rules: C9, C21, D6, D13.

**Start.** Ines and Tom are in the family group. Tom's folder tf1 holds ;hrs and is not shared yet.
Ines, offline, creates her own ;hrs while Tom shares tf1 with the group.

Devices: A (ines), C (tom). Profiles: `p-ines` (personal, ines), `p-tom` (personal, tom), `g-fam`
(family, ines).

| Row | Kind | Refers to | Rev | Fields |
| --- | --- | --- | --- | --- |
| `m-ines` | membership | group `g-fam`, user `ines` | 2 | role `owner` |
| `m-tom` | membership | group `g-fam`, user `tom` | 6 | role `member` |
| `f1` | folder | profile `p-ines` | 3 | name `Replies` |
| `tf1` | folder | profile `p-tom` | 7 | name `Shop` |
| `ts1` | snippet | folder `tf1` | 8 | abbreviation `;hrs`, body `Open 9 to 5.` |

**Steps.**

1. A goes offline.
2. `a1` · A creates the snippet `s2` (folder `f1`): abbreviation `;hrs`, body `My hours: Tuesday to
   Friday.`.
3. `c1` · C creates the folder share `sh1` (folder `tf1`, group `g-fam`): permission `view`. Answer:
   `sh1` `applied`.
4. A comes back online and syncs. Answer: `s2` `applied`. *A's feed holds folder_visible(tf1): A
   fetches the folder whole (D6). Its own s2 is pushed as an ordinary new row.*

**Outcome.** On Ines's device her own snippet holds ;hrs and the shared one is listed in library
health ("Your own ;hrs is used instead"). On Tom's device, which cannot see s2, his snippet expands
as before.

| Row | Server | A | C |
| --- | --- | --- | --- |
| `m-ines` | live · start | = | = |
| `m-tom` | live · start | = | = |
| `f1` | live · start | = | absent |
| `tf1` | live · start | = | = |
| `ts1` | live · start | = | = |
| `s2` | live · a1 | = | absent |
| `sh1` | live · c1 | = | = |

- History (replaced versions the server keeps): none.
- Conflicts: none.
- A: shown shared, duplicate; expands `;hrs` as `s2`; permission `tf1` view.
- C: shown nothing; expands `;hrs` as `ts1`.

#### S11 · Two shared folders hold the same abbreviation

Category: abbreviation. Covers: same-abbreviation-across-shares. Rules: C9, C21, D6, D13.

**Start.** Ines, Tom and Jun are in the family group. Ines shares her folder f1, then Tom shares his
folder tf1; both hold a snippet ;hrs. Jun has no ;hrs of his own.

Devices: A (ines), C (tom), E (jun). Profiles: `p-ines` (personal, ines), `p-tom` (personal, tom),
`p-jun` (personal, jun), `g-fam` (family, ines).

| Row | Kind | Refers to | Rev | Fields |
| --- | --- | --- | --- | --- |
| `m-ines` | membership | group `g-fam`, user `ines` | 2 | role `owner` |
| `m-tom` | membership | group `g-fam`, user `tom` | 6 | role `member` |
| `m-jun` | membership | group `g-fam`, user `jun` | 9 | role `member` |
| `f1` | folder | profile `p-ines` | 3 | name `Replies` |
| `s1` | snippet | folder `f1` | 5 | abbreviation `;hrs`, body `Open 9 to 5.` |
| `tf1` | folder | profile `p-tom` | 7 | name `Shop` |
| `ts1` | snippet | folder `tf1` | 8 | abbreviation `;hrs`, body `Open 10 to 6.` |

**Steps.**

1. `a1` · A creates the folder share `sh-a` (folder `f1`, group `g-fam`): permission `view`. Answer:
   `sh-a` `applied`.
2. `c1` · C creates the folder share `sh-c` (folder `tf1`, group `g-fam`): permission `view`.
   Answer: `sh-c` `applied`.

**Outcome.** Everyone sees both folders. Ines and Tom each keep their own ;hrs (personal first). For
Jun the folder shared first, f1, holds the abbreviation; ts1 is listed in his library health. The
order comes from the server's time of each share, so no device clock can change it.

| Row | Server | A | C | E |
| --- | --- | --- | --- | --- |
| `m-ines` | live · start | = | = | = |
| `m-tom` | live · start | = | = | = |
| `m-jun` | live · start | = | = | = |
| `f1` | live · start | = | = | = |
| `s1` | live · start | = | = | = |
| `tf1` | live · start | = | = | = |
| `ts1` | live · start | = | = | = |
| `sh-a` | live · a1 | = | = | = |
| `sh-c` | live · c1 | = | = | = |

- History (replaced versions the server keeps): none.
- Conflicts: none.
- A: shown shared, duplicate; expands `;hrs` as `s1`.
- C: shown shared, duplicate; expands `;hrs` as `ts1`.
- E: shown shared, duplicate; expands `;hrs` as `s1`; permission `f1` view, `tf1` view.

#### S12 · One device renames a folder, the other moves it

Category: structure. Covers: folder-rename-vs-move. Rules: C2, C3, D2.

**Start.** Ines has folders f1 and f2 at the top and f3 inside f1. Device A renames f3 while device
B moves it into f2.

Devices: A (ines), B (ines). Profiles: `p-ines` (personal, ines).

| Row | Kind | Refers to | Rev | Fields |
| --- | --- | --- | --- | --- |
| `f1` | folder | profile `p-ines` | 3 | name `Replies` |
| `f2` | folder | profile `p-ines` | 4 | name `Orders` |
| `f3` | folder | profile `p-ines`, parent `f1` | 5 | name `Returns` |

**Steps.**

1. A goes offline.
2. B goes offline.
3. `a1` · A edits `f3`: name `Returns and refunds`.
4. `b1` · B moves `f3` to `f2`.
5. A comes back online and syncs. Answer: `f3` `applied`.
6. B comes back online and syncs. Answer: `f3` `applied+conflict`. *Name and parent are fields of
   the same row. The name was changed since B's base, so the conflict is recorded.*

**Outcome.** The folder is inside f2 with its old name. The renamed version is in the history and a
conflict offers it back; renaming again is as quick.

| Row | Server | A | B |
| --- | --- | --- | --- |
| `f1` | live · start | = | = |
| `f2` | live · start | = | = |
| `f3` | live · b1 (name `Returns`, parent `f2`) | = | = |

- History (replaced versions the server keeps): `f3` start, a1.
- Conflicts: `f3` edit_edit, winner b1, loser a1, shown to ines, open.
- A: shown conflict.
- B: shown conflict.

#### S13 · Two devices move two folders into each other

Category: structure. Covers: folder-cycle. Rules: C8, D3, D4.

**Start.** Ines has top-level folders fa and fb. Device A moves fa into fb while device B moves fb
into fa.

Devices: A (ines), B (ines). Profiles: `p-ines` (personal, ines).

| Row | Kind | Refers to | Rev | Fields |
| --- | --- | --- | --- | --- |
| `fa` | folder | profile `p-ines` | 3 | name `Replies` |
| `fb` | folder | profile `p-ines` | 4 | name `Orders` |

**Steps.**

1. A goes offline.
2. B goes offline.
3. `a1` · A moves `fa` to `fb`.
4. `b1` · B moves `fb` to `fa`.
5. A comes back online and syncs. Answer: `fa` `applied`.
6. B comes back online and syncs. Answer: `fb` `unchanged+adjusted`. *B's push of fb is not stale
   (nobody changed fb), but it would close a cycle. The server keeps fb's parent as it is and
   applies the rest of the row, which changes nothing (C8). B takes the server's row. Between its
   pull and this answer B's store holds the cycle; the view shows such folders at the top level.*

**Outcome.** fa is inside fb, and fb stays at the top level. B's move is not made, and B says so
("Orders could not be moved into Replies, which was moved into it on another device").

| Row | Server | A | B |
| --- | --- | --- | --- |
| `fa` | live · a1 (parent `fb`) | = | = |
| `fb` | live · start | = | = |

- History (replaced versions the server keeps): `fa` start.
- Conflicts: none.
- A: shown nothing.
- B: shown adjusted.

#### S14 · A new sub-folder would be a fifth level after a move elsewhere

Category: structure. Covers: folder-depth. Rules: C8, D4.

**Start.** Ines has f1 > f2 > f3 (three levels) and a top-level folder g1. Device A moves f1 into
g1, which makes f3 the fourth level. Device B, offline, creates f4 inside f3.

Devices: A (ines), B (ines). Profiles: `p-ines` (personal, ines).

| Row | Kind | Refers to | Rev | Fields |
| --- | --- | --- | --- | --- |
| `g1` | folder | profile `p-ines` | 2 | name `Shop` |
| `f1` | folder | profile `p-ines` | 3 | name `Replies` |
| `f2` | folder | profile `p-ines`, parent `f1` | 4 | name `Orders` |
| `f3` | folder | profile `p-ines`, parent `f2` | 5 | name `Returns` |

**Steps.**

1. B goes offline.
2. `b1` · B creates the folder `f4` (profile `p-ines`, parent `f3`): name `Refused`.
3. `a1` · A moves `f1` to `g1`. Answer: `f1` `applied`.
4. B comes back online and syncs. Answer: `f4` `applied+adjusted`. *Inside f3 the new folder would
   be a fifth level. The server puts it under the nearest ancestor that keeps four levels, f2 (C8),
   and B takes that row.*

**Outcome.** The new folder exists, beside f3 instead of inside it, and B says that it was placed
one level up. Nothing is lost and no tree is deeper than four levels.

| Row | Server | A | B |
| --- | --- | --- | --- |
| `g1` | live · start | = | = |
| `f1` | live · a1 (parent `g1`) | = | = |
| `f2` | live · start | = | = |
| `f3` | live · start | = | = |
| `f4` | live · b1 (parent `f2`) | = | = |

- History (replaced versions the server keeps): `f1` start.
- Conflicts: none.
- A: shown nothing.
- B: shown adjusted.

#### S15 · A folder is deleted, then a snippet created in it arrives

Category: structure. Covers: folder-delete-vs-create. Rules: C6, C7, D3.

**Start.** Ines's folder f1 holds s1 and s2. Device A deletes the folder. Device B, offline, creates
s3 in it.

Devices: A (ines), B (ines). Profiles: `p-ines` (personal, ines).

| Row | Kind | Refers to | Rev | Fields |
| --- | --- | --- | --- | --- |
| `f1` | folder | profile `p-ines` | 3 | name `Returns` |
| `s1` | snippet | folder `f1` | 5 | abbreviation `;rret`, body `Return label attached.` |
| `s2` | snippet | folder `f1` | 6 | abbreviation `;rno`, body `We cannot take it back.` |

**Steps.**

1. B goes offline.
2. `b1` · B creates the snippet `s3` (folder `f1`): abbreviation `;rwhen`, body `Refunds take five
   days.`.
3. `a1` · A deletes `f1`. Answer: `f1` `applied`. *The push names the rows the delete cascaded to on
   A (s1, s2) with their base revisions. The server deletes the whole subtree (C6).*
4. B comes back online and syncs. Answer: `s3` `applied`. *B applies the three tombstones (it has no
   unsent change to them). Its new snippet arrives in a deleted folder: the server restores the
   folder, which Ines may do, and records that on the folder (C7). What the delete had removed stays
   deleted.*

**Outcome.** The folder is back with the new snippet only; s1 and s2 stay deleted and can be
restored for 30 days. A conflict on the folder says that it was deleted on one device and used on
the other, and offers to delete it after all.

| Row | Server | A | B |
| --- | --- | --- | --- |
| `f1` | live · b1 | = | = |
| `s1` | deleted · a1 | = | = |
| `s2` | deleted · a1 | = | = |
| `s3` | live · b1 | = | = |

- History (replaced versions the server keeps): `f1` start, a1; `s1` start; `s2` start.
- Conflicts: `f1` folder_restored, winner b1, loser a1, shown to ines, open.
- A: shown conflict.
- B: shown conflict.

#### S16 · A snippet is created in a folder, then the folder's delete arrives

Category: structure. Covers: folder-delete-cascade. Rules: C6, D3.

**Start.** As S15, but the new snippet reaches the server first and the deleting device was offline.

Devices: A (ines), B (ines). Profiles: `p-ines` (personal, ines).

| Row | Kind | Refers to | Rev | Fields |
| --- | --- | --- | --- | --- |
| `f1` | folder | profile `p-ines` | 3 | name `Returns` |
| `s1` | snippet | folder `f1` | 5 | abbreviation `;rret`, body `Return label attached.` |
| `s2` | snippet | folder `f1` | 6 | abbreviation `;rno`, body `We cannot take it back.` |

**Steps.**

1. A goes offline.
2. `a1` · A deletes `f1`.
3. `b1` · B creates the snippet `s3` (folder `f1`): abbreviation `;rwhen`, body `Refunds take five
   days.`. Answer: `s3` `applied`.
4. A comes back online and syncs. Answer: `f1` `applied`. *A's cascade list names s1 and s2 only.
   The server deletes the whole subtree as it is now; s3 was never seen by the deleting device, so
   its deletion is recorded as a conflict (C6).*

**Outcome.** The folder and all three snippets are deleted. The conflict on s3 says that it was
deleted with its folder before this device had seen it, and offers to restore it, which also brings
the folder back.

| Row | Server | A | B |
| --- | --- | --- | --- |
| `f1` | deleted · a1 | = | = |
| `s1` | deleted · a1 | = | = |
| `s2` | deleted · a1 | = | = |
| `s3` | deleted · a1 | = | = |

- History (replaced versions the server keeps): `f1` start; `s1` start; `s2` start; `s3` b1.
- Conflicts: `s3` deleted_with_folder, winner a1, loser b1, shown to ines, open.
- A: shown conflict.
- B: shown conflict.

#### S17 · One device renames a tag, the other puts it on a snippet

Category: tags. Covers: tag-rename-vs-assignment. Rules: C2, C10.

**Start.** Ines has a tag t1 and a snippet s1 without tags. Device A renames the tag while device B
puts it on s1.

Devices: A (ines), B (ines). Profiles: `p-ines` (personal, ines).

| Row | Kind | Refers to | Rev | Fields |
| --- | --- | --- | --- | --- |
| `f1` | folder | profile `p-ines` | 3 | name `Replies` |
| `s1` | snippet | folder `f1` | 5 | abbreviation `;rty`, body `Thanks for your order.` |
| `t1` | tag | profile `p-ines` | 4 | name `urgent` |

**Steps.**

1. A goes offline.
2. B goes offline.
3. `a1` · A edits `t1`: name `priority`.
4. `b1` · B creates the snippet tag `l1` (snippet `s1`, tag `t1`).
5. A comes back online and syncs. Answer: `t1` `applied`.
6. B comes back online and syncs. Answer: `l1` `applied`. *The link is a row of its own that names
   the tag by id, so the two changes do not touch the same row (C10).*

**Outcome.** The tag is called "priority" and s1 carries it, on both devices. No conflict.

| Row | Server | A | B |
| --- | --- | --- | --- |
| `f1` | live · start | = | = |
| `s1` | live · start | = | = |
| `t1` | live · a1 | = | = |
| `l1` | live · b1 | = | = |

- History (replaced versions the server keeps): `t1` start.
- Conflicts: none.
- A: shown nothing.
- B: shown nothing.

#### S18 · One device deletes a tag, the other puts it on a snippet

Category: tags. Covers: tag-delete-vs-assignment. Rules: C5, C10, D3.

**Start.** As S17, but device A deletes the tag.

Devices: A (ines), B (ines). Profiles: `p-ines` (personal, ines).

| Row | Kind | Refers to | Rev | Fields |
| --- | --- | --- | --- | --- |
| `f1` | folder | profile `p-ines` | 3 | name `Replies` |
| `s1` | snippet | folder `f1` | 5 | abbreviation `;rty`, body `Thanks for your order.` |
| `t1` | tag | profile `p-ines` | 4 | name `urgent` |

**Steps.**

1. A goes offline.
2. B goes offline.
3. `a1` · A deletes `t1`.
4. `b1` · B creates the snippet tag `l1` (snippet `s1`, tag `t1`).
5. A comes back online and syncs. Answer: `t1` `applied`.
6. B comes back online and syncs. Answer: `l1` `applied`. *B applies the tag's tombstone. Its link
   is accepted although the tag is deleted: a link counts only while it and both its ends are live
   (C10).*

**Outcome.** The tag is deleted and s1 shows no tag on either device. The link row exists but has no
effect; it is purged with the tag, and it would count again if the tag were restored within 30 days.

| Row | Server | A | B |
| --- | --- | --- | --- |
| `f1` | live · start | = | = |
| `s1` | live · start | = | = |
| `t1` | deleted · a1 | = | = |
| `l1` | live · b1 | = | = |

- History (replaced versions the server keeps): `t1` start.
- Conflicts: none.
- A: shown nothing.
- B: shown nothing.

#### S19 · An image is added to a snippet that was deleted elsewhere, the edit arrives last

Category: attachments. Covers: attachment-vs-snippet-delete. Rules: C5, C10, C19, D2.

**Start.** Ines has a formatted snippet s1. Device A, offline, adds an image to it: an attachment
row, a changed body and a link. Device B deletes the snippet.

Devices: A (ines), B (ines). Profiles: `p-ines` (personal, ines).

| Row | Kind | Refers to | Rev | Fields |
| --- | --- | --- | --- | --- |
| `f1` | folder | profile `p-ines` | 3 | name `Replies` |
| `s1` | snippet | folder `f1` | 5 | abbreviation `;map`, kind `rich`, body `We are behind the bakery.` |

**Steps.**

1. A goes offline.
2. `a1` · A creates the attachment `at1` (profile `p-ines`): sha256 `sha-map`, mime `image/png`.
3. `a2` · A edits `s1`: body `We are behind the bakery. ![map](attachment:sha-map)`.
4. `a3` · A creates the snippet attachment `la1` (snippet `s1`, attachment `at1`).
5. `b1` · B deletes `s1`. Answer: `s1` `applied`.
6. A comes back online and syncs. Answer: `at1` `applied`, `s1` `applied+conflict`, `la1` `applied`.
   *The rows go first, in one request; the bytes are uploaded after the attachment row is accepted.
   The edit arrives after the delete and brings the snippet back (C5).*

**Outcome.** The snippet is live with the image on both devices; B downloads the bytes and checks
them against the hash. The conflict says that the snippet had been deleted on the other device.

| Row | Server | A | B |
| --- | --- | --- | --- |
| `f1` | live · start | = | = |
| `s1` | live · a2 | = | = |
| `at1` | live · a1 | = | = |
| `la1` | live · a3 | = | = |

- History (replaced versions the server keeps): `s1` start, b1.
- Conflicts: `s1` edit_over_delete, winner a2, loser b1, shown to ines, open.
- A: shown conflict.
- B: shown conflict.

#### S20 · An image is added to a snippet that was deleted elsewhere, the delete arrives last

Category: attachments. Covers: attachment-vs-snippet-delete. Rules: C5, C10, C19, D2.

**Start.** As S19, but the device that adds the image is online and the deleting device is offline.

Devices: A (ines), B (ines). Profiles: `p-ines` (personal, ines).

| Row | Kind | Refers to | Rev | Fields |
| --- | --- | --- | --- | --- |
| `f1` | folder | profile `p-ines` | 3 | name `Replies` |
| `s1` | snippet | folder `f1` | 5 | abbreviation `;map`, kind `rich`, body `We are behind the bakery.` |

**Steps.**

1. B goes offline.
2. `b1` · B deletes `s1`.
3. `a1` · A creates the attachment `at1` (profile `p-ines`): sha256 `sha-map`, mime `image/png`.
   Answer: `at1` `applied`.
4. `a2` · A edits `s1`: body `We are behind the bakery. ![map](attachment:sha-map)`. Answer: `s1`
   `applied`.
5. `a3` · A creates the snippet attachment `la1` (snippet `s1`, attachment `at1`). Answer: `la1`
   `applied`.
6. B comes back online and syncs. Answer: `s1` `applied+conflict`. *B applies the attachment row and
   the link, keeps its own delete over the remote edit (D2), and pushes the delete, which wins.*

**Outcome.** The snippet is deleted on both devices and the conflict offers the version with the
image back. The attachment row and the link stay as rows: the link has no effect while the snippet
is deleted, and an attachment that no live snippet uses for 30 days is removed with its bytes.

| Row | Server | A | B |
| --- | --- | --- | --- |
| `f1` | live · start | = | = |
| `s1` | deleted · b1 | = | = |
| `at1` | live · a1 | = | = |
| `la1` | live · a3 | = | = |

- History (replaced versions the server keeps): `s1` start, a2.
- Conflicts: `s1` delete_over_edit, winner b1, loser a2, shown to ines, open.
- A: shown conflict.
- B: shown conflict.

#### S21 · The same image is attached on two devices

Category: attachments. Covers: attachments-two-devices. Rules: C10, C19.

**Start.** Ines has two formatted snippets. Offline, device A adds an image to s1 and device B adds
the very same file to s2.

Devices: A (ines), B (ines). Profiles: `p-ines` (personal, ines).

| Row | Kind | Refers to | Rev | Fields |
| --- | --- | --- | --- | --- |
| `f1` | folder | profile `p-ines` | 3 | name `Replies` |
| `s1` | snippet | folder `f1` | 5 | abbreviation `;map`, kind `rich`, body `We are behind the bakery.` |
| `s2` | snippet | folder `f1` | 6 | abbreviation `;find`, kind `rich`, body `How to find us.` |

**Steps.**

1. A goes offline.
2. B goes offline.
3. `a1` · A creates the attachment `at-a` (profile `p-ines`): sha256 `sha-map`, mime `image/png`.
4. `a2` · A edits `s1`: body `We are behind the bakery. ![map](attachment:sha-map)`.
5. `a3` · A creates the snippet attachment `la` (snippet `s1`, attachment `at-a`).
6. `b1` · B creates the attachment `at-b` (profile `p-ines`): sha256 `sha-map`, mime `image/png`.
7. `b2` · B edits `s2`: body `How to find us. ![map](attachment:sha-map)`.
8. `b3` · B creates the snippet attachment `lb` (snippet `s2`, attachment `at-b`).
9. A comes back online and syncs. Answer: `at-a` `applied`, `s1` `applied`, `la` `applied`.
10. B comes back online and syncs. Answer: `at-b` `applied`, `s2` `applied`, `lb` `applied`. *Two
    attachment rows for one content are allowed (C19). The bytes are stored once under the profile
    and the hash: B's upload is answered "already there".*

**Outcome.** Both snippets show the image on both devices. There are two attachment rows and one
stored file. No conflict.

| Row | Server | A | B |
| --- | --- | --- | --- |
| `f1` | live · start | = | = |
| `s1` | live · a2 | = | = |
| `s2` | live · b2 | = | = |
| `at-a` | live · a1 | = | = |
| `at-b` | live · b1 | = | = |
| `la` | live · a3 | = | = |
| `lb` | live · b3 | = | = |

- History (replaced versions the server keeps): `s1` start; `s2` start.
- Conflicts: none.
- A: shown nothing.
- B: shown nothing.

#### S22 · A device comes back after 20 days offline

Category: offline. Covers: long-offline-inside-window. Rules: C3, C5, C22, D1, D2.

**Start.** Ines's folder f1 holds s1 and s2. Device B goes offline and edits s1, creates s3 and
deletes s2. Meanwhile device A deletes s1 and edits s2. B returns after 20 days, inside the 30 days
for which the server keeps tombstones and the feed.

Devices: A (ines), B (ines). Profiles: `p-ines` (personal, ines).

| Row | Kind | Refers to | Rev | Fields |
| --- | --- | --- | --- | --- |
| `f1` | folder | profile `p-ines` | 3 | name `Replies` |
| `s1` | snippet | folder `f1` | 5 | abbreviation `;rty`, body `Thanks for your order.` |
| `s2` | snippet | folder `f1` | 6 | abbreviation `;rlate`, body `Your order left us late.` |

**Steps.**

1. B goes offline.
2. `b1` · B edits `s1`: body `Thanks a lot for your order.`.
3. `b2` · B creates the snippet `s3` (folder `f1`): abbreviation `;rwhen`, body `It ships within
   five days.`.
4. `b3` · B deletes `s2`.
5. `a1` · A deletes `s1`. Answer: `s1` `applied`.
6. `a2` · A edits `s2`: body `Sorry, your order left us late.`. Answer: `s2` `applied`.
7. 20 days pass.
8. B comes back online and syncs. Answer: `s1` `applied+conflict`, `s3` `applied`, `s2`
   `applied+conflict`. *The feed still reaches back to B's position. B keeps its own versions of s1
   and s2 over the remote ones (D2) and pushes; both pushes are stale and both win.*

**Outcome.** s1 is live with B's content (it had been deleted on A), s2 is deleted (it had been
edited on A), s3 exists. Two conflicts offer the other versions. Being offline for 20 days changes
nothing about the rules.

| Row | Server | A | B |
| --- | --- | --- | --- |
| `f1` | live · start | = | = |
| `s1` | live · b1 | = | = |
| `s2` | deleted · b3 | = | = |
| `s3` | live · b2 | = | = |

- History (replaced versions the server keeps): `s1` start, a1; `s2` start, a2.
- Conflicts: `s1` edit_over_delete, winner b1, loser a1, shown to ines, open; `s2` delete_over_edit,
winner b3, loser a2, shown to ines, open.
- A: shown conflict.
- B: shown conflict.

#### S23 · A device comes back after 45 days offline

Category: offline. Covers: long-offline-beyond-window. Rules: C13, C22, D5, D7, D8.

**Start.** Ines has f1 with s1 and s2, and f2 with s4. Device B goes offline and edits s1, creates
s3 in f1 and edits s4. On the first day device A deletes s1 and the folder f2. B returns after 45
days: the server purged the deleted rows and their history on day 31, and B's feed position is older
than the feed reaches.

Devices: A (ines), B (ines). Profiles: `p-ines` (personal, ines).

| Row | Kind | Refers to | Rev | Fields |
| --- | --- | --- | --- | --- |
| `f1` | folder | profile `p-ines` | 3 | name `Replies` |
| `s1` | snippet | folder `f1` | 5 | abbreviation `;rty`, body `Thanks for your order.` |
| `s2` | snippet | folder `f1` | 6 | abbreviation `;rlate`, body `Your order left us late.` |
| `f2` | folder | profile `p-ines` | 7 | name `Returns` |
| `s4` | snippet | folder `f2` | 8 | abbreviation `;rret`, body `Return label attached.` |

**Steps.**

1. B goes offline.
2. `b1` · B edits `s1`: body `Thanks a lot for your order.`.
3. `b2` · B creates the snippet `s3` (folder `f1`): abbreviation `;rwhen`, body `It ships within
   five days.`.
4. `b3` · B edits `s4`: body `A prepaid return label is attached.`.
5. `a1` · A deletes `s1`. Answer: `s1` `applied`.
6. `a2` · A deletes `f2`. Answer: `f2` `applied`.
7. 45 days pass. *On day 31 the server's purge removes s1, f2 and s4 and their history for good.*
8. B comes back online and syncs. Answer: `reset_required:expired`, `s1` `rejected:gone`, `s3`
   `applied`, `s4` `rejected:gone`. *B's position is older than the feed, so B resyncs (D7): it
   fetches everything it may see and removes synced rows the server no longer has, except rows with
   unsent changes and the folders they need. Its pushes for s1 and s4 name rows the server does not
   know (C13). B keeps both changes for the user and removes the rows, and then f2.*

**Outcome.** The new snippet s3 is everywhere. The rows deleted more than 30 days ago stay gone on
the server; B's two changes to them are kept on B, in the list of changes that were not synced,
where the user can copy them into new snippets. A's own tombstones were purged on A after 30 days.

| Row | Server | A | B |
| --- | --- | --- | --- |
| `f1` | live · start | = | = |
| `s1` | absent | = | = |
| `s2` | live · start | = | = |
| `s3` | live · b2 | = | = |
| `f2` | absent | = | = |
| `s4` | absent | = | = |

- History (replaced versions the server keeps): none.
- Conflicts: none.
- A: shown nothing.
- B: shown kept; kept for the user: b1, b3.

#### S24 · The device whose clock runs ahead does not win by its timestamp

Category: clock. Covers: clock-ahead. Rules: C1, C2, C3.

**Start.** Device A's clock is two hours ahead. Offline, A edits s1 and creates s2; later in true
time device B edits s1. A reaches the server first.

Devices: A (ines), B (ines). Profiles: `p-ines` (personal, ines).

| Row | Kind | Refers to | Rev | Fields |
| --- | --- | --- | --- | --- |
| `f1` | folder | profile `p-ines` | 3 | name `Replies` |
| `s1` | snippet | folder `f1` | 5 | abbreviation `;rty`, body `Thanks for your order.` |

**Steps.**

1. A's clock is 2 hours ahead of true time.
2. A goes offline.
3. B goes offline.
4. `a1` · A edits `s1`: body `Thank you for your order.`.
5. `a2` · A creates the snippet `s2` (folder `f1`): abbreviation `;rlate`, body `Your order left us
   late.`.
6. `b1` · B edits `s1`: body `Thanks a lot for your order.`.
7. A comes back online and syncs. Answer: `s1` `applied`, `s2` `applied`. *The server stamps
   updated_at with its own time. It keeps the creation time a device gives, but never later than its
   own clock: s2's is cut back, and A takes the server's row.*
8. B comes back online and syncs. Answer: `s1` `applied+conflict`.

**Outcome.** b1 wins because it reached the server last, although A's own timestamp for a1 was two
hours later. s2 exists with a creation time no later than the server's. The conflict offers a1 back.

| Row | Server | A | B |
| --- | --- | --- | --- |
| `f1` | live · start | = | = |
| `s1` | live · b1 | = | = |
| `s2` | live · a2 | = | = |

- History (replaced versions the server keeps): `s1` start, a1.
- Conflicts: `s1` edit_edit, winner b1, loser a1, shown to ines, open.
- A: shown conflict.
- B: shown conflict.

#### S25 · The device whose clock runs behind still wins by arriving last

Category: clock. Covers: clock-behind. Rules: C1, C2, C3, C9, D13.

**Start.** Device B's clock is three days behind. Device A edits s1 and creates s2 with the
abbreviation ;sig. Then B, offline since before, edits s1 and creates s3, also ;sig; B stamps both
three days in the past.

Devices: A (ines), B (ines). Profiles: `p-ines` (personal, ines).

| Row | Kind | Refers to | Rev | Fields |
| --- | --- | --- | --- | --- |
| `f1` | folder | profile `p-ines` | 3 | name `Replies` |
| `s1` | snippet | folder `f1` | 5 | abbreviation `;rty`, body `Thanks for your order.` |

**Steps.**

1. B's clock is 3 days behind true time.
2. B goes offline.
3. `a1` · A edits `s1`: body `Thank you for your order.`. Answer: `s1` `applied`.
4. `a2` · A creates the snippet `s2` (folder `f1`): abbreviation `;sig`, body `Kind regards, Ines`.
   Answer: `s2` `applied`.
5. `b1` · B edits `s1`: body `Thanks a lot for your order.`.
6. `b2` · B creates the snippet `s3` (folder `f1`): abbreviation `;sig`, body `Best, Ines
   Marchetti`.
7. B comes back online and syncs. Answer: `s1` `applied+conflict`, `s3` `applied`. *A creation time
   in the past is kept as given. Among duplicates the earlier creation time holds the abbreviation,
   so here the skewed clock decides which of the two expands; it cannot make devices disagree.*

**Outcome.** b1 wins s1 by arriving last, although B stamped it three days before a1. Both ;sig
snippets exist; s3 holds the abbreviation on both devices because its creation time, as stamped, is
the earlier one, and library health lists s2.

| Row | Server | A | B |
| --- | --- | --- | --- |
| `f1` | live · start | = | = |
| `s1` | live · b1 | = | = |
| `s2` | live · a2 | = | = |
| `s3` | live · b2 | = | = |

- History (replaced versions the server keeps): `s1` start, a1.
- Conflicts: `s1` edit_edit, winner b1, loser a1, shown to ines, open.
- A: shown conflict, duplicate; expands `;sig` as `s3`.
- B: shown conflict, duplicate; expands `;sig` as `s3`.

#### S26 · The user edits a snippet again while its push is in flight

Category: transport. Covers: edit-during-push. Rules: D1, D4.

**Start.** Device A pushes an edit of s1. Before the answer arrives, the user saves s1 again.

Devices: A (ines), B (ines). Profiles: `p-ines` (personal, ines).

| Row | Kind | Refers to | Rev | Fields |
| --- | --- | --- | --- | --- |
| `f1` | folder | profile `p-ines` | 3 | name `Replies` |
| `s1` | snippet | folder `f1` | 5 | abbreviation `;rty`, body `Thanks for your order.` |

**Steps.**

1. A stays connected but syncs only when a step says so.
2. `a1` · A edits `s1`: body `Thank you for your order.`.
3. A pushes. Answer: `held`. *The request carries a1. The server applies it; the answer is on its
   way.*
4. `a2` · A edits `s1`: body `Thank you very much for your order.`.
5. The held answer reaches A. Answer: `s1` `applied`. *The row changed since it was read for the
   push, so the store takes only the new revision from the answer and sets it as the base of the
   change still waiting (D4).*
6. A syncs by itself again. Answer: `s1` `applied`. *a2 is pushed on top of a1's revision: not
   stale, no conflict with itself.*

**Outcome.** The snippet has the content of a2 everywhere, after two clean revisions. No conflict is
recorded.

| Row | Server | A | B |
| --- | --- | --- | --- |
| `f1` | live · start | = | = |
| `s1` | live · a2 | = | = |

- History (replaced versions the server keeps): `s1` start, a1.
- Conflicts: none.
- A: shown nothing.
- B: shown nothing.

#### S27 · Many offline changes to the same rows are sent as one change per row

Category: offline. Covers: offline-burst. Rules: D1.

**Start.** Offline, device A saves s1 three times, creates s2 and edits it, and creates s3 and
deletes it again.

Devices: A (ines), B (ines). Profiles: `p-ines` (personal, ines).

| Row | Kind | Refers to | Rev | Fields |
| --- | --- | --- | --- | --- |
| `f1` | folder | profile `p-ines` | 3 | name `Replies` |
| `s1` | snippet | folder `f1` | 5 | abbreviation `;rty`, body `Thanks for your order.` |

**Steps.**

1. A goes offline.
2. `a1` · A edits `s1`: body `Thank you.`.
3. `a2` · A edits `s1`: body `Thank you for your order.`.
4. `a3` · A edits `s1`: body `Thank you very much for your order.`.
5. `a4` · A creates the snippet `s2` (folder `f1`): abbreviation `;rlate`, body `Your order is
   late.`.
6. `a5` · A edits `s2`: body `Your order left us late.`.
7. `a6` · A creates the snippet `s3` (folder `f1`): abbreviation `;tmp`, body `Scratch.`.
8. `a7` · A deletes `s3`.
9. A comes back online and syncs. Answer: `s1` `applied`, `s2` `applied`. *Seven outbox entries
   become two pushed rows: each row is sent once, as it is now, with the base of its first unsent
   change. s3 was created and deleted without the server ever seeing it, so nothing is sent for it
   and the device drops the row (D1).*

**Outcome.** The server gets one revision of s1 (the content of a3) and one of s2 (the content of
a5). s3 never leaves device A and is gone there too. The versions a1, a2 and a4 existed only on A.

| Row | Server | A | B |
| --- | --- | --- | --- |
| `f1` | live · start | = | = |
| `s1` | live · a3 | = | = |
| `s2` | live · a5 | = | = |
| `s3` | absent | = | = |

- History (replaced versions the server keeps): `s1` start.
- Conflicts: none.
- A: shown nothing.
- B: shown nothing.

#### S28 · A family is given exactly one folder and nothing else

Category: sharing. Covers: exactly-one-folder. Rules: C21, D6.

**Start.** Ines and Tom are in the family group. Ines has the folder f1 with a sub-folder f1b, and a
private folder f2. The tag t1 is on s1 (in f1) and on s4 (in f2); the tag t2 is on s3 (in f2) only.
The image at1 is used by s2 (in f1b), the image at2 by s3. Ines shares f1 with the family, to use.

Devices: A (ines), C (tom). Profiles: `p-ines` (personal, ines), `p-tom` (personal, tom), `g-fam`
(family, ines).

| Row | Kind | Refers to | Rev | Fields |
| --- | --- | --- | --- | --- |
| `m-ines` | membership | group `g-fam`, user `ines` | 2 | role `owner` |
| `m-tom` | membership | group `g-fam`, user `tom` | 6 | role `member` |
| `f1` | folder | profile `p-ines` | 3 | name `Shop replies` |
| `f1b` | folder | profile `p-ines`, parent `f1` | 4 | name `Returns` |
| `f2` | folder | profile `p-ines` | 5 | name `Private` |
| `s1` | snippet | folder `f1` | 7 | abbreviation `;hrs`, body `Open 9 to 5.` |
| `s2` | snippet | folder `f1b` | 8 | abbreviation `;rret`, kind `rich`, body `Label: ![label](attachment:sha-label)` |
| `s3` | snippet | folder `f2` | 9 | abbreviation `;iban`, kind `rich`, body `My account: ![card](attachment:sha-card)` |
| `s4` | snippet | folder `f2` | 10 | abbreviation `;home`, body `12 Millrace Lane` |
| `t1` | tag | profile `p-ines` | 11 | name `shop` |
| `t2` | tag | profile `p-ines` | 12 | name `money` |
| `l1` | snippet tag | snippet `s1`, tag `t1` | 13 |  |
| `l2` | snippet tag | snippet `s3`, tag `t2` | 14 |  |
| `l3` | snippet tag | snippet `s4`, tag `t1` | 15 |  |
| `at1` | attachment | profile `p-ines` | 16 | sha256 `sha-label` |
| `at2` | attachment | profile `p-ines` | 17 | sha256 `sha-card` |
| `la1` | snippet attachment | snippet `s2`, attachment `at1` | 18 |  |
| `la2` | snippet attachment | snippet `s3`, attachment `at2` | 19 |  |

**Steps.**

1. `a1` · A creates the folder share `sh1` (folder `f1`, group `g-fam`): permission `view`. Answer:
   `sh1` `applied`. *The share is one row. The server adds access entries for Tom to f1 and f1b and
   puts folder_visible(f1) in his feed; his device fetches the folder whole and then the bytes of
   at1.*

**Outcome.** Tom's device holds the shared folder, its sub-folder, their two snippets, the tag and
the image those snippets use, the links between them and the share row; it can only read them. It
holds nothing else of Ines's: not f2 or its snippets, not the tag t2, not the link from the private
snippet s4 to the tag t1, not the image at2 or its bytes. Queries as Tom return none of those rows,
and a request for the stored file of at2 is refused exactly as for a file that does not exist.

| Row | Server | A | C |
| --- | --- | --- | --- |
| `m-ines` | live · start | = | = |
| `m-tom` | live · start | = | = |
| `f1` | live · start | = | = |
| `f1b` | live · start | = | = |
| `f2` | live · start | = | absent |
| `s1` | live · start | = | = |
| `s2` | live · start | = | = |
| `s3` | live · start | = | absent |
| `s4` | live · start | = | absent |
| `t1` | live · start | = | = |
| `t2` | live · start | = | absent |
| `l1` | live · start | = | = |
| `l2` | live · start | = | absent |
| `l3` | live · start | = | absent |
| `at1` | live · start | = | = |
| `at2` | live · start | = | absent |
| `la1` | live · start | = | = |
| `la2` | live · start | = | absent |
| `sh1` | live · a1 | = | = |

- History (replaced versions the server keeps): none.
- Conflicts: none.
- A: shown nothing.
- C: shown shared; permission `f1` view, `f1b` view.

#### S29 · A device was offline when a folder stopped being shared

Category: sharing. Covers: missed-folder-revoked. Rules: C21, C22, D6, D7.

**Start.** Ines shares f1 with the family. Tom has two devices: D is online, C is offline. Ines
stops sharing. C comes back 31 days later, after the feed entry and the share's tombstone were
purged.

Devices: A (ines), C (tom), D (tom). Profiles: `p-ines` (personal, ines), `p-tom` (personal, tom),
`g-fam` (family, ines).

| Row | Kind | Refers to | Rev | Fields |
| --- | --- | --- | --- | --- |
| `m-ines` | membership | group `g-fam`, user `ines` | 2 | role `owner` |
| `m-tom` | membership | group `g-fam`, user `tom` | 6 | role `member` |
| `f1` | folder | profile `p-ines` | 3 | name `Shop replies` |
| `s1` | snippet | folder `f1` | 5 | abbreviation `;hrs`, body `Open 9 to 5.` |
| `sh1` | folder share | folder `f1`, group `g-fam` | 7 | permission `view` |

**Steps.**

1. C goes offline.
2. `a1` · A deletes `sh1`. Answer: `sh1` `applied`. *The server removes Tom's access entries and
   puts folder_revoked(f1) in his feed. D reads it at once and purges the folder, its snippets,
   their approvals and cached files.*
3. 31 days pass.
4. C comes back online and syncs. Answer: `reset_required:expired`. *C's position is older than the
   feed. C fetches everything it may see and removes what is not in it: the folder, its snippet and
   the share row (D7). Had the feed entry been lost for any other reason, the access check that
   every device runs at start and once a day would have removed the folder the same way.*

**Outcome.** Neither of Tom's devices holds the folder any more, whether it heard of the change at
once or a month later. Each says once that the folder is no longer shared. Ines keeps the folder;
the share's tombstone is purged everywhere after 30 days.

| Row | Server | A | C | D |
| --- | --- | --- | --- | --- |
| `m-ines` | live · start | = | = | = |
| `m-tom` | live · start | = | = | = |
| `f1` | live · start | = | absent | absent |
| `s1` | live · start | = | absent | absent |
| `sh1` | absent | = | = | = |

- History (replaced versions the server keeps): none.
- Conflicts: none.
- A: shown nothing.
- C: shown revoked; permission `f1` none.
- D: shown revoked; permission `f1` none.

#### S30 · A member is removed while their device holds unsent changes

Category: membership. Covers: member-removed-with-pending. Rules: C12, C16, C21, D5, D6.

**Start.** Ines shares f1 with the family so that members can change it. Tom's device goes offline;
he edits s1 and creates s5 in the folder. Ines removes Tom from the group.

Devices: A (ines), C (tom). Profiles: `p-ines` (personal, ines), `p-tom` (personal, tom), `g-fam`
(family, ines).

| Row | Kind | Refers to | Rev | Fields |
| --- | --- | --- | --- | --- |
| `m-ines` | membership | group `g-fam`, user `ines` | 2 | role `owner` |
| `m-tom` | membership | group `g-fam`, user `tom` | 6 | role `member` |
| `f1` | folder | profile `p-ines` | 3 | name `Shop replies` |
| `s1` | snippet | folder `f1` | 5 | abbreviation `;hrs`, body `Open 9 to 5.` |
| `sh1` | folder share | folder `f1`, group `g-fam` | 7 | permission `edit` |

**Steps.**

1. C goes offline.
2. `c1` · C edits `s1`: body `Open 9 to 6.`.
3. `c2` · C creates the snippet `s5` (folder `f1`): abbreviation `;sat`, body `Closed on
   Saturdays.`.
4. `x1` · A: the user removes the member `m-tom`. Answer: `ok`.
5. C comes back online and syncs. *C pulls before it pushes. The feed holds folder_revoked(f1):
   before the folder is purged, the two unsent changes are copied into the list of changes that were
   not synced and taken out of the outbox (D6). Nothing is left to push; had they been pushed, the
   server would have refused them (C12).*

**Outcome.** The server and Ines's device never see Tom's two changes. Tom's device no longer holds
the folder; it says that the folder is no longer shared with him and that two changes he made there
were not synced and are kept on this computer, where he can copy them into his own snippets. Tom
keeps the tombstone of his own membership and sees nothing else of the group.

| Row | Server | A | C |
| --- | --- | --- | --- |
| `m-ines` | live · start | = | absent |
| `m-tom` | deleted · x1 | = | = |
| `f1` | live · start | = | absent |
| `s1` | live · start | = | absent |
| `sh1` | live · start | = | absent |
| `s5` | absent | = | = |

- History (replaced versions the server keeps): `m-tom` start.
- Conflicts: none.
- A: shown nothing.
- C: shown revoked, kept; kept for the user: c1, c2; permission `f1` none.

#### S31 · A member who may only use a folder tries to change it

Category: membership. Covers: viewer-write. Rules: C11, C12, D5.

**Start.** Ines shares f1 with the family so that members can change it. Tom's device goes offline
and he edits s1. Ines lowers the share to "use only".

Devices: A (ines), C (tom). Profiles: `p-ines` (personal, ines), `p-tom` (personal, tom), `g-fam`
(family, ines).

| Row | Kind | Refers to | Rev | Fields |
| --- | --- | --- | --- | --- |
| `m-ines` | membership | group `g-fam`, user `ines` | 2 | role `owner` |
| `m-tom` | membership | group `g-fam`, user `tom` | 6 | role `member` |
| `f1` | folder | profile `p-ines` | 3 | name `Shop replies` |
| `s1` | snippet | folder `f1` | 5 | abbreviation `;hrs`, body `Open 9 to 5.` |
| `sh1` | folder share | folder `f1`, group `g-fam` | 7 | permission `edit` |

**Steps.**

1. C goes offline.
2. `c1` · C edits `s1`: body `Open 9 to 6.`.
3. `a1` · A edits `sh1`: permission `view`. Answer: `sh1` `applied`.
4. C comes back online and syncs. Answer: `s1` `rejected:forbidden`. *The server checks the
   permission as it is when the change arrives (C12). The answer carries the server's row; C puts it
   back and moves its own version to the list of changes that were not synced (D5). A changed client
   that ignores the read-only state gets the same answer.*

**Outcome.** The snippet is unchanged on the server and on every device. Tom's device shows the
folder as read-only and keeps his change in the list of changes that were not synced, where he can
copy it into his own snippets.

| Row | Server | A | C |
| --- | --- | --- | --- |
| `m-ines` | live · start | = | = |
| `m-tom` | live · start | = | = |
| `f1` | live · start | = | = |
| `s1` | live · start | = | = |
| `sh1` | live · a1 | = | = |

- History (replaced versions the server keeps): `sh1` start.
- Conflicts: none.
- A: shown nothing.
- C: shown kept; kept for the user: c1; permission `f1` view.

#### S32 · An invitation is accepted just before the inviter withdraws it

Category: membership. Covers: invitation-accept-vs-withdraw. Rules: C16, C21, D6.

**Start.** Ines owns the family group, has shared f1 with it, and has invited Tom. Tom presses Join
at the moment Ines presses Withdraw; Tom's call commits first.

Devices: A (ines), C (tom). Profiles: `p-ines` (personal, ines), `p-tom` (personal, tom), `g-fam`
(family, ines).

| Row | Kind | Refers to | Rev | Fields |
| --- | --- | --- | --- | --- |
| `m-ines` | membership | group `g-fam`, user `ines` | 2 | role `owner` |
| `inv1` | invitation | group `g-fam`, user `tom` | 6 | role `member`, status `pending` |
| `f1` | folder | profile `p-ines` | 3 | name `Shop replies` |
| `s1` | snippet | folder `f1` | 5 | abbreviation `;hrs`, body `Open 9 to 5.` |
| `sh1` | folder share | folder `f1`, group `g-fam` | 7 | permission `view` |

**Steps.**

1. `c1` · C: the user accepts the invitation `inv1` (membership `m-tom`). Answer: `ok`. *Both calls
   lock the invitation. Tom's commits first: the membership exists, the invitation is accepted, and
   folder_visible(f1) is in Tom's feed.*
2. `a1` · A: the user withdraws the invitation `inv1`. Answer: `error:already_accepted`. *The
   invitation is no longer pending, so there is nothing to withdraw. To undo it Ines removes the
   member.*

**Outcome.** Tom is a member and has the shared folder within the ten seconds; the answered
invitation is no longer listed for him. Ines's device says that Tom has already joined and shows him
in the member list, where she can remove him.

| Row | Server | A | C |
| --- | --- | --- | --- |
| `m-ines` | live · start | = | = |
| `inv1` | live · c1 (status `accepted`) | = | absent |
| `m-tom` | live · c1 | = | = |
| `f1` | live · start | = | = |
| `s1` | live · start | = | = |
| `sh1` | live · start | = | = |

- History (replaced versions the server keeps): `inv1` start.
- Conflicts: none.
- A: shown refused.
- C: shown shared; permission `f1` view.

#### S33 · An invitation is withdrawn just before it is accepted

Category: membership. Covers: invitation-accept-vs-withdraw. Rules: C16.

**Start.** As S32, but Ines's call commits first.

Devices: A (ines), C (tom). Profiles: `p-ines` (personal, ines), `p-tom` (personal, tom), `g-fam`
(family, ines).

| Row | Kind | Refers to | Rev | Fields |
| --- | --- | --- | --- | --- |
| `m-ines` | membership | group `g-fam`, user `ines` | 2 | role `owner` |
| `inv1` | invitation | group `g-fam`, user `tom` | 6 | role `member`, status `pending` |
| `f1` | folder | profile `p-ines` | 3 | name `Shop replies` |
| `s1` | snippet | folder `f1` | 5 | abbreviation `;hrs`, body `Open 9 to 5.` |
| `sh1` | folder share | folder `f1`, group `g-fam` | 7 | permission `view` |

**Steps.**

1. `a1` · A: the user withdraws the invitation `inv1`. Answer: `ok`.
2. `c1` · C: the user accepts the invitation `inv1` (membership `m-tom`). Answer:
   `error:invitation_invalid`. *The invitation is withdrawn, so the call creates nothing. A
   withdrawn invitation is no longer visible to the person invited.*

**Outcome.** No membership exists and Tom's device holds nothing of the group. It says that the
invitation is no longer valid, and the banner disappears.

| Row | Server | A | C |
| --- | --- | --- | --- |
| `m-ines` | live · start | = | absent |
| `inv1` | live · a1 (status `withdrawn`) | = | absent |
| `m-tom` | absent | = | = |
| `f1` | live · start | = | absent |
| `s1` | live · start | = | absent |
| `sh1` | live · start | = | absent |

- History (replaced versions the server keeps): `inv1` start.
- Conflicts: none.
- A: shown nothing.
- C: shown invitation_invalid.

#### S34 · The owner of a team hands the team to another member

Category: membership. Covers: ownership-transfer. Rules: C12, C16.

**Start.** Ines owns a team with Tom as admin and Jun as member. The team owns the folder gf1, which
members may only use. Ines transfers ownership to Tom. Then Tom tries to leave, Ines tries to delete
the team, and Ines edits a snippet of the team.

Devices: A (ines), C (tom), E (jun). Profiles: `p-ines` (personal, ines), `p-tom` (personal, tom),
`p-jun` (personal, jun), `g-team` (company, ines).

| Row | Kind | Refers to | Rev | Fields |
| --- | --- | --- | --- | --- |
| `m-ines` | membership | group `g-team`, user `ines` | 2 | role `owner` |
| `m-tom` | membership | group `g-team`, user `tom` | 6 | role `admin` |
| `m-jun` | membership | group `g-team`, user `jun` | 9 | role `member` |
| `gf1` | folder | profile `g-team` | 3 | name `Team replies`, member_permission `view` |
| `gs1` | snippet | folder `gf1` | 5 | abbreviation `;hrs`, body `Open 9 to 5.` |

**Steps.**

1. `x1` · A: the user transfers the ownership of `g-team` to tom. Answer: `ok`. *One transaction:
   the profile's owner becomes Tom, Tom's membership becomes owner and Ines's becomes admin. A group
   has exactly one owner at every moment.*
2. `x2` · C: the user leaves the group (membership `m-tom`). Answer: `error:owner_cannot_leave`.
3. `x3` · A: the user deletes the group `g-team`. Answer: `error:not_owner`.
4. `a1` · A edits `gs1`: body `Open 9 to 6.`. Answer: `gs1` `applied`. *As admin Ines may still
   change the team's snippets.*

**Outcome.** Tom is the owner and Ines an admin on every member's device. The two refused calls
change nothing and each device says why. Ines's edit is accepted because admins may change the
team's content.

| Row | Server | A | C | E |
| --- | --- | --- | --- | --- |
| `m-ines` | live · x1 (role `admin`) | = | = | = |
| `m-tom` | live · x1 (role `owner`) | = | = | = |
| `m-jun` | live · start | = | = | = |
| `gf1` | live · start | = | = | = |
| `gs1` | live · a1 | = | = | = |

- History (replaced versions the server keeps): `m-ines` start; `m-tom` start; `gs1` start.
- Conflicts: none.
- A: shown refused; permission `gf1` manage.
- C: shown refused; permission `gf1` manage.
- E: shown nothing; permission `gf1` view.

#### S35 · A group is deleted while a member holds unsent changes

Category: membership. Covers: profile-deletion. Rules: C16, C21, D5, D6.

**Start.** Ines owns the family group with Tom as member. She shares her folder f1 with it, to
change, and the group owns the folder gf1, which members may change. Tom's device goes offline; he
edits s1 and gs1. Ines deletes the group.

Devices: A (ines), C (tom). Profiles: `p-ines` (personal, ines), `p-tom` (personal, tom), `g-fam`
(family, ines).

| Row | Kind | Refers to | Rev | Fields |
| --- | --- | --- | --- | --- |
| `m-ines` | membership | group `g-fam`, user `ines` | 2 | role `owner` |
| `m-tom` | membership | group `g-fam`, user `tom` | 6 | role `member` |
| `f1` | folder | profile `p-ines` | 3 | name `Shop replies` |
| `s1` | snippet | folder `f1` | 5 | abbreviation `;hrs`, body `Open 9 to 5.` |
| `sh1` | folder share | folder `f1`, group `g-fam` | 7 | permission `edit` |
| `gf1` | folder | profile `g-fam` | 8 | name `Family`, member_permission `edit` |
| `gs1` | snippet | folder `gf1` | 9 | abbreviation `;wifi`, body `The guest network is Millrace.` |

**Steps.**

1. C goes offline.
2. `c1` · C edits `s1`: body `Open 9 to 6.`.
3. `c2` · C edits `gs1`: body `The guest network is Millrace-Guest.`.
4. `x1` · A: the user deletes the group `g-fam`. Answer: `ok`. *One transaction: the profile, its
   memberships, the shares into it and the folders it owns, with their content, are deleted. Every
   member, the owner included, gets folder_revoked for the group's folders; the members other than
   the folder's owner get it for the folders shared into the group.*
5. C comes back online and syncs. *C pulls first: both folders are revoked, the two unsent changes
   are kept for the user and the folders purged (D6).*

**Outcome.** The group, its own folder and the share are gone; the group's content can be restored
by nobody after 30 days. Ines keeps her folder f1, unshared. Tom's device holds nothing of the group
and keeps his two changes in the list of changes that were not synced. Each of them keeps only the
tombstone of their own membership.

| Row | Server | A | C |
| --- | --- | --- | --- |
| `m-ines` | deleted · x1 | = | absent |
| `m-tom` | deleted · x1 | absent | = |
| `f1` | live · start | = | absent |
| `s1` | live · start | = | absent |
| `sh1` | deleted · x1 | = | absent |
| `gf1` | deleted · x1 | absent | absent |
| `gs1` | deleted · x1 | absent | absent |

- History (replaced versions the server keeps): `m-ines` start; `m-tom` start; `sh1` start; `gf1`
start; `gs1` start.
- Conflicts: none.
- A: shown nothing.
- C: shown revoked, kept; kept for the user: c1, c2.

#### S36 · A folder is shared with two groups and one share ends

Category: sharing. Covers: two-shares-same-folder. Rules: C11, C12, C21, D5.

**Start.** Ines shares f1 with the family, to change, and with the team, to use. Tom is in both
groups. Tom's device goes offline and he edits s1. Ines stops sharing with the family.

Devices: A (ines), C (tom). Profiles: `p-ines` (personal, ines), `p-tom` (personal, tom), `g-fam`
(family, ines), `g-team` (company, ines).

| Row | Kind | Refers to | Rev | Fields |
| --- | --- | --- | --- | --- |
| `mf-ines` | membership | group `g-fam`, user `ines` | 2 | role `owner` |
| `mf-tom` | membership | group `g-fam`, user `tom` | 6 | role `member` |
| `mt-ines` | membership | group `g-team`, user `ines` | 10 | role `owner` |
| `mt-tom` | membership | group `g-team`, user `tom` | 11 | role `member` |
| `f1` | folder | profile `p-ines` | 3 | name `Shop replies` |
| `s1` | snippet | folder `f1` | 5 | abbreviation `;hrs`, body `Open 9 to 5.` |
| `sh1` | folder share | folder `f1`, group `g-fam` | 7 | permission `edit` |
| `sh2` | folder share | folder `f1`, group `g-team` | 12 | permission `view` |

**Steps.**

1. C goes offline.
2. `c1` · C edits `s1`: body `Open 9 to 6.`.
3. `a1` · A deletes `sh1`. Answer: `sh1` `applied`. *Tom still reaches the folder through the team's
   share, so his access entry stays, with the permission of that share, and no folder_revoked is
   written. A folder is revoked only when the last grant ends.*
4. C comes back online and syncs. Answer: `s1` `rejected:forbidden`. *C applies the tombstone of sh1
   and learns that its permission is now "use only". Its edit is refused, the server's row put back,
   and the change kept for the user.*

**Outcome.** Tom keeps the folder, read-only now, on his device. His edit, made while he could still
change the folder, is refused and kept in the list of changes that were not synced. Nothing is
purged.

| Row | Server | A | C |
| --- | --- | --- | --- |
| `mf-ines` | live · start | = | = |
| `mf-tom` | live · start | = | = |
| `mt-ines` | live · start | = | = |
| `mt-tom` | live · start | = | = |
| `f1` | live · start | = | = |
| `s1` | live · start | = | = |
| `sh1` | deleted · a1 | = | = |
| `sh2` | live · start | = | = |

- History (replaced versions the server keeps): `sh1` start.
- Conflicts: none.
- A: shown nothing.
- C: shown kept; kept for the user: c1; permission `f1` view.

#### S37 · A stale device shares a folder that was unshared in the meantime

Category: sharing. Covers: share-stale-put, two-shares-same-folder. Rules: C11, C21, D2, D5, D6.

**Start.** Ines has two devices. Offline, device B shares f1 with the family, to change. Device A
shares the same folder with the same group, to use, and later stops sharing it. Then B comes back. A
share's id is derived from the folder and the group, so both devices wrote the same row.

Devices: A (ines), B (ines), C (tom). Profiles: `p-ines` (personal, ines), `p-tom` (personal, tom),
`g-fam` (family, ines).

| Row | Kind | Refers to | Rev | Fields |
| --- | --- | --- | --- | --- |
| `m-ines` | membership | group `g-fam`, user `ines` | 2 | role `owner` |
| `m-tom` | membership | group `g-fam`, user `tom` | 6 | role `member` |
| `f1` | folder | profile `p-ines` | 3 | name `Shop replies` |
| `s1` | snippet | folder `f1` | 5 | abbreviation `;hrs`, body `Open 9 to 5.` |

**Steps.**

1. B goes offline.
2. `b1` · B creates the folder share `sh1` (folder `f1`, group `g-fam`): permission `edit`.
3. `a1` · A creates the folder share `sh1` (folder `f1`, group `g-fam`): permission `view`. Answer:
   `sh1` `applied`. *Tom's device gets the folder.*
4. `a2` · A deletes `sh1`. Answer: `sh1` `applied`. *Tom's device purges the folder.*
5. B comes back online and syncs. Answer: `sh1` `superseded`. *B's share was made without knowing of
   a1 and a2. For shares a stale write never wins: the server keeps its row, deleted, and B takes it
   (C11). Sharing again from B, now that B has seen the delete, would not be stale.*

**Outcome.** The folder stays unshared: a change that would open access cannot win by arriving late.
B drops its own share and says that sharing was stopped on another device. Tom's device saw the
folder appear and disappear and holds nothing of it.

| Row | Server | A | B | C |
| --- | --- | --- | --- | --- |
| `m-ines` | live · start | = | = | = |
| `m-tom` | live · start | = | = | = |
| `f1` | live · start | = | = | absent |
| `s1` | live · start | = | = | absent |
| `sh1` | deleted · a2 | = | = | absent |

- History (replaced versions the server keeps): `sh1` a1.
- Conflicts: none.
- A: shown nothing.
- B: shown superseded.
- C: shown shared, revoked; permission `f1` none.

#### S38 · A snippet includes a snippet from a folder the user loses

Category: sharing. Covers: include-loses-access. Rules: C21, D6.

**Start.** Ines shares f1, which holds ;addr, with the family. Tom's own snippet ts1 includes it:
its content is {snippet abbr=";addr"}. Ines stops sharing.

Devices: A (ines), C (tom). Profiles: `p-ines` (personal, ines), `p-tom` (personal, tom), `g-fam`
(family, ines).

| Row | Kind | Refers to | Rev | Fields |
| --- | --- | --- | --- | --- |
| `m-ines` | membership | group `g-fam`, user `ines` | 2 | role `owner` |
| `m-tom` | membership | group `g-fam`, user `tom` | 6 | role `member` |
| `f1` | folder | profile `p-ines` | 3 | name `Shop replies` |
| `s1` | snippet | folder `f1` | 5 | abbreviation `;addr`, body `12 Millrace Lane` |
| `sh1` | folder share | folder `f1`, group `g-fam` | 7 | permission `view` |
| `tf1` | folder | profile `p-tom` | 8 | name `Mine` |
| `ts1` | snippet | folder `tf1` | 9 | abbreviation `;find`, body `Find us at {snippet abbr=";addr"}.` |

**Steps.**

1. `a1` · A deletes `sh1`. Answer: `sh1` `applied`. *Tom's device purges f1. Nothing about ts1
   changes on the server: the server never reads a snippet's content.*

**Outcome.** Tom's snippet stays as it is, but on his devices its reference no longer resolves:
typing ;find types nothing and the log says E_SNIPPET_NOT_FOUND, and library health lists it ("It
includes ;addr, which does not exist"). It works again if the folder is shared again or Tom makes
his own ;addr.

| Row | Server | A | C |
| --- | --- | --- | --- |
| `m-ines` | live · start | = | = |
| `m-tom` | live · start | = | = |
| `f1` | live · start | = | absent |
| `s1` | live · start | = | absent |
| `sh1` | deleted · a1 | = | absent |
| `tf1` | live · start | absent | = |
| `ts1` | live · start | absent | = |

- History (replaced versions the server keeps): `sh1` start.
- Conflicts: none.
- A: shown nothing.
- C: shown revoked, broken; reference broken: `ts1`; permission `f1` none.

#### S39 · An admin is made a member while their device holds an unsent edit

Category: membership. Covers: membership-racing-edit. Rules: C12, C16, D5.

**Start.** A team owns the folder gf1, which members may only use. Tom is an admin; offline, he
edits the team's snippet gs1. Ines, the owner, makes Tom a member.

Devices: A (ines), C (tom). Profiles: `p-ines` (personal, ines), `p-tom` (personal, tom), `g-team`
(company, ines).

| Row | Kind | Refers to | Rev | Fields |
| --- | --- | --- | --- | --- |
| `m-ines` | membership | group `g-team`, user `ines` | 2 | role `owner` |
| `m-tom` | membership | group `g-team`, user `tom` | 6 | role `admin` |
| `gf1` | folder | profile `g-team` | 3 | name `Team replies`, member_permission `view` |
| `gs1` | snippet | folder `gf1` | 5 | abbreviation `;hrs`, body `Open 9 to 5.` |

**Steps.**

1. C goes offline.
2. `c1` · C edits `gs1`: body `Open 9 to 6.`.
3. `x1` · A: the user makes `m-tom` a member. Answer: `ok`.
4. C comes back online and syncs. Answer: `gs1` `rejected:forbidden`. *What counts is Tom's role
   when the change arrives, not when he made it (C12).*

**Outcome.** The snippet is unchanged. Tom's device shows the folder as read-only and keeps his edit
in the list of changes that were not synced.

| Row | Server | A | C |
| --- | --- | --- | --- |
| `m-ines` | live · start | = | = |
| `m-tom` | live · x1 (role `member`) | = | = |
| `gf1` | live · start | = | = |
| `gs1` | live · start | = | = |

- History (replaced versions the server keeps): `m-tom` start.
- Conflicts: none.
- A: shown nothing.
- C: shown kept; kept for the user: c1; permission `gf1` view.

#### S40 · A member adds key presses to a shared snippet, and the owner's own edit does not hide that

Category: sharing. Covers: review-hold. Rules: C17, D10.

**Start.** Ines shares f1 with the family, to change. Its snippet s1 is one line that Ines wrote.
Tom changes it so that it reads the clipboard and presses Enter. Later Ines changes only the label,
on her second device.

Devices: A (ines), B (ines), C (tom), E (jun). Profiles: `p-ines` (personal, ines), `p-tom`
(personal, tom), `p-jun` (personal, jun), `g-fam` (family, ines).

| Row | Kind | Refers to | Rev | Fields |
| --- | --- | --- | --- | --- |
| `m-ines` | membership | group `g-fam`, user `ines` | 2 | role `owner` |
| `m-tom` | membership | group `g-fam`, user `tom` | 6 | role `member` |
| `m-jun` | membership | group `g-fam`, user `jun` | 9 | role `member` |
| `f1` | folder | profile `p-ines` | 3 | name `Shop replies` |
| `s1` | snippet | folder `f1` | 5 | abbreviation `;wtrack`, label `Tracking link`, body `Your tracking link: ` |
| `sh1` | folder share | folder `f1`, group `g-fam` | 7 | permission `edit` |

**Steps.**

1. `c1` · C edits `s1`: body `Your tracking link: {clipboard}{key name="enter"}`. Answer: `s1`
   `applied`. *The server records Tom as the author of the row and of its content (C17). On Ines's
   and Jun's devices the new content reads the clipboard and presses a key and was written by
   someone else, so it is held: they keep typing the last content they were allowed to type, the one
   line of the start.*
2. `b1` · B edits `s1`: label `Paste tracking link`. Answer: `s1` `applied`. *Ines's push carries
   Tom's content with her new label. The row's author is now Ines, but the fields that decide what a
   snippet does did not change in this write, so the server leaves Tom as the author of the
   content.*

**Outcome.** The row has Ines's label and Tom's content, whose author is still Tom. It stays held
for review on Ines's two devices and on Jun's, where typing ;wtrack keeps typing the old line until
each of them approves; it is not held on Tom's device. A label edit by the owner cannot make someone
else's key presses count as her own.

| Row | Server | A | B | C | E |
| --- | --- | --- | --- | --- | --- |
| `m-ines` | live · start | = | = | = | = |
| `m-tom` | live · start | = | = | = | = |
| `m-jun` | live · start | = | = | = | = |
| `f1` | live · start | = | = | = | = |
| `s1` | live · b1 (content_by `tom`, updated_by `ines`) | = | = | = | = |
| `sh1` | live · start | = | = | = | = |

- History (replaced versions the server keeps): `s1` start, c1.
- Conflicts: none.
- A: shown review; held for review: `s1`.
- B: shown review; held for review: `s1`.
- C: shown nothing.
- E: shown review; held for review: `s1`.

#### S41 · The answer to a push is lost and the push is sent again

Category: transport. Covers: replayed-push. Rules: C15, D4.

**Start.** Device A pushes an edit of s1. The server applies it, but the answer never arrives.
Before A tries again, device B edits the same snippet on top of A's change.

Devices: A (ines), B (ines). Profiles: `p-ines` (personal, ines).

| Row | Kind | Refers to | Rev | Fields |
| --- | --- | --- | --- | --- |
| `f1` | folder | profile `p-ines` | 3 | name `Replies` |
| `s1` | snippet | folder `f1` | 5 | abbreviation `;rty`, body `Thanks for your order.` |

**Steps.**

1. A stays connected but syncs only when a step says so.
2. `a1` · A edits `s1`: body `Thank you for your order.`.
3. A pushes. Answer: `lost`. *The server applied a1 and stored the answer under the change's id. A
   still holds the change in its outbox.*
4. `b1` · B edits `s1`: body `Thank you so much for your order.`. Answer: `s1` `applied`. *B had
   pulled a1, so this edit is based on it: not stale.*
5. A pushes. Answer: `s1` `replayed`. *The same change id again: the server answers what it answered
   the first time and writes nothing (C15). Without the stored answer this push would look stale and
   would replace b1.*
6. A syncs by itself again. *A pulls b1 and applies it: nothing of its own is waiting any more.*

**Outcome.** a1 was applied once and b1 on top of it. The snippet has the content of b1 everywhere,
with no conflict: sending a change twice never creates a second revision.

| Row | Server | A | B |
| --- | --- | --- | --- |
| `f1` | live · start | = | = |
| `s1` | live · b1 | = | = |

- History (replaced versions the server keeps): `s1` start, a1.
- Conflicts: none.
- A: shown nothing.
- B: shown nothing.

#### S42 · The network delivers the same push twice

Category: transport. Covers: duplicated-push. Rules: C4, C15.

**Start.** Device A pushes an edit of s1 and the request reaches the server twice.

Devices: A (ines), B (ines). Profiles: `p-ines` (personal, ines).

| Row | Kind | Refers to | Rev | Fields |
| --- | --- | --- | --- | --- |
| `f1` | folder | profile `p-ines` | 3 | name `Replies` |
| `s1` | snippet | folder `f1` | 5 | abbreviation `;rty`, body `Thanks for your order.` |

**Steps.**

1. A stays connected but syncs only when a step says so.
2. `a1` · A edits `s1`: body `Thank you for your order.`.
3. A pushes. The request reaches the server twice. Answer: `s1` `applied`. *The two requests take
   the same lock on the device's receipts. The first applies the change; the second finds the stored
   answer and returns it. Whichever answer the device reads, it is the same.*
4. A syncs by itself again.

**Outcome.** The snippet has one new revision. The history holds the start version once, and nothing
is shown.

| Row | Server | A | B |
| --- | --- | --- | --- |
| `f1` | live · start | = | = |
| `s1` | live · a1 | = | = |

- History (replaced versions the server keeps): `s1` start.
- Conflicts: none.
- A: shown nothing.
- B: shown nothing.

#### S43 · The server is restored from a backup

Category: server. Covers: server-restore. Rules: C18, D7.

**Start.** Ines's folder f1 holds s1 and s3. After a backup is taken, device A edits s1 and deletes
s3 and device B creates s2; all of it is synced. Then the server's database is lost and restored
from the backup.

Devices: A (ines), B (ines). Profiles: `p-ines` (personal, ines).

| Row | Kind | Refers to | Rev | Fields |
| --- | --- | --- | --- | --- |
| `f1` | folder | profile `p-ines` | 3 | name `Replies` |
| `s1` | snippet | folder `f1` | 5 | abbreviation `;rty`, body `Thanks for your order.` |
| `s3` | snippet | folder `f1` | 6 | abbreviation `;tmp`, body `Scratch.` |

**Steps.**

1. `a1` · A edits `s1`: body `Thank you for your order.`. Answer: `s1` `applied`.
2. `k1` · A backup of the server's database is taken.
3. `a2` · A edits `s1`: body `Thank you very much for your order.`. Answer: `s1` `applied`.
4. `b1` · B creates the snippet `s2` (folder `f1`): abbreviation `;rlate`, body `Your order left us
   late.`. Answer: `s2` `applied`.
5. `a3` · A deletes `s3`. Answer: `s3` `applied`.
6. The server's database is restored from the backup `k1`. *The restored server holds s1 as of a1,
   s3 live and no s2. It starts a new epoch: a new epoch id, and revision and feed counters moved
   past anything it can have issued before.*
7. A pulls. Answer: `reset_required:epoch`, `s1` `applied`, `s2` `applied`, `s3` `applied`. *A's
   position belongs to the old epoch. A resyncs: it fetches everything, and sends again each row it
   holds in a revision newer than the backup (C18): s1 as of a2, s2, and the tombstone of s3. The
   restored rows were not touched in the new epoch, so nothing is in conflict.*
8. B pulls. Answer: `reset_required:epoch`, `s1` `unchanged`, `s2` `unchanged`, `s3` `unchanged`. *B
   holds the same revisions and sends the same rows; they are already there.*

**Outcome.** The devices give back what the server lost: s1 has the content of a2, s2 exists, and s3
is deleted again. No change made after the backup is lost and no conflict is recorded, because both
devices held the same versions.

| Row | Server | A | B |
| --- | --- | --- | --- |
| `f1` | live · start | = | = |
| `s1` | live · a2 | = | = |
| `s2` | live · b1 | = | = |
| `s3` | deleted · a3 | = | = |

- History (replaced versions the server keeps): `s1` start, a1; `s3` start.
- Conflicts: none.
- A: shown nothing.
- B: shown nothing.

#### S44 · A device speaks a protocol version the server no longer accepts

Category: server. Covers: version-mismatch. Rules: C23, D7, D12.

**Start.** Device A runs a current build (protocol 2), device B an old one (protocol 1). The server
stops accepting protocol 1. Then both devices make an edit, and later B is updated.

Devices: A (ines, protocol 2), B (ines, protocol 1). Profiles: `p-ines` (personal, ines).

| Row | Kind | Refers to | Rev | Fields |
| --- | --- | --- | --- | --- |
| `f1` | folder | profile `p-ines` | 3 | name `Replies` |
| `s1` | snippet | folder `f1` | 5 | abbreviation `;rty`, body `Thanks for your order.` |
| `s2` | snippet | folder `f1` | 6 | abbreviation `;rlate`, body `Your order left us late.` |

**Steps.**

1. `k1` · The server accepts protocol versions 2 to 2 from now on.
2. `a1` · A edits `s1`: body `Thank you for your order.`. Answer: `s1` `applied`.
3. `b1` · B edits `s2`: body `Sorry, your order left us late.`. Answer: `upgrade_required`. *Every
   call of B is answered this way, pulls included. B stops syncing, keeps expanding and editing,
   keeps its change in the outbox, and says that Iamb must be updated to keep syncing (D12). It does
   not see a1.*
4. `u1` · B is updated to a build that speaks protocol 2. Answer: `s2` `applied`. *A build that
   speaks a newer protocol resyncs once (D7), then pushes what waited.*

**Outcome.** While B is too old, nothing of B's reaches the server and nothing reaches B, but
nothing is lost: after the update B fetches a1 and pushes b1. Both devices end with both edits.

| Row | Server | A | B |
| --- | --- | --- | --- |
| `f1` | live · start | = | = |
| `s1` | live · a1 | = | = |
| `s2` | live · b1 | = | = |

- History (replaced versions the server keeps): `s1` start; `s2` start.
- Conflicts: none.
- A: shown nothing.
- B: shown upgrade.

#### S45 · One row of a push is too large to store

Category: transport. Covers: partial-failure. Rules: C14, D5.

**Start.** Offline, device A creates three snippets. The content of the second is larger than the
server stores.

Devices: A (ines), B (ines). Profiles: `p-ines` (personal, ines).

| Row | Kind | Refers to | Rev | Fields |
| --- | --- | --- | --- | --- |
| `f1` | folder | profile `p-ines` | 3 | name `Replies` |

**Steps.**

1. A goes offline.
2. `a1` · A creates the snippet `s2` (folder `f1`): abbreviation `;rty`, body `Thanks for your
   order.`.
3. `a2` · A creates the snippet `s3` (folder `f1`): abbreviation `;terms`, body `(500 kB of text)`.
4. `a3` · A creates the snippet `s4` (folder `f1`): abbreviation `;rlate`, body `Your order left us
   late.`.
5. A comes back online and syncs. Answer: `s2` `applied`, `s3` `blocked:too_large`, `s4` `applied`.
   *Each row of a push has its own answer and one refused row does not hold up the others (C14). The
   blocked change stays in the outbox, marked, and is not sent again until the row changes.*

**Outcome.** The two ordinary snippets are everywhere. The large one stays on device A, where it
keeps working, marked as not synced with the reason; it is sent again when the user shortens it.

| Row | Server | A | B |
| --- | --- | --- | --- |
| `f1` | live · start | = | = |
| `s2` | live · a1 | = | = |
| `s3` | absent | live · a2 | = |
| `s4` | live · a3 | = | = |

- History (replaced versions the server keeps): none.
- Conflicts: none.
- A: shown blocked; blocked: a2; 1 change waiting in the outbox.
- B: shown nothing.

#### S46 · The plan does not allow another shared folder

Category: sharing. Covers: plan-limit. Rules: C11, D5.

**Start.** Ines's plan allows one shared folder and f1 is shared with the family already. She shares
f2 as well.

Devices: A (ines), C (tom). Profiles: `p-ines` (personal, ines), `p-tom` (personal, tom), `g-fam`
(family, ines).

| Row | Kind | Refers to | Rev | Fields |
| --- | --- | --- | --- | --- |
| `m-ines` | membership | group `g-fam`, user `ines` | 2 | role `owner` |
| `m-tom` | membership | group `g-fam`, user `tom` | 6 | role `member` |
| `f1` | folder | profile `p-ines` | 3 | name `Shop replies` |
| `f2` | folder | profile `p-ines` | 4 | name `Returns` |
| `sh1` | folder share | folder `f1`, group `g-fam` | 7 | permission `view` |

**Steps.**

1. `k1` · The plan's limit `shared_folders_max` is 1.
2. `a1` · A creates the folder share `sh2` (folder `f2`, group `g-fam`): permission `view`. Answer:
   `sh2` `rejected:plan_limit`. *A change that gives access is never left waiting on a device: it is
   refused, and the device removes its share row (C11, D5). Content rows over a plan's limit are
   blocked instead and stay local.*

**Outcome.** The second folder is not shared and no device thinks it is. Ines's device shows the
sentence the server gives for the limit. The first share is untouched.

| Row | Server | A | C |
| --- | --- | --- | --- |
| `m-ines` | live · start | = | = |
| `m-tom` | live · start | = | = |
| `f1` | live · start | = | = |
| `f2` | live · start | = | absent |
| `sh1` | live · start | = | = |
| `sh2` | absent | = | = |

- History (replaced versions the server keeps): none.
- Conflicts: none.
- A: shown plan_limit.
- C: shown nothing.

#### S47 · The live channel is down

Category: transport. Covers: channel-down. Rules: D11.

**Start.** Device B is in use and its live channel has dropped, while its ordinary requests still
work. Device A edits s1.

Devices: A (ines), B (ines). Profiles: `p-ines` (personal, ines).

| Row | Kind | Refers to | Rev | Fields |
| --- | --- | --- | --- | --- |
| `f1` | folder | profile `p-ines` | 3 | name `Replies` |
| `s1` | snippet | folder `f1` | 5 | abbreviation `;rty`, body `Thanks for your order.` |

**Steps.**

1. B's live channel drops.
2. `a1` · A edits `s1`: body `Thank you for your order.`. Answer: `s1` `applied`. *No poke reaches
   B. While its channel is down B asks for the feed every 2 seconds, and it keeps trying to
   reconnect the channel.*
3. B pulls. *The next poll, at most 2 seconds later, brings a1. The channel carries no content, so
   nothing is different about how the change is applied.*

**Outcome.** B has the change within the five seconds without the channel. Nothing is shown.

| Row | Server | A | B |
| --- | --- | --- | --- |
| `f1` | live · start | = | = |
| `s1` | live · a1 | = | = |

- History (replaced versions the server keeps): `s1` start.
- Conflicts: none.
- A: shown nothing.
- B: shown nothing.

#### S48 · A second computer with its own snippets signs in for the first time

Category: server. Covers: first-sign-in. Rules: C9, D9, D13.

**Start.** Ines uses device A with an account: the folder f1 "Snippets" holds s1 (;em) and s2
(;sig). Device B was used without an account: its local profile holds a folder lf1, also called
"Snippets", with ls1 (;em, another address) and ls2 (;addr). Ines signs in on B.

Devices: A (ines), B (ines, not signed in). Profiles: `p-ines` (personal, ines), `lp-b` (personal,
ines, local to B).

| Row | Kind | Refers to | Rev | Fields |
| --- | --- | --- | --- | --- |
| `f1` | folder | profile `p-ines` | 3 | name `Snippets`; only on A |
| `s1` | snippet | folder `f1` | 5 | abbreviation `;em`, body `ines@brindlewood.example`; only on A |
| `s2` | snippet | folder `f1` | 6 | abbreviation `;sig`, body `Kind regards, Ines`; only on A |
| `lf1` | folder | profile `lp-b` | 0 | name `Snippets`; only on B |
| `ls1` | snippet | folder `lf1` | 0 | abbreviation `;em`, body `ines.marchetti@mail.example`; only on B |
| `ls2` | snippet | folder `lf1` | 0 | abbreviation `;addr`, body `12 Millrace Lane`; only on B |

**Steps.**

1. `b1` · B: ines signs in. Answer: `ls1` `applied`, `ls2` `applied`. *B's library is adopted (D9):
   its rows move to the account's personal profile; B fetches the account's library; a local folder
   with the name and place of an account folder is merged into it, so ls1 and ls2 move into f1 and
   lf1 is dropped without ever being sent; a local snippet identical to one in the account would be
   dropped too. What is left is pushed as new rows.*

**Outcome.** Both computers hold one folder "Snippets" with all four snippets. The two ;em snippets
differ, so both are kept: s1, created first, holds the abbreviation and library health lists ls1 on
both computers. Nothing that was on either computer is lost.

| Row | Server | A | B |
| --- | --- | --- | --- |
| `f1` | live · start | = | = |
| `s1` | live · start | = | = |
| `s2` | live · start | = | = |
| `lf1` | absent | = | = |
| `ls1` | live · b1 (folder `f1`) | = | = |
| `ls2` | live · b1 (folder `f1`) | = | = |

- History (replaced versions the server keeps): none.
- Conflicts: none.
- A: shown duplicate; expands `;em` as `s1`.
- B: shown duplicate; expands `;em` as `s1`.

#### S49 · The user signs out with an unsent change and signs in again

Category: offline. Covers: sign-out. Rules: C3, D2, D7, D14.

**Start.** Device A is offline and edits s1. Ines signs out on A, and signs in again later with the
same account. In between, device B edits the same snippet.

Devices: A (ines), B (ines). Profiles: `p-ines` (personal, ines).

| Row | Kind | Refers to | Rev | Fields |
| --- | --- | --- | --- | --- |
| `f1` | folder | profile `p-ines` | 3 | name `Replies` |
| `s1` | snippet | folder `f1` | 5 | abbreviation `;rty`, body `Thanks for your order.` |

**Steps.**

1. A goes offline.
2. `a1` · A edits `s1`: body `Thank you for your order.`.
3. `x1` · A: the user signs out. *Signing out removes the tokens and everything that belongs to
   other people's profiles. The user's own library stays on the computer and keeps working, with its
   outbox, its revisions and the id of the account it belongs to (D14).*
4. `b1` · B edits `s1`: body `Thanks a lot for your order.`. Answer: `s1` `applied`.
5. A comes back online and syncs.
6. `x2` · A: ines signs in. Answer: `s1` `applied+conflict`. *The same account as before: nothing is
   adopted again. As after every sign-in A resyncs, which leaves rows with unsent changes alone, and
   then pushes the waiting change, which is stale by now and wins.*

**Outcome.** The change made before signing out is not lost: it is pushed at the next sign-in and
wins over b1, which a conflict offers back.

| Row | Server | A | B |
| --- | --- | --- | --- |
| `f1` | live · start | = | = |
| `s1` | live · a1 | = | = |

- History (replaced versions the server keeps): `s1` start, b1.
- Conflicts: `s1` edit_edit, winner a1, loser b1, shown to ines, open.
- A: shown conflict.
- B: shown conflict.

<!-- scenarios:end -->

## 13. Open questions for the owner

Each is decided one way in this document so that the design is complete; each is the owner's to
change, and the cost of the other answer is stated.

| # | Question | Recommendation | If decided otherwise |
| --- | --- | --- | --- |
| 1 | **Does signing in sync the whole personal library?** Flow 6 of the UX specification tells a person who signs in to share one folder: "Your other folders stay on this computer until you choose to sync or share them." Under this design signing in adopts the library and syncs all of it (D9). Both cannot be true | Sync everything in version 1, and change that sentence to say so: "Signing in keeps your snippets in step between your computers. Only folders you share can be seen by other people." It is what the Phase 4 gate tests (two devices of one account), and it is simple enough to prove free of lost edits | A switch per device, "Keep my own snippets on this computer only", is one gate and no server change: the device neither adopts nor pushes its personal profile and still receives shared folders; it cannot share a folder of its own while the switch is on. Sharing single folders while the rest stays local, as the sentence promises, needs a sync scope per folder with its own rules for tags, attachments and moves between synced and local folders; that is a feature of its own and should not be in version 1 |
| 2 | **Should the server be unable to read snippets (end-to-end encryption)?** | Not in version 1. Say plainly in the privacy text that synced snippets are stored on the server and readable by its operator, keep the server from interpreting content (C9) so that encryption can be added per profile later, and decide again before team plans are sold | End-to-end encryption brings a recovery key the user must keep (there is no password), a key per shared folder handed over by a member's device (the 10-second target then depends on one being online), re-keying on removal, and no server-side check of attachment hashes; roughly a phase of work, and it changes tasks 4.2, 4.4, 4.5 and 4.6 |
| 3 | **Where is the server hosted, and on which hosting plan?** The plan decides how often the database is backed up, and so how much a restore loses before the devices give it back (C18) | One region in the European Union, and say so in the privacy text; the 5 and 10 second targets leave room for a distant region. A plan with daily backups at least, and point-in-time recovery before paying customers depend on it | Another region changes nothing in the design; more than one region is not supported by it. Without backups a lost database is rebuilt only from what devices hold |
| 4 | **Plan limits and rate limits.** They decide cost and pricing: devices per account, snippets, shared folders, groups owned, members per group, storage, days of history; and the rate limits of §6.8 | Start with one default plan per scope and generous values while there is no billing: 5 devices, 10,000 snippets, 10 shared folders, 3 groups owned, 10 members per group, 500 MB of files, 90 days of history; the rate limits as in §6.8. All are rows in `plan` and settings of the server, changed without a release | Any values work; only `history_days` above 90 needs more storage, and a limit lowered later never deletes rows (§5.10) |
| 5 | **How long is replaced content kept?** Tombstones for 30 days is fixed by the architecture. History for 90 days and open conflicts for 90 days are chosen here | Confirm 90 and 90. Longer history can become a plan feature (`history_days`) | Shorter history means a conflict can lose its other version sooner; it must stay above the 30 days of the tombstones |
| 6 | **Deleting an account: at once or after a waiting time?** | At once. Every device keeps the whole library locally, so the user loses nothing they have not asked to lose, and "delete" means delete. Backups age out with the hosting provider's retention, which the privacy text must name | A waiting time of 7 or 30 days needs a "deletion pending" state on the account and a way back in |
| 7 | **Who may share a folder with a group?** | Every member, for families and teams alike, as flow 6 shows. A share only adds a folder that members can leave unused | "Owner and admins only" for teams is one more check in C11 and a flag on the group |
| 8 | **Invitation emails** need a mail-sending service and a sender address under Iamb's own domain; the sign-in emails need the same | Use one service for both, set up in Supabase's authentication settings, before task 4.5 is tested with real addresses | — |
| 9 | **May someone run their own server?** The server's code is AGPL like the rest | Keep `server/` complete enough to be run by others, and let the address of the server be a value in a configuration file without interface in version 1 | A fixed address is simpler to support and to sign; it can be opened later |

## 14. What each task takes from here

| Task | Builds | From |
| --- | --- | --- |
| 4.2 Server schema and policies | The schemas, tables, constraints and indexes; `refresh_access` and its slow reference; the feed with its ordering rule; the functions of `api` with the rules C1 to C23; the read policies; the storage and live-channel policies; the daily job; `begin_epoch` and the restore procedure; the default plans | §4, §5, §6.1 to §6.5, §6.11, §8, §11.5 |
| 4.3 Security review | Tests of G1 to G20 as each kind of user; the queries of §4.5; the required authentication settings of §11.3; the log check | §4.5, §5.13, §11 |
| 4.4 Sync client | `iamb-sync`; the store's migration 3 and the Sync messages; D1 to D14; the effective library over all profiles with D13; the scenario replay and the simulation with the invariants I1 to I6; the 5-second measurement | §1, §6, §7, §9, §12 |
| 4.5 Account interface | Sign-in with the sentence of question 1; groups, invitations, members and roles on the account endpoint; the share dialog (with the count of snippets that include snippets outside the folder, and the sentence about copies that stay with removed members); conflicts ("Show the other version", "Restore", "Keep"); the list of changes that were not synced; blocked rows; devices and "sign out everywhere"; the review dialog with author, time and difference; the 10-second measurement | §4, §5.6, §7.2, D5, D10, §11.3, §12 ("Shown") |
| 4.6 Media sync | Upload and download with the server-side hash check; the size limits; removal of files nothing uses | §5.9, §6.11, C19 |
| 4.7 Billing readiness | `plan`, `subscription`, the flags; `entitlements()`; the enforcement points; the values of question 4 | §5.10, §6.12, C11, C14 |

## 15. What changes in the architecture and the decision records

The architecture's frame holds. Three new records add to it, and the architecture document
points to them where they apply.

| Record | Adds or amends | Why |
| --- | --- | --- |
| [ADR-0027](decisions/0027-sync-protocol-and-conflict-rules.md) | Adds to ADR-0016: the server's revision counter and clock as the only order; writes through functions with receipts; deletes as writes, with the folder cascade and the restored folder; unsent changes are never overwritten by a pull; refused changes are kept on the device and blocked ones stay local; resync and epochs | ADR-0016 fixed the frame and left the rules to this task |
| [ADR-0028](decisions/0028-data-model-for-sync.md) | Amends ADR-0014: abbreviations are not unique on the server, and the store enforces uniqueness for local edits only; a share's id is derived; profiles and memberships are written by the server only; access entries. Amends ADR-0022: the server records the author of a snippet's content, and a device approves content, not a revision | Uniqueness per user's view cannot be a server constraint; sharing must be safe against late changes; a review that follows the row's last writer can be bypassed by an innocent edit |
| [ADR-0029](decisions/0029-server-readable-content.md) | New: synced content is readable by the server in version 1, the server never interprets it, and end-to-end encryption stays possible. Proposed, with open question 2 | Architecture §8 did not say; it has to be said before anything is stored |

In `architecture.md`: §4.4 (the store enforces uniqueness for local edits; precedence inside a
profile), §6.2 and §6.4 (the messages of §7.2), §7 (the device-local tables of §7.1, the
server-set fields, who writes profiles and memberships), §8 rule 10 (review follows the author
of the content), §9 (a summary of the rules and a pointer here) and §11 (this document's name).
Nothing in §8 rule 1 changes.
