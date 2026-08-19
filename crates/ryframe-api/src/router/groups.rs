use super::*;

pub(super) fn monitor_router(
    state: AppState,
    monitor_state: ryframe_monitor::MonitorState,
) -> Router {
    let public = ryframe_monitor::public_monitor_router(monitor_state.clone());
    let mut protected = ryframe_monitor::protected_monitor_router(monitor_state)
        .merge(route!(runtime_status).with_state(state.clone()))
        .merge(overview_handler::overview_router(state.clone()))
        .merge(job_handler::job_router(state.clone()))
        .merge(retention_handler::retention_router(state.clone()));
    if state.config.jobs.scheduler_enabled {
        protected = protected.merge(schedule_handler::schedule_router(state.clone()));
    }
    protected = protected.layer(from_fn_with_state(
        OperLogMiddlewareState::new_arc(state.services.audit_outbox.clone()),
        oper_log_middleware,
    ));

    public.merge(protect(protected, &state))
}

/// 系统管理路由（认证主体 + 租户限流 + 用户限流 + 操作日志）
///
/// .layer() 链的语义：后注册的 layer 包裹先注册的，即后注册的先执行（外层先执行）。
/// 执行顺序（从外到内）：
///   1. auth_middleware（一次注入 RequestPrincipal）
///   2. authenticated_tenant_rate_limit（使用已认证租户）
///   3. 用户限流中间件（`user_rate_limit_middleware`）
///   4. 操作日志中间件（`oper_log_middleware`）
pub(super) fn system_router(
    state: AppState,
    rate_limit_state: RateLimitState,
    idempotency_state: IdempotencyState,
) -> Router {
    // 配置迁移写接口已经使用 MySQL 唯一键和后台任务去重实现持久幂等，
    // 不应让 Redis 可用性成为创建、预览、应用或回滚的前置条件。
    let database_idempotent = Router::new()
        .nest(
            "/config-packages",
            tenant_config_handler::config_package_router(state.clone()),
        )
        .nest(
            "/config-transfers",
            tenant_config_handler::config_transfer_router(state.clone()),
        );
    // 始终挂载稳定路由：外层通用 Capability guard 会先区分部署 501
    // 和租户产品授权 403，通过后才进入具体 RBAC 和 handler。
    let database_idempotent = database_idempotent
        .nest(
            "/service-accounts",
            service_account_handler::service_account_router(state.clone()).layer(
                from_fn_with_state(
                    CapabilityGuardState::new(
                        state.clone(),
                        ryframe_application::system::SERVICE_ACCOUNTS_CAPABILITY,
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
                        ryframe_application::system::SERVICE_ACCOUNTS_CAPABILITY,
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
                        state.clone(),
                        ryframe_application::system::SERVICE_ACCOUNTS_CAPABILITY,
                    ),
                    capability_guard,
                ),
            ),
        );
    let database_idempotent = database_idempotent.layer(from_fn_with_state(
        OperLogMiddlewareState::new_arc(state.services.audit_outbox.clone()),
        oper_log_middleware,
    ));

    let redis_idempotent = Router::new()
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
        ));

    let router = Router::new()
        .merge(database_idempotent)
        .merge(redis_idempotent)
        // 从内到外注册：公共系统管理层继续统一提供用户限流。
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
