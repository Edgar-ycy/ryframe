use ryframe_kernel::{
    ActorContext, AppResult, DataScope, DataScopeContext, PageResult, ValidatedPageQuery,
};

use crate::ports::system::DeptFilter;

use super::{CACHE_TTL_SECS, DEPT_TREE_CACHE_NAMESPACE, DeptService, DeptTreeNode, DeptVo};

impl DeptService {
    async fn visible_dept_ids(
        &self,
        tenant_id: &str,
        scope: DataScopeContext,
    ) -> AppResult<Option<Vec<i64>>> {
        let ids = match scope.scope {
            DataScope::All => return Ok(None),
            DataScope::Custom => scope.custom_dept_ids,
            DataScope::Dept | DataScope::SelfOnly => scope.dept_id.into_iter().collect(),
            DataScope::DeptAndChildren => match scope.dept_id {
                Some(dept_id) => self.read.find_child_ids(tenant_id, dept_id).await?,
                None => Vec::new(),
            },
        };
        Ok(Some(ids))
    }

    async fn tree_list(&self, tenant_id: &str) -> AppResult<Vec<DeptTreeNode>> {
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
            .read
            .find_tree(tenant_id, None)
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
        match self.visible_dept_ids(tenant_id, scope).await? {
            None => self.tree_list(tenant_id).await,
            Some(ids) => self
                .read
                .find_tree(tenant_id, Some(&ids))
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
        let visible_ids = self.visible_dept_ids(tenant_id, scope).await?;
        let page = self
            .read
            .find_page(
                tenant_id,
                query,
                DeptFilter { name, status },
                visible_ids.as_deref(),
            )
            .await?;
        Ok(PageResult::new(
            page.records.into_iter().map(DeptVo::from).collect(),
            page.total,
            &query,
        ))
    }

    pub async fn find_by_id(&self, actor: &ActorContext, id: i64) -> AppResult<Option<DeptVo>> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        let scope = actor.data_scope_context();
        if let Some(ids) = self.visible_dept_ids(tenant_id, scope).await?
            && !ids.contains(&id)
        {
            return Ok(None);
        }
        self.read
            .find_by_id(tenant_id, id)
            .await
            .map(|record| record.map(DeptVo::from))
    }
}
