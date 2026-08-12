use ryframe_db::entities::menu;
use ryframe_kernel::{AppError, AppResult};
use sea_orm::{ColumnTrait, DatabaseTransaction, EntityTrait, QueryFilter, QuerySelect};

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
        transaction: &DatabaseTransaction,
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

        if let Some(perm_id) = binding.perm_id {
            let exists = ryframe_db::entities::permission::Entity::find_by_id(perm_id)
                .filter(ryframe_db::entities::permission::Column::TenantId.eq(tenant_id))
                .lock(sea_orm::sea_query::LockType::Update)
                .one(transaction)
                .await
                .map_err(|error| AppError::Database(error.to_string()))?;
            if exists.is_none() {
                return Err(AppError::Validation(
                    "关联权限不存在或不属于当前租户".into(),
                ));
            }
        }

        if let Some(route_key) = binding.route_key
            && let Some(existing) = menu::Entity::find()
                .filter(menu::Column::TenantId.eq(tenant_id))
                .filter(menu::Column::DelFlag.eq(menu::Model::DEL_FLAG_NORMAL))
                .filter(menu::Column::RouteKey.eq(route_key))
                .lock(sea_orm::sea_query::LockType::Update)
                .one(transaction)
                .await
                .map_err(|error| AppError::Database(error.to_string()))?
            && Some(existing.id) != binding.current_id
        {
            return Err(AppError::Conflict("页面标识已被其他菜单使用".into()));
        }

        if let Some(parent_id) = binding.parent_id {
            if Some(parent_id) == binding.current_id {
                return Err(AppError::Validation("菜单不能将自己设为上级".into()));
            }
            let mut cursor = Some(parent_id);
            while let Some(id) = cursor {
                let parent = menu::Entity::find_by_id(id)
                    .filter(menu::Column::TenantId.eq(tenant_id))
                    .filter(menu::Column::DelFlag.eq(menu::Model::DEL_FLAG_NORMAL))
                    .lock(sea_orm::sea_query::LockType::Update)
                    .one(transaction)
                    .await
                    .map_err(|error| AppError::Database(error.to_string()))?
                    .ok_or_else(|| AppError::Validation("上级菜单不存在".into()))?;
                if Some(parent.id) == binding.current_id {
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
