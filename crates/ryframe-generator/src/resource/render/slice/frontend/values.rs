use super::{FieldIr, ValueType, WidgetIr};

pub(super) fn push_typescript_object(output: &mut String, properties: &[String]) {
    output.push_str("    {\n");
    for property in properties {
        output.push_str("      ");
        output.push_str(property);
        output.push_str(",\n");
    }
    output.push_str("    },\n");
}

pub(super) fn ts_default(field: &FieldIr) -> &'static str {
    if field.value_type == ValueType::Json {
        return "''";
    }
    if matches!(
        field.value_type,
        ValueType::I64 | ValueType::U64 | ValueType::Decimal
    ) && field.wire_type == ValueType::String
    {
        return "''";
    }
    match field.value_type {
        ValueType::String | ValueType::Date | ValueType::DateTime | ValueType::Decimal => "''",
        ValueType::I8
        | ValueType::I16
        | ValueType::I32
        | ValueType::I64
        | ValueType::U8
        | ValueType::U16
        | ValueType::U32
        | ValueType::U64 => "0",
        ValueType::Bool => "false",
        ValueType::Json => unreachable!("JSON 表单默认值已在函数开头处理"),
    }
}

pub(super) fn ts_enum_value(value: &str, field: &FieldIr) -> String {
    if field.wire_type == ValueType::String {
        return format!("{value:?}");
    }
    match field.value_type {
        value_type if value_type.integer_bounds().is_some() => value.to_owned(),
        ValueType::Bool if matches!(value, "true" | "false") => value.to_owned(),
        _ => format!("{value:?}"),
    }
}

pub(super) fn form_input(field: &FieldIr, optional: bool) -> String {
    let value = format!("form.{}", field.name);
    if field.value_type == ValueType::Json {
        return if field.nullable {
            format!("{value}.trim() === '' ? null : JSON.parse({value})")
        } else if optional {
            format!("{value}.trim() === '' ? undefined : JSON.parse({value})")
        } else {
            format!("{value}.trim() === '' ? null : JSON.parse({value})")
        };
    }
    if matches!(
        field.value_type,
        ValueType::I64 | ValueType::U64 | ValueType::Decimal
    ) && field.wire_type == ValueType::String
    {
        if field.nullable {
            return format!("{value} === '' ? null : {value}");
        }
        if optional {
            return format!("{value} === '' ? undefined : {value}");
        }
    }
    value
}

pub(super) fn form_typescript_type(field: &FieldIr) -> String {
    if field.value_type == ValueType::Json {
        "string".into()
    } else {
        field.typescript_type.clone()
    }
}

pub(super) fn edit_form_value(field: &FieldIr) -> String {
    if field.value_type == ValueType::Json {
        return if field.nullable {
            format!(
                "record.{} == null ? '' : JSON.stringify(record.{}, null, 2)",
                field.name, field.name
            )
        } else {
            format!("JSON.stringify(record.{}, null, 2)", field.name)
        };
    }
    if field.nullable {
        format!("record.{} ?? null", field.name)
    } else {
        format!("record.{}", field.name)
    }
}

pub(super) fn form_widget(field: &FieldIr) -> &'static str {
    if !field.enum_values.is_empty() {
        "radio"
    } else {
        match field.widget {
            WidgetIr::Textarea => "textarea",
            WidgetIr::Number => "number",
            WidgetIr::Switch => "switch",
            WidgetIr::Date => "date",
            WidgetIr::DateTime => "date_time",
            WidgetIr::Json => "json",
            _ => "text",
        }
    }
}
