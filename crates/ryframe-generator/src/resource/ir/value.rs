use std::collections::BTreeMap;

use super::{
    EnumValueSpec, FieldIr, FieldSpec, ResourceError, ValidationSpec, ValueType, field_error,
};

pub(super) fn validate_enum_key(
    resource: &str,
    source_path: &str,
    field: &FieldSpec,
    key: &str,
) -> Result<(), ResourceError> {
    if field.value_type == ValueType::String && field.usage.filter && key.is_empty() {
        return Err(field_error(
            resource,
            &field.name,
            source_path,
            "可筛选字符串枚举不能使用空字符串键",
            "使用非空枚举键；空字符串保留为前端“未筛选”哨兵",
        ));
    }
    let valid = match field.value_type {
        ValueType::String => true,
        ValueType::I32 => key
            .parse::<i32>()
            .is_ok_and(|parsed| parsed.to_string() == key),
        ValueType::I64 => key
            .parse::<i64>()
            .is_ok_and(|parsed| parsed.to_string() == key),
        ValueType::Bool => matches!(key, "true" | "false"),
        ValueType::Decimal | ValueType::Date | ValueType::DateTime | ValueType::Json => false,
    };
    if valid {
        Ok(())
    } else {
        Err(field_error(
            resource,
            &field.name,
            source_path,
            format!(
                "枚举键 `{key}` 与字段 value_type={:?} 不匹配",
                field.value_type
            ),
            "string 使用字符串键；i32/i64 使用规范十进制整数；bool 仅使用 true/false",
        ))
    }
}

pub(super) fn validate_field_value(
    resource: &str,
    source_path: &str,
    field: &FieldSpec,
    value: &toml::Value,
    owner: &str,
) -> Result<(), ResourceError> {
    validate_typed_value(
        resource,
        source_path,
        &field.name,
        field.value_type,
        value,
        owner,
    )?;
    validate_value_constraints(
        resource,
        source_path,
        &field.name,
        field.value_type,
        &field.validation,
        &field.enum_values,
        value,
        owner,
    )
}

pub(super) fn validate_ir_value(
    resource: &str,
    source_path: &str,
    field: &FieldIr,
    value: &toml::Value,
    owner: &str,
) -> Result<(), ResourceError> {
    validate_typed_value(
        resource,
        source_path,
        &field.name,
        field.value_type,
        value,
        owner,
    )?;
    let enum_values = field
        .enum_values
        .keys()
        .map(|key| {
            (
                key.clone(),
                EnumValueSpec {
                    zh_cn: String::new(),
                    en: String::new(),
                },
            )
        })
        .collect::<BTreeMap<_, _>>();
    let validation = ValidationSpec {
        required: field.validation.required,
        min_length: field.validation.min_length,
        max_length: field.validation.max_length,
        min_utf8_bytes: field.validation.min_utf8_bytes,
        max_utf8_bytes: field.validation.max_utf8_bytes,
        minimum: field.validation.minimum,
        maximum: field.validation.maximum,
        pattern: field.validation.pattern.clone(),
    };
    validate_value_constraints(
        resource,
        source_path,
        &field.name,
        field.value_type,
        &validation,
        &enum_values,
        value,
        owner,
    )
}

fn validate_typed_value(
    resource: &str,
    source_path: &str,
    field: &str,
    value_type: ValueType,
    value: &toml::Value,
    owner: &str,
) -> Result<(), ResourceError> {
    let valid = match (value_type, value) {
        (ValueType::String, toml::Value::String(_))
        | (ValueType::I64, toml::Value::Integer(_))
        | (ValueType::Bool, toml::Value::Boolean(_)) => true,
        (ValueType::I32, toml::Value::Integer(value)) => i32::try_from(*value).is_ok(),
        _ => false,
    };
    if valid {
        return Ok(());
    }
    Err(field_error(
        resource,
        field,
        source_path,
        format!("{owner} 的 TOML 值与 value_type={value_type:?} 不匹配"),
        "使用严格同类型的 string/integer/boolean；date、date_time、decimal、json 默认值放入强类型扩展",
    ))
}

#[allow(clippy::too_many_arguments)]
fn validate_value_constraints(
    resource: &str,
    source_path: &str,
    field: &str,
    value_type: ValueType,
    validation: &ValidationSpec,
    enum_values: &BTreeMap<String, EnumValueSpec>,
    value: &toml::Value,
    owner: &str,
) -> Result<(), ResourceError> {
    let invalid = match (value_type, value) {
        (ValueType::String, toml::Value::String(value)) => {
            let length = value.chars().count() as u32;
            let utf8_bytes = u32::try_from(value.len()).unwrap_or(u32::MAX);
            (validation.required && value.is_empty())
                || validation
                    .min_length
                    .is_some_and(|minimum| length < minimum)
                || validation
                    .max_length
                    .is_some_and(|maximum| length > maximum)
                || validation
                    .min_utf8_bytes
                    .is_some_and(|minimum| utf8_bytes < minimum)
                || validation
                    .max_utf8_bytes
                    .is_some_and(|maximum| utf8_bytes > maximum)
        }
        (ValueType::I32 | ValueType::I64, toml::Value::Integer(value)) => {
            validation.minimum.is_some_and(|minimum| *value < minimum)
                || validation.maximum.is_some_and(|maximum| *value > maximum)
        }
        _ => false,
    };
    if invalid {
        return Err(field_error(
            resource,
            field,
            source_path,
            format!("{owner} 不满足字段 validation 约束"),
            "调整默认值或 validation，使长度、范围和 required 约束一致",
        ));
    }
    if !enum_values.is_empty() {
        let key = match value {
            toml::Value::String(value) => value.clone(),
            toml::Value::Integer(value) => value.to_string(),
            toml::Value::Boolean(value) => value.to_string(),
            _ => String::new(),
        };
        if !enum_values.contains_key(&key) {
            return Err(field_error(
                resource,
                field,
                source_path,
                format!("{owner} `{key}` 不在 enum_values 中"),
                "将默认值改为已声明枚举键，或补充对应中英文枚举项",
            ));
        }
    }
    Ok(())
}
