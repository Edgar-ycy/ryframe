use ryframe_common::{AppError, AppResult};
use ryframe_core::Repository;
use ryframe_db::entities::menu;

use super::{MenuService, MenuType};

impl MenuService {
    pub(super) async fn validate_binding(
        &self,
        tenant_id: &str,
        current_id: Option<i64>,
        parent_id: Option<i64>,
        menu_type: MenuType,
        perm_id: Option<i64>,
        route_key: Option<&str>,
    ) -> AppResult<()> {
        let db = self.db.write();
        match menu_type {
            MenuType::Action => {
                if perm_id.is_none() {
                    return Err(AppError::Validation("按钮菜单必须关联权限".into()));
                }
                if route_key.is_some() {
                    return Err(AppError::Validation("按钮菜单不能设置页面标识".into()));
                }
            }
            MenuType::Page => {
                if perm_id.is_none() {
                    return Err(AppError::Validation("菜单必须关联权限".into()));
                }
                if route_key.is_none() {
                    return Err(AppError::Validation("菜单缺少可用的前端页面映射".into()));
                }
            }
            MenuType::Directory => {}
        }

        if let Some(perm_id) = perm_id {
            let exists = self.perm_repo.find_by_id(db, tenant_id, perm_id).await?;
            if exists.is_none() {
                return Err(AppError::Validation(
                    "关联权限不存在或不属于当前租户".into(),
                ));
            }
        }

        if let Some(route_key) = route_key
            && let Some(existing) = self
                .menu_repo
                .find_by_route_key(db, tenant_id, route_key)
                .await?
            && Some(existing.id) != current_id
        {
            return Err(AppError::Conflict("页面标识已被其他菜单使用".into()));
        }

        if let Some(parent_id) = parent_id {
            if Some(parent_id) == current_id {
                return Err(AppError::Validation("菜单不能将自己设为上级".into()));
            }
            let mut cursor = Some(parent_id);
            while let Some(id) = cursor {
                let parent = self
                    .menu_repo
                    .find_by_id(db, tenant_id, id)
                    .await?
                    .ok_or_else(|| AppError::Validation("上级菜单不存在".into()))?;
                if Some(parent.id) == current_id {
                    return Err(AppError::Validation(
                        "不能将菜单移动到自己的后代节点".into(),
                    ));
                }
                if parent.menu_type == menu::Model::MENU_TYPE_BUTTON {
                    return Err(AppError::Validation("按钮不能作为上级菜单".into()));
                }
                cursor = parent.parent_id;
            }
        }
        Ok(())
    }
}
