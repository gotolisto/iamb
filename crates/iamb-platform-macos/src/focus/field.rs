//! What the Accessibility API's answers about the focused element mean. Plain Rust, built and
//! tested on every system.
//!
//! # The only questions asked
//!
//! [`Attribute`] lists every attribute this crate ever asks an element for: which element has
//! the focus, and that element's role and subrole. Roles and subroles are identifiers chosen
//! by the application's developer (`AXTextField`, `AXSecureTextField`), not text the user
//! typed. There is no variant for an element's value, title, description, placeholder or
//! selected text, and the function that reads an attribute takes this enum, so the code that
//! inspects the focus cannot read what a field contains (architecture §8).
//!
//! # Failing closed
//!
//! [`judge`] answers `Normal` only when the element's role was read and neither the role nor
//! the subrole names a secure field. Every failure is an `Unknown`.

use iamb_platform::{FieldSecurity, UnknownReason};

/// `kAXSecureTextFieldSubrole`: the subrole of a password field (`NSSecureTextField`, and
/// what browsers report for `<input type="password">`).
pub(crate) const SECURE_TEXT_FIELD: &str = "AXSecureTextField";

/// The `AXError` codes the decisions below depend on (`HIServices/AXError.h`). A macOS test
/// compares them with the system bindings.
pub(crate) mod code {
    /// `kAXErrorSuccess`.
    pub(crate) const SUCCESS: i32 = 0;
    /// `kAXErrorCannotComplete`: the application did not answer (also what a call returns
    /// when the messaging timeout ran out).
    pub(crate) const CANNOT_COMPLETE: i32 = -25204;
    /// `kAXErrorAttributeUnsupported`: the element has no such attribute.
    pub(crate) const ATTRIBUTE_UNSUPPORTED: i32 = -25205;
    /// `kAXErrorNotificationUnsupported`: the element does not send that notification.
    pub(crate) const NOTIFICATION_UNSUPPORTED: i32 = -25207;
    /// `kAXErrorNotificationAlreadyRegistered`.
    pub(crate) const NOTIFICATION_ALREADY_REGISTERED: i32 = -25209;
    /// `kAXErrorAPIDisabled`: this process is not a trusted Accessibility client.
    #[cfg(test)]
    pub(crate) const API_DISABLED: i32 = -25211;
    /// `kAXErrorNoValue`: the attribute exists and has no value.
    pub(crate) const NO_VALUE: i32 = -25212;
}

/// The attributes this crate asks an Accessibility element for. Nothing else is ever read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Attribute {
    /// `kAXFocusedUIElementAttribute` of an application: the element with the keyboard focus.
    FocusedElement,
    /// `kAXRoleAttribute` of an element.
    Role,
    /// `kAXSubroleAttribute` of an element.
    Subrole,
}

impl Attribute {
    /// Every attribute, for the tests.
    #[cfg(test)]
    pub(crate) const ALL: [Self; 3] = [Self::FocusedElement, Self::Role, Self::Subrole];

    /// The attribute's name, as the system header defines it.
    pub(crate) const fn name(self) -> &'static str {
        match self {
            Self::FocusedElement => "AXFocusedUIElement",
            Self::Role => "AXRole",
            Self::Subrole => "AXSubrole",
        }
    }
}

/// The notifications the focus watcher asks an application for
/// (`HIServices/AXNotificationConstants.h`).
pub(crate) mod notification {
    /// `kAXFocusedUIElementChangedNotification`: another element has the keyboard focus.
    pub(crate) const FOCUSED_ELEMENT_CHANGED: &str = "AXFocusedUIElementChanged";
    /// `kAXFocusedWindowChangedNotification`: another window of the application has the
    /// keyboard focus.
    pub(crate) const FOCUSED_WINDOW_CHANGED: &str = "AXFocusedWindowChanged";
    /// `kAXMainWindowChangedNotification`: another window of the application is its main
    /// window.
    pub(crate) const MAIN_WINDOW_CHANGED: &str = "AXMainWindowChanged";

    /// Every notification the watcher registers for.
    pub(crate) const ALL: [&str; 3] = [
        FOCUSED_ELEMENT_CHANGED,
        FOCUSED_WINDOW_CHANGED,
        MAIN_WINDOW_CHANGED,
    ];
}

/// The role or the subrole of an element, as far as it could be read.
pub(crate) enum Text {
    /// The attribute's value.
    Is(String),
    /// The element has no such attribute, or it has no value. Most elements have no subrole.
    Absent,
    /// The read failed with this `AXError`.
    Failed(i32),
}

impl Text {
    /// The outcome of a read that failed with `code`.
    pub(crate) const fn failed(code: i32) -> Self {
        match code {
            code::NO_VALUE | code::ATTRIBUTE_UNSUPPORTED => Self::Absent,
            other => Self::Failed(other),
        }
    }

    /// Whether the attribute names a secure text field.
    pub(crate) fn names_secure_field(&self) -> bool {
        matches!(self, Self::Is(name) if name == SECURE_TEXT_FIELD)
    }
}

/// Why a failed Accessibility read leaves the field's state unknown. `past_deadline` says
/// whether the time allowed for the inspection had run out when the read returned: a call the
/// application did not answer in time is a timeout, every other failure (no Accessibility
/// permission, no focused element, an element that went away) means the window cannot be
/// inspected.
pub(crate) const fn unreadable(code: i32, past_deadline: bool) -> UnknownReason {
    if code == code::CANNOT_COMPLETE && past_deadline {
        UnknownReason::Timeout
    } else {
        UnknownReason::NotInspectable
    }
}

/// The state of the focused element from its role and subrole.
///
/// - A role or a subrole that names a secure text field: `Secure`, whatever the other read
///   did.
/// - A failed read: `Unknown`.
/// - No role at all: `Unknown(NotInspectable)`. Every real element has a role; one without
///   cannot be told from a field that hides what it is.
/// - Otherwise `Normal`.
pub(crate) fn judge(role: &Text, subrole: &Text, past_deadline: bool) -> FieldSecurity {
    if role.names_secure_field() || subrole.names_secure_field() {
        return FieldSecurity::Secure;
    }
    for read in [subrole, role] {
        if let Text::Failed(code) = read {
            return FieldSecurity::Unknown(unreadable(*code, past_deadline));
        }
    }
    match role {
        Text::Is(_) => FieldSecurity::Normal,
        Text::Absent | Text::Failed(_) => FieldSecurity::Unknown(UnknownReason::NotInspectable),
    }
}

/// What the answer to one notification registration means for the watcher.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Registration {
    /// The application will send the notification.
    Registered,
    /// The application does not send this notification; the others may still work.
    Unsupported,
    /// The application did not take the registration; try the whole registration again
    /// later.
    Failed,
}

/// Maps the `AXError` of `AXObserverAddNotification`.
pub(crate) const fn registration(code: i32) -> Registration {
    match code {
        code::SUCCESS | code::NOTIFICATION_ALREADY_REGISTERED => Registration::Registered,
        code::NOTIFICATION_UNSUPPORTED => Registration::Unsupported,
        _ => Registration::Failed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn is(name: &str) -> Text {
        Text::Is(name.to_owned())
    }

    #[test]
    fn only_the_focus_the_role_and_the_subrole_are_ever_asked_for() {
        let names: Vec<_> = Attribute::ALL.iter().map(|a| a.name()).collect();
        assert_eq!(names, ["AXFocusedUIElement", "AXRole", "AXSubrole"]);
        // No attribute that carries what a field contains or shows.
        for forbidden in [
            "AXValue",
            "AXTitle",
            "AXDescription",
            "AXSelectedText",
            "AXPlaceholderValue",
            "AXHelp",
        ] {
            assert!(!names.contains(&forbidden));
        }
    }

    #[test]
    fn a_secure_subrole_or_role_is_secure() {
        // A native password field: a text field with the secure subrole.
        assert_eq!(
            judge(&is("AXTextField"), &is(SECURE_TEXT_FIELD), false),
            FieldSecurity::Secure
        );
        // A toolkit that puts the name in the role.
        assert_eq!(
            judge(&is(SECURE_TEXT_FIELD), &Text::Absent, false),
            FieldSecurity::Secure
        );
        // The subrole alone decides, even when the role could not be read.
        assert_eq!(
            judge(
                &Text::Failed(code::CANNOT_COMPLETE),
                &is(SECURE_TEXT_FIELD),
                true
            ),
            FieldSecurity::Secure
        );
    }

    #[test]
    fn an_ordinary_element_is_normal() {
        for (role, subrole) in [
            ("AXTextField", None),
            ("AXTextArea", None),
            ("AXTextField", Some("AXSearchField")),
            ("AXWebArea", None),
            ("AXWindow", Some("AXStandardWindow")),
        ] {
            let subrole = subrole.map_or(Text::Absent, is);
            assert_eq!(
                judge(&is(role), &subrole, false),
                FieldSecurity::Normal,
                "{role}"
            );
        }
    }

    #[test]
    fn the_secure_name_is_matched_exactly() {
        for near in ["axsecuretextfield", "AXSecureTextField ", "SecureTextField"] {
            assert_eq!(
                judge(&is("AXTextField"), &is(near), false),
                FieldSecurity::Normal
            );
        }
    }

    #[test]
    fn a_failed_read_is_unknown_never_normal() {
        let not_inspectable = FieldSecurity::Unknown(UnknownReason::NotInspectable);
        let timeout = FieldSecurity::Unknown(UnknownReason::Timeout);
        // The role is fine, the subrole read failed.
        assert_eq!(
            judge(&is("AXTextField"), &Text::Failed(code::API_DISABLED), false),
            not_inspectable
        );
        assert_eq!(
            judge(
                &is("AXTextField"),
                &Text::Failed(code::CANNOT_COMPLETE),
                true
            ),
            timeout
        );
        // The subrole is absent, the role read failed.
        assert_eq!(
            judge(&Text::Failed(-25202), &Text::Absent, false),
            not_inspectable
        );
        assert_eq!(
            judge(&Text::Failed(code::CANNOT_COMPLETE), &Text::Absent, true),
            timeout
        );
        // An application that did not answer although there was time left: not a timeout.
        assert_eq!(
            judge(&Text::Failed(code::CANNOT_COMPLETE), &Text::Absent, false),
            not_inspectable
        );
    }

    #[test]
    fn an_element_without_a_role_is_unknown() {
        assert_eq!(
            judge(&Text::Absent, &Text::Absent, false),
            FieldSecurity::Unknown(UnknownReason::NotInspectable)
        );
        assert_eq!(
            judge(&Text::Absent, &is("AXSearchField"), false),
            FieldSecurity::Unknown(UnknownReason::NotInspectable)
        );
    }

    #[test]
    fn a_missing_attribute_is_absent_and_other_errors_are_failures() {
        assert!(matches!(Text::failed(code::NO_VALUE), Text::Absent));
        assert!(matches!(
            Text::failed(code::ATTRIBUTE_UNSUPPORTED),
            Text::Absent
        ));
        for code in [code::CANNOT_COMPLETE, code::API_DISABLED, -25200, -25202] {
            assert!(matches!(Text::failed(code), Text::Failed(c) if c == code));
        }
    }

    #[test]
    fn only_an_unanswered_call_past_the_deadline_is_a_timeout() {
        assert_eq!(
            unreadable(code::CANNOT_COMPLETE, true),
            UnknownReason::Timeout
        );
        assert_eq!(
            unreadable(code::CANNOT_COMPLETE, false),
            UnknownReason::NotInspectable
        );
        for code in [code::API_DISABLED, code::NO_VALUE, -25200, -25202] {
            for past in [false, true] {
                assert_eq!(unreadable(code, past), UnknownReason::NotInspectable);
            }
        }
    }

    #[test]
    fn registrations_are_sorted_into_three() {
        assert_eq!(registration(code::SUCCESS), Registration::Registered);
        assert_eq!(
            registration(code::NOTIFICATION_ALREADY_REGISTERED),
            Registration::Registered
        );
        assert_eq!(
            registration(code::NOTIFICATION_UNSUPPORTED),
            Registration::Unsupported
        );
        for code in [code::CANNOT_COMPLETE, code::API_DISABLED, -25200, -25202] {
            assert_eq!(registration(code), Registration::Failed);
        }
    }

    #[test]
    fn the_watcher_asks_for_focus_notifications_only() {
        assert_eq!(
            notification::ALL,
            [
                "AXFocusedUIElementChanged",
                "AXFocusedWindowChanged",
                "AXMainWindowChanged"
            ]
        );
    }
}
