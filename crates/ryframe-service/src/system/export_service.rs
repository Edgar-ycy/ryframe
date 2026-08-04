use std::sync::Arc;

use chrono::{DateTime, Duration, Utc};
use ryframe_config::JobConfig;
use ryframe_core::repository::Repository;
use ryframe_db::{
    BackgroundJobRepository, CreateExportJob, DatabaseCluster, EnqueueBackgroundJob,
    ExportJobRepository, FileRepository, MarkExportJobSucceeded, ReadConsistency,
    entities::{export_job, sys_file},
};
use ryframe_kernel::{ActorContext, AppError, AppResult};
use sea_orm::TransactionTrait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::Digest;

use super::{
    ConfigService, DictService, LoginInfoService, OperLogService, PostService, RoleService,
    UserService,
};

/// Worker 消费异步导出任务的稳定类型标识。
pub const EXPORT_JOB_TYPE: &str = "system.export.execute";

/// 清理过期导出结果的稳定任务类型标识。
pub const EXPORT_CLEANUP_JOB_TYPE: &str = "system.export.cleanup";

/// 导出文件的对象存储桶名称。
pub const EXPORT_BUCKET: &str = "exports";

/// 单次清理查询的最大任务数，游标会继续排空同一时间快照下的剩余任务。
const EXPORT_CLEANUP_BATCH_SIZE: u64 = 100;

/// 创建公开导出任务的通用参数。
#[derive(Clone, Debug)]
pub struct RequestExportCommand {
    pub resource: String,
    pub permission_code: String,
    pub request_params: Value,
}

/// 面向 API 的导出任务安全视图，不暴露内部后台任务载荷。
#[derive(Clone, Debug, Serialize)]
pub struct ExportJobVo {
    pub id: String,
    pub resource: String,
    pub status: String,
    pub result_file_name: Option<String>,
    pub content_type: Option<String>,
    pub file_size: Option<i64>,
    pub expires_at: Option<DateTime<Utc>>,
    pub error_message: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
}

/// 已完成导出对应的受控文件定位信息。
#[derive(Clone, Debug)]
pub struct ExportDownloadLocation {
    pub bucket: String,
    pub path: String,
}

impl From<export_job::Model> for ExportJobVo {
    fn from(job: export_job::Model) -> Self {
        Self {
            id: job.id.to_string(),
            resource: job.resource,
            status: job.status,
            result_file_name: job.result_file_name,
            content_type: job.content_type,
            file_size: job.file_size,
            expires_at: job.expires_at,
            error_message: job.error_message,
            created_at: job.created_at,
            updated_at: job.updated_at,
            completed_at: job.completed_at,
        }
    }
}

/// 异步导出任务服务。
pub struct ExportService {
    db: DatabaseCluster,
    background_jobs: BackgroundJobRepository,
    exports: ExportJobRepository,
    files: FileRepository,
    users: Arc<UserService>,
    roles: RoleService,
    posts: PostService,
    configs: ConfigService,
    dicts: DictService,
    oper_logs: OperLogService,
    login_infos: LoginInfoService,
    storage: Arc<dyn ryframe_storage::ObjectStorage>,
    default_max_attempts: i32,
    export_max_rows: usize,
    export_retention: Duration,
}

impl ExportService {
    pub fn new(
        db: DatabaseCluster,
        users: Arc<UserService>,
        storage: Arc<dyn ryframe_storage::ObjectStorage>,
        jobs: &JobConfig,
    ) -> Self {
        Self {
            db: db.clone(),
            background_jobs: BackgroundJobRepository,
            exports: ExportJobRepository,
            files: FileRepository,
            roles: RoleService::new(db.clone(), crate::AuthorizationCache::disabled()),
            posts: PostService::new(db.clone()),
            configs: ConfigService::new(db.clone(), crate::AuthorizationCache::disabled()),
            dicts: DictService::new(db.clone(), None),
            oper_logs: OperLogService::new(db.clone()),
            login_infos: LoginInfoService::new(db.clone()),
            users,
            storage,
            default_max_attempts: jobs.default_max_attempts,
            export_max_rows: jobs.export_max_rows,
            export_retention: Duration::hours(i64::from(jobs.export_retention_hours)),
        }
    }

    /// 在同一事务内创建内部 Worker 任务与公开导出任务。
    pub async fn request(
        &self,
        actor: &ActorContext,
        command: RequestExportCommand,
    ) -> AppResult<ExportJobVo> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        validate_request_command(&command)?;
        self.users
            .resolve_current_export_authorization(
                tenant_id,
                actor.user_id,
                &command.permission_code,
            )
            .await?;
        let transaction = self.db.write().begin().await.map_err(database_error)?;
        let result = async {
            let now = self.background_jobs.database_utc_now(&transaction).await?;
            let trace_context = crate::trace_context::current_trace_context();
            let job = self
                .background_jobs
                .enqueue_in_transaction(
                    &transaction,
                    EnqueueBackgroundJob {
                        tenant_id: Some(tenant_id.to_owned()),
                        job_type: EXPORT_JOB_TYPE.to_owned(),
                        payload: serde_json::json!({ "resource": command.resource }),
                        priority: 5,
                        available_at: now,
                        max_attempts: self.default_max_attempts,
                        dedupe_key: None,
                        traceparent: trace_context.traceparent,
                        tracestate: trace_context.tracestate,
                    },
                    now,
                )
                .await?;
            self.exports
                .create_in_transaction(
                    &transaction,
                    CreateExportJob {
                        tenant_id: tenant_id.to_owned(),
                        requester_id: actor.user_id,
                        resource: command.resource,
                        background_job_id: job.job.id,
                        request_params: serde_json::to_value(StoredExportRequest {
                            request: command.request_params,
                            authorization_fingerprint: None,
                        })
                        .map_err(|error| {
                            AppError::Internal(format!("导出请求编码失败: {error}"))
                        })?,
                        permission_code: command.permission_code,
                    },
                    now,
                )
                .await
        }
        .await;
        match result {
            Ok(export) => {
                crate::commit_current_audit(transaction).await?;
                Ok(ExportJobVo::from(export))
            }
            Err(error) => {
                let _ = transaction.rollback().await;
                Err(error)
            }
        }
    }

    /// 读取申请人自己的导出任务。
    pub async fn find_for_requester(
        &self,
        actor: &ActorContext,
        id: i64,
    ) -> AppResult<ExportJobVo> {
        validate_job_id(id)?;
        let tenant_id = crate::validated_tenant_id(actor)?;
        let db = self.db.select_read(ReadConsistency::Strong).connection;
        let export = self
            .exports
            .find_by_id_for_requester(&db, tenant_id, actor.user_id, id)
            .await?
            .ok_or_else(|| AppError::NotFound("导出任务不存在或不属于当前用户".into()))?;
        self.users
            .ensure_current_permission(actor, &export.permission_code)
            .await?;
        Ok(ExportJobVo::from(export))
    }

    /// 读取当前申请人仍具备查看权限的最近导出任务。
    pub async fn list_for_requester(&self, actor: &ActorContext) -> AppResult<Vec<ExportJobVo>> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        let db = self.db.select_read(ReadConsistency::Eventual).connection;
        let exports = self
            .exports
            .list_for_requester(&db, tenant_id, actor.user_id, 100)
            .await?;
        let mut result = Vec::with_capacity(exports.len());
        for export in exports {
            if self
                .users
                .ensure_current_permission(actor, &export.permission_code)
                .await
                .is_ok()
            {
                result.push(ExportJobVo::from(export));
            }
        }
        Ok(result)
    }

    /// 取消申请人自己的尚未完成导出任务。
    pub async fn cancel_for_requester(
        &self,
        actor: &ActorContext,
        id: i64,
    ) -> AppResult<ExportJobVo> {
        validate_job_id(id)?;
        let tenant_id = crate::validated_tenant_id(actor)?;
        let db = self.db.select_read(ReadConsistency::Strong).connection;
        let export = self
            .exports
            .find_by_id_for_requester(&db, tenant_id, actor.user_id, id)
            .await?
            .ok_or_else(|| AppError::NotFound("导出任务不存在或不属于当前用户".into()))?;
        self.users
            .ensure_current_permission(actor, &export.permission_code)
            .await?;
        let now = self
            .background_jobs
            .database_utc_now(self.db.write())
            .await?;
        let transaction = self.db.write().begin().await.map_err(database_error)?;
        if !self
            .exports
            .cancel_for_requester(&transaction, tenant_id, actor.user_id, id, now)
            .await?
        {
            let _ = transaction.rollback().await;
            return Err(AppError::Conflict(
                "导出任务已完成、已过期或状态已变化".into(),
            ));
        }
        crate::commit_current_audit(transaction).await?;
        self.find_for_requester(actor, id).await
    }

    /// 返回当前申请人尚未过期的导出结果位置。
    pub async fn download_location_for_requester(
        &self,
        actor: &ActorContext,
        id: i64,
    ) -> AppResult<ExportDownloadLocation> {
        validate_job_id(id)?;
        let tenant_id = crate::validated_tenant_id(actor)?;
        let db = self.db.select_read(ReadConsistency::Strong).connection;
        let export = self
            .exports
            .find_by_id_for_requester(&db, tenant_id, actor.user_id, id)
            .await?
            .ok_or_else(|| AppError::NotFound("导出任务不存在或不属于当前用户".into()))?;
        let (_, current_fingerprint) = self
            .users
            .resolve_current_export_authorization(tenant_id, actor.user_id, &export.permission_code)
            .await?;
        let stored_request: StoredExportRequest =
            serde_json::from_value(export.request_params.clone())
                .map_err(|_| AppError::Authorization("导出授权记录无效".into()))?;
        ensure_download_authorization_matches(
            stored_request.authorization_fingerprint.as_deref(),
            &current_fingerprint,
        )?;
        let now = self
            .background_jobs
            .database_utc_now(self.db.write())
            .await?;
        if export.status != export_job::Model::STATUS_SUCCEEDED
            || export.expires_at.is_none_or(|expires_at| expires_at <= now)
        {
            return Err(AppError::Conflict("导出结果尚未就绪或已过期".into()));
        }
        let file_id = export
            .result_file_id
            .ok_or_else(|| AppError::Internal("导出任务缺少结果文件".into()))?;
        let file = self
            .files
            .find_by_id(self.db.write(), tenant_id, file_id)
            .await?
            .ok_or_else(|| AppError::NotFound("导出结果文件不存在".into()))?;
        Ok(ExportDownloadLocation {
            bucket: file.bucket,
            path: file.storage_path,
        })
    }

    /// 清理过期导出结果。
    ///
    /// 整轮清理只读取一次数据库时间，并使用稳定主键游标排空所有已到期任务。
    /// 单个任务失败不会阻塞后续任务；完成本轮其余任务后返回首个错误，让后台任务重试失败项。
    pub async fn cleanup_expired(&self) -> AppResult<u64> {
        let now = self
            .background_jobs
            .database_utc_now(self.db.write())
            .await?;
        let mut cleaned = 0_u64;
        let mut failed = 0_u64;
        let mut first_error = None;
        let mut after_id = None;

        loop {
            let exports = self
                .exports
                .list_expired_succeeded_after_id(
                    self.db.write(),
                    now,
                    after_id,
                    EXPORT_CLEANUP_BATCH_SIZE,
                )
                .await?;
            if exports.is_empty() {
                break;
            }

            for export in exports {
                let export_id = export.id;
                after_id = Some(export_id);
                match self.cleanup_expired_export(export, now).await {
                    Ok(true) => cleaned += 1,
                    Ok(false) => {}
                    Err(error) => {
                        failed += 1;
                        tracing::warn!(
                            export_id,
                            %error,
                            "单个过期导出结果清理失败，将继续处理后续任务"
                        );
                        if first_error.is_none() {
                            first_error = Some(error);
                        }
                    }
                }
            }
        }

        if let Some(error) = first_error {
            tracing::warn!(cleaned, failed, "过期导出结果已完成部分清理，将重试失败项");
            Err(error)
        } else {
            Ok(cleaned)
        }
    }

    /// 清理一个已在固定时间快照下到期的导出结果。
    async fn cleanup_expired_export(
        &self,
        export: export_job::Model,
        now: DateTime<Utc>,
    ) -> AppResult<bool> {
        let Some(file_id) = export.result_file_id else {
            return self.mark_export_expired(export.id, now).await;
        };
        let Some(file) = self
            .files
            .find_by_id(self.db.write(), &export.tenant_id, file_id)
            .await?
        else {
            return self.mark_export_expired(export.id, now).await;
        };
        self.storage
            .delete(&file.bucket, &file.storage_path)
            .await
            .map_err(storage_error)?;
        self.delete_export_file_and_mark_expired(&export.tenant_id, export.id, file.id, now)
            .await
    }

    /// 在后台清理短事务内将无文件元数据的导出任务改为过期。
    async fn mark_export_expired(&self, export_id: i64, now: DateTime<Utc>) -> AppResult<bool> {
        let transaction = self.db.write().begin().await.map_err(database_error)?;
        let marked = self
            .exports
            .mark_expired(&transaction, export_id, now)
            .await?;
        crate::commit_current_audit(transaction).await?;
        Ok(marked)
    }

    /// 原子软删除导出文件元数据并将任务改为过期。
    ///
    /// 对象存储删除先于该事务执行；若事务失败，下一轮会再次执行幂等删除，
    /// 不会留下已过期任务仍引用可见文件元数据的中间状态。
    async fn delete_export_file_and_mark_expired(
        &self,
        tenant_id: &str,
        export_id: i64,
        file_id: i64,
        now: DateTime<Utc>,
    ) -> AppResult<bool> {
        let transaction = self.db.write().begin().await.map_err(database_error)?;
        let result = async {
            let current = self
                .exports
                .find_by_id_for_update_in_transaction(&transaction, export_id)
                .await?;
            let still_expired = current.is_some_and(|current| {
                current.tenant_id == tenant_id
                    && current.status == export_job::Model::STATUS_SUCCEEDED
                    && current.result_file_id == Some(file_id)
                    && current
                        .expires_at
                        .is_some_and(|expires_at| expires_at <= now)
            });
            if !still_expired {
                return Ok(false);
            }
            self.files
                .delete_in_txn(&transaction, tenant_id, file_id)
                .await?;
            self.exports
                .mark_expired(&transaction, export_id, now)
                .await
        }
        .await;
        match result {
            Ok(true) => {
                crate::commit_current_audit(transaction).await?;
                Ok(true)
            }
            Ok(false) => {
                let _ = transaction.rollback().await;
                Ok(false)
            }
            Err(error) => {
                let _ = transaction.rollback().await;
                Err(error)
            }
        }
    }

    /// 执行一个已领取的后台导出任务。
    pub async fn execute_background_job(&self, background_job_id: i64) -> AppResult<()> {
        let now = self
            .background_jobs
            .database_utc_now(self.db.write())
            .await?;
        let export = self
            .exports
            .find_by_background_job_id(self.db.write(), background_job_id)
            .await?
            .ok_or_else(|| AppError::NotFound("后台任务未关联导出请求".into()))?;
        if export.status == export_job::Model::STATUS_CANCELLED {
            return Ok(());
        }
        let transaction = self.db.write().begin().await.map_err(database_error)?;
        let marked_running = self
            .exports
            .mark_running(&transaction, export.id, now)
            .await?;
        crate::commit_current_audit(transaction).await?;
        if !marked_running && export.status != export_job::Model::STATUS_RUNNING {
            return Ok(());
        }
        let mut request: StoredExportRequest =
            serde_json::from_value(export.request_params.clone())
                .map_err(|error| AppError::Validation(format!("导出请求快照无效: {error}")))?;
        let (actor, authorization_fingerprint) = self
            .users
            .resolve_current_export_authorization(
                &export.tenant_id,
                export.requester_id,
                &export.permission_code,
            )
            .await?;
        request.authorization_fingerprint = Some(authorization_fingerprint);
        let request_params = serde_json::to_value(&request)
            .map_err(|error| AppError::Internal(format!("导出授权记录编码失败: {error}")))?;
        let request = request.request;
        let mut export = export;
        export.request_params = request_params;
        match export.resource.as_str() {
            "users" => self.execute_user_export(export, actor, request, now).await,
            "roles" => self.execute_role_export(export, actor, request, now).await,
            "posts" => self.execute_post_export(export, actor, request, now).await,
            "configs" => {
                self.execute_config_export(export, actor, request, now)
                    .await
            }
            "dict-types" => {
                self.execute_dict_type_export(export, actor, request, now)
                    .await
            }
            "operlogs" => {
                self.execute_oper_log_export(export, actor, request, now)
                    .await
            }
            "loginlogs" => {
                self.execute_login_log_export(export, actor, request, now)
                    .await
            }
            resource => Err(AppError::Validation(format!(
                "不支持的导出资源: {resource}"
            ))),
        }
    }

    /// 记录 Worker 执行失败，并与后台任务的重试预算保持一致。
    pub async fn record_execution_failure(
        &self,
        background_job_id: i64,
        terminal: bool,
        error_message: &str,
    ) -> AppResult<()> {
        let Some(export) = self
            .exports
            .find_by_background_job_id(self.db.write(), background_job_id)
            .await?
        else {
            return Ok(());
        };
        let now = self
            .background_jobs
            .database_utc_now(self.db.write())
            .await?;
        let transaction = self.db.write().begin().await.map_err(database_error)?;
        if terminal {
            self.exports
                .mark_failed(&transaction, export.id, error_message, now)
                .await?;
        } else {
            self.exports
                .mark_queued_after_failure(&transaction, export.id, error_message, now)
                .await?;
        }
        crate::commit_current_audit(transaction).await?;
        Ok(())
    }

    async fn execute_user_export(
        &self,
        export: export_job::Model,
        actor: ActorContext,
        request: Value,
        now: DateTime<Utc>,
    ) -> AppResult<()> {
        let filters: UserExportFilters = serde_json::from_value(request)
            .map_err(|error| AppError::Validation(format!("用户导出筛选条件无效: {error}")))?;
        let users = self
            .users
            .find_for_export(
                &actor,
                filters.username.as_deref(),
                filters.phone.as_deref(),
                filters.status.as_deref(),
                filters.dept_id,
                self.export_max_rows,
            )
            .await?;
        let data = users
            .into_iter()
            .map(|user| UserExportRow {
                user_id: user.id,
                username: user.username,
                nickname: user.nickname,
                email: user.email,
                phone: user.phone,
                dept_name: user.dept_name,
                status: user.status,
                remark: user.remark,
                created_at: user.created_at.to_rfc3339(),
            })
            .collect::<Vec<_>>();
        let bytes = ryframe_excel::ExcelExporter::export_to_bytes(
            &data,
            "用户数据",
            UserExportRow::headers(),
        )?;
        self.persist_export_file(export, actor, bytes, "users", now)
            .await
    }

    async fn execute_role_export(
        &self,
        export: export_job::Model,
        actor: ActorContext,
        request: Value,
        now: DateTime<Utc>,
    ) -> AppResult<()> {
        let filters: RoleExportFilters = decode_export_filters(request, "角色")?;
        let roles = self
            .roles
            .find_for_export(
                &actor,
                filters.name.as_deref(),
                filters.code.as_deref(),
                filters.status.as_deref(),
                self.export_max_rows,
            )
            .await?;
        let data = roles
            .into_iter()
            .map(|item| {
                serde_json::json!({
                    "role_id": item.id, "role_name": item.name, "role_code": item.code,
                    "data_scope": item.data_scope, "status": item.status, "sort": item.sort,
                    "remark": item.remark, "created_at": item.created_at.to_rfc3339(),
                })
            })
            .collect::<Vec<_>>();
        let bytes = ryframe_excel::ExcelExporter::export_to_bytes(&data, "角色数据", ROLE_HEADERS)?;
        self.persist_export_file(export, actor, bytes, "roles", now)
            .await
    }

    async fn execute_post_export(
        &self,
        export: export_job::Model,
        actor: ActorContext,
        request: Value,
        now: DateTime<Utc>,
    ) -> AppResult<()> {
        let filters: PostExportFilters = decode_export_filters(request, "岗位")?;
        let posts = self
            .posts
            .find_for_export(
                &actor,
                filters.name.as_deref(),
                filters.code.as_deref(),
                filters.status.as_deref(),
                self.export_max_rows,
            )
            .await?;
        let data = posts.into_iter().map(|item| serde_json::json!({
            "post_id": item.id, "name": item.name, "code": item.code, "sort": item.sort,
            "status": item.status, "remark": item.remark, "created_at": item.created_at.to_rfc3339(),
        })).collect::<Vec<_>>();
        let bytes = ryframe_excel::ExcelExporter::export_to_bytes(&data, "岗位数据", POST_HEADERS)?;
        self.persist_export_file(export, actor, bytes, "posts", now)
            .await
    }

    async fn execute_config_export(
        &self,
        export: export_job::Model,
        actor: ActorContext,
        request: Value,
        now: DateTime<Utc>,
    ) -> AppResult<()> {
        let filters: ConfigExportFilters = decode_export_filters(request, "参数配置")?;
        let configs = self
            .configs
            .find_for_export(
                &actor,
                filters.name.as_deref(),
                filters.key.as_deref(),
                self.export_max_rows,
            )
            .await?;
        let data = configs
            .into_iter()
            .map(|item| {
                serde_json::json!({
                    "name": item.name, "key": item.key, "value": item.value, "remark": item.remark,
                    "created_at": item.created_at.to_rfc3339(),
                })
            })
            .collect::<Vec<_>>();
        let bytes =
            ryframe_excel::ExcelExporter::export_to_bytes(&data, "参数配置", CONFIG_HEADERS)?;
        self.persist_export_file(export, actor, bytes, "configs", now)
            .await
    }

    async fn execute_dict_type_export(
        &self,
        export: export_job::Model,
        actor: ActorContext,
        request: Value,
        now: DateTime<Utc>,
    ) -> AppResult<()> {
        let filters: DictTypeExportFilters = decode_export_filters(request, "字典类型")?;
        let types = self
            .dicts
            .find_types_for_export(
                &actor,
                filters.name.as_deref(),
                filters.code.as_deref(),
                filters.status.as_deref(),
                self.export_max_rows,
            )
            .await?;
        let data = types.into_iter().map(|item| serde_json::json!({
            "name": item.name, "code": item.code, "status": item.status, "remark": item.remark,
            "created_at": item.created_at.to_rfc3339(),
        })).collect::<Vec<_>>();
        let bytes =
            ryframe_excel::ExcelExporter::export_to_bytes(&data, "字典类型", DICT_TYPE_HEADERS)?;
        self.persist_export_file(export, actor, bytes, "dict-types", now)
            .await
    }

    async fn execute_oper_log_export(
        &self,
        export: export_job::Model,
        actor: ActorContext,
        request: Value,
        now: DateTime<Utc>,
    ) -> AppResult<()> {
        let filters: LogExportFilters = decode_export_filters(request, "操作日志")?;
        let logs = self
            .oper_logs
            .find_for_export(
                &actor,
                filters.name.as_deref(),
                filters.status.as_deref(),
                filters.begin_time.as_deref(),
                filters.end_time.as_deref(),
                self.export_max_rows,
            )
            .await?;
        let data = logs.into_iter().map(|item| serde_json::json!({
            "title": item.title, "business_type": item.business_type, "oper_name": item.oper_name,
            "oper_url": item.oper_url, "oper_ip": item.oper_ip, "status": item.status,
            "cost_time": item.cost_time, "oper_time": item.oper_time,
        })).collect::<Vec<_>>();
        let bytes =
            ryframe_excel::ExcelExporter::export_to_bytes(&data, "操作日志", OPER_LOG_HEADERS)?;
        self.persist_export_file(export, actor, bytes, "operlogs", now)
            .await
    }

    async fn execute_login_log_export(
        &self,
        export: export_job::Model,
        actor: ActorContext,
        request: Value,
        now: DateTime<Utc>,
    ) -> AppResult<()> {
        let filters: LogExportFilters = decode_export_filters(request, "登录日志")?;
        let logs = self
            .login_infos
            .find_for_export(
                &actor,
                filters.name.as_deref(),
                filters.status.as_deref(),
                filters.begin_time.as_deref(),
                filters.end_time.as_deref(),
                self.export_max_rows,
            )
            .await?;
        let data = logs.into_iter().map(|item| serde_json::json!({
            "user_name": item.user_name, "ipaddr": item.ipaddr, "login_location": item.login_location,
            "browser": item.browser, "os": item.os, "status": item.status, "msg": item.msg,
            "login_time": item.login_time,
        })).collect::<Vec<_>>();
        let bytes =
            ryframe_excel::ExcelExporter::export_to_bytes(&data, "登录日志", LOGIN_LOG_HEADERS)?;
        self.persist_export_file(export, actor, bytes, "loginlogs", now)
            .await
    }

    async fn persist_export_file(
        &self,
        export: export_job::Model,
        actor: ActorContext,
        bytes: Vec<u8>,
        resource: &str,
        now: DateTime<Utc>,
    ) -> AppResult<()> {
        let (file_name, key) = export_file_location(&export.tenant_id, resource, export.id);
        let file_id = deterministic_export_file_id(export.id);
        let content_type =
            "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet".to_owned();
        let file_size = i64::try_from(bytes.len())
            .map_err(|_| AppError::PayloadTooLarge("导出文件超过数据库大小范围".into()))?;
        let file_sha256 = hex::encode(sha2::Sha256::digest(&bytes));
        self.storage
            .ensure_bucket(EXPORT_BUCKET)
            .await
            .map_err(storage_error)?;
        let file = sys_file::Model {
            id: file_id,
            tenant_id: export.tenant_id.clone(),
            original_name: file_name.clone(),
            storage_name: file_name.clone(),
            storage_path: key.clone(),
            bucket: EXPORT_BUCKET.into(),
            file_url: format!("{EXPORT_BUCKET}/{key}"),
            file_size,
            content_type: content_type.clone(),
            file_sha256,
            upload_by: Some(actor.username),
            upload_status: sys_file::Model::UPLOAD_STATUS_READY.into(),
            reservation_token: None,
            reservation_expires_at: None,
            del_flag: sys_file::Model::DEL_FLAG_NORMAL.into(),
            created_at: now,
            updated_at: now,
        };
        let transaction = self.db.write().begin().await.map_err(database_error)?;
        let result = async {
            let current = self
                .exports
                .find_by_id_for_update_in_transaction(&transaction, export.id)
                .await?
                .ok_or_else(|| AppError::NotFound("导出任务不存在".into()))?;
            if current.status == export_job::Model::STATUS_SUCCEEDED {
                return if current.result_file_id == Some(file_id) {
                    Ok(false)
                } else {
                    Err(AppError::Conflict("导出任务结果文件标识冲突".into()))
                };
            }
            if current.status != export_job::Model::STATUS_RUNNING {
                return Err(AppError::Conflict("导出任务已不再允许运行".into()));
            }

            self.storage
                .put(EXPORT_BUCKET, &key, &bytes, &content_type)
                .await
                .map_err(storage_error)?;
            let file = self
                .files
                .insert_in_txn(&transaction, &export.tenant_id, file)
                .await?;
            let completed_at = self.background_jobs.database_utc_now(&transaction).await?;
            if !self
                .exports
                .mark_succeeded_in_transaction(
                    &transaction,
                    MarkExportJobSucceeded {
                        id: export.id,
                        file_id: file.id,
                        file_name,
                        content_type: file.content_type,
                        file_size: file.file_size,
                        request_params: export.request_params,
                        expires_at: completed_at + self.export_retention,
                        completed_at,
                    },
                )
                .await?
            {
                return Err(AppError::Conflict("导出任务状态已变化".into()));
            }
            Ok(true)
        }
        .await;

        match result {
            Ok(_) => match crate::commit_current_audit(transaction).await {
                Ok(()) => Ok(()),
                Err(error) => {
                    self.compensate_uncommitted_object(export.id, &key).await;
                    Err(error)
                }
            },
            Err(error) => {
                let _ = transaction.rollback().await;
                self.compensate_uncommitted_object(export.id, &key).await;
                Err(error)
            }
        }
    }

    /// 事务失败后只删除未被成功状态引用的确定性对象；读取失败时宁可保留孤儿对象。
    async fn compensate_uncommitted_object(&self, export_id: i64, key: &str) {
        let Ok(transaction) = self.db.write().begin().await else {
            return;
        };
        let Ok(Some(current)) = self
            .exports
            .find_by_id_for_update_in_transaction(&transaction, export_id)
            .await
        else {
            let _ = transaction.rollback().await;
            return;
        };
        if should_delete_uncommitted_object(&current.status) {
            let _ = self.storage.delete(EXPORT_BUCKET, key).await;
        }
        let _ = transaction.rollback().await;
    }
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct StoredExportRequest {
    request: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    authorization_fingerprint: Option<String>,
}

/// 用户导出的可持久化筛选条件。
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct UserExportFilters {
    pub username: Option<String>,
    pub phone: Option<String>,
    pub status: Option<String>,
    pub dept_id: Option<i64>,
}

#[derive(Deserialize)]
struct RoleExportFilters {
    name: Option<String>,
    code: Option<String>,
    status: Option<String>,
}

#[derive(Deserialize)]
struct PostExportFilters {
    name: Option<String>,
    code: Option<String>,
    status: Option<String>,
}

#[derive(Deserialize)]
struct ConfigExportFilters {
    name: Option<String>,
    key: Option<String>,
}

#[derive(Deserialize)]
struct DictTypeExportFilters {
    name: Option<String>,
    code: Option<String>,
    status: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct LogExportFilters {
    name: Option<String>,
    status: Option<String>,
    begin_time: Option<String>,
    end_time: Option<String>,
}

const ROLE_HEADERS: &[(&str, &str)] = &[
    ("role_id", "角色 ID"),
    ("role_name", "角色名称"),
    ("role_code", "角色编码"),
    ("data_scope", "数据范围"),
    ("status", "状态"),
    ("sort", "排序"),
    ("remark", "备注"),
    ("created_at", "创建时间"),
];
const POST_HEADERS: &[(&str, &str)] = &[
    ("post_id", "岗位 ID"),
    ("name", "岗位名称"),
    ("code", "岗位编码"),
    ("sort", "排序"),
    ("status", "状态"),
    ("remark", "备注"),
    ("created_at", "创建时间"),
];
const CONFIG_HEADERS: &[(&str, &str)] = &[
    ("name", "参数名称"),
    ("key", "参数键名"),
    ("value", "参数键值"),
    ("remark", "备注"),
    ("created_at", "创建时间"),
];
const DICT_TYPE_HEADERS: &[(&str, &str)] = &[
    ("name", "字典名称"),
    ("code", "字典类型"),
    ("status", "状态"),
    ("remark", "备注"),
    ("created_at", "创建时间"),
];
const OPER_LOG_HEADERS: &[(&str, &str)] = &[
    ("title", "操作模块"),
    ("business_type", "业务类型"),
    ("oper_name", "操作人员"),
    ("oper_url", "请求地址"),
    ("oper_ip", "操作 IP"),
    ("status", "状态"),
    ("cost_time", "耗时(ms)"),
    ("oper_time", "操作时间"),
];
const LOGIN_LOG_HEADERS: &[(&str, &str)] = &[
    ("user_name", "用户名"),
    ("ipaddr", "IP 地址"),
    ("login_location", "登录地点"),
    ("browser", "浏览器"),
    ("os", "操作系统"),
    ("status", "状态"),
    ("msg", "提示消息"),
    ("login_time", "登录时间"),
];

fn decode_export_filters<T: serde::de::DeserializeOwned>(
    request: Value,
    resource: &str,
) -> AppResult<T> {
    serde_json::from_value(request)
        .map_err(|error| AppError::Validation(format!("{resource} 导出筛选条件无效: {error}")))
}

#[derive(Serialize)]
struct UserExportRow {
    user_id: String,
    username: String,
    nickname: String,
    email: String,
    phone: String,
    dept_name: Option<String>,
    status: String,
    remark: Option<String>,
    created_at: String,
}

impl UserExportRow {
    const fn headers() -> &'static [(&'static str, &'static str)] {
        &[
            ("user_id", "用户 ID"),
            ("username", "用户名"),
            ("nickname", "昵称"),
            ("email", "邮箱"),
            ("phone", "手机号"),
            ("dept_name", "部门"),
            ("status", "状态"),
            ("remark", "备注"),
            ("created_at", "创建时间"),
        ]
    }
}

fn validate_request_command(command: &RequestExportCommand) -> AppResult<()> {
    for (name, value, maximum) in [
        ("resource", command.resource.as_str(), 64),
        ("permission_code", command.permission_code.as_str(), 128),
    ] {
        if value.trim().is_empty() || value.len() > maximum {
            return Err(AppError::Validation(format!(
                "导出请求 {name} 长度必须介于 1 和 {maximum} 之间"
            )));
        }
    }
    Ok(())
}

fn validate_job_id(id: i64) -> AppResult<()> {
    if id <= 0 {
        return Err(AppError::Validation("导出任务 ID 必须是正整数".into()));
    }
    Ok(())
}

fn database_error(error: sea_orm::DbErr) -> AppError {
    AppError::Database(error.to_string())
}

fn storage_error(error: ryframe_storage::StorageError) -> AppError {
    AppError::ServiceUnavailable(format!("导出对象存储操作失败: {error}"))
}

fn export_file_location(tenant_id: &str, resource: &str, export_id: i64) -> (String, String) {
    let file_name = format!("{resource}-{export_id}.xlsx");
    let key = format!("{tenant_id}/exports/{file_name}");
    (file_name, key)
}

fn deterministic_export_file_id(export_id: i64) -> i64 {
    export_id
}

fn ensure_download_authorization_matches(
    stored_fingerprint: Option<&str>,
    current_fingerprint: &str,
) -> AppResult<()> {
    if stored_fingerprint.is_some_and(|stored| stored == current_fingerprint) {
        Ok(())
    } else {
        Err(AppError::Authorization(
            "导出完成后的授权或数据范围已变化，请重新创建导出任务".into(),
        ))
    }
}

fn should_delete_uncommitted_object(status: &str) -> bool {
    status != export_job::Model::STATUS_SUCCEEDED
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{
        LogExportFilters, decode_export_filters, deterministic_export_file_id,
        ensure_download_authorization_matches, export_file_location,
        should_delete_uncommitted_object,
    };

    #[test]
    fn export_retry_uses_the_same_object_location() {
        let first = export_file_location("tenant-a", "users", 42);
        let retry = export_file_location("tenant-a", "users", 42);

        assert_eq!(first, retry);
        assert_eq!(first.0, "users-42.xlsx");
        assert_eq!(first.1, "tenant-a/exports/users-42.xlsx");
    }

    #[test]
    fn export_retry_uses_the_same_file_id_and_never_deletes_a_committed_object() {
        let file_id = deterministic_export_file_id(42);
        assert_eq!(file_id, deterministic_export_file_id(42));
        assert!(!should_delete_uncommitted_object("succeeded"));
        assert!(should_delete_uncommitted_object("running"));
    }

    #[test]
    fn download_fails_closed_after_permission_or_scope_changes() {
        assert!(ensure_download_authorization_matches(Some("same"), "same").is_ok());
        assert!(ensure_download_authorization_matches(Some("old"), "new").is_err());
        assert!(ensure_download_authorization_matches(None, "new").is_err());
    }

    #[test]
    fn log_export_filters_accept_only_the_canonical_name_field() {
        let filters = decode_export_filters::<LogExportFilters>(
            json!({ "name": "alice", "status": "1" }),
            "日志",
        )
        .unwrap();
        assert_eq!(filters.name.as_deref(), Some("alice"));
        assert_eq!(filters.status.as_deref(), Some("1"));

        for legacy in [
            json!({ "oper_name": "alice" }),
            json!({ "user_name": "alice" }),
        ] {
            let error = decode_export_filters::<LogExportFilters>(legacy, "日志").unwrap_err();
            assert!(error.to_string().contains("导出筛选条件无效"));
        }
    }
}
