use chrono::{DateTime, Utc};
use ryframe_db::entities::export_job;
use ryframe_kernel::{ActorContext, AppError, AppResult};
use sea_orm::TransactionTrait;
use serde_json::Value;

use super::*;

impl ExportService {
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
        let data = posts
            .into_iter()
            .map(|item| {
                serde_json::json!({
                    "post_id": item.id, "name": item.name, "code": item.code, "sort": item.sort,
                    "status": item.status, "remark": item.remark, "created_at": item.created_at.to_rfc3339(),
                })
            })
            .collect::<Vec<_>>();
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
        let data = types
            .into_iter()
            .map(|item| {
                serde_json::json!({
                    "name": item.name, "code": item.code, "status": item.status, "remark": item.remark,
                    "created_at": item.created_at.to_rfc3339(),
                })
            })
            .collect::<Vec<_>>();
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
        let data = logs
            .into_iter()
            .map(|item| {
                serde_json::json!({
                    "title": item.title, "business_type": item.business_type, "oper_name": item.oper_name,
                    "oper_url": item.oper_url, "oper_ip": item.oper_ip, "status": item.status,
                    "cost_time": item.cost_time, "oper_time": item.oper_time,
                })
            })
            .collect::<Vec<_>>();
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
        let data = logs
            .into_iter()
            .map(|item| {
                serde_json::json!({
                    "user_name": item.user_name, "ipaddr": item.ipaddr, "login_location": item.login_location,
                    "browser": item.browser, "os": item.os, "status": item.status, "msg": item.msg,
                    "login_time": item.login_time,
                })
            })
            .collect::<Vec<_>>();
        let bytes =
            ryframe_excel::ExcelExporter::export_to_bytes(&data, "登录日志", LOGIN_LOG_HEADERS)?;
        self.persist_export_file(export, actor, bytes, "loginlogs", now)
            .await
    }
}
