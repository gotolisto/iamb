# Accessibility

Iamb is built to be used from the keyboard, and to follow your system's appearance settings. This
page says what Iamb does, what is checked, and what is not checked yet.

## What you can do from the keyboard

- The library window is designed so that every task has a keyboard path. See
  [Keyboard](keyboard.md). An automated keyboard-only run checks the main screens.
- Shortcuts are shown on the controls they work with, and are marked for assistive technology.
- Tab moves between controls. It never types a tab character in the editor, so you cannot get stuck
  there.
- Esc closes a dialog, a panel or a menu, and returns the focus to where it was.
- Iamb's own panels, the fill-in form and the search window, give the keyboard back to the app you
  were typing in when they close.
- You can zoom the library window in and out with Ctrl+Plus and Ctrl+Minus (⌘ on macOS), and reset it
  with Ctrl+0.

## Appearance

- **Appearance** follows the system's light or dark setting by default. You can choose Light or Dark
  in [Settings](settings.md#general).
- Focus rings, selected rows and switches are drawn so they remain visible in high-contrast modes.
- Motion is reduced when the system asks for reduced motion. The settings switches do not slide.

## Panels and focus

The fill-in form and the search window appear over the app you are using. They take the keyboard
while they are open, and give it back to that app when they close. They appear without animation.

## What is checked

Iamb is tested automatically on every change:

- The screens are checked against common accessibility rules, including colour contrast and labels.
- A keyboard run goes through the main screens with keys only, and checks that the focus is visible.

## What is not checked yet

These checks are not finished. Do not assume a screen works with a screen reader until it has been
tested:

- Testing with screen readers on macOS and Windows has not been completed.
- The focus hand-back from the form and the search window has not been tested with a screen reader.
- Increased contrast on macOS, and zoom to 200% in every screen, have not been tested by hand.

<!-- TODO: link the accessibility test results here once the manual runs are recorded. -->

## Report a problem

If something cannot be reached from the keyboard, or is hard to read or announce, tell the project
through its repository. Say which screen you were on, what you pressed, and what happened.
