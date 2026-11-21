# Get started

This page takes you from install to your first expansion. It takes about two minutes.

## Install

Download the installer for your system from the project's releases page.
<!-- TODO: add the releases page link before publishing. -->

### macOS

Iamb needs macOS 13 or later. One disk image works on Apple silicon and Intel Macs.

1. Download the disk image (`Iamb_<version>_universal.dmg`) from the releases page.
2. Open the disk image and drag **Iamb** to **Applications**.
3. Open **Iamb** from Applications.

A build that is not signed and notarized shows a warning that Apple could not verify the app. To open
it anyway, go to System Settings, then Privacy & Security, choose **Open Anyway** and confirm with
your password. On an unsigned build, you also have to grant the permissions below again after you
install a new version.

To uninstall, move **Iamb** to the Bin. Your library stays in
`~/Library/Application Support/Iamb/`.

<!-- TODO: say which release is signed once the signing secrets exist (docs/release.md). Until then every build is unsigned. -->

### Windows

Iamb needs Windows 10 or 11 (64-bit).

1. Download the installer (`Iamb_<version>_x64-setup.exe`) from the releases page.
2. Run it. It installs for your user only and does not ask for administrator rights.

An unsigned installer shows **Windows protected your PC**. Choose **More info**, then **Run anyway**.
Iamb installs to `%LOCALAPPDATA%\Iamb` and adds a Start menu entry.

To uninstall, use Settings, then Apps. Your library in `%APPDATA%\Iamb\` stays unless you tick
**Delete the application data**.

### Linux

There is no desktop app for Linux. The project builds a Linux package for development only. The
command-line tool works on Linux. See [Command line](cli.md).

## First launch: the welcome window

The first time Iamb starts, a short setup window opens. Choose **Get started** to begin. You can close
it at any step, and Iamb keeps running. Until setup is finished, the menu offers **Set up Iamb…**.

If the background service is not running, the first step says so and offers **Start it again**.
In this version that button cannot restart the service. Quit Iamb from its menu and open it again.
<!-- TODO: confirm what starts the background service when Iamb is opened. The restart button is not connected to anything yet (onboarding.rs, NoSupervisor). -->

## Permissions

Iamb needs permissions only on macOS. Windows does not ask for any.

### macOS

Iamb asks for two permissions. Each one has a row in the setup window.

| Permission | Why Iamb needs it |
| --- | --- |
| **Input Monitoring** | Lets Iamb notice when you type an abbreviation. |
| **Accessibility** | Lets Iamb type your snippet for you, and recognise password fields so it can stay out of them. |

To turn them on:

1. Press **Open settings** on the row.
2. In System Settings, turn on the switch next to **Iamb Service**. This is the background helper
   that carries Iamb's permissions. The setup window may just say **Iamb**. You may need to enter your password or
   use your fingerprint.
3. Go back to Iamb. The row says **Granted** when the permission is on. The window checks by itself.
4. Do the same for the second permission.

When both rows say **Granted**, the window says **Both are on.** Choose **Continue**. You can change the
permissions later in System Settings.

If a permission is missing, the menu shows **Fix permissions…**. Choose it to return to these steps.

### Windows

Windows does not ask you to grant anything, so the permissions step is left out. Setup on Windows
is Welcome, First snippet, Try it, and Done. The first snippet step appears only when your library
has no snippets yet.

## Start at login

The last step has a box, **Start Iamb when I log in**. Iamb turns it on as soon as you reach that
step, so it is already on. Clear the box if you do not want Iamb to start when you sign in. On macOS
the system may ask you to approve this once, in System Settings under Login Items (the entry is
named **Iamb Service**). If Iamb cannot
change it, you can start Iamb yourself from the Applications folder (macOS) or the Start menu
(Windows).

This box is only offered in setup. See [Settings](settings.md#settings-not-in-this-version).

What starts at login is Iamb's background service, the part that expands snippets. In this version it
does not start the menu icon or the windows.
<!-- TODO: confirm. The supervisor does not start the UI host yet (supervisor.rs, "Hook points"). The Settings window has no start-at-login switch. -->

## Your first snippet

1. In the first snippet step, the **When I type** box has `;em` filled in. You can change it. Use
   a symbol such as `;` at the start: short words made only of letters can expand in the middle of
   ordinary typing.
2. In the **Iamb types** box, type the text you want Iamb to insert, for example your email address.
3. Choose **Save and try it**. The snippet is saved.

Read [Snippets](snippets.md) for the rules on abbreviations.

## Your first expansion

1. In the **Try it here** field, type `;em`.
2. Type a **space**.
3. Iamb replaces `;em` with your text. The window then says **That is all there is to it.**

Iamb expands a snippet after a space or punctuation. Pressing **Enter** or **Tab** does not expand a
snippet. This is on purpose, because those keys can send a message or move to the next field before
Iamb can type. See [Troubleshooting](troubleshooting.md#enter-and-tab-do-not-expand).

## Find Iamb after setup

Iamb keeps running after you close the setup window. On the last step you can choose **Open my library**
or **Close**.

- **macOS**: look for the Iamb icon in the menu bar. Iamb has no Dock icon.
- **Windows**: look for the Iamb icon in the notification area.
- Choose **Open library** from the icon's menu to open the library window.

You can hide the icon in [Settings](settings.md#general). Iamb keeps working without it. With the icon
hidden, press the [search shortcut](search.md), then **Ctrl+E** (**⌘E** on macOS) to open a snippet in
the library window. Opening the Iamb app a second time does not open the library window.
<!-- TODO: confirm with the design how to reach the library when the icon is hidden. The spec says everything in the menu stays reachable from the library window. -->

## The menu

Click the Iamb icon to open its menu. It has these items:

- Two status lines at the top. The first says how Iamb is: **Iamb is on** (with the number of
  snippets under it), **Iamb is paused** ("Typed abbreviations do not expand. Search still
  works."), **Iamb cannot expand yet** (for example "2 permissions are missing") or **Iamb is not
  running** ("Its background service stopped."). While Iamb starts, it says **Connecting…**.
- **Set up Iamb…**, while setup is not finished. **Fix permissions…**, when a permission is missing
  on macOS.
- **Search snippets…**, which opens [search](search.md). The search shortcut is shown beside it.
- **Fill in the last form again…**, which reopens the last [fill-in form](forms.md#your-last-answers).
- **New snippet…**, which opens the library window with a new, empty snippet.
- **Open library**.
- **Pause Iamb**, a switch. See [Settings](settings.md#general).
- **Never expand in** *app*, for the app you were using when you opened the menu. If that app is
  already excluded, the item says **Expand in** *app* **again**. The item is not shown for Iamb's
  own windows, or when Iamb cannot tell which app was in front. See [Exclusions](exclusions.md).
- **Why didn't it expand?**, which opens that view in the library window. See
  [Troubleshooting](troubleshooting.md).
- **Settings…** (⌘, on macOS, Ctrl+, on Windows), which opens [Settings](settings.md) in the
  library window.
- **Quit Iamb** on macOS, **Exit Iamb** on Windows. This closes the menu icon and the windows. If a
  snippet has unsaved edits, Iamb asks first. It does not ask whether you want expansion to stop.
  Whether the background service keeps expanding after you quit is not confirmed for this version.

The icon itself shows three states: the plain mark when Iamb is on, a fainter mark when it is
paused, and the mark with a dot when Iamb needs you (it is not running, or a permission is
missing). It does not change with the app you are using.

<!-- TODO: Quit calls app.exit(0) in the UI host only (commands.rs, app_quit). The UX spec says Quit asks first and stops the service too. Neither is built. -->

## Next steps

- [Snippets](snippets.md): add more snippets and organise them into folders.
- [Templates](templates.md): insert the date, ask for values, and more.
- [Keyboard](keyboard.md): the shortcuts.
- [Troubleshooting](troubleshooting.md): if something does not expand.
