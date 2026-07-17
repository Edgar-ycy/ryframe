mod crud;
mod import_export;
mod password_reset;

pub(crate) use crud::*;
pub(crate) use import_export::*;
pub(crate) use password_reset::*;

use axum::Router;
use ryframe_auth::{RequestPrincipal, rbac};
use ryframe_common::{AppError, AppResult};
use ryframe_core::PageQuery;
use ryframe_macro::route;
use ryframe_service::system::UserListParams;

use crate::list_query;
use crate::state::AppState;

fn ensure_current_user_permission(
    actor: &RequestPrincipal,
    permission: &str,
    message: &str,
) -> AppResult<()> {
    if actor.is_super_admin || rbac::has_permission(&actor.permissions, permission) {
        Ok(())
    } else {
        Err(AppError::Authorization(message.into()))
    }
}

list_query!(pub UserListQuery, UserFilterQuery {
    username: String,
    phone: String,
    status: String,
    dept_id: String,
});

impl UserListQuery {
    fn into_service_params(self) -> AppResult<UserListParams> {
        let (page, filter) = self.into_parts();
        filter.into_service_params(page)
    }
}

impl UserFilterQuery {
    fn into_service_params(self, page: PageQuery) -> AppResult<UserListParams> {
        let dept_id = self
            .dept_id
            .as_deref()
            .map(str::trim)
            .filter(|id| !id.is_empty())
            .map(|id| {
                id.parse::<i64>()
                    .map_err(|_| AppError::Validation(format!("无效的部门ID: {id}")))
            })
            .transpose()?;
        Ok(UserListParams {
            page,
            username: self.username,
            phone: self.phone,
            status: self.status,
            dept_id,
        })
    }
}

pub fn user_router(state: AppState) -> Router {
    Router::new()
        .merge(route!(list))
        .merge(route!(list_no_page))
        .merge(route!(detail))
        .merge(route!(create))
        .merge(route!(update))
        .merge(route!(remove))
        .merge(route!(batch_remove))
        .merge(route!(request_password_reset))
        .merge(route!(replace_roles))
        .merge(route!(update_status))
        .merge(route!(export_users))
        .merge(route!(import_users))
        .merge(route!(download_import_template))
        .with_state(state)
}
