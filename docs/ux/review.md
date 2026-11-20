# Review sheet: UX specification and mock-ups

Task 3.1 · Test: mock-ups reviewed by the owner and two friends · KPI: the owner approves all
six flows

**Status (2026-11-20):** the owner approved all six flows and kept every decision under "Needs
your judgement" as specified. The review by two friends is still to happen; their notes can
still change copy and details, not the approved flows.

## How to review

1. Open [`mockups/index.html`](mockups/index.html) in a browser (it works offline). Each flow
   has its own page; the steps are side by side with three lines under each: **Keys** (what the
   person presses), **Screen** (what Iamb does) and **Service** (what happens underneath;
   friends can skip that line).
2. Use the switches at the top right to see **Light** and **Dark**, and **macOS** and
   **Windows** window chrome. Look at every flow at least once in the system you use.
3. For each flow below, answer the questions, then mark the flow **Approve** or **Changes**.
   "Changes" needs a note that says what, as concretely as possible ("the button should say…").
4. Friends: the first question of each flow is best answered **before** reading the notes
   under the pictures. Say what you expect to happen; there are no wrong answers. If a word on
   a screen made you stop, write the word down.
5. The owner decides the items under "Needs your judgement". Each is already taken one way in
   the specification; "Keep" means that way stays.

The mock-ups are pictures: nothing can be clicked except the switches. Names, addresses and
apps in them are invented.

| Reviewer | Name | Uses | Date |
| --- | --- | --- | --- |
| Owner | | macOS ☐ Windows ☐ | |
| Friend 1 | | macOS ☐ Windows ☐ | |
| Friend 2 | | macOS ☐ Windows ☐ | |

## Flow 1: Onboarding

[`mockups/onboarding.html`](mockups/onboarding.html) · [`flows/1-onboarding.md`](flows/1-onboarding.md)

Questions:

1. Look only at step 1 for five seconds. In your own words: what does Iamb do, and what does
   it do with what you type?
2. (macOS) At step 2, do you know what to do in System Settings, and why Iamb asks? Is anything
   about the two permissions worrying or unclear?
3. At step 3, would you keep `;em` and your email address, or type something else? Does the
   tip about starting with `;` make sense?
4. At step 4, what do you press after `;em`? What do you expect if you press Enter instead?
5. After "Try it: Enter was pressed": is the explanation enough? Would you switch this snippet
   to "expand without a space"?
6. After step 5, where is Iamb when the window is closed, and how do you find a snippet whose
   abbreviation you forgot?
7. Is any step one too many? Is anything missing before you would feel ready?

| Reviewer | Approve | Changes | Notes |
| --- | --- | --- | --- |
| Owner | ☐ | ☐ | |
| Friend 1 | ☐ | ☐ | |
| Friend 2 | ☐ | ☐ | |

Needs your judgement:

| # | Decision taken | Alternative | Keep / change |
| --- | --- | --- | --- |
| 1.1 | The first snippet is the user's own email address under `;em`, typed by them | A ready-made sample snippet (faster, teaches less); or a free choice with no suggestion | |
| 1.2 | Onboarding recommends starting abbreviations with `;` | No recommendation; or another character (`,,` `:` `/`) | |
| 1.3 | Permissions may be skipped ("Skip for now"); Iamb then says "cannot expand yet" everywhere until they are granted | No skip: the window cannot continue without both | |
| 1.4 | Input Monitoring is asked before Accessibility, each explained in one sentence | Another order; a longer explanation; a link to the privacy page | |
| 1.5 | "Start Iamb when I log in" is on by default, shown as a checkbox on the last step | Off by default; or not shown at all | |
| 1.6 | The privacy promise on the welcome step is two sentences and promises only what the architecture guarantees | Longer, with the limits (terminals, remote desktops) named up front | |

## Flow 2: Create a snippet

[`mockups/create-snippet.html`](mockups/create-snippet.html) · [`flows/2-create-snippet.md`](flows/2-create-snippet.md)

Questions:

1. In step 1, where would you click or what would you press to add a snippet?
2. In step 2, what is the difference between "Abbreviation" and "Label"? Is "Label" needed?
3. In step 3, you want today's date in your text. What do you do? Does the list after `{` say
   enough about each item? Are "Fill-in field" and "Optional part" understandable names?
4. In step 5, what do the coloured chips mean to you? What do the grey tags in the preview
   mean?
5. In step 6, do the two "Expand" choices and the three "Capital letters" choices make sense
   as written? Which would you pick for a signature?
6. In step 7, are the three messages clear about what is wrong and what to do?
7. You have typed half a snippet and click another one. Is the question in "Leaving with
   changes" what you expect?

| Reviewer | Approve | Changes | Notes |
| --- | --- | --- | --- |
| Owner | ☐ | ☐ | |
| Friend 1 | ☐ | ☐ | |
| Friend 2 | ☐ | ☐ | |

Needs your judgement:

| # | Decision taken | Alternative | Keep / change |
| --- | --- | --- | --- |
| 2.1 | Saving is explicit (⌘S / Ctrl+S); a draft cannot expand; leaving with changes asks | Autosave after a pause, with the abbreviation becoming live only when it is valid and the field has lost focus | |
| 2.2 | Abbreviation comes before Content in the form and the tab order | Content first (write, then name it) | |
| 2.3 | Macros are chips, grouped by four colours (inserts, asks, does something, reads); the raw template text is behind "Show as text" | One chip colour; or raw text by default for people who write templates by hand | |
| 2.4 | The UI says "fill-in field", "optional part", "key press", "another snippet" and never "macro" or "template" | Other words (list them) | |
| 2.5 | A snippet with a template error can be saved and is marked "Broken" | Saving is blocked until the error is fixed | |
| 2.6 | Options (trigger, capitals, start of word) are closed by default and summarised in the footer | Always visible | |
| 2.7 | Three panes: folders and tags, list, editor; Settings, health and the log are views of the same window | Separate windows for Settings and the log | |
| 2.8 | A tag has one of eight named colours | Any colour; or no colours | |
| 2.9 | Deleting a snippet does not ask; it shows "Undo" for 10 seconds | Ask every time | |

## Flow 3: Expand

[`mockups/expand.html`](mockups/expand.html) · [`flows/3-expand.md`](flows/3-expand.md)

Questions:

1. Step 1: is it right that nothing at all appears on screen when a snippet expands?
2. Step 3: you typed an abbreviation, pressed Enter, and the chat sent `;rty`. What would you
   think happened? Where would you look?
3. Step 4: is it reassuring, confusing or unnoticed that Iamb says nothing in a password
   field?
4. Step 5: is the menu the right length? Is anything there you would never use, or missing?
   Can you tell "on" from "paused" from the icon alone?
5. Step 6: read the six log entries. Which is unclear? Would this page answer your question
   from step 2?
6. Try the "Test an abbreviation" idea in your head: is "Not yet. … waits for a space or
   punctuation" the answer you needed?

| Reviewer | Approve | Changes | Notes |
| --- | --- | --- | --- |
| Owner | ☐ | ☐ | |
| Friend 1 | ☐ | ☐ | |
| Friend 2 | ☐ | ☐ | |

Needs your judgement:

| # | Decision taken | Alternative | Keep / change |
| --- | --- | --- | --- |
| 3.1 | No feedback when a typed abbreviation does not expand: no popup, no icon change; the log answers when asked | A brief hint near the tray for the Enter and Tab case only; or the icon dims while Iamb is held back | |
| 3.2 | The icon has three states (on, paused, needs you) and ignores the focused app | A fourth state for "not expanding here" | |
| 3.3 | Pause is on or off; there is no "pause for an hour" | Timed pauses | |
| 3.4 | "Never expand in *App*" is in the menu, one click | Only in Settings (fewer accidental exclusions) | |
| 3.5 | Quitting from the menu asks first and stops expansion too | Quit closes only the windows; the service keeps expanding | |
| 3.6 | The log hides purely technical entries unless "Show technical entries" is on | Show everything | |
| 3.7 | The mark is a breve and a macron (˘ ¯): short becomes long | Another mark | |

## Flow 4: Fill-in

[`mockups/fill-in.html`](mockups/fill-in.html) · [`flows/4-fill-in.md`](flows/4-fill-in.md)

Questions:

1. Step 1: a form appeared while you were writing a mail. Do you know what it wants and what
   Enter will do? Is `;rlate` still being visible behind it confusing?
2. Step 2: can you tell the five kinds of field apart and how to answer each from the
   keyboard? What do you expect "Start empty" to do?
3. Step 2: you are in "Work needed" and press Enter. What happens? How do you insert?
4. Step 4: you pressed Esc. Is it right that `;rlate` is still in your mail?
5. Step 5: you have ten customers to answer. Is "Fill in … again" from search good enough, or
   do you want a key of its own?
6. Step 6: do the two messages tell you what happened to your answers and what to do?

| Reviewer | Approve | Changes | Notes |
| --- | --- | --- | --- |
| Owner | ☐ | ☐ | |
| Friend 1 | ☐ | ☐ | |
| Friend 2 | ☐ | ☐ | |

Needs your judgement:

| # | Decision taken | Alternative | Keep / change |
| --- | --- | --- | --- |
| 4.1 | The form has no live preview of the final text | A collapsible preview under the fields (one more request per key, a taller panel) | |
| 4.2 | Clicking outside the form does not cancel it; only Esc or Cancel does | Click-away cancels, like a menu | |
| 4.3 | The form opens with the last answers when it was used in the last 30 minutes, and says so; "Start empty" clears them | Always open with the snippet's defaults; offer "Use last answers" | |
| 4.4 | Up to six options are a list of radio buttons; seven or more a select | Always a select; or another threshold | |
| 4.5 | "Fill in again" has no shortcut by default and lives in the palette | A default shortcut (which?) | |
| 4.6 | The footer names the app the text will go into ("Goes into Mail") | Leave it out | |
| 4.7 | The panel sits in the upper third of the screen, always in the same place | Near the mouse pointer; or remember where the user dragged it | |

## Flow 5: Search

[`mockups/search.html`](mockups/search.html) · [`flows/5-search.md`](flows/5-search.md)

Questions:

1. Step 1: would you remember Ctrl+Shift+Space? Does it clash with something you use?
2. Step 2: can you tell why each of the four rows matched "ret"? Is the preview at the bottom
   useful or in the way?
3. Step 2: what do you expect Enter to do? And ⌘E / Ctrl+E?
4. Step 4: is "New snippet labelled “warranty”" what you would want when nothing matches?
5. Step 5: do the badges "Needs your review" and "Broken" tell you why Enter will not simply
   insert?
6. Step 6: is one notice in the corner of the screen the right amount of telling?

| Reviewer | Approve | Changes | Notes |
| --- | --- | --- | --- |
| Owner | ☐ | ☐ | |
| Friend 1 | ☐ | ☐ | |
| Friend 2 | ☐ | ☐ | |

Needs your judgement:

| # | Decision taken | Alternative | Keep / change |
| --- | --- | --- | --- |
| 5.1 | The default shortcut is Ctrl+Shift+Space on both systems (⌃⇧Space on macOS) | ⌥Space or ⌘⇧Space on macOS, Alt+Space or Win+Shift+S-style chords on Windows; or no default, chosen in onboarding | |
| 5.2 | The palette never remembers the last query | Reopen with the last query selected | |
| 5.3 | With an empty query the palette lists the snippets used most recently on this computer | Most used; or nothing but a hint | |
| 5.4 | A failed pick shows a notice over other apps for 6 seconds | Nothing (log only); or a sound | |
| 5.5 | Enter is the only action besides "edit"; there is no "copy to clipboard instead" | Add ⌘C / Ctrl+C to copy the rendered text | |
| 5.6 | Switched-off snippets can still be inserted from the palette | Hide them | |

## Flow 6: Share a folder

[`mockups/share-folder.html`](mockups/share-folder.html) · [`flows/6-share-folder.md`](flows/6-share-folder.md)

Questions:

1. Step 3: who exactly will get the folder if you press Share? What will they be able to do?
2. Step 2: does the sign-in explanation answer "why do I need an account" and "what happens
   to my other snippets"?
3. Step 4: is "group" the right word for a family or a shop's staff? Are "Family" and "Team"
   the right two kinds?
4. Step 6 (as the invited person): do you understand what joining gives the others access to?
5. Step 8, left: read the review. What will this snippet do when you type `;wtrack` in a chat?
   Would you approve it from a colleague? From someone you barely know?
6. Step 8, right: can you see what Tomás changed? What happens if you choose "Not now"?
7. Step 7: your own `;addr` and the shop's `;addr` clash. Is "Yours is used" the behaviour
   you expect?

| Reviewer | Approve | Changes | Notes |
| --- | --- | --- | --- |
| Owner | ☐ | ☐ | |
| Friend 1 | ☐ | ☐ | |
| Friend 2 | ☐ | ☐ | |

Needs your judgement:

| # | Decision taken | Alternative | Keep / change |
| --- | --- | --- | --- |
| 6.1 | The UI says "group", with the kinds "Family" and "Team" (the data model's `family` and `company`) | "Company" instead of "Team"; or no kinds in the UI at all | |
| 6.2 | The two permissions read "Use the snippets" and "Use, add and change snippets" | "Can view" / "Can edit" | |
| 6.3 | The review dialog focuses "Not now"; approving takes a deliberate second key | Focus "Approve" (faster, riskier) | |
| 6.4 | There is no "Reject": an unapproved snippet simply does not expand (or keeps its approved text) | A "Turn off on this computer" button in the review | |
| 6.5 | The review explains the consequence in plain words ("in a chat, the Enter sends the message") | A shorter, general warning | |
| 6.6 | Held snippets are reviewed one after another ("1 of 2") from one banner | "Approve all from Ines" (convenient; weakens the review) | |
| 6.7 | The share dialog asks the user to sign in at the moment they first share; nothing before that needs an account | Offer an account during onboarding | |
| 6.8 | A folder that arrives while the library is closed is announced in the menu's status line, not with a notice | A notice over other apps | |

## Design system

[`design-system.md`](design-system.md) · [`mockups/index.html`](mockups/index.html) (lower half)

Questions:

1. Light and dark: is either harder to read anywhere? Name the screen.
2. Is the text size comfortable in the library (13 px) and in the form and palette (15 px)?
3. Does the accent colour (indigo in light, periwinkle in dark) suit a tool you would leave
   running all day?
4. Does Iamb look at home on your system with the macOS / Windows switch set to yours?

| Reviewer | Approve | Changes | Notes |
| --- | --- | --- | --- |
| Owner | ☐ | ☐ | |
| Friend 1 | ☐ | ☐ | |
| Friend 2 | ☐ | ☐ | |

## Result

| Flow | Owner | Friend 1 | Friend 2 | Changes agreed |
| --- | --- | --- | --- | --- |
| 1 Onboarding | | | | |
| 2 Create a snippet | | | | |
| 3 Expand | | | | |
| 4 Fill-in | | | | |
| 5 Search | | | | |
| 6 Share a folder | | | | |
| Design system | | | | |

The KPI is met when the Owner column reads "Approve" for all six flows. Changes that are agreed
are made in `spec.md`, the flow file and the mock-up source together, and
`python3 docs/ux/mockups/build.py` is run again.

Things the flows need from the service that do not exist yet are listed in
[`spec.md` §11.2](spec.md#112-needed-from-the-service); they are for the people building the
service, not for this review.
