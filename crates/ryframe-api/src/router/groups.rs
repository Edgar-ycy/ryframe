use super::*;

pub(super) fn monitor_router(
    state: AppState,
    monitor_state: crate::monitor::MonitorState,
) -> Router {
    let public = domains::monitor::public(monitor_state.clone());
    let protected =
        domains::monitor::protected(state.clone(), monitor_state).layer(from_fn_with_state(
            OperLogMiddlewareState::new_arc(state.services.operations.audit_outbox.clone()),
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
    let database_idempotent =
        domains::system::database_idempotent(state.clone()).layer(from_fn_with_state(
            OperLogMiddlewareState::new_arc(state.services.operations.audit_outbox.clone()),
            oper_log_middleware,
        ));

    let redis_idempotent = domains::system::redis_idempotent(state.clone())
        // 从内到外注册：内层 layer 先注册
        .layer(from_fn_with_state(
            OperLogMiddlewareState::new_arc(state.services.operations.audit_outbox.clone()),
            oper_log_middleware,
        ))
        .layer(from_fn_with_state(
            idempotency_state.clone(),
            idempotency_middleware,
        ));

    // 强制下线写入 Redis，不参与数据库业务事务。审计策略必须位于审计
    // 中间件外层，确保请求进入审计逻辑前已经可见。
    let independent_online = domains::system::independent_online(state.clone())
        .layer(from_fn_with_state(
            OperLogMiddlewareState::new_arc(state.services.operations.audit_outbox.clone()),
            oper_log_middleware,
        ))
        .layer(Extension(AuditMode::Independent))
        .layer(from_fn_with_state(
            idempotency_state,
            idempotency_middleware,
        ));

    let router = Router::new()
        .merge(database_idempotent)
        .merge(redis_idempotent)
        .merge(independent_online)
        // 从内到外注册：公共系统管理层继续统一提供用户限流。
        .layer(from_fn_with_state(
            rate_limit_state,
            user_rate_limit_middleware,
        ));

    protect(router, &state)
}

/// 通用功能路由（文件上传等）
/// 上传和下载都要求认证主体，并记录操作日志。
pub(super) fn common_router(state: AppState, idempotency_state: IdempotencyState) -> Router {
    let oper_log_state =
        OperLogMiddlewareState::new_arc(state.services.operations.audit_outbox.clone());

    let upload = protect(
        domains::common::upload(state.clone()).layer(from_fn_with_state(
            oper_log_state.clone(),
            oper_log_middleware,
        )),
        &state,
    );

    let download = protect(
        domains::common::download(state.clone()).layer(from_fn_with_state(
            oper_log_state.clone(),
            oper_log_middleware,
        )),
        &state,
    );
    let regular_exports = domains::common::exports(state.clone()).layer(from_fn_with_state(
        oper_log_state.clone(),
        oper_log_middleware,
    ));
    // 通知已读只更新界面状态，不与导出业务事务绑定。后注册的审计策略
    // 位于操作审计外层，确保中间件读取到 Independent。
    let notification_read = domains::common::notification_read(state.clone())
        .layer(from_fn_with_state(oper_log_state, oper_log_middleware))
        .layer(Extension(AuditMode::Independent));
    let exports = protect(
        Router::new()
            .merge(regular_exports)
            .merge(notification_read)
            .layer(from_fn_with_state(
                idempotency_state,
                idempotency_middleware,
            )),
        &state,
    );

    Router::new()
        .merge(protect(domains::common::settings(state.clone()), &state))
        .nest("/upload", upload)
        .nest("/file", download)
        .nest("/jobs", exports)
}
