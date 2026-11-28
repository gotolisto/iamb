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
//! # A control without a kind
//!
//! Before its tree exists an engine shows two things as focused: its window (a pane, or the
//! window the system makes up for a window that says nothing) and the root of the page (a
//! document). Those stay not believed, whatever else they say. A **custom** element, a control
//! the engine gives no standard kind, is believed only under every one of these conditions
//! ([`custom_candidate`], and `secure` for the last):
//!
//! - its framework is the browser engine's own (`FrameworkId` "Chrome", which the system's web
//!   view reports too): the case that was measured, a chat application's editable message box
//!   on the system's web view. Another engine family, and an element the system translated
//!   from the older accessibility interface (where a role it does not know becomes custom), stay
//!   not believed until there is data for them;
//! - the engine says it has the keyboard focus (`HasKeyboardFocus`);
//! - it does not name a window of its own: a `NativeWindowHandle` that is not 0, or one that
//!   could not be read, refuses it. This can only refuse. It is no evidence of a built page:
//!   a handle that is absent reads as 0, like a page node's, and the root of an unbuilt page
//!   gives the same 0 and the same focus;
//! - and it is **not the first look**: the first custom answer for a window, its focused
//!   control and the count of focus changes is `NotInspectable` and looked at again; the same
//!   answer at a look again of the same subject, with no focus change counted between, settles.
//!   A placeholder an engine shows while its page is being built is what the looks again are
//!   for: it is replaced by the field before the look after.
//!
//! None of these proves that the tree is built: the two facts tell a node from a window, the
//! framework narrows the rule to what was measured, and the second look gives a transient
//! answer the time to change. A password field reports `IsPassword`, and that is `Secure`
//! before any kind is looked at; a password box a page draws itself with an unusual role and
//! no `IsPassword` is not one, and is a listed limit (architecture §8, "Detection limits").

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

/// The UI framework an element says it comes from (`FrameworkId`), as one of the names the
/// system's frameworks use. Only for `--diagnose-focus`: the decision does not use it. The
/// name itself is not kept, so no text of the application's can be in it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Framework {
    /// A browser engine (`Chrome`).
    Chrome,
    /// The newer Windows toolkits (`XAML`, `DirectUI`).
    Xaml,
    /// System controls (`Win32`).
    Win32,
    /// `WPF`.
    Wpf,
    /// `WinForm`.
    WinForm,
    /// `Qt`.
    Qt,
    /// Another name, or none.
    Other,
}

impl Framework {
    /// The framework a `FrameworkId` names.
    pub(crate) fn named(name: &str) -> Self {
        match name {
            "Chrome" => Self::Chrome,
            "XAML" | "DirectUI" => Self::Xaml,
            "Win32" => Self::Win32,
            "WPF" => Self::Wpf,
            "WinForm" => Self::WinForm,
            "Qt" => Self::Qt,
            _ => Self::Other,
        }
    }
}

/// What the engine says about an element besides its kind: whether it comes from a built
/// tree (module docs, "A control without a kind").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Detail {
    /// `HasKeyboardFocus`.
    pub(crate) has_focus: bool,
    /// `NativeWindowHandle` is not 0, or could not be read: the element may be a window's
    /// own. Can only refuse (module docs).
    pub(crate) window_root: bool,
    /// `FrameworkId`.
    pub(crate) framework: Framework,
}

impl Detail {
    /// Nothing said: no focus, no window, no framework.
    #[cfg(test)]
    pub(crate) const NONE: Self = Self {
        has_focus: false,
        window_root: false,
        framework: Framework::Other,
    };
}

/// What UI Automation said about the focused element. By construction this holds no name,
/// no value and no text: facts about the element's kind, its state and the process that
/// provides it, nothing of its content.
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
        /// Its focus, whether it is a window's own, its framework.
        detail: Detail,
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

/// True for a focused custom element of the browser engine's own framework that names no
/// window: one that may be believed at a second look (module docs, "A control without a
/// kind").
pub(crate) const fn custom_candidate(kind: ControlKind, observation: Observation) -> bool {
    match observation {
        Observation::Element {
            password: false,
            control_type,
            detail,
            ..
        } => {
            matches!(kind, ControlKind::WebEngine)
                && control_type == control_type::CUSTOM
                && matches!(detail.framework, Framework::Chrome)
                && detail.has_focus
                && !detail.window_root
        }
        _ => false,
    }
}

/// What `observation`, made in a control of kind `kind`, says about the field. `confirmed`:
/// the observation is a [`custom_candidate`] seen at a second look of the same subject (the
/// caller keeps that), which believes it.
pub(crate) const fn classify(
    kind: ControlKind,
    observation: Observation,
    confirmed: bool,
) -> Judgement {
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
            let believed_custom = confirmed && custom_candidate(kind, observation);
            if matches!(kind, ControlKind::WebEngine) && is_shell(control_type) && !believed_custom
            {
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
        with(password, control_type, Detail::NONE)
    }

    const fn with(password: bool, control_type: i32, detail: Detail) -> Observation {
        Observation::Element {
            password,
            control_type,
            process: 40,
            detail,
        }
    }

    #[test]
    fn a_custom_element_of_the_engine_is_believed_only_at_a_second_look() {
        // The real-machine data of a chat application on the system's web view: an editable
        // region with a kind the engine reports as custom, focused, inside the page.
        let node = Detail {
            has_focus: true,
            window_root: false,
            framework: Framework::Chrome,
        };
        let custom = with(false, control_type::CUSTOM, node);
        assert!(custom_candidate(ControlKind::WebEngine, custom));
        assert_eq!(
            classify(ControlKind::WebEngine, custom, true),
            Judgement::settled(FieldSecurity::Normal)
        );
        // The first look: looked at again.
        assert_eq!(
            classify(ControlKind::WebEngine, custom, false),
            Judgement::passing(UnknownReason::NotInspectable)
        );
        // And a password field is secure whatever its kind, detail and look.
        for confirmed in [false, true] {
            assert_eq!(
                classify(
                    ControlKind::WebEngine,
                    with(true, control_type::CUSTOM, node),
                    confirmed
                ),
                Judgement::settled(FieldSecurity::Secure)
            );
        }
        // The cached-probe review's M1: another framework (another engine family, the system's
        // translation of the older interface), no focus, a window's own: never, at any look.
        for detail in [
            Detail {
                framework: Framework::Other,
                ..node
            },
            Detail {
                framework: Framework::Win32,
                ..node
            },
            Detail {
                has_focus: false,
                ..node
            },
            Detail {
                window_root: true,
                ..node
            },
            Detail::NONE,
        ] {
            let element = with(false, control_type::CUSTOM, detail);
            assert!(!custom_candidate(ControlKind::WebEngine, element));
            assert_eq!(
                classify(ControlKind::WebEngine, element, true),
                Judgement::passing(UnknownReason::NotInspectable),
                "{detail:?}"
            );
        }
        // The window and the page root are never believed, whatever they say.
        for control_type in [
            control_type::PANE,
            control_type::WINDOW,
            control_type::DOCUMENT,
        ] {
            let element = with(false, control_type, node);
            assert!(!custom_candidate(ControlKind::WebEngine, element));
            assert_eq!(
                classify(ControlKind::WebEngine, element, true),
                Judgement::passing(UnknownReason::NotInspectable),
                "{control_type}"
            );
        }
    }

    #[test]
    fn framework_names_are_kept_as_kinds_only() {
        assert_eq!(Framework::named("Chrome"), Framework::Chrome);
        assert_eq!(Framework::named("XAML"), Framework::Xaml);
        assert_eq!(Framework::named("DirectUI"), Framework::Xaml);
        assert_eq!(Framework::named("Qt"), Framework::Qt);
        assert_eq!(
            Framework::named("anything a provider says"),
            Framework::Other
        );
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
                    classify(kind, element(true, control_type), false),
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
                classify(kind, element(false, control_type), false),
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
                classify(ControlKind::WebEngine, element(false, control_type), false),
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
                let judgement = classify(kind, observation, false);
                assert_eq!(
                    judgement.field,
                    FieldSecurity::Unknown(UnknownReason::NotInspectable)
                );
                assert!(judgement.look_again);
            }
        }
    }
}
