use super::super::{FieldIr, ResourceIr, ValueType};

pub(super) fn render_utf8_byte_validators(resource: &ResourceIr, output: &mut String) {
    for field in resource.fields.iter().filter(|field| {
        field.validation.min_utf8_bytes.is_some() || field.validation.max_utf8_bytes.is_some()
    }) {
        let constant = field.name.to_ascii_uppercase();
        if let Some(minimum) = field.validation.min_utf8_bytes {
            output.push_str(&format!(
                "pub const {constant}_MIN_UTF8_BYTES: usize = {minimum};\n"
            ));
        }
        if let Some(maximum) = field.validation.max_utf8_bytes {
            output.push_str(&format!(
                "pub const {constant}_MAX_UTF8_BYTES: usize = {maximum};\n"
            ));
        }
        let invalid = match (
            field.validation.min_utf8_bytes,
            field.validation.max_utf8_bytes,
        ) {
            (Some(_), Some(_)) => {
                format!("!({constant}_MIN_UTF8_BYTES..={constant}_MAX_UTF8_BYTES).contains(&bytes)")
            }
            (Some(_), None) => format!("bytes < {constant}_MIN_UTF8_BYTES"),
            (None, Some(_)) => format!("bytes > {constant}_MAX_UTF8_BYTES"),
            (None, None) => unreachable!("UTF-8 字节校验器必须至少声明一个边界"),
        };
        output.push_str(&format!(
            "fn validate_{name}_utf8_bytes(value: &str) -> Result<(), validator::ValidationError> {{\n    let bytes = value.len();\n    if {invalid} {{\n        Err(validator::ValidationError::new({error:?}))\n    }} else {{\n        Ok(())\n    }}\n}}\n\n",
            name = field.name,
            error = format!("invalid_{}_utf8_bytes", field.name),
        ));
    }
}

pub(super) fn render_enum_validators(resource: &ResourceIr, output: &mut String) {
    for field in resource
        .fields
        .iter()
        .filter(|field| (field.usage.create || field.usage.update) && !field.enum_values.is_empty())
    {
        let (argument_type, values) = match field.value_type {
            ValueType::String => (
                "&str",
                field
                    .enum_values
                    .keys()
                    .map(|value| format!("{value:?}"))
                    .collect::<Vec<_>>()
                    .join(", "),
            ),
            ValueType::I32 => (
                "i32",
                field
                    .enum_values
                    .keys()
                    .map(|value| format!("{value}_i32"))
                    .collect::<Vec<_>>()
                    .join(", "),
            ),
            ValueType::I64 => (
                "i64",
                field
                    .enum_values
                    .keys()
                    .map(|value| format!("{value}_i64"))
                    .collect::<Vec<_>>()
                    .join(", "),
            ),
            ValueType::Bool => (
                "bool",
                field
                    .enum_values
                    .keys()
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(", "),
            ),
            _ => unreachable!("枚举 value_type 已在 IR 边界校验"),
        };
        output.push_str(&format!(
            "fn validate_{name}_enum(value: {argument_type}) -> Result<(), validator::ValidationError> {{\n    if [{values}].contains(&value) {{\n        Ok(())\n    }} else {{\n        Err(validator::ValidationError::new({error:?}))\n    }}\n}}\n\n",
            name = field.name,
            error = format!("invalid_{}", field.name),
        ));
    }
}

pub(super) fn validation_attribute(field: &FieldIr) -> Option<String> {
    let validation = &field.validation;
    let mut validators = Vec::new();
    match field.value_type {
        ValueType::String
            if validation.required
                || validation.min_length.is_some()
                || validation.max_length.is_some() =>
        {
            let mut parts = Vec::new();
            let minimum = validation
                .min_length
                .map(|minimum| minimum.max(u32::from(validation.required)))
                .or_else(|| validation.required.then_some(1));
            if let Some(minimum) = minimum {
                parts.push(format!("min = {minimum}"));
            }
            if let Some(maximum) = validation.max_length {
                parts.push(format!("max = {maximum}"));
            }
            validators.push(format!("length({})", parts.join(", ")));
        }
        ValueType::I32 | ValueType::I64
            if validation.minimum.is_some() || validation.maximum.is_some() =>
        {
            let mut parts = Vec::new();
            if let Some(minimum) = validation.minimum {
                parts.push(format!("min = {minimum}"));
            }
            if let Some(maximum) = validation.maximum {
                parts.push(format!("max = {maximum}"));
            }
            validators.push(format!("range({})", parts.join(", ")));
        }
        _ => {}
    }
    if !field.enum_values.is_empty() {
        validators.push(format!(
            "custom(function = \"validate_{}_enum\")",
            field.name
        ));
    }
    if validation.min_utf8_bytes.is_some() || validation.max_utf8_bytes.is_some() {
        validators.push(format!(
            "custom(function = \"validate_{}_utf8_bytes\")",
            field.name
        ));
    }
    (!validators.is_empty()).then(|| format!("#[validate({})]", validators.join(", ")))
}

pub(super) fn schema_attribute(field: &FieldIr) -> Option<String> {
    let mut parts = Vec::new();
    if let Some(minimum) = field.validation.min_utf8_bytes {
        parts.push(format!("min_length = {minimum}"));
    }
    if let Some(maximum) = field.validation.max_utf8_bytes {
        parts.push(format!("max_length = {maximum}"));
    }
    (!parts.is_empty()).then(|| format!("#[schema({})]", parts.join(", ")))
}
