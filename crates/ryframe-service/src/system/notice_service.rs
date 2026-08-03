use ryframe_core::{
    Repository,
    auto_fill::{AutoFill, FillContext},
    repository::{PageResult, ValidatedPageQuery},
};
use ryframe_db::{DatabaseCluster, ReadConsistency};
use ryframe_db::{NoticeFilter, NoticeRepository, entities::notice};
use ryframe_kernel::{ActorContext, AppError, AppResult};
use ryframe_utils::snowflake;
use sea_orm::TransactionTrait;
use serde::Serialize;

#[derive(Debug, Serialize)]
pub struct NoticeVo {
    /// id 使用 String 避免 Snowflake 64 位 ID 超出 JS Number.MAX_SAFE_INTEGER
    pub id: String,
    pub title: String,
    pub content_markdown: String,
    pub notice_type: Option<String>,
    pub status: String,
    pub created_by: Option<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

impl From<notice::Model> for NoticeVo {
    fn from(n: notice::Model) -> Self {
        Self {
            id: n.id.to_string(),
            title: n.title,
            content_markdown: n.content,
            notice_type: n.r#type,
            status: n.status,
            created_by: n.created_by.map(|id| id.to_string()),
            created_at: n.created_at,
        }
    }
}

#[derive(Debug)]
pub struct NoticeListParams {
    pub page: ValidatedPageQuery,
    pub title: Option<String>,
    pub notice_type: Option<String>,
    pub status: Option<String>,
}

pub struct NoticeService {
    db: DatabaseCluster,
    notice_repo: NoticeRepository,
}

impl NoticeService {
    pub fn new(db: DatabaseCluster) -> Self {
        Self {
            db,
            notice_repo: NoticeRepository,
        }
    }

    pub async fn find_by_page(
        &self,
        actor: &ActorContext,
        params: NoticeListParams,
    ) -> AppResult<PageResult<NoticeVo>> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        let scope_ctx = actor.data_scope_context();
        let db = self.db.select_read(ReadConsistency::Eventual).connection;
        let page = self
            .notice_repo
            .find_by_page_filtered(
                &db,
                tenant_id,
                &params.page,
                &NoticeFilter {
                    title: params.title.as_deref(),
                    notice_type: params.notice_type.as_deref(),
                    status: params.status.as_deref(),
                    data_scope: &scope_ctx,
                },
            )
            .await?;
        let records = page.records.into_iter().map(NoticeVo::from).collect();
        Ok(PageResult::new(records, page.total, &params.page))
    }

    pub async fn find_by_id(&self, actor: &ActorContext, id: i64) -> AppResult<Option<NoticeVo>> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        let db = self.db.select_read(ReadConsistency::Eventual).connection;
        Ok(self
            .notice_repo
            .find_by_id(&db, tenant_id, id)
            .await?
            .map(NoticeVo::from))
    }

    pub async fn create(
        &self,
        actor: &ActorContext,
        title: &str,
        content_markdown: &str,
        notice_type: Option<&str>,
    ) -> AppResult<NoticeVo> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        let db = self.db.write();
        let mut new_notice = notice::Model {
            id: snowflake::try_next_snowflake_id()?,
            tenant_id: tenant_id.to_owned(),
            title: title.to_string(),
            content: content_markdown.to_string(),
            r#type: notice_type.map(|s| s.to_string()),
            status: notice::Model::STATUS_PUBLISHED.to_string(),
            created_by: Some(actor.user_id),
            del_flag: notice::Model::DEL_FLAG_NORMAL.to_string(),
            created_at: Default::default(),
            updated_at: Default::default(),
        };
        new_notice.fill_on_insert(&FillContext::new())?;
        let transaction = db.begin().await.map_err(database_error)?;
        let saved = self
            .notice_repo
            .insert_in_transaction(&transaction, tenant_id, new_notice)
            .await?;
        crate::commit_current_audit(transaction).await?;
        Ok(NoticeVo::from(saved))
    }

    pub async fn update(
        &self,
        actor: &ActorContext,
        id: i64,
        title: &str,
        content_markdown: &str,
        notice_type: Option<&str>,
        status: String,
    ) -> AppResult<NoticeVo> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        let db = self.db.write();
        let mut n = self
            .notice_repo
            .find_by_id(db, tenant_id, id)
            .await?
            .ok_or_else(|| AppError::NotFound("通知公告不存在".into()))?;
        n.title = title.to_string();
        n.content = content_markdown.to_string();
        n.r#type = notice_type.map(|s| s.to_string());
        n.status = status;
        n.fill_on_update(&FillContext::new())?;
        let transaction = db.begin().await.map_err(database_error)?;
        let saved = self
            .notice_repo
            .update_in_transaction(&transaction, tenant_id, n)
            .await?;
        crate::commit_current_audit(transaction).await?;
        Ok(NoticeVo::from(saved))
    }

    pub async fn delete(&self, actor: &ActorContext, id: i64) -> AppResult<()> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        let db = self.db.write();
        self.notice_repo
            .find_by_id(db, tenant_id, id)
            .await?
            .ok_or_else(|| AppError::NotFound("通知公告不存在".into()))?;
        let transaction = db.begin().await.map_err(database_error)?;
        self.notice_repo
            .delete_in_transaction(&transaction, tenant_id, id)
            .await?;
        crate::commit_current_audit(transaction).await
    }
}

fn database_error(error: impl std::fmt::Display) -> AppError {
    AppError::Database(error.to_string())
}
