// Copyright (c) 2026 The Botho Foundation

use super::UtcTimestamp;
use regex::{Captures, Regex};
use serde::{de, Deserialize, Deserializer, Serialize, Serializer};
use std::{fmt, sync::LazyLock};
use time::{Date, Duration, Month, Time, UtcDateTime};

// Older bridge records accept signed extended years, unpadded components,
// spaces between components, UTC or Z, and offsets with or without a colon.
// Capture the civil fields first. Calendar validation and offset arithmetic
// below use time, including across day and year boundaries.
static RECORD: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(concat!(
        r"(?x)\A\s*(?P<year>[+-][0-9]+|[0-9]{1,4})\s*-\s*",
        r"(?P<month>[0-9]{1,2})\s*-\s*(?P<day>[0-9]{1,2})[Tt\x20]\s*",
        r"(?P<hour>[0-9]{1,2})\s*:\s*(?P<minute>[0-9]{1,2})\s*:\s*",
        r"(?P<second>[0-9]{1,2})(?:\.(?P<fraction>[0-9]+))?\s*",
        r"(?:(?i:UTC|Z)|(?P<sign>[-+−])(?P<oh>[0-9]{2})[:\s]*(?P<om>[0-9]{2}))\s*\z"
    ))
    .expect("UTC record format")
});

fn component(captures: &Captures<'_>, name: &str) -> Option<u8> {
    captures.name(name)?.as_str().parse().ok()
}

fn parse_record(text: &str) -> Option<UtcTimestamp> {
    let fields = RECORD.captures(text)?;
    let year = fields["year"].parse::<i32>().ok()?;
    let month = Month::try_from(component(&fields, "month")?).ok()?;
    let date = Date::from_calendar_date(year, month, component(&fields, "day")?).ok()?;
    let second = component(&fields, "second")?;
    let leap = second == 60;
    // Fractions finer than a nanosecond were truncated on input.
    let nanos = match fields.name("fraction") {
        Some(fraction) => {
            let digits = &fraction.as_str()[..fraction.len().min(9)];
            digits.parse::<u32>().ok()? * 10_u32.pow(9 - digits.len() as u32)
        }
        None => 0,
    };
    let wall = Time::from_hms_nano(
        component(&fields, "hour")?,
        component(&fields, "minute")?,
        if leap { 59 } else { second },
        nanos,
    )
    .ok()?;
    let mut utc = UtcDateTime::new(date, wall);
    UtcTimestamp::from_datetime(utc, leap)?;
    if let Some(sign) = fields.name("sign") {
        let hour = component(&fields, "oh")?;
        let minute = component(&fields, "om")?;
        if hour > 23 || minute > 59 {
            return None;
        }
        let offset = Duration::minutes(i64::from(hour) * 60 + i64::from(minute));
        utc = if sign.as_str() == "+" {
            utc.checked_sub(offset)?
        } else {
            utc.checked_add(offset)?
        };
    }
    UtcTimestamp::from_datetime(utc, leap)
}

impl Serialize for UtcTimestamp {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        // The historical Serde spelling uses the UTC designator and nanosecond groups.
        let mut text = String::with_capacity(32);
        self.write_timestamp(&mut text, 'T', "Z")
            .expect("write to String");
        serializer.serialize_str(&text)
    }
}

impl<'de> Deserialize<'de> for UtcTimestamp {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Visitor;
        impl de::Visitor<'_> for Visitor {
            type Value = UtcTimestamp;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a UTC timestamp with a timezone offset")
            }

            fn visit_str<E: de::Error>(self, text: &str) -> Result<Self::Value, E> {
                parse_record(text).ok_or_else(|| E::invalid_value(de::Unexpected::Str(text), &self))
            }
        }
        deserializer.deserialize_str(Visitor)
    }
}
