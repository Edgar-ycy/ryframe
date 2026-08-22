use ryframe_application::ports::tenants::TenantProvisioningPlacement;
use ryframe_db::entities::tenant;
use ryframe_tenant_db::application_ports::tenants::{
    map_tenant, map_tenant_model, to_application_placement, to_infrastructure_placement,
};

#[test]
fn tenant_mapping_preserves_every_field() {
    let now = "2026-08-21T01:02:03Z".parse().unwrap();
    let model = tenant::Model {
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
        runtime_epoch: 5,
        configuration_version: 6,
        created_at: now,
        updated_at: now,
    };

    assert_eq!(map_tenant_model(map_tenant(model.clone())), model);
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
