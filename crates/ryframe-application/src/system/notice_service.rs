use std::sync::Arc;

use chrono::Utc;
use ryframe_kernel::{ActorContext, AppError, AppResult, PageResult, ValidatedPageQuery};
use serde::Serialize;

use crate::{NoticeFilter, NoticePersistencePort, NoticeRecord};

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

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use chrono::TimeZone;
    use ryframe_kernel::DataScope;

    use super::*;
    use crate::{ControlTransaction, NoticeTransaction, PersistenceFuture};

    struct FakePersistence {
        calls: Arc<Mutex<Vec<&'static str>>>,
        record: NoticeRecord,
    }

    struct FakeTransaction {
        calls: Arc<Mutex<Vec<&'static str>>>,
        record: NoticeRecord,
    }

    impl NoticePersistencePort for FakePersistence {
        fn find_by_id<'a>(
            &'a self,
            _tenant_id: &'a str,
            _id: i64,
        ) -> PersistenceFuture<'a, Option<NoticeRecord>> {
            Box::pin(async { unreachable!("本测试不读取详情") })
        }

        fn find_by_page<'a>(
            &'a self,
            _tenant_id: &'a str,
            _page: ValidatedPageQuery,
            _filter: NoticeFilter<'a>,
        ) -> PersistenceFuture<'a, PageResult<NoticeRecord>> {
            Box::pin(async { unreachable!("本测试不读取列表") })
        }

        fn begin(&self) -> PersistenceFuture<'_, Box<dyn NoticeTransaction>> {
            self.calls.lock().expect("调用记录锁应可用").push("begin");
            let transaction = FakeTransaction {
                calls: Arc::clone(&self.calls),
                record: self.record.clone(),
            };
            Box::pin(async move { Ok(Box::new(transaction) as Box<dyn NoticeTransaction>) })
        }
    }

    impl NoticeTransaction for FakeTransaction {
        fn find_by_id_for_update<'a>(
            &'a self,
            _tenant_id: &'a str,
            _id: i64,
        ) -> PersistenceFuture<'a, Option<NoticeRecord>> {
            self.calls.lock().expect("调用记录锁应可用").push("find");
            let record = self.record.clone();
            Box::pin(async move { Ok(Some(record)) })
        }

        fn insert<'a>(
            &'a self,
            _tenant_id: &'a str,
            record: NoticeRecord,
        ) -> PersistenceFuture<'a, NoticeRecord> {
            Box::pin(async move { Ok(record) })
        }

        fn update<'a>(
            &'a self,
            _tenant_id: &'a str,
            record: NoticeRecord,
        ) -> PersistenceFuture<'a, NoticeRecord> {
            self.calls.lock().expect("调用记录锁应可用").push("update");
            Box::pin(async move { Ok(record) })
        }

        fn delete<'a>(&'a self, _tenant_id: &'a str, _id: i64) -> PersistenceFuture<'a, ()> {
            Box::pin(async { Ok(()) })
        }
    }

    impl ControlTransaction for FakeTransaction {
        fn commit(self: Box<Self>) -> PersistenceFuture<'static, ()> {
            self.calls.lock().expect("调用记录锁应可用").push("commit");
            Box::pin(async { Ok(()) })
        }
    }

    #[tokio::test]
    async fn update_locks_row_inside_application_owned_transaction() {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let timestamp = Utc
            .with_ymd_and_hms(2026, 8, 20, 0, 0, 0)
            .single()
            .expect("测试时间应有效");
        let persistence = Arc::new(FakePersistence {
            calls: Arc::clone(&calls),
            record: NoticeRecord {
                id: 8,
                title: "旧通知".into(),
                content: "旧内容".into(),
                notice_type: Some("1".into()),
                status: "1".into(),
                created_by: Some(1),
                created_at: timestamp,
                updated_at: timestamp,
            },
        });
        let service = NoticeService::new(persistence);
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

        let updated = service
            .update(&actor, 8, "新通知", "新内容", Some("2"), "0".into())
            .await
            .expect("通知更新应成功");

        assert_eq!(updated.title, "新通知");
        assert_eq!(updated.content_markdown, "新内容");
        assert_eq!(updated.notice_type.as_deref(), Some("2"));
        assert_eq!(
            *calls.lock().expect("调用记录锁应可用"),
            ["begin", "find", "update", "commit"]
        );
    }
}
