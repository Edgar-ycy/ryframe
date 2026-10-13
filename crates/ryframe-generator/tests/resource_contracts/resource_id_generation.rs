use ryframe_generator::{
    IdGeneration, IndexSpec, ResourceSpec, normalize_resource, render_resources,
};

fn tenant_spec() -> ResourceSpec {
    ResourceSpec::parse(include_str!("../fixtures/order.toml"), "order.toml").unwrap()
}

fn auto_spec() -> ResourceSpec {
    let mut spec = tenant_spec();
    spec.database.id_generation = IdGeneration::AutoIncrement;
    spec.database.indexes.push(IndexSpec {
        name: "idx_order_auto_id".into(),
        fields: vec!["id".into()],
        unique: false,
    });
    spec
}

#[test]
fn auto_increment_keeps_database_fake_service_and_catalog_consistent() {
    let resource = normalize_resource(auto_spec(), "order.toml", "auto").unwrap();
    let snowflake = normalize_resource(tenant_spec(), "order.toml", "snowflake").unwrap();
    assert_ne!(resource.schema_hash, snowflake.schema_hash);
    let generated = render_resources(&[resource]).unwrap();
    let asset = |suffix: &str| {
        generated
            .assets
            .iter()
            .find(|asset| asset.path.ends_with(suffix))
            .unwrap()
            .content
            .as_str()
    };
    assert!(asset("order/migration.rs").contains("`id` BIGINT NOT NULL AUTO_INCREMENT"));
    assert!(asset("order/migration.rs").contains("PRIMARY KEY (`tenant_id`, `id`)"));
    assert!(!asset("order/entity.rs").contains("auto_increment = true"));
    let repository = asset("order/repository.rs");
    assert!(repository.contains("active.id = sea_orm::ActiveValue::NotSet"));
    assert!(repository.contains(".exec_without_returning(&self.transaction)"));
    assert!(repository.contains("Column::TenantId.eq(tenant_id)"));
    assert!(repository.contains("LAST_INSERT_ID()"));
    let service = asset("order/service.rs");
    assert!(service.contains("id: 0,"));
    assert!(!service.contains("next_id()"));
    assert!(asset("order/fake.rs").contains("record.id = state.next_id"));
    assert!(asset("order/fake.rs").contains("checked_add(1)"));
    assert!(asset("generated/catalog.rs").contains("auto_increment"));
    for asset in generated
        .assets
        .iter()
        .filter(|asset| asset.path.ends_with(".rs"))
    {
        syn::parse_file(&asset.content).unwrap_or_else(|error| panic!("{}: {error}", asset.path));
    }
}

#[test]
fn auto_increment_requires_id_first_index_and_disallows_user_supplied_ids() {
    let mut spec = tenant_spec();
    spec.database.id_generation = IdGeneration::AutoIncrement;
    assert!(
        normalize_resource(spec, "order.toml", "missing-index")
            .unwrap_err()
            .to_string()
            .contains("索引的首列")
    );
    let mut spec = auto_spec();
    spec.fields
        .iter_mut()
        .find(|field| field.name == "id")
        .unwrap()
        .default = Some(toml::Value::Integer(0));
    assert!(
        normalize_resource(spec, "order.toml", "default-id")
            .unwrap_err()
            .to_string()
            .contains("自增 ID")
    );
    let mut spec = auto_spec();
    let id = spec
        .fields
        .iter_mut()
        .find(|field| field.name == "id")
        .unwrap();
    id.usage.create = true;
    assert!(normalize_resource(spec, "order.toml", "editable-id").is_err());
}

#[test]
fn default_snowflake_still_supplies_application_id_without_auto_increment() {
    let resource = normalize_resource(tenant_spec(), "order.toml", "snowflake").unwrap();
    assert_eq!(resource.id_generation, IdGeneration::Snowflake);
    let generated = render_resources(&[resource]).unwrap();
    let service = generated
        .assets
        .iter()
        .find(|asset| asset.path.ends_with("order/service.rs"))
        .unwrap();
    assert!(service.content.contains("next_id()"));
    assert!(
        !generated
            .assets
            .iter()
            .any(|asset| asset.content.contains("LAST_INSERT_ID()"))
    );
}
