# Privacy

Iamb is built to keep what you type to itself. This page says what Iamb keeps, where it keeps it,
what leaves your computer, and what Iamb cannot protect you from.

## What you type

- Iamb keeps the keys you type in memory for a moment, so it can match an abbreviation. It never
  saves them to disk, never writes them to a log, and never sends them over a network.
- Iamb asks the operating system to keep the memory that holds your last keys out of the swap file
  or page file, and overwrites that memory with zeros when it is no longer needed. If the system
  refuses the request, Iamb writes a warning to its log and keeps working. A few short-lived
  copies of the key being handled can remain in ordinary memory until later work overwrites them.
- In password fields and in apps you have excluded, Iamb does not expand and does not match what
  you type. Each key still passes through the same memory for an instant, and is then dropped
  without being stored. See [Exclusions](exclusions.md).
- The background part of Iamb that reads your keys has no network code. Automatic checks in the
  project's build pipeline look for it.

## What Iamb stores, and where

Iamb keeps its files in your user account only. Other users of the computer cannot read them.
Iamb does not encrypt them: they are protected by your account's file permissions, not by a
password.

| What | Where (macOS) | Where (Windows) |
| --- | --- | --- |
| Your library: snippets (with their text), folders, tags, settings, excluded apps, usage counts | `~/Library/Application Support/Iamb/library.db` | `%APPDATA%\Iamb\library.db` |
| Images used in formatted snippets | An `attachments` folder beside the library | The same, under `%APPDATA%\Iamb\` |
| Logs and local crash reports | A `logs` folder beside the library | The same |

Because the library holds your snippet text, do not put secrets in a snippet that you would not want
on this computer's disk.

The logs record what Iamb did, such as which message was handled and how long it took. They never
record what you typed, what you filled into a form, or your clipboard.

Some things are kept in memory only, and are gone when Iamb restarts:

- **Why didn't it expand?** This log holds up to 200 entries. It names a snippet only when one
  matched, or when a search or form request for a named snippet was refused. An entry can also name
  the app. It never quotes a key.
- **Your last answers** in a fill-in form. They are cleared after 30 minutes without use, when the
  screen locks, when you sign out, and when Iamb restarts.
- **Test an abbreviation**. What you type there is not stored.

## Usage counts

Iamb counts how often and when you used each snippet. The counts are stored in the library on this
computer. They are used to sort lists and to show snippets you have not used for 90 days. You can
reset them in [Settings](settings.md#privacy).

## The clipboard

Iamb reads the clipboard's text when a snippet uses `{clipboard}`, once, at the moment the
abbreviation matches.

Iamb also uses the clipboard to paste: formatted snippets, and long text in apps where pasting works
better than typing. For that it saves what was on the clipboard, pastes, and puts the saved content
back. If it cannot save the clipboard whole, it does not paste. While this happens the content is
briefly on the clipboard.

What Iamb puts on the clipboard is marked so that clipboard history and cloud clipboard services
skip it. An app can ignore a marker. On macOS the content is also kept for this device only, so the
system does not offer it to your other devices.

## What leaves your computer

Nothing you type leaves your computer.

The desktop app is designed to use the network only for update checks and, if you turn it on, for
sending crash reports. This version does neither: it has no update check and no code that sends
crash reports. Sync between your computers is not part of this version either. See
[Sync](sync.md).

## Crash reports

When Iamb's own programs fail, they write a small report to the `logs` folder on this computer. It
holds the process name, version, system, time, process id, thread name and the place in the code
where it failed. It never holds the panic message, a backtrace, a memory dump or what you typed.
Iamb keeps the last 10 reports for each of its programs.

The **Send crash reports** switch in [Settings](settings.md#privacy) is off by default. In this
version the switch is only saved. Nothing reads the local reports or sends them anywhere, whether the
switch is on or off.

Iamb also tells the operating system not to collect memory dumps of its programs, where the system
allows it. On macOS the system still writes its own crash report (in `~/Library/Logs/DiagnosticReports`);
it holds backtraces and registers, not heap memory. Iamb's web view, which draws its windows, is
started by the system and is not covered by this. It never receives the keys you type. What you fill
into a form reaches it while the form is shown.

## What Iamb cannot protect you from

Iamb protects what you type from its own bugs, from logs, and from other users of the computer. It
cannot protect you from everything:

- **Malware already running as your user.** It can watch the keyboard directly, without Iamb.
- **An administrator who records full memory dumps** of all programs. A dump taken from outside Iamb
  can contain typed characters. Iamb cannot prevent this.
- **Apps that do not say what they show.** Iamb cannot always tell that a field is a password field.
  A password prompt in a terminal is not detected on Windows, and on macOS only while the terminal
  has Secure Keyboard Entry on. See [Exclusions](exclusions.md#apps-that-iamb-cannot-tell-apart).
- **Snippets that type commands.** A snippet can press keys, wait and read the clipboard. Be careful
  about what you put in a snippet, especially one that ends with Enter in a terminal or chat.

## Questions

If you have a question about privacy that this page does not answer, use the project's repository
to ask. Do not include what you typed.
