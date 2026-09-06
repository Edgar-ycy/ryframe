use crate::TenantDatabaseRouter;
use ryframe_application::ports::backup::*;
use ryframe_config::{TenantDatabaseTargetKind, TenantDatabaseTargetMode};
use ryframe_db::{ControlDatabaseCluster, backup_verification as verify};
use ryframe_kernel::{AppError, AppResult};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

mod inventory;
use inventory::canonical_placements;

pub fn verifier(
    control: ControlDatabaseCluster,
    router: Arc<TenantDatabaseRouter>,
    scope_id: String,
) -> Arc<dyn BackupDatabaseVerifier> {
    Arc::new(DatabaseVerification {
        control,
        router,
        scope_id,
    })
}

struct DatabaseVerification {
    control: ControlDatabaseCluster,
    router: Arc<TenantDatabaseRouter>,
    scope_id: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct DatabaseShape {
    pub(super) kind: BackupDatabaseKind,
    pub(super) shared: bool,
    pub(super) control: bool,
}

#[async_trait::async_trait]
impl BackupDatabaseVerifier for DatabaseVerification {
    async fn target_inventory(&self, key: &str) -> AppResult<DatabaseTargetInventory> {
        self.complete_target_inventory(key).await
    }

    async fn validate_restore_targets(
        &self,
        manifest: &BackupManifest,
        plan: &RestorePlan,
    ) -> AppResult<()> {
        self.validate_restore_contract(manifest, plan)?;
        for target in &plan.databases {
            let expected = expected_database(manifest, &target.source_key)?;
            let actual = self.target_snapshot(&target.target_key).await?;
            validate_physical_target(manifest, target, &actual)?;
            validate_database_shape(expected.kind, expected.shared, actual.kind, actual.shared)?;
        }
        require_safe_overwrite_proof()
    }

    async fn snapshot(&self) -> AppResult<Vec<DatabaseBackup>> {
        let resources =
            ryframe_db::repositories::backup_repo::required_resources(self.control.write()).await?;
        let mut databases = Vec::new();
        let mut identities = BTreeSet::new();
        for key in resources
            .iter()
            .filter_map(|resource| resource.strip_prefix("db:"))
        {
            let snapshot = self.target_snapshot(key).await?;
            if !identities.insert((
                snapshot.server_uuid.clone(),
                snapshot.database.to_lowercase(),
            )) {
                return Err(AppError::Validation(
                    "多个目标引用同一个物理数据库，不能重复登记备份".into(),
                ));
            }
            databases.push(snapshot);
        }
        Ok(databases)
    }

    async fn restored_databases(
        &self,
        manifest: &BackupManifest,
        plan: &RestorePlan,
    ) -> AppResult<()> {
        self.validate_restore_contract(manifest, plan)?;
        for target in &plan.databases {
            let expected = expected_database(manifest, &target.source_key)?;
            let actual = self.target_snapshot(&target.target_key).await?;
            validate_physical_target(manifest, target, &actual)?;
            validate_database_shape(expected.kind, expected.shared, actual.kind, actual.shared)?;
            if canonical_tables(expected.tables.clone())? != canonical_tables(actual.tables)?
                || canonical_placements(expected.placements.clone())?
                    != canonical_placements(actual.placements)?
            {
                return Err(AppError::Validation(
                    "恢复库的数据或租户关系完整校验失败".into(),
                ));
            }
        }
        Ok(())
    }
}

impl DatabaseVerification {
    fn validate_restore_contract(
        &self,
        manifest: &BackupManifest,
        plan: &RestorePlan,
    ) -> AppResult<()> {
        validate_scope_and_schema(&self.scope_id, manifest, plan)?;
        let expected = expected_database_map(manifest)?;
        if expected.len() != plan.databases.len() {
            return Err(AppError::Validation(
                "恢复目标必须完整覆盖备份数据库".into(),
            ));
        }
        let mut planned = BTreeSet::new();
        for target in &plan.databases {
            validate_logical_target_key(&target.source_key, &target.target_key)?;
            if !planned.insert(target.source_key.as_str()) {
                return Err(AppError::Validation("恢复目标键重复".into()));
            }
            let database = expected
                .get(target.source_key.as_str())
                .ok_or_else(|| AppError::Validation("恢复目标没有对应备份".into()))?;
            let configured = self.configured_shape(&target.target_key)?;
            validate_database_shape(
                database.kind,
                database.shared,
                configured.kind,
                configured.shared,
            )?;
        }
        if planned != expected.keys().copied().collect() {
            return Err(AppError::Validation(
                "恢复目标必须精确覆盖备份数据库".into(),
            ));
        }
        Ok(())
    }

    fn configured_shape(&self, key: &str) -> AppResult<DatabaseShape> {
        let targets = self.router.targets();
        let kind = targets
            .target_kind(key)
            .ok_or_else(|| AppError::Validation("恢复目标未在当前配置登记".into()))?;
        let mode = targets
            .target_mode(key)
            .ok_or_else(|| AppError::Validation("恢复目标未在当前配置登记".into()))?;
        database_shape(kind, mode)
    }
}

pub(super) fn database_shape(
    kind: TenantDatabaseTargetKind,
    mode: TenantDatabaseTargetMode,
) -> AppResult<DatabaseShape> {
    match (kind, mode) {
        (TenantDatabaseTargetKind::Control, TenantDatabaseTargetMode::Shared) => {
            Ok(DatabaseShape {
                kind: BackupDatabaseKind::Combined,
                shared: true,
                control: true,
            })
        }
        (TenantDatabaseTargetKind::Mysql, TenantDatabaseTargetMode::Shared) => Ok(DatabaseShape {
            kind: BackupDatabaseKind::Tenant,
            shared: true,
            control: false,
        }),
        (TenantDatabaseTargetKind::Mysql, TenantDatabaseTargetMode::Dedicated) => {
            Ok(DatabaseShape {
                kind: BackupDatabaseKind::Tenant,
                shared: false,
                control: false,
            })
        }
        (TenantDatabaseTargetKind::Control, TenantDatabaseTargetMode::Dedicated) => Err(
            AppError::Validation("控制库恢复目标必须使用共享组合模式".into()),
        ),
    }
}

fn validate_scope_and_schema(
    scope_id: &str,
    manifest: &BackupManifest,
    plan: &RestorePlan,
) -> AppResult<()> {
    if plan.scope_id != scope_id || plan.scope_id == manifest.scope_id {
        return Err(AppError::Validation(
            "恢复配置未使用已登记隔离 scope".into(),
        ));
    }
    validate_schema_fingerprints(
        &manifest.control_schema_fingerprint,
        &manifest.tenant_schema_fingerprint,
    )
}

fn validate_schema_fingerprints(control: &str, tenant: &str) -> AppResult<()> {
    if control != ryframe_db::migration::schema_fingerprint()
        || tenant != crate::migration::tenant_data_schema_fingerprint()
    {
        return Err(AppError::Validation(
            "恢复清单与当前编译期 schema 指纹不匹配".into(),
        ));
    }
    Ok(())
}

fn expected_database_map(manifest: &BackupManifest) -> AppResult<BTreeMap<&str, &DatabaseBackup>> {
    let mut databases = BTreeMap::new();
    for database in &manifest.databases {
        if databases.insert(database.key.as_str(), database).is_some() {
            return Err(AppError::Validation("备份数据库目标键重复".into()));
        }
    }
    if databases.is_empty() {
        return Err(AppError::Validation("备份数据库清单为空".into()));
    }
    Ok(databases)
}

fn expected_database<'a>(manifest: &'a BackupManifest, key: &str) -> AppResult<&'a DatabaseBackup> {
    manifest
        .databases
        .iter()
        .find(|database| database.key == key)
        .ok_or_else(|| AppError::Validation("恢复目标没有对应备份".into()))
}

fn validate_logical_target_key(source: &str, target: &str) -> AppResult<()> {
    if source != target {
        return Err(AppError::Validation(
            "恢复配置需保持逻辑 target key，只改变隔离物理目标".into(),
        ));
    }
    Ok(())
}

fn validate_database_shape(
    expected_kind: BackupDatabaseKind,
    expected_shared: bool,
    actual_kind: BackupDatabaseKind,
    actual_shared: bool,
) -> AppResult<()> {
    if expected_kind != actual_kind || expected_shared != actual_shared {
        return Err(AppError::Validation(
            "恢复目标的数据库类型或共享模式与备份不匹配".into(),
        ));
    }
    Ok(())
}

fn validate_physical_target(
    manifest: &BackupManifest,
    target: &RestoreDatabase,
    actual: &DatabaseBackup,
) -> AppResult<()> {
    let source_reused = manifest.databases.iter().any(|source| {
        source.server_uuid == actual.server_uuid
            && source.database.eq_ignore_ascii_case(&actual.database)
    });
    if actual.server_uuid != target.server_uuid
        || actual.database != target.database
        || source_reused
    {
        return Err(AppError::Validation(
            "隔离恢复物理目标不匹配或指向原业务库".into(),
        ));
    }
    Ok(())
}

fn canonical_tables(mut tables: Vec<BackupTableDigest>) -> AppResult<Vec<BackupTableDigest>> {
    tables.sort_by(|left, right| left.table.cmp(&right.table));
    if tables.is_empty() || tables.windows(2).any(|pair| pair[0].table == pair[1].table) {
        return Err(AppError::Validation("数据表清单为空或重复".into()));
    }
    Ok(tables)
}

fn require_safe_overwrite_proof() -> AppResult<()> {
    Err(AppError::Validation(
        "恢复目标缺少已绑定的初始化前像，禁止覆盖现有业务数据".into(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn database_shape_is_bound_to_kind_and_mode() {
        let combined = database_shape(
            TenantDatabaseTargetKind::Control,
            TenantDatabaseTargetMode::Shared,
        )
        .unwrap();
        assert_eq!(combined.kind, BackupDatabaseKind::Combined);
        assert!(combined.shared && combined.control);

        let shared = database_shape(
            TenantDatabaseTargetKind::Mysql,
            TenantDatabaseTargetMode::Shared,
        )
        .unwrap();
        assert_eq!(shared.kind, BackupDatabaseKind::Tenant);
        assert!(shared.shared && !shared.control);

        let dedicated = database_shape(
            TenantDatabaseTargetKind::Mysql,
            TenantDatabaseTargetMode::Dedicated,
        )
        .unwrap();
        assert_eq!(dedicated.kind, BackupDatabaseKind::Tenant);
        assert!(!dedicated.shared && !dedicated.control);
        assert!(
            database_shape(
                TenantDatabaseTargetKind::Control,
                TenantDatabaseTargetMode::Dedicated,
            )
            .is_err()
        );
        assert!(
            validate_database_shape(
                BackupDatabaseKind::Tenant,
                true,
                dedicated.kind,
                dedicated.shared,
            )
            .is_err()
        );
    }

    #[test]
    fn schema_fingerprints_are_bound_before_target_inspection() {
        let control = ryframe_db::migration::schema_fingerprint();
        let tenant = crate::migration::tenant_data_schema_fingerprint();
        assert!(validate_schema_fingerprints(&control, tenant).is_ok());
        assert!(validate_schema_fingerprints("changed", tenant).is_err());
        assert!(validate_schema_fingerprints(&control, "changed").is_err());
    }

    #[test]
    fn logical_target_key_cannot_be_remapped() {
        assert!(validate_logical_target_key("shared-control", "shared-control").is_ok());
        assert!(validate_logical_target_key("source", "target").is_err());
    }

    #[test]
    fn restore_preflight_stays_closed_without_bound_initialization_preimage() {
        let error = require_safe_overwrite_proof().unwrap_err();
        assert!(matches!(
            error,
            AppError::Validation(message)
                if message == "恢复目标缺少已绑定的初始化前像，禁止覆盖现有业务数据"
        ));
    }
}
