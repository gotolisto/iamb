# Formatted snippets

A **formatted** snippet can contain bold and italic text, links, lists and images. When it expands,
Iamb pastes the formatted text into the app. A **plain text** snippet types text only.

Formatted snippets use the same [templates](templates.md) as plain ones. Their fields, dates and
keys work the same way.

## Make a formatted snippet

1. Open or create a snippet in the library.
2. Set **Kind** to **Formatted**.
3. Write the text. Use the toolbar, or the keys below, for formatting.
4. Save with **Ctrl+S** (**⌘S** on macOS).

The preview shows the formatted result as it will be pasted.

Switching a formatted snippet back to plain text asks first, because the formatting would be lost.
The text stays as it is. Marks such as `**` are then typed as written, and images become their
names.

## Formatting

| What | Toolbar | Keys |
| --- | --- | --- |
| Bold | **B** | **Ctrl+B** (**⌘B** on macOS) |
| Italic | *I* | **Ctrl+I** (**⌘I** on macOS) |
| Link | Link | **Ctrl+K** (**⌘K** on macOS) |
| List | List | none |
| Image | Image | none |

## Links

Links may use only three kinds of address: `http`, `https` and `mailto`. Other addresses are not
turned into links.

Clicking a link in the preview opens it in your default browser or mail app.

## Images

Add an image with the **Image** button, by dropping it onto the editor, or by pasting it.

- Accepted types: PNG, JPEG, GIF and WebP.
- Largest size: 5 MB per image.

Images are kept with the library on this computer. They are copied into a formatted snippet when it
expands.

## Where formatting may not survive

Formatting reaches an app through the clipboard, as formatted text and as plain text. Each app
decides how much of it to keep. Some apps keep bold, links and images. Others keep only the text.
When an app does not accept formatted text, Iamb types the plain-text version.

<!-- TODO: publish the paste-fidelity results for the apps we test, once the runs are done. -->

## What does not apply

- **Cursor position** is not offered in a formatted snippet. The caret cannot be moved reliably
  through pasted formatting.
- **Capital letters that follow your typing** (the adaptive mode) do not change pasted content.
  Text that a field fills in is never changed either.
- A line break inside a paragraph stays a line break. A signature or an address keeps its lines.

## Keys and pauses inside a formatted snippet

A key press or a wait splits the snippet. Each part between two of them is pasted as its own piece
of formatted text, in order.

## Related

- [Templates](templates.md): the macros you can use in any snippet.
- [Snippets](snippets.md): options for all snippets.
