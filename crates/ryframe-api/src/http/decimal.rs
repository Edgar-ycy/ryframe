use rust_decimal::Decimal;
use serde::{Deserialize, Deserializer, de::Error};

/// 精确十进制不经过浮点数，不接受指数、空白或可造成歧义的写法。
pub fn parse_decimal(value: &str) -> Result<Decimal, &'static str> {
    let digits = value.strip_prefix('-').unwrap_or(value);
    let (integer, fraction) = digits
        .split_once('.')
        .map_or((digits, None), |(integer, fraction)| {
            (integer, Some(fraction))
        });
    if integer.is_empty()
        || !integer.bytes().all(|byte| byte.is_ascii_digit())
        || (integer.len() > 1 && integer.starts_with('0'))
        || fraction.is_some_and(|fraction| {
            fraction.is_empty()
                || fraction.len() > 28
                || !fraction.bytes().all(|byte| byte.is_ascii_digit())
        })
    {
        return Err("必须是规范十进制字符串，不允许指数、空白、加号或前导零");
    }
    let parsed = Decimal::from_str_exact(value).map_err(|_| "十进制数超过精确容量；未执行舍入")?;
    if parsed.is_zero() && value.starts_with('-') {
        return Err("十进制零不能带负号");
    }
    Ok(parsed)
}

pub fn parse_decimal_with_capacity(
    value: &str,
    precision: u32,
    scale: u32,
) -> Result<Decimal, &'static str> {
    let parsed = parse_decimal(value)?;
    ryframe_kernel::decimal_fits_precision(parsed, precision, scale)
        .then_some(parsed)
        .ok_or("十进制数超出列的整数容量或小数位数；未执行舍入")
}

pub fn deserialize_decimal<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Decimal, D::Error> {
    parse_decimal(&String::deserialize(deserializer)?).map_err(D::Error::custom)
}

pub fn deserialize_optional_decimal<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<Decimal>, D::Error> {
    Option::<String>::deserialize(deserializer)?
        .map(|value| parse_decimal(&value).map_err(D::Error::custom))
        .transpose()
}
