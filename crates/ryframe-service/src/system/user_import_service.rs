use std::{collections::HashSet, sync::Arc};

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use futures_util::future::try_join_all;
use ryframe_auth::password;
use ryframe_config::UserImportConfig;
use ryframe_core::repository::{PageResult, ValidatedPageQuery};
use ryframe_db::{
    CreateUserImportJob, DatabaseCluster, DeptRepository, EnqueueBackgroundJob, TenantRepository,
    UserImportFilter, UserImportRepository, UserRepository, background_job,
    entities::{user, user_import_job, user_import_row_result},
};
use ryframe_excel::{ExcelExporter, ExcelImportRow, ExcelImporter};
use ryframe_kernel::{ActorContext, AppError, AppResult, DataScope};
use ryframe_utils::{file_upload::UploadConfig, snowflake::try_next_snowflake_id};
use sea_orm::{EntityTrait, TransactionTrait};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::sync::Semaphore;
use uuid::Uuid;
use validator::Validate;

use super::{DownloadedFile, FileService, IMPORT_BUCKET, UploadCommand, UserService};
use crate::{JobHandler, JobQueue};

/// 可恢复用户导入的稳定后台任务类型。
pub const USER_IMPORT_JOB_TYPE: &str = "system.user.import";
const USER_IMPORT_PERMISSION: &str = "system:user-import:add";
const USER_IMPORT_MAX_RUNTIME_SECONDS: i32 = 14_400;
const USER_IMPORT_MAX_ATTEMPTS: i32 = 3;

/// 用户导入模板和 Worker 共同使用的行结构。
#[derive(Clone, Debug, Deserialize, Serialize, Validate)]
pub struct UserImportData {
    #[serde(alias = "用户名")]
    #[validate(length(min = 2, max = 64, message = "用户名长度必须为 2-64 个字符"))]
    pub username: String,
    #[serde(alias = "昵称")]
    #[validate(length(min = 1, max = 64, message = "昵称长度必须为 1-64 个字符"))]
    pub nickname: String,
    #[serde(alias = "邮箱")]
    #[validate(email(message = "邮箱格式不正确"))]
    pub email: String,
    #[serde(alias = "手机号")]
    #[validate(length(max = 32, message = "手机号最多 32 个字符"))]
    pub phone: Option<String>,
    #[serde(alias = "部门ID")]
    pub dept_id: Option<String>,
}

impl UserImportData {
    pub const fn excel_headers() -> &'static [(&'static str, &'static str)] {
        &[
            ("username", "用户名"),
            ("nickname", "昵称"),
            ("email", "邮箱"),
            ("phone", "手机号"),
            ("dept_id", "部门ID"),
        ]
    }
}

/// 面向管理端的异步导入任务安全视图。
#[derive(Clone, Debug, Serialize)]
pub struct UserImportJobVo {
    pub id: String,
    pub requester_user_id: String,
    pub background_job_id: String,
    pub source_name: String,
    pub duplicate_policy: String,
    pub status: String,
    pub total_rows: i32,
    pub processed_rows: i32,
    pub success_count: i32,
    pub skipped_count: i32,
    pub failure_count: i32,
    pub cancel_requested: bool,
    pub report_available: bool,
    pub last_error: Option<String>,
    pub started_at: Option<DateTime<Utc>>,
    pub completed_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl From<user_import_job::Model> for UserImportJobVo {
    fn from(job: user_import_job::Model) -> Self {
        Self {
            id: job.id.to_string(),
            requester_user_id: job.requester_user_id.to_string(),
            background_job_id: job.background_job_id.to_string(),
            source_name: job.source_name_snapshot,
            duplicate_policy: job.duplicate_policy,
            status: job.status,
            total_rows: job.total_rows,
            processed_rows: job.processed_rows,
            success_count: job.success_count,
            skipped_count: job.skipped_count,
            failure_count: job.failure_count,
            cancel_requested: job.cancel_requested,
            report_available: job.error_report_file_id.is_some(),
            last_error: job.last_error,
            started_at: job.started_at,
            completed_at: job.completed_at,
            created_at: job.created_at,
            updated_at: job.updated_at,
        }
    }
}

/// 面向管理端的导入异常行安全视图。
#[derive(Clone, Debug, Serialize)]
pub struct UserImportRowVo {
    pub row_number: i32,
    pub username: String,
    pub outcome: String,
    pub code: String,
    pub message: String,
    pub created_at: DateTime<Utc>,
}

impl From<user_import_row_result::Model> for UserImportRowVo {
    fn from(row: user_import_row_result::Model) -> Self {
        Self {
            row_number: row.row_number,
            username: row.username_snapshot,
            outcome: row.outcome,
            code: row.code,
            message: row.message,
            created_at: row.created_at,
        }
    }
}

/// 幂等创建导入任务的结果。
pub struct RequestUserImportOutcome {
    pub job: UserImportJobVo,
    pub inserted: bool,
}

/// 创建异步导入任务的受控输入。
pub struct RequestUserImportCommand {
    pub idempotency_key_hash: String,
    pub source_file_id: i64,
    pub source_name: String,
    pub source_sha256: String,
}

/// 用户导入列表查询。
pub struct UserImportListParams {
    pub page: ValidatedPageQuery,
    pub status: Option<String>,
}

#[derive(Clone)]
pub struct UserImportService {
    db: DatabaseCluster,
    queue: Arc<JobQueue>,
    user_service: Arc<UserService>,
    file_service: Arc<FileService>,
    config: UserImportConfig,
    hash_permits: Arc<Semaphore>,
}

impl UserImportService {
    pub fn new(
        db: DatabaseCluster,
        queue: Arc<JobQueue>,
        user_service: Arc<UserService>,
        file_service: Arc<FileService>,
        config: UserImportConfig,
    ) -> Self {
        Self {
            db,
            queue,
            user_service,
            file_service,
            hash_permits: Arc::new(Semaphore::new(config.hash_parallelism)),
            config,
        }
    }

    pub fn upload_config(&self) -> UploadConfig {
        UploadConfig {
            upload_dir: IMPORT_BUCKET.to_owned(),
            max_file_size: u64::try_from(self.config.max_file_bytes).unwrap_or(u64::MAX),
            allowed_extensions: vec!["xlsx".to_owned()],
        }
    }

    pub async fn find_by_idempotency(
        &self,
        actor: &ActorContext,
        idempotency_key_hash: &str,
    ) -> AppResult<Option<UserImportJobVo>> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        validate_sha256("幂等键", idempotency_key_hash)?;
        UserImportRepository
            .find_by_idempotency(self.db.write(), tenant_id, idempotency_key_hash)
            .await
            .map(|job| job.map(UserImportJobVo::from))
    }

    pub async fn request(
        &self,
        actor: &ActorContext,
        command: RequestUserImportCommand,
    ) -> AppResult<RequestUserImportOutcome> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        validate_sha256("幂等键", &command.idempotency_key_hash)?;
        validate_sha256("源文件", &command.source_sha256)?;
        if command.source_file_id <= 0 {
            return Err(AppError::Validation("导入源文件标识无效".into()));
        }
        if command.source_name.is_empty() || command.source_name.len() > 255 {
            return Err(AppError::Validation(
                "导入文件名长度必须介于 1 和 255 字节之间".into(),
            ));
        }

        let transaction = self.db.write().begin().await.map_err(database_error)?;
        TenantRepository
            .lock_tenant_in_txn(&transaction, tenant_id)
            .await?;
        if let Some(existing) = UserImportRepository
            .find_by_idempotency_in_txn(&transaction, tenant_id, &command.idempotency_key_hash)
            .await?
        {
            transaction.rollback().await.map_err(database_error)?;
            return Ok(RequestUserImportOutcome {
                job: UserImportJobVo::from(existing),
                inserted: false,
            });
        }
        let active = UserImportRepository
            .count_active_in_txn(&transaction, tenant_id)
            .await?;
        if active
            >= u64::try_from(self.config.max_active_per_tenant)
                .map_err(|_| AppError::Config("用户导入活动任务上限无效".into()))?
        {
            return Err(AppError::Conflict(
                "当前租户已有进行中的用户导入任务".into(),
            ));
        }

        let import_id = try_next_snowflake_id()?;
        let now = UserImportRepository.database_utc_now(&transaction).await?;
        let trace_context = crate::trace_context::current_trace_context();
        let queued = self
            .queue
            .enqueue_in_transaction(
                &transaction,
                EnqueueBackgroundJob {
                    tenant_id: Some(tenant_id.to_owned()),
                    schedule_id: None,
                    scheduled_for: Some(now),
                    max_runtime_seconds: Some(USER_IMPORT_MAX_RUNTIME_SECONDS),
                    job_type: USER_IMPORT_JOB_TYPE.to_owned(),
                    payload: serde_json::json!({ "import_job_id": import_id.to_string() }),
                    priority: 0,
                    available_at: now,
                    max_attempts: USER_IMPORT_MAX_ATTEMPTS,
                    dedupe_key: Some(format!("{tenant_id}:{}", command.idempotency_key_hash)),
                    traceparent: trace_context.traceparent,
                    tracestate: trace_context.tracestate,
                },
            )
            .await?;
        let job = UserImportRepository
            .create_in_txn(
                &transaction,
                CreateUserImportJob {
                    id: import_id,
                    tenant_id: tenant_id.to_owned(),
                    requester_user_id: actor.user_id,
                    background_job_id: queued.job.id,
                    idempotency_key_hash: command.idempotency_key_hash,
                    source_file_id: command.source_file_id,
                    source_name_snapshot: command.source_name,
                    source_sha256: command.source_sha256,
                },
                now,
            )
            .await?;
        crate::commit_current_audit(transaction).await?;
        self.queue.notify_background_jobs().await;
        Ok(RequestUserImportOutcome {
            job: UserImportJobVo::from(job),
            inserted: true,
        })
    }

    pub async fn list(
        &self,
        actor: &ActorContext,
        params: UserImportListParams,
    ) -> AppResult<PageResult<UserImportJobVo>> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        let status = normalize_status(params.status.as_deref())?;
        let page = UserImportRepository
            .list_for_tenant(
                self.db.write(),
                tenant_id,
                &params.page,
                UserImportFilter {
                    status: status.as_deref(),
                },
            )
            .await?;
        Ok(PageResult::new(
            page.records
                .into_iter()
                .map(UserImportJobVo::from)
                .collect(),
            page.total,
            &params.page,
        ))
    }

    pub async fn get(&self, actor: &ActorContext, id: i64) -> AppResult<UserImportJobVo> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        UserImportRepository
            .find_by_id_for_tenant(self.db.write(), tenant_id, id)
            .await?
            .map(UserImportJobVo::from)
            .ok_or_else(|| AppError::NotFound("用户导入任务不存在".into()))
    }

    pub async fn rows(
        &self,
        actor: &ActorContext,
        id: i64,
        page: ValidatedPageQuery,
    ) -> AppResult<PageResult<UserImportRowVo>> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        self.ensure_visible(tenant_id, id).await?;
        let rows = UserImportRepository
            .list_row_results(self.db.write(), tenant_id, id, &page)
            .await?;
        Ok(PageResult::new(
            rows.records
                .into_iter()
                .map(UserImportRowVo::from)
                .collect(),
            rows.total,
            &page,
        ))
    }

    pub async fn cancel(&self, actor: &ActorContext, id: i64) -> AppResult<UserImportJobVo> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        self.ensure_visible(tenant_id, id).await?;
        let now = UserImportRepository
            .database_utc_now(self.db.write())
            .await?;
        if !UserImportRepository
            .request_cancel(self.db.write(), tenant_id, id, now)
            .await?
        {
            return Err(AppError::Conflict("用户导入任务已结束或状态已变化".into()));
        }
        self.get(actor, id).await
    }

    pub async fn download_report(
        &self,
        actor: &ActorContext,
        id: i64,
    ) -> AppResult<DownloadedFile> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        let job = UserImportRepository
            .find_by_id_for_tenant(self.db.write(), tenant_id, id)
            .await?
            .ok_or_else(|| AppError::NotFound("用户导入任务不存在".into()))?;
        if !job.is_terminal() {
            return Err(AppError::Conflict("用户导入报告尚未生成".into()));
        }
        let file_id = job.error_report_file_id.ok_or_else(|| {
            if job.failure_count == 0 && job.skipped_count == 0 {
                AppError::NotFound("该导入任务没有失败或跳过记录".into())
            } else {
                AppError::Conflict("用户导入报告尚未就绪".into())
            }
        })?;
        self.file_service
            .download_by_id(actor, file_id, IMPORT_BUCKET)
            .await
    }

    async fn ensure_visible(&self, tenant_id: &str, id: i64) -> AppResult<()> {
        UserImportRepository
            .find_by_id_for_tenant(self.db.write(), tenant_id, id)
            .await?
            .ok_or_else(|| AppError::NotFound("用户导入任务不存在".into()))?;
        Ok(())
    }

    /// 执行后台导入；已提交批次通过持久化游标恢复，不会重复创建用户。
    pub async fn execute_background_job(&self, background_job_id: i64) -> AppResult<()> {
        let mut import = UserImportRepository
            .find_by_background_job(self.db.write(), background_job_id)
            .await?
            .ok_or_else(|| AppError::NotFound("后台任务没有关联用户导入记录".into()))?;
        if import.is_terminal() {
            return self.ensure_error_report(&import).await;
        }

        import = self.mark_running(import.id).await?;
        if import.status == user_import_job::Model::STATUS_CANCELLED {
            return Ok(());
        }
        let source = self
            .file_service
            .download_internal(&import.tenant_id, import.source_file_id, IMPORT_BUCKET)
            .await?;
        let source_sha256 = hex::encode(Sha256::digest(&source.data));
        if source_sha256 != import.source_sha256 {
            self.mark_failed(import.id, "导入源文件完整性校验失败")
                .await?;
            return Ok(());
        }
        let rows = tokio::task::spawn_blocking(move || {
            ExcelImporter::read_rows_from_bytes::<UserImportData>(&source.data, None)
        })
        .await
        .map_err(|error| AppError::Internal(format!("用户导入解析任务异常结束: {error}")))??;

        if rows.len() > self.config.max_rows {
            self.mark_failed(
                import.id,
                &format!("导入行数超过 {} 条上限", self.config.max_rows),
            )
            .await?;
            return Ok(());
        }
        if rows.is_empty() {
            self.mark_failed(import.id, "导入文件未包含任何用户数据行")
                .await?;
            return Ok(());
        }
        self.set_total_rows(import.id, rows.len()).await?;
        self.process_rows(import.id, rows).await?;
        let finished = self.finalize_import(import.id).await?;
        self.ensure_error_report(&finished).await
    }

    async fn process_rows(
        &self,
        import_id: i64,
        rows: Vec<ExcelImportRow<UserImportData>>,
    ) -> AppResult<()> {
        loop {
            let current = user_import_job::Entity::find_by_id(import_id)
                .one(self.db.write())
                .await
                .map_err(database_error)?
                .ok_or_else(|| AppError::NotFound("用户导入任务不存在".into()))?;
            if current.cancel_requested {
                self.mark_cancelled(import_id).await?;
                return Ok(());
            }
            if current.is_terminal() {
                return Ok(());
            }
            let offset = usize::try_from(current.processed_rows)
                .map_err(|_| AppError::Internal("用户导入进度游标无效".into()))?;
            if offset >= rows.len() {
                return Ok(());
            }

            let authorization = match self
                .user_service
                .resolve_current_authorization(
                    &current.tenant_id,
                    current.requester_user_id,
                    USER_IMPORT_PERMISSION,
                )
                .await
            {
                Ok(authorization) => authorization,
                Err(error) if is_terminal_authorization_error(&error) => {
                    self.mark_failed(import_id, &error.to_string()).await?;
                    return Ok(());
                }
                Err(error) => return Err(error),
            };
            let end = offset
                .saturating_add(self.config.batch_size)
                .min(rows.len());
            let prepared = self
                .prepare_batch(&current.tenant_id, &authorization.actor, &rows[offset..end])
                .await?;
            self.commit_batch(import_id, offset, end, prepared).await?;
        }
    }

    async fn prepare_batch(
        &self,
        tenant_id: &str,
        actor: &ActorContext,
        rows: &[ExcelImportRow<UserImportData>],
    ) -> AppResult<PreparedBatch> {
        let mut issues = Vec::new();
        let mut candidates = Vec::new();
        let mut batch_usernames = HashSet::new();

        for row in rows {
            let row_number = i32::try_from(row.row_number)
                .map_err(|_| AppError::Validation("Excel 行号超出支持范围".into()))?;
            let mut data = match &row.value {
                Ok(data) => data.clone(),
                Err(error) => {
                    issues.push(RowIssue::failed(row_number, "", "invalid_row", error));
                    continue;
                }
            };
            normalize_import_data(&mut data);
            if let Err(error) = data.validate() {
                issues.push(RowIssue::failed(
                    row_number,
                    &data.username,
                    "validation_failed",
                    &error.to_string(),
                ));
                continue;
            }
            let dept_id = match parse_required_department(data.dept_id.as_deref()) {
                Ok(dept_id) => dept_id,
                Err(error) => {
                    issues.push(RowIssue::failed(
                        row_number,
                        &data.username,
                        "invalid_department",
                        &error.to_string(),
                    ));
                    continue;
                }
            };
            if !batch_usernames.insert(data.username.clone()) {
                issues.push(RowIssue::skipped(
                    row_number,
                    &data.username,
                    "duplicate_in_file",
                    "同一批次中已出现相同用户名",
                ));
                continue;
            }
            candidates.push(ImportCandidate {
                row_number,
                data,
                dept_id,
            });
        }

        let mut requested_depts = candidates
            .iter()
            .map(|candidate| candidate.dept_id)
            .collect::<Vec<_>>();
        requested_depts.sort_unstable();
        requested_depts.dedup();
        let existing_depts = DeptRepository
            .find_filtered_by_ids(self.db.write(), tenant_id, None, None, &requested_depts)
            .await?
            .into_iter()
            .map(|dept| dept.id)
            .collect::<HashSet<_>>();

        let mut valid = Vec::new();
        for candidate in candidates {
            if !existing_depts.contains(&candidate.dept_id) {
                issues.push(RowIssue::failed(
                    candidate.row_number,
                    &candidate.data.username,
                    "department_not_found",
                    "部门不存在或不属于当前租户",
                ));
            } else if !department_is_visible(actor, candidate.dept_id) {
                issues.push(RowIssue::failed(
                    candidate.row_number,
                    &candidate.data.username,
                    "department_out_of_scope",
                    "部门超出申请人的当前数据范围",
                ));
            } else {
                valid.push(candidate);
            }
        }

        let prepared = try_join_all(valid.into_iter().map(|candidate| {
            let permits = self.hash_permits.clone();
            async move {
                let permit = permits
                    .acquire_owned()
                    .await
                    .map_err(|_| AppError::Internal("用户导入密码哈希并发控制器已关闭".into()))?;
                let activation_secret = format!("pending:{}", Uuid::new_v4());
                let password_hash = tokio::task::spawn_blocking(move || {
                    let _permit = permit;
                    password::hash(&activation_secret)
                })
                .await
                .map_err(|error| AppError::Internal(format!("密码哈希任务异常结束: {error}")))??;
                Ok::<_, AppError>(PreparedUser {
                    candidate,
                    password_hash,
                })
            }
        }))
        .await?;

        Ok(PreparedBatch {
            users: prepared,
            issues,
        })
    }

    async fn commit_batch(
        &self,
        import_id: i64,
        expected_offset: usize,
        next_offset: usize,
        mut prepared: PreparedBatch,
    ) -> AppResult<()> {
        let transaction = self.db.write().begin().await.map_err(database_error)?;
        let mut import = UserImportRepository
            .lock_by_id_in_txn(&transaction, import_id)
            .await?
            .ok_or_else(|| AppError::NotFound("用户导入任务不存在".into()))?;
        if import.cancel_requested {
            let now = UserImportRepository.database_utc_now(&transaction).await?;
            import.status = user_import_job::Model::STATUS_CANCELLED.to_owned();
            import.completed_at = Some(now);
            import.updated_at = now;
            UserImportRepository
                .save_in_txn(&transaction, import)
                .await?;
            transaction.commit().await.map_err(database_error)?;
            return Ok(());
        }
        if usize::try_from(import.processed_rows).ok() != Some(expected_offset) {
            transaction.rollback().await.map_err(database_error)?;
            return Ok(());
        }
        let now = UserImportRepository.database_utc_now(&transaction).await?;
        let usernames = prepared
            .users
            .iter()
            .map(|item| item.candidate.data.username.clone())
            .collect::<Vec<_>>();
        TenantRepository
            .lock_tenant_in_txn(&transaction, &import.tenant_id)
            .await?;
        let existing = UserRepository
            .find_existing_usernames_in_txn(&transaction, &import.tenant_id, &usernames)
            .await?
            .into_iter()
            .collect::<HashSet<_>>();
        let mut new_users = Vec::new();
        for prepared_user in prepared.users {
            if existing.contains(&prepared_user.candidate.data.username) {
                prepared.issues.push(RowIssue::skipped(
                    prepared_user.candidate.row_number,
                    &prepared_user.candidate.data.username,
                    "username_exists",
                    "用户名已存在，未覆盖现有用户",
                ));
            } else {
                new_users.push(prepared_user);
            }
        }

        if !new_users.is_empty()
            && let Err(error) = TenantRepository
                .ensure_user_quota_for_batch_in_txn(
                    &transaction,
                    &import.tenant_id,
                    new_users.len(),
                )
                .await
        {
            if !matches!(error, AppError::Validation(_)) {
                return Err(error);
            }
            for user in new_users.drain(..) {
                prepared.issues.push(RowIssue::failed(
                    user.candidate.row_number,
                    &user.candidate.data.username,
                    "tenant_quota_exceeded",
                    "当前批次将超过租户用户配额",
                ));
            }
        }

        let user_models = new_users
            .into_iter()
            .map(|prepared| build_user_model(&import.tenant_id, prepared, now))
            .collect::<AppResult<Vec<_>>>()?;
        let success_count = i32::try_from(user_models.len())
            .map_err(|_| AppError::Internal("用户导入成功计数溢出".into()))?;
        let skipped_count = i32::try_from(
            prepared
                .issues
                .iter()
                .filter(|issue| issue.outcome == user_import_row_result::Model::OUTCOME_SKIPPED)
                .count(),
        )
        .map_err(|_| AppError::Internal("用户导入跳过计数溢出".into()))?;
        let failure_count = i32::try_from(
            prepared
                .issues
                .iter()
                .filter(|issue| issue.outcome == user_import_row_result::Model::OUTCOME_FAILED)
                .count(),
        )
        .map_err(|_| AppError::Internal("用户导入失败计数溢出".into()))?;

        UserRepository
            .insert_many_in_txn(&transaction, &import.tenant_id, user_models)
            .await?;
        let row_models = prepared
            .issues
            .into_iter()
            .map(|issue| issue.into_model(&import.tenant_id, import_id, now))
            .collect::<AppResult<Vec<_>>>()?;
        UserImportRepository
            .insert_row_results_in_txn(&transaction, row_models)
            .await?;
        import.processed_rows = i32::try_from(next_offset)
            .map_err(|_| AppError::Internal("用户导入进度计数溢出".into()))?;
        import.success_count = import.success_count.saturating_add(success_count);
        import.skipped_count = import.skipped_count.saturating_add(skipped_count);
        import.failure_count = import.failure_count.saturating_add(failure_count);
        import.updated_at = now;
        import.last_error = None;
        UserImportRepository
            .save_in_txn(&transaction, import)
            .await?;
        transaction.commit().await.map_err(database_error)
    }

    async fn mark_running(&self, import_id: i64) -> AppResult<user_import_job::Model> {
        let transaction = self.db.write().begin().await.map_err(database_error)?;
        let mut import = UserImportRepository
            .lock_by_id_in_txn(&transaction, import_id)
            .await?
            .ok_or_else(|| AppError::NotFound("用户导入任务不存在".into()))?;
        if import.is_terminal() {
            transaction.rollback().await.map_err(database_error)?;
            return Ok(import);
        }
        let now = UserImportRepository.database_utc_now(&transaction).await?;
        if import.cancel_requested {
            import.status = user_import_job::Model::STATUS_CANCELLED.to_owned();
            import.completed_at = Some(now);
        } else {
            import.status = user_import_job::Model::STATUS_RUNNING.to_owned();
            import.started_at.get_or_insert(now);
        }
        import.updated_at = now;
        let saved = UserImportRepository
            .save_in_txn(&transaction, import)
            .await?;
        transaction.commit().await.map_err(database_error)?;
        Ok(saved)
    }

    async fn set_total_rows(&self, import_id: i64, total: usize) -> AppResult<()> {
        let transaction = self.db.write().begin().await.map_err(database_error)?;
        let mut import = UserImportRepository
            .lock_by_id_in_txn(&transaction, import_id)
            .await?
            .ok_or_else(|| AppError::NotFound("用户导入任务不存在".into()))?;
        let total = i32::try_from(total)
            .map_err(|_| AppError::Validation("用户导入行数超出数据库范围".into()))?;
        if import.processed_rows > total {
            return Err(AppError::Internal("用户导入进度超过源文件行数".into()));
        }
        let now = UserImportRepository.database_utc_now(&transaction).await?;
        import.total_rows = total;
        import.updated_at = now;
        UserImportRepository
            .save_in_txn(&transaction, import)
            .await?;
        transaction.commit().await.map_err(database_error)
    }

    async fn finalize_import(&self, import_id: i64) -> AppResult<user_import_job::Model> {
        let transaction = self.db.write().begin().await.map_err(database_error)?;
        let mut import = UserImportRepository
            .lock_by_id_in_txn(&transaction, import_id)
            .await?
            .ok_or_else(|| AppError::NotFound("用户导入任务不存在".into()))?;
        if import.is_terminal() {
            transaction.rollback().await.map_err(database_error)?;
            return Ok(import);
        }
        if import.processed_rows != import.total_rows {
            return Err(AppError::Conflict("用户导入尚未处理完全部行".into()));
        }
        let now = UserImportRepository.database_utc_now(&transaction).await?;
        import.status = if import.failure_count > 0 || import.skipped_count > 0 {
            user_import_job::Model::STATUS_PARTIAL.to_owned()
        } else {
            user_import_job::Model::STATUS_SUCCEEDED.to_owned()
        };
        import.completed_at = Some(now);
        import.updated_at = now;
        import.last_error = None;
        let saved = UserImportRepository
            .save_in_txn(&transaction, import)
            .await?;
        transaction.commit().await.map_err(database_error)?;
        Ok(saved)
    }

    async fn mark_cancelled(&self, import_id: i64) -> AppResult<()> {
        self.mark_terminal(import_id, user_import_job::Model::STATUS_CANCELLED, None)
            .await
            .map(|_| ())
    }

    async fn mark_failed(&self, import_id: i64, error: &str) -> AppResult<user_import_job::Model> {
        self.mark_terminal(
            import_id,
            user_import_job::Model::STATUS_FAILED,
            Some(error),
        )
        .await
    }

    async fn mark_terminal(
        &self,
        import_id: i64,
        status: &str,
        error: Option<&str>,
    ) -> AppResult<user_import_job::Model> {
        let transaction = self.db.write().begin().await.map_err(database_error)?;
        let mut import = UserImportRepository
            .lock_by_id_in_txn(&transaction, import_id)
            .await?
            .ok_or_else(|| AppError::NotFound("用户导入任务不存在".into()))?;
        let now = UserImportRepository.database_utc_now(&transaction).await?;
        import.status = status.to_owned();
        import.completed_at = Some(now);
        import.updated_at = now;
        import.last_error = error.map(truncate_error);
        let saved = UserImportRepository
            .save_in_txn(&transaction, import)
            .await?;
        transaction.commit().await.map_err(database_error)?;
        Ok(saved)
    }

    async fn ensure_error_report(&self, import: &user_import_job::Model) -> AppResult<()> {
        if import.error_report_file_id.is_some()
            || import.failure_count.saturating_add(import.skipped_count) == 0
        {
            return Ok(());
        }
        let rows = UserImportRepository
            .all_row_results(self.db.write(), &import.tenant_id, import.id)
            .await?;
        if rows.is_empty() {
            return Ok(());
        }
        let report_rows = rows
            .into_iter()
            .map(|row| UserImportReportRow {
                row_number: row.row_number,
                username: row.username_snapshot,
                outcome: row.outcome,
                code: row.code,
                message: row.message,
            })
            .collect::<Vec<_>>();
        let bytes = tokio::task::spawn_blocking(move || {
            ExcelExporter::export_to_bytes(
                &report_rows,
                "导入结果",
                UserImportReportRow::excel_headers(),
            )
        })
        .await
        .map_err(|error| AppError::Internal(format!("用户导入报告生成任务异常结束: {error}")))??;
        let mut config = self.upload_config();
        config.max_file_size = config
            .max_file_size
            .max(u64::try_from(bytes.len()).unwrap_or(u64::MAX));
        let uploaded = self
            .file_service
            .upload_internal(
                &import.tenant_id,
                "system:user-import",
                UploadCommand {
                    original_name: format!("user_import_{}_report.xlsx", import.id),
                    data: bytes,
                    config: &config,
                    bucket: IMPORT_BUCKET,
                    compress: false,
                },
            )
            .await?;
        let file_id = uploaded
            .file_id
            .parse::<i64>()
            .map_err(|_| AppError::Internal("用户导入报告文件标识无效".into()))?;
        let transaction = self.db.write().begin().await.map_err(database_error)?;
        let mut current = UserImportRepository
            .lock_by_id_in_txn(&transaction, import.id)
            .await?
            .ok_or_else(|| AppError::NotFound("用户导入任务不存在".into()))?;
        if current.error_report_file_id.is_none() {
            let now = UserImportRepository.database_utc_now(&transaction).await?;
            current.error_report_file_id = Some(file_id);
            current.updated_at = now;
            current.last_error = None;
            UserImportRepository
                .save_in_txn(&transaction, current)
                .await?;
            transaction.commit().await.map_err(database_error)?;
        } else {
            transaction.rollback().await.map_err(database_error)?;
        }
        Ok(())
    }

    pub async fn record_execution_failure(
        &self,
        background_job_id: i64,
        terminal: bool,
        error: &str,
    ) -> AppResult<()> {
        let Some(import) = UserImportRepository
            .find_by_background_job(self.db.write(), background_job_id)
            .await?
        else {
            return Ok(());
        };
        if import.is_terminal() {
            return Ok(());
        }
        let transaction = self.db.write().begin().await.map_err(database_error)?;
        let mut current = UserImportRepository
            .lock_by_id_in_txn(&transaction, import.id)
            .await?
            .ok_or_else(|| AppError::NotFound("用户导入任务不存在".into()))?;
        let now = UserImportRepository.database_utc_now(&transaction).await?;
        current.status = if terminal {
            user_import_job::Model::STATUS_FAILED.to_owned()
        } else {
            user_import_job::Model::STATUS_PENDING.to_owned()
        };
        current.last_error = Some(truncate_error(error));
        current.updated_at = now;
        current.completed_at = terminal.then_some(now);
        UserImportRepository
            .save_in_txn(&transaction, current)
            .await?;
        transaction.commit().await.map_err(database_error)
    }
}

/// Worker 中执行可恢复用户导入的处理器。
pub struct UserImportJobHandler {
    service: Arc<UserImportService>,
}

impl UserImportJobHandler {
    pub fn new(service: Arc<UserImportService>) -> Self {
        Self { service }
    }
}

#[async_trait]
impl JobHandler for UserImportJobHandler {
    fn job_type(&self) -> &'static str {
        USER_IMPORT_JOB_TYPE
    }

    async fn handle(&self, job: &background_job::Model) -> AppResult<()> {
        match self.service.execute_background_job(job.id).await {
            Ok(()) => Ok(()),
            Err(error) => {
                let terminal = is_terminal_import_error(&error) || job.attempts >= job.max_attempts;
                if let Err(record_error) = self
                    .service
                    .record_execution_failure(job.id, terminal, &error.to_string())
                    .await
                {
                    tracing::error!(%record_error, job_id = job.id, "记录用户导入失败状态时发生错误");
                }
                if is_terminal_import_error(&error) {
                    tracing::warn!(%error, job_id = job.id, "用户导入因不可重试错误终止");
                    Ok(())
                } else {
                    Err(error)
                }
            }
        }
    }
}

struct ImportCandidate {
    row_number: i32,
    data: UserImportData,
    dept_id: i64,
}

struct PreparedUser {
    candidate: ImportCandidate,
    password_hash: String,
}

struct PreparedBatch {
    users: Vec<PreparedUser>,
    issues: Vec<RowIssue>,
}

struct RowIssue {
    row_number: i32,
    username: String,
    outcome: &'static str,
    code: String,
    message: String,
}

impl RowIssue {
    fn failed(row_number: i32, username: &str, code: &str, message: &str) -> Self {
        Self::new(
            row_number,
            username,
            user_import_row_result::Model::OUTCOME_FAILED,
            code,
            message,
        )
    }

    fn skipped(row_number: i32, username: &str, code: &str, message: &str) -> Self {
        Self::new(
            row_number,
            username,
            user_import_row_result::Model::OUTCOME_SKIPPED,
            code,
            message,
        )
    }

    fn new(
        row_number: i32,
        username: &str,
        outcome: &'static str,
        code: &str,
        message: &str,
    ) -> Self {
        Self {
            row_number,
            username: truncate_utf8(username, 64),
            outcome,
            code: truncate_utf8(code, 64),
            message: truncate_utf8(message, 500),
        }
    }

    fn into_model(
        self,
        tenant_id: &str,
        import_job_id: i64,
        now: DateTime<Utc>,
    ) -> AppResult<user_import_row_result::Model> {
        Ok(user_import_row_result::Model {
            id: try_next_snowflake_id()?,
            tenant_id: tenant_id.to_owned(),
            import_job_id,
            row_number: self.row_number,
            username_snapshot: self.username,
            outcome: self.outcome.to_owned(),
            code: self.code,
            message: self.message,
            created_at: now,
        })
    }
}

#[derive(Serialize)]
struct UserImportReportRow {
    row_number: i32,
    username: String,
    outcome: String,
    code: String,
    message: String,
}

impl UserImportReportRow {
    const fn excel_headers() -> &'static [(&'static str, &'static str)] {
        &[
            ("row_number", "行号"),
            ("username", "用户名"),
            ("outcome", "结果"),
            ("code", "代码"),
            ("message", "说明"),
        ]
    }
}

fn build_user_model(
    tenant_id: &str,
    prepared: PreparedUser,
    now: DateTime<Utc>,
) -> AppResult<user::Model> {
    Ok(user::Model {
        id: try_next_snowflake_id()?,
        tenant_id: tenant_id.to_owned(),
        username: prepared.candidate.data.username,
        password_hash: prepared.password_hash,
        nickname: prepared.candidate.data.nickname,
        email: prepared.candidate.data.email,
        phone: prepared.candidate.data.phone.unwrap_or_default(),
        avatar: None,
        avatar_file_id: None,
        preferred_locale: None,
        status: user::Model::STATUS_PENDING_ACTIVATION.to_owned(),
        authorization_version: 1,
        dept_id: Some(prepared.candidate.dept_id),
        remark: None,
        login_ip: None,
        login_date: None,
        del_flag: user::Model::DEL_FLAG_NORMAL.to_owned(),
        created_at: now,
        updated_at: now,
    })
}

fn normalize_import_data(data: &mut UserImportData) {
    data.username = data.username.trim().to_owned();
    data.nickname = data.nickname.trim().to_owned();
    data.email = data.email.trim().to_owned();
    data.phone = data
        .phone
        .take()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty());
    data.dept_id = data
        .dept_id
        .take()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty());
}

fn parse_required_department(value: Option<&str>) -> AppResult<i64> {
    let value = value.ok_or_else(|| AppError::Validation("部门ID不能为空".into()))?;
    let id = value
        .parse::<i64>()
        .map_err(|_| AppError::Validation("部门ID必须是正整数".into()))?;
    if id <= 0 {
        return Err(AppError::Validation("部门ID必须是正整数".into()));
    }
    Ok(id)
}

fn department_is_visible(actor: &ActorContext, dept_id: i64) -> bool {
    if actor.is_super_admin || actor.data_scope == DataScope::All {
        return true;
    }
    match actor.data_scope {
        DataScope::All => true,
        DataScope::SelfOnly => false,
        DataScope::Dept => actor.dept_id == Some(dept_id),
        DataScope::DeptAndChildren | DataScope::Custom => actor.custom_dept_ids.contains(&dept_id),
    }
}

fn normalize_status(value: Option<&str>) -> AppResult<Option<String>> {
    let Some(value) = value.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok(None);
    };
    if !matches!(
        value,
        user_import_job::Model::STATUS_PENDING
            | user_import_job::Model::STATUS_RUNNING
            | user_import_job::Model::STATUS_SUCCEEDED
            | user_import_job::Model::STATUS_PARTIAL
            | user_import_job::Model::STATUS_FAILED
            | user_import_job::Model::STATUS_CANCELLED
    ) {
        return Err(AppError::Validation("用户导入状态筛选无效".into()));
    }
    Ok(Some(value.to_owned()))
}

fn validate_sha256(name: &str, value: &str) -> AppResult<()> {
    if value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(AppError::Validation(format!("{name}摘要格式无效")));
    }
    Ok(())
}

fn is_terminal_authorization_error(error: &AppError) -> bool {
    matches!(
        error,
        AppError::Validation(_) | AppError::Authorization(_) | AppError::NotFound(_)
    )
}

fn is_terminal_import_error(error: &AppError) -> bool {
    matches!(
        error,
        AppError::Validation(_)
            | AppError::Authentication(_)
            | AppError::Authorization(_)
            | AppError::NotFound(_)
            | AppError::Conflict(_)
            | AppError::PayloadTooLarge(_)
    )
}

fn truncate_error(value: &str) -> String {
    truncate_utf8(value, 4_000)
}

fn truncate_utf8(value: &str, maximum_bytes: usize) -> String {
    if value.len() <= maximum_bytes {
        return value.to_owned();
    }
    let mut end = maximum_bytes.saturating_sub('…'.len_utf8());
    while !value.is_char_boundary(end) {
        end = end.saturating_sub(1);
    }
    format!("{}…", &value[..end])
}

fn database_error(error: impl std::fmt::Display) -> AppError {
    AppError::Database(error.to_string())
}
