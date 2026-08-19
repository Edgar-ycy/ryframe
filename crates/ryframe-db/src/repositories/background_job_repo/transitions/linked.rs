use super::*;

impl BackgroundJobRepository {
    pub(super) async fn sync_linked_job_state<C>(
        db: &C,
        job: &background_job::Model,
        disposition: LinkedJobDisposition,
        error_message: Option<&str>,
        now: DateTime<Utc>,
    ) -> AppResult<bool>
    where
        C: ConnectionTrait,
    {
        let error = error_message.map(truncate_error);
        let mut linked_transitioned = true;
        match job.job_type.as_str() {
            USER_IMPORT_JOB_TYPE => {
                if matches!(disposition, LinkedJobDisposition::ManuallyRetried) {
                    let result = user_import_job::Entity::update_many()
                        .col_expr(
                            user_import_job::Column::LastError,
                            Expr::value(Option::<String>::None),
                        )
                        .col_expr(user_import_job::Column::UpdatedAt, Expr::value(now))
                        .filter(user_import_job::Column::BackgroundJobId.eq(job.id))
                        .filter(user_import_job::Column::Status.is_in([
                            user_import_job::Model::STATUS_SUCCEEDED,
                            user_import_job::Model::STATUS_PARTIAL,
                        ]))
                        .exec(db)
                        .await
                        .map_err(database_error)?;
                    if result.rows_affected > 0 {
                        return Ok(true);
                    }
                }
                let (status, completed_at, statuses) = match disposition {
                    LinkedJobDisposition::Retried => (
                        user_import_job::Model::STATUS_PENDING,
                        None,
                        vec![
                            user_import_job::Model::STATUS_PENDING,
                            user_import_job::Model::STATUS_RUNNING,
                        ],
                    ),
                    LinkedJobDisposition::Dead => (
                        user_import_job::Model::STATUS_FAILED,
                        Some(now),
                        vec![
                            user_import_job::Model::STATUS_PENDING,
                            user_import_job::Model::STATUS_RUNNING,
                            user_import_job::Model::STATUS_FAILED,
                        ],
                    ),
                    LinkedJobDisposition::ManuallyRetried => (
                        user_import_job::Model::STATUS_PENDING,
                        None,
                        vec![user_import_job::Model::STATUS_FAILED],
                    ),
                };
                let mut update = user_import_job::Entity::update_many()
                    .col_expr(user_import_job::Column::Status, Expr::value(status))
                    .col_expr(
                        user_import_job::Column::CompletedAt,
                        Expr::value(completed_at),
                    )
                    .col_expr(user_import_job::Column::UpdatedAt, Expr::value(now))
                    .filter(user_import_job::Column::BackgroundJobId.eq(job.id))
                    .filter(user_import_job::Column::Status.is_in(statuses));
                if let Some(error) = error {
                    update = update
                        .col_expr(user_import_job::Column::LastError, Expr::value(Some(error)));
                } else if matches!(disposition, LinkedJobDisposition::ManuallyRetried) {
                    update = update.col_expr(
                        user_import_job::Column::LastError,
                        Expr::value(Option::<String>::None),
                    );
                }
                update.exec(db).await.map_err(database_error)?;
            }
            EXPORT_JOB_TYPE => {
                let (status, completed_at, statuses) = match disposition {
                    LinkedJobDisposition::Retried => (
                        export_job::Model::STATUS_QUEUED,
                        None,
                        vec![
                            export_job::Model::STATUS_QUEUED,
                            export_job::Model::STATUS_RUNNING,
                        ],
                    ),
                    LinkedJobDisposition::Dead => (
                        export_job::Model::STATUS_FAILED,
                        Some(now),
                        vec![
                            export_job::Model::STATUS_QUEUED,
                            export_job::Model::STATUS_RUNNING,
                        ],
                    ),
                    LinkedJobDisposition::ManuallyRetried => (
                        export_job::Model::STATUS_QUEUED,
                        None,
                        vec![export_job::Model::STATUS_FAILED],
                    ),
                };
                let mut update = export_job::Entity::update_many()
                    .col_expr(export_job::Column::Status, Expr::value(status))
                    .col_expr(export_job::Column::CompletedAt, Expr::value(completed_at))
                    .col_expr(export_job::Column::UpdatedAt, Expr::value(now))
                    .filter(export_job::Column::BackgroundJobId.eq(job.id))
                    .filter(export_job::Column::Status.is_in(statuses));
                match disposition {
                    LinkedJobDisposition::Retried => {
                        update =
                            update.col_expr(export_job::Column::ExportedRows, Expr::value(0_i64));
                    }
                    LinkedJobDisposition::Dead => {
                        update = update.col_expr(
                            export_job::Column::ActiveRequestFingerprint,
                            Expr::value(Option::<String>::None),
                        );
                    }
                    LinkedJobDisposition::ManuallyRetried => {
                        update = update
                            .col_expr(export_job::Column::ExportedRows, Expr::value(0_i64))
                            .col_expr(
                                export_job::Column::ActiveRequestFingerprint,
                                Expr::col(export_job::Column::RequestFingerprint),
                            );
                    }
                }
                if let Some(error) = error {
                    update =
                        update.col_expr(export_job::Column::ErrorMessage, Expr::value(Some(error)));
                } else if matches!(disposition, LinkedJobDisposition::ManuallyRetried) {
                    update = update.col_expr(
                        export_job::Column::ErrorMessage,
                        Expr::value(Option::<String>::None),
                    );
                }
                update.exec(db).await.map_err(database_error)?;
            }
            DATA_RETENTION_JOB_TYPE => {
                Self::ensure_retention_run(db, job, now).await?;
                let (status, completed_at, statuses) = match disposition {
                    LinkedJobDisposition::Retried => (
                        data_retention_run::Model::STATUS_PENDING,
                        None,
                        vec![
                            data_retention_run::Model::STATUS_PENDING,
                            data_retention_run::Model::STATUS_RUNNING,
                            data_retention_run::Model::STATUS_FAILED,
                        ],
                    ),
                    LinkedJobDisposition::Dead => (
                        data_retention_run::Model::STATUS_FAILED,
                        Some(now),
                        vec![
                            data_retention_run::Model::STATUS_PENDING,
                            data_retention_run::Model::STATUS_RUNNING,
                        ],
                    ),
                    LinkedJobDisposition::ManuallyRetried => (
                        data_retention_run::Model::STATUS_PENDING,
                        None,
                        vec![data_retention_run::Model::STATUS_FAILED],
                    ),
                };
                let mut update = data_retention_run::Entity::update_many()
                    .col_expr(data_retention_run::Column::Status, Expr::value(status))
                    .col_expr(
                        data_retention_run::Column::CompletedAt,
                        Expr::value(completed_at),
                    )
                    .col_expr(data_retention_run::Column::UpdatedAt, Expr::value(now))
                    .filter(data_retention_run::Column::BackgroundJobId.eq(job.id))
                    .filter(data_retention_run::Column::Status.is_in(statuses));
                if let Some(error) = error {
                    update = update.col_expr(
                        data_retention_run::Column::ErrorSummary,
                        Expr::value(Some(error)),
                    );
                } else if matches!(disposition, LinkedJobDisposition::ManuallyRetried) {
                    update = update.col_expr(
                        data_retention_run::Column::ErrorSummary,
                        Expr::value(Option::<String>::None),
                    );
                }
                update.exec(db).await.map_err(database_error)?;
            }
            TENANT_CONFIG_EXPORT_JOB_TYPE => {
                let (status, statuses) = match disposition {
                    LinkedJobDisposition::Retried => (
                        tenant_config_bundle::Model::STATUS_PENDING,
                        vec![
                            tenant_config_bundle::Model::STATUS_PENDING,
                            tenant_config_bundle::Model::STATUS_RUNNING,
                        ],
                    ),
                    LinkedJobDisposition::Dead => (
                        tenant_config_bundle::Model::STATUS_FAILED,
                        vec![
                            tenant_config_bundle::Model::STATUS_PENDING,
                            tenant_config_bundle::Model::STATUS_RUNNING,
                            tenant_config_bundle::Model::STATUS_FAILED,
                        ],
                    ),
                    LinkedJobDisposition::ManuallyRetried => (
                        tenant_config_bundle::Model::STATUS_PENDING,
                        vec![tenant_config_bundle::Model::STATUS_FAILED],
                    ),
                };
                let mut update = tenant_config_bundle::Entity::update_many()
                    .col_expr(tenant_config_bundle::Column::Status, Expr::value(status))
                    .col_expr(tenant_config_bundle::Column::UpdatedAt, Expr::value(now))
                    .filter(tenant_config_bundle::Column::BackgroundJobId.eq(job.id))
                    .filter(tenant_config_bundle::Column::Status.is_in(statuses));
                if error.is_some() {
                    update = update.col_expr(
                        tenant_config_bundle::Column::ErrorSummary,
                        Expr::value(Some(TENANT_CONFIG_EXPORT_SAFE_ERROR.to_owned())),
                    );
                } else if matches!(disposition, LinkedJobDisposition::ManuallyRetried) {
                    update = update.col_expr(
                        tenant_config_bundle::Column::ErrorSummary,
                        Expr::value(Option::<String>::None),
                    );
                }
                linked_transitioned = update
                    .exec(db)
                    .await
                    .map(|result| result.rows_affected == 1)
                    .map_err(database_error)?;
            }
            TENANT_CONFIG_PREVIEW_JOB_TYPE => {
                linked_transitioned = Self::sync_config_transfer_state(
                    db,
                    job,
                    disposition,
                    error,
                    now,
                    ConfigTransferJobKind::Preview,
                )
                .await?;
            }
            TENANT_CONFIG_APPLY_JOB_TYPE => {
                linked_transitioned = Self::sync_config_transfer_state(
                    db,
                    job,
                    disposition,
                    error,
                    now,
                    ConfigTransferJobKind::Apply,
                )
                .await?;
            }
            TENANT_CONFIG_ROLLBACK_JOB_TYPE => {
                linked_transitioned = Self::sync_config_transfer_state(
                    db,
                    job,
                    disposition,
                    error,
                    now,
                    ConfigTransferJobKind::Rollback,
                )
                .await?;
            }
            _ => {}
        }
        Ok(linked_transitioned)
    }
}
