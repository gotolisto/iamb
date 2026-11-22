# iamb-sync

The sync agent of task 4.4: push, pull, retry with backoff and live updates between the local
library and the Iamb server. The rules are `docs/sync-design.md` (§6 the protocol, §7 and §9 the
device, §8 the server); this crate implements the device's half and a model of the server's.

The agent is its own process, started by the supervisor (architecture §3). It has the network and
no store; the service has the store and no network (ADR-0008). Everything it learns it hands to the
service over the local endpoint with the role `sync`, and the store (`iamb_store::sync`) applies
the rules D1 to D16.

## Layout

| Module | What |
| --- | --- |
| `api` | The call layer: each function of the server's `api` schema, its arguments and its answer, over a `Transport`. The only module that knows the JSON shapes, so a change of the server's functions (the security review of task 4.3) changes this module and nothing else |
| `agent` | The loop of §6.7: `hello`, pull until `more` is false (rows, then visibility entries in feed order, then the position), push until the outbox is empty (in a new epoch: the calls that took access away, share deletes, folders, the rest), pull again; resync (D7) with `snapshot`, `ReconcileBegin` / `ReconcileEnd` and the access check; D15 (`rows`); D17 (halve a failing request, block a change that fails alone) |
| `schedule` | When to act: the 250 ms debounce, polling every 2 s (active) or 60 s (idle) while the channel is down and every 5 min while it is up, reconnecting after 1, 2, 5, 10, 30 s then every minute, backoff 1, 2, 4 … s up to 5 min, each with up to 20% jitter |
| `device` | The `Device` trait (§7.2) and `IpcDevice`, which sends each request to the service |
| `http` | HTTPS to the REST interface (`POST <project>/rest/v1/rpc/<function>`, `Content-Profile: api`), token refresh and sign-out at the authentication service |
| `realtime` | The live channel: the realtime service's Phoenix protocol over a WebSocket, a private channel `feed:<user>`, heartbeats every 25 s, the access token renewed on the socket |
| `auth` | `TokenStore`: the Keychain (macOS) and the Credential Manager (Windows) through `keyring`, a 0600 file in the data directory on Linux; `Tokens` never prints a token |
| `model` (feature `model`) | A model of the server's rules C1 to C25, in memory, speaking the same JSON as the SQL functions; `StoreDevice`, the device side over an `iamb_store::Store` in process |
| `bin/iamb-sync.rs` | The thin binary |

## Crates and licences

| Crate | Licence | Why |
| --- | --- | --- |
| `ureq` 3 | MIT or Apache-2.0 | A blocking HTTP client: the agent makes one call at a time and needs no async runtime; small, maintained, TLS through `rustls` |
| `rustls` 0.23 with `ring` | Apache-2.0, ISC or MIT (`ring`: Apache-2.0 and ISC) | TLS without OpenSSL, as the brief asks; one client configuration shared by the HTTP client and the WebSocket |
| `rustls-native-certs` 0.8 | Apache-2.0, ISC or MIT | The operating system's trusted roots (and `SSL_CERT_FILE`), so a machine behind an inspecting proxy works as the system does; avoids bundling a root list |
| `tungstenite` 0.30 | MIT or Apache-2.0 | A blocking WebSocket client over the same `rustls` configuration |
| `keyring` 3 (macOS, Windows) | MIT or Apache-2.0 | The OS keychains through a safe API (the workspace forbids `unsafe`) |
| `getrandom` 0.4 | MIT or Apache-2.0 | Randomness for device ids; already in the lock file |

All are compatible with AGPL-3.0-or-later. None of them is in the service's dependency tree: the
service's no-networking checks (`crates/iamb-service/tests/no_network.rs`, the soak's
`netcheck`, the binary-imports step of CI) still pass.

## Live channel or polling

The live channel is implemented: a poke reaches a device in well under a second, and the 5-second
target (§1) rests on it. Polling every 2 seconds while the user is active is the fallback the
design gives for a channel that is down or refused (`too_many_connections`). In the simulation the
p95 of the time from a write to the other online device holding it is 4.2 seconds over 1,000 runs
with the faults of the simulation (1.0 s at the median; `docs/test-strategy.md` §10).

## Tests

- `tests/two_devices.rs`: two devices of one account against the model; the basic paths.
- `tests/simulation.rs`: the two-device simulation (`docs/test-strategy.md` §10).
  `IAMB_SYNC_SIM_RUNS` (default 120; nightly 1,000), `IAMB_SYNC_SIM_SEED`, `IAMB_SYNC_SIM_REPORT`.
- `crates/iamb-testkit/tests/sync_replay.rs`: every scenario of `spec/sync/scenarios.toml` through
  the real store, the real agent and the model.

Run with the model: `cargo test -p iamb-sync --features model`.

## Decisions and proposals for the design

Things the implementation had to decide, and proposals where the design would change. None of
them is edited into `docs/sync-design.md` here.

1. **A dropped row that a pull skipped is behind.** D15 reports a row as behind when `OutboxAck`
   removes its last entry. A row created and deleted before any entry was handed out is dropped by
   `OutboxRead` (D1) instead, so `OutboxRead` answers such rows too (`behind` beside `changes`), and
   also any row whose earlier read failed. Proposal: say so in D15 and §7.2.
2. **A cascade answer in the short form for a row the device had changed.** A folder's delete names
   a row with the base of its oldest entry; if the device had moved or edited the row before the
   folder took it, the device does not hold the content of that base, and "live in the base
   revision" cannot be put back from what the device holds. The store reads such a row with `rows`
   (D15). Proposal: add this to D4.
3. **A purge keeps a pull's note.** A row a pull skipped for an unsent change, that a purge
   removes with its folder, can be readable elsewhere by then (the server moved it out of the
   tree): the note stays and the row is read with `rows`. Proposal: add to D6 and D15.
4. **A snippet that comes back brings its links** (the model). §5.5 says that a snippet that becomes
   readable through a move brings its links; a snippet that comes back to life (undelete, a restore
   of a folder) is the same case for a reader whose device removed the tag (D16). Proposal: say
   "a snippet that becomes readable or live again".
5. **A share of a deleted folder is refused** (the model): `rejected: parent_gone`, since C6 ended the
   folder's shares and access to tombstones only is nothing. Proposal: add to C11.
6. **A re-pushed share** is judged against the row the restored server holds, not as a stale write
   (C11 against C18): the device could not have seen what was written since. Proposal: add to C18.
7. **`folder_visible` is for trees that existed before the write.** A folder created by a write
   reaches its readers as a `row` entry. Proposal: say so in §5.5.
8. **A tombstone the server never saw is removed with its entries** when the row is dropped (D1), so a
   device does not keep it (S27 says `absent`).
9. **A device keeps its own membership's tombstone, and the group's profile row it refers to**, after
   its membership ended (S30, S35, S79 state it); D6 says the profile row goes. Proposal: D6 removes
   the group's other memberships and keeps the profile row while the user's own membership row is
   there.
10. **The IPC protocol is 4** (§7.2 says so): `OutboxRead`, `OutboxAck` and `ApplyRemote` changed.
    The rest of §7.2 that the agent needs is added; the account endpoint (task 4.5), `Adopt` /
    `AdoptMerge` over IPC, attachment files (task 4.6) and `ApproveSnippet` by content (D10) are not
    in this task.
11. **Every sign-out is remembered until the resync after the next sign-in.** D14 says signing in
    again resyncs; after a confirmed sign-out nothing in `sync_state` said so, and a pull from the
    kept position would not bring back what the sign-out removed. `sync_state.signed_out_at` (store
    migration 3) is set by `SignedOut`, confirmed or not, and cleared with `set_aside_at` by the end
    of a whole-library reconcile; `SyncState` carries it as an optional field, and the agent resyncs
    at the first `hello` while it is set. Proposal: add the column to §7.1.
12. **A confirmed sign-out keeps the user's own shares** and the profile rows of the groups they
    name: a share of a folder of the personal profile is part of the user's own library (D14), and
    its row refers to the group's profile. Proposal: say so in D14.
13. **S60 was corrected** (`spec/sync/scenarios.toml`, rendered into §12): with the second pull of
    §6.7 step 4, A's round issues two positions, so the copy B, which presents the position both
    had at the copy, is the installation caught by C25 at its first call, not A. B signs itself
    out, signs in again as a device of its own and pushes b1, which wins with the conflict the
    scenario states; the outcome on the server is unchanged.
14. **The replay compares a device's tombstone as I1 does**: for a row the server holds as a
    tombstone, a device may hold the tombstone or nothing (S63: a link's tombstone leaves the
    member's device with its tag, D16, because a link refers to its tag).
15. **A refused folder change without a row reads back what its purge took.** When the server
    answers a folder's change `rejected` without a row, the store removes the folder's subtree as
    the device holds it, which can still hold a child the server moved out (kept under the folder
    here by an unsent change) with rows a pull had just brought. Every row the purge takes that the
    server once gave is reported as behind and read with `rows` (D15); what the server no longer
    has stays away. Proposal: add to D15.
16. **A link with an unsent change keeps its tag or attachment** through the clean-up of D16:
    the link's row refers to it, and removing the tag removed the link and its change with it.
17. **`rows` follows references.** A row read with `rows` that the store refuses because a row it
    refers to is missing (a link whose tag the clean-up removed) is read again in one batch with
    what it refers to, up to three levels; in one batch, so that the clean-up at the end of the
    batch does not remove the tag again before the link is there.
18. **A request the server undid** (`invalid` for the whole push: an op failed in the database,
    4.3) is sent again in halves, and a single change that is answered so is set aside as blocked
    with the reason `invalid` (D17), so that it does not hold up the rest of the outbox.

## The server as of task 4.3

The call layer (`api`) and the model follow the server after the fixes of the security review
(commit 4615ae0, ADR-0031). In the model: a conflict reaches a party, in the list, the feed and
`get_conflict`, only while the party can read the row; `get_conflict` offers only the versions the
caller wrote or could read while they were the row (the revision each access began, `since_rev`);
`resolve_conflict` answers `not_found` to a party who cannot read the row (also for "keep"),
`invalid` for a version not on offer, and `forbidden` for a restore the caller may not write,
which leaves the conflict open; a delete of a row the caller cannot read is `unchanged`; an op on
a profile or group without a grant is `unchanged` (a delete) or `rejected: parent_gone`; a create
whose id is in use in another profile is `blocked: invalid`; `content_by` is null when the writer
could not read the base version, and `follows` counts only the writing device's own ops; a repeated
`create_group` after leaving is `invalid`, a repeated `accept_invitation` after removal
`invitation_invalid`, and a non-member is told `not_found` before `forbidden`. The agent already
followed the cursor of short `snapshot` and `fetch_folder` pages and read the short cascade entries
(`fetch: true`) with `rows`; it now also narrows a request answered `invalid` (decision 18). Not
modelled: the rate of `revoke_device` and `sign_out_everywhere` (no rate limits in the model), a
session past `not_after` (the same `session_refused`), the upload limits and
`attachment_upload_cancel` (task 4.6), and a request-level `invalid` (a database failure; tested
with a transport that answers it).
