# Iamb

Iamb turns short abbreviations into longer text. Type `;em` and a space in most apps, and Iamb
replaces it with your email address. It stays out of password fields and apps you exclude.

Iamb is free and open source. It is licensed under the GNU Affero General Public License,
version 3 or later (AGPL-3.0-or-later).

> Status: pre-alpha. The desktop app is being built. Some features described here are not in the
> app yet. Each page says so where it applies. Check [coverage](coverage.md) for the full list.

## What Iamb does

- You save text you type often as a **snippet**, with a short **abbreviation**.
- You type the abbreviation in any app. Iamb replaces it with the snippet.
- A snippet can also ask you for a few values first, insert the date or time, or paste formatted
  text with images.

Read more:

- [Get started](getting-started.md): install, first launch and your first expansion.
- [Snippets](snippets.md): create, organise and turn snippets on or off.
- [Templates](templates.md): insert dates, fill-in fields and other parts.
- [Search](search.md): find a snippet without its abbreviation.
- [Settings](settings.md): every setting, with its default.
- [Command line](cli.md): manage the library from a terminal.

## Who it is for

- People who type the same text again and again: addresses, replies, order numbers, sign-offs.
- People who write the same kind of document or message often and want the boilerplate typed for
  them.
- People who want their text tools to stay on their own computer.

## Where your data lives

Iamb keeps your library on your computer, in a file that other users of the computer cannot read.
The file is not encrypted. The keys you type are held in memory for a moment to spot abbreviations.
Iamb never saves them, logs them or sends them. This version sends nothing over the network. For
the details, and for what Iamb cannot protect you from, read [Privacy](privacy.md).

## Platforms

- **macOS and Windows**: the desktop app. Iamb runs in the background, and you open the library
  from the menu bar (macOS) or the notification area (Windows). There is no desktop app for Linux.
- **Command line**: the `iamb` tool manages the same library on macOS, Windows and Linux. See
  [Command line](cli.md).

## Accessibility

Iamb is built to work from the keyboard, and the shortcuts are shown on the controls. Read
[Accessibility](accessibility.md) for what is tested and what is not yet. Testing with screen
readers is not finished.

## Sync and accounts

Sync between computers and accounts are in progress. They are not part of this version. See
[Sync](sync.md) and [Accounts](accounts.md).
