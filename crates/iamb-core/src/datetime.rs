//! Date math and formatting for the `{date}` and `{time}` macros (task 1.4).
//!
//! Everything here is pure calendar arithmetic on a wall-clock date and time without a time zone
//! (proleptic Gregorian, years 1 to 9999). The behaviour follows `docs/template-language.md` §4.1
//! and the reference implementation in `spec/template/reference.py`.

use std::fmt;

const SECS_PER_DAY: i64 = 86_400;
const MAX_YEAR: i32 = 9999;

/// Errors from parsing or applying formats, shifts and dates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DateError {
    /// A format string has an unknown `%` specifier or a trailing lone `%`.
    BadFormat,
    /// A shift string is not a list of valid shift terms.
    BadShift,
    /// A result lies outside the years 1 to 9999.
    DateRange,
    /// A format string is empty.
    EmptyFormat,
    /// A date or time component is out of range, or an ISO string is malformed.
    InvalidDate,
}

impl fmt::Display for DateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::BadFormat => "unknown format specifier",
            Self::BadShift => "invalid shift",
            Self::DateRange => "date outside years 1 to 9999",
            Self::EmptyFormat => "empty format",
            Self::InvalidDate => "invalid date or time",
        })
    }
}

impl std::error::Error for DateError {}

/// Day of the week, Monday first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Weekday {
    /// Monday.
    Monday,
    /// Tuesday.
    Tuesday,
    /// Wednesday.
    Wednesday,
    /// Thursday.
    Thursday,
    /// Friday.
    Friday,
    /// Saturday.
    Saturday,
    /// Sunday.
    Sunday,
}

impl Weekday {
    const ALL: [Weekday; 7] = [
        Weekday::Monday,
        Weekday::Tuesday,
        Weekday::Wednesday,
        Weekday::Thursday,
        Weekday::Friday,
        Weekday::Saturday,
        Weekday::Sunday,
    ];

    /// True for Saturday and Sunday.
    #[must_use]
    pub fn is_weekend(self) -> bool {
        matches!(self, Self::Saturday | Self::Sunday)
    }

    fn index(self) -> usize {
        self as usize
    }
}

/// Whether `year` is a leap year in the Gregorian calendar.
fn is_leap(year: i32) -> bool {
    (year % 4 == 0 && year % 100 != 0) || year % 400 == 0
}

fn days_in_month(year: i32, month: u8) -> u8 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        _ if is_leap(year) => 29,
        _ => 28,
    }
}

/// Days since 1970-01-01 for a proleptic Gregorian date (days-from-civil algorithm).
///
/// The date is not validated; callers pass a real calendar date.
#[must_use]
pub fn days_from_civil(year: i32, month: u8, day: u8) -> i64 {
    let y = i64::from(year) - i64::from(month <= 2);
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let m = i64::from(month);
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + i64::from(day) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// Inverse of [`days_from_civil`]: `(year, month, day)` for days since 1970-01-01.
#[must_use]
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
pub fn civil_from_days(days: i64) -> (i32, u8, u8) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    (y as i32, m as u8, d as u8)
}

/// A calendar date and wall-clock time without a time zone, years 1 to 9999.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct LocalDateTime {
    year: i32,
    month: u8,
    day: u8,
    hour: u8,
    minute: u8,
    second: u8,
}

impl LocalDateTime {
    /// Builds a date and time, validating every component.
    ///
    /// # Errors
    /// [`DateError::InvalidDate`] when a component is out of range or the day does not exist.
    pub fn new(
        year: i32,
        month: u8,
        day: u8,
        hour: u8,
        minute: u8,
        second: u8,
    ) -> Result<Self, DateError> {
        if !(1..=MAX_YEAR).contains(&year)
            || !(1..=12).contains(&month)
            || day < 1
            || day > days_in_month(year, month)
            || hour > 23
            || minute > 59
            || second > 59
        {
            return Err(DateError::InvalidDate);
        }
        Ok(Self {
            year,
            month,
            day,
            hour,
            minute,
            second,
        })
    }

    /// Parses `YYYY-MM-DDTHH:MM:SS`.
    ///
    /// # Errors
    /// [`DateError::InvalidDate`] when the text is malformed or a component is out of range.
    pub fn parse_iso(s: &str) -> Result<Self, DateError> {
        let b = s.as_bytes();
        if b.len() != 19
            || b[4] != b'-'
            || b[7] != b'-'
            || b[10] != b'T'
            || b[13] != b':'
            || b[16] != b':'
        {
            return Err(DateError::InvalidDate);
        }
        let num = |range: std::ops::Range<usize>| -> Result<u32, DateError> {
            let part = &b[range];
            if part.iter().all(u8::is_ascii_digit) {
                Ok(part.iter().fold(0, |acc, d| acc * 10 + u32::from(d - b'0')))
            } else {
                Err(DateError::InvalidDate)
            }
        };
        let small = |v: u32| u8::try_from(v).map_err(|_| DateError::InvalidDate);
        Self::new(
            i32::try_from(num(0..4)?).map_err(|_| DateError::InvalidDate)?,
            small(num(5..7)?)?,
            small(num(8..10)?)?,
            small(num(11..13)?)?,
            small(num(14..16)?)?,
            small(num(17..19)?)?,
        )
    }

    /// Year, 1 to 9999.
    #[must_use]
    pub fn year(&self) -> i32 {
        self.year
    }

    /// Month, 1 to 12.
    #[must_use]
    pub fn month(&self) -> u8 {
        self.month
    }

    /// Day of the month, 1 to 31.
    #[must_use]
    pub fn day(&self) -> u8 {
        self.day
    }

    /// Hour, 0 to 23.
    #[must_use]
    pub fn hour(&self) -> u8 {
        self.hour
    }

    /// Minute, 0 to 59.
    #[must_use]
    pub fn minute(&self) -> u8 {
        self.minute
    }

    /// Second, 0 to 59.
    #[must_use]
    pub fn second(&self) -> u8 {
        self.second
    }

    fn days(&self) -> i64 {
        days_from_civil(self.year, self.month, self.day)
    }

    /// Day of the week.
    #[must_use]
    pub fn weekday(&self) -> Weekday {
        // 1970-01-01 was a Thursday (index 3).
        let idx = (self.days() + 3).rem_euclid(7);
        Weekday::ALL[usize::try_from(idx).unwrap_or(0)]
    }

    /// Day of the year, 1-based (1 to 366).
    #[must_use]
    pub fn ordinal(&self) -> u16 {
        let d = self.days() - days_from_civil(self.year, 1, 1) + 1;
        u16::try_from(d).unwrap_or(0)
    }

    /// Builds from days since 1970-01-01 and seconds of day, checking the year range.
    fn from_parts(days: i64, secs: i64) -> Result<Self, DateError> {
        let min = days_from_civil(1, 1, 1);
        let max = days_from_civil(MAX_YEAR, 12, 31);
        if days < min || days > max {
            return Err(DateError::DateRange);
        }
        let (year, month, day) = civil_from_days(days);
        let narrow = |v: i64| u8::try_from(v).map_err(|_| DateError::InvalidDate);
        Ok(Self {
            year,
            month,
            day,
            hour: narrow(secs / 3600)?,
            minute: narrow(secs % 3600 / 60)?,
            second: narrow(secs % 60)?,
        })
    }

    fn secs_of_day(&self) -> i64 {
        i64::from(self.hour) * 3600 + i64::from(self.minute) * 60 + i64::from(self.second)
    }

    fn add_seconds(self, delta: i64) -> Result<Self, DateError> {
        let total = self
            .days()
            .checked_mul(SECS_PER_DAY)
            .and_then(|s| s.checked_add(self.secs_of_day()))
            .and_then(|s| s.checked_add(delta))
            .ok_or(DateError::DateRange)?;
        Self::from_parts(
            total.div_euclid(SECS_PER_DAY),
            total.rem_euclid(SECS_PER_DAY),
        )
    }

    fn add_days(self, delta: i64) -> Result<Self, DateError> {
        let days = self.days().checked_add(delta).ok_or(DateError::DateRange)?;
        Self::from_parts(days, self.secs_of_day())
    }

    fn add_months(self, months: i64) -> Result<Self, DateError> {
        let idx = i64::from(self.year) * 12 + i64::from(self.month) - 1 + months;
        let year = idx.div_euclid(12);
        if !(1..=i64::from(MAX_YEAR)).contains(&year) {
            return Err(DateError::DateRange);
        }
        let year = i32::try_from(year).map_err(|_| DateError::DateRange)?;
        let month = u8::try_from(idx.rem_euclid(12) + 1).map_err(|_| DateError::DateRange)?;
        let day = self.day.min(days_in_month(year, month));
        Ok(Self {
            year,
            month,
            day,
            ..self
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Unit {
    Day,
    Week,
    Month,
    Year,
    BusinessDay,
    Hour,
    Minute,
    Second,
}

const UNITS: [(&str, Unit); 8] = [
    ("min", Unit::Minute),
    ("mo", Unit::Month),
    ("bd", Unit::BusinessDay),
    ("d", Unit::Day),
    ("w", Unit::Week),
    ("y", Unit::Year),
    ("h", Unit::Hour),
    ("s", Unit::Second),
];

/// A parsed list of shift terms such as `+1mo -1d`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Shift {
    terms: Vec<(i64, Unit)>,
}

impl Shift {
    /// Parses shift terms separated by white space. An empty or blank string has no terms.
    ///
    /// # Errors
    /// [`DateError::BadShift`] when a term is not a sign, one to four ASCII digits and a unit
    /// (`d w mo y bd h min s`).
    pub fn parse(s: &str) -> Result<Shift, DateError> {
        let mut terms = Vec::new();
        for term in s.split_whitespace() {
            let sign = match term.as_bytes().first() {
                Some(b'+') => 1,
                Some(b'-') => -1,
                _ => return Err(DateError::BadShift),
            };
            let body = &term[1..];
            let (suffix, unit) = UNITS
                .iter()
                .find(|(u, _)| body.ends_with(u))
                .ok_or(DateError::BadShift)?;
            let digits = &body[..body.len() - suffix.len()];
            if digits.is_empty() || digits.len() > 4 || !digits.bytes().all(|b| b.is_ascii_digit())
            {
                return Err(DateError::BadShift);
            }
            let n: i64 = digits.parse().map_err(|_| DateError::BadShift)?;
            terms.push((sign * n, *unit));
        }
        Ok(Shift { terms })
    }

    /// Applies the terms left to right.
    ///
    /// # Errors
    /// [`DateError::DateRange`] when any intermediate or final result leaves years 1 to 9999.
    pub fn apply(&self, t: LocalDateTime) -> Result<LocalDateTime, DateError> {
        let mut t = t;
        for &(n, unit) in &self.terms {
            t = match unit {
                Unit::Day => t.add_days(n)?,
                Unit::Week => t.add_days(n * 7)?,
                Unit::Month => t.add_months(n)?,
                Unit::Year => t.add_months(n * 12)?,
                Unit::Hour => t.add_seconds(n * 3600)?,
                Unit::Minute => t.add_seconds(n * 60)?,
                Unit::Second => t.add_seconds(n)?,
                Unit::BusinessDay => t.add_days(business_day_offset(t.days(), n))?,
            };
        }
        Ok(t)
    }
}

/// The number of calendar days to move from day `days` (since 1970-01-01) to land `n`
/// business days away, in constant time.
///
/// Same result as stepping one calendar day at a time in the direction of `n` and counting
/// every Monday to Friday reached until `n` are counted (the reference implementation). Every
/// run of seven consecutive days holds exactly five weekdays, so all but the last one to five
/// business days are whole weeks; the rest is stepped, at most nine days. Moving is monotonic,
/// so the result leaves the year range exactly when some step of the reference would: the
/// caller's range check on the final day is enough.
fn business_day_offset(days: i64, n: i64) -> i64 {
    if n == 0 {
        return 0;
    }
    let step = n.signum();
    let count = n.abs();
    let weeks = (count - 1) / 5;
    let mut left = count - weeks * 5;
    let mut offset = weeks * 7 * step;
    while left > 0 {
        offset += step;
        // 1970-01-01 was a Thursday: index 3 with Monday = 0, so 5 and 6 are the weekend.
        if (days + offset + 3).rem_euclid(7) < 5 {
            left -= 1;
        }
    }
    offset
}

/// A locale for month and weekday names: `en`, `es`, `de` or `fr`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Locale {
    lang: Lang,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Lang {
    En,
    Es,
    De,
    Fr,
}

type Names = (
    [&'static str; 12],
    [&'static str; 12],
    [&'static str; 7],
    [&'static str; 7],
);

const EN: Names = (
    [
        "January",
        "February",
        "March",
        "April",
        "May",
        "June",
        "July",
        "August",
        "September",
        "October",
        "November",
        "December",
    ],
    [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ],
    [
        "Monday",
        "Tuesday",
        "Wednesday",
        "Thursday",
        "Friday",
        "Saturday",
        "Sunday",
    ],
    ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"],
);
const ES: Names = (
    [
        "enero",
        "febrero",
        "marzo",
        "abril",
        "mayo",
        "junio",
        "julio",
        "agosto",
        "septiembre",
        "octubre",
        "noviembre",
        "diciembre",
    ],
    [
        "ene", "feb", "mar", "abr", "may", "jun", "jul", "ago", "sep", "oct", "nov", "dic",
    ],
    [
        "lunes",
        "martes",
        "miércoles",
        "jueves",
        "viernes",
        "sábado",
        "domingo",
    ],
    ["lun", "mar", "mié", "jue", "vie", "sáb", "dom"],
);
const DE: Names = (
    [
        "Januar",
        "Februar",
        "März",
        "April",
        "Mai",
        "Juni",
        "Juli",
        "August",
        "September",
        "Oktober",
        "November",
        "Dezember",
    ],
    [
        "Jan", "Feb", "Mär", "Apr", "Mai", "Jun", "Jul", "Aug", "Sep", "Okt", "Nov", "Dez",
    ],
    [
        "Montag",
        "Dienstag",
        "Mittwoch",
        "Donnerstag",
        "Freitag",
        "Samstag",
        "Sonntag",
    ],
    ["Mo", "Di", "Mi", "Do", "Fr", "Sa", "So"],
);
const FR: Names = (
    [
        "janvier",
        "février",
        "mars",
        "avril",
        "mai",
        "juin",
        "juillet",
        "août",
        "septembre",
        "octobre",
        "novembre",
        "décembre",
    ],
    [
        "janv.", "févr.", "mars", "avr.", "mai", "juin", "juil.", "août", "sept.", "oct.", "nov.",
        "déc.",
    ],
    [
        "lundi", "mardi", "mercredi", "jeudi", "vendredi", "samedi", "dimanche",
    ],
    ["lun.", "mar.", "mer.", "jeu.", "ven.", "sam.", "dim."],
);

impl Locale {
    /// Picks a locale from a tag such as `de-AT`: the region is ignored, matching is
    /// case-insensitive, and unknown languages use English.
    #[must_use]
    pub fn from_tag(tag: &str) -> Locale {
        let lang = tag.split('-').next().unwrap_or("").to_lowercase();
        let lang = match lang.as_str() {
            "es" => Lang::Es,
            "de" => Lang::De,
            "fr" => Lang::Fr,
            _ => Lang::En,
        };
        Locale { lang }
    }

    fn names(self) -> &'static Names {
        match self.lang {
            Lang::En => &EN,
            Lang::Es => &ES,
            Lang::De => &DE,
            Lang::Fr => &FR,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Spec {
    Year4,
    Year2,
    Month,
    MonthPlain,
    Day,
    DayPlain,
    MonthFull,
    MonthShort,
    WeekdayFull,
    WeekdayShort,
    Hour,
    HourPlain,
    Hour12,
    Hour12Plain,
    Minute,
    Second,
    Meridiem,
    OrdinalDay,
    Percent,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Part {
    Lit(String),
    Spec(Spec),
}

const SPECS: [(&str, Spec); 19] = [
    ("%-m", Spec::MonthPlain),
    ("%-d", Spec::DayPlain),
    ("%-H", Spec::HourPlain),
    ("%-I", Spec::Hour12Plain),
    ("%Y", Spec::Year4),
    ("%y", Spec::Year2),
    ("%m", Spec::Month),
    ("%d", Spec::Day),
    ("%B", Spec::MonthFull),
    ("%b", Spec::MonthShort),
    ("%A", Spec::WeekdayFull),
    ("%a", Spec::WeekdayShort),
    ("%H", Spec::Hour),
    ("%I", Spec::Hour12),
    ("%M", Spec::Minute),
    ("%S", Spec::Second),
    ("%p", Spec::Meridiem),
    ("%j", Spec::OrdinalDay),
    ("%%", Spec::Percent),
];

/// A parsed format string such as `%Y-%m-%d`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Format {
    parts: Vec<Part>,
}

impl Format {
    /// Parses a format string.
    ///
    /// # Errors
    /// [`DateError::EmptyFormat`] for an empty string, [`DateError::BadFormat`] for an unknown
    /// `%` specifier or a trailing lone `%`.
    pub fn parse(s: &str) -> Result<Format, DateError> {
        if s.is_empty() {
            return Err(DateError::EmptyFormat);
        }
        let mut parts = Vec::new();
        let mut rest = s;
        while !rest.is_empty() {
            if rest.starts_with('%') {
                let (spec, len) = SPECS
                    .iter()
                    .find(|(p, _)| rest.starts_with(p))
                    .map(|(p, sp)| (*sp, p.len()))
                    .ok_or(DateError::BadFormat)?;
                parts.push(Part::Spec(spec));
                rest = &rest[len..];
            } else {
                let end = rest.find('%').unwrap_or(rest.len());
                parts.push(Part::Lit(rest[..end].to_owned()));
                rest = &rest[end..];
            }
        }
        Ok(Format { parts })
    }

    /// Renders `t` using the month and weekday names of `locale`.
    #[must_use]
    pub fn render(&self, t: &LocalDateTime, locale: &Locale) -> String {
        let (months, months_short, days, days_short) = locale.names();
        let wd = t.weekday().index();
        let mi = usize::from(t.month - 1);
        let h12 = (t.hour + 11) % 12 + 1;
        let mut out = String::new();
        for part in &self.parts {
            let spec = match part {
                Part::Lit(s) => {
                    out.push_str(s);
                    continue;
                }
                Part::Spec(spec) => *spec,
            };
            let text = match spec {
                Spec::Year4 => format!("{:04}", t.year),
                Spec::Year2 => format!("{:02}", t.year % 100),
                Spec::Month => format!("{:02}", t.month),
                Spec::MonthPlain => t.month.to_string(),
                Spec::Day => format!("{:02}", t.day),
                Spec::DayPlain => t.day.to_string(),
                Spec::MonthFull => months[mi].to_owned(),
                Spec::MonthShort => months_short[mi].to_owned(),
                Spec::WeekdayFull => days[wd].to_owned(),
                Spec::WeekdayShort => days_short[wd].to_owned(),
                Spec::Hour => format!("{:02}", t.hour),
                Spec::HourPlain => t.hour.to_string(),
                Spec::Hour12 => format!("{h12:02}"),
                Spec::Hour12Plain => h12.to_string(),
                Spec::Minute => format!("{:02}", t.minute),
                Spec::Second => format!("{:02}", t.second),
                Spec::Meridiem => (if t.hour < 12 { "AM" } else { "PM" }).to_owned(),
                Spec::OrdinalDay => format!("{:03}", t.ordinal()),
                Spec::Percent => "%".to_owned(),
            };
            out.push_str(&text);
        }
        out
    }
}
