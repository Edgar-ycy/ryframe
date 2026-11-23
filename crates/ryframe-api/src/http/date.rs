use std::ops::RangeInclusive;

use serde::{Deserialize, Deserializer, Serialize, Serializer, de::Error as _, ser::Error as _};
use time::{Date, Month};

const DATE_YEARS: RangeInclusive<i32> = 0..=9999;
const INVALID_DATE: &str = "日期必须是四位非负年份内的有效日期字符串";

fn parse_date(value: &str) -> Option<Date> {
    let mut parts = value.split('-').map(str::trim);
    let (Some(year), Some(month), Some(day), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return None;
    };
    if year.len() > 4 && !year.starts_with('+') {
        return None;
    }
    let year = year.parse().ok()?;
    if !DATE_YEARS.contains(&year) {
        return None;
    }
    let month = Month::try_from(date_component(month)?).ok()?;
    Date::from_calendar_date(year, month, date_component(day)?).ok()
}

fn date_component(value: &str) -> Option<u8> {
    if !(1..=2).contains(&value.len()) || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    value.parse().ok()
}

fn canonical_date(value: &Date) -> Result<String, &'static str> {
    if !DATE_YEARS.contains(&value.year()) {
        return Err(INVALID_DATE);
    }
    Ok(value.to_string())
}

pub fn deserialize_date<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Date, D::Error> {
    parse_date(&String::deserialize(deserializer)?).ok_or_else(|| D::Error::custom(INVALID_DATE))
}

pub fn deserialize_optional_date<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<Date>, D::Error> {
    Option::<String>::deserialize(deserializer)?
        .map(|value| parse_date(&value).ok_or_else(|| D::Error::custom(INVALID_DATE)))
        .transpose()
}

pub fn serialize_date<S: Serializer>(value: &Date, serializer: S) -> Result<S::Ok, S::Error> {
    canonical_date(value)
        .map_err(S::Error::custom)?
        .serialize(serializer)
}

pub fn serialize_optional_date<S: Serializer>(
    value: &Option<Date>,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    value
        .as_ref()
        .map(canonical_date)
        .transpose()
        .map_err(S::Error::custom)?
        .serialize(serializer)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn date_inputs_keep_legacy_spellings_within_four_digit_years() {
        for (input, expected) in [
            ("0000-2-29", "0000-02-29"),
            ("0001-1-2", "0001-01-02"),
            ("1000-1-1", "1000-01-01"),
            ("2024-2-29", "2024-02-29"),
            (" +002024 - 2 - 9 ", "2024-02-09"),
            ("9999-12-31", "9999-12-31"),
        ] {
            assert_eq!(
                canonical_date(&parse_date(input).unwrap()).unwrap(),
                expected
            );
        }
        for input in ["0000-0-0", "+10000-1-1", "2023-2-29", "2024-002-9"] {
            assert!(parse_date(input).is_none(), "{input}");
        }
    }
}
