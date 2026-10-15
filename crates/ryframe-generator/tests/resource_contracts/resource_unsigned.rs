use ryframe_generator::{
    ResourceSpec, ValueType, WidgetSpec, normalize_resource, render_resources,
};

fn spec() -> ResourceSpec {
    let mut input =
        ResourceSpec::parse(include_str!("../fixtures/order.toml"), "order.toml").unwrap();
    let field = input
        .fields
        .iter_mut()
        .find(|field| field.name == "status")
        .unwrap();
    field.value_type = ValueType::U64;
    field.wire_type = None;
    field.validation.minimum = None;
    field.validation.maximum = None;
    field.default = Some(toml::Value::String(u64::MAX.to_string()));
    let labels = field.enum_values.values().next().unwrap().clone();
    field.enum_values.insert(u64::MAX.to_string(), labels);
    field.usage.create_optional = true;
    field.validation.required = false;
    input
}

#[test]
fn unsigned_full_capacity_uses_native_storage_and_exact_wire_defaults() {
    let resource = normalize_resource(spec(), "order.toml", "unsigned").unwrap();
    let field = resource
        .fields
        .iter()
        .find(|field| field.name == "status")
        .unwrap();
    assert_eq!(field.rust_type, "u64");
    assert_eq!(field.wire_type, ValueType::String);
    assert_eq!(
        (field.validation.minimum, field.validation.maximum),
        (Some(0), None)
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
    assert!(content("order/migration.rs").contains("`status` BIGINT UNSIGNED"));
    assert!(content("order/service.rs").contains("unwrap_or(18446744073709551615_u64)"));
    let dto = content("order/dto.rs");
    assert!(dto.contains("deserialize_optional_u64"));
    assert!(dto.contains("value_type = Option<String>"));
    assert!(dto.contains("crate::http::integer::parse_u64"));
    assert!(dto.contains("18446744073709551615_u64"));
    assert!(content("order/fields.ts").contains("value: \"18446744073709551615\""));
}

#[test]
fn unsigned_defaults_enums_and_wire_reject_invalid_representations() {
    for invalid in ["-1", "-0", "01", "+1", "18446744073709551616"] {
        let mut input = spec();
        input
            .fields
            .iter_mut()
            .find(|field| field.name == "status")
            .unwrap()
            .default = Some(toml::Value::String(invalid.into()));
        assert!(
            normalize_resource(input, "order.toml", "default").is_err(),
            "{invalid}"
        );
        let mut input = spec();
        let field = input
            .fields
            .iter_mut()
            .find(|field| field.name == "status")
            .unwrap();
        field.default = None;
        let labels = field.enum_values.values().next().unwrap().clone();
        field.enum_values.insert(invalid.into(), labels);
        assert!(
            normalize_resource(input, "order.toml", "enum")
                .unwrap_err()
                .to_string()
                .contains("枚举键")
        );
    }
    let mut input = spec();
    input
        .fields
        .iter_mut()
        .find(|field| field.name == "status")
        .unwrap()
        .wire_type = Some(ValueType::U64);
    assert!(
        normalize_resource(input, "order.toml", "wire")
            .unwrap_err()
            .to_string()
            .contains("string 传输")
    );
    let mut input = spec();
    let field = input
        .fields
        .iter_mut()
        .find(|field| field.name == "status")
        .unwrap();
    field.widget = WidgetSpec::Number;
    field.enum_values.clear();
    assert!(
        normalize_resource(input, "order.toml", "widget")
            .unwrap_err()
            .to_string()
            .contains("丢失精度")
    );
}

#[test]
fn unsigned_physical_ranges_are_checked_without_signed_overflow() {
    for (sql_type, accepted, maximum) in [
        ("BIGINT UNSIGNED", true, None),
        ("INT UNSIGNED", true, Some(i64::from(u32::MAX))),
        ("BIGINT", false, None),
        ("INT", false, None),
    ] {
        let mut input = spec();
        let field = input
            .fields
            .iter_mut()
            .find(|field| field.name == "status")
            .unwrap();
        field.sql_type = Some(sql_type.into());
        field.default = None;
        field.usage.create_optional = false;
        field.enum_values.retain(|key, _| key == "1");
        let result = normalize_resource(input, "order.toml", "physical");
        assert_eq!(result.is_ok(), accepted, "{sql_type}");
        if let Ok(resource) = result {
            let field = resource
                .fields
                .iter()
                .find(|field| field.name == "status")
                .unwrap();
            assert_eq!(field.validation.maximum, maximum);
        }
    }
}

#[test]
fn unsigned_soft_delete_constants_accept_canonical_string_values() {
    let mut input = spec();
    let field = input
        .fields
        .iter_mut()
        .find(|field| field.name == "del_flag")
        .unwrap();
    field.value_type = ValueType::U64;
    field.default = Some(toml::Value::String("0".into()));
    let soft_delete = input.database.soft_delete.as_mut().unwrap();
    soft_delete.active = toml::Value::String("0".into());
    soft_delete.deleted = toml::Value::String("1".into());
    let resource = normalize_resource(input, "order.toml", "unsigned-soft-delete").unwrap();
    let generated = render_resources(&[resource]).unwrap();
    let entity = &generated
        .assets
        .iter()
        .find(|asset| asset.path.ends_with("order/entity.rs"))
        .unwrap()
        .content;
    assert!(entity.contains("SOFT_DELETE_ACTIVE: u64 = 0;"));
    assert!(entity.contains("SOFT_DELETE_DELETED: u64 = 1;"));
}
