use ryframe_common::utils::snowflake;
use ryframe_common::{AppError, AppResult};
use ryframe_core::LoggedRepo;
use ryframe_core::Repository;
use ryframe_core::repository::{PageQuery, PageResult};
use ryframe_db::PostRepository;
use ryframe_db::entities::post;
use sea_orm::DatabaseConnection;
use serde::Serialize;

#[derive(Debug, Serialize)]
pub struct PostVo {
    pub id: i64,
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
            id: p.id,
            name: p.name,
            code: p.code,
            sort: p.sort,
            status: p.status,
            remark: p.remark,
            created_at: p.created_at,
        }
    }
}

pub struct PostServiceImpl {
    pub post_repo: LoggedRepo<PostRepository>,
}

impl PostServiceImpl {
    pub async fn find_by_page(
        &self,
        db: &DatabaseConnection,
        query: PageQuery,
    ) -> AppResult<PageResult<PostVo>> {
        let page = self.post_repo.find_by_page(db, query.clone()).await?;
        let records = page.records.into_iter().map(PostVo::from).collect();
        Ok(PageResult::new(records, page.total, &query))
    }

    pub async fn find_by_id(&self, db: &DatabaseConnection, id: i64) -> AppResult<Option<PostVo>> {
        Ok(self.post_repo.find_by_id(db, id).await?.map(PostVo::from))
    }

    pub async fn create(
        &self,
        db: &DatabaseConnection,
        name: &str,
        code: &str,
        sort: i32,
    ) -> AppResult<PostVo> {
        if self.post_repo.find_by_code(db, code).await?.is_some() {
            return Err(AppError::Conflict("岗位编码已存在".into()));
        }

        let now = chrono::Utc::now();
        let new_post = post::Model {
            id: snowflake::next_snowflake_id(),
            name: name.to_string(),
            code: code.to_string(),
            sort,
            status: "1".to_string(),
            remark: None,
            del_flag: post::Model::DEL_FLAG_NORMAL.to_string(),
            created_at: now,
            updated_at: now,
        };
        let saved = self.post_repo.insert(db, new_post).await?;
        Ok(PostVo::from(saved))
    }

    pub async fn update(
        &self,
        db: &DatabaseConnection,
        id: i64,
        name: &str,
        sort: i32,
        status: String,
    ) -> AppResult<PostVo> {
        let mut post = self
            .post_repo
            .find_by_id(db, id)
            .await?
            .ok_or_else(|| AppError::NotFound("岗位不存在".into()))?;

        post.name = name.to_string();
        post.sort = sort;
        post.status = status;
        post.updated_at = chrono::Utc::now();

        let saved = self.post_repo.update(db, post).await?;
        Ok(PostVo::from(saved))
    }

    pub async fn delete(&self, db: &DatabaseConnection, id: i64) -> AppResult<()> {
        self.post_repo
            .find_by_id(db, id)
            .await?
            .ok_or_else(|| AppError::NotFound("岗位不存在".into()))?;
        self.post_repo.delete(db, id).await
    }

    /// 带搜索条件的分页查询
    pub async fn find_by_page_filtered(
        &self,
        db: &DatabaseConnection,
        query: PageQuery,
        name: Option<&str>,
        code: Option<&str>,
        status: Option<&str>,
    ) -> AppResult<PageResult<PostVo>> {
        let page = self
            .post_repo
            .find_by_page_filtered(db, query.clone(), name, code, status)
            .await?;
        let records = page.records.into_iter().map(PostVo::from).collect();
        Ok(PageResult::new(records, page.total, &query))
    }

    /// 查询所有岗位（用于导出）
    pub async fn find_all(&self, db: &DatabaseConnection) -> AppResult<Vec<PostVo>> {
        let models = self.post_repo.find_all(db).await?;
        Ok(models.into_iter().map(PostVo::from).collect())
    }
}
