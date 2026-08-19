use chrono::{DateTime, Utc};
use ryframe_db::{LoginInfoFilter, OperLogFilter, UserFilter, entities::export_job};
use ryframe_kernel::{ActorContext, AppError, AppResult};
use sea_orm::TransactionTrait;

use super::*;

impl ExportService {
    /// 执行一个已领取的后台导出任务。
    pub async fn execute_background_job(
        &self,
        background_job_id: i64,
        payload: &ExportJobPayload,
    ) -> AppResult<()> {
        payload.validate()?;
        let now = self
            .background_jobs
            .database_utc_now(self.db.write())
            .await?;
        let export = self
            .exports
            .find_by_background_job_id(self.db.write(), background_job_id)
            .await?
            .ok_or_else(|| AppError::NotFound("后台任务未关联导出请求".into()))?;
        if export.resource != payload.resource() {
            return Err(AppError::Validation(
                "导出后台任务资源与公开任务资源不一致".into(),
            ));
        }
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
        let request: StoredExportRequest = serde_json::from_value(export.request_params.clone())
            .map_err(|error| AppError::Validation(format!("导出请求快照无效: {error}")))?;
        request.validate(&export.resource)?;
        request.validate_persisted_snapshot(&export)?;
        let (actor, authorization_fingerprint) = self
            .users
            .resolve_current_export_authorization(
                &export.tenant_id,
                export.requester_id,
                &export.permission_code,
            )
            .await?;
        ensure_download_authorization_matches(
            &request.authorization_fingerprint,
            &authorization_fingerprint,
        )?;
        let upper_id = request.upper_id;
        let selection = request.selection;
        match selection {
            ExportSelection::Users(filter) => {
                self.execute_user_export(export, actor, filter, upper_id, now)
                    .await
            }
            ExportSelection::Roles(filter) => {
                self.execute_role_export(export, actor, filter, upper_id, now)
                    .await
            }
            ExportSelection::Posts(filter) => {
                self.execute_post_export(export, actor, filter, upper_id, now)
                    .await
            }
            ExportSelection::Configs(filter) => {
                self.execute_config_export(export, actor, filter, upper_id, now)
                    .await
            }
            ExportSelection::DictTypes(filter) => {
                self.execute_dict_type_export(export, actor, filter, upper_id, now)
                    .await
            }
            ExportSelection::OperLogs(filter) => {
                self.execute_oper_log_export(export, actor, filter, upper_id, now)
                    .await
            }
            ExportSelection::LoginLogs(filter) => {
                self.execute_login_log_export(export, actor, filter, upper_id, now)
                    .await
            }
        }
    }

    async fn execute_user_export(
        &self,
        export: export_job::Model,
        actor: ActorContext,
        filters: UserExportFilter,
        upper_id: i64,
        now: DateTime<Utc>,
    ) -> AppResult<()> {
        let users = self
            .users
            .find_for_export(
                &actor,
                UserFilter {
                    username: filters.username(),
                    phone: filters.phone(),
                    status: filters.status(),
                    dept_id: filters.dept_id(),
                },
                upper_id,
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
        filters: RoleExportFilter,
        upper_id: i64,
        now: DateTime<Utc>,
    ) -> AppResult<()> {
        let roles = self
            .roles
            .find_for_export(
                &actor,
                filters.name(),
                filters.code(),
                filters.status(),
                upper_id,
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
        filters: PostExportFilter,
        upper_id: i64,
        now: DateTime<Utc>,
    ) -> AppResult<()> {
        let posts = self
            .posts
            .find_for_export(
                &actor,
                filters.name(),
                filters.code(),
                filters.status(),
                upper_id,
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
        filters: ConfigExportFilter,
        upper_id: i64,
        now: DateTime<Utc>,
    ) -> AppResult<()> {
        let configs = self
            .configs
            .find_for_export(
                &actor,
                filters.name(),
                filters.key(),
                upper_id,
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
        filters: DictTypeExportFilter,
        upper_id: i64,
        now: DateTime<Utc>,
    ) -> AppResult<()> {
        let types = self
            .dicts
            .find_types_for_export(
                &actor,
                filters.name(),
                filters.code(),
                filters.status(),
                upper_id,
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
        filters: OperLogExportFilter,
        upper_id: i64,
        now: DateTime<Utc>,
    ) -> AppResult<()> {
        let logs = self
            .oper_logs
            .find_for_export(
                &actor,
                OperLogFilter {
                    oper_name: filters.oper_name(),
                    status: filters.status(),
                    begin_time: filters.begin_time(),
                    end_time: filters.end_time(),
                },
                upper_id,
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
        filters: LoginLogExportFilter,
        upper_id: i64,
        now: DateTime<Utc>,
    ) -> AppResult<()> {
        let logs = self
            .login_infos
            .find_for_export(
                &actor,
                LoginInfoFilter {
                    user_name: filters.user_name(),
                    status: filters.status(),
                    begin_time: filters.begin_time(),
                    end_time: filters.end_time(),
                },
                upper_id,
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
