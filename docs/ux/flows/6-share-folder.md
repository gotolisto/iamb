# Flow 6: Share a folder

Mock-up: [`../mockups/share-folder.html`](../mockups/share-folder.html) · Specification:
[`../spec.md`](../spec.md) §4.2 and §4.10 · Built in Phase 4 (tasks 4.4 and 4.5); designed
here so that the library of Phase 3 leaves room for it

## Goal

One person shares exactly one folder (with its sub-folders) with a group, a family or a team,
and everyone in the group has its snippets within ten seconds. A person who receives snippets
stays in control of their own machine: a snippet written by someone else cannot press keys,
wait, read the clipboard or type several lines until they have looked at it and approved it
(ADR-0022).

The data model fixes the shape (ADR-0014): folders are shared to groups, never to single
addresses and never by link; "can use" and "can use, add and change" are the two permissions;
only the folder's owner can rename, move, delete, share or unshare it.

## Entry points

| Who | From |
| --- | --- |
| The owner of a folder | The folder header's [Share…] button; the folder's context menu; ⌘⇧S / Ctrl+Shift+S with the folder selected |
| A person who was invited | The banner at the top of the library after signing in with the invited address |
| A person who received snippets | "Needs review" in the sidebar; the banner in the shared folder; a "Needs your review" row in the list or the palette |

## Steps: the person who shares

| # | Screen and copy | Keys | Service |
| --- | --- | --- | --- |
| 1 | **The folder.** Header: "Shop replies" · badge "Only you" · [Share… ⌘⇧S]. | ⌘⇧S / Ctrl+Shift+S | `AccountStatus` (account endpoint): signed in or not |
| 2 | **Sign in** (skipped when signed in). Dialog "**Sign in to share “Shop replies”**" · "Sharing needs an Iamb account, so the people you share with can be recognised. Signing in happens in your browser; Iamb never sees your password." · "Your other folders stay on this computer until you choose to sync or share them." · [Cancel] [Continue in browser ⏎]. Then "**Finish signing in in your browser**" · "This window continues by itself when you are done." · "Open the browser again" · [Cancel esc]. | Enter; Esc cancels at any point | `BeginSignIn(provider)` → system browser, authorisation-code flow with PKCE (ADR-0020) → `AccountChanged`. `CancelSignIn` on cancel |
| 3 | **Share.** Dialog "**Share “Shop replies”**". **With**: a list of the user's groups, each "Brindlewood team · Team · 4 people: you, Tomás, Jun and Odile", and "New group…". **They can**: "Use the snippets" / "Use, add and change snippets". Summary: "All 19 snippets and the sub-folders of “Shop replies” go to everyone in Brindlewood team, now and as the group grows. Only you can rename, move, delete or stop sharing the folder." · [Cancel] [Share ⏎]. | ↑ ↓ inside a list, Tab between them, Enter shares | `Profiles`, `Members(profile)`; `SetFolderShare(folder, profile, permission)` with `view` or `edit` |
| 4 | **New group** (from "New group…"). **Name**, **Kind** (Family / Team), **Invite by email** with a role (Member / Admin) and [Invite]; help "They get an email asking them to open Iamb. They accept inside Iamb." Below, the people: "Ines Marchetti (you) · Owner", "tomas@brindlewood.example · Invited just now · Waiting · Withdraw". [Cancel] [Create group]. Returns to step 3 with the new group selected. | Enter invites the typed address and clears the field; ⌘⏎ / Ctrl+Enter creates | *Needed:* `CreateProfile(kind, name)`; then `Invite(profile, email, role)`; *needed:* list and withdraw sent invitations |
| 5 | **Shared.** Toast: "Shared. Brindlewood team now has “Shop replies”." The header's badge reads "Shared with Brindlewood team"; the folder has the people icon in the sidebar; [Manage sharing] opens the same dialog with the current choice and [Stop sharing]. | — | `SyncProgress` until pushed. Recipients: the server puts `folder_visible` in their feed; their sync agent fetches the folder and applies it (`ApplyRemote`); their UI hears `LibraryChanged` |

Stopping: [Stop sharing] asks "**Stop sharing “Shop replies”?** It is removed from the devices
of the 3 other people. Your copy stays." [Stop sharing] [Cancel] → `RemoveFolderShare`; the
other devices purge the folder (`PurgeFolder`).

## Steps: the person who receives

| # | Screen and copy | Keys | Service |
| --- | --- | --- | --- |
| 6 | **The invitation.** The email says only that Ines invited them and to open Iamb. In the library, after signing in: banner "**Ines Marchetti invited you to “Brindlewood team”.** You will get the folders shared with this group. Nothing of yours is shared." [Decline] [Join]. | Tab reaches the banner first; Enter joins | `Invitations` after sign-in and on `AccountChanged`; `AcceptInvitation(id)`; *needed:* `DeclineInvitation(id)` |
| 7 | **The folder arrives**, within 10 seconds. Toast: "“Shop replies” was shared with you: 19 snippets." The folder appears in the sidebar with the people icon. Header badge: "Shared by Ines · you can use". When some snippets are held: banner "**2 snippets wait for your review.** They press keys or read your clipboard, so Iamb asks you before it types them. The other 17 work now." [Review]. Held rows carry "Needs review" and say why: "Several lines · by Ines", "Reads the clipboard · by Ines". | Enter on the banner opens the first review | `LibraryChanged`; `ListPendingReviews`; `Health` for abbreviations that lost to the user's own. *Needed:* folder and snippet lists that include other profiles |
| 8 | **Review, first time.** Dialog "**Review “Paste tracking link”**" · "Written by **Ines Marchetti** in “Shop replies” · today 14:02" · "**Besides typing text, this snippet**: [Clipboard] types whatever you last copied · [Enter] presses Enter once at the end" · "**What it types**": the content with those parts as chips · "Approve only if you trust Ines with this. Once approved, typing `;wtrack` does exactly what you see here, in whatever app you are in: in a chat, the Enter sends the message." · "1 of 2" · [Not now esc] [Approve]. | Focus starts on "Not now". Tab to Approve, or ⌘⏎ / Ctrl+Enter | `ListPendingReviews` → snippet, revision, author; `GetSnippet`; `ApproveSnippet(id, rev)` |
| 9 | **Review, after a change.** Dialog "**Review the change to “Wheel pickup steps”**" · "Changed by **Tomás Arce** · today 16:40. You approved the earlier text on 12 March." · "**What changed**": unchanged lines plain, removed lines struck through, added lines marked · "Until you approve, `;wpick` keeps typing the text you approved before." · [Not now esc] [Approve]. | As step 8 | As step 8. *Needed:* the approved body and the author's display name |

What is held, in the words the review uses (the sentence after "Besides typing text, this
snippet"):

| In the snippet | The review says |
| --- | --- |
| `{key}` | "[Enter] presses Enter once at the end" · "[Tab ×2] presses Tab twice" (one line per key press) |
| `{wait}` | "[Wait 0.5 s] pauses before going on" |
| `{clipboard}` | "[Clipboard] types whatever you last copied" |
| A line break | "types several lines" (for the first review of a plain multi-line text, the heading is simply "This snippet types several lines") |

Not held: single-line plain text from other members, and anything the user wrote themselves.
A snippet with fill-in fields but none of the above is not held either; its form shows what is
asked each time.

Rules for the recipient's library:

- A folder shared with "can use" is read-only: the editor shows the snippet without Save and
  offers [Copy to my snippets]. With "can use, add and change", snippets and sub-folders can
  be created, edited and deleted; a snippet cannot be moved out of the folder, only copied.
- When the user's own snippet and a shared one have the same abbreviation, the user's own
  expands on this computer. The shared row says "Yours is used" and library health lists it.
- Approvals are per computer; a second computer asks again.
- Usage counts never leave the computer, so nobody sees how often others use a snippet
  (ADR-0015).

## Errors and recovery

| Situation | What the user sees | Recovery |
| --- | --- | --- |
| Sign-in closed or failed | "Sign-in did not finish. Nothing was shared." [Try again] [Cancel] | Step 2 again |
| Offline when sharing | The share is saved; the badge reads "Will be shared when you are back online" | Sync pushes it later |
| Not a valid address (step 4) | "That does not look like an email address." | Fix it |
| The address is already in the group | "Odile is already in Brindlewood team." | — |
| The plan does not allow another member or group (`Entitlements`) | The sentence the server gives, above the actions; nothing else changes | Phase 4 decides the wording with task 4.7 |
| The invitation was withdrawn before it was answered | The banner disappears; if Join was pressed meanwhile: "This invitation is no longer valid." | — |
| The folder was unshared, or the user removed from the group | The folder disappears from the library with a toast: "“Shop replies” is no longer shared with you." Approvals for it are forgotten | — |
| A held snippet is typed before review | Nothing is typed, or the text approved before. Log: "“Paste tracking link” waits for your review" [Review] (*needed:* a diagnostic for this) | Review it |
| The same snippet was changed on two devices | Phase 4 (`Conflicts`, `ConflictRecorded`): the later change wins; "Changed on another device" in the editor with [Show the other version] | Restore the other version |
| The service or the sync agent is not running | Share and Join are disabled with "Iamb's sync is not running." [Start it again] | The supervisor restarts it |

## What the sync design changes or adds (task 4.1)

[`../../sync-design.md`](../../sync-design.md) fixes the rules behind this flow. Four points
touch its copy:

- **Step 2 promises more than version 1 does.** "Your other folders stay on this computer until
  you choose to sync or share them" is not true under the sync design: signing in adopts the
  library on the computer into the account and syncs all of it; only shared folders are visible
  to other people. This is open question 1 of the sync design (§13) and the owner's to decide.
  The design's proposal for the sentence: "Signing in keeps your snippets in step between your
  computers. Only folders you share can be seen by other people."
- **A change the server refuses is kept.** When the folder became read-only, was unshared, or
  the person was removed while their computer held changes it had not sent, those changes are
  kept on that computer in a list, with [Copy to my snippets] (rule D5; UX spec §11.2, entry 20).
  The row "The folder was unshared, or the user removed from the group" above gains: "2 changes
  you made there were not synced. They are kept on this computer." [Show]
- **Sharing that was stopped elsewhere stays stopped.** A computer that was offline and shares a
  folder which another of the owner's computers has unshared in the meantime does not win:
  "Sharing of “Shop replies” was stopped on another computer." (rule C11, scenario S37).
- **The share dialog should say two more things**: how many snippets of the folder include
  snippets outside it, which will not work for the others; and, under [Stop sharing], that
  copies already on the others' computers are removed the next time those computers are online,
  and that what someone copied before cannot be taken back (sync design §11.3).

## Timing

| Budget | Value |
| --- | --- |
| Share pressed to the folder visible on the other devices | Within 10 seconds while they are online (architecture §9) |
| Sign-in | Outside Iamb; the waiting dialog continues by itself within a second of the browser returning |
| Review dialog | Opens at once; the list of what the snippet does comes from `ParseTemplate` |

## Decisions in this flow

- **Groups only**, and the UI calls a `company` profile a "team" and a `family` profile a
  "family"; the rules are the same.
- **Review focuses "Not now"**, so approving is always a deliberate second key.
- **No "Reject".** Not approving has the same effect and needs no extra state; a person who
  no longer wants a folder leaves the group or asks the owner.
- **The review states consequences in plain words** ("in a chat, the Enter sends the
  message") instead of a general warning that people learn to click through.
- **Reviews are batched** ("1 of 2") so a newly shared folder with several held snippets is
  one sitting, not several interruptions.
