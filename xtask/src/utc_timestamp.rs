//! 系统 UTC 时刻的文本输出与历史 RFC3339 时间比较。

use std::borrow::Cow;

use time::{OffsetDateTime, UtcOffset, format_description::well_known::Rfc3339};

enum Precision {
    AutoSi,
    Micros,
    SecondsOrMicros,
}

pub(crate) fn rfc3339(value: OffsetDateTime) -> String {
    encode(value, Precision::AutoSi, "+00:00")
}

pub(crate) fn now_rfc3339() -> String {
    rfc3339(time::OffsetDateTime::now_utc())
}

pub(crate) fn rfc3339_order_key(value: &str) -> crate::Result<(i64, u32)> {
    let bytes = value.as_bytes();
    if !matches!(bytes.get(10), Some(b'T' | b't' | b' ')) {
        return Err("RFC3339 日期时间分隔符无效".into());
    }
    let suffix = value.get(19..).ok_or("RFC3339 时间字段不完整")?;
    let offset = match suffix.strip_prefix('.') {
        Some(fraction) => fraction.trim_start_matches(|ch: char| ch.is_ascii_digit()),
        None => suffix,
    };
    let leap_second = bytes.get(17..19) == Some(b"60");
    let mut input = Cow::Borrowed(value);
    if leap_second {
        // 保留旧比较中秒 59 上额外的十亿纳秒及原有小数精度。
        input.to_mut().replace_range(17..19, "59");
    }
    if offset.starts_with('−') {
        let start = value.len() - offset.len();
        input
            .to_mut()
            .replace_range(start..start + '−'.len_utf8(), "-");
    }
    let parsed = OffsetDateTime::parse(&input, &Rfc3339)?;
    Ok((
        parsed.unix_timestamp(),
        parsed.nanosecond() + u32::from(leap_second) * 1_000_000_000,
    ))
}

pub(crate) fn unix_timestamp_millis(value: OffsetDateTime) -> i128 {
    value.unix_timestamp_nanos().div_euclid(1_000_000)
}

pub(crate) fn rfc3339_micros(value: OffsetDateTime) -> String {
    encode(value, Precision::Micros, "+00:00")
}

pub(crate) fn rfc3339_micros_z(value: OffsetDateTime) -> String {
    encode(value, Precision::Micros, "Z")
}

pub(crate) fn rfc3339_seconds_or_micros(value: OffsetDateTime) -> String {
    encode(value, Precision::SecondsOrMicros, "+00:00")
}

fn encode(value: OffsetDateTime, precision: Precision, suffix: &str) -> String {
    let utc = value.to_offset(UtcOffset::UTC);
    let mut encoded = format!(
        "{}-{:02}-{:02}T{:02}:{:02}:{:02}",
        year(utc.year()),
        utc.month() as u8,
        utc.day(),
        utc.hour(),
        utc.minute(),
        utc.second(),
    );
    let nanos = utc.nanosecond();
    let fraction = match precision {
        Precision::AutoSi if nanos == 0 => None,
        Precision::AutoSi => Some(if nanos.is_multiple_of(1_000_000) {
            (nanos / 1_000_000, 3)
        } else if nanos.is_multiple_of(1_000) {
            (nanos / 1_000, 6)
        } else {
            (nanos, 9)
        }),
        Precision::Micros => Some((nanos / 1_000, 6)),
        Precision::SecondsOrMicros if nanos / 1_000 == 0 => None,
        Precision::SecondsOrMicros => Some((nanos / 1_000, 6)),
    };
    if let Some((fraction, digits)) = fraction {
        encoded.push_str(&format!(".{fraction:0digits$}"));
    }
    encoded.push_str(suffix);
    encoded
}

pub(crate) fn migration_name(value: OffsetDateTime) -> String {
    let utc = value.to_offset(UtcOffset::UTC);
    format!(
        "{}{:02}{:02}_{:02}{:02}{:02}",
        year(utc.year()),
        utc.month() as u8,
        utc.day(),
        utc.hour(),
        utc.minute(),
        utc.second(),
    )
}

pub(crate) fn s3_signing_timestamp(value: OffsetDateTime) -> String {
    let utc = value.to_offset(UtcOffset::UTC);
    format!(
        "{}{:02}{:02}T{:02}{:02}{:02}Z",
        year(utc.year()),
        utc.month() as u8,
        utc.day(),
        utc.hour(),
        utc.minute(),
        utc.second(),
    )
}

pub(crate) fn run_directory_parts(value: OffsetDateTime) -> (String, String) {
    let utc = value.to_offset(UtcOffset::UTC);
    let year = year(utc.year());
    let date = format!("{year}-{:02}-{:02}", utc.month() as u8, utc.day());
    let stamp = format!(
        "{year}{:02}{:02}T{:02}{:02}{:02}{:09}Z",
        utc.month() as u8,
        utc.day(),
        utc.hour(),
        utc.minute(),
        utc.second(),
        utc.nanosecond(),
    );
    (date, stamp)
}

fn year(value: i32) -> String {
    if (0..=9999).contains(&value) {
        format!("{value:04}")
    } else {
        format!("{value:+05}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::{Date, Month};

    #[test]
    fn rfc3339_order_key_preserves_chrono_syntax_precision_and_order() {
        for (text, expected) in [
            ("1970-01-01t00:00:00z", (0, 0)),
            ("1969-12-31 22:30:00-01:30", (0, 0)),
            ("1970-01-01T01:30:00.123456789987+01:30", (0, 123_456_789)),
            ("1969-12-31t22:30:00.123456789987−01:30", (0, 123_456_789)),
            ("1970-01-01T23:59:00+23:59", (0, 0)),
            ("1969-12-31T23:59:59.999999999Z", (-1, 999_999_999)),
            ("1969-12-31T23:59:60.01Z", (-1, 1_010_000_000)),
            ("1970-01-01T00:01:60.25+00:02", (-1, 1_250_000_000)),
            ("1969-12-31T23:58:60.25−00:01", (-1, 1_250_000_000)),
            ("1970-01-01T00:01:60.25Z", (119, 1_250_000_000)),
            ("0000-01-01T00:00:00+01:00", (-62_167_222_800, 0)),
            ("9999-12-31T23:59:59-01:00", (253_402_304_399, 0)),
        ] {
            let old = chrono::DateTime::parse_from_rfc3339(text).unwrap();
            assert_eq!((old.timestamp(), old.timestamp_subsec_nanos()), expected);
            assert_eq!(rfc3339_order_key(text).unwrap(), expected);
        }
        let ordered = [
            "1969-12-31T23:59:59.999999999Z",
            "1969-12-31T23:59:60.01Z",
            "1970-01-01T00:01:60.25+00:02",
            "1970-01-01t00:00:00z",
        ];
        assert!(ordered.windows(2).all(|pair| {
            rfc3339_order_key(pair[0]).unwrap() < rfc3339_order_key(pair[1]).unwrap()
        }));
        for invalid in [
            "1970-01-01X00:00:00Z",
            "1970-01-01\t00:00:00Z",
            "1970-01-01T00:00:00.−01:30",
            "1970-01-01T00:00:00+24:00",
            "1970-01-01T00:00:00−01:60",
            "1970−01-01T00:00:00Z",
        ] {
            assert!(
                chrono::DateTime::parse_from_rfc3339(invalid).is_err(),
                "{invalid}"
            );
            assert!(rfc3339_order_key(invalid).is_err(), "{invalid}");
        }
    }

    #[test]
    fn utc_text_keeps_zero_millisecond_microsecond_and_nanosecond_precision() {
        for (nanos, fraction) in [
            (0, ""),
            (1_000_000, ".001"),
            (120_000_000, ".120"),
            (1_000, ".000001"),
            (123_400_000, ".123400"),
            (123_456_000, ".123456"),
            (1, ".000000001"),
            (123_456_789, ".123456789"),
            (999_999_999, ".999999999"),
        ] {
            let value = OffsetDateTime::UNIX_EPOCH
                .replace_nanosecond(nanos)
                .unwrap();
            assert_eq!(
                rfc3339(value),
                format!("1970-01-01T00:00:00{fraction}+00:00")
            );
            assert_eq!(migration_name(value), "19700101_000000");
        }
    }

    #[test]
    fn microsecond_precision_truncates_and_omits_fraction_only_when_micros_are_zero() {
        for (nanos, fixed, optional) in [
            (0, ".000000", ""),
            (1, ".000000", ""),
            (999, ".000000", ""),
            (1_000, ".000001", ".000001"),
            (1_999, ".000001", ".000001"),
            (999_999, ".000999", ".000999"),
            (1_000_000, ".001000", ".001000"),
            (120_000_000, ".120000", ".120000"),
            (123_456_789, ".123456", ".123456"),
            (999_999_999, ".999999", ".999999"),
        ] {
            let value = OffsetDateTime::UNIX_EPOCH
                .replace_nanosecond(nanos)
                .unwrap();
            assert_eq!(
                rfc3339_micros(value),
                format!("1970-01-01T00:00:00{fixed}+00:00")
            );
            assert_eq!(
                rfc3339_seconds_or_micros(value),
                format!("1970-01-01T00:00:00{optional}+00:00")
            );
        }
    }

    #[test]
    fn run_directory_stamp_always_has_nine_fraction_digits() {
        for (nanos, expected) in [
            (0, "000000000"),
            (1, "000000001"),
            (999, "000000999"),
            (1_000, "000001000"),
            (1_000_000, "001000000"),
            (120_000_000, "120000000"),
            (123_456_789, "123456789"),
            (999_999_999, "999999999"),
        ] {
            let value = OffsetDateTime::UNIX_EPOCH
                .replace_nanosecond(nanos)
                .unwrap();
            assert_eq!(
                run_directory_parts(value),
                ("1970-01-01".into(), format!("19700101T000000{expected}Z"))
            );
        }
    }

    #[test]
    fn run_directory_date_and_stamp_share_utc_at_year_and_epoch_boundaries() {
        let last = Date::from_calendar_date(2026, Month::December, 31)
            .unwrap()
            .with_hms_nano(23, 59, 59, 999_999_999)
            .unwrap()
            .assume_utc();
        let leap = Date::from_calendar_date(2000, Month::February, 29)
            .unwrap()
            .with_hms_nano(12, 0, 0, 1)
            .unwrap()
            .assume_utc();
        for (value, date, stamp) in [
            (
                OffsetDateTime::from_unix_timestamp_nanos(-1).unwrap(),
                "1969-12-31",
                "19691231T235959999999999Z",
            ),
            (
                last.to_offset(UtcOffset::from_hms(9, 30, 0).unwrap()),
                "2026-12-31",
                "20261231T235959999999999Z",
            ),
            (
                last + time::Duration::nanoseconds(1),
                "2027-01-01",
                "20270101T000000000000000Z",
            ),
            (leap, "2000-02-29", "20000229T120000000000001Z"),
        ] {
            assert_eq!(run_directory_parts(value), (date.into(), stamp.into()));
        }
    }

    #[test]
    fn utc_text_keeps_signed_and_zero_padded_calendar_years() {
        for (value, expected) in [
            (-9999, "-9999"),
            (-1, "-0001"),
            (0, "0000"),
            (1, "0001"),
            (999, "0999"),
            (1000, "1000"),
            (1582, "1582"),
            (1900, "1900"),
            (1969, "1969"),
            (2000, "2000"),
            (2026, "2026"),
            (2099, "2099"),
            (9999, "9999"),
        ] {
            let value = Date::from_calendar_date(value, Month::December, 31)
                .unwrap()
                .with_hms(23, 59, 59)
                .unwrap()
                .assume_utc();
            assert_eq!(rfc3339(value), format!("{expected}-12-31T23:59:59+00:00"));
            assert_eq!(
                rfc3339_micros(value),
                format!("{expected}-12-31T23:59:59.000000+00:00")
            );
            assert_eq!(
                rfc3339_seconds_or_micros(value),
                format!("{expected}-12-31T23:59:59+00:00")
            );
            assert_eq!(migration_name(value), format!("{expected}1231_235959"));
            assert_eq!(
                run_directory_parts(value),
                (
                    format!("{expected}-12-31"),
                    format!("{expected}1231T235959000000000Z")
                )
            );
        }
        let leap_day = Date::from_calendar_date(0, Month::February, 29)
            .unwrap()
            .midnight()
            .assume_utc();
        assert_eq!(rfc3339(leap_day), "0000-02-29T00:00:00+00:00");
        assert_eq!(migration_name(leap_day), "00000229_000000");
        assert_eq!(
            run_directory_parts(leap_day),
            ("0000-02-29".into(), "00000229T000000000000000Z".into())
        );
    }

    #[test]
    fn utc_text_normalizes_offsets_at_the_negative_epoch_boundary() {
        let value = OffsetDateTime::from_unix_timestamp_nanos(-1)
            .unwrap()
            .to_offset(UtcOffset::from_hms(9, 30, 0).unwrap());
        assert_eq!(rfc3339(value), "1969-12-31T23:59:59.999999999+00:00");
        assert_eq!(rfc3339_micros(value), "1969-12-31T23:59:59.999999+00:00");
        assert_eq!(
            rfc3339_seconds_or_micros(value),
            "1969-12-31T23:59:59.999999+00:00"
        );
        assert_eq!(migration_name(value), "19691231_235959");
    }
}
