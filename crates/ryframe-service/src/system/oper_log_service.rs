use chrono::Utc;
use ryframe_common::{ActorContext, AppResult, utils::snowflake};
use ryframe_core::{LoggedRepo, PageQuery, PageResult, Repository};
use ryframe_db::DatabaseCluster;
use ryframe_db::{OperLogFilter, OperLogRepository, entities::oper_log};
use serde::Serialize;
use utoipa::ToSchema;

use super::log_time_range::parse_log_time_range;

/// 操作日志视图对象
#[derive(Debug, Clone, Serialize, ToSchema)]
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
    pub page: PageQuery,
    pub oper_name: Option<String>,
    pub status: Option<String>,
    pub begin_time: Option<String>,
    pub end_time: Option<String>,
}

#[derive(Debug, Clone, Copy)]
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

#[derive(Debug)]
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
    db: DatabaseCluster,
    oper_log_repo: LoggedRepo<OperLogRepository>,
}

impl OperLogService {
    pub fn new(db: DatabaseCluster) -> Self {
        Self {
            db,
            oper_log_repo: LoggedRepo::new(OperLogRepository),
        }
    }

    pub async fn record(
        &self,
        actor: &ActorContext,
        command: RecordOperLogCommand,
    ) -> AppResult<()> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        let log = oper_log::Model {
            id: snowflake::next_snowflake_id(),
            tenant_id: tenant_id.to_owned(),
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

    pub async fn find_by_page(
        &self,
        actor: &ActorContext,
        query: OperLogQuery,
    ) -> AppResult<PageResult<OperLogVo>> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        let scope_ctx = actor.data_scope_context();
        let db = self.db.read();
        let (begin_time, end_time) =
            parse_log_time_range(query.begin_time.as_deref(), query.end_time.as_deref());
        let filter = OperLogFilter {
            oper_name: query.oper_name.as_deref(),
            status: query.status.as_deref(),
            begin_time,
            end_time,
        };

        let result = self
            .oper_log_repo
            .find_by_page_filtered(db, tenant_id, &query.page, filter, &scope_ctx)
            .await?;
        Ok(PageResult {
            records: result.records.into_iter().map(OperLogVo::from).collect(),
            total: result.total,
            page: result.page,
            page_size: result.page_size,
        })
    }

    pub async fn clean(&self, actor: &ActorContext) -> AppResult<u64> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        let db = self.db.write();
        self.oper_log_repo.clean_all(db, tenant_id).await
    }

    /// 导出操作日志（带过滤条件，返回全部匹配结果）
    pub async fn find_all(
        &self,
        actor: &ActorContext,
        mut query: OperLogQuery,
    ) -> AppResult<Vec<OperLogVo>> {
        query.page = PageQuery::all_records();
        let result = self.find_by_page(actor, query).await?;
        Ok(result.records)
    }
}
