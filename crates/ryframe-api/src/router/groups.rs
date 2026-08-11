use super::*;

pub(super) fn monitor_router(
    state: AppState,
    monitor_state: ryframe_monitor::MonitorState,
) -> Router {
    let public = ryframe_monitor::public_monitor_router(monitor_state.clone());
    let mut protected = ryframe_monitor::protected_monitor_router(monitor_state)
        .merge(route!(runtime_status).with_state(state.clone()))
        .merge(job_handler::job_router(state.clone()));
    if state.config.jobs.scheduler_enabled {
        protected = protected.merge(schedule_handler::schedule_router(state.clone()));
    }
    protected = protected.layer(from_fn_with_state(
        OperLogMiddlewareState::new_arc(state.services.audit_outbox.clone()),
        oper_log_middleware,
    ));

    public.merge(protect(protected, &state))
}

/// 系统管理路由（认证主体 + 租户限流 + 用户限流 + 在线跟踪 + 操作日志）
///
/// .layer() 链的语义：后注册的 layer 包裹先注册的，即后注册的先执行（外层先执行）。
/// 执行顺序（从外到内）：
///   1. auth_middleware（一次注入 RequestPrincipal）
///   2. authenticated_tenant_rate_limit（使用已认证租户）
///   3. 用户限流中间件（`user_rate_limit_middleware`）
///   4. 在线用户跟踪（`online_user_tracking`）
///   5. 操作日志中间件（`oper_log_middleware`）
pub(super) fn system_router(
    state: AppState,
    rate_limit_state: RateLimitState,
    idempotency_state: IdempotencyState,
) -> Router {
    let router = Router::new()
        .nest("/users", user_handler::user_router(state.clone()))
        .nest("/roles", role_handler::role_router(state.clone()))
        .nest(
            "/perms",
            permission_handler::permission_router(state.clone()),
        )
        .nest("/menus", menu_handler::menu_router(state.clone()))
        .nest("/depts", dept_handler::dept_router(state.clone()))
        .nest("/posts", post_handler::post_router(state.clone()))
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
        .nest(
            "/online",
            online_user_handler::online_user_router(state.clone()),
        )
        // 从内到外注册：内层 layer 先注册
        .layer(from_fn_with_state(
            OperLogMiddlewareState::new_arc(state.services.audit_outbox.clone()),
            oper_log_middleware,
        ))
        .layer(from_fn_with_state(
            idempotency_state,
            idempotency_middleware,
        ))
        .layer(from_fn_with_state(
            state.services.online_user.clone(),
            online_user_tracking,
        ))
        .layer(from_fn_with_state(
            rate_limit_state,
            user_rate_limit_middleware,
        ));

    protect(router, &state)
}

/// 工具路由（认证主体 + 租户限流 + 用户限流 + 操作日志）
///
/// 执行顺序（从外到内）：auth → tenant_rate_limit → user_rate_limit → oper_log
pub(super) fn tools_router(state: AppState, rate_limit_state: RateLimitState) -> Router {
    let router = Router::new()
        .nest("/gen", generator_handler::generator_router(state.clone()))
        .layer(from_fn_with_state(
            OperLogMiddlewareState::new_arc(state.services.audit_outbox.clone()),
            oper_log_middleware,
        ))
        .layer(from_fn_with_state(
            rate_limit_state,
            user_rate_limit_middleware,
        ));

    protect(router, &state)
}

/// 通用功能路由（文件上传等）
/// 上传和下载都要求认证主体，并记录操作日志。
pub(super) fn common_router(state: AppState) -> Router {
    let oper_log_state = OperLogMiddlewareState::new_arc(state.services.audit_outbox.clone());

    let upload = protect(
        common_handler::upload_router(state.clone()).layer(from_fn_with_state(
            oper_log_state.clone(),
            oper_log_middleware,
        )),
        &state,
    );

    let download = protect(
        common_handler::download_router(state.clone())
            .layer(from_fn_with_state(oper_log_state, oper_log_middleware)),
        &state,
    );
    let exports = protect(export_handler::export_router(state.clone()), &state);

    Router::new()
        .nest("/upload", upload)
        .nest("/file", download)
        .nest("/jobs", exports)
}
