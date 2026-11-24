//! 系统 UTC 时刻的文本输出，保持既有年份、小数精度与时区后缀。

use time::{OffsetDateTime, UtcOffset};

enum Precision {
    AutoSi,
    Micros,
    SecondsOrMicros,
}

pub(crate) fn rfc3339(value: OffsetDateTime) -> String {
    encode(value, Precision::AutoSi)
}

pub(crate) fn rfc3339_micros(value: OffsetDateTime) -> String {
    encode(value, Precision::Micros)
}

pub(crate) fn rfc3339_seconds_or_micros(value: OffsetDateTime) -> String {
    encode(value, Precision::SecondsOrMicros)
}

fn encode(value: OffsetDateTime, precision: Precision) -> String {
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
    encoded.push_str("+00:00");
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
        }
        let leap_day = Date::from_calendar_date(0, Month::February, 29)
            .unwrap()
            .midnight()
            .assume_utc();
        assert_eq!(rfc3339(leap_day), "0000-02-29T00:00:00+00:00");
        assert_eq!(migration_name(leap_day), "00000229_000000");
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
