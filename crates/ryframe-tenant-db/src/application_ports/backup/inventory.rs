use super::{DatabaseShape, DatabaseVerification, database_shape, verify};
use crate::TenantDataTargetHandle;
use ryframe_application::ports::backup::{
    BackupPlacement, DatabaseBackup, DatabaseTargetInventory,
};
use ryframe_config::TenantDatabaseTargetKind;
use ryframe_db::DbResultExt;
use ryframe_kernel::{AppError, AppResult};
use sea_orm::{AccessMode, DatabaseTransaction, IsolationLevel, TransactionTrait};
use std::collections::BTreeSet;

impl DatabaseVerification {
    pub(super) async fn complete_target_inventory(
        &self,
        key: &str,
    ) -> AppResult<DatabaseTargetInventory> {
        self.verify_target_schema(key).await?;
        let handle = self
            .router
            .open_target(key)
            .await
            .map_err(super::super::map_error)?;
        let shape = database_shape(handle.kind(), handle.mode())?;
        let external_placements = if shape.control {
            None
        } else {
            Some(self.control_placements(key).await?)
        };
        let transaction = handle
            .connection()
            .begin_with_config(
                Some(IsolationLevel::RepeatableRead),
                Some(AccessMode::ReadOnly),
            )
            .await
            .db()?;
        let result = self
            .inventory_in_transaction(&transaction, &handle, shape, external_placements.as_deref())
            .await;
        let rollback = transaction.rollback().await.db();
        let inventory = result?;
        rollback?;
        self.verify_target_schema(key).await?;
        if let Some(before) = external_placements {
            let after = self.control_placements(key).await?;
            verify_placement_stability(&before, &after)?;
        }
        Ok(inventory)
    }

    pub(super) async fn target_snapshot(&self, key: &str) -> AppResult<DatabaseBackup> {
        Ok(self.complete_target_inventory(key).await?.database)
    }

    async fn inventory_in_transaction(
        &self,
        transaction: &DatabaseTransaction,
        handle: &TenantDataTargetHandle,
        shape: DatabaseShape,
        external_placements: Option<&[BackupPlacement]>,
    ) -> AppResult<DatabaseTargetInventory> {
        let tables = target_tables(shape.control)?;
        verify::verify_table_set(transaction, &tables.all).await?;
        verify_ownership(transaction, &self.scope_id, shape.control).await?;
        let (server_uuid, database) = verify::physical_identity(transaction).await?;
        let placements = match external_placements {
            Some(placements) => placements.to_vec(),
            None => {
                canonical_placements(verify::placements(transaction, handle.target_key()).await?)?
            }
        };
        let database = DatabaseBackup {
            key: handle.target_key().into(),
            kind: shape.kind,
            server_uuid,
            database,
            shared: shape.shared,
            placements,
            tables: verify::tables(transaction, &tables.business).await?,
        };
        let preserved_tables = verify::tables(transaction, &tables.preserved).await?;
        verify_ownership_count(&preserved_tables, shape.control)?;
        Ok(DatabaseTargetInventory {
            database,
            preserved_tables,
        })
    }

    async fn verify_target_schema(&self, key: &str) -> AppResult<()> {
        self.router
            .verify_target_now(key)
            .await
            .map_err(super::super::map_error)?;
        if self.router.targets().target_kind(key) == Some(TenantDatabaseTargetKind::Control) {
            ryframe_db::migration::verify(self.control.write())
                .await
                .db()?;
        }
        Ok(())
    }

    async fn control_placements(&self, key: &str) -> AppResult<Vec<BackupPlacement>> {
        canonical_placements(verify::placements(self.control.write(), key).await?)
    }
}

struct TargetTables {
    business: Vec<String>,
    preserved: Vec<String>,
    all: Vec<String>,
}

fn target_tables(control: bool) -> AppResult<TargetTables> {
    let mut business = if control {
        verify::control_tables()?
    } else {
        Vec::new()
    };
    business.extend([
        "biz_tenant_fence".to_owned(),
        "biz_tenant_target_slot".to_owned(),
    ]);
    business.extend(
        crate::migration::TENANT_DATA_CATALOG
            .tables()
            .iter()
            .map(|table| table.table.to_owned()),
    );
    canonical_table_names(&mut business)?;

    let mut preserved = if control {
        verify::preserved_control_tables()?
    } else {
        vec![ryframe_db::resource_ownership::RESOURCE_OWNERSHIP_TABLE.into()]
    };
    preserved.push(crate::migration::TENANT_DATA_MIGRATION_LEDGER.into());
    canonical_table_names(&mut preserved)?;

    let business_names = business.iter().map(String::as_str).collect::<BTreeSet<_>>();
    if preserved
        .iter()
        .any(|table| business_names.contains(table.as_str()))
    {
        return Err(AppError::Internal("备份表分组存在重复职责".into()));
    }
    let mut all = business.clone();
    all.extend(preserved.iter().cloned());
    canonical_table_names(&mut all)?;
    Ok(TargetTables {
        business,
        preserved,
        all,
    })
}

fn canonical_table_names(tables: &mut [String]) -> AppResult<()> {
    tables.sort();
    if tables.is_empty() || tables.windows(2).any(|pair| pair[0] == pair[1]) {
        return Err(AppError::Internal("备份表清单为空或重复".into()));
    }
    Ok(())
}

pub(super) fn canonical_placements(
    mut placements: Vec<BackupPlacement>,
) -> AppResult<Vec<BackupPlacement>> {
    placements.sort_by(|left, right| {
        (&left.tenant_id, left.generation, &left.switch_token).cmp(&(
            &right.tenant_id,
            right.generation,
            &right.switch_token,
        ))
    });
    if placements
        .windows(2)
        .any(|pair| pair[0].tenant_id == pair[1].tenant_id)
    {
        return Err(AppError::Validation("租户 placement 包含重复租户".into()));
    }
    Ok(placements)
}

fn verify_placement_stability(
    before: &[BackupPlacement],
    after: &[BackupPlacement],
) -> AppResult<()> {
    if before != after {
        return Err(AppError::Conflict(
            "库存采集期间跨库租户 placement 已变化".into(),
        ));
    }
    Ok(())
}

async fn verify_ownership(
    transaction: &DatabaseTransaction,
    scope_id: &str,
    control: bool,
) -> AppResult<()> {
    if control {
        ryframe_db::resource_ownership::verify_resource_ownership(transaction, scope_id, "control")
            .await
            .db()?;
    }
    ryframe_db::resource_ownership::verify_resource_ownership(transaction, scope_id, "tenant-data")
        .await
        .db()
}

fn verify_ownership_count(
    preserved: &[ryframe_application::ports::backup::BackupTableDigest],
    control: bool,
) -> AppResult<()> {
    let owner = preserved
        .iter()
        .find(|table| table.table == ryframe_db::resource_ownership::RESOURCE_OWNERSHIP_TABLE);
    if owner.map(|table| table.rows) != Some(if control { 2 } else { 1 }) {
        return Err(AppError::Validation(
            "目标 ownership 存在额外或缺失的归属记录".into(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_target_inventory_partitions_every_declared_table_once() {
        let control = target_tables(true).unwrap();
        let business = control
            .business
            .iter()
            .map(String::as_str)
            .collect::<BTreeSet<_>>();
        let preserved = control
            .preserved
            .iter()
            .map(String::as_str)
            .collect::<BTreeSet<_>>();
        assert!(business.is_disjoint(&preserved));
        assert_eq!(control.all.len(), business.len() + preserved.len());
        for table in [
            "biz_tenant_fence",
            "biz_tenant_target_slot",
            "sys_tenant_data_placement",
        ] {
            assert!(business.contains(table));
        }
        for table in [
            "ryframe_resource_ownership",
            "seaql_migrations",
            "seaql_tenant_data_migrations",
            "sys_backup_set",
            "sys_backup_resource",
            "sys_restore_run",
        ] {
            assert!(preserved.contains(table));
        }

        let tenant = target_tables(false).unwrap();
        assert_eq!(
            tenant.preserved,
            ["ryframe_resource_ownership", "seaql_tenant_data_migrations"]
        );
        assert!(!tenant.business.is_empty());
        assert_eq!(
            tenant.all.len(),
            tenant.business.len() + tenant.preserved.len()
        );
    }

    #[test]
    fn placements_use_rust_order_and_reject_duplicate_tenants() {
        let placement = |tenant: &str, generation: i64, token: &str| BackupPlacement {
            tenant_id: tenant.into(),
            generation,
            switch_token: token.into(),
        };
        let sorted = canonical_placements(vec![
            placement("tenant_z", 1, "token-z"),
            placement("tenant-a", 2, "token-a"),
        ])
        .unwrap();
        assert_eq!(sorted[0].tenant_id, "tenant-a");
        assert_eq!(sorted[1].tenant_id, "tenant_z");
        assert!(
            canonical_placements(vec![
                placement("tenant-a", 1, "token-a"),
                placement("tenant-a", 2, "token-b"),
            ])
            .is_err()
        );
        assert!(verify_placement_stability(&sorted, &sorted).is_ok());
        let changed = canonical_placements(vec![
            placement("tenant_z", 2, "token-new"),
            placement("tenant-a", 2, "token-a"),
        ])
        .unwrap();
        assert!(verify_placement_stability(&sorted, &changed).is_err());
    }
}
