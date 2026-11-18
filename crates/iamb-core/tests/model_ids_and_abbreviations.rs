//! Tests for `Id`, `Sha256` and `Abbreviation`.
#![allow(clippy::unwrap_used)]

use iamb_core::model::*;
use proptest::prelude::*;

#[test]
fn id_roundtrip_and_format() {
    let text = "0192f3a4-b5c6-7d8e-9fa0-b1c2d3e4f506";
    let id: Id = text.parse().unwrap();
    assert_eq!(id.to_string(), text);
    assert_eq!(serde_json::to_string(&id).unwrap(), format!("\"{text}\""));
    let upper: Id = text.to_uppercase().parse().unwrap();
    assert_eq!(upper, id);
    assert_eq!(upper.to_string(), text);
}

#[test]
fn id_rejects_bad_text() {
    for bad in [
        "",
        "0192f3a4b5c67d8e9fa0b1c2d3e4f506",
        "0192f3a4-b5c6-7d8e-9fa0-b1c2d3e4f50",
        "0192f3a4-b5c6-7d8e-9fa0-b1c2d3e4f5066",
        "0192f3a4_b5c6-7d8e-9fa0-b1c2d3e4f506",
        "0192f3a4-b5c6-7d8e-9fa0-b1c2d3e4f50g",
        "{0192f3a4-b5c6-7d8e-9fa0-b1c2d3e4f506}",
        "0192f3a4-b5c6-7d8e-9fa0-b1c2d3e4f5é6",
    ] {
        assert!(bad.parse::<Id>().is_err(), "{bad:?}");
    }
    assert_eq!(
        "".parse::<Id>().unwrap_err(),
        ModelError::IdLength { found: 0 }
    );
    assert!(serde_json::from_str::<Id>("123").is_err());
}

#[test]
fn v7_sets_version_and_variant() {
    let id = Id::v7(1_700_000_000_000, [0xff; 10]);
    let b = id.as_bytes();
    assert_eq!(b[6] >> 4, 7);
    assert_eq!(b[8] >> 6, 0b10);
    assert_eq!(id.version(), 7);
    assert_eq!(id.timestamp_ms(), Some(1_700_000_000_000));
    let z = Id::v7(0, [0; 10]);
    assert_eq!(z.to_string(), "00000000-0000-7000-8000-000000000000");
    assert_eq!(Id::from_bytes([0; 16]).timestamp_ms(), None);
}

#[test]
fn v7_sorts_by_time() {
    let a = Id::v7(1000, [0xff; 10]);
    let b = Id::v7(1001, [0x00; 10]);
    let c = Id::v7(1_000_000_000_000, [0x00; 10]);
    assert!(a < b && b < c);
    assert!(a.to_string() < b.to_string() && b.to_string() < c.to_string());
}

#[test]
fn sha256_hex() {
    let h = Sha256::from_bytes([0x0f; 32]);
    assert_eq!(h.to_string(), "0f".repeat(32));
    assert_eq!(h.to_string().parse::<Sha256>().unwrap(), h);
    assert_eq!("0F".repeat(32).parse::<Sha256>().unwrap(), h);
    assert_eq!(
        "00".parse::<Sha256>().unwrap_err(),
        ModelError::Sha256Length { found: 2 }
    );
    assert_eq!(
        format!("{}g", "0".repeat(63))
            .parse::<Sha256>()
            .unwrap_err(),
        ModelError::Sha256HexDigit { index: 63 }
    );
}

#[test]
fn abbreviation_rules() {
    assert_eq!(
        Abbreviation::new("").unwrap_err(),
        ModelError::AbbreviationEmpty
    );
    assert!(Abbreviation::new("a").is_ok());
    assert_eq!(Abbreviation::new(&"a".repeat(64)).unwrap().char_len(), 64);
    assert_eq!(
        Abbreviation::new(&"a".repeat(65)).unwrap_err(),
        ModelError::AbbreviationTooLong { found: 65 }
    );
    // Multi-byte: 64 characters is well over 64 bytes but still allowed.
    let ok = "é".repeat(64);
    assert!(ok.len() > 64);
    assert_eq!(Abbreviation::new(&ok).unwrap().char_len(), 64);
    assert!(Abbreviation::new(&"😀".repeat(65)).is_err());
    assert!(Abbreviation::new(&"😀".repeat(64)).is_ok());
    assert!(matches!(
        Abbreviation::new("a b").unwrap_err(),
        ModelError::AbbreviationWhitespace { found: ' ' }
    ));
    assert!(matches!(
        Abbreviation::new("a\u{a0}b").unwrap_err(),
        ModelError::AbbreviationWhitespace { .. }
    ));
    assert!(matches!(
        Abbreviation::new("a\u{3000}b").unwrap_err(),
        ModelError::AbbreviationWhitespace { .. }
    ));
    assert!(matches!(
        Abbreviation::new("a\nb").unwrap_err(),
        ModelError::AbbreviationControl { .. }
    ));
    assert!(matches!(
        Abbreviation::new("a\u{7f}").unwrap_err(),
        ModelError::AbbreviationControl { .. }
    ));
    assert!(Abbreviation::new("🙂🇨🇭;sig").is_ok());
    assert!("x".parse::<Abbreviation>().is_ok());
}

#[test]
fn abbreviation_fold_key() {
    assert_eq!(Abbreviation::new("ÄBC").unwrap().fold_key(), "äbc");
    assert_eq!(Abbreviation::new("MiXeD1").unwrap().fold_key(), "mixed1");
    assert_eq!(
        Abbreviation::new("Abc").unwrap().fold_key(),
        Abbreviation::new("aBC").unwrap().fold_key()
    );
    // Preserves what was typed.
    assert_eq!(Abbreviation::new("ÄBC").unwrap().as_str(), "ÄBC");
}

/// Review 1.9 finding 12: the fold key folds like the matcher (per character, final sigma as
/// sigma), so the store's uniqueness check rejects what the matcher would treat as one.
#[test]
fn abbreviation_fold_key_agrees_with_the_matcher_on_sigma() {
    use iamb_core::matcher::{Delimiters, MatchEntry, Matcher};
    use iamb_core::model::{CaseMode, Id, Trigger};

    let key = |s: &str| Abbreviation::new(s).unwrap().fold_key();
    assert_eq!(key("ΟΔΟΣ"), "οδοσ");
    assert_eq!(key("ΟΔΟΣ"), key("οδοσ"));
    assert_eq!(key("οδος"), key("οδοσ"));
    assert_eq!(iamb_core::model::fold_abbreviation("ΟΔΟΣ"), key("οδος"));

    // And the matcher drops the second of two such abbreviations as a duplicate of the first.
    let entry = |n: u8, a: &str| MatchEntry {
        snippet: Id::from_bytes([n; 16]),
        abbreviation: Abbreviation::new(a).unwrap(),
        trigger: Trigger::Delimiter,
        case_mode: CaseMode::Adaptive,
        word_start: true,
    };
    let m = Matcher::build([entry(1, "ΟΔΟΣ"), entry(2, "οδοσ")], Delimiters::default());
    assert_eq!(m.duplicates().len(), 1);
}

#[test]
fn error_display_and_trait() {
    let e: Box<dyn std::error::Error> = Box::new(ModelError::AbbreviationEmpty);
    assert!(!e.to_string().is_empty());
    for err in [
        ModelError::IdLength { found: 1 },
        ModelError::IdHyphen { index: 8 },
        ModelError::IdHexDigit { index: 0 },
        ModelError::Sha256Length { found: 1 },
        ModelError::Sha256HexDigit { index: 0 },
        ModelError::AbbreviationTooLong { found: 70 },
        ModelError::AbbreviationWhitespace { found: ' ' },
        ModelError::AbbreviationControl { found: '\n' },
    ] {
        assert!(!err.to_string().is_empty());
    }
}

proptest! {
    #[test]
    fn id_parse_never_panics(s in ".*") {
        let _ = s.parse::<Id>();
        let _ = s.parse::<Sha256>();
        let _ = Abbreviation::new(&s);
    }

    #[test]
    fn id_parse_never_panics_on_hexish(s in "[0-9a-fA-F-]{0,40}") {
        let _ = s.parse::<Id>();
    }

    #[test]
    fn v7_text_roundtrip(ms in 0u64..(1 << 48), random in any::<[u8; 10]>()) {
        let id = Id::v7(ms, random);
        let text = id.to_string();
        prop_assert_eq!(text.len(), 36);
        prop_assert_eq!(text.parse::<Id>().unwrap(), id);
        prop_assert_eq!(id.timestamp_ms(), Some(ms));
        prop_assert_eq!(id.as_bytes()[8] >> 6, 0b10);
        let json = serde_json::to_string(&id).unwrap();
        prop_assert_eq!(serde_json::from_str::<Id>(&json).unwrap(), id);
    }

    #[test]
    fn v7_order_follows_time(a in 0u64..(1 << 48), b in 0u64..(1 << 48), ra in any::<[u8; 10]>(), rb in any::<[u8; 10]>()) {
        prop_assume!(a != b);
        prop_assert_eq!(Id::v7(a, ra) < Id::v7(b, rb), a < b);
    }

    #[test]
    fn valid_abbreviations_roundtrip(s in "[^\\s\\p{Cc}]{1,64}") {
        let a = Abbreviation::new(&s).unwrap();
        let json = serde_json::to_string(&a).unwrap();
        prop_assert_eq!(serde_json::from_str::<Abbreviation>(&json).unwrap(), a);
    }
}
