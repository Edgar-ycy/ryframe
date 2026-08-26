use super::{
    FieldSpec, LabelsSpec, ResourceError, ValueType, WidgetSpec, is_snake_identifier,
    validate_enum_key, validate_field_value,
};

pub(super) fn validate_field(
    resource: &str,
    source_path: &str,
    field: &FieldSpec,
) -> Result<(), ResourceError> {
    if !is_snake_identifier(&field.name) {
        return Err(field_error(
            resource,
            &field.name,
            source_path,
            "字段名必须是小写 snake_case 标识符",
            "字段名只使用小写字母、数字和下划线",
        ));
    }
    if let Some(column) = &field.column
        && !is_snake_identifier(column)
    {
        return Err(field_error(
            resource,
            &field.name,
            source_path,
            format!("数据库列名 `{column}` 不是安全标识符"),
            "column 只使用小写字母、数字和下划线",
        ));
    }
    if let Some(wire_type) = field.wire_type
        && wire_type != field.value_type
        && !matches!(
            (field.value_type, wire_type),
            (ValueType::I64, ValueType::String)
        )
    {
        return Err(field_error(
            resource,
            &field.name,
            source_path,
            "不支持该 value_type 与 wire_type 组合",
            "目前只有 i64 可在 API 中安全序列化为 string",
        ));
    }
    validate_labels(&field.labels, resource, Some(&field.name), source_path)?;
    if field.value_type == ValueType::Decimal {
        return Err(field_error(
            resource,
            &field.name,
            source_path,
            "flat_crud v1 不生成 decimal 字段",
            "将精度与舍入规则放入强类型业务切片，或先扩展完整 decimal 契约测试",
        ));
    }
    if field.validation.required && field.nullable {
        return Err(field_error(
            resource,
            &field.name,
            source_path,
            "nullable 与 validation.required=true 冲突",
            "将字段改为非空，或取消 required",
        ));
    }
    if let (Some(min), Some(max)) = (field.validation.min_length, field.validation.max_length)
        && min > max
    {
        return Err(field_error(
            resource,
            &field.name,
            source_path,
            "min_length 大于 max_length",
            "调整长度边界，使最小值不大于最大值",
        ));
    }
    if let (Some(min), Some(max)) = (
        field.validation.min_utf8_bytes,
        field.validation.max_utf8_bytes,
    ) && min > max
    {
        return Err(field_error(
            resource,
            &field.name,
            source_path,
            "min_utf8_bytes 大于 max_utf8_bytes",
            "调整 UTF-8 字节长度边界，使最小值不大于最大值",
        ));
    }
    if let (Some(min), Some(max)) = (field.validation.minimum, field.validation.maximum)
        && min > max
    {
        return Err(field_error(
            resource,
            &field.name,
            source_path,
            "minimum 大于 maximum",
            "调整数值边界，使最小值不大于最大值",
        ));
    }
    if field.validation.pattern.is_some() {
        return Err(field_error(
            resource,
            &field.name,
            source_path,
            "flat_crud v1 不生成 validation.pattern",
            "将正则语义放入强类型 DTO/frontend extension，并为两端添加一致性测试",
        ));
    }
    if (field.validation.min_length.is_some() || field.validation.max_length.is_some())
        && field.value_type != ValueType::String
    {
        return Err(field_error(
            resource,
            &field.name,
            source_path,
            "字符串校验被用于非 string 字段",
            "移除长度校验，或将 value_type 改为 string",
        ));
    }
    if (field.validation.min_utf8_bytes.is_some() || field.validation.max_utf8_bytes.is_some())
        && field.value_type != ValueType::String
    {
        return Err(field_error(
            resource,
            &field.name,
            source_path,
            "UTF-8 字节校验被用于非 string 字段",
            "移除字节长度校验，或将 value_type 改为 string",
        ));
    }
    if (field.validation.minimum.is_some() || field.validation.maximum.is_some())
        && !matches!(field.value_type, ValueType::I32 | ValueType::I64)
    {
        return Err(field_error(
            resource,
            &field.name,
            source_path,
            "数值范围校验被用于非 i32/i64 字段",
            "移除 minimum/maximum，或使用 i32/i64；精确数值规则放入强类型扩展",
        ));
    }
    if (field.usage.create || field.usage.update)
        && !matches!(
            field.widget,
            WidgetSpec::Text | WidgetSpec::Number | WidgetSpec::Select
        )
    {
        return Err(field_error(
            resource,
            &field.name,
            source_path,
            "可编辑字段使用了 FlatCrud 尚不支持的控件",
            "v1 仅支持 text、number、select；其他控件放入强类型 frontend extension",
        ));
    }
    if !field.enum_values.is_empty() && !matches!(field.widget, WidgetSpec::Select) {
        return Err(field_error(
            resource,
            &field.name,
            source_path,
            "enum_values 只能配合 select 控件",
            "将 widget 改为 select，或删除 enum_values",
        ));
    }
    if matches!(field.widget, WidgetSpec::Select) && field.enum_values.is_empty() {
        return Err(field_error(
            resource,
            &field.name,
            source_path,
            "select 控件缺少 enum_values",
            "声明有限枚举项，复杂选项加载放入前端扩展",
        ));
    }
    for (key, labels) in &field.enum_values {
        validate_labels(
            &LabelsSpec {
                zh_cn: labels.zh_cn.clone(),
                en: labels.en.clone(),
            },
            resource,
            Some(&field.name),
            source_path,
        )?;
        validate_enum_key(resource, source_path, field, key)?;
    }
    if field.usage.create_optional && !field.usage.create {
        return Err(field_error(
            resource,
            &field.name,
            source_path,
            "create_optional 只能用于 create 字段",
            "启用 usage.create，或删除 create_optional",
        ));
    }
    if field.usage.update_optional && !field.usage.update {
        return Err(field_error(
            resource,
            &field.name,
            source_path,
            "update_optional 只能用于 update 字段",
            "启用 usage.update，或删除 update_optional",
        ));
    }
    if field.usage.sort_desc && !field.usage.sort {
        return Err(field_error(
            resource,
            &field.name,
            source_path,
            "sort_desc 只能用于默认排序字段",
            "同时启用 usage.sort，或删除 sort_desc",
        ));
    }
    if field.nullable && field.usage.update_optional {
        return Err(field_error(
            resource,
            &field.name,
            source_path,
            "nullable 与 update_optional 无法区分未提交和显式置空",
            "使用非可空可选更新，或在手写 DTO 中使用三态字段",
        ));
    }
    if field.nullable && field.usage.create_optional {
        return Err(field_error(
            resource,
            &field.name,
            source_path,
            "nullable 字段无需 create_optional，当前组合会丢失显式空值语义",
            "删除 create_optional；nullable 字段的创建 DTO 已经是 Option",
        ));
    }
    if let Some(default) = &field.default {
        validate_field_value(resource, source_path, field, default, "default")?;
    }
    Ok(())
}

pub(super) fn validate_labels(
    labels: &LabelsSpec,
    resource: &str,
    field: Option<&str>,
    source_path: &str,
) -> Result<(), ResourceError> {
    if labels.zh_cn.trim().is_empty() || labels.en.trim().is_empty() {
        let mut error =
            ResourceError::new("中英文标签均不能为空", "同时填写 labels.zh_cn 和 labels.en")
                .with_resource(resource)
                .with_file(source_path);
        if let Some(field) = field {
            error = error.with_field(field);
        }
        return Err(error);
    }
    Ok(())
}

pub(super) fn field_error(
    resource: &str,
    field: &str,
    source_path: &str,
    message: impl Into<String>,
    suggestion: impl Into<String>,
) -> ResourceError {
    ResourceError::new(message, suggestion)
        .with_resource(resource)
        .with_field(field)
        .with_file(source_path)
}
