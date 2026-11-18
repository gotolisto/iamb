//! Hands the `injection_overrides` setting to the injector (ADR-0025).
//!
//! The setting is applied with [`Injector::configure`] at start and after every settings
//! change that alters it. A refusal is logged (the log line names what was wrong, never the
//! setting's text) and the injector keeps the last settings it accepted. A client that writes
//! the setting with `PutSetting` hears the refusal as an `invalid` error and nothing is stored
//! ([`InjectionSettings::try_apply`]).
//!
//! The state lives on the store thread, which performs every settings write and rebuilds the
//! view after it, so the injector's settings and the stored setting change in one order.

use std::collections::BTreeMap;
use std::sync::Arc;

use iamb_platform::{INJECTION_SETTINGS_KEY, Injector, PlatformError};

use crate::log::Logger;

/// The value of the setting in `settings`: empty when it is not set.
pub(crate) fn setting_value(settings: &BTreeMap<String, String>) -> &str {
    settings
        .get(INJECTION_SETTINGS_KEY)
        .map_or("", String::as_str)
}

/// What the injector was given and what it took.
pub(crate) struct InjectionSettings {
    injector: Arc<dyn Injector>,
    logger: Arc<Logger>,
    /// The value last handed to the injector, accepted or not (`None` before the first).
    seen: Option<String>,
    /// The value last accepted: the settings in force.
    good: String,
}

impl InjectionSettings {
    /// Applies the stored value at start.
    pub(crate) fn start(
        injector: Arc<dyn Injector>,
        logger: Arc<Logger>,
        settings: &BTreeMap<String, String>,
    ) -> Self {
        let mut state = Self {
            injector,
            logger,
            seen: None,
            good: String::new(),
        };
        state.follow(settings);
        state
    }

    /// After a settings change: applies the stored value if it differs from the one handed
    /// over last. A value that was refused once is not tried (or logged) again until it
    /// changes.
    pub(crate) fn follow(&mut self, settings: &BTreeMap<String, String>) {
        let value = setting_value(settings);
        if self.seen.as_deref() != Some(value) {
            let _ = self.try_apply(value);
        }
    }

    /// Hands `value` to the injector. On a refusal the injector keeps the settings it had.
    ///
    /// # Errors
    ///
    /// The injector's refusal, already logged.
    pub(crate) fn try_apply(&mut self, value: &str) -> Result<(), PlatformError> {
        self.seen = Some(value.to_owned());
        match self.injector.configure(value) {
            Ok(()) => {
                value.clone_into(&mut self.good);
                self.logger.info(format_args!(
                    "injection settings applied ({} bytes)",
                    value.len()
                ));
                Ok(())
            }
            Err(e) => {
                self.logger.warn(format_args!(
                    "the {INJECTION_SETTINGS_KEY} setting was refused ({e}); the previous \
                     injection settings stay in force"
                ));
                Err(e)
            }
        }
    }

    /// The state now, for [`restore`](Self::restore).
    pub(crate) fn checkpoint(&self) -> Checkpoint {
        Checkpoint {
            seen: self.seen.clone(),
            good: self.good.clone(),
        }
    }

    /// Goes back to `checkpoint`, handing its settings to the injector again: for a value
    /// applied with [`try_apply`](Self::try_apply) that could then not be stored.
    pub(crate) fn restore(&mut self, checkpoint: Checkpoint) {
        if self.injector.configure(&checkpoint.good).is_err() {
            // The injector took these settings before; nothing better can be done.
            self.logger.error(format_args!(
                "the injector refused its previous injection settings"
            ));
        }
        self.seen = checkpoint.seen;
        self.good = checkpoint.good;
    }
}

/// A saved [`InjectionSettings`] state.
pub(crate) struct Checkpoint {
    seen: Option<String>,
    good: String,
}

#[cfg(test)]
mod tests {
    use iamb_platform::mock::MockPlatform;

    use super::*;

    fn settings(value: Option<&str>) -> BTreeMap<String, String> {
        value
            .map(|v| (INJECTION_SETTINGS_KEY.to_owned(), v.to_owned()))
            .into_iter()
            .collect()
    }

    const GOOD: &str = r#"{"apps": {"editor.exe": {"method": "keys"}}}"#;
    const OTHER: &str = r#"{"defaults": {"paste_threshold": 10}}"#;
    const BAD: &str = r#"{"apps": {"editor.exe": {"typing": "keys"}}}"#;

    fn start(os: &MockPlatform, value: Option<&str>) -> InjectionSettings {
        InjectionSettings::start(
            Arc::new(os.clone()),
            Arc::new(Logger::disabled()),
            &settings(value),
        )
    }

    #[test]
    fn the_stored_value_is_applied_at_start_and_on_change_only() {
        let os = MockPlatform::new();
        let mut state = start(&os, Some(GOOD));
        assert_eq!(os.configured(), [GOOD]);
        state.follow(&settings(Some(GOOD)));
        assert_eq!(os.configured(), [GOOD], "unchanged: not handed over again");
        state.follow(&settings(Some(OTHER)));
        state.follow(&settings(None));
        assert_eq!(os.configured(), [GOOD, OTHER, ""]);
        assert_eq!(os.injection_settings(), "");
    }

    #[test]
    fn without_the_setting_the_empty_string_is_applied() {
        let os = MockPlatform::new();
        let _state = start(&os, None);
        assert_eq!(os.configured(), [""]);
    }

    #[test]
    fn a_refused_value_keeps_the_last_good_settings_and_is_tried_once() {
        let os = MockPlatform::new();
        let mut state = start(&os, Some(GOOD));
        state.follow(&settings(Some(BAD)));
        state.follow(&settings(Some(BAD)));
        assert_eq!(os.configured(), [GOOD, BAD]);
        assert_eq!(os.injection_settings(), GOOD);
        assert!(matches!(
            state.try_apply(BAD),
            Err(PlatformError::Invalid { .. })
        ));
        assert_eq!(os.injection_settings(), GOOD);
    }

    #[test]
    fn restore_puts_the_previous_settings_back() {
        let os = MockPlatform::new();
        let mut state = start(&os, Some(GOOD));
        let before = state.checkpoint();
        assert_eq!(state.try_apply(OTHER), Ok(()));
        assert_eq!(os.injection_settings(), OTHER);
        state.restore(before);
        assert_eq!(os.injection_settings(), GOOD);
        // The stored value is still the one seen, so the next rebuild does not hand it over.
        state.follow(&settings(Some(GOOD)));
        assert_eq!(os.configured(), [GOOD, OTHER, GOOD]);
    }
}
