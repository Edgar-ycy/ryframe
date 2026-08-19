use chrono::Utc;
use ryframe_adapters::snowflake;
use ryframe_adapters::{PageResult, Repository, ValidatedPageQuery};
use ryframe_db::{ControlDatabaseCluster, ReadConsistency};
use ryframe_db::{ExportCursorWindow, OperLogFilter, OperLogRepository, entities::oper_log};
use ryframe_kernel::{ActorContext, AppError, AppResult};
use sea_orm::{DatabaseTransaction, TransactionTrait};
use serde::{Deserialize, Serialize};

use super::log_time_range::parse_log_time_range;

/// 操作日志视图对象
#[derive(Debug, Clone, Serialize)]
pub struct OperLogVo {
    /// id 使用 String 避免 Snowflake 64 位 ID 超出 JS Number.MAX_SAFE_INTEGER
    pub id: String,
    pub title: String,
    pub business_type: String,
    pub method: String,
    pub request_method: String,
    pub oper_name: String,
    pub oper_url: String,
    pub oper_ip: String,
    pub oper_location: Option<String>,
    pub oper_param: Option<String>,
    pub json_result: Option<String>,
    pub status: String,
    pub error_msg: Option<String>,
    pub cost_time: i64,
    pub oper_time: String,
}

impl From<oper_log::Model> for OperLogVo {
    fn from(log: oper_log::Model) -> Self {
        Self {
            id: log.id.to_string(),
            title: log.title,
            business_type: log.business_type,
            method: log.method,
            request_method: log.request_method,
            oper_name: log.oper_name,
            oper_url: log.oper_url,
            oper_ip: log.oper_ip,
            oper_location: log.oper_location,
            oper_param: log.oper_param,
            json_result: log.json_result,
            status: log.status,
            error_msg: log.error_msg,
            cost_time: log.cost_time,
            oper_time: log.oper_time.format("%Y-%m-%d %H:%M:%S").to_string(),
        }
    }
}

#[derive(Debug)]
pub struct OperLogQuery {
    pub page: ValidatedPageQuery,
    pub oper_name: Option<String>,
    pub status: Option<String>,
    pub begin_time: Option<String>,
    pub end_time: Option<String>,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum OperLogStatus {
    Success,
    Failure,
}

impl OperLogStatus {
    fn as_str(self) -> &'static str {
        match self {
            Self::Success => oper_log::Model::STATUS_SUCCESS,
            Self::Failure => oper_log::Model::STATUS_FAIL,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct RecordOperLogCommand {
    pub title: String,
    pub business_type: String,
    pub method: String,
    pub request_method: String,
    pub oper_name: String,
    pub oper_url: String,
    pub oper_ip: String,
    pub oper_param: Option<String>,
    pub json_result: Option<String>,
    pub status: OperLogStatus,
    pub error_msg: Option<String>,
    pub cost_time: i64,
}

pub struct OperLogService {
    db: ControlDatabaseCluster,
    oper_log_repo: OperLogRepository,
}

impl OperLogService {
    pub fn new(db: ControlDatabaseCluster) -> Self {
        Self {
            db,
            oper_log_repo: OperLogRepository,
        }
    }

    pub async fn record(
        &self,
        actor: &ActorContext,
        command: RecordOperLogCommand,
    ) -> AppResult<()> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        self.record_for_tenant(tenant_id, command).await
    }

    /// 由后台任务使用租户标识写入操作日志，不依赖请求上下文。
    pub async fn record_for_tenant(
        &self,
        tenant_id: &str,
        command: RecordOperLogCommand,
    ) -> AppResult<()> {
        ryframe_adapters::validate_explicit_tenant(tenant_id)?;
        let log = oper_log::Model {
            id: snowflake::try_next_snowflake_id()?,
            tenant_id: tenant_id.to_owned(),
            event_id: None,
            request_id: None,
            title: command.title,
            business_type: command.business_type,
            method: command.method,
            request_method: command.request_method,
            oper_name: command.oper_name,
            oper_url: command.oper_url,
            oper_ip: command.oper_ip,
            oper_location: None,
            oper_param: command.oper_param,
            json_result: command.json_result,
            status: command.status.as_str().to_string(),
            error_msg: command.error_msg,
            oper_time: Utc::now(),
            cost_time: command.cost_time,
        };
        self.oper_log_repo
            .insert(self.db.write(), tenant_id, log)
            .await?;
        Ok(())
    }

    /// 在 Outbox 消费事务内按审计事件标识幂等写入操作日志。
    pub async fn record_event_in_transaction(
        &self,
        transaction: &DatabaseTransaction,
        event_id: &str,
        request_id: &str,
        tenant_id: &str,
        command: RecordOperLogCommand,
    ) -> AppResult<bool> {
        ryframe_adapters::validate_explicit_tenant(tenant_id)?;
        if event_id.is_empty() || event_id.len() > 36 {
            return Err(AppError::Validation(
                "审计事件标识长度必须介于 1 和 36 之间".into(),
            ));
        }
        if request_id.is_empty() || request_id.len() > 36 {
            return Err(AppError::Validation(
                "请求标识长度必须介于 1 和 36 之间".into(),
            ));
        }
        let log = oper_log::Model {
            id: snowflake::try_next_snowflake_id()?,
            tenant_id: tenant_id.to_owned(),
            event_id: Some(event_id.to_owned()),
            request_id: Some(request_id.to_owned()),
            title: command.title,
            business_type: command.business_type,
            method: command.method,
            request_method: command.request_method,
            oper_name: command.oper_name,
            oper_url: command.oper_url,
            oper_ip: command.oper_ip,
            oper_location: None,
            oper_param: command.oper_param,
            json_result: command.json_result,
            status: command.status.as_str().to_string(),
            error_msg: command.error_msg,
            oper_time: Utc::now(),
            cost_time: command.cost_time,
        };
        self.oper_log_repo
            .insert_event_in_transaction(transaction, tenant_id, log)
            .await
    }

    pub async fn find_by_page(
        &self,
        actor: &ActorContext,
        query: OperLogQuery,
    ) -> AppResult<PageResult<OperLogVo>> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        let scope_ctx = actor.data_scope_context();
        let db = self.db.select_read(ReadConsistency::Eventual).connection;
        let (begin_time, end_time) =
            parse_log_time_range(query.begin_time.as_deref(), query.end_time.as_deref())?;
        let filter = OperLogFilter {
            oper_name: query.oper_name.as_deref(),
            status: query.status.as_deref(),
            begin_time,
            end_time,
        };

        let result = self
            .oper_log_repo
            .find_by_page_filtered(&db, tenant_id, &query.page, filter, &scope_ctx)
            .await?;
        Ok(PageResult {
            records: result.records.into_iter().map(OperLogVo::from).collect(),
            total: result.total,
            page: result.page,
            page_size: result.page_size,
        })
    }

    /// 按稳定主键窗口读取一批操作日志，并延续当前数据范围约束。
    pub(crate) async fn find_export_batch(
        &self,
        actor: &ActorContext,
        filter: OperLogFilter<'_>,
        window: ExportCursorWindow,
    ) -> AppResult<Vec<OperLogVo>> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        let scope_ctx = actor.data_scope_context();
        let db = self.db.select_read(ReadConsistency::Strong).connection;
        Ok(self
            .oper_log_repo
            .find_for_export_after_id(&db, tenant_id, &filter, &scope_ctx, window)
            .await?
            .into_iter()
            .map(OperLogVo::from)
            .collect())
    }

    pub async fn clean(&self, actor: &ActorContext) -> AppResult<u64> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        let transaction = self.db.write().begin().await.map_err(database_error)?;
        let rows_affected = self
            .oper_log_repo
            .clean_all_in_transaction(&transaction, tenant_id)
            .await?;
        crate::commit_current_audit(transaction).await?;
        Ok(rows_affected)
    }
}

fn database_error(error: impl std::fmt::Display) -> AppError {
    AppError::Database(error.to_string())
}
