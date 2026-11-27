# Snippets

A snippet is text you save once and type with a short abbreviation. This page covers making,
changing and organising snippets.

## Make a snippet

1. Open the library window: choose **Open library** from the menu bar or notification area icon.
2. Press **Ctrl+N** (**⌘N** on macOS), or choose **New**.
3. Fill in the fields:
   - **Abbreviation**: what you type. Required.
   - **Label**: a name for lists and search. Optional.
   - **Content**: the text to insert. You can add [templates](templates.md) here.
   - **Folder** and **Tags**: where the snippet sits.
   - **Options**: when it expands and how capital letters are handled. See below.
4. Save with **Ctrl+S** (**⌘S** on macOS).

Until you save, nothing reaches the library. So a half-typed abbreviation never expands.

## Abbreviation rules

- Use at most 64 characters.
- Do not use spaces or line breaks.
- Each abbreviation belongs to one snippet. Letters are compared without regard to capitals, so
  `;sig` and `;SIG` cannot both be used.
- Start with a symbol such as `;`. Short abbreviations made only of letters can expand while you
  type ordinary words.
- If one abbreviation starts with another, the shorter one can expand first. For example, if `;rl`
  expands immediately, it fires before you can finish typing `;rlate`. Change one of the two, or
  let the shorter one wait for a space.

If an abbreviation is already in use, the editor says which snippet has it.

## Options

Open **Options** in the editor to change how a snippet expands.

**Expand**

- **After a space or punctuation** (the default for new snippets): expands when the word is
  finished. Enter and Tab do not count.
- **Immediately**: expands on the last character. Good for abbreviations that are never part of a
  word.

**Capital letters**

- **Exactly as written**: `;sig` types as written, and `;Sig` and `;SIG` do not match.
- **Ignore** (the default for new snippets): the abbreviation matches in any capitals.
- **Ignore, and follow mine**: `;sig` types as written, `;Sig` starts with a capital letter, and
  `;SIG` is all capitals. Text you fill in is never changed.

**Only at the start of a word**

- On by default when a snippet expands after a space or punctuation, and off for **Immediately**.
  The abbreviation must then follow a space, punctuation or the start of the text, so it does not
  expand inside another word.

The default for new snippets is set in [Settings](settings.md#typing).

## Turn a snippet on or off

Select the snippet and press **Space**, use the **On** switch in the editor, or choose **Turn on** or
**Turn off** from its menu. A snippet that is off does not expand. It stays in the library. Another
snippet can still include it.

## Duplicate a snippet

Select the snippet and press **Ctrl+D** (**⌘D** on macOS). Or choose **Duplicate** from its menu or
from **More actions** in the editor. **Copy content** in **More actions** copies the snippet's text.

## Delete a snippet

Select the snippet and press **Delete**. (On macOS, press **⌘⌫**.) Iamb does not ask first. A
message appears at the bottom of the window with **Undo**. It stays for 10 seconds.

## Folders

- Folders can be nested up to four levels deep.
- Press **Ctrl+Shift+N** (**⌘⇧N** on macOS) to make a new folder. To make a folder inside another,
  choose **New folder inside…** from the folder's menu.
- To move a snippet, change its **Folder** field in the editor.
- Open a folder's menu to change its settings, make a folder inside it, or delete it. **Folder
  settings…** has the name, the parent folder, an optional abbreviation prefix, and what new snippets
  in the folder start with.

<!-- TODO: drag-and-drop reordering of folders and snippets is not in the desktop app yet. -->

Deleting a folder asks first. It also deletes the snippets inside it. Other snippets that include
one of them stop working. See [Library health](#library-health).

## Tags

- Tags are labels you can put on snippets. Each tag has a colour. You can choose from red, orange,
  yellow, green, teal, blue, purple and grey.
- Select a tag in the sidebar to see every snippet that has it, across folders.
- Rename a tag, change its colour, or delete it from its menu.

## Find and sort

- Type in the **Filter snippets** field, or press **Ctrl+F** (**⌘F** on macOS).
- Sort the list by **Label**, **Recently changed**, or **Most used on this computer**. The last
  option appears only when usage counts are available.

Usage counts are kept on this computer only. See [Privacy](privacy.md).

## Select several snippets

Press **Ctrl+A** (**⌘A** on macOS) to select every snippet in the list. Hold **Shift** and press
an arrow key to extend the selection. With several snippets selected, the list's menu offers
**Turn on** or **Turn off** (one item, which turns them all off when any is on) and **Delete**. The
editor pane shows the count with **Turn off** and **Delete** buttons. Duplicate works on one snippet
at a time.

## Library health

**Library health** is in the sidebar. It lists snippets that will not work as they should:

- **Will not expand: error in the snippet.** The template has an error, or it uses a snippet that
  does not exist.
- **Will not expand: abbreviation taken.** Another snippet already has the abbreviation.
- **Never reached.** A shorter abbreviation fires first, so this one cannot be typed.
- **Not used on this computer for 90 days.** Shown so you can turn it off or delete it. Counted on
  this computer only.

Each entry says what is wrong and offers **Open snippet**, **Turn off** or **Delete**.

## Next steps

- [Templates](templates.md): dates, fill-in fields, and other parts of a snippet.
- [Formatted snippets](formatted-snippets.md): bold text, links and images.
- [Keyboard](keyboard.md): all the shortcuts.
