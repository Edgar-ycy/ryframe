use ryframe_common::{AppError, AppResult, utils::snowflake};
use ryframe_core::{
    LoggedRepo, Repository,
    auto_fill::{AutoFill, FillContext},
    repository::{PageQuery, PageResult},
};
use ryframe_db::{NoticeRepository, entities::notice};
use sea_orm::DatabaseConnection;
use serde::Serialize;

#[derive(Debug, Serialize)]
pub struct NoticeVo {
    pub id: i64,
    pub title: String,
    pub content: String,
    pub r#type: Option<String>,
    pub status: String,
    pub created_by: Option<i64>,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

impl From<notice::Model> for NoticeVo {
    fn from(n: notice::Model) -> Self {
        Self {
            id: n.id,
            title: n.title,
            content: n.content,
            r#type: n.r#type,
            status: n.status,
            created_by: n.created_by,
            created_at: n.created_at,
        }
    }
}

pub struct NoticeServiceImpl {
    pub notice_repo: LoggedRepo<NoticeRepository>,
}

impl NoticeServiceImpl {
    pub async fn find_by_page(
        &self,
        db: &DatabaseConnection,
        query: PageQuery,
    ) -> AppResult<PageResult<NoticeVo>> {
        let page = self.notice_repo.find_by_page(db, query.clone()).await?;
        let records = page.records.into_iter().map(NoticeVo::from).collect();
        Ok(PageResult::new(records, page.total, &query))
    }

    pub async fn find_by_id(
        &self,
        db: &DatabaseConnection,
        id: i64,
    ) -> AppResult<Option<NoticeVo>> {
        Ok(self
            .notice_repo
            .find_by_id(db, id)
            .await?
            .map(NoticeVo::from))
    }

    pub async fn create(
        &self,
        db: &DatabaseConnection,
        title: &str,
        content: &str,
        notice_type: Option<&str>,
        created_by: Option<i64>,
    ) -> AppResult<NoticeVo> {
        let mut new_notice = notice::Model {
            id: snowflake::next_snowflake_id(),
            title: title.to_string(),
            content: content.to_string(),
            r#type: notice_type.map(|s| s.to_string()),
            status: notice::Model::STATUS_PUBLISHED.to_string(),
            created_by,
            del_flag: notice::Model::DEL_FLAG_NORMAL.to_string(),
            created_at: Default::default(),
            updated_at: Default::default(),
        };
        new_notice.fill_on_insert(&FillContext::new());
        Ok(NoticeVo::from(
            self.notice_repo.insert(db, new_notice).await?,
        ))
    }

    pub async fn update(
        &self,
        db: &DatabaseConnection,
        id: i64,
        title: &str,
        content: &str,
        notice_type: Option<&str>,
        status: String,
    ) -> AppResult<NoticeVo> {
        let mut n = self
            .notice_repo
            .find_by_id(db, id)
            .await?
            .ok_or_else(|| AppError::NotFound("通知公告不存在".into()))?;
        n.title = title.to_string();
        n.content = content.to_string();
        n.r#type = notice_type.map(|s| s.to_string());
        n.status = status;
        n.fill_on_update(&FillContext::new());
        Ok(NoticeVo::from(self.notice_repo.update(db, n).await?))
    }

    pub async fn delete(&self, db: &DatabaseConnection, id: i64) -> AppResult<()> {
        self.notice_repo
            .find_by_id(db, id)
            .await?
            .ok_or_else(|| AppError::NotFound("通知公告不存在".into()))?;
        self.notice_repo.delete(db, id).await
    }

    /// 带搜索条件的分页查询
    pub async fn find_by_page_filtered(
        &self,
        db: &DatabaseConnection,
        query: PageQuery,
        title: Option<&str>,
        notice_type: Option<&str>,
        status: Option<&str>,
    ) -> AppResult<PageResult<NoticeVo>> {
        let page = self
            .notice_repo
            .find_by_page_filtered(db, query.clone(), title, notice_type, status)
            .await?;
        let records = page.records.into_iter().map(NoticeVo::from).collect();
        Ok(PageResult::new(records, page.total, &query))
    }
}
