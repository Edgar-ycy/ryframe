use std::collections::HashSet;

use ryframe_kernel::{AppError, AppResult};

use crate::MenuWriteTransaction;

use super::{MenuService, MenuType};

pub(super) struct MenuBinding<'a> {
    pub current_id: Option<i64>,
    pub parent_id: Option<i64>,
    pub menu_type: MenuType,
    pub perm_id: Option<i64>,
    pub route_key: Option<&'a str>,
}

impl MenuService {
    pub(super) async fn validate_binding(
        &self,
        transaction: &dyn MenuWriteTransaction,
        tenant_id: &str,
        binding: MenuBinding<'_>,
    ) -> AppResult<()> {
        match binding.menu_type {
            MenuType::Action => {
                if binding.perm_id.is_none() {
                    return Err(AppError::Validation("按钮菜单必须关联权限".into()));
                }
                if binding.route_key.is_some() {
                    return Err(AppError::Validation("按钮菜单不能设置页面标识".into()));
                }
            }
            MenuType::Page => {
                if binding.perm_id.is_none() {
                    return Err(AppError::Validation("菜单必须关联权限".into()));
                }
                if binding.route_key.is_none() {
                    return Err(AppError::Validation("菜单缺少可用的前端页面映射".into()));
                }
            }
            MenuType::Directory => {}
        }

        if let Some(perm_id) = binding.perm_id
            && !transaction
                .permission_exists_for_update(tenant_id, perm_id)
                .await?
        {
            return Err(AppError::Validation(
                "关联权限不存在或不属于当前租户".into(),
            ));
        }

        if let Some(route_key) = binding.route_key
            && let Some(existing) = transaction
                .find_by_route_key_for_update(tenant_id, route_key)
                .await?
            && Some(existing.id) != binding.current_id
        {
            return Err(AppError::Conflict("页面标识已被其他菜单使用".into()));
        }

        if binding.parent_id == binding.current_id && binding.parent_id.is_some() {
            return Err(AppError::Validation("菜单不能将自己设为上级".into()));
        }
        let mut cursor = binding.parent_id;
        let mut visited = HashSet::new();
        while let Some(id) = cursor {
            if !visited.insert(id) {
                return Err(AppError::Internal("菜单父级链存在循环".into()));
            }
            let parent = transaction
                .find_by_id_for_update(tenant_id, id)
                .await?
                .ok_or_else(|| AppError::Validation("上级菜单不存在".into()))?;
            if Some(parent.id) == binding.current_id {
                return Err(AppError::Validation(
                    "不能将菜单移动到自己的后代节点".into(),
                ));
            }
            if parent.menu_type == MenuType::Action.as_str() {
                return Err(AppError::Validation("按钮不能作为上级菜单".into()));
            }
            cursor = parent.parent_id;
        }
        Ok(())
    }
}
