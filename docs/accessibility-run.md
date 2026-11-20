# Accessibility run by hand

What the automated checks cannot judge (`docs/test-strategy.md` §8.10). Do it on Windows with NVDA
(and Narrator once) and on macOS with VoiceOver, on a build of the release candidate, with the
mouse put away. Write down every step that fails, with the screen reader's words.

The scripted part: `pnpm build` then `node scripts/keyboard-run.mjs` in `apps/desktop` walks
the same path with the keyboard and checks the focus ring and axe-core in Chromium. It does not
speak. This page is what a person repeats with ears.

## Set-up

1. Fresh profile; start NVDA (Insert is the NVDA key) or VoiceOver (VO = Ctrl+Option).
2. Settings, General: Appearance "Follow the system". Run once in light, once in dark, once with
   the system's contrast theme (Windows) or Increase contrast (macOS), once with Reduce motion.
3. Say "Iamb, on" status: the tray or menu-bar item must read "Iamb, on" (paused: "Iamb, paused";
   needs attention: "Iamb, needs attention").

## Walk (every step by keyboard)

1. **Onboarding.** On open the window title is read, then "Step 1 of 4" and the heading; focus is
   on "Get started". Enter moves on; each step's heading is read; Tab never leaves the window;
   Esc closes. The permissions step (macOS): both permissions are read with their state, and a
   grant is announced when it happens. The Try step: typing the abbreviation and a space announces
   "Iamb replaced ;em with your snippet."
2. **Library.** Title "Iamb - All snippets". Ctrl+1, Ctrl+2, Ctrl+3 and F6 move between the
   navigation, the list ("Snippets in ..., list box") and the editor ("Snippet"). Arrow keys in the
   list read label, abbreviation, state and "asks for values". Ctrl+N: focus goes to Abbreviation
   with the folder's prefix read. Tab to Label, then the insert buttons, then Content ("Content,
   edit text, multi line").
3. **Template editor.** Type `{`: "Insert, list box" and each item as the arrows move. Enter inserts
   a chip; its card opens as a dialog named for the chip; Esc closes it and focus returns to the
   text. Left and Right move over a chip and read it. Tab leaves the editor, never types a tab.
   Switch Kind to Formatted with Space; "Formatting, toolbar" is read; Ctrl+B, Ctrl+I, Ctrl+K work.
4. **Save.** Ctrl+S: "Saved. Type ;greet to use it." is announced without moving focus. A broken
   template (`{` never closed) is announced as an error tied to Content.
5. **Preview.** Ctrl+Shift+P: the preview is read; again returns to the text.
6. **Menus, dialogs, toasts.** Shift+F10 on a row (Ctrl+Return on macOS): menu named for the
   snippet; Esc returns to the row. Ctrl+Shift+N: the dialog is read with its fields, Tab and
   Shift+Tab stay inside, Esc returns focus to where it was. Delete on a row: the toast is read
   ("Deleted ..."); F6 reaches Undo; Enter restores and focus is back on the list.
7. **Settings.** Ctrl+, : "Settings" heading, focus on it. Tab to each section, Enter; the section
   heading is read. A switch reads its label and "on" or "off"; Appearance is a radio group read
   with the arrow keys. A refused shortcut in Shortcuts is announced as an alert.
8. **Library health and "Why didn't it expand?".** From the navigation with Enter; each section
   is a region with its title; the buttons in an entry say which snippet they act on.
9. **Palette.** The global shortcut: the screen reader must move to the panel ("Search snippets,
   edit text") on both systems, including the non-activating panel on macOS. Type: after each
   change "4 results, Return label, abbreviation semicolon r r e t". Arrows move, Enter inserts,
   Esc closes and the screen reader returns to the app that had it (the text field you were in).
10. **Form.** Opened by a snippet with fields: "Repair quote, fill-in form, 4 fields. Customer,
    edit text". Tab and Shift+Tab, arrows in a choice, Space on a toggle. Enter inserts and focus
    returns to the app. Make the return fail (close the app first): the alert "Iamb could not
    get back to ..." is read and the answers are kept.
11. **Zoom.** Ctrl++ (Cmd++) to 200% in the library: nothing scrolls sideways, every control is
    reachable, names are unchanged. Ctrl+0 resets.
12. **Contrast and motion.** In the contrast theme the focus ring, the selected row, chips, badges
    and switches have a visible outline; with Reduce motion the switch does not slide.

## Panels and screen readers (spec §8)

The form and the palette must take the screen reader's focus when they appear and give it back
when they close, on both systems. Check on NVDA, Narrator and VoiceOver: the first word read on
opening is the panel's name; after Esc the next thing read is the app's own text field, not the
Iamb window or the desktop. Record the result for each pair.
