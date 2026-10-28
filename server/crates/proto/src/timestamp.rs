// google.protobuf.Timestamp, written in JSON as RFC 3339 in UTC, as pbjson-types writes it: with
// 0, 3, 6 or 9 digits of fraction, as few as hold the nanoseconds.

use std::time::{SystemTime, UNIX_EPOCH};

use gem_json::{Error, FromJson, ToJson, Value};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Timestamp {
    pub seconds: i64,
    pub nanos: i32,
}

impl Timestamp {
    pub fn now() -> Timestamp {
        let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();
        Timestamp { seconds: now.as_secs() as i64, nanos: now.subsec_nanos() as i32 }
    }

    pub fn to_rfc3339(&self) -> String {
        let days = self.seconds.div_euclid(86_400);
        let secs = self.seconds.rem_euclid(86_400);
        let (year, month, day) = civil_from_days(days);
        let (h, m, s) = (secs / 3600, secs / 60 % 60, secs % 60);
        let fraction = match self.nanos {
            0 => String::new(),
            n if n % 1_000_000 == 0 => format!(".{:03}", n / 1_000_000),
            n if n % 1_000 == 0 => format!(".{:06}", n / 1_000),
            n => format!(".{n:09}"),
        };
        format!("{year:04}-{month:02}-{day:02}T{h:02}:{m:02}:{s:02}{fraction}Z")
    }

    // RFC 3339, with any offset and up to 9 digits of fraction.
    pub fn parse(text: &str) -> Option<Timestamp> {
        let b = text.as_bytes();
        let num = |range: std::ops::Range<usize>| -> Option<i64> {
            let part = text.get(range)?;
            if part.is_empty() || !part.bytes().all(|c| c.is_ascii_digit()) {
                return None;
            }
            part.parse().ok()
        };
        if b.len() < 20 || b[4] != b'-' || b[7] != b'-' || !matches!(b[10], b'T' | b't' | b' ') {
            return None;
        }
        if b[13] != b':' || b[16] != b':' {
            return None;
        }
        let (year, month, day) = (num(0..4)?, num(5..7)?, num(8..10)?);
        let (h, m, s) = (num(11..13)?, num(14..16)?, num(17..19)?);
        if !(1..=12).contains(&month) || !(1..=31).contains(&day) || h > 23 || m > 59 || s > 60 {
            return None;
        }
        let mut at = 19;
        let mut nanos = 0i64;
        if b[at] == b'.' {
            let start = at + 1;
            at = start;
            while at < b.len() && b[at].is_ascii_digit() {
                at += 1;
            }
            let digits = at - start;
            if digits == 0 || digits > 9 {
                return None;
            }
            nanos = num(start..at)? * 10i64.pow(9 - digits as u32);
        }
        let offset = match b.get(at)? {
            b'Z' | b'z' if at + 1 == b.len() => 0,
            sign @ (b'+' | b'-') if at + 6 == b.len() && b[at + 3] == b':' => {
                let minutes = num(at + 1..at + 3)? * 60 + num(at + 4..at + 6)?;
                if *sign == b'+' { minutes } else { -minutes }
            }
            _ => return None,
        };
        let seconds = days_from_civil(year, month, day) * 86_400 + h * 3600 + m * 60 + s - offset * 60;
        Some(Timestamp { seconds, nanos: nanos as i32 })
    }
}

// Howard Hinnant's algorithms, days since 1970-01-01 to and from a proleptic Gregorian date.
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (yoe + era * 400 + if m <= 2 { 1 } else { 0 }, m, d)
}

impl ToJson for Timestamp {
    fn to_json(&self) -> Value {
        Value::String(self.to_rfc3339())
    }
}

impl FromJson for Timestamp {
    fn from_json(value: &Value) -> Result<Timestamp, Error> {
        let text = value
            .as_str()
            .ok_or_else(|| Error::new(format!("expected an RFC 3339 timestamp, found {}", value.kind())))?;
        Timestamp::parse(text).ok_or_else(|| Error::new(format!("invalid timestamp {text:?}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips() {
        for text in ["1970-01-01T00:00:00Z", "2026-10-28T20:38:33.123Z", "1999-12-31T23:59:59.000001Z", "2024-02-29T12:00:00.000000001Z"] {
            assert_eq!(Timestamp::parse(text).unwrap().to_rfc3339(), text);
        }
        assert_eq!(Timestamp::parse("2026-10-28T22:38:33+02:00"), Timestamp::parse("2026-10-28T20:38:33Z"));
        assert_eq!(Timestamp::parse("1969-12-31T23:59:59Z").unwrap().seconds, -1);
        assert!(Timestamp::parse("2026-13-01T00:00:00Z").is_none());
        assert!(Timestamp::parse("2026-10-28").is_none());
    }
}
