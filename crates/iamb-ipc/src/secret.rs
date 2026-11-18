//! [`Secret`]: user-entered values inside a message (architecture §8 rules 2 and 3).

use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// A value the user typed or copied (form values, a dry-run's typed text, a palette query, a
/// clipboard text): it is serialised only into the IPC frame that carries it.
///
/// It implements neither `Display` nor a revealing `Debug` (`Debug` prints `<redacted>`), so a
/// message that holds one can be debug-printed without leaking it. It is the wire-side
/// counterpart of `iamb_core::sensitive::Sensitive`, which cannot be serialised at all.
#[derive(Clone, PartialEq, Eq, Default)]
pub struct Secret<T>(T);

impl<T> Secret<T> {
    /// Wraps a value.
    pub const fn new(value: T) -> Self {
        Self(value)
    }

    /// The value, for the code that must use it.
    pub const fn expose(&self) -> &T {
        &self.0
    }

    /// Unwraps the value.
    pub fn into_inner(self) -> T {
        self.0
    }
}

impl<T> fmt::Debug for Secret<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("<redacted>")
    }
}

impl<T: Serialize> Serialize for Secret<T> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.0.serialize(serializer)
    }
}

impl<'de, T: Deserialize<'de>> Deserialize<'de> for Secret<T> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        T::deserialize(deserializer).map(Self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_is_redacted_and_serde_is_transparent() {
        let s = Secret::new("hunter2".to_owned());
        assert_eq!(format!("{s:?}"), "<redacted>");
        let json = serde_json::to_string(&s).expect("serialises");
        assert_eq!(json, "\"hunter2\"");
        let back: Secret<String> = serde_json::from_str(&json).expect("parses");
        assert_eq!(back.expose(), "hunter2");
        assert_eq!(back.into_inner(), "hunter2");
    }
}
