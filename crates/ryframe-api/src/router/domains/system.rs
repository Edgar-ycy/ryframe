use axum::{Router, middleware::from_fn_with_state};

use crate::{
    generated,
    handlers::{
        authorization_diagnostic_handler, config_handler, dept_handler, dict_handler,
        login_log_handler, menu_handler, message_handler, notice_handler, online_user_handler,
        oper_log_handler, permission_handler, post_export_handler, role_handler,
        service_account_handler, tenant_config_handler, user_handler, user_import_handler,
    },
    state::AppState,
};

use super::super::{CapabilityGuardState, capability_guard};

pub(in crate::router) fn database_idempotent(state: AppState) -> Router {
    Router::new()
        .nest(
            "/config-packages",
            tenant_config_handler::config_package_router(state.clone()),
        )
        .nest(
            "/config-transfers",
            tenant_config_handler::config_transfer_router(state.clone()),
        )
        .nest(
            "/service-accounts",
            service_account_handler::service_account_router(state.clone()).layer(
                from_fn_with_state(
                    CapabilityGuardState::new(
                        state.clone(),
                        ryframe_application::system::platform::SERVICE_ACCOUNTS_CAPABILITY,
                    ),
                    capability_guard,
                ),
            ),
        )
        .nest(
            "/service-delegations",
            service_account_handler::service_delegation_router(state.clone()).layer(
                from_fn_with_state(
                    CapabilityGuardState::new(
                        state.clone(),
                        ryframe_application::system::platform::SERVICE_ACCOUNTS_CAPABILITY,
                    ),
                    capability_guard,
                ),
            ),
        )
        .nest(
            "/service-access-audits",
            service_account_handler::service_access_audit_router(state.clone()).layer(
                from_fn_with_state(
                    CapabilityGuardState::new(
                        state,
                        ryframe_application::system::platform::SERVICE_ACCOUNTS_CAPABILITY,
                    ),
                    capability_guard,
                ),
            ),
        )
}

pub(in crate::router) fn redis_idempotent(state: AppState) -> Router {
    Router::new()
        .nest(
            "/authorization-diagnostics",
            authorization_diagnostic_handler::authorization_diagnostic_router(state.clone()),
        )
        .nest("/users", user_handler::user_router(state.clone()))
        .nest(
            "/user-imports",
            user_import_handler::user_import_router(state.clone()),
        )
        .nest("/roles", role_handler::role_router(state.clone()))
        .nest(
            "/perms",
            permission_handler::permission_router(state.clone()),
        )
        .nest("/menus", menu_handler::menu_router(state.clone()))
        .nest("/depts", dept_handler::dept_router(state.clone()))
        .merge(generated::generated_router(
            &state.services.content.generated,
            state.settings.pagination,
        ))
        .nest(
            "/posts",
            post_export_handler::post_export_router(state.clone()),
        )
        .nest("/configs", config_handler::config_router(state.clone()))
        .nest("/dict", dict_handler::dict_router(state.clone()))
        .nest("/notices", notice_handler::notice_router(state.clone()))
        .nest("/messages", message_handler::message_router(state.clone()))
        .nest(
            "/operlogs",
            oper_log_handler::oper_log_router(state.clone()),
        )
        .nest(
            "/loginlogs",
            login_log_handler::login_log_router(state.clone()),
        )
        .nest("/online", online_user_handler::online_user_router(state))
}

pub(in crate::router) fn independent_online(state: AppState) -> Router {
    Router::new().nest("/online", online_user_handler::force_logout_router(state))
}
