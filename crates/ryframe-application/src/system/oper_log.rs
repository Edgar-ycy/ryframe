use std::sync::Arc;

use chrono::Utc;
use ryframe_kernel::{ActorContext, AppResult, ExportCursorWindow, PageResult, ValidatedPageQuery};
use serde::{Deserialize, Serialize};

use crate::ports::system::{OperLogFilter, OperLogPersistencePort, OperLogRecord};

use super::log_time_range::parse_log_time_range;

/// 操作日志视图对象。
#[derive(Debug, Clone, Serialize)]
pub struct OperLogVo {
    /// ID 使用字符串，避免 64 位值超出 JavaScript 安全整数范围。
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

impl From<OperLogRecord> for OperLogVo {
    fn from(log: OperLogRecord) -> Self {
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
            error_msg: log.error_message,
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
            Self::Success => "1",
            Self::Failure => "0",
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

impl RecordOperLogCommand {
    pub(crate) fn into_record(
        self,
        event_id: Option<String>,
        request_id: Option<String>,
    ) -> AppResult<OperLogRecord> {
        Ok(OperLogRecord {
            id: crate::next_id()?,
            event_id,
            request_id,
            title: self.title,
            business_type: self.business_type,
            method: self.method,
            request_method: self.request_method,
            oper_name: self.oper_name,
            oper_url: self.oper_url,
            oper_ip: self.oper_ip,
            oper_location: None,
            oper_param: self.oper_param,
            json_result: self.json_result,
            status: self.status.as_str().to_owned(),
            error_message: self.error_msg,
            oper_time: Utc::now(),
            cost_time: self.cost_time,
        })
    }
}

pub struct OperLogService {
    persistence: Arc<dyn OperLogPersistencePort>,
}

impl OperLogService {
    pub fn new(persistence: Arc<dyn OperLogPersistencePort>) -> Self {
        Self { persistence }
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
        crate::enforce_tenant_scope(tenant_id)?;
        self.persistence
            .insert(tenant_id, command.into_record(None, None)?)
            .await
    }

    pub async fn find_by_page(
        &self,
        actor: &ActorContext,
        query: OperLogQuery,
    ) -> AppResult<PageResult<OperLogVo>> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        let data_scope = actor.data_scope_context();
        let (begin_time, end_time) =
            parse_log_time_range(query.begin_time.as_deref(), query.end_time.as_deref())?;
        let filter = OperLogFilter {
            oper_name: query.oper_name.as_deref(),
            status: query.status.as_deref(),
            begin_time,
            end_time,
        };
        let result = self
            .persistence
            .find_by_page(tenant_id, query.page, filter, &data_scope)
            .await?;
        Ok(PageResult::new(
            result.records.into_iter().map(OperLogVo::from).collect(),
            result.total,
            &query.page,
        ))
    }

    /// 按稳定主键窗口读取一批操作日志，并延续当前数据范围约束。
    pub(crate) async fn find_export_batch(
        &self,
        actor: &ActorContext,
        filter: OperLogFilter<'_>,
        window: ExportCursorWindow,
    ) -> AppResult<Vec<OperLogVo>> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        let data_scope = actor.data_scope_context();
        Ok(self
            .persistence
            .find_export_batch(tenant_id, filter, &data_scope, window)
            .await?
            .into_iter()
            .map(OperLogVo::from)
            .collect())
    }

    pub async fn clean(&self, actor: &ActorContext) -> AppResult<u64> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        let transaction = self.persistence.begin().await?;
        let rows_affected = transaction.clean(tenant_id).await?;
        transaction.commit().await?;
        Ok(rows_affected)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use ryframe_kernel::DataScope;

    use super::*;
    use crate::{ControlTransaction, PersistenceFuture, ports::system::OperLogTransaction};

    struct FakePersistence {
        calls: Arc<Mutex<Vec<&'static str>>>,
    }

    struct FakeTransaction {
        calls: Arc<Mutex<Vec<&'static str>>>,
    }

    impl OperLogPersistencePort for FakePersistence {
        fn insert<'a>(
            &'a self,
            _tenant_id: &'a str,
            _record: OperLogRecord,
        ) -> PersistenceFuture<'a, ()> {
            Box::pin(async { unreachable!("本测试不写入日志") })
        }

        fn find_by_page<'a>(
            &'a self,
            _tenant_id: &'a str,
            _page: ValidatedPageQuery,
            _filter: OperLogFilter<'a>,
            _data_scope: &'a ryframe_kernel::DataScopeContext,
        ) -> PersistenceFuture<'a, PageResult<OperLogRecord>> {
            Box::pin(async { unreachable!("本测试不读取列表") })
        }

        fn find_export_batch<'a>(
            &'a self,
            _tenant_id: &'a str,
            _filter: OperLogFilter<'a>,
            _data_scope: &'a ryframe_kernel::DataScopeContext,
            _window: ExportCursorWindow,
        ) -> PersistenceFuture<'a, Vec<OperLogRecord>> {
            Box::pin(async { unreachable!("本测试不执行导出") })
        }

        fn begin(&self) -> PersistenceFuture<'_, Box<dyn OperLogTransaction>> {
            self.calls.lock().expect("调用记录锁应可用").push("begin");
            let transaction = FakeTransaction {
                calls: Arc::clone(&self.calls),
            };
            Box::pin(async move { Ok(Box::new(transaction) as Box<dyn OperLogTransaction>) })
        }
    }

    impl OperLogTransaction for FakeTransaction {
        fn clean<'a>(&'a self, _tenant_id: &'a str) -> PersistenceFuture<'a, u64> {
            self.calls.lock().expect("调用记录锁应可用").push("clean");
            Box::pin(async { Ok(4) })
        }
    }

    impl ControlTransaction for FakeTransaction {
        fn commit(self: Box<Self>) -> PersistenceFuture<'static, ()> {
            self.calls.lock().expect("调用记录锁应可用").push("commit");
            Box::pin(async { Ok(()) })
        }
    }

    #[tokio::test]
    async fn clean_is_committed_by_application_use_case() {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let service = OperLogService::new(Arc::new(FakePersistence {
            calls: Arc::clone(&calls),
        }));
        let actor = ActorContext {
            user_id: 1,
            tenant_id: "tenant-a".into(),
            username: "tester".into(),
            dept_id: None,
            dept_path: None,
            data_scope: DataScope::SelfOnly,
            custom_dept_ids: Vec::new(),
            include_self: true,
            is_super_admin: false,
        };

        assert_eq!(service.clean(&actor).await.expect("清理应成功"), 4);
        assert_eq!(
            *calls.lock().expect("调用记录锁应可用"),
            ["begin", "clean", "commit"]
        );
    }

    #[test]
    fn operation_status_keeps_persisted_codes() {
        assert_eq!(OperLogStatus::Success.as_str(), "1");
        assert_eq!(OperLogStatus::Failure.as_str(), "0");
    }
}
