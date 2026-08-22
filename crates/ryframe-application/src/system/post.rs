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
