//! `CGEvent` output behind [`Output`], and `Injector` for [`Platform`]. See the parent module
//! for the design.

use std::ffi::c_ulong;
use std::sync::{PoisonError, RwLock};
use std::thread;
use std::time::Duration;

use iamb_platform::{
    ClipboardContent, InjectionOutcome, InjectionRequest, Injector, Marker, PermissionKind,
    PlatformError, Result, TypedHandoff,
};
use objc2_core_foundation::CFRetained;
use objc2_core_graphics::{
    CGEvent, CGEventField, CGEventFlags, CGEventSource, CGEventSourceStateID, CGEventTapLocation,
    CGPreflightPostEventAccess,
};

use super::chunk::TextChunk;
use super::plan::{KeyPress, keycode};
use super::run::{self, Output};
use super::strategy::{Mods, StrategyTable};
use crate::Platform;
use crate::clipboard::paste::paste_with_restore;

/// The injector's state: the strategy table with the user's settings.
#[derive(Debug, Default)]
pub(crate) struct State {
    table: RwLock<StrategyTable>,
}

impl Platform {
    /// Replaces the user's per-application injection settings with `settings_json` (the
    /// format is in the docs of the crate's strategy module; an empty string clears them).
    /// The built-in table stays underneath. On error the previous settings stay in force.
    ///
    /// # Errors
    ///
    /// [`PlatformError::Invalid`] naming what was wrong (never the settings' content).
    pub fn set_injection_settings(&self, settings_json: &str) -> Result<()> {
        let table = StrategyTable::with_user_settings(settings_json).map_err(|error| {
            PlatformError::Invalid {
                context: error.what,
            }
        })?;
        *self
            .injector
            .table
            .write()
            .unwrap_or_else(PoisonError::into_inner) = table;
        Ok(())
    }
}

impl Injector for Platform {
    fn inject(&self, request: &InjectionRequest, typed: &TypedHandoff) -> InjectionOutcome {
        let strategy = self
            .injector
            .table
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .resolve(request.app.as_ref(), &request.overrides);
        run::inject(request, typed, &strategy, may_post, || Events::open(self))
    }
}

/// The system's answer to "may this process post events?", which Accessibility grants.
fn may_post() -> Result<()> {
    if CGPreflightPostEventAccess() {
        Ok(())
    } else {
        Err(PlatformError::PermissionDenied(
            PermissionKind::Accessibility,
        ))
    }
}

/// The event flags for `mods`. Every synthetic event carries its flags explicitly, so a
/// modifier the user holds does not change Iamb's output.
fn flags(mods: Mods) -> CGEventFlags {
    let mut flags = CGEventFlags::MaskNonCoalesced;
    if mods.shift {
        flags |= CGEventFlags::MaskShift;
    }
    if mods.option {
        flags |= CGEventFlags::MaskAlternate;
    }
    if mods.control {
        flags |= CGEventFlags::MaskControl;
    }
    if mods.command {
        flags |= CGEventFlags::MaskCommand;
    }
    flags
}

/// Puts `units` into the event's Unicode string.
fn set_string(event: &CGEvent, units: &[u16]) -> Result<()> {
    let len = c_ulong::try_from(units.len()).map_err(|_| PlatformError::Invalid {
        context: "text chunk too long",
    })?;
    // SAFETY: `units` is a live slice of `len` UTF-16 code units for the duration of the call;
    // the function copies them into the event and keeps no pointer.
    unsafe { CGEvent::keyboard_set_unicode_string(Some(event), len, units.as_ptr()) };
    Ok(())
}

/// The event source of one injection, and the platform for its pastes.
struct Events<'p> {
    source: CFRetained<CGEventSource>,
    platform: &'p Platform,
}

impl<'p> Events<'p> {
    fn open(platform: &'p Platform) -> Result<Self> {
        // A private source: its events neither read nor change the modifier state of the real
        // keyboard.
        let source = CGEventSource::new(CGEventSourceStateID::Private)
            .ok_or(PlatformError::os(0, "CGEventSourceCreate"))?;
        CGEventSource::set_user_data(Some(&source), Marker::Output.encode_i64());
        Ok(Self { source, platform })
    }

    /// A key event from this source with `flags` and `marker`.
    fn key_event(
        &self,
        code: u16,
        down: bool,
        flags: CGEventFlags,
        marker: Marker,
    ) -> Result<CFRetained<CGEvent>> {
        let event = CGEvent::new_keyboard_event(Some(&self.source), code, down)
            .ok_or(PlatformError::os(0, "CGEventCreateKeyboardEvent"))?;
        CGEvent::set_flags(Some(&event), flags);
        CGEvent::set_integer_value_field(
            Some(&event),
            CGEventField::EventSourceUserData,
            marker.encode_i64(),
        );
        Ok(event)
    }

    /// A press and release of `key`, carrying `text` when given. Both events are made before
    /// either is posted, so a failure never leaves a key down.
    fn pair(&self, key: KeyPress, text: Option<&[u16]>) -> Result<[CFRetained<CGEvent>; 2]> {
        let flags = flags(key.mods);
        let down = self.key_event(key.code, true, flags, Marker::Output)?;
        let up = self.key_event(key.code, false, flags, Marker::Output)?;
        if let Some(units) = text {
            set_string(&down, units)?;
            set_string(&up, units)?;
        }
        Ok([down, up])
    }

    fn post(events: &[CFRetained<CGEvent>]) {
        for event in events {
            CGEvent::post(CGEventTapLocation::HIDEventTap, Some(event));
        }
    }

    fn press_now(&self, key: KeyPress, text: Option<&[u16]>) -> Result<()> {
        Self::post(&self.pair(key, text)?);
        Ok(())
    }
}

/// The string the paste shortcut carries ("v"), so that it matches on any keyboard layout.
const PASTE_STRING: [u16; 1] = [0x76];

impl Output for Events<'_> {
    fn marker(&mut self, marker: Marker) -> Result<()> {
        let event = self.key_event(keycode::F20, false, flags(Mods::NONE), marker)?;
        Self::post(&[event]);
        Ok(())
    }

    fn press(&mut self, key: KeyPress) -> Result<()> {
        self.press_now(key, None)
    }

    fn text(&mut self, chunk: &TextChunk) -> Result<()> {
        self.press_now(KeyPress::plain(keycode::TEXT), Some(chunk.units()))
    }

    fn pause(&mut self, duration: Duration) {
        thread::sleep(duration);
    }

    fn paste(&mut self, content: &ClipboardContent, settle: Duration) -> Result<()> {
        let events = &*self;
        paste_with_restore(
            events.platform,
            content,
            settle,
            &mut || events.press_now(KeyPress::PASTE, Some(&PASTE_STRING)),
            &mut thread::sleep,
        )
    }
}

#[cfg(test)]
mod tests {
    //! What the events carry, read back from real `CGEvent`s without posting them: this needs
    //! no permission and runs on every Mac. Posting is tested in `tests/injector_textedit.rs`.

    use std::ffi::c_ulong;

    use iamb_platform::core_types::ExpansionId;

    use super::*;

    fn string_of(event: &CGEvent) -> Vec<u16> {
        let mut buf = [0u16; 32];
        let mut len: c_ulong = 0;
        // SAFETY: `buf` has room for the 32 units passed as the maximum and `len` is a live
        // out-parameter; the function writes at most that many units and the length.
        unsafe {
            CGEvent::keyboard_get_unicode_string(Some(event), 32, &raw mut len, buf.as_mut_ptr());
        };
        buf[..usize::try_from(len).expect("a small length")].to_vec()
    }

    fn marker_of(event: &CGEvent) -> Option<Marker> {
        Marker::decode_i64(CGEvent::integer_value_field(
            Some(event),
            CGEventField::EventSourceUserData,
        ))
    }

    fn platform_events(platform: &Platform) -> Events<'_> {
        Events::open(platform).expect("a private event source")
    }

    #[test]
    fn every_event_carries_its_marker_and_flags() {
        let platform = Platform::new();
        let events = platform_events(&platform);
        assert_eq!(
            CGEventSource::user_data(Some(&events.source)),
            Marker::Output.encode_i64()
        );
        let [down, up] = events
            .pair(KeyPress::DELETE, None)
            .expect("events are created");
        for event in [&down, &up] {
            assert_eq!(marker_of(event), Some(Marker::Output));
            assert_eq!(CGEvent::flags(Some(event)), CGEventFlags::MaskNonCoalesced);
            assert_eq!(
                CGEvent::integer_value_field(Some(event), CGEventField::KeyboardEventKeycode),
                i64::from(keycode::DELETE)
            );
        }
        let e = ExpansionId::from_raw(42);
        for marker in [Marker::InjectionStart(e), Marker::InjectionEnd(e)] {
            let event = events
                .key_event(keycode::F20, false, flags(Mods::NONE), marker)
                .expect("a marker event");
            assert_eq!(marker_of(&event), Some(marker));
        }
    }

    #[test]
    fn modifiers_become_flags() {
        let all = Mods {
            shift: true,
            option: true,
            control: true,
            command: true,
        };
        let expected = CGEventFlags::MaskNonCoalesced
            | CGEventFlags::MaskShift
            | CGEventFlags::MaskAlternate
            | CGEventFlags::MaskControl
            | CGEventFlags::MaskCommand;
        assert_eq!(flags(all), expected);
        let platform = Platform::new();
        let events = platform_events(&platform);
        let [down, _] = events
            .pair(KeyPress::PASTE, Some(&PASTE_STRING))
            .expect("events");
        assert_eq!(
            CGEvent::flags(Some(&down)),
            CGEventFlags::MaskNonCoalesced | CGEventFlags::MaskCommand
        );
        assert_eq!(string_of(&down), PASTE_STRING);
    }

    #[test]
    fn text_events_carry_the_chunk_exactly() {
        let platform = Platform::new();
        let events = platform_events(&platform);
        let text = "Café 👋🏽 👩‍💻 🇯🇵";
        let chunks = super::super::chunk::chunk_chars(text.chars(), 20);
        let mut round_trip = Vec::new();
        for chunk in &chunks {
            let [down, up] = events
                .pair(KeyPress::plain(keycode::TEXT), Some(chunk.units()))
                .expect("events");
            assert_eq!(string_of(&down), chunk.units());
            assert_eq!(string_of(&up), chunk.units());
            round_trip.extend(string_of(&down));
        }
        assert_eq!(String::from_utf16(&round_trip).expect("valid UTF-16"), text);
    }

    #[test]
    fn settings_are_validated_and_kept_on_error() {
        let platform = Platform::new();
        assert_eq!(
            platform.set_injection_settings(r#"{"apps": {"com.example.A": {"chunk_units": 3}}}"#),
            Ok(())
        );
        assert_eq!(
            platform.set_injection_settings(r#"{"apps": {"com.example.A": {"chunk_units": 0}}}"#),
            Err(PlatformError::Invalid {
                context: "\"chunk_units\" out of range"
            })
        );
        let kept = platform
            .injector
            .table
            .read()
            .expect("not poisoned")
            .resolve(
                Some(&iamb_platform::core_types::AppId(
                    "com.example.a".to_owned(),
                )),
                &iamb_platform::InjectionOverrides::default(),
            );
        assert_eq!(kept.chunk_units, 3);
        assert_eq!(platform.set_injection_settings(""), Ok(()));
    }

    #[test]
    fn without_permission_the_outcome_names_accessibility() {
        if CGPreflightPostEventAccess() {
            note(
                "injector::mac::tests::without_permission_the_outcome_names_accessibility: \
                 this process may post events, so the refusal cannot be shown here; \
                 `injector::run::tests` covers it",
            );
            return;
        }
        let platform = Platform::new();
        let request = InjectionRequest::new(ExpansionId::from_raw(1), Vec::new());
        assert_eq!(
            platform.inject(&request, &TypedHandoff::new()),
            InjectionOutcome::Failed(PlatformError::PermissionDenied(
                PermissionKind::Accessibility
            ))
        );
    }

    /// A line on standard error, which the test harness does not capture.
    pub(super) fn note(text: &str) {
        use std::io::Write as _;
        let _ = writeln!(std::io::stderr().lock(), "NOTE {text}");
    }
}
