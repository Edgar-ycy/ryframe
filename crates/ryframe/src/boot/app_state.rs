use std::sync::Arc;

use ryframe_config::AppConfig;
use ryframe_core::{AppContext, RedisClient, TokenBlacklist};
use ryframe_middleware::RateLimiter;
use sea_orm::DatabaseConnection;

use super::services::Services;

/// 将所有已初始化的组件聚合为 AppState
#[allow(clippy::too_many_arguments)]
pub fn assemble(
    primary_db: DatabaseConnection,
    extra_dbs: Vec<DatabaseConnection>,
    config: Arc<AppConfig>,
    context: AppContext,
    redis_client: Option<RedisClient>,
    token_blacklist: TokenBlacklist,
    services: Services,
    limiter: Arc<RateLimiter>,
    object_storage: Arc<dyn ryframe_common::utils::ObjectStorage>,
) -> ryframe_api::AppState {
    ryframe_api::AppState {
        db: primary_db.clone(),
        config,
        context,
        auth_service: services.auth_service,
        user_service: services.user_service,
        role_service: services.role_service,
        permission_service: services.permission_service,
        menu_service: services.menu_service,
        dept_service: services.dept_service,
        post_service: services.post_service,
        config_service: services.config_service,
        dict_service: services.dict_service,
        notice_service: services.notice_service,
        oper_log_service: services.oper_log_service,
        login_info_service: services.login_info_service,
        job_service: services.job_service,
        generator_service: services.generator_service,
        profile_service: services.profile_service,
        online_user_service: services.online_user_service,
        captcha_store: services.captcha_store,
        scheduler: services.scheduler.clone(),
        monitor_db: primary_db.clone(),
        redis: redis_client.clone(),
        token_blacklist,
        replica_dbs: extra_dbs,
        rate_limiter: limiter.clone(),
        object_storage,
    }
}
