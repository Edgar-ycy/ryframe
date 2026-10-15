use super::{FieldIr, ResourceError, ResourceIr, ResourceSpec, ValueType};

pub(super) fn normalize(
    spec: &mut ResourceSpec,
    resource: &str,
    path: &str,
) -> Result<(), ResourceError> {
    let error = |message: String| {
        ResourceError::new(message,
        "排序规则须为当前 MySQL 可用的 utf8mb4 规则；其他字符集先审阅迁移或使用强类型扩展，生成器不会转换已有数据")
        .with_resource(resource).with_file(path)
    };
    normalize_name(&mut spec.database.collation).map_err(&error)?;
    for field in &mut spec.fields {
        if field.collation.is_some() && field.value_type != ValueType::String {
            return Err(error("只有字符串字段可以声明 collation".into()).with_field(&field.name));
        }
        normalize_name(&mut field.collation)
            .map_err(|message| error(message).with_field(&field.name))?;
    }
    Ok(())
}

fn normalize_name(value: &mut Option<String>) -> Result<(), String> {
    let Some(name) = value else {
        return Ok(());
    };
    let normalized = name.to_ascii_lowercase();
    if !normalized.starts_with("utf8mb4_")
        || normalized.len() <= 8
        || normalized.len() > 64
        || !normalized
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
    {
        return Err(format!(
            "排序规则 `{name}` 无效或不属于受支持的 utf8mb4 字符集；本批未写入"
        ));
    }
    *name = normalized;
    Ok(())
}

impl ResourceIr {
    pub(crate) fn table_collation(&self) -> &str {
        self.collation.as_deref().unwrap_or("utf8mb4_general_ci")
    }

    pub(crate) fn column_collation<'a>(&'a self, field: &'a FieldIr) -> &'a str {
        field
            .collation
            .as_deref()
            .unwrap_or_else(|| self.table_collation())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec() -> ResourceSpec {
        ResourceSpec::parse(
            include_str!("../../tests/fixtures/order.toml"),
            "order.toml",
        )
        .unwrap()
    }

    #[test]
    fn normalizes_table_and_column_collations_without_changing_implicit_defaults() {
        let mut spec = spec();
        normalize(&mut spec, "order", "order.toml").unwrap();
        assert!(spec.database.collation.is_none());
        spec.database.collation = Some("UTF8MB4_0900_AI_CI".into());
        spec.fields
            .iter_mut()
            .find(|field| field.name == "name")
            .unwrap()
            .collation = Some("utf8mb4_bin".into());
        normalize(&mut spec, "order", "order.toml").unwrap();
        let resource = crate::normalize_resource(spec, "order.toml", "collation").unwrap();
        assert_eq!(resource.table_collation(), "utf8mb4_0900_ai_ci");
        assert_eq!(
            resource.column_collation(
                resource
                    .fields
                    .iter()
                    .find(|field| field.name == "name")
                    .unwrap()
            ),
            "utf8mb4_bin"
        );
        assert_eq!(
            resource.column_collation(
                resource
                    .fields
                    .iter()
                    .find(|field| field.name == "tenant_id")
                    .unwrap()
            ),
            "utf8mb4_0900_ai_ci"
        );
    }

    #[test]
    fn rejects_sql_fragments_other_charsets_and_nonstring_columns_with_context() {
        for invalid in [
            "",
            "ascii_bin",
            "latin1_swedish_ci",
            "utf8mb4_",
            "utf8mb4_bin;DROP",
            "utf8mb4_bin --",
        ] {
            let mut spec = spec();
            spec.database.collation = Some(invalid.into());
            let error = normalize(&mut spec, "order", "order.toml")
                .unwrap_err()
                .to_string();
            for detail in [
                "资源 order",
                "order.toml",
                "本批未写入",
                "生成器不会转换已有数据",
            ] {
                assert!(error.contains(detail), "{error}");
            }
        }
        let mut spec = spec();
        spec.fields
            .iter_mut()
            .find(|field| field.name == "status")
            .unwrap()
            .collation = Some("utf8mb4_bin".into());
        assert!(
            normalize(&mut spec, "order", "order.toml")
                .unwrap_err()
                .to_string()
                .contains("字段 status")
        );
    }
}
