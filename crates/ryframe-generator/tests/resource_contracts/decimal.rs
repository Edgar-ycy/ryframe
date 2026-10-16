use ryframe_generator::{
    ResourceSpec, ValueType, WidgetSpec, normalize_resource, render_resources,
};

fn spec() -> ResourceSpec {
    let mut spec =
        ResourceSpec::parse(include_str!("../fixtures/order.toml"), "order.toml").unwrap();
    let field = spec
        .fields
        .iter_mut()
        .find(|field| field.name == "status")
        .unwrap();
    field.value_type = ValueType::Decimal;
    field.sql_type = Some("decimal(20,6)".into());
    field.default = Some(toml::Value::String("0.125".into()));
    field.wire_type = None;
    field.enum_values.clear();
    field.widget = WidgetSpec::Text;
    field.validation.minimum = None;
    field.validation.maximum = None;
    field.validation.required = false;
    field.usage.create_optional = true;
    spec
}

#[test]
fn decimal_model_preserves_precision_defaults_and_all_transport_boundaries() {
    let resource = normalize_resource(spec(), "order.toml", "decimal").unwrap();
    let field = resource
        .fields
        .iter()
        .find(|field| field.name == "status")
        .unwrap();
    assert_eq!(field.rust_type, "rust_decimal::Decimal");
    assert_eq!(field.wire_type, ValueType::String);
    assert_eq!(field.default.as_ref().unwrap().as_str(), Some("0.125000"));
    let generated = render_resources(&[resource]).unwrap();
    let content = |suffix: &str| {
        &generated
            .assets
            .iter()
            .find(|asset| asset.path.ends_with(suffix))
            .unwrap()
            .content
    };
    let dto = content("order/dto.rs");
    assert!(dto.contains("deserialize_optional_decimal"));
    assert!(dto.contains("value_type = Option<String>"));
    assert!(dto.contains("parse_decimal_with_capacity(&value, 20, 6)"));
    assert!(dto.contains("validate_status_decimal"));
    assert!(content("order/migration.rs").contains("DECIMAL(20,6) NOT NULL DEFAULT '0.125000'"));
    let service = content("order/service.rs");
    assert!(service.contains("Decimal::from_i128_with_scale(125000_i128, 6)"));
    assert!(service.matches("decimal_fits_precision").count() >= 3);
    let fields = content("order/fields.ts");
    assert!(fields.contains("status: ''"));
    assert!(fields.contains("form.status === '' ? undefined : form.status"));
}

#[test]
fn unsupported_decimal_capacity_and_lossy_defaults_fail_before_generation() {
    for declaration in [
        None,
        Some("DECIMAL(29,6)"),
        Some("DECIMAL(65,30)"),
        Some("DECIMAL(20,21)"),
        Some("DOUBLE"),
        Some("DECIMAL(20,6); DROP TABLE table1"),
    ] {
        let mut input = spec();
        input
            .fields
            .iter_mut()
            .find(|field| field.name == "status")
            .unwrap()
            .sql_type = declaration.map(str::to_owned);
        let error = normalize_resource(input, "order.toml", "bad-type")
            .unwrap_err()
            .to_string();
        assert!(error.contains("status") && error.contains("未写入"));
    }
    for value in [
        "999999999999999.000000",
        "0.1234567",
        "1.0000000",
        "1e3",
        "+1",
        "01",
        "-0",
        "-0.000000",
    ] {
        let mut input = spec();
        input
            .fields
            .iter_mut()
            .find(|field| field.name == "status")
            .unwrap()
            .default = Some(toml::Value::String(value.into()));
        assert!(
            normalize_resource(input, "order.toml", "bad-default").is_err(),
            "{value}"
        );
    }
    let mut input = spec();
    input
        .fields
        .iter_mut()
        .find(|field| field.name == "status")
        .unwrap()
        .widget = WidgetSpec::Number;
    assert!(
        normalize_resource(input, "order.toml", "number")
            .unwrap_err()
            .to_string()
            .contains("丢失精度")
    );
    let mut input = spec();
    input
        .fields
        .iter_mut()
        .find(|field| field.name == "status")
        .unwrap()
        .wire_type = Some(ValueType::Decimal);
    assert!(normalize_resource(input, "order.toml", "wire").is_err());
}

#[test]
fn readonly_nullable_decimal_default_keeps_option_in_generated_record() {
    let mut input = spec();
    let field = input
        .fields
        .iter_mut()
        .find(|field| field.name == "status")
        .unwrap();
    field.nullable = true;
    field.usage.create = false;
    field.usage.create_optional = false;
    field.usage.update = false;
    let resource = normalize_resource(input, "order.toml", "nullable-default").unwrap();
    let output = render_resources(&[resource]).unwrap();
    let service = output
        .assets
        .iter()
        .find(|asset| asset.path.ends_with("order/service.rs"))
        .unwrap();
    assert!(
        service
            .content
            .contains("status: Some(rust_decimal::Decimal::from_i128_with_scale(125000_i128, 6))")
    );
}
