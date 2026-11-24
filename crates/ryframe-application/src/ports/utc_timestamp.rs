//! 保持现有 UTC 时间在 JSON 中的字符串格式。
use std::fmt::{self, Write as _};

use serde::{Deserialize, Deserializer, Serializer, de, ser::Error};
use time::{
    OffsetDateTime, UtcOffset, format_description::well_known::Rfc3339, macros::format_description,
};

const OUT_OF_RANGE: &str = "时间超出 UTC 表示范围";

pub fn serialize<S: Serializer>(value: &OffsetDateTime, serializer: S) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(&encode::<S::Error>(value, "Z", OUT_OF_RANGE)?)
}

pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<OffsetDateTime, D::Error> {
    let value = String::deserialize(deserializer)?;
    if !matches!(value.as_bytes().get(10), Some(b'T' | b't')) {
        return Err(de::Error::custom("时间必须使用 RFC3339 格式"));
    }
    if value.get(17..19) == Some("60") {
        return Err(de::Error::custom("时间不能精确表示闰秒"));
    }
    OffsetDateTime::parse(&value, &Rfc3339)
        .map_err(de::Error::custom)?
        .checked_to_offset(UtcOffset::UTC)
        .ok_or_else(|| de::Error::custom(OUT_OF_RANGE))
}

pub fn format_with_offset(value: &OffsetDateTime) -> Result<String, fmt::Error> {
    encode(value, "+00:00", OUT_OF_RANGE)
}

pub fn serialize_optional<S: Serializer>(
    value: &Option<OffsetDateTime>,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    serialize_optional_with_range_error(value, serializer, OUT_OF_RANGE)
}

pub(super) fn serialize_optional_with_range_error<S: Serializer>(
    value: &Option<OffsetDateTime>,
    serializer: S,
    out_of_range: &'static str,
) -> Result<S::Ok, S::Error> {
    match value {
        Some(value) => serializer.serialize_some(&encode::<S::Error>(value, "Z", out_of_range)?),
        None => serializer.serialize_none(),
    }
}

fn encode<E: Error>(
    value: &OffsetDateTime,
    suffix: &str,
    out_of_range: &'static str,
) -> Result<String, E> {
    let utc = value
        .checked_to_offset(UtcOffset::UTC)
        .ok_or_else(|| E::custom(out_of_range))?;
    let mut encoded = utc
        .format(format_description!(
            "[year]-[month]-[day]T[hour]:[minute]:[second]"
        ))
        .map_err(E::custom)?;
    let nanos = utc.nanosecond();
    if nanos != 0 {
        let (fraction, digits) = if nanos.is_multiple_of(1_000_000) {
            (nanos / 1_000_000, 3)
        } else if nanos.is_multiple_of(1_000) {
            (nanos / 1_000, 6)
        } else {
            (nanos, 9)
        };
        write!(encoded, ".{fraction:0digits$}").map_err(E::custom)?;
    }
    encoded.push_str(suffix);
    Ok(encoded)
}
