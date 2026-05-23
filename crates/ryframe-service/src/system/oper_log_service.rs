use chrono::Utc;
use ryframe_common::AppResult;
use ryframe_core::{PageQuery, PageResult, Repository};
use ryframe_db::entities::oper_log;
use ryframe_db::OperLogRepository;
use sea_orm::DatabaseConnection;
use serde::Serialize;
use ryframe_common::utils::snowflake;

/// 操作日志视图对象
#[derive(Debug, Clone, Serialize)]
pub struct OperLogVo {
    pub id: i64,
    pub title: String,
    pub business_type: String,
    pub oper_name: String,
    pub oper_url: String,
    pub oper_ip: String,
    pub status: String,
    pub cost_time: i64,
    pub oper_time: String,
}

impl From<oper_log::Model> for OperLogVo {
    fn from(log: oper_log::Model) -> Self {
        Self {
            id: log.id,
            title: log.title,
            business_type: log.business_type,
            oper_name: log.oper_name,
            oper_url: log.oper_url,
            oper_ip: log.oper_ip,
            status: log.status,
            cost_time: log.cost_time,
            oper_time: log.oper_time.format("%Y-%m-%d %H:%M:%S").to_string(),
        }
    }
}

pub struct OperLogServiceImpl {
    pub oper_log_repo: OperLogRepository,
}

impl OperLogServiceImpl {
    #[allow(clippy::too_many_arguments)]
    pub async fn record(
        &self,
        db: &DatabaseConnection,
        title: &str,
        business_type: &str,
        method: &str,
        request_method: &str,
        oper_name: &str,
        oper_url: &str,
        oper_ip: &str,
        oper_param: Option<&str>,
        json_result: Option<&str>,
        status: &str,
        error_msg: Option<&str>,
        cost_time: i64,
    ) -> AppResult<()> {
        let log = oper_log::Model {
            id: snowflake::next_snowflake_id(),
            title: title.to_string(),
            business_type: business_type.to_string(),
            method: method.to_string(),
            request_method: request_method.to_string(),
            oper_name: oper_name.to_string(),
            oper_url: oper_url.to_string(),
            oper_ip: oper_ip.to_string(),
            oper_location: None,
            oper_param: oper_param.map(|s| s.to_string()),
            json_result: json_result.map(|s| s.to_string()),
            status: status.to_string(),
            error_msg: error_msg.map(|s| s.to_string()),
            oper_time: Utc::now(),
            cost_time,
        };
        self.oper_log_repo.insert(db, log).await?;
        Ok(())
    }

    pub async fn find_by_page(
        &self,
        db: &DatabaseConnection,
        query: PageQuery,
        oper_name: Option<&str>,
        status: Option<String>,
        begin_time: Option<&str>,
        end_time: Option<&str>,
    ) -> AppResult<PageResult<OperLogVo>> {
        let begin = begin_time
            .and_then(|s| chrono::NaiveDateTime::parse_from_str(&format!("{} 00:00:00", s), "%Y-%m-%d %H:%M:%S").ok())
            .map(|d| d.and_utc());
        let end = end_time
            .and_then(|s| chrono::NaiveDateTime::parse_from_str(&format!("{} 23:59:59", s), "%Y-%m-%d %H:%M:%S").ok())
            .map(|d| d.and_utc());

        let result = self
            .oper_log_repo
            .find_by_page_filtered(db, query, oper_name, status, begin, end)
            .await?;
        Ok(PageResult {
            records: result.records.into_iter().map(OperLogVo::from).collect(),
            total: result.total,
            page: result.page,
            page_size: result.page_size,
        })
    }

    pub async fn clean(&self, db: &DatabaseConnection) -> AppResult<u64> {
        self.oper_log_repo.clean_all(db).await
    }

    /// 导出操作日志（带过滤条件，返回全部匹配结果）
    pub async fn find_all_filtered(
        &self,
        db: &DatabaseConnection,
        oper_name: Option<&str>,
        status: Option<String>,
        begin_time: Option<&str>,
        end_time: Option<&str>,
    ) -> AppResult<Vec<OperLogVo>> {
        let query = PageQuery { page: 1, page_size: 10000 };
        let result = self.find_by_page(db, query, oper_name, status, begin_time, end_time).await?;
        Ok(result.records)
    }
}
