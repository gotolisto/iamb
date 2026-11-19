# Iamb design system

Status: proposed for review · Task 3.1 · Tokens: [`tokens.json`](tokens.json) ·
Specification: [`spec.md`](spec.md) · Mock-ups: [`mockups/index.html`](mockups/index.html)

This document defines what the Iamb desktop UI is drawn with: the tokens (colour, type, spacing,
radii, elevation, motion) and the components with every state. `tokens.json` is the source of
truth; where this document and the file disagree, the file is right and this document has a
defect.

## 1. Character

Iamb is a utility that people meet for a fraction of a second, many times a day. The interface
is therefore plain, dense and still.

- **Native first.** The system font, the system's window chrome, the system's button order. No
  bundled font, no custom title bar.
- **One accent, used for one thing:** the thing the Enter key will do, and the place the focus
  is. Everything else is a neutral with a slight blue cast, so that greys sit with the accent.
- **Colour never carries meaning alone.** Every status has an icon and words; every macro chip
  has an icon and a label; a match in search is heavier as well as tinted.
- **The mark** is the scansion of an iamb, a short syllable and a long one: a breve and a
  macron (˘ ¯). It is the product in two strokes: something short becomes something long. It is
  drawn in one colour so it works as a menu-bar template image and as a tray icon.

## 2. Tokens

`tokens.json` follows the Design Tokens Format Module (W3C Design Tokens Community Group,
2025.10): every token has a `$value`, groups set the `$type`, colours are objects with a colour
space, components and a hex fallback, and dimensions and durations are objects with a value and
a unit.

- Colours and shadows exist once per theme, under `color.light` / `color.dark` and
  `shadow.light` / `shadow.dark`, with identical names. A component never names a theme; it
  names `color.bg.surface` and the theme decides.
- Everything else (type, spacing, radii, sizes, motion) is shared by both themes.
- In CSS a token is a custom property whose name is the token's path joined with hyphens,
  without the theme: `color.light.bg.surface` and `color.dark.bg.surface` are both
  `--color-bg-surface`. Typography tokens become a `font` shorthand, `--type-body`. The light
  values are defined on `:root`; the dark values replace them under
  `@media (prefers-color-scheme: dark)` unless the user chose light, and under
  `:root[data-theme="dark"]` when the user chose dark. `mockups/build.py` generates exactly
  this from the file; task 3.2 should generate the app's stylesheet the same way so the two
  cannot drift.

### 2.1 Colour

| Group | Tokens | Use |
| --- | --- | --- |
| `bg` | `app`, `surface`, `subtle`, `hover`, `selected`, `highlight`, `inverse`, `scrim` | Window background; content surfaces; quiet fills; hovered and selected rows; search matches; toasts; the veil behind a dialog |
| `text` | `primary`, `secondary`, `disabled`, `on-accent`, `on-inverse`, `on-danger` | Text. `secondary` is still full AA text; only `disabled` is below it |
| `border` | `subtle`, `control`, `focus` | Decorative dividers; the outline of a control (3:1); the focus ring |
| `accent` | `solid`, `solid-hover`, `text` | Primary button and checked controls; links |
| `status` | `success`, `warning`, `danger`, each with `text` and `bg`; `danger.solid` | Banners, badges, field messages, the destructive button |
| `macro` | `insert`, `ask`, `act`, `read`, each with `text` and `bg` | Macro chips, by what the macro does (§4.6) |
| `tag` | `red`, `orange`, `yellow`, `green`, `teal`, `blue`, `purple`, `grey` | The dot of a tag. The stored `Tag.color` is one of these eight names, not a hex value, so a tag looks right in both themes |

Light is white surfaces on a faintly blue-grey window; dark is the same structure inverted, with
the accent lightened to a periwinkle that carries dark text. Neither theme uses pure black.

### 2.2 Contrast checks

WCAG 2.1 relative-luminance contrast, computed by `python3 docs/ux/mockups/build.py --contrast`
from the token values and floored to two decimals. Text needs 4.5:1 (success criterion 1.4.3);
the outline of a control, a focus ring and a meaningful graphic need 3:1 (1.4.11). All 88 checks
pass; `build.py --check` fails when one stops passing.

| Foreground | Background | Use | Needs | Light | Dark |
| --- | --- | --- | --- | --- | --- |
| `text.primary` | `bg.surface` | Body text on a surface | 4.5 | 17.72 (`#16181F` on `#FFFFFF`) | 14.67 (`#ECEEF5` on `#1A1C23`) |
| `text.primary` | `bg.app` | Body text on the window background | 4.5 | 16.43 (`#16181F` on `#F6F6F9`) | 16.00 (`#ECEEF5` on `#121318`) |
| `text.primary` | `bg.subtle` | Text in a neutral chip or well | 4.5 | 15.30 (`#16181F` on `#EDEEF3`) | 12.51 (`#ECEEF5` on `#262933`) |
| `text.primary` | `bg.hover` | Text of a hovered row | 4.5 | 14.77 (`#16181F` on `#E9EAF1`) | 11.83 (`#ECEEF5` on `#2A2D38`) |
| `text.primary` | `bg.selected` | Text of a selected row | 4.5 | 14.08 (`#16181F` on `#E1E4FA`) | 10.69 (`#ECEEF5` on `#2B3060`) |
| `text.primary` | `bg.highlight` | Matched characters in search results | 4.5 | 14.75 (`#16181F` on `#FFE9A8`) | 7.47 (`#ECEEF5` on `#5A4A12`) |
| `text.secondary` | `bg.surface` | Help text and metadata on a surface | 4.5 | 7.08 (`#535868` on `#FFFFFF`) | 7.59 (`#A8ADBD` on `#1A1C23`) |
| `text.secondary` | `bg.app` | Help text on the window background | 4.5 | 6.56 (`#535868` on `#F6F6F9`) | 8.28 (`#A8ADBD` on `#121318`) |
| `text.secondary` | `bg.subtle` | Keyboard badge | 4.5 | 6.11 (`#535868` on `#EDEEF3`) | 6.47 (`#A8ADBD` on `#262933`) |
| `text.secondary` | `bg.hover` | Metadata of a hovered row | 4.5 | 5.90 (`#535868` on `#E9EAF1`) | 6.12 (`#A8ADBD` on `#2A2D38`) |
| `text.secondary` | `bg.selected` | Metadata of a selected row | 4.5 | 5.62 (`#535868` on `#E1E4FA`) | 5.53 (`#A8ADBD` on `#2B3060`) |
| `text.on-accent` | `accent.solid` | Primary button label | 4.5 | 7.40 (`#FFFFFF` on `#3B45C4`) | 7.58 (`#14163A` on `#9BA4FF`) |
| `text.on-accent` | `accent.solid-hover` | Primary button label, hovered | 4.5 | 9.32 (`#FFFFFF` on `#2F38A8`) | 9.53 (`#14163A` on `#B4BBFF`) |
| `text.on-inverse` | `bg.inverse` | Toast and notice text | 4.5 | 13.64 (`#F1F2F6` on `#23252E`) | 14.49 (`#16181F` on `#E6E8F0`) |
| `text.on-danger` | `status.danger.solid` | Destructive button label | 4.5 | 6.53 (`#FFFFFF` on `#B3261E`) | 8.24 (`#3B0D0A` on `#FF9A91`) |
| `accent.text` | `bg.surface` | Link on a surface | 4.5 | 7.99 (`#3640BD` on `#FFFFFF`) | 8.44 (`#A9B1FF` on `#1A1C23`) |
| `accent.text` | `bg.app` | Link on the window background | 4.5 | 7.41 (`#3640BD` on `#F6F6F9`) | 9.20 (`#A9B1FF` on `#121318`) |
| `accent.text` | `bg.selected` | Link in a selected row | 4.5 | 6.35 (`#3640BD` on `#E1E4FA`) | 6.14 (`#A9B1FF` on `#2B3060`) |
| `status.success.text` | `status.success.bg` | Success banner and badge | 4.5 | 5.74 (`#176B3A` on `#E3F4EA`) | 8.35 (`#7ED9A0` on `#15301F`) |
| `status.success.text` | `bg.surface` | “Granted” in a permission row | 4.5 | 6.56 (`#176B3A` on `#FFFFFF`) | 9.98 (`#7ED9A0` on `#1A1C23`) |
| `status.warning.text` | `status.warning.bg` | Warning banner and badge | 4.5 | 6.61 (`#7A4A00` on `#FCF0D6`) | 8.07 (`#F2C063` on `#3A2C0E`) |
| `status.warning.text` | `bg.surface` | Warning text under a field | 4.5 | 7.48 (`#7A4A00` on `#FFFFFF`) | 10.12 (`#F2C063` on `#1A1C23`) |
| `status.danger.text` | `status.danger.bg` | Error banner and badge | 4.5 | 5.54 (`#B3261E` on `#FCE8E6`) | 7.49 (`#FF9A91` on `#3F1A17`) |
| `status.danger.text` | `bg.surface` | Error text under a field | 4.5 | 6.53 (`#B3261E` on `#FFFFFF`) | 8.33 (`#FF9A91` on `#1A1C23`) |
| `macro.insert.text` | `macro.insert.bg` | Chip: date, time, snippet | 4.5 | 8.19 (`#2C3599` on `#E4E6FB`) | 7.79 (`#C2C8FF` on `#2A2F5E`) |
| `macro.ask.text` | `macro.ask.bg` | Chip: field, if, else, endif | 4.5 | 6.60 (`#0B5D54` on `#D9F2EE`) | 8.17 (`#8FE0D3` on `#123A36`) |
| `macro.act.text` | `macro.act.bg` | Chip: key, wait, cursor | 4.5 | 6.98 (`#8F1D57` on `#FBE3EF`) | 7.53 (`#F7A8CE` on `#4A1D36`) |
| `macro.read.text` | `macro.read.bg` | Chip: clipboard | 4.5 | 6.35 (`#7A4A00` on `#FBEBCB`) | 7.91 (`#F0C674` on `#40300F`) |
| `border.control` | `bg.surface` | Input outline on a surface | 3.0 | 3.88 (`#7C8192` on `#FFFFFF`) | 4.09 (`#767C90` on `#1A1C23`) |
| `border.control` | `bg.app` | Input outline on the window background | 3.0 | 3.59 (`#7C8192` on `#F6F6F9`) | 4.46 (`#767C90` on `#121318`) |
| `border.control` | `bg.subtle` | Input outline inside a well | 3.0 | 3.34 (`#7C8192` on `#EDEEF3`) | 3.49 (`#767C90` on `#262933`) |
| `border.focus` | `bg.surface` | Focus ring on a surface | 3.0 | 7.40 (`#3B45C4` on `#FFFFFF`) | 7.41 (`#9BA4FF` on `#1A1C23`) |
| `border.focus` | `bg.app` | Focus ring on the window background | 3.0 | 6.86 (`#3B45C4` on `#F6F6F9`) | 8.08 (`#9BA4FF` on `#121318`) |
| `border.focus` | `bg.selected` | Focus ring on a selected row | 3.0 | 5.88 (`#3B45C4` on `#E1E4FA`) | 5.39 (`#9BA4FF` on `#2B3060`) |
| `accent.solid` | `bg.surface` | Checked switch, checkbox and radio | 3.0 | 7.40 (`#3B45C4` on `#FFFFFF`) | 7.41 (`#9BA4FF` on `#1A1C23`) |
| `status.danger.solid` | `bg.surface` | Destructive button against its surface | 3.0 | 6.53 (`#B3261E` on `#FFFFFF`) | 8.33 (`#FF9A91` on `#1A1C23`) |
| `tag.red` | `bg.surface` | Tag dot | 3.0 | 5.33 (`#C4372F` on `#FFFFFF`) | 7.45 (`#FF8A80` on `#1A1C23`) |
| `tag.orange` | `bg.surface` | Tag dot | 3.0 | 4.59 (`#B85C00` on `#FFFFFF`) | 8.79 (`#FFA65C` on `#1A1C23`) |
| `tag.yellow` | `bg.surface` | Tag dot | 3.0 | 4.59 (`#8F7200` on `#FFFFFF`) | 10.32 (`#E6C84F` on `#1A1C23`) |
| `tag.green` | `bg.surface` | Tag dot | 3.0 | 4.96 (`#2B7F47` on `#FFFFFF`) | 9.16 (`#74D197` on `#1A1C23`) |
| `tag.teal` | `bg.surface` | Tag dot | 3.0 | 4.98 (`#0F7C80` on `#FFFFFF`) | 9.25 (`#5CD0D3` on `#1A1C23`) |
| `tag.blue` | `bg.surface` | Tag dot | 3.0 | 5.07 (`#2F6BD0` on `#FFFFFF`) | 8.14 (`#8AB4FF` on `#1A1C23`) |
| `tag.purple` | `bg.surface` | Tag dot | 3.0 | 5.69 (`#7D4CC2` on `#FFFFFF`) | 7.99 (`#C3A1FF` on `#1A1C23`) |
| `tag.grey` | `bg.surface` | Tag dot | 3.0 | 5.01 (`#6A6F7E` on `#FFFFFF`) | 7.59 (`#A8ADBD` on `#1A1C23`) |

Not checked, on purpose: `text.disabled` (disabled controls are exempt), `border.subtle`
(decorative; no information is lost without it) and the background of a chip against its
surface (the chip is identified by its icon and label, which are checked).

Rules that keep the table true:

- Text on `bg.hover`, `bg.selected` and `bg.highlight` is `text.primary` or `text.secondary`
  only (matched characters are always `text.primary`).
- Status text goes on its own status background or on `bg.surface`, nowhere else.
- A macro chip's text goes only on its own chip background.

### 2.3 Type

The interface font is the operating system's (`font.family.ui`); abbreviations, template source
and key names use the system's monospace (`font.family.mono`). No font file ships with Iamb.

| Token | Size / line | Weight | Use |
| --- | --- | --- | --- |
| `typography.display` | 28 / 34 | 600 | Onboarding welcome |
| `typography.heading` | 22 / 28 | 600 | Onboarding step headings, page titles |
| `typography.title` | 18 / 24 | 600 | Dialog, panel and pane titles |
| `typography.input` | 15 / 22 | 400 | Text typed into the fill-in form, the palette and the onboarding fields |
| `typography.body` | 13 / 20 | 400 | Rows, labels, buttons (500), menus |
| `typography.body-strong` | 13 / 20 | 600 | Selected tree item, emphasised labels |
| `typography.caption` | 12 / 16 | 400 | Help text, metadata, badges (500) |
| `typography.overline` | 11 / 16 | 600 | Group labels, upper case, 0.4 px tracking |
| `typography.code` | 13 / 20 | 400 | Template source, abbreviation fields |
| `typography.code-small` | 12 / 16 | 400 | Abbreviation badges |
| `typography.kbd` | 11 / 16 | 500 | Keyboard hint badges |

Sizes are written in px in the tokens and applied in `rem` (13 px = 0.8125 rem), so the whole
interface follows the system's text-size setting and the window's zoom (⌘+ / Ctrl++, 100% to
200%). Nothing is ever smaller than 11 px at 100%. Running text is at most 76 characters wide.
Numbers that line up in a column use tabular figures.

### 2.4 Spacing, radii, sizes

| Group | Values | Rule |
| --- | --- | --- |
| `space` | 2, 4, 8, 12, 16, 20, 24, 32, 40, 48 px | A 4 px grid. Inside a control 8; between a label and its control 4; between fields 12; between groups 16 to 24; window padding 16 (panels) or 24 (onboarding) |
| `radius` | `sm` 4, `md` 6, `lg` 10, `full` | Chips and badges; controls and rows; panels, dialogs and menus; switches, dots and pills |
| `size.control-*` | 24, 28, 36 px | Compact buttons; standard buttons, inputs and rows; fill-in and palette inputs |
| `size.sidebar`, `size.list` | 220, 320 px | Default widths of the library's panes; both resizable, the last width is remembered per window |
| `size.form-panel`, `size.palette` | 440, 620 px | Width of the two panels. Their height follows the content up to 70% of the screen |

The smallest pointer target is 24 × 24 px (`size.control-sm`), with at least 4 px between
neighbours. Rows and menu items are full-width targets.

### 2.5 Elevation

Three shadows, each a soft drop plus a one-pixel ring so the edge holds on any background
(the panels float over other apps): `shadow.popover` for menus, autocomplete lists and chip
cards; `shadow.panel` for the fill-in form and the palette; `shadow.dialog` for dialogs inside
the library window. Panes of a window are separated by `border.subtle`, never by shadow.

### 2.6 Motion

| Token | Value | Use |
| --- | --- | --- |
| `duration.instant` | 0 ms | The fill-in form and the palette, showing and hiding. Their budgets (150 ms and 100 ms) leave no room for an entrance |
| `duration.fast` | 80 ms | Hover, press, focus ring |
| `duration.base` | 140 ms | Popovers, toasts, switches |
| `duration.slow` | 220 ms | Dialogs, onboarding step changes |
| `easing.standard`, `easing.exit` | cubic-bezier(0.2, 0, 0, 1), cubic-bezier(0.4, 0, 1, 1) | Entering or moving; leaving |

Only opacity and transforms of at most 8 px are animated. With "reduce motion" on in the
system, every duration is 0 except opacity fades, which stay at 80 ms. Nothing loops, blinks or
moves without the user acting, with one exception: the text caret.

## 3. Icons

One set, drawn for Iamb: a 16 × 16 grid, 1.5 px strokes, round caps and joins, `currentColor`,
no fills. The mock-ups carry the whole set as an inline sprite
(`mockups/src/icons.svg`); the app should ship the same file. An icon next to a label is hidden
from assistive technology; an icon that stands alone has an accessible name. Icons are never
the only way to tell two states apart.

## 4. Components

Every component below is drawn, with its states, on the mock-up overview page
([`mockups/index.html`](mockups/index.html)) and used in the flows. "States" lists what must
be designed and tested; "Keys" is what the component itself handles; "Name" is what a screen
reader announces.

Shared rules:

- **Focus.** Every interactive element shows a 2 px `border.focus` ring, 2 px away from it
  (1 px for inputs), whenever it has keyboard focus. The ring is never removed, only hidden for
  pointer clicks on buttons.
- **Disabled.** `text.disabled` on `bg.subtle`, no hover, not focusable, and always paired
  with a visible reason nearby (a disabled Save sits next to the message that explains it).
- **Loading.** A control that waits for the service keeps its label and shows a 12 px spinner
  after 300 ms; nothing in the product uses a skeleton screen, because nothing should take
  that long.

### 4.1 Button

| | |
| --- | --- |
| Variants | Primary (one per view: what Enter does), default, quiet (toolbar and "Back"), link, danger (destructive, only inside a confirmation), icon-only |
| Sizes | 24 (in rows and toolbars), 28 (standard), 36 (the main action of an onboarding step) |
| States | Rest, hover, pressed (same as hover), focus, disabled, busy |
| Content | A verb that says what happens: "Save", "Insert", "Stop sharing". Never "OK" or "Yes". The key that triggers it may follow as a keyboard badge |
| Order | macOS: the main action is rightmost. Windows: the main action is leftmost of the group. Destructive and "do not save" choices sit apart, on the far side |
| Keys | Enter or Space activates. In a dialog, Enter activates the primary button from any non-text control |
| Name | Its label; an icon-only button has an `aria-label` ("More actions") |

### 4.2 Text input, text area, select, hotkey recorder

| | |
| --- | --- |
| Anatomy | Label above (always visible; placeholders are examples, never labels), control, then one line of help, warning or error |
| States | Rest, hover, focus, filled, error (outline and message in `status.danger.text`, with an icon), warning (message only; the outline stays), disabled, read-only (shared folder without edit rights) |
| Sizes | 28 px in the library and settings; 36 px in the fill-in form, the palette and onboarding |
| Select | A button that opens a list; arrow keys change the value without opening, typing jumps to an option, Enter or Space opens |
| Hotkey recorder | A field that says "Press the keys" while focused, shows the chord as key badges, rejects a chord without a modifier ("Add Ctrl, Alt or the Windows key") and one the system refuses ("Another app already uses this shortcut"). Esc leaves it unchanged; Backspace clears it |
| Name | The label, then the value, then the message (`aria-describedby`). Errors are announced when they appear (`role="alert"`), warnings politely |

### 4.3 Checkbox, radio group, switch, segmented control

| | |
| --- | --- |
| Checkbox | A setting inside a form that takes effect on Save, and fill-in toggles |
| Switch | A setting that takes effect at once ("On" for a snippet, "Start Iamb when I log in") |
| Radio group | A choice of two to six with explanations; arrow keys move and select, Tab leaves the group |
| Segmented control | Two or three short, equal choices that change the view ("Plain text / Formatted") |
| States | Off, on, focus, disabled; checkbox also "mixed" for a multi-selection |
| Name | Label and state. The group has its own label ("Expand") |

### 4.4 Snippet row

Two lines, 48 px: label (or the first line of the content when there is no label) and
abbreviation badge; below, the first line of the content with blanks shown as ‹names›.

| | |
| --- | --- |
| Marks | "Asks for values" icon (has fill-in fields); Formatted icon (rich); badges "Off", "Broken", "Needs review", "Yours is used" (lost an abbreviation clash) |
| States | Rest, hover, selected, selected without window focus (same fill, no ring), focus, off (label and abbreviation struck through and `text.secondary`), dragging, drop target |
| Keys | ↑ ↓ move, Home and End jump, typing jumps to a label, Enter opens in the editor, Delete deletes (with Undo in a toast), Space toggles On/Off, ⌘C / Ctrl+C copies the content, multi-selection with Shift and ⌘ / Ctrl |
| Name | "Late delivery, abbreviation semicolon r late, asks for values, 3 of 42". `role="option"` inside a `listbox`; only visible rows exist in the page, and the list announces the true count |

### 4.5 Folder tree

28 px rows, 16 px indent per level, at most 4 levels (the data model's limit). A folder shows
its name and the count of snippets inside; a shared folder has the people icon instead of the
folder icon and says "shared" in its name for assistive technology.

| | |
| --- | --- |
| States | Rest, hover, selected, focus, expanded or collapsed, renaming (the name becomes an input), drop target, read-only |
| Keys | ↑ ↓ move, → expands or enters, ← collapses or goes to the parent, Enter or F2 renames, Delete deletes after confirmation, ⌘⇧N / Ctrl+Shift+N new folder |
| Name | `role="tree"`; "Replies, 42 snippets, expanded, level 1" |

### 4.6 Macro chip

A macro of the template language, shown in the editor as one unbreakable unit, 20 px high, with
an icon and a label. Colour groups the macros by what they do, which is also what a reviewer of
someone else's snippet needs to see at a glance (ADR-0022).

| Group | Macro | Chip label | Icon |
| --- | --- | --- | --- |
| Insert (`macro.insert`) | `{date}` | "Date", or with the shift in words: "Date +3 working days" | calendar |
| | `{time}` | "Time", "Time +2 hours" | clock |
| | `{snippet abbr=…}` | The abbreviation: ";sig" | braces |
| Ask (`macro.ask`) | `{field}` | The field's name; the icon tells the type: text, several lines, choice, toggle | field, lines, options, switch |
| | `{if}` … `{else}` … `{endif}` | "If Add voucher", "If Bike is Gravel", "If Bike is not Gravel"; "Else"; "End if" | branch |
| Act (`macro.act`) | `{key}` | The key as the keyboard shows it: "Enter", "Tab ×2", "Shift+Tab" | return arrow |
| | `{wait}` | "Wait 0.5 s" | hourglass |
| | `{cursor}` | "Cursor" | text cursor |
| Read (`macro.read`) | `{clipboard}` | "Clipboard" | clipboard |

`{lbrace}` has no chip: the editor shows a plain `{` and writes the macro itself when the text
would otherwise be read as one.

| | |
| --- | --- |
| States | Rest, hover, selected (focus ring), error (a red inner outline plus the message under the editor), in preview (neutral grey with the field's name: a blank that has no value yet), dragging |
| Keys | A chip is one character for ← →, Shift+arrows, Backspace and Delete. Enter or Space on a selected chip opens its card; ⌘C / Ctrl+C copies its source text |
| Name | "Fill-in field, First name, text" · "Date, plus 3 working days" · "Key press, Enter". The editor is a text box; chips are announced inline as the caret passes them |

### 4.7 Tag, abbreviation badge, status badge, keyboard badge

| Component | Look | Notes |
| --- | --- | --- |
| Tag | A pill with a coloured dot and the name | The name is always shown. Removable in the editor (× button, or Backspace in the tag field) |
| Abbreviation badge | Monospace on `bg.subtle`, radius 4 | Always the full abbreviation, never truncated in the middle; a long one is cut at the end with an ellipsis and has the full text as its name |
| Status badge | A pill: neutral, success, warning or danger, with an icon for the last three | "Off", "Granted", "Needs review", "Broken", "Waiting", "Owner" |
| Keyboard badge | Monospace 11 px on `bg.subtle` with a darker lower edge | macOS shows symbols (⌘ ⇧ ⌥ ⌃ ⏎ esc), Windows shows names joined with + (Ctrl+Shift+S). Hidden from assistive technology when the control already exposes the shortcut (`aria-keyshortcuts`) |

### 4.8 Dialog

Used only for a decision that cannot be undone or that needs several inputs: leaving unsaved
changes, deleting a folder, sharing, reviewing a received snippet. Never for information.

| | |
| --- | --- |
| Anatomy | Title that is the question or the task, body, actions. Width 480 px, or 620 px for review |
| States | Open, busy (actions disabled, the primary shows a spinner), error (a banner above the actions) |
| Keys | Focus moves in and is trapped; Esc is the cancel button; Enter is the primary button, except in the review dialog, where focus starts on "Not now" |
| Name | `role="dialog"` (or `alertdialog` for a destructive confirmation), labelled by its title |

### 4.9 Toast and notice

| | Toast | Notice |
| --- | --- | --- |
| Where | Bottom centre of an Iamb window | A corner of the screen, over other apps (top right on macOS, bottom right on Windows) |
| When | Something the user did succeeded, or can be undone ("Deleted “Opening hours”. Undo") | The user asked for something outside Iamb (a search pick, a form insert) and it could not be typed |
| Behaviour | Leaves after 4 s (10 s with an action); stays while hovered or focused; one at a time | Leaves after 6 s; takes no focus; one at a time. Never shown for a typed abbreviation that did not expand |
| Look | `bg.inverse`, one line, at most one action | `bg.inverse`, a title and one sentence |
| Name | `role="status"`, announced politely | `role="alert"` |

### 4.10 Banner

A message that belongs to a view and stays until its cause is gone: missing permissions, a
snippet that will not expand, snippets waiting for review, the service not running. Neutral,
success, warning or danger; an icon, one or two sentences, at most one button. It is never
dismissible while the cause remains.

### 4.11 Menu, popover, autocomplete list

| | |
| --- | --- |
| Tray and menu-bar menu | The system's native menu, not a web view (the mock-ups draw it for review). First line: status in words |
| Context menu | The same actions as the "More actions" button of the row or folder; never the only way to reach something |
| Chip card | A popover anchored to a chip with that macro's settings and a one-line preview |
| Insert list | Opens when `{` is typed in the editor; filters as the user types; nine items with a one-line explanation each |
| Keys | ↑ ↓ move, Enter chooses, Esc closes and returns focus to where it came from, typing filters or jumps |

### 4.12 Permission status row

One row per permission the system requires (macOS: Input Monitoring and Accessibility).

| | |
| --- | --- |
| Anatomy | Status disc (an eye or key icon on amber while missing, a tick on green when granted), name, one sentence of what it lets Iamb do, the state in words, a button that opens the right page of System Settings |
| States | Not granted yet; granted (the button disappears); taken away later (same as not granted, with "This was turned off"); unknown (the service is not answering: "Checking…", then the service banner) |
| Behaviour | The state comes from the service's worker and updates by itself; the row never asks the user to press "check again" |
| Name | "Input Monitoring: not granted yet. Lets Iamb notice when you type an abbreviation. Button: Open settings" and, on change, a polite announcement "Input Monitoring: granted" |

### 4.13 Fill-in fields

The four field types of the template language, as they appear in the fill-in form. All are
36 px controls with the label above; none is ever required, so none has an error state.

| Type | Control | Default shown | Keys |
| --- | --- | --- | --- |
| `text` | One-line input | The field's default, or the last value | Enter inserts the snippet |
| `area` | Text area, 3 lines, grows to 8 and then scrolls | Same | Enter is a new line; ⌘⏎ / Ctrl+Enter inserts |
| `choice`, up to 6 options | Radio list, one option per row | The default option, or the last one chosen | ↑ ↓ choose; typing a letter jumps; Enter inserts |
| `choice`, 7 options or more | Select with type-to-filter | Same | ↑ ↓ change the value; Space opens the list; Enter inserts when the list is closed |
| `toggle` | Checkbox with the field's name as its label | Checked when the default is `yes` | Space flips; Enter inserts |

A field that appears only inside an optional part is still listed (the form does not change
while it is open, as the template language requires). The toggle that controls an optional part
is an ordinary toggle with the name the author gave it.

### 4.14 Panels: the fill-in form and the search palette

Borderless floating panels with `shadow.panel` and radius 10 on both systems; no title bar, no
close button (Esc closes). A header or query row, a body that scrolls when long, and a footer
on `bg.app` with the key hints on the left and, in the form, the two buttons on the right.
They never animate, never resize while the user types (the palette keeps a fixed height of
eight rows once there is a query), and always appear in the same place.

### 4.15 Template editor field

A text box that holds text and chips, with the insert toolbar above and the preview below.
States: rest, focus, error (message under the field, the faulty chip outlined), read-only.
Line height 24 px so chips do not change the height of a line. The preview is a quiet well
(`bg.subtle`) labelled "Preview", read-only and selectable.

### 4.16 Empty state

A centred mark, one line that names the state ("No snippets yet"), one sentence of what would
be here, and the one or two actions that fill it. Used for an empty library, an empty folder, a
filter without results, an empty log and a healthy library ("Nothing to fix").

### 4.17 Change view

For the review of a changed snippet: unchanged lines plain, removed lines struck through on
`status.danger.bg`, added lines on `status.success.bg`, each also announced as "Removed" or
"Added". Monospace, wrapped, never scrolled sideways.

## 5. System settings the UI must follow

| Setting | Behaviour |
| --- | --- |
| Light or dark appearance | Followed live; Settings → General can pin one |
| Increased contrast (macOS) and contrast themes (Windows, `forced-colors`) | System colours replace the tokens; every boundary that is a background change in the normal themes (selected row, chip, badge, switch, panel edge) gains a 1 px outline in `CanvasText`; the focus ring uses `Highlight`; icons use `currentColor` so they follow |
| Reduce motion | §2.6 |
| Reduce transparency | Nothing in Iamb is translucent; no change needed |
| Text size and zoom | §2.3; every layout must hold at 200% in a 1280 × 800 window without horizontal scrolling of any pane |
| Accent colour of the system | Not followed: the accent is part of the contrast checks |

## 6. Rebuilding the mock-ups

    python3 docs/ux/mockups/build.py            # writes the seven HTML files
    python3 docs/ux/mockups/build.py --check    # fails on a stale file or a failed contrast check
    python3 docs/ux/mockups/build.py --contrast # prints the table of §2.2

The pages are assembled from `mockups/src/` (the page bodies, one stylesheet, the icon sprite,
a few lines of script for the theme and chrome switches). The script needs only the Python
standard library.
