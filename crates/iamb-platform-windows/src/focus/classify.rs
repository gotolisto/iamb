//! What the answers about the focused control mean. No system call.
//!
//! Two sources say whether a field is a password field:
//!
//! 1. **The control's window class and style.** A control of the system's edit classes with
//!    the password style is a password field, and one without it is not. This needs no call
//!    into the application, so it cannot wait for one.
//! 2. **UI Automation**, for everything else: the focused element's `IsPassword` property.
//!    Browsers and applications built on a browser engine draw their fields themselves, and
//!    this is the only way to see them. Toolkits that speak the older accessibility interface
//!    are covered too: the system translates its "protected" state into the same property.
//!
//! # Fail closed, also for an answer that looks normal
//!
//! "Not a password field" is only worth something when the element asked is the field. A
//! browser engine builds its accessibility tree when somebody first asks for it, and until
//! the tree is there the focused element is the window or the empty page: not a password
//! field, whatever the user is typing into. So in a window of a browser engine, an element
//! of a kind the engine shows before its tree exists ([`is_shell`]) is
//! [`NotInspectable`](UnknownReason::NotInspectable), and the caller is told to look again
//! shortly (`rules::Settle`). An element of any other kind comes out of the built tree, and
//! its answer is taken.
//!
//! Everywhere else the element is believed. An application that draws its own fields and
//! tells the system nothing about them (see the limits in the module docs of `focus`) cannot
//! be told apart from an editor that does the same, and refusing all of them would stop Iamb
//! in most terminals and several editors.

use iamb_platform::{FieldSecurity, UnknownReason};

/// The password style of the system's edit controls (`ES_PASSWORD`).
pub(crate) const ES_PASSWORD: u32 = 0x0020;

/// UI Automation control types (`UIA_*ControlTypeId`), as far as the rules name them.
pub(crate) mod control_type {
    /// A custom control.
    pub(crate) const CUSTOM: i32 = 50025;
    /// A document: in a browser, the page.
    pub(crate) const DOCUMENT: i32 = 50030;
    /// A window.
    pub(crate) const WINDOW: i32 = 50032;
    /// A pane: what the system reports for a window that says nothing about itself.
    pub(crate) const PANE: i32 = 50033;
    /// A text field.
    #[cfg(test)]
    pub(crate) const EDIT: i32 = 50004;
    /// A group: what a browser engine reports for an editable region without a role.
    #[cfg(test)]
    pub(crate) const GROUP: i32 = 50026;
}

/// What kind of control has the keyboard focus, as far as its window class tells.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ControlKind {
    /// One of the system's edit controls with the password style.
    PasswordEdit,
    /// One of the system's edit controls without it.
    PlainEdit,
    /// A window of a browser engine, which draws its own fields.
    WebEngine,
    /// Anything else.
    Other,
}

/// What UI Automation said about the focused element. By construction this holds no name,
/// no value and no text: two facts about the element's kind and the process that provides it,
/// nothing of its content.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Observation {
    /// There is a focused element.
    Element {
        /// Its `IsPassword` property.
        password: bool,
        /// Its control type.
        control_type: i32,
        /// The process that provides it (`ProcessId`): which window the answer is about, for a
        /// late answer (`inspect`).
        process: u32,
    },
    /// The system named no focused element.
    NoElement,
    /// A call failed.
    Failed,
}

/// The answer to `secure_state`, and whether asking again shortly may give a better one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Judgement {
    /// What the field is.
    pub(crate) field: FieldSecurity,
    /// The answer is `Unknown` for a reason that may pass (an application that was busy, a
    /// browser engine that is still building its tree).
    pub(crate) look_again: bool,
}

impl Judgement {
    /// An answer that asking again would not change.
    pub(crate) const fn settled(field: FieldSecurity) -> Self {
        Self {
            field,
            look_again: false,
        }
    }

    /// `Unknown`, for a reason that may pass.
    pub(crate) const fn passing(reason: UnknownReason) -> Self {
        Self {
            field: FieldSecurity::Unknown(reason),
            look_again: true,
        }
    }
}

/// The kind of the control with window class `class` and window style `style`.
///
/// `class` is the class the control is built on (`RealGetWindowClassW`), so a toolkit's own
/// name for a system edit control still reads `Edit`.
pub(crate) fn control_kind(class: &str, style: u32) -> ControlKind {
    // `Edit`, and the rich edit family: `RichEdit20W`, `RICHEDIT50W`, `RichEditD2DPT`, ...
    // All of them use the same style bit for a password field.
    let rich = class
        .get(..8)
        .is_some_and(|start| start.eq_ignore_ascii_case("RichEdit"));
    if class.eq_ignore_ascii_case("Edit") || rich {
        return if style & ES_PASSWORD == 0 {
            ControlKind::PlainEdit
        } else {
            ControlKind::PasswordEdit
        };
    }
    // The windows of the two browser engines, and of everything built on them (browsers,
    // applications shipped with an engine, web views inside other applications).
    if class.starts_with("Chrome_") || class.starts_with("Mozilla") {
        return ControlKind::WebEngine;
    }
    ControlKind::Other
}

/// True for the control types a browser engine's window shows as focused while the page's
/// own elements are not there to be asked: the window itself, the pane the system makes up
/// for it, the page as a whole, a control without a kind.
pub(crate) const fn is_shell(control_type: i32) -> bool {
    matches!(
        control_type,
        control_type::PANE | control_type::WINDOW | control_type::DOCUMENT | control_type::CUSTOM
    )
}

/// What `observation`, made in a control of kind `kind`, says about the field.
pub(crate) const fn classify(kind: ControlKind, observation: Observation) -> Judgement {
    match observation {
        Observation::Failed | Observation::NoElement => {
            Judgement::passing(UnknownReason::NotInspectable)
        }
        Observation::Element { password: true, .. } => Judgement::settled(FieldSecurity::Secure),
        Observation::Element {
            password: false,
            control_type,
            ..
        } => {
            if matches!(kind, ControlKind::WebEngine) && is_shell(control_type) {
                Judgement::passing(UnknownReason::NotInspectable)
            } else {
                Judgement::settled(FieldSecurity::Normal)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const fn element(password: bool, control_type: i32) -> Observation {
        Observation::Element {
            password,
            control_type,
            process: 40,
        }
    }

    #[test]
    fn system_edit_controls_are_told_by_their_style() {
        for class in [
            "Edit",
            "EDIT",
            "edit",
            "RichEdit",
            "RichEdit20W",
            "RICHEDIT50W",
            "RichEditD2DPT",
            "RICHEDIT60W",
        ] {
            assert_eq!(control_kind(class, 0), ControlKind::PlainEdit, "{class}");
            // The style word holds other bits too.
            assert_eq!(
                control_kind(class, 0x5001_0080),
                ControlKind::PlainEdit,
                "{class}"
            );
            assert_eq!(
                control_kind(class, ES_PASSWORD),
                ControlKind::PasswordEdit,
                "{class}"
            );
            assert_eq!(
                control_kind(class, 0x5001_00A0),
                ControlKind::PasswordEdit,
                "{class}"
            );
        }
    }

    #[test]
    fn the_password_bit_means_nothing_for_other_classes() {
        // For a button the same bit is part of its kind; for a list box it is a tab style.
        for class in [
            "Button",
            "ListBox",
            "Static",
            "Editor",
            "EditBox",
            "MyEdit",
            "RichEd",
            "Scintilla",
            "ConsoleWindowClass",
            "",
            // Not ASCII: must not be cut in the middle of a character.
            "Ünïcödé-Class",
        ] {
            assert_eq!(
                control_kind(class, ES_PASSWORD),
                ControlKind::Other,
                "{class}"
            );
            assert_eq!(control_kind(class, 0), ControlKind::Other, "{class}");
        }
    }

    #[test]
    fn browser_engine_windows_are_recognised() {
        for class in [
            "Chrome_WidgetWin_1",
            "Chrome_WidgetWin_0",
            "Chrome_RenderWidgetHostHWND",
            "MozillaWindowClass",
            "MozillaDialogClass",
        ] {
            assert_eq!(control_kind(class, 0), ControlKind::WebEngine, "{class}");
            // The style bit of an edit control means nothing here.
            assert_eq!(
                control_kind(class, ES_PASSWORD),
                ControlKind::WebEngine,
                "{class}"
            );
        }
        assert_eq!(control_kind("Chrome", 0), ControlKind::Other);
        assert_eq!(control_kind("chrome_widget", 0), ControlKind::Other);
    }

    #[test]
    fn a_password_element_is_secure_in_any_window() {
        for kind in [ControlKind::WebEngine, ControlKind::Other] {
            for control_type in [
                control_type::EDIT,
                control_type::PANE,
                control_type::DOCUMENT,
                control_type::CUSTOM,
                0,
            ] {
                assert_eq!(
                    classify(kind, element(true, control_type)),
                    Judgement::settled(FieldSecurity::Secure)
                );
            }
        }
    }

    #[test]
    fn an_ordinary_element_is_normal() {
        for (kind, control_type) in [
            (ControlKind::Other, control_type::EDIT),
            // An editor that draws itself and says nothing: believed (see the module docs).
            (ControlKind::Other, control_type::PANE),
            (ControlKind::Other, control_type::WINDOW),
            (ControlKind::Other, control_type::DOCUMENT),
            (ControlKind::Other, control_type::CUSTOM),
            // In a browser: elements that only exist in the built tree.
            (ControlKind::WebEngine, control_type::EDIT),
            (ControlKind::WebEngine, control_type::GROUP),
            (ControlKind::WebEngine, 50003), // a combo box: a search field with suggestions
        ] {
            assert_eq!(
                classify(kind, element(false, control_type)),
                Judgement::settled(FieldSecurity::Normal),
                "{kind:?} {control_type}"
            );
        }
    }

    #[test]
    fn a_browser_engine_that_shows_no_field_is_not_believed() {
        for control_type in [
            control_type::PANE,
            control_type::WINDOW,
            control_type::DOCUMENT,
            control_type::CUSTOM,
        ] {
            assert!(is_shell(control_type));
            assert_eq!(
                classify(ControlKind::WebEngine, element(false, control_type)),
                Judgement::passing(UnknownReason::NotInspectable),
                "{control_type}"
            );
        }
        assert!(!is_shell(control_type::EDIT));
        assert!(!is_shell(control_type::GROUP));
    }

    #[test]
    fn no_answer_is_never_normal() {
        for kind in [
            ControlKind::WebEngine,
            ControlKind::Other,
            ControlKind::PlainEdit,
            ControlKind::PasswordEdit,
        ] {
            for observation in [Observation::Failed, Observation::NoElement] {
                let judgement = classify(kind, observation);
                assert_eq!(
                    judgement.field,
                    FieldSecurity::Unknown(UnknownReason::NotInspectable)
                );
                assert!(judgement.look_again);
            }
        }
    }
}
