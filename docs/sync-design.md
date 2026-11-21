# Iamb sync design

Status: proposed for the owner's approval; revised after its independent review
([`reviews/4.1-sync-design-review.md`](reviews/4.1-sync-design-review.md), whose last section
says where each finding went) · Task 4.1 · Builds on [`architecture.md`](architecture.md) §6, §7,
§8 and §9 and on ADR-0014, ADR-0016, ADR-0020 and ADR-0022 · Decisions recorded in
[ADR-0027](decisions/0027-sync-protocol-and-conflict-rules.md),
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
and the server talk and what a device stores; §8 and §9 are the rules, numbered `C1` to `C25` for
the server and `D1` to `D17` for the device, and they are what the scenarios of §12 cite; §10
answers the conflict questions one by one; §11 is privacy and security, and §11.6 lists what the
design assumes of the hosting platform and how each assumption was checked or avoided.

## 1. What the design must achieve

| Goal | Target | Where |
| --- | --- | --- |
| Two online devices of one account show the same library | within 5 seconds of a change (p95) | §6.6, §6.7 |
| A newly shared folder appears for the people it is shared with | within 10 seconds while they are online | §6.4, D6 |
| No edit is lost | 0 in 1,000 simulated runs with random edits, offline periods and clock skew | below, §8, §9 |
| A group gets exactly the folder that was shared and nothing else | every query as a member returns only that folder's content | §4.4, §4.5, §11.5 |
| Content whose access ended disappears from devices | at the next sync; at the latest at the daily access check, which a device keeps running even when it may not sync | D6, D7, D12, D16 |
| Editing works offline | every local change is queued in the transaction that makes it | §7, D1 |
| The service stays without network code | only `iamb-sync` talks to the server; it never sees keys | §11.2 |
| Typed text, form values and usage never reach the server | not part of any synced row | §11.2 |

**What "no lost edit" means.** A local change is one action of a user on one row on one device: a
new row, a save, a move, a delete, a restore. A folder's delete is one change, of the folder; what
it takes with it is decided by the server (C6). When every device has been online long enough to
finish syncing, each change is in exactly one of these places:

| | Place | Rule |
| --- | --- | --- |
| 1 | It is the row's current version on the server | C1, C2 |
| 2 | It is in the server's history because a later write replaced it knowingly: that write was based on it or on a later version | C2 |
| 3 | It is in the server's history because a later write replaced it without having seen it, and a conflict offers it back to the people concerned: as the version that conflict's write replaced, or as one a later conflict on the row took over | C3 |
| 4 | The server refused it, or the folder it was made in left the device before it was sent: the device that made it keeps it in the list of changes that were not synced, with its content (a refused delete as a line that says what was not deleted) | D5, D6, D14 |
| 5 | The server cannot store it yet (too large, over a plan's limit, a request that keeps failing): the row stays on the device that made it, works there, and is marked as not synced | D5, D17 |
| 6 | It was a position in a list (the order of folders), which is arrangement and not content: the last write wins without a record | C3 |
| 7 | A later change to the same row on the same device took its place before it was sent; that later change then stands for it. A row created and deleted again before any of its entries was handed to the agent ends here too | D1 |
| 8 | The server already held exactly that (`unchanged`) | C4, C5 |
| 9 | The server made it differently and the device says so: a move that would have broken the folder tree (`adjusted`), a change to a share made without the share's latest state (`superseded`) | C8, C11 |
| 10 | At the first sign-in the row was merged into one of the account that is identical, or a folder into one of the same name and place: its content is there | D9 |
| 11 | The user discarded it: dismissed from the list of place 4, or chose "discard" when signing out | D5, D14 |

Never: nowhere. The simulation of task 4.4 records every local change with its device, row and
content and, at the end of a run, puts each into one of these places from what the server and the
devices hold: the server's rows and history, the conflicts with all the versions they offer, each
device's list of kept changes, its blocked rows and its outbox. A change it can put nowhere fails
the run. It checks this together with the other invariants:

| | Invariant |
| --- | --- |
| I1 | **Convergence.** When all devices are online and idle, every device of a user holds exactly the live rows the server shows that user, in the same revisions, and no row the server does not show that user; rows blocked on that device (place 5) are the exception. For a row the server holds as a tombstone, the device holds the same tombstone or nothing |
| I2 | **No lost edit**, as defined above |
| I3 | **Revisions only grow.** No device replaces a row by an older revision of it. The one exception is the reconcile after the server was restored from a backup, where rows only the server writes are taken as the server has them (D7) |
| I4 | **No access leak.** After a device has synced past the end of an access, it holds none of the rows that access gave it, and no tag, attachment or file that only those rows made readable (D6, D16) |
| I5 | **Exactly once.** A change that is sent several times makes one revision |
| I6 | **Same answer everywhere.** Which snippet holds an abbreviation, and what a user may do in a folder, is the same on all devices of that user that have finished syncing. (Which snippets wait for review is per device by design, D10) |
| I7 | **A valid tree, always.** After every commit the server holds no folder cycle, no tree deeper than four levels, no live row under a deleted folder, and no access entry without the share or membership that gives it (C24) |

## 2. The main choices

| Question | Choice | Why |
| --- | --- | --- |
| Which write is "last"? | The one the server accepts last. The server gives every accepted write a revision from one growing counter and its own time. Device clocks decide nothing (C1) | ADR-0016 rejects client clocks as the authority; skewed or wrong clocks then cannot make devices disagree or let an old edit win by its date |
| Row or field? | Row. A write carries the whole row and replaces it (C2). Fields are not merged | A snippet's fields are one behaviour (abbreviation, trigger and content together), a merged row would be a revision nobody wrote and nobody approved (ADR-0022), and ADR-0016 already says so. Conflicts are rare for one person's snippets and every one is visible and reversible |
| What about the version that lost? | It goes to a history table on the server. If the winner had not seen it, a conflict is recorded and shown to both authors, who can restore it; a conflict that a newer one replaces hands its versions on (C3, C20) | "A stale offline edit still wins, but visibly and reversibly" (ADR-0016), also with three writers |
| Deletes? | A delete is a write like any other: whichever of an edit and a delete arrives last wins, and the other is offered back for 30 days (C5). A folder's delete takes the whole subtree as it is on the server (C6); a later write into a deleted folder brings the folder back (C7) | One rule in both orders is easier to hold in the head and to test than "edits always beat deletes"; nothing disappears without a record |
| What may two requests assume about each other? | Nothing: every writing transaction first locks the profiles it writes, in one fixed order, so the writes of one profile happen one after the other and each sees what the one before committed (C24) | The checks of §8 (no cycle, no live row under a deleted folder, access entries equal to the shares) are only true if nothing changes between the check and the write. One lock per profile is cheap at this size and leaves task 4.2 nothing to choose |
| Does the server enforce unique abbreviations? | No. The server never reads `abbreviation`, `label`, `body` or names (C9). Devices refuse duplicates in their own edits, and what sync brings together is ordered by a fixed precedence that no device's clock and no other person's write can turn against the user's own snippets (D13) | Uniqueness is per user's view (personal plus shared folders), which no server constraint can express; refusing a row would leave an edit that exists on one device only; and a server that does not interpret content leaves room for end-to-end encryption later (§13) |
| How does a device send changes? | One request to a database function with up to 200 changed rows, each with an id that makes a repeat harmless (C15) and the revision it was based on. Each row gets its own answer, and a folder's delete an answer for every row it named (§6.3) | Partial failures stay partial; retries after a lost answer cannot write twice; no row is left without an answer |
| How does a device learn of changes? | It reads its user's feed from its position, in pages. A live channel only says "there is something new"; it carries no content (§6.4, §6.6) | One code path applies changes whether they come after a poke, a poll or a month offline; a dropped channel costs seconds, never correctness |
| What if a device skips a row it then never hears of again? | It cannot stay that way: the store notes every pulled row it skipped because of an unsent change, and fetches the row once the change is settled and the row is still behind (D15) | The feed has one entry per row, and a position moves past entries that were not applied; without the note, a lost answer or a refused delete could leave two devices of one account different for good |
| Who may write tables? | Nobody directly. Every table has row-level security with read rules only; every write goes through a function that checks the rules of §4 against the current state (§5.13) | The write rules need the previous version, the history, the feed and the access entries in one transaction; policies alone cannot do that, and the review of task 4.3 can test each function as each kind of user |
| How is "may this user read this folder" answered fast? | A table of access entries, one per user and folder, kept in step with shares and memberships by the functions that change them. Policies look it up; `folder_visible` and `folder_revoked` are exactly its additions and removals (§5.4, C21) | Sharing and unsharing reach devices from the same fact the policies use, so they cannot disagree |
| Sharing changes that race? | A share has one row per folder and group, with an id derived from the two. A stale change to it never wins; a delete always applies (C11) | Access must not widen because a device was late; "stop sharing" must stick |
| Who wrote a snippet's content? | The server records it (`content_by`), separately from who last touched the row, and names the writer as author only of content the writer changed against the version the write was based on. A device holds a snippet for review by that author, and remembers what it approved as content, not as a revision number (C17, D10) | Otherwise the owner's own label change, sent from a device that was a moment behind, would make a member's key presses count as the owner's (scenarios S40, S55) |
| How is a device told from a copy of itself? | Every position a device holds was issued by the server and can be used once; a second installation with the same device id is noticed at its next call and comes back as a device of its own (C25) | A data directory copied to another computer would otherwise be one device to the server, and a write is never in conflict with the same device's own earlier one |
| Is snippet content hidden from the server? | Not in version 1: it is protected in transit and at rest, and readable by the server (§11.1) | End-to-end encryption needs key distribution, recovery without a password, and a member online to hand a key to a new member, against the 10-second target. It is open question 2 (§13) |
| What does signing in do to the library on a device? | It adopts it into the account and syncs all of it (D9) | The gate is two devices of one account in sync. A choice of what stays local is a later option; the sign-in dialog must say what happens (open question 1, §13) |

## 3. Identities

### 3.1 Accounts

An account is one person: a row in the authentication service's user table with a confirmed email
address. Sign-in is the browser flow of ADR-0020 (authorisation code with PKCE; magic link or an
identity provider); tokens live in the operating system's keychain and only `iamb-sync` reads
them. The server keeps a `user_profile` row per account with the display name people in the same
group see ("Ines Marchetti"). The name starts empty and the person chooses it: no part of the
email address is ever shown as a name, and the calls that make a person visible to others
(creating a group, inviting, accepting an invitation) are refused until it is set (§5.14). The
email address is shown to the owner and admins of a group the person belongs to or is invited to,
and to nobody else.

### 3.2 Devices

A device is one installation of Iamb signed in to an account. It makes its own id (UUID version 7)
the first time it signs in and keeps it in `sync_state`. It registers with `hello` (§6.2), which
binds the device row to the authentication session of the token used: one session has one device
row and one device row one session (C25). The device row holds a name the user can change
(default "Mac", "Windows PC"; the host name is not sent), the platform, the app version and the
protocol version. Devices exist for four reasons: the receipts that make a repeated push harmless
are kept per device (C15); a conflict can tell the user on which of their own devices the other
version was written; the user can see the list and revoke one (§11.3); and a write is never in
conflict with an earlier write of the same change sequence on the same device (C3).

A device id lives in the data directory, and data directories get copied: by a system's migration
tool, by a restored backup of the home folder. Two installations with one id are told apart by
the position each holds, which the server issues and accepts once (C25); the one that is behind
signs itself out and comes back with an id of its own.

### 3.3 Profiles, memberships, roles

As in architecture §7 and ADR-0014:

- Every account has exactly one **personal** profile. The server creates it at the account's
  first `hello`. Its name is always `Personal`; people see the owner's display name instead.
- A **group** is a profile of kind `family` or `company` (the interface says "family" and "team").
  The rules are the same for both, except who may share a folder with the group (§4.1). A group
  has members through `membership` rows with the role `owner`, `admin` or `member`. It has
  exactly one owner at every moment, and the profile's `owner_user_id` names the same person.
- There are no groups inside groups and no sharing with single addresses or by link.

A folder belongs to exactly one profile and never changes it. Moving content from a personal
profile to a group, or between groups, is a copy made by the device (new rows) followed by a
delete; the server refuses a row whose profile changes (C8), and the store refuses to copy a
snippet that waits for review (D10).

### 3.4 Ids

| Row | Id | Made by |
| --- | --- | --- |
| Folder, snippet, tag, attachment, the two kinds of link | UUID version 7 | The device that creates the row |
| Folder share | Derived: the first 16 bytes of SHA-256 over `iamb:folder_share:` followed by the 16 bytes of the folder id and the 16 bytes of the group's profile id, with the version set to 8 and the variant to `10` | Any device; the same pair gives the same id everywhere (C11) |
| Group profile, invitation | UUID version 7 | The device that makes the call, so that a repeated call is harmless |
| Personal profile, membership, conflict, attachment upload | UUID version 7 | The server |
| Device | UUID version 7 | The device; a new one whenever the server answers `device_conflict`, and at the first sign-in after `device_revoked` (C25, D14) |
| Change sent in a push (`op_id`) | UUID | The store, when it writes the outbox entry |

## 4. Who may do what

### 4.1 Roles in a group

| Call | Owner | Admin | Member |
| --- | --- | --- | --- |
| See the group, its members' display names and roles | yes | yes | yes |
| See members' and invited people's email addresses (`members`, `invitations`) | yes | yes | no |
| Invite (as member or admin), withdraw an invitation, whoever sent it | yes | yes | no |
| Remove a member or an admin; change a role between member and admin | yes | yes | no |
| Remove the owner, or change the owner's role | no | no | no |
| Rename the group | yes | yes | no |
| Offer the ownership to a member or admin; the change happens when that person accepts, and the old owner becomes admin | yes | no | no |
| Delete the group | yes | no | no |
| Leave the group | no (hand it over first) | yes | yes |
| Share one of their own folders with the group | yes | yes | family: yes · team: no (open question 7) |

These calls need the server and are not queued offline: an invitation sends an email, a membership
is what gives access, and both are subject to the plan (§5.10). The device creates none of these
rows itself; it receives them through the feed (C16). §5.14 lists every call with the codes it
can answer.

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
  can only copy (architecture §7). The access root itself, wherever it sits in its owner's tree,
  is changed by `manage` only: a share gives its members what is in the folder, never the folder
  (§4.3).
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
| Folder that is the root of a live share, at any depth of its owner's tree | (as the kind of folder it is) | `manage`, for every field: name, defaults, position, parent | `manage` |
| Any other sub-folder | `edit` on the parent | Name, defaults, position, moving within the access root: `edit` on it and on the new parent. Anything else: `manage` | `edit` on its parent |
| Snippet | `edit` on the folder | `edit` on the folder it is in and on the folder it moves to; both in one access root unless `manage` | `edit` on the folder |
| Tag (belongs to a profile) | `edit` on at least one folder of the profile | `manage` in the profile | `manage` in the profile |
| Snippet–tag link | `edit` on the snippet's folder; the tag is in the folder's profile | — (a link has no fields) | `edit` on the snippet's folder |
| Attachment (belongs to a profile) | `edit` on at least one folder of the profile | — (never changes) | `manage` in the profile; the server removes attachments nothing uses (C19) |
| Snippet–attachment link | `edit` on the snippet's folder; the attachment is in the folder's profile | — | `edit` on the snippet's folder |
| Folder share | `manage` on the folder, the folder is in a personal profile, the writer is a member of the group and, in a team, its owner or an admin | The same | `manage` on the folder |
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
from other snippets, even to a visible tag; attachments no visible snippet uses, and their files,
also when the member knows a file's hash (§5.13); the history of any row (§5.6); the owner's
other shares, devices, entitlements; and, as always, anything that never leaves a device (§11.2).
A member who may change the folder can also read the tags and attachment rows they wrote
themselves there, for as long as they may change it; that gives them no file they did not upload.
A snippet in `F` that includes a snippet outside `F` with `{snippet}` does not resolve for the
members (`E_SNIPPET_NOT_FOUND`); the share dialog should say how many such snippets the folder
has, which the service can count from the snapshot.

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
| Tags of `P` | Exactly the tags carried by those snippets (and, with `edit`, the tags `T` wrote) |
| Snippet–tag links whose tag is in `P` | Exactly the links of those snippets; not the link from a private snippet to a visible tag |
| Attachments of `P` | Exactly those linked from those snippets (and, with `edit`, the rows `T` wrote) |
| Files under `P/` in storage | Exactly the verified files of attachments linked from those snippets; a request for any other file, also one whose hash `T` knows, answers as for a file that does not exist |
| Shares of folders of `P` | Exactly the share to `G` |
| History rows of `P`'s rows | None |
| The same queries after the share row is deleted, or `T` removed | Nothing, including what `T` wrote there |
| Any insert, update or delete on any table | Refused |

Task 4.3 tests every line (§11.5).

## 5. Server data model

### 5.1 Conventions

- **Schemas.** `iamb` holds the tables and is not exposed through Supabase's REST interface.
  `api` holds the functions devices call and is the only schema exposed to them. `api_service`
  holds the two functions the edge functions call with the service key (§6.11, §11.1); the REST
  interface exposes it to `service_role` only. `iamb_private` holds helper functions; it is not
  exposed, so nothing in it can be called through the interface. Supabase's own `auth` (users,
  sessions), `storage` (objects) and `realtime` are used as they are.
- **Roles.** `anon` has no privilege on anything of Iamb. `authenticated` may read tables through
  their policies, execute the functions of `api`, and execute the *gate helpers* listed below,
  which the policies and the reading functions call for it. The daily job runs as the tables'
  owner. The service key exists in one place, the secrets of the edge functions; no device ever
  holds it.
- **Row-level security** is enabled on every table of `iamb`. Policies are read rules only
  (§5.13). There is no insert, update or delete policy and no such grant: every write goes
  through a function. The functions that write are `security definer`, owned by the tables'
  owner, with a fixed `search_path`; they take the caller from the token (`auth.uid()`, the
  session id claim) and never from an argument, and check §4 themselves. That check is the only
  one they are under: on Supabase the owner of the tables is not subject to row-level security,
  "forced" or not, so the policies protect reads by devices and nothing a definer function does.
- **Reading functions** (`pull`, `rows`, `fetch_folder`, `snapshot`, `visible_folders`,
  `access`, `users`, `members`) are `security invoker`: each calls `iamb_private.enter` first
  and then returns rows through one query under the caller's policies, so a mistake in the feed
  cannot show a row its reader may not read.
- **Gate helpers.** A policy cannot read the table it protects, and an invoker function cannot
  write a counter its caller may not write. These `security definer` functions of `iamb_private`
  do that for them. `authenticated` may execute exactly these; each takes the caller from the
  token, returns nothing about anyone else, and writes nothing but the caller's own device row
  and rate counters:

  | Helper | Returns | Used by |
  | --- | --- | --- |
  | `session_ok()` | Whether the token's session has a device row that is not revoked and still exists in `auth.sessions` | Every policy |
  | `member_of(profile)`, `role_in(profile)` | Whether the caller has a live membership in the group; their role | The policies on `membership`, `profile`, `folder_share`, `tag`, `attachment`, `invitation`, `user_profile` |
  | `shares_group_with(user)` | Whether the caller and that account have a live membership in one group | The policy on `user_profile` |
  | `own_address()` | The caller's confirmed email address, or null when it is not confirmed (§5.8) | The policy on `invitation` |
  | `enter(device, protocol, call)` | An error code or null, and the current epoch. It checks the session and the device row (C25), compares the protocol with the server's range (C23), counts the call in `rate_bucket` (§6.8), notes `device.last_seen_at` (at most once an hour) and begins a new epoch when the caller proves that the server lost data (C18) | Every function of `api`, as its first statement |
  | `position(device, presented, advance)` | A new position token, or `device_conflict` (C25) | `pull`, `snapshot`, `push` |

  Everything else in `iamb_private` (`refresh_access`, `begin_epoch`, the purge, the feed
  writer) is executable by the tables' owner only and is reached only from inside a definer
  function or the daily job.
- **Isolation and locks.** Functions run at Postgres's default isolation level, read committed,
  and are `volatile`, so each statement in them sees what was committed before it began. Every
  writing function takes its locks first, in the order of C24, and reads what it decides on
  afterwards.
- **Time-outs.** Every function of `api` carries its own `statement_timeout` (§5.14), which the
  REST interface applies to the call; the role's default of 8 seconds is not relied on. No
  function may need more than 60 seconds, the most the interface allows. The work of one request
  is bounded so that it stays far below its time-out (§6.3, C6).
- **Types.** Ids are `uuid`. Times are `bigint` UTC milliseconds, as in the local store and the
  model's wire form, so a row is the same value on both sides. Enumerations are `text` with a
  `CHECK`, in the spelling the model serialises (`personal`, `edit`, `delimiter`).
- **Common columns** of the nine synced tables: `id uuid primary key`, `created_at bigint`,
  `updated_at bigint`, `updated_by uuid`, `rev bigint`, `created_rev bigint`, `deleted_at
  bigint` (null while live), and `updated_device uuid` (the device that wrote the revision; null
  for a write the server made; never sent to devices of other users).
- **Revisions.** `rev` comes from one sequence, `iamb.rev_seq`, taken after the profile is
  locked, so the revisions of a row only grow, also across a delete, a purge and a restore of
  the row, and the revisions of one profile grow in the order of its commits. `created_rev` is
  the revision the server gave the row when it first accepted it; it never changes and is what
  orders rows that compete for an abbreviation (D13). `updated_at` is the server's clock at the
  write. `created_at` is what the creating device sent, but never later than the server's
  clock; it never changes afterwards and is used for display only. `updated_by` is the caller.
- **Content columns** (`abbreviation`, `label`, `body`, names, `file_name`) are stored and compared
  for equality, and never parsed, searched or used in a constraint (C9).

### 5.2 Synced tables

The columns beyond the common ones are those of the local store
(`crates/iamb-store/migrations/0001_initial.sql`), with these server-side additions and rules.

| Table | Columns | Server-side rules |
| --- | --- | --- |
| `profile` | `kind`, `name`, `owner_user_id`; added: `offered_owner uuid`, `offered_at bigint` | One personal profile per account (unique on `owner_user_id` where `kind = 'personal'`), created by `hello` with the name `Personal` when the account has none. The two added columns hold a pending offer of the ownership (C16). Written only by account calls. Its row is the lock of C24 |
| `membership` | `profile_id`, `user_id`, `role` | Group profiles only. Unique live `(profile_id, user_id)`; exactly one live `owner` per profile (unique on `profile_id` where `role = 'owner'`). Written only by account calls |
| `folder` | `profile_id`, `parent_id`, `name`, `position`, `member_permission`, `default_trigger`, `default_case_mode`, `abbreviation_prefix` | `profile_id` never changes. The parent is in the same profile. At most four levels and no cycle, kept by C8 under the lock of C24 |
| `folder_share` | `folder_id`, `profile_id`, `permission` | The id is derived from `folder_id` and `profile_id` (§3.4) and checked. The folder is in a personal profile; `profile_id` is a group |
| `snippet` | `folder_id`, `abbreviation`, `label`, `kind`, `body`, `trigger_mode`, `case_mode`, `word_start`, `enabled`; added: `profile_id`, `content_by uuid`, `content_at bigint` | `profile_id` is the folder's profile, kept by the server for the policies. `content_by` and `content_at` say who last changed what the snippet does and when; `content_by` is null when the server cannot say (C17). No unique index on the abbreviation |
| `tag` | `profile_id`, `name`, `color` | No unique index on the name |
| `snippet_tag` | `snippet_id`, `tag_id` | Snippet and tag are in the same profile. Hard-deleted with either end at the purge |
| `attachment` | `profile_id`, `sha256`, `mime`, `size`, `file_name`; added: `unlinked_at bigint` | Never changes after its creation. Several rows may name the same `(profile_id, sha256)` (C19). `unlinked_at` is the server's time since which no live link from a live snippet names the row; null while one does |
| `snippet_attachment` | `snippet_id`, `attachment_id` | As `snippet_tag` |

Size limits, checked by `push` (C14): `body` at most 400,000 bytes (the engine types at most
100,000 characters); `label`, a folder's or tag's `name` and `file_name` at most 1,024 bytes;
`abbreviation` at most 64 characters; `abbreviation_prefix` at most 64 characters; `color` at
most 32 bytes; `mime` at most 127 bytes.

### 5.3 Accounts and devices

| Table | Columns | Notes |
| --- | --- | --- |
| `user_profile` | `user_id` (primary key, the authentication user), `display_name`, `created_at`, `updated_at` | Created by the account's first `hello`. The display name starts empty; the user sets it (§3.1) |
| `device` | `id` (primary key), `user_id`, `session_id` (unique), `name`, `platform`, `app_version`, `protocol`, `created_at`, `last_seen_at`, `revoked_at`, `position_token`, `position_token_before` | One row per authentication session (C25). A revoked row is kept for 30 days and then removed. The two tokens are the last two positions the server issued to the device. The row is the lock on the device's receipts (C15) |

No trigger on the authentication service's tables is needed: `hello` creates the personal profile
and the `user_profile` row when they are missing, in its own transaction, so a failure there
cannot block a sign-up.

### 5.4 Access entries

`folder_access (user_id, folder_id, permission, via, granted_at, root_folder_id)`, primary key
`(user_id, folder_id)`.

One row for every user and folder such that the user may read the folder under §4.2:
`permission` is the highest permission, `via` is `own`, `group` or `share`, `root_folder_id` is the
user's access root for the folder, and `granted_at` is the server's clock at the moment the
user's access to that root began: the time its entry was first written, kept for as long as the
access lasts without a break, whichever grants come and go meanwhile. It is never taken from a
row a device wrote, so no device clock can move it (D13 orders shared folders by it).

A deleted folder keeps exactly the entries it would have if it were live, computed from the
shares and memberships that are live now, until the purge removes them. So the owner of a
deleted folder, the members of the group that owns it, and the members who reach it through a
live share of a folder above it can read its tombstone. A folder whose own share was deleted
with it (C6) has no entry for the people that share served: for them the folder ends as any
share ends, with `folder_revoked` (C21).

`iamb_private.refresh_access` recomputes the entries for the folders, groups or users a change
touches and is called, in the same transaction, by everything that can change them: a folder's
creation, move, delete or restore, a share's creation, change or delete, every membership change
and a group's deletion. Each of those transactions holds the locks of C24, so the folders, shares
and memberships it reads cannot change under it. **Invariant:** after every commit the table
equals the rule of §4.2 computed from `folder`, `folder_share`, `membership` and `profile`. Task
4.2 ships a slow function that computes the rule from those tables alone, and task 4.3 compares
the two after every step of its tests, including the two-connection tests of G21.

Its changes are the visibility entries of the feed (C21): for a user, the top of each newly
readable tree gets `folder_visible`; the top of each tree that stops being readable while the
folder still exists as a row gets `folder_revoked`.

### 5.5 The change feed

`feed (user_id, seq, kind, entity, row_id, folder_id, profile_id, sha256, detail jsonb,
created_at)`, primary key `(user_id, seq)`, and `feed_head (user_id, head, floor)`.

| `kind` | Meaning | The device |
| --- | --- | --- |
| `row` | The row `entity`/`row_id` changed, or became readable or unreadable for this user by a move | Applies the row as `pull` delivers it (D3) |
| `folder_visible` | A tree with the top `folder_id` became readable through a share or a membership | Fetches the tree whole (D6) |
| `folder_revoked` | The tree with the top `folder_id` stopped being readable although it still exists; `detail.except` lists the folders inside it that stay readable through another grant | Purges the tree, except those (D6) |
| `conflict` | A conflict the user is party to was recorded, or its state changed | Tells the UI host (`ConflictRecorded`) |
| `invitations` | An invitation for or by the user changed | Tells the UI host (`AccountChanged`) |
| `entitlements` | The plan of the user or of one of their groups changed | Reads the entitlements again (§6.12) |
| `blob` | The bytes of an attachment (`profile_id`, `sha256`) were verified | Asks the service what is missing (§6.11) |
| `account` | The display name, a device or the account itself changed | Reads the account again; a revoked device signs out |

Rules for task 4.2:

- **Order.** A transaction collects its entries and writes them last, after everything else: it
  locks the `feed_head` rows of all users concerned in the order of the user ids (C24) and gives
  each entry `seq = head + 1`. The locks are held to the commit, so entries become visible in
  `seq` order: a reader that has seen `n` will never later find an entry below `n`.
- **Who gets a `row` entry.** Everyone who could read the row before the write or can read it
  after it. The writer's own feed gets it too: that is how the other devices of the same account
  hear of the change. A snippet that becomes readable for a user through a move brings its
  links: each gets a `row` entry for that user, and the tags and attachments they name come
  with the page (§6.4). A new member's feed gets a `row` entry for the group's profile and for
  each live membership of the group.
- **One entry per row.** A new `row` entry for the same user and row replaces the older one. The
  feed of a user is therefore never longer than the rows they can read plus the other kinds of
  entry of the last 30 days. Visibility entries are never replaced or merged: a device handles
  them in the order of the feed (D6).
- **Content is not in the feed.** `pull` joins the entry to the row as it is at that moment,
  through the reader's policies.
- **Retention.** Entries older than 30 days are deleted and `floor` is set to the highest `seq`
  deleted. A position below `floor` cannot be continued (`reset_required`, D7).
- **Poke.** After the entries are written the function sends `{ "epoch": …, "head": … }` on the
  user's live channel (§6.6); the service delivers it after the commit.

### 5.6 History and conflicts

`history (entity, row_id, rev, row jsonb, deleted, written_at, written_by, written_device,
replaced_at, replaced_by_rev)`, primary key `(entity, row_id, rev)`: every version a write
replaced, as it was, tombstones included; a snippet's version with its `content_by` and
`content_at`. Invitations are not synced rows and have no history.

`conflict (id, entity, row_id, profile_id, kind, winner_rev, winner_user, winner_device,
created_at, state, resolved_at, resolved_by, resolution)` and `conflict_version (conflict_id,
rev, user_id, device_id, taken_over)`: the versions the conflict offers back. The first is the
version the conflict's write replaced; the others, marked `taken_over`, are the versions of the
conflict it replaced (C3). `kind` is `edit_edit`, `edit_over_delete`, `delete_over_edit`,
`deleted_with_folder` or `folder_restored` and describes the write against the version it
replaced; `state` is `open`, `resolved`, `replaced` (a newer conflict on the same row took it
over, with its versions) or `expired`. At most one conflict per row is open. The content of an
offered version is the history row `(entity, row_id, rev)`; the winner's is the row itself until
it changes again. The parties of a conflict are the winner's author and the author of every
version it offers.

What a conflict gives the user (through `Conflicts` and `GetConflict` on the account endpoint):
the row, the current version and each offered version with its author's display name and time,
which fields differ (computed by the UI from the versions), and for each offered version
"Restore this version", besides "Keep the current version" (C20). A version the reader wrote
also names the device it was written on; another person's version names the person only (§5.1).
The interface shows it in the editor ("Changed on another device" · [Show the other version],
flow 6) and in a list.

History is not a feature of its own in version 1: a client reads a history row only through a
conflict it is party to. There is no read policy on `history`.

### 5.7 Receipts

`push_receipt (device_id, op_id, status, reason, rev, conflict_id, created_at)`, primary key
`(device_id, op_id)`: what the server answered to a pushed change, with the revision it wrote,
kept 30 days (C15). C3 reads it to know a device's own earlier change.

### 5.8 Invitations

`invitation (id, profile_id, email, role, invited_by, created_at, expires_at, status, answered_at,
accepted_by)`. `email` is stored in lower case; `role` is `admin` or `member`; `status` is
`pending`, `accepted`, `declined`, `withdrawn` or `expired`; `expires_at` is 14 days after
creation. At most one pending invitation per group and address. An invitation is not a synced
row: it has no revision and no history, and devices read it through `invitations()`.

An invitation is matched to an account by the account's **confirmed** address, read at the time
of the call from the authentication service's user table: `email` of the row of `auth.uid()`,
and only when `email_confirmed_at` is set. A claim of the token is not used for this. An account
whose address is not confirmed sees and accepts no invitation (§11.3 has the settings this
needs). An invitation belongs to the group, not to the person who sent it: it stays valid when
the inviter leaves or is removed, and any owner or admin can withdraw it. The email that is sent
says who invited the person and to open Iamb, and carries no link that accepts anything
(ADR-0020).

### 5.9 Attachment files

Bytes live in one private storage bucket, `attachments`. A verified file is the object
`<profile id>/<sha256>`; an upload under way is the object `<profile id>/upload/<upload id>`.

| Table | Columns | Notes |
| --- | --- | --- |
| `attachment_blob` | `profile_id`, `sha256` (primary key together), `size`, `mime`, `verified_at` | A row exists only for a file the server has hashed itself and found equal to its name. Only such files can be read |
| `attachment_upload` | `id` (primary key), `profile_id`, `sha256`, `size`, `mime`, `uploaded_by`, `created_at`, `expires_at` | One upload under way: who may write which object. Expires 24 hours after `created_at`; the daily job removes expired rows and their objects. Its size counts against `storage_bytes_max` while it is open |

A file is at most 25 MiB (`attachment_bytes_max`, a server setting). That is the largest file the
local store accepts (architecture §9: images up to 5 MiB, other media up to 25 MiB), so every
attachment a device can hold can be synced, and it is well inside what one edge function call can
hash (§11.6). If the setting is ever lowered, a file over it stays on the device that has it, the
row syncs without it, and the snippet is marked like a blocked row (D5). §6.11 has the calls.

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
| `sync` | user | `hello` | `error: plan` (the device stays local, and keeps the access check, D12) |
| `devices_max` | user | `hello` for a new device | `error: plan_limit` |
| `snippets_max`, `folders_max` | user (personal profile), group (group's rows) | `push`, on a create | `blocked: plan_limit` (C14) |
| `shared_folders_max` | user | `push`, on a share | `rejected: plan_limit` (C11) |
| `groups_owned_max` | user | `create_group`, `accept_ownership` | `error: plan_limit` |
| `members_max` | group | `invite`, `accept_invitation` | `error: plan_limit` |
| `storage_bytes_max` | user, group | `attachment_upload_begin` | `error: plan_limit` |
| `history_days` | user, group | The retention job | — |

- A limit counts live rows of the profile, **whoever wrote them**: a snippet a member creates in
  a shared folder counts against the plan of the folder's owner, because that is whose library
  it is in. A member who may change a folder can therefore use up the owner's limit; the rate
  limits (§6.8) bound how fast, and the owner sees and can delete what was added (open question
  4).
- `shared_folders_max` counts folders, not shares: the folders of the user's personal profile
  that have at least one live share.
- A row blocked by a plan's limit is offered again when it changes, when the device receives an
  `entitlements` entry, and once a day (D5): a member's device is not told when the owner's plan
  changes, and the daily retry covers that.
- Lowering a plan never deletes anything: existing rows stay, and only new ones are refused.

### 5.11 Server state and rate limits

`server_state` has one row: the current `epoch uuid`, `protocol_min`, `protocol_max`,
`protocol_oldest` (the oldest version any build ever spoke; the access check accepts from there,
D12), `retention_days` (30), `history_days` (90), `cascade_max` (2,000),
`attachment_bytes_max`, and `maintenance` (while it is set, `enter` answers every call with
`maintenance`; C18). `epoch (id, started_at, base_rev, first_rev, restored_to)` has one row per
epoch the database has had: `base_rev` is the value `rev_seq` had when the epoch began, which in
a restored database is at or above every revision the backup holds; `first_rev` is where the
epoch's own revisions start (`base_rev` plus 2^40); `restored_to` is the latest `updated_at` the
restored database held. `rate_bucket (subject, bucket, window_start, count)` holds the counters
of §6.8.

### 5.12 Retention

One scheduled database job a day (Supabase Cron, which runs a database function on a schedule).
"Purge" is a hard delete. The job works profile by profile, takes the profile's lock (C24) and
handles at most `cascade_max` rows per transaction, so it never holds a lock for long.

| What | Rule |
| --- | --- |
| Tombstones of synced rows | Purged 30 days after `deleted_at`, children before parents, with their history, their conflicts, their links and their access entries. After that the row id is unknown to the server (C13) |
| Attachments nothing uses | An attachment row whose `unlinked_at` is more than `history_days` (90) old is deleted (a tombstone, purged 30 days later). The 90 days are those of the history, so that a version restored from a conflict still finds its images. When the last row for a `(profile, sha256)` is purged, the stored file and its `attachment_blob` row are removed |
| Uploads under way | An `attachment_upload` row past `expires_at` is removed with its object |
| Feed entries | Deleted after 30 days; `floor` raised |
| History | A version is deleted 90 days after it was replaced (`history_days`), but not while an open conflict offers it |
| Conflicts | An open conflict expires after 90 days; a closed one is deleted 30 days after it was closed |
| Receipts | Deleted after 30 days |
| Invitations | A pending invitation expires after 14 days; an answered or expired one is deleted after 90 days. A pending offer of ownership lapses after 14 days |
| Devices | A device that has not called for 180 days is removed; it registers again, and resyncs, when it comes back. A revoked device row is removed 30 days after it was revoked |
| Deleted groups | The profile row, its memberships, and the folders and snippets it owned are tombstones for 30 days, then purged. Nobody can read them in that time and no call brings a group back |
| Deleted accounts | §11.1 |

The 30 days are one number, `retention_days`, for tombstones, the feed and receipts, so there is
one line to remember: **a device that was offline for more than 30 days resyncs** (D7).

### 5.13 Row-level security, in words

Read rules. "Reads the folder" means a `folder_access` row for the caller and the folder exists.
Every rule also requires `iamb_private.session_ok()`: a device row for the token's session that
is not revoked, and a session that still exists (C25). "Member of" and "role in" are the gate
helpers of §5.1, never a subquery on `membership` itself.

| Table | A user may read a row when |
| --- | --- |
| `profile` | It is their personal profile; or they are a member of it; or they read a folder that belongs to it |
| `membership` | It is their own (also as a tombstone); or it is live and they are a member of the same group |
| `folder` | They read the folder (tombstones included, until the purge, §5.4) |
| `folder_share` | They have `manage` on the folder; or they are a member of the group it names and read the folder |
| `snippet` | They read its folder |
| `tag` | The tag's profile is their personal profile or a group they are a member of; or a snippet they can read has a live link to it; or they wrote it last **and** have `edit` on at least one folder of its profile now |
| `snippet_tag` | They can read its snippet |
| `attachment` | As `tag`, with the links to attachments |
| `snippet_attachment` | They can read its snippet |
| `user_profile` | It is their own; or they share a live group with that person; or that person owns a profile they can read |
| `device` | It is their own |
| `folder_access` | It is their own |
| `feed`, `feed_head` | It is their own |
| `conflict`, `conflict_version` | They are the winner's author or the author of a version the conflict offers |
| `invitation` | They are owner or admin of the group; or it is pending and its address is their confirmed address (`own_address()`) |
| `attachment_blob` | Its profile is their personal profile or a group they are a member of; or a snippet they can read has a live link to an attachment row with its profile and hash. Having written an attachment row with that hash is not enough |
| `attachment_upload` | They are its uploader |
| `history`, `push_receipt`, `plan`, `subscription`, `feature_flag`, `feature_override`, `server_state`, `epoch`, `rate_bucket` | Never; functions read them for the caller |
| Storage objects in `attachments` | Reading: the object's name is `<profile>/<sha256>` of an `attachment_blob` row they may read. Writing: only the name `<profile>/upload/<id>` of an `attachment_upload` row that names them as uploader and has not expired, and only once. Nobody reads an upload object |
| Live channel messages | The topic is `feed:<their user id>`. Nobody may send |

Write rules are §4.1 and §4.3, enforced by the functions.

### 5.14 Functions of `api`

Every function takes the device id and the protocol version, calls `iamb_private.enter` first,
and returns one JSON object with an `error` member that is null or a code (§6.1). The time-out
is the function's own `statement_timeout`. "Who" is checked by the function against the state it
finds after it has taken its locks; a call the caller's role does not allow answers `forbidden`,
and one that names a group or row the caller cannot see answers `not_found`.

| Function | Time-out | Does | Codes of its own |
| --- | --- | --- | --- |
| `hello` | 8 s | Registers or refreshes the device; creates the personal profile and `user_profile` when missing; agrees the protocol version; returns the server's state (§6.2) | `device_revoked`, `device_conflict`, `plan`, `plan_limit` |
| `push` | 20 s | Applies changed rows under §8 (§6.3) | `device_conflict`, `too_large` |
| `pull` | 8 s | Returns a page of the caller's feed with the rows (§6.4) | `reset_required`, `device_conflict` |
| `rows` | 8 s | Returns the given rows as the caller may read them (§6.5, D15) | `too_large` |
| `fetch_folder`, `snapshot` | 15 s a page | Whole-tree and whole-library reads (§6.5) | `not_found` |
| `visible_folders`, `access`, `users`, `entitlements` | 8 s | The list of readable folders (also for a device that may not sync, D12), the caller's access roots, display names, entitlements | — |
| `create_group(id, kind, name)` | 8 s | Creates the group with the caller as owner | `name_required`, `plan_limit`, `invalid` |
| `rename_group(group, name)` | 8 s | Owner, admin | — |
| `delete_group(group)` | 30 s | Owner. Deletes the group (C16) | `not_owner` |
| `transfer_ownership(group, user)` | 8 s | Owner. Records an offer to a live member; with no user, withdraws it | `not_owner`, `not_a_member` |
| `accept_ownership(group)`, `decline_ownership(group)` | 8 s | The member the offer names. Accepting makes them owner and the old owner admin | `no_offer`, `plan_limit` |
| `members(group)` | 8 s | Members: display names and roles. Owner and admins also get each member's email address | — |
| `invite(id, group, email, role)` | 8 s | Owner, admin. Creates the invitation and has the email sent | `name_required`, `already_member`, `already_invited`, `plan_limit` |
| `withdraw_invitation(id)` | 8 s | Owner, admin, whoever sent it | `already_accepted` |
| `accept_invitation(id)` | 8 s | The account whose confirmed address it names. Creates the membership; when that account already is a member, closes the invitation as accepted and changes nothing else | `invitation_invalid`, `name_required`, `plan_limit` |
| `decline_invitation(id)` | 8 s | The invited account | `invitation_invalid` |
| `invitations()` | 8 s | The pending invitations for the caller's confirmed address, and those of groups the caller owns or administers, with the invited address | — |
| `set_member_role(group, user, role)` | 8 s | Owner, admin; between `member` and `admin` | `not_a_member`, `owner_role` |
| `remove_member(group, user)` | 30 s | Owner, admin; never the owner | `not_a_member`, `owner_role` |
| `leave_group(group)` | 30 s | Admin, member | `owner_cannot_leave`, `not_a_member` |
| `conflicts`, `get_conflict`, `resolve_conflict` | 8 s | Conflicts (C20) | `not_found`, `forbidden`, `gone` |
| `attachment_upload_begin` | 8 s | Opens an upload (§6.11) | `exists`, `plan_limit`, `too_large`, `forbidden` |
| `devices`, `rename_device`, `revoke_device`, `sign_out_everywhere`, `set_display_name` | 8 s | The account (§11.3) | `not_found`, `invalid` |

Two calls are Supabase edge functions, because they need what a database function cannot do
(§11.6): `attachment-commit` reads a stored object to hash it (§6.11), and `account-delete`
removes the account from the authentication service (§11.1). Each checks the caller's token,
does its work with the service key, and calls a function of `api_service` for the database part.

A call that is repeated after it succeeded (the answer was lost) answers as it did the first
time: `create_group` and `invite` carry the id of the row they create, and the others find the
state they asked for.

## 6. Protocol

### 6.1 Transport and answers

`iamb-sync` calls the functions of `api` over HTTPS through Supabase's REST interface
(`POST …/rest/v1/rpc/<function>` with the `api` schema selected), with the project's public key
and the account's access token. Every call names the calling `device` and the `protocol` version
agreed in `hello`. A request body is at most 4 MiB.

A function always answers with one JSON object. `error` is null, or one of the codes below, or a
code of the call itself (§5.14). Refusals are data, not HTTP errors, so that what the function
already wrote (a rate counter, a receipt) is committed.

| `error` | Meaning | The agent |
| --- | --- | --- |
| `upgrade_required` | The device's protocol is older than the server accepts | Stops syncing and says so; keeps the access check (D12) |
| `server_too_old` | The device speaks only versions newer than the server's | Tries `hello` again every hour |
| `device_unknown` | No device row for this id and session | Calls `hello`, then repeats |
| `device_revoked` | The device, or the session's device, was revoked | Signs out (D14) |
| `device_conflict` | Another installation uses this device id, or this session already has another device (C25) | Makes a new device id and calls `hello` once more. If that is refused as well, the session itself is the other installation's: the agent signs out (D14) and asks the user to sign in again |
| `rate_limited` | Too many calls; `retry_after_ms` says when | Waits that long, then goes on (§6.8) |
| `reset_required` | `pull` only: the position cannot be continued; `reason` is `expired`, `epoch` or `ahead` | Resyncs (D7) |
| `too_large` | The request is over a size or count limit | Halves the batch and repeats |
| `plan` | The plan does not include sync | Stops syncing; the library stays local; keeps the access check (D12) |
| `maintenance` | The server is being restored or migrated (C18) | Waits and retries as after a network error |

HTTP status 401 means the access token is no longer good: the agent refreshes it once and repeats;
a second 401 means the session ended, and the agent signs out as D14 says, which keeps the
user's own library and its outbox. A network error, a time-out (40 seconds, above every
function's own) or a 5xx status is retried (§6.8). A function that fails unexpectedly rolls its
transaction back and is retried the same way; receipts make that safe (C15), and D17 says what
happens when it keeps failing.

### 6.2 `hello`: registering the device and agreeing the version

Called after sign-in, at every start of the agent and after `device_unknown`.

```json
{ "device": { "id": "…", "name": "Mac", "platform": "macos", "app_version": "0.4.0" },
  "protocols": [1, 2], "max_rev": 9012 }
```

`protocols` is the oldest and the newest protocol version the build speaks; `max_rev` is the
highest revision of any row the device holds (it lets a server that was restored without its
operator saying so notice at the first call, C18). The server picks the highest version both
sides speak, creates the account's personal profile and `user_profile` row if they are missing,
registers the device under C25, and answers:

```json
{ "error": null, "protocol": 2, "user": "…", "personal_profile": "…", "epoch": "…",
  "head": 1841, "retention_days": 30, "server_time": 1795190400000, "display_name": "",
  "limits": { "push_ops": 200, "push_bytes": 4194304, "pull_entries": 500,
              "cascade_max": 2000, "attachment_bytes_max": 26214400 } }
```

No common version gives `upgrade_required` (the device is too old) or `server_too_old`. C25 says
when `hello` answers `device_revoked` or `device_conflict` and what the device does then.

### 6.3 `push`

```json
{ "device": "…", "protocol": 2, "position": "<epoch>:1841:<token>", "ops": [
  { "op_id": "…", "follows": ["…"], "entity": "snippet", "id": "…", "kind": "put",
    "base_rev": 5,
    "row": { "folder_id": "…", "abbreviation": ";rty", "label": "Thanks", "kind": "plain",
             "body": "Thanks for your order.", "trigger": "delimiter",
             "case_mode": "insensitive", "word_start": true, "enabled": true,
             "created_at": 1795190000000 } },
  { "op_id": "…", "entity": "folder", "id": "…", "kind": "delete", "base_rev": 3,
    "cascade": [ { "entity": "snippet", "id": "…", "base_rev": 5 } ] } ] }
```

- An op is one row as it is now on the device (`put`, with the model's fields and the creation
  time) or its deletion (`delete`). `base_rev` is the revision the device had when it made its
  first unsent change to the row; 0 for a row the device holds no revision of. The device sends
  nothing the server assigns (`rev`, `created_rev`, `updated_at`, `updated_by`, `content_by`).
- `follows` lists the op ids of earlier changes to the same row that this device handed to the
  agent and never saw answered (D1). The server uses it to know the device's own earlier write
  (C3).
- `cascade` is on a folder's delete only: the sub-folders and snippets the device deleted with
  it, each with its base (C6). They are not sent as ops of their own.
- `repush` is on ops a device sends again after the server lost them (C18), with `repush_rev`,
  the revision the device holds, and `author: "self"` when the device holds its own user as the
  author of a snippet's content.
- `position` is the device's current position (C25).
- A request has at most 200 ops; a cascade list may name up to 20,000 rows and counts as one.
  The device builds the request under D1, so it never names a parent the server cannot know.
- The server applies the ops in dependency order whatever order they were sent in: folders with
  parents first, tags, attachments, snippets, links, shares; deletes last, children first.
- One request is one transaction, under the locks of C24. Each op is judged by itself under §8,
  and a refused op does not undo the others.
- **The work of one request is bounded.** Besides its ops a request deletes at most
  `cascade_max` (2,000) rows by cascade. A folder's delete that would need more deletes that
  many, deepest first, leaves the folder live, and is answered `partial`; the device sends the
  same op again at once, and the folder goes with the last part (C6).

The answer has one result per op, in the order sent:

```json
{ "error": null, "head": 1843, "results": [
  { "op_id": "…", "status": "applied", "reason": null, "conflict": "…", "adjusted": [],
    "replayed": false, "blob": null,
    "row": { "id": "…", "rev": 9012, "created_at": 1795190000000, "updated_at": 1795190402113,
             "updated_by": "…", "deleted_at": null, "…": "…" },
    "origin": { "content_by": "…", "content_at": 1795190402113, "created_rev": 8840 },
    "cascade": [ { "entity": "snippet", "id": "…", "row": { "…": "…" },
                   "origin": { "…": "…" } } ] } ] }
```

| `status` | `reason` | Meaning | Written in the scenarios as |
| --- | --- | --- | --- |
| `applied` | — | A new revision was written. `conflict` is the id of the conflict it recorded, if any; `adjusted` lists fields the server set differently (C8) | `applied`, `applied+conflict`, `applied+adjusted` |
| `unchanged` | — | The row already is what was sent; nothing was written (C4) | `unchanged`, `unchanged+adjusted` |
| `superseded` | — | A share's stale change, or a re-push that another device's newer re-push preceded: the server's row stands (C11, C18) | `superseded` |
| `partial` | — | A folder's delete that is not finished; send the op again (C6) | — (no scenario is that large) |
| `rejected` | `forbidden`, `gone`, `parent_gone`, `plan_limit`, `not_pushable` | The change is refused for good (C11, C12, C13, C16) | `rejected:…` |
| `blocked` | `too_large`, `plan_limit`, `invalid`, `parent_blocked` | The change cannot be stored as it is; the row stays on the device (C14) | `blocked:…` |

`replayed` is true when the op id was seen before (C15; `replayed` in the scenarios). `row` is
the row as the server has it now and as the caller may read it, with `origin` for a snippet, or
null when the caller may not read it or it does not exist. **`cascade` is on the result of a
folder's delete, whatever its status**: for every row of the op's cascade list, the row as the
server has it after the request and as the caller may read it, or null. A row the device deleted
with the folder therefore always gets an answer: deleted with it, still live because it had left
the folder, or live because the delete was refused (D4, D5). `blob` is on an attachment's
result: `missing` when the caller cannot read a verified file for it (§6.11). D4 and D5 say what
the store does with each result.

### 6.4 `pull`

```json
{ "device": "…", "protocol": 2, "after": "<epoch>:1841:<token>", "limit": 500 }
```

```json
{ "error": null, "epoch": "…", "next": "<epoch>:1846:<token>", "head": 1846, "more": false,
  "entries": [
    { "seq": 1843, "kind": "row", "entity": "snippet", "id": "…", "row": { "…": "…" },
      "origin": { "content_by": "…", "content_at": 1795190402113, "created_rev": 8840 } },
    { "seq": 1844, "kind": "row", "entity": "snippet", "id": "…", "row": null },
    { "seq": 1845, "kind": "folder_revoked", "folder": "…", "except": [] },
    { "seq": 1846, "kind": "folder_visible", "folder": "…" } ] }
```

- The **position** is the text `<epoch>:<seq>:<token>`, kept in `sync_state.position`. The server
  issues it (`next` of a pull, `start` of a snapshot) and a device presents each one once: C25.
  A device asks for what comes after its position and stores `next` in the same transaction as
  the page's effects (D11).
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
  carries `lost_above`: the `base_rev` of the epoch that followed the one in the device's
  position, or of the current one when the server does not know the device's (C18).

### 6.5 Whole reads

| Function | Returns | Used for |
| --- | --- | --- |
| `fetch_folder(folder, cursor)` | The live rows of the tree under `folder` that the caller can read: folders, snippets with their origin, links, the tags and attachments those links name (with whether the bytes are verified), the shares of those folders the caller may read, the owning profile; and `head`, the feed's head when the first page was read. Pages of 500 rows by `cursor` | `folder_visible` (D6) |
| `snapshot(cursor)` | The same for everything the caller can read, with the profiles and memberships, and tombstones the server still holds; its first page carries `start`, a position at the feed's head | Resync (D7) |
| `rows(refs)` | Up to 500 rows named by entity and id, each as the caller may read it now, or null | Catching up after a skipped row (D15) |
| `visible_folders()` | The ids of all folders the caller can read. Answered for every protocol version from `protocol_oldest` on and for accounts whose plan has no sync | The access check (D7, D12) |
| `access()` | The caller's access roots outside their personal profile: `root`, `profile`, `permission`, `via`, `granted_at`; and their role in each group | `SetAccess` (§7.1), after every page that held a visibility entry or a share, membership or profile row |
| `users(ids)` | The display names of those of the given accounts the caller may see (§5.13) | Names of authors and members |
| `members(group)` | §5.14 | The member list |

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
- **When the service refuses the join because the project's connections are used up**
  (`too_many_connections`; the hosting plan sets how many devices can be connected at once,
  §13) the device polls the same way and tries the channel again every 5 minutes. Correctness
  does not depend on the channel; the 5-second target for that device then rests on the
  2-second poll.
- While the channel is up the device still pulls every 5 minutes, which catches a channel that
  died without saying so.

### 6.7 The sync loop

The agent runs one loop per signed-in account; D11 is the rule.

1. `hello` (once per start).
2. **Pull** until `more` is false: apply rows, purge revoked folders and fetch newly visible ones
   in the order of the feed (D6).
3. **Push** until the outbox has nothing to send, 200 ops at a time; act on every result; fetch
   the rows the store reports as behind (D15).
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
- **Order is kept, but no change blocks the others for ever.** A push that fails holds up later
  pushes, never pulls. After three failures of the same request the agent narrows it down until
  it has found the change the server cannot take, and sets that one aside (D17).
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
| A push is cut off before the answer and the agent stops before it repeats it | The entries are marked as handed out in the store. Whatever the user does to those rows next is sent as a change that follows them (D1); a row the pull skipped meanwhile is fetched once its change is settled (D15) |
| A push is delivered twice | The second finds the receipts (C15) |
| A push fails every time | After three tries the agent sends less, down to one change; a change that fails alone is set aside as blocked and the rest goes on (D17) |
| The agent stops between the pages of a pull | Each page's effects and its position are one store transaction; the next start continues after the last complete page, with the position the server knows as the one before its latest (D11, C25) |
| The agent stops in the middle of a whole read | The position is unchanged; the read starts again and applies rows by revision |
| The agent stops between adopting a library and merging it | The phase is in `sync_state`; the next start goes on from it, and nothing is pushed before the merge is done (D9) |
| The service is not running | The agent cannot read the outbox or apply rows; it waits and tries the endpoint again. Nothing is acknowledged that was not stored |
| The store refuses a row of a page (it should not) | The position does not move past it; after three tries the agent resyncs (D7) |
| The server is restored from a backup | A new epoch; devices send back what the server lost and take back what only the server writes (C18, D7) |

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
  once (D7). A resync writes every row the server sends, also one whose revision the device
  already holds, which fills in what the older build could not hold.
- **The access check outlives every version.** `visible_folders` keeps its shape for good and is
  answered for every version from `protocol_oldest` on, so a device that is too old to sync
  still learns that a folder is no longer shared with it (D12).
- The local database's schema version and the IPC protocol version (architecture §6.1) are
  separate numbers; neither is sent to the server.

### 6.11 Attachment files (the frame for task 4.6)

Rows first, bytes after (architecture §9).

- **Up.** The push result of an attachment row says `blob: missing` when the caller cannot read a
  verified file for its profile and hash. That is judged by what the caller may read (§5.13), so
  the answer does not tell a member whether the owner holds a file with that hash. The agent
  then calls `attachment_upload_begin(profile, sha256, size, mime)`, which checks that the
  caller may write in the profile (§4.3), `attachment_bytes_max` and `storage_bytes_max`, and
  answers `exists` when the caller can already read the verified file, or else creates an
  `attachment_upload` row and returns its id; the agent uploads the file to
  `attachments/<profile>/upload/<id>` and calls the edge function `attachment-commit(id)`.
- **`attachment-commit`** checks that the caller is the upload's uploader, reads the uploaded
  object, computes its SHA-256 and size, and: when they equal the upload's, copies the object to
  `<profile>/<sha256>` unless a verified file is there already, calls
  `api_service.attachment_verified(id, sha256, size)`, and removes the upload object; when they
  differ, removes the upload and its object and answers `hash_mismatch`.
  `attachment_verified` checks the hash against the upload row once more, inserts the
  `attachment_blob` row if there is none, deletes the upload row and writes a `blob` entry to
  the feed of everyone who can read the file, under the profile's lock (C24). Every step can be
  repeated: a commit that was cut off is called again and finds what is already done. An upload
  that is never committed expires after 24 hours (§5.12).
- **A file that is already there is still uploaded by a caller who cannot read it.** A member
  who may change a folder, and who attaches a file the owner happens to hold elsewhere, uploads
  it like any other; the commit finds the verified file, keeps it, and discards the upload. The
  member has proved that they hold the bytes, learned nothing, and can read the file once a
  snippet they can read links to it.
- **Down.** `MissingAttachments` lists the attachments of the effective library whose file the
  device lacks and whose bytes are verified on the server. The agent downloads each, checks the
  hash, writes it to `attachments/` (temporary file, rename) and tells the service
  (`AttachmentAvailable`). Until then the snippet falls back to plain text
  (`attachment_missing`).
- The device records in `attachment_state` what it has uploaded; a file that is not on the
  device (it only has the row) is not uploaded by it.
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
| `outbox.handed_out` (set by the store, in the transaction in which `OutboxRead` returns the entry) | Whether the server may have seen the entry: a row with such an entry is never dropped unsent, and a later change names it in `follows` (D1) |
| `outbox.cascade_of` (the `seq` of a folder's delete entry, on the entries its cascade wrote) | The cascade list of a folder's delete (C6) |
| `outbox.blocked` (a reason, or null) and `outbox.tries` | Changes the server cannot store yet, and changes that keep failing (D5, D17) |
| `outbox.repush_rev` (a revision, or null) | Rows a device sends again after the server was restored, with the revision it holds (C18, D7) |
| `skipped (entity, row_id, rev, gone)` | Pulled rows the store did not apply because of an unsent change, with the revision it skipped, or that the row was delivered as gone (D2, D15) |
| `left_alone (entity, row_id)` | Rows a resync did not find on the server and left because of an unsent change, and the folders above them (D7) |
| The unique index `snippet_abbreviation` becomes an ordinary index; the check stays in the store's local write path | Rows from the server are applied as they are; duplicates are ordered by precedence (C9, D13) |
| `sync_state` gains `account_user_id`, `device_id`, `personal_profile_id`, `adoption` (null, `adopted` or `merging`) | Which account the library belongs to, and how far a first sign-in has come (D9, D14) |
| `folder_access (folder_id, profile_id, permission, via, granted_at)` | The user's access roots as the server computed them (§4.2); read-only state in the interface, and precedence (D13) |
| `snippet_origin (snippet_id, content_by, content_at, created_rev)` | Who wrote a snippet's content, and when the server first accepted the row (C17, D10, D13) |
| `approval` gains `approved_content` (the abbreviation, kind, body, trigger, case mode and word-start flag that were allowed, as JSON); `approved_rev` stays for the review dialog | "The last approved revision keeps expanding" without the server (D10) |
| `kept_change (id, entity, row_id, kind, content, held, place, reason, made_at, kept_at)` | Changes the server refused or that lost their folder, kept for the user: `kind` is `put` or `delete`; `held` says that the content was waiting for review when it was kept (D5, D10) |
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
| `OutboxRead(limit)` | Returns changes ready to send, built under D1: for each, the op id, the op ids it follows, the entity, the row as it is now (or that it is deleted), the base revision, the cascade list, and the `seq` values it covers. Marks the entries it returns as handed out, in the same transaction |
| `OutboxAck(results)` | Takes the server's result for each change (status, reason, the row and origin the server returned, the rows of a cascade, the `seq` values) and acts on it under D4 and D5, in one transaction; answers with the rows that are now behind what the device skipped (D15) and the rows a resync had left alone that it has now removed (D7) |
| `ApplyRemote(batch, position, mode)` | Applies rows under D2 and D3; a null row removes the local row; stores `position` in the same transaction when given; answers per row (`applied`, `shadowed`, `stale`, `removed`, `refused`). `mode` is `feed`, `reconcile` (equal revisions are written too) or `epoch` (rows only the server writes are taken whatever their revision), D7. Runs the clean-up of D16 at the end |
| `PurgeFolder(id, except)` | Under D6: first moves unsent changes of the tree to `kept_change`, then removes the tree, its approvals, usage, origins and cached files, and rows of other people's profiles that nothing left uses; leaves the sub-trees named in `except` |
| `SyncState` (new) | The account id, device id, position, the adoption phase, the count of waiting, blocked and kept changes |
| `Adopt(user, personal_profile, mode)`, `AdoptMerge` (new) | D9 |
| `ReconcileBegin(scope, epoch)`, `ReconcileEnd(token, position)` (new) | D7: rows applied between the two with the token are noted; `End` removes synced rows of the scope that were not among them and have no unsent change, and records the others in `left_alone`. With `epoch` (after `reset_required: epoch`), `End` also returns what the device must send again and which calls it must make again |
| `SetAccess(roots, roles)`, `SetKnownUsers(users)` (new) | Fill `folder_access` and `known_user` |
| `PutShare(share)`, `DeleteShare(id)` (new) | Write a share as a local change, with an outbox entry, for the account endpoint's `SetFolderShare` and `RemoveFolderShare`, so that sharing works offline (flow 6) and in order with the folder's own rows |
| `SetBlocked(op, reason)` (new) | D17 |
| `SignedOut(keep)` (new) | D14 |
| `UploadedAttachment(profile, sha256)`, `AttachmentsToUpload` (new) | §6.11 |

Library group (role `ui`): `ListKeptChanges`, `CopyKeptChange(id, folder)` (makes a new snippet
in a folder of the user's from a kept change; refused with `needs_review` for one marked `held`
until the user has reviewed its content, D10), `DismissKeptChange(id)`; `ListFolders`,
`ListSnippets` and `GetSnippet` over the whole effective library with each folder's profile,
owner, access and permission (UX spec §11.2, entry 16); `ListPendingReviews` with the approved
content, the pending content, the author's name and the time of the change (entry 18);
`ApproveSnippet(id, content_hash)`, which approves exactly the content the dialog showed and
answers `changed` when the row holds other content by now (D10); a diagnostic `needs_review` when
a held snippet is typed (entry 19). Copying or duplicating a snippet, and moving one to another
profile, answer `needs_review` while the snippet is held on this device (D10).

Account endpoint (architecture §6.4), besides what is listed there: `CreateProfile(kind, name)`,
`RenameProfile`, `DeleteProfile`, `TransferOwnership`, `AcceptOwnership`, `DeclineOwnership`,
`Members`, `SetMemberRole`, `LeaveProfile`, `WithdrawInvitation`, `DeclineInvitation` (UX spec
§11.2, entry 17); `GetConflict(id)`, `ResolveConflict(id, choice, version)`; `Devices`,
`RenameDevice`, `RevokeDevice`, `SignOutEverywhere`, `SetDisplayName`, `DeleteAccount`;
`SyncNow`. `Invitations` answers both the invitations the user received and those of the groups
they administer.

These are additions an older peer can ignore, except the changed answers of `OutboxRead`,
`OutboxAck` and `ApplyRemote`; no build ever answered those (the service says `not_implemented`
for the Sync group today), so task 4.4 raises the IPC protocol to 4 when it implements them.

### 7.3 The answers to the four questions the store left open

`crates/iamb-store/src/ops.rs` lists four edges of soft delete "for the sync design" (review 1.9,
finding 22). Task 4.4 replaces that comment with a pointer to this table.

| Question | Rule |
| --- | --- |
| Deleting a snippet or a tag leaves its link rows live; deleting a profile leaves its shares and folders | Kept, on both sides, for links: a link counts only while it and both its ends are live, and is purged with either end (C10). That is what lets Undo bring a snippet back with its tags. A folder's delete now also deletes the folder's shares, on the device and on the server (C6). A profile is deleted only by the server, which deletes its memberships, the shares into it and the folders it owns in the same transaction (C16) |
| `put` on a deleted folder restores it but not what the delete cascaded to | Kept, on both sides (C7): a restored folder comes back empty of what was deleted with it, and unshared; the interface says so when Undo brings back a folder that was shared ("Restored. It is no longer shared."). Each cascaded row is restored by itself: Undo right after the delete (a `put` per row), or the conflict that the server recorded for a row the deleting device had not seen (C6) |
| A purge can remove rows that still have unsent outbox entries | Not any more (D8): the local purge skips every row with an entry in the outbox, waiting or blocked |
| One conflicting row fails a whole `apply_remote` batch | Not any more (D3): a row from the server cannot conflict on its abbreviation (C9), and each row of a batch is applied or reported by itself |

## 8. The rules on the server

How `push` and the account calls decide. Scenario numbers point to §12.

**C1.** *Arrival order.* The last write is the one the server accepts last. The server takes the
locks of C24, gives the write the next value of `rev_seq` and its own clock as `updated_at`. A
device's clock is used for nothing but `created_at` (and that is cut back to the server's clock
when it lies in the future), which is shown and decides nothing. (S24, S25)

**C2.** *The whole row.* An accepted `put` replaces every field of the row with what was sent; a
`delete` sets `deleted_at`. The version that was there goes to `history` as it was. Fields are
never merged. (S01, S02, S06, S12)

**C3.** *A stale write still wins, on the record.* A write is stale when its `base_rev` is below
the row's `rev`. It is applied like any other. A conflict is recorded when both hold:

- the version it replaces is not the same device's own earlier change: it was written by another
  device, or under this device's id by a change the op does not name in `follows` (the server
  finds the op that wrote a version in the receipts, §5.7). So a device that edits a row again
  before it saw its own push answered gets no conflict with itself, and two installations that
  share one device id by accident do get one (C25);
- since the base someone changed a recorded field or whether the row is deleted. Recorded fields
  are, for a snippet: folder, abbreviation, label, kind, body, trigger, case mode, word-start,
  enabled; for a folder: parent, name, the member permission and the defaults for new snippets;
  for a tag: name and colour. A folder's `position` is not recorded, and links, attachments and
  shares never get conflicts. The comparison is between the base version, taken from the
  history, and the version being replaced; when the history no longer holds the base, between
  what was sent and the version being replaced.

The conflict names the writer as winner and offers the replaced version back; its parties are
the writer and the authors of the versions it offers, and it is shown to all of them. **A row has
one open conflict. When a new one is recorded while another is open, the new one takes over
every version the old one offered, with their authors as parties, and the old one is closed as
`replaced`**: a version that was replaced unseen stays on offer until somebody answers, however
many writes follow. A write that records no conflict leaves an open conflict as it is. (S01, S08,
S22, S49, S50, S57, S60)

**C4.** *Nothing new, nothing written.* When what was sent equals the row in every field and in
whether it is deleted, the answer is `unchanged` with the row's revision: no revision, no
history, no feed entry, no conflict, whatever the base. (S05, S42, S43)

**C5.** *A delete is a write.* Whichever of an edit and a delete arrives last wins. A stale
`delete` deletes the row and, under C3, records `delete_over_edit` with the live version on
offer. A `put` on a tombstone makes the row live again and, when stale, records
`edit_over_delete` with the tombstone on offer. A `delete` of a tombstone is `unchanged`; a
`delete` of a row the server does not know is `unchanged` too, with no row in the answer. (S03,
S04, S05, S18, S19, S20, S22, S50)

**C6.** *A folder's delete takes the subtree, as the server has it.* Deleting a folder deletes,
in the same transaction, every live folder and snippet below it as they are on the server at
that moment, and every live share of those folders and of the folder itself (C21 then ends the
access they gave). Each gets its own revision and feed entry. The op's `cascade` list says which
rows the deleting device deleted with the folder and in which revision. It does not decide what
is deleted: a row the list names that has left the folder stays live, and a row in the folder
that the list does not name is deleted. A row that another device wrote and that the list does
not name, or names with a base below its revision after a change to a recorded field, gets a
`deleted_with_folder` conflict, so it can be restored. Restoring a row restores the folders
above it (C7). **The answer carries, for every row of the list, the row as the server has it
after the request** (§6.3), whether the delete was applied, refused or changed nothing, so the
device never keeps a row deleted that the server has live. **A delete of more than `cascade_max`
rows is made in parts**: each request deletes at most that many rows of the subtree, deepest
first, with the conflicts above, and answers `partial` while rows remain; the folder itself is
deleted in the request that finds nothing left below it. Between the parts the tree is valid:
there are fewer rows in a live folder. (S15, S16, S23, S51, S52, S53, S61)

**C7.** *A live row needs live folders.* A `put` that would leave a live snippet or folder under
a deleted folder restores every deleted folder on the way up, if the writer may: `manage` for a
top-level folder, `edit` on its parent for a sub-folder. Each restored folder gets a new
revision and a `folder_restored` conflict between the writer and whoever deleted it. What was
deleted with the folder stays deleted, and its shares stay deleted: access never comes back by
itself. If the writer may not restore a folder on the way, the `put` is `rejected: parent_gone`.
Under C24 this holds for requests that arrive together as for those that arrive apart. (S15, S53)

**C8.** *Structure is put right, not refused.* A folder `put` that would make the folder its own
ancestor, or make any tree deeper than four levels, keeps the folder's current parent; a new
folder that would be a fifth level is put under the nearest folder above the requested parent
that keeps four levels, or at the top. Everything else of the row is applied, `adjusted` names
`parent_id`, and the device takes the server's row. A row whose profile would change (a folder
under a parent of another profile, a snippet moved to a folder of another profile, a link across
profiles) is `blocked: invalid`. (S13, S14, S53)

**C9.** *The server does not read content.* No uniqueness, search or parsing applies to
abbreviations, labels, bodies or names; the server only stores them and compares them for
equality (C4, C17). Two snippets with the same abbreviation in one profile, two tags or two
sibling folders with the same name are all accepted. Which snippet holds an abbreviation is
decided on each device by the same precedence (D13). (S09, S10, S11, S25, S48, S54)

**C10.** *Links are rows.* A link is created and deleted by itself; nothing cascades to it and it
cascades nothing. It counts only while it and both rows it joins are live, and it is purged with
either. A link to a tombstone is accepted and waits; a link to a row the server does not know is
`rejected: parent_gone`. Two links for the same pair may exist; devices treat them as one, and a
device that takes a tag or an image off a snippet deletes every live link of the pair (D3).
(S17, S18, S19, S20, S21, S63, S64)

**C11.** *Sharing never wins by being late.* A share's id is derived from its folder and group,
so a pair has one row. A `put` needs `manage` on the folder, a folder in a personal profile, the
writer's live membership in the group and, when the group is a team, the role of owner or admin
there (§4.1, open question 7); over the plan's `shared_folders_max` it is `rejected:
plan_limit`. A stale `put` is not applied: the answer is `superseded` with the server's row. A
`put` based on the row's current revision is applied, also on a tombstone (sharing again; the
access that begins is a new one, with a new `granted_at`). A `delete` is applied whatever its
base. Shares get no conflicts; their versions are in the history. (S31, S36, S37, S46, S67)

**C12.** *Permission is judged on arrival.* Every op is checked against §4.3 with the memberships,
shares and folders as they are on the server at that moment, for the place the row is in and
the place it would be in. What the device believed, and what was true when the user made the
change, do not count. A refused op is `rejected: forbidden` and the answer carries the server's
row if the caller may still read it, and for a folder's delete the rows of its cascade list
(C6). (S30, S31, S34, S36, S39, S52, S62)

**C13.** *Rows the server does not know.* A `put` with a base above 0 for an id the server does
not have is `rejected: gone`: the row was purged more than 30 days after its delete, and it does
not come back. The one exception is a re-push of a revision that a restore of the server lost
(C18). A `put` with base 0 creates the row. A row whose folder, or other row it must refer to,
is unknown to the server or unreadable for the writer is `rejected: parent_gone`. (S23, S43,
S59)

**C14.** *What cannot be stored stays local.* A row over a size limit is `blocked: too_large`; a
content row over the plan's limit is `blocked: plan_limit`; a row the server's version of the
protocol cannot accept is `blocked: invalid`; a row whose parent was blocked in the same request
is `blocked: parent_blocked`. Nothing is written for it and the other ops go on. (S45)

**C15.** *Once.* The server keeps, for 30 days, the answer it gave to each `(device, op_id)`,
with the revision it wrote, and holds the lock on the device's row for the length of a push
(C24). An op whose id it has seen writes nothing: the answer is the stored status with
`replayed` set and the row as it is now. A `partial` answer is not final and is not kept: the op
goes on where it stopped. (S41, S42)

**C16.** *Profiles, memberships and invitations belong to the server.* They change only through
account calls, never through `push` (`rejected: not_pushable`). Every call takes the locks of
C24 first, checks §4.1 and its own conditions (§5.14) against the state it finds, and writes
everything in one transaction; of two calls that race, the one that gets the lock first decides
what the other finds. `accept_invitation` needs a pending, unexpired invitation whose address is
the caller's confirmed address (§5.8); `withdraw_invitation` needs it pending. **Ownership is
handed over by offer and acceptance**: `transfer_ownership` records the offer on the group and
changes no role; `accept_ownership`, by the member it names, needs that member to be a live
member still and within their own plan's `groups_owned_max`, and changes the profile's owner and
both memberships together; nobody can be made an owner without having said yes. Ending a
membership (remove, leave) deletes the shares from that person's folders into the group and
removes that person's access entries for everything shared with the group. Deleting a group
deletes its memberships, the shares into it and the folders it owns with their content; every
member gets `folder_revoked` for the group's folders and for the folders others shared with it.
The group's own rows get their tombstones in two statements over the whole profile, with no feed
entries, because nobody can read them any more; the work is bounded by the group's plan. No call
brings a deleted group back. (S30, S32, S33, S34, S35, S39, S58)

**C17.** *The server says who wrote it.* `updated_by` is the caller of the write and
`updated_device` their device, for every row; a client cannot set either. For a snippet the
server also keeps `content_by` and `content_at`: who last changed what the snippet does, which
is its abbreviation, kind, body, trigger, case mode and word-start flag. A `put` is the row as
the device holds it, and a device may hold content its user never changed and never approved.
So for every accepted `put` the server compares those six fields as sent:

1. with the row it replaces. Equal: nothing changes.
2. Otherwise with the version the write was based on: the version of `base_rev`, which is the
   row or a history version; for base 0 on a row that exists, the version written by the latest
   op the request names in `follows`. Equal: the writer did not change the content, the device
   only held an older one. `content_by` and `content_at` become those of that base version.
3. Otherwise the writer changed the content: `content_by` is the caller and `content_at` the
   server's clock. A new row is this case.
4. When the base version cannot be found (the history no longer holds it, or a base of 0 names
   no earlier op), the server cannot tell: `content_by` is null, "nobody".

A re-push after a restore (C18) that differs from the row sets the caller when the op says
`author: "self"`, and nobody otherwise. Restoring a version from a conflict (C20) keeps the
author that version had. "Nobody" counts as someone else on every device (D10). All comparisons
are the server's: the most a changed client can achieve is to be named as the author of content
it sent itself. (S40, S54, S55, S56)

**C18.** *After a restore, devices give back what the server lost.* Restoring the database from a
backup loses every write since the backup. The rule has a part for the operator, one for the
server and one for devices.

- *The operator.* The REST interface is closed before the restore (`server_state.maintenance`
  in the old database, and the project's network restrictions while the restore runs) and
  opened only after `iamb_private.begin_epoch()` has run in the restored one. The account calls
  made since the backup are lost with everything else; the function log, which is kept outside
  the database and holds ids and codes (§11.1), names them, and the operator runs the
  `delete_account` calls among them again before opening. Task 4.2 ships this as a script and a
  page of instructions, and tests it.
- *The server.* `begin_epoch` writes a new `epoch` row (`base_rev`: the restored value of
  `rev_seq`, which no revision in the backup exceeds; `restored_to`: the latest `updated_at` in
  the backup), moves `rev_seq` 2^40 further, past any revision a device can hold, empties the
  feeds and forgets the devices' position tokens. As a net against a forgotten step, `enter`
  runs it by itself when a device proves that the server lost data: a `hello` whose `max_rev` is
  above what `rev_seq` ever gave out in the current epoch, or a position ahead of the feed's
  head. Until such a device calls, a server that was restored without `begin_epoch` gives out
  revisions a second time, and what it accepted in that time can collide with what devices
  hold; that is why the procedure, and not the net, is the rule.
- *Devices.* Every position of an older epoch gets `reset_required` with `lost_above`. The
  device resyncs and compares row by row (D7): a row it may write that the server holds in an
  older revision, or does not hold, it sends again as a `repush` with the revision it holds;
  rows only the server writes it takes as the server has them, and where its user had taken
  access away after the backup, it makes that call again (D7).

A re-push is honoured as one when its `repush_rev` lies in the span the restore lost: above
`lost_above` as the server computes it for the device's epoch, and below the current epoch's
`first_rev`. Then: onto a row nobody has written in the new epoch it is applied without a
conflict, and the version it replaces goes to the history; onto a row another re-push already
wrote, the higher `repush_rev` stays and the other is answered `superseded` (equal content is
`unchanged`, C4); onto a row an ordinary write changed in the new epoch it is an ordinary stale
write (C3); for a row the server does not hold it creates the row, as long as the epoch is
younger than `retention_days` (after that the server cannot tell a lost row from one deleted and
purged since, and answers `rejected: gone`, which the device keeps, D5). A `repush` outside the
span is judged as an ordinary write with `repush_rev` as its base. Permission is judged on
arrival as always (C12). (S43, S58, S59)

**C19.** *Attachments.* An attachment row never changes after its creation; a `put` that
differs from it is `unchanged` (the store never changes one either, so nothing is lost by
that). Several rows may name the same profile and hash; the stored file exists once per profile
and hash and is readable, when verified, by those who can read a snippet that links to it
(§5.13). The server keeps `unlinked_at` in step with the links, and deletes an attachment row
that no live snippet has linked to for 90 days (§5.12). (S19, S20, S21)

**C20.** *Resolving a conflict.* Any party may close a conflict. "Keep the current version"
closes it. "Restore this version", for one of the versions it offers, needs the right to write
the row now; the server writes that version as a new revision by that user (a live version makes
the row live and restores the folders above it, C7; a tombstone deletes the row, a folder with
its subtree), keeps the author its content had (C17), records no conflict for that write, and
closes the conflict as resolved. The version it replaces goes to the history like any other. A
version whose history row has been purged answers `gone`. (S07)

**C21.** *Visibility.* What a user may read follows from `folder_access` (§5.4), which the server
keeps equal to §4.2 inside every transaction that changes a folder's place, a share or a
membership, under the locks of C24. When a tree becomes readable for a user, `folder_visible`
for its top goes into their feed; when it stops being readable while its rows still exist,
`folder_revoked`. That includes a shared folder its owner deletes: the share is deleted with it
(C6), so the people the share served get `folder_revoked` and not tombstones they could no
longer read, while those who read the folder as its owner, as members of the group that owns
it, or through a live share of a folder above it get the tombstones. A tree that is revoked
while part of it stays readable through another grant gets one entry: `folder_revoked` for its
top with the parts that stay listed in `except`. A folder that is still reachable through
another grant is not revoked; only its permission changes. Visibility entries are written in
the order the grants changed and a device handles them in that order (D6). (S10, S11, S28, S29,
S30, S35, S36, S37, S38, S61, S62, S65, S67)

**C22.** *Thirty days.* Tombstones, the feed and receipts are kept 30 days (§5.12). Inside that
time every rule above applies whatever the delay. After it a deleted row is unknown (C13) and a
device's position is too old to continue (D7). (S22, S23, S29)

**C23.** *Versions.* The server accepts the protocol versions from `protocol_min` to
`protocol_max` and answers every call of an older device with `upgrade_required`, having done
nothing; the access check is the exception (§6.10, D12). Each version's functions read and write
the row shape of that version (§6.10). (S44)

**C24.** *One writer per profile at a time.* Every transaction that writes a synced row, a share,
a membership or an access entry takes its locks in this order, before it reads anything it
decides on:

1. the `device` row of the calling device (`FOR UPDATE`), which is also the lock on its
   receipts (C15) and its position (C25);
2. the `profile` row of every **group** the request touches, in the order of their ids;
3. the `profile` row of every **personal profile** the request touches, in the order of their
   ids;
4. and last, after everything else is written, the `feed_head` rows of every user whose feed
   gets an entry, in the order of the user ids (§5.5).

What a request touches: an op on a folder, snippet, tag, attachment or link touches the profile
that owns the row; a share touches its group and the personal profile of its folder; an account
call touches its group, and a call that changes who is a member (`accept_invitation`,
`remove_member`, `leave_group`, `delete_group`, the removal of an account) also every personal
profile that holds a folder shared with that group; `resolve_conflict`, the commit of an upload
and the daily job touch the profile of the rows they write. The owning profile of a row never
changes (C8), so it is read before any lock is held: from the row if the server has it, else
from the folder, snippet, tag or attachment the op names, else from another op of the same
request; an op for which no profile is found touches nothing and is `rejected: parent_gone`. The
profiles that share into a group are read after the group is locked, and cannot change then,
because writing a share locks the group first.

Why this is enough. All writes to one profile happen one after the other, and because the
functions lock before they read (§5.1), the later one reads what the earlier one committed.
Every change to a profile's tree, to the shares of its folders, or to the memberships of a
group one of its folders is shared with, is made by a transaction that holds that profile's
lock. Therefore:

- *No cycle, no fifth level.* Two moves in one profile are serial; the second finds the first
  and C8 applies to it. (S13, S53)
- *No live row under a deleted folder.* A folder's delete and a write into the folder are
  serial: the delete's cascade finds the new row (C6), or the new row finds the tombstone and
  restores the folder (C7). (S15, S16, S53)
- *No access for an ended share.* Ending a share locks the group and the folder's profile; a
  push that creates a sub-folder there locks the same profile; removing a member locks the
  group and every profile that shares into it. Whichever comes second computes the access
  entries from what the first left (§5.4). A member cannot keep or gain an entry through a race.
- *No deadlock by design.* There is one order for all locks: devices, groups by id, personal
  profiles by id, feed heads by user id. If Postgres reports a deadlock all the same, the
  function fails as a whole and the device repeats it (C15).

The cost is that writes to one profile wait for each other for the milliseconds a request
takes, which is nothing for people's snippets; reads never wait. (S53; §11.5, G21)

**C25.** *One session, one device, one position.* A device row belongs to one authentication
session and a session to at most one device row.

- `hello` decides by the token's session and the device id it is given. The session has a row
  with that id: the row is refreshed. The session has a revoked row: `device_revoked`; a
  revoked row is kept for 30 days, longer than any token of its session can live, so a revoked
  session cannot register again under another id. The session has a row under another id:
  `device_conflict`. The session has no row, and the id has a row that is revoked, that belongs
  to another account, or whose session still exists: `device_conflict`, because the id is in
  use or was revoked. The session has no row, and the id has none, or one whose session has
  ended (the user signed out and in again on this installation): the row is created, or moved
  to the new session.
- Every call, through `enter`, and every policy, through `session_ok()`, requires a device row
  for the token's session that is not revoked, and a session that still exists in
  `auth.sessions`. Revoking a device therefore stops it with the commit of `revoke_device`, also
  while its access token is still valid; nothing depends on the authentication service ending
  the session at that moment (§11.6).
- A position is issued by the server, in `next` of a `pull` or `start` of a `snapshot`, and
  carries a random token. The server keeps the last two it issued to the device. A call that
  presents the latest one is the normal case. One that presents the one before it is a repeat
  after an answer that did not arrive, and is answered again. A call that presents any other
  position of the current epoch is answered `device_conflict`: two installations are using one
  device id, or a data directory was put back from a backup. `push` presents the position as
  well, so an installation that is behind writes nothing.
- An installation told `device_conflict` makes a new device id and calls `hello` again. When
  its session is its own (the user signed in on this installation, and only the device id came
  with a copied data directory, or an earlier session of this installation was never ended),
  that registers it as a device of its own, and it resyncs (D7). When the session came with the
  copy too, `hello` refuses again, because the session already has a device: the installation
  signs itself out, keeps its library and outbox, and asks the user to sign in again (D14).
  Until one of two such installations is caught, which is at the next call of the one whose
  position is no longer among the last two, their writes carry one device id; C3 still records
  a conflict between them. (S60, S66)

## 9. The rules on the device

**D1.** *From outbox entries to changes.* The store writes one outbox entry per local change, in
the transaction that makes the change, as it does today. What is sent is one change per row:
the row as it is now, or its deletion; the base revision of its oldest waiting entry; the op id
of its newest. When `OutboxRead` returns an entry, the store marks it as **handed out** in the
same transaction: from then on the server may have seen it, whatever becomes of the request. A
change whose row has handed-out entries besides the newest names their op ids in `follows`. A
row that was created and deleted is dropped with its entries only when none of them was ever
handed out; otherwise its delete is sent, with base 0, and the server answers `applied` or, if
it never saw the row, `unchanged` (C5). Rows are taken in the order of their oldest entry, up to
200; then every row those refer to that the server has never seen is added, so that a request
is complete in itself. A row that refers to a blocked row is blocked with it. Changes are sent
in dependency order (§6.3). (S26, S27, S50)

**D2.** *A pulled row never replaces an unsent change.* While a row has an entry in the outbox,
waiting or blocked, `ApplyRemote` leaves the row alone, answers `shadowed`, and notes in
`skipped` the revision it did not apply, or that the row was delivered as gone. The device's
own push follows, wins on the server (C2), and the server keeps the version the device did not
apply, with a conflict (C3). (S01, S03, S04, S22, S50, S51)

**D3.** *Applying pulled rows.* Each row of a batch is handled by itself, in dependency order
with foreign keys checked at the end of the batch. A row is written only when its revision is
above the local one; an equal or lower revision is `stale` and ignored (in a resync an equal
revision is written too, D7). A null row removes the local row (`removed`) unless D2 holds. A
tombstone for a row the device does not have is ignored. No uniqueness check applies to rows
from the server. A folder whose chain of parents does not reach the top within eight steps is
shown at the top level: the server never holds a cycle (C24), but a device can for the moment
between a pull and the answer to its own push. A local change that takes a tag or an image off
a snippet deletes every live link of that pair, not only the one the device made (C10). (S13,
S15, S16, S18, S62, S64)

**D4.** *Acting on `applied` and `unchanged`.* The entries the change covered are removed. If
the row has no newer entry, the store replaces it with the row of the answer (revision, times,
author, origin, and any field the server adjusted), or removes it when the answer has none (a
delete of a row the server never saw). If the user changed the row again while the push was
under way, the store keeps the row, takes only the revision from the answer, and sets it as the
base of the entries that remain, so that the next push is based on the device's own accepted
version. For a folder's delete the store does the same for every row of the answer's `cascade`:
the entries of the cascade are removed and each row is replaced by what the server sent, so a
row that had left the folder comes back as the server has it. `partial` removes nothing: the
same op is sent again at once. (S05, S13, S14, S26, S41, S50, S51)

**D5.** *Acting on the other answers.* `rejected`: every entry of the row is removed, newer
ones included, and the change is recorded in `kept_change` with the reason and the folder's
name: for a `put` the device's version of the row as it is now, for a `delete` a line that
names what was not deleted. Then the row of the answer replaces the local row, or the local row
is removed when the answer has none; for a refused folder's delete, each row of the answer's
`cascade` is put back the same way. For a refused share nothing is kept, the row is put back or
removed, and the user is told. `superseded`: the entries are removed and the server's row
replaces the local one. `blocked`: the entries stay, marked with the reason; the row stays as
it is, works on this device, and is offered again when it changes, when the entitlements change,
and once a day. The user sees kept and blocked changes in the library, never as a notice over
another application (UX spec §4.11); a kept change stays until the user copies or dismisses it.
(S23, S30, S31, S36, S37, S39, S45, S46, S52, S62)

**D6.** *Folders that appear and disappear.* The visibility entries of a page are handled one by
one, **in the order of the feed**, after the page's rows; the page's position is stored only
when all of them are done, so a device that stops in between does them again (D11). On
`folder_visible` the agent fetches the tree (`fetch_folder`), applies it between
`ReconcileBegin` and `ReconcileEnd` for that tree, and reads `access` again. On
`folder_revoked` the service purges the tree (`PurgeFolder`), leaving the sub-trees the entry
lists in `except`, which become access roots of their own: unsent changes in the purged part go
to `kept_change` first and leave the outbox, unsent changes in the part that stays are left
where they are; then the folders, snippets, links, shares, approvals, usage and origins of the
purged part are removed, with cached files and rows of other people's profiles that nothing the
device still holds uses (D16). When the tombstone of the user's own membership arrives, the
service also removes that group's profile row and its other memberships; the group's folders
go through `folder_revoked`, which the server writes in the same transaction. (S10, S28, S29,
S30, S35, S37, S38, S58, S61, S65, S67)

**D7.** *Resync.* On `reset_required`, after every sign-in, after the first start of a build
with a newer protocol, and when the store refuses a pulled row three times, the device fetches
everything it may read (`snapshot`) between `ReconcileBegin(all)` and `ReconcileEnd`, writing
every row whose revision is the local one or above, and then removes synced rows that were not
in it.

- *Rows with unsent changes, and the folders above them, are left alone*, and the store records
  in `left_alone` those of them the snapshot did not hold. The push that follows settles the
  changes (C13, D5); when a left-alone row has no entry in the outbox any more and no row with
  an entry below it, and the server's answers did not bring it, the store removes it, children
  before parents. Nothing the server does not hold stays behind.
- *In a new epoch* (`reset_required: epoch`) the device compares row by row instead of trusting
  revisions, because the server's rows are older than the device's. A row the device may write
  (its own profile, or a folder it has `edit` on) that the server holds in an older revision, or
  does not hold while the device's revision is above `lost_above`, is queued again, marked
  `repush` with the revision the device holds. A row the server does not hold and that is not
  above `lost_above` is removed like any other. Rows only the server writes (profiles,
  memberships) and rows the device may not write are taken as the server has them, whatever
  their revision (the one exception to I3). Where the device held a tombstone of such a row and
  the server shows it live, a change that **took access away** after the backup was lost: if the
  user may still make that call, the device makes it again by itself (`remove_member`,
  `leave_group`, `withdraw_invitation`, `set_member_role` from admin to member, `delete_group`).
  It never repeats a call that gives access; those the user makes again. Either way the device
  shows once that the server was restored (`restored`).
- A resync ends with the **access check**: the device asks for the folders it may read
  (`visible_folders`) and purges every folder of another profile that is not among them. The
  access check also runs at every start of the agent and once a day, so that no lost message
  can leave a revoked folder on a device that is online.

(S23, S29, S43, S44, S49, S58, S59)

**D8.** *The local purge.* Once a day and after each sync the store removes tombstones whose
`deleted_at` is more than 30 days old, with their links, usage and approvals, but never a row
that has an entry in the outbox. A tombstone the server never saw (revision 0, no account) is
removed after 30 days as well. (S23, S29)

**D9.** *Adoption.* The first time an account signs in on a device, the library that is there
becomes that account's, in three steps whose progress is kept in `sync_state.adoption`, so that
a stop at any point is taken up again at the next start:

1. *Adopt.* In one transaction the store inserts the account's personal profile row, moves
   every folder, tag and attachment of the local personal profile to it (and the files under
   `attachments/`), removes the local profile row and all tombstones, records the account,
   **writes an outbox entry for every row that was never synced**, and sets the phase to
   `adopted`. No row can be left without an entry. While a phase is set the agent pulls and
   does not push.
2. *Resync* (D7). The account's library arrives.
3. *Merge*, in one transaction (`AdoptMerge`), top down: a local folder that was never synced
   and has the name and parent of a folder of the account gives its content to that folder and
   is dropped; a local tag with the name of an account tag likewise; a local snippet that
   equals a snippet of the same folder in abbreviation, label, kind, body and options is
   dropped. A dropped row's outbox entry goes with it. The phase is cleared, and the agent
   pushes what is left, as new rows in dependency order.

Two computers that sign in for the first time at the same moment cannot see each other's rows
in time to merge; the account then has two folders of the same name, which the user merges by
hand. A library that belongs to another account is never adopted silently: the interface asks
whether to add it to the new account (every row gets a new id) or to remove it from the device.
(S48)

**D10.** *Review by content.* A snippet is held on a device when its content was written by
someone else (`content_by` is not the signed-in user; "nobody" counts as someone else), its
template contains `{key}`, `{wait}`, `{clipboard}`, `{snippet}` or a line break (ADR-0022, with
`{snippet}` added, open question 10; a template that does not parse counts), and its
abbreviation, kind, body, trigger, case mode and word-start flag are not exactly what the
device's `approved_content` holds for it. Whenever a revision is not held, the user approves
one, or the user saves the snippet's content on this device, those six fields are stored as
`approved_content`. While a snippet is held, the engine's reference set and matchable set use
the stored content, or leave the snippet out when there is none.

- An approval names the content the dialog showed (`ApproveSnippet(id, content_hash)`, the
  SHA-256 of the six fields); when the row holds other content by then, nothing is approved and
  the dialog shows the new content.
- **A held snippet cannot be copied.** Copy, Duplicate, "Copy to my snippets" and a move to
  another profile make a new row, whose author is its creator (C17). The store refuses them
  with `needs_review` while the snippet is held on this device, and for a kept change whose
  content was held when it was kept; the interface offers the review. After the review the copy
  is the user's own, by their own decision.
- The editor shows the row's content, held or not. A user who changes that content and saves
  has written it; a user who changes only the label or the folder has not, and the server keeps
  the author (C17).

Approvals are per device and are removed with the folder (D6). (S40, S55, S56)

**D11.** *One loop, one position.* The agent pulls, pushes, and pulls again (§6.7). The effects
of a page (its rows, the purges and fetches its visibility entries ask for) and its position
are stored together, and a page that is read twice has the same effect as once. A device that
stops before it stored a page presents its previous position again, which the server accepts
as a repeat (C25). A poke or a poll only starts the loop early; without the live channel the
device polls (§6.6). (S47, S67)

**D12.** *Too old, or without a plan.* On `upgrade_required` or `plan` the device stops syncing,
keeps expanding and editing, keeps every change in its outbox, and says why (Iamb must be
updated; the plan does not include sync). It keeps one call: the access check
(`visible_folders`, D7) at every start and once a day, which every version of the server
answers, so that a folder that is no longer shared with the user still leaves the device. (S44)

**D13.** *Who holds an abbreviation.* The service orders the effective library before it builds
the snapshot. For the signed-in user:

1. the snippets of the user's personal profile whose content the user wrote (`content_by` is
   the user);
2. the other snippets of the user's personal profile, which members wrote in folders the user
   shares, or whose author is nobody;
3. the folders of other profiles, by the time access to them began (`granted_at`, from the
   server) and then by the root folder's id.

Inside one of these the order is the server's: by `created_rev`, the revision at which the
server first accepted the row, and then by id; a row the server has not accepted yet comes last.
The first snippet with a folded abbreviation holds it; the others neither expand nor resolve,
and library health lists them. No device clock and no creation time takes part. It follows that
a snippet whose content someone else wrote never takes an abbreviation from a snippet whose
content the user wrote, wherever the two lie, and that nothing in a folder shared with the user
takes one from the user's own profile. The store still refuses a local edit that would create a
duplicate inside a profile, and the editor still warns about a shared one (UX spec §4.3). (S09,
S10, S11, S25, S48, S54)

**D14.** *Signing out.* The agent drops the tokens and ends the session on the server when it
can. The service removes everything of other people's profiles (D6). Unsent changes to shared
folders are asked about first, when the user signs out by hand: keep them in the list of
changes that were not synced, or discard them. The user's own library stays, with its
revisions, its outbox and the account it belongs to, and keeps working. Signing in again with
the same account resyncs (D7) and pushes what waited; a different account is D9's last
sentence. **A device the server signs out**, because it was revoked (`device_revoked`), because
its session ended, or because its session and device id are another installation's as well
(`device_conflict` twice, C25), does the same without asking: unsent changes to shared folders
are kept, and the device says that it was signed out. Its next sign-in makes a new device id.
(S49, S60, S66)

**D15.** *Catching up after a skipped row.* A position moves past rows the store did not apply
(D2), and the feed says each change once. So the store remembers what it skipped. When the last
outbox entry of a row is removed, by an answer (D4, D5) or because the row was dropped (D1),
and the row's revision is still below the one skipped, or the row was skipped as gone and is
still there, `OutboxAck` reports the row as behind. The agent fetches such rows with `rows`
and applies them like pulled rows; a row the server answers with null is removed. The note is
cleared when the row has caught up. With D1 and the answers for a cascade (D4, D5) no rule
leaves a row behind; this is the net under them, and what the simulation of task 4.4 leans on
for I1. (S50, S51)

**D16.** *What leaves with a link.* A user reads a tag, an attachment row or a file of someone
else's profile only through a snippet that links to it (§5.13), and the server says nothing
when the last such link goes, because the tag or attachment itself did not change. After every
batch that removes or tombstones a link or a snippet of a profile the user neither owns nor is
a member of, and in every purge (D6), the store removes the tags, attachment rows and cached
files of that profile that no live link from a live snippet it holds uses any more. The other
way round, a pulled snippet whose body names an attachment of its profile that the device holds
without a live link gets the link from the first device that may write the snippet (a local
change like any other); that is how a version restored from a conflict gets its images back.
(S62, S63)

**D17.** *A push that keeps failing.* A request the server cannot finish (a time-out, an
unexpected failure of the function) fails the same way each time and would hold up every later
push. After three failures of the same request without an answer, the agent sends half of it,
and so on down to a single change. A single change that fails three times is marked blocked
with the reason `server_error` (`SetBlocked`), with every change that depends on it
(`parent_blocked`); the rest of the outbox goes on. Such a change is tried again once a day and
after Iamb is updated, and the user sees it among the blocked rows (place 5 of §1). (No
scenario: the server of a scenario answers.)

## 10. The conflict questions, one by one

| Question | Answer | Rules | Scenarios |
| --- | --- | --- | --- |
| Last write wins by which clock? | By the order in which the server accepts writes. The server's revision counter orders them and its own clock dates them. No device clock, no hybrid clock | C1 | S24, S25 |
| What is kept in the history? | Every version a write replaced, whole, tombstones included, for 90 days, and for as long as an open conflict offers it | C2, §5.6, §5.12 | S01, S04 |
| What does a recorded conflict hold, and how does the user restore? | The row, the current version and every version replaced unseen, who wrote each and when, and a kind. All authors see it. "Restore this version" makes the server write that version as a new revision; "Keep the current version" closes it | C3, C20, §5.6 | S01, S07 |
| Three writers | One conflict stays open on the row and offers every version that was replaced unseen, the first one included | C3 | S57 |
| Field by field, or the row? | The row. A stale write replaces fields it did not mean to change, and the conflict offers the other version back. The position of a folder is the one field that is replaced without a record | C2, C3 | S02, S06, S08 |
| Delete against edit | Whichever arrives last wins, in both orders; the other is offered back; a tombstone lasts 30 days | C5 | S03, S04, S22 |
| Delete against delete | One delete; the second changes nothing | C4, C5 | S05 |
| A delete after a create whose answer was lost | The delete is sent, not dropped, and the row is deleted everywhere | D1, C5, D15 | S50 |
| Move against edit | A snippet's folder is a field of its row, so it is an ordinary conflict of two edits | C2, C3 | S06, S12 |
| A folder's delete against changes inside it | The delete takes everything in the folder as the server has it, and records a conflict for what the deleting device had not seen; a write that arrives after the delete brings the folder back with that row only; a row that had been moved out stays, and the deleting device takes it back | C6, C7, D4 | S15, S16, S51 |
| A folder's delete that is refused | Nothing is deleted; the device puts back the folder and every row it had deleted with it, and lists the delete as not made | C6, C12, D5 | S52 |
| Folders moved into each other, or too deep | The server keeps the tree valid by leaving the offending folder where it is, or one level up, and tells the device | C8 | S13, S14 |
| Two requests at the same moment | They are made one after the other, per profile; the outcome is that of some order of the two | C24 | S53 |
| The same abbreviation on two devices | Both snippets are kept. The one the server accepted first holds the abbreviation on every device; the other is listed in library health | C9, D13 | S09, S25, S48 |
| The same abbreviation in a personal and a shared folder, or in two shared folders | The user's own always holds it, and among the user's own those whose content the user wrote come first. Between shared folders the one shared with the user first, by the server's time | C9, D13 | S10, S11, S54 |
| Tag renamed while it is put on a snippet; folder renamed while moved | Tag and link are different rows: both apply. Name and parent of a folder are one row: the later write wins, with a conflict | C2, C3, C10 | S12, S17 |
| Tag deleted while it is put on a snippet; the same tag put on twice | The link is kept and has no effect while the tag is deleted. Two links for one pair count once, and taking the tag off deletes both | C10 | S18, S64 |
| Attachments added on two devices | Two attachment rows, one stored file; nothing to resolve | C19 | S21 |
| An attachment added to a snippet that is deleted elsewhere | The snippet's row decides (delete against edit); the attachment row and the link follow it, and an attachment nothing uses is removed after 90 days | C5, C10, C19 | S19, S20 |
| Membership or permission changes racing an edit | The permission at the moment the edit arrives counts. A refused edit is kept on the device for the user | C12, C16, D5 | S30, S31, S36, S39 |
| Shares racing each other; a share that ends and begins again | One row per folder and group; a stale change never wins; a delete always applies; a device handles the end and the new beginning in the order they happened | C11, D6 | S37, S67 |
| A shared folder deleted, or snippets moved across its edge | For the people it was shared with, the content ends as a share ends; their unsent changes are kept. What moves in arrives with its tags; what moves out is gone, and so is a tag nothing they can read carries any more | C6, C21, D6, D16 | S61, S62, S63, S65 |
| An invitation accepted while it is withdrawn; ownership handed over | The call that gets the lock first decides; the other is refused with a reason. Ownership changes only when the new owner accepts | C16 | S32, S33, S34 |
| A group deleted with unsent changes on a member's device | The group's folders are revoked and purged; the unsent changes are kept on the device for the user | C16, D6 | S35 |
| Offline for less than 30 days | Nothing special: tombstones and the feed are still there | C22 | S22 |
| Offline for more than 30 days | The device resyncs. Its changes to rows that were deleted and purged meanwhile are refused and kept for the user; everything else is pushed | C13, C22, D7 | S23, S29 |
| Clock skew, ahead or behind | Does not affect which write wins, nor which of two duplicate abbreviations holds | C1, D13 | S24, S25, S54 |
| The same change sent twice | One revision | C15 | S41, S42 |
| The server restored from a backup | Devices send back what it lost. What only the server writes comes back as the backup had it; a removal made since is made again by the device of someone who may | C18, D7 | S43, S58, S59 |
| Device and server on different protocol versions | The older side stops syncing and keeps working locally until it is updated; it still learns when a folder is no longer shared with it | C23, D12 | S44 |
| A revoked device; a copied data directory | The revoked device is refused at once, signs itself out, and keeps what was its user's own. A copy is noticed by its position and comes back as a device of its own | C25, D14 | S60, S66 |

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
  of function arguments stays off on the server. Every account call logs its name, the ids of
  the caller, the group and the person concerned, and its answer; that log is kept outside the
  database, and it is what tells the operator after a restore which account deletions to make
  again (C18). The agent's log on the device records message names, counts, durations and
  codes, never content, names or addresses (architecture §6.1).
- **Deleting an account** is the edge function `account-delete`. It is refused while the user
  owns a group that has other members. Otherwise it calls
  `api_service.delete_account_data`, which marks every device of the account revoked (so every
  token already issued is refused at once, C25), deletes the groups the user owns, ends their
  memberships, and removes their personal rows, history and files from the database without
  the 30 days (§13); and then it removes the account from the authentication service through
  that service's own interface for administrators, which deletes the account's sessions and
  refresh tokens with it. The devices keep their local library, which is a local library again
  (D14). A restore of the server from a backup taken before the deletion brings the account
  back; the restore procedure deletes it again before the server is opened (C18).

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
| A member who may change a folder | Add, change and delete snippets and sub-folders in the tree; add attachments | On every other member's device, content they wrote that presses keys, waits, reads the clipboard, includes another snippet or has several lines is held until approved (D10), under the author the server recorded. No client can set that author, and no write by the reader's own device, stale or not, and no copy can turn it into the reader's (C17, D10; S40, S55, S56). Their snippet can never take over an abbreviation that a snippet with the reader's own content uses, in a shared folder or in the folder owner's private ones, whatever creation time their device sends (D13; S54). They cannot rename, move, delete or unshare the shared folder itself, also when it is a sub-folder of the owner's tree (§4.3). Deletes are tombstones for 30 days; wrong bytes cannot be stored under a hash, and knowing a file's hash gives no way to read it or to learn that the owner has it (§5.13, §6.11); they cannot move content out of the tree, share it onward or change who sees it; the rate limits bound how fast they can add | Single-line text they write, without key presses and without including another snippet, expands on the others' devices without review, for abbreviations that none of the reader's own snippets use (ADR-0022 accepts this). They can delete the tree's content, which the owner must restore within 30 days. They can use up the owner's plan limit with rows (§5.10) |
| A removed member, or one whose share ended | Nothing new from the moment the removal commits: every policy and every function refuses them, including for the tags and attachment rows they wrote there, and unsent changes are refused (C12). Their devices purge the content at the next sync, and at the latest at the daily access check, which runs also on a device that is too old to sync (D6, D7, D12, D16) | — | What their devices already held they may have copied, and a device that never comes online again keeps its copy. The server cannot erase a device. The share dialog should say so. A restore of the server from a backup taken before the removal makes them a member again until a device of an owner or admin has synced and made the removal again (C18, S58); the restore procedure tells owners |
| Someone who holds a member's unlocked device | Read the library on it, and act as that member | The owner of the account revokes the device from another device, or signs out everywhere. The device row is marked revoked, and because every call and every policy checks it, access stops with that commit and not when a token expires; the session cannot register another device, and a token that outlives the session is refused because the session is gone (C25). Then they change what must change | The library on that device stays readable to whoever holds it; malware running as the user is outside the model (architecture §8) |
| Someone who stole a refresh token | Read and write as the user, from a session that is the user's own | Tokens are in the keychain and only `iamb-sync` reads them; sign-in uses PKCE and a loopback address open for seconds (ADR-0020). Refresh tokens rotate, but rotation does not reliably expose a thief: the authentication service honours the token before the active one, and any token for ten seconds, so a thief and the user can go on side by side; only the use of a token older than that ends the session. A stolen token used from another installation with the same device id is noticed by its position (C25); used with a device id of its own it cannot register, because the session already has a device. Access tokens live one hour at most; the device list shows each device's last call; "sign out everywhere" revokes every device row and ends all sessions | Until the user notices, the thief has what the session can do. The remedy is the device list and "sign out everywhere" |
| Someone on the network | See that a device talks to the server | TLS; the live channel carries a number, no content | — |
| Another user of the same computer | Nothing | The data root is the user's alone; the endpoints check the peer (architecture §3.3, §6.1) | — |
| The server's operator, or someone who breaks into the server | Read every synced row; change rows, including who is recorded as an author | Hosting controls, and task 4.3 | In version 1 the server is trusted, for reading and for the author it reports. End-to-end encryption with signed content would end that trust (§13) |
| Someone who sends invitations at random | Make people receive an email | An invitation needs the right to invite and is rate-limited; the email carries no link that accepts; accepting shares nothing of the person's own; only the account whose confirmed address it names can see or accept it (§5.8) | — |

**Required settings of the authentication service** (task 4.3 checks each): refresh-token
rotation with reuse detection on, the reuse interval at its default of ten seconds; access
tokens of one hour or less; email confirmation required, so that no account acts with an
address it has not confirmed; only identity providers that assert a verified address enabled,
and for each a test that an address the provider has not verified leaves `email_confirmed_at`
empty; the realtime service set to private channels only. The service's own limits on a
session's lifetime (time-boxed sessions, inactivity time-out) need a paid hosting plan and are
not relied on; a device that has not called for 180 days is removed by the daily job (§5.12).

### 11.4 Review before use

ADR-0022 holds a received snippet until the user approves it. Sync supplies what that needs: the
author of the content, set by the server (C17) and stored on the device in `snippet_origin`; the
time of the change (`content_at`); the author's display name (`known_user`); and the content the
device was last allowed to type (`approved_content`), which is what keeps expanding and what the
review dialog compares with. A snippet is judged by its content, not by its revision number
(D10): a move, a new label or a switched-off snippet does not ask again. The user's own content
is never held, on any of their devices, and the user's own is only what the user changed.
There are four ways in which content someone else wrote could come to count as the reader's
own, and each is closed:

| Way | Closed by | Scenario |
| --- | --- | --- |
| The reader changes the label or the folder, and the push carries the member's content | The content equals the row's: the author stays | S40 |
| The same from a device that was a moment behind, so the content differs from the row | The content equals the write's base version: the base's author stays | S55 |
| The reader copies the snippet, or moves it to another profile, which makes a new row | The store refuses to copy a held snippet until it is reviewed | S56 |
| The reader's device sends the row again after a restore of the server, or restores a version from a conflict | A re-push names the caller as author only where the device held the caller as author; a restored version keeps its author | — |

What is held, besides key presses, waits, clipboard reads and line breaks: a `{snippet}`
reference written by someone else. Such a reference resolves in the reader's own library, private
folders included, and types what it finds, with that snippet's own key presses. The review
dialog shows the content and what each reference resolves to on this device. This adds to
ADR-0022 and is the owner's to confirm (open question 10).

### 11.5 What the policies and functions must guarantee

The list task 4.3 tests, each as the kinds of user of §4 (owner, admin, member with `edit`,
member with `view`, former member, stranger, signed out).

| | Guarantee |
| --- | --- |
| G1 | `folder_access` equals §4.2 after every commit (compared with the slow reference function, §5.4) |
| G2 | A user reads a folder, a snippet, a link, a share only under §5.13; every table of §4.5 returns exactly the rows listed there, and nothing after the share ends |
| G3 | A tag or an attachment row of someone else's profile is readable only while a readable snippet has a live link to it, or by the user who wrote the row last while that user has `edit` on a folder of the profile; after the share ends or the user is removed, not at all |
| G4 | No role a device can have may insert, update or delete in any table of `iamb`. Nothing in `iamb_private` or `api_service` can be called through the REST interface with a device's token. The gate helpers of §5.1 are the only functions of `iamb_private` that `authenticated` may execute, and none of them returns a fact about anyone but the caller or changes anything but the caller's device row and rate counters. `anon` can read nothing and call nothing |
| G5 | `history` is unreadable directly; a history version is returned only by `get_conflict` to a party of the conflict who may still read the row |
| G6 | `feed`, `feed_head`, `device`, `folder_access`, `attachment_upload` and `conflict` rows are readable by their user only |
| G7 | Every write of §4.3 succeeds for the users that table allows and is `rejected: forbidden` for all others, judged at arrival (C12): after a membership, role or share change commits, the next push sees it. A member with `edit` cannot rename, move or delete the root of the share that gives the access, at any depth |
| G8 | Every account call of §4.1 succeeds for the roles that table allows and fails for all others with the code of §5.14; a group always has exactly one owner, and nobody becomes one without `accept_ownership` |
| G9 | `updated_by`, `updated_device`, `content_by`, `rev`, `created_rev`, `updated_at` and `deleted_at` are set by the server; nothing a client sends changes them. `content_by` follows the four cases of C17: the caller only for content that differs from the row and from the write's base version |
| G10 | A profile, membership or invitation cannot be written by `push` (C16) |
| G11 | An invitation is readable by the group's owner and admins and by the account whose confirmed address it names, and only that account can accept it; an account with an unconfirmed address sees none |
| G12 | A stale change to a share is never applied, and a share's delete always is (C11) |
| G13 | After a member is removed, a share deleted, a shared folder deleted or a group deleted, the same transaction has removed the access entries: the former reader's next query returns no row of that content, including rows they wrote, and their feed holds `folder_revoked` |
| G14 | A stored file is readable only as `<profile>/<sha256>` of a verified file that a snippet the user can read links to, or of the user's own profile or group; a request for any other name, also for an existing file whose hash the user knows, answers exactly like one for a file that does not exist; `attachment_upload_begin` and the push result say the same to a caller who cannot read the file whether it exists or not; an upload object can be written once, by its uploader, and read by nobody |
| G15 | A revoked device, a session without a device row, and a session that no longer exists are refused by every function and read no row through any policy; a revoked session cannot register a device again |
| G16 | The live channel delivers to a user only the topic of their own id and accepts no message from a client |
| G17 | Functions that write are `security definer` with a fixed `search_path`, take the user and the session from the token only, and build no statement from their arguments |
| G18 | Size, count and rate limits are enforced by the server whatever the client sends (§5.2, §6.8); no request deletes more than `cascade_max` rows by cascade |
| G19 | A repeated push writes once (C15), also when two copies arrive at the same time |
| G20 | No function's log line and no error text contains row content |
| G21 | Two requests at the same moment end as some order of the two (C24). Tested with two connections, the second started while the first holds its locks: folders moved into each other; a create in a folder being deleted; a sub-folder created while its share ends; a push into a group's folder while a member is removed. After each, I7 holds and G1 holds. Every writing function holds its profile locks before its first read of a row it decides on |
| G22 | A position is accepted once; a call with a position that is neither the latest nor the one before it is answered `device_conflict` and writes nothing (C25) |
| G23 | After `begin_epoch`, a `repush` is honoured only for a revision of the lost span, creates a row only then, and never without the permission of C12 (C18) |

### 11.6 What the design assumes of the platform

Each assumption about Supabase was either checked in its documentation, removed from the
design, or is a step task 4.2 (or 4.6) verifies on the deployed project before it builds on it.
"Checked" names the guide; the review of this design (its §6) checked the first three.

| Assumption | Status | What the design does with it |
| --- | --- | --- |
| The access token carries the session's id, and the session can be looked up | Checked. Sessions guide: every access token has a `session_id` claim, which can be compared with the rows of `auth.sessions` | `session_ok()` (C25) |
| The database can send on a private live channel from inside the writing transaction; clients cannot send | Checked by the review (broadcast guide) | §5.5, §6.6 |
| Definer functions owned by the tables' owner are not subject to row-level security | Checked by the review (row-level security guide) | §5.1 says so and relies on the functions' own checks, not on "forced" |
| A policy cannot read its own table | Knowledge of Postgres, and the helper pattern is in the same guide | The gate helpers of §5.1. **4.2 verifies first:** one policy test per table |
| Refresh tokens: the token before the active one is honoured, any token for ten seconds, an older one ends the session | Checked. Sessions guide | §11.3 claims no more than that |
| Time-boxed sessions and inactivity time-outs need a paid plan | Checked. Sessions guide: "only available on Pro Plans and up" | Not relied on |
| A function can run longer than the role's 8 seconds | Checked. Timeouts guide: `authenticated` has 8 seconds; a function's own `statement_timeout` is honoured for calls through the REST interface; 60 seconds at most there | Time-outs per function (§5.14) and bounded work per request (§6.3, C6). **4.2 verifies** that a function's own time-out applies on the deployed project, and measures a push of 200 ops and a cascade of 2,000 rows against 20 seconds |
| Every online device can hold a live channel | Checked, with a limit. Realtime quotas: 200 connections at once on the free plan, 500 on the first paid one, 10,000 above; a join over the limit is refused with `too_many_connections` | A refused device polls (§6.6); the hosting plan is open question 3 |
| A backup is consistent to one moment, and the restored revision counter is not below a restored revision | Checked by the review for current versions (physical backups); the counter by knowledge of Postgres | Devices compare row by row, so exactness is not needed (C18, D7). **4.2 verifies** in its restore test that `rev_seq` is at or above every `rev` after a restore |
| A trigger on the authentication service's user table creates the personal profile | Documented (managing user data guide), with the warning that a failing trigger blocks sign-ups | Removed: `hello` creates the profile (§5.3) |
| A database function can end a session or delete an account in the authentication service | Not needed. Revocation is the device row, which every call and policy checks (C25). An account is deleted through the service's interface for administrators from an edge function; the same guide says this deletes the user's sessions and refresh tokens and that tokens already issued stay valid until they expire, which C25 covers | §11.1, C25. **4.2 verifies** that the functions' owner may read `auth.sessions` and `auth.users`, which `session_ok()` and `own_address()` need |
| An edge function can hash a stored file and call the database | Limits checked. Edge function limits: 256 MB of memory, 2 seconds of processor time, 150 seconds of wall-clock time on the free plan and 400 on paid ones | Files of at most 25 MiB, read into memory once and hashed with the runtime's own SHA-256; the function calls `api_service` through the REST interface with the service key (§6.11). **4.6 verifies** by committing a 25 MiB file and reading the processor time it took |
| A daily job runs in the database | Checked. Cron guide: Supabase Cron runs SQL and database functions on a schedule. The guide does not say on which plans | The job is one function that can be run any number of times. **4.2 verifies** that the project's plan has Cron; if not, any scheduler that calls the function daily will do |
| An address from an identity provider is verified | Not checked | Not assumed: the functions read `email_confirmed_at` (§5.8). **4.3 verifies** per enabled provider (§11.3) |
| The REST interface can be closed while a database is restored | Not checked | The restore procedure needs it (C18). **4.2 verifies** with the hosting plan how (network restrictions, or pausing the project) and writes it into the procedure |

## 12. Conflict scenarios

Each scenario gives the starting state, the steps in order, and the defined outcome: the state
of every row on the server and on every device, the history and the conflicts the server holds,
and what each device shows its user. They are written out here from
[`spec/sync/scenarios.toml`](../spec/sync/scenarios.toml), which is the source
(`python3 spec/sync/render_scenarios.py` renders this section; `--check` fails when it is out of
date). The format is documented in `crates/iamb-testkit/src/sync_scenarios.rs`.

**What the check does.** `cargo test -p iamb-testkit --test sync_scenarios` fails when there are
fewer than 30 scenarios, when an id is used twice, when a scenario does not state every row for
the server and for every device, when a case the task or the review asks for is covered by no
scenario, or when this section lacks a scenario or a rule it cites. Beyond that structure it
checks that each outcome agrees with the scenario's own steps and answers. It follows every
change from the device that made it to the answer the scenario gives for it, keeping a ledger of
the version each row has on the server and of the version each device had seen, and requires:

- the server's row is the last write that reached it, live or deleted as that write left it, or
  absent when nothing reached it or a purge is due; a refused or blocked write is never the
  server's row;
- the history holds exactly the versions that later writes replaced;
- a write that replaced a version its device had not seen, written on another device, has a
  conflict that names both, of the kind the two writes make, shown on the devices of the
  authors; a conflict that was replaced hands its versions on;
- a refused change is kept on the device that made it; a blocked one stays there with its row
  and its outbox entry; no change is left unanswered on a device that ends online;
- a device that ends online holds each row as the server has it or not at all, holds what it
  wrote itself, and two such devices of one account hold the same.

The tests feed it scenarios that are wrong on purpose, among them the eight with which the
review showed that a purely structural check accepts anything; each must be refused.

**What it does not do, and where the line is.** The check takes the answers (`applied`,
`rejected:forbidden`), the rows a write takes with it, and what each user may see from the
scenario; it does not derive them, because it has no model of permissions, content, the feed or
the store. An outcome that is consistent with its own answers and wrong under the rules of §8
and §9 still passes. That is checked when the two-device simulation of task 4.4 replays every
scenario against the real store and sync client and a model of the server, and compares each
outcome field by field; its 1,000 random runs check the invariants of §1 instead of fixed
outcomes.

How to read one:

- Devices start signed in, online and in step with the server unless the start says otherwise.
  An online device syncs after every step that concerns it; `offline` and `online` bracket work
  done without the server; `manual` leaves the network up but lets the scenario say when the
  device pulls and pushes.
- A version is named after the step that wrote it (`a1`), and `start` is what a row holds at the
  start. In an outcome, `live · b1` means the row is live in the revision the step `b1` wrote;
  `deleted · b1` that `b1` deleted it; `absent` that no row with that id is there (never
  received, purged, or not visible to that user; for an invitation: not listed for that user).
  A step that says "on the server the change also writes" names the rows the server writes
  because of it: what a folder's delete takes with it, a folder a write restores, the
  memberships an account call changes. `=` in a device's column means "as on the server".
- "Shown" is what the device shows its user, as a code: `conflict` (a conflict is listed and
  marked on the row), `kept` (changes that were not synced are kept on this computer), `blocked`
  (a row cannot be synced yet), `shared` and `revoked` (a folder arrived or went), `duplicate`
  and `broken` (library health), `review` (a snippet waits for review), `adjusted` (a folder was
  placed elsewhere), `superseded` (a sharing change was not made), `plan_limit`, `upgrade`,
  `invitation_invalid`, `refused` (an account call was refused, with its reason), `signed_out`
  (the server signed this device out) and `restored` (the server was restored from a backup;
  look at your groups). The wording is task 4.5's; none of them is a notice over another
  application (UX spec §4.11).

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
| S34 | The owner of a team hands the team to another member, who accepts | membership | ownership-transfer |
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
| S50 | A new snippet reaches the server, the answer is lost, and the snippet is deleted before the next push | transport | lost-answer-then-delete |
| S51 | One device deletes a folder, the other had moved a snippet out of it | structure | folder-delete-vs-move-out |
| S52 | A folder's delete is refused, and what it had deleted with it comes back | sharing | folder-delete-refused |
| S53 | Two requests arrive at the same moment | structure | simultaneous-requests |
| S54 | A member creates, in a shared folder, an abbreviation the owner uses privately | abbreviation | abbreviation-takeover |
| S55 | The reader's stale write carries a member's unapproved content | sharing | stale-write-held-content |
| S56 | A held snippet is copied into the reader's own folder only after its review | sharing | copy-held-snippet |
| S57 | Three people edit one snippet offline | edit | three-writers |
| S58 | The server is restored from a backup taken before a member was removed | server | restore-membership |
| S59 | After a restore, only one device holds what the backup lacks | server | restore-single-holder |
| S60 | A data directory is copied to a second computer and both are used | server | copied-data-directory |
| S61 | The owner deletes a shared folder while a member holds an unsent change in it | sharing | shared-folder-deleted |
| S62 | The owner moves snippets across the edge of a shared folder | sharing | move-across-share-edge |
| S63 | A tag is taken off the only shared snippet that carried it | tags | tag-leaves-share |
| S64 | A tag is put on one snippet on two devices and taken off on one | tags | duplicate-links |
| S65 | A folder and a sub-folder are shared separately and the outer share ends | sharing | nested-shares |
| S66 | A device is revoked while it holds unsent changes | server | device-revoked-with-pending |
| S67 | A share ends and begins again inside one page of the feed | sharing | share-ends-and-begins |

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

**Outcome.** Both snippets exist everywhere. On both devices the one the server accepted first, s2,
holds the abbreviation; s3 neither expands nor resolves and library health lists it under
"abbreviation taken" until the user renames or deletes one.

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
   pull and this answer B's store holds the cycle; the view shows such folders at the top level. The
   server itself never holds a cycle, also when the two requests arrive together (C24, S53).*

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
   days.`. On the server the change also writes `f1`.
3. `a1` · A deletes `f1`. On the server the change also writes `s1`, `s2`. Answer: `f1` `applied`.
   *The push names the rows the delete cascaded to on A (s1, s2) with their base revisions. The
   server deletes the whole subtree (C6).*
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
2. `a1` · A deletes `f1`. On the server the change also writes `s1`, `s2`, `s3`.
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
6. `a2` · A deletes `f2`. On the server the change also writes `s4`. Answer: `f2` `applied`.
7. 45 days pass. *On day 31 the server's purge removes s1, f2 and s4 and their history for good.*
8. B comes back online and syncs. Answer: `reset_required:expired`, `s1` `rejected:gone`, `s3`
   `applied`, `s4` `rejected:gone`. *B's position is older than the feed, so B resyncs (D7): it
   fetches everything it may see and removes synced rows the server no longer has, except rows with
   unsent changes and the folders above them, which it notes as left alone: s1, s4 and f2. Its
   pushes for s1 and s4 name rows the server does not know (C13). B keeps both changes for the user
   and removes the rows (D5). Then nothing is unsent under f2 any more, the server's snapshot did
   not hold it, and the store removes it as well (D7).*

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
   in the past is kept as given, and decides nothing. Among duplicates the snippet the server
   accepted first holds the abbreviation (D13): that is s2, whatever the two clocks say.*

**Outcome.** b1 wins s1 by arriving last, although B stamped it three days before a1. Both ;sig
snippets exist; s2 holds the abbreviation on both devices because the server accepted it first,
although B stamped s3 three days earlier, and library health lists s3.

| Row | Server | A | B |
| --- | --- | --- | --- |
| `f1` | live · start | = | = |
| `s1` | live · b1 | = | = |
| `s2` | live · a2 | = | = |
| `s3` | live · b2 | = | = |

- History (replaced versions the server keeps): `s1` start, a1.
- Conflicts: `s1` edit_edit, winner b1, loser a1, shown to ines, open.
- A: shown conflict, duplicate; expands `;sig` as `s2`.
- B: shown conflict, duplicate; expands `;sig` as `s2`.

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
keeps the tombstone of his own membership and sees nothing else of the group; on Ines's device his
membership is simply gone, because nobody reads another person's tombstone.

| Row | Server | A | C |
| --- | --- | --- | --- |
| `m-ines` | live · start | = | absent |
| `m-tom` | deleted · x1 | absent | = |
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
| `inv1` | invitation | group `g-fam`, user `tom` | — | role `member`, status `pending` |
| `f1` | folder | profile `p-ines` | 3 | name `Shop replies` |
| `s1` | snippet | folder `f1` | 5 | abbreviation `;hrs`, body `Open 9 to 5.` |
| `sh1` | folder share | folder `f1`, group `g-fam` | 7 | permission `view` |

**Steps.**

1. `c1` · C: the user accepts the invitation `inv1` (membership `m-tom`). Answer: `ok`. *Both calls
   lock the group's profile row (C24). Tom's commits first: the membership exists, the invitation is
   accepted, and folder_visible(f1) is in Tom's feed.*
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

- History (replaced versions the server keeps): none.
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
| `inv1` | invitation | group `g-fam`, user `tom` | — | role `member`, status `pending` |
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

- History (replaced versions the server keeps): none.
- Conflicts: none.
- A: shown nothing.
- C: shown invitation_invalid.

#### S34 · The owner of a team hands the team to another member, who accepts

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

1. `x1` · A: the user offers the ownership of `g-team` to tom. Answer: `ok`. *The call records an
   offer on the group and changes nobody's role: owning a group is not something that can be done to
   a person. Ines stays the owner until Tom answers.*
2. `x2` · C: the user accepts the ownership of `g-team`. On the server the change also writes
   `m-ines`, `m-tom`. Answer: `ok`. *One transaction: the profile's owner becomes Tom, Tom's
   membership becomes owner and Ines's becomes admin. A group has exactly one owner at every
   moment.*
3. `x3` · C: the user leaves the group (membership `m-tom`). Answer: `error:owner_cannot_leave`.
4. `x4` · A: the user deletes the group `g-team`. Answer: `error:not_owner`.
5. `a1` · A edits `gs1`: body `Open 9 to 6.`. Answer: `gs1` `applied`. *As admin Ines may still
   change the team's snippets.*

**Outcome.** Once Tom has accepted, he is the owner and Ines an admin on every member's device. The
two refused calls change nothing and each device says why. Ines's edit is accepted because admins
may change the team's content.

| Row | Server | A | C | E |
| --- | --- | --- | --- | --- |
| `m-ines` | live · x2 (role `admin`) | = | = | = |
| `m-tom` | live · x2 (role `owner`) | = | = | = |
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
4. `x1` · A: the user deletes the group `g-fam`. On the server the change also writes `m-ines`,
   `m-tom`, `sh1`, `gf1`, `gs1`. Answer: `ok`. *One transaction: the profile, its memberships, the
   shares into it and the folders it owns, with their content, are deleted. Every member, the owner
   included, gets folder_revoked for the group's folders; the members other than the folder's owner
   get it for the folders shared into the group.*
5. C comes back online and syncs. *C pulls first: both folders are revoked, the two unsent changes
   are kept for the user and the folders purged (D6).*

**Outcome.** The group, its own folder and the share are gone for everyone at once; no call brings a
deleted group or its content back. Ines keeps her folder f1, unshared. Tom's device holds nothing of
the group and keeps his two changes in the list of changes that were not synced. Each of them keeps
only the tombstone of their own membership.

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

Category: server. Covers: server-restore. Rules: C13, C18, D7.

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
   position belongs to the old epoch. A resyncs and compares row by row (D7): it sends again each
   row the server holds in an older revision or does not hold at all: s1 as of a2, s2, and the
   tombstone of s3. The re-push of s2 creates the row, which a re-push of a revision the restore
   lost may do (C13, C18). The restored rows were not written in the new epoch, so nothing is in
   conflict.*
8. B pulls. Answer: `reset_required:epoch`, `s1` `unchanged`, `s2` `unchanged`, `s3` `unchanged`. *B
   pulls after A's re-push: the rows it would send are already there as it holds them, so its
   re-pushes change nothing (C4).*

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
   its rows move to the account's personal profile and every one of them is queued in the same
   transaction; B fetches the account's library; a local folder with the name and place of an
   account folder is merged into it, so ls1 and ls2 move into f1 and lf1 is dropped with its queue
   entry, without ever being sent; a local snippet identical to one in the account would be dropped
   too. Nothing is pushed before the merge is done; then what is left is pushed as new rows.*

**Outcome.** Both computers hold one folder "Snippets" with all four snippets. The two ;em snippets
differ, so both are kept: s1, which the server accepted first, holds the abbreviation and library
health lists ls1 on both computers. Nothing that was on either computer is lost.

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

#### S50 · A new snippet reaches the server, the answer is lost, and the snippet is deleted before the next push

Category: transport. Covers: lost-answer-then-delete. Rules: C3, C5, C15, D1, D2, D4, D15.

**Start.** Device A creates s2 and pushes it. The server commits the row, but the answer is lost and
the sync agent quits before it can repeat the request. Then the user deletes s2.

Devices: A (ines), B (ines). Profiles: `p-ines` (personal, ines).

| Row | Kind | Refers to | Rev | Fields |
| --- | --- | --- | --- | --- |
| `f1` | folder | profile `p-ines` | 3 | name `Replies` |

**Steps.**

1. A stays connected but syncs only when a step says so.
2. `a1` · A creates the snippet `s2` (folder `f1`): abbreviation `;rlate`, body `Your order left us
   late.`.
3. A pushes. Answer: `lost`. *The server commits s2 and writes its feed entries; B receives it. The
   answer never arrives. On A the outbox entry of a1 is marked as handed out, in the store, when the
   agent reads it (D1).*
4. A's sync agent stops and starts again. *The agent quits before it repeats the request. Nothing
   remembers the request but the outbox.*
5. `a2` · A deletes `s2`. *The row's revision on A is still 0. Because an entry of the row was
   handed out, the delete is not dropped with the row: it waits in the outbox with base 0 and names
   a1 as the change it follows.*
6. A syncs by itself again. Answer: `s2` `applied`. *A pulls: the feed holds s2 as the server has
   it. The row has an unsent change, so it is skipped and the skipped revision is noted (D2, D15). A
   pushes the delete. For the server it is a stale delete, and the version it replaces was written
   by the change the op names as its own earlier one: applied, no conflict (C3, C5). The answer's
   revision is above the one skipped, so nothing is left to fetch.*

**Outcome.** The snippet is deleted on the server and on both devices, by a2. The delete is not lost
although device A never learned that its create had arrived, and the two devices of the account
agree.

| Row | Server | A | B |
| --- | --- | --- | --- |
| `f1` | live · start | = | = |
| `s2` | deleted · a2 | = | = |

- History (replaced versions the server keeps): `s2` a1.
- Conflicts: none.
- A: shown nothing.
- B: shown nothing.

#### S51 · One device deletes a folder, the other had moved a snippet out of it

Category: structure. Covers: folder-delete-vs-move-out. Rules: C6, D2, D4, D15.

**Start.** Ines's folder f1 holds s1 and s2; g1 is another folder. Device A, offline, deletes f1.
Device B moves s1 to g1. Then A comes back.

Devices: A (ines), B (ines). Profiles: `p-ines` (personal, ines).

| Row | Kind | Refers to | Rev | Fields |
| --- | --- | --- | --- | --- |
| `f1` | folder | profile `p-ines` | 3 | name `Returns` |
| `g1` | folder | profile `p-ines` | 4 | name `Shop` |
| `s1` | snippet | folder `f1` | 5 | abbreviation `;rret`, body `Return label attached.` |
| `s2` | snippet | folder `f1` | 6 | abbreviation `;rno`, body `We cannot take it back.` |

**Steps.**

1. A goes offline.
2. `a1` · A deletes `f1`. On the server the change also writes `s2`. *On A the delete takes s1 and
   s2 with it; both are named in the cascade list of the push.*
3. `b1` · B moves `s1` to `g1`. Answer: `s1` `applied`.
4. A comes back online and syncs. Answer: `f1` `applied`. *A pulls: s1 in g1 is skipped, because A's
   own delete of it is unsent (D2), and the skip is noted (D15). The server deletes what is below f1
   now, which is s2 only (C6). The answer carries the server's row for every row of the cascade
   list. A takes s1 as the server has it, live in g1, and drops its own delete of it (D4).*

**Outcome.** The folder and the snippet still in it are deleted. The snippet that was moved out
before the delete arrived is live in its new folder on both devices: a folder's delete takes what is
in the folder on the server, not what the deleting device remembered. No conflict is needed, because
nothing was replaced.

| Row | Server | A | B |
| --- | --- | --- | --- |
| `f1` | deleted · a1 | = | = |
| `g1` | live · start | = | = |
| `s1` | live · b1 (folder `g1`) | = | = |
| `s2` | deleted · a1 | = | = |

- History (replaced versions the server keeps): `f1` start; `s1` start; `s2` start.
- Conflicts: none.
- A: shown nothing.
- B: shown nothing.

#### S52 · A folder's delete is refused, and what it had deleted with it comes back

Category: sharing. Covers: folder-delete-refused. Rules: C6, C12, D5.

**Start.** Ines shares f1 with the family, to change. It has a sub-folder f1b with the snippet s2.
Tom, offline, deletes the sub-folder. Before his device is back, Ines lowers the share to "use
only".

Devices: A (ines), C (tom). Profiles: `p-ines` (personal, ines), `p-tom` (personal, tom), `g-fam`
(family, ines).

| Row | Kind | Refers to | Rev | Fields |
| --- | --- | --- | --- | --- |
| `m-ines` | membership | group `g-fam`, user `ines` | 2 | role `owner` |
| `m-tom` | membership | group `g-fam`, user `tom` | 6 | role `member` |
| `f1` | folder | profile `p-ines` | 3 | name `Shop replies` |
| `f1b` | folder | profile `p-ines`, parent `f1` | 4 | name `Returns` |
| `s2` | snippet | folder `f1b` | 5 | abbreviation `;rret`, body `Return label attached.` |
| `sh1` | folder share | folder `f1`, group `g-fam` | 7 | permission `edit` |

**Steps.**

1. C goes offline.
2. `c1` · C deletes `f1b`. *On C the delete takes s2 with it.*
3. `a1` · A edits `sh1`: permission `view`. Answer: `sh1` `applied`.
4. C comes back online and syncs. Answer: `f1b` `rejected:forbidden`. *The permission counts as it
   is when the delete arrives (C12). The answer carries the server's row for the folder and for
   every row of the cascade list. C puts f1b and s2 back as the server has them, removes its entries
   for both, and lists the delete as a change that was not made (D5).*

**Outcome.** Nothing is deleted. On Tom's device the sub-folder and its snippet are back, the folder
is read-only, and the list of changes that were not synced says that his deletion of "Returns" was
not made.

| Row | Server | A | C |
| --- | --- | --- | --- |
| `m-ines` | live · start | = | = |
| `m-tom` | live · start | = | = |
| `f1` | live · start | = | = |
| `f1b` | live · start | = | = |
| `s2` | live · start | = | = |
| `sh1` | live · a1 | = | = |

- History (replaced versions the server keeps): `sh1` start.
- Conflicts: none.
- A: shown nothing.
- C: shown kept; kept for the user: c1; permission `f1` view.

#### S53 · Two requests arrive at the same moment

Category: structure. Covers: simultaneous-requests. Rules: C6, C7, C8, C24.

**Start.** Both of Ines's devices are online and in step. Within the same second device A moves fa
into fb and deletes f1, and device B moves fb into fa and creates s3 in f1. Neither has seen the
other's changes, and the two pushes reach the server together.

Devices: A (ines), B (ines). Profiles: `p-ines` (personal, ines).

| Row | Kind | Refers to | Rev | Fields |
| --- | --- | --- | --- | --- |
| `fa` | folder | profile `p-ines` | 3 | name `Replies` |
| `fb` | folder | profile `p-ines` | 4 | name `Orders` |
| `f1` | folder | profile `p-ines` | 5 | name `Returns` |
| `s1` | snippet | folder `f1` | 6 | abbreviation `;rret`, body `Return label attached.` |

**Steps.**

1. A stays connected but syncs only when a step says so.
2. B stays connected but syncs only when a step says so.
3. `a1` · A moves `fa` to `fb`.
4. `b1` · B moves `fb` to `fa`.
5. `a2` · A deletes `f1`. On the server the change also writes `s1`.
6. `b2` · B creates the snippet `s3` (folder `f1`): abbreviation `;rwhen`, body `Refunds take five
   days.`. On the server the change also writes `f1`.
7. A pushes. Answer: `fa` `applied`, `f1` `applied`. *Both requests are at the server. Each first
   locks the row of the profile it writes, before it reads anything it decides on. A's request gets
   the lock; B's waits for A's commit (C24).*
8. B pushes. Answer: `fb` `unchanged+adjusted`, `s3` `applied`. *B's request reads what A's
   committed. Moving fb into fa would close a cycle, so the parent is kept (C8). s3 arrives in a
   folder that is deleted, so the folder is restored, with a conflict on it (C7), as in S15. Had B's
   request taken the lock first, A's would have waited, with the outcomes of S13 mirrored and of
   S16. In neither order can both moves commit, or a live snippet stay under a deleted folder.*
9. A syncs by itself again.
10. B syncs by itself again.

**Outcome.** The outcome is that of the two requests one after the other, A's first: fa is inside fb
and fb stays at the top; f1 is back with the new snippet only, and a conflict on it says that it was
deleted on one device and used on the other. The server never holds a folder cycle or a live snippet
under a deleted folder.

| Row | Server | A | B |
| --- | --- | --- | --- |
| `fa` | live · a1 (parent `fb`) | = | = |
| `fb` | live · start | = | = |
| `f1` | live · b2 | = | = |
| `s1` | deleted · a2 | = | = |
| `s3` | live · b2 | = | = |

- History (replaced versions the server keeps): `fa` start; `f1` start, a2; `s1` start.
- Conflicts: `f1` folder_restored, winner b2, loser a2, shown to ines, open.
- A: shown conflict.
- B: shown adjusted, conflict.

#### S54 · A member creates, in a shared folder, an abbreviation the owner uses privately

Category: abbreviation. Covers: abbreviation-takeover. Rules: C9, C17, D13.

**Start.** Ines's private folder f2 holds her snippet ;iban. She shares another folder, f1, with the
family, to change. Tom's device, whose clock is far behind, creates a snippet ;iban in f1.

Devices: A (ines), C (tom). Profiles: `p-ines` (personal, ines), `p-tom` (personal, tom), `g-fam`
(family, ines).

| Row | Kind | Refers to | Rev | Fields |
| --- | --- | --- | --- | --- |
| `m-ines` | membership | group `g-fam`, user `ines` | 2 | role `owner` |
| `m-tom` | membership | group `g-fam`, user `tom` | 6 | role `member` |
| `f1` | folder | profile `p-ines` | 3 | name `Shop replies` |
| `f2` | folder | profile `p-ines` | 4 | name `Private` |
| `s3` | snippet | folder `f2` | 5 | abbreviation `;iban`, body `Our account number is on the invoice.` |
| `sh1` | folder share | folder `f1`, group `g-fam` | 7 | permission `edit` |

**Steps.**

1. C's clock is 400 days behind true time.
2. `c1` · C creates the snippet `s5` (folder `f1`): abbreviation `;iban`, body `Please pay to the
   account Tom gave you.`. Answer: `s5` `applied`. *The server does not compare abbreviations (C9).
   It keeps the creation time Tom's device sent, which lies long before that of s3, and that time
   decides nothing. The shared folder is part of Ines's own profile, so "her own profile first"
   would not protect her. What does: on her devices a snippet whose content she wrote comes before
   every snippet whose content someone else wrote, and the server, not Tom's device, says who wrote
   s5 (C17, D13).*

**Outcome.** Both snippets exist. On Ines's device her own ;iban keeps expanding and Tom's is listed
in library health; one line of text by a member can never take over an abbreviation the owner's own
snippets use. On Tom's device, which cannot see s3, his snippet holds the abbreviation.

| Row | Server | A | C |
| --- | --- | --- | --- |
| `m-ines` | live · start | = | = |
| `m-tom` | live · start | = | = |
| `f1` | live · start | = | = |
| `f2` | live · start | = | absent |
| `s3` | live · start | = | absent |
| `sh1` | live · start | = | = |
| `s5` | live · c1 (content_by `tom`) | = | = |

- History (replaced versions the server keeps): none.
- Conflicts: none.
- A: shown duplicate; expands `;iban` as `s3`.
- C: shown nothing; expands `;iban` as `s5`.

#### S55 · The reader's stale write carries a member's unapproved content

Category: sharing. Covers: stale-write-held-content. Rules: C2, C3, C17, D10.

**Start.** Ines shares f1 with the family, to change. Tom changes its one-line snippet s1 so that it
reads the clipboard and presses Enter; on Ines's devices that content is held for review. Tom
changes it again. Before Ines's device A has received the second change, she changes the label
there.

Devices: A (ines), B (ines), C (tom). Profiles: `p-ines` (personal, ines), `p-tom` (personal, tom),
`g-fam` (family, ines).

| Row | Kind | Refers to | Rev | Fields |
| --- | --- | --- | --- | --- |
| `m-ines` | membership | group `g-fam`, user `ines` | 2 | role `owner` |
| `m-tom` | membership | group `g-fam`, user `tom` | 6 | role `member` |
| `f1` | folder | profile `p-ines` | 3 | name `Shop replies` |
| `s1` | snippet | folder `f1` | 5 | abbreviation `;wtrack`, label `Tracking link`, body `Your tracking link: ` |
| `sh1` | folder share | folder `f1`, group `g-fam` | 7 | permission `edit` |

**Steps.**

1. `c1` · C edits `s1`: body `Your tracking link: {clipboard}{key name="enter"}`. Answer: `s1`
   `applied`. *Held on A and B: content by someone else that reads the clipboard and presses a key.
   Both keep typing the line of the start.*
2. A stays connected but syncs only when a step says so. *Stands for the seconds between two syncs,
   or for a train ride.*
3. `c2` · C edits `s1`: body `Tracking: {clipboard}{key name="enter"}`. Answer: `s1` `applied`.
4. `a1` · A edits `s1`: label `Paste tracking link`. *The row on A carries the content of c1, held.
   Ines changes the label only.*
5. A syncs by itself again. Answer: `s1` `applied+conflict`. *A's push carries the whole row (C2):
   the content of c1 with the new label, based on c1's revision. It is stale and it wins (C3). Its
   abbreviation, kind, body, trigger, case mode and word-start flag differ from the row it replaces,
   but they equal those of its base version, which the server holds in the history: Ines did not
   change the content. The author of the content stays the base's, Tom (C17). A conflict offers c2
   back.*

**Outcome.** The row has Ines's label and the content of c1, whose author is still Tom: it stays
held for review on both of Ines's devices, and typing ;wtrack keeps typing the old line there. A
stale write by the reader cannot make a member's key presses count as the reader's own. The conflict
offers Tom's second version back.

| Row | Server | A | B | C |
| --- | --- | --- | --- | --- |
| `m-ines` | live · start | = | = | = |
| `m-tom` | live · start | = | = | = |
| `f1` | live · start | = | = | = |
| `s1` | live · a1 (content_by `tom`, updated_by `ines`) | = | = | = |
| `sh1` | live · start | = | = | = |

- History (replaced versions the server keeps): `s1` start, c1, c2.
- Conflicts: `s1` edit_edit, winner a1, loser c2, shown to ines and tom, open.
- A: shown review, conflict; held for review: `s1`.
- B: shown review, conflict; held for review: `s1`.
- C: shown conflict.

#### S56 · A held snippet is copied into the reader's own folder only after its review

Category: sharing. Covers: copy-held-snippet. Rules: C17, D10.

**Start.** Ines shares f1 with the family, to change. Its snippet s1 was written by Tom, reads the
clipboard and presses Enter, and is held for review on both of Ines's devices. On device A she
chooses "Copy to my snippets".

Devices: A (ines), B (ines), C (tom). Profiles: `p-ines` (personal, ines), `p-tom` (personal, tom),
`g-fam` (family, ines).

| Row | Kind | Refers to | Rev | Fields |
| --- | --- | --- | --- | --- |
| `m-ines` | membership | group `g-fam`, user `ines` | 2 | role `owner` |
| `m-tom` | membership | group `g-fam`, user `tom` | 6 | role `member` |
| `f1` | folder | profile `p-ines` | 3 | name `Shop replies` |
| `f2` | folder | profile `p-ines` | 4 | name `Mine` |
| `s1` | snippet | folder `f1` | 8 | abbreviation `;wtrack`, body `Your tracking link: {clipboard}{key name="enter"}`; written by tom |
| `sh1` | folder share | folder `f1`, group `g-fam` | 7 | permission `edit` |

**Steps.**

1. `a1` · A: the user copies `s1` into `f2`. Answer: `error:needs_review`. *A copy is a new row, and
   the server names the creator of a row as the author of its content (C17): copied unseen, Tom's
   key presses would be Ines's own on all her devices. The store refuses to copy, duplicate or move
   to another profile a snippet that is held on this device, and offers the review (D10).*
2. A: the user reviews `s1` and approves it on this device. *Ines sees the content, its author and
   the time of the change in the review dialog, and allows it on this device.*
3. `a2` · A: the user copies `s1` into `f2` (new snippet `s9`). Answer: `s9` `applied`.

**Outcome.** The copy exists only after Ines has reviewed the snippet on the device that makes the
copy; from then on it is her own snippet, on both her devices. The shared snippet itself is still
held on her other device, where she has not reviewed it.

| Row | Server | A | B | C |
| --- | --- | --- | --- | --- |
| `m-ines` | live · start | = | = | = |
| `m-tom` | live · start | = | = | = |
| `f1` | live · start | = | = | = |
| `f2` | live · start | = | = | absent |
| `s1` | live · start | = | = | = |
| `sh1` | live · start | = | = | = |
| `s9` | live · a2 (content_by `ines`) | = | = | absent |

- History (replaced versions the server keeps): none.
- Conflicts: none.
- A: shown review.
- B: shown review; held for review: `s1`.
- C: shown nothing.

#### S57 · Three people edit one snippet offline

Category: edit. Covers: three-writers. Rules: C3, D2.

**Start.** Ines shares f1 with the family, to change. Tom, Ines and Jun each change the snippet s1
while offline, and come back in that order.

Devices: A (ines), C (tom), E (jun). Profiles: `p-ines` (personal, ines), `p-tom` (personal, tom),
`p-jun` (personal, jun), `g-fam` (family, ines).

| Row | Kind | Refers to | Rev | Fields |
| --- | --- | --- | --- | --- |
| `m-ines` | membership | group `g-fam`, user `ines` | 2 | role `owner` |
| `m-tom` | membership | group `g-fam`, user `tom` | 6 | role `member` |
| `m-jun` | membership | group `g-fam`, user `jun` | 9 | role `member` |
| `f1` | folder | profile `p-ines` | 3 | name `Shop replies` |
| `s1` | snippet | folder `f1` | 5 | abbreviation `;hrs`, body `Open 9 to 5.` |
| `sh1` | folder share | folder `f1`, group `g-fam` | 7 | permission `edit` |

**Steps.**

1. C goes offline.
2. A goes offline.
3. E goes offline.
4. `c1` · C edits `s1`: body `Open 9 to 6.`.
5. `a1` · A edits `s1`: body `Open 10 to 5.`.
6. `e1` · E edits `s1`: body `Open 9 to 5, closed on Sundays.`.
7. C comes back online and syncs. Answer: `s1` `applied`.
8. A comes back online and syncs. Answer: `s1` `applied+conflict`. *The first conflict: a1 replaced
   c1 unseen. It is shown to Ines and Tom.*
9. E comes back online and syncs. Answer: `s1` `applied+conflict`. *e1 replaces a1 unseen: a second
   conflict. A row has one open conflict at a time, and the first one was answered by nobody. The
   new conflict takes over the version the first one offered, c1, with its author as a party, and
   the first is closed as replaced (C3). Without that, Tom's version would be in the history with no
   conflict that names it.*

**Outcome.** The snippet has Jun's content. One conflict is open on it and offers both versions it
replaced, Ines's and Tom's, to all three of them; each can be restored. No edit drops out of reach
because a third one arrived.

| Row | Server | A | C | E |
| --- | --- | --- | --- | --- |
| `m-ines` | live · start | = | = | = |
| `m-tom` | live · start | = | = | = |
| `m-jun` | live · start | = | = | = |
| `f1` | live · start | = | = | = |
| `s1` | live · e1 | = | = | = |
| `sh1` | live · start | = | = | = |

- History (replaced versions the server keeps): `s1` start, c1, a1.
- Conflicts: `s1` edit_edit, winner a1, loser c1, shown to ines and tom, replaced; `s1` edit_edit,
winner e1, loser a1 (and, taken over, c1), shown to jun and ines and tom, open.
- A: shown conflict.
- C: shown conflict.
- E: shown conflict.

#### S58 · The server is restored from a backup taken before a member was removed

Category: server. Covers: restore-membership. Rules: C16, C18, C21, D6, D7.

**Start.** Ines shares f1 with the family. A backup is taken. Then Ines removes Tom from the group,
and Tom's device purges the folder. Then the server's database is lost and restored from the backup.

Devices: A (ines), C (tom). Profiles: `p-ines` (personal, ines), `p-tom` (personal, tom), `g-fam`
(family, ines).

| Row | Kind | Refers to | Rev | Fields |
| --- | --- | --- | --- | --- |
| `m-ines` | membership | group `g-fam`, user `ines` | 2 | role `owner` |
| `m-tom` | membership | group `g-fam`, user `tom` | 6 | role `member` |
| `f1` | folder | profile `p-ines` | 3 | name `Shop replies` |
| `s1` | snippet | folder `f1` | 5 | abbreviation `;hrs`, body `Open 9 to 5.` |
| `sh1` | folder share | folder `f1`, group `g-fam` | 7 | permission `view` |

**Steps.**

1. `k1` · A backup of the server's database is taken.
2. `x1` · A: the user removes the member `m-tom`. Answer: `ok`. *Tom's device purges the folder.*
3. The server's database is restored from the backup `k1`. *The restored database knows nothing of
   the removal: Tom is a member again. Account calls made after the backup are lost with it, and no
   device can push a membership (C16). The server starts a new epoch (C18).*
4. C pulls. Answer: `reset_required:epoch`. *Tom's device resyncs. Rows only the server writes are
   taken as the server has them, so it holds the membership again, and the folder arrives again.*
5. A pulls. Answer: `reset_required:epoch`. *Ines's device resyncs and compares row by row (D7). It
   held the tombstone of m-tom in a revision the server no longer has, and the server shows the
   membership live: a removal made after the backup. The device takes the server's row, tells Ines
   that the server was restored and that she should look at her members, and does not wait for her:
   what took access away is made again.*
6. `x2` · A: the user removes the member `m-tom`. Answer: `ok`. *The device makes the call by
   itself, because its user may still make it (D7). A device never repeats a call that gives access.
   Tom's device purges the folder again.*

**Outcome.** Tom is removed again as soon as a device of someone who may remove him has synced;
until then he had the folder back, which is what a restore costs and what the restore procedure
tells owners. Ines's device says that the server was restored. Tom's device says that the folder is
no longer shared with him.

| Row | Server | A | C |
| --- | --- | --- | --- |
| `m-ines` | live · start | = | absent |
| `m-tom` | deleted · x2 | absent | = |
| `f1` | live · start | = | absent |
| `s1` | live · start | = | absent |
| `sh1` | live · start | = | absent |

- History (replaced versions the server keeps): `m-tom` start.
- Conflicts: none.
- A: shown restored.
- C: shown revoked, shared; permission `f1` none.

#### S59 · After a restore, only one device holds what the backup lacks

Category: server. Covers: restore-single-holder. Rules: C13, C18, D7.

**Start.** Device B is offline. A backup is taken. Device A creates s2 and edits s1, and syncs both.
Then the server's database is lost and restored from the backup. B comes back before A does.

Devices: A (ines), B (ines). Profiles: `p-ines` (personal, ines).

| Row | Kind | Refers to | Rev | Fields |
| --- | --- | --- | --- | --- |
| `f1` | folder | profile `p-ines` | 3 | name `Replies` |
| `s1` | snippet | folder `f1` | 5 | abbreviation `;rty`, body `Thanks for your order.` |

**Steps.**

1. B goes offline.
2. `k1` · A backup of the server's database is taken.
3. `a1` · A creates the snippet `s2` (folder `f1`): abbreviation `;rlate`, body `Your order left us
   late.`. Answer: `s2` `applied`.
4. `a2` · A edits `s1`: body `Thank you for your order.`. Answer: `s1` `applied`.
5. The server's database is restored from the backup `k1`.
6. B comes back online and syncs. Answer: `reset_required:epoch`. *B was offline since before the
   backup. It resyncs and finds every row as it holds it: nothing to send.*
7. A pulls. Answer: `reset_required:epoch`, `s1` `applied`, `s2` `applied`. *A compares row by row
   (D7): the server holds s1 in an older revision and does not hold s2. A sends both again, marked
   as re-pushes with the revisions it holds. Both revisions lie in the span the restore lost. The
   server takes s1 without a conflict, because nobody wrote it in the new epoch, and creates s2: a
   re-push of a lost revision is the one put with a base above 0 that may create a row (C13, C18).*

**Outcome.** The one device that held the changes made after the backup gives them back: s1 has the
content of a2 and s2 exists, on the server and on both devices. Nothing is shown.

| Row | Server | A | B |
| --- | --- | --- | --- |
| `f1` | live · start | = | = |
| `s1` | live · a2 | = | = |
| `s2` | live · a1 | = | = |

- History (replaced versions the server keeps): `s1` start.
- Conflicts: none.
- A: shown nothing.
- B: shown nothing.

#### S60 · A data directory is copied to a second computer and both are used

Category: server. Covers: copied-data-directory. Rules: C3, C25, D14.

**Start.** Ines moves to a new computer with the system's migration tool: B starts as a copy of A's
data directory, with A's device id, feed position and tokens. She keeps using both, and edits s1 on
each while offline.

Devices: A (ines), B (ines, a copy of A's data directory). Profiles: `p-ines` (personal, ines).

| Row | Kind | Refers to | Rev | Fields |
| --- | --- | --- | --- | --- |
| `f1` | folder | profile `p-ines` | 3 | name `Replies` |
| `s1` | snippet | folder `f1` | 5 | abbreviation `;rty`, body `Thanks for your order.` |

**Steps.**

1. A goes offline.
2. B goes offline.
3. `a1` · A edits `s1`: body `Thank you for your order.`.
4. `b1` · B edits `s1`: body `Thanks a lot for your order.`.
5. A comes back online and syncs. Answer: `s1` `applied`. *A pulls with the position both
   installations hold, receives the next one, and pushes.*
6. B comes back online and syncs. Answer: `s1` `applied+conflict`. *B presents the position A used
   before. The server knows it as the one before its latest and must take the call for a repeat
   after a lost answer (C25). B's push is stale and replaces a1. a1 was written under the same
   device id, but not by a change B's op names as its own earlier one, so the conflict is recorded
   all the same (C3).*
7. A pulls. Answer: `error:device_conflict`. *A's position is now neither the server's latest nor
   the one before it: two installations use one device id. A is told so, makes a new device id and
   calls hello, which is refused as well: the session came with the copy and already has a device. A
   signs itself out, keeping its library and its outbox (D14), and asks Ines to sign in again on
   this computer.*
8. `x1` · A: ines signs in. *A new session and a new device id. A resyncs and takes b1.*

**Outcome.** The second installation's edit wins by arriving last, and a conflict offers the first
one back, although both were written under one device id. The server notices the two installations
at the next call of the one that is behind, which signs itself out and comes back as a device of its
own. Nothing is replaced silently.

| Row | Server | A | B |
| --- | --- | --- | --- |
| `f1` | live · start | = | = |
| `s1` | live · b1 | = | = |

- History (replaced versions the server keeps): `s1` start, a1.
- Conflicts: `s1` edit_edit, winner b1, loser a1, shown to ines, open.
- A: shown conflict, signed_out.
- B: shown conflict.

#### S61 · The owner deletes a shared folder while a member holds an unsent change in it

Category: sharing. Covers: shared-folder-deleted. Rules: C6, C21, D6.

**Start.** Ines shares f1 with the family, to change. Tom's device goes offline and he edits s1.
Ines deletes the folder.

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
3. `a1` · A deletes `f1`. On the server the change also writes `s1`, `sh1`. Answer: `f1` `applied`.
   *The delete takes the folder's share with it (C6). For Tom the share was the only way to the
   folder, so its end reaches him as folder_revoked, exactly as when sharing is stopped, and not as
   tombstones of rows he may no longer read (C21). Ines's own devices get the tombstones.*
4. C comes back online and syncs. *C pulls first: the folder is revoked. The unsent change is kept
   for the user and the tree is purged (D6). Nothing is left to push.*

**Outcome.** The folder, its snippet and its share are deleted; Ines can restore them for 30 days,
unshared. Tom's device no longer holds the folder, says that it is no longer shared with him, and
keeps his change in the list of changes that were not synced.

| Row | Server | A | C |
| --- | --- | --- | --- |
| `m-ines` | live · start | = | = |
| `m-tom` | live · start | = | = |
| `f1` | deleted · a1 | = | absent |
| `s1` | deleted · a1 | = | absent |
| `sh1` | deleted · a1 | = | absent |

- History (replaced versions the server keeps): `f1` start; `s1` start; `sh1` start.
- Conflicts: none.
- A: shown nothing.
- C: shown revoked, kept; kept for the user: c1; permission `f1` none.

#### S62 · The owner moves snippets across the edge of a shared folder

Category: sharing. Covers: move-across-share-edge. Rules: C12, C21, D2, D3, D5, D16.

**Start.** Ines shares f1 with the family, to change; f2 is private. Tom, offline, edits s1 in the
shared folder. Ines moves s1 out, into f2, and moves s2, which carries the tag t2, from f2 into the
shared folder.

Devices: A (ines), C (tom). Profiles: `p-ines` (personal, ines), `p-tom` (personal, tom), `g-fam`
(family, ines).

| Row | Kind | Refers to | Rev | Fields |
| --- | --- | --- | --- | --- |
| `m-ines` | membership | group `g-fam`, user `ines` | 2 | role `owner` |
| `m-tom` | membership | group `g-fam`, user `tom` | 6 | role `member` |
| `f1` | folder | profile `p-ines` | 3 | name `Shop replies` |
| `f2` | folder | profile `p-ines` | 4 | name `Private` |
| `s1` | snippet | folder `f1` | 5 | abbreviation `;hrs`, body `Open 9 to 5.` |
| `s2` | snippet | folder `f2` | 8 | abbreviation `;xmas`, body `We are closed from 24 to 26 December.` |
| `t2` | tag | profile `p-ines` | 9 | name `seasonal` |
| `l2` | snippet tag | snippet `s2`, tag `t2` | 10 |  |
| `sh1` | folder share | folder `f1`, group `g-fam` | 7 | permission `edit` |

**Steps.**

1. C goes offline.
2. `c1` · C edits `s1`: body `Open 9 to 6.`.
3. `a1` · A moves `s1` to `f2`. Answer: `s1` `applied`.
4. `a2` · A moves `s2` to `f1`. Answer: `s2` `applied`. *For Tom s2 becomes readable by the move:
   his feed gets the snippet and its link, and the page brings the tag the link names (C21).*
5. C comes back online and syncs. Answer: `s1` `rejected:forbidden`. *C pulls: s1 is delivered as no
   longer there. C has an unsent change to it and leaves it alone (D2). s2 arrives with its link and
   its tag (D3). C's push of s1 is refused, because the snippet is now in a folder Tom cannot write,
   and the answer carries no row, because he cannot read it either. C keeps his version for him and
   removes the row (D5).*

**Outcome.** Tom's device no longer holds the snippet that left the shared folder and keeps his edit
of it in the list of changes that were not synced. It holds the snippet that came in, with its tag.
Ines's devices and the server never see Tom's edit.

| Row | Server | A | C |
| --- | --- | --- | --- |
| `m-ines` | live · start | = | = |
| `m-tom` | live · start | = | = |
| `f1` | live · start | = | = |
| `f2` | live · start | = | absent |
| `s1` | live · a1 (folder `f2`) | = | absent |
| `s2` | live · a2 (folder `f1`) | = | = |
| `t2` | live · start | = | = |
| `l2` | live · start | = | = |
| `sh1` | live · start | = | = |

- History (replaced versions the server keeps): `s1` start; `s2` start.
- Conflicts: none.
- A: shown nothing.
- C: shown kept; kept for the user: c1.

#### S63 · A tag is taken off the only shared snippet that carried it

Category: tags. Covers: tag-leaves-share. Rules: C10, D3, D16.

**Start.** Ines shares f1 with the family. Its snippet s1 carries her tag t1, which Tom's device
therefore holds. Ines takes the tag off the snippet.

Devices: A (ines), C (tom). Profiles: `p-ines` (personal, ines), `p-tom` (personal, tom), `g-fam`
(family, ines).

| Row | Kind | Refers to | Rev | Fields |
| --- | --- | --- | --- | --- |
| `m-ines` | membership | group `g-fam`, user `ines` | 2 | role `owner` |
| `m-tom` | membership | group `g-fam`, user `tom` | 6 | role `member` |
| `f1` | folder | profile `p-ines` | 3 | name `Shop replies` |
| `s1` | snippet | folder `f1` | 5 | abbreviation `;hrs`, body `Open 9 to 5.` |
| `t1` | tag | profile `p-ines` | 8 | name `shop` |
| `l1` | snippet tag | snippet `s1`, tag `t1` | 9 |  |
| `sh1` | folder share | folder `f1`, group `g-fam` | 7 | permission `view` |

**Steps.**

1. `a1` · A deletes `l1`. Answer: `l1` `applied`. *Tom's device gets the link's tombstone. Nothing
   was written to the tag, so no entry tells it that Tom may no longer read the tag. After every
   page that removes or tombstones a link or a snippet of someone else's profile, the store removes
   the tags and attachments of that profile, with their files, that no live link from a snippet it
   holds uses any more (D16).*

**Outcome.** The snippet has no tag. Tom's device no longer holds the tag's name: content whose
access ended does not stay on a device because nothing happened to its own row. Ines keeps the tag.

| Row | Server | A | C |
| --- | --- | --- | --- |
| `m-ines` | live · start | = | = |
| `m-tom` | live · start | = | = |
| `f1` | live · start | = | = |
| `s1` | live · start | = | = |
| `t1` | live · start | = | absent |
| `l1` | deleted · a1 | = | = |
| `sh1` | live · start | = | = |

- History (replaced versions the server keeps): `l1` start.
- Conflicts: none.
- A: shown nothing.
- C: shown nothing.

#### S64 · A tag is put on one snippet on two devices and taken off on one

Category: tags. Covers: duplicate-links. Rules: C10, D3.

**Start.** Offline, both of Ines's devices put the tag t1 on the snippet s1: two link rows for one
pair. Later she takes the tag off on device A.

Devices: A (ines), B (ines). Profiles: `p-ines` (personal, ines).

| Row | Kind | Refers to | Rev | Fields |
| --- | --- | --- | --- | --- |
| `f1` | folder | profile `p-ines` | 3 | name `Replies` |
| `s1` | snippet | folder `f1` | 5 | abbreviation `;rty`, body `Thanks for your order.` |
| `t1` | tag | profile `p-ines` | 4 | name `urgent` |

**Steps.**

1. A goes offline.
2. B goes offline.
3. `a1` · A creates the snippet tag `l1` (snippet `s1`, tag `t1`).
4. `b1` · B creates the snippet tag `l2` (snippet `s1`, tag `t1`).
5. A comes back online and syncs. Answer: `l1` `applied`.
6. B comes back online and syncs. Answer: `l2` `applied`. *Two link rows for the same pair are
   accepted; devices show the tag once (C10).*
7. `a2` · A deletes `l1`. Answer: `l1` `applied`.
8. `a3` · A deletes `l2`. Answer: `l2` `applied`. *One action of the user: taking a tag off deletes
   every live link of the pair in the same local change, so that no second link keeps the tag on
   (C10).*

**Outcome.** The snippet carries no tag on either device: both link rows are deleted, although only
one of them was made on the device that took the tag off.

| Row | Server | A | B |
| --- | --- | --- | --- |
| `f1` | live · start | = | = |
| `s1` | live · start | = | = |
| `t1` | live · start | = | = |
| `l1` | deleted · a2 | = | = |
| `l2` | deleted · a3 | = | = |

- History (replaced versions the server keeps): `l1` a1; `l2` b1.
- Conflicts: none.
- A: shown nothing.
- B: shown nothing.

#### S65 · A folder and a sub-folder are shared separately and the outer share ends

Category: sharing. Covers: nested-shares. Rules: C21, D6.

**Start.** Ines shares f1 with the family, to change, and its sub-folder f1b too, with a share of
its own. Tom, offline, edits s1 in f1 and s2 in f1b. Ines stops sharing f1.

Devices: A (ines), C (tom). Profiles: `p-ines` (personal, ines), `p-tom` (personal, tom), `g-fam`
(family, ines).

| Row | Kind | Refers to | Rev | Fields |
| --- | --- | --- | --- | --- |
| `m-ines` | membership | group `g-fam`, user `ines` | 2 | role `owner` |
| `m-tom` | membership | group `g-fam`, user `tom` | 6 | role `member` |
| `f1` | folder | profile `p-ines` | 3 | name `Shop replies` |
| `f1b` | folder | profile `p-ines`, parent `f1` | 4 | name `Returns` |
| `s1` | snippet | folder `f1` | 5 | abbreviation `;hrs`, body `Open 9 to 5.` |
| `s2` | snippet | folder `f1b` | 8 | abbreviation `;rret`, body `Return label attached.` |
| `sh1` | folder share | folder `f1`, group `g-fam` | 7 | permission `edit` |
| `sh2` | folder share | folder `f1b`, group `g-fam` | 9 | permission `edit` |

**Steps.**

1. C goes offline.
2. `c1` · C edits `s2`: body `A return label is attached.`.
3. `c2` · C edits `s1`: body `Open 9 to 6.`.
4. `a1` · A deletes `sh1`. Answer: `sh1` `applied`. *Tom loses f1 and keeps f1b through its own
   share. His feed gets one entry: folder_revoked for f1, naming f1b as the part that stays (C21).*
5. C comes back online and syncs. Answer: `s2` `applied`. *C purges f1 except f1b (D6): the unsent
   change to s1 is kept for the user; the unsent change to s2 lies in the part Tom may still change,
   stays in the outbox and is pushed. f1b is shown at the top level from now on.*

**Outcome.** Tom's device holds the sub-folder and nothing else of the folder above it. His edit
inside the sub-folder is synced like any other; his edit outside it is kept in the list of changes
that were not synced. A change he was still allowed to make is not refused because a wider share
ended.

| Row | Server | A | C |
| --- | --- | --- | --- |
| `m-ines` | live · start | = | = |
| `m-tom` | live · start | = | = |
| `f1` | live · start | = | absent |
| `f1b` | live · start | = | = |
| `s1` | live · start | = | absent |
| `s2` | live · c1 | = | = |
| `sh1` | deleted · a1 | = | absent |
| `sh2` | live · start | = | = |

- History (replaced versions the server keeps): `s2` start; `sh1` start.
- Conflicts: none.
- A: shown nothing.
- C: shown revoked, kept; kept for the user: c2; permission `f1` none, `f1b` edit.

#### S66 · A device is revoked while it holds unsent changes

Category: server. Covers: device-revoked-with-pending. Rules: C25, D6, D14.

**Start.** Ines's second computer, B, is offline and holds two unsent changes: one to her own
snippet s1 and one to ts1 in a folder Tom shares with the family. She revokes B from her first
computer.

Devices: A (ines), B (ines), C (tom). Profiles: `p-ines` (personal, ines), `p-tom` (personal, tom),
`g-fam` (family, ines).

| Row | Kind | Refers to | Rev | Fields |
| --- | --- | --- | --- | --- |
| `m-ines` | membership | group `g-fam`, user `ines` | 2 | role `owner` |
| `m-tom` | membership | group `g-fam`, user `tom` | 6 | role `member` |
| `f1` | folder | profile `p-ines` | 3 | name `Replies` |
| `s1` | snippet | folder `f1` | 5 | abbreviation `;rty`, body `Thanks for your order.` |
| `tf1` | folder | profile `p-tom` | 7 | name `Shop` |
| `ts1` | snippet | folder `tf1` | 8 | abbreviation `;hrs`, body `Open 9 to 5.` |
| `sh1` | folder share | folder `tf1`, group `g-fam` | 9 | permission `edit` |

**Steps.**

1. B goes offline.
2. `b1` · B edits `s1`: body `Thank you for your order.`.
3. `b2` · B edits `ts1`: body `Open 9 to 6.`.
4. A: the user revokes the device B. Answer: `ok`. *B's device row is marked revoked. From this
   commit on every function and every policy refuses B's session, also while its access token has
   not expired, and the session cannot register another device (C25).*
5. B comes back online and syncs. Answer: `device_revoked`. *B is refused at its first call and
   signs itself out (D14). What belongs to other people's profiles is removed; the unsent change to
   Tom's snippet is kept for the user first (D6). Ines's own library stays and keeps working, with
   the unsent change to s1 in its outbox: if she signs in on B again, it is pushed (S49).*

**Outcome.** Nothing from B reaches the server after the revocation. B is a computer without an
account again: it keeps Ines's own library, with her unsent edit still waiting, holds nothing of
Tom's folder, and keeps her edit of his snippet in the list of changes that were not synced. It says
that it was signed out.

| Row | Server | A | B | C |
| --- | --- | --- | --- | --- |
| `m-ines` | live · start | = | absent | = |
| `m-tom` | live · start | = | absent | = |
| `f1` | live · start | = | = | absent |
| `s1` | live · start | = | live · b1 | absent |
| `tf1` | live · start | = | absent | = |
| `ts1` | live · start | = | absent | = |
| `sh1` | live · start | = | absent | = |

- History (replaced versions the server keeps): none.
- Conflicts: none.
- A: shown nothing.
- B: shown signed_out, kept; kept for the user: b2; 1 change waiting in the outbox.
- C: shown nothing.

#### S67 · A share ends and begins again inside one page of the feed

Category: sharing. Covers: share-ends-and-begins. Rules: C11, C21, D6, D11.

**Start.** Ines shares f1 with the family, to change. Tom's device is offline and he edits s1. Ines
stops sharing the folder and, a moment later, shares it again. Tom's device reads both changes in
one page.

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
3. `a1` · A deletes `sh1`. Answer: `sh1` `applied`.
4. `a2` · A creates the folder share `sh1` (folder `f1`, group `g-fam`): permission `edit`. Answer:
   `sh1` `applied`. *Sharing again writes the same row, live, on top of its tombstone (C11). Tom's
   access begins anew, with a new time.*
5. C comes back online and syncs. *One page holds folder_revoked and then folder_visible for the
   same folder. C handles visibility entries in the order of the feed: it purges the tree, keeping
   the unsent change for the user, and then fetches the tree again; the position is stored only once
   both are done (D6, D11). Handled the other way round, the folder would be fetched and then
   purged, and be missing from the device.*

**Outcome.** Tom's device holds the folder, as the server has it. His edit was made under the access
that ended: it is kept in the list of changes that were not synced, where he can make it again. The
device says that the folder was unshared and shared.

| Row | Server | A | C |
| --- | --- | --- | --- |
| `m-ines` | live · start | = | = |
| `m-tom` | live · start | = | = |
| `f1` | live · start | = | = |
| `s1` | live · start | = | = |
| `sh1` | live · a2 | = | = |

- History (replaced versions the server keeps): `sh1` start, a1.
- Conflicts: none.
- A: shown nothing.
- C: shown revoked, kept, shared; kept for the user: c1; permission `f1` edit.

<!-- scenarios:end -->

## 13. Open questions for the owner

Each is decided one way in this document so that the design is complete; each is the owner's to
change, and the cost of the other answer is stated. Questions 10 and 11 are new since the review
of the design, and questions 1, 3, 4, 5, 6 and 7 say more than they did.

| # | Question | Recommendation | If decided otherwise |
| --- | --- | --- | --- |
| 1 | **Does signing in sync the whole personal library?** Flow 6 of the UX specification tells a person who signs in to share one folder: "Your other folders stay on this computer until you choose to sync or share them." Under this design signing in adopts the library and syncs all of it (D9): every snippet on the computer is then stored on the server, where its operator can read it (question 2). Both cannot be true | Sync everything in version 1, and change that sentence to say all of it: "Signing in stores your snippets on Iamb's server and keeps them in step between your computers. Only folders you share can be seen by other people." It is what the Phase 4 gate tests (two devices of one account), and it is simple enough to prove free of lost edits | A switch per device, "Keep my own snippets on this computer only", is one gate and no server change: the device neither adopts nor pushes its personal profile and still receives shared folders; it cannot share a folder of its own while the switch is on. Sharing single folders while the rest stays local, as the sentence promises, needs a sync scope per folder with its own rules for tags, attachments and moves between synced and local folders; that is a feature of its own and should not be in version 1 |
| 2 | **Should the server be unable to read snippets (end-to-end encryption)?** | Not in version 1. Say plainly in the privacy text and at sign-in (question 1) that synced snippets are stored on the server and readable by its operator, keep the server from interpreting content (C9) so that encryption can be added per profile later, and decide again before team plans are sold | End-to-end encryption brings a recovery key the user must keep (there is no password), a key per shared folder handed over by a member's device (the 10-second target then depends on one being online), re-keying on removal, and no server-side check of attachment hashes; roughly a phase of work, and it changes tasks 4.2, 4.4, 4.5 and 4.6 |
| 3 | **Where is the server hosted, and on which hosting plan?** The plan decides three things. *Backups*: how much a restore loses. Devices give back the rows (C18), but what only the server writes is lost for good: memberships, invitations and groups changed since the backup come back as the backup had them, a removed member is a member again until a device of an owner or admin has synced (S58), and an account deleted since exists again until the operator deletes it again from the log. *Connections*: how many devices can hold the live channel at once: 200 on the free plan, 500 on the first paid one, 10,000 above; devices beyond that poll every 2 seconds while in use (§6.6). *The daily job*: Supabase Cron must be part of the plan (§11.6) | One region in the European Union, and say so in the privacy text; the 5 and 10 second targets leave room for a distant region. A paid plan from the first real user, with daily backups, and point-in-time recovery before paying customers depend on it, because every hour of lost account calls is an hour of access that may have come back. Move to the plan above when more than about 400 devices are online at once | Another region changes nothing in the design; more than one region is not supported by it. Without backups a lost database is rebuilt only from what devices hold, and every group must be made again |
| 4 | **Plan limits, rate limits, and whose limit a member uses.** They decide cost and pricing: devices per account, snippets, shared folders, groups owned, members per group, storage, days of history; the rate limits of §6.8; `cascade_max` and the size of a file. And: a snippet a member creates in a shared folder counts against the plan of the folder's owner (§5.10), so a member who may change a folder can use up the owner's limit | Start with one default plan per scope and generous values while there is no billing: 5 devices, 10,000 snippets, 10 shared folders, 3 groups owned, 10 members per group, 500 MB of files, 25 MiB per file as in the local store, 90 days of history; the rate limits as in §6.8. All are rows in `plan` and settings of the server, changed without a release. Count rows against the owner of the profile they are in: the library is the owner's, the owner chose who may change it, and no other rule can be explained in one sentence | Any values work; only `history_days` above 90 needs more storage, and a limit lowered later never deletes rows (§5.10). Counting a member's rows against the member's own plan needs an author per row in the limit's count and an answer to what happens to the rows when the member leaves |
| 5 | **How long is replaced content kept?** Tombstones for 30 days is fixed by the architecture. History for 90 days and open conflicts for 90 days are chosen here, and an image nothing uses is kept for the same 90 days, so that a restored version still has its images (§5.12) | Confirm 90 for all three. Longer history can become a plan feature (`history_days`) | Shorter history means a conflict can lose its other version sooner; it must stay above the 30 days of the tombstones, and the images' time must not be shorter than the history's |
| 6 | **Deleting an account: at once or after a waiting time?** | At once. Every device keeps the whole library locally, so the user loses nothing they have not asked to lose, and "delete" means delete. Backups age out with the hosting provider's retention, which the privacy text must name; if the server is restored from a backup in that time, the operator deletes the account again before the server is opened (C18) | A waiting time of 7 or 30 days needs a "deletion pending" state on the account and a way back in |
| 7 | **Who may share a folder with a group?** A shared folder is on every member's device within ten seconds, and its single-line snippets expand there at once: there is no step at which a recipient accepts a folder and no way to switch one off. A shared snippet cannot take over an abbreviation the recipient's own snippets use (D13) and cannot press keys or include other snippets without review (D10), but for any abbreviation the recipient does not use yet, whoever may share can put expanding text on every member's computer | In a family, every member, as flow 6 shows: the people know each other and the folder is meant to work at once. In a team, the owner and admins only (one check in C11, by the kind of group). The design is written for this | (a) "Every member" in teams too: no check, and any colleague can put expanding text on everyone's computer. (b) "New shared folders start switched off for each recipient": a device-local switch per shared folder and a step in flow 6 before "the other 17 work now"; it protects families as well, and the 10-second target then measures that the folder appears, not that it works. (c) A setting per team that lets members share is one more column and one more account call, and can be added later without changing anything else |
| 8 | **Invitation emails** need a mail-sending service and a sender address under Iamb's own domain; the sign-in emails need the same | Use one service for both, set up in Supabase's authentication settings, before task 4.5 is tested with real addresses | — |
| 9 | **May someone run their own server?** The server's code is AGPL like the rest | Keep `server/` complete enough to be run by others, and let the address of the server be a value in a configuration file without interface in version 1 | A fixed address is simpler to support and to sign; it can be opened later |
| 10 | **Should a `{snippet}` reference written by someone else be held for review?** ADR-0022 holds key presses, waits, clipboard reads and line breaks. A member who may change a shared folder can write one line that includes, by its abbreviation, a snippet of the reader's own private library; the reference resolves on the reader's device and types that private text, with its key presses, into whatever the reader is writing to. It is also a wanted use: a shared reply that ends in each member's own signature | Hold it: a change by someone else that adds or changes a `{snippet}` reference waits for review like a key press (D10), and the review dialog shows what each reference resolves to on this device. One approval per content, and the shared signature works from then on. This adds a fifth item to ADR-0022's list and is recorded in ADR-0028 | (a) Leave it as it is and list it in §11.3 under "what is left": no review step, and a member can make the reader type their own private snippets. (b) Resolve references in content by others only inside the same shared folder: no review step and no leak, but the shared signature no longer works |
| 11 | **Can ownership of a group be given to someone who has not agreed?** The owner of a group cannot leave it, and its plan is paid by its owner | No: the owner offers, the member accepts (`transfer_ownership`, `accept_ownership`; C16, S34), and the member's own plan must allow one more group. The design is written for this | An immediate transfer is one call less and one dialog less; a member can then be made to own, and later to pay for, a group, and has to hand it on to be able to leave |

## 14. What each task takes from here

| Task | Builds | From |
| --- | --- | --- |
| 4.2 Server schema and policies | The schemas, tables, constraints and indexes; the gate helpers and the grants on them; `refresh_access` and its slow reference; the lock order of C24 in every writing function; the feed with its ordering rule; the functions of `api` with the rules C1 to C25, their codes and their time-outs; the bounded cascade; the read policies; the storage and live-channel policies; the daily job; `begin_epoch`, the `maintenance` switch and the restore procedure as a script, a page and a test; the default plans; and the verification steps §11.6 gives it, before anything is built on them | §4, §5, §6.1 to §6.5, §6.11, §8, §11.5, §11.6 |
| 4.3 Security review | Tests of G1 to G23 as each kind of user, among them the two-connection tests of G21; the queries of §4.5, also for a known file hash and for a member's own rows after removal; the required authentication settings of §11.3, with the test per identity provider; the log check | §4.5, §5.13, §11 |
| 4.4 Sync client | `iamb-sync`; the store's migration 3 and the Sync messages; D1 to D17; the effective library over all profiles with D13; the scenario replay, which derives every outcome from the rules and compares it field by field; and the simulation, which must include an agent that stops between a commit and its answer, a second change before the answer, a restore of the server and a copied data directory, with the invariants I1 to I7 and the eleven places of §1 as its definition of a lost edit; the 5-second measurement | §1, §6, §7, §9, §12 |
| 4.5 Account interface | Sign-in with the sentence of question 1 and the display name; groups, invitations, members with their addresses for owners and admins, roles, the offer of ownership, on the account endpoint; the share dialog (with the count of snippets that include snippets outside the folder, and the sentence about copies that stay with removed members); conflicts with every version on offer ("Show the other version", "Restore", "Keep"); the list of changes that were not synced, refused deletes included; blocked rows with their reason; devices and "sign out everywhere"; "this computer was signed out" and "the server was restored"; the review dialog with author, time, difference and what each `{snippet}` resolves to, and "review first" where a copy is refused; the 10-second measurement | §4, §5.6, §5.14, §7.2, D5, D10, D14, §11.3, §12 ("Shown") |
| 4.6 Media sync | Upload through `attachment_upload` rows and the `attachment-commit` edge function with the server-side hash check; download; the size limit; the expiry of uploads; `unlinked_at` and the removal of files nothing has used for 90 days; the link a device adds for an image a restored body names (D16); the measurement of §11.6 | §5.9, §5.12, §5.13, §6.11, C19, D16 |
| 4.7 Billing readiness | `plan`, `subscription`, the flags; `entitlements()`; the enforcement points, with rows counted per profile and shared folders counted as folders; the daily retry of rows blocked by a limit; the values of question 4 | §5.10, §6.12, C11, C14, D5 |

## 15. What changes in the architecture and the decision records

The architecture's frame holds. Three new records add to it, and the architecture document
points to them where they apply.

| Record | Adds or amends | Why |
| --- | --- | --- |
| [ADR-0027](decisions/0027-sync-protocol-and-conflict-rules.md) | Adds to ADR-0016: the server's revision counter and clock as the only order; writes through functions with receipts, one profile at a time; deletes as writes, with the folder cascade answered row by row and the restored folder; unsent changes are never overwritten by a pull, and a skipped row is fetched later; refused changes are kept on the device and blocked ones stay local; one session, one device, one position; resync and epochs | ADR-0016 fixed the frame and left the rules to this task |
| [ADR-0028](decisions/0028-data-model-for-sync.md) | Amends ADR-0014: abbreviations are not unique on the server, and the store enforces uniqueness for local edits only; precedence by the author of the content and the server's order; a share's id is derived; profiles and memberships are written by the server only, ownership by offer; access entries. Amends ADR-0022: the server records the author of a snippet's content against the version a write was based on, a device approves content, not a revision, a held snippet cannot be copied, and a `{snippet}` reference by someone else is held | Uniqueness per user's view cannot be a server constraint; sharing must be safe against late changes; a review that follows the row's last writer can be bypassed by an innocent edit |
| [ADR-0029](decisions/0029-server-readable-content.md) | New: synced content is readable by the server in version 1, the server never interprets it, and end-to-end encryption stays possible. Proposed, with open question 2 | Architecture §8 did not say; it has to be said before anything is stored |

In `architecture.md`: §4.4 (the store enforces uniqueness for local edits; precedence by author
and by the server's order), §6.2 and §6.4 (the messages of §7.2), §7 (the device-local tables of
§7.1, the server-set fields, who writes profiles and memberships), §8 rule 10 (review follows
the author of the content, and covers `{snippet}` references by others, open question 10), §9 (a
summary of the rules and a pointer here) and §11 (this document's name). Nothing in §8 rule 1
changes.
