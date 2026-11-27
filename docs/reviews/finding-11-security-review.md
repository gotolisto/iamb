# Security review of the installed-binary check (finding 11)

Date: 2026-11-27 · Commit reviewed: `4093396` ("Accept the installation's own UI host in release
builds (finding 11)") · Reviewer: independent, did not write the code and judged it from the
code, not from the commit message

Scope: `crates/iamb-platform/src/installation.rs`, `crates/iamb-platform-macos/src/endpoint.rs`,
`crates/iamb-platform-windows/src/endpoint.rs`, `crates/iamb-ipc/src/roles.rs`,
`apps/desktop/src-tauri/src/{check.rs,main.rs}`, `apps/desktop/release/smoke/{macos.sh,windows.ps1,linux.sh}`,
the release entitlements, and what `docs/architecture.md` §6.1 and `docs/release.md` now claim.

The review changed no file of the tree and added this one. Everything was run on Linux: the
five unit tests of `installation.rs` and a scratch test of its pure functions (kept outside the
tree). The two endpoints that carry the fix exist only on macOS and Windows and were not run;
every finding says "by reading" or "shown by a run".

## 1. Verdict

The design is right and, for a signed installation on macOS, the decision rests on the right
fact: the code object of the connecting process, taken from its audit token and checked against
a team requirement. A copy elsewhere, a process whose id was reused, a process that ran another
program after it connected, and another signer are refused there.

It is not finished. Two medium findings should be fixed before the check is called closed:

1. **A signed installation can be made to fall back to the path check** (finding 1). Whether
   "this installation is signed" is read lazily, from the file on disk, at the first peer check,
   and any failure to read it counts as "unsigned". The sentence the fix is built around, "a
   signed installation never falls back to the path check", does not hold against a process of
   the user that can write into the installation, which on both systems it can.
2. **The smoke tests pass while the service's check is broken** (finding 2). The "a copy is
   refused" step is satisfied by the copy refusing the service, before the service decides
   anything. A service that accepted every client would pass both steps.

The question asked, whether a process of the same user that is not a member can be trusted as
`ui` or `service`:

| Installation | Answer |
| --- | --- |
| macOS, signed | Not by any way found, except through finding 1 |
| macOS, unsigned or ad hoc | Yes, by design: the check is the path, and the app bundle of a drag-install belongs to the user. Stated in the module docs |
| Windows, signed | Yes, in three ways (findings 1 and 4). A process of the same user at the same integrity level can also write into the service's memory, which no check of this kind stops |
| Windows, unsigned | Yes, by design, as on macOS |

## 2. Findings

### Medium

#### 1. "Signed" is decided from the disk, lazily, and a failure means "unsigned"

Where: `crates/iamb-platform-macos/src/endpoint.rs:228-241` (`own_team`), `:306-316`;
`crates/iamb-platform-windows/src/endpoint.rs:361-371` (`own_image`), `:386-396`.

By reading; not run.

Both endpoints read this process's own signature the first time a peer is checked, keep the
answer for the life of the process, and treat every way of not getting one (an API error, no
team identifier, a verification that fails) as `OwnSigning::Unsigned`. From then on the process
accepts whatever is at a member's path. On macOS the signature is read from the static code,
the file on disk (`copy_static_code`, then `copy_signing_information`); on Windows with
`WinVerifyTrust` on the path of the running image.

Scenarios, all for a process of the user without any grant of its own:

- **macOS.** The bundle of a drag-installed app belongs to the user.
  1. Leave `run/desktop.quit` and end the UI host, so that no genuine client connects (the
     marker keeps the supervisor from opening it again).
  2. Create `run/restart-worker`. The supervisor starts a new worker from the genuine, signed
     helper. It holds the keyboard permissions, and it has not read its own signature yet.
  3. Unlink the helper's executable and put any file in its place (the running worker keeps
     its image), and put a program at `Iamb.app/Contents/MacOS/iamb-sync`, a member that does
     not ship (finding 5), or in place of `iamb-desktop`.
  4. Start that program from there and connect. The worker now reads its own signature for the
     first time: the static code of its path is gone or unsigned, `own_team()` is `None`, the
     installation counts as unsigned, and the program at the member's path is accepted, as
     `sync` (`OutboxRead`, `ApplyRemote`) or as `ui`.

  Whether the system resolves the running process's static code to the replaced path, or
  fails, was not checked on a Mac; both end in `None`.
- **Windows.** Any of: add the publisher's certificate to the current user's "Untrusted
  Certificates" store (a registry key the user's processes can write), so that `WinVerifyTrust`
  fails for the service's own image; rename the running `iamb-service.exe` and put an unsigned
  file at its path before the first check; or simply wait for the certificate to expire on a
  build whose signature carries no timestamp, after which every installation of that build
  degrades by itself. Then drop `iamb-sync.exe` or `iamb.exe` into `%LOCALAPPDATA%\Iamb`.

What the role gives such a process: not keystrokes (no message carries them), but the library,
the last form answers, and on macOS a service that types what it is asked to type under the
user's Accessibility permission.

Fix.

1. Pin the signer at build time for builds that are meant to be signed: the team identifier on
   macOS and the certificate's public-key hash on Windows, compiled in from the release
   pipeline. Such a build never asks whether it is signed; it requires that signer of every
   peer. A build without the pin is an unsigned build and says so in its log.
2. Read this process's own signing state once, first thing in `main`, not at the first
   connection, and from the running code: on macOS check the dynamic code object of the process
   itself against `anchor apple generic` and read the team identifier from it, not from the
   static code's path.
3. Fail closed. Three states, not two: signed (by whom), unsigned (no signature at all:
   `errSecCSUnsigned`, `TRUST_E_NOSIGNATURE`), and could-not-tell. The third trusts no peer and
   is logged. Never keep "unsigned" when it came from an error.
4. Test: `trusted` has no input for the third state today. Add it, and a test for each endpoint
   that an error reading the own signature refuses every peer.

#### 2. The smoke tests cannot tell a broken service-side check

Where: `apps/desktop/release/smoke/macos.sh:88-95`, `windows.ps1:102-110`,
`apps/desktop/src-tauri/src/check.rs:30-45`.

By reading.

`--check-service` connects with `PeerPolicy::for_build()`, so the client checks the server
first. A copy of `iamb-desktop` outside the installation belongs to no installation and trusts
no peer: it refuses the service itself. The script asserts a non-zero exit and an output that
starts with "service check failed", which that client-side refusal produces. The service's own
decision about the copy is never observed.

So both steps pass with a service that accepts every client; with one that accepts anything
named `iamb-desktop`; and with a signed service that fell back to the path check (finding 1).
The steps do catch a service that refuses the installed UI host, which is the regression the
commit set out to prevent, and a build shipped with the debug policy.

The signed half of the decision is not run at all unless the pipeline has signing set up
(`docs/release.md` says so). The requirement string, the team identifier read, the leaf
certificate comparison and the refusal of an unsigned peer by a signed service have never been
executed anywhere.

Fix.

1. Assert on the service's side: after the copy's attempt, the worker's log must hold
   `rejected a client:` once more than before (`crates/iamb-service/src/worker/ipc.rs:128-129`),
   and it must not have gained one for the installed UI host. Or give `--check-service` a
   switch, for the smoke test only, that skips the client's check of the server, so that the
   refusal can only be the service's.
2. On a signed run add the case that matters: replace `iamb-desktop` inside the installed app
   with an unsigned build (macOS: an ad hoc re-signed copy) and require the service's log to
   refuse it; and drop an unsigned `iamb-sync` at its member path and require the same.
3. Until a signed run exists, say in `docs/architecture.md` §8 that finding 11 is closed for
   unsigned builds and written but unexercised for signed ones. "Closed" is stronger than the
   evidence.

### Low

#### 3. The macOS smoke test deletes the account's real data root

Where: `apps/desktop/release/smoke/macos.sh:76`, `:98`.

By reading. The script starts the installed helper for the default data root
(`~/Library/Application Support/Iamb`, the only one a release helper serves) and ends with
`rm -rf "$default_root"`. It checks that `/Applications/Iamb.app` does not exist before it
starts, but not the data root. On a machine that has Iamb's data and no installed app (a
developer who runs development builds, or who removed the app), running the script serves the
real library to the test and then deletes it. Fix: fail at the start when the data root exists,
as for the app, and remove only what the script created.

#### 4. Windows: the file is verified, not the running image, and the process is found by its id

Where: `crates/iamb-platform-windows/src/endpoint.rs:753-764`, `:264-286`, `:374-398`.

By reading. The macOS endpoint identifies the peer by audit token; the Windows endpoint by the
process id the pipe reports, then by the image path of whatever has that id when it looks, then
by the signature of the file at that path.

- **Process id reuse.** A client connects (the pipe keeps a pending instance open for the next
  client, `:753-756`, so a connection can be made before the server looks at it), hands the pipe
  handle to another process and exits; the attacker starts the genuine `iamb-desktop.exe` until
  one gets the same id; the server then finds the genuine image at a member's path, signed by
  the same certificate, and trusts the handle's holder as `ui`. The window is short unless the
  server is slowed, which a process of the same user can do.
- **Image and file apart.** With write access to `%LOCALAPPDATA%\Iamb` (the user has it): start
  a process whose image was mapped from other content at `iamb-desktop.exe`'s path and put the
  genuine signed file back before connecting. The path is a member's, the file's signature is
  the installation's, the code that runs is neither. The sentence "a signed installation never
  accepts an unsigned peer" should read "a peer whose file is unsigned" on Windows.

Neither gives more than writing into the service's memory, which a process of the same user at
the same integrity level may do on Windows; §8's threat model already excludes it. Fix what is
cheap and say the rest: open the process first and keep the handle while it is examined (an
open handle keeps the id from being reused), refuse a process created after the pipe instance
it connected to was created, and compare the image's file identity with the member's
(`FILE_ID_INFO`) instead of the hard-coded `true` at `:398`. State the limit in §6.1.

#### 5. Two members do not exist, so their paths are free to take

Where: `crates/iamb-platform/src/installation.rs:48`, `:67-88`.

Shown by a run (the scratch test): `…/Contents/MacOS/iamb`, `…/iamb-sync`,
`%LOCALAPPDATA%\Iamb\iamb.exe` and `iamb-sync.exe` are members, and `roles.rs` gives them `cli`
and `sync`. Neither program ships. In an unsigned installation, or one that fell back
(finding 1), a new file at one of those paths is trusted without anything being replaced, so
nothing looks changed. On a signed macOS installation the opposite will happen when they do
ship: the requirement names only `org.iamb.Iamb` and `org.iamb.Iamb.Service`
(`installation.rs:42`), so they will be refused. Fix: list as members only what the installer
puts there, and add each program together with its bundle identifier when it ships.

#### 6. Windows: the installation is found by comparing a canonical path with an environment variable's text

Where: `crates/iamb-platform/src/installation.rs:129-157`,
`crates/iamb-platform-windows/src/endpoint.rs:378`.

Shown by a run of the pure functions, on Linux.

- **Fails closed where the two are written differently.** This process's own path is
  canonical; `%LOCALAPPDATA%` is taken as text. A profile reached through a junction or
  redirected to another drive, an 8.3 name in the variable, and a mapped drive all give "no
  installation", and then every peer is refused: the symptom finding 11 was opened for,
  on those machines. A peer that the system reports with an 8.3 name is refused the same way
  (`member_for` wants the reported and the canonical path equal as text).
- **The variable is the starter's.** A copy of the service started with
  `LOCALAPPDATA=X:\fake` from `X:\fake\Iamb` is an installation (shown). It does not make the
  running service trust anything more, so this matters only for an unsigned build, where the
  copy accepts the attacker's client once the real service is ended and its locks are free.

Fix: take the folder from the system (`SHGetKnownFolderPath(FOLDERID_LocalAppData)`), make it
canonical, and compare canonical with canonical; for the peer, compare the canonical path with
the member's and drop the textual "reported equals canonical" test, which on both systems
checks nothing the system does not already guarantee (finding 8).

#### 7. Names are folded with Unicode lower case, which is not how the file systems compare

Where: `crates/iamb-platform/src/installation.rs:161-178`, `:203-214`.

Shown by a run: a path whose `k` is U+212A (KELVIN SIGN) has the same key as the member's path.
NTFS and APFS treat it as another name, so it is another file or folder. A folder
`…\AppData\Loca<U+212A>\Iamb\iamb-desktop.exe` is therefore "a member, at its own path" for an
unsigned Windows installation, without write access to the installation itself, and the
endpoint passes `same_file = true` there. (As a file name the trick fails later: the role is
matched in ASCII only.) On macOS the device-and-inode comparison catches it. Fix: fold ASCII
only, or compare file identities on Windows too (finding 4).

#### 8. On macOS the "without a link" and "same file" tests do not test the process

Where: `crates/iamb-platform-macos/src/endpoint.rs:270-276`, `:292-297`, `:317-323`.

By reading. `proc_pidpath` builds the path from the executable's vnode, so it holds no symbolic
link: a process started through a link to a member is reported at the member's path and is
accepted, and with a hard link the kernel may report either name. Both are the member's own
code, so nothing is gained, but the rows "a link to a member" and "a hard link outside the
installation: no" of the table in `installation.rs` describe a refusal the code cannot make.
`same_file(reported, &member.exe)` compares two look-ups of what is, after `member_for`, the
same path; it tells two files apart on a volume that distinguishes case and nothing else. What
ties the unsigned process to the file is the path alone. That is consistent with "an unsigned
installation trusts what is at its members' paths"; the table of `docs/architecture.md` §6.1
("the executable is the member's file (device and inode)") says more than that: the comparison
is between two files found by path, not with the file the process runs from. The system offers
no direct way to ask for that file's identity, so the fix is the wording.

#### 9. The bundle path from the code-signing API is compared as text

Where: `crates/iamb-platform/src/installation.rs:219-224`.

Shown by a run: `/Applications/Iamb.app/` (a trailing separator) does not agree with the member
`/Applications/Iamb.app`. If the path the API returns for a bundle ever carries one, every UI
host is refused. It fails closed and the smoke test's first step would show it, on a Mac.
Compare by components.

#### 10. Smaller

- **The client accepts any member as the server.** The UI host checks that the server is a
  member, not that it is `iamb-service` (`roles.rs` binds names for clients only). When
  `iamb-sync` ships with its own endpoint, a link from `run/service.sock` to its socket has the
  UI host speak to the sync agent. Check the expected member per endpoint.
- **An older signed build is a member.** The signed check accepts any build of Iamb by the
  same signer at a member's path, so a process that can write there can put back an older
  `iamb-desktop` with a known flaw. A version floor (the peer's version at least this build's)
  would close it; note it for the update design.
- **An installation is any `.app`, anywhere** (`installation.rs:92-106`, shown: `/tmp/x/Evil.app`
  is one). A copy of an unsigned service inside a bundle of the attacker's making trusts the
  attacker's `iamb-desktop`. Only the unsigned case, and only for a service the attacker
  started.

## 3. What held

- **macOS, signed, the audit token.** `LOCAL_PEERTOKEN` is read with its length checked and
  given to the code-signing API as the audit attribute; the code object it yields is what the
  requirement is checked against (`endpoint.rs:145-181`, `:245-267`, `:299-314`). The process id
  is used only to find the path, and the path's member must be the member the token's code
  names. A process that connects and then runs a genuine member, and a process id reused after
  the connection, no longer match their token. By reading.
- **The requirement.** `anchor apple generic`, the leaf's organisational unit equal to this
  process's team identifier, and one of two identifiers; the team identifier is checked to be
  ten letters or digits before it is quoted into the string (unit test passes). Another team,
  an ad hoc signature and no signature fail it. No network access is requested.
- **No injection path through entitlements.** Both entitlement files are empty, so a signed
  build under the hardened runtime loads no foreign library and allows no debugger of the same
  user (reading the files; the smoke test checks the runtime flag on a signed run).
- **Windows, the certificate.** The peer's leaf certificate must equal this image's, byte for
  byte, after a successful verification, so a root certificate added to the user's store does
  not help an attacker (it does help make verification fail: finding 1). Revocation and network
  retrieval are off.
- **Copies.** A copy of a member outside the installation is no member on either system
  (unit tests and the scratch run).
- **Roles.** The role still follows the executable's name, `iamb-service` gets no client role,
  and a peer whose executable is unknown is untrusted under the installed-binary policy.
- **`--check-service`.** One `GetStatus`, no window, no instance lock, the build's own policy;
  it prints the service's version and process id and nothing else. Nothing to misuse.
- **The `LOCALAPPDATA` override** cannot make a running service trust more: its own
  environment is fixed at its start (finding 6 has the rest).

## 4. What could not be checked

- **Everything on macOS and Windows.** In particular: that `SecCodeCopyGuestWithAttributes`
  accepts the token for an unsigned process (the unsigned path depends on it); what the static
  code of a process resolves to after its file was unlinked or replaced (finding 1); the path
  `proc_pidpath` reports for a hard link and for a firmlinked volume; whether
  `SecCodeCopyPath` ever returns a trailing separator; what `QueryFullProcessImageNameW` reports
  after a rename and through an 8.3 name; the pending pipe instance's window.
- **Whether the smoke tests were run on real runners with this commit.** Nothing in the tree
  says so; on Linux the step runs but Linux has no installed-binary check.
- **The signed path, anywhere.** No signed build exists yet.

Tests run: `cargo test -p iamb-platform --lib -- installation` (5 passed) and a scratch test of
`windows_members`, `windows_key`, `member_for`, `member_for_code_path` and `macos_members` for
the edge cases above, both built outside the shared target directory.

## 5. Fixes

Date: 2026-11-27 · By the author of the fix, not the reviewer · Not committed when written. As
for the review, everything was run on Linux; the two endpoints were linted for their targets
(`cargo clippy --target x86_64-pc-windows-msvc` and `aarch64-apple-darwin`, with and without
`probe`) but not run. "Before" means the new test run against the code of `874aeb2`.

| # | Finding | Fix | Test | Before → after |
| --- | --- | --- | --- | --- |
| 1 | "Signed" read lazily from disk; an error meant "unsigned" | Three states (`OwnState`: signed by a signer, unsigned, could not tell); "could not tell" trusts no peer. Read once, first in `main` of the service and the UI host (`iamb_ipc::read_own_signing`, logged as `installed-binary check: …`), from the running code: macOS checks the dynamic code object (`SecCodeCopySelf`, `SecCodeCheckValidity` without and with `anchor apple generic` and the team), only `errSecCSUnsigned` or a valid ad hoc signature is "unsigned"; Windows maps `WinVerifyTrust` on the own image: only `TRUST_E_NOSIGNATURE` is "unsigned". The signer is pinned at build time when the pipeline signs: `IAMB_SIGNER_TEAM` from `APPLE_TEAM_ID` (helper and app builds), `IAMB_SIGNER_THUMBPRINT` from the imported certificate (the service is now built after "Prepare Windows signing"); a pinned build never asks, a malformed pin trusts no one. Not pinned: Windows with `WINDOWS_SIGN_COMMAND`, whose certificate is not known at build time; it reads its signature at its start | `an_error_reading_the_own_signature_trusts_no_peer`; each endpoint: a test binary is never "signed" | New API, no before run; after: passes. The endpoints' tests run only on their systems |
| 2 | The smoke tests could not tell a broken service-side check | `iamb-desktop --check-service --skip-server-check` (the client takes the server as the same user's, so a refusal can only be the service's). macOS and Windows: the copy runs that way, must fail, and `worker.log` must gain `rejected a client: the peer is not the installed Iamb binary`; the installed UI host must leave no rejection. Signed macOS runs: an ad hoc re-signed `iamb-desktop` put in place of the real one must be refused and logged. Signed Windows: not exercised (no unsigned build of a signed release on the runner), said in the script and in `docs/release.md`. §8 now says "closed for unsigned builds; written, and not yet exercised, for signed ones" | The smoke scripts (CI only) | Runs on the next installers workflow |
| 3 | The macOS smoke test deleted the real data root | It fails at the start when `~/Library/Application Support/Iamb` exists, and removes only what it created | Script (CI only) | – |
| 4 | Windows: file, not image; the process found by its id | The process is opened once and the handle kept while it is examined; a process created after the connection (`GetProcessTimes` against the time the connection was seen) is refused; the file identity (volume serial number, file index) of the peer's canonical path is compared with the member's instead of `true`. The rest (a file put back after other content ran from its path) is stated as a limit in §6.1 | `a_process_created_after_the_connection_is_not_the_client` (the pure rule) | New, passes |
| 5 | Two members that do not ship | Members are the UI host and the service only; the others are added with their bundle identifiers when they ship | `review::programs_that_do_not_ship_are_no_members` | fails (`…/Contents/MacOS/iamb` was a member) → passes |
| 6 | Windows installation found by comparing with an environment variable's text | The directory is the `LocalAppData` known folder (`SHGetKnownFolderPath`) and `Iamb`, made canonical (junctions, 8.3 names, redirection resolved), compared with the canonical own directory; the peer's canonical path is compared with the member's, the textual "reported equals canonical" test is gone | `the_windows_installation_is_the_canonical_per_user_directory` | The known-folder call runs on Windows only |
| 7 | Unicode folding | ASCII letters only are folded, on both systems | `review::only_ascii_letters_are_folded` (a Kelvin sign in a user name) | fails (the keys were equal) → passes |
| 8 | macOS link and same-file wording | The tables in `installation.rs` and §6.1 now say what is checked: the path the system reports (no link on macOS), and the identity of the file found at the member's path, not of the mapped image | – | – |
| 9 | Bundle path compared as text | Paths are compared by components (trailing, repeated and `.` separators do not matter) | `review::trailing_and_repeated_separators_do_not_matter` | fails (`/Applications/Iamb.app/` matched nothing) → passes |
| 10 | Smaller | Left as documented in the review | – | – |

What still could not be checked here: every call into the Security framework and the Windows
APIs (in particular that `SecCodeCopySigningInformation` describes the dynamic code object it
is given, and what `GetProcessTimes` and `SHGetKnownFolderPath` return on a redirected profile),
and the smoke scripts' new steps, which run in the installers workflow only.

## 6. Re-verification

Date: 2026-11-27 · Commit reviewed: `2a58a18` ("Close the strict review's findings on the
installed-binary check") · By the reviewer of sections 1 to 4, not the author of the fix · Same
limits: run on Linux only (the ten unit tests of `installation.rs` pass; a second scratch test
of its pure functions, kept outside the tree); the two endpoints, the workflow and the smoke
scripts were read, not run.

### 6.1 Verdict

Both medium findings are closed as written, and the fixes brought no new medium or high
problem. Two things keep this short of "done": nothing of it has run on macOS or Windows yet
(section 5 says the same), and one kind of signed Windows build still has no compiled-in signer.

### 6.2 The findings

| # | Now | Evidence |
| --- | --- | --- |
| M1, "signed" read lazily from disk; an error meant "unsigned" | **Closed** for builds with a compiled-in signer and for every error case; **partly** for Windows builds signed through `WINDOWS_SIGN_COMMAND` (6.3, a) | Reading: `pinned` returns `Signed(pin)` without asking the system (`installation.rs:352-365`), so such a build demands its signer of every peer whatever happens to its files. Without a pin the state is read once, first in `main` of the service and the UI host (`crates/iamb-service/src/main.rs:150`, `apps/desktop/src-tauri/src/main.rs:198`), and there are three states; `trusted(…, Unknown, …)` is `false` for every input (run: all four combinations). Run: a malformed pin gives "could not tell", not "unsigned"; a Windows verification that fails for distrust or expiry gives "could not tell"; only "no signature at all" gives "unsigned" |
| M2, the smoke tests could not tell a broken service-side check | **Closed** as written; not run yet | Reading: the copy runs with `--skip-server-check`, so the client refuses nothing, and both scripts require the worker's log to gain `rejected a client: the peer is not the installed Iamb binary` (the text is `PeerRejection::UntrustedBinary`'s, `crates/iamb-platform/src/endpoint.rs:123`) and to hold none after the installed UI host connected. A signed macOS run replaces `iamb-desktop` in place with an ad hoc re-signed copy and requires the same refusal. The same case on Windows is not exercised, and the script says so. `docs/architecture.md` now reads "closed for unsigned builds; written, and not yet exercised, for signed ones" |
| L3, the macOS smoke test deleted the real data root | **Closed** | `macos.sh:34-35` fails at the start when the data root exists, before anything is started |
| L4, Windows: the file, not the image; the process found by its id | **Partly**, and the rest is stated | The process handle is held while the peer is examined and the file identities are compared. The creation time is compared with the moment the server noticed the connection (`endpoint.rs:913`), not with the moment the client connected, so an id reused in between is still not seen; the module docs say "created before it connected", which is a little more than the code does. 6.3, b |
| L5, members that do not ship | **Closed** | Run: `iamb` and `iamb-sync` are no members on either system; two members each |
| L6, the installation found from an environment variable's text | **Closed** by reading | `SHGetKnownFolderPath(FOLDERID_LocalAppData)` joined with `Iamb` and made canonical, compared with the canonical own directory; the peer's canonical path is compared with the member's. Not run on a redirected profile |
| L7, Unicode folding | **Closed** | Run: a folder whose `k` is the Kelvin sign is no member; ASCII case still folds |
| L8, the macOS "link" and "same file" wording | **Closed** | The module docs and §6.1 now say what is compared: the path the system reports and the file found at the member's path |
| L9, the bundle path compared as text | **Closed** | Run: a trailing separator, a doubled one and a `.` agree with the member |

### 6.3 The three points asked about, and what is new

**The build-time pins (`.github/workflows/installers.yml`).** macOS: `IAMB_SIGNER_TEAM` is set
from `APPLE_TEAM_ID` for the helper's build and for the app's build, only in the Developer ID
mode. Windows: `IAMB_SIGNER_THUMBPRINT` is taken from the imported certificate itself and
written to the job's environment before the service and the installer are built (the service's
build step was moved after it). Both are read with `option_env!`, which the compiler records,
so a cached unpinned build is not reused.

- a. **Low: a Windows build signed through `WINDOWS_SIGN_COMMAND` has no pin.** The thumbprint
  is written only in the `certificate` mode (`installers.yml:217`); `docs/release.md` says so.
  Such a build reads its own signature at its start, which closes every error case, but it
  still becomes "unsigned" when its own file has no signature. A process of the user that
  strips the Authenticode signature from the installed `iamb-service.exe` (the code stays the
  same and runs) and replaces `iamb-desktop.exe` gets the path check at the next start; a
  pinned build would still demand the signer. Signing through a service is the usual way to
  hold a certificate today, so this is likely to be the mode that ships. Fix: require the
  certificate's thumbprint as a repository variable in that mode and fail the job without it.
- **The macOS pin is the notarization team, not read from the signing identity.** If the two
  ever differ, every peer is refused; it fails closed and the signed smoke run shows it. On a
  signed run, also assert the line `installed-binary check: signed by <signer>` in the logs
  against the installer's actual signer, on both systems; today a signed run without the pin
  would pass.

**The "could not tell" state.** Sound in the pure part (above). Two notes on how the macOS
endpoint fills it for a build without a pin (`endpoint.rs:267-296`), both by reading:

- The team identifier is asked of the running code object, but the signing-information call
  describes the code on disk for anything except the dynamic flags; the comment says otherwise.
  It does no harm as written, because validity is checked first on the running code and the
  team is then confirmed by a requirement checked against the running code.
- c. **Low: "unsigned" is taken from the validity check's `errSecCSUnsigned`**, which also
  looks at the file on disk. A file swapped for an unsigned one in the first moments after the
  worker starts (the restart request gives the moment) may give that answer for a signed
  process. Only for a build without a pin, and the window is now the start of `main`. Take
  "unsigned" from the kernel's code-signing flags of the process instead.

**`--skip-server-check`.** It cannot weaken a production client. It is read in one place, the
`--check-service` branch (`apps/desktop/src-tauri/src/main.rs:154-159`, `:210`); the UI host's
own connection always uses the build's policy. With it the genuine UI host sends one hello and
one `GetStatus` to whatever serves the data root it was given, and receives a status: nothing
of the user's leaves, and towards the real service it is the installed UI host doing what it
may do anyway. Two small things: the flag is accepted without `--check-service` and then
ignored (make that a usage error), and the version string of a server that was not checked is
printed and written to the log as it comes (drop control characters).

**New with the fixes.**

- b. **Low: the creation-time test compares wall-clock times** (`endpoint.rs:913`,
  `installation.rs:427-429`). When the system clock is set back after the UI host started (a
  time correction, a zone or date change by hand), the UI host's creation time is later than
  "now" and the service refuses it as `Unidentified` until the clock passes that time. It fails
  closed and the UI says "Iamb is not running"; restarting the UI host cures it. Log the reason
  as its own line, and consider a tolerance of the size of ordinary corrections.
- Finding 10's three notes are unchanged, as section 5 says.

### 6.4 Not checked

Everything that needs a Mac or Windows: the Security framework and Windows calls, the
known-folder path on a redirected profile, the new smoke steps (they run in the installers
workflow only), and any signed build. The scratch test covered `windows_members`,
`member_for`, `member_for_code_path`, `trusted`, `pinned`, `classify_macos`,
`classify_windows` and `created_before`.
