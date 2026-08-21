use std::sync::Arc;

use chrono::Utc;
use ryframe_kernel::{
    ActorContext, AppError, AppResult, ExportCursorWindow, PageResult, ValidatedPageQuery,
};
use serde::Serialize;

use crate::ports::system::{PostFilter, PostPersistencePort, PostRecord};

#[derive(Debug, Serialize)]
pub struct PostVo {
    /// ID 使用字符串，避免 64 位值超出 JavaScript 安全整数范围。
    pub id: String,
    pub name: String,
    pub code: String,
    pub sort: i32,
    pub status: String,
    pub remark: Option<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

impl From<PostRecord> for PostVo {
    fn from(post: PostRecord) -> Self {
        Self {
            id: post.id.to_string(),
            name: post.name,
            code: post.code,
            sort: post.sort,
            status: post.status,
            remark: post.remark,
            created_at: post.created_at,
        }
    }
}

#[derive(Debug)]
pub struct PostListParams {
    pub page: ValidatedPageQuery,
    pub name: Option<String>,
    pub code: Option<String>,
    pub status: Option<String>,
}

pub struct PostService {
    persistence: Arc<dyn PostPersistencePort>,
}

impl PostService {
    pub fn new(persistence: Arc<dyn PostPersistencePort>) -> Self {
        Self { persistence }
    }

    pub async fn find_by_id(&self, actor: &ActorContext, id: i64) -> AppResult<Option<PostVo>> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        Ok(self
            .persistence
            .find_by_id(tenant_id, id)
            .await?
            .map(PostVo::from))
    }

    pub async fn create(
        &self,
        actor: &ActorContext,
        name: &str,
        code: &str,
        sort: i32,
    ) -> AppResult<PostVo> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        let now = Utc::now();
        let record = PostRecord {
            id: crate::next_id()?,
            name: name.to_owned(),
            code: code.to_owned(),
            sort,
            status: "1".into(),
            remark: None,
            created_at: now,
            updated_at: now,
        };
        let transaction = self.persistence.begin().await?;
        transaction.lock_configuration(tenant_id).await?;
        if transaction
            .find_by_code_for_update(tenant_id, code)
            .await?
            .is_some()
        {
            return Err(AppError::Conflict("岗位编码已存在".into()));
        }
        let saved = transaction.insert(tenant_id, record).await?;
        transaction
            .increment_configuration_version(tenant_id)
            .await?;
        transaction.commit().await?;
        Ok(PostVo::from(saved))
    }

    pub async fn update(
        &self,
        actor: &ActorContext,
        id: i64,
        name: &str,
        sort: i32,
        status: String,
    ) -> AppResult<PostVo> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        let transaction = self.persistence.begin().await?;
        transaction.lock_configuration(tenant_id).await?;
        let mut post = transaction
            .find_by_id_for_update(tenant_id, id)
            .await?
            .ok_or_else(|| AppError::NotFound("岗位不存在".into()))?;
        post.name = name.to_owned();
        post.sort = sort;
        post.status = status;
        post.updated_at = Utc::now();
        let saved = transaction.update(tenant_id, post).await?;
        transaction
            .increment_configuration_version(tenant_id)
            .await?;
        transaction.commit().await?;
        Ok(PostVo::from(saved))
    }

    pub async fn delete(&self, actor: &ActorContext, id: i64) -> AppResult<()> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        let transaction = self.persistence.begin().await?;
        transaction.lock_configuration(tenant_id).await?;
        transaction
            .find_by_id_for_update(tenant_id, id)
            .await?
            .ok_or_else(|| AppError::NotFound("岗位不存在".into()))?;
        transaction.delete(tenant_id, id).await?;
        transaction
            .increment_configuration_version(tenant_id)
            .await?;
        transaction.commit().await
    }

    pub async fn find_by_page(
        &self,
        actor: &ActorContext,
        params: PostListParams,
    ) -> AppResult<PageResult<PostVo>> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        let filter = PostFilter {
            name: params.name.as_deref(),
            code: params.code.as_deref(),
            status: params.status.as_deref(),
        };
        let page = self
            .persistence
            .find_by_page(tenant_id, params.page, filter)
            .await?;
        Ok(PageResult::new(
            page.records.into_iter().map(PostVo::from).collect(),
            page.total,
            &params.page,
        ))
    }

    /// 按稳定主键窗口读取一批岗位导出数据。
    pub(crate) async fn find_export_batch(
        &self,
        actor: &ActorContext,
        name: Option<&str>,
        code: Option<&str>,
        status: Option<&str>,
        window: ExportCursorWindow,
    ) -> AppResult<Vec<PostVo>> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        Ok(self
            .persistence
            .find_export_batch(tenant_id, PostFilter { name, code, status }, window)
            .await?
            .into_iter()
            .map(PostVo::from)
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use chrono::TimeZone;
    use ryframe_kernel::DataScope;

    use super::*;
    use crate::{ControlTransaction, PersistenceFuture, ports::system::PostTransaction};

    struct FakePersistence {
        calls: Arc<Mutex<Vec<&'static str>>>,
        record: PostRecord,
    }

    struct FakeTransaction {
        calls: Arc<Mutex<Vec<&'static str>>>,
        record: PostRecord,
    }

    impl PostPersistencePort for FakePersistence {
        fn find_by_id<'a>(
            &'a self,
            _tenant_id: &'a str,
            _id: i64,
        ) -> PersistenceFuture<'a, Option<PostRecord>> {
            Box::pin(async { unreachable!("本测试不读取详情") })
        }

        fn find_by_page<'a>(
            &'a self,
            _tenant_id: &'a str,
            _page: ValidatedPageQuery,
            _filter: PostFilter<'a>,
        ) -> PersistenceFuture<'a, PageResult<PostRecord>> {
            Box::pin(async { unreachable!("本测试不读取列表") })
        }

        fn find_export_batch<'a>(
            &'a self,
            _tenant_id: &'a str,
            _filter: PostFilter<'a>,
            _window: ExportCursorWindow,
        ) -> PersistenceFuture<'a, Vec<PostRecord>> {
            Box::pin(async { unreachable!("本测试不执行导出") })
        }

        fn begin(&self) -> PersistenceFuture<'_, Box<dyn PostTransaction>> {
            self.calls.lock().expect("调用记录锁应可用").push("begin");
            let transaction = FakeTransaction {
                calls: Arc::clone(&self.calls),
                record: self.record.clone(),
            };
            Box::pin(async move { Ok(Box::new(transaction) as Box<dyn PostTransaction>) })
        }
    }

    impl PostTransaction for FakeTransaction {
        fn lock_configuration<'a>(&'a self, _tenant_id: &'a str) -> PersistenceFuture<'a, ()> {
            self.calls.lock().expect("调用记录锁应可用").push("lock");
            Box::pin(async { Ok(()) })
        }

        fn find_by_code_for_update<'a>(
            &'a self,
            _tenant_id: &'a str,
            _code: &'a str,
        ) -> PersistenceFuture<'a, Option<PostRecord>> {
            Box::pin(async { Ok(None) })
        }

        fn find_by_id_for_update<'a>(
            &'a self,
            _tenant_id: &'a str,
            _id: i64,
        ) -> PersistenceFuture<'a, Option<PostRecord>> {
            self.calls.lock().expect("调用记录锁应可用").push("find");
            let record = self.record.clone();
            Box::pin(async move { Ok(Some(record)) })
        }

        fn insert<'a>(
            &'a self,
            _tenant_id: &'a str,
            record: PostRecord,
        ) -> PersistenceFuture<'a, PostRecord> {
            Box::pin(async move { Ok(record) })
        }

        fn update<'a>(
            &'a self,
            _tenant_id: &'a str,
            record: PostRecord,
        ) -> PersistenceFuture<'a, PostRecord> {
            self.calls.lock().expect("调用记录锁应可用").push("update");
            Box::pin(async move { Ok(record) })
        }

        fn delete<'a>(&'a self, _tenant_id: &'a str, _id: i64) -> PersistenceFuture<'a, ()> {
            Box::pin(async { Ok(()) })
        }

        fn increment_configuration_version<'a>(
            &'a self,
            _tenant_id: &'a str,
        ) -> PersistenceFuture<'a, ()> {
            self.calls.lock().expect("调用记录锁应可用").push("version");
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
    async fn update_owns_transaction_order() {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let timestamp = Utc
            .with_ymd_and_hms(2026, 8, 20, 0, 0, 0)
            .single()
            .expect("测试时间应有效");
        let persistence = Arc::new(FakePersistence {
            calls: Arc::clone(&calls),
            record: PostRecord {
                id: 7,
                name: "旧岗位".into(),
                code: "old".into(),
                sort: 1,
                status: "1".into(),
                remark: None,
                created_at: timestamp,
                updated_at: timestamp,
            },
        });
        let service = PostService::new(persistence);
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
            .update(&actor, 7, "新岗位", 2, "0".into())
            .await
            .expect("岗位更新应成功");

        assert_eq!(updated.name, "新岗位");
        assert_eq!(updated.sort, 2);
        assert_eq!(updated.status, "0");
        assert_eq!(
            *calls.lock().expect("调用记录锁应可用"),
            ["begin", "lock", "find", "update", "version", "commit"]
        );
    }
}
