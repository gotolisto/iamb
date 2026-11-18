//! Table test for date shifts and formats. Expected values come from the reference
//! implementation in `spec/template/reference.py`.

use iamb_core::datetime::{DateError, Format, LocalDateTime, Locale, Shift};

type Case = (
    &'static str,
    &'static str,
    &'static str,
    &'static str,
    Result<&'static str, DateError>,
);

fn run(now: &str, shift: &str, fmt: &str, locale: &str) -> Result<String, DateError> {
    let t = LocalDateTime::parse_iso(now)?;
    let format = Format::parse(fmt)?;
    let shift = Shift::parse(shift)?;
    let shifted = shift.apply(t)?;
    Ok(format.render(&shifted, &Locale::from_tag(locale)))
}

#[rustfmt::skip]
const CASES: &[Case] = &[
    ("2026-01-31T10:00:00", "+1mo", "%Y-%m-%d", "en", Ok("2026-02-28")),
    ("2026-03-31T10:00:00", "-1mo", "%Y-%m-%d", "en", Ok("2026-02-28")),
    ("2026-04-30T10:00:00", "+1mo", "%Y-%m-%d", "en", Ok("2026-05-30")),
    ("2024-01-31T10:00:00", "+1mo", "%Y-%m-%d", "en", Ok("2024-02-29")),
    ("2024-02-29T10:00:00", "+1y", "%Y-%m-%d", "en", Ok("2025-02-28")),
    ("2024-02-29T10:00:00", "-1y", "%Y-%m-%d", "en", Ok("2023-02-28")),
    ("2024-02-29T10:00:00", "+4y", "%Y-%m-%d", "en", Ok("2028-02-29")),
    ("2026-12-31T10:00:00", "+2mo", "%Y-%m-%d", "en", Ok("2027-02-28")),
    ("2026-01-31T10:00:00", "+1mo +1mo", "%Y-%m-%d", "en", Ok("2026-03-28")),
    ("2026-03-18T10:00:00", "+1mo -18d", "%Y-%m-%d", "en", Ok("2026-03-31")),
    ("2026-03-18T10:00:00", "+12mo", "%Y-%m-%d", "en", Ok("2027-03-18")),
    ("2026-05-31T10:00:00", "-3mo", "%Y-%m-%d", "en", Ok("2026-02-28")),
    ("2026-03-18T10:00:00", "-15mo", "%Y-%m-%d", "en", Ok("2024-12-18")),
    ("1900-02-28T10:00:00", "+1d", "%Y-%m-%d", "en", Ok("1900-03-01")),
    ("2000-02-28T10:00:00", "+1d", "%Y-%m-%d", "en", Ok("2000-02-29")),
    ("2000-02-29T10:00:00", "+1d", "%Y-%m-%d", "en", Ok("2000-03-01")),
    ("1900-03-01T10:00:00", "-1d", "%Y-%m-%d", "en", Ok("1900-02-28")),
    ("2000-03-01T10:00:00", "-1d", "%Y-%m-%d", "en", Ok("2000-02-29")),
    ("1900-02-28T10:00:00", "+1y", "%Y-%m-%d", "en", Ok("1901-02-28")),
    ("2000-02-29T10:00:00", "+1y", "%Y-%m-%d", "en", Ok("2001-02-28")),
    ("1900-03-01T10:00:00", "", "%j", "en", Ok("060")),
    ("2000-03-01T10:00:00", "", "%j", "en", Ok("061")),
    ("2024-12-31T10:00:00", "", "%j", "en", Ok("366")),
    ("2026-12-31T10:00:00", "", "%j", "en", Ok("365")),
    ("2026-01-01T10:00:00", "", "%j", "en", Ok("001")),
    ("2026-03-18T14:05:09", "", "%j", "en", Ok("077")),
    ("2026-12-31T00:00:00", "+1d", "%Y-%m-%d", "en", Ok("2027-01-01")),
    ("2026-01-01T00:00:00", "-1d", "%Y-%m-%d", "en", Ok("2025-12-31")),
    ("2026-12-31T23:59:59", "+1s", "%Y-%m-%d %H:%M:%S", "en", Ok("2027-01-01 00:00:00")),
    ("2026-01-01T00:00:00", "-1s", "%Y-%m-%d %H:%M:%S", "en", Ok("2025-12-31 23:59:59")),
    ("2026-03-18T23:50:00", "+15min", "%Y-%m-%d %H:%M:%S", "en", Ok("2026-03-19 00:05:00")),
    ("2026-03-18T00:10:00", "-15min", "%Y-%m-%d %H:%M:%S", "en", Ok("2026-03-17 23:55:00")),
    ("2026-03-18T14:05:09", "+2h", "%H:%M", "en", Ok("16:05")),
    ("2026-03-18T22:00:00", "+3h", "%Y-%m-%d %H:%M:%S", "en", Ok("2026-03-19 01:00:00")),
    ("2026-03-18T14:05:09", "-25h", "%Y-%m-%d %H:%M:%S", "en", Ok("2026-03-17 13:05:09")),
    ("2026-03-18T14:05:09", "+1d +2h -30min +10s", "%Y-%m-%d %H:%M:%S", "en", Ok("2026-03-19 15:35:19")),
    ("2026-03-16T10:00:00", "+1bd", "%a %Y-%m-%d", "en", Ok("Tue 2026-03-17")),
    ("2026-03-16T10:00:00", "-1bd", "%a %Y-%m-%d", "en", Ok("Fri 2026-03-13")),
    ("2026-03-16T10:00:00", "+5bd", "%a %Y-%m-%d", "en", Ok("Mon 2026-03-23")),
    ("2026-03-16T10:00:00", "-5bd", "%a %Y-%m-%d", "en", Ok("Mon 2026-03-09")),
    ("2026-03-17T10:00:00", "+1bd", "%a %Y-%m-%d", "en", Ok("Wed 2026-03-18")),
    ("2026-03-17T10:00:00", "-1bd", "%a %Y-%m-%d", "en", Ok("Mon 2026-03-16")),
    ("2026-03-17T10:00:00", "+5bd", "%a %Y-%m-%d", "en", Ok("Tue 2026-03-24")),
    ("2026-03-17T10:00:00", "-5bd", "%a %Y-%m-%d", "en", Ok("Tue 2026-03-10")),
    ("2026-03-18T10:00:00", "+1bd", "%a %Y-%m-%d", "en", Ok("Thu 2026-03-19")),
    ("2026-03-18T10:00:00", "-1bd", "%a %Y-%m-%d", "en", Ok("Tue 2026-03-17")),
    ("2026-03-18T10:00:00", "+5bd", "%a %Y-%m-%d", "en", Ok("Wed 2026-03-25")),
    ("2026-03-18T10:00:00", "-5bd", "%a %Y-%m-%d", "en", Ok("Wed 2026-03-11")),
    ("2026-03-19T10:00:00", "+1bd", "%a %Y-%m-%d", "en", Ok("Fri 2026-03-20")),
    ("2026-03-19T10:00:00", "-1bd", "%a %Y-%m-%d", "en", Ok("Wed 2026-03-18")),
    ("2026-03-19T10:00:00", "+5bd", "%a %Y-%m-%d", "en", Ok("Thu 2026-03-26")),
    ("2026-03-19T10:00:00", "-5bd", "%a %Y-%m-%d", "en", Ok("Thu 2026-03-12")),
    ("2026-03-20T10:00:00", "+1bd", "%a %Y-%m-%d", "en", Ok("Mon 2026-03-23")),
    ("2026-03-20T10:00:00", "-1bd", "%a %Y-%m-%d", "en", Ok("Thu 2026-03-19")),
    ("2026-03-20T10:00:00", "+5bd", "%a %Y-%m-%d", "en", Ok("Fri 2026-03-27")),
    ("2026-03-20T10:00:00", "-5bd", "%a %Y-%m-%d", "en", Ok("Fri 2026-03-13")),
    ("2026-03-21T10:00:00", "+1bd", "%a %Y-%m-%d", "en", Ok("Mon 2026-03-23")),
    ("2026-03-21T10:00:00", "-1bd", "%a %Y-%m-%d", "en", Ok("Fri 2026-03-20")),
    ("2026-03-21T10:00:00", "+5bd", "%a %Y-%m-%d", "en", Ok("Fri 2026-03-27")),
    ("2026-03-21T10:00:00", "-5bd", "%a %Y-%m-%d", "en", Ok("Mon 2026-03-16")),
    ("2026-03-22T10:00:00", "+1bd", "%a %Y-%m-%d", "en", Ok("Mon 2026-03-23")),
    ("2026-03-22T10:00:00", "-1bd", "%a %Y-%m-%d", "en", Ok("Fri 2026-03-20")),
    ("2026-03-22T10:00:00", "+5bd", "%a %Y-%m-%d", "en", Ok("Fri 2026-03-27")),
    ("2026-03-22T10:00:00", "-5bd", "%a %Y-%m-%d", "en", Ok("Mon 2026-03-16")),
    ("2026-03-21T10:00:00", "+0bd", "%a %Y-%m-%d", "en", Ok("Sat 2026-03-21")),
    ("2026-03-22T10:00:00", "-0bd", "%a %Y-%m-%d", "en", Ok("Sun 2026-03-22")),
    ("2026-03-18T10:00:00", "+10bd", "%Y-%m-%d", "en", Ok("2026-04-01")),
    ("2026-03-18T10:00:00", "+3bd -1w", "%Y-%m-%d", "en", Ok("2026-03-16")),
    ("2026-03-18T10:00:00", "+520w", "%Y-%m-%d", "en", Ok("2036-03-05")),
    ("2026-03-18T10:00:00", "-520w", "%Y-%m-%d", "en", Ok("2016-03-30")),
    ("2026-03-18T10:00:00", "+9999d", "%Y-%m-%d", "en", Ok("2053-08-02")),
    ("2026-03-18T10:00:00", "+9999d +9999d", "%Y-%m-%d", "en", Ok("2080-12-17")),
    ("2026-03-18T10:00:00", "  +1d	+1d 
", "%Y-%m-%d", "en", Ok("2026-03-20")),
    ("2026-03-18T10:00:00", "", "%Y-%m-%d", "en", Ok("2026-03-18")),
    ("2026-03-18T10:00:00", "   ", "%Y-%m-%d", "en", Ok("2026-03-18")),
    ("2026-03-08T09:05:07", "", "%Y", "en", Ok("2026")),
    ("2026-03-08T09:05:07", "", "%y", "en", Ok("26")),
    ("2026-03-08T09:05:07", "", "%m", "en", Ok("03")),
    ("2026-03-08T09:05:07", "", "%-m", "en", Ok("3")),
    ("2026-03-08T09:05:07", "", "%d", "en", Ok("08")),
    ("2026-03-08T09:05:07", "", "%-d", "en", Ok("8")),
    ("2026-03-08T09:05:07", "", "%B", "en", Ok("March")),
    ("2026-03-08T09:05:07", "", "%b", "en", Ok("Mar")),
    ("2026-03-08T09:05:07", "", "%A", "en", Ok("Sunday")),
    ("2026-03-08T09:05:07", "", "%a", "en", Ok("Sun")),
    ("2026-03-08T09:05:07", "", "%H", "en", Ok("09")),
    ("2026-03-08T09:05:07", "", "%-H", "en", Ok("9")),
    ("2026-03-08T09:05:07", "", "%I", "en", Ok("09")),
    ("2026-03-08T09:05:07", "", "%-I", "en", Ok("9")),
    ("2026-03-08T09:05:07", "", "%M", "en", Ok("05")),
    ("2026-03-08T09:05:07", "", "%S", "en", Ok("07")),
    ("2026-03-08T09:05:07", "", "%p", "en", Ok("AM")),
    ("2026-03-08T09:05:07", "", "%j", "en", Ok("067")),
    ("2026-03-08T09:05:07", "", "%%", "en", Ok("%")),
    ("2026-03-08T09:05:07", "", "100%% %-m/%-d", "en", Ok("100% 3/8")),
    ("2026-03-08T09:05:07", "", "%Y%m%d-%H%M%S", "en", Ok("20260308-090507")),
    ("2026-03-08T09:05:07", "", "[%A, %B %-d]", "en", Ok("[Sunday, March 8]")),
    ("2026-03-08T09:05:07", "", "héllo %Y", "en", Ok("héllo 2026")),
    ("2026-03-18T00:30:00", "", "%I %p", "en", Ok("12 AM")),
    ("2026-03-18T00:30:00", "", "%-I %p", "en", Ok("12 AM")),
    ("2026-03-18T00:30:00", "", "%H %-H", "en", Ok("00 0")),
    ("2026-03-18T11:30:00", "", "%I %p", "en", Ok("11 AM")),
    ("2026-03-18T11:30:00", "", "%-I %p", "en", Ok("11 AM")),
    ("2026-03-18T11:30:00", "", "%H %-H", "en", Ok("11 11")),
    ("2026-03-18T12:30:00", "", "%I %p", "en", Ok("12 PM")),
    ("2026-03-18T12:30:00", "", "%-I %p", "en", Ok("12 PM")),
    ("2026-03-18T12:30:00", "", "%H %-H", "en", Ok("12 12")),
    ("2026-03-18T13:30:00", "", "%I %p", "en", Ok("01 PM")),
    ("2026-03-18T13:30:00", "", "%-I %p", "en", Ok("1 PM")),
    ("2026-03-18T13:30:00", "", "%H %-H", "en", Ok("13 13")),
    ("2026-03-18T23:30:00", "", "%I %p", "en", Ok("11 PM")),
    ("2026-03-18T23:30:00", "", "%-I %p", "en", Ok("11 PM")),
    ("2026-03-18T23:30:00", "", "%H %-H", "en", Ok("23 23")),
    ("2026-03-18T14:05:09", "", "%Y-%m-%d %H:%M:%S %j %p", "en", Ok("2026-03-18 14:05:09 077 PM")),
    ("2026-03-18T10:00:00", "", "%A %a %B %b", "en", Ok("Wednesday Wed March Mar")),
    ("2026-03-18T10:00:00", "", "%A %a %B %b", "es", Ok("miércoles mié marzo mar")),
    ("2026-03-18T10:00:00", "", "%A %a %B %b", "de", Ok("Mittwoch Mi März Mär")),
    ("2026-03-18T10:00:00", "", "%A %a %B %b", "fr", Ok("mercredi mer. mars mars")),
    ("2026-03-18T10:00:00", "", "%A %a %B %b", "de-AT", Ok("Mittwoch Mi März Mär")),
    ("2026-03-18T10:00:00", "", "%A %a %B %b", "DE", Ok("Mittwoch Mi März Mär")),
    ("2026-03-18T10:00:00", "", "%A %a %B %b", "fr-CA", Ok("mercredi mer. mars mars")),
    ("2026-03-18T10:00:00", "", "%A %a %B %b", "es_ES", Ok("Wednesday Wed March Mar")),
    ("2026-03-18T10:00:00", "", "%A %a %B %b", "pt", Ok("Wednesday Wed March Mar")),
    ("2026-03-18T10:00:00", "", "%A %a %B %b", "", Ok("Wednesday Wed March Mar")),
    ("2026-03-18T10:00:00", "", "%A %a %B %b", "zz-ZZ", Ok("Wednesday Wed March Mar")),
    ("2026-03-18T10:00:00", "", "%A %a %B %b", "EN-us", Ok("Wednesday Wed March Mar")),
    ("2026-08-02T10:00:00", "", "%A %a %B %b", "fr", Ok("dimanche dim. août août")),
    ("2026-08-02T10:00:00", "", "%A %a %B %b", "es", Ok("domingo dom agosto ago")),
    ("2026-08-02T10:00:00", "", "%A %a %B %b", "de", Ok("Sonntag So August Aug")),
    ("2026-03-18T10:00:00", "", "%A %-d de %B", "es", Ok("miércoles 18 de marzo")),
    ("0001-01-01T00:00:00", "", "%Y-%y %a %j", "en", Ok("0001-01 Mon 001")),
    ("0099-07-04T00:00:00", "", "%Y %y", "en", Ok("0099 99")),
    ("9999-12-31T23:59:59", "", "%Y-%m-%d %H:%M:%S %j", "en", Ok("9999-12-31 23:59:59 365")),
    ("0001-01-01T00:00:00", "-1d", "%Y-%m-%d", "en", Err(DateError::DateRange)),
    ("0001-01-01T00:00:00", "-1s", "%Y-%m-%d", "en", Err(DateError::DateRange)),
    ("0001-01-01T00:00:00", "-1mo", "%Y-%m-%d", "en", Err(DateError::DateRange)),
    ("0001-01-01T00:00:00", "-1y", "%Y-%m-%d", "en", Err(DateError::DateRange)),
    ("0001-01-01T00:00:00", "+1d", "%Y-%m-%d", "en", Ok("0001-01-02")),
    ("0001-01-01T00:00:00", "-1bd", "%Y-%m-%d", "en", Err(DateError::DateRange)),
    ("9999-12-31T23:59:59", "+1d", "%Y-%m-%d", "en", Err(DateError::DateRange)),
    ("9999-12-31T23:59:59", "+1s", "%Y-%m-%d", "en", Err(DateError::DateRange)),
    ("9999-12-31T23:59:59", "+1mo", "%Y-%m-%d", "en", Err(DateError::DateRange)),
    ("9999-12-31T23:59:59", "+1y", "%Y-%m-%d", "en", Err(DateError::DateRange)),
    ("9999-12-31T23:59:59", "-1y", "%Y-%m-%d", "en", Ok("9998-12-31")),
    ("9999-12-30T12:00:00", "+1d", "%Y-%m-%d", "en", Ok("9999-12-31")),
    ("9999-12-30T12:00:00", "+1d +1d", "%Y-%m-%d", "en", Err(DateError::DateRange)),
    ("9999-12-31T00:00:00", "+23h +59min +59s", "%Y-%m-%d %H:%M:%S", "en", Ok("9999-12-31 23:59:59")),
    ("2026-03-18T10:00:00", "+9999y", "%Y-%m-%d", "en", Err(DateError::DateRange)),
    ("2026-03-18T10:00:00", "-9999y", "%Y-%m-%d", "en", Err(DateError::DateRange)),
    ("0001-01-31T00:00:00", "+1mo", "%Y-%m-%d", "en", Ok("0001-02-28")),
    ("9999-12-31T00:00:00", "+1bd", "%Y-%m-%d", "en", Err(DateError::DateRange)),
    ("0001-01-01T00:00:00", "+1y -1y", "%Y-%m-%d", "en", Ok("0001-01-01")),
    ("9999-12-31T00:00:00", "+1d -1d", "%Y-%m-%d", "en", Err(DateError::DateRange)),
    ("2026-03-18T10:00:00", "", "%Q", "en", Err(DateError::BadFormat)),
    ("2026-03-18T10:00:00", "", "%", "en", Err(DateError::BadFormat)),
    ("2026-03-18T10:00:00", "", "100%", "en", Err(DateError::BadFormat)),
    ("2026-03-18T10:00:00", "", "%-", "en", Err(DateError::BadFormat)),
    ("2026-03-18T10:00:00", "", "%-y", "en", Err(DateError::BadFormat)),
    ("2026-03-18T10:00:00", "", "%-Y", "en", Err(DateError::BadFormat)),
    ("2026-03-18T10:00:00", "", "%E", "en", Err(DateError::BadFormat)),
    ("2026-03-18T10:00:00", "", "abc%", "en", Err(DateError::BadFormat)),
    ("2026-03-18T10:00:00", "", "%%%", "en", Err(DateError::BadFormat)),
    ("2026-03-18T10:00:00", "", "%-%", "en", Err(DateError::BadFormat)),
    ("2026-03-18T10:00:00", "", "% d", "en", Err(DateError::BadFormat)),
    ("2026-03-18T10:00:00", "", "%1", "en", Err(DateError::BadFormat)),
    ("2026-03-18T10:00:00", "", "", "en", Err(DateError::EmptyFormat)),
    ("2026-03-18T10:00:00", "+1m", "%Y-%m-%d", "en", Err(DateError::BadShift)),
    ("2026-03-18T10:00:00", "1d", "%Y-%m-%d", "en", Err(DateError::BadShift)),
    ("2026-03-18T10:00:00", "+d", "%Y-%m-%d", "en", Err(DateError::BadShift)),
    ("2026-03-18T10:00:00", "+12345d", "%Y-%m-%d", "en", Err(DateError::BadShift)),
    ("2026-03-18T10:00:00", "++1d", "%Y-%m-%d", "en", Err(DateError::BadShift)),
    ("2026-03-18T10:00:00", "tomorrow", "%Y-%m-%d", "en", Err(DateError::BadShift)),
    ("2026-03-18T10:00:00", "+1D", "%Y-%m-%d", "en", Err(DateError::BadShift)),
    ("2026-03-18T10:00:00", "+1", "%Y-%m-%d", "en", Err(DateError::BadShift)),
    ("2026-03-18T10:00:00", "-", "%Y-%m-%d", "en", Err(DateError::BadShift)),
    ("2026-03-18T10:00:00", "+", "%Y-%m-%d", "en", Err(DateError::BadShift)),
    ("2026-03-18T10:00:00", "+1d,", "%Y-%m-%d", "en", Err(DateError::BadShift)),
    ("2026-03-18T10:00:00", "+1 d", "%Y-%m-%d", "en", Err(DateError::BadShift)),
    ("2026-03-18T10:00:00", "+1dd", "%Y-%m-%d", "en", Err(DateError::BadShift)),
    ("2026-03-18T10:00:00", "+1md", "%Y-%m-%d", "en", Err(DateError::BadShift)),
    ("2026-03-18T10:00:00", "+1M", "%Y-%m-%d", "en", Err(DateError::BadShift)),
    ("2026-03-18T10:00:00", "+-1d", "%Y-%m-%d", "en", Err(DateError::BadShift)),
    ("2026-03-18T10:00:00", "+１d", "%Y-%m-%d", "en", Err(DateError::BadShift)),
    ("2026-03-18T10:00:00", "+1w2", "%Y-%m-%d", "en", Err(DateError::BadShift)),
    ("2026-03-18T10:00:00", "1", "%Y-%m-%d", "en", Err(DateError::BadShift)),
    ("2026-03-18T10:00:00", "+0", "%Y-%m-%d", "en", Err(DateError::BadShift)),
    ("2026-03-18T10:00:00", "+1H", "%Y-%m-%d", "en", Err(DateError::BadShift)),
    ("2026-03-18T10:00:00", "+1Min", "%Y-%m-%d", "en", Err(DateError::BadShift)),
    ("2026-03-18T10:00:00", "+1mo+1d", "%Y-%m-%d", "en", Err(DateError::BadShift)),
    ("2026-03-18T10:00:00", "+00001d", "%Y-%m-%d", "en", Err(DateError::BadShift)),
    ("2026-03-18T10:00:00", "+1d tomorrow", "%Y-%m-%d", "en", Err(DateError::BadShift)),
    ("2026-03-18T10:00:00", "+0d", "%Y-%m-%d", "en", Ok("2026-03-18")),
    ("2026-03-18T10:00:00", "-0d", "%Y-%m-%d", "en", Ok("2026-03-18")),
    ("2026-03-18T10:00:00", "+0mo", "%Y-%m-%d", "en", Ok("2026-03-18")),
    ("2026-03-18T10:00:00", "+0001d", "%Y-%m-%d", "en", Ok("2026-03-19")),
    ("2026-03-18T10:00:00", "+9999d", "%Y-%m-%d", "en", Ok("2053-08-02")),
    ("2026-03-18T10:00:00", "+5min +5h", "%Y-%m-%d", "en", Ok("2026-03-18")),
    ("2026-03-18T10:00:00", "+1w +1d", "%Y-%m-%d", "en", Ok("2026-03-26")),
    ("2026-03-18T10:00:00", "+1m", "%Q", "en", Err(DateError::BadFormat)),
];

#[test]
fn table() {
    assert!(CASES.len() >= 60, "need at least 60 cases");
    let mut failures = Vec::new();
    for (now, shift, fmt, locale, expected) in CASES {
        let got = run(now, shift, fmt, locale);
        let want = expected.map(str::to_owned);
        if got != want {
            failures.push(format!(
                "now={now:?} shift={shift:?} fmt={fmt:?} locale={locale:?}: got {got:?}, want {want:?}"
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "{} failures:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

#[test]
fn constructor_validation() {
    assert!(LocalDateTime::new(2026, 2, 29, 0, 0, 0).is_err());
    assert!(LocalDateTime::new(2024, 2, 29, 0, 0, 0).is_ok());
    assert!(LocalDateTime::new(0, 1, 1, 0, 0, 0).is_err());
    assert!(LocalDateTime::new(10000, 1, 1, 0, 0, 0).is_err());
    assert!(LocalDateTime::new(2026, 13, 1, 0, 0, 0).is_err());
    assert!(LocalDateTime::new(2026, 1, 1, 24, 0, 0).is_err());
    assert!(LocalDateTime::new(2026, 1, 1, 0, 60, 0).is_err());
    assert!(LocalDateTime::new(2026, 1, 1, 0, 0, 60).is_err());
    assert_eq!(
        LocalDateTime::parse_iso("2026-03-18"),
        Err(DateError::InvalidDate)
    );
    assert_eq!(
        LocalDateTime::parse_iso("2026-03-18 14:05:09"),
        Err(DateError::InvalidDate)
    );
    assert_eq!(
        LocalDateTime::parse_iso("2026-02-30T00:00:00"),
        Err(DateError::InvalidDate)
    );
    assert_eq!(
        LocalDateTime::parse_iso("20x6-03-18T00:00:00"),
        Err(DateError::InvalidDate)
    );
    assert!(DateError::BadFormat.to_string().len() > 3);
}
