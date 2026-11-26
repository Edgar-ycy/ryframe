use ryframe_application::ports::tenants::TenantProvisioningPlacement;
use ryframe_db::entities::tenant;
use ryframe_kernel::AppError;
use ryframe_tenant_db::application_ports::tenants::{
    map_tenant, map_tenant_model, to_application_placement, to_infrastructure_placement,
};
use time::UtcOffset;

#[test]
fn tenant_mapping_preserves_every_field() {
    let now = "2026-08-21T01:02:03Z".parse().unwrap();
    let mut model = tenant::Model {
        id: 42,
        tenant_id: "tenant-a".to_owned(),
        name: "Tenant A".to_owned(),
        domain: Some("tenant.example.com".to_owned()),
        status: tenant::Model::STATUS_ENABLED.to_owned(),
        expire_at: None,
        max_users: 100,
        max_roles: 20,
        max_storage_mb: 1024,
        max_requests_per_min: 1000,
        session_version: 3,
        authorization_epoch: 4,
        data_scope_version: 1,
        runtime_epoch: 5,
        configuration_version: 6,
        created_at: now,
        updated_at: now,
    };

    let mapped = map_tenant(model.clone()).unwrap();
    assert_eq!(mapped.expire_at, None);
    assert_eq!(map_tenant_model(mapped).unwrap(), model);

    model.expire_at = Some("1969-12-31T23:59:59.123456789Z".parse().unwrap());
    let mut mapped = map_tenant(model.clone()).unwrap();
    let expires = mapped.expire_at.unwrap();
    assert_eq!(expires.unix_timestamp_nanos(), -876_543_211);
    assert_eq!(expires.offset(), UtcOffset::UTC);
    mapped.expire_at = Some(expires.to_offset(UtcOffset::from_hms(8, 0, 0).unwrap()));
    assert_eq!(map_tenant_model(mapped).unwrap(), model);

    model.expire_at = Some("2016-12-31T23:59:60.123456Z".parse().unwrap());
    assert!(matches!(
        map_tenant(model),
        Err(AppError::Internal(message)) if message == "租户响应的 expire_at 无法精确表示为 UTC"
    ));
}

#[test]
fn placement_mapping_preserves_all_fields() {
    let application = TenantProvisioningPlacement {
        tenant_id: "tenant-a".into(),
        target_key: "primary".into(),
        generation: 3,
        switch_token: "token".into(),
    };
    let infrastructure = to_infrastructure_placement(&application);
    assert_eq!(to_application_placement(infrastructure), application);
}
