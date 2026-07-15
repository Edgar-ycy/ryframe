use ryframe_common::{
    AppError, AppResult,
    annotations::data_scope::{DataScope, DataScopeContext},
    utils::snowflake,
};
use ryframe_core::{
    LoggedRepo, RedisClient, Repository,
    auto_fill::{AutoFill, FillContext},
    repository::{PageQuery, PageResult},
};
use ryframe_db::{
    DeptRepository,
    entities::{dept, role_dept, user},
    repositories::dept_repo::DeptTreeNode,
};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, TransactionTrait,
};
use serde::Serialize;

/// 缓存过期时间（1 小时）
const CACHE_TTL_SECS: u64 = 3600;

fn dept_tree_cache_key() -> String {
    format!("tenant:{}:sys_dept:tree", ryframe_core::current_tenant_id())
}

/// 部门视图对象
#[derive(Debug, Serialize)]
pub struct DeptVo {
    /// id 使用 String 避免 Snowflake 64 位 ID 超出 JS Number.MAX_SAFE_INTEGER
    pub id: String,
    pub name: String,
    pub parent_id: Option<String>,
    pub ancestors: String,
    pub sort: i32,
    pub status: String,
    pub remark: Option<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

impl From<dept::Model> for DeptVo {
    fn from(d: dept::Model) -> Self {
        Self {
            id: d.id.to_string(),
            name: d.name,
            parent_id: d.parent_id.map(|p| p.to_string()),
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
    async fn visible_dept_ids(
        &self,
        db: &DatabaseConnection,
        scope: &DataScopeContext,
    ) -> AppResult<Option<Vec<i64>>> {
        let ids = match scope.scope {
            DataScope::All => return Ok(None),
            DataScope::Custom => scope.custom_dept_ids.clone(),
            DataScope::Dept | DataScope::SelfOnly => scope.dept_id.into_iter().collect(),
            DataScope::DeptAndChildren => match scope.dept_id {
                Some(dept_id) => self.dept_repo.find_child_dept_ids(db, dept_id).await?,
                None => Vec::new(),
            },
        };
        Ok(Some(ids))
    }

    pub async fn tree_list(&self, db: &DatabaseConnection) -> AppResult<Vec<DeptTreeNode>> {
        let cache_key = dept_tree_cache_key();
        // 尝试从 Redis 缓存读取
        if let Some(ref redis) = self.redis
            && let Ok(Some(json)) = redis.get(&cache_key).await
            && let Ok(cached) = serde_json::from_str::<Vec<DeptTreeNode>>(&json)
        {
            return Ok(cached);
        }

        let tree = self.dept_repo.find_tree(db).await?;

        // 写入缓存
        if let Some(ref redis) = self.redis
            && let Ok(json) = serde_json::to_string(&tree)
        {
            let _ = redis.set_ex(&cache_key, &json, CACHE_TTL_SECS).await;
        }

        Ok(tree)
    }

    pub async fn filter_dept_by_user(
        &self,
        db: &DatabaseConnection,
        scope: &DataScopeContext,
    ) -> AppResult<Vec<DeptTreeNode>> {
        match self.visible_dept_ids(db, scope).await? {
            None => self.tree_list(db).await,
            Some(ids) => self.dept_repo.find_tree_by_visible_ids(db, &ids).await,
        }
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
            tenant_id: ryframe_core::current_tenant_id(),
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

        let parent_changed = dept.parent_id != parent_id;
        if parent_id == Some(id) {
            return Err(AppError::Validation("部门不能将自己设为上级".into()));
        }
        let old_ancestors = dept.ancestors.clone();
        let descendants = if parent_changed {
            let descendants = self.dept_repo.find_descendants(db, id).await?;
            if parent_id.is_some_and(|parent| descendants.iter().any(|item| item.id == parent)) {
                return Err(AppError::Validation(
                    "不能将部门移动到自己的后代节点".into(),
                ));
            }
            dept.ancestors = self.dept_repo.build_ancestors(db, parent_id).await?;
            descendants
        } else {
            Vec::new()
        };

        dept.name = name.to_string();
        dept.parent_id = parent_id;
        dept.sort = sort;
        dept.status = status;
        dept.fill_on_update(&FillContext::new());

        if !parent_changed {
            return self.dept_repo.update(db, dept).await.inspect(|_| {
                self.invalidate_dept_cache();
            });
        }

        let new_ancestors = dept.ancestors.clone();
        let old_prefix = format!("{},{}", old_ancestors, id);
        let new_prefix = format!("{},{}", new_ancestors, id);
        let txn = db
            .begin()
            .await
            .map_err(|e| AppError::Database(e.to_string()))?;
        let saved = dept::ActiveModel::from(dept)
            .update(&txn)
            .await
            .map_err(|e| AppError::Database(e.to_string()))?;
        for mut child in descendants {
            let suffix = child
                .ancestors
                .strip_prefix(&old_prefix)
                .ok_or_else(|| AppError::Internal("部门祖级路径不一致，无法移动子树".into()))?;
            child.ancestors = format!("{}{}", new_prefix, suffix);
            child.fill_on_update(&FillContext::new());
            dept::ActiveModel::from(child)
                .update(&txn)
                .await
                .map_err(|e| AppError::Database(e.to_string()))?;
        }
        txn.commit()
            .await
            .map_err(|e| AppError::Database(e.to_string()))?;
        self.invalidate_dept_cache();
        Ok(saved)
    }

    pub async fn delete(&self, db: &DatabaseConnection, id: i64) -> AppResult<()> {
        self.dept_repo
            .find_by_id(db, id)
            .await?
            .ok_or_else(|| AppError::NotFound("部门不存在".into()))?;

        if self.dept_repo.has_children(db, id).await? {
            return Err(AppError::Validation("存在子部门，无法删除".into()));
        }
        let tenant_id = ryframe_core::current_tenant_id();
        let has_users = user::Entity::find()
            .filter(user::Column::TenantId.eq(&tenant_id))
            .filter(user::Column::DelFlag.eq(user::Model::DEL_FLAG_NORMAL))
            .filter(user::Column::DeptId.eq(id))
            .one(db)
            .await
            .map_err(|e| AppError::Database(e.to_string()))?
            .is_some();
        let has_role_scopes = role_dept::Entity::find()
            .filter(role_dept::Column::TenantId.eq(tenant_id))
            .filter(role_dept::Column::DeptId.eq(id))
            .one(db)
            .await
            .map_err(|e| AppError::Database(e.to_string()))?
            .is_some();
        if has_users || has_role_scopes {
            return Err(AppError::Conflict(
                "部门仍被用户或角色数据权限引用，无法删除".into(),
            ));
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

    pub async fn find_filtered_with_data_scope(
        &self,
        db: &DatabaseConnection,
        name: Option<&str>,
        status: Option<&str>,
        scope: &DataScopeContext,
    ) -> AppResult<Vec<DeptVo>> {
        let models = match self.visible_dept_ids(db, scope).await? {
            None => self.dept_repo.find_filtered(db, name, status).await?,
            Some(ids) => {
                self.dept_repo
                    .find_filtered_by_ids(db, name, status, &ids)
                    .await?
            }
        };
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

    pub async fn find_by_page_filtered_with_data_scope(
        &self,
        db: &DatabaseConnection,
        query: PageQuery,
        name: Option<&str>,
        status: Option<&str>,
        scope: &DataScopeContext,
    ) -> AppResult<PageResult<DeptVo>> {
        let page = match self.visible_dept_ids(db, scope).await? {
            None => {
                self.dept_repo
                    .find_by_page_filtered(db, query.clone(), name, status)
                    .await?
            }
            Some(ids) => {
                self.dept_repo
                    .find_by_page_filtered_by_ids(db, query.clone(), name, status, &ids)
                    .await?
            }
        };
        let records = page.records.into_iter().map(DeptVo::from).collect();
        Ok(PageResult::new(records, page.total, &query))
    }

    /// 按 ID 查询部门详情
    pub async fn find_by_id(&self, db: &DatabaseConnection, id: i64) -> AppResult<Option<DeptVo>> {
        Ok(self.dept_repo.find_by_id(db, id).await?.map(DeptVo::from))
    }

    pub async fn find_by_id_with_data_scope(
        &self,
        db: &DatabaseConnection,
        id: i64,
        scope: &DataScopeContext,
    ) -> AppResult<Option<DeptVo>> {
        if let Some(ids) = self.visible_dept_ids(db, scope).await?
            && !ids.contains(&id)
        {
            return Ok(None);
        }
        self.find_by_id(db, id).await
    }

    /// 清除部门树缓存
    fn invalidate_dept_cache(&self) {
        if let Some(ref redis) = self.redis {
            let redis = redis.clone();
            let cache_key = dept_tree_cache_key();
            tokio::spawn(async move {
                let _ = redis.del(&cache_key).await;
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::dept_tree_cache_key;
    use ryframe_core::{TenantContext, with_tenant_context};

    #[tokio::test]
    async fn cache_key_is_tenant_scoped() {
        let first = with_tenant_context(
            TenantContext {
                tenant_id: "tenant-a".into(),
                is_admin: false,
            },
            async { dept_tree_cache_key() },
        )
        .await;
        let second = with_tenant_context(
            TenantContext {
                tenant_id: "tenant-b".into(),
                is_admin: false,
            },
            async { dept_tree_cache_key() },
        )
        .await;
        assert_eq!(first, "tenant:tenant-a:sys_dept:tree");
        assert_eq!(second, "tenant:tenant-b:sys_dept:tree");
        assert_ne!(first, second);
    }
}
