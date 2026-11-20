// Copyright (c) 2026 The Botho Foundation

//! UTC formats used in bridge records, scanner state, exports and release
//! metadata.

use std::{
    cmp::Ordering,
    fmt::{self, Write},
};
use time::{Duration, UtcDateTime};

#[cfg(feature = "serde")]
mod serde_impl;

/// An instant in a Botho UTC record. The calendar and epoch arithmetic use
/// `time`. Historical records can also label the leap second following :59.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct UtcTimestamp {
    datetime: UtcDateTime,
    leap_second: bool,
}

impl UtcTimestamp {
    /// Current UTC instant from the system clock.
    pub fn now() -> Self {
        Self {
            datetime: UtcDateTime::now(),
            leap_second: false,
        }
    }

    /// Reopen an integer epoch from the database or a signed federation order.
    pub fn from_unix_seconds(seconds: i64) -> Option<Self> {
        let datetime = UtcDateTime::from_unix_timestamp(seconds).ok()?;
        Self::from_datetime(datetime, false)
    }

    fn from_datetime(datetime: UtcDateTime, leap_second: bool) -> Option<Self> {
        // Bounds of the original DateTime<Utc> representation in these records.
        (-262143..=262142)
            .contains(&datetime.year())
            .then_some(Self {
                datetime,
                leap_second,
            })
    }

    /// Unix seconds use the preceding :59 for an explicitly labeled leap
    /// second.
    pub fn timestamp(self) -> i64 {
        self.datetime.unix_timestamp()
    }

    /// Age for order expiry, retaining the leap-second convention of stored
    /// orders.
    pub fn elapsed(self) -> Duration {
        self.elapsed_at(UtcDateTime::now())
    }

    fn elapsed_at(self, now: UtcDateTime) -> Duration {
        let age = now - self.datetime;
        // The original elapsed-time calculation accounts separately for whole
        // calendar days and the two times of day. A labeled leap second adds
        // one second to the starting time until the end time passes its :59.
        if self.leap_second && now.as_hms() <= self.datetime.as_hms() {
            age - Duration::SECOND
        } else {
            age
        }
    }

    fn write_seconds(self, out: &mut impl Write, separator: char) -> fmt::Result {
        let (hour, minute, second) = self.datetime.as_hms();
        let second = if self.leap_second { 60 } else { second };
        write!(
            out,
            "{}{separator}{hour:02}:{minute:02}:{second:02}",
            self.datetime.date()
        )
    }

    fn write_fraction(self, out: &mut impl Write) -> fmt::Result {
        let nanos = self.datetime.nanosecond();
        if nanos != 0 {
            if nanos.is_multiple_of(1_000_000) {
                write!(out, ".{:03}", nanos / 1_000_000)?;
            } else if nanos.is_multiple_of(1_000) {
                write!(out, ".{:06}", nanos / 1_000)?;
            } else {
                write!(out, ".{nanos:09}")?;
            }
        }
        Ok(())
    }

    fn write_timestamp(self, out: &mut impl Write, separator: char, suffix: &str) -> fmt::Result {
        self.write_seconds(out, separator)?;
        self.write_fraction(out)?;
        out.write_str(suffix)
    }

    /// Offset spelling used by scanner deposits, benchmarks and testnet state.
    pub fn to_rfc3339(self) -> String {
        let mut result = String::new();
        self.write_timestamp(&mut result, 'T', "+00:00")
            .expect("write to String");
        result
    }

    /// Timestamp used by wallet exports and the scanner status display.
    pub fn export_label(self) -> String {
        let mut result = String::new();
        self.write_seconds(&mut result, ' ')
            .expect("write to String");
        result.push_str(" UTC");
        result
    }

    /// Whole-second UTC label for SOURCE_DATE_EPOCH and local builds.
    pub fn build_label(self) -> String {
        let mut result = String::new();
        self.write_seconds(&mut result, 'T')
            .expect("write to String");
        result.push('Z');
        result
    }
}

impl fmt::Display for UtcTimestamp {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.write_timestamp(formatter, ' ', " UTC")
    }
}

impl Ord for UtcTimestamp {
    fn cmp(&self, other: &Self) -> Ordering {
        // All fractional ticks of :59 precede every tick labeled :60.
        let key = |ts: &Self| (ts.timestamp(), ts.leap_second, ts.datetime.nanosecond());
        key(self).cmp(&key(other))
    }
}

impl PartialOrd for UtcTimestamp {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl fmt::Debug for UtcTimestamp {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.write_timestamp(formatter, 'T', "Z")
    }
}

#[cfg(all(test, feature = "serde"))]
mod tests {
    use super::*;
    use serde_json::{from_value, json, to_value};

    fn read(text: &str) -> UtcTimestamp {
        from_value(json!(text)).unwrap()
    }

    #[test]
    fn persisted_precision_and_offsets_round_trip() {
        for (input, canonical) in [
            ("1969-12-31T23:59:59Z", "1969-12-31T23:59:59Z"),
            ("2000-3-1T1:2:3.1+0130", "2000-02-29T23:32:03.100Z"),
            ("2000-01-01T00:00:00.1234Z", "2000-01-01T00:00:00.123400Z"),
            (
                "2000-01-01T00:00:00.0000001209Z",
                "2000-01-01T00:00:00.000000120Z",
            ),
            ("-0001-12-31 23:59:59 UTC", "-0001-12-31T23:59:59Z"),
            ("+10000-01-01T00:00:00Z", "+10000-01-01T00:00:00Z"),
            ("2017-01-01T00:59:60.12+01:00", "2016-12-31T23:59:60.120Z"),
        ] {
            let ts = read(input);
            assert_eq!(to_value(ts).unwrap(), json!(canonical));
            assert_eq!(read(canonical), ts);
            assert_eq!(ts.to_rfc3339(), canonical.replace('Z', "+00:00"));
        }
        let before_epoch = read("1970-01-01T00:59:59.000001+01:00");
        assert_eq!(before_epoch.timestamp(), -1);
        assert_eq!(before_epoch.export_label(), "1969-12-31 23:59:59 UTC");
        assert_eq!(before_epoch.build_label(), "1969-12-31T23:59:59Z");
        assert_eq!(before_epoch.to_string(), "1969-12-31 23:59:59.000001 UTC");
    }

    #[test]
    fn stored_leap_second_remains_distinct_from_adjacent_instants() {
        let before = read("2016-12-31T23:59:59.999999999Z");
        let leap = read("2016-12-31T23:59:60.25Z");
        let next = read("2017-01-01T00:00:00Z");
        assert!(before < leap && leap < next);
        assert_eq!(before.timestamp(), leap.timestamp());
        assert_eq!(
            leap.elapsed_at(before.datetime).whole_nanoseconds(),
            -250_000_001
        );
        // Historical arithmetic counts whole calendar days separately.
        assert_eq!(
            leap.elapsed_at(next.datetime).whole_nanoseconds(),
            -250_000_000
        );
    }

    #[test]
    fn historical_calendar_bounds_are_preserved() {
        let first = read("-262143-01-01T00:00:00Z");
        let last = read("+262142-12-31T23:59:59Z");
        for boundary in [first, last] {
            assert_eq!(
                UtcTimestamp::from_unix_seconds(boundary.timestamp()),
                Some(boundary)
            );
        }
        assert!(UtcTimestamp::from_unix_seconds(first.timestamp() - 1).is_none());
        assert!(UtcTimestamp::from_unix_seconds(last.timestamp() + 1).is_none());
        for text in [
            "1900-02-29T00:00:00Z",
            "2000-01-01T00:00:61Z",
            "2000-01-01T00:00:00+24:00",
            "2000-01-01T00:00:00",
        ] {
            assert!(from_value::<UtcTimestamp>(json!(text)).is_err());
        }
    }
}
