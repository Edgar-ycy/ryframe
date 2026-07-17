use ryframe_common::{
    ActorContext, AppResult,
    annotations::data_scope::{DataScope, DataScopeContext},
};
use ryframe_core::{
    Repository,
    repository::{PageQuery, PageResult},
};
use sea_orm::DatabaseConnection;

use super::{CACHE_TTL_SECS, DeptService, DeptTreeNode, DeptVo, dept_tree_cache_key};

impl DeptService {
    async fn visible_dept_ids(
        &self,
        db: &DatabaseConnection,
        tenant_id: &str,
        scope: &DataScopeContext,
    ) -> AppResult<Option<Vec<i64>>> {
        let ids = match scope.scope {
            DataScope::All => return Ok(None),
            DataScope::Custom => scope.custom_dept_ids.clone(),
            DataScope::Dept | DataScope::SelfOnly => scope.dept_id.into_iter().collect(),
            DataScope::DeptAndChildren => match scope.dept_id {
                Some(dept_id) => {
                    self.dept_repo
                        .find_child_dept_ids(db, tenant_id, dept_id)
                        .await?
                }
                None => Vec::new(),
            },
        };
        Ok(Some(ids))
    }

    async fn tree_list(
        &self,
        db: &DatabaseConnection,
        tenant_id: &str,
    ) -> AppResult<Vec<DeptTreeNode>> {
        let cache_key = dept_tree_cache_key(tenant_id);
        if let Some(redis) = &self.redis
            && let Ok(Some(json)) = redis.get(&cache_key).await
            && let Ok(cached) = serde_json::from_str::<Vec<DeptTreeNode>>(&json)
        {
            return Ok(cached);
        }

        let tree = self
            .dept_repo
            .find_tree(db, tenant_id)
            .await?
            .into_iter()
            .map(DeptTreeNode::from)
            .collect::<Vec<_>>();

        if let Some(redis) = &self.redis
            && let Ok(json) = serde_json::to_string(&tree)
            && let Err(error) = redis.set_ex(&cache_key, &json, CACHE_TTL_SECS).await
        {
            tracing::warn!(tenant_id, %error, "failed to cache department tree");
        }
        Ok(tree)
    }

    pub async fn filter_dept_by_user(&self, actor: &ActorContext) -> AppResult<Vec<DeptTreeNode>> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        let scope = actor.data_scope_context();
        let db = self.db.read();
        match self.visible_dept_ids(db, tenant_id, &scope).await? {
            None => self.tree_list(db, tenant_id).await,
            Some(ids) => self
                .dept_repo
                .find_tree_by_visible_ids(db, tenant_id, &ids)
                .await
                .map(|nodes| nodes.into_iter().map(DeptTreeNode::from).collect()),
        }
    }

    pub async fn find_filtered(
        &self,
        actor: &ActorContext,
        name: Option<&str>,
        status: Option<&str>,
    ) -> AppResult<Vec<DeptVo>> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        let scope = actor.data_scope_context();
        let db = self.db.read();
        let models = match self.visible_dept_ids(db, tenant_id, &scope).await? {
            None => {
                self.dept_repo
                    .find_filtered(db, tenant_id, name, status)
                    .await?
            }
            Some(ids) => {
                self.dept_repo
                    .find_filtered_by_ids(db, tenant_id, name, status, &ids)
                    .await?
            }
        };
        Ok(models.into_iter().map(DeptVo::from).collect())
    }

    pub async fn find_by_page_filtered(
        &self,
        actor: &ActorContext,
        query: PageQuery,
        name: Option<&str>,
        status: Option<&str>,
    ) -> AppResult<PageResult<DeptVo>> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        let scope = actor.data_scope_context();
        let db = self.db.read();
        let page = match self.visible_dept_ids(db, tenant_id, &scope).await? {
            None => {
                self.dept_repo
                    .find_by_page_filtered(db, tenant_id, query.clone(), name, status)
                    .await?
            }
            Some(ids) => {
                self.dept_repo
                    .find_by_page_filtered_by_ids(db, tenant_id, query.clone(), name, status, &ids)
                    .await?
            }
        };
        let records = page.records.into_iter().map(DeptVo::from).collect();
        Ok(PageResult::new(records, page.total, &query))
    }

    pub async fn find_by_id(&self, actor: &ActorContext, id: i64) -> AppResult<Option<DeptVo>> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        let scope = actor.data_scope_context();
        let db = self.db.read();
        if let Some(ids) = self.visible_dept_ids(db, tenant_id, &scope).await?
            && !ids.contains(&id)
        {
            return Ok(None);
        }
        Ok(self
            .dept_repo
            .find_by_id(db, tenant_id, id)
            .await?
            .map(DeptVo::from))
    }
}
