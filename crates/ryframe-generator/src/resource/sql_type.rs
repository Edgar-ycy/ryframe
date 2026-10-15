use super::{FieldSpec, ResourceError, ValueType};

/// 物理类型只接受完整的类型声明，不接受默认值、约束或 SQL 片段。
pub(super) fn normalize_fields(
    fields: &mut [FieldSpec],
    resource: &str,
    path: &str,
) -> Result<(), ResourceError> {
    for field in fields {
        let Some(input) = field.sql_type.as_deref() else {
            continue;
        };
        let declaration = parse(input, field.value_type)
            .map_err(|reason| field_error(resource, path, &field.name, reason))?;
        constrain(field, &declaration)
            .map_err(|reason| field_error(resource, path, &field.name, reason))?;
        field.sql_type = Some(declaration.sql);
    }
    Ok(())
}

fn field_error(resource: &str, path: &str, field: &str, reason: String) -> ResourceError {
    ResourceError::new(
        format!("sql_type 无效：{reason}；本批未写入"),
        "使用与 Rust 字段匹配的 MySQL 类型声明；长度、范围校验不能超过物理列容量，未支持类型使用强类型业务扩展",
    )
    .with_resource(resource)
    .with_file(path)
    .with_field(field)
}

struct Declaration {
    sql: String,
    max_length: Option<u32>,
    max_bytes: Option<u32>,
    range: Option<(i64, i64)>,
}

fn parse(input: &str, value_type: ValueType) -> Result<Declaration, String> {
    let normalized = input.trim().to_ascii_uppercase();
    let (base, size) = parts(&normalized)?;
    let mut declaration = Declaration {
        sql: normalized.clone(),
        max_length: None,
        max_bytes: None,
        range: None,
    };
    match (value_type, base, size) {
        (ValueType::String, "CHAR" | "VARCHAR", Some(length))
            if length > 0 && length <= if base == "CHAR" { 255 } else { 65_535 } =>
        {
            declaration.max_length = Some(length);
        }
        (ValueType::String, "TINYTEXT" | "TEXT" | "MEDIUMTEXT" | "LONGTEXT", None) => {
            declaration.max_bytes = Some(match base {
                "TINYTEXT" => 255,
                "TEXT" => 65_535,
                "MEDIUMTEXT" => 16_777_215,
                _ => u32::MAX,
            });
        }
        (ValueType::I32, "TINYINT" | "SMALLINT" | "MEDIUMINT" | "INT", width)
            if width.is_none_or(|value| (1..=255).contains(&value)) =>
        {
            declaration.range = Some(match base {
                "TINYINT" => (i8::MIN.into(), i8::MAX.into()),
                "SMALLINT" => (i16::MIN.into(), i16::MAX.into()),
                "MEDIUMINT" => (-8_388_608, 8_388_607),
                _ => (i32::MIN.into(), i32::MAX.into()),
            });
        }
        (ValueType::I64, "BIGINT", width)
            if width.is_none_or(|value| (1..=255).contains(&value)) =>
        {
            declaration.range = Some((i64::MIN, i64::MAX));
        }
        (ValueType::Bool, "BOOL" | "BOOLEAN", None) | (ValueType::Bool, "TINYINT", Some(1)) => {
            declaration.sql = "TINYINT(1)".into()
        }
        (ValueType::Date, "DATE", None) | (ValueType::Json, "JSON", None) => {}
        (ValueType::DateTime, "DATETIME", precision)
            if precision.is_none_or(|value| value <= 6) =>
        {
            if precision == Some(0) {
                declaration.sql = "DATETIME".into();
            }
        }
        _ => {
            return Err(format!("类型 `{input}` 与 {value_type:?} 不匹配或尚未支持"));
        }
    }
    Ok(declaration)
}

fn parts(value: &str) -> Result<(&str, Option<u32>), String> {
    let invalid = || {
        "类型只允许类型名及一个整数参数，例如 VARCHAR(32)、DATETIME(6)；不能包含 SQL 子句"
            .to_owned()
    };
    let Some((base, tail)) = value.split_once('(') else {
        if !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_uppercase()) {
            return Ok((value, None));
        }
        return Err(invalid());
    };
    let digits = tail.strip_suffix(')').ok_or_else(invalid)?;
    if base.is_empty()
        || !base.bytes().all(|byte| byte.is_ascii_uppercase())
        || digits.is_empty()
        || !digits.bytes().all(|byte| byte.is_ascii_digit())
        || (digits.len() > 1 && digits.starts_with('0'))
    {
        return Err(invalid());
    }
    Ok((base, Some(digits.parse().map_err(|_| invalid())?)))
}

fn constrain(field: &mut FieldSpec, declaration: &Declaration) -> Result<(), String> {
    let validation = &mut field.validation;
    if let Some(capacity) = declaration.max_length {
        if validation.max_length.is_some_and(|value| value > capacity)
            || validation.min_length.is_some_and(|value| value > capacity)
        {
            return Err(format!("字符长度校验超过 {} 的容量", declaration.sql));
        }
        validation.max_length.get_or_insert(capacity);
    }
    if let Some(capacity) = declaration.max_bytes {
        if validation
            .max_utf8_bytes
            .is_some_and(|value| value > capacity)
            || validation
                .min_utf8_bytes
                .is_some_and(|value| value > capacity)
        {
            return Err(format!("UTF-8 字节校验超过 {} 的容量", declaration.sql));
        }
        validation.max_utf8_bytes.get_or_insert(capacity);
    }
    if let Some((minimum, maximum)) = declaration.range {
        if [validation.minimum, validation.maximum]
            .into_iter()
            .flatten()
            .any(|value| value < minimum || value > maximum)
        {
            return Err(format!("数值校验超过 {} 的有符号范围", declaration.sql));
        }
        validation.minimum.get_or_insert(minimum);
        validation.maximum.get_or_insert(maximum);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preserves_supported_physical_declarations_and_capacities() {
        let varchar = parse("varchar(32)", ValueType::String).unwrap();
        assert_eq!(varchar.sql, "VARCHAR(32)");
        assert_eq!(varchar.max_length, Some(32));
        assert_eq!(
            parse("text", ValueType::String).unwrap().max_bytes,
            Some(65_535)
        );
        assert_eq!(
            parse("mediumint", ValueType::I32).unwrap().range,
            Some((-8_388_608, 8_388_607))
        );
        assert_eq!(parse("int(11)", ValueType::I32).unwrap().sql, "INT(11)");
        assert_eq!(parse("boolean", ValueType::Bool).unwrap().sql, "TINYINT(1)");
        assert_eq!(
            parse("datetime(0)", ValueType::DateTime).unwrap().sql,
            "DATETIME"
        );
    }

    #[test]
    fn rejects_mismatch_unsupported_types_and_sql_fragments() {
        for invalid in [
            "VARCHAR(0)",
            "CHAR(256)",
            "VARCHAR(65536)",
            "VARCHAR(01)",
            "VARCHAR(32) NOT NULL",
            "TEXT; DROP TABLE x",
            "TEXT/**/",
            "INT UNSIGNED",
            "VARCHAR(-1)",
            "VARCHAR(3,2)",
            "VARCHAR(32))",
            "VARCHAR( 32 )",
        ] {
            assert!(parse(invalid, ValueType::String).is_err(), "{invalid}");
        }
        assert!(parse("VARCHAR(32)", ValueType::I64).is_err());
        assert!(parse("DATETIME(7)", ValueType::DateTime).is_err());
        assert!(parse("DECIMAL(10,2)", ValueType::Decimal).is_err());
    }

    #[test]
    fn physical_capacity_is_inferred_and_contradictions_include_field_context() {
        let source = include_str!("../../tests/fixtures/order.toml");
        let mut spec = super::super::ResourceSpec::parse(source, "order.toml").unwrap();
        let field = spec
            .fields
            .iter_mut()
            .find(|field| field.name == "name")
            .unwrap();
        field.sql_type = Some("varchar(32)".into());
        field.validation.max_length = None;
        normalize_fields(&mut spec.fields, "order", "order.toml").unwrap();
        let field = spec
            .fields
            .iter_mut()
            .find(|field| field.name == "name")
            .unwrap();
        assert_eq!(field.validation.max_length, Some(32));
        field.validation.max_length = Some(33);
        let error = normalize_fields(&mut spec.fields, "order", "order.toml")
            .unwrap_err()
            .to_string();
        for detail in [
            "资源 order",
            "字段 name",
            "order.toml",
            "VARCHAR(32)",
            "本批未写入",
            "修复建议",
        ] {
            assert!(error.contains(detail), "{error}");
        }
        let field = spec
            .fields
            .iter_mut()
            .find(|field| field.name == "status")
            .unwrap();
        field.sql_type = Some("tinyint".into());
        field.validation.minimum = None;
        field.validation.maximum = None;
        let declaration = parse("tinyint", ValueType::I32).unwrap();
        constrain(field, &declaration).unwrap();
        assert_eq!(field.validation.minimum, Some(-128));
        assert_eq!(field.validation.maximum, Some(127));
        field.validation.minimum = Some(-129);
        assert!(constrain(field, &declaration).is_err());
    }
}
