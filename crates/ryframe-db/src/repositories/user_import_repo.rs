use chrono::{DateTime, Utc};
use ryframe_kernel::{AppError, AppResult, PageResult, ValidatedPageQuery};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, DatabaseConnection, DatabaseTransaction,
    EntityTrait, PaginatorTrait, QueryFilter, QueryOrder, QuerySelect, Statement,
    sea_query::{Expr, LockType},
};

use crate::entities::{user_import_job, user_import_row_result};

/// 创建异步用户导入任务所需的不可变数据。
#[derive(Clone, Debug)]
pub struct CreateUserImportJob {
    pub id: i64,
    pub tenant_id: String,
    pub requester_user_id: i64,
    pub background_job_id: i64,
    pub idempotency_key_hash: String,
    pub source_file_id: i64,
    pub source_name_snapshot: String,
    pub source_sha256: String,
}

/// 用户导入列表的低成本精确筛选条件。
#[derive(Clone, Copy, Debug, Default)]
pub struct UserImportFilter<'a> {
    pub status: Option<&'a str>,
}

/// 到期且不再被保留期内导入任务引用的私有文件。
#[derive(Clone, Debug)]
pub struct UserImportArtifact {
    pub tenant_id: String,
    pub file_id: i64,
}

/// 用户导入任务与异常行结果仓储。
pub struct UserImportRepository;

impl UserImportRepository {
    pub async fn database_utc_now<C>(&self, db: &C) -> AppResult<DateTime<Utc>>
    where
        C: ConnectionTrait,
    {
        let row = db
            .query_one_raw(Statement::from_string(
                db.get_database_backend(),
                "SELECT UTC_TIMESTAMP(6) AS db_now".to_owned(),
            ))
            .await
            .map_err(database_error)?
            .ok_or_else(|| AppError::Database("数据库时钟查询没有返回记录".into()))?;
        let value: chrono::NaiveDateTime = row
            .try_get("", "db_now")
            .map_err(|error| AppError::Database(error.to_string()))?;
        Ok(DateTime::from_naive_utc_and_offset(value, Utc))
    }

    pub async fn find_by_idempotency_in_txn(
        &self,
        transaction: &DatabaseTransaction,
        tenant_id: &str,
        idempotency_key_hash: &str,
    ) -> AppResult<Option<user_import_job::Model>> {
        user_import_job::Entity::find()
            .filter(user_import_job::Column::TenantId.eq(tenant_id))
            .filter(user_import_job::Column::IdempotencyKeyHash.eq(idempotency_key_hash))
            .lock(LockType::Update)
            .one(transaction)
            .await
            .map_err(database_error)
    }

    pub async fn find_by_idempotency(
        &self,
        db: &DatabaseConnection,
        tenant_id: &str,
        idempotency_key_hash: &str,
    ) -> AppResult<Option<user_import_job::Model>> {
        user_import_job::Entity::find()
            .filter(user_import_job::Column::TenantId.eq(tenant_id))
            .filter(user_import_job::Column::IdempotencyKeyHash.eq(idempotency_key_hash))
            .one(db)
            .await
            .map_err(database_error)
    }

    pub async fn count_active_in_txn(
        &self,
        transaction: &DatabaseTransaction,
        tenant_id: &str,
    ) -> AppResult<u64> {
        user_import_job::Entity::find()
            .filter(user_import_job::Column::TenantId.eq(tenant_id))
            .filter(user_import_job::Column::Status.is_in([
                user_import_job::Model::STATUS_PENDING,
                user_import_job::Model::STATUS_RUNNING,
            ]))
            .count(transaction)
            .await
            .map_err(database_error)
    }

    pub async fn create_in_txn(
        &self,
        transaction: &DatabaseTransaction,
        command: CreateUserImportJob,
        now: DateTime<Utc>,
    ) -> AppResult<user_import_job::Model> {
        validate_create_command(&command)?;
        user_import_job::ActiveModel::from(user_import_job::Model {
            id: command.id,
            tenant_id: command.tenant_id,
            requester_user_id: command.requester_user_id,
            background_job_id: command.background_job_id,
            idempotency_key_hash: command.idempotency_key_hash,
            source_file_id: command.source_file_id,
            source_name_snapshot: command.source_name_snapshot,
            source_sha256: command.source_sha256,
            duplicate_policy: user_import_job::Model::DUPLICATE_SKIP_EXISTING.to_owned(),
            status: user_import_job::Model::STATUS_PENDING.to_owned(),
            total_rows: 0,
            processed_rows: 0,
            success_count: 0,
            skipped_count: 0,
            failure_count: 0,
            cancel_requested: false,
            error_report_file_id: None,
            last_error: None,
            started_at: None,
            completed_at: None,
            created_at: now,
            updated_at: now,
        })
        .insert(transaction)
        .await
        .map_err(database_error)
    }

    pub async fn find_by_id_for_tenant(
        &self,
        db: &DatabaseConnection,
        tenant_id: &str,
        id: i64,
    ) -> AppResult<Option<user_import_job::Model>> {
        user_import_job::Entity::find_by_id(id)
            .filter(user_import_job::Column::TenantId.eq(tenant_id))
            .one(db)
            .await
            .map_err(database_error)
    }

    pub async fn find_by_background_job(
        &self,
        db: &DatabaseConnection,
        background_job_id: i64,
    ) -> AppResult<Option<user_import_job::Model>> {
        user_import_job::Entity::find()
            .filter(user_import_job::Column::BackgroundJobId.eq(background_job_id))
            .one(db)
            .await
            .map_err(database_error)
    }

    pub async fn list_for_tenant(
        &self,
        db: &DatabaseConnection,
        tenant_id: &str,
        page: &ValidatedPageQuery,
        filter: UserImportFilter<'_>,
    ) -> AppResult<PageResult<user_import_job::Model>> {
        let mut query =
            user_import_job::Entity::find().filter(user_import_job::Column::TenantId.eq(tenant_id));
        if let Some(status) = filter.status.filter(|value| !value.is_empty()) {
            query = query.filter(user_import_job::Column::Status.eq(status));
        }
        crate::pagination::paginate(
            db,
            query
                .order_by_desc(user_import_job::Column::CreatedAt)
                .order_by_desc(user_import_job::Column::Id),
            page,
        )
        .await
    }

    pub async fn list_row_results(
        &self,
        db: &DatabaseConnection,
        tenant_id: &str,
        import_job_id: i64,
        page: &ValidatedPageQuery,
    ) -> AppResult<PageResult<user_import_row_result::Model>> {
        crate::pagination::paginate(
            db,
            user_import_row_result::Entity::find()
                .filter(user_import_row_result::Column::TenantId.eq(tenant_id))
                .filter(user_import_row_result::Column::ImportJobId.eq(import_job_id))
                .order_by_asc(user_import_row_result::Column::RowNumber),
            page,
        )
        .await
    }

    pub async fn all_row_results(
        &self,
        db: &DatabaseConnection,
        tenant_id: &str,
        import_job_id: i64,
    ) -> AppResult<Vec<user_import_row_result::Model>> {
        user_import_row_result::Entity::find()
            .filter(user_import_row_result::Column::TenantId.eq(tenant_id))
            .filter(user_import_row_result::Column::ImportJobId.eq(import_job_id))
            .order_by_asc(user_import_row_result::Column::RowNumber)
            .all(db)
            .await
            .map_err(database_error)
    }

    pub async fn request_cancel(
        &self,
        db: &DatabaseConnection,
        tenant_id: &str,
        id: i64,
        now: DateTime<Utc>,
    ) -> AppResult<bool> {
        let result = user_import_job::Entity::update_many()
            .col_expr(user_import_job::Column::CancelRequested, Expr::value(true))
            .col_expr(user_import_job::Column::UpdatedAt, Expr::value(now))
            .filter(user_import_job::Column::Id.eq(id))
            .filter(user_import_job::Column::TenantId.eq(tenant_id))
            .filter(user_import_job::Column::Status.is_in([
                user_import_job::Model::STATUS_PENDING,
                user_import_job::Model::STATUS_RUNNING,
            ]))
            .exec(db)
            .await
            .map_err(database_error)?;
        Ok(result.rows_affected == 1)
    }

    pub async fn lock_by_id_in_txn(
        &self,
        transaction: &DatabaseTransaction,
        id: i64,
    ) -> AppResult<Option<user_import_job::Model>> {
        user_import_job::Entity::find_by_id(id)
            .lock(LockType::Update)
            .one(transaction)
            .await
            .map_err(database_error)
    }

    pub async fn save_in_txn(
        &self,
        transaction: &DatabaseTransaction,
        model: user_import_job::Model,
    ) -> AppResult<user_import_job::Model> {
        let id = model.id;
        let result = user_import_job::Entity::update_many()
            .col_expr(user_import_job::Column::Status, Expr::value(model.status))
            .col_expr(
                user_import_job::Column::TotalRows,
                Expr::value(model.total_rows),
            )
            .col_expr(
                user_import_job::Column::ProcessedRows,
                Expr::value(model.processed_rows),
            )
            .col_expr(
                user_import_job::Column::SuccessCount,
                Expr::value(model.success_count),
            )
            .col_expr(
                user_import_job::Column::SkippedCount,
                Expr::value(model.skipped_count),
            )
            .col_expr(
                user_import_job::Column::FailureCount,
                Expr::value(model.failure_count),
            )
            .col_expr(
                user_import_job::Column::CancelRequested,
                Expr::value(model.cancel_requested),
            )
            .col_expr(
                user_import_job::Column::ErrorReportFileId,
                Expr::value(model.error_report_file_id),
            )
            .col_expr(
                user_import_job::Column::LastError,
                Expr::value(model.last_error),
            )
            .col_expr(
                user_import_job::Column::StartedAt,
                Expr::value(model.started_at),
            )
            .col_expr(
                user_import_job::Column::CompletedAt,
                Expr::value(model.completed_at),
            )
            .col_expr(
                user_import_job::Column::UpdatedAt,
                Expr::value(model.updated_at),
            )
            .filter(user_import_job::Column::Id.eq(id))
            .exec(transaction)
            .await
            .map_err(database_error)?;
        if result.rows_affected != 1 {
            return Err(AppError::NotFound("用户导入任务不存在".into()));
        }
        user_import_job::Entity::find_by_id(id)
            .one(transaction)
            .await
            .map_err(database_error)?
            .ok_or_else(|| AppError::NotFound("用户导入任务不存在".into()))
    }

    pub async fn insert_row_results_in_txn(
        &self,
        transaction: &DatabaseTransaction,
        rows: Vec<user_import_row_result::Model>,
    ) -> AppResult<()> {
        if rows.is_empty() {
            return Ok(());
        }
        user_import_row_result::Entity::insert_many(
            rows.into_iter()
                .map(user_import_row_result::ActiveModel::from),
        )
        .exec(transaction)
        .await
        .map_err(database_error)?;
        Ok(())
    }

    pub async fn count_expired_artifacts(
        &self,
        db: &DatabaseConnection,
        before: DateTime<Utc>,
    ) -> AppResult<u64> {
        let sql = format!(
            "SELECT COUNT(*) AS artifact_count FROM sys_file file WHERE {}",
            expired_artifact_predicate()
        );
        let row = db
            .query_one_raw(Statement::from_sql_and_values(
                db.get_database_backend(),
                sql,
                [
                    before.naive_utc().into(),
                    before.naive_utc().into(),
                    before.naive_utc().into(),
                ],
            ))
            .await
            .map_err(database_error)?
            .ok_or_else(|| AppError::Database("导入文件统计没有返回记录".into()))?;
        let count: i64 = row
            .try_get("", "artifact_count")
            .map_err(|error| AppError::Database(error.to_string()))?;
        u64::try_from(count).map_err(|_| AppError::Database("导入文件统计结果无效".into()))
    }

    pub async fn list_expired_artifacts_after_id(
        &self,
        db: &DatabaseConnection,
        before: DateTime<Utc>,
        after_id: Option<i64>,
        limit: usize,
    ) -> AppResult<Vec<UserImportArtifact>> {
        let mut sql = format!(
            "SELECT file.tenant_id, file.id FROM sys_file file WHERE {}",
            expired_artifact_predicate()
        );
        let mut values = vec![
            before.naive_utc().into(),
            before.naive_utc().into(),
            before.naive_utc().into(),
        ];
        if let Some(after_id) = after_id {
            sql.push_str(" AND file.id > ?");
            values.push(after_id.into());
        }
        sql.push_str(" ORDER BY file.id LIMIT ?");
        values.push(
            i64::try_from(limit.clamp(1, 5_000))
                .map_err(|_| AppError::Config("导入文件清理批大小无效".into()))?
                .into(),
        );
        db.query_all_raw(Statement::from_sql_and_values(
            db.get_database_backend(),
            sql,
            values,
        ))
        .await
        .map_err(database_error)?
        .into_iter()
        .map(|row| {
            Ok(UserImportArtifact {
                tenant_id: row
                    .try_get("", "tenant_id")
                    .map_err(|error| AppError::Database(error.to_string()))?,
                file_id: row
                    .try_get("", "id")
                    .map_err(|error| AppError::Database(error.to_string()))?,
            })
        })
        .collect()
    }
}

fn expired_artifact_predicate() -> &'static str {
    "file.bucket = 'imports' AND file.del_flag = '0' AND file.upload_status IN ('ready', 'cleanup') \
     AND (EXISTS (SELECT 1 FROM sys_user_import_job expired \
            WHERE expired.tenant_id = file.tenant_id \
              AND expired.status IN ('succeeded', 'partial', 'failed', 'cancelled') \
              AND expired.completed_at < ? \
              AND (expired.source_file_id = file.id OR expired.error_report_file_id = file.id)) \
          OR (file.created_at < ? AND NOT EXISTS (SELECT 1 FROM sys_user_import_job referenced \
            WHERE referenced.tenant_id = file.tenant_id \
              AND (referenced.source_file_id = file.id OR referenced.error_report_file_id = file.id)))) \
     AND NOT EXISTS (SELECT 1 FROM sys_user_import_job retained \
       WHERE retained.tenant_id = file.tenant_id \
         AND (retained.source_file_id = file.id OR retained.error_report_file_id = file.id) \
         AND (retained.status NOT IN ('succeeded', 'partial', 'failed', 'cancelled') \
              OR retained.completed_at IS NULL OR retained.completed_at >= ?))"
}

fn validate_create_command(command: &CreateUserImportJob) -> AppResult<()> {
    if command.id <= 0
        || command.requester_user_id <= 0
        || command.background_job_id <= 0
        || command.source_file_id <= 0
    {
        return Err(AppError::Validation("用户导入关联标识必须为正数".into()));
    }
    if command.tenant_id.is_empty() || command.tenant_id.len() > 64 {
        return Err(AppError::Validation("用户导入租户标识无效".into()));
    }
    if command.idempotency_key_hash.len() != 64 || command.source_sha256.len() != 64 {
        return Err(AppError::Validation("用户导入摘要格式无效".into()));
    }
    if command.source_name_snapshot.is_empty() || command.source_name_snapshot.len() > 255 {
        return Err(AppError::Validation(
            "用户导入文件名长度必须介于 1 和 255 字节之间".into(),
        ));
    }
    Ok(())
}

fn database_error(error: impl std::fmt::Display) -> AppError {
    AppError::Database(error.to_string())
}
