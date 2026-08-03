use ryframe_core::{
    Repository,
    repository::{PageResult, ValidatedPageQuery},
};
use ryframe_db::ReadConsistency;
use ryframe_kernel::{ActorContext, AppResult, DataScope, DataScopeContext};
use sea_orm::DatabaseConnection;

use super::{CACHE_TTL_SECS, DEPT_TREE_CACHE_NAMESPACE, DeptService, DeptTreeNode, DeptVo};

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
        let cache_lookup = self
            .authorization_cache
            .read_tenant_value(tenant_id, DEPT_TREE_CACHE_NAMESPACE)
            .await?;
        if let Some(json) = cache_lookup
            .as_ref()
            .and_then(|lookup| lookup.value.as_deref())
            && let Ok(cached) = serde_json::from_str::<Vec<DeptTreeNode>>(json)
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

        if let Some(cache_lookup) = cache_lookup {
            let json = serde_json::to_string(&tree).map_err(|error| {
                ryframe_kernel::AppError::Internal(format!("序列化部门树缓存失败: {error}"))
            })?;
            self.authorization_cache
                .store_tenant_value(
                    tenant_id,
                    DEPT_TREE_CACHE_NAMESPACE,
                    cache_lookup.tenant_authorization_epoch,
                    &json,
                    CACHE_TTL_SECS,
                )
                .await?;
        }
        Ok(tree)
    }

    pub async fn filter_dept_by_user(&self, actor: &ActorContext) -> AppResult<Vec<DeptTreeNode>> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        let scope = actor.data_scope_context();
        let db = self.db.select_read(ReadConsistency::Strong).connection;
        match self.visible_dept_ids(&db, tenant_id, &scope).await? {
            None => self.tree_list(&db, tenant_id).await,
            Some(ids) => self
                .dept_repo
                .find_tree_by_visible_ids(&db, tenant_id, &ids)
                .await
                .map(|nodes| nodes.into_iter().map(DeptTreeNode::from).collect()),
        }
    }

    pub async fn find_by_page_filtered(
        &self,
        actor: &ActorContext,
        query: ValidatedPageQuery,
        name: Option<&str>,
        status: Option<&str>,
    ) -> AppResult<PageResult<DeptVo>> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        let scope = actor.data_scope_context();
        let db = self.db.select_read(ReadConsistency::Strong).connection;
        let page = match self.visible_dept_ids(&db, tenant_id, &scope).await? {
            None => {
                self.dept_repo
                    .find_by_page_filtered(&db, tenant_id, query.clone(), name, status)
                    .await?
            }
            Some(ids) => {
                self.dept_repo
                    .find_by_page_filtered_by_ids(&db, tenant_id, query.clone(), name, status, &ids)
                    .await?
            }
        };
        let records = page.records.into_iter().map(DeptVo::from).collect();
        Ok(PageResult::new(records, page.total, &query))
    }

    pub async fn find_by_id(&self, actor: &ActorContext, id: i64) -> AppResult<Option<DeptVo>> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        let scope = actor.data_scope_context();
        let db = self.db.select_read(ReadConsistency::Strong).connection;
        if let Some(ids) = self.visible_dept_ids(&db, tenant_id, &scope).await?
            && !ids.contains(&id)
        {
            return Ok(None);
        }
        Ok(self
            .dept_repo
            .find_by_id(&db, tenant_id, id)
            .await?
            .map(DeptVo::from))
    }
}
