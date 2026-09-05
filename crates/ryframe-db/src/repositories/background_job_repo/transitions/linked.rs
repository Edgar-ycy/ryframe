use super::*;
use crate::DbResultExt;

impl BackgroundJobRepository {
    pub(super) async fn sync_linked_job_state<C>(
        db: &C,
        job: &background_job::Model,
        disposition: LinkedJobDisposition,
        error_message: Option<&str>,
        now: DateTime<Utc>,
    ) -> AppResult<LinkedJobSyncResult>
    where
        C: ConnectionTrait,
    {
        let error = error_message.map(truncate_error);
        match job.job_type.as_str() {
            USER_IMPORT_JOB_TYPE => Self::sync_import_job(db, job, disposition, error, now).await,
            EXPORT_JOB_TYPE => Self::sync_export_job(db, job, disposition, error, now).await,
            DATA_RETENTION_JOB_TYPE => {
                Self::sync_retention_job(db, job, disposition, error, now).await
            }
            TENANT_CONFIG_EXPORT_JOB_TYPE => {
                Self::sync_config_bundle(db, job, disposition, error, now).await
            }
            TENANT_CONFIG_PREVIEW_JOB_TYPE => {
                Self::sync_config_transfer_state(
                    db,
                    job,
                    disposition,
                    error,
                    now,
                    ConfigTransferJobKind::Preview,
                )
                .await
            }
            TENANT_CONFIG_APPLY_JOB_TYPE => {
                Self::sync_config_transfer_state(
                    db,
                    job,
                    disposition,
                    error,
                    now,
                    ConfigTransferJobKind::Apply,
                )
                .await
            }
            TENANT_CONFIG_ROLLBACK_JOB_TYPE => {
                Self::sync_config_transfer_state(
                    db,
                    job,
                    disposition,
                    error,
                    now,
                    ConfigTransferJobKind::Rollback,
                )
                .await
            }
            _ => Ok(LinkedJobSyncResult::NotLinked),
        }
    }

    async fn sync_import_job<C: ConnectionTrait>(
        db: &C,
        job: &background_job::Model,
        disposition: LinkedJobDisposition,
        error: Option<String>,
        now: DateTime<Utc>,
    ) -> AppResult<LinkedJobSyncResult> {
        let (status, completed_at, statuses): (&str, Option<DateTime<Utc>>, &[&str]) =
            match disposition {
                LinkedJobDisposition::Retried => (
                    user_import_job::Model::STATUS_PENDING,
                    None,
                    &[
                        user_import_job::Model::STATUS_PENDING,
                        user_import_job::Model::STATUS_RUNNING,
                    ],
                ),
                LinkedJobDisposition::Dead => (
                    user_import_job::Model::STATUS_FAILED,
                    Some(now),
                    &[
                        user_import_job::Model::STATUS_PENDING,
                        user_import_job::Model::STATUS_RUNNING,
                        user_import_job::Model::STATUS_FAILED,
                    ],
                ),
                LinkedJobDisposition::ManuallyRetried => (
                    user_import_job::Model::STATUS_PENDING,
                    None,
                    &[user_import_job::Model::STATUS_FAILED],
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
            .filter(user_import_job::Column::Status.is_in(statuses.iter().copied()));
        if let Some(error) = error {
            update = update.col_expr(user_import_job::Column::LastError, Expr::value(Some(error)));
        } else if matches!(disposition, LinkedJobDisposition::ManuallyRetried) {
            update = update.col_expr(
                user_import_job::Column::LastError,
                Expr::value(Option::<String>::None),
            );
        }
        let updated = update.exec(db).await.db()?;
        let current = if updated.rows_affected == 1 {
            None
        } else {
            user_import_job::Entity::find()
                .select_only()
                .column(user_import_job::Column::Status)
                .filter(user_import_job::Column::BackgroundJobId.eq(job.id))
                .lock(LockType::Update)
                .into_tuple::<String>()
                .one(db)
                .await
                .db()?
        };
        Ok(classify_linked_update(
            updated.rows_affected,
            current.as_deref(),
            statuses,
            import_terminal,
        ))
    }

    async fn sync_export_job<C: ConnectionTrait>(
        db: &C,
        job: &background_job::Model,
        disposition: LinkedJobDisposition,
        error: Option<String>,
        now: DateTime<Utc>,
    ) -> AppResult<LinkedJobSyncResult> {
        let (status, completed_at, statuses): (&str, Option<DateTime<Utc>>, &[&str]) =
            match disposition {
                LinkedJobDisposition::Retried => (
                    export_job::Model::STATUS_QUEUED,
                    None,
                    &[
                        export_job::Model::STATUS_QUEUED,
                        export_job::Model::STATUS_RUNNING,
                    ],
                ),
                LinkedJobDisposition::Dead => (
                    export_job::Model::STATUS_FAILED,
                    Some(now),
                    &[
                        export_job::Model::STATUS_QUEUED,
                        export_job::Model::STATUS_RUNNING,
                    ],
                ),
                LinkedJobDisposition::ManuallyRetried => (
                    export_job::Model::STATUS_QUEUED,
                    None,
                    &[export_job::Model::STATUS_FAILED],
                ),
            };
        let mut update = export_job::Entity::update_many()
            .col_expr(export_job::Column::Status, Expr::value(status))
            .col_expr(export_job::Column::CompletedAt, Expr::value(completed_at))
            .col_expr(export_job::Column::UpdatedAt, Expr::value(now))
            .filter(export_job::Column::BackgroundJobId.eq(job.id))
            .filter(export_job::Column::Status.is_in(statuses.iter().copied()));
        match disposition {
            LinkedJobDisposition::Retried => {
                update = update.col_expr(export_job::Column::ExportedRows, Expr::value(0_i64));
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
            update = update.col_expr(export_job::Column::ErrorMessage, Expr::value(Some(error)));
        } else if matches!(disposition, LinkedJobDisposition::ManuallyRetried) {
            update = update.col_expr(
                export_job::Column::ErrorMessage,
                Expr::value(Option::<String>::None),
            );
        }
        let updated = update.exec(db).await.db()?;
        let current = if updated.rows_affected == 1 {
            None
        } else {
            export_job::Entity::find()
                .select_only()
                .column(export_job::Column::Status)
                .filter(export_job::Column::BackgroundJobId.eq(job.id))
                .lock(LockType::Update)
                .into_tuple::<String>()
                .one(db)
                .await
                .db()?
        };
        Ok(classify_linked_update(
            updated.rows_affected,
            current.as_deref(),
            statuses,
            export_terminal,
        ))
    }

    async fn sync_retention_job<C: ConnectionTrait>(
        db: &C,
        job: &background_job::Model,
        disposition: LinkedJobDisposition,
        error: Option<String>,
        now: DateTime<Utc>,
    ) -> AppResult<LinkedJobSyncResult> {
        Self::ensure_retention_run(db, job, now).await?;
        let (status, completed_at, statuses): (&str, Option<DateTime<Utc>>, &[&str]) =
            match disposition {
                LinkedJobDisposition::Retried => (
                    data_retention_run::Model::STATUS_PENDING,
                    None,
                    &[
                        data_retention_run::Model::STATUS_PENDING,
                        data_retention_run::Model::STATUS_RUNNING,
                        data_retention_run::Model::STATUS_FAILED,
                    ],
                ),
                LinkedJobDisposition::Dead => (
                    data_retention_run::Model::STATUS_FAILED,
                    Some(now),
                    &[
                        data_retention_run::Model::STATUS_PENDING,
                        data_retention_run::Model::STATUS_RUNNING,
                    ],
                ),
                LinkedJobDisposition::ManuallyRetried => (
                    data_retention_run::Model::STATUS_PENDING,
                    None,
                    &[data_retention_run::Model::STATUS_FAILED],
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
            .filter(data_retention_run::Column::Status.is_in(statuses.iter().copied()));
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
        let updated = update.exec(db).await.db()?;
        let current = if updated.rows_affected == 1 {
            None
        } else {
            data_retention_run::Entity::find()
                .select_only()
                .column(data_retention_run::Column::Status)
                .filter(data_retention_run::Column::BackgroundJobId.eq(job.id))
                .lock(LockType::Update)
                .into_tuple::<String>()
                .one(db)
                .await
                .db()?
        };
        Ok(classify_linked_update(
            updated.rows_affected,
            current.as_deref(),
            statuses,
            retention_terminal,
        ))
    }

    async fn sync_config_bundle<C: ConnectionTrait>(
        db: &C,
        job: &background_job::Model,
        disposition: LinkedJobDisposition,
        error: Option<String>,
        now: DateTime<Utc>,
    ) -> AppResult<LinkedJobSyncResult> {
        let (status, statuses): (&str, &[&str]) = match disposition {
            LinkedJobDisposition::Retried => (
                tenant_config_bundle::Model::STATUS_PENDING,
                &[
                    tenant_config_bundle::Model::STATUS_PENDING,
                    tenant_config_bundle::Model::STATUS_RUNNING,
                ],
            ),
            LinkedJobDisposition::Dead => (
                tenant_config_bundle::Model::STATUS_FAILED,
                &[
                    tenant_config_bundle::Model::STATUS_PENDING,
                    tenant_config_bundle::Model::STATUS_RUNNING,
                    tenant_config_bundle::Model::STATUS_FAILED,
                ],
            ),
            LinkedJobDisposition::ManuallyRetried => (
                tenant_config_bundle::Model::STATUS_PENDING,
                &[tenant_config_bundle::Model::STATUS_FAILED],
            ),
        };
        let mut update = tenant_config_bundle::Entity::update_many()
            .col_expr(tenant_config_bundle::Column::Status, Expr::value(status))
            .col_expr(tenant_config_bundle::Column::UpdatedAt, Expr::value(now))
            .filter(tenant_config_bundle::Column::BackgroundJobId.eq(job.id))
            .filter(tenant_config_bundle::Column::Status.is_in(statuses.iter().copied()));
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
        let updated = update.exec(db).await.db()?;
        let current = if updated.rows_affected == 1 {
            None
        } else {
            tenant_config_bundle::Entity::find()
                .select_only()
                .column(tenant_config_bundle::Column::Status)
                .filter(tenant_config_bundle::Column::BackgroundJobId.eq(job.id))
                .lock(LockType::Update)
                .into_tuple::<String>()
                .one(db)
                .await
                .db()?
        };
        Ok(classify_linked_update(
            updated.rows_affected,
            current.as_deref(),
            statuses,
            config_bundle_terminal,
        ))
    }
}

fn classify_linked_update(
    rows_affected: u64,
    current_status: Option<&str>,
    eligible_statuses: &[&str],
    terminal: fn(&str) -> Option<LinkedBusinessTerminal>,
) -> LinkedJobSyncResult {
    match rows_affected {
        1 => LinkedJobSyncResult::Transitioned,
        0 => match current_status {
            Some(status) if eligible_statuses.contains(&status) => {
                LinkedJobSyncResult::Transitioned
            }
            Some(status) => terminal(status)
                .map(LinkedJobSyncResult::Terminal)
                .unwrap_or(LinkedJobSyncResult::Conflict),
            None => LinkedJobSyncResult::Conflict,
        },
        _ => LinkedJobSyncResult::Conflict,
    }
}

fn import_terminal(status: &str) -> Option<LinkedBusinessTerminal> {
    match status {
        user_import_job::Model::STATUS_SUCCEEDED | user_import_job::Model::STATUS_PARTIAL => {
            Some(LinkedBusinessTerminal::Succeeded)
        }
        user_import_job::Model::STATUS_CANCELLED => Some(LinkedBusinessTerminal::Cancelled),
        user_import_job::Model::STATUS_FAILED => Some(LinkedBusinessTerminal::Failed),
        _ => None,
    }
}

fn export_terminal(status: &str) -> Option<LinkedBusinessTerminal> {
    match status {
        export_job::Model::STATUS_SUCCEEDED => Some(LinkedBusinessTerminal::Succeeded),
        export_job::Model::STATUS_CANCELLED => Some(LinkedBusinessTerminal::Cancelled),
        export_job::Model::STATUS_EXPIRED => Some(LinkedBusinessTerminal::Expired),
        export_job::Model::STATUS_FAILED => Some(LinkedBusinessTerminal::Failed),
        _ => None,
    }
}

fn retention_terminal(status: &str) -> Option<LinkedBusinessTerminal> {
    match status {
        data_retention_run::Model::STATUS_SUCCEEDED | data_retention_run::Model::STATUS_PARTIAL => {
            Some(LinkedBusinessTerminal::Succeeded)
        }
        data_retention_run::Model::STATUS_FAILED => Some(LinkedBusinessTerminal::Failed),
        _ => None,
    }
}

fn config_bundle_terminal(status: &str) -> Option<LinkedBusinessTerminal> {
    match status {
        tenant_config_bundle::Model::STATUS_SUCCEEDED => Some(LinkedBusinessTerminal::Succeeded),
        tenant_config_bundle::Model::STATUS_EXPIRED => Some(LinkedBusinessTerminal::Expired),
        tenant_config_bundle::Model::STATUS_FAILED => Some(LinkedBusinessTerminal::Failed),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn linked_updates_distinguish_transitions_terminals_and_conflicts() {
        let eligible = [user_import_job::Model::STATUS_FAILED];
        assert_eq!(
            classify_linked_update(1, None, &eligible, import_terminal),
            LinkedJobSyncResult::Transitioned
        );
        assert_eq!(
            classify_linked_update(
                0,
                Some(user_import_job::Model::STATUS_FAILED),
                &eligible,
                import_terminal,
            ),
            LinkedJobSyncResult::Transitioned
        );
        assert_eq!(
            classify_linked_update(
                0,
                Some(user_import_job::Model::STATUS_SUCCEEDED),
                &eligible,
                import_terminal,
            ),
            LinkedJobSyncResult::Terminal(LinkedBusinessTerminal::Succeeded)
        );
        assert_eq!(
            classify_linked_update(0, Some("unknown"), &eligible, import_terminal),
            LinkedJobSyncResult::Conflict
        );
        assert_eq!(
            classify_linked_update(2, None, &eligible, import_terminal),
            LinkedJobSyncResult::Conflict
        );
    }

    #[test]
    fn business_terminal_statuses_keep_their_authoritative_meaning() {
        for (actual, expected) in [
            (
                import_terminal(user_import_job::Model::STATUS_PARTIAL),
                Some(LinkedBusinessTerminal::Succeeded),
            ),
            (
                import_terminal(user_import_job::Model::STATUS_CANCELLED),
                Some(LinkedBusinessTerminal::Cancelled),
            ),
            (
                export_terminal(export_job::Model::STATUS_EXPIRED),
                Some(LinkedBusinessTerminal::Expired),
            ),
            (
                retention_terminal(data_retention_run::Model::STATUS_FAILED),
                Some(LinkedBusinessTerminal::Failed),
            ),
            (
                config_bundle_terminal(tenant_config_bundle::Model::STATUS_SUCCEEDED),
                Some(LinkedBusinessTerminal::Succeeded),
            ),
        ] {
            assert_eq!(actual, expected);
        }
    }
}
