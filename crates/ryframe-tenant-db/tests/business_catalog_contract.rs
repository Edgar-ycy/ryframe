use ryframe_application::ports::tenant_data::TenantDataTableDescriptor;
use ryframe_tenant_db::migration::{
    register_business_catalog, tenant_data_catalog, tenant_data_schema_fingerprint,
};

const TABLE: TenantDataTableDescriptor = TenantDataTableDescriptor {
    table: "biz_order",
    copy_order: 1,
    tenant_column: "tenant_id",
    primary_key_cursor_columns: &["tenant_id", "id"],
    checksum_columns: &["tenant_id", "id"],
    column_types: &["varchar", "bigint"],
    has_generated_columns: false,
    foreign_key_dependencies: &[],
    foreign_keys: &[],
    schema_canonical: "order-schema",
};

#[test]
fn frozen_catalog_is_shared_and_rejects_late_replacement() {
    register_business_catalog([TABLE], ["biz_catalog"]).unwrap();
    let fingerprint = tenant_data_schema_fingerprint();
    assert_eq!(tenant_data_catalog().tables(), &[TABLE]);
    assert_eq!(tenant_data_catalog().schema_fingerprint(), fingerprint);
    assert!(tenant_data_catalog().validate().is_ok());
    register_business_catalog([TABLE], ["biz_catalog"]).unwrap();
    assert!(register_business_catalog([], ["biz_catalog"]).is_err());
    assert!(
        register_business_catalog(
            [TenantDataTableDescriptor {
                schema_canonical: "changed-schema",
                ..TABLE
            }],
            ["biz_catalog"]
        )
        .is_err()
    );
    assert!(register_business_catalog([TABLE], ["biz_other_control"]).is_err());
    assert!(register_business_catalog([TABLE], ["biz_order"]).is_err());
    assert_eq!(tenant_data_catalog().tables(), &[TABLE]);
    assert_eq!(tenant_data_schema_fingerprint(), fingerprint);
}
