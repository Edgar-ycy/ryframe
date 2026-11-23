use std::sync::Arc;

use ryframe_kernel::{
    ActorContext, AppError, AppResult, ExportCursorWindow, PageResult, ValidatedPageQuery,
};
use serde::Serialize;
use time::{OffsetDateTime, UtcOffset, macros::format_description};

use crate::ports::system::{LoginInfoFilter, LoginInfoPersistencePort, LoginInfoRecord};
use crate::{TransactionAuditMode, complete_transaction};

use super::log_time_range::parse_log_time_range;

/// 登录日志视图对象。
#[derive(Debug, Clone, Serialize)]
pub struct LoginInfoVo {
    /// ID 使用字符串，避免 64 位值超出 JavaScript 安全整数范围。
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

impl TryFrom<LoginInfoRecord> for LoginInfoVo {
    type Error = AppError;

    fn try_from(log: LoginInfoRecord) -> AppResult<Self> {
        let login_time = log
            .login_time
            .checked_to_offset(UtcOffset::UTC)
            .ok_or_else(|| AppError::Internal("登录时间超出 UTC 表示范围".into()))?
            .format(format_description!(
                "[year]-[month]-[day] [hour]:[minute]:[second]"
            ))
            .map_err(|error| AppError::Internal(format!("格式化登录时间失败：{error}")))?;
        Ok(Self {
            id: log.id.to_string(),
            user_name: log.user_name,
            ipaddr: log.ipaddr,
            login_location: log.login_location,
            browser: log.browser,
            os: log.os,
            status: log.status,
            msg: log.message,
            login_time,
        })
    }
}

#[derive(Debug, Clone, Copy)]
pub enum LoginStatus {
    Success,
    Failure,
}

impl LoginStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Success => "1",
            Self::Failure => "0",
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
    persistence: Arc<dyn LoginInfoPersistencePort>,
}

impl LoginInfoService {
    pub fn new(persistence: Arc<dyn LoginInfoPersistencePort>) -> Self {
        Self { persistence }
    }

    pub async fn record_login(&self, command: RecordLoginCommand) -> AppResult<()> {
        crate::enforce_tenant_scope(&command.tenant_id)?;
        let tenant_id = command.tenant_id;
        let record = LoginInfoRecord {
            id: crate::next_id()?,
            user_name: command.user_name,
            ipaddr: command.ipaddr,
            login_location: None,
            browser: command.browser,
            os: command.os,
            status: command.status.as_str().to_owned(),
            message: command.message,
            login_time: OffsetDateTime::now_utc(),
        };
        self.persistence.insert(&tenant_id, record).await
    }

    pub async fn find_by_page(
        &self,
        actor: &ActorContext,
        query: LoginInfoQuery,
    ) -> AppResult<PageResult<LoginInfoVo>> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        let data_scope = actor.data_scope_context();
        let (begin_time, end_time) =
            parse_log_time_range(query.begin_time.as_deref(), query.end_time.as_deref())?;
        let filter = LoginInfoFilter {
            user_name: query.user_name.as_deref(),
            status: query.status.as_deref(),
            begin_time,
            end_time,
        };
        let result = self
            .persistence
            .find_by_page(tenant_id, query.page, filter, &data_scope)
            .await?;
        let records = result
            .records
            .into_iter()
            .map(LoginInfoVo::try_from)
            .collect::<AppResult<Vec<_>>>()?;
        Ok(PageResult::new(records, result.total, &query.page))
    }

    /// 按稳定主键窗口读取一批登录日志，并延续当前数据范围约束。
    pub(crate) async fn find_export_batch(
        &self,
        actor: &ActorContext,
        filter: LoginInfoFilter<'_>,
        window: ExportCursorWindow<'_>,
    ) -> AppResult<Vec<LoginInfoVo>> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        let data_scope = actor.data_scope_context();
        self.persistence
            .find_export_batch(tenant_id, filter, &data_scope, window)
            .await?
            .into_iter()
            .map(LoginInfoVo::try_from)
            .collect()
    }

    pub async fn clean(&self, actor: &ActorContext) -> AppResult<u64> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        let transaction = self.persistence.begin().await?;
        let operation = transaction.clean(tenant_id).await;
        complete_transaction(transaction, operation, TransactionAuditMode::CurrentRequest).await
    }
}
