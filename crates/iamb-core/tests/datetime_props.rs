//! Property tests for the date arithmetic.

use iamb_core::datetime::{
    Format, LocalDateTime, Locale, Shift, Weekday, civil_from_days, days_from_civil,
};
use proptest::prelude::*;

fn arb_datetime() -> impl Strategy<Value = LocalDateTime> {
    (1i32..=9999, 1u8..=12, 1u8..=31, 0u8..24, 0u8..60, 0u8..60).prop_map(|(y, mo, d, h, mi, s)| {
        // Clamp the day to a real one; try 31 down to 28.
        (1..=d)
            .rev()
            .find_map(|day| LocalDateTime::new(y, mo, day, h, mi, s).ok())
            .expect("day 1 always exists")
    })
}

proptest! {
    #[test]
    fn format_parse_never_panics(s in ".*") {
        let _ = Format::parse(&s);
    }

    #[test]
    fn format_parse_percent_heavy_never_panics(s in "[%a-zA-Z\\-]{0,24}") {
        let _ = Format::parse(&s);
    }

    #[test]
    fn render_never_panics(s in "[%a-zA-Z\\- ]{1,24}", t in arb_datetime(), tag in "[a-zA-Z-]{0,6}") {
        if let Ok(f) = Format::parse(&s) {
            let _ = f.render(&t, &Locale::from_tag(&tag));
        }
    }

    #[test]
    fn shift_parse_never_panics(s in ".*") {
        let _ = Shift::parse(&s);
    }

    #[test]
    fn shift_parse_termlike_never_panics(s in "[+\\- 0-9a-zA-Z]{0,24}") {
        let _ = Shift::parse(&s);
    }

    #[test]
    fn plus_then_minus_days_roundtrips(t in arb_datetime(), n in 0u32..=9999) {
        // Keep within range: choose the direction that cannot overflow.
        let plus = Shift::parse(&format!("+{n}d")).expect("ok");
        let minus = Shift::parse(&format!("-{n}d")).expect("ok");
        if let Ok(moved) = plus.apply(t) {
            prop_assert_eq!(minus.apply(moved).expect("ok"), t);
        }
        if let Ok(moved) = minus.apply(t) {
            prop_assert_eq!(plus.apply(moved).expect("ok"), t);
        }
    }

    #[test]
    fn days_match_stepping(t in arb_datetime(), n in 0u32..=60) {
        let bulk = Shift::parse(&format!("+{n}d")).expect("ok");
        let one = Shift::parse("+1d").expect("ok");
        let mut stepped = Ok(t);
        for _ in 0..n {
            stepped = stepped.and_then(|x| one.apply(x));
        }
        prop_assert_eq!(bulk.apply(t), stepped);
    }

    #[test]
    fn civil_roundtrip(t in arb_datetime()) {
        let days = days_from_civil(t.year(), t.month(), t.day());
        prop_assert_eq!(civil_from_days(days), (t.year(), t.month(), t.day()));
    }

    #[test]
    fn weekday_advances_with_days(t in arb_datetime()) {
        let one = Shift::parse("+1d").expect("ok");
        if let Ok(next) = one.apply(t) {
            prop_assert_ne!(next.weekday(), t.weekday());
            prop_assert!(next > t);
        }
    }

    /// Review 1.9 finding 1: business days are computed in closed form; this compares them with
    /// the reference's day-by-day stepping (including `E_DATE_RANGE` near years 1 and 9999).
    #[test]
    fn business_days_match_day_by_day_stepping(
        t in prop_oneof![
            arb_datetime(),
            (1u8..=12, 1u8..=28).prop_map(|(m, d)| LocalDateTime::new(1, m, d, 9, 0, 0).expect("valid")),
            (1u8..=12, 1u8..=28).prop_map(|(m, d)| LocalDateTime::new(9999, m, d, 9, 0, 0).expect("valid")),
        ],
        n in 0i64..=3000,
        backwards in any::<bool>(),
    ) {
        let (sign, step) = if backwards { ("-", "-1d") } else { ("+", "+1d") };
        let closed = Shift::parse(&format!("{sign}{n}bd")).expect("ok").apply(t);
        let one = Shift::parse(step).expect("ok");
        let mut stepped = Ok(t);
        let mut left = n;
        while left > 0 {
            stepped = stepped.and_then(|x| one.apply(x));
            match stepped {
                Ok(x) if !x.weekday().is_weekend() => left -= 1,
                Ok(_) => {}
                Err(_) => break,
            }
        }
        prop_assert_eq!(closed, stepped);
    }

    #[test]
    fn business_days_land_on_weekdays(t in arb_datetime(), n in 1u32..=30) {
        for sign in ["+", "-"] {
            let s = Shift::parse(&format!("{sign}{n}bd")).expect("ok");
            if let Ok(r) = s.apply(t) {
                prop_assert!(!r.weekday().is_weekend());
            }
        }
    }
}

#[test]
fn known_weekdays() {
    let a = LocalDateTime::parse_iso("2026-03-18T00:00:00").expect("ok");
    assert_eq!(a.weekday(), Weekday::Wednesday);
    let b = LocalDateTime::parse_iso("0001-01-01T00:00:00").expect("ok");
    assert_eq!(b.weekday(), Weekday::Monday);
    assert_eq!(a.ordinal(), 77);
    assert_eq!(days_from_civil(1970, 1, 1), 0);
}
