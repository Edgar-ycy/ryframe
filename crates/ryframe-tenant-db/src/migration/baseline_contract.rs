#[cfg(all(test, feature = "migration"))]
pub(super) const BASELINE_MIGRATION_NAME: &str = "m20260820_000000_tenant_baseline";

#[cfg(any(not(feature = "migration"), test))]
pub(super) const RESOURCE_OWNERSHIP_SCHEMA_DESCRIPTOR: &str = "v2|table=\"ryframe_resource_ownership\"|engine=\"innodb\"|charset=\"utf8mb4\"|collation=\"utf8mb4_general_ci\"|columns=[\"resource_kind\":\"varchar(32)\":\"NO\":Some(\"ascii\"):Some(\"ascii_bin\"):\"PRI\":None:\"\":\"\";\"scope_id\":\"varchar(48)\":\"NO\":Some(\"ascii\"):Some(\"ascii_bin\"):\"MUL\":None:\"\":\"\";\"marker\":\"varchar(128)\":\"NO\":Some(\"ascii\"):Some(\"ascii_bin\"):\"UNI\":None:\"\":\"\";\"created_at\":\"datetime(6)\":\"NO\":None:None:\"\":Some(\"current_timestamp(6)\"):\"\":\"\";\"updated_at\":\"datetime(6)\":\"NO\":None:None:\"\":Some(\"current_timestamp(6)\"):\"on update current_timestamp(6)\":\"\";]|indexes=[\"PRIMARY\":\"resource_kind\":1:0:\"btree\":None:\"YES\";\"uq_resource_ownership_marker\":\"marker\":1:0:\"btree\":None:\"YES\";\"uq_resource_ownership_scope\":\"scope_id\":1:0:\"btree\":None:\"YES\";\"uq_resource_ownership_scope\":\"resource_kind\":2:0:\"btree\":None:\"YES\";]|constraints=[\"PRIMARY\":\"PRIMARY KEY\":\"YES\";\"uq_resource_ownership_marker\":\"UNIQUE\":\"YES\";\"uq_resource_ownership_scope\":\"UNIQUE\":\"YES\";]|checks=[]|foreign_keys=[]";

#[cfg(all(feature = "migration", not(test)))]
pub(super) use super::m20260820_000000_tenant_baseline::RESOURCE_OWNERSHIP_SCHEMA_DESCRIPTOR;

#[cfg(all(test, feature = "migration"))]
mod tests {
    #[test]
    fn read_only_contract_matches_frozen_baseline() {
        assert_eq!(
            super::RESOURCE_OWNERSHIP_SCHEMA_DESCRIPTOR,
            crate::migration::m20260820_000000_tenant_baseline::RESOURCE_OWNERSHIP_SCHEMA_DESCRIPTOR
        );
        use sea_orm_migration::MigrationName;
        assert_eq!(
            super::BASELINE_MIGRATION_NAME,
            crate::migration::m20260820_000000_tenant_baseline::Migration.name()
        );
    }
}
