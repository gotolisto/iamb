# Exclusions

An excluded app is one where Iamb does not expand snippets, and does not keep the keys you type.
Add the apps where you type passwords or secrets.

## Add an app

1. Open **Settings**, then **Apps**.
2. Under **Never expand in these apps**, fill in the app's identifier:
   - **macOS**: the bundle identifier, for example `com.example.bank`.
   - **Windows**: the program file name, for example `mybank.exe`.
3. Optionally, add a name. It is shown in the list.
4. Choose **Add app**.

If you leave the identifier empty, Iamb asks you to write it.

Quicker, from the menu: while you are in the app, open the Iamb menu and choose **Never expand in**
*app*. The item names the app by its identifier (the last part of a macOS bundle identifier, or the
Windows program name without `.exe`). For an app that is already excluded, the same item says
**Expand in** *app* **again** and removes it from the list.

## Remove an app

In the same list, choose **Remove** next to the app. Or, while you are in the app, choose
**Expand in** *app* **again** in the Iamb menu.

## What an excluded app means

- Iamb does not expand snippets in the app.
- Iamb does not match or keep the keys you type there. Each key passes through memory for an instant
  and is dropped without being stored.
- A search pick or a fill-in form cannot insert into the app. Iamb shows a short notice and types
  nothing.

## Apps that Iamb cannot tell apart

Some apps do not say whether they are showing a password. Iamb treats those as ordinary text fields.
If an app like that needs protecting, add it to the list. The same applies to:

- **Terminals**: on Windows, Iamb cannot see that a terminal is asking for a password (for example
  for `ssh`, `sudo` or `runas`). On macOS, Iamb can see it only while the terminal has Secure
  Keyboard Entry on.
- **Remote desktop and virtual machine windows**, and some games. Their windows may not describe
  their fields.

Use the exclusion list, or the pause, for those apps. See [Troubleshooting](troubleshooting.md).

## Password fields

Iamb always stays out of password fields that it can recognise, whether or not the app is on the
list. Nothing expands there.

## Pause everywhere

To stop Iamb in every app at once, use the menu's **Pause Iamb**, turn off
**Expand snippets as I type** in [Settings](settings.md#general), or choose **Pause Iamb** from the
status line at the bottom of the library window's sidebar. Search still works while paused.

A pause lasts until you resume. Iamb keeps it when it quits, when you log out and when the
computer restarts, so it is still paused when it starts again. The menu then says **Iamb is still
paused** ("Paused since before Iamb last started"), and so do the library window's status line and
[Why didn't it expand?](troubleshooting.md#nothing-expands-at-all). If you hid the menu icon, look
there.
