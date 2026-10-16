use super::{FieldSpec, ResourceError, ValueType};

pub(crate) fn capacity(input: &str) -> Option<(u32, u32)> {
    let input = input.trim().to_ascii_uppercase();
    let value = input
        .strip_prefix("DECIMAL")
        .or_else(|| input.strip_prefix("NUMERIC"))?
        .trim();
    let (precision, scale) = value
        .strip_prefix('(')?
        .strip_suffix(')')?
        .split_once(',')?;
    let precision = precision.trim().parse::<u32>().ok()?;
    let scale = scale.trim().parse::<u32>().ok()?;
    ((1..=28).contains(&precision) && scale <= precision).then_some((precision, scale))
}

pub(crate) fn normalize_type(input: &str) -> Result<String, String> {
    let (precision, scale) = capacity(input).ok_or_else(|| "Decimal 必须声明 DECIMAL(p,s)，其中 1<=p<=28、0<=s<=p；超过该范围不能保证整个 SQL 数值域精确可用".to_owned())?;
    Ok(format!("DECIMAL({precision},{scale})"))
}

fn parts(value: &str) -> Option<(&str, &str, bool)> {
    let negative = value.starts_with('-');
    let digits = value.strip_prefix('-').unwrap_or(value);
    let (integer, fraction) = digits.split_once('.').unwrap_or((digits, ""));
    if integer.is_empty()
        || !integer.bytes().all(|byte| byte.is_ascii_digit())
        || (integer.len() > 1 && integer.starts_with('0'))
        || !fraction.bytes().all(|byte| byte.is_ascii_digit())
        || value.ends_with('.')
        || (negative
            && digits
                .bytes()
                .filter(|byte| *byte != b'.')
                .all(|byte| byte == b'0'))
    {
        return None;
    }
    Some((integer, fraction, negative))
}

pub(crate) fn is_literal(value: &str) -> bool {
    parts(value).is_some()
}

pub(crate) fn normalize_literal(value: &str, precision: u32, scale: u32) -> Option<String> {
    let (integer, fraction, negative) = parts(value)?;
    let integer_digits = if integer == "0" {
        0
    } else {
        u32::try_from(integer.len()).ok()?
    };
    if integer_digits > precision - scale || fraction.len() > scale as usize {
        return None;
    }
    let sign = if negative { "-" } else { "" };
    if scale == 0 {
        return Some(format!("{sign}{integer}"));
    }
    Some(format!(
        "{sign}{integer}.{fraction}{}",
        "0".repeat(scale as usize - fraction.len())
    ))
}

pub(super) fn normalize_fields(
    fields: &mut [FieldSpec],
    resource: &str,
    path: &str,
) -> Result<(), ResourceError> {
    for field in fields
        .iter_mut()
        .filter(|field| field.value_type == ValueType::Decimal)
    {
        let (precision, scale) =
            capacity(field.sql_type.as_deref().expect("Decimal SQL 类型已校验"))
                .expect("Decimal 容量已校验");
        if let Some(value) = &field.default {
            let normalized = value
                .as_str()
                .and_then(|value| normalize_literal(value, precision, scale))
                .ok_or_else(|| {
                    ResourceError::new(
                        "Decimal 默认值必须是列容量内的规范十进制字符串；本批未写入",
                        "使用带引号的十进制默认值，不能超过整数容量和小数位数；不会自动舍入",
                    )
                    .with_resource(resource)
                    .with_field(&field.name)
                    .with_file(path)
                })?;
            field.default = Some(toml::Value::String(normalized));
        }
    }
    Ok(())
}

pub(crate) fn widening(before: &str, after: &str) -> bool {
    matches!((capacity(before), capacity(after)), (Some((old_p, old_s)), Some((new_p, new_s))) if new_s >= old_s && new_p-new_s >= old_p-old_s)
}

#[cfg(test)]
mod tests {
    use super::widening;

    #[test]
    fn widening_preserves_both_integer_capacity_and_fractional_precision() {
        assert!(widening("DECIMAL(20,6)", "DECIMAL(22,8)"));
        assert!(widening("DECIMAL(20,6)", "DECIMAL(22,6)"));
        assert!(!widening("DECIMAL(20,6)", "DECIMAL(20,8)"));
        assert!(!widening("DECIMAL(20,6)", "DECIMAL(20,4)"));
        assert!(!widening("DECIMAL(20,6)", "DECIMAL(29,6)"));
        assert!(!widening("DECIMAL(20,6)", "DOUBLE"));
    }
}
