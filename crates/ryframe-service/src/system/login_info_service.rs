use chrono::Utc;
use ryframe_common::AppResult;
use ryframe_core::{PageQuery, PageResult, Repository};
use ryframe_db::entities::login_info;
use ryframe_db::LoginInfoRepository;
use sea_orm::DatabaseConnection;
use serde::Serialize;
use ryframe_common::utils::snowflake;

/// 登录日志视图对象
#[derive(Debug, Clone, Serialize)]
pub struct LoginInfoVo {
    pub id: i64,
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
            id: log.id,
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

pub struct LoginInfoServiceImpl {
    pub login_info_repo: LoginInfoRepository,
}

impl LoginInfoServiceImpl {
    #[allow(clippy::too_many_arguments)]
    pub async fn record_login(
        &self,
        db: &DatabaseConnection,
        user_name: &str,
        ipaddr: &str,
        browser: Option<&str>,
        os: Option<&str>,
        status: &str,
        msg: Option<&str>,
    ) -> AppResult<()> {
        let log = login_info::Model {
            id: snowflake::next_snowflake_id(),
            user_name: user_name.to_string(),
            ipaddr: ipaddr.to_string(),
            login_location: None,
            browser: browser.map(|s| s.to_string()),
            os: os.map(|s| s.to_string()),
            status: status.to_string(),
            msg: msg.map(|s| s.to_string()),
            login_time: Utc::now(),
        };
        self.login_info_repo.insert(db, log).await?;
        Ok(())
    }

    pub async fn find_by_page(
        &self,
        db: &DatabaseConnection,
        query: PageQuery,
        user_name: Option<&str>,
        status: Option<String>,
        begin_time: Option<&str>,
        end_time: Option<&str>,
    ) -> AppResult<PageResult<LoginInfoVo>> {
        let begin = begin_time
            .and_then(|s| chrono::NaiveDateTime::parse_from_str(&format!("{} 00:00:00", s), "%Y-%m-%d %H:%M:%S").ok())
            .map(|d| d.and_utc());
        let end = end_time
            .and_then(|s| chrono::NaiveDateTime::parse_from_str(&format!("{} 23:59:59", s), "%Y-%m-%d %H:%M:%S").ok())
            .map(|d| d.and_utc());

        let result = self
            .login_info_repo
            .find_by_page_filtered(db, query, user_name, status, begin, end)
            .await?;
        Ok(PageResult {
            records: result.records.into_iter().map(LoginInfoVo::from).collect(),
            total: result.total,
            page: result.page,
            page_size: result.page_size,
        })
    }

    pub async fn clean(&self, db: &DatabaseConnection) -> AppResult<u64> {
        self.login_info_repo.clean_all(db).await
    }

    /// 导出登录日志（带过滤条件，返回全部匹配结果）
    pub async fn find_all_filtered(
        &self,
        db: &DatabaseConnection,
        user_name: Option<&str>,
        status: Option<String>,
        begin_time: Option<&str>,
        end_time: Option<&str>,
    ) -> AppResult<Vec<LoginInfoVo>> {
        let query = PageQuery { page: 1, page_size: 10000 };
        let result = self.find_by_page(db, query, user_name, status, begin_time, end_time).await?;
        Ok(result.records)
    }
}
