//! RFC 3339 (UTC, millisecond) formatting and parsing of note timestamps, without a date crate.

/// Smallest value written as RFC 3339 (0000-01-01T00:00:00.000Z).
const MIN_RFC: i64 = -62_167_219_200_000;
/// Largest value written as RFC 3339 (9999-12-31T23:59:59.999Z).
const MAX_RFC: i64 = 253_402_300_799_999;
const DAY_MS: i64 = 86_400_000;

/// Days since 1970-01-01 of a proleptic Gregorian date (Howard Hinnant's algorithm).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// Inverse of [`days_from_civil`].
fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}

fn is_leap(y: i64) -> bool {
    (y % 4 == 0 && y % 100 != 0) || y % 400 == 0
}

fn days_in_month(y: i64, m: i64) -> i64 {
    match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        _ if is_leap(y) => 29,
        _ => 28,
    }
}

/// Format milliseconds since the epoch as `YYYY-MM-DDTHH:MM:SS.mmmZ`. Values outside the years
/// 0000 to 9999 are written as a plain integer of milliseconds.
pub(crate) fn format(ms: i64) -> String {
    if !(MIN_RFC..=MAX_RFC).contains(&ms) {
        return ms.to_string();
    }
    let days = ms.div_euclid(DAY_MS);
    let rem = ms.rem_euclid(DAY_MS);
    let (y, m, d) = civil_from_days(days);
    let (h, mi, s, milli) = (
        rem / 3_600_000,
        rem / 60_000 % 60,
        rem / 1000 % 60,
        rem % 1000,
    );
    format!("{y:04}-{m:02}-{d:02}T{h:02}:{mi:02}:{s:02}.{milli:03}Z")
}

fn digits(s: &str, n: usize) -> Option<(i64, &str)> {
    let head = s.get(..n)?;
    if !head.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    Some((head.parse().ok()?, &s[n..]))
}

fn eat(s: &str, c: char) -> Option<&str> {
    s.strip_prefix(c)
}

/// Parse an RFC 3339 timestamp (`Z` or numeric offset, optional fraction, `T` or space), a plain
/// date (`YYYY-MM-DD`, midnight UTC) or an integer of milliseconds. Sub-millisecond digits are
/// truncated. Returns `None` if the text is none of these.
pub(crate) fn parse(text: &str) -> Option<i64> {
    let text = text.trim();
    if let Ok(n) = text.parse::<i64>() {
        return Some(n);
    }
    let (y, r) = digits(text, 4)?;
    let (m, r) = digits(eat(r, '-')?, 2)?;
    let (d, r) = digits(eat(r, '-')?, 2)?;
    if !(1..=12).contains(&m) || d < 1 || d > days_in_month(y, m) {
        return None;
    }
    let mut ms = days_from_civil(y, m, d) * DAY_MS;
    if r.is_empty() {
        return Some(ms);
    }
    let r = r.strip_prefix(['T', 't', ' '])?;
    let (h, r) = digits(r, 2)?;
    let (mi, r) = digits(eat(r, ':')?, 2)?;
    let (s, mut r) = digits(eat(r, ':')?, 2)?;
    if h > 23 || mi > 59 || s > 59 {
        return None;
    }
    ms += h * 3_600_000 + mi * 60_000 + s * 1000;
    if let Some(frac) = r.strip_prefix('.') {
        let n = frac.bytes().take_while(u8::is_ascii_digit).count();
        if n == 0 {
            return None;
        }
        let mut milli = 0;
        for (i, b) in frac.bytes().take(3).enumerate() {
            milli += i64::from(b - b'0') * [100, 10, 1][i];
        }
        ms += milli;
        r = &frac[n..];
    }
    match r {
        "Z" | "z" => Some(ms),
        _ => {
            let sign = match r.as_bytes().first()? {
                b'+' => 1,
                b'-' => -1,
                _ => return None,
            };
            let (oh, r) = digits(&r[1..], 2)?;
            let (om, r) = digits(eat(r, ':')?, 2)?;
            if !r.is_empty() || oh > 23 || om > 59 {
                return None;
            }
            Some(ms - sign * (oh * 3_600_000 + om * 60_000))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_values_format_as_rfc_3339() {
        assert_eq!(format(0), "1970-01-01T00:00:00.000Z");
        assert_eq!(format(1_700_000_000_123), "2023-11-14T22:13:20.123Z");
        assert_eq!(format(-1), "1969-12-31T23:59:59.999Z");
        assert_eq!(format(MAX_RFC), "9999-12-31T23:59:59.999Z");
        assert_eq!(format(MIN_RFC), "0000-01-01T00:00:00.000Z");
    }

    #[test]
    fn out_of_range_values_fall_back_to_integers() {
        for ms in [MAX_RFC + 1, MIN_RFC - 1, i64::MAX, i64::MIN] {
            assert_eq!(parse(&format(ms)), Some(ms));
        }
    }

    #[test]
    fn format_parse_round_trips() {
        for ms in [
            0,
            1,
            -1,
            951_782_400_000,
            1_709_251_199_999,
            MAX_RFC,
            MIN_RFC,
            -2_208_988_800_000,
        ] {
            assert_eq!(parse(&format(ms)), Some(ms), "{ms}");
        }
    }

    #[test]
    fn offsets_dates_and_fractions_are_understood() {
        assert_eq!(parse("2023-11-14"), Some(1_699_920_000_000));
        assert_eq!(
            parse("2023-11-14T23:13:20.123+01:00"),
            Some(1_700_000_000_123)
        );
        assert_eq!(parse("2023-11-14 22:13:20Z"), Some(1_700_000_000_000));
        assert_eq!(
            parse("2023-11-14T22:13:20.1239999Z"),
            Some(1_700_000_000_123)
        );
        assert_eq!(parse("12345"), Some(12345));
    }

    #[test]
    fn invalid_timestamps_are_rejected() {
        for bad in [
            "",
            "yesterday",
            "2023-13-01",
            "2023-02-29",
            "2023-11-14T24:00:00Z",
            "2023-11-14T22:13:20",
            "2023-11-14T22:13:20.Z",
            "2023-11-14T22:13:20+0100",
            "2023-11-14T22:13:20Zjunk",
        ] {
            assert_eq!(parse(bad), None, "{bad}");
        }
        assert!(parse("2024-02-29").is_some());
    }
}
