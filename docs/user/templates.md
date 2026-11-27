# Templates

A snippet's content is a template. Most of it is plain text, typed as written. Some parts are
**macros**: they insert something that changes, ask you for a value, or press a key.

You do not need to learn the syntax. In the editor, press **{** or use the insert menu, and Iamb
writes the macro for you. Each macro shows as a chip you can open and change.

This page shows the macros with examples. The examples were checked against the template language
specification, [`template-language.md`](../template-language.md).

## Plain text

Text is typed as written. Braces are plain text unless they start a macro. So JSON, code and
brackets in prose need no escaping:

```text
{"name": "Ada"}
```

types `{"name": "Ada"}`.

## Date and time

`{date}` inserts today's date. `{time}` inserts the current time. Both use the format set in
[Settings](settings.md#dates) unless you choose another.

| Template | Result on 18 March 2026 at 14:05 |
| --- | --- |
| `Invoice sent on {date}.` | `Invoice sent on 2026-03-18.` |
| `Report for {date fmt="%B %-d, %Y"}` | `Report for March 18, 2026` |
| `Call at {time fmt="%-I:%M %p"}` | `Call at 2:05 PM` |
| `Due {date shift="+14d"}` | `Due 2026-04-01` |
| `Sent {date shift="+1mo -1d"}` | `Sent 2026-04-17` |
| `Ship {date shift="+2bd"}` | `Ship 2026-03-20` (a Friday; two business days later) |

Common date codes: `%Y` year, `%m` month number, `%d` day, `%B` month name, `%A` weekday name,
`%H` hour (24-hour), `%I` hour (12-hour), `%M` minute, `%p` AM or PM. Add a `-` before a code to
drop the leading zero. This works for `%m`, `%d`, `%H` and `%I`, as in `%-d`.

To move the date, add `shift`. Units: `d` days, `w` weeks, `mo` months, `y` years, `bd` business
days (Monday to Friday; public holidays are not known), and `h`, `min`, `s` for exact hours, minutes
and seconds. Terms apply left to right.

## Clipboard

`{clipboard}` inserts the text you last copied, when the snippet expands. An empty clipboard inserts
nothing.

```text
Quote: "{clipboard trim="yes"}"
```

With `"  quoted text  "` on the clipboard, this types `Quote: "quoted text"`.

## Fill-in fields

A field asks you for a value before the snippet is inserted. See [Fill-in forms](forms.md).

```text
Hi {field name="Customer"}, thanks for your order.
```

With `Ada` typed into the form, this types `Hi Ada, thanks for your order.`

Field types:

```text
Priority: {field name="Priority" type="choice" options="Low|Normal|High" default="Normal"}
```

A `choice` field shows the options. The default is `Normal`. Choose `High` in the form and the
text is `Priority: High`.

```text
{field name="Urgent" type="toggle" on="URGENT: " off=""}Status update
```

A `toggle` is a checkbox. When it is checked, the text `URGENT: ` is typed before `Status update`.
When it is not, nothing is typed.

Use `type="area"` for several lines of text. Use the same `name` in two places to ask once and insert
the value in both.

## Optional parts

`{if}` includes text only when a field says so. `{else}` is optional and `{endif}` closes it.

```text
{if field="Gift"}Wrapped as a gift.{endif} Thanks
```

Because `Gift` has no `{field}` in this template, Iamb adds a checkbox called **Gift** to the form. If
it is checked, the text is `Wrapped as a gift. Thanks`. If not, it is ` Thanks`.

```text
Plan: {field name="Plan" type="choice" options="Free|Pro"}
{if field="Plan" is="Pro"}Pro plan details.{else}Free plan details.{endif}
```

With `Free` chosen, the text is `Plan: Free` and then `Free plan details.` on the next line.

Conditions compare exactly. `is` checks for a value that matches; `not` checks for one that does not.

## Cursor position

`{cursor}` places the caret where you want to keep typing after the snippet is inserted. Only the
first `{cursor}` counts.

```text
Hello {cursor}, thanks
```

Iamb types `Hello , thanks` and moves the caret back 8 characters, to just after `Hello `.

## Another snippet

`{snippet}` inserts another snippet, by its abbreviation. The other snippet's fields join this form.

```text
{snippet abbr=";sig"}
```

The abbreviation must match exactly, including capitals. A snippet cannot include itself, directly or
through other snippets. Iamb reports that as an error. Nesting is limited to eight levels.

## Key presses and waits

`{key}` presses a key. `{wait}` pauses.

```text
Hello{wait ms="500"} there
```

Waits the half second before typing ` there`. Each wait is 1 to 10000 milliseconds, and the waits in
one expansion add up to at most 30 seconds.

```text
Thanks{key name="enter"}
```

Types `Thanks` and presses Enter. Be careful with Enter: in a chat it sends the message. The keys
you can press are Enter, Tab, Escape, Backspace, Delete, Space, the arrow keys, Home, End, Page Up
and Page Down. Add `shift="yes"` to hold Shift. Add `times="2"` to press the key more than once.

Keys cannot be used after `{cursor}`, because a key press can move the caret.

## Insert a literal brace

Use `{lbrace}` for a brace that would otherwise start a macro:

```text
Use {lbrace}date} to insert a date.
```

types `Use {date} to insert a date.` The editor does this for you when you type `{date}` as text.

## Preview

While you edit, the editor shows a preview of the expanded text. Blanks that you have not filled in
show as grey name tags. **Try the form** opens the real fill-in form, and its answers appear in the
preview. Key presses show as key badges, and waits show as a pause. If the template has an error, the
preview says **Preview unavailable**, with the reason.

## Errors

If a template has an error, Iamb types nothing. The abbreviation stays as you typed it, and no form
appears. The editor shows the error under the **Content** field, at the place it is.

| When | What to do |
| --- | --- |
| A macro is not closed (`{date` without `}`) | Add the closing `}`. |
| A required attribute is missing (for example, a `choice` with no `options`) | Add the attribute. |
| `fmt` has a code Iamb does not know (for example `%Q`) | Use a code from the list above. |
| A condition names a field that does not exist | Add a `{field}` with that name, or use a condition without `is` or `not`. |
| An `{if}` has no `{endif}` | Add `{endif}`. |
| `{snippet}` names an abbreviation that is not in the library | Check the abbreviation. |
| A wait is too long, or a wait value is out of range | Keep each wait from 1 to 10000 milliseconds and the total at most 30 seconds. |

A snippet with an error can be saved. It is marked **Broken**, does not expand, and is listed in
[Library health](snippets.md#library-health).

## Next steps

- [Fill-in forms](forms.md): how the form looks and what the buttons do.
- [Formatted snippets](formatted-snippets.md): when a snippet is formatted text.
