# Settings

Open Settings from the library window: press **Ctrl+,** (**⌘,** on macOS), or choose **Settings**
in the sidebar.

Every setting takes effect as soon as you change it. There is no Save button and no restart.

This page lists every setting in the app, with its default.

## General

| Setting | Choices | Default | What it does |
| --- | --- | --- | --- |
| **Start Iamb when I log in** | On or off | On (set at the end of setup) | Starts Iamb when you sign in to your computer: its background service and the menu icon. On macOS the system may ask you to approve it once, in System Settings under Login Items. If this copy of Iamb cannot start at login, the switch is off and cannot be changed. |
| **Expand snippets as I type** | On or off | On | Turn off to pause Iamb everywhere. The menu has the same switch, **Pause Iamb**. Search still works while paused. |
| **Show the Iamb icon in the menu bar** (macOS) or **in the notification area** (Windows) | On or off | On | Turn off to hide the icon. Iamb keeps running. With the icon hidden, the library window is reached with the [search shortcut](search.md) and **Ctrl+E** (**⌘E** on macOS). |
| **Appearance** | Follow the system, Light, Dark | Follow the system | The colours of the library window. |

## Typing

| Setting | Choices | Default | What it does |
| --- | --- | --- | --- |
| **A snippet expands after** | A space (always on), Punctuation `. , ; : ! ?`, Closing brackets and quotes `) ] } " '`, Enter, Tab | Space, punctuation, and closing brackets and quotes are on. Enter and Tab are off. | The keys that end an abbreviation and let it expand. |
| **New snippets expand** | After a space or punctuation, Immediately | After a space or punctuation | The default for snippets you create from now on. Each snippet can change it. |
| **Capital letters in new snippets** | Exactly as written, Ignore, Ignore and follow mine | Ignore | The default for snippets you create from now on. See [Snippets](snippets.md#options). |

About Enter and Tab: turning either on asks first. Apps act on Enter and Tab before Iamb can type.
A chat sends the message, a dialog closes, a form moves on. With either key on, a snippet can be
typed into whatever has the focus afterwards, and Iamb deletes characters there first. The setting
then shows a warning while it is on. Turn it on only if you use Iamb in plain documents.

## Dates

| Setting | Choices | Default | What it does |
| --- | --- | --- | --- |
| **Date format** | A list of formats, or **Custom…** | `%Y-%m-%d` (for example, 2026-03-18) | The format `{date}` uses. |
| **Time format** | A list of formats, or **Custom…** | `%H:%M` (for example, 14:05) | The format `{time}` uses. |
| **Month and day names in** | English, Español, Deutsch, Français | English | The language of month and day names in `{date}` and `{time}`. |

The list of date formats shows each format with today's date as an example. Date formats offered:
`%Y-%m-%d`, `%d/%m/%Y`, `%m/%d/%Y`, `%d.%m.%Y`, `%B %-d, %Y` and `%-d %B %Y`. Time formats offered:
`%H:%M`, `%H:%M:%S`, `%-I:%M %p` and `%I:%M %p`.

With **Custom…**, type a format. If it has a code Iamb does not know, Iamb says so. For the codes,
see [Templates](templates.md#date-and-time).

## Shortcuts

| Setting | Default | What it does |
| --- | --- | --- |
| **Search snippets** | Ctrl+Shift+Space (⌃⇧Space on macOS) | The shortcut that opens [search](search.md) from any app. |
| **Fill in the last form again** | Not set | Opens the last [fill-in form](forms.md#your-last-answers) again, with your answers, in the app you are in. |
| **Pause or resume** | Not set | Turns expansion off or on everywhere, as the menu's **Pause Iamb** does. |

To change a shortcut, select its box and press the new keys. On Windows a shortcut needs Ctrl, Alt or the
Windows key, and one other key. On macOS it needs Control, Option or Command, and one other key. A
function key can stand alone. Iamb does not take the editing shortcuts every app uses (Ctrl or ⌘
alone with C, V, X, Z, A or Y, and with Delete), nor a shortcut that types a character: Option
(Alt) with a letter or digit.

On Windows, Iamb does not take Ctrl+Alt with a letter or digit (with or without Shift) on any
keyboard. On many keyboards Ctrl+Alt is AltGr, which types characters such as @, € or ą, so a
shortcut there would take those characters away. Iamb refuses it on keyboards without AltGr too,
where Ctrl+Alt with a letter is a common pattern for shortcuts, because the keyboard layout can
change at any time. Use Ctrl+Shift instead, or add the Windows key (Ctrl+Alt+Win+K works).

Backspace clears the shortcut. Esc leaves it as it was. If
another app already uses the shortcut, Iamb says so and keeps the old one. When the search shortcut
is not the default, a button next to it puts the default back. The other two have no default.

Shortcuts inside the library window are fixed. See [Keyboard](keyboard.md).

## Apps

### Never expand in these apps

Iamb does not expand snippets in these apps, and it does not keep the keys you type there. Add
password managers, remote desktop apps and terminals where you type passwords. See
[Exclusions](exclusions.md).

### If snippets come out wrong in an app

Use these settings only when an app gets the text wrong. Each app you change gets its own row.

| Setting | Choices | Default | What it does |
| --- | --- | --- | --- |
| **Send the text** | Let Iamb decide, Type it key by key, Paste it | Let Iamb decide | How the text reaches the app. |
| **Line breaks as** | Let Iamb decide, Enter, Shift+Enter | Let Iamb decide | The key that makes a new line in the app. |
| **Slow down by** | A whole number of milliseconds, 0 to 1000 on macOS or 0 to 10000 on Windows | Not set | Pauses between key presses, for apps that drop characters. |

The app is named by its bundle identifier (on macOS, for example `com.example.bank`) or its program
file name (on Windows, for example `mybank.exe`). Type it in the box under the list, choose **Add
app**, then set the choices on the app's row. **Remove** on the row forgets them.

## Privacy

| Setting | Default | What it does |
| --- | --- | --- |
| **Usage counts** | Kept | How often and when each snippet was used, kept on this computer only. **Reset usage counts…** forgets them. Your snippets do not change. |
| **Send crash reports** | Off | The switch is saved, but this version has nothing that sends reports. A report says where Iamb failed and never contains what you typed, your snippets or your clipboard. See [Privacy](privacy.md#crash-reports). |

<!-- TODO: remove this note when the desktop app can send crash reports. No code in the UI host reads or uploads them yet (crash.rs, Cargo.toml). The Settings page still says "A report says where Iamb failed". -->

Three statements that Settings also shows:

- Keys you type are held in memory for a moment and never saved or sent.
- How often you use a snippet is counted on this computer only.
- The log of why something did not expand lives in memory and is gone when Iamb restarts.

## About

- **Version**: the version of Iamb, for example **Iamb 0.1.0**.
- **Licence**: AGPL-3.0-or-later.
- **Source code**: the address of the project repository, shown as text.
- **Log folder**: where Iamb writes its log files, and an **Open the log folder** button that opens
  it in Finder or File Explorer. The logs never contain what you type. See
  [Privacy](privacy.md#what-iamb-stores-and-where).

## Settings not in this version

These settings are planned but not in the app yet:

- Check for updates and install them.
- Account and sync settings (see [Sync](sync.md) and [Accounts](accounts.md)).
