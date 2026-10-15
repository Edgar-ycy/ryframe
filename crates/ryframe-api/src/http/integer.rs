use serde::{Deserialize, Deserializer, de::Error};

/// JSON 及查询参数共用规范十进制校验，避免经过浮点数或接受多种等价写法。
pub fn parse_i64(value: &str) -> Result<i64, &'static str> {
    value
        .parse::<i64>()
        .ok()
        .filter(|number| number.to_string() == value)
        .ok_or("必须是 i64 范围内的规范十进制字符串")
}

pub fn deserialize_i64<'de, D: Deserializer<'de>>(deserializer: D) -> Result<i64, D::Error> {
    let value = String::deserialize(deserializer)?;
    parse_i64(&value).map_err(D::Error::custom)
}

pub fn deserialize_optional_i64<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<i64>, D::Error> {
    Option::<String>::deserialize(deserializer)?
        .map(|value| parse_i64(&value).map_err(D::Error::custom))
        .transpose()
}
