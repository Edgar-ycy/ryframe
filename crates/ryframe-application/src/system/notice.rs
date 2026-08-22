use std::sync::Arc;

use chrono::Utc;
use ryframe_kernel::{ActorContext, AppError, AppResult, PageResult, ValidatedPageQuery};
use serde::Serialize;

use crate::ports::system::{NoticeFilter, NoticePersistencePort, NoticeRecord};

#[derive(Debug, Serialize)]
pub struct NoticeVo {
    /// ID 使用字符串，避免 64 位值超出 JavaScript 安全整数范围。
    pub id: String,
    pub title: String,
    pub content_markdown: String,
    pub notice_type: Option<String>,
    pub status: String,
    pub created_by: Option<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

impl From<NoticeRecord> for NoticeVo {
    fn from(notice: NoticeRecord) -> Self {
        Self {
            id: notice.id.to_string(),
            title: notice.title,
            content_markdown: notice.content,
            notice_type: notice.notice_type,
            status: notice.status,
            created_by: notice.created_by.map(|id| id.to_string()),
            created_at: notice.created_at,
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
    persistence: Arc<dyn NoticePersistencePort>,
}

impl NoticeService {
    pub fn new(persistence: Arc<dyn NoticePersistencePort>) -> Self {
        Self { persistence }
    }

    pub async fn find_by_page(
        &self,
        actor: &ActorContext,
        params: NoticeListParams,
    ) -> AppResult<PageResult<NoticeVo>> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        let data_scope = actor.data_scope_context();
        let filter = NoticeFilter {
            title: params.title.as_deref(),
            notice_type: params.notice_type.as_deref(),
            status: params.status.as_deref(),
            data_scope: &data_scope,
        };
        let page = self
            .persistence
            .find_by_page(tenant_id, params.page, filter)
            .await?;
        Ok(PageResult::new(
            page.records.into_iter().map(NoticeVo::from).collect(),
            page.total,
            &params.page,
        ))
    }

    pub async fn find_by_id(&self, actor: &ActorContext, id: i64) -> AppResult<Option<NoticeVo>> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        Ok(self
            .persistence
            .find_by_id(tenant_id, id)
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
        let now = Utc::now();
        let record = NoticeRecord {
            id: crate::next_id()?,
            title: title.to_owned(),
            content: content_markdown.to_owned(),
            notice_type: notice_type.map(str::to_owned),
            status: "1".into(),
            created_by: Some(actor.user_id),
            created_at: now,
            updated_at: now,
        };
        let transaction = self.persistence.begin().await?;
        let saved = transaction.insert(tenant_id, record).await?;
        transaction.commit().await?;
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
        let transaction = self.persistence.begin().await?;
        let mut notice = transaction
            .find_by_id_for_update(tenant_id, id)
            .await?
            .ok_or_else(|| AppError::NotFound("通知公告不存在".into()))?;
        notice.title = title.to_owned();
        notice.content = content_markdown.to_owned();
        notice.notice_type = notice_type.map(str::to_owned);
        notice.status = status;
        notice.updated_at = Utc::now();
        let saved = transaction.update(tenant_id, notice).await?;
        transaction.commit().await?;
        Ok(NoticeVo::from(saved))
    }

    pub async fn delete(&self, actor: &ActorContext, id: i64) -> AppResult<()> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        let transaction = self.persistence.begin().await?;
        transaction
            .find_by_id_for_update(tenant_id, id)
            .await?
            .ok_or_else(|| AppError::NotFound("通知公告不存在".into()))?;
        transaction.delete(tenant_id, id).await?;
        transaction.commit().await
    }
}
