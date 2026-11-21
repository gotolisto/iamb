# Fill-in forms

A snippet with fields asks you for values before it inserts. The form appears over the app you are
typing in. You fill it in and press **Enter**. Iamb then types the snippet into that app.

You make a snippet ask for values with a `{field}` macro. See [Templates](templates.md#fill-in-fields).

## Open a form

- Type the abbreviation of a snippet that has fields. The form opens.
- Or press the search shortcut, find the snippet and press Enter. See [Search](search.md).

The form opens with its first field selected.

## Fill it in

- Type in each field. Press **Tab** for the next field and **Shift+Tab** for the previous one.
- Use the **arrow keys** to choose an option in a choice field. Or type the first letters of the
  option.
- Press **Space** to check or uncheck a toggle.
- In a several-line field, Enter starts a new line. Press **Ctrl+Enter** (**⌘⏎** on macOS) to
  insert from there. In every other field, Enter inserts.
- No field is required. A blank field inserts nothing.

## Insert or cancel

- **Enter** inserts. The form closes, Iamb gives the keyboard back to the app, and then types the
  snippet.
- **Esc** cancels. The form closes, and nothing is typed.
- **Ctrl+Backspace** (**⌘⌫** on macOS) starts with empty fields, when the form shows your last
  answers. See below.

Clicking outside the form does not close it. If you switch to another app, the form stays open
and comes back when you return to that app.

## Your last answers

Iamb remembers the last answers for each form for 30 minutes after you use it. To fill it in again:

- Open the search window and choose **Fill in … again**, or choose **Fill in the last form again…**
  from the menu. The form opens with your last answers.
- The form then says **Your last answers.** To start empty instead, press **Ctrl+Backspace**.

Answers are kept only in memory. They are not saved to disk, and they are cleared after 30 minutes
without use, when the screen locks, when you sign out, and when Iamb restarts.

## When Iamb cannot type

Iamb does not type into a field it cannot confirm. In these cases the form is not inserted:

- The field is a password field.
- The app is one you have excluded. See [Exclusions](exclusions.md).
- Iamb cannot confirm the window it would type into, for example a window it is not allowed to
  inspect.
- An input method is in the middle of composing text.

Iamb then shows a short notice, and your answers are kept for 30 minutes. Click where the text
should go, press the search shortcut and choose **Fill in … again**. Nothing is typed in the wrong
place.

If Iamb cannot give the keyboard back to the app, it shows **Iamb could not get back to the app.**
Your answers are kept.

## Field types

| Type | What you do |
| --- | --- |
| One line (default) | Type a line of text. |
| Several lines | Type text with line breaks. |
| Choice | Pick one of the options. |
| Toggle | Check or uncheck a box. Checked inserts its "on" text; unchecked inserts its "off" text. |

A form can have up to 100 fields. Fields with the same name are asked once.

## Related

- [Templates](templates.md): how to write fields, options and conditions.
- [Keyboard](keyboard.md#fill-in-form): the form's shortcuts in one list.
