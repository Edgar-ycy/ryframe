use ryframe_core::{
    Repository,
    auto_fill::{AutoFill, FillContext},
    repository::{PageResult, ValidatedPageQuery},
};
use ryframe_db::{DatabaseCluster, ReadConsistency};
use ryframe_db::{PostFilter, PostRepository, entities::post};
use ryframe_kernel::{ActorContext, AppError, AppResult};
use ryframe_utils::snowflake;
use sea_orm::TransactionTrait;
use serde::Serialize;

#[derive(Debug, Serialize)]
pub struct PostVo {
    /// id 使用 String 避免 Snowflake 64 位 ID 超出 JS Number.MAX_SAFE_INTEGER
    pub id: String,
    pub name: String,
    pub code: String,
    pub sort: i32,
    pub status: String,
    pub remark: Option<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

impl From<post::Model> for PostVo {
    fn from(p: post::Model) -> Self {
        Self {
            id: p.id.to_string(),
            name: p.name,
            code: p.code,
            sort: p.sort,
            status: p.status,
            remark: p.remark,
            created_at: p.created_at,
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
    db: DatabaseCluster,
    post_repo: PostRepository,
}

impl PostService {
    pub fn new(db: DatabaseCluster) -> Self {
        Self {
            db,
            post_repo: PostRepository,
        }
    }

    pub async fn find_by_id(&self, actor: &ActorContext, id: i64) -> AppResult<Option<PostVo>> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        let db = self.db.select_read(ReadConsistency::Eventual).connection;
        Ok(self
            .post_repo
            .find_by_id(&db, tenant_id, id)
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
        let db = self.db.write();
        if self
            .post_repo
            .find_by_code(db, tenant_id, code)
            .await?
            .is_some()
        {
            return Err(AppError::Conflict("岗位编码已存在".into()));
        }

        let mut new_post = post::Model {
            id: snowflake::try_next_snowflake_id()?,
            tenant_id: tenant_id.to_owned(),
            name: name.to_string(),
            code: code.to_string(),
            sort,
            status: "1".to_string(),
            remark: None,
            del_flag: post::Model::DEL_FLAG_NORMAL.to_string(),
            created_at: Default::default(),
            updated_at: Default::default(),
        };
        new_post.fill_on_insert(&FillContext::new())?;
        let transaction = db.begin().await.map_err(database_error)?;
        let saved = self
            .post_repo
            .insert_in_transaction(&transaction, tenant_id, new_post)
            .await?;
        crate::commit_current_audit(transaction).await?;
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
        let db = self.db.write();
        let mut post = self
            .post_repo
            .find_by_id(db, tenant_id, id)
            .await?
            .ok_or_else(|| AppError::NotFound("岗位不存在".into()))?;

        post.name = name.to_string();
        post.sort = sort;
        post.status = status;
        post.fill_on_update(&FillContext::new())?;

        let transaction = db.begin().await.map_err(database_error)?;
        let saved = self
            .post_repo
            .update_in_transaction(&transaction, tenant_id, post)
            .await?;
        crate::commit_current_audit(transaction).await?;
        Ok(PostVo::from(saved))
    }

    pub async fn delete(&self, actor: &ActorContext, id: i64) -> AppResult<()> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        let db = self.db.write();
        self.post_repo
            .find_by_id(db, tenant_id, id)
            .await?
            .ok_or_else(|| AppError::NotFound("岗位不存在".into()))?;
        let transaction = db.begin().await.map_err(database_error)?;
        self.post_repo
            .delete_in_transaction(&transaction, tenant_id, id)
            .await?;
        crate::commit_current_audit(transaction).await
    }

    /// 带搜索条件的分页查询
    pub async fn find_by_page(
        &self,
        actor: &ActorContext,
        params: PostListParams,
    ) -> AppResult<PageResult<PostVo>> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        let db = self.db.select_read(ReadConsistency::Eventual).connection;
        let page = self
            .post_repo
            .find_by_page_filtered(
                &db,
                tenant_id,
                params.page.clone(),
                params.name.as_deref(),
                params.code.as_deref(),
                params.status.as_deref(),
            )
            .await?;
        let records = page.records.into_iter().map(PostVo::from).collect();
        Ok(PageResult::new(records, page.total, &params.page))
    }

    /// 以稳定主键游标分批读取岗位导出数据。
    pub async fn find_for_export(
        &self,
        actor: &ActorContext,
        name: Option<&str>,
        code: Option<&str>,
        status: Option<&str>,
        maximum_records: usize,
    ) -> AppResult<Vec<PostVo>> {
        const BATCH_SIZE: u64 = 1_000;

        let tenant_id = crate::validated_tenant_id(actor)?;
        let db = self.db.select_read(ReadConsistency::Eventual).connection;
        let filter = PostFilter { name, code, status };
        let mut after_id = None;
        let mut records = Vec::new();
        loop {
            let batch = self
                .post_repo
                .find_for_export_after_id(&db, tenant_id, &filter, after_id, BATCH_SIZE)
                .await?;
            if batch.is_empty() {
                break;
            }
            after_id = batch.last().map(|post| post.id);
            records.extend(batch.into_iter().map(PostVo::from));
            if records.len() > maximum_records {
                return Err(AppError::Validation(format!(
                    "导出记录数超过 {maximum_records} 条上限"
                )));
            }
        }
        Ok(records)
    }
}

fn database_error(error: impl std::fmt::Display) -> AppError {
    AppError::Database(error.to_string())
}
