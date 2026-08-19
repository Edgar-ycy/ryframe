mod crud;
mod import_export;
mod password_reset;

pub(crate) use crud::*;
pub(crate) use import_export::*;
pub(crate) use password_reset::*;

use crate::http::HttpResult;
use axum::{Router, routing::post};
use ryframe_adapters::ValidatedPageQuery;
use ryframe_application::system::UserListParams;
use ryframe_auth::rbac;
use ryframe_config::PaginationConfig;
use ryframe_kernel::AppError;
use ryframe_macro::route;

use crate::{RequestPrincipal, list_query, state::AppState};

fn ensure_current_user_permission(
    actor: &RequestPrincipal,
    permission: &str,
    message: &str,
) -> HttpResult<()> {
    if actor.is_super_admin || rbac::has_permission(&actor.permissions, permission) {
        Ok(())
    } else {
        Err(AppError::Authorization(message.into()).into())
    }
}

list_query!(pub UserListQuery, UserFilterQuery {
    username: String,
    phone: String,
    status: String,
    dept_id: String,
});

impl UserListQuery {
    fn into_service_params(self, policy: &PaginationConfig) -> HttpResult<UserListParams> {
        let (page, filter) = self.into_parts(policy)?;
        filter.into_service_params(page)
    }
}

impl UserFilterQuery {
    fn into_service_params(self, page: ValidatedPageQuery) -> HttpResult<UserListParams> {
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
        .route("/import", post(removed_synchronous_import))
        .merge(route!(list))
        .merge(route!(options))
        .merge(route!(detail))
        .merge(route!(create))
        .merge(route!(update))
        .merge(route!(remove))
        .merge(route!(batch_remove))
        .merge(route!(request_password_reset))
        .merge(route!(replace_roles))
        .merge(route!(update_status))
        .merge(route!(request_user_export))
        .merge(route!(download_import_template))
        .with_state(state)
}

async fn removed_synchronous_import() -> HttpResult<()> {
    Err(AppError::NotFound("同步用户导入接口已移除，请使用异步用户导入接口".into()).into())
}
