use ryframe_common::{AppError, AppResult, utils::snowflake};
use ryframe_core::{
    LoggedRepo, RedisClient, Repository,
    auto_fill::{AutoFill, FillContext},
    repository::{PageQuery, PageResult},
};
use ryframe_db::{DeptRepository, entities::dept, repositories::dept_repo::DeptTreeNode};
use sea_orm::DatabaseConnection;
use serde::Serialize;

/// 部门树缓存 Redis key
const DEPT_TREE_CACHE_KEY: &str = "sys_dept:tree";
/// 缓存过期时间（1 小时）
const CACHE_TTL_SECS: u64 = 3600;

/// 部门视图对象
#[derive(Debug, Serialize)]
pub struct DeptVo {
    pub id: i64,
    pub name: String,
    pub parent_id: Option<i64>,
    pub ancestors: String,
    pub sort: i32,
    pub status: String,
    pub remark: Option<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

impl From<dept::Model> for DeptVo {
    fn from(d: dept::Model) -> Self {
        Self {
            id: d.id,
            name: d.name,
            parent_id: d.parent_id,
            ancestors: d.ancestors,
            sort: d.sort,
            status: d.status,
            remark: d.remark,
            created_at: d.created_at,
        }
    }
}

pub struct DeptServiceImpl {
    pub dept_repo: LoggedRepo<DeptRepository>,
    pub redis: Option<RedisClient>,
}

impl DeptServiceImpl {
    pub async fn find_tree(&self, db: &DatabaseConnection) -> AppResult<Vec<DeptTreeNode>> {
        // 尝试从 Redis 缓存读取
        if let Some(ref redis) = self.redis
            && let Ok(Some(json)) = redis.get(DEPT_TREE_CACHE_KEY).await
            && let Ok(cached) = serde_json::from_str::<Vec<DeptTreeNode>>(&json)
        {
            return Ok(cached);
        }

        let tree = self.dept_repo.find_tree(db).await?;

        // 写入缓存
        if let Some(ref redis) = self.redis
            && let Ok(json) = serde_json::to_string(&tree)
        {
            let _ = redis
                .set_ex(DEPT_TREE_CACHE_KEY, &json, CACHE_TTL_SECS)
                .await;
        }

        Ok(tree)
    }

    pub async fn create(
        &self,
        db: &DatabaseConnection,
        name: &str,
        parent_id: Option<i64>,
        sort: i32,
    ) -> AppResult<dept::Model> {
        // 自动计算 ancestors
        let ancestors = self.dept_repo.build_ancestors(db, parent_id).await?;

        let mut new_dept = dept::Model {
            id: snowflake::next_snowflake_id(),
            name: name.to_string(),
            parent_id,
            ancestors,
            sort,
            status: dept::Model::STATUS_NORMAL.to_string(),
            remark: None,
            del_flag: dept::Model::DEL_FLAG_NORMAL.to_string(),
            created_at: Default::default(),
            updated_at: Default::default(),
        };
        let ctx = FillContext::new();
        new_dept.fill_on_insert(&ctx);
        self.dept_repo.insert(db, new_dept).await.inspect(|_| {
            self.invalidate_dept_cache();
        })
    }

    pub async fn update(
        &self,
        db: &DatabaseConnection,
        id: i64,
        name: &str,
        parent_id: Option<i64>,
        sort: i32,
        status: String,
    ) -> AppResult<dept::Model> {
        let mut dept = self
            .dept_repo
            .find_by_id(db, id)
            .await?
            .ok_or_else(|| AppError::NotFound("部门不存在".into()))?;

        // 如果父部门变更，重新计算 ancestors
        if dept.parent_id != parent_id {
            dept.ancestors = self.dept_repo.build_ancestors(db, parent_id).await?;
        }

        dept.name = name.to_string();
        dept.parent_id = parent_id;
        dept.sort = sort;
        dept.status = status;
        let ctx = FillContext::new();
        dept.fill_on_update(&ctx);

        self.dept_repo.update(db, dept).await.inspect(|_| {
            self.invalidate_dept_cache();
        })
    }

    pub async fn delete(&self, db: &DatabaseConnection, id: i64) -> AppResult<()> {
        self.dept_repo
            .find_by_id(db, id)
            .await?
            .ok_or_else(|| AppError::NotFound("部门不存在".into()))?;

        if self.dept_repo.has_children(db, id).await? {
            return Err(AppError::Validation("存在子部门，无法删除".into()));
        }

        self.dept_repo.delete(db, id).await.map(|_| {
            self.invalidate_dept_cache();
        })
    }

    /// 按名称/状态搜索部门列表
    pub async fn find_filtered(
        &self,
        db: &DatabaseConnection,
        name: Option<&str>,
        status: Option<&str>,
    ) -> AppResult<Vec<DeptVo>> {
        let models = self.dept_repo.find_filtered(db, name, status).await?;
        Ok(models.into_iter().map(DeptVo::from).collect())
    }

    /// 按名称/状态搜索部门列表（分页）
    pub async fn find_by_page_filtered(
        &self,
        db: &DatabaseConnection,
        query: PageQuery,
        name: Option<&str>,
        status: Option<&str>,
    ) -> AppResult<PageResult<DeptVo>> {
        let page = self
            .dept_repo
            .find_by_page_filtered(db, query.clone(), name, status)
            .await?;
        let records = page.records.into_iter().map(DeptVo::from).collect();
        Ok(PageResult::new(records, page.total, &query))
    }

    /// 按 ID 查询部门详情
    pub async fn find_by_id(&self, db: &DatabaseConnection, id: i64) -> AppResult<Option<DeptVo>> {
        Ok(self.dept_repo.find_by_id(db, id).await?.map(DeptVo::from))
    }

    /// 清除部门树缓存
    fn invalidate_dept_cache(&self) {
        if let Some(ref redis) = self.redis {
            let redis = redis.clone();
            tokio::spawn(async move {
                let _ = redis.del(DEPT_TREE_CACHE_KEY).await;
            });
        }
    }
}
