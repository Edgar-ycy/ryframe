use std::sync::Arc;

use chrono::{DateTime, Duration, Utc};
use ryframe_core::repository::Repository;
use ryframe_db::{
    BackgroundJobRepository, CreateExportJob, DatabaseCluster, EnqueueBackgroundJob,
    ExportJobRepository, FileRepository, MarkExportJobSucceeded,
    entities::{export_job, sys_file},
};
use ryframe_kernel::{ActorContext, AppError, AppResult};
use sea_orm::TransactionTrait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::Digest;
use utoipa::ToSchema;

use super::{
    ConfigListParams, ConfigService, DictService, DictTypeListParams, LoginInfoQuery,
    LoginInfoService, OperLogQuery, OperLogService, PostListParams, PostService, RoleListParams,
    RoleService, UserListParams, UserService,
};
use ryframe_core::PageQuery;

/// Worker 消费异步导出任务的稳定类型标识。
pub const EXPORT_JOB_TYPE: &str = "system.export.execute";

/// 清理过期导出结果的稳定任务类型标识。
pub const EXPORT_CLEANUP_JOB_TYPE: &str = "system.export.cleanup";

/// 导出文件的对象存储桶名称。
pub const EXPORT_BUCKET: &str = "exports";

/// 创建公开导出任务的通用参数。
#[derive(Clone, Debug)]
pub struct RequestExportCommand {
    pub resource: String,
    pub permission_code: String,
    pub request_params: Value,
}

/// 面向 API 的导出任务安全视图，不暴露内部后台任务载荷。
#[derive(Clone, Debug, Serialize, ToSchema)]
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
}

impl ExportService {
    pub fn new(
        db: DatabaseCluster,
        users: Arc<UserService>,
        storage: Arc<dyn ryframe_storage::ObjectStorage>,
    ) -> Self {
        Self {
            db: db.clone(),
            background_jobs: BackgroundJobRepository,
            exports: ExportJobRepository,
            files: FileRepository,
            roles: RoleService::new(db.clone(), None),
            posts: PostService::new(db.clone()),
            configs: ConfigService::new(db.clone(), None),
            dicts: DictService::new(db.clone(), None),
            oper_logs: OperLogService::new(db.clone()),
            login_infos: LoginInfoService::new(db.clone()),
            users,
            storage,
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
        let transaction = self.db.write().begin().await.map_err(database_error)?;
        let result = async {
            let now = self.background_jobs.database_utc_now(&transaction).await?;
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
                        max_attempts: 3,
                        dedupe_key: None,
                        traceparent: crate::trace_context::current_traceparent(),
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
                        request_params: serde_json::json!({
                            "actor": actor,
                            "request": command.request_params,
                        }),
                        permission_code: command.permission_code,
                    },
                    now,
                )
                .await
        }
        .await;
        match result {
            Ok(export) => {
                transaction.commit().await.map_err(database_error)?;
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
        let export = self
            .exports
            .find_by_id_for_requester(self.db.write(), tenant_id, actor.user_id, id)
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
        let exports = self
            .exports
            .list_for_requester(self.db.write(), tenant_id, actor.user_id, 100)
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
        let export = self
            .exports
            .find_by_id_for_requester(self.db.write(), tenant_id, actor.user_id, id)
            .await?
            .ok_or_else(|| AppError::NotFound("导出任务不存在或不属于当前用户".into()))?;
        self.users
            .ensure_current_permission(actor, &export.permission_code)
            .await?;
        let now = self
            .background_jobs
            .database_utc_now(self.db.write())
            .await?;
        if !self
            .exports
            .cancel_for_requester(self.db.write(), tenant_id, actor.user_id, id, now)
            .await?
        {
            return Err(AppError::Conflict(
                "导出任务已完成、已过期或状态已变化".into(),
            ));
        }
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
        let export = self
            .exports
            .find_by_id_for_requester(self.db.write(), tenant_id, actor.user_id, id)
            .await?
            .ok_or_else(|| AppError::NotFound("导出任务不存在或不属于当前用户".into()))?;
        self.users
            .ensure_current_permission(actor, &export.permission_code)
            .await?;
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

    /// 清理过期导出结果。对象删除失败时保留任务为成功状态，以便后续安全重试。
    pub async fn cleanup_expired(&self) -> AppResult<u64> {
        let now = self
            .background_jobs
            .database_utc_now(self.db.write())
            .await?;
        let exports = self
            .exports
            .list_expired_succeeded(self.db.write(), now, 100)
            .await?;
        let mut cleaned = 0_u64;
        for export in exports {
            let Some(file_id) = export.result_file_id else {
                self.exports
                    .mark_expired(self.db.write(), export.id, now)
                    .await?;
                cleaned += 1;
                continue;
            };
            let Some(file) = self
                .files
                .find_by_id(self.db.write(), &export.tenant_id, file_id)
                .await?
            else {
                self.exports
                    .mark_expired(self.db.write(), export.id, now)
                    .await?;
                cleaned += 1;
                continue;
            };
            self.storage
                .delete(&file.bucket, &file.storage_path)
                .await
                .map_err(storage_error)?;
            self.files
                .delete(self.db.write(), &export.tenant_id, file.id)
                .await?;
            if self
                .exports
                .mark_expired(self.db.write(), export.id, now)
                .await?
            {
                cleaned += 1;
            }
        }
        Ok(cleaned)
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
        if !self
            .exports
            .mark_running(self.db.write(), export.id, now)
            .await?
            && export.status != export_job::Model::STATUS_RUNNING
        {
            return Ok(());
        }
        let request: StoredExportRequest = serde_json::from_value(export.request_params.clone())
            .map_err(|error| AppError::Validation(format!("导出请求快照无效: {error}")))?;
        self.users
            .ensure_current_permission(&request.actor, &export.permission_code)
            .await?;
        match export.resource.as_str() {
            "users" => {
                self.execute_user_export(export, request.actor, request.request, now)
                    .await
            }
            "roles" => {
                self.execute_role_export(export, request.actor, request.request, now)
                    .await
            }
            "posts" => {
                self.execute_post_export(export, request.actor, request.request, now)
                    .await
            }
            "configs" => {
                self.execute_config_export(export, request.actor, request.request, now)
                    .await
            }
            "dict-types" => {
                self.execute_dict_type_export(export, request.actor, request.request, now)
                    .await
            }
            "operlogs" => {
                self.execute_oper_log_export(export, request.actor, request.request, now)
                    .await
            }
            "loginlogs" => {
                self.execute_login_log_export(export, request.actor, request.request, now)
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
        if terminal {
            self.exports
                .mark_failed(self.db.write(), export.id, error_message, now)
                .await?;
        } else {
            self.exports
                .mark_queued_after_failure(self.db.write(), export.id, error_message, now)
                .await?;
        }
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
                &UserListParams {
                    page: Default::default(),
                    username: filters.username,
                    phone: filters.phone,
                    status: filters.status,
                    dept_id: filters.dept_id,
                },
                500_000,
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
            .find_by_page(
                &actor,
                RoleListParams {
                    page: export_page(),
                    name: filters.name,
                    code: filters.code,
                    status: filters.status,
                },
            )
            .await?
            .records;
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
            .find_by_page(
                &actor,
                PostListParams {
                    page: export_page(),
                    name: filters.name,
                    code: filters.code,
                    status: filters.status,
                },
            )
            .await?
            .records;
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
            .find_all(
                &actor,
                ConfigListParams {
                    page: export_page(),
                    name: filters.name,
                    key: filters.key,
                },
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
            .find_types_by_page(
                &actor,
                DictTypeListParams {
                    page: export_page(),
                    name: filters.name,
                    code: filters.code,
                    status: filters.status,
                },
            )
            .await?
            .records;
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
            .find_all(
                &actor,
                OperLogQuery {
                    page: export_page(),
                    oper_name: filters.name,
                    status: filters.status,
                    begin_time: filters.begin_time,
                    end_time: filters.end_time,
                },
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
            .find_all(
                &actor,
                LoginInfoQuery {
                    page: export_page(),
                    user_name: filters.name,
                    status: filters.status,
                    begin_time: filters.begin_time,
                    end_time: filters.end_time,
                },
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
        let file_name = format!("{resource}-{}.xlsx", export.id);
        let key = format!("{}/exports/{}", export.tenant_id, file_name);
        self.storage
            .ensure_bucket(EXPORT_BUCKET)
            .await
            .map_err(storage_error)?;
        self.storage
            .put(
                EXPORT_BUCKET,
                &key,
                &bytes,
                "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
            )
            .await
            .map_err(storage_error)?;
        let file = sys_file::Model {
            id: ryframe_utils::snowflake::try_next_snowflake_id()?,
            tenant_id: export.tenant_id.clone(),
            original_name: file_name.clone(),
            storage_name: file_name.clone(),
            storage_path: key.clone(),
            bucket: EXPORT_BUCKET.into(),
            file_url: format!("{EXPORT_BUCKET}/{key}"),
            file_size: i64::try_from(bytes.len())
                .map_err(|_| AppError::PayloadTooLarge("导出文件超过数据库大小范围".into()))?,
            content_type: "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"
                .into(),
            file_md5: Some(format!("{:x}", md5::compute(&bytes))),
            file_sha256: Some(hex::encode(sha2::Sha256::digest(&bytes))),
            upload_by: Some(actor.username),
            upload_status: sys_file::Model::UPLOAD_STATUS_READY.into(),
            reservation_token: None,
            reservation_expires_at: None,
            del_flag: sys_file::Model::DEL_FLAG_NORMAL.into(),
            created_at: now,
            updated_at: now,
        };
        let file = match self
            .files
            .insert(self.db.write(), &export.tenant_id, file)
            .await
        {
            Ok(file) => file,
            Err(error) => {
                let _ = self.storage.delete(EXPORT_BUCKET, &key).await;
                return Err(error);
            }
        };
        let completed_at = self
            .background_jobs
            .database_utc_now(self.db.write())
            .await?;
        if !self
            .exports
            .mark_succeeded(
                self.db.write(),
                MarkExportJobSucceeded {
                    id: export.id,
                    file_id: file.id,
                    file_name,
                    content_type: file.content_type,
                    file_size: file.file_size,
                    expires_at: completed_at + Duration::hours(24),
                    completed_at,
                },
            )
            .await?
        {
            let _ = self.storage.delete(EXPORT_BUCKET, &key).await;
            let _ = self
                .files
                .delete(self.db.write(), &export.tenant_id, file.id)
                .await;
        }
        Ok(())
    }
}

#[derive(Deserialize)]
struct StoredExportRequest {
    actor: ActorContext,
    request: Value,
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

#[derive(Deserialize)]
struct LogExportFilters {
    #[serde(alias = "oper_name", alias = "user_name")]
    name: Option<String>,
    status: Option<String>,
    begin_time: Option<String>,
    end_time: Option<String>,
}

const EXPORT_PAGE_SIZE: u64 = 500_000;
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

fn export_page() -> PageQuery {
    PageQuery {
        page: 1,
        page_size: EXPORT_PAGE_SIZE,
    }
}

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
