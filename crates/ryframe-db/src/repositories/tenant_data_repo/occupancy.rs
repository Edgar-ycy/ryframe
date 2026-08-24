use std::collections::HashSet;

use super::*;

impl TenantDataRepository {
    /// 只读控制库批量快照，供目标列表计算 dedicated 资格；不连接任何目标库。
    pub async fn occupied_target_keys<C>(
        &self,
        db: &C,
        configured_target_keys: &[String],
    ) -> AppResult<HashSet<String>>
    where
        C: ConnectionTrait,
    {
        let configured_target_keys = configured_target_keys
            .iter()
            .cloned()
            .collect::<HashSet<_>>();
        if configured_target_keys.is_empty() {
            return Ok(HashSet::new());
        }
        if configured_target_keys.len() > 200 {
            return Err(AppError::Config(
                "tenant-data target occupancy snapshot exceeds the configured 200-target limit"
                    .into(),
            ));
        }
        let configured_target_keys = configured_target_keys.into_iter().collect::<Vec<_>>();
        let result_limit = configured_target_keys.len() as u64;
        let placements = tenant_data_placement::Entity::find()
            .select_only()
            .distinct()
            .column(tenant_data_placement::Column::CurrentTargetKey)
            .filter(
                tenant_data_placement::Column::CurrentTargetKey
                    .is_in(configured_target_keys.clone()),
            )
            .filter(tenant_data_placement::Column::State.is_in([
                tenant_data_placement::Model::STATE_ACTIVE,
                tenant_data_placement::Model::STATE_MAINTENANCE,
                tenant_data_placement::Model::STATE_PROVISIONING,
            ]))
            .limit(result_limit)
            .into_tuple::<String>()
            .all(db)
            .await
            .map_err(database_error)?;
        let prepared = tenant_data_migration::Entity::find()
            .select_only()
            .distinct()
            .column(tenant_data_migration::Column::TargetKey)
            .filter(tenant_data_migration::Column::TargetKey.is_in(configured_target_keys.clone()))
            .filter(tenant_data_migration::Column::State.is_not_in([
                tenant_data_migration::Model::STATE_FINALIZED,
                tenant_data_migration::Model::STATE_FAILED,
                tenant_data_migration::Model::STATE_CANCELLED,
            ]))
            .limit(result_limit)
            .into_tuple::<String>()
            .all(db)
            .await
            .map_err(database_error)?;
        let retained_sources = tenant_data_migration::Entity::find()
            .select_only()
            .distinct()
            .column(tenant_data_migration::Column::SourceTargetKey)
            .filter(
                tenant_data_migration::Column::SourceTargetKey
                    .is_in(configured_target_keys.clone()),
            )
            .filter(tenant_data_migration::Column::State.is_not_in([
                tenant_data_migration::Model::STATE_FINALIZED,
                tenant_data_migration::Model::STATE_FAILED,
                tenant_data_migration::Model::STATE_CANCELLED,
            ]))
            .limit(result_limit)
            .into_tuple::<String>()
            .all(db)
            .await
            .map_err(database_error)?;
        Ok(placements
            .into_iter()
            .chain(prepared)
            .chain(retained_sources)
            .collect())
    }
}
