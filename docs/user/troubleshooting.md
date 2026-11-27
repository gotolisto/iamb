# Troubleshooting

Start with the question you have. Each section says what Iamb does, and what to do.

If a snippet does not expand, open **Why didn't it expand?** from the library window's sidebar. The
icon's menu does not have it. It lists the last reasons, newest first, up to 200. Technical entries,
which are meant for bug reports, are hidden until you turn on **Show technical entries**. Type an
abbreviation into **Test an abbreviation** to see what would happen. What you type there is not
stored.

## Iamb is not running

The library says **Iamb is not running.** Its background service stopped, so nothing can be saved or
expanded.

Iamb restarts the service by itself after a failure, waiting a little longer after each repeated
failure, up to 30 seconds between tries. You do not have to wait: choose **Start it again** in the
banner, or **Start Iamb again** in the menu (the icon shows a dot while the service is not running).
The search panel says **Iamb is not running.** with the same **Start it again** button. Iamb then
starts its background service afresh, and the banner goes as soon as the service answers.

If Iamb cannot start the service from there, the banner says so. Quit Iamb from its menu and open it
again.

## A permission is missing (macOS)

If a permission is missing, the menu shows **Fix permissions…**.

1. Choose **Fix permissions…** and turn on both switches in System Settings. See
   [Get started](getting-started.md#permissions).
2. If Iamb says it could not restart its helper, quit Iamb from its menu and open it again.

The library window does not show a banner for a missing permission in this version. Look for
**Fix permissions…** in the icon's menu.

<!-- TODO: the library window does not yet show a banner for missing permissions. -->

## Nothing expands at all

Iamb may be paused. A pause lasts until you resume, also across a quit, a logout and a restart of
the computer. The menu says **Iamb is paused** or **Iamb is still paused**, and so does the status
line at the bottom of the library window's sidebar; **Why didn't it expand?** says so at the top
while Iamb is paused. To resume, choose **Pause Iamb** again in the menu or in the status line,
or turn on **Expand snippets as I type** in [Settings](settings.md#general).

## Enter and Tab do not expand

This is on purpose. Enter and Tab often do something in the app before Iamb can type: a chat sends
the message, a dialog closes, a form moves on. Iamb expands a snippet when you finish the
abbreviation with a space or punctuation, such as `.`, `,` or `)`.

To use Enter or Tab as an ending, turn it on in [Settings](settings.md#typing). Iamb asks you to
confirm first, and then shows a warning. Only do this in plain documents.

## Nothing expands in a password field

This is expected. Iamb recognises password fields and does not type into them, and it does not keep
what you type there.

## Nothing expands in a terminal

- **Windows**: Iamb cannot tell when a terminal is asking for a password (for example `ssh`, `sudo` or
  `runas`). Add the terminal to [Exclusions](exclusions.md) if you type secrets there.
- **macOS**: Iamb can see a password prompt only while the terminal has Secure Keyboard Entry on.
  Add the terminal to [Exclusions](exclusions.md).
- In a terminal, Enter runs the line. A snippet with a line break runs each line. Check a snippet
  there before you rely on it.

## Nothing expands in a game, a remote desktop or a virtual machine

These windows often do not describe their fields, so Iamb treats them as ordinary text fields. Add
them to [Exclusions](exclusions.md) if you do not want Iamb there, or use the pause.

## Nothing expands while you type a special character

Some keyboard input methods, for example those that build accents or characters one step at a time,
hold the keys until the character is finished. Iamb does not expand while an input method is
composing text. Finish the character, then type the abbreviation again.

On Windows, Iamb does not recognise some input methods. A switch to one of them may be noticed one
key late.

## Text comes out wrong in an app

Use the setting for the app: **Settings**, then **Apps**, then **If snippets come out wrong in an
app**. See [Settings](settings.md#apps).

- **Type it key by key** types each character. Try it in an app where pasted text goes wrong.
- **Paste it** pastes the text through the clipboard. Try it in an app where typed text is dropped
  or slow. Iamb restores your clipboard afterwards.
- **Slow down by** adds a pause between key presses, for an app that loses characters.
- **Line breaks as** chooses between Enter and Shift+Enter. In a chat, Enter sends the message, so
  choose Shift+Enter.

## Keys typed fast come out in the wrong order

If you type very fast, the keys you press during an expansion can land in the wrong order. Iamb
records this as a reason in **Why didn't it expand?**. Type more slowly around the abbreviation, or
pause before the next word.

## Windows will not let Iamb type into a window

Windows does not let a program type into a window that runs with more rights than Iamb, such as a
window started as administrator. Iamb cannot expand there. Type the text yourself in that window.

## Formatting is lost in an app

Formatted snippets reach an app as formatted text and as plain text. Each app decides how much of the
formatting to keep. If an app takes only plain text, Iamb pastes the plain version. See
[Formatted snippets](formatted-snippets.md#where-formatting-may-not-survive).

## Apps that need care

Some apps handle typed text in their own way. The table lists the differences that the compatibility
plan checks for. The test results are not published yet.

| Kind of app | What may happen | What to do |
| --- | --- | --- |
| Chat apps | Enter sends the message. | Set **Line breaks as** to Shift+Enter, or use **Paste it** for long text. |
| Code editors | Brackets and quotes close themselves, and new lines indent. | Check the result after each expansion. |
| Word processors | Autocorrect changes quotes and capitals. | Check the result after each expansion. |
| Spreadsheets | Enter finishes a cell. A new line in a cell needs Alt+Enter (Windows) or Control+Option+Return (macOS). | Use single-line snippets. |
| Web editors | The page redraws as text is typed. | Use **Paste it** for long text. |
| Terminals | Enter runs the line. A multi-line paste can ask for confirmation. | Use single-line snippets. |
| Password managers and secure fields | Nothing expands, on purpose. | Nothing to do. |

<!-- TODO: replace this table with the results of the compatibility runs, app by app. -->

## Something else

- Check **Why didn't it expand?** for the reason.
- Check [Library health](snippets.md#library-health) for snippets with errors.
- If a snippet is broken, its editor shows the error. See [Templates](templates.md#errors).
- To report a problem, use the project's repository. Include the reason from **Why didn't it
  expand?**, and never include what you typed.
