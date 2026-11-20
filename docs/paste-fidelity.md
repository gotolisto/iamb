# Paste fidelity of formatted snippets

Task 3.8 · Tests: "Paste-fidelity matrix: Gmail, Outlook, Word, Google Docs, Slack, Notes" ·
KPI: **at least 90% of fidelity cases pass; plain-text fallback everywhere.**

A formatted snippet is pasted through the clipboard as HTML (images embedded as `data:` URIs)
together with its plain-text version ([ADR-0018](decisions/0018-rich-content.md), architecture
§4.2, §9). How well that HTML survives depends on each application's paste handling, which only
the applications themselves can show. This document is the matrix to run on real machines, the
fixture snippets, what counts as a pass, and what CI checks without them.

## 1. What Iamb puts on the clipboard

| OS | Formats written by the platform crate | Notes |
| --- | --- | --- |
| Windows | `CF_UNICODETEXT` (plain text, CR LF), `HTML Format` (the fragment inside `<html><body>` between `<!--StartFragment-->` and `<!--EndFragment-->`, with the byte offsets of `StartHTML`, `EndHTML`, `StartFragment`, `EndFragment` in its header), `CF_HDROP` for a lone media file; the three formats that keep it out of clipboard history | `iamb-platform-windows::clipboard` |
| macOS | One item with `public.utf8-plain-text` and `public.html` (the fragment after `<meta charset="utf-8">`), one `public.file-url` item for a lone media file; the nspasteboard.org transient and concealed markers; this device only | `iamb-platform-macos::clipboard::layout` |

Afterwards the user's clipboard is put back exactly as the plain-text paste path does (architecture
§4.6, §8 rule 8). Where an application must not be pasted into (its injection method is `keys`,
built in or set by the user, ADR-0025), the plain text is typed instead.

## 2. The fixtures

`docs/paste-fidelity/fixtures.json` is an Iamb library file: import it with **Import…** in the
library window's More menu (or `iamb import`), and the nine snippets below appear in the folder
"Snippets" with the logo attachment (`logo.png`, a 32 × 32 teal square with a white "i").
`crates/iamb-service/tests/paste_fidelity.rs` writes it with the store's own export and keeps it
current; `docs/paste-fidelity/clipboard.json` beside it is the clipboard content Iamb builds for
each fixture (HTML and plain text), which the automatic round trip pastes.

| Abbreviation | Fixture | Markdown (logo shortened) | A pass |
| --- | --- | --- | --- |
| `;fx-head` | Heading | `# Order update` · blank line · `Your order has shipped.` | "Order update" as a heading (or, where the app has none, larger or bold text on its own line), then the sentence as a paragraph |
| `;fx-bold` | Bold | `Thanks for your order, **Kim**.` | "Kim" bold, nothing else bold |
| `;fx-italic` | Italic | `We are *really* sorry about the delay.` | "really" italic, nothing else italic |
| `;fx-link` | Links | `Track it on [our site](https://example.com/track) or write to <orders@example.com>.` | "our site" a link to `https://example.com/track`, the address a `mailto:` link (or plain text where the app links addresses itself) |
| `;fx-list` | List | `Please bring:` · blank line · three `- ` items | A bulleted list of three items after the sentence |
| `;fx-nested` | Nested list | `1. Pack` with two `- ` items under it, `2. Ship`, `3. Confirm` | A numbered list of three, the first with a bulleted list of two inside it |
| `;fx-image` | Inline image | `Kim Lee` / `Orders team` · blank line · `![Iamb logo](attachment:…)` | Two lines of text, then the logo as an image in the message or document |
| `;fx-field` | Field inside formatting | `Dear **{field name="Name" default="Ada <Lovelace> & *co*"}**, your *{field name="Item" default="blue mug"}* is ready.` (submit the form as it opens) | "Ada &lt;Lovelace&gt; &amp; \*co\*" bold, exactly those characters (the asterisks and angle brackets shown, never formatting); "blue mug" italic |
| `;fx-sig` | Signature, everything | `**Kim Lee** · *Orders team*` / `[example.com](https://example.com) · ~~old number~~` · blank line · logo | Bold name, italic title on the same line, a link, struck-through text, then the logo |

**General rule for a pass.** The formatting looks as the application shows the same formatting
made with its own toolbar; where the application has no such formatting at all, the text
arrives intact, in order, on the right lines, with no Markdown marks, HTML tags, entities or
`data:` text, and nothing else is lost. Plain text appearing instead of formatting the
application does support is a **fail**. In every case the clipboard holds the user's own
content again afterwards (step 5 below); a clipboard that was not restored fails the case.

## 3. The matrix

Each cell is one application on one OS, run with all nine fixtures: 11 cells, 99 cases.

| Application | Windows | macOS |
| --- | --- | --- |
| Gmail (in the browser: Chrome on Windows, Safari on macOS) | ✓ | ✓ |
| Outlook (the desktop app: new Outlook on Windows, Outlook for Mac) | ✓ | ✓ |
| Word (desktop) | ✓ | ✓ |
| Google Docs (Chrome on Windows, Safari on macOS) | ✓ | ✓ |
| Slack (desktop app, message box) | ✓ | ✓ |
| Notes (Apple Notes) | — (macOS only) | ✓ |

**The 90% rule.** The KPI is met when at least 90% of the cases pass (89 of 99) **and** the
plain-text fallback passes everywhere (section 4). A case that fails because the application
cannot show something at all, and that the general rule above therefore counts as a pass, is
recorded as "pass (no such formatting)", so later runs can see what each application lacks.
Expected gaps, from what the applications document about pasting HTML: Slack drops images
embedded in pasted HTML and has no headings; Notes has no struck-through text in older versions.
They are counted by the rule, not excused in advance: an image that disappears without its name
is a fail.

## 4. The plain-text fallback

Run each fixture once more in each of these fields; a pass is exactly the plain text of
`clipboard.json` (the Markdown marks gone, list items as `- ` and `1. `, a link as
`text (address)`, an image as its alt text), and the clipboard restored:

| Where | Windows | macOS |
| --- | --- | --- |
| A plain text editor | Notepad | TextEdit in plain-text mode |
| A terminal | Windows Terminal | Terminal |
| A single-line field | the browser's address bar | the browser's address bar |
| An application set to type key by key (Settings → Typing, "Type it key by key") | Notepad | TextEdit |

The last row checks the path that types instead of pasting.

## 5. How to run a cell

1. Import `fixtures.json` (section 2). Check Settings → Typing has no override for the app.
2. Copy the marker text `clipboard-check-<cell>` from anywhere, so the clipboard holds
   something of the user's own.
3. Open a new message or document in the application; for Slack, the message box of a
   private channel; for Notes, a new note.
4. Type each abbreviation followed by a space, on its own line; for `;fx-field`, press Insert in
   the form as it opens.
5. After the last fixture, paste with Ctrl+V (Windows) or Cmd+V (macOS): it must paste the
   marker text. Then record the cell:

| Application | OS and version | App version | `;fx-head` | `;fx-bold` | `;fx-italic` | `;fx-link` | `;fx-list` | `;fx-nested` | `;fx-image` | `;fx-field` | `;fx-sig` | Clipboard restored | Notes |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| | | | | | | | | | | | | | |

A screenshot of each cell goes with the run (`docs/compatibility-run.md` keeps the runs).

## 6. What CI checks without the applications

| Check | Where | Runs on |
| --- | --- | --- |
| Markdown to HTML and plain text, hostile input included (script, event attributes, `javascript:` and `data:` links, remote images, emphasis and quote bombs, size limits) | `spec/rich/examples.toml` (`crates/iamb-core/tests/rich_spec.rs`), `crates/iamb-core/tests/rich_props.rs`, `crates/iamb-core/src/rich.rs` | every OS (`test` job) |
| Macro output in Markdown is escaped; `{cursor}` ignored; plain and rich snippets inside each other; text between keys pasted piece by piece | `crates/iamb-testkit/sessions/072`, `119`–`121`, `rich_props.rs` | every OS |
| The clipboard content of every fixture: HTML with the image embedded, plain text, no attachment reference left; the fixture file current and importable | `crates/iamb-service/tests/paste_fidelity.rs` | every OS |
| A formatted snippet pastes through the worker; the clipboard comes back; a missing image and an app set to `keys` give plain text | `crates/iamb-service/tests/rich.rs` (mock platform) | every OS |
| `HTML Format` header offsets are byte offsets that slice out exactly the fragment, for a fixture-shaped fragment | `crates/iamb-platform-windows/src/clipboard/html.rs` | every OS (pure code) |
| The macOS pasteboard item: plain text, `public.html` with the character set, the markers | `crates/iamb-platform-macos/src/clipboard/layout.rs` | every OS (pure code) |
| **Round trip through a browser's paste**: each fixture's clipboard content written to headless Google Chrome's clipboard as HTML and plain text, pasted with the shortcut into a rich-text editor (`contenteditable`, as web mail and Google Docs use) and into a plain text field; formatting, links, lists, the decoded image and the exact plain text are checked | `docs/paste-fidelity/browser-paste.mjs` | `desktop` job, Linux |

The browser round trip passed all nine fixtures here (2026-11-20, Google Chrome headless, Linux).
It shows that the HTML is what a browser-based editor takes as intended; it does not show what
Gmail's or Google Docs' own paste filters, Word, Outlook, Slack or Notes make of it, which is what
the matrix above is for.
