# Iamb sync design

Status: proposed for the owner's approval; revised after its independent review
([`reviews/4.1-sync-design-review.md`](reviews/4.1-sync-design-review.md)), again after the
follow-up review ([`reviews/4.1-sync-design-followup.md`](reviews/4.1-sync-design-followup.md)) and
a third time after the close-out review
([`reviews/4.1-sync-design-closeout.md`](reviews/4.1-sync-design-closeout.md)); the last section of
each says where its findings went · Task 4.1 · Builds on [`architecture.md`](architecture.md) §6,
§7, §8 and §9 and on ADR-0014, ADR-0016, ADR-0020 and
ADR-0022 · Decisions recorded in [ADR-0027](decisions/0027-sync-protocol-and-conflict-rules.md),
[ADR-0028](decisions/0028-data-model-for-sync.md) and
[ADR-0029](decisions/0029-server-readable-content.md) · Scenarios:
[`spec/sync/scenarios.toml`](../spec/sync/scenarios.toml), checked by `cargo test -p iamb-testkit
--test sync_scenarios`

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
| Content whose access ended disappears from devices | at the next sync; at the latest at the daily access check, which a device keeps running even when it may not sync. A device that was signed out without the server's word sets shared content aside, unused and unseen, and removes it at the sync after its next sign-in or after 30 days | D6, D7, D12, D14, D16 |
| Editing works offline | every local change is queued in the transaction that makes it | §7, D1 |
| The service stays without network code | only `iamb-sync` talks to the server; it never sees keys | §11.2 |
| Typed text, form values and usage never reach the server | not part of any synced row | §11.2 |

**What "no lost edit" means.** A local change is one action of a user on one row on one device: a
new row, a save, a move, a delete, a restore. A folder's delete is one change, of the folder; what
it takes with it is decided by the server (C6). When every device has been online, and signed in,
long enough to finish syncing, each change is in one of these places:

| | Place | Rule |
| --- | --- | --- |
| 1 | It is the row's current version on the server | C1, C2 |
| 2 | It is in the server's history because a later write replaced it knowingly: that write was based on it or on a later version | C2 |
| 3 | It is in the server's history because a later write replaced it without having seen it, and a conflict offers it back to the people concerned: as the version that conflict's write replaced, or as one a later conflict on the row took over | C3 |
| 4 | The server refused it, or the folder it was made in left the device before it was sent, or before the device could learn whether the server still held it (a sign-out it could not confirm, with no sign-in in the 30 days after it): the device that made it keeps it in the list of changes that were not synced, with its content (a refused delete as a line that says what was not deleted) | D5, D6, D14 |
| 5 | The server cannot store it yet (too large, over a plan's limit, a request that keeps failing): the row stays on the device that made it, works there, and is marked as not synced | D5, D17 |
| 6 | It was a position in a list (the order of folders), which is arrangement and not content: the last write wins without a record | C3 |
| 7 | A later change to the same row on the same device took its place before it was sent; that later change then stands for it. A row created and deleted again before any of its entries was handed to the agent ends here too | D1 |
| 8 | The server already held exactly that (`unchanged`) | C4, C5 |
| 9 | The server made it differently and the device says so: a move that would have broken the folder tree (`adjusted`), a change to a share made without the share's latest state (`superseded`) | C8, C11 |
| 10 | At the first sign-in the row was merged into one of the account that is identical, or a folder into one of the same name and place: its content is there | D9 |
| 11 | The user discarded it: dismissed from the list of place 4, or chose "discard" when signing out | D5, D14 |
| 12 | The server was restored from a backup taken before the change, and at that moment neither a device that may write the row nor a device of the user who made the change still held it as the row's version. A later change to the row had taken its place on each of them, and that later change comes back and stands for it; or the row had left each of them on the server's word (an access that ended, a sign-out the server confirmed, D14); or the change was a delete more than 30 days old, whose tombstone the devices had purged (D8), and the row is back as the backup had it. The history and the conflicts of the time a restore loses do not come back | C18, D7, D14 |

Never: nowhere. **A restore of the server loses nothing that a device still holds, signed in or
not.** What a device holds as a row's version, in a revision the restore lost, is sent again
whatever the server holds by then, when the device may write the row or its own user wrote that
version. It ends like any other write: as the row (place 1), in the history with a conflict that
offers it (place 3), or refused and kept (place 4). A restore also puts back the sessions of the
backup, so it can end a device's session before the device has learned of the new epoch. A device
that is signed out without the server's word therefore removes nothing: it sets aside what it holds
of other people's folders, and gives back what was lost when its account has signed in on it again
(D14, S78). Place 12 is only for a change that no such device held any more: it had already been
replaced, or had left the device on the server's word, and what is gone is the record of it (C18,
D7). After a restore a change can be in two places at once, given back by one device and kept as
refused on another; the simulation counts the first of them in the order of the table.

The simulation of task 4.4 records every local change with its device, row and content and, at the
end of a run, puts each into one of these places from what the server and the devices hold: the
server's rows and history, the conflicts with all the versions they offer, each device's list of
kept changes, its blocked rows and its outbox, and, for a run in which the server is restored, the
version of each row that each device held at that moment. A change it can put nowhere fails the run.
It checks this together with the other invariants:

| | Invariant |
| --- | --- |
| I1 | **Convergence.** When all devices are online and idle, every device of a user holds exactly the live rows the server shows that user, in the same revisions, and no row the server does not show that user; rows blocked on that device (place 5) are the exception. For a row the server holds as a tombstone and shows that user, the device holds the same tombstone or nothing. A tombstone the server does not show the user, because the row was deleted before the user's access to it began (§5.4), is a row the server does not show that user: the device holds nothing of it, also not the live copy it may have held under an earlier access (D3) |
| I2 | **No lost edit**, as defined above |
| I3 | **Revisions only grow.** No device replaces a row by an older revision of it. The one exception is the reconcile after the server was restored from a backup: rows the backup held, rows only the server writes, and rows the device may not write and whose version its user did not write, are taken as the server has them; only what the restore lost is held back and sent again (D7) |
| I4 | **No access leak.** After a device has synced past the end of an access, it holds none of the rows that access gave it, and no tag, attachment or file that only those rows made readable (D6, D16). A device that is signed out cannot sync; one that was signed out without the server's word holds such rows set aside, where nothing uses or shows them, until the sync after its next sign-in and for 30 days at most (D14) |
| I5 | **Exactly once.** A change that is sent several times makes one revision |
| I6 | **Same answer everywhere.** Which snippet holds an abbreviation, and what a user may do in a folder, is the same on all devices of that user that have finished syncing. (Which snippets wait for review is per device by design, D10) |
| I7 | **A valid tree, always.** After every commit the server holds no folder cycle, no live folder below the fourth level (a deleted folder can lie deeper after a folder above it was moved, and is held to four levels when it comes back, C8), no live row under a deleted folder, and no access entry without the share or membership that gives it (C24) |

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
| How is a device told from a copy of itself? | Every position a device holds was issued by the server, which remembers the one it issued last and the one that was presented for it; an installation that presents neither is a second one with the same device id, and comes back as a device of its own. A position presented again after a lost answer is accepted every time (C25) | A data directory copied to another computer would otherwise be one device to the server, and a write is never in conflict with the same device's own earlier one |
| What does a restore of the server from a backup cost? | A new epoch, begun by the operator and never on a device's word. Each profile's row records the revision of the profile's last write, so the restored database itself says which revisions it holds; a device sends again every row it holds in a later revision, whatever the server holds by then. What took access away comes first: the operator's script makes those calls again from the function log before the server opens, and each device makes again those of its own list before it gives anything back. A device whose session the restore ended keeps what it holds until its account has signed in again. The history and the conflicts of the lost time are gone (C18, D7, D14) | A revision counter is not a moment in time, and a row written after the restore is not newer than an edit the restore lost: without the first a row could be taken for purged, without the second an acknowledged edit could be replaced unseen (scenarios S68, S69). A restore puts back sessions and memberships as well as rows: without the third a device would remove what only it holds, and without the order a folder would reach a person who had been removed (S78, S79) |
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
tool, by a restored backup of the home folder. Two installations with one id are told apart by the
position each holds: the server issues every position and knows the last one it issued and the one
that was presented for it (C25). The installation that presents neither signs itself out and comes
back with an id of its own.

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
| Any other sub-folder | `edit` on the parent | Name, defaults, position, moving within the access root: `edit` on it and on the new parent. Anything else: `manage` | `edit` on its parent; when the root of a live share lies below it, `manage` on that root as well (C6) |
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
| The links between those snippets and tags, and the tag rows those links name (name and colour), while the tag is live | A tag is visible with the snippets that carry it (architecture §7) |
| The links between those snippets and attachments, the attachment rows they name while they are live, and the stored files of exactly those attachments | A rich snippet is useless without its images |
| The share row itself, and other shares of those folders with groups the member is in | So that the device can show "Shared by Ines · you can use" |
| The owner's personal profile row (id, kind, the fixed name, the owner's id) and the owner's display name | A folder names its profile |

Not readable: the owner's other folders and their snippets; tags no visible snippet carries; links
from other snippets, even to a visible tag; attachments no visible snippet uses, and their files,
also when the member knows a file's hash (§5.13); the history of any row (§5.6); the owner's
other shares, devices, entitlements; and, as always, anything that never leaves a device (§11.2).
A member who may change the folder can also read the tags and attachment rows they wrote themselves
there, for as long as they may change it; that gives them no file they did not upload. An attachment
row counts for a file only when it is *backed*: its creator could read the file when they made the
row, or has uploaded those bytes since (C19). A link from a shared snippet to a row that is not
backed makes no file readable, for anyone (§5.13). Two more things a member can do give no backed
row either: a body that names a file's hash is text, and no device adds a link on its word (D16,
S81); and a member's row that someone else's device gives back after a restore of the server has no
creator, and is not backed (C19, S84).
A snippet in `F` that includes a snippet outside `F` with `{snippet}` does not resolve for the
members (`E_SNIPPET_NOT_FOUND`); the share dialog should say how many such snippets the folder
has, which the service can count from the snapshot.

A folder below `F` whose parent a user cannot read is delivered with an empty parent and shown at
the top level (architecture §7). That happens only for a sub-folder shared by itself.

**A share carries what is in the folder when it begins, and what comes after.** A deleted row
stays on the server as a tombstone for 30 days, with the content it had (§5.12). A tombstone is
read by who deleted the row and by those who could read the row before it was deleted, and by
nobody else: a snippet, a sub-folder, a link or a tag that was deleted before `F` was shared, or
before a member joined the group, does not reach that member, through no read and not as an id
(§5.4, §5.13). For them it is a row that does not exist, and no write of theirs brings it back
(C5, C7, C13). If someone who can read it restores it, it is a live row again and reaches every
reader of the folder as a new revision. What is true of versions a write replaced (§5.6,
ADR-0031) is true of what a delete left behind. A deleted tag or attachment row of the owner's is
not carried at all: a link gives its row while the row is live, so the name of a deleted tag does
not stay on a member's device, and does not reach one that comes later, because a snippet still
has a link to it (C10). (S90, S91, S92)

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
| Files under `P/` in storage | Exactly the verified files of backed attachments linked from those snippets; a request for any other file, also one whose hash `T` knows and has written into an attachment row and a link of their own, answers as for a file that does not exist |
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
  owner, with a fixed `search_path` in which the caller's temporary schema comes last
  (`pg_catalog, pg_temp`); they take the caller from the token (`auth.uid()`, the
  session id claim) and never from an argument, and check §4 themselves. What a function collects
  during a call (the feed entries it will write, the rows it wrote) is kept in tables of
  `iamb_private` keyed by the transaction, never in a temporary table, which would live in the
  caller's session; and every `UPDATE` and `DELETE` has a `WHERE`, because the platform refuses one
  without in the sessions of the REST interface. That check is the only
  one they are under: on Supabase the owner of the tables is not subject to row-level security,
  "forced" or not, so the policies protect reads by devices and nothing a definer function does.
- **Reading functions** (`pull`, `rows`, `fetch_folder`, `snapshot`, `visible_folders`,
  `access`, `users`, `members`, `conflicts`, `get_conflict`) are `security invoker`: each calls
  `iamb_private.enter` first and then returns rows through queries under the caller's policies,
  so a mistake in the feed cannot show a row its reader may not read. Two of them need one thing
  each that no policy can give, and get it from a reader helper (below): `members` the addresses,
  `get_conflict` the history rows of the versions on offer.
- **Gate helpers.** A policy cannot read the table it protects, and an invoker function cannot
  write a counter its caller may not write. These `security definer` functions of `iamb_private`
  do that for them. `authenticated` may execute exactly these and the two reader helpers after
  them; each takes the caller from the token, returns nothing about anyone else, and writes
  nothing but the caller's own device row and rate counters:

  | Helper | Returns | Used by |
  | --- | --- | --- |
  | `session_ok()` | Whether the token's session still exists in `auth.sessions`, was opened by one of Iamb's ways of signing in (the token's `amr` claim against `server_state.sign_in_methods`, C25) and has a device row that is not revoked | Every policy |
  | `member_of(profile)`, `role_in(profile)` | Whether the caller has a live membership in the group; their role | The policies on `membership`, `profile`, `folder_share`, `tag`, `attachment`, `invitation`, `user_profile` |
  | `member_before(profile, rev)` | Whether the caller has a live membership in the group that began before that revision | The policies on `tag` and `attachment`, for a deleted row of a group (§5.13) |
  | `shares_group_with(user)` | Whether the caller and that account have a live membership in one group | The policy on `user_profile` |
  | `own_address()` | The caller's confirmed email address, or null when it is not confirmed (§5.8) | The policy on `invitation` |
  | `enter(device, protocol, call)` | An error code or null, and the current epoch, which every answer names, a refusal included (§6.1, D14). It checks the session and the way it was opened (`session_refused`, C25) and the device row (C25; in a function that writes or issues a position it takes the row's lock first and checks the row afterwards, C24), compares the protocol with the server's range (C23), counts the call in `rate_bucket` (§6.8) and notes `device.last_seen_at` (at most once an hour). It never changes `server_state`, `epoch`, a feed or another device's row: a device that reports more than the server holds is answered `server_behind`, and a line goes to the function log (C18) | Every function of `api`, as its first statement |
  | `position(device, presented, advance)` | Whether the presented position is one of the two the server keeps for the device, and, with `advance`, a new position; `device_conflict` otherwise. It writes the two position columns of the caller's device row (C25) | `pull`, `snapshot`, `push` |

  The two **reader helpers** are `security definer` as well, write nothing, and answer nothing to a
  session `session_ok()` refuses. They are the only helpers that return something about other
  people, each to the callers §4 names and to nobody else:

  | Helper | Returns | Used by |
  | --- | --- | --- |
  | `member_addresses(group)` | The email addresses of the group's live members, when the caller is the group's owner or an admin (§4.1); no row otherwise | `members` |
  | `conflict_history(conflict)` | The history rows of the versions the conflict offers to the caller: the caller is a party, can read the row now, and wrote the version or could read it while it was the row (C20, G5); no row otherwise | `get_conflict` |

  Everything else in `iamb_private` (`refresh_access`, `begin_epoch`, `replay_call`, the purge, the
  feed writer) is executable by the tables' owner only and is reached only from inside a definer
  function, the daily job or the operator's restore script (C18). One function there is executed by
  the authentication service's own role and by nobody else: `token_hook`, which that service calls
  before it issues a token, where the project has such a hook, and which refuses a token asked for
  with a password (C25, §11.6). It reads the method the token was asked with and no table.
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
  `updated_at bigint`, `updated_by uuid`, `rev bigint`, `created_rev bigint`,
  `deleted_at bigint` (null while live), `updated_device uuid` (the device that wrote the revision;
  null for a write the server made; never sent to devices of other users), `updated_op uuid` (the
  pushed change that wrote the revision; for a row a folder's delete took along, the folder's
  change; null for a write no push made; never sent) and `lost_rev bigint` (null, or the highest
  lost revision the row has taken in again after a restore of the server; it stays when the row is
  written again in the ordinary way, C18; never sent).
- **Revisions.** `rev` comes from one sequence, `iamb.rev_seq`, taken after the profile is
  locked, so the revisions of a row only grow, also across a delete, a purge and a restore of
  the row, and the revisions of one profile grow in the order of its commits. The sequence has no
  cache (`CACHE 1`): a value fetched later is higher, whichever connection fetches it, and the
  order above depends on that. `created_rev` is
  the revision the server gave the row when it first accepted it; it never changes and is what
  orders rows that compete for an abbreviation (D13). `updated_at` is the server's clock at the
  write. `created_at` is what the creating device sent, but never later than the server's
  clock; it never changes afterwards and is used for display only. `updated_by` is the caller. Every
  transaction that holds a profile's lock records the last revision it drew on that profile's row
  (`head_rev`, §5.2), so a profile's writes commit in the order of their revisions, and the
  revisions of a profile at or below its `head_rev` are exactly those the database holds (C18).
- **Content columns** (`abbreviation`, `label`, `body`, names, `file_name`) are stored and compared
  for equality, and never parsed, searched or used in a constraint (C9).

### 5.2 Synced tables

The columns beyond the common ones are those of the local store
(`crates/iamb-store/migrations/0001_initial.sql`), with these server-side additions and rules.

| Table | Columns | Server-side rules |
| --- | --- | --- |
| `profile` | `kind`, `name`, `owner_user_id`; added: `offered_owner uuid`, `offered_at bigint`, `head_rev bigint` | One personal profile per account (unique on `owner_user_id` where `kind = 'personal'`), created by `hello` with the name `Personal` when the account has none. `offered_owner` and `offered_at` hold a pending offer of the ownership (C16). Written only by account calls. Its row is the lock of C24, and `head_rev` is the last revision a transaction that held that lock drew: every such transaction sets it, without making a new revision of the profile row, and it is never sent. In any consistent copy of the database it is the last revision of this profile the copy holds (C18) |
| `membership` | `profile_id`, `user_id`, `role` | Group profiles only. Unique live `(profile_id, user_id)`; exactly one live `owner` per profile (unique on `profile_id` where `role = 'owner'`). Written only by account calls |
| `folder` | `profile_id`, `parent_id`, `name`, `position`, `member_permission`, `default_trigger`, `default_case_mode`, `abbreviation_prefix` | `profile_id` never changes. The parent is in the same profile. At most four levels and no cycle, kept by C8 under the lock of C24 |
| `folder_share` | `folder_id`, `profile_id`, `permission` | The id is derived from `folder_id` and `profile_id` (§3.4) and checked. The folder is in a personal profile; `profile_id` is a group |
| `snippet` | `folder_id`, `abbreviation`, `label`, `kind`, `body`, `trigger_mode`, `case_mode`, `word_start`, `enabled`; added: `profile_id`, `content_by uuid`, `content_at bigint` | `profile_id` is the folder's profile, kept by the server for the policies. `content_by` and `content_at` say who last changed what the snippet does and when; `content_by` is null when the server cannot say (C17). No unique index on the abbreviation |
| `tag` | `profile_id`, `name`, `color` | No unique index on the name |
| `snippet_tag` | `snippet_id`, `tag_id` | Snippet and tag are in the same profile. Hard-deleted with either end at the purge |
| `attachment` | `profile_id`, `sha256`, `mime`, `size`, `file_name`; added: `unlinked_at bigint`, `backed boolean` | Never changes after its creation. Several rows may name the same `(profile_id, sha256)` (C19). `unlinked_at` is the server's time since which no live link from a live snippet names the row; null while one does. `backed` is set by the server: when the row is created, if its creator can read the verified file of its profile and hash at that moment; and when its creator commits an upload of those bytes (§6.11). Only a link to a backed row makes a file readable (§5.13). A row that a device gave back after a restore without its user being the row's writer has no creator (`updated_by` is null): it is not backed, and no upload makes it so, until its writer sends it as their own (C19) |
| `snippet_attachment` | `snippet_id`, `attachment_id` | As `snippet_tag` |

Size limits, checked by `push` (C14): `body` at most 400,000 bytes (the engine types at most
100,000 characters); `label`, a folder's or tag's `name` and `file_name` at most 1,024 bytes;
`abbreviation` at most 64 characters; `abbreviation_prefix` at most 64 characters; `color` at
most 32 bytes; `mime` at most 127 bytes.

### 5.3 Accounts and devices

| Table | Columns | Notes |
| --- | --- | --- |
| `user_profile` | `user_id` (primary key, the authentication user), `display_name`, `created_at`, `updated_at` | Created by the account's first `hello`. The display name starts empty; the user sets it (§3.1) |
| `device` | `id` (primary key), `user_id`, `session_id` (unique), `name`, `platform`, `app_version`, `protocol`, `created_at`, `last_seen_at`, `revoked_at`, `position_token`, `position_token_before` | One row per authentication session (C25). A revoked row is kept for as long as its session exists (§5.12). `position_token` is the position the server issued to the device last, `position_token_before` the position that the call it was issued to had presented (C25). The row is the lock on the device's receipts (C15). `begin_epoch` empties the position columns and `last_seen_at` of every row: a device counts again when it has called in the new epoch (C25), and for a row that has not, the 180 days of §5.12 run from the epoch's start |

No trigger on the authentication service's tables is needed: `hello` creates the personal profile
and the `user_profile` row when they are missing, in its own transaction, so a failure there
cannot block a sign-up.

### 5.4 Access entries

`folder_access (user_id, folder_id, permission, via, granted_at, root_folder_id, since_rev)`,
primary key `(user_id, folder_id)`.

One row for every user and folder such that the user may read the folder under §4.2:
`permission` is the highest permission, `via` is `own`, `group` or `share`, `root_folder_id` is the
user's access root for the folder, and `granted_at` is the server's clock at the moment the
user's access to that root began: the time its entry was first written, kept for as long as the
access lasts without a break, whichever grants come and go meanwhile. It is never taken from a
row a device wrote, so no device clock can move it (D13 orders shared folders by it).
`since_rev` is the value of the server's revision counter **when the request that made this entry,
for this user and this folder, had ended**: at or above every revision that request drew. An entry
that ends and begins again begins with a new value, and a folder that
comes into a tree the user already reads gets the value of that moment, not the tree's. Every
write of a profile draws its revision under the profile's lock, and so does the change that makes
an entry, so the number says exactly which versions of a row the user could read through the
entry: a version that a later request replaced, with a revision above `since_rev`, was the row in
a state of the server the user could have seen; one replaced at or below it was not (C20). That
includes a version replaced in the very request that made the entry: a push applies its ops in
dependency order, folders before snippets (§6.3), so "she emptied the snippet, then moved its
folder into the shared tree", sent as one push, is applied as "moved, then emptied", and the entry
must not count as older than the emptying (S88, S89). Until its request has ended an entry is
"not begun" and offers no version to anyone. No clock is involved.

A deleted folder keeps exactly the entries it would have if it were live, computed from the
shares and memberships that are live now, until the purge removes them. So the owner of a
deleted folder, the members of the group that owns it, and the members who reach it through a
live share of a folder above it can read its tombstone, **if their entry is older than the
delete**. A folder whose own share was deleted
with it (C6) has no entry for the people that share served: for them the folder ends as any
share ends, with `folder_revoked` (C21).

**An entry reads a tombstone only when it began before the delete.** A row that is read through
an access entry (a folder through its own entry; a snippet, and a link of that snippet, through
the entry for the snippet's folder; a share through the entry for its folder) is readable through
it while it is live, and as a tombstone only when the reader deleted the row (`updated_by`), or
the tombstone's revision is above the entry's `since_rev`. A tombstone's revision is the revision
of its delete: no write changes a deleted row except one that makes it live. So the comparison
says whether the user could read the row, in a state of the server the user could have seen,
before it was deleted; a row deleted in the request that made the entry was never such a row. For
everybody else the row is not there: no policy shows it, the feed's entry for it is delivered as
null, a snapshot leaves it out, `rows` answers null, a cascade answer says `gone`, and
`visible_folders` and `access` do not name a deleted folder they may not read. The entry itself
stays, because a folder that comes back needs it; it gives nothing while the folder is deleted.

- **Who deleted a row reads its tombstone**, as who wrote a version is offered it (§5.6). It is
  not a courtesy: a device that creates a folder and, still offline, deletes a folder above it
  sends both in one push, the new folder is created and deleted in one request, and no entry for
  it can be older than its delete. The device must get the tombstones of what it made, in the
  answer to that push, or it would hold them live for good (I1).
- **A snippet that changes its folder leaves its deleted links behind.** A link is read through
  the entry for its snippet's folder, and a snippet can move: into a folder whose readers could
  not read it when a link was deleted, or into one that is younger than that delete. So the
  server removes a snippet's deleted links, with their history, when the snippet changes its
  folder, as the purge would 30 days after their delete (§5.12). What it holds of a snippet's
  deleted links was therefore deleted while the snippet was where it is now, and the entry says
  the right thing about each. A device that holds such a link as deleted keeps it until its own
  purge (D8); one that still held it live has the entry of the delete in its feed, which is
  delivered as null.
- **A tag or an attachment row is not in a folder.** Its tombstone is read by the owner of its
  personal profile, and by a member of its group whose membership began before the delete. Through
  a link from a shared snippet the row is read only while it is live: when it is deleted, the
  readers it had through links get its entry as null and their devices drop it, with the links to
  it, which count for nothing while it is deleted (C10, D3); when it comes back, its entry brings
  the links again (§5.5; S92). The snippet that carries the link can move as well, and no entry
  would say the right thing about a deleted row that is reached through it.

**What a device holds of a withheld tombstone.** Nothing, and the rules that exist see to it; no
new one is needed on the device. There are two halves to it. *A device never holds a live copy of
a row whose tombstone it cannot get.* A reader for whom a tombstone is withheld either never had
the row, or had it under an entry that ended. An entry never ends silently: its end puts
`folder_revoked` for the top of the lost tree into the reader's feed (C21), or the folder arrives
as a null row, and either purges the tree on the device, tombstones and unsent changes included
(D3, D6). When the new entry begins, `folder_visible` makes the device fetch the tree's live rows
and remove what it holds of the tree that the fetch did not bring (D6). A device that was away
for all of it applies the feed in order and does the same; the entry of a delete that the reader
could have seen, still in the feed, is now delivered as null and removes the row (D3). A device
whose position is too old resyncs: the snapshot does not hold the row and the resync removes it
(D7), and a change to the row that the device had not sent is pushed, answered `rejected: gone`
and kept in the list of changes that were not synced (C13, D5). *And the server never withholds a
tombstone that a device holds under an access that still stands.* Whether a tombstone is readable
depends on its revision, on who deleted it and on the entry, none of which changes while the
entry stands; the one row that hangs on something that moves, a link, is removed from the server
when its snippet moves. So what a reader could read once stays readable until that access ends or
the row is purged, and the two-device simulation holds I1 and I4 with these rules (S91).

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
| `folder_revoked` | The tree with the top `folder_id` stopped being readable although it still exists; `detail.except` lists the folders inside it that stay readable through another grant | Purges the tree, except those, and fetches one of those it does not hold (D6) |
| `conflict` | A conflict the user is party to was recorded, or its state changed | Tells the UI host (`ConflictRecorded`) |
| `invitations` | An invitation for or by the user changed | Tells the UI host (`AccountChanged`) |
| `entitlements` | The plan of the user or of one of their groups changed | Reads the entitlements again (§6.12) |
| `blob` | The bytes of an attachment (`profile_id`, `sha256`) were verified | Asks the service what is missing (§6.11) |
| `account` | The display name, a device or the account itself changed | Reads the account again; a revoked device signs out |

Rules for task 4.2:

- **Order.** A transaction collects its entries and writes them last, after everything else: it
  locks the `feed_head` rows of all users concerned in the order of the user ids (C24) and gives
  each entry `seq = head + 1`. The locks are held to the commit, so entries become visible in `seq`
  order: a reader that has seen `n` will never later find an entry below `n`.
- **Who gets a `row` entry.** Everyone who could read the row before the write or can read it after
  it. The writer's own feed gets it too: that is how the other devices of the same account hear of
  the change. A snippet that becomes readable for a user through a move brings its links: each gets
  a `row` entry for that user, and the tags and attachments they name come with the page (§6.4). A
  tag or an attachment row that comes back from a delete brings its live links the same way, for
  the readers who had lost the row (§5.4). A
  new member's feed gets a `row` entry for the group's profile and for each live membership of the
  group. For a user who can no longer read a row after the write, the entry is delivered as null
  (§6.4). A device treats a null folder as the end of that tree, except the parts of it that are
  shared by themselves (D3), so the entries of a tree that leaves a user's sight, and its
  `folder_revoked`, do their work in whatever order a page holds them, and a part that stays
  readable is not removed with the rest.
- **One entry per row.** A new `row` entry for the same user and row replaces the older one. The
  feed of a user is therefore never longer than the rows they can read plus the other kinds of entry
  of the last 30 days. Visibility entries are never replaced or merged: a device handles them in the
  order of the feed (D6).
- **Content is not in the feed.** `pull` joins the entry to the row as it is at that moment, through
  the reader's policies.
- **Retention.** Entries older than 30 days are deleted and `floor` is set to the highest `seq`
  deleted. A position below `floor` cannot be continued (`reset_required`, D7).
- **Poke.** After the entries are written the function sends `{ "epoch": …, "head": … }` on the
  user's live channel (§6.6); the service delivers it after the commit.

### 5.6 History and conflicts

`history (entity, row_id, rev, row jsonb, deleted, written_at, written_by, written_device,
replaced_at, replaced_by_rev, within_request)`, primary key `(entity, row_id, rev)`: every version a write replaced,
as it was, tombstones included; a snippet's version with its `content_by` and `content_at`.
`within_request` marks a version that the same request wrote and replaced (below). A
version that a device gave back after a restore of the server, and that a later one had already
replaced, is kept under the lost revision it had (C18). Invitations are not synced rows and have no
history.

`conflict (id, entity, row_id, profile_id, kind, winner_rev, winner_user, winner_device,
created_at, state, resolved_at, resolved_by, resolution, parties, closed_for)` and `conflict_version (conflict_id,
rev, user_id, device_id, taken_over)`: the versions the conflict offers back. The first is the
version the conflict's write replaced; the others, marked `taken_over`, are the versions of the
conflict it replaced (C3). `kind` is `edit_edit`, `edit_over_delete`, `delete_over_edit`,
`deleted_with_folder` or `folder_restored` and describes the write against the version it
replaced; `state` is `open`, `resolved`, `replaced` (a newer conflict on the same row took it
over, with its versions) or `expired`. At most one conflict per row is open. The content of an
offered version is the history row `(entity, row_id, rev)`; the winner's is the row itself until
it changes again. The parties of a conflict are the winner's author and the author of every
version it offers. A party **takes part** in a conflict only while they can read its row: only
then is it in their list, in their feed and theirs to close (C20). And a party is **offered** a
version only when they wrote it or could read it while it was the row, **in a state of the server
they could have seen**: they read now, and have
read without a break since before the request that replaced the version, the folder that version
was in (for a folder's version, the folder itself; §5.4 `since_rev`); for a tag's version, they own
the tag's personal profile or have been a member of its group since before the version was
replaced. A version that was written and replaced within one request (a request can name a row
twice) was never the row in such a state: the history marks it (`within_request`), and only its
writer is offered it. A deleted version, the tombstone that a later write replaced when it made
the row live again, is offered as the tombstone was readable: to a reader whose access began
before that delete, not merely before the restore (§5.4). A
version that is not offered to a party is, for that party, not a version of the conflict: it is
not listed and cannot be restored by them. Because parties can be offered different versions, an
answer by one of them does not always close the conflict for the others: `closed_for` on the
conflict lists the parties for whom it is closed while it is still open for somebody else (C20). So a conflict never shows a member a version from the
owner's private folder or from before the folder was shared, and never tells a removed member
anything (§4.4, §4.5, §11.3).

What a conflict gives the user (through `Conflicts` and `GetConflict` on the account endpoint):
the row, the current version and each offered version with its author's display name and time,
which fields differ (computed by the UI from the versions), and for each offered version
"Restore this version", besides "Keep the current version" (C20). A version the reader wrote
also names the device it was written on; another person's version names the person only (§5.1).
The interface shows it in the editor ("Changed on another device" · [Show the other version],
flow 6) and in a list.

History is not a feature of its own in version 1: a client reads a history row only through a
conflict it is party to, and only a version that conflict offers to it. There is no read policy
on `history`.

### 5.7 Receipts

`push_receipt (device_id, op_id, status, reason, rev, conflict_id, created_at)`, primary key
`(device_id, op_id)`: what the server answered to a pushed change, with the revision it wrote,
kept 30 days (C15). Which change wrote a version is kept with the version (`updated_op`, §5.1); C3
compares it with the changes an op names in `follows`.

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
| `attachment_upload` | `id` (primary key), `profile_id`, `sha256`, `size`, `mime`, `uploaded_by`, `created_at`, `expires_at` | One upload under way: who may write which object. Expires 24 hours after `created_at`; the daily job removes expired rows and their objects. While it is open it counts against its **uploader**: an account has at most 20 uploads under way, and their sizes together stay within the uploader's own `storage_bytes_max`. It counts against the profile's `storage_bytes_max` as well when the uploader belongs to the profile (its owner, or a member of the group); the upload of anybody else, a member who may change a shared folder, is counted against the profile when it is committed, so that such a member can neither reserve the owner's allowance with uploads never sent nor read the size of the owner's files off the answers. The uploader and whoever manages the profile can read the row and end the upload |

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
| `devices_max` | user | `hello` for a new device, counting the account's device rows that are not revoked, whose session still exists and that called in the last 30 days and since the current epoch began (C25) | `error: plan_limit` |
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
- A limit of the account (`devices_max`, `groups_owned_max`) is counted under the lock of the
  account's personal profile, and `storage_bytes_max` under the lock of the profile the file is for,
  so two calls at the same moment cannot both pass a limit that has room for one (C24).
- Lowering a plan never deletes anything: existing rows stay, and only new ones are refused.

### 5.11 Server state and rate limits

`server_state` has one row: the current `epoch uuid`, `protocol_min`, `protocol_max`,
`protocol_oldest` (the oldest version any build ever spoke; the access check accepts from there,
D12), `retention_days` (30), `history_days` (90), `cascade_max` (2,000), `attachment_bytes_max`,
`sign_in_methods` (the ways a session may have been opened for Iamb to accept it, as the token's
`amr` claim names them; never `password`; C25), and `maintenance` (while it is set, `enter` answers
every call with `maintenance`; C18), where the retention job stopped (`job_after`, §5.12), and the seed of the locks on row ids (`lock_seed`, C24). `epoch (id, started_at, first_rev)` has one row per epoch the
database has had: `first_rev` is where the epoch's own revisions start: a time in milliseconds times
1,024, where the time is the later of the server's clock and a time the operator passes
(`not_before`, C18), and in any case above every revision the restored database knows of (the
restored value of `rev_seq`, the highest `head_rev` of a profile), which puts it above every
revision any device can hold (C18). `epoch_profile (epoch_id, profile_id, backup_rev)` has, for
every profile the database held when the epoch began, the profile's `head_rev` at that moment: the
last revision of that profile the backup holds, and so the line above which its writes were lost
(C18). `rate_bucket (subject, bucket, window_start, count)` holds the counters of §6.8.

### 5.12 Retention

One scheduled database job a day (Supabase Cron, which runs a database function on a schedule).
"Purge" is a hard delete. The job works profile by profile, takes the profile's lock (C24) and
handles at most `cascade_max` rows per transaction, so it never holds a lock for long. **No
profile can keep the job from another's rows**: of the rows one run may purge, a profile gets a
share (a quarter), a run goes on where the last one stopped, and what is not a purge (the expiry
of invitations, offers and conflicts) is done for every profile a run visits whatever is left of
its budget. The ids of groups are chosen by their creators, so the order of the ids must decide
nothing. The job's log line says how many profiles used up their share, so that a job that falls
behind is seen.

| What | Rule |
| --- | --- |
| Tombstones of synced rows | Purged 30 days after `deleted_at`, children before parents, with their history, their conflicts, their links and their access entries. After that the row id is unknown to the server (C13). A deleted link goes earlier when its snippet changes its folder: the write that moves the snippet removes it, with its history (§5.4) |
| Attachments nothing uses | An attachment row whose `unlinked_at` is more than `history_days` (90) old is deleted (a tombstone, purged 30 days later). The 90 days are those of the history, so that a version restored from a conflict still finds its images. When the last row for a `(profile, sha256)` is purged, the stored file and its `attachment_blob` row are removed |
| Uploads under way | An `attachment_upload` row past `expires_at` is removed with its object |
| Feed entries | Deleted after 30 days; `floor` raised |
| History | A version is deleted 90 days after it was replaced (`history_days`), but not while an open conflict offers it |
| Conflicts | An open conflict expires after 90 days; a closed one is deleted 30 days after it was closed |
| Receipts | Deleted after 30 days |
| Invitations | A pending invitation expires after 14 days; an answered or expired one is deleted after 90 days. A pending offer of ownership lapses after 14 days |
| Devices | A device row that is not revoked and has not called for 180 days is removed; the device registers again, and resyncs, when it comes back. A revoked device row is kept for as long as its session exists in `auth.sessions`, however long that is, and removed once the session is gone (C25) |
| Deleted groups | The profile row, its memberships, and the folders and snippets it owned are tombstones for 30 days, then purged. Nobody can read them in that time and no call brings a group back |
| Deleted accounts | §11.1 |

The 30 days are one number, `retention_days`, for tombstones, the feed and receipts, so there is
one line to remember: **a device that was offline for more than 30 days resyncs** (D7).

### 5.13 Row-level security, in words

Read rules. "Reads the folder" means a `folder_access` row for the caller and the folder exists.
"Reads through it" a row means: the row is live, or the caller deleted it, or it was deleted after
that entry began (its revision is above the entry's `since_rev`, §5.4).
Every rule also requires `iamb_private.session_ok()`: a session that still exists and was opened by
one of Iamb's ways of signing in, with a device row that is not revoked (C25). "Member of" and "role
in" are the gate helpers of §5.1, never a subquery on `membership` itself.

| Table | A user may read a row when |
| --- | --- |
| `profile` | It is their personal profile; or they are a member of it; or they read a folder that belongs to it |
| `membership` | It is their own (also as a tombstone); or it is live and they are a member of the same group |
| `folder` | They read the folder, and read the row through that entry: a tombstone, until the purge, only when their access began before the delete (§5.4) |
| `folder_share` | They have `manage` on the folder; or they are a member of the group it names and read the folder; in both cases they read the share's row through the folder's entry |
| `snippet` | They read its folder, and read the row through that entry |
| `tag` | The tag's profile is their personal profile; or it is a group they are a member of, and the tag is live or their membership began before it was deleted (`member_before`); or the tag is live and a snippet they can read has a live link to it; or they wrote it last **and** have `edit` on at least one folder of its profile now |
| `snippet_tag` | They can read its snippet, and read the link's row through the entry for the snippet's folder |
| `attachment` | As `tag`, with the links to attachments |
| `snippet_attachment` | As `snippet_tag` |
| `user_profile` | It is their own; or they share a live group with that person; or that person owns a profile they can read |
| `device` | It is their own |
| `folder_access` | It is their own |
| `feed`, `feed_head` | It is their own |
| `conflict`, `conflict_version` | They are a party (the winner's author or the author of a version the conflict offers) **and** can read the conflict's row under that row's own rule above. The content of a version is not in these tables: it is a history row, which `get_conflict` returns only for a version on offer to the caller (§5.6) |
| `invitation` | They are owner or admin of the group; or it is pending and its address is their confirmed address (`own_address()`) |
| `attachment_blob` | Its profile is their personal profile or a group they are a member of; or a snippet they can read has a live link to a **backed** attachment row with its profile and hash (§5.2). Having written an attachment row with that hash is not enough, and neither is linking that row from a snippet: a row that is not backed gives no file to anyone |
| `attachment_upload` | They are its uploader; or its profile is their personal profile; or they are owner or admin of the group it is for |
| `history`, `push_receipt`, `plan`, `subscription`, `feature_flag`, `feature_override`, `server_state`, `epoch`, `rate_bucket` | Never; functions read them for the caller |
| Storage objects in `attachments` | Reading: the object's name is `<profile>/<sha256>` of an `attachment_blob` row they may read. Writing: only the name `<profile>/upload/<id>` of an `attachment_upload` row that names them as uploader and has not expired, and only once. Nobody reads an upload object |
| Live channel messages | The topic is `feed:<their user id>`. Nobody may send |

Write rules are §4.1 and §4.3, enforced by the functions.

### 5.14 Functions of `api`

Every function takes the device id and the protocol version, calls `iamb_private.enter` first,
and returns one JSON object with an `error` member that is null or a code (§6.1). The time-out
is the function's own `statement_timeout`. "Who" is checked by the function against the state it
finds after it has taken its locks; a call the caller's role does not allow answers `forbidden`,
and one that names a group or row the caller cannot see answers `not_found`. A call takes no lock
on a group or profile for a caller who has no part in it: that is checked before the lock, and
again under it (C24).

| Function | Time-out | Does | Codes of its own |
| --- | --- | --- | --- |
| `hello` | 8 s | Registers or refreshes the device; creates the personal profile and `user_profile` when missing; agrees the protocol version; returns the server's state (§6.2) | `device_revoked`, `device_conflict`, `server_behind`, `plan`, `plan_limit` |
| `push` | 20 s | Applies changed rows under §8 (§6.3) | `reset_required`, `device_conflict`, `too_large`, `invalid` |
| `pull` | 8 s | Returns a page of the caller's feed with the rows (§6.4) | `reset_required`, `device_conflict`, `server_behind` |
| `rows` | 8 s | Returns the given rows as the caller may read them (§6.5, D15) | `too_large` |
| `fetch_folder`, `snapshot` | 15 s a page | Whole-tree and whole-library reads (§6.5) | `not_found`; `snapshot` also `device_conflict` (C25) |
| `visible_folders`, `access`, `users`, `entitlements` | 8 s | The list of readable folders (also for a device that may not sync, D12), the caller's access roots, display names, entitlements | — |
| `check` | 8 s | Nothing but `enter`: what the two edge functions call with the caller's token before they act (below) | — |
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
| `conflicts`, `get_conflict` | 8 s | Conflicts (C20): the open conflicts the caller takes part in; one of them with the current row and the versions on offer to the caller | `not_found` (also for a conflict on a row the caller cannot read) |
| `resolve_conflict` | 20 s | Closes a conflict, or restores one of the versions it offers to the caller (C20); restoring a deleted folder's tombstone deletes at most `cascade_max` rows per call | `not_found`, `forbidden` (the restore is a write the caller may not make), `invalid` (not a version on offer to the caller), `gone`, `partial` |
| `attachment_upload_begin` | 8 s | Opens an upload (§6.11) | `exists`, `plan_limit`, `too_large`, `forbidden`, `invalid` |
| `attachment_upload_cancel(upload)` | 8 s | Ends an upload under way: its uploader, or whoever manages the profile it is for (§5.9) | `not_found` |
| `devices`, `rename_device`, `set_display_name` | 8 s | The account (§11.3) | `not_found`, `invalid` |
| `revoke_device(device)`, `sign_out_everywhere()` | 8 s | Mark one device row of the account, or all of them with the caller's, as revoked, and answer the ids (C25). After `sign_out_everywhere` the agent asks the authentication service to end all of the user's sessions. The two are counted by themselves, per calling device (§6.8), so that no other device of the account, the one to be revoked among them, can use up their rate; an agent that is answered `rate_limited` all the same ends the sessions at the authentication service at once and makes the call again later | `not_found` |

Two calls are Supabase edge functions, because they need what a database function cannot do (§11.6):
`attachment-commit` reads a stored object to hash it (§6.11), and `account-delete` removes the
account from the authentication service (§11.1). Each checks the caller's token, does its work with
the service key, and calls a function of `api_service` for the database part. Checking the token
means what it means everywhere else: before it uses the service key, the edge function calls
`api.check` with the caller's token, a function that runs `iamb_private.enter` and nothing more, so
a revoked device and a session Iamb does not accept are refused here as well (C25, G15). A token
whose signature is good is not enough: a revoked session could otherwise delete the account.

A call that is repeated after it succeeded (the answer was lost) answers as it did the first time:
`create_group` and `invite` carry the id of the row they create, and the others find the state they
asked for. That holds for as long as the caller is still in the group: a repeated call by somebody
who has left it since, or was removed, answers like a call by anybody else who is not in it, and
never with the group's `mark`. Every account call that changes something in a group answers with `mark`, the revision it
drew under the group's lock (C16); the device keeps the mark of each call that took access away, to
know after a restore of the server whether the call was lost (D7). A repeated call that finds the
state it asked for draws no revision and answers the group's `head_rev` as its mark: that is never
below the mark of the first call, so a device that goes by it makes a call again once too often
rather than once too rarely. A call that concerns no group (`revoke_device`, `sign_out_everywhere`)
has no mark: after a restore the device compares what it revoked with the list of devices.

## 6. Protocol

### 6.1 Transport and answers

`iamb-sync` calls the functions of `api` over HTTPS through Supabase's REST interface
(`POST …/rest/v1/rpc/<function>` with the `api` schema selected), with the project's public key
and the account's access token. Every call names the calling `device` and the `protocol` version
agreed in `hello`. A request body is at most 4 MiB.

A function always answers with one JSON object. `error` is null, or one of the codes below, or a
code of the call itself (§5.14). Refusals are data, not HTTP errors, so that what the function
already wrote (a rate counter, a receipt) is committed. Every answer names the server's current
`epoch`, a refusal included: a device that is told to sign out learns from it whether the server is
still the one it has been in step with (D14).

| `error` | Meaning | The agent |
| --- | --- | --- |
| `upgrade_required` | The device's protocol is older than the server accepts | Stops syncing and says so; keeps the access check (D12) |
| `server_too_old` | The device speaks only versions newer than the server's | Tries `hello` again every hour |
| `device_unknown` | No device row for this id and session | Calls `hello`, then repeats |
| `device_revoked` | The device, or the session's device, was revoked | Signs out (D14) |
| `session_refused` | The token's session is not one Iamb accepts: it no longer exists at the authentication service (it was ended there, or the database was restored from a backup that does not know it), or it was opened in a way Iamb does not sign in, such as a password (C25) | Signs out without having chosen it (D14) and asks the user to sign in again |
| `device_conflict` | Another installation uses this device id, or this session already has another device (C25) | Makes a new device id and calls `hello` once more. If that is refused as well, the session itself is the other installation's: the agent signs out (D14) and asks the user to sign in again |
| `rate_limited` | Too many calls; `retry_after_ms` says when | Waits that long, then goes on (§6.8) |
| `reset_required` | `pull` and `push`: the position presented cannot be continued; `reason` is `expired` or `epoch`. A push answered so has written nothing (C25) | Resyncs (D7) |
| `server_behind` | `hello` and `pull`: the device holds a revision or a position this server never issued. The server may have been restored without its procedure, or the client is not what it says; the server changes nothing (C18) | Stops syncing, keeps its library and its outbox, and tries `hello` again every hour (D12) |
| `too_large` | The request is over a size or count limit, or a `push` cost the server more time or space than one request may (§6.3) | Halves the batch and repeats; a batch of one that is still refused is set aside (D17) |
| `plan` | The plan does not include sync | Stops syncing; the library stays local; keeps the access check (D12) |
| `maintenance` | The server is being restored or migrated (C18) | Waits and retries as after a network error |

HTTP status 401 means the access token is no longer good: the agent asks the authentication service
for a new one and repeats. When that service refuses the refresh token, or the repeated call is
answered 401 as well, the session has ended without a word from Iamb's server. The agent signs out
as D14 says for a sign-out it cannot confirm: the user's own library and its outbox stay, and what
the device holds of other people's folders is set aside, not removed, because the device cannot know
whether the server still holds it. A network error, a time-out (40 seconds, above every function's
own) or a 5xx status is retried (§6.8). A function that fails unexpectedly rolls its transaction
back and is retried the same way; receipts make that safe (C15), and D17 says what happens when it
keeps failing.

### 6.2 `hello`: registering the device and agreeing the version

Called after sign-in, at every start of the agent and after `device_unknown`.

```json
{ "device": { "id": "…", "name": "Mac", "platform": "macos", "app_version": "0.4.0" },
  "protocols": [1, 2], "max_rev": 9012 }
```

`protocols` is the oldest and the newest protocol version the build speaks; `max_rev` is the highest
revision of any row the device holds. A server that never gave out such a revision answers
`server_behind` and tells its operator; it does nothing else on the strength of a number a client
sent (C18). The server picks the highest version both sides speak, creates the account's personal
profile and `user_profile` row if they are missing, registers the device under C25, and answers:

```json
{ "error": null, "protocol": 2, "user": "…", "personal_profile": "…", "epoch": "…",
  "head": 1841, "retention_days": 30, "server_time": 1795190400000, "display_name": "",
  "limits": { "push_ops": 200, "push_bytes": 4194304, "pull_entries": 500,
              "cascade_max": 2000, "attachment_bytes_max": 26214400 } }
```

No common version gives `upgrade_required` (the device is too old) or `server_too_old`. C25 says
when `hello` answers `session_refused`, `device_revoked` or `device_conflict` and what the device
does then. An `epoch`
other than the one in the device's position means that the server was restored: the device resyncs
as after `reset_required: epoch` (D7).

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
- `follows` lists the op ids of earlier changes to the same row that this device handed to the agent
  and never saw answered (D1); for a row the device had deleted with a folder, that is the id of the
  folder's op, under which the server wrote the row. The server uses it to know the device's own
  earlier write (C3).
- `cascade` is on a folder's delete only: the sub-folders and snippets the device deleted with
  it, each with its base (C6). They are not sent as ops of their own.
- `repush` is on ops a device sends again after the server lost them (C18): `"row"` when the op is
  the lost version itself, `"base"` when it is a change the device had made on top of a lost version
  and not sent yet. With it come `repush_rev`, the revision of that lost version, `repush_epoch`,
  the epoch of the position the device held when it found that the server was restored (D7), and
  `author: "self"` when the device holds its own user as the author of a snippet's content, or as
  the writer of an attachment row (C17, C19).
- `position` is the device's current position (C25). A push whose position cannot be continued is
  answered `reset_required` like a pull, and writes nothing.
- A request has at most 200 ops; a cascade list may name up to 20,000 rows and counts as one
  (a longer one is `blocked: too_large`, and one that names a row twice `blocked: invalid`, before
  any work). The whole rows in the cascade answers of one request stay within 4 MiB between them;
  which rows come whole is decided from the sizes the server has stored, before any content is
  read, so the work for a list is bounded by its length and by that allowance, not by what it
  names.
  The device builds the request under D1, so it never names a parent the server cannot know.
- **Nothing a device sends ends a request in an error of the database.** Every op is looked at
  before any lock is taken: an op that is not an object, a `base_rev` or `repush_rev` that is not a
  whole number from 0, a `follows` or `cascade` that is not a list, a number in the row that no
  column can hold, are `blocked: invalid` for that op alone (a key whose value is JSON null counts
  as absent). Should an op fail all the same, the request's writes are undone and the answer is the
  error `invalid`, as data: a request that fails must still be counted by the rate limiter (§6.8).
  A request that is cut off (the function's time-out, or the database out of temporary file space,
  disk or memory for it) is caught the same way: its writes are undone, the answer is `too_large`
  as data, so that the device halves the batch (§6.1), and the request is counted as ten.
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
    "cascade": [ { "entity": "snippet", "id": "…", "rev": 9013, "deleted": true },
                 { "entity": "snippet", "id": "…", "rev": 9007, "deleted": false,
                   "row": { "…": "…" }, "origin": { "…": "…" } } ] } ] }
```

| `status` | `reason` | Meaning | Written in the scenarios as |
| --- | --- | --- | --- |
| `applied` | — | A new revision was written. `conflict` is the id of the conflict it recorded, if any; `adjusted` lists fields the server set differently (C8) | `applied`, `applied+conflict`, `applied+adjusted` |
| `unchanged` | — | The row already is what was sent; nothing was written (C4) | `unchanged`, `unchanged+adjusted` |
| `superseded` | — | A share's stale change, or a re-push that another device's newer re-push preceded: the server's row stands (C11, C18) | `superseded` |
| `partial` | — | A folder's delete that is not finished; the answer has no `cascade`; send the op again (C6) | — (no scenario is that large) |
| `rejected` | `forbidden`, `gone`, `parent_gone`, `plan_limit`, `not_pushable` | The change is refused for good (C11, C12, C13, C16) | `rejected:…` |
| `blocked` | `too_large`, `plan_limit`, `invalid`, `parent_blocked` | The change cannot be stored as it is; the row stays on the device (C14). `invalid` is also the answer to an op whose id is in use by a row of another profile (C8, C24) | `blocked:…` |

`replayed` is true when the op id was seen before (C15; `replayed` in the scenarios). `row` is the
row as the server has it now and as the caller may read it, with `origin` for a snippet, or null
when the caller may not read it or it does not exist. **`cascade` is on the result of a folder's
delete, whatever its status except `partial`**: one entry for every row of the op's cascade list,
with the row's revision and whether it is deleted after the request, or `gone` when the caller may
not read it (a tombstone the caller may not read included, §5.4) or it does not exist. An entry carries the whole row, with its origin, only where the
device does not hold that content: the row is live in a revision other than the base the list named
for it. A row the device deleted with the folder therefore always gets an answer: deleted with it,
still live because it had left the folder, or live because the delete was refused (D4, D5). The
answer stays within the 4 MiB of a request: entries whose whole row no longer fits come in the short
form with `fetch` set, and the device reads those rows with `rows` (D15). `blob` is on an
attachment's result: `missing` when the caller cannot read a verified file for it (§6.11). D4 and D5
say what the store does with each result.

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
  issues it (`next` of a pull, `start` of a snapshot) and remembers, for each device, the one it
  issued last and the one that was presented for it: C25. A device asks for what comes after its
  position and stores `next` in the same transaction as the page's effects (D11).
- A `row` entry carries the row as it is now, read through the caller's policies. `row` is **null**
  when the row is no longer there for the caller: purged, no longer readable, or deleted and a
  tombstone the caller may not read because their access began after the delete (§5.4). The device
  then removes its copy (D3). A folder is delivered with an empty parent when the caller cannot read the
  parent.
- **A page is closed under references.** With each row the page carries every row it refers to (its
  folder and the folders above, the tag or attachment of a link, the profile) whose own entry is not
  in or before this page. A device can therefore apply every page by itself.
- **Pages.** At most `limit` entries (500 at most) and 4 MiB; `more` says whether to ask again at
  once. A page always ends at an entry boundary.
- **No gaps to detect.** Entries appear in `seq` order (§5.5), so a device never has to look back.
  Numbers missing from a page are entries that a newer one for the same row replaced.
- `reset_required` when the position cannot be continued: its epoch is not the server's (`epoch`),
  its `seq` is below the feed's `floor`, or the server has issued no position to this device row yet
  (`expired`). All are judged before the position's token is looked at (C25). After the reason
  `epoch` the snapshot tells the device what the restore lost (§6.5, D7). A position above the
  feed's `head` is not continued either: the answer is `server_behind` (C18).

### 6.5 Whole reads

| Function | Returns | Used for |
| --- | --- | --- |
| `fetch_folder(folder, cursor)` | The live rows of the tree under `folder` that the caller can read: folders, snippets with their origin, links, the tags and attachments those links name (with whether the bytes are verified), the shares of those folders the caller may read, the owning profile; and `head`, the feed's head when the first page was read. Pages of at most 500 rows and about 4 MiB by `cursor`; a page holds at least one row | `folder_visible` (D6) |
| `snapshot(position, cursor)` | The same for everything the caller can read, with the profiles and memberships, and the tombstones the server still holds that the caller may read (§5.4). Its first page carries `start`, a position at the feed's head; a device that has just registered presents the position it holds from before, or none (C25). When the position presented is of an older epoch, it also carries `lost`: for every profile the caller can read, the last revision of that profile the restored database held (its `backup_rev` in the epoch that followed the device's, or in the current one when the server does not know that epoch), with that epoch's id and `started_at` (C18) | Resync (D7) |
| `rows(refs)` | Up to 500 rows named by entity and id, each as the caller may read it now, or null | Catching up after a skipped row (D15) |
| `visible_folders()` | The ids of all folders the caller can read, a deleted one only where the caller may read its tombstone. Answered for every protocol version from `protocol_oldest` on and for accounts whose plan has no sync | The access check (D7, D12) |
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
service's heartbeat every 25 seconds. The realtime service asks the database whether a socket may
receive on a topic when the socket joins, not at each message: a socket that joined before its
device was revoked goes on receiving pokes, two numbers each, until it is closed or its token
ends, an hour at most. A join after the revocation is refused, and so is every call the poke
would lead to (G15, G16).

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

Pull comes before push so that a device learns of a revoked folder or a lowered permission before it
sends, and has the parents of what it will send. It changes no outcome: the server refuses what is
not allowed in any order, and a pulled row never replaces an unsent change (D2).

One order does change an outcome, and is kept: **in a new epoch, what took access away goes first.**
After a resync that found the server restored, and before step 3 sends anything, the agent makes
again the calls of the device's list that the restore lost; then step 3 sends the deletes of shares,
then the folders, then everything else, each of these answered before the next is sent (D7). A
device that gave rows back first could put a folder on the device of a person its user had removed
(S79, S80).

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
  `hello` 10 calls; account calls 30 per account; `revoke_device` and `sign_out_everywhere` 10 per
  calling device, in a counter of their own that no other device can use up; invitations 20 an
  hour per group and 50 a day per account. Over the limit the call answers `rate_limited` with
  `retry_after_ms`, having written nothing else. A call that is refused is counted like any other,
  and a `push` that is cut off at its time-out or for want of space is counted as ten (§6.3). Of the
  other calls, one that the database cuts off is not counted: none of them does work that grows
  with what the caller sends beyond the bounds of §6.5. The first upload of a library of 10,000 snippets takes about two minutes
  at these values. The values are settings of the server, not of the protocol (§13).

### 6.9 Partial failures

| What fails | What holds |
| --- | --- |
| One op of a push is refused or blocked | The other ops are applied; each result is acted on by itself (D4, D5) |
| A push is cut off before the answer | The outbox still holds the changes; the same request is sent again and the receipts return the first answers (C15) |
| A push is cut off before the answer and the agent stops before it repeats it | The entries are marked as handed out in the store. Whatever the user does to those rows next is sent as a change that follows them (D1); a row the pull skipped meanwhile is fetched once its change is settled (D15) |
| A push is delivered twice | The second finds the receipts (C15) |
| A push fails every time | After three tries the agent sends less, down to one change; a change that fails alone is set aside as blocked and the rest goes on (D17) |
| The agent stops between the pages of a pull | Each page's effects and its position are one store transaction; the next start continues after the last complete page, with the position the server knows as the one presented for its latest (D11, C25) |
| The answer to a pull is lost, twice or ten times in a row | The device presents the same position each time. The server accepts it each time and issues a new one; nothing is conflicting about a repeat (C25) |
| The agent stops in the middle of a whole read | The position is unchanged; the read starts again and applies rows by revision |
| The agent stops between adopting a library and merging it | The phase is in `sync_state`; the next start goes on from it, and nothing is pushed before the merge is done (D9) |
| The service is not running | The agent cannot read the outbox or apply rows; it waits and tries the endpoint again. Nothing is acknowledged that was not stored |
| The store refuses a row of a page (it should not) | The position does not move past it; after three tries the agent resyncs (D7) |
| The server is restored from a backup | A new epoch, begun by the operator, who first makes again from the function log the calls that had taken access away. Devices send back every row they hold in a revision the backup does not have, also where another device has written the row since, after the calls of their own lists; they take back what only the server writes (C18, D7) |
| A restore ends a device's session before the device has learned of it | The device signs out without removing anything it holds for other people: it sets that aside, unused and unseen, and gives back what was lost at the sync after its next sign-in (D14, C18) |
| A database is restored and the operator forgets `begin_epoch` | A device that holds more than the server is answered `server_behind`, stops, and the operator is alerted; nothing a device sends starts an epoch (C18) |

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
  caller may write in the profile (§4.3), `attachment_bytes_max` and the allowances of §5.9 (the
  uploader's own, and the profile's for an uploader who belongs to it; a size of 0 is `invalid`), and
  answers `exists` when the caller can already read the verified file, or else creates an
  `attachment_upload` row and returns its id; the agent uploads the file to
  `attachments/<profile>/upload/<id>` and calls the edge function `attachment-commit(id)`.
- **`attachment-commit`** checks that the caller is the upload's uploader, reads the uploaded
  object, computes its SHA-256 and size, and: when they equal the upload's, copies the object to
  `<profile>/<sha256>` unless a verified file is there already, calls
  `api_service.attachment_verified(id, sha256, size)`, and removes the upload object; when they
  differ, removes the upload and its object and answers `hash_mismatch`.
  `attachment_verified` checks the hash and the size against the upload row once more, checks the
  profile's `storage_bytes_max` (answering `plan_limit` and recording nothing when the file does
  not fit; the edge function then removes the object), inserts the
  `attachment_blob` row if there is none, marks as `backed` the attachment rows of that profile and
  hash that the uploader created (§5.2), deletes the upload row and writes a `blob` entry to the
  feed of everyone who can read the file, under the profile's lock (C24). Every step can be
  repeated: a commit that was cut off is called again and finds what is already done. An upload
  that is never committed expires after 24 hours (§5.12).
- **A file that is already there is still uploaded by a caller who cannot read it, and only that
  upload makes the caller's row count.** A member who may change a folder, and who attaches a file
  the owner happens to hold elsewhere, uploads it like any other; the commit finds the verified
  file, keeps it, discards the upload and marks the member's attachment row as backed. The member
  has proved that they hold the bytes and learned nothing. Before that the member's row is a row
  without a file: it is not backed, so a link from a shared snippet to it makes the file readable
  for nobody (§5.13), and the push result and `attachment_upload_begin` keep answering as for a file
  that is not there. A row and a link cost nothing to write; the bytes are what cannot be made up.
- **Down.** `MissingAttachments` lists the attachments of the effective library whose file the
  device lacks and whose bytes are verified on the server. The agent downloads each, checks the
  hash, writes it to `attachments/` (temporary file, rename) and tells the service
  (`AttachmentAvailable`). Until then the snippet falls back to plain text
  (`attachment_missing`).
- The device records in `attachment_state` what it has uploaded; a file that is not on the device
  (it only has the row) is not uploaded by it.
- **After a restore of the server** the rows and the stored files no longer need to agree: the
  bucket is not part of the database's backup, so a file committed after the backup is there without
  its `attachment_blob` row and without the `backed` marks its commit set, and a file removed since
  is gone although its rows are back. The operator's script puts the two in step before the server
  opens (C18). A device, in a new epoch, forgets what it has uploaded and offers again every file it
  holds for an attachment row of a profile it may write whose bytes the snapshot reports as not
  verified (§6.5); `attachment_upload_begin` answers `exists` for those the server still has, and
  the commit of the others marks the uploader's rows as backed again.
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
| `outbox.handed_out` (set by the store, in the transaction in which `OutboxRead` returns the entry) | Whether the server may have seen the entry: a row with such an entry is never dropped unsent, and a later change names it in `follows`. The mark is as durable as the entry itself (D1) |
| `outbox.cascade_of` (the `seq` of a folder's delete entry, on the entries its cascade wrote) | The cascade list of a folder's delete (C6) |
| `outbox.blocked` (a reason, or null) and `outbox.tries` | Changes the server cannot store yet, and changes that keep failing (D5, D17) |
| `outbox.repush_rev` (a revision, or null) and `outbox.repush_epoch` | Rows a device sends again after the server was restored, with the revision it holds and the epoch the device was in when it found the restore (C18, D7) |
| `skipped (entity, row_id, rev, gone)` | Pulled rows the store did not apply because of an unsent change, with the revision it skipped, or that the row was delivered as gone (D2, D15) |
| `left_alone (entity, row_id)` | Rows a resync did not find on the server and left because of an unsent change, and the folders above them (D7) |
| `access_removal (id, call, profile_id, subject, mark, made_at)` | The account calls by which this device's user took access away from someone, each with the group, the person, invitation or device it concerned and the mark the server answered; written before the call is sent and completed when it is answered; dropped when the device sees the same access given back; kept 90 days, and across a sign-out. What the device makes again after a restore of the server, before it gives anything back (D7) |
| The unique index `snippet_abbreviation` becomes an ordinary index; the check stays in the store's local write path | Rows from the server are applied as they are; duplicates are ordered by precedence (C9, D13) |
| `sync_state` gains `account_user_id`, `device_id`, `personal_profile_id`, `adoption` (null, `adopted` or `merging`), `set_aside_at` (null, or the time of a sign-out the device could not confirm with the server) | Which account the library belongs to, and how far a first sign-in has come (D9, D14). While `set_aside_at` is set, the rows outside the personal profile are kept and left out of the effective library (D14) |
| `folder_access (folder_id, profile_id, permission, via, granted_at)` | The user's access roots as the server computed them (§4.2); read-only state in the interface, and precedence (D13) |
| `snippet_origin (snippet_id, content_by, content_at, created_rev)` | Who wrote a snippet's content, and when the server first accepted the row (C17, D10, D13) |
| `approval` gains `approved_content` (the abbreviation, kind, body, trigger, case mode and word-start flag that were allowed, as JSON); `approved_rev` stays for the review dialog | "The last approved revision keeps expanding" without the server (D10) |
| `kept_change (id, entity, row_id, kind, content, held, place, reason, made_at, kept_at)` | Changes the server refused or that lost their folder, kept for the user: `kind` is `put` or `delete`; `held` says that the content was waiting for review when it was kept (D5, D10) |
| `known_user (user_id, display_name)` | Authors' and members' names for the review dialog and the folder header |
| `attachment_state (profile_id, sha256, uploaded)` | Which files this device has sent (§6.11); forgotten when a resync finds a new epoch |

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
| `ApplyRemote(batch, position, mode)` | Applies rows under D2 and D3; a null row removes the local row; stores `position` in the same transaction when given; answers per row (`applied`, `shadowed`, `stale`, `removed`, `refused`). `mode` is `feed`, `reconcile` (equal revisions are written too) or `epoch` (a row the device holds above its profile's lost line, and may write or holds in a version its own user wrote, is left as it is and queued again; every other row is taken as the server has it, whatever its revision), D7. A null row for a folder purges that folder, except the sub-trees that are shared by themselves (D3). Runs the clean-up of D16 at the end |
| `PurgeFolder(id, except)` | Under D6: first moves unsent changes of the tree to `kept_change` (a row that is only queued to be sent again after a restore of the server: when the device's user wrote its version, D7), then removes the tree, its approvals, usage, origins and cached files, and rows of other people's profiles that nothing left uses; leaves the sub-trees named in `except` |
| `SyncState` (new) | The account id, device id, position, the adoption phase, the count of waiting, blocked and kept changes |
| `Adopt(user, personal_profile, mode)`, `AdoptMerge` (new) | D9 |
| `ReconcileBegin(scope, epoch, lost)`, `ReconcileEnd(token, position)` (new) | D7: rows applied between the two with the token are noted; `End` removes synced rows of the scope that were not among them and have no unsent change, and records the others in `left_alone`. With `epoch` (after `reset_required: epoch`), `lost` is the snapshot's line per profile; rows held above it are kept and queued again, and `End` returns the entries of `access_removal` whose mark lies above their group's line or that have no mark. A resync after a sign-in ends what a sign-out had set aside (D14) |
| `NoteAccessRemoval(call, profile, subject, mark)`, `AccessRemovals` (new) | D7: the agent notes a call that takes access away before it sends it, without a mark, and again with the mark when the account endpoint is answered `ok`; it reads the list back after a restore of the server |
| `SetAccess(roots, roles)`, `SetKnownUsers(users)` (new) | Fill `folder_access` and `known_user` |
| `PutShare(share)`, `DeleteShare(id)` (new) | Write a share as a local change, with an outbox entry, for the account endpoint's `SetFolderShare` and `RemoveFolderShare`, so that sharing works offline (flow 6) and in order with the folder's own rows |
| `SetBlocked(op, reason)` (new) | D17 |
| `SignedOut(confirmed, keep)` (new) | D14: after a sign-out the server confirmed, the rows outside the personal profile are purged, and `keep` says whether their unsent changes are kept or discarded; after any other, they are set aside |
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
  device, or under this device's id by a change the op does not name in `follows`. The server keeps
  with each version the change that wrote it (`updated_op`, §5.1); for a row a folder's delete took
  along, that is the folder's change. So a device that edits a row again before it saw its own push
  answered gets no conflict with itself, also when the row is one it had just deleted with its
  folder, and two installations that share one device id by accident do get one (C25);
- since the base someone changed a recorded field or whether the row is deleted. Recorded fields
  are, for a snippet: folder, abbreviation, label, kind, body, trigger, case mode, word-start,
  enabled; for a folder: parent, name, the member permission and the defaults for new snippets;
  for a tag: name and colour. A folder's `position` is not recorded, and links, attachments and
  shares never get conflicts. The comparison is between the base version, taken from the
  history, and the version being replaced; when the history no longer holds the base, between
  what was sent and the version being replaced.

The conflict names the writer as winner and offers the replaced version back; its parties are
the writer and the authors of the versions it offers, and it is shown to those of them who can
read the row. **A row has
one open conflict. When a new one is recorded while another is open, the new one takes over
every version the old one offered, with their authors as parties, and the old one is closed as
`replaced`**: a version that was replaced unseen stays on offer until somebody answers, however
many writes follow. A write that records no conflict leaves an open conflict as it is.
**Taking a version over does not widen who may see it.** Each party is offered exactly the
versions they wrote or could read while they were the row (§5.6): the writer of the new conflict
is offered the version the write replaced, which the writer could read to replace it, and a
taken-over version only when the writer had access to the folder it was in since before it was
replaced. Three people who shared the folder throughout are each offered every version (S57); a
member whose stale write lands on a snippet that came out of the owner's private folder, or on a
folder that was shared after the older versions were replaced, is offered none of those (S85),
and it makes no difference whether the owner's device sent the replacement and the sharing in two
requests or in one (S88, S89).
(S01, S08, S22, S49, S50, S57, S60, S85, S88, S89)

**C4.** *Nothing new, nothing written.* When what was sent equals the row in every field and in
whether it is deleted, the answer is `unchanged` with the row's revision: no revision, no
history, no feed entry, no conflict, whatever the base. (S05, S42, S43)

**C5.** *A delete is a write.* Whichever of an edit and a delete arrives last wins. A stale
`delete` deletes the row and, under C3, records `delete_over_edit` with the live version on
offer. A `put` on a tombstone makes the row live again and, when stale, records
`edit_over_delete` with the tombstone on offer; a `put` on a tombstone the writer may not read is
a `put` on a row that is gone (C13). A `delete` of a tombstone is `unchanged`; a
`delete` of a row the server does not know is `unchanged` too, with no row in the answer. (S03,
S04, S05, S18, S19, S20, S22, S50)

**C6.** *A folder's delete takes the subtree, as the server has it.* Deleting a folder deletes, in
the same transaction, every live folder and snippet below it as they are on the server at that
moment, and every live share of those folders and of the folder itself (C21 then ends the access
they gave). Each gets its own revision and feed entry. The op's `cascade` list says which rows the
deleting device deleted with the folder and in which revision. It does not decide what is deleted: a
row the list names that has left the folder stays live, and a row in the folder that the list does
not name is deleted. A row that another device wrote and that the list does not name, or names with
a base below its revision after a change to a recorded field, gets a `deleted_with_folder` conflict,
so it can be restored. Restoring a row restores the folders above it (C7). **A share is ended only
by who may end it**: when the root of a live share lies below the folder, the delete needs `manage`
on that root, and without it the whole delete is `rejected: forbidden` and nothing is deleted. A
member who may change a shared folder can therefore not end another share by deleting a folder above
it (§4.3). **The answer has an entry for every row of the list, with the row as the server has it
after the request** (§6.3; in short form where the device holds the content already), whether the
delete was applied, refused or changed nothing, so the device never keeps a row deleted that the
server has live. **A delete of more than `cascade_max` rows is made in parts**: each request deletes
at most that many rows of the subtree, deepest first, with the conflicts above, and answers
`partial`, without entries for the list, while rows remain; the folder itself is deleted in the
request that finds nothing left below it, and that answer has the entries. Between the parts the
tree is valid: there are fewer rows in a live folder. (S15, S16, S23, S51, S52, S53, S61, S76)

**C7.** *A live row needs live folders.* A `put` that would leave a live snippet or folder under
a deleted folder restores every deleted folder on the way up, if the writer may: `manage` for a
top-level folder, `edit` on its parent for a sub-folder. Each restored folder gets a new
revision and a `folder_restored` conflict between the writer and whoever deleted it. What was
deleted with the folder stays deleted, and its shares stay deleted: access never comes back by
itself. If the writer may not restore a folder on the way, the `put` is `rejected: parent_gone`;
the writer may not when the folder is a tombstone the writer cannot read (§5.4): restored, it would
hand over a name the writer never had.
Under C24 this holds for requests that arrive together as for those that arrive apart. (S15, S53)

**C8.** *Structure is put right, not refused.* A folder `put` that would make the folder its own
ancestor, or make any tree deeper than four levels, keeps the folder's current parent; a new
folder that would be a fifth level is put under the nearest folder above the requested parent
that keeps four levels, or at the top. **So is a deleted folder that comes back**: the folders
below a folder that is moved count only while they are live, so a deleted sub-folder can come to
lie on a fifth level; when it is restored (a `put`, a conflict's version, or the folders C7
restores on the way up) it goes under the nearest folder above that keeps four levels, if the
writer may write there, and otherwise the restore is refused like any other write the writer may
not make. Everything else of the row is applied, `adjusted` names
`parent_id`, and the device takes the server's row. A row whose profile would change (a folder
under a parent of another profile, a snippet moved to a folder of another profile, a link across
profiles) is `blocked: invalid`. (S13, S14, S53)

**C9.** *The server does not read content.* No uniqueness, search or parsing applies to
abbreviations, labels, bodies or names; the server only stores them and compares them for
equality (C4, C17). Two snippets with the same abbreviation in one profile, two tags or two
sibling folders with the same name are all accepted. Which snippet holds an abbreviation is
decided on each device by the same precedence (D13). (S09, S10, S11, S25, S48, S54, S81)

**C10.** *Links are rows.* A link is created and deleted by itself; nothing cascades to it and it
cascades nothing. It counts only while it and both rows it joins are live, and it is purged with
either; a deleted link is also removed when its snippet changes its folder (§5.4). A link to a tombstone is accepted and waits; a link to a row the server does not know is
`rejected: parent_gone`. Two links for the same pair may exist; devices treat them as one, and a
device that takes a tag or an image off a snippet deletes every live link of the pair (D3).
(S17, S18, S19, S20, S21, S63, S64, S92)

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
(C6). `forbidden` is an answer about something the caller can see. Where the caller has no grant
at all (a profile they neither own, nor are a member of, nor read a folder of; a group they are
not in), the server answers what it answers for a profile or group that does not exist (C13), and
a `delete` of a row the caller cannot read is answered like one of an id the server does not know:
`unchanged`, with nothing deleted. One thing no answer can hide: a `put` with base 0 whose id is in
use is not applied, whoever's row holds the id; ids are random, so that tells something only to
somebody who holds the id already. (S30, S31, S34, S36, S39, S52, S62)

**C13.** *Rows the server does not know.* A `put` with a base above 0 for an id the server does
not have is `rejected: gone`: the row was purged more than 30 days after its delete, and it does
not come back. The one exception is a re-push of a revision that a restore of the server lost
(C18). A `put` with base 0 creates the row. **A tombstone the writer may not read (§5.4) is such
a row for the writer**: a `put` with its id is `rejected: gone` whatever its base, the row stays
deleted, and the device keeps the change and drops its copy (D5). Were the row restored instead,
its live links would be live again with it (C10), and the writer would read tags and files that
were never shared with them. A share is the exception: its id is derived, and who may write it is
decided by `manage` on the folder alone (C11). A row whose folder, or other row it must refer to,
is unknown to the server or unreadable for the writer is `rejected: parent_gone`; so is a tag, an
attachment or a top-level folder that names a profile, and a share that names a group, in which
the writer has no grant, whether it exists or not. (S23, S43, S59)

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
account calls, never through `push` (`rejected: not_pushable`). Every call takes the locks of C24
first, checks §4.1 and its own conditions (§5.14) against the state it finds, and writes everything
in one transaction; of two calls that race, the one that gets the lock first decides what the other
finds. Every call that changes something draws a revision under its group's lock, also when it
writes no synced row (an invitation, an offer of ownership), records it as the group's `head_rev`
and answers it as `mark`: after a restore of the server that number says exactly whether the call is
in the backup (C18, D7). `accept_invitation` needs a pending, unexpired invitation whose address is
the caller's confirmed address (§5.8); `withdraw_invitation` needs it pending. **Ownership is handed
over by offer and acceptance**: `transfer_ownership` records the offer on the group and changes no
role; `accept_ownership`, by the member it names, needs that member to be a live member still and
within their own plan's `groups_owned_max`, and changes the profile's owner and both memberships
together, and with them the access entries of the group's folders; nobody can be made an owner
without having said yes. **An offer is made to a membership and ends with it**: removing the
person, or their leaving, withdraws the offer in the same transaction, and a membership that
begins later (an admin invites the person back) cannot accept an offer made before it began.
Ending a membership (remove, leave)
deletes the shares from that person's folders into the group and removes that person's access
entries for everything shared with the group. Deleting a group deletes its memberships, the shares
into it and the folders it owns with their content; every member gets `folder_revoked` for the
group's folders and for the folders others shared with it. The group's own rows get their tombstones
in two statements over the whole profile, with no feed entries, because nobody can read them any
more; the work is bounded by the group's plan. No call brings a deleted group back. (S30, S32, S33,
S34, S35, S39, S58, S79, S80)

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
   no earlier op), the server cannot tell: `content_by` is null, "nobody". A base version the
   writer could not read while it was the row (§5.6) cannot be found either: otherwise a member
   could send guesses with a `base_rev` from before the folder was shared and read off the answer
   which guess equals the old content. The same holds wherever a base version is used (C3).

A re-push after a restore (C18) that differs from the row sets the caller when the op says
`author: "self"`, and nobody otherwise. Restoring a version from a conflict (C20) keeps the
author that version had. "Nobody" counts as someone else on every device (D10). All comparisons
are the server's: the most a changed client can achieve is to be named as the author of content
it sent itself. (S40, S54, S55, S56)

**C18.** *After a restore, devices give back what the server lost.* Restoring the database from a
backup loses every write since the backup, and puts back whatever the backup held: rows,
memberships, sessions. The rule has a part for the operator, one for the server and one for devices.

- *The operator.* The REST interface is closed before the restore (`server_state.maintenance` in the
  old database, and the project's network restrictions while the restore runs) and opened only after
  the restore script has run in the restored one. Task 4.2 ships the script and a page of
  instructions, and tests both. The script does four things, in this order, and a restored database
  is never opened without them:
  1. *It begins the epoch*, with `iamb_private.begin_epoch(not_before)`, below. No call by a device
     runs it.
  2. *It makes again what took access away.* The account calls made since the backup are lost with
     everything else. The function log, which is kept outside the database and holds ids and codes
     (§11.1), names them. Through `iamb_private.replay_call` the script makes again, in the order of
     the log, every logged call that was answered `ok` after the backup's moment and took something
     away: `delete_account`; `remove_member`, `leave_group`, `withdraw_invitation`,
     `set_member_role` to member, a withdrawn offer of ownership, `delete_group`; `revoke_device`
     and `sign_out_everywhere`; and the delete of a share, which `push` logs with the share's id for
     this purpose. Each is made as its logged caller and under the call's own rules. A call is
     skipped, with a line in the script's report, when the restored state no longer lets that caller
     make it, when what it names is not in the backup, or when the log shows the same access given
     back afterwards (the person accepted a new invitation to the group, the role was raised again,
     the folder was shared with the group again). The script never makes a call that gives access.
     So a person who was removed after the backup is removed again before any device is let in,
     whichever device returns first, and whether or not the device that removed them ever returns
     (S79). The list each device keeps (D7) is the net under a log that was lost or incomplete (S58,
     S80).
  3. *It puts the stored files and their rows in step.* The bucket is not part of the database's
     backup: a restore neither brings back an object removed since nor removes one added since
     (§11.6). An `attachment_blob` row whose object is gone is removed, so that devices are asked
     for the bytes again (§6.11). An object `<profile>/<sha256>` without a row is hashed as a commit
     hashes an upload, and gets its row when the hash is its name; otherwise it is removed. Upload
     objects without a row are removed.
  4. *It reports* what it made again and what it skipped, and the operator reads the report before
     opening.
- *The server.* `begin_epoch` writes a new `epoch` row and, for every profile, an `epoch_profile`
  row with the profile's `head_rev` as the restored database holds it (`backup_rev`); moves
  `rev_seq` to the epoch's `first_rev`; empties the feeds; and forgets the devices' positions and
  when each device last called (C25). `first_rev` must lie past every revision a device can hold:
  those the lost time gave out, and those of an epoch the backup knows nothing of, when a database
  is restored a second time from a backup older than the first. So it is taken from a time: that
  time in milliseconds times 1,024, or one above the restored value of `rev_seq` and above every
  `head_rev` should that be more. Revisions are drawn far more slowly than a thousand per
  millisecond, so every revision given out up to a moment, in whatever epoch, is below that moment's
  number; and revisions stay below 2^53 for two centuries. The time is the later of the server's
  clock and `not_before`, **a floor the operator passes**: the time of the last line the function
  log holds from the database that was lost. The log is stamped outside the database, so the floor
  holds also when the clock of the restored server is behind the clock under which the lost
  revisions were given out; without it the devices that hold those revisions would be answered
  `server_behind` with no way on. `begin_epoch` writes the epoch's id, its first revision and the
  time to the function log, where the next restore finds them.
- *What was lost is read from the profiles, not from the counter.* Every transaction that writes in
  a profile holds the profile's lock, draws its revisions under it and records the last one on the
  profile's row (C24, §5.2). So the writes of one profile commit in the order of their revisions,
  and in a restored database every revision of a profile at or below its `head_rev` was committed
  before the backup's moment and is in it, and every higher one was lost. That line is exact, and it
  is a column of the same copy as the rows. The value of `rev_seq` after a restore says less: a
  sequence is not part of any transaction, and Postgres writes it to its log up to 32 values ahead,
  so a restored counter can stand above revisions that belonged to writes the restore lost (§11.6).
  Nothing in this rule reads it except to place the new epoch's revisions above everything.
- *The server never begins an epoch on a device's word.* A `hello` whose `max_rev` is above anything
  `rev_seq` gave out, or a position ahead of the feed's head, can mean that a database was restored
  without the script. It can as well be a changed client, and both are numbers a client sends.
  `enter` answers that device `server_behind`, which stops its sync and keeps its library and its
  outbox (§6.1, D12), writes a line to the function log, on which the operator's alert fires, and
  changes nothing. The line carries the account, the device and the number it claimed; the alert
  counts an account once an hour, however often it calls. The instructions say how to tell the two
  apart: a database restored without the script shows as reports from many accounts, with numbers a
  little above what `rev_seq` has given out and below the clock's number (now, times 1,024); one
  account alone, or a number beyond the clock's, is a claim. The operator runs the script only after
  a restore they made or found: `begin_epoch` on a database that was not restored costs every device
  a whole read. Until it has run, a server that was restored without it gives out revisions a second
  time, and what it accepts in that time can collide with what devices hold; that is why the
  procedure is the rule, and the report only an alarm.
- *Devices.* Every position of an older epoch gets `reset_required: epoch`. The device resyncs, and
  the snapshot tells it, for each profile it reads, the last revision the backup holds (`lost`,
  §6.5). Then, row by row (D7): a row it holds in a higher revision than its profile's line is a
  write the restore lost, and when the device may write the row, or its own user wrote that version,
  it sends it again as a `repush` with the revision it holds, **whatever the server holds by then**;
  every other row it takes as the server has it. Before it gives anything back it makes again the
  calls by which its user took access away and whose `mark` lies above the group's line, and sends
  what else took access away first (D7).
- *A device need not be signed in still when it learns of the epoch.* The sessions and refresh
  tokens of the restored database are those of the backup. A refresh token is used once and
  replaced, so a device in use holds one that is younger than any backup, which the restored
  database never issued; and a session opened after the backup does not exist in it at all. Such a
  device finds its session refused, by the authentication service or by `enter` (`session_refused`),
  possibly before it has synced once (§11.6 has what task 4.2 must find out about this; the rule is
  the same either way). It signs out without removing anything it holds for other people, sets that
  aside, and takes the steps above at the sync after its user has signed in again (D14, S78). After
  a restore every device may have to sign in again, and the operator's notice to the users says so.

A re-push is honoured as one when its `repush_rev` lies in the span the restore lost: above the
profile's `backup_rev` in the epoch that followed the one the device was in (`repush_epoch`, the
epoch of the position the device held when it found the restore; the current epoch when the server
does not know that one), and below that epoch's `first_rev`. That is the line the snapshot named to
the device (§6.5). What happens then depends on what the row is by now, and on its `lost_rev`: the
highest lost revision the row has taken in again, which stays on the row when it is written again in
the ordinary way (§5.1).

1. *Not written since that restore* (its revision is at or below that `backup_rev`): the re-push is
   applied without a conflict, the version it replaces goes to the history, and `lost_rev` becomes
   its `repush_rev`.
2. *Written since, and the row has taken in no lost revision as high as this one* (`lost_rev` is
   null or below `repush_rev`): the re-push wins. Over an ordinary write it is an ordinary stale
   write (C3), and a conflict offers the version it replaces to both authors: a write made after the
   restore could not have seen what the restore had lost, which is the case conflicts exist for.
   Over an earlier lost version that another device gave back, the earlier one goes to the history,
   and when the two differ in a recorded field a conflict offers it back. `lost_rev` becomes its
   `repush_rev`.
3. *The row has taken in a lost revision at or above this one* (`lost_rev` is at or above
   `repush_rev`), whether or not it was written again since: this is an earlier version of the same
   lost history, or one the row has had already, and it replaces neither a later version nor an
   ordinary write made on top of one. With the same revision and the same content as the row the
   answer is `unchanged` (C4). Otherwise the answer is `superseded`, the version is kept in the
   history under its lost revision if it is not there yet, and when it differs from the row in a
   recorded field a conflict offers it back. The server no longer knows whether the later version
   was written with the earlier one in view, and the conflict that would have said so was lost with
   the rest. One case it does know: a change a device made on top of a lost revision and had not
   sent yet (`repush: "base"`, D7) counts as later than that revision and records it as the row's
   `lost_rev`; when the revision itself is given back afterwards, it goes to the history as a
   version replaced knowingly, with the answer `superseded` and no conflict.
4. *Unknown to the server*: the re-push creates the row, a tombstone as a tombstone, with a
   `created_rev` of the new epoch like any new row, as long as that epoch began less than
   `retention_days` ago. After that the server cannot tell a lost row from one that was given back,
   deleted and purged since, and answers `rejected: gone`, which the device keeps (D5). A row
   created again is a new row to D13: of two of a user's own snippets with one abbreviation, the one
   that holds it after a restore can be the other one, alike on all devices.

A `repush` outside the span is judged as an ordinary write with `repush_rev` as its base. A
re-pushed delete of a folder is a folder's delete like any other (C6). Permission is judged on
arrival as always (C12); a device keeps a refused re-push when its own user wrote that version, and
drops it otherwise (D7). What does not come back is what no device holds: the history and the
conflicts of the lost time, and with them every version that had already been replaced on every
device that may write its row or whose user wrote it (place 12 of §1). The procedure is meant for a
backup of the last days. With one older than `retention_days`, a delete made more than that long
before the restore comes undone as well: the devices have purged its tombstone (D8) and take the row
as the backup has it. (S43, S58, S59, S68, S69, S70, S77, S78, S79, S80, S84)

**C19.** *Attachments.* An attachment row never changes after its creation; a `put` that differs
from it is `unchanged` (the store never changes one either, so nothing is lost by that). Several
rows may name the same profile and hash; the stored file exists once per profile and hash and is
readable, when verified, by those who can read a snippet that links to a **backed** row for it
(§5.13). A row is backed when its creator could read the verified file of its profile and hash at
the moment the row was created, or has committed an upload of those bytes since (§6.11); the server
sets the mark and no client can. A row that only names a hash its writer knows is a row without a
file, however it is linked. **A row that comes back after a restore does not become its sender's.**
A re-push (C18) creates an attachment row with the caller as its creator only when the op says
`author: "self"`, which a device says where it holds its own user as the row's writer; otherwise the
row is created with nobody as its creator. A row without a creator is not backed, at its creation or
by anybody's upload: when a device of the profile's owner gives back a row that a member wrote, the
row must not borrow the owner's right to read the file (S84). Its writer gets it back by sending it
as their own: a re-push with `author: "self"` of a row that has no creator makes the caller its
creator, and the server then judges the row as if the caller had just created it. The most a changed
client gains by claiming a row is a row that is not backed until that client has uploaded the bytes.
The server keeps `unlinked_at` in step with the links, and deletes an attachment row that no live
snippet has linked to for 90 days (§5.12). (S19, S20, S21, S73, S84)

**C20.** *Resolving a conflict.* A party takes part in a conflict while they can read its row, and
only then: the conflict is in their list and in their feed, and theirs to read and to close. For a
party who cannot read the row (a removed member, a member whose share ended) the conflict is not
there: every call about it answers `not_found`, also "Keep", and nothing about it reaches their
feed, in the same transaction that ended their access (G13). Any party who takes part may answer
a conflict. **An answer closes the conflict for everybody when the party who gives it is offered
every version the conflict holds; otherwise it closes it for that party alone**: the conflict
leaves their list and reads as resolved for them, and it stays open, with every version, for the
parties who are offered more. (A member whose stale write took over the owner's open conflict
about a private version cannot answer that conflict away for her.) Once nobody who takes part is
left for whom it is open, it is closed. "Keep the current version" is such an answer. "Restore this version", for one of the versions the conflict offers to the caller (§5.6; any
other answers `invalid`), **is a write of that version by that user and is judged as a push of it
would be** (§4.3, C8, C12): the place the version puts the row in counts as the place the row would
move to, so restoring a version that puts a folder at the top level, back under another parent or
out of the caller's access root needs what that move needs, and one the caller may not make
answers `forbidden` and writes nothing.
The server writes the version as a new revision by that user (a live version makes the row live and
restores the folders above it, C7; a tombstone deletes the row, a folder with its subtree under the
conditions and the bound of C6: at most `cascade_max` rows per call, with `partial` and a repeated
call for more), keeps the author its content had (C17), records no conflict for that write, and
closes the conflict as an answer does (above). The version it replaces goes to the history like any other. A
version whose history row has been purged answers `gone`. A deleted version (the tombstone that a
restore of the row replaced) is offered as the tombstone itself is read: to a party whose access
began before that delete (§5.4, §5.6). (S07, S85, S86, S87, S88)

**C21.** *Visibility.* What a user may read follows from `folder_access` (§5.4), which the server
keeps equal to §4.2 inside every transaction that changes a folder's place, a share or a
membership, under the locks of C24. When a tree becomes readable for a user, `folder_visible`
for its top goes into their feed; when it stops being readable while its rows still exist,
`folder_revoked`. That includes a shared folder its owner deletes: the share is deleted with it
(C6), so the people the share served get `folder_revoked` and not tombstones they could no
longer read, while those who read the folder as its owner, as members of the group that owns
it, or through a live share of a folder above it get the tombstones. **A tree that becomes
readable brings no tombstone**: what was deleted in it before the reader's access began is not
theirs to read (§5.4), `folder_visible` is written only for a live folder, and the fetch that
follows it returns live rows (§6.5). A tree that is revoked
while part of it stays readable through another grant gets one entry: `folder_revoked` for its
top with the parts that stay listed in `except`. A folder that is still reachable through
another grant is not revoked; only its permission changes. Visibility entries are written in
the order the grants changed and a device handles them in that order (D6). (S10, S11, S28, S29,
S30, S35, S36, S37, S38, S61, S62, S65, S67, S82, S90, S91)

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

1. the `device` rows the request locks (`FOR UPDATE`), in the order of their ids: the calling
   device's, which is also the lock on its receipts (C15) and its position (C25), and any other
   device row the call writes (`revoke_device`, `sign_out_everywhere`, the removal of an account).
   `enter` takes the calling device's lock in every function that writes or issues a position, and
   checks the row only once it holds the lock; a function that only reads checks the row without
   locking it;
2. the `profile` row of every **group** the request touches, in the order of their ids;
3. the `profile` row of every **personal profile** the request touches, in the order of their
   ids;
4. and last, after everything else is written, the `feed_head` rows of every user whose feed
   gets an entry, in the order of the user ids (§5.5).

A `push` takes one more kind of lock, after the profiles and before it reads a row: **the id of
every row its ops may write** (an advisory lock whose 64-bit key is made from entity and id with a
seed only the server holds, so that nobody can choose an id that shares another row's lock; all
of them in one order). An id is
unique across all profiles, and two requests can name one new id in two profiles, whose locks are
different locks; under the id's lock, "is there a row with this id" and the write that follows are
one step for both. The second request finds the first one's row and is answered `blocked: invalid`:
a create inserts or fails, and never becomes a change to a row another request made, in whatever
profile (S53 covers one profile; the two-connection tests of G21 cover two).

**A request locks only what its caller can write in.** Before any lock, an op whose profile the
caller has no grant in (not its owner, not a member, no access entry for a folder of it), and a
share whose group the caller is not in, lose their profile: they are answered as for a profile that
does not exist (C12, C13) and lock nothing. An op in a profile the caller can read and write
nothing in (no `edit` or `manage` on any of its folders, and not its manager) locks nothing
either: it is judged by the same rules without the lock, which refuse it, and it could not write
if they did not, because a revision is drawn only under a lock. Whoever can write in a profile can
hold its lock for as long as a request of theirs runs; that is what "one writer at a time" means,
and the bounds on a request (§6.3) are what keeps it short. An account call checks the caller's membership, and
`attachment_upload_begin` the caller's right in the profile, before it locks, and again under the
lock. Otherwise whoever knows a profile's id could hold its lock, and every write of its owner
would wait, for as long as a request of his runs.

What a request touches: an op on a folder, snippet, tag, attachment or link touches the profile that
owns the row; a share touches its group and the personal profile of its folder; an account call
touches its group, and a call that changes who is a member (`accept_invitation`, `remove_member`,
`leave_group`, `delete_group`, the removal of an account) also every personal profile that holds a
folder shared with that group; `resolve_conflict`, the commit of an upload and the daily job touch
the profile of the rows they write; a call that counts against a limit of an account
(`create_group`, `accept_ownership`, a `hello` that registers a new device) touches the personal
profile of the account whose limit it is, and `attachment_upload_begin` the profile the file is for,
so that the count and the write that follows it are one step (§5.10). The owning profile of a row
never changes (C8), so it is read before any lock is held: from the row if the server has it, else
from the folder, snippet, tag or attachment the op names, else from another op of the same request;
an op for which no profile is found touches nothing and is `rejected: parent_gone` (a `delete`:
`unchanged`). The profiles
that share into a group are read after the group is locked. No share into the group can begin then,
because writing a share locks the group first. One can end: a folder's delete ends the folder's
shares under the lock of the folder's profile alone (C6). So the call locks the profiles it found
and reads the shares again under their locks; the set it works on can only have become smaller, and
what it computes is from what it finds then. Every transaction records the last revision it drew on
each profile row it holds (`head_rev`, C18).

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
- *No deadlock by design.* There is one order for all locks: devices by id, groups by id, personal
  profiles by id, feed heads by user id. If Postgres reports a deadlock all the same, the
  function fails as a whole and the device repeats it (C15).

The cost is that writes to one profile wait for each other for the milliseconds a request
takes, which is nothing for people's snippets; reads never wait. (S53; §11.5, G21)

**C25.** *One session, one device, one position.* A device row belongs to one authentication
session and a session to at most one device row.

- `hello` decides by the token's session and the device id it is given. A session that does not
  exist in `auth.sessions`, or that was not opened by one of Iamb's ways of signing in (below):
  `session_refused`. The session has a row with that id: the row is refreshed. The session has a
  revoked row: `device_revoked`. A revoked row is kept for as long as its session exists in
  `auth.sessions` (§5.12), because nothing else ends that session: the authentication service lets a
  user end this session, all sessions or all other sessions, never one other session; a session
  lasts until then; and its refresh token does not expire (§11.6). The row is what refuses the
  session, a day or a year after the revocation, and a revoked session cannot register again under
  another id. The session has a row under another id: `device_conflict`. The session has no row, and
  the id has a row that is revoked, that belongs to another account, or whose session still exists:
  `device_conflict`, because the id is in use or was revoked. The session has no row, and the id has
  none, or one whose session has ended (the user signed out and in again on this installation): the
  row is created, or moved to the new session.
- Every call, through `enter`, and every policy, through `session_ok()`, requires a session that
  still exists in `auth.sessions` and was opened by one of Iamb's ways of signing in, and a device
  row for it that is not revoked. Revoking a device therefore stops it with the commit of
  `revoke_device`, also while its access token is still valid, and keeps it stopped for as long as
  the session exists; nothing depends on the authentication service ending the session, then or
  later (§11.6). `sign_out_everywhere` marks every device row of the account revoked, the caller's
  included; the agent then asks the authentication service to end all of the user's sessions, which
  a user's own token may ask for. If that second step does not happen, the rows refuse every session
  all the same.
- **A revoked session is still a session at the authentication service**, which knows nothing of
  device rows. For as long as that service honours its tokens, whoever holds them can ask it for
  what any signed-in user may ask: to set a password for the account, without knowing an old one; to
  change the account's address; to link another identity to it; to end the user's other sessions.
  None of these calls passes `enter`, so the device row stops none of them, and each is answered
  here:
  - *Iamb accepts only sessions that were opened the way Iamb signs in.* A token says how its
    session was opened (its `amr` claim). `enter` and `session_ok()` refuse, with `session_refused`,
    a token that does not say, and a session of which any method is outside
    `server_state.sign_in_methods`: the link or code sent to the account's address, and the identity
    providers that are switched on. A password is never among them. So a password that was set
    through a revoked session, or in any other way, opens nothing at Iamb: the session it gives is
    refused by every function and every policy before a device row is looked at, and cannot register
    a device (S83).
  - *Where the authentication service lets a project decide before a token is issued*, the hook
    `iamb_private.token_hook` refuses to issue one that was asked for with a password, so that such
    a sign-in fails at the service itself. That is a second lock on the same door; the first does
    not depend on it (§11.6).
  - *A session cannot turn itself into another way of signing in*: a change of address must be
    confirmed from the old address as well as the new, linking an identity by hand is switched off,
    and so are sign-in by phone and sign-in without an account. These are settings of the project
    (§11.3), and task 4.3 checks each.
  - *What is left* to the holder of a revoked session is to end the user's other sessions. That
    costs each of those devices a sign-in and nothing else, because a device that is signed out
    without having chosen it removes nothing it could not get back (D14).

  `revoke_device` leaves the session where it is: the platform offers no way to end one other
  session (§11.6), and a revoked computer that comes back with its session is told `device_revoked`,
  which is the server's word, and removes at once what belongs to other people (D14).
- A position is issued by the server, in `next` of a `pull` or `start` of a `snapshot`, and carries
  a random token. The server keeps two for each device: **the one it issued last, and the one that
  the call it was issued to had presented**. A call is accepted when it presents either. The last
  one is the normal case; the other is a repeat after an answer that did not arrive or was not
  stored. Every answer that issues a position sets the pair anew: *presented* becomes the position
  this call presented, *last* the one it issues. A repeat therefore leaves the presented position
  where it is and replaces only the last one, and the same position can be presented any number of
  times: two lost answers in a row, a page the store refuses three times, an agent that stops before
  it stored a page, and a resync that is interrupted are all repeats. A call that presents any other
  position of the current epoch is answered `device_conflict`: two installations are using one
  device id, or a data directory was put back from a backup. `push` presents the position as well
  and issues none, so an installation that is behind writes nothing.
- A position that cannot be continued anyway, because it is of an older epoch or below the feed's
  floor, is answered `reset_required` **before** its token is looked at, by `pull` and by `push`
  alike; such a push writes nothing. The `snapshot` that follows presents that position, is accepted
  with it, and sets the pair in the same way, with `start` as the last one; a snapshot that presents
  a position of the current epoch that can be continued is accepted under the rule above.
- A device row to which no position has been issued yet has no pair to compare with: a device that
  has just registered, also under a new id after `device_conflict`, after a revocation or after 180
  days away, and every row after `begin_epoch`. Its first `snapshot` is accepted with whatever
  position the installation holds from before, or with none; the server reads only the epoch from
  such a position, to say what a restore lost (§6.5). A `pull` or a `push` on such a row is answered
  `reset_required` (`expired`, or `epoch` for a position of an older epoch), so the device resyncs
  and gets its first position from `start`.
- An installation told `device_conflict` makes a new device id and calls `hello` again. When its
  session is its own (the user signed in on this installation, and only the device id came with a
  copied data directory, or an earlier session of this installation was never ended), that registers
  it as a device of its own, and it resyncs (D7). When the session came with the copy too, `hello`
  refuses again, because the session already has a device: the installation signs itself out, keeps
  its library and outbox, and asks the user to sign in again (D14). Until one of two such
  installations is caught, which is at the next call of the one whose position is neither of the
  two, their writes carry one device id; C3 still records a conflict between them.
- *What counts as a device.* For `devices_max`, `hello` counts the account's device rows that are
  not revoked, whose session still exists, and that have called in the last 30 days and since the
  current epoch began, under the lock of the account's personal profile (C24). A row that an
  installation left behind, by a new id after `device_conflict` or by a sign-out it could not tell
  the server about, stops counting when its session ends, and after 30 days without a call at the
  latest. After a restore the rows and sessions are those of the backup, and a device that had to
  sign in again returns beside its old row, whose session the restore put back; `begin_epoch`
  forgets when each row last called (C18), so a row counts again only once its device has called in
  the new epoch.

(S60, S66, S71, S74, S83)

## 9. The rules on the device

**D1.** *From outbox entries to changes.* The store writes one outbox entry per local change, in the
transaction that makes the change, as it does today. What is sent is one change per row: the row as
it is now, or its deletion; the base revision of its oldest waiting entry; the op id of its newest.
When `OutboxRead` returns an entry, the store marks it as **handed out** in the same transaction:
from then on the server may have seen it, whatever becomes of the request. The mark must be as
durable as the entry: it is written in the transaction that returns the entries, and the store
commits every transaction with a full sync to disk (`synchronous = FULL`, and `fullfsync` on macOS;
`crates/iamb-store/src/lib.rs`), so a mark that was returned survives a power cut. This rule depends
on that setting: a store that committed less strictly could send an entry and forget that it had,
and task 4.4 keeps a test that the setting is in force. A change whose row has handed-out entries
besides the newest names their op ids in `follows`; where such an entry is one a folder's delete
wrote for the row (`cascade_of`), the id to name is that of the folder's change, under which the
server wrote the row (C6). A row that was created and deleted is dropped with its entries only when
none of them was ever handed out; otherwise its delete is sent, with base 0, and the server answers
`applied` or, if it never saw the row, `unchanged` (C5). Rows are taken in the order of their oldest
entry, up to 200; then every row those refer to that the server has never seen is added, so that a
request is complete in itself. A row that refers to a blocked row is blocked with it. Changes are
sent in dependency order (§6.3). (S26, S27, S50)

**D2.** *A pulled row never replaces an unsent change.* While a row has an entry in the outbox,
waiting or blocked, `ApplyRemote` leaves the row alone, answers `shadowed`, and notes in
`skipped` the revision it did not apply, or that the row was delivered as gone. The device's
own push follows, wins on the server (C2), and the server keeps the version the device did not
apply, with a conflict (C3). (S01, S03, S04, S22, S50, S51)

**D3.** *Applying pulled rows.* Each row of a batch is handled by itself, in dependency order with
foreign keys checked at the end of the batch. A row is written only when its revision is above the
local one; an equal or lower revision is `stale` and ignored (in a resync an equal revision is
written too, D7). A null row removes the local row (`removed`) unless D2 holds. **A null row for a
folder ends that tree on the device, except the parts of it that are shared by themselves**: the
store does for the folder what `PurgeFolder` does (D6): unsent changes inside it go to
`kept_change`, one to the folder itself included (D2 does not hold a null folder back), then the
folder and everything in it is removed. What it leaves in place is every sub-tree whose top folder
has, on the device, a live share with a group the device's user is a member of, with the unsent
changes in it: such a part can stay readable when the folder above it does not (S65), and a null row
does not say which parts do. The device decides nothing by this: it only does not remove yet. The
server's word follows and settles it: the `folder_revoked` of the same change names the part in
`except`, or the part's own revocation or the access check removes it (D6, D7). The store's tables
do not let a folder go while it holds snippets, and the rows of a tree that leaves the user's sight
arrive with its `folder_revoked` in no fixed order (§5.5); handled this way, each of them can come
first, and the later ones find nothing left to do. The same is done wherever another rule removes a
folder the server no longer shows the user (D5, D7, D15): a folder never goes without what the
device holds in it. A null row for a snippet takes the snippet's links with it. A tombstone for a
row the device does not have is ignored. A row that was deleted while the device's user could not
read it, or before they could, never arrives as a tombstone (§5.4): if the device still holds it
from an earlier access, it goes as a null row, with its tree when the access ended, in the fetch
of the tree when the access began again (D6), or in a resync (D7). A tag or an attachment row of
someone else's profile that is deleted arrives as a null row too, and the links to it go with it
on the device; they come again with the row if it is restored (§5.5; S92). No uniqueness check applies to rows from the server. A
folder whose chain of parents does not reach the top within eight steps is shown at the top level:
the server never holds a cycle (C24), but a device can for the moment between a pull and the answer
to its own push. A local change that takes a tag or an image off a snippet deletes every live link
of that pair, not only the one the device made (C10). (S13, S15, S16, S18, S61, S62, S64, S72, S82)

**D4.** *Acting on `applied` and `unchanged`.* The entries the change covered are removed. If the
row has no newer entry, the store replaces it with the row of the answer (revision, times, author,
origin, and any field the server adjusted), or removes it when the answer has none (a delete of a
row the server never saw). If the user changed the row again while the push was under way, the store
keeps the row, takes only the revision from the answer, and sets it as the base of the entries that
remain, so that the next push is based on the device's own accepted version. For a folder's delete
the store does the same for every entry of the answer's `cascade`: the entries the cascade wrote are
removed and each row takes what the server sent (the whole row, or in the short form its revision
and whether it is deleted), so a row that had left the folder comes back as the server has it. The
rule for a newer entry holds for these rows too: a row whose delete the user undid while the push
was under way keeps its content, takes only the revision, and its own change is sent next, naming
the folder's change in `follows` (D1). An entry marked `fetch` is noted as skipped and read with
`rows` (D15). `partial` removes nothing and carries no entries: the same op is sent again at once.
(S05, S13, S14, S26, S41, S50, S51)

**D5.** *Acting on the other answers.* `rejected`: every entry of the row is removed, newer ones
included, and the change is recorded in `kept_change` with the reason and the folder's name: for a
`put` the device's version of the row as it is now, for a `delete` a line that names what was not
deleted. Then the row of the answer replaces the local row, or the local row is removed when the
answer has none; for a refused folder's delete, each row of the answer's `cascade` is put back the
same way, except a row with a newer entry of its own, which stays as the user left it and is sent. A
refused re-push after a restore of the server is kept only when the device's own user wrote that
version (D7). For a refused share nothing is kept, the row is put back or removed, and the user is
told. `superseded`: the entries are removed and the server's row replaces the local one. `blocked`:
the entries stay, marked with the reason; the row stays as it is, works on this device, and is
offered again when it changes, when the entitlements change, and once a day. The user sees kept and
blocked changes in the library, never as a notice over another application (UX spec §4.11); a kept
change stays until the user copies or dismisses it. (S23, S30, S31, S36, S37, S39, S45, S46, S52,
S62, S76)

**D6.** *Folders that appear and disappear.* The visibility entries of a page are handled one by
one, **in the order of the feed**, after the page's rows; the page's position is stored only when
all of them are done, so a device that stops in between does them again (D11). On `folder_visible`
the agent fetches the tree (`fetch_folder`), applies it between `ReconcileBegin` and `ReconcileEnd`
for that tree, and reads `access` again. On `folder_revoked` the service purges the tree
(`PurgeFolder`), leaving the sub-trees the entry lists in `except`, which become access roots of
their own: unsent changes in the purged part go to `kept_change` first and leave the outbox, unsent
changes in the part that stays are left where they are; then the folders, snippets, links, shares,
approvals, usage and origins of the purged part are removed, with cached files and rows of other
people's profiles that nothing the device still holds uses (D16). A sub-tree the entry lists that
the device does not hold is fetched like a newly visible one. A folder that arrives as a null row
before its `folder_revoked` is purged at that moment, with the parts that are shared by themselves
left in place (D3), and the entry then finds nothing left to purge. When the tombstone of the user's
own membership arrives, the service also removes that group's profile row and its other memberships;
the group's folders go through `folder_revoked`, which the server writes in the same transaction.
(S10, S28, S29, S30, S35, S37, S38, S58, S61, S65, S67, S72, S77, S82)

**D7.** *Resync.* On `reset_required`, after every sign-in, after the first start of a build
with a newer protocol, and when the store refuses a pulled row three times, the device fetches
everything it may read (`snapshot`) between `ReconcileBegin(all)` and `ReconcileEnd`, writing every
row whose revision is the local one or above, and then removes synced rows that were not in it. Rows
that a sign-out had set aside (D14) take part as rows the device holds; when the resync and its
access check are done, nothing is set aside any more.

- *Rows with unsent changes, and the folders above them, are left alone*, and the store records
  in `left_alone` those of them the snapshot did not hold. The push that follows settles the
  changes (C13, D5); when a left-alone row has no entry in the outbox any more and no row with
  an entry below it, and the server's answers did not bring it, the store removes it, children
  before parents. Nothing the server does not hold stays behind.
- *In a new epoch* (`reset_required: epoch`, or a `hello` that names another epoch than the device's
  position) the device cannot go by revisions, because the server's rows are older than its own. The
  first page of the snapshot carries `lost`: for every profile the device reads, the last revision
  of that profile the restored database held (C18). The store reconciles row by row (`ApplyRemote`
  in the mode `epoch`):
  - **A row the device holds in a revision above its profile's line** is a write the restore lost.
    **When the device may write the row, or its own user wrote that version** (`updated_by`), the
    store leaves the row as it is, **whatever the server holds**, also a row that someone has
    written since the restore; queues it again, marked `repush` with the revision it holds and the
    epoch of the position the device held until this resync; and notes the server's row as skipped
    (D2, D15). The push that follows gives the version back, the server decides between it and what
    was written since (C18), and the device acts on the answer as on any other (D4, D5). A row with
    an unsent change whose base is above the line is marked as well (`repush: "base"`, §6.3), so
    that the server knows its base for a lost revision and the change for a later one. After a
    restore, what a device holds above the line is unsent again, and nothing the server sends may
    replace it before it has been sent.
  - *What "may write" means here* is what the device knew before this resync, read generously,
    because the server judges the permission when the re-push arrives (C12): a row of its user's own
    profile; a row of a group in which its user is owner or admin; a folder, snippet, link or share
    in a tree for which `folder_access` says `edit` or `manage`; a tag or an attachment of a profile
    in which it has `edit` on some folder. The agent reads `access` again only after `ReconcileEnd`.
    A row is measured against the line of its own profile: a snippet's is its folder's, a link's its
    snippet's, a share's its folder's. A profile the snapshot does not name is one the restored
    server does not know (a group made after the backup) or no longer shows this user (a share or a
    membership that began after the backup): it has no line. Of such a profile the rows that count
    as lost are those the server wrote in the `retention_days` before the new epoch began (their
    `updated_at` against the `started_at` the snapshot names): the restore procedure is meant for a
    backup no older than that (C18), and a device that comes back after a long time away, to a
    folder it would have lost anyway, does not turn every row its user ever wrote there into a
    change that was not synced.
  - Every other synced row is taken as the server has it, whatever its revision (the exception to
    I3): a row at or below the line, which the backup held, so that the server's row is that version
    or a later one; a row only the server writes (profiles, memberships); a row above the line that
    the device may not write and whose version someone else wrote. A row at or below the line that
    the server does not hold was deleted and purged, and is removed like any other.
  - A re-push the server refuses (C12) is kept in `kept_change` when the device's user wrote that
    version (`updated_by`); a version someone else wrote is dropped, because it is theirs to give
    back, from a device of their own. The same holds for a row that is queued to be sent again and
    whose folder leaves the device first, at the access check below or by a revocation (D6): the
    purge keeps it where the device's user wrote that version, and drops it otherwise. So a version
    a user wrote, which the restore lost and which that user may no longer write, ends in the user's
    list of changes that were not synced (S77).
  - **What took access away goes first.** In a new epoch the device gives nothing back before it has
    taken away again what its user had taken away. The order is: first the calls of its own list
    that the restore lost (next point); then the deletes of shares it holds above the line; then the
    folders it holds above the line, parents first, because a folder's place decides who reads what
    is in it; then everything else, the live shares last. Each of these is answered before the next
    is sent, and one request is one transaction (§6.3), so nobody sees a state between two rows of
    one step. A call that cannot be made yet, because the server does not answer, holds everything
    else back; one the server refuses for good does not. Without this order the share of a folder,
    or a snippet in a folder whose share had ended, would reach a person the user had removed, for
    as long as it takes the device to make the removal again (S80). What no device can put in order,
    because the removal was made on another device of the same user, the operator's script has made
    again before the server opened (C18, S79).
  - **Calls that took access away are made again from the device's own list.** Nothing the server
    sends tells a device that its user once removed someone: nobody reads another person's
    membership tombstone, an invitation is not a row, a deleted group's profile is readable by
    nobody, a lowered role is a live row, and a revoked device a mark on a row the restore puts
    back. So the agent notes each such call in `access_removal` (§7.1): `remove_member`,
    `leave_group`, `withdraw_invitation`, `set_member_role` from admin to member, the withdrawal of
    an offer of ownership, `delete_group`, `revoke_device`, and `sign_out_everywhere` as one entry
    per device it revoked; each with the group, the person, invitation or device it concerned. It
    writes the entry before it sends the call, and adds the `mark` the server answers (C16) when the
    call is answered `ok`; an entry whose call was refused is removed, and one whose answer never
    came stays, without a mark. Entries are kept for 90 days, longer than a backup is kept, and
    across a sign-out (D14). An entry is dropped when the device sees the same access given back
    afterwards, by whoever gave it: the person's membership live again in a revision above the mark,
    the role raised again, a new invitation for the same address. The last word was then to give,
    and the device does not make a call against it. In a new epoch the device compares its entries
    with what the restore left: a call whose `mark` lies above its group's line, or whose group the
    snapshot does not name, was lost, and so was the revocation of a device that `devices` lists as
    not revoked; an entry without a mark counts as lost when the snapshot still shows what the call
    was to take away. If the user may still make the call, the device makes it again by itself and
    notes the new mark; a call the operator's script has made already finds the state it asked for
    and changes nothing (§5.14). If the user may no longer make it, the device names the call in its
    notice. It never repeats a call that gives access; those the user makes again. A device whose
    user owns or administers a group shows once that the server was restored (`restored`), with what
    it could not make again.
  - The device forgets which files it has uploaded, and offers again those the snapshot reports as
    not verified (§6.11).
- A resync ends with the **access check**: the device asks for the folders it may read
  (`visible_folders`), purges every folder of another profile that is not among them, and fetches
  every tree among them that it does not hold (as for `folder_visible`, D6; a device that may not
  sync only purges, D12). The access check also runs at every start of the agent and once a day, so
  that no lost message can leave a revoked folder on a device that is online, or a readable one
  missing from it.

(S23, S29, S43, S44, S49, S58, S59, S68, S69, S70, S77, S78, S79, S80, S84)

**D8.** *The local purge.* Once a day and after each sync the store removes tombstones whose
`deleted_at` is more than 30 days old, with their links, usage and approvals, but never a row that
has an entry in the outbox. A tombstone the server never saw (revision 0, no account) is removed
after 30 days as well. Once a day it also ends what a sign-out set aside more than 30 days ago
(D14); the service does that by itself, because a signed-out device has no agent at work. (S23, S29)

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

**D11.** *One loop, one position.* The agent pulls, pushes, and pulls again (§6.7). The effects of a
page (its rows, the purges and fetches its visibility entries ask for) and its position are stored
together, and a page that is read twice has the same effect as once. A device that stops before it
stored a page, or never received the answer, presents its previous position again, which the server
accepts as a repeat, as often as it takes (C25). A poke or a poll only starts the loop early;
without the live channel the device polls (§6.6). (S47, S67, S71)

**D12.** *Too old, or without a plan.* On `upgrade_required` or `plan` the device stops syncing,
keeps expanding and editing, keeps every change in its outbox, and says why (Iamb must be updated;
the plan does not include sync). It keeps one call: the access check (`visible_folders`, D7) at
every start and once a day, which every version of the server answers, so that a folder that is no
longer shared with the user still leaves the device. On `server_behind` the device stops syncing in
the same way, and its sync status says that it is waiting for the server, but it makes no call
except `hello`, once an hour: a server that holds less than the device may have been restored
without its procedure, and what it says about access is not acted on until it has answered `hello`
again. (S44, S75)

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

**D14.** *Signing out.* A sign-out ends the session on the device. What it does to the library
depends on one question: does the device know that the server still holds what it acknowledged?

- *The user's own library always stays*: the personal profile with its revisions, its outbox, its
  position, the account it belongs to and the list of calls that took access away (D7). It keeps
  working. Signing in again with the same account resyncs (D7) and pushes what waited; the resync
  presents the position the device kept, under its old device id or a new one, which is how the
  device learns that the server was restored while it was signed out (C25). A different account is
  D9's last sentence.
- *A sign-out the server confirmed removes what belongs to other people.* A sign-out is confirmed
  when the server's own answer at that moment, the one that ends the session or the one to the round
  of sync a sign-out by hand makes first, names the epoch of the device's own position; every answer
  names the epoch, a refusal included (§6.1), and an answer of some earlier day confirms nothing.
  With such an answer no restore lies between the device and the server: everything the server
  acknowledged to this device, the server holds. The service removes everything outside the personal
  profile, the group's folders and the folders shared with the user, as a purge does (D6), and
  unsent changes there go to the list of changes that were not synced. `device_revoked`,
  `session_refused` and `device_conflict` twice (C25) are such answers when they name the device's
  epoch: the device does this without asking, and says that it was signed out. When the user signs
  out by hand, the agent first makes one round of the sync loop if the server answers (§6.7: what
  waited is sent, and a new epoch is reconciled first, D7); then the user is asked about the unsent
  changes to shared folders that are left, keep them in the list or discard them; then the agent
  ends the session with the authentication service and drops the tokens.
- *Any other sign-out sets it aside.* The device cannot ask when the user signs out without a
  connection or while the server is closed. It has no word from the server when the authentication
  service refuses its refresh token (§6.1). And the server's word is of another epoch when a refusal
  (`session_refused`, `device_revoked`) names one. In each case the server may have been restored: a
  restore puts back the sessions of the backup, and can end a device's session before the device has
  learned of the new epoch (C18). Removing shared content then could remove the only copy of an edit
  the restore lost. So the service **sets aside** everything outside the personal profile instead
  (`sync_state.set_aside_at`, §7.1): the rows stay in the store as they are, with their revisions,
  their authors, the access entries and their unsent changes, and are left out of the effective
  library. Nothing in them expands, is listed, searched or exported, and the interface says only
  that shared folders were set aside until the next sign-in. They stay so until one of:
  1. *The same account signs in again on this device.* The resync that follows treats them as rows
     the device holds (D7). In the same epoch it takes the server's rows and removes what the server
     no longer shows the user, keeping unsent changes there. In a new epoch it gives back what the
     restore lost, after what took access away (D7, C18). Then nothing is set aside.
  2. *Thirty days (`retention_days`) pass, another account signs in, or the user asks for the shared
     folders to be removed.* The service purges them as D6 does, which keeps unsent changes, and
     keeps as well, as changes that were not synced, every row whose version the device's own user
     wrote in the 30 days before the sign-out (by the server's `updated_at`). If a restore lost such
     a version, this is where it is; if none did, the list says more than it had to. The 30 days are
     those the restore procedure is meant for (C18), and the daily purge does this without an agent
     (D8).
- When the agent cannot reach the authentication service to end a session the user chose to end, it
  keeps the refresh token for that one purpose, uses it for nothing else, and ends the session at
  its next start with a network: a session nobody holds a token for would otherwise exist for good,
  and its device row with it (C25).
- A device that was signed out without having chosen it makes a new device id at its next sign-in
  when the server refuses the old one (C25).

(S49, S60, S66, S74, S78, S83)

**D15.** *Catching up after a skipped row.* A position moves past rows the store did not apply
(D2), and the feed says each change once. So the store remembers what it skipped. When the last
outbox entry of a row is removed, by an answer (D4, D5) or because the row was dropped (D1),
and the row's revision is still below the one skipped, or the row was skipped as gone and is
still there, `OutboxAck` reports the row as behind. The agent fetches such rows with `rows`
and applies them like pulled rows; a row the server answers with null is removed. Rows of a
cascade that the answer marked `fetch` (§6.3) are read the same way. The note is
cleared when the row has caught up. With D1 and the answers for a cascade (D4, D5) no rule
leaves a row behind; this is the net under them, and what the simulation of task 4.4 leans on
for I1. (S50, S51)

**D16.** *What leaves with a link, and what a link is added for.* A user reads a tag, an attachment
row or a file of someone else's profile only through a snippet that links to it (§5.13), and the
server says nothing when the last such link goes, because the tag or attachment itself did not
change. After every batch that removes or tombstones a link or a snippet of a profile the user
neither owns nor is a member of, and in every purge (D6), the store removes the tags, attachment
rows and cached files of that profile that no live link from a live snippet it holds uses any more.
The other way round, a snippet's body can name an attachment of its profile to which the snippet has
no live link: a version restored from a conflict, whose link went when the image was taken off. The
first device that may write the snippet adds the link, as a local change like any other, **but only
where the link gives nobody a file that they did not have through this snippet or through its
author**. That is one of two cases. The device holds a deleted link between this snippet and that
attachment row: whoever read the snippet had the image before. Or the content's author
(`content_by`) is someone other than the device's user and created that attachment row (its
`updated_by`): the link is to the author's own row, which is backed only if the author had the file
(C19). That the device's own user is the author is not enough. An author is the author of a body as
a whole (C17): a user who corrects a word in a body that someone else wrote becomes its author
without having put the reference there. For the same reason a device links an image when its user
inserts it, in that local change, and never a reference that was already in the body the user edits.
In every other case the name in the body is text, and the snippet shows the image as missing
(`attachment_missing`). A member who may change a shared snippet can therefore not make the owner's
device attach one of the owner's files to it by writing the file's hash into the body, also not when
the owner edits that body afterwards: the link would be the owner's own write to the owner's own
backed row, which the server has no reason to refuse (S81). The cost is small: a version that is
restored more than 30 days after its image was taken off, when the link's tombstone is purged, comes
back without the image unless someone else wrote it, and the user attaches the image again; the file
is still there (§5.12). (S62, S63, S81)

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
| A member writes an attachment row with the hash of a file they do not have, or the hash into a snippet's body | The row and its link are accepted and give no file to anyone until the member uploads the bytes; a hash in a body is text, and no device adds a link for it; a member's row that the owner's device gives back after a restore has no creator and is not backed | C19, D16, §5.13 | S73, S81, S84 |
| An attachment added to a snippet that is deleted elsewhere | The snippet's row decides (delete against edit); the attachment row and the link follow it, and an attachment nothing uses is removed after 90 days | C5, C10, C19 | S19, S20 |
| Membership or permission changes racing an edit | The permission at the moment the edit arrives counts. A refused edit is kept on the device for the user | C12, C16, D5 | S30, S31, S36, S39 |
| Shares racing each other; a share that ends and begins again | One row per folder and group; a stale change never wins; a delete always applies; a device handles the end and the new beginning in the order they happened | C11, D6 | S37, S67 |
| A shared folder deleted, or snippets or a sub-folder moved across its edge | For the people it was shared with, the content ends as a share ends, in one step and without a resync, whatever order the rows and the revocation arrive in; their unsent changes are kept. A part that is shared by itself stays on their devices. What moves in arrives with its tags; what moves out is gone, and so is a tag nothing they can read carries any more | C6, C21, D3, D6, D16 | S61, S62, S63, S65, S72, S82 |
| A member deletes a folder above the root of another share | Refused as a whole: a share ends only by the hand of who may end it | C6, C12, D5 | S76 |
| An invitation accepted while it is withdrawn; ownership handed over | The call that gets the lock first decides; the other is refused with a reason. Ownership changes only when the new owner accepts | C16 | S32, S33, S34 |
| A group deleted with unsent changes on a member's device | The group's folders are revoked and purged; the unsent changes are kept on the device for the user | C16, D6 | S35 |
| Offline for less than 30 days | Nothing special: tombstones and the feed are still there | C22 | S22 |
| Offline for more than 30 days | The device resyncs. Its changes to rows that were deleted and purged meanwhile are refused and kept for the user; everything else is pushed | C13, C22, D7 | S23, S29 |
| Clock skew, ahead or behind | Does not affect which write wins, nor which of two duplicate abbreviations holds | C1, D13 | S24, S25, S54 |
| The same change sent twice | One revision | C15 | S41, S42 |
| The server restored from a backup | Devices send back what it lost: every row a device holds in a revision the backup does not have, measured per profile. What only the server writes comes back as the backup had it. What took access away since is made again first: by the operator's script, from the function log, before the server opens, and by the device that made a removal, from its own list, before it gives anything back | C18, D7 | S43, S58, S59, S69, S79, S80 |
| A change the restore lost, on a row that was changed again after the restore | The lost change is sent again all the same and wins as a stale write; the conflict offers the newer one back. Of two lost versions of one row, the later one stays and the earlier is offered back | C18, C3, D7 | S68, S70 |
| A change the restore lost, on a row its author may no longer write (the share that let them was lost as well) | The author's device keeps the change in the list of changes that were not synced. The share comes back when its owner's device gives it back | C18, D6, D7 | S77 |
| A restore that ends the devices' sessions before they have learned of it | A device that is signed out without the server's word removes nothing it holds for other people: it sets it aside, and gives back what was lost at the sync after its next sign-in | C18, D14, D7 | S78 |
| A device reports more than the server holds | That device stops and the operator is told; the server starts nothing on a client's word, and nothing changes for anyone else | C18, D12 | S75 |
| Device and server on different protocol versions | The older side stops syncing and keeps working locally until it is updated; it still learns when a folder is no longer shared with it | C23, D12 | S44 |
| A revoked device; a copied data directory; answers that do not arrive | The revoked device is refused at once and for as long as its session exists, signs itself out, and keeps what was its user's own. A password set with its tokens at the authentication service opens nothing: Iamb accepts only sessions opened the way Iamb signs in. A copy is noticed by its position and comes back as a device of its own. A position presented again after lost answers is no copy, however often | C25, D11, D14 | S60, S66, S71, S74, S83 |

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
- **Logs.** The functions log ids, counts and codes, never arguments or rows; statement logging of
  function arguments stays off on the server. Every account call logs its name, the ids of the
  caller, the group and the person, invitation or device concerned, its mark and its answer; `push`
  logs the id of every share it deletes, with the share's folder and group; `begin_epoch` logs the
  epoch and its first revision. That log is kept outside the database and carries its own time, and
  it is what the restore script reads after a restore: which calls that took something away to make
  again, and the time before which the new epoch may not begin (C18). The agent's log on the device
  records message names, counts, durations and codes, never content, names or addresses
  (architecture §6.1).
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
| A member who may only use a folder | Read the shared tree (§4.4) | No table accepts a direct write; `push` refuses them (C12); nothing outside the tree is readable, and a refused read looks like a row or file that does not exist. What was deleted from the tree before their access began is not readable either, although the server keeps it for 30 days (§5.4) | They can copy what they can read. What was deleted while they could read the folder reaches their devices as a tombstone, with its content, until the purge |
| A member who may change a folder | Add, change and delete snippets and sub-folders in the tree; add attachments | On every other member's device, content they wrote that presses keys, waits, reads the clipboard, includes another snippet or has several lines is held until approved (D10), under the author the server recorded. No client can set that author, and no write by the reader's own device, stale or not, and no copy can turn it into the reader's (C17, D10; S40, S55, S56). Their snippet can never take over an abbreviation that a snippet with the reader's own content uses, in a shared folder or in the folder owner's private ones, whatever creation time their device sends (D13; S54). They cannot rename, move, delete or unshare the shared folder itself, also when it is a sub-folder of the owner's tree (§4.3), and cannot delete a folder above the root of another share (C6; S76). Deletes are tombstones for 30 days; wrong bytes cannot be stored under a hash, and knowing a file's hash gives no way to read it or to learn that the owner has it, also not through an attachment row and a link of their own, which count for nothing until they have uploaded the bytes (§5.13, §6.11; S73), not by writing the hash into a snippet's body, which no device turns into a link (D16; S81), and not through a restore of the server, after which a row that someone else's device gives back has no creator (C19; S84); they cannot move content out of the tree, share it onward or change who sees it; the rate limits bound how fast they can add | Single-line text they write, without key presses and without including another snippet, expands on the others' devices without review, for abbreviations that none of the reader's own snippets use (ADR-0022 accepts this). They can delete the tree's content, which the owner must restore within 30 days. They can use up the owner's plan limit with rows (§5.10), and with files once they have really uploaded them (§5.9); an upload they only announce counts against themselves. What they are offered of a row's past is what they could read when it was the present (C3, C20; S85), and "Restore this version" lets them write nothing a push would not (S87) |
| A removed member, or one whose share ended | Nothing new from the moment the removal commits: every policy and every function refuses them, including for the tags and attachment rows they wrote there and for the conflicts on rows they wrote (a conflict they were party to is not listed, not announced and not theirs to close any more, C20; S86), and unsent changes are refused (C12). What they still hold are ids, and the answers tell them nothing about those: an op on the group or the profile answers as for one that does not exist, a delete of a row as for an unknown id, and a repeated account call gives no revision of the group (C12, C13, §5.14). An offer of the ownership made to them ended with the membership (C16). Their devices purge the content at the next sync, and at the latest at the daily access check, which runs also on a device that is too old to sync (D6, D7, D12, D16) | — | What their devices already held they may have copied, and a device that never comes online again keeps its copy. The server cannot erase a device. The share dialog should say so. A device that was signed out without the server's word holds what was shared with it, set aside, for 30 days at most (D14). A restore of the server from a backup taken before the removal would make them a member again: the restore script makes the removal again from the function log before the server is opened, so they receive nothing, also not a folder that was shared after they were removed (C18; S79). Where the log does not name the call, they are a member again until the device from which they were removed has synced, which makes the removal again before it gives anything back (D7; S58, S80); if that device never comes back, until an owner or admin removes them again, which the notice after a restore asks owners to check. A removal that was taken back on another device, while the device that made it was offline, is made again by that device after a restore: its list errs towards less access |
| Someone who holds a member's unlocked device | Read the library on it, and act as that member | The owner of the account revokes the device from another device, or signs out everywhere. The device row is marked revoked, and because every call and every policy checks it, access stops with that commit and not when a token expires. The platform cannot end one other session, so the revoked row is kept for as long as that session exists: the computer is refused whether it is switched on the next day or the next year, and the session cannot register another device (C25; S74). "Sign out everywhere" ends the sessions themselves. At the authentication service a revoked session is still a session: its holder can set a password there, and that opens nothing, because Iamb accepts only sessions opened by a link or code to the account's address or by an identity provider, and where the project has the hook no token is issued for a password at all; it cannot change the address without the old mailbox, nor link another identity (C25, the settings below; S83). Then they change what must change | The library on that device stays readable to whoever holds it; malware running as the user is outside the model (architecture §8). The holder of a revoked session can end the user's other sessions, which costs a sign-in on each and nothing else (D14). A computer that "sign out everywhere" reaches while it is switched off has no word from the server when it comes back, and sets shared folders aside for up to 30 days before it removes them (D14) |
| Someone who stole a refresh token | Read and write as the user, from a session that is the user's own | Tokens are in the keychain and only `iamb-sync` reads them; sign-in uses PKCE and a loopback address open for seconds (ADR-0020). Refresh tokens rotate, but rotation does not reliably expose a thief: the authentication service honours the token before the active one, and any token for ten seconds, so a thief and the user can go on side by side; only the use of a token older than that ends the session. A stolen token used from another installation with the same device id is noticed by its position (C25); used with a device id of its own it cannot register, because the session already has a device. Access tokens live one hour at most; the device list shows each device's last call; "sign out everywhere" revokes every device row and ends all sessions | Until the user notices, the thief has what the session can do. The remedy is the device list and "sign out everywhere". A restore of the server brings back sessions and device rows as the backup had them; the restore script revokes again, from the function log, what was revoked since, before the server opens, and the device that had revoked them does so again when it syncs (C18, D7) |
| A signed-in client that lies about what it holds | Send `hello` with a revision the server never gave out, or a position ahead of the feed | The server changes nothing on a client's word: that client is answered `server_behind` and stops itself, and the operator sees a line in the log. No call by a device begins an epoch, empties a feed or touches another device's row (C18; S75) | Every account can learn how many revisions the server has given out, by asking with rising numbers: the counter is public to accounts, and says how busy the whole server is and nothing about anyone. Each such try writes the log line, so the operator's instructions must not read a few such lines as a restore that went wrong |
| A signed-in client that sends what no device sends | Ops of the wrong shape, lists far longer than a device builds, requests made to fail or to run long, calls that name other people's profiles | Every op is checked before any lock and refused as data (§6.3); the work and the answer of one request are bounded (a cascade list, a page, a request); a request locks nothing its caller has no part in (C24); refusals are data, so the rate limiter counts them (§6.8) | A `push` that is cut off is counted, as ten calls; another call that the database cuts off is not. Nothing in the database limits the temporary files one statement may write: on the hosting platform that limit (`temp_file_limit`) cannot be set by a migration and is the owner's to set, so the bounds above are the only ones until it is (§11.6). A reader holds no lock; whoever may write in a profile holds its lock while a request of theirs runs, 20 seconds at most |
| Someone on the network | See that a device talks to the server | TLS; the live channel carries a number, no content | — |
| Another user of the same computer | Nothing | The data root is the user's alone; the endpoints check the peer (architecture §3.3, §6.1) | — |
| The server's operator, or someone who breaks into the server | Read every synced row; change rows, including who is recorded as an author | Hosting controls, and task 4.3 | In version 1 the server is trusted, for reading and for the author it reports. End-to-end encryption with signed content would end that trust (§13) |
| Someone who sends invitations at random | Make people receive an email | An invitation needs the right to invite and is rate-limited; the email carries no link that accepts; accepting shares nothing of the person's own; only the account whose confirmed address it names can see or accept it (§5.8) | — |

**Required settings of the authentication service** (task 4.3 checks each): refresh-token rotation
with reuse detection on, the reuse interval at its default of ten seconds; access tokens of one hour
or less; email confirmation required, so that no account acts with an address it has not confirmed;
only identity providers that assert a verified address enabled, and for each a test that an address
the provider has not verified leaves `email_confirmed_at` empty; the realtime service set to private
channels only. So that a session cannot make itself another way into the account (C25): a change of
address confirmed from the old address and the new one; linking identities by hand switched off;
sign-in by phone and sign-in without an account switched off; a change of password only with a
recent sign-in or a code sent to the address, and with the current password where the project can
require it; and, where the project's plan has it, the hook that issues no token for a password.
Sign-in by password is not used by Iamb and is refused by Iamb whatever these settings are; no
setting was found that switches it off alone (§11.6). The service's own limits on a session's
lifetime (time-boxed sessions, inactivity time-out) need a paid hosting plan and are not relied on;
a device that has not called for 180 days is removed by the daily job (§5.12).

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
| The reader's device sends the row again after a restore of the server, or restores a version from a conflict | A re-push names the caller as author only where the device held the caller as author; a restored version keeps its author | — (S68 to S70 re-push the user's own content) |

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
| G2 | A user reads a folder, a snippet, a link, a share only under §5.13; every table of §4.5 returns exactly the rows listed there, and nothing after the share ends. A row that was deleted by someone else before the user's access to it began is returned by no table and no function, as a tombstone or otherwise: not by `pull`, `snapshot`, `rows`, a push's answers or a conflict; no write of that user makes it live again or restores a deleted folder they may not read; and the policies and `readers` agree on it for every user after every step of the tests |
| G3 | A tag or an attachment row of someone else's profile is readable only while it is live and a readable snippet has a live link to it, or by the user who wrote the row last while that user has `edit` on a folder of the profile; after the share ends or the user is removed, not at all. A deleted tag or attachment row of a group is readable by a member only when the membership began before the delete |
| G4 | No role a device can have may insert, update or delete in any table of `iamb`. Nothing in `iamb_private` or `api_service` can be called through the REST interface with a device's token. The gate helpers and the two reader helpers of §5.1 are the only functions of `iamb_private` that `authenticated` may execute; no gate helper returns a fact about anyone but the caller or changes anything but the caller's device row and rate counters, and the reader helpers change nothing and return the addresses of a group's members to its owner and admins, and the history rows of a conflict's versions to a party they are on offer to, and nothing to anyone else: whatever a device sends, `server_state`, `epoch`, every feed and every other device's row are as they were. `anon` can read nothing and call nothing |
| G5 | `history` is unreadable directly; a history version is returned only by `get_conflict`, to a party of the conflict who can read the row now and who wrote that version or could read it while it was the row: a version from a folder the party could not read then, or replaced before the party's access to its folder began, is not returned, not listed and not restorable by that party, also when a later conflict took it over. "Could read" means in a state of the server the party could have seen: a version replaced in the request that began the party's access, and a version written and replaced within one request, are not such versions. The base version of a write (C3, C17) is found under the same rule |
| G6 | `feed`, `feed_head`, `device` and `folder_access` rows are readable by their user only; a `conflict` by a party who can read its row; an `attachment_upload` by its uploader and by who manages the profile it is for |
| G7 | Every write of §4.3 succeeds for the users that table allows and is `rejected: forbidden` for all others, judged at arrival (C12): after a membership, role or share change commits, the next push sees it. A member with `edit` cannot rename, move or delete the root of the share that gives the access, at any depth, nor delete a folder above the root of another live share. "Restore this version" is one of these writes and is judged as the push of that version would be: it puts no folder at the top level, under another parent or out of the access root for a user who may not move it there |
| G8 | Every account call of §4.1 succeeds for the roles that table allows and fails for all others with the code of §5.14; a group always has exactly one owner, and nobody becomes one without `accept_ownership`, which accepts an offer only for the membership it was made to: an offer ends when that membership ends |
| G9 | `updated_by`, `updated_device`, `content_by`, `rev`, `created_rev`, `updated_at` and `deleted_at` are set by the server; nothing a client sends changes them. `content_by` follows the four cases of C17: the caller only for content that differs from the row and from the write's base version |
| G10 | A profile, membership or invitation cannot be written by `push` (C16) |
| G11 | An invitation is readable by the group's owner and admins and by the account whose confirmed address it names, and only that account can accept it; an account with an unconfirmed address sees none |
| G12 | A stale change to a share is never applied, and a share's delete always is (C11) |
| G13 | After a member is removed, a share deleted, a shared folder deleted or a group deleted, the same transaction has removed the access entries: the former reader's next query returns no row of that content, including rows they wrote, and their feed holds `folder_revoked`; no conflict on that content is listed for them, announced in their feed or closable by them from then on, and no answer of any function tells them whether a row, a profile or a group they remember still exists or what revision a group has reached |
| G14 | A stored file is readable only as `<profile>/<sha256>` of a verified file that a snippet the user can read links to through a backed attachment row, or of the user's own profile or group; a request for any other name, also for an existing file whose hash the user knows and has written into an attachment row and a link of their own, answers exactly like one for a file that does not exist; a row becomes backed only by the server's hand, at its creation by someone who can read the file or at the commit of its creator's upload; a row that is given back after a restore by anyone but its writer has no creator and is not backed, whoever sends it and whatever is uploaded, until its writer sends it as their own; `attachment_upload_begin` and the push result say the same to a caller who cannot read the file whether it exists or not; an upload object can be written once, by its uploader, and read by nobody |
| G15 | A revoked device, a session without a device row, and a session that no longer exists are refused by every function and read no row through any policy; a revoked session cannot register a device again, however long after the revocation and however often its token was refreshed: its revoked row is there for as long as the session is, also after the daily job has run. A session that was not opened by one of Iamb's ways of signing in is refused in the same way (`session_refused`), a session opened with a password among them: with the tokens of a revoked session, and again after `sign_out_everywhere`, nothing that can be asked of the authentication service opens a session that Iamb accepts |
| G16 | The live channel delivers to a user only the topic of their own id and accepts no message from a client. The right to receive is checked when a socket joins: a socket that joined before its device was revoked receives pokes (two numbers) until it closes or its token ends; a join after the revocation is refused |
| G17 | Functions that write are `security definer` with a fixed `search_path`, take the user and the session from the token only, and build no statement from their arguments |
| G18 | Size, count and rate limits are enforced by the server whatever the client sends (§5.2, §6.8); no request deletes more than `cascade_max` rows by cascade; a cascade list names at most 20,000 rows, each once, and the cascade answers of one request stay within 4 MiB, as does the content read for them; a push that is cut off is undone, answered as data and counted; a page of a whole read ends at about 4 MiB; an op of the wrong shape is refused as data and the request is counted; `revoke_device` and `sign_out_everywhere` cannot be kept from being made by another device of the account using up a counter; the retention job gives every profile its share of each run |
| G19 | A repeated push writes once (C15), also when two copies arrive at the same time |
| G20 | No function's log line and no error text contains row content |
| G21 | Two requests at the same moment end as some order of the two (C24). Tested with two connections, the second started while the first holds its locks: folders moved into each other; a create in a folder being deleted; a sub-folder created while its share ends; a push into a group's folder while a member is removed. After each, I7 holds and G1 holds. Every writing function holds its profile locks before its first read of a row it decides on. Two requests that create one id in two profiles (an attachment row, a folder) are serial as well: the second is `blocked: invalid` and the first one's row is unchanged, not backed and where the first put it. A request waits for no lock on a profile its caller has no grant in, or can only read in: an owner's write is answered while a stranger's or a reader's request that names her profile is still open, and while a stranger creates a row under any id of his choosing |
| G22 | A call is accepted with the position the server issued last or with the one that was presented for it, and with the second as often as it is presented; a call with any other position of the current epoch is answered `device_conflict` and writes nothing; a position of an older epoch or below the floor is answered `reset_required` before its token is looked at, by `pull` and `push` alike, and such a push writes nothing; a device row to which no position was issued yet accepts its first `snapshot` whatever position is presented (C25) |
| G23 | After a restore, no row of a profile has a revision above the profile's `backup_rev`, and every write of the profile committed after the backup's moment had one; the epoch's `first_rev` is above every revision given out before; a `repush` is honoured only for a revision above the `backup_rev` and below the `first_rev`, creates a row only then, and never without the permission of C12; onto a row written since, it records the conflict of C18. The first revision is above every revision given out before also when the server's clock is behind the floor the operator passed. No call by a device runs `begin_epoch` or `replay_call`. After the restore script has run, nothing that a logged call took away after the backup is there again (a membership, an invitation, an admin's role, a share, a device that is not revoked), except where the log shows it given back; and the script has made no call that gives access (C18) |

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
| A backup is consistent to one moment | Checked by the review for current versions (physical backups); a logical dump reads its rows in one snapshot as well | Needed, and enough: what a restore lost is measured by each profile's `head_rev`, a column of the same copy as the rows (C18). **4.2 verifies** in its restore test that right after a restore no row of a profile has a `rev` above the profile's `head_rev`, and that a write committed just before the backup's moment is at or below it and one committed just after is above; and that `rev_seq` is created without a cache |
| The restored revision counter says which revisions the backup holds | False. Checked for this revision in the source of Postgres (`sequence.c`: "we pre-log a few fetches in advance", `SEQ_LOG_VALS` is 32, and after a crash as many values can be skipped) and in its manual (sequence functions: a value `nextval` gave is not taken back when the transaction aborts). So after a recovery the counter can stand up to 32 values past the last one handed out before the recovery point, and values at or below it can belong to writes the restore lost | Removed from the design. Nothing reads `rev_seq` after a restore except `begin_epoch`, which places the new epoch's revisions above it, above every `head_rev` and at the server's clock (C18) |
| A trigger on the authentication service's user table creates the personal profile | Documented (managing user data guide), with the warning that a failing trigger blocks sign-ups | Removed: `hello` creates the profile (§5.3) |
| One other device's session can be ended | Not offered. Checked (sign-out guide, by the follow-up review and again for this revision): a sign-out ends all of the user's sessions, this session, or all other sessions; none names one other session, and access tokens of an ended session stay valid until they expire. Sessions guide (checked by the follow-up review): refresh tokens do not expire, and a session lasts until its user signs out unless a time limit of a paid plan ends it | Designed around. Revocation is the device row, which every call and policy checks and which is kept for as long as the session exists (C25); "sign out everywhere" is the scope for all sessions, asked for by the agent with the user's own token. **4.2 verifies**: that the session id in the token stays the same when the token is refreshed; that `auth.sessions` loses the row at a sign-out of each scope and when the user is deleted; that the functions' owner may read `auth.sessions` and `auth.users`, which `session_ok()` and `own_address()` need; and, as the test of G15, that a session whose device row is revoked is still refused after a refresh of its token and a run of the daily job |
| A database function can delete an account in the authentication service | Not needed. An account is deleted through the service's interface for administrators from an edge function; the managing user data guide says this deletes the user's sessions and refresh tokens and that tokens already issued stay valid until they expire, which C25 covers | §11.1, C25 |
| An edge function can hash a stored file and call the database | Limits checked. Edge function limits: 256 MB of memory, 2 seconds of processor time, 150 seconds of wall-clock time on the free plan and 400 on paid ones | Files of at most 25 MiB, read into memory once and hashed with the runtime's own SHA-256; the function calls `api_service` through the REST interface with the service key (§6.11). **4.6 verifies** by committing a 25 MiB file and reading the processor time it took |
| A daily job runs in the database | Checked. Cron guide: Supabase Cron runs SQL and database functions on a schedule. The guide does not say on which plans | The job is one function that can be run any number of times. **4.2 verifies** that the project's plan has Cron; if not, any scheduler that calls the function daily will do |
| An address from an identity provider is verified | Not checked | Not assumed: the functions read `email_confirmed_at` (§5.8). **4.3 verifies** per enabled provider (§11.3) |
| The REST interface can be closed while a database is restored | Not checked | The restore procedure needs it (C18). **4.2 verifies** with the hosting plan how (network restrictions, or pausing the project) and writes it into the procedure |
| After a restore, a refresh token that was issued after the backup is refused | Unconfirmed. Sessions guide: a session is a row of `auth.sessions`, and a refresh token is used once and replaced; sign-out guide: the tokens are removed with the session's other rows. So a restored database holds the sessions and tokens of the backup, and the close-out review knows no reason why the service would honour a later token; nobody has seen it after a restore | Not relied on, either way: a device whose session is refused sets aside what it holds for other people and gives back what was lost after its next sign-in; a device whose session survives reconciles as before (D14, C18). **4.2 verifies** in its restore test: sign in on two devices, take the backup, refresh the token of one, open a session on a third, restore; then refresh each token, and call `hello` with each access token that is still valid; and writes into the procedure which devices have to sign in again |
| A session can change nothing about the account at the authentication service | False. Checked for this revision. Passwords guide: a signed-in user sets a password with `updateUser`, and the current password is asked for only if the project requires it. Password security guide: a project can require a session younger than 24 hours or a code sent to the address, and can require the current password. Identity linking guide: linking by hand is a setting of the project. No guide names a switch that turns off sign-in by password alone while links by email stay on | Designed around in Iamb's own functions: a session is accepted only when its token says that it was opened the way Iamb signs in (C25). JWT guide: every access token carries `session_id`; `amr`, the methods a session was opened with (`password`, `otp`, `magiclink`, `oauth` and others), is listed as optional. **4.2 verifies, before it builds `revoke_device`**: that every access token of Iamb's own sign-in flows carries `amr`, with which names, and still after a refresh, and sets `sign_in_methods` from that (if the claim is not dependable, `session_ok()` reads the session's methods from the authentication service's own table); that a session opened with a password names `password` after any number of refreshes; as the test of G15, with the tokens of a revoked session and again after `sign_out_everywhere`: set a password, sign in with it, call `hello`, and expect `session_refused`; and which of the settings of §11.3 the project offers, what each is by default, and whether requiring the current password keeps an account that has none from setting one |
| The authentication service can be told not to issue a token | Checked. Auth hooks guide: the custom access token hook is a database function that runs before a token is issued, is given the method the token was asked with, and can answer with an error; it is available on the free plan and the first paid one. The hook for password attempts needs a higher plan and is not used | A second lock only: `iamb_private.token_hook` refuses the method `password` (C25). **4.2 verifies** that the hook runs for a sign-in with a password and that its error stops the token; that sign-ins by link, code and identity provider and every refresh pass it; and the grants it needs. Without the hook nothing else changes |
| A database backup holds the stored files | False. Backups guide (looked up by the close-out review): a database backup does not include the objects stored through the storage interface, and a restore does not bring back objects deleted since | The restore script puts rows and objects in step, and devices offer their files again in a new epoch (C18, §6.11). **4.2 verifies** in its restore test that a file committed after the backup is readable again once the script has run, and that a file removed after the backup is asked for again |
| The function log outlives the database and carries its own time | Not checked | The restore script needs it: which calls to make again, and the floor of the new epoch (C18). **4.2 verifies** where the project keeps the functions' log lines, for how long, and that a line's time is not taken from the database; where they are kept for less time than the backups, the procedure's page says so, and what then rests on the devices' own lists (D7) |

## 12. Conflict scenarios

Each scenario gives the starting state, the steps in order, and the defined outcome: the state
of every row on the server and on every device, the history and the conflicts the server holds,
and what each device shows its user. They are written out here from
[`spec/sync/scenarios.toml`](../spec/sync/scenarios.toml), which is the source
(`python3 spec/sync/render_scenarios.py` renders this section; `--check` fails when it is out of
date). The format is documented in `crates/iamb-testkit/src/sync_scenarios.rs`.

**What the check does.** `cargo test -p iamb-testkit --test sync_scenarios` fails when there are
fewer than 30 scenarios, when an id is used twice, when a scenario does not state every row for the
server and for every device, when a case the task or one of its reviews asks for is covered by no
scenario, or when this section lacks a scenario or a rule it cites. Beyond that structure it checks
that each outcome agrees with the scenario's own steps and answers. It follows every change from the
device that made it to the answer the scenario gives for it, keeping a ledger of the version each
row has on the server and of the version each device had seen, and requires:

- the server's row is the last write that reached it, live or deleted as that write left it, or
  absent when nothing reached it or a purge is due; a refused or blocked write is never the
  server's row;
- the history holds exactly the versions that later writes replaced;
- a write that replaced a version its device had not seen, written on another device, has a
  conflict that names both, of the kind the two writes make, shown on the devices of the
  authors; a conflict that was replaced hands its versions on;
- a refused change is kept on the device that made it; a blocked one stays there with its row
  and its outbox entry; no change is left unanswered on a device that ends online;
- a device that ends online holds each row as the server has it or not at all, holds what it wrote
  itself, holds every live row of its user's own personal profile, and two such devices of one
  account hold the same;
- a device holds a row of someone else's profile only where a membership or a share reaches that
  profile for its user: one that is live on the server when the device ends online, one that the
  scenario had at some time otherwise; and a device that was signed out or revoked, and not signed
  in again, holds no version that reached the server afterwards;
- a field an outcome states for a row is what the scenario's own steps left in that version; the
  author of a snippet's content is the one C17 gives for those steps (the writer only of a step that
  changed the content, otherwise the author of the version the device held); a snippet whose content
  someone else wrote and that must be reviewed is listed as held on every device of a reader that
  has not approved it, and on no device of its author; and where several snippets a device holds
  have one abbreviation, the one stated to expand is not behind another under D13, as far as the
  scenario decides it (the author of the content, the profile, the order in which the server
  accepted the rows);
- after a restore of the server, a version the backup lacks that a device of the profile's owner
  held ends as the server's row or in its history, with the conflict a write over an unseen version
  needs; one that a device of the user who wrote it held ends there or among that device's kept
  changes, and a device whose session the restore ended, and that was not signed in again, still
  holds it; of two versions of one row that the restore lost, the later one stays, whichever is
  given back first; a removal made after the backup is made again, by the operator's replay or by
  the device that made it when it ends online, and before that device gives a share with the same
  group back; the operator's replay ends only what a lost call had ended; and only a scenario that
  restores the server shows `restored`.

The tests feed it scenarios that are wrong on purpose: the eight with which the first review showed
that a purely structural check accepts anything, the six of its own that the follow-up review saw
refused, five of the seven it saw pass (the reader made the author, a member's snippet taking the
owner's abbreviation, a device that never receives a row of its own profile, a removal that is not
made again after a restore, a folder that stays on a member's device after its only share ended),
the two sequences of that review's first finding as the design then gave them (a lost edit replaced
by a newer one, a row made after the backup taken for purged), a lost edit that its writer's device
neither gives back nor keeps, four of the eight the close-out review saw pass (the earlier of two
lost versions winning, a revoked computer that holds what was written since, `restored` shown where
nothing was restored, rows of an account the user shares nothing with), and the close-out's findings
as the design then was (a device that removes shared rows when a restore ends its session, a folder
shared after a removal that reaches the removed member, a share given back before the removal is
made again, a session opened with a password that receives rows); each must be refused.

**What it does not do, and where the line is.** The check takes from the scenario the answers
(`applied`, `rejected:forbidden`), the rows a write takes with it, and which folders of a profile a
user may see; it does not derive them, because it has no model of permissions, the feed or the
store. What it knows beyond versions is read off the scenario's own data: which fields a step wrote,
who owns a personal profile, who wrote a snippet's content, which memberships and shares there are
and so which profiles they reach, which devices were signed out and when. An outcome that is
consistent with its own answers and wrong under the rules of §8 and §9 still passes: a write the
permissions forbid that is answered `applied`, a cascade C6 does not give, a sub-folder that stays
on a member's device after it left the folder shared with them, a mark only the server sets (whether
an attachment row is backed), a link a device should not have added. The tests hold one example of
each of the first three, as examples that pass, so that nobody takes them for guarded. Of the
scenarios added for the close-out review, the outcomes of S81, S82 and S84 are therefore right by
reading only. That is checked when the two-device simulation of task 4.4 replays every scenario
against the real store and sync client and a model of the server, and compares each outcome field by
field; its 1,000 random runs check the invariants of §1 instead of fixed outcomes.

How to read one:

- Devices start signed in, online and in step with the server unless the start says otherwise. An
  online device syncs after every step that concerns it; `offline` and `online` bracket work done
  without the server; `manual` leaves the network up but lets the scenario say when the device pulls
  and pushes. A step answered `session_refused` is one at which the device finds its session no
  longer good without having chosen that; a row it holds of someone else's folder is then set aside,
  and its column still states it (D14).
- A version is named after the step that wrote it (`a1`), and `start` is what a row holds at the
  start. In an outcome, `live · b1` means the row is live in the revision the step `b1` wrote;
  `deleted · b1` that `b1` deleted it; `absent` that no row with that id is there (never
  received, purged, or not visible to that user; for an invitation: not listed for that user).
  A step that says "on the server the change also writes" names the rows the server writes
  because of it: what a folder's delete takes with it, a folder a write restores, the
  memberships an account call changes. `=` in a device's column means "as on the server".
- "Shown" is what the device shows its user, as a code: `conflict` (a conflict is listed and marked
  on the row), `kept` (changes that were not synced are kept on this computer), `blocked` (a row
  cannot be synced yet), `shared` and `revoked` (a folder arrived or went), `duplicate` and `broken`
  (library health), `review` (a snippet waits for review), `adjusted` (a folder was placed
  elsewhere), `superseded` (a sharing change was not made), `plan_limit`, `upgrade`,
  `invitation_invalid`, `refused` (an account call or a sign-in was refused, with its reason),
  `signed_out` (this device was signed out, by the server or because its session was no longer good)
  and `restored` (the server was restored from a backup; look at your groups). The wording is task
  4.5's; none of them is a notice over another application (UX spec §4.11).

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
| S68 | After a restore, a lost edit comes back although the snippet was changed since | server | restore-lost-edit-vs-new-edit |
| S69 | After a restore, a snippet made right after the backup comes back, whatever the revision counter says | server | restore-line-per-profile |
| S70 | After a restore, two devices hold different lost versions of one snippet | server | restore-two-lost-versions |
| S71 | The answer to a pull is lost twice in a row | transport | lost-pull-answers |
| S72 | The owner moves a sub-folder out of a shared folder | sharing | subfolder-leaves-share |
| S73 | A member links a row of their own to a file of the owner's whose hash they know | attachments | file-by-known-hash |
| S74 | A revoked computer is switched on again forty days later | server | revoked-device-returns |
| S75 | A changed client claims to hold more than the server | server | client-claims-newer-state |
| S76 | A member deletes a folder above the root of another share | sharing | delete-above-nested-share |
| S77 | After a restore, a member's edit in a folder that is no longer shared with them | server | restore-lost-edit-kept |
| S78 | A restore ends the devices' sessions before they have learned of it | server | restore-ends-sessions |
| S79 | After a restore, a folder that was shared after a member's removal does not reach that member | server | restore-share-after-removal |
| S80 | After a restore, a device makes a lost removal again before it gives a share back | server | restore-removal-before-share |
| S81 | A member writes the hash of the owner's file into the body of a shared snippet | attachments | file-hash-in-body |
| S82 | A sub-folder leaves a shared folder and a part of it stays shared by itself | sharing | nested-share-survives |
| S83 | A password set with a revoked computer's tokens opens nothing | server | password-session |
| S84 | After a restore, the owner's device gives back an attachment row a member wrote | attachments | restore-attachment-row |
| S85 | A conflict does not hand a member a version from the owner's private folder | sharing | conflict-version-audience |
| S86 | A removed member is told of no conflict and can close none | membership | conflict-after-removal |
| S87 | A member cannot restore a version that puts a folder where a push may not | structure | restore-version-permission |
| S88 | A version replaced in the same push that shares its folder is not handed to a member | sharing | conflict-version-same-push |
| S89 | A folder name replaced in the same push that shares the folder above it is not handed to a member | sharing | folder-name-same-push |
| S90 | What was deleted from a folder before it was shared does not reach the people it is shared with | sharing | tombstone-before-access |
| S91 | A share ends, a snippet is rewritten and deleted, and the share begins again while a member's device is offline | sharing | tombstone-after-access-ended |
| S92 | The owner deletes a tag that a shared snippet still carries, and brings it back | tags | deleted-tag-shared-snippet |
| S93 | A device comes back after a month to a folder that was unshared, changed and shared again | sharing | tombstone-resync |

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
   position belongs to the old epoch. A resyncs, and the snapshot tells it the last revision the
   backup holds of its profile. Row by row (D7), A sends again each row it holds in a later
   revision: s1 as of a2, s2, and the tombstone of s3. The re-push of s2 creates the row, which a
   re-push of a revision the restore lost may do (C13, C18). The restored rows were not written in
   the new epoch, so nothing is in conflict.*
8. B pulls. Answer: `reset_required:epoch`, `s1` `unchanged`, `s2` `unchanged`, `s3` `unchanged`. *B
   holds the same three rows above the line and sends them again as well, whatever the server holds
   by now (D7). They are already there as B holds them, so its re-pushes change nothing (C4).*

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
3. `x1` · A: the user signs out. *Signing out removes the tokens. The user's own library stays on
   the computer and keeps working, with its outbox, its revisions and the id of the account it
   belongs to. A is offline, so it cannot ask the server before it signs out: had it held folders of
   other people, they would be set aside until the next sign-in, not removed (D14).*
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

**Start.** Ines shares f1 with the family. A backup is taken. Then Ines removes Tom from the group
on her device A, which notes the call, and Tom's device purges the folder. Then the server's
database is lost and restored from the backup. The function log does not reach back to the removal,
so the restore script cannot make it again before the server opens (S79 is the case in which it
can): what is left is the list on the device.

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
2. `x1` · A: the user removes the member `m-tom`. Answer: `ok`. *The answer carries the mark of the
   call: the revision it drew under the group's lock (C16). A's agent notes the removal, with the
   group, the person and that mark, in the device's list of calls that took access away (D7).
   Nothing else on A records it: Ines's device does not read Tom's membership tombstone, so for A
   the membership is simply gone. Tom's device purges the folder.*
3. The server's database is restored from the backup `k1`. *The restored database knows nothing of
   the removal: Tom is a member again. Account calls made after the backup are lost with it, and no
   device can push a membership (C16). The server starts a new epoch (C18).*
4. C pulls. Answer: `reset_required:epoch`. *Tom's device resyncs. Rows only the server writes are
   taken as the server has them, so it holds the membership again, and the folder arrives again.*
5. A pulls. Answer: `reset_required:epoch`. *Ines's device resyncs. The snapshot gives it, for the
   group, the last revision the backup holds. The device takes Tom's membership as the server has
   it, like every row only the server writes. Then it reads its own list: the removal of Tom has a
   mark above the group's line, so the restore lost that call (D7). The device tells Ines that the
   server was restored and that she should look at her members, and does not wait for her: what took
   access away is made again.*
6. `x2` · A: the user removes the member `m-tom`. Answer: `ok`. *The device makes the call by
   itself, because its user may still make it, and notes the new mark (D7). It makes it before it
   gives anything back that the restore lost (S80). A device never repeats a call that gives access.
   Tom's device purges the folder again.*

**Outcome.** Tom is removed again as soon as the device from which he was removed has synced: it
finds the removal in its own list of calls that took access away, with a mark the restored server no
longer covers. Until then he had the folder back, which is what a restore costs when the function
log does not name the call, and what the restore procedure tells owners. Ines's device says that the
server was restored. Tom's device says that the folder is no longer shared with him.

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
   backup. It resyncs: it holds no row in a revision above the last one the backup holds of its
   profile, so it has nothing to send, and it finds every row as it holds it.*
7. A pulls. Answer: `reset_required:epoch`, `s1` `applied`, `s2` `applied`. *A goes row by row (D7):
   it holds s1 and s2 in revisions above the last one the backup holds of its profile. A sends both
   again, marked as re-pushes with the revisions it holds. Both revisions lie in the span the
   restore lost. The server takes s1 without a conflict, because nobody wrote it in the new epoch,
   and creates s2: a re-push of a lost revision is the one put with a base above 0 that may create a
   row (C13, C18).*

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
   installations hold and receives the next one, pushes, and pulls again (§6.7 step 4), which issues
   another: the server now keeps the last position it issued to A and the one A presented for it
   (C25).*
6. B comes back online and syncs. Answer: `error:device_conflict`. *B presents the position both
   installations had at the copy. It is neither of the two the server keeps for the device: two
   installations use one device id (C25). B makes a new device id and calls hello, which is refused
   as well: the session came with the copy and already has a device. B signs itself out, keeping its
   library and its outbox with b1 (D14), and asks Ines to sign in again on this computer. Nothing B
   sent was written. (Corrected in task 4.4: an earlier version had B accepted as a repeat and A
   caught; with the second pull of §6.7 the installation that is behind is the one caught.)*
7. `x1` · B: ines signs in. Answer: `s1` `applied+conflict`. *A new session and B's new device id: B
   registers as a device of its own and resyncs, which leaves s1 alone because of its unsent change
   (D7), then pushes b1, which is stale and replaces a1. a1 was written under the device id B had
   too, but not by a change B's op names as its own earlier one, so the conflict is recorded all the
   same (C3).*

**Outcome.** The second installation's edit wins by arriving last, and a conflict offers the first
one back, although both were written under one device id. The server notices the two installations
at the first call of the one that is behind, which signs itself out and comes back as a device of
its own. Nothing is replaced silently.

| Row | Server | A | B |
| --- | --- | --- | --- |
| `f1` | live · start | = | = |
| `s1` | live · b1 | = | = |

- History (replaced versions the server keeps): `s1` start, a1.
- Conflicts: `s1` edit_edit, winner b1, loser a1, shown to ines, open.
- A: shown conflict.
- B: shown conflict, signed_out.

#### S61 · The owner deletes a shared folder while a member holds an unsent change in it

Category: sharing. Covers: shared-folder-deleted. Rules: C6, C21, D2, D3, D6.

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
4. C comes back online and syncs. *C pulls first. Its feed holds the rows the delete wrote, each
   delivered as null because Tom can no longer read them, and folder_revoked for f1. The null row
   for s1 is not applied, because s1 has an unsent change (D2). The null row for f1 is a null row
   for a folder: the store handles it as a purge of that folder (D3), keeps the unsent change for
   the user and removes the tree in one step, where removing the folder row alone would fail on the
   snippet it still holds. folder_revoked then finds nothing left (D6). No page is refused, nothing
   is resynced, and nothing is left to push.*

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

#### S68 · After a restore, a lost edit comes back although the snippet was changed since

Category: server. Covers: restore-lost-edit-vs-new-edit. Rules: C3, C18, D2, D7.

**Start.** Ines's snippet s1 is in step on her devices A and B. A backup is taken and B goes
offline. A edits s1, and the edit is synced. Then the server's database is lost and restored from
the backup. B comes back first and edits s1; then A comes back.

Devices: A (ines), B (ines). Profiles: `p-ines` (personal, ines).

| Row | Kind | Refers to | Rev | Fields |
| --- | --- | --- | --- | --- |
| `f1` | folder | profile `p-ines` | 3 | name `Replies` |
| `s1` | snippet | folder `f1` | 40 | abbreviation `;rty`, body `Thanks for your order.` |

**Steps.**

1. `k1` · A backup of the server's database is taken.
2. B goes offline.
3. `a1` · A edits `s1`: body `Thank you for your order.`. Answer: `s1` `applied`. *Acknowledged by
   the server, in a revision after the backup.*
4. The server's database is restored from the backup `k1`. *The restored database holds s1 as of the
   start. For Ines's profile, the last revision it holds is the one the profile's row recorded
   before the backup (C18).*
5. B comes back online and syncs. Answer: `reset_required:epoch`. *B was offline since before the
   backup. It holds nothing above the line of its profile and finds every row as it holds it.*
6. `b1` · B edits `s1`: body `Thanks a lot for your order.`. Answer: `s1` `applied`. *An ordinary
   write in the new epoch, based on the revision the restored server holds: not stale, no conflict.
   Neither B nor the server can know that A holds an edit the restore lost.*
7. A pulls. Answer: `reset_required:epoch`, `s1` `applied+conflict`. *A resyncs. It holds s1 in a
   revision above the line the snapshot gives for the profile: a write the restore lost. A does not
   take b1 over it, although b1 has the higher revision; it leaves its row as it is and sends it
   again as a re-push (D2, D7). On the server the row was written since the restore, so the re-push
   is an ordinary stale write: it wins, and a conflict offers b1 back (C18, C3). Taking b1 because
   it is newer would have replaced an acknowledged edit unseen, and left it nowhere.*

**Outcome.** The edit the restore lost is the snippet again, and the edit made after the restore is
kept in the history and offered back by a conflict on both devices. Neither is lost, and Ines
chooses.

| Row | Server | A | B |
| --- | --- | --- | --- |
| `f1` | live · start | = | = |
| `s1` | live · a1 (body `Thank you for your order.`) | = | = |

- History (replaced versions the server keeps): `s1` start, b1.
- Conflicts: `s1` edit_edit, winner a1, loser b1, shown to ines, open.
- A: shown conflict.
- B: shown conflict.

#### S69 · After a restore, a snippet made right after the backup comes back, whatever the revision counter says

Category: server. Covers: restore-line-per-profile. Rules: C13, C18, C24, D7.

**Start.** Ines and Tom each have their own library and share nothing. Tom edits a snippet of his; a
backup is taken; then Ines creates the snippet s2, which is synced. Then the server's database is
lost and restored from the backup.

Devices: A (ines), C (tom). Profiles: `p-ines` (personal, ines), `p-tom` (personal, tom).

| Row | Kind | Refers to | Rev | Fields |
| --- | --- | --- | --- | --- |
| `f1` | folder | profile `p-ines` | 3 | name `Replies` |
| `s1` | snippet | folder `f1` | 40 | abbreviation `;rty`, body `Thanks for your order.` |
| `tf1` | folder | profile `p-tom` | 7 | name `Shop` |
| `ts1` | snippet | folder `tf1` | 41 | abbreviation `;hrs`, body `Open 9 to 5.` |

**Steps.**

1. `c1` · C edits `ts1`: body `Open 9 to 6.`. Answer: `ts1` `applied`. *A write in Tom's profile.
   Its revision is above every revision of Ines's profile, whose row still records 40 as its last.*
2. `k1` · A backup of the server's database is taken.
3. `a1` · A creates the snippet `s2` (folder `f1`): abbreviation `;rlate`, body `Your order left us
   late.`. Answer: `s2` `applied`. *Written after the backup, in the next revision after c1's. That
   revision is above the last one the backup holds of Ines's profile. It is not above the value the
   revision counter has in the restored database: a sequence is logged up to 32 values ahead, so the
   restored counter stands past revisions that were given out after the backup's moment.*
4. The server's database is restored from the backup `k1`.
5. A pulls. Answer: `reset_required:epoch`, `s2` `applied`. *The snapshot gives A the line of each
   profile it reads: for p-ines, the last revision the restored database holds of that profile,
   which its own row says (C18, C24). s2 lies above it, so it is a write the restore lost: A sends
   it again, and the re-push creates the row (C13). One line for the whole server, read from the
   restored counter, would lie above s2's revision; A would take the snippet for one that was
   deleted and purged, and remove it.*
6. C pulls. Answer: `reset_required:epoch`. *Tom's device holds nothing above the line of his
   profile: his edit is in the backup.*

**Outcome.** The snippet Ines made right after the backup is on the server and on her device again,
and Tom's edit, which the backup holds, is untouched. What a restore lost is measured per profile,
by the revision the profile's own row recorded, and not by the revision counter.

| Row | Server | A | C |
| --- | --- | --- | --- |
| `f1` | live · start | = | absent |
| `s1` | live · start | = | absent |
| `tf1` | live · start | absent | = |
| `ts1` | live · c1 | absent | = |
| `s2` | live · a1 | = | absent |

- History (replaced versions the server keeps): `ts1` start.
- Conflicts: none.
- A: shown nothing.
- C: shown nothing.

#### S70 · After a restore, two devices hold different lost versions of one snippet

Category: server. Covers: restore-two-lost-versions. Rules: C3, C18, D7.

**Start.** After a backup, Ines edits s1 on device B, which then goes offline, and edits it
differently on device A, which had been offline and had not seen B's edit: A's edit wins, with a
conflict. Then the server's database is lost and restored from the backup, and with it the conflict.
B comes back first.

Devices: A (ines), B (ines). Profiles: `p-ines` (personal, ines).

| Row | Kind | Refers to | Rev | Fields |
| --- | --- | --- | --- | --- |
| `f1` | folder | profile `p-ines` | 3 | name `Replies` |
| `s1` | snippet | folder `f1` | 40 | abbreviation `;rty`, body `Thanks for your order.` |

**Steps.**

1. `k1` · A backup of the server's database is taken.
2. A goes offline.
3. `b1` · B edits `s1`: body `Thanks a lot for your order.`. Answer: `s1` `applied`.
4. B goes offline.
5. `a1` · A edits `s1`: body `Thank you for your order.`.
6. A comes back online and syncs. Answer: `s1` `applied+conflict`. *A's edit was made without b1: it
   is stale, it wins, and a conflict offers b1 back (C3). B is offline and still holds b1 as the
   snippet.*
7. The server's database is restored from the backup `k1`. *The restore loses both edits, the
   history of s1 and the conflict.*
8. B comes back online and syncs. Answer: `reset_required:epoch`, `s1` `applied`. *B holds b1, in a
   revision above the line of the profile, and sends it again. The row has not been written since
   the restore: the re-push is applied without a conflict (C18).*
9. A pulls. Answer: `reset_required:epoch`, `s1` `applied+conflict`. *A holds a1, a later revision
   of the same lost history, and sends it again whatever the server holds (D7). The row was written
   since by a re-push of an earlier lost revision: the later one is the row, and the earlier one
   goes to the history. The two differ, and the server no longer knows whether a1 was written with
   b1 in view, so a conflict offers b1 back (C18). Had A come back first, B's re-push would have
   been answered superseded, and b1 would be in the history, under its lost revision, with the same
   conflict.*

**Outcome.** Both lost versions come back: the later one is the snippet, the earlier one is in the
history, and the conflict the restore had lost is there again on both devices. A restore loses
nothing that a device still holds.

| Row | Server | A | B |
| --- | --- | --- | --- |
| `f1` | live · start | = | = |
| `s1` | live · a1 | = | = |

- History (replaced versions the server keeps): `s1` start, b1.
- Conflicts: `s1` edit_edit, winner a1, loser b1, shown to ines, open.
- A: shown conflict.
- B: shown conflict.

#### S71 · The answer to a pull is lost twice in a row

Category: transport. Covers: lost-pull-answers. Rules: C25, D11.

**Start.** Ines's device A is on a bad connection. Device B edits s1. A pulls three times before an
answer reaches it.

Devices: A (ines), B (ines). Profiles: `p-ines` (personal, ines).

| Row | Kind | Refers to | Rev | Fields |
| --- | --- | --- | --- | --- |
| `f1` | folder | profile `p-ines` | 3 | name `Replies` |
| `s1` | snippet | folder `f1` | 5 | abbreviation `;rty`, label `Thanks`, body `Thanks for your order.` |

**Steps.**

1. A stays connected but syncs only when a step says so. *So that each of A's calls is a step.*
2. `b1` · B edits `s1`: body `Thanks a lot for your order.`. Answer: `s1` `applied`.
3. A pulls. Answer: `lost`. *A presents the position it holds, P. The server issues the next
   position and keeps two for the device: the one it issued last, and P, the one that was presented
   for it. The answer does not arrive.*
4. A pulls. Answer: `lost`. *A presents P again: a repeat. The server issues another position; the
   presented one is still P, and only the last one is replaced (C25). A rule that kept the last two
   positions issued would now hold neither of them as P.*
5. A pulls. *P a third time, and it is accepted like the second: a position presented again is no
   copy of the device, however often. This answer arrives. A applies b1 and stores the new position
   with it (D11).*
6. `a1` · A edits `s1`: label `Thanks, short`.
7. A pushes. Answer: `s1` `applied`. *The push presents the position A now holds, the one the server
   issued last.*
8. A syncs by itself again.

**Outcome.** Nothing happens to the device whose answers were lost: it is not taken for a second
installation, is not signed out, and syncs on. Both devices hold b1's content with a1's label, and
nothing is shown.

| Row | Server | A | B |
| --- | --- | --- | --- |
| `f1` | live · start | = | = |
| `s1` | live · a1 (label `Thanks, short`, body `Thanks a lot for your order.`) | = | = |

- History (replaced versions the server keeps): `s1` start, b1.
- Conflicts: none.
- A: shown nothing.
- B: shown nothing.

#### S72 · The owner moves a sub-folder out of a shared folder

Category: sharing. Covers: subfolder-leaves-share. Rules: C21, D2, D3, D6.

**Start.** Ines shares f1 with the family, to change. It has a sub-folder f1b with the snippet s2.
Tom's device is offline and he edits s2. Ines moves the sub-folder into her private folder f2.

Devices: A (ines), C (tom). Profiles: `p-ines` (personal, ines), `p-tom` (personal, tom), `g-fam`
(family, ines).

| Row | Kind | Refers to | Rev | Fields |
| --- | --- | --- | --- | --- |
| `m-ines` | membership | group `g-fam`, user `ines` | 2 | role `owner` |
| `m-tom` | membership | group `g-fam`, user `tom` | 6 | role `member` |
| `f1` | folder | profile `p-ines` | 3 | name `Shop replies` |
| `f1b` | folder | profile `p-ines`, parent `f1` | 4 | name `Returns` |
| `f2` | folder | profile `p-ines` | 8 | name `Private` |
| `s1` | snippet | folder `f1` | 5 | abbreviation `;hrs`, body `Open 9 to 5.` |
| `s2` | snippet | folder `f1b` | 9 | abbreviation `;rret`, body `Return label attached.` |
| `sh1` | folder share | folder `f1`, group `g-fam` | 7 | permission `edit` |

**Steps.**

1. C goes offline.
2. `c1` · C edits `s2`: body `Return label attached. Thank you.`.
3. `a1` · A moves `f1b` to `f2`. Answer: `f1b` `applied`. *The move writes one row, f1b. Its snippet
   is not written. For Tom the tree under f1b stops being readable: his feed gets the row f1b,
   delivered as null, and folder_revoked for f1b (C21).*
4. C comes back online and syncs. *C applies the page's rows first. The null row is one for a
   folder, so the store ends that tree as a purge does: the unsent change to s2 goes to the list of
   kept changes, then f1b and s2 are removed together (D3). Removing the folder row alone would
   fail, because the store does not let a folder go while it holds a snippet, and the page would be
   refused each time until the device resynced. folder_revoked finds nothing left (D6). The rest of
   the shared folder stays as it is.*

**Outcome.** The sub-folder and its snippet are in Ines's private folder, unchanged. They leave
Tom's device in one step, with no refused page and no resync; his device says that the folder is no
longer shared with him and keeps his unsent edit in the list of changes that were not synced. The
shared folder itself stays.

| Row | Server | A | C |
| --- | --- | --- | --- |
| `m-ines` | live · start | = | = |
| `m-tom` | live · start | = | = |
| `f1` | live · start | = | = |
| `f1b` | live · a1 (parent `f2`) | = | absent |
| `f2` | live · start | = | absent |
| `s1` | live · start | = | = |
| `s2` | live · start | = | absent |
| `sh1` | live · start | = | = |

- History (replaced versions the server keeps): `f1b` start.
- Conflicts: none.
- A: shown nothing.
- C: shown revoked, kept; kept for the user: c1; permission `f1` edit, `f1b` none.

#### S73 · A member links a row of their own to a file of the owner's whose hash they know

Category: attachments. Covers: file-by-known-hash. Rules: C12, C19.

**Start.** Ines shares f1 with the family, to change. Her private snippet s3, in f2, uses an image
whose stored file Tom cannot read. Tom knows the file's hash. He writes an attachment row with that
hash and links it from the shared snippet s1.

Devices: A (ines), C (tom). Profiles: `p-ines` (personal, ines), `p-tom` (personal, tom), `g-fam`
(family, ines).

| Row | Kind | Refers to | Rev | Fields |
| --- | --- | --- | --- | --- |
| `m-ines` | membership | group `g-fam`, user `ines` | 2 | role `owner` |
| `m-tom` | membership | group `g-fam`, user `tom` | 6 | role `member` |
| `f1` | folder | profile `p-ines` | 3 | name `Shop replies` |
| `f2` | folder | profile `p-ines` | 5 | name `Private` |
| `s1` | snippet | folder `f1` | 7 | abbreviation `;hrs`, kind `rich`, body `Open 9 to 5.` |
| `s3` | snippet | folder `f2` | 9 | abbreviation `;iban`, kind `rich`, body `My account: ![card](attachment:sha-card)` |
| `at2` | attachment | profile `p-ines` | 17 | sha256 `sha-card`, backed `true` |
| `la2` | snippet attachment | snippet `s3`, attachment `at2` | 19 |  |
| `sh1` | folder share | folder `f1`, group `g-fam` | 20 | permission `edit` |

**Steps.**

1. `c1` · C creates the attachment `at9` (profile `p-ines`): sha256 `sha-card`. Answer: `at9`
   `applied`. *Tom may create an attachment row in the profile, because he may change a folder of
   it. The row names the hash of a file he does not have. The server marks a new row as backed only
   when its creator can read the verified file at that moment; Tom cannot, so at9 is not backed
   (C19). The answer says that the file is missing, exactly as for a file nobody has.*
2. `c2` · C creates the snippet attachment `la9` (snippet `s1`, attachment `at9`). Answer: `la9`
   `applied`. *Allowed as well: s1 is in the shared folder and at9 is in the folder's profile (C12).
   A snippet Tom can read now has a live link to a row with the hash of Ines's file. That gives him
   nothing: a link counts for a file only when its attachment row is backed. A request for the
   stored file answers as for a file that does not exist, as it did before, and the call that opens
   an upload does not say that the file exists. If Tom had the bytes he would upload them; the
   commit would find the file, mark his row as backed, and the link would count from then on.*

**Outcome.** The row and the link exist and give no file to anyone who could not read it before.
Tom's device holds his two rows and nothing of f2: not the snippet, not Ines's attachment row, not
the file, and no answer tells him whether Ines has a file with that hash.

| Row | Server | A | C |
| --- | --- | --- | --- |
| `m-ines` | live · start | = | = |
| `m-tom` | live · start | = | = |
| `f1` | live · start | = | = |
| `f2` | live · start | = | absent |
| `s1` | live · start | = | = |
| `s3` | live · start | = | absent |
| `at2` | live · start | = | absent |
| `la2` | live · start | = | absent |
| `sh1` | live · start | = | = |
| `at9` | live · c1 (backed `false`) | = | = |
| `la9` | live · c2 | = | = |

- History (replaced versions the server keeps): none.
- Conflicts: none.
- A: shown nothing.
- C: shown nothing; permission `f1` edit.

#### S74 · A revoked computer is switched on again forty days later

Category: server. Covers: revoked-device-returns. Rules: C25, D14.

**Start.** Ines's second computer, B, is lost while it is switched off. She revokes it from her
first computer and goes on working. Forty days later B is switched on, with the session and the
refresh token it had.

Devices: A (ines), B (ines). Profiles: `p-ines` (personal, ines).

| Row | Kind | Refers to | Rev | Fields |
| --- | --- | --- | --- | --- |
| `f1` | folder | profile `p-ines` | 3 | name `Replies` |
| `s1` | snippet | folder `f1` | 5 | abbreviation `;rty`, body `Thanks for your order.` |

**Steps.**

1. B goes offline.
2. A: the user revokes the device B. Answer: `ok`. *B's device row is marked revoked. B's session
   stays where it is, in the authentication service: nothing there ends one other session, and its
   refresh token does not expire (C25).*
3. `a1` · A edits `s1`: body `Thank you for your order.`. Answer: `s1` `applied`.
4. 40 days pass. *The daily job has run forty times. It keeps B's revoked row, because the row's
   session still exists; a revoked row goes only when its session is gone.*
5. B comes back online and syncs. Answer: `device_revoked`. *B refreshes its token, which the
   authentication service allows, and calls hello. The session has a revoked row: refused, as it
   would have been on the first day. Had the row been removed after 30 days, hello would have found
   a session without a device and registered B again, and B would have read the whole account. B
   signs itself out (D14), which ends the session; only then does the daily job remove the row.*

**Outcome.** The revoked computer is refused however late it comes back, and receives nothing that
was written since. It signs itself out and says so; what it held of Ines's own library is still on
it, as on any computer without an account.

| Row | Server | A | B |
| --- | --- | --- | --- |
| `f1` | live · start | = | = |
| `s1` | live · a1 | = | live · start |

- History (replaced versions the server keeps): `s1` start.
- Conflicts: none.
- A: shown nothing.
- B: shown signed_out.

#### S75 · A changed client claims to hold more than the server

Category: server. Covers: client-claims-newer-state. Rules: C18, D12.

**Start.** Ines has two devices in step. Tom, who has an account of his own and shares nothing with
her, runs a changed client. It reports a revision far above any the server ever gave out, and a
position ahead of its feed.

Devices: A (ines), B (ines), C (tom). Profiles: `p-ines` (personal, ines), `p-tom` (personal, tom).

| Row | Kind | Refers to | Rev | Fields |
| --- | --- | --- | --- | --- |
| `f1` | folder | profile `p-ines` | 3 | name `Replies` |
| `s1` | snippet | folder `f1` | 5 | abbreviation `;rty`, body `Thanks for your order.` |
| `tf1` | folder | profile `p-tom` | 7 | name `Shop` |
| `ts1` | snippet | folder `tf1` | 8 | abbreviation `;hrs`, body `Open 9 to 5.` |

**Steps.**

1. C pulls. Answer: `error:server_behind`. *Both numbers are what a device would send after the
   database was restored without its procedure, and both are numbers a client makes up as it likes.
   The server does not decide which it is. It answers this device server_behind, writes a line to
   its log for the operator, and changes nothing: no epoch begins, no feed is emptied, no other
   device's position is forgotten (C18). An honest device answered like this stops syncing, keeps
   its library and its outbox, and tries hello again every hour (D12).*
2. `a1` · A edits `s1`: body `Thank you for your order.`. Answer: `s1` `applied`. *Ines's devices
   sync as before. B receives a1 through its feed, from the position it held: nothing was reset.*

**Outcome.** The only device that stops is the one that made the claim. Ines's devices are not made
to read their library again; her edit goes from one to the other as on any other day, and nothing is
shown on them.

| Row | Server | A | B | C |
| --- | --- | --- | --- | --- |
| `f1` | live · start | = | = | absent |
| `s1` | live · a1 | = | = | absent |
| `tf1` | live · start | absent | absent | = |
| `ts1` | live · start | absent | absent | = |

- History (replaced versions the server keeps): `s1` start.
- Conflicts: none.
- A: shown nothing.
- B: shown nothing.
- C: shown nothing.

#### S76 · A member deletes a folder above the root of another share

Category: sharing. Covers: delete-above-nested-share. Rules: C6, C12, D5.

**Start.** Ines shares f1 with the family, to change. Deep inside it, the folder f1c is shared with
her team as well, to use. Tom, a member of the family and not of the team, deletes the sub-folder
f1b, which holds f1c.

Devices: A (ines), C (tom), E (jun). Profiles: `p-ines` (personal, ines), `p-tom` (personal, tom),
`p-jun` (personal, jun), `g-fam` (family, ines), `g-team` (company, ines).

| Row | Kind | Refers to | Rev | Fields |
| --- | --- | --- | --- | --- |
| `m-ines` | membership | group `g-fam`, user `ines` | 2 | role `owner` |
| `m-tom` | membership | group `g-fam`, user `tom` | 6 | role `member` |
| `m-ines-t` | membership | group `g-team`, user `ines` | 10 | role `owner` |
| `m-jun` | membership | group `g-team`, user `jun` | 11 | role `member` |
| `f1` | folder | profile `p-ines` | 3 | name `Shop replies` |
| `f1b` | folder | profile `p-ines`, parent `f1` | 4 | name `Returns` |
| `f1c` | folder | profile `p-ines`, parent `f1b` | 5 | name `Labels` |
| `s2` | snippet | folder `f1b` | 8 | abbreviation `;rret`, body `Return label attached.` |
| `s3` | snippet | folder `f1c` | 9 | abbreviation `;lbl`, body `Print the label at home.` |
| `sh1` | folder share | folder `f1`, group `g-fam` | 7 | permission `edit` |
| `sh2` | folder share | folder `f1c`, group `g-team` | 12 | permission `view` |

**Steps.**

1. `c1` · C deletes `f1b`. Answer: `f1b` `rejected:forbidden`. *On C the delete takes f1c, s2 and s3
   with it. Tom may change what is in f1, and f1b is a sub-folder like any other. But below it lies
   f1c, the root of the share with the team, and deleting f1b would delete that share with it. A
   share is ended only by who may end it: the delete needs manage on f1c, which a member never has,
   so the whole delete is refused and nothing is deleted (C6, C12). The answer has an entry for
   every row Tom's device had deleted with the folder; the device puts all four back and lists the
   delete as a change that was not made (D5).*

**Outcome.** Nothing is deleted, and the team keeps its folder. On Tom's device the sub-folder and
everything in it are back, and the list of changes that were not synced says that his deletion of
"Returns" was not made. A member who may change a shared folder cannot end a share by deleting a
folder above it.

| Row | Server | A | C | E |
| --- | --- | --- | --- | --- |
| `m-ines` | live · start | = | = | absent |
| `m-tom` | live · start | = | = | absent |
| `m-ines-t` | live · start | = | absent | = |
| `m-jun` | live · start | = | absent | = |
| `f1` | live · start | = | = | absent |
| `f1b` | live · start | = | = | absent |
| `f1c` | live · start | = | = | = |
| `s2` | live · start | = | = | absent |
| `s3` | live · start | = | = | = |
| `sh1` | live · start | = | = | absent |
| `sh2` | live · start | = | absent | = |

- History (replaced versions the server keeps): none.
- Conflicts: none.
- A: shown nothing.
- C: shown kept; kept for the user: c1; permission `f1` edit.
- E: shown nothing; permission `f1c` view.

#### S77 · After a restore, a member's edit in a folder that is no longer shared with them

Category: server. Covers: restore-lost-edit-kept. Rules: C11, C18, D6, D7.

**Start.** After a backup, Ines shares f1 with the family, to change, and her device goes offline.
Tom edits s1 in the folder, and the edit is synced. Then the server's database is lost and restored
from the backup, which knows nothing of the share. Tom's device comes back first, then Ines's.

Devices: A (ines), C (tom). Profiles: `p-ines` (personal, ines), `p-tom` (personal, tom), `g-fam`
(family, ines).

| Row | Kind | Refers to | Rev | Fields |
| --- | --- | --- | --- | --- |
| `m-ines` | membership | group `g-fam`, user `ines` | 2 | role `owner` |
| `m-tom` | membership | group `g-fam`, user `tom` | 6 | role `member` |
| `f1` | folder | profile `p-ines` | 3 | name `Shop replies` |
| `s1` | snippet | folder `f1` | 5 | abbreviation `;hrs`, body `Open 9 to 5.` |

**Steps.**

1. `k1` · A backup of the server's database is taken.
2. `a1` · A creates the folder share `sh1` (folder `f1`, group `g-fam`): permission `edit`. Answer:
   `sh1` `applied`. *After the backup. Tom's device receives the folder.*
3. A goes offline.
4. `c1` · C edits `s1`: body `Open 9 to 6.`. Answer: `s1` `applied`. *Acknowledged by the server, in
   a revision after the backup. A is offline and does not receive it: Tom's device is the only one
   that holds this version.*
5. The server's database is restored from the backup `k1`. *The restored database holds s1 as of the
   start and no share: for Tom, nothing of Ines's exists there any more.*
6. C pulls. Answer: `reset_required:epoch`. *Tom's device resyncs. The snapshot holds nothing of
   Ines's profile and names no line for it, so everything the device holds of that profile counts as
   lost (D7). s1 is a row Tom could write, in a version he wrote himself: the store leaves it and
   queues it to be sent again. The resync ends with the access check: f1 is not among the folders
   Tom may read, so it is purged, and the purge keeps the queued change for the user, because Tom
   wrote that version (D6, D7). Nothing is sent; the server would have refused it. The share row,
   which Ines wrote, leaves with the folder and is kept nowhere: it is hers to give back. Had the
   restore ended the device's session before this pull, the folder would have been set aside and not
   removed, and the resync after Tom's next sign-in would have taken exactly these steps (D14,
   S78).*
7. A comes back online and syncs. Answer: `reset_required:epoch`, `sh1` `applied`. *Ines's device
   holds the share in a revision above the line of her profile and sends it again. The server does
   not know the row, so the re-push creates it, and Ines may share the folder (C18, C11). The folder
   is shared again as she had it, and Tom's device fetches it with s1 as the server holds it. A
   never held c1, so no device can give that edit back to the server: it stays in Tom's list of
   changes that were not synced, from which he copies it into the snippet again if he wants it.*

**Outcome.** Tom's edit is not lost and not silently dropped: his device keeps it in the list of
changes that were not synced, because the restored server no longer let him write the snippet when
his device came back. The share returns when Ines's device gives it back, and Tom has the folder
again with the snippet as the backup had it. Ines's device says that the server was restored; Tom's
says that the folder went and came back, and that a change of his was kept.

| Row | Server | A | C |
| --- | --- | --- | --- |
| `m-ines` | live · start | = | = |
| `m-tom` | live · start | = | = |
| `f1` | live · start | = | = |
| `s1` | live · start (body `Open 9 to 5.`) | = | = |
| `sh1` | live · a1 | = | = |

- History (replaced versions the server keeps): none.
- Conflicts: none.
- A: shown restored.
- C: shown shared, revoked, kept; kept for the user: c1; permission `f1` edit.

#### S78 · A restore ends the devices' sessions before they have learned of it

Category: server. Covers: restore-ends-sessions. Rules: C18, C25, D7, D14.

**Start.** The team owns the folder gf1; Ines, its owner, and Tom may change it. A backup is taken.
Tom edits gs1, and the edit is synced to Ines's device. Then the server's database is lost and
restored from the backup, which also puts back the sessions and refresh tokens of that moment: the
tokens the two devices hold by now are younger than the backup.

Devices: A (ines), C (tom). Profiles: `p-ines` (personal, ines), `p-tom` (personal, tom), `g-team`
(company, ines).

| Row | Kind | Refers to | Rev | Fields |
| --- | --- | --- | --- | --- |
| `m-ines` | membership | group `g-team`, user `ines` | 2 | role `owner` |
| `m-tom` | membership | group `g-team`, user `tom` | 6 | role `member` |
| `gf1` | folder | profile `g-team` | 3 | name `Team replies`, member_permission `edit` |
| `gs1` | snippet | folder `gf1` | 5 | abbreviation `;hrs`, body `Open 9 to 5.` |

**Steps.**

1. `k1` · A backup of the server's database is taken.
2. `c1` · C edits `gs1`: body `Open 9 to 6.`. Answer: `gs1` `applied`. *Acknowledged by the server,
   in a revision after the backup. Ines's device receives it: two devices hold this version, and the
   server will not.*
3. The server's database is restored from the backup `k1`. *The restored database holds gs1 as of
   the start. It also holds the sessions as of the backup: a refresh token is used once and
   replaced, so the one each device holds now was issued after the backup, and the restored database
   never issued it.*
4. C pulls. Answer: `session_refused`. *C's access token has expired. It asks the authentication
   service for a new one and is refused: for C the session has ended, without a word from Iamb's
   server. C signs itself out. It cannot know whether the server still holds what it acknowledged,
   so it removes nothing: the team's folder is set aside, unused and unseen, with gs1 as Tom wrote
   it (D14). A device that removed the folder here would remove one of the two copies of c1 that
   exist; when the other device does the same, the edit is nowhere.*
5. A pulls. Answer: `session_refused`. *The same on Ines's device. It keeps what it holds of the
   team, set aside, until she signs in again.*
6. `x1` · C: tom signs in. Answer: `reset_required:epoch`, `gs1` `applied`. *Tom signs in again. The
   resync presents the position the device kept, and the server names another epoch: it was restored
   (C25). What was set aside takes part in the reconcile as what the device holds. gs1 lies above
   the line of the team's profile, C may write it and Tom wrote that version: C sends it again, and
   the server, which has not written the row since the restore, applies it without a conflict (C18).
   When the resync is done nothing is set aside any more.*

**Outcome.** Tom's edit is the snippet again: his device kept the team's folder when the restore
ended its session, and gave the edit back at his next sign-in. Ines's device, which nobody has
signed in on again, still holds the folder, set aside; it will find the server in step with it when
she does. Both devices say that they were signed out.

| Row | Server | A | C |
| --- | --- | --- | --- |
| `m-ines` | live · start | = | = |
| `m-tom` | live · start | = | = |
| `gf1` | live · start | = | = |
| `gs1` | live · c1 (body `Open 9 to 6.`) | = | = |

- History (replaced versions the server keeps): `gs1` start.
- Conflicts: none.
- A: shown signed_out.
- C: shown signed_out.

#### S79 · After a restore, a folder that was shared after a member's removal does not reach that member

Category: server. Covers: restore-share-after-removal. Rules: C16, C18, D7.

**Start.** Ines owns the family and shares f1 with it; Tom is a member. A backup is taken. On her
device A, Ines removes Tom. On her device B she then shares f9 with the family, a folder Tom has
never been able to read. Then the server's database is lost and restored from the backup, in which
Tom is a member. The function log, kept outside the database, names the removal.

Devices: A (ines), B (ines), C (tom). Profiles: `p-ines` (personal, ines), `p-tom` (personal, tom),
`g-fam` (family, ines).

| Row | Kind | Refers to | Rev | Fields |
| --- | --- | --- | --- | --- |
| `m-ines` | membership | group `g-fam`, user `ines` | 2 | role `owner` |
| `m-tom` | membership | group `g-fam`, user `tom` | 6 | role `member` |
| `f1` | folder | profile `p-ines` | 3 | name `Shop replies` |
| `s1` | snippet | folder `f1` | 5 | abbreviation `;hrs`, body `Open 9 to 5.` |
| `sh1` | folder share | folder `f1`, group `g-fam` | 7 | permission `view` |
| `f9` | folder | profile `p-ines` | 8 | name `Family only` |
| `s9` | snippet | folder `f9` | 9 | abbreviation `;key`, body `The key is under the blue pot.` |

**Steps.**

1. `k1` · A backup of the server's database is taken.
2. `x1` · A: the user removes the member `m-tom`. Answer: `ok`. *A notes the call in its list, with
   the mark the server answers (D7). The function log records the call, its caller, the group, the
   person and the mark (§11.1). Tom's device purges f1.*
3. `b1` · B creates the folder share `sh9` (folder `f9`, group `g-fam`): permission `view`. Answer:
   `sh9` `applied`. *On Ines's other device, after the removal: the family is Ines alone, and B
   shows it so.*
4. The server's database is restored from the backup `k1`. *In the restored database Tom is a member
   and f9 is not shared.*
5. `o1` · The operator's restore script makes again, from the function log, the calls that took
   access away after the backup. On the server the change also writes `m-tom`. *This is a step of
   the restore procedure, before the server is opened: each call is made as its caller and under the
   call's own rules (C18). Tom is removed again before any device is let in. The script makes no
   call that gives access.*
6. B pulls. Answer: `reset_required:epoch`, `sh9` `applied`. *B comes back first. It holds the share
   in a revision above the line of Ines's profile and gives it back; the server creates it (C18). B
   has no entry for the removal in its own list, because the call was made on A, and no rule on B
   could have held the share back. It does not matter: Tom is not a member. Without the operator's
   step the folder would be on Tom's device now, until A had synced.*
7. C pulls. Answer: `reset_required:epoch`. *Tom's device resyncs and finds his membership ended. It
   holds nothing of the family, as before the restore.*
8. A pulls. Answer: `reset_required:epoch`. *A finds the removal in its list with a mark above the
   group's line and makes the call again; the server finds Tom removed already, answers as it did
   the first time and writes nothing (§5.14). The list is the net for a log that was lost (S58,
   S80).*

**Outcome.** The folder Ines shared after she had removed Tom is shared with the family again and
never reaches him: he was removed again, from the function log, before the restored server let any
device in. Ines's devices say that the server was restored; Tom's device holds nothing of the family
and says, as it did when he was removed, that the folder is no longer shared with him.

| Row | Server | A | B | C |
| --- | --- | --- | --- | --- |
| `m-ines` | live · start | = | = | absent |
| `m-tom` | deleted · o1 | absent | absent | = |
| `f1` | live · start | = | = | absent |
| `s1` | live · start | = | = | absent |
| `sh1` | live · start | = | = | absent |
| `f9` | live · start | = | = | absent |
| `s9` | live · start | = | = | absent |
| `sh9` | live · b1 | = | = | absent |

- History (replaced versions the server keeps): `m-tom` start.
- Conflicts: none.
- A: shown restored.
- B: shown restored.
- C: shown revoked; permission `f1` none, `f9` none.

#### S80 · After a restore, a device makes a lost removal again before it gives a share back

Category: server. Covers: restore-removal-before-share. Rules: C16, C18, D7.

**Start.** Ines owns the family; Tom is a member. A backup is taken. On her device A, Ines removes
Tom and then shares f9 with the family. Then the server's database is lost and restored from the
backup. The function log of those hours is lost as well, so the restore script has nothing to make
again: what is left is the list on the device.

Devices: A (ines), C (tom). Profiles: `p-ines` (personal, ines), `p-tom` (personal, tom), `g-fam`
(family, ines).

| Row | Kind | Refers to | Rev | Fields |
| --- | --- | --- | --- | --- |
| `m-ines` | membership | group `g-fam`, user `ines` | 2 | role `owner` |
| `m-tom` | membership | group `g-fam`, user `tom` | 6 | role `member` |
| `f9` | folder | profile `p-ines` | 8 | name `Family only` |
| `s9` | snippet | folder `f9` | 9 | abbreviation `;key`, body `The key is under the blue pot.` |

**Steps.**

1. `k1` · A backup of the server's database is taken.
2. `x1` · A: the user removes the member `m-tom`. Answer: `ok`. *A notes the call in its list before
   it sends it, and the mark when it is answered (D7).*
3. `a1` · A creates the folder share `sh9` (folder `f9`, group `g-fam`): permission `view`. Answer:
   `sh9` `applied`.
4. The server's database is restored from the backup `k1`. *Tom is a member again, and f9 is not
   shared.*
5. C pulls. Answer: `reset_required:epoch`. *Tom's device holds his membership again. Nothing is
   shared with the family in the restored database, so nothing arrives.*
6. A pulls. Answer: `reset_required:epoch`. *A resyncs. It holds the share above the line of Ines's
   profile, and its list holds the removal of Tom with a mark above the family's line: the restore
   lost both. A does not push yet. In a new epoch what took access away goes first: the calls of the
   list, then the deletes of shares, then folders, then the rest (D7).*
7. `x2` · A: the user removes the member `m-tom`. Answer: `ok`, `sh9` `applied`. *The device makes
   the call by itself and notes the new mark. Only when it is answered does A push: the share is
   created again (C18), for a family that is Ines alone. Pushed before the call, the share would
   have put the folder on Tom's device for the time between the two requests.*

**Outcome.** Tom is removed again before the folder is shared again, by the device that had done
both, in that order. The folder never reaches him. Ines's device says that the server was restored.

| Row | Server | A | C |
| --- | --- | --- | --- |
| `m-ines` | live · start | = | absent |
| `m-tom` | deleted · x2 | absent | = |
| `f9` | live · start | = | absent |
| `s9` | live · start | = | absent |
| `sh9` | live · a1 | = | absent |

- History (replaced versions the server keeps): `m-tom` start.
- Conflicts: none.
- A: shown restored.
- C: shown nothing; permission `f9` none.

#### S81 · A member writes the hash of the owner's file into the body of a shared snippet

Category: attachments. Covers: file-hash-in-body. Rules: C9, C19, D16.

**Start.** Ines shares f1 with the family, to change. Her private snippet s3, in f2, uses an image;
the attachment row at2 is hers and backed. Tom cannot read the file and knows its hash. He saves the
shared snippet s1 with a body that names that hash: one change, no attachment row, no link, no
upload.

Devices: A (ines), C (tom). Profiles: `p-ines` (personal, ines), `p-tom` (personal, tom), `g-fam`
(family, ines).

| Row | Kind | Refers to | Rev | Fields |
| --- | --- | --- | --- | --- |
| `m-ines` | membership | group `g-fam`, user `ines` | 2 | role `owner` |
| `m-tom` | membership | group `g-fam`, user `tom` | 6 | role `member` |
| `f1` | folder | profile `p-ines` | 3 | name `Shop replies` |
| `f2` | folder | profile `p-ines` | 5 | name `Private` |
| `s1` | snippet | folder `f1` | 7 | abbreviation `;hrs`, kind `rich`, body `Open 9 to 5.` |
| `s3` | snippet | folder `f2` | 9 | abbreviation `;iban`, kind `rich`, body `My account: ![card](attachment:sha-card)` |
| `at2` | attachment | profile `p-ines` | 17 | sha256 `sha-card`, backed `true` |
| `la2` | snippet attachment | snippet `s3`, attachment `at2` | 19 |  |
| `sh1` | folder share | folder `f1`, group `g-fam` | 20 | permission `edit` |

**Steps.**

1. `c1` · C edits `s1`: body `Open 9 to 5. ![card](attachment:sha-card)`. Answer: `s1` `applied`.
   *Allowed: Tom may change the snippet, and the server does not read bodies (C9). Ines's device
   pulls s1. The body names an attachment of the profile that her device holds, at2, and s1 has no
   link to it. A device adds such a link only where one existed before, or where it leads to a row
   that the body's author, someone other than the device's user, created: no link between s1 and at2
   has ever existed, and Tom did not create at2 (D16). Nothing is added, and the reference stays
   text, also if Ines later corrects a word in that body and so becomes its author. A device that
   added the link on the word of the body would link Ines's own, backed row, and the file would be
   Tom's to download as soon as her device had synced.*

**Outcome.** The snippet has Tom's body and no image: no device adds a link because a body names a
hash. Tom's device holds nothing of f2, not Ines's attachment row and not the file, as before.

| Row | Server | A | C |
| --- | --- | --- | --- |
| `m-ines` | live · start | = | = |
| `m-tom` | live · start | = | = |
| `f1` | live · start | = | = |
| `f2` | live · start | = | absent |
| `s1` | live · c1 (body `Open 9 to 5. ![card](attachment:sha-card)`) | = | = |
| `s3` | live · start | = | absent |
| `at2` | live · start | = | absent |
| `la2` | live · start | = | absent |
| `sh1` | live · start | = | = |

- History (replaced versions the server keeps): `s1` start.
- Conflicts: none.
- A: shown nothing.
- C: shown nothing; permission `f1` edit.

#### S82 · A sub-folder leaves a shared folder and a part of it stays shared by itself

Category: sharing. Covers: nested-share-survives. Rules: C21, D3, D6.

**Start.** Ines shares f1 with the family, to change. Its sub-folder f1b holds the snippet s2 and
the folder f1c, which is shared with the family by itself as well and holds s3. Tom's device is
offline; he edits s2 and s3. Ines moves f1b into her private folder f2.

Devices: A (ines), C (tom). Profiles: `p-ines` (personal, ines), `p-tom` (personal, tom), `g-fam`
(family, ines).

| Row | Kind | Refers to | Rev | Fields |
| --- | --- | --- | --- | --- |
| `m-ines` | membership | group `g-fam`, user `ines` | 2 | role `owner` |
| `m-tom` | membership | group `g-fam`, user `tom` | 6 | role `member` |
| `f1` | folder | profile `p-ines` | 3 | name `Shop replies` |
| `f1b` | folder | profile `p-ines`, parent `f1` | 4 | name `Returns` |
| `f1c` | folder | profile `p-ines`, parent `f1b` | 10 | name `Labels` |
| `f2` | folder | profile `p-ines` | 8 | name `Private` |
| `s1` | snippet | folder `f1` | 5 | abbreviation `;hrs`, body `Open 9 to 5.` |
| `s2` | snippet | folder `f1b` | 9 | abbreviation `;rret`, body `Return label attached.` |
| `s3` | snippet | folder `f1c` | 11 | abbreviation `;lbl`, body `Print the label at home.` |
| `sh1` | folder share | folder `f1`, group `g-fam` | 7 | permission `edit` |
| `sh2` | folder share | folder `f1c`, group `g-fam` | 12 | permission `edit` |

**Steps.**

1. C goes offline.
2. `c1` · C edits `s2`: body `Return label attached. Thank you.`.
3. `c2` · C edits `s3`: body `Print the label at home, please.`.
4. `a1` · A moves `f1b` to `f2`. Answer: `f1b` `applied`. *The move writes one row, f1b. For Tom the
   tree under f1b stops being readable, except f1c, which he reads through its own share: his feed
   gets the row f1b, delivered as null, and folder_revoked for f1b with f1c in except (C21).*
5. C comes back online and syncs. Answer: `s3` `applied`. *C applies the page's rows first. The null
   row for f1b ends that tree on the device, except the parts shared by themselves: f1c has a live
   share with a group Tom is in, so the store leaves it in place with what is in it and with the
   unsent change to s3 (D3). The unsent change to s2 goes to the list of kept changes, and f1b and
   s2 are removed. folder_revoked then finds f1b gone and the part it names in place (D6). f1c is an
   access root of its own from now on and is shown at the top level; the change to s3 is pushed like
   any other. A null row that took everything below f1b would remove f1c as well, nothing would
   bring it back before a resync, and the change to s3 would be kept as not synced although Tom may
   still make it.*

**Outcome.** The sub-folder and its snippet are in Ines's private folder and leave Tom's device,
with his unsent edit there kept in the list of changes that were not synced. The folder inside it
that is shared by itself stays on his device, at the top level, and his edit there is synced like
any other. No device ends without a folder the server shows its user.

| Row | Server | A | C |
| --- | --- | --- | --- |
| `m-ines` | live · start | = | = |
| `m-tom` | live · start | = | = |
| `f1` | live · start | = | = |
| `f1b` | live · a1 (parent `f2`) | = | absent |
| `f1c` | live · start | = | = |
| `f2` | live · start | = | absent |
| `s1` | live · start | = | = |
| `s2` | live · start | = | absent |
| `s3` | live · c2 | = | = |
| `sh1` | live · start | = | = |
| `sh2` | live · start | = | = |

- History (replaced versions the server keeps): `f1b` start; `s3` start.
- Conflicts: none.
- A: shown nothing.
- C: shown revoked, kept; kept for the user: c1; permission `f1` edit, `f1b` none, `f1c` edit.

#### S83 · A password set with a revoked computer's tokens opens nothing

Category: server. Covers: password-session. Rules: C25, D14.

**Start.** Ines's second computer, B, is lost. She revokes it from her first computer. Whoever holds
B takes its tokens from the keychain. Iamb's functions refuse them, but at the authentication
service the session still exists: with it they set a password for the account, which never had one,
and sign in with that password from an installation of their own, X.

Devices: A (ines), B (ines), X (ines, not signed in). Profiles: `p-ines` (personal, ines).

| Row | Kind | Refers to | Rev | Fields |
| --- | --- | --- | --- | --- |
| `f1` | folder | profile `p-ines` | 3 | name `Replies`; only on A, B |
| `s1` | snippet | folder `f1` | 5 | abbreviation `;rty`, body `Thanks for your order.`; only on A, B |

**Steps.**

1. B goes offline.
2. A: the user revokes the device B. Answer: `ok`. *B's device row is marked revoked: every function
   and every policy of Iamb refuses B's session from this commit on (C25). The session itself stays
   a session at the authentication service, which knows nothing of device rows; there its holder can
   still set a password without knowing an old one.*
3. `x1` · X: ines signs in. Answer: `session_refused`. *Signing in with the password opens a new
   session, which has no device row and is not revoked. hello refuses it all the same: a token says
   how its session was opened, and Iamb accepts only sessions opened the ways Iamb signs in, by a
   link or a code sent to the account's address or by an identity provider (C25). Where the project
   runs the hook that issues no token for a password, the sign-in fails one step earlier, at the
   authentication service. Either way X is never registered and reads nothing.*
4. `a1` · A edits `s1`: body `Thank you for your order.`. Answer: `s1` `applied`.

**Outcome.** The installation that signed in with a password holds nothing of the account and
receives nothing that is written afterwards; it says that the sign-in was refused. The lost computer
holds what it held when it was lost. Ines goes on working, and changes nothing but what she chose to
change.

| Row | Server | A | B | X |
| --- | --- | --- | --- | --- |
| `f1` | live · start | = | = | absent |
| `s1` | live · a1 | = | live · start | absent |

- History (replaced versions the server keeps): `s1` start.
- Conflicts: none.
- A: shown nothing.
- B: shown nothing.
- X: shown refused.

#### S84 · After a restore, the owner's device gives back an attachment row a member wrote

Category: attachments. Covers: restore-attachment-row. Rules: C18, C19, D7.

**Start.** Ines shares f1 with the family, to change. Her private snippet s3 uses an image whose
file Tom cannot read; her attachment row at2 is backed. A backup is taken. Then Tom, who knows the
file's hash, writes an attachment row at9 with that hash and links it from the shared snippet s1;
the row is not backed, and Ines's device receives both rows. Then the server's database is lost and
restored from the backup. Ines's device comes back first.

Devices: A (ines), C (tom). Profiles: `p-ines` (personal, ines), `p-tom` (personal, tom), `g-fam`
(family, ines).

| Row | Kind | Refers to | Rev | Fields |
| --- | --- | --- | --- | --- |
| `m-ines` | membership | group `g-fam`, user `ines` | 2 | role `owner` |
| `m-tom` | membership | group `g-fam`, user `tom` | 6 | role `member` |
| `f1` | folder | profile `p-ines` | 3 | name `Shop replies` |
| `f2` | folder | profile `p-ines` | 5 | name `Private` |
| `s1` | snippet | folder `f1` | 7 | abbreviation `;hrs`, kind `rich`, body `Open 9 to 5.` |
| `s3` | snippet | folder `f2` | 9 | abbreviation `;iban`, kind `rich`, body `My account: ![card](attachment:sha-card)` |
| `at2` | attachment | profile `p-ines` | 17 | sha256 `sha-card`, backed `true` |
| `la2` | snippet attachment | snippet `s3`, attachment `at2` | 19 |  |
| `sh1` | folder share | folder `f1`, group `g-fam` | 20 | permission `edit` |

**Steps.**

1. `k1` · A backup of the server's database is taken.
2. `c1` · C creates the attachment `at9` (profile `p-ines`): sha256 `sha-card`. Answer: `at9`
   `applied`. *As in S73: Tom cannot read the file, so the row is not backed (C19).*
3. `c2` · C creates the snippet attachment `la9` (snippet `s1`, attachment `at9`). Answer: `la9`
   `applied`. *The link counts for nothing while its row is not backed. Ines's device receives both
   rows: they are rows of her profile.*
4. The server's database is restored from the backup `k1`. *The restored database knows neither
   row.*
5. A pulls. Answer: `reset_required:epoch`, `at9` `applied`, `la9` `applied`. *Both rows lie above
   the line of Ines's profile and her device may write them, so it gives them back, and the server
   creates them (C18). Her device holds Tom as the writer of at9, so the op does not say that the
   row is its sender's own, and the server creates it with nobody as its creator: it is not backed,
   although the caller of this push can read the file, and no upload makes it so (C19). Created as
   the caller's row it would be backed, the link would count, and the file would be Tom's to
   download.*
6. C pulls. Answer: `reset_required:epoch`, `at9` `unchanged`, `la9` `unchanged`. *Tom's device
   holds the same two rows above the line and sends them as well, at9 as its own user's. The server
   holds both: nothing is written. Tom is the creator of at9 again, and the row is still not backed:
   he cannot read the file and has not uploaded its bytes.*

**Outcome.** The two rows are back as they were, and the attachment row is as little backed as
before the restore: an owner's device that gives back a member's row does not lend it the owner's
right to read the file. Tom's device holds his two rows and nothing of the private folder. Ines's
device says that the server was restored.

| Row | Server | A | C |
| --- | --- | --- | --- |
| `m-ines` | live · start | = | = |
| `m-tom` | live · start | = | = |
| `f1` | live · start | = | = |
| `f2` | live · start | = | absent |
| `s1` | live · start | = | = |
| `s3` | live · start | = | absent |
| `at2` | live · start | = | absent |
| `la2` | live · start | = | absent |
| `sh1` | live · start | = | = |
| `at9` | live · c1 (backed `false`) | = | = |
| `la9` | live · c2 | = | = |

- History (replaced versions the server keeps): none.
- Conflicts: none.
- A: shown restored.
- C: shown nothing; permission `f1` edit.

#### S85 · A conflict does not hand a member a version from the owner's private folder

Category: sharing. Covers: conflict-version-audience. Rules: C3, C20, C21, D2.

**Start.** Ines shares f1 with the family, to change; f2 is private. Her two devices disagree about
the snippet s2 in the private folder, and the conflict stays open. She then moves s2 into the shared
folder. Tom edits it offline, Ines edits it once more, and Tom's edit arrives last.

Devices: A (ines), B (ines), C (tom). Profiles: `p-ines` (personal, ines), `p-tom` (personal, tom),
`g-fam` (family, ines).

| Row | Kind | Refers to | Rev | Fields |
| --- | --- | --- | --- | --- |
| `m-ines` | membership | group `g-fam`, user `ines` | 2 | role `owner` |
| `m-tom` | membership | group `g-fam`, user `tom` | 6 | role `member` |
| `f1` | folder | profile `p-ines` | 3 | name `Shop replies` |
| `f2` | folder | profile `p-ines` | 4 | name `Private` |
| `s2` | snippet | folder `f2` | 8 | abbreviation `;door`, body `The door code is 4711.` |
| `sh1` | folder share | folder `f1`, group `g-fam` | 7 | permission `edit` |

**Steps.**

1. A goes offline.
2. B goes offline.
3. `a1` · A edits `s2`: body `The door code is 4711 until Friday, then 2468.`.
4. `b1` · B edits `s2`: body `Ask me for the door code.`.
5. A comes back online and syncs. Answer: `s2` `applied`.
6. B comes back online and syncs. Answer: `s2` `applied+conflict`. *Ines's own conflict, in her
   private folder: b1 replaced a1 unseen. Nobody answers it.*
7. `b2` · B moves `s2` to `f1`. Answer: `s2` `applied`. *The snippet, now harmless, goes into the
   shared folder. It becomes readable for Tom as b2 places it (C21). The move records no conflict,
   so the open one stays as it is, with a1 on offer (C3).*
8. C goes offline.
9. `c1` · C edits `s2`: body `Ask Ines for the door code.`.
10. `a2` · A edits `s2`: body `Ask me or Tom for the door code.`. Answer: `s2` `applied`.
11. C comes back online and syncs. Answer: `s2` `applied+conflict`. *c1 replaces a2 unseen: a new
    conflict, which takes over a1 from the open one (C3). Tom is a party and is shown it. What it
    offers him is a2, the version his write replaced, which he could read while it was the row. a1
    is not offered to him: it was written, and replaced, while the snippet was in a folder he could
    not read, and the conflict view and `Restore this version` treat it for him as a version that is
    not there (C20). Ines is offered both. Had Tom's device sent base 0 instead, as a changed client
    can, the answer would be the same.*

**Outcome.** The snippet has Tom's content. One conflict is open on it. Ines's devices offer her a2
and a1 back; Tom's device offers him a2 only. The door code that was in the snippet while it was
private has reached no device of Tom's, in no form: not as a row, not as a version of a conflict,
and not through a restore.

| Row | Server | A | B | C |
| --- | --- | --- | --- | --- |
| `m-ines` | live · start | = | = | = |
| `m-tom` | live · start | = | = | = |
| `f1` | live · start | = | = | = |
| `f2` | live · start | = | = | absent |
| `s2` | live · c1 (folder `f1`) | = | = | = |
| `sh1` | live · start | = | = | = |

- History (replaced versions the server keeps): `s2` start, a1, b1, b2, a2.
- Conflicts: `s2` edit_edit, winner b1, loser a1, shown to ines, replaced; `s2` edit_edit, winner
c1, loser a2 (and, taken over, a1), shown to tom and ines, open.
- A: shown conflict.
- B: shown conflict.
- C: shown conflict.

#### S86 · A removed member is told of no conflict and can close none

Category: membership. Covers: conflict-after-removal. Rules: C3, C16, C20, C21, D6.

**Start.** Ines shares f1 with the family, to change. Tom and Ines change the snippet s1 while
offline; Ines's change arrives last, and the conflict offers Tom's version back to both of them.
Then Ines removes Tom from the family.

Devices: A (ines), B (ines), C (tom). Profiles: `p-ines` (personal, ines), `p-tom` (personal, tom),
`g-fam` (family, ines).

| Row | Kind | Refers to | Rev | Fields |
| --- | --- | --- | --- | --- |
| `m-ines` | membership | group `g-fam`, user `ines` | 2 | role `owner` |
| `m-tom` | membership | group `g-fam`, user `tom` | 6 | role `member` |
| `f1` | folder | profile `p-ines` | 3 | name `Shop replies` |
| `s1` | snippet | folder `f1` | 5 | abbreviation `;hrs`, body `Open 9 to 5.` |
| `sh1` | folder share | folder `f1`, group `g-fam` | 7 | permission `edit` |

**Steps.**

1. C goes offline.
2. A goes offline.
3. `c1` · C edits `s1`: body `Open 9 to 6.`.
4. `a1` · A edits `s1`: body `Open 10 to 5.`.
5. C comes back online and syncs. Answer: `s1` `applied`.
6. A comes back online and syncs. Answer: `s1` `applied+conflict`. *a1 replaced c1 unseen. The
   conflict is shown to Ines and to Tom, who is a member and can read the row.*
7. `x1` · A: the user removes the member `m-tom`. Answer: `ok`. *The removal ends Tom's access to
   the folder in the same transaction (G13). From that commit he takes no part in the conflict: his
   feed gets no entry about it, the list of conflicts is empty for him, the conflict answers
   `not_found`, and so does his `Keep the current version`, which before would have closed it for
   Ines as well (C20). His device purges the folder and what it showed about it (D6).*
8. `r1` · B: the user restores the other version in the conflict on `s1`. Answer: `ok`. *The version
   Tom wrote stays on offer to the people who can read the row: Ines restores it, as a new revision
   by her that keeps Tom as the author of its content (C17, C20). Tom's device hears nothing of it.*

**Outcome.** The snippet has the content Tom wrote, restored by Ines after he was removed. The
conflict is resolved; it names Ines and Tom as its parties, the authors of its two versions, and
from the removal on only Ines took part in it. Tom's device holds neither the folder nor the
snippet, and learned nothing of what happened to them after the removal.

| Row | Server | A | B | C |
| --- | --- | --- | --- | --- |
| `m-ines` | live · start | = | = | absent |
| `m-tom` | deleted · x1 | absent | absent | = |
| `f1` | live · start | = | = | absent |
| `s1` | live · r1 (body `Open 9 to 6.`) | = | = | absent |
| `sh1` | live · start | = | = | absent |

- History (replaced versions the server keeps): `m-tom` start; `s1` start, c1, a1.
- Conflicts: `s1` edit_edit, winner a1, loser c1, shown to ines and tom, resolved.
- A: shown conflict.
- B: shown conflict.
- C: shown conflict, revoked; permission `f1` none.

#### S87 · A member cannot restore a version that puts a folder where a push may not

Category: structure. Covers: restore-version-permission. Rules: C3, C8, C12, C20.

**Start.** The family owns two folders at its top level, gf1, which members may change, and gx.
Ines, the owner, renames gx on one device and moves it under gf1 on the other; the move arrives
last. Later Tom, a member, and Ines both rename gx, and Tom's rename arrives last.

Devices: A (ines), B (ines), C (tom). Profiles: `p-ines` (personal, ines), `p-tom` (personal, tom),
`g-fam` (family, ines).

| Row | Kind | Refers to | Rev | Fields |
| --- | --- | --- | --- | --- |
| `m-ines` | membership | group `g-fam`, user `ines` | 2 | role `owner` |
| `m-tom` | membership | group `g-fam`, user `tom` | 6 | role `member` |
| `gf1` | folder | profile `g-fam` | 8 | name `Family`, member_permission `edit` |
| `gx` | folder | profile `g-fam` | 9 | name `Templates`, member_permission `edit` |

**Steps.**

1. A goes offline.
2. B goes offline.
3. `a1` · A edits `gx`: name `Templates 2026`.
4. `b1` · B moves `gx` to `gf1`.
5. A comes back online and syncs. Answer: `gx` `applied`.
6. B comes back online and syncs. Answer: `gx` `applied+conflict`. *The owner's own conflict: the
   move replaced the rename unseen. gx is now a sub-folder of gf1. The conflict offers a1, in which
   gx is at the top level of the group.*
7. C goes offline.
8. `c1` · C edits `gx`: name `Our templates`.
9. `a2` · A edits `gx`: name `Templates (2026)`. Answer: `gx` `applied`.
10. C comes back online and syncs. Answer: `gx` `applied+conflict`. *Tom may rename a sub-folder of
    a tree members may change (§4.3). His rename replaces a2 unseen and takes a1 over from the open
    conflict (C3). Tom has been a member all along, so both versions are on offer to him.*
11. `x1` · C: the user restores the other version in the conflict on `gx`. Answer:
    `error:forbidden`. *Tom chooses a1. Restoring it would put gx back at the top level of the
    group, with the member permission it had there. That is a move to the top, which needs `manage`:
    a restore is judged as a push of that version by that user would be (C20, §4.3), and a push of
    it by Tom is `rejected: forbidden`. Nothing is written and the conflict stays open. Restoring
    a2, which only changes the name, would have been his to do; restoring a1 is Ines's.*

**Outcome.** The folder is where the owner moved it, under gf1, with the name Tom gave it. The
conflict is open and offers a2 and a1 to both of them. Tom's attempt to restore the version from the
top level was refused and changed nothing.

| Row | Server | A | B | C |
| --- | --- | --- | --- | --- |
| `m-ines` | live · start | = | = | = |
| `m-tom` | live · start | = | = | = |
| `gf1` | live · start | = | = | = |
| `gx` | live · c1 (parent `gf1`, name `Our templates`) | = | = | = |

- History (replaced versions the server keeps): `gx` start, a1, b1, a2.
- Conflicts: `gx` edit_edit, winner b1, loser a1, shown to ines, replaced; `gx` edit_edit, winner
c1, loser a2 (and, taken over, a1), shown to tom and ines, open.
- A: shown conflict.
- B: shown conflict.
- C: shown conflict.

#### S88 · A version replaced in the same push that shares its folder is not handed to a member

Category: sharing. Covers: conflict-version-same-push. Rules: C3, C20, C21, C24, D2.

**Start.** Ines shares f1 with the family, to change; f2 is private and holds the snippet s2. On her
first device she writes a door code into s2. Her second device, which was offline, replaces the text
of s2 and then moves the whole folder f2 into the shared folder; it sends both changes in one push.
Tom edits s2 offline, Ines edits it once more, and Tom's edit arrives last.

Devices: A (ines), B (ines), C (tom). Profiles: `p-ines` (personal, ines), `p-tom` (personal, tom),
`g-fam` (family, ines).

| Row | Kind | Refers to | Rev | Fields |
| --- | --- | --- | --- | --- |
| `m-ines` | membership | group `g-fam`, user `ines` | 2 | role `owner` |
| `m-tom` | membership | group `g-fam`, user `tom` | 6 | role `member` |
| `f1` | folder | profile `p-ines` | 3 | name `Shop replies` |
| `f2` | folder | profile `p-ines` | 4 | name `Private` |
| `s2` | snippet | folder `f2` | 8 | abbreviation `;door`, body `The door code changes on Friday.` |
| `sh1` | folder share | folder `f1`, group `g-fam` | 7 | permission `edit` |

**Steps.**

1. B goes offline.
2. `a1` · A edits `s2`: body `The door code is 4711 until Friday, then 2468.`. Answer: `s2`
   `applied`.
3. `b1` · B edits `s2`: body `Ask me for the door code.`.
4. `b2` · B moves `f2` to `f1`.
5. B comes back online and syncs. Answer: `s2` `applied+conflict`, `f2` `applied`. *One push with
   both changes. The server applies folders before snippets (§6.3), so it moves f2 under the shared
   folder first and replaces the text of s2 second; Ines made them in the other order. b1 replaced
   a1 unseen: her own conflict, which offers a1. For Tom the folder and the snippet become readable
   as this request leaves them, with b1's text (C21). His access to f2 begins when the request has
   ended (§5.4): a1 was replaced inside it, and no state of the server ever showed it to him.*
6. C goes offline.
7. `c1` · C edits `s2`: body `Ask Ines for the door code.`.
8. `a2` · A edits `s2`: body `Ask me or Tom for the door code.`. Answer: `s2` `applied`.
9. C comes back online and syncs. Answer: `s2` `applied+conflict`. *c1 replaces a2 unseen: a new
   conflict, which takes over a1 from the open one (C3). Tom is offered a2, which was the row while
   he could read the folder. a1 is not offered to him and he cannot restore it, although the folder
   was already 'his' when the server replaced it: the replacement and the move were one request
   (C20). Ines is offered both. A guess of a1's text, sent by a changed client with a1's revision as
   its base, is answered like any other write (C17).*

**Outcome.** The folder is in the shared tree and the snippet has Tom's content. One conflict is
open on it. Ines's devices offer her a2 and a1 back; Tom's device offers him a2 only. The door code
that Ines took out of the snippet before she moved the folder has reached no device of Tom's,
whether her device sent the two changes apart or together.

| Row | Server | A | B | C |
| --- | --- | --- | --- | --- |
| `m-ines` | live · start | = | = | = |
| `m-tom` | live · start | = | = | = |
| `f1` | live · start | = | = | = |
| `f2` | live · b2 (parent `f1`) | = | = | = |
| `s2` | live · c1 | = | = | = |
| `sh1` | live · start | = | = | = |

- History (replaced versions the server keeps): `f2` start; `s2` start, a1, b1, a2.
- Conflicts: `s2` edit_edit, winner b1, loser a1, shown to ines, replaced; `s2` edit_edit, winner
c1, loser a2 (and, taken over, a1), shown to tom and ines, open.
- A: shown conflict.
- B: shown conflict.
- C: shown conflict.

#### S89 · A folder name replaced in the same push that shares the folder above it is not handed to a member

Category: sharing. Covers: folder-name-same-push. Rules: C3, C20, C21, C24.

**Start.** Ines shares f1 with the family, to change; f2 is private and has a sub-folder f3. On her
first device she gives f3 a name that says what it holds. Her second device, which was offline,
renames f3 to something plain and then moves f2 into the shared folder; it sends both changes in one
push. Tom renames f3 offline, Ines renames it once more, and Tom's rename arrives last.

Devices: A (ines), B (ines), C (tom). Profiles: `p-ines` (personal, ines), `p-tom` (personal, tom),
`g-fam` (family, ines).

| Row | Kind | Refers to | Rev | Fields |
| --- | --- | --- | --- | --- |
| `m-ines` | membership | group `g-fam`, user `ines` | 2 | role `owner` |
| `m-tom` | membership | group `g-fam`, user `tom` | 6 | role `member` |
| `f1` | folder | profile `p-ines` | 3 | name `Shop replies` |
| `f2` | folder | profile `p-ines` | 4 | name `Private` |
| `f3` | folder | profile `p-ines`, parent `f2` | 8 | name `Letters` |
| `sh1` | folder share | folder `f1`, group `g-fam` | 7 | permission `edit` |

**Steps.**

1. B goes offline.
2. `a1` · A edits `f3`: name `Letters to the landlord about the arrears`. Answer: `f3` `applied`.
3. `b1` · B edits `f3`: name `Misc`.
4. `b2` · B moves `f2` to `f1`.
5. B comes back online and syncs. Answer: `f2` `applied`, `f3` `applied+conflict`. *One push. The
   server applies a folder before the folders below it (§6.3): f2 moves under the shared folder,
   then f3 is renamed. b1 replaced a1 unseen: Ines's own conflict, which offers a1. Tom's access to
   f2 and f3 begins when the request has ended (§5.4).*
6. C goes offline.
7. `c1` · C edits `f3`: name `Misc letters`.
8. `a2` · A edits `f3`: name `Miscellaneous`. Answer: `f3` `applied`.
9. C comes back online and syncs. Answer: `f3` `applied+conflict`. *Tom may rename a sub-folder of a
   tree he may change (§4.3). His rename replaces a2 unseen and takes a1 over from the open conflict
   (C3). He is offered a2; the name f3 had before the folder above it was shared is not offered to
   him (C20).*

**Outcome.** The folders are in the shared tree and f3 has the name Tom gave it. One conflict is
open on it: Ines is offered a2 and a1, Tom a2 only. The name that said what the folder held has
reached no device of Tom's.

| Row | Server | A | B | C |
| --- | --- | --- | --- | --- |
| `m-ines` | live · start | = | = | = |
| `m-tom` | live · start | = | = | = |
| `f1` | live · start | = | = | = |
| `f2` | live · b2 (parent `f1`) | = | = | = |
| `f3` | live · c1 (name `Misc letters`) | = | = | = |
| `sh1` | live · start | = | = | = |

- History (replaced versions the server keeps): `f2` start; `f3` start, a1, b1, a2.
- Conflicts: `f3` edit_edit, winner b1, loser a1, shown to ines, replaced; `f3` edit_edit, winner
c1, loser a2 (and, taken over, a1), shown to tom and ines, open.
- A: shown conflict.
- B: shown conflict.
- C: shown conflict.

#### S90 · What was deleted from a folder before it was shared does not reach the people it is shared with

Category: sharing. Covers: tombstone-before-access. Rules: C6, C21, C22, D3, D6.

**Start.** Ines and Tom are in the family group. Ines has the folder f1 with a sub-folder f1b;
nothing is shared. Before she shares f1 she deletes a snippet that names a debt, and the sub-folder
with the complaints. Then she shares f1 with the family, to use. Afterwards she deletes one more
snippet.

Devices: A (ines), C (tom). Profiles: `p-ines` (personal, ines), `p-tom` (personal, tom), `g-fam`
(family, ines).

| Row | Kind | Refers to | Rev | Fields |
| --- | --- | --- | --- | --- |
| `m-ines` | membership | group `g-fam`, user `ines` | 2 | role `owner` |
| `m-tom` | membership | group `g-fam`, user `tom` | 6 | role `member` |
| `f1` | folder | profile `p-ines` | 3 | name `Shop replies` |
| `f1b` | folder | profile `p-ines`, parent `f1` | 4 | name `Complaints about the landlord` |
| `s1` | snippet | folder `f1` | 5 | abbreviation `;hrs`, body `Open 9 to 5.` |
| `s2` | snippet | folder `f1` | 7 | abbreviation `;lena`, body `Lena still owes 40 for March.` |
| `s3` | snippet | folder `f1b` | 8 | abbreviation `;damp`, body `The damp in the back room is still not fixed.` |
| `s4` | snippet | folder `f1` | 9 | abbreviation `;ret`, body `Returns within 14 days.` |

**Steps.**

1. `a1` · A deletes `s2`. Answer: `s2` `applied`.
2. `a2` · A deletes `f1b`. On the server the change also writes `s3`. Answer: `f1b` `applied`. *The
   server keeps the three deleted rows for 30 days, with their content (C22). Only Ines can read the
   folder, so only her devices hold the tombstones.*
3. `a3` · A creates the folder share `sh1` (folder `f1`, group `g-fam`): permission `view`. Answer:
   `sh1` `applied`. *Tom's access to f1 begins here. folder_visible(f1) goes into his feed and his
   device fetches the tree: f1, s1, s4 and the share. The server also has an access entry for Tom
   for the deleted sub-folder, as for any folder below a share, and it gives him nothing: a
   tombstone is read through an entry that began before the delete (§5.4). No read hands Tom s2, f1b
   or s3: not the fetch, not a snapshot at his next sign-in, not a request for the rows by their
   ids.*
4. `a4` · A deletes `s4`. Answer: `s4` `applied`. *Tom could read s4 before it was deleted. Its
   tombstone reaches his device as before, and the device keeps it until its purge (D8).*

**Outcome.** Tom's device holds the folder and the snippet that is live in it, and the tombstone of
the snippet that was deleted while he could read it. It holds nothing of the debt note, the
sub-folder or the complaint: for Tom they were never there. Ines's devices hold all four deleted
rows until the purge.

| Row | Server | A | C |
| --- | --- | --- | --- |
| `m-ines` | live · start | = | = |
| `m-tom` | live · start | = | = |
| `f1` | live · start | = | = |
| `f1b` | deleted · a2 | = | absent |
| `s1` | live · start | = | = |
| `s2` | deleted · a1 | = | absent |
| `s3` | deleted · a2 | = | absent |
| `s4` | deleted · a4 | = | = |
| `sh1` | live · a3 | = | = |

- History (replaced versions the server keeps): `f1b` start; `s2` start; `s3` start; `s4` start.
- Conflicts: none.
- A: shown nothing.
- C: shown shared; permission `f1` view.

#### S91 · A share ends, a snippet is rewritten and deleted, and the share begins again while a member's device is offline

Category: sharing. Covers: tombstone-after-access-ended. Rules: C11, C21, D3, D6, D11.

**Start.** Ines shares f1 with the family, to change. Tom's device holds the folder with s1 and s2,
is offline, and Tom edits s2. Ines stops sharing the folder, writes something private into s2,
deletes s2, and shares the folder again.

Devices: A (ines), C (tom). Profiles: `p-ines` (personal, ines), `p-tom` (personal, tom), `g-fam`
(family, ines).

| Row | Kind | Refers to | Rev | Fields |
| --- | --- | --- | --- | --- |
| `m-ines` | membership | group `g-fam`, user `ines` | 2 | role `owner` |
| `m-tom` | membership | group `g-fam`, user `tom` | 6 | role `member` |
| `f1` | folder | profile `p-ines` | 3 | name `Shop replies` |
| `s1` | snippet | folder `f1` | 5 | abbreviation `;hrs`, body `Open 9 to 5.` |
| `s2` | snippet | folder `f1` | 8 | abbreviation `;key`, body `Ask at the counter for the key.` |
| `sh1` | folder share | folder `f1`, group `g-fam` | 7 | permission `edit` |

**Steps.**

1. C goes offline.
2. `c1` · C edits `s2`: body `Ask at the counter or ring the bell for the key.`.
3. `a1` · A deletes `sh1`. Answer: `sh1` `applied`.
4. `a2` · A edits `s2`: body `The spare key is under the blue pot.`. Answer: `s2` `applied`.
5. `a3` · A deletes `s2`. Answer: `s2` `applied`. *Tom cannot read the folder while this happens:
   nothing about s2 goes into his feed.*
6. `a4` · A creates the folder share `sh1` (folder `f1`, group `g-fam`): permission `edit`. Answer:
   `sh1` `applied`. *Tom's access begins anew, after the delete of s2 (C11, §5.4). The tombstone of
   s2, with the text Ines wrote while the folder was hers alone, is not his to read: no fetch,
   snapshot or request by id returns it.*
7. C comes back online and syncs. *C still holds s2, live, in the version of the start, with Tom's
   unsent edit. One page holds folder_revoked and then folder_visible for f1. On the first the
   device purges the tree: s2 goes with it, and the unsent edit goes to the list of changes that
   were not synced (D6). On the second it fetches the tree, which holds what is live: f1, s1 and the
   share. The device never asks for s2 again, and had it asked, the answer would have been that the
   row is not there.*

**Outcome.** Tom's device holds the folder as the server shows it to him: with s1, without s2. It
was not left with a live copy of a snippet whose delete it could not be told of, and it has not
received what Ines wrote in between. His own edit of s2 is kept in the list of changes that were not
synced. The device says that the folder was unshared and shared.

| Row | Server | A | C |
| --- | --- | --- | --- |
| `m-ines` | live · start | = | = |
| `m-tom` | live · start | = | = |
| `f1` | live · start | = | = |
| `s1` | live · start | = | = |
| `s2` | deleted · a3 | = | absent |
| `sh1` | live · a4 | = | = |

- History (replaced versions the server keeps): `s2` start, a2; `sh1` start, a1.
- Conflicts: none.
- A: shown nothing.
- C: shown revoked, kept, shared; kept for the user: c1; permission `f1` edit.

#### S92 · The owner deletes a tag that a shared snippet still carries, and brings it back

Category: tags. Covers: deleted-tag-shared-snippet. Rules: C5, C10, D3, D16.

**Start.** Ines shares f1 with the family, to use. The snippet s1 in it carries her tag t1. She
deletes the tag; the link from s1 to it stays, as links do (C10). Later she restores the tag.

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

1. `a1` · A deletes `t1`. Answer: `t1` `applied`. *Tom read the tag through the link from a snippet
   he can read, and a link gives a tag only while the tag is live (§5.13). His feed gets an entry
   for the tag, delivered as null: his device removes the tag's row. The link stays on both devices
   and counts for nothing while the tag is deleted (C10).*
2. `a2` · A restores the deleted `t1`. Answer: `t1` `applied`. *The tag is live again and the link
   counts again. Tom's feed gets the tag and, with it, the link (§5.5): his device had dropped the
   link with the tag.*

**Outcome.** While the tag was deleted, Ines's devices held it as deleted and Tom's device did not
hold it at all: a tag of someone else's profile leaves a device when it is deleted, as it does when
the last link to it goes, and its name does not stay behind. Restored, the tag is on every device
again, and the snippet carries it on every device.

| Row | Server | A | C |
| --- | --- | --- | --- |
| `m-ines` | live · start | = | = |
| `m-tom` | live · start | = | = |
| `f1` | live · start | = | = |
| `s1` | live · start | = | = |
| `t1` | live · a2 | = | = |
| `l1` | live · start | = | = |
| `sh1` | live · start | = | = |

- History (replaced versions the server keeps): `t1` start, a1.
- Conflicts: none.
- A: shown nothing.
- C: shown nothing.

#### S93 · A device comes back after a month to a folder that was unshared, changed and shared again

Category: sharing. Covers: tombstone-resync. Rules: C13, C21, C22, D5, D7.

**Start.** Ines shares f1 with the family, to change. Tom's device holds the folder with s1, s2 and
s3, goes offline, and Tom edits s2. Ines stops sharing the folder. A month later she writes
something private into s2, deletes s2 and s3, and shares the folder again. Tom's device comes back
the day after: its position is older than the feed reaches.

Devices: A (ines), C (tom). Profiles: `p-ines` (personal, ines), `p-tom` (personal, tom), `g-fam`
(family, ines).

| Row | Kind | Refers to | Rev | Fields |
| --- | --- | --- | --- | --- |
| `m-ines` | membership | group `g-fam`, user `ines` | 2 | role `owner` |
| `m-tom` | membership | group `g-fam`, user `tom` | 6 | role `member` |
| `f1` | folder | profile `p-ines` | 3 | name `Shop replies` |
| `s1` | snippet | folder `f1` | 5 | abbreviation `;hrs`, body `Open 9 to 5.` |
| `s2` | snippet | folder `f1` | 8 | abbreviation `;key`, body `Ask at the counter for the key.` |
| `s3` | snippet | folder `f1` | 9 | abbreviation `;ret`, body `Returns within 14 days.` |
| `sh1` | folder share | folder `f1`, group `g-fam` | 7 | permission `edit` |

**Steps.**

1. C goes offline.
2. `c1` · C edits `s2`: body `Ask at the counter or ring the bell for the key.`.
3. `a1` · A deletes `sh1`. Answer: `sh1` `applied`.
4. 31 days pass. *The feed entries of that day, folder_revoked among them, and the share's tombstone
   are purged (C22).*
5. `a2` · A edits `s2`: body `The spare key is under the blue pot.`. Answer: `s2` `applied`.
6. `a3` · A deletes `s2`. Answer: `s2` `applied`.
7. `a4` · A deletes `s3`. Answer: `s3` `applied`.
8. `a5` · A creates the folder share `sh1` (folder `f1`, group `g-fam`): permission `edit`. Answer:
   `sh1` `applied`. *Tom's access begins anew, after both deletes. Their tombstones are not his to
   read (§5.4).*
9. C comes back online and syncs. Answer: `reset_required:expired`, `s2` `rejected:gone`. *C never
   saw the folder leave: its position is older than the feed, so it resyncs (D7). The snapshot holds
   f1, s1 and the share, and neither deleted snippet. C removes s3, which it held live, and leaves
   s2 alone because of the unsent edit. The push of that edit names a row that, for Tom, is gone
   (C13): the server refuses it and writes nothing, C keeps the edit for the user and removes the
   row (D5). Had the server restored the snippet instead, Tom would have brought back a row Ines
   deleted while the folder was hers alone, with whatever still hung on it.*

**Outcome.** Tom's device holds the folder as the server shows it to him: with s1, without s2 and
s3. It was told of neither delete and holds no live copy of either snippet; what Ines wrote into s2
while the folder was not shared has not reached it. Tom's edit of s2 is kept in the list of changes
that were not synced. On the server both snippets stay deleted.

| Row | Server | A | C |
| --- | --- | --- | --- |
| `m-ines` | live · start | = | = |
| `m-tom` | live · start | = | = |
| `f1` | live · start | = | = |
| `s1` | live · start | = | = |
| `s2` | deleted · a3 | = | absent |
| `s3` | deleted · a4 | = | absent |
| `sh1` | live · a5 | = | = |

- History (replaced versions the server keeps): `s2` start, a2; `s3` start.
- Conflicts: none.
- A: shown nothing.
- C: shown kept; kept for the user: c1; permission `f1` edit.

<!-- scenarios:end -->

## 13. Open questions for the owner

Each is decided one way in this document so that the design is complete; each is the owner's to
change, and the cost of the other answer is stated. Questions 10 and 11 are new since the first
review of the design, question 12 since the follow-up review and question 13 since the close-out
review; questions 1, 3, 4, 5, 6 and 7 say more than they did, 3 and 4 were corrected after the
follow-up, and 3, 4 and 12 again after the close-out.

| # | Question | Recommendation | If decided otherwise |
| --- | --- | --- | --- |
| 1 | **Does signing in sync the whole personal library?** Flow 6 of the UX specification tells a person who signs in to share one folder: "Your other folders stay on this computer until you choose to sync or share them." Under this design signing in adopts the library and syncs all of it (D9): every snippet on the computer is then stored on the server, where its operator can read it (question 2). Both cannot be true | Sync everything in version 1, and change that sentence to say all of it: "Signing in stores your snippets on Iamb's server and keeps them in step between your computers. Only folders you share can be seen by other people." It is what the Phase 4 gate tests (two devices of one account), and it is simple enough to prove free of lost edits | A switch per device, "Keep my own snippets on this computer only", is one gate and no server change: the device neither adopts nor pushes its personal profile and still receives shared folders; it cannot share a folder of its own while the switch is on. Sharing single folders while the rest stays local, as the sentence promises, needs a sync scope per folder with its own rules for tags, attachments and moves between synced and local folders; that is a feature of its own and should not be in version 1 |
| 2 | **Should the server be unable to read snippets (end-to-end encryption)?** | Not in version 1. Say plainly in the privacy text and at sign-in (question 1) that synced snippets are stored on the server and readable by its operator, keep the server from interpreting content (C9) so that encryption can be added per profile later, and decide again before team plans are sold | End-to-end encryption brings a recovery key the user must keep (there is no password), a key per shared folder handed over by a member's device (the 10-second target then depends on one being online), re-keying on removal, and no server-side check of attachment hashes; roughly a phase of work, and it changes tasks 4.2, 4.4, 4.5 and 4.6 |
| 3 | **Where is the server hosted, and on which hosting plan?** The plan decides three things. *Backups*: how much a restore loses. Devices give back every row they still hold, also where someone has changed the row since the restore (C18, S68), but four things do not come back. (a) The history and the open conflicts of the lost time: a version that had already been replaced on every device, also one a conflict was still offering, is gone. (b) What only the server writes: memberships, invitations and groups changed since the backup come back as the backup had them. What took access away since (a removal, a withdrawn invitation, a lowered role, a deleted group, a share that was ended, a revoked device) the operator's restore script makes again from the function log before the server opens, so that a removed member receives nothing (C18, S79). Where the log no longer names a call, the member is a member again until the computer from which they were removed has synced, which makes the removal again before it gives anything back (D7, S58, S80); if that computer does not come back, until an owner or admin removes them again, which the notice after a restore asks them to check. Invitations and role changes that gave something must be made again by hand. (c) Accounts and groups: an account deleted since exists again until the operator deletes it again from the log; an account created since is gone, and its user signs up again and adds the library that is still on their computer; a group created since is gone, and what was written in it is kept on its authors' computers as changes that were not synced. (d) Sessions: the restored database holds the sessions of the backup, so every computer may have to sign in again: a session opened since is gone, and one whose token was renewed since is refused as far as is known (§11.6). A computer that is signed out this way keeps what it holds of shared folders, set aside, and gives back what was lost once its user has signed in again (D14, S78; question 13). A computer that was revoked since is revoked again by the restore script. *Connections*: how many devices can hold the live channel at once: 200 on the free plan, 500 on the first paid one, 10,000 above; devices beyond that poll every 2 seconds while in use (§6.6). *The daily job*: Supabase Cron must be part of the plan (§11.6) | One region in the European Union, and say so in the privacy text; the 5 and 10 second targets leave room for a distant region. A paid plan from the first real user, with daily backups, and point-in-time recovery before paying customers depend on it: whatever a restore loses, every user signs in again and every owner looks at their groups. The plan must keep the functions' log at least as long as its backups, because the restore script reads from it which removals to make again (§11.6). Move to the plan above when more than about 400 devices are online at once | Another region changes nothing in the design; more than one region is not supported by it. Without backups a lost database is rebuilt only from what devices hold, and every group must be made again |
| 4 | **Plan limits, rate limits, and whose limit a member uses.** They decide cost and pricing: devices per account, snippets, shared folders, groups owned, members per group, storage, days of history; the rate limits of §6.8; `cascade_max` and the size of a file. And: a snippet a member creates in a shared folder counts against the plan of the folder's owner (§5.10), so a member who may change a folder can use up the owner's limit | Start with one default plan per scope and generous values while there is no billing: 5 devices (a device counts while it is not revoked, its session exists and it has called in the last 30 days and since the last restore of the server, so a computer that was signed out, replaced or revoked, or that came back under a new id after a restore, does not use up a place, C25), 10,000 snippets, 10 shared folders, 3 groups owned, 10 members per group, 500 MB of files, 25 MiB per file as in the local store, 90 days of history; the rate limits as in §6.8. All are rows in `plan` and settings of the server, changed without a release. Count rows against the owner of the profile they are in: the library is the owner's, the owner chose who may change it, and no other rule can be explained in one sentence | Any values work; only `history_days` above 90 needs more storage, and a limit lowered later never deletes rows (§5.10). Counting a member's rows against the member's own plan needs an author per row in the limit's count and an answer to what happens to the rows when the member leaves |
| 5 | **How long is replaced content kept?** Tombstones for 30 days is fixed by the architecture. History for 90 days and open conflicts for 90 days are chosen here, and an image nothing uses is kept for the same 90 days, so that a restored version still has its images (§5.12) | Confirm 90 for all three. Longer history can become a plan feature (`history_days`) | Shorter history means a conflict can lose its other version sooner; it must stay above the 30 days of the tombstones, and the images' time must not be shorter than the history's |
| 6 | **Deleting an account: at once or after a waiting time?** | At once. Every device keeps the whole library locally, so the user loses nothing they have not asked to lose, and "delete" means delete. Backups age out with the hosting provider's retention, which the privacy text must name; if the server is restored from a backup in that time, the operator deletes the account again before the server is opened (C18) | A waiting time of 7 or 30 days needs a "deletion pending" state on the account and a way back in |
| 7 | **Who may share a folder with a group?** A shared folder is on every member's device within ten seconds, and its single-line snippets expand there at once: there is no step at which a recipient accepts a folder and no way to switch one off. A shared snippet cannot take over an abbreviation the recipient's own snippets use (D13) and cannot press keys or include other snippets without review (D10), but for any abbreviation the recipient does not use yet, whoever may share can put expanding text on every member's computer | In a family, every member, as flow 6 shows: the people know each other and the folder is meant to work at once. In a team, the owner and admins only (one check in C11, by the kind of group). The design is written for this | (a) "Every member" in teams too: no check, and any colleague can put expanding text on everyone's computer. (b) "New shared folders start switched off for each recipient": a device-local switch per shared folder and a step in flow 6 before "the other 17 work now"; it protects families as well, and the 10-second target then measures that the folder appears, not that it works. (c) A setting per team that lets members share is one more column and one more account call, and can be added later without changing anything else |
| 8 | **Invitation emails** need a mail-sending service and a sender address under Iamb's own domain; the sign-in emails need the same | Use one service for both, set up in Supabase's authentication settings, before task 4.5 is tested with real addresses | — |
| 9 | **May someone run their own server?** The server's code is AGPL like the rest | Keep `server/` complete enough to be run by others, and let the address of the server be a value in a configuration file without interface in version 1 | A fixed address is simpler to support and to sign; it can be opened later |
| 10 | **Should a `{snippet}` reference written by someone else be held for review?** ADR-0022 holds key presses, waits, clipboard reads and line breaks. A member who may change a shared folder can write one line that includes, by its abbreviation, a snippet of the reader's own private library; the reference resolves on the reader's device and types that private text, with its key presses, into whatever the reader is writing to. It is also a wanted use: a shared reply that ends in each member's own signature | Hold it: a change by someone else that adds or changes a `{snippet}` reference waits for review like a key press (D10), and the review dialog shows what each reference resolves to on this device. One approval per content, and the shared signature works from then on. This adds a fifth item to ADR-0022's list and is recorded in ADR-0028 | (a) Leave it as it is and list it in §11.3 under "what is left": no review step, and a member can make the reader type their own private snippets. (b) Resolve references in content by others only inside the same shared folder: no review step and no leak, but the shared signature no longer works |
| 11 | **Can ownership of a group be given to someone who has not agreed?** The owner of a group cannot leave it, and its plan is paid by its owner | No: the owner offers, the member accepts (`transfer_ownership`, `accept_ownership`; C16, S34), and the member's own plan must allow one more group. The design is written for this | An immediate transfer is one call less and one dialog less; a member can then be made to own, and later to pay for, a group, and has to hand it on to be able to leave |
| 12 | **What should the server do when a device holds more than it does?** That is the sign of a database that was restored without the procedure of C18, and it is also what any signed-in client can claim by sending a large number | Nothing by itself: the device is told to wait (`server_behind`), the operator is alerted, with the account and the number it claimed, and runs the restore script if a restore is what happened; the script begins the epoch above a floor the operator takes from the function log (C18). A restore is an act of the operator, and so is the epoch that follows it. The design is written for this | A server that begins an epoch on such a report heals a forgotten step with nobody awake; it also lets any signed-in account make every device of every account read its whole library again, as often as the rate limit on `hello` allows, which the follow-up review showed |
| 13 | **What does a computer keep of shared folders when it is signed out without the server's word?** A computer whose session has ended can ask the server nothing. Usually nothing is wrong: the user signed out everywhere from another computer. After a restore of the server it is what happens to most computers (question 3), and then a computer may hold the only copy of an edit the restore lost. Removing shared folders at once, as a confirmed sign-out does, loses that edit; keeping them leaves other people's snippets on a computer nobody is signed in on | Set them aside for up to 30 days: not used, not shown, not exported; removed when the same account has signed in again and synced, when another account signs in, when the user asks for it, or when the 30 days are over, and what the computer's own user wrote there in the last 30 days is then kept in the list of changes that were not synced (D14). The interface says that shared folders were set aside until the next sign-in. The design is written for this | (a) Remove at once and keep only what the computer's own user wrote in the last 30 days: nothing of other people's stays, the list of kept changes fills with changes that were synced after every such sign-out, and an edit by someone else that only this computer still held is gone. (b) Keep until the next sign-in, without a limit: nothing is ever lost, and a computer nobody signs in on again holds shared folders for good. (c) Fewer than 30 days: the time must stay above the age of the oldest backup the operator would restore from |

## 14. What each task takes from here

| Task | Builds | From |
| --- | --- | --- |
| 4.2 Server schema and policies | The schemas, tables, constraints and indexes; the gate helpers and the grants on them; `refresh_access` and its slow reference; the lock order of C24 in every writing function; the feed with its ordering rule; the functions of `api` with the rules C1 to C25, their codes and their time-outs; the bounded cascade; the read policies; the storage and live-channel policies; the daily job; `rev_seq` without a cache and `head_rev` on the profile rows, `begin_epoch` with each profile's `backup_rev` and a first revision taken from a time, the `maintenance` switch, the `server_behind` answer with its log line, and the restore procedure as a script, a page and a test: `begin_epoch` with the operator's floor, `replay_call` for what took access away, and the stored files put in step with their rows; the function log with the ids and marks the script needs; the device rows kept while their sessions exist; `session_refused` for a session that does not exist or was not opened the way Iamb signs in, `sign_in_methods`, and the token hook where the project has one; an attachment row without a creator after a re-push; the default plans; and the verification steps §11.6 gives it, before anything is built on them | §4, §5, §6.1 to §6.5, §6.11, §8, §11.5, §11.6 |
| 4.3 Security review | Tests of G1 to G23 as each kind of user, among them the two-connection tests of G21; the queries of §4.5, also for a known file hash and for a member's own rows after removal; the required authentication settings of §11.3, with the test per identity provider and the test that nothing a revoked session can ask of the authentication service opens a session Iamb accepts (G15); the restore script against G23; the log check | §4.5, §5.13, §11 |
| 4.4 Sync client | `iamb-sync`; the store's migration 3 and the Sync messages; D1 to D17; the effective library over all profiles with D13; the scenario replay, which derives every outcome from the rules and compares it field by field; the list of calls that took access away, written before each call and used before anything is given back; the sign-out that sets shared folders aside and the purge that ends it (D14, D8); the null folder that leaves the parts shared by themselves, and the access check that fetches what is missing; and the simulation, which must include an agent that stops between a commit and its answer, a second change before the answer, answers to `pull` that are lost several times in a row, a copied data directory, and a restore of the server with devices that return in every order, with writes made after the restore before a device that holds a lost change returns, with a share that the restore lost under a member's edit, with sessions that the restore ends before a device has synced, and devices that sign in again in every order or stay signed out, and with removals and shares made on different devices of one account before the restore, with the invariants I1 to I7 and the twelve places of §1 as its definition of a lost edit; the 5-second measurement | §1, §6, §7, §9, §12 |
| 4.5 Account interface | Sign-in with the sentence of question 1 and the display name; groups, invitations, members with their addresses for owners and admins, roles, the offer of ownership, on the account endpoint; the share dialog (with the count of snippets that include snippets outside the folder, and the sentence about copies that stay with removed members); conflicts with every version on offer ("Show the other version", "Restore", "Keep"); the list of changes that were not synced, refused deletes included; blocked rows with their reason; devices and "sign out everywhere"; "this computer was signed out", with "shared folders were set aside until you sign in again" and the way to remove them now, and "the server was restored", with the calls that could not be made again; the sign-out dialog, which syncs first and says when it could not; the review dialog with author, time, difference and what each `{snippet}` resolves to, and "review first" where a copy is refused; the 10-second measurement | §4, §5.6, §5.14, §7.2, D5, D10, D14, §11.3, §12 ("Shown") |
| 4.6 Media sync | Upload through `attachment_upload` rows and the `attachment-commit` edge function with the server-side hash check; download; the size limit; the expiry of uploads; `unlinked_at` and the removal of files nothing has used for 90 days; the link a device adds for an image a restored body names, and the two cases in which it may (D16); the files offered again after a restore of the server, and a re-pushed attachment row with or without `author: "self"` (§6.11, C19); the measurement of §11.6 | §5.9, §5.12, §5.13, §6.11, C19, D16 |
| 4.7 Billing readiness | `plan`, `subscription`, the flags; `entitlements()`; the enforcement points, with rows counted per profile and shared folders counted as folders; the daily retry of rows blocked by a limit; the values of question 4 | §5.10, §6.12, C11, C14, D5 |

## 15. What changes in the architecture and the decision records

The architecture's frame holds. Four new records add to it, and the architecture document
points to them where they apply.

| Record | Adds or amends | Why |
| --- | --- | --- |
| [ADR-0027](decisions/0027-sync-protocol-and-conflict-rules.md) | Adds to ADR-0016: the server's revision counter and clock as the only order; writes through functions with receipts, one profile at a time; deletes as writes, with the folder cascade answered row by row and the restored folder; unsent changes are never overwritten by a pull, and a skipped row is fetched later; refused changes are kept on the device and blocked ones stay local; one session, one device, and a position that can be presented again; resync, and epochs that only the operator begins, with what a restore lost measured per profile and given back whatever was written since, after what took access away; a sign-out that removes other people's folders only on the server's word, and sets them aside otherwise; sessions accepted only when they were opened the way Iamb signs in | ADR-0016 fixed the frame and left the rules to this task |
| [ADR-0028](decisions/0028-data-model-for-sync.md) | Amends ADR-0014: abbreviations are not unique on the server, and the store enforces uniqueness for local edits only; precedence by the author of the content and the server's order; a share's id is derived; profiles and memberships are written by the server only, ownership by offer; access entries. Amends ADR-0022: the server records the author of a snippet's content against the version a write was based on, a device approves content, not a revision, a held snippet cannot be copied, and a `{snippet}` reference by someone else is held; an attachment row gives a file only when it is backed, a row given back after a restore by someone other than its writer has no creator, and a device adds a link for an image a body names only where the link existed before or leads to a row the body's author, someone other than the device's user, created | Uniqueness per user's view cannot be a server constraint; sharing must be safe against late changes; a review that follows the row's last writer can be bypassed by an innocent edit |
| [ADR-0029](decisions/0029-server-readable-content.md) | New: synced content is readable by the server in version 1, the server never interprets it, and end-to-end encryption stays possible. Proposed, with open question 2 | Architecture §8 did not say; it has to be said before anything is stored |
| [ADR-0031](decisions/0031-conflicts-follow-access.md) | Amends ADR-0027: a conflict offers a version only to a party who wrote it or could read it while it was the row; a party takes part only while they can read the row; "Restore this version" is judged as a push; a request locks only what its caller has a part in, and a new id is locked across profiles. Proposed | The security review of task 4.3 showed that the rules as first written let a conflict carry a private version to a member, kept a removed member in the conflicts of the folder he left, and let a restore move what a push may not (C3 and C20 against §4.4, §4.5, §11.3 and G13) |

In `architecture.md`: §4.4 (the store enforces uniqueness for local edits; precedence by author
and by the server's order), §6.2 and §6.4 (the messages of §7.2), §7 (the device-local tables of
§7.1, the server-set fields, who writes profiles and memberships), §8 rule 10 (review follows
the author of the content, and covers `{snippet}` references by others, open question 10), §9 (a
summary of the rules and a pointer here) and §11 (this document's name). Nothing in §8 rule 1
changes.
