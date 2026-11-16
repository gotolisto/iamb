# Iamb template language

Status: draft for approval · Task 0.2 · Version 1

A snippet body is a template: text that is typed as written, plus macros in braces that are
replaced or acted on when the snippet expands. This document defines the grammar, every macro, and
what happens on every error.

The 166 examples in [`spec/template/examples/`](../spec/template/examples/) are part of this
specification. Where prose and an example disagree, that is a defect in the specification; the
example is authoritative until it is fixed. The examples are the test suite for the parser and
renderer (task 1.3) and run again on iOS and Android (task 6.2).
[`spec/template/reference.py`](../spec/template/reference.py) is a small reference implementation
that CI runs against the examples.

## 1. Design rules

1. **Plain text needs no escaping.** A brace starts a macro only when it is followed by a macro
   name. JSON, code and prose with braces are typed unchanged.
2. **All or nothing.** If a template has any error, nothing is typed and the abbreviation stays as
   the user typed it. Iamb never types half a snippet.
3. **Values are never templates.** Clipboard content and fill-in values are inserted as text and
   are never parsed for macros.
4. **Deterministic.** Given the same template, library, settings, clock, clipboard and form values,
   the output is identical on every platform.
5. **Limited reach, honestly stated.** A template can type text, press a small set of keys, wait,
   and read the clipboard. It cannot press Ctrl, Alt, Cmd or Win shortcuts or run programs
   itself. Typing is still powerful (text followed by Enter runs a command in a terminal), so
   snippets written by other people are reviewed before use when they contain `{key}`, `{wait}`,
   `{clipboard}` or a line break (architecture §8).

## 2. Grammar

```ebnf
template   = { text | macro } ;
macro      = "{" name { ws attribute } [ ws ] "}" ;
name       = "date" | "time" | "clipboard" | "cursor" | "snippet" | "field"
           | "key" | "wait" | "if" | "else" | "endif" | "lbrace" ;
attribute  = attr-name "=" ( quoted | bare ) ;
attr-name  = lower { lower } ;
quoted     = '"' { any character except '"' | '\"' | '\\' } '"' ;
bare       = bare-char { bare-char } ;
bare-char  = letter | digit | "_" | "+" | "-" | "." | "%" | ":" | "/" ;
ws         = ( " " | tab | newline ) { " " | tab | newline } ;
lower      = "a" … "z" ;
```

### 2.1 When a brace starts a macro

`{` starts a macro when it is immediately followed by one of the twelve names and then by `}` or
white space. In every other case the `{` is literal text. `}` outside a macro is always literal.

| Source | Meaning |
| --- | --- |
| `{date}` | Macro |
| `{date fmt="%Y"}` | Macro |
| `{"a": 1}` | Literal text |
| `{name}`, `{Date}`, `{dates}`, `{date:x}`, `{ date}` | Literal text |
| `{lbrace}date}` | The literal text `{date}` |

Once a macro has started, it must be well formed; a mistake inside it is an error, not literal
text. Names and attribute names are lower-case ASCII.

The editor shows macros as chips and writes this syntax for the user. When a user types literal
text that would be read as a macro, the editor stores it with `{lbrace}`.

### 2.2 Attributes

- Written `name=value`, with no spaces around `=`, separated from each other by white space.
- A quoted value may contain anything, including line breaks. Inside quotes `\"` is a quote and
  `\\` is a backslash; any other backslash is an ordinary character.
- A bare value is a run of letters, digits and `_ + - . % : /`. `fmt=%H:%M` and `fmt="%H:%M"` are
  the same.
- Each macro accepts a fixed set of attributes. An unknown attribute, a repeated attribute, a
  missing required attribute and a value outside the allowed set are errors (§5).
- Yes-or-no attributes take exactly `yes` or `no`.

### 2.3 Text

- Line endings in the template (`CRLF`, `CR`) are normalised to `LF` before parsing. The same
  normalisation applies to clipboard content, field defaults and entered values.
- No white space is trimmed or collapsed anywhere.
- A line break in text is text. How it is delivered to an app (for example as Shift+Enter in a chat
  box) is the injector's decision. `{key name=enter}` is a real Enter key press.

## 3. Output

Rendering produces three things:

| Part | Content |
| --- | --- |
| Form | The fill-in fields to ask for, in order. Empty when the template has no fields |
| Actions | An ordered list of `text`, `key` and `wait` actions. Adjacent text is merged into one action |
| Cursor | Optional. `cursor_back` = how many characters of typed text follow the cursor position |

Rendering happens in two steps. Step 1 (resolve) parses the template, replaces every `{snippet}`
with the parsed body of that snippet, and builds the form. Every error except the three run-time
limits in §5 is found in step 1, before any form is shown. Step 2 (render) runs with the form
values and produces the actions.

"Character" in `cursor_back` and in limits means an extended grapheme cluster (Unicode UAX #29):
`👍` is one, `🇪🇸` is one, a letter with combining accents is one.

## 4. Macros

### 4.1 `{date}` and `{time}`

Insert the current local date or time. They are the same macro with different default formats.

| Attribute | Required | Value | Default |
| --- | --- | --- | --- |
| `fmt` | no | Format string, not empty | `{date}`: the date-format setting (`%Y-%m-%d`). `{time}`: the time-format setting (`%H:%M`) |
| `shift` | no | One or more shift terms separated by spaces | No shift |

All `{date}` and `{time}` macros in one expansion, including nested snippets, use the same instant,
captured once when the abbreviation is matched, before any fill-in form is shown.

Format specifiers (anything else after `%` is `E_BAD_FORMAT`; other characters are copied):

| Specifier | Meaning | Example |
| --- | --- | --- |
| `%Y` / `%y` | Year, four digits / two digits | `2026` / `26` |
| `%m` / `%-m` | Month number, padded / unpadded | `03` / `3` |
| `%d` / `%-d` | Day of month, padded / unpadded | `08` / `8` |
| `%B` / `%b` | Month name, full / short | `March` / `Mar` |
| `%A` / `%a` | Weekday name, full / short | `Wednesday` / `Wed` |
| `%H` / `%-H` | Hour 00 to 23, padded / unpadded | `09` / `9` |
| `%I` / `%-I` | Hour 01 to 12, padded / unpadded | `02` / `2` |
| `%M` | Minute | `05` |
| `%S` | Second | `09` |
| `%p` | `AM` or `PM` (the same in every locale) | `PM` |
| `%j` | Day of the year, three digits | `077` |
| `%%` | A percent sign | `%` |

Month and weekday names follow the locale setting. Version 1 has names for `en`, `es`, `de` and
`fr`; the region part of a locale is ignored (`de-AT` is `de`) and any other language uses `en`.

| | Months, full | Months, short | Weekdays from Monday, full | Weekdays, short |
| --- | --- | --- | --- | --- |
| `en` | January … December | Jan Feb Mar Apr May Jun Jul Aug Sep Oct Nov Dec | Monday … Sunday | Mon Tue Wed Thu Fri Sat Sun |
| `es` | enero febrero marzo abril mayo junio julio agosto septiembre octubre noviembre diciembre | ene feb mar abr may jun jul ago sep oct nov dic | lunes martes miércoles jueves viernes sábado domingo | lun mar mié jue vie sáb dom |
| `de` | Januar Februar März April Mai Juni Juli August September Oktober November Dezember | Jan Feb Mär Apr Mai Jun Jul Aug Sep Okt Nov Dez | Montag Dienstag Mittwoch Donnerstag Freitag Samstag Sonntag | Mo Di Mi Do Fr Sa So |
| `fr` | janvier février mars avril mai juin juillet août septembre octobre novembre décembre | janv. févr. mars avr. mai juin juil. août sept. oct. nov. déc. | lundi mardi mercredi jeudi vendredi samedi dimanche | lun. mar. mer. jeu. ven. sam. dim. |

A shift term is a sign, one to four digits, and a unit: `+3d`, `-2w`, `+1mo`.

| Unit | Meaning | Rule |
| --- | --- | --- |
| `d` | Days | Calendar days |
| `w` | Weeks | 7 days |
| `mo` | Months | Same day of the month; if that day does not exist, the last day of the month (31 January + 1 month = 28 or 29 February) |
| `y` | Years | 12 months, same rule (29 February + 1 year = 28 February) |
| `bd` | Business days | Step one day at a time, counting only Monday to Friday. `+0bd` never moves. Public holidays are not known |
| `h`, `min`, `s` | Hours, minutes, seconds | Exact elapsed time; may change the date |

Terms apply left to right: `shift="+1mo -1d"` adds a month, then subtracts a day. A bare `m` is
rejected because it could mean months or minutes. Calendar units work on the local wall-clock date
and time. A result outside the years 1 to 9999 is `E_DATE_RANGE`.

### 4.2 `{clipboard}`

Insert the text that was on the clipboard when the abbreviation was matched (before any fill-in
form is shown).

| Attribute | Required | Value | Default |
| --- | --- | --- | --- |
| `trim` | no | `yes` or `no`: remove white space at both ends | `no` |

An empty clipboard, or one that holds no text, gives an empty string and is not an error. The
clipboard is read at most once per expansion, and only if the template uses this macro.

### 4.3 `{cursor}`

Mark where the caret should be when the expansion is finished. No attributes.

- After all actions have run, the caret moves back `cursor_back` characters.
- If the output contains several cursors, the first in output order wins and the rest are ignored.
- A cursor in an `{if}` branch that is not taken does not count.
- A `{key}` action anywhere after the cursor is `E_CURSOR_BEFORE_KEY`, because a key press can move
  the caret by an unknown amount. Keys before the cursor, and waits anywhere, are fine.

### 4.4 `{snippet}`

Insert another snippet from the library.

| Attribute | Required | Value |
| --- | --- | --- |
| `abbr` | yes | The other snippet's abbreviation, matched exactly and case-sensitively |

- The nested snippet's body is parsed and rendered as if it were written in place: its macros run,
  its fields join the same form, its cursor counts.
- Lookup uses the user's effective library, including disabled snippets (disabling only stops a
  snippet from triggering on its own).
- A snippet that includes itself, directly or through others, is `E_SNIPPET_CYCLE`. This is checked
  on the whole template, including branches that would not be taken. Including the same snippet
  twice side by side is allowed.
- Nesting may be 8 snippets deep below the template; a ninth level is `E_TOO_DEEP`.
- Any error in a nested snippet fails the whole expansion with that error.

### 4.5 `{field}`

Ask the user for a value in the fill-in form and insert it.

| Attribute | Required | Value | Default |
| --- | --- | --- | --- |
| `name` | yes | The label shown in the form; 1 to 64 characters, one line | |
| `type` | no | `text` (one line), `area` (several lines), `choice`, `toggle` | `text` |
| `default` | no | `text`, `area`: any text (one line for `text`). `choice`: one of the options. `toggle`: `yes` or `no` | Empty; first option; `no` |
| `options` | `choice` only, required | Options separated by `\|`; each non-empty, no duplicates | |
| `on`, `off` | `toggle` only | Text inserted when checked / unchecked | Empty |

- A field's value is `text`/`area`: what was entered; `choice`: the chosen option; `toggle`: `yes`
  or `no`. What is inserted is the value, except for a toggle, which inserts its `on` or `off`
  text.
- Fields with the same name are one form field, asked once and inserted everywhere. If two
  definitions of one name differ in type or options, that is `E_FIELD_CONFLICT`; if they differ
  only in default, the first definition wins.
- The form lists fields in order of first appearance in the resolved template, reading straight
  through nested snippets and through both branches of every `{if}`. Fields in a branch that ends
  up not being taken are still shown: the form does not change while it is open.
- Using an attribute that does not belong to the field's type (for example `options` on a text
  field) is `E_UNKNOWN_ATTR`.

### 4.6 `{key}`

Press a key.

| Attribute | Required | Value | Default |
| --- | --- | --- | --- |
| `name` | yes | `enter`, `tab`, `escape`, `backspace`, `delete`, `space`, `up`, `down`, `left`, `right`, `home`, `end`, `pageup`, `pagedown` | |
| `shift` | no | `yes` or `no`: hold Shift | `no` |
| `times` | no | Whole number, 1 to 50 | `1` |

Shift is the only modifier, by design rule 5. Adjacent key macros stay separate actions.

### 4.7 `{wait}`

Pause before the next action.

| Attribute | Required | Value |
| --- | --- | --- |
| `ms` | yes | Whole number of milliseconds, 1 to 10000 |

The waits that actually run in one expansion may total at most 30000 ms; more is `E_WAIT_TOTAL`.

### 4.8 `{if}`, `{else}`, `{endif}`

Include text depending on a fill-in field.

| Attribute of `{if}` | Required | Value |
| --- | --- | --- |
| `field` | yes | A field name |
| `is` | no | True when the field's value equals this text exactly |
| `not` | no | True when the field's value differs from this text |

- With neither `is` nor `not`, the condition is true when a toggle is checked, or when a text, area
  or choice value is not empty.
- `is` and `not` compare case-sensitively. A toggle compares as `yes` or `no`. Using both is
  `E_BAD_VALUE`.
- If the field is not defined by any `{field}` in the resolved template: a condition with neither
  `is` nor `not` creates a toggle with that name, unchecked by default, at this position in the
  form; a condition with `is` or `not` is `E_FIELD_UNDEFINED`. A `{field}` with the same name
  anywhere in the template replaces the implicit toggle.
- `{else}` is optional and may appear once. `{if}` may be nested 8 deep; a ninth level is
  `E_TOO_DEEP`. A missing `{endif}`, or an `{else}` or `{endif}` with no open `{if}`, is
  `E_UNBALANCED_IF`. `{else}` and `{endif}` take no attributes.

### 4.9 `{lbrace}`

Insert a literal `{`. No attributes. Needed only to write text that would otherwise be read as a
macro: `{lbrace}date}` types `{date}`.

## 5. Errors

Every error has a stable code. The engine reports the code, the snippet and, for syntax errors,
the position; it never reports what the user typed.

| Code | When | Found |
| --- | --- | --- |
| `E_UNCLOSED_MACRO` | A macro or a quoted value has no end | Parse |
| `E_BAD_SYNTAX` | Malformed attribute: no value, spaces around `=`, no white space between attributes | Parse |
| `E_UNKNOWN_ATTR` | Attribute not accepted by this macro or field type | Parse |
| `E_DUPLICATE_ATTR` | Attribute given twice | Parse |
| `E_MISSING_ATTR` | Required attribute absent | Parse |
| `E_BAD_VALUE` | Value empty where one is required, or outside the allowed set or range | Parse |
| `E_BAD_FORMAT` | Unknown date or time specifier, or a lone `%` | Parse |
| `E_BAD_SHIFT` | Shift term without a sign, number or known unit | Parse |
| `E_UNBALANCED_IF` | `{if}`, `{else}`, `{endif}` do not pair up | Parse |
| `E_TOO_DEEP` | More than 8 levels of `{if}` or of nested snippets | Parse, resolve |
| `E_SNIPPET_NOT_FOUND` | `{snippet}` names an abbreviation that is not in the library | Resolve |
| `E_SNIPPET_CYCLE` | A snippet includes itself, directly or indirectly | Resolve |
| `E_FIELD_CONFLICT` | One field name defined with two types or option lists | Resolve |
| `E_FIELD_UNDEFINED` | `{if}` with `is` or `not` on a field that has no `{field}` | Resolve |
| `E_DATE_RANGE` | A shifted date falls outside the years 1 to 9999 | Render |
| `E_CURSOR_BEFORE_KEY` | A key action follows the cursor in the output | Render |
| `E_WAIT_TOTAL` | Waits that run total more than 30000 ms | Render |
| `E_OUTPUT_TOO_LARGE` | The typed text exceeds 100,000 characters | Render |

What the user sees:

| Where | Behaviour |
| --- | --- |
| Editor | Parse and resolve errors are shown while editing, at the position of the mistake. A snippet with a parse error can be saved but is marked broken and does not trigger |
| Expansion | Nothing is typed, the abbreviation is left as typed, no form is shown. The "why didn't it expand" log gets an entry with the snippet and the code |
| Library health | Lists every snippet with a parse or resolve error, including references broken by a rename or delete |
| Import and sync | Snippets with errors are accepted and stored unchanged, then treated as above |

`E_OUTPUT_TOO_LARGE` has no example file entry because of its size; it is covered by a unit test in
task 1.3. A date-format or time-format setting that is itself invalid is rejected when the setting
is saved, with `E_BAD_FORMAT`.

## 6. Rich snippets

A rich snippet's body is Markdown with the same macros. The rules above apply unchanged, with
these additions:

- Markdown is CommonMark with tables and strikethrough. Raw HTML is not interpreted, and links
  may use only `http`, `https` and `mailto`.
- Macros are found before Markdown is interpreted. Text inserted by `{date}`, `{time}`,
  `{clipboard}` and `{field}` is escaped so that it can never become formatting.
- `{snippet}` naming a rich snippet inserts its Markdown; naming a plain snippet inserts its text,
  escaped. A plain snippet that includes a rich one receives the rich snippet's plain-text
  rendering.
- Each run of text between `key` and `wait` actions is pasted as one piece of formatted content.
- `{cursor}` is ignored in rich snippets, because the caret cannot be moved reliably through
  pasted formatting.

Examples for rich rendering are added with the Markdown renderer in task 3.8; all examples in
version 1 of this specification are plain.

## 7. Outside this specification

- Abbreviation matching, trigger modes and case modes belong to the matcher (architecture §4.4).
  With the `adaptive` case mode, case changes apply to the `text` actions after rendering and never
  to field values or clipboard content. The renderer therefore marks those ranges in each text
  action as verbatim; the example files do not show the marks.
- How actions reach an application belongs to the platform injectors.
- The look and behaviour of the fill-in form belong to the UX specification (task 3.1).

## 8. Example file format

Each file in `spec/template/examples/` is TOML with a list of `[[example]]` tables.

| Key | Meaning |
| --- | --- |
| `id` | Unique, `<group>-<number>` |
| `macro` | Group: `date`, `time`, `clipboard`, `cursor`, `snippet`, `field`, `key`, `wait`, `if` or `syntax` |
| `note` | What the example shows; error cases start with "Error:" |
| `template` | The template source |
| `ctx.now` | Local date and time. Default `2026-03-18T14:05:09`, a Wednesday |
| `ctx.locale` | Default `en` |
| `ctx.date_format`, `ctx.time_format` | Settings. Defaults `%Y-%m-%d` and `%H:%M` |
| `ctx.clipboard` | Clipboard text. Absent means empty |
| `ctx.snippets` | Library for `{snippet}`: abbreviation → body |
| `ctx.values` | Values entered in the form: field name → value. An absent field keeps its default |
| `expect.error` | The error code, for error cases |
| `expect.text` | Shorthand for a single text action (empty or absent with no `actions`: no actions) |
| `expect.actions` | List of `{ text }`, `{ key, shift, times }` (`shift` defaults to false, `times` to 1) and `{ wait }` |
| `expect.cursor_back` | Present only when the output has a cursor |
| `expect.form` | List of `{ name, type, default, options }`. Absent means no fields |

Coverage rules, enforced in CI by `python3 spec/template/reference.py`: at least 80 examples; at
least 5 per group; at least one error case per group; unique ids; every example matches the
reference implementation.

| Group | Examples | Error cases |
| --- | --- | --- |
| `date` | 25 | 5 |
| `time` | 16 | 4 |
| `clipboard` | 10 | 2 |
| `cursor` | 14 | 2 |
| `snippet` | 16 | 8 |
| `field` | 21 | 8 |
| `key` | 14 | 7 |
| `wait` | 11 | 6 |
| `if` | 20 | 8 |
| `syntax` | 19 | 7 |
| Total | 166 | 57 |
