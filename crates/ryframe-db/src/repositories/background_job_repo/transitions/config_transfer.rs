use super::*;

impl BackgroundJobRepository {
    pub(super) async fn is_tenant_config_job_owner<C>(
        db: &C,
        job: &background_job::Model,
        tenant_id: &str,
        retry_requested_by: i64,
    ) -> AppResult<bool>
    where
        C: ConnectionTrait,
    {
        if job.tenant_id.as_deref() != Some(tenant_id) {
            return Ok(false);
        }
        match job.job_type.as_str() {
            TENANT_CONFIG_EXPORT_JOB_TYPE => {
                let Some(bundle_id) = linked_resource_id(job, "bundle_id") else {
                    return Ok(false);
                };
                tenant_config_bundle::Entity::find_by_id(bundle_id)
                    .filter(tenant_config_bundle::Column::TenantId.eq(tenant_id))
                    .filter(tenant_config_bundle::Column::BackgroundJobId.eq(job.id))
                    .filter(tenant_config_bundle::Column::CreatedBy.eq(retry_requested_by))
                    .lock(LockType::Update)
                    .one(db)
                    .await
                    .map(|bundle| bundle.is_some())
                    .map_err(database_error)
            }
            TENANT_CONFIG_PREVIEW_JOB_TYPE
            | TENANT_CONFIG_APPLY_JOB_TYPE
            | TENANT_CONFIG_ROLLBACK_JOB_TYPE => {
                let Some(transfer_id) = linked_resource_id(job, "transfer_id") else {
                    return Ok(false);
                };
                let query = tenant_config_transfer::Entity::find_by_id(transfer_id)
                    .filter(tenant_config_transfer::Column::TenantId.eq(tenant_id))
                    .filter(tenant_config_transfer::Column::RequestedBy.eq(retry_requested_by));
                let query = match job.job_type.as_str() {
                    TENANT_CONFIG_PREVIEW_JOB_TYPE => query
                        .filter(tenant_config_transfer::Column::PreviewBackgroundJobId.eq(job.id)),
                    TENANT_CONFIG_APPLY_JOB_TYPE => query
                        .filter(tenant_config_transfer::Column::ApplyBackgroundJobId.eq(job.id)),
                    TENANT_CONFIG_ROLLBACK_JOB_TYPE => query
                        .filter(tenant_config_transfer::Column::RollbackBackgroundJobId.eq(job.id)),
                    _ => unreachable!("配置迁移任务类型已经过匹配"),
                };
                query
                    .lock(LockType::Update)
                    .one(db)
                    .await
                    .map(|transfer| transfer.is_some())
                    .map_err(database_error)
            }
            _ => Ok(true),
        }
    }
}

impl BackgroundJobRepository {
    pub(super) async fn sync_config_transfer_state<C>(
        db: &C,
        job: &background_job::Model,
        disposition: LinkedJobDisposition,
        error: Option<String>,
        now: DateTime<Utc>,
        kind: ConfigTransferJobKind,
    ) -> AppResult<bool>
    where
        C: ConnectionTrait,
    {
        let (status, statuses) = match (kind, disposition) {
            (ConfigTransferJobKind::Preview, LinkedJobDisposition::Retried) => (
                tenant_config_transfer::Model::STATUS_PREVIEW_PENDING,
                vec![
                    tenant_config_transfer::Model::STATUS_PREVIEW_PENDING,
                    tenant_config_transfer::Model::STATUS_PREVIEWING,
                ],
            ),
            (ConfigTransferJobKind::Preview, LinkedJobDisposition::Dead) => (
                tenant_config_transfer::Model::STATUS_FAILED,
                vec![
                    tenant_config_transfer::Model::STATUS_PREVIEW_PENDING,
                    tenant_config_transfer::Model::STATUS_PREVIEWING,
                    tenant_config_transfer::Model::STATUS_FAILED,
                ],
            ),
            (ConfigTransferJobKind::Preview, LinkedJobDisposition::ManuallyRetried) => (
                tenant_config_transfer::Model::STATUS_PREVIEW_PENDING,
                vec![tenant_config_transfer::Model::STATUS_FAILED],
            ),
            (ConfigTransferJobKind::Apply, LinkedJobDisposition::Retried) => (
                tenant_config_transfer::Model::STATUS_APPLY_PENDING,
                vec![
                    tenant_config_transfer::Model::STATUS_APPLY_PENDING,
                    tenant_config_transfer::Model::STATUS_APPLYING,
                ],
            ),
            (ConfigTransferJobKind::Apply, LinkedJobDisposition::Dead) => (
                tenant_config_transfer::Model::STATUS_FAILED,
                vec![
                    tenant_config_transfer::Model::STATUS_APPLY_PENDING,
                    tenant_config_transfer::Model::STATUS_APPLYING,
                    tenant_config_transfer::Model::STATUS_FAILED,
                ],
            ),
            (ConfigTransferJobKind::Apply, LinkedJobDisposition::ManuallyRetried) => (
                tenant_config_transfer::Model::STATUS_APPLY_PENDING,
                vec![tenant_config_transfer::Model::STATUS_FAILED],
            ),
            (ConfigTransferJobKind::Rollback, LinkedJobDisposition::Retried) => (
                tenant_config_transfer::Model::STATUS_ROLLBACK_PENDING,
                vec![
                    tenant_config_transfer::Model::STATUS_ROLLBACK_PENDING,
                    tenant_config_transfer::Model::STATUS_ROLLING_BACK,
                ],
            ),
            (ConfigTransferJobKind::Rollback, LinkedJobDisposition::Dead) => (
                tenant_config_transfer::Model::STATUS_FAILED,
                vec![
                    tenant_config_transfer::Model::STATUS_ROLLBACK_PENDING,
                    tenant_config_transfer::Model::STATUS_ROLLING_BACK,
                    tenant_config_transfer::Model::STATUS_FAILED,
                ],
            ),
            (ConfigTransferJobKind::Rollback, LinkedJobDisposition::ManuallyRetried) => (
                tenant_config_transfer::Model::STATUS_ROLLBACK_PENDING,
                vec![tenant_config_transfer::Model::STATUS_FAILED],
            ),
        };
        let Some(transfer_id) = linked_resource_id(job, "transfer_id") else {
            return Ok(false);
        };
        let Some(tenant_id) = job.tenant_id.as_deref() else {
            return Ok(false);
        };
        let mut update = tenant_config_transfer::Entity::update_many()
            .col_expr(tenant_config_transfer::Column::Status, Expr::value(status))
            .col_expr(tenant_config_transfer::Column::UpdatedAt, Expr::value(now))
            .filter(tenant_config_transfer::Column::Id.eq(transfer_id))
            .filter(tenant_config_transfer::Column::TenantId.eq(tenant_id))
            .filter(tenant_config_transfer::Column::Status.is_in(statuses));
        update = match kind {
            ConfigTransferJobKind::Preview => {
                update.filter(tenant_config_transfer::Column::PreviewBackgroundJobId.eq(job.id))
            }
            ConfigTransferJobKind::Apply => {
                update.filter(tenant_config_transfer::Column::ApplyBackgroundJobId.eq(job.id))
            }
            ConfigTransferJobKind::Rollback => {
                update.filter(tenant_config_transfer::Column::RollbackBackgroundJobId.eq(job.id))
            }
        };
        if error.is_some() {
            let safe_error = match kind {
                ConfigTransferJobKind::Preview => TENANT_CONFIG_PREVIEW_SAFE_ERROR,
                ConfigTransferJobKind::Apply => TENANT_CONFIG_APPLY_SAFE_ERROR,
                ConfigTransferJobKind::Rollback => TENANT_CONFIG_ROLLBACK_SAFE_ERROR,
            };
            update = update.col_expr(
                tenant_config_transfer::Column::ErrorSummary,
                Expr::value(Some(safe_error.to_owned())),
            );
        } else if matches!(disposition, LinkedJobDisposition::ManuallyRetried) {
            update = update.col_expr(
                tenant_config_transfer::Column::ErrorSummary,
                Expr::value(Option::<String>::None),
            );
        }
        update
            .exec(db)
            .await
            .map(|result| result.rows_affected == 1)
            .map_err(database_error)
    }
}

#[derive(Clone, Copy)]
pub(super) enum ConfigTransferJobKind {
    Preview,
    Apply,
    Rollback,
}
