use chrono::Utc;
use ryframe_db::{ControlDatabaseCluster, ReadConsistency};
use ryframe_db::{LoginInfoFilter, LoginInfoRepository, Repository, entities::login_info};
use ryframe_kernel::{
    ActorContext, AppError, AppResult, ExportCursorWindow, PageResult, ValidatedPageQuery,
};
use sea_orm::TransactionTrait;
use serde::Serialize;

use super::log_time_range::parse_log_time_range;

/// 登录日志视图对象
#[derive(Debug, Clone, Serialize)]
pub struct LoginInfoVo {
    /// id 使用 String 避免 Snowflake 64 位 ID 超出 JS Number.MAX_SAFE_INTEGER
    pub id: String,
    pub user_name: String,
    pub ipaddr: String,
    pub login_location: Option<String>,
    pub browser: Option<String>,
    pub os: Option<String>,
    pub status: String,
    pub msg: Option<String>,
    pub login_time: String,
}

impl From<login_info::Model> for LoginInfoVo {
    fn from(log: login_info::Model) -> Self {
        Self {
            id: log.id.to_string(),
            user_name: log.user_name,
            ipaddr: log.ipaddr,
            login_location: log.login_location,
            browser: log.browser,
            os: log.os,
            status: log.status,
            msg: log.msg,
            login_time: log.login_time.format("%Y-%m-%d %H:%M:%S").to_string(),
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub enum LoginStatus {
    Success,
    Failure,
}

impl LoginStatus {
    fn as_str(self) -> &'static str {
        match self {
            Self::Success => login_info::Model::STATUS_SUCCESS,
            Self::Failure => login_info::Model::STATUS_FAIL,
        }
    }
}

#[derive(Debug)]
pub struct RecordLoginCommand {
    pub tenant_id: String,
    pub user_name: String,
    pub ipaddr: String,
    pub browser: Option<String>,
    pub os: Option<String>,
    pub status: LoginStatus,
    pub message: Option<String>,
}

#[derive(Debug)]
pub struct LoginInfoQuery {
    pub page: ValidatedPageQuery,
    pub user_name: Option<String>,
    pub status: Option<String>,
    pub begin_time: Option<String>,
    pub end_time: Option<String>,
}

pub struct LoginInfoService {
    db: ControlDatabaseCluster,
    login_info_repo: LoginInfoRepository,
}

impl LoginInfoService {
    pub fn new(db: ControlDatabaseCluster) -> Self {
        Self {
            db,
            login_info_repo: LoginInfoRepository,
        }
    }

    pub async fn record_login(&self, command: RecordLoginCommand) -> AppResult<()> {
        crate::enforce_tenant_scope(&command.tenant_id)?;
        let db = self.db.write();
        let tenant_id = command.tenant_id;
        let log = login_info::Model {
            id: crate::next_id()?,
            tenant_id: tenant_id.clone(),
            user_name: command.user_name,
            ipaddr: command.ipaddr,
            login_location: None,
            browser: command.browser,
            os: command.os,
            status: command.status.as_str().to_string(),
            msg: command.message,
            login_time: Utc::now(),
        };
        self.login_info_repo.insert(db, &tenant_id, log).await?;
        Ok(())
    }

    pub async fn find_by_page(
        &self,
        actor: &ActorContext,
        query: LoginInfoQuery,
    ) -> AppResult<PageResult<LoginInfoVo>> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        let scope_ctx = actor.data_scope_context();
        let db = self.db.select_read(ReadConsistency::Eventual).connection;
        let (begin_time, end_time) =
            parse_log_time_range(query.begin_time.as_deref(), query.end_time.as_deref())?;
        let filter = LoginInfoFilter {
            user_name: query.user_name.as_deref(),
            status: query.status.as_deref(),
            begin_time,
            end_time,
        };

        let result = self
            .login_info_repo
            .find_by_page_filtered(&db, tenant_id, &query.page, filter, &scope_ctx)
            .await?;
        Ok(PageResult {
            records: result.records.into_iter().map(LoginInfoVo::from).collect(),
            total: result.total,
            page: result.page,
            page_size: result.page_size,
        })
    }

    /// 按稳定主键窗口读取一批登录日志，并延续当前数据范围约束。
    pub(crate) async fn find_export_batch(
        &self,
        actor: &ActorContext,
        filter: LoginInfoFilter<'_>,
        window: ExportCursorWindow,
    ) -> AppResult<Vec<LoginInfoVo>> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        let scope_ctx = actor.data_scope_context();
        let db = self.db.select_read(ReadConsistency::Strong).connection;
        Ok(self
            .login_info_repo
            .find_for_export_after_id(&db, tenant_id, &filter, &scope_ctx, window)
            .await?
            .into_iter()
            .map(LoginInfoVo::from)
            .collect())
    }

    pub async fn clean(&self, actor: &ActorContext) -> AppResult<u64> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        let transaction = self.db.write().begin().await.map_err(database_error)?;
        let rows_affected = self
            .login_info_repo
            .clean_all_in_transaction(&transaction, tenant_id)
            .await?;
        crate::commit_current_audit(transaction).await?;
        Ok(rows_affected)
    }
}

fn database_error(error: impl std::fmt::Display) -> AppError {
    AppError::Database(error.to_string())
}
