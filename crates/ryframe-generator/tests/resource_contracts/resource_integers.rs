use ryframe_generator::{ResourceSpec, ValueType, normalize_resource, render_resources};

fn spec(value_type: ValueType) -> ResourceSpec {
    let mut spec =
        ResourceSpec::parse(include_str!("../fixtures/order.toml"), "order.toml").unwrap();
    let field = spec
        .fields
        .iter_mut()
        .find(|field| field.name == "status")
        .unwrap();
    field.value_type = value_type;
    field.wire_type = None;
    field.usage.create_optional = true;
    field.validation.required = false;
    field.validation.minimum = None;
    field.validation.maximum = None;
    spec
}

#[test]
fn integer_kinds_preserve_dto_defaults_enum_and_sql_types() {
    for (kind, rust_type, sql_type) in [
        (ValueType::I8, "i8", "TINYINT"),
        (ValueType::I16, "i16", "SMALLINT"),
        (ValueType::U8, "u8", "TINYINT UNSIGNED"),
        (ValueType::U16, "u16", "SMALLINT UNSIGNED"),
        (ValueType::U32, "u32", "INT UNSIGNED"),
    ] {
        let resource = normalize_resource(spec(kind), "order.toml", "integer").unwrap();
        let field = resource
            .fields
            .iter()
            .find(|field| field.name == "status")
            .unwrap();
        assert_eq!(field.rust_type, rust_type);
        assert_eq!(
            (field.validation.minimum, field.validation.maximum),
            kind.integer_bounds()
                .map(|(min, max)| (
                    Some(i64::try_from(min).unwrap()),
                    Some(i64::try_from(max).unwrap())
                ))
                .unwrap()
        );
        let generated = render_resources(&[resource]).unwrap();
        let content = |suffix: &str| {
            &generated
                .assets
                .iter()
                .find(|asset| asset.path.ends_with(suffix))
                .unwrap()
                .content
        };
        assert!(content("order/migration.rs").contains(&format!("`status` {sql_type}")));
        assert!(content("order/dto.rs").contains(&format!("Option<{rust_type}>")));
        assert!(content("order/dto.rs").contains(&format!("value: {rust_type}")));
        assert!(content("order/service.rs").contains(&format!("unwrap_or(1_{rust_type})")));
        let catalog = &generated
            .assets
            .iter()
            .find(|asset| asset.path == "crates/ryframe-tenant-db/src/generated/catalog.rs")
            .unwrap()
            .content;
        assert!(catalog.contains(&sql_type.to_ascii_lowercase()));
    }
}

#[test]
fn integer_default_enum_and_validation_must_fit_the_declared_kind() {
    for kind in [
        ValueType::I8,
        ValueType::I16,
        ValueType::U8,
        ValueType::U16,
        ValueType::U32,
    ] {
        let (minimum, maximum) = kind.integer_bounds().unwrap();
        for invalid in [minimum - 1, maximum + 1] {
            let mut input = spec(kind);
            let field = input
                .fields
                .iter_mut()
                .find(|field| field.name == "status")
                .unwrap();
            field.default = Some(toml::Value::Integer(i64::try_from(invalid).unwrap()));
            assert!(
                normalize_resource(input, "order.toml", "default")
                    .unwrap_err()
                    .to_string()
                    .contains("字段 status")
            );
            let mut input = spec(kind);
            let field = input
                .fields
                .iter_mut()
                .find(|field| field.name == "status")
                .unwrap();
            let labels = field.enum_values.values().next().unwrap().clone();
            field.enum_values.insert(invalid.to_string(), labels);
            assert!(
                normalize_resource(input, "order.toml", "enum")
                    .unwrap_err()
                    .to_string()
                    .contains("枚举键")
            );
            let mut input = spec(kind);
            input
                .fields
                .iter_mut()
                .find(|field| field.name == "status")
                .unwrap()
                .validation
                .maximum = Some(i64::try_from(invalid).unwrap());
            assert!(
                normalize_resource(input, "order.toml", "range")
                    .unwrap_err()
                    .to_string()
                    .contains("范围")
            );
        }
    }
}

#[test]
fn explicit_sql_integer_range_cannot_overflow_the_rust_model() {
    for (kind, sql_type, accepted) in [
        (ValueType::I8, "TINYINT", true),
        (ValueType::I16, "TINYINT", true),
        (ValueType::I8, "SMALLINT", false),
        (ValueType::I16, "INT", false),
        (ValueType::U8, "TINYINT UNSIGNED", true),
        (ValueType::U8, "TINYINT", false),
        (ValueType::I32, "INT UNSIGNED", false),
        (ValueType::U32, "MEDIUMINT UNSIGNED", true),
    ] {
        let mut input = spec(kind);
        input
            .fields
            .iter_mut()
            .find(|field| field.name == "status")
            .unwrap()
            .sql_type = Some(sql_type.into());
        assert_eq!(
            normalize_resource(input, "order.toml", "physical").is_ok(),
            accepted,
            "{kind:?}/{sql_type}"
        );
    }
}

#[test]
fn integer_enum_keys_must_fit_the_physical_range_and_explicit_validation() {
    let mut input = spec(ValueType::I32);
    let field = input
        .fields
        .iter_mut()
        .find(|field| field.name == "status")
        .unwrap();
    field.sql_type = Some("TINYINT".into());
    let labels = field.enum_values.values().next().unwrap().clone();
    field.enum_values.insert("200".into(), labels);
    assert!(
        normalize_resource(input, "order.toml", "physical-enum")
            .unwrap_err()
            .to_string()
            .contains("枚举键")
    );
    let mut input = spec(ValueType::U16);
    input
        .fields
        .iter_mut()
        .find(|field| field.name == "status")
        .unwrap()
        .validation
        .maximum = Some(0);
    assert!(
        normalize_resource(input, "order.toml", "explicit-enum")
            .unwrap_err()
            .to_string()
            .contains("枚举键")
    );
}

#[test]
fn signed_i64_string_wire_stays_text_in_dto_schema_form_and_enum_options() {
    let mut input = spec(ValueType::I64);
    let field = input
        .fields
        .iter_mut()
        .find(|field| field.name == "status")
        .unwrap();
    field.wire_type = Some(ValueType::String);
    let resource = normalize_resource(input.clone(), "order.toml", "exact").unwrap();
    let generated = render_resources(&[resource]).unwrap();
    let content = |suffix: &str| {
        &generated
            .assets
            .iter()
            .find(|asset| asset.path.ends_with(suffix))
            .unwrap()
            .content
    };
    assert!(content("order/dto.rs").contains("deserialize_optional_i64"));
    assert!(content("order/dto.rs").contains("value_type = Option<String>"));
    assert!(content("order/dto.rs").contains("crate::http::integer::parse_i64"));
    let form = content("order/fields.ts");
    assert!(form.contains("value: \"1\"") && !form.contains("value: 1,"));
    assert!(form.contains("status: ''"));
    assert!(form.contains("form.status === '' ? undefined : form.status"));
    let field = input
        .fields
        .iter_mut()
        .find(|field| field.name == "status")
        .unwrap();
    field.widget = ryframe_generator::WidgetSpec::Number;
    assert!(
        normalize_resource(input, "order.toml", "unsafe-widget")
            .unwrap_err()
            .to_string()
            .contains("丢失精度")
    );
}
