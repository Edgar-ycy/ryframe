use utoipa::OpenApi;

/// RyFrame API 文档
///
/// 访问 `/swagger-ui` 查看交互式 API 文档
#[derive(OpenApi)]
#[openapi(
    info(
        title = "RyFrame API",
        version = env!("CARGO_PKG_VERSION"),
        description = r#"RyFrame —— 基于 Rust + Axum 的现代化企业级后端框架。

## 认证
所有受保护接口需在请求头携带 `Authorization: Bearer <access_token>`。
登录接口在 JSON 中返回短期 `access_token`，长期 refresh token 只通过 HttpOnly Cookie 下发。

## 响应格式
```json
{
  "code": 200,
  "message": "操作成功",
  "data": { ... },
  "request_id": "01K...",
  "error_key": null,
  "details": null
}
```
分页数据统一位于 `data`，字段为 `items/page/page_size/total/total_pages/max_page_size`；
不再使用旧 `msg`、顶层 `rows` 或顶层 `total`。

## 菜单类型
菜单管理使用 `menu_type` 字段区分节点类型：
- `M`（目录）：侧边栏一级分组，无实际页面
- `C`（菜单）：可点击的页面路由
- `F`（按钮）：页面内的操作按钮，显示与授权由独立权限码控制
"#,
        license(name = "MIT")
    ),
    tags(
        (name = "认证", description = "登录/登出/刷新令牌/验证码获取。登录需验证码（可通过配置关闭），支持暴力破解防护。"),
        (name = "用户管理", description = "用户 CRUD、分页查询、详情、导入导出、密码重置请求、状态变更。"),
        (name = "用户导入", description = "可恢复、可取消的异步用户导入和错误报告。"),
        (name = "权限诊断", description = "从主库重算目标用户授权，并展示角色、权限、菜单、数据范围和缓存版本状态。"),
        (name = "角色管理", description = "角色 CRUD、权限分配(role_permission)、数据权限设置(data_scope + sys_role_dept)。"),
        (name = "菜单管理", description = "菜单树管理（含目录M/菜单C/按钮F）。管理端只允许维护上级菜单、名称、图标、排序、可见和状态。"),
        (name = "权限管理", description = "权限码树查询，用于角色分配权限时展示可选权限列表。"),
        (name = "部门管理", description = "部门树 CRUD，支持祖级列表(ancestors)快速查询子部门。"),
        (name = "岗位管理", description = "岗位 CRUD，用户可关联岗位。"),
        (name = "字典管理", description = "字典类型 + 字典数据 CRUD，前端可据此渲染下拉选项。"),
        (name = "参数配置", description = "系统参数键值对 CRUD，支持按 key 精确查询。"),
        (name = "通知公告", description = "通知公告 CRUD，支持草稿/发布/关闭状态。"),
        (name = "消息中心", description = "持久化收件箱、确认、已读状态和按租户固化的受众快照。"),
        (name = "操作日志", description = "POST/PUT/DELETE 请求自动记录，支持分页查询、详情和导出；业务管理端不提供清空入口。"),
        (name = "登录日志", description = "登录成功/失败记录，含 IP、浏览器、操作系统信息。"),
        (name = "在线用户", description = "查看当前在线设备会话，使用稳定 sid 精确强制下线。"),
        (name = "后台任务", description = "查看当前租户的持久化任务队列状态，并人工重试死信任务。"),
        (name = "运维总览", description = "严格按当前租户聚合依赖状态、任务状态和固定时间桶趋势。"),
        (name = "数据保留", description = "预览并运行系统租户的数据生命周期清理。"),
        (name = "定时任务", description = "管理租户隔离的 Cron 计划、立即执行和执行历史。"),
        (name = "服务器监控", description = "/metrics(Prometheus) 公开；进程与依赖探针分别使用根路径 /livez、/readyz；/server、/cache、/db-pool、/runtime 需认证。"),
        (name = "运行探针", description = "/livez 只报告进程存活；后台任务检查 MySQL、required Redis 与对象存储，/readyz 只读取有时效上限的内存快照。"),
        (name = "个人中心", description = "当前用户信息查看/修改、密码修改、头像更新（全部需认证）。"),
        (name = "通用", description = "/upload、/upload/image、/upload/avatar、/file/download 均需认证。上传链路包含魔数校验、去重和熔断保护。"),
        (name = "租户管理", description = "系统租户管理租户生命周期、配额和管理员初始化。"),
        (name = "租户配置迁移", description = "导出、上传、预览、应用和回滚不含数据库 ID 与敏感凭据的租户配置包。")
        ,(name = "产品能力", description = "编译期能力目录与租户有效产品上下文。")
        ,(name = "产品套餐", description = "产品套餐元数据、不可变发布版本及租户产品变更。")
        ,(name = "租户数据放置", description = "安全目标元数据、租户 placement 与迁移资格。")
        ,(name = "租户数据迁移", description = "停写复制、校验、切换、取消与保留期清理。")
        ,(name = "租户数据备份", description = "数据库平台 opaque 备份恢复点登记结果。")
        ,(name = "服务账号", description = "管理不可登录的服务账号、角色范围和一次性 API Key。")
        ,(name = "服务委托", description = "当前用户本人创建的限时双主体查询委托，以及管理员只读治理入口。")
        ,(name = "服务访问审计", description = "查询 Agent API 的最小化访问审计，不包含请求或响应正文。")
        ,(name = "个人服务委托", description = "当前用户本人查看、创建和撤销限时服务委托。")
        ,(name = "Agent API", description = "仅接受 RyFrameApiKey 的编译期白名单只读接口，不接受普通 Bearer。")
    ),
    paths(
        crate::router::api_version,
        // 认证接口
        crate::handlers::auth_handler::session::csrf,
        crate::handlers::auth_handler::login::login,
        crate::handlers::auth_handler::session::logout,
        crate::handlers::auth_handler::session::refresh,
        crate::handlers::auth_handler::context::context,
        crate::handlers::auth_handler::password_reset::complete_password_reset,
        crate::handlers::auth_handler::session::list_sessions,
        crate::handlers::auth_handler::session::revoke_session,
        crate::handlers::auth_handler::session::revoke_other_sessions,
        crate::handlers::auth_handler::ws_ticket::websocket_ticket,
        crate::handlers::captcha_handler::generate_captcha_handler,
        crate::handlers::captcha_handler::captcha_image_handler,
        crate::handlers::captcha_handler::verify_captcha_handler,
        crate::handlers::captcha_handler::get_captcha_config_handler,
        // 用户管理
        crate::handlers::user_handler::list,
        crate::handlers::user_handler::options,
        crate::handlers::user_handler::detail,
        crate::handlers::user_handler::create,
        crate::handlers::user_handler::update,
        crate::handlers::user_handler::remove,
        crate::handlers::user_handler::batch_remove,
        crate::handlers::user_handler::request_password_reset,
        crate::handlers::user_handler::update_status,
        crate::handlers::user_handler::replace_roles,
        crate::handlers::user_handler::request_user_export,
        crate::handlers::user_handler::download_import_template,
        crate::handlers::user_import_handler::create,
        crate::handlers::user_import_handler::list,
        crate::handlers::user_import_handler::detail,
        crate::handlers::user_import_handler::cancel,
        crate::handlers::user_import_handler::rows,
        crate::handlers::user_import_handler::report,
        crate::handlers::authorization_diagnostic_handler::diagnose_user,
        // 角色管理
        crate::handlers::role_handler::list,
        crate::handlers::role_handler::options,
        crate::handlers::role_handler::detail,
        crate::handlers::role_handler::create,
        crate::handlers::role_handler::update,
        crate::handlers::role_handler::remove,
        crate::handlers::role_handler::batch_remove,
        crate::handlers::role_handler::replace_permissions,
        crate::handlers::role_handler::get_role_perms,
        crate::handlers::role_handler::replace_data_scope,
        crate::handlers::role_handler::request_role_export,
        // 部门管理
        crate::handlers::dept_handler::tree,
        crate::handlers::dept_handler::list_page,
        crate::handlers::dept_handler::detail,
        crate::handlers::dept_handler::create,
        crate::handlers::dept_handler::update,
        crate::handlers::dept_handler::remove,
        // 岗位管理
        crate::handlers::post_handler::list,
        crate::handlers::post_handler::detail,
        crate::handlers::post_handler::create,
        crate::handlers::post_handler::update,
        crate::handlers::post_handler::remove,
        crate::handlers::post_handler::request_post_export,
        // 菜单管理
        crate::handlers::menu_handler::tree,
        crate::handlers::menu_handler::list_page,
        crate::handlers::menu_handler::detail,
        crate::handlers::menu_handler::create,
        crate::handlers::menu_handler::update,
        crate::handlers::menu_handler::remove,
        // 参数配置
        crate::handlers::config_handler::list,
        crate::handlers::config_handler::detail,
        crate::handlers::config_handler::create,
        crate::handlers::config_handler::update,
        crate::handlers::config_handler::remove,
        crate::handlers::config_handler::get_by_key,
        crate::handlers::config_handler::refresh_cache,
        crate::handlers::config_handler::request_config_export,
        // 字典管理
        crate::handlers::dict_handler::list_types,
        crate::handlers::dict_handler::create_type,
        crate::handlers::dict_handler::update_type,
        crate::handlers::dict_handler::delete_type,
        crate::handlers::dict_handler::list_data,
        crate::handlers::dict_handler::list_data_by_type_path,
        crate::handlers::dict_handler::create_data,
        crate::handlers::dict_handler::update_data,
        crate::handlers::dict_handler::delete_data,
        crate::handlers::dict_handler::request_dict_type_export,
        // 通知公告
        crate::handlers::notice_handler::list,
        crate::handlers::notice_handler::detail,
        crate::handlers::notice_handler::create,
        crate::handlers::notice_handler::update,
        crate::handlers::notice_handler::publish_to_message_center,
        crate::handlers::notice_handler::remove,
        // 消息中心
        crate::handlers::message_handler::inbox,
        crate::handlers::message_handler::unread_count,
        crate::handlers::message_handler::publish,
        crate::handlers::message_handler::acknowledge,
        crate::handlers::message_handler::delete_messages,
        crate::handlers::message_handler::mark_read,
        crate::handlers::message_handler::mark_all_read,
        // 操作日志
        crate::handlers::oper_log_handler::list,
        crate::handlers::oper_log_handler::request_oper_log_export,
        // 登录日志
        crate::handlers::login_log_handler::list,
        crate::handlers::login_log_handler::request_login_log_export,
        // 在线用户
        crate::handlers::online_user_handler::list_online_users_page,
        crate::handlers::online_user_handler::force_logout,
        // 后台任务
        crate::handlers::job_handler::list,
        crate::handlers::job_handler::stats,
        crate::handlers::job_handler::retry_dead,
        crate::handlers::overview_handler::overview,
        crate::handlers::overview_handler::trends,
        crate::handlers::retention_handler::overview,
        crate::handlers::retention_handler::preview,
        crate::handlers::retention_handler::run,
        crate::handlers::retention_handler::runs,
        crate::handlers::schedule_handler::targets,
        crate::handlers::schedule_handler::preview,
        crate::handlers::schedule_handler::list,
        crate::handlers::schedule_handler::detail,
        crate::handlers::schedule_handler::create,
        crate::handlers::schedule_handler::update,
        crate::handlers::schedule_handler::update_status,
        crate::handlers::schedule_handler::run_now,
        crate::handlers::schedule_handler::remove,
        crate::handlers::schedule_handler::executions,
        crate::handlers::export_handler::list,
        crate::handlers::export_handler::unread_notification_count,
        crate::handlers::export_handler::mark_notifications_read,
        crate::handlers::export_handler::delete_records,
        crate::handlers::export_handler::detail,
        crate::handlers::export_handler::cancel,
        crate::handlers::export_handler::download,
        // 监控、生成器、通用上传下载和导出导入
        crate::probes::livez,
        crate::probes::readyz,
        crate::monitor::metrics_handler,
        crate::monitor::server_info_handler,
        crate::monitor::cache_info_handler,
        crate::monitor::cache_commands_handler,
        crate::monitor::db_pool_handler,
        crate::router::runtime_status,
        crate::handlers::common_handler::upload_file,
        crate::handlers::common_handler::upload_image,
        crate::handlers::common_handler::upload_avatar,
        crate::handlers::common_handler::download_file,
        // 个人中心
        crate::handlers::profile_handler::get_profile,
        crate::handlers::profile_handler::update_profile,
        crate::handlers::profile_handler::change_password,
        crate::handlers::profile_handler::update_avatar,
        // 权限管理
        crate::handlers::permission_handler::tree,
        crate::handlers::permission_handler::detail,
        crate::handlers::permission_handler::create,
        crate::handlers::permission_handler::update,
        crate::handlers::permission_handler::remove,
        crate::handlers::permission_handler::sync_perm_from_route,
        // 租户管理
        crate::handlers::tenant_handler::list,
        crate::handlers::tenant_handler::page,
        crate::handlers::tenant_handler::detail,
        crate::handlers::tenant_handler::usage,
        crate::handlers::tenant_handler::create,
        crate::handlers::tenant_handler::update,
        crate::handlers::tenant_handler::update_status,
        // 产品套餐与租户能力
        crate::handlers::product_handler::capabilities,
        crate::handlers::product_handler::list_plans,
        crate::handlers::product_handler::plan_detail,
        crate::handlers::product_handler::create_plan,
        crate::handlers::product_handler::update_plan,
        crate::handlers::product_handler::list_versions,
        crate::handlers::product_handler::create_version,
        crate::handlers::product_handler::update_version,
        crate::handlers::product_handler::publish_version,
        crate::handlers::product_handler::retire_version,
        crate::handlers::product_handler::tenant_context,
        crate::handlers::product_handler::preview_tenant_change,
        crate::handlers::product_handler::apply_tenant_change,
        crate::handlers::tenant_data_handler::list_targets,
        crate::handlers::tenant_data_handler::target_detail,
        crate::handlers::tenant_data_handler::backup_points,
        crate::handlers::tenant_data_handler::placement,
        crate::handlers::tenant_data_handler::preview_migration,
        crate::handlers::tenant_data_handler::create_migration,
        crate::handlers::tenant_data_handler::list_tenant_migrations,
        crate::handlers::tenant_data_handler::migration_detail,
        crate::handlers::tenant_data_handler::cancel_migration,
        crate::handlers::tenant_data_handler::finalize_migration,
        // 租户配置包迁移
        crate::handlers::tenant_config_handler::request_package_export,
        crate::handlers::tenant_config_handler::list_packages,
        crate::handlers::tenant_config_handler::get_package,
        crate::handlers::tenant_config_handler::download_package,
        crate::handlers::tenant_config_handler::upload_transfer,
        crate::handlers::tenant_config_handler::create_transfer_from_package,
        crate::handlers::tenant_config_handler::list_transfers,
        crate::handlers::tenant_config_handler::get_transfer,
        crate::handlers::tenant_config_handler::list_transfer_items,
        crate::handlers::tenant_config_handler::request_preview,
        crate::handlers::tenant_config_handler::request_apply,
        crate::handlers::tenant_config_handler::request_rollback,
        // 服务账号与个人委托
        crate::handlers::service_account_handler::list_accounts,
        crate::handlers::service_account_handler::create_account,
        crate::handlers::service_account_handler::account_detail,
        crate::handlers::service_account_handler::update_account,
        crate::handlers::service_account_handler::update_account_status,
        crate::handlers::service_account_handler::remove_account,
        crate::handlers::service_account_handler::account_roles,
        crate::handlers::service_account_handler::replace_account_roles,
        crate::handlers::service_account_handler::list_credentials,
        crate::handlers::service_account_handler::create_credential,
        crate::handlers::service_account_handler::revoke_credential,
        crate::handlers::service_account_handler::list_delegations,
        crate::handlers::service_account_handler::revoke_delegation,
        crate::handlers::service_account_handler::list_access_audits,
        crate::handlers::service_delegation_profile_handler::list_my_delegations,
        crate::handlers::service_delegation_profile_handler::delegation_capabilities,
        crate::handlers::service_delegation_profile_handler::create_my_delegation,
        crate::handlers::service_delegation_profile_handler::revoke_my_delegation,
        // 独立 Agent API
        crate::handlers::agent_handler::capabilities,
        crate::handlers::agent_handler::users,
        crate::handlers::agent_handler::departments,
        crate::handlers::agent_handler::posts,
        crate::handlers::agent_handler::dictionary,
    ),
    components(schemas(
        // 认证 DTO
        crate::dto::auth_dto::LoginRequest,
        crate::dto::auth_dto::CompletePasswordResetRequest,
        crate::dto::auth_dto::LoginResponse,
        crate::dto::auth_dto::SessionUserVo,
        crate::dto::auth_dto::TenantBusinessDataContextVo,
        crate::dto::auth_dto::SessionContextVo,
        crate::dto::auth_dto::CsrfResponse,
        crate::dto::auth_dto::AuthSessionResponse,
        crate::dto::auth_dto::RevokeOtherSessionsResponse,
        crate::dto::empty_dto::EmptyRequestDto,
        crate::message_socket::WebSocketTicketResponse,
        crate::handlers::captcha_handler::CaptchaQuery,
        crate::handlers::captcha_handler::CaptchaResponse,
        crate::handlers::captcha_handler::CaptchaVerifyRequest,
        crate::handlers::captcha_handler::CaptchaVerifyResponse,
        crate::handlers::captcha_handler::CaptchaConfigResponse,
        crate::dto::public_dto::UserInfo,
        // 用户 DTO
        crate::dto::user_dto::CreateUserDto,
        crate::dto::user_dto::UpdateUserDto,
        crate::dto::user_dto::PasswordResetRequestDto,
        crate::dto::user_dto::PasswordResetRequestResponse,
        crate::dto::user_dto::UpdateUserStatusDto,
        crate::dto::user_dto::ReplaceUserRolesDto,
        crate::dto::user_import_dto::UserImportPageQuery,
        crate::dto::user_import_dto::UserImportRowPageQuery,
        crate::dto::user_import_dto::UserImportUploadForm,
        crate::dto::multipart_dto::FileUploadForm,
        crate::dto::public_dto::UserVo,
        crate::dto::public_dto::UserDetailVo,
        crate::dto::public_dto::RoleBriefVo,
        crate::dto::public_dto::OptionItem,
        crate::dto::public_dto::OptionList,
        crate::dto::public_dto::UserImportJobVo,
        crate::dto::public_dto::UserImportRowVo,
        crate::dto::public_dto::AuthorizationDiagnosticVo,
        crate::dto::public_dto::AuthorizationDiagnosticUserVo,
        crate::dto::public_dto::AuthorizationDiagnosticTenantVo,
        crate::dto::public_dto::AuthorizationDiagnosticRoleVo,
        crate::dto::public_dto::AuthorizationDiagnosticPermissionVo,
        crate::dto::public_dto::AuthorizationDiagnosticMenuVo,
        crate::dto::public_dto::AuthorizationDiagnosticDataScopeVo,
        crate::dto::public_dto::AuthorizationDiagnosticDepartmentVo,
        crate::dto::public_dto::AuthorizationDiagnosticDataScopeSourceVo,
        crate::dto::public_dto::AuthorizationDiagnosticVersionVo,
        crate::dto::public_dto::AuthorizationDiagnosticRefreshVo,
        // 角色 DTO
        crate::dto::role_dto::CreateRoleDto,
        crate::dto::role_dto::UpdateRoleDto,
        crate::dto::role_dto::ReplaceRolePermissionsDto,
        crate::dto::role_dto::ReplaceRoleDataScopeDto,
        crate::dto::public_dto::RoleVo,
        crate::dto::public_dto::PermissionType,
        // 部门 DTO
        crate::dto::dept_dto::CreateDeptDto,
        crate::dto::dept_dto::UpdateDeptDto,
        crate::dto::public_dto::DeptVo,
        crate::dto::public_dto::DeptTreeNode,
        // 岗位 DTO
        crate::dto::post_dto::CreatePostDto,
        crate::dto::post_dto::UpdatePostDto,
        crate::dto::public_dto::PostVo,
        // 菜单 DTO
        crate::dto::menu_dto::CreateMenuDto,
        crate::dto::menu_dto::UpdateMenuDto,
        crate::dto::permission_dto::CreatePermissionDto,
        crate::dto::permission_dto::UpdatePermissionDto,
        crate::dto::public_dto::MenuVo,
        crate::dto::public_dto::MenuTreeNode,
        crate::dto::public_dto::PermissionVo,
        crate::dto::public_dto::PermissionTreeNode,
        crate::dto::public_dto::PermissionSyncReport,
        // 参数配置 DTO
        crate::dto::config_dto::CreateConfigDto,
        crate::dto::config_dto::UpdateConfigDto,
        crate::dto::public_dto::ConfigVo,
        // 字典 DTO
        crate::dto::dict_dto::CreateDictTypeDto,
        crate::dto::dict_dto::UpdateDictTypeDto,
        crate::dto::dict_dto::CreateDictDataDto,
        crate::dto::dict_dto::UpdateDictDataDto,
        crate::dto::dict_dto::DictOptionDto,
        crate::dto::public_dto::DictTypeVo,
        crate::dto::public_dto::DictDataVo,
        // 通知 DTO
        crate::dto::notice_dto::CreateNoticeDto,
        crate::dto::notice_dto::UpdateNoticeDto,
        crate::dto::public_dto::NoticeVo,
        // 消息中心 DTO
        crate::dto::message_dto::MessageAudienceDto,
        crate::dto::message_dto::PublishMessageDto,
        crate::dto::message_dto::AcknowledgeMessagesDto,
        crate::message_presenter::MessageVo,
        crate::message_presenter::MessageInboxPage,
        crate::message_presenter::PublishedMessageVo,
        // 后台任务 DTO
        crate::dto::job_dto::BackgroundJobPageQuery,
        crate::dto::public_dto::BackgroundJobVo,
        crate::dto::public_dto::BackgroundJobQueueStats,
        crate::dto::overview_dto::OverviewTrendQuery,
        crate::dto::public_dto::MonitorOverviewVo,
        crate::dto::public_dto::MonitorOverviewDependenciesVo,
        crate::dto::public_dto::MonitorOverviewDependencyVo,
        crate::dto::public_dto::MonitorOverviewSystemVo,
        crate::dto::public_dto::MonitorOverviewDatabasePoolVo,
        crate::dto::public_dto::MonitorOverviewJobsVo,
        crate::dto::public_dto::MonitorOverviewTrendsVo,
        crate::dto::public_dto::MonitorOverviewTrendBucketVo,
        crate::dto::schedule_dto::MisfirePolicyDto,
        crate::dto::schedule_dto::ConcurrencyPolicyDto,
        crate::dto::schedule_dto::SchedulePreviewRequest,
        crate::dto::schedule_dto::CreateScheduleRequest,
        crate::dto::schedule_dto::UpdateScheduleRequest,
        crate::dto::schedule_dto::UpdateScheduleStatusRequest,
        crate::dto::schedule_dto::ScheduleVersionRequest,
        crate::dto::schedule_dto::SchedulePageQuery,
        crate::dto::schedule_dto::ScheduleExecutionPageQuery,
        crate::dto::public_dto::ScheduleTargetVo,
        crate::dto::public_dto::JobScheduleVo,
        crate::dto::public_dto::JobScheduleExecutionVo,
        crate::dto::public_dto::JobScheduleOccurrence,
        crate::dto::public_dto::JobSchedulePreview,
        crate::dto::retention_dto::RetentionRunPageQuery,
        crate::dto::public_dto::DataRetentionPolicy,
        crate::dto::public_dto::DataRetentionCutoff,
        crate::dto::public_dto::DataRetentionOverview,
        crate::dto::public_dto::DataRetentionPreview,
        crate::dto::public_dto::DataRetentionRunVo,
        // 日志 DTO
        crate::dto::oper_log_dto::OperLogPageQuery,
        crate::dto::login_log_dto::LoginLogPageQuery,
        crate::dto::public_dto::OperLogVo,
        crate::dto::public_dto::LoginInfoVo,
        crate::dto::public_dto::OnlineUserVo,
        // 个人中心 DTO
        crate::dto::profile_dto::UpdateProfileRequest,
        crate::dto::profile_dto::ChangePasswordRequest,
        crate::dto::profile_dto::AvatarResponse,
        crate::dto::public_dto::UserProfileResponse,
        crate::dto::tenant_dto::CreateTenantDto,
        crate::dto::tenant_dto::UpdateTenantDto,
        crate::dto::tenant_dto::UpdateTenantStatusDto,
        crate::dto::tenant_dto::TenantCapacityPageQuery,
        crate::dto::tenant_dto::TenantStatusFilter,
        crate::dto::tenant_dto::TenantExpirationStatusFilter,
        crate::dto::tenant_dto::TenantCapacityStatusFilter,
        crate::dto::public_dto::TenantVo,
        crate::dto::public_dto::TenantCapacityVo,
        crate::dto::public_dto::TenantUsageVo,
        crate::dto::public_dto::TenantQuotaUsageVo,
        crate::dto::public_dto::TenantRequestWindowUsageVo,
        crate::dto::public_dto::TenantAuxiliaryUsageVo,
        crate::dto::product_dto::CapabilitySnapshotDto,
        crate::dto::product_dto::CapabilityOverrideDto,
        crate::dto::product_dto::CreateProductPlanDto,
        crate::dto::product_dto::UpdateProductPlanDto,
        crate::dto::product_dto::CreateProductPlanVersionDto,
        crate::dto::product_dto::ProductChangePreviewDto,
        crate::dto::product_dto::ProductChangeApplyDto,
        crate::dto::product_dto::CapabilityVariantVo,
        crate::dto::product_dto::CapabilityCatalogVo,
        crate::dto::product_dto::ProductCapabilityVo,
        crate::dto::product_dto::ProductPlanVersionVo,
        crate::dto::product_dto::ProductPlanVo,
        crate::dto::product_dto::EffectiveCapabilityVo,
        crate::dto::product_dto::CapabilityOverrideVo,
        crate::dto::product_dto::ProductContextVo,
        crate::dto::product_dto::ProductCapabilityChangeVo,
        crate::dto::product_dto::ProductChangePreviewVo,
        crate::dto::product_dto::SessionCapabilityVo,
        crate::dto::product_dto::SessionProductContextVo,
        crate::dto::tenant_data_dto::DataTargetListQuery,
        crate::dto::tenant_data_dto::BackupPointListQuery,
        crate::dto::tenant_data_dto::MigrationPreviewDto,
        crate::dto::tenant_data_dto::CreateMigrationDto,
        crate::dto::tenant_data_dto::MigrationListQuery,
        crate::dto::tenant_data_dto::DataTargetSummary,
        crate::dto::tenant_data_dto::DataTargetDetail,
        crate::dto::tenant_data_dto::BackupPointView,
        crate::dto::tenant_data_dto::DataPlacementView,
        crate::dto::tenant_data_dto::MigrationImpact,
        crate::dto::tenant_data_dto::MigrationPreview,
        crate::dto::tenant_data_dto::MigrationItemView,
        crate::dto::tenant_data_dto::MigrationView,
        crate::dto::tenant_config_dto::TenantConfigPageQuery,
        crate::dto::tenant_config_dto::TenantConfigPackageUploadForm,
        crate::dto::tenant_config_dto::CreateTenantConfigTransferDto,
        crate::dto::tenant_config_dto::ApplyTenantConfigTransferDto,
        crate::dto::public_dto::TenantConfigBundleSummaryVo,
        crate::dto::public_dto::TenantConfigBundleVo,
        crate::dto::public_dto::TenantConfigTransferVo,
        crate::dto::public_dto::TenantConfigTransferItemVo,
        crate::dto::service_account_dto::ServiceResourcePageQuery,
        crate::dto::service_account_dto::CreateServiceAccountDto,
        crate::dto::service_account_dto::UpdateServiceAccountDto,
        crate::dto::service_account_dto::ServiceAccountStatusDto,
        crate::dto::service_account_dto::UpdateServiceAccountStatusDto,
        crate::dto::service_account_dto::ReplaceServiceAccountRolesDto,
        crate::dto::service_account_dto::CreateServiceCredentialDto,
        crate::dto::service_account_dto::CreateServiceDelegationDto,
        crate::dto::public_dto::ServiceCapabilityVo,
        crate::dto::public_dto::ServiceAccountVo,
        crate::dto::public_dto::ServiceAccountDetailVo,
        crate::dto::public_dto::ServiceCredentialVo,
        crate::dto::public_dto::CreatedServiceCredentialVo,
        crate::dto::public_dto::ServiceDelegationVo,
        crate::dto::public_dto::CreatedServiceDelegationVo,
        crate::dto::public_dto::ServiceAccessAuditVo,
        crate::handlers::service_delegation_profile_handler::ServiceDelegationTargetResponse,
        crate::dto::agent_dto::AgentPageQuery,
        crate::dto::agent_dto::AgentCapabilityResponse,
        crate::dto::agent_dto::AgentUserResponse,
        crate::dto::agent_dto::AgentDepartmentResponse,
        crate::dto::agent_dto::AgentPostResponse,
        crate::dto::agent_dto::AgentDictionaryResponse,
        crate::dto::agent_dto::AgentDictionaryItemResponse,
        crate::dto::public_dto::UploadResponse,
        crate::router::ApiVersionInfo,
        crate::router::ApiVersionEndpoints,
        crate::monitor::ServerInfo,
        crate::monitor::CacheInfo,
        crate::monitor::CacheCommandStats,
        crate::monitor::CacheCommandStatsStatus,
        crate::monitor::CacheKeysInfo,
        crate::monitor::RedisServerInfo,
        crate::monitor::RedisMemoryInfo,
        crate::monitor::DbPoolInfo,
        crate::probes::LivenessResponse,
        crate::probes::ReadinessResponse,
    )),
    modifiers(&ApiDocModifier)
)]
pub struct ApiDoc;

/// 以确定性的对象键排序渲染 OpenAPI 文档。
///
/// Utoipa 将扩展字段存入哈希映射，直接序列化 `OpenApi` 可能使原本相同的进程
/// 产生字节级差异。
pub fn render_openapi_json(
    document: &utoipa::openapi::OpenApi,
) -> Result<String, serde_json::Error> {
    let canonical = serde_json::to_value(document)?;
    Ok(format!("{}\n", serde_json::to_string_pretty(&canonical)?))
}

/// Bearer Token 安全方案
struct ApiDocModifier;

fn menu_route_contract() -> serde_json::Value {
    let routes = crate::permission_catalog::menu_routes()
        .iter()
        .map(|menu| {
            serde_json::json!({
                "route_key": menu.route_key,
                "menu_type": menu.menu_type,
                "permission_code": menu.permission_code,
                "capability_code": menu.capability_code,
            })
        })
        .collect::<Vec<_>>();

    serde_json::json!({
        "version": 1,
        "routes": routes,
    })
}

fn product_capability_contract() -> serde_json::Value {
    let capabilities = ryframe_application::system::CAPABILITY_CATALOG
        .iter()
        .map(|descriptor| {
            serde_json::json!({
                "code": descriptor.code,
                "dependencies": descriptor.dependencies,
                "conflicts": descriptor.conflicts,
                "route_keys": descriptor.route_keys,
                "permission_codes": descriptor.permission_codes,
                "default_admin_permissions": descriptor.default_admin_permissions,
                "deployment_dependencies": descriptor.deployment_dependencies,
                "deployment_available": true,
                "client_config_fields": descriptor.client_config_fields,
                "variants": descriptor.variants.iter().map(|variant| serde_json::json!({
                    "code": variant.code,
                    "schema_version": variant.schema_version,
                })).collect::<Vec<_>>(),
            })
        })
        .collect::<Vec<_>>();
    serde_json::json!({ "version": 1, "capabilities": capabilities })
}

fn password_policy_contract() -> serde_json::Value {
    serde_json::json!({
        "version": 1,
        "min_length": ryframe_auth::password::MIN_PASSWORD_LENGTH,
        "max_length": ryframe_auth::password::MAX_PASSWORD_LENGTH,
        "pattern": ryframe_auth::password::COMPLEXITY_PATTERN,
        "allowed_characters": "ascii_graphic",
        "required_classes": ["uppercase", "lowercase", "digit", "special"],
    })
}

fn notice_policy_contract() -> serde_json::Value {
    serde_json::json!({
        "version": 1,
        "content_markdown": {
            "min_utf8_bytes": crate::dto::notice_dto::NOTICE_MARKDOWN_MIN_UTF8_BYTES,
            "max_utf8_bytes": crate::dto::notice_dto::NOTICE_MARKDOWN_MAX_UTF8_BYTES,
        },
    })
}

fn api_prefix_contract() -> serde_json::Value {
    serde_json::json!({
        "version": 1,
        "value": crate::http::API_PREFIX,
    })
}

fn permission_catalog_contract() -> serde_json::Value {
    serde_json::json!({
        "version": 1,
        "codes": crate::permission_catalog::permission_codes(),
    })
}

fn route_contract() -> serde_json::Value {
    let bindings = crate::permission_catalog::route_capability_bindings();
    let mut endpoint_keys = std::collections::BTreeSet::new();
    for descriptor in ryframe_application::system::CAPABILITY_CATALOG {
        for permission in descriptor.permission_codes {
            assert!(
                bindings
                    .iter()
                    .any(|binding| binding.capability_code == descriptor.code
                        && binding.permission_code == Some(*permission)),
                "capability permission {permission} has no compiled route binding"
            );
        }
    }
    let routes = bindings
        .iter()
        .map(|binding| {
            assert!(
                binding.path == crate::http::API_PREFIX
                    || binding
                        .path
                        .starts_with(&format!("{}/", crate::http::API_PREFIX)),
                "route contract path must include the public API prefix: {}",
                binding.path
            );
            assert!(
                endpoint_keys.insert((binding.method, binding.path)),
                "route contract has duplicate method/path binding: {} {}",
                binding.method,
                binding.path
            );
            let descriptor = ryframe_application::system::CAPABILITY_CATALOG
                .iter()
                .find(|descriptor| descriptor.code == binding.capability_code)
                .expect("route capability must exist in the compiled catalog");
            if let Some(permission_code) = binding.permission_code {
                assert!(
                    descriptor.permission_codes.contains(&permission_code),
                    "route capability permission is outside its descriptor"
                );
            }
            serde_json::json!({
                "source": binding.source,
                "handler": binding.handler,
                "method": binding.method,
                "path": binding.path,
                "permission_code": binding.permission_code,
                "capability_code": binding.capability_code,
            })
        })
        .collect::<Vec<_>>();
    serde_json::json!({ "version": 1, "routes": routes })
}

fn tenant_context_header_contract() -> serde_json::Value {
    serde_json::json!({
        "version": 1,
        "headers": [
            "X-Authorization-Epoch",
            "X-Tenant-Runtime-Epoch",
            "X-Tenant-Data-Generation",
            "X-Tenant-Data-State"
        ]
    })
}

fn product_error_contract() -> serde_json::Value {
    serde_json::json!({
        "version": 1,
        "errors": [
            {"error_key": "capability_unavailable", "status": 501},
            {"error_key": "tenant_capability_denied", "status": 403},
            {"error_key": "permission_denied", "status": 403},
            {"error_key": "stale_runtime_epoch", "status": 409},
            {"error_key": "stale_placement_generation", "status": 409},
            {"error_key": "tenant_operation_conflict", "status": 409},
            {"error_key": "tenant_data_maintenance", "status": 423, "retry_after": true},
            {"error_key": "tenant_data_target_unavailable", "status": 503, "retry_after": true}
        ]
    })
}

fn agent_capability_contract() -> serde_json::Value {
    let capabilities = ryframe_application::agent::AgentCapability::ALL
        .into_iter()
        .map(|capability| {
            let descriptor = capability.descriptor();
            serde_json::json!({
                "capability_key": descriptor.key,
                "operation_id": descriptor.operation_id,
                "method": descriptor.method,
                "path": descriptor.path,
                "required_permission": descriptor.required_permission,
                "direct": descriptor.direct,
                "delegated": descriptor.delegated,
                "cost": descriptor.cost,
            })
        })
        .collect::<Vec<_>>();
    serde_json::json!({ "version": 1, "capabilities": capabilities })
}

impl utoipa::Modify for ApiDocModifier {
    fn modify(&self, openapi: &mut utoipa::openapi::OpenApi) {
        if let Some(components) = openapi.components.as_mut() {
            components.add_security_scheme(
                "bearer",
                utoipa::openapi::security::SecurityScheme::Http(
                    utoipa::openapi::security::Http::new(
                        utoipa::openapi::security::HttpAuthScheme::Bearer,
                    ),
                ),
            );
            components.add_security_scheme(
                "refreshCookie",
                utoipa::openapi::security::SecurityScheme::ApiKey(
                    utoipa::openapi::security::ApiKey::Cookie(
                        utoipa::openapi::security::ApiKeyValue::new("ryframe_refresh_token"),
                    ),
                ),
            );
            components.add_security_scheme(
                "ryframeApiKey",
                utoipa::openapi::security::SecurityScheme::ApiKey(
                    utoipa::openapi::security::ApiKey::Header(
                        utoipa::openapi::security::ApiKeyValue::with_description(
                            "Authorization",
                            "格式：`RyFrameApiKey rfk_<key_id>.<256-bit-secret>`；仅用于 `/api/v1/agent/v1/**`。",
                        ),
                    ),
                ),
            );
            components.add_security_scheme(
                "delegationToken",
                utoipa::openapi::security::SecurityScheme::ApiKey(
                    utoipa::openapi::security::ApiKey::Header(
                        utoipa::openapi::security::ApiKeyValue::with_description(
                            "X-RyFrame-Delegation",
                            "可选的本人限时委托令牌，格式：`rfd_<256-bit-secret>`。",
                        ),
                    ),
                ),
            );
        }

        openapi
            .extensions
            .get_or_insert_default()
            .insert("x-ryframe-menu-routes".into(), menu_route_contract());
        openapi.extensions.get_or_insert_default().insert(
            "x-ryframe-product-capabilities".into(),
            product_capability_contract(),
        );
        openapi.extensions.get_or_insert_default().insert(
            "x-ryframe-password-policy".into(),
            password_policy_contract(),
        );
        openapi
            .extensions
            .get_or_insert_default()
            .insert("x-ryframe-notice-policy".into(), notice_policy_contract());
        openapi
            .extensions
            .get_or_insert_default()
            .insert("x-ryframe-api-prefix".into(), api_prefix_contract());
        openapi.extensions.get_or_insert_default().insert(
            "x-ryframe-permission-catalog".into(),
            permission_catalog_contract(),
        );
        openapi
            .extensions
            .get_or_insert_default()
            .insert("x-ryframe-route-contract".into(), route_contract());
        openapi.extensions.get_or_insert_default().insert(
            "x-ryframe-tenant-context-headers".into(),
            tenant_context_header_contract(),
        );
        openapi
            .extensions
            .get_or_insert_default()
            .insert("x-ryframe-product-errors".into(), product_error_contract());
        openapi.extensions.get_or_insert_default().insert(
            "x-ryframe-agent-capabilities".into(),
            agent_capability_contract(),
        );

        for (path, item) in &mut openapi.paths.paths {
            finalize_operation(&mut item.get, "get", path);
            finalize_operation(&mut item.post, "post", path);
            finalize_operation(&mut item.put, "put", path);
            finalize_operation(&mut item.delete, "delete", path);
            finalize_operation(&mut item.patch, "patch", path);
            finalize_operation(&mut item.options, "options", path);
            finalize_operation(&mut item.head, "head", path);
            finalize_operation(&mut item.trace, "trace", path);
        }
    }
}

fn finalize_operation(
    operation: &mut Option<utoipa::openapi::path::Operation>,
    method: &str,
    path: &str,
) {
    set_operation_id(operation, method, path);
    let Some(operation) = operation else {
        return;
    };
    // utoipa 5 的 tuple 参数语法无法声明 Header required；运行时 handler 已强制
    // 提取该值，这里把生成契约同步为必填，避免客户端生成可空调用签名。
    if method == "post"
        && path == "/api/v1/platform/tenants"
        && let Some(parameter) = operation.parameters.as_mut().and_then(|parameters| {
            parameters
                .iter_mut()
                .find(|parameter| parameter.name.eq_ignore_ascii_case("Idempotency-Key"))
        })
    {
        parameter.required = utoipa::openapi::Required::True;
    }
    let bearer =
        utoipa::openapi::security::SecurityRequirement::new("bearer", std::iter::empty::<String>());
    if !operation
        .security
        .as_ref()
        .is_some_and(|requirements| requirements.contains(&bearer))
    {
        return;
    }
    for response in operation.responses.responses.values_mut() {
        let utoipa::openapi::RefOr::T(response) = response else {
            continue;
        };
        for (name, description) in [
            ("X-Authorization-Epoch", "本次响应所依据的租户授权纪元"),
            ("X-Tenant-Runtime-Epoch", "本次响应所依据的租户产品运行纪元"),
            (
                "X-Tenant-Data-Generation",
                "本次响应所依据的租户数据放置代次",
            ),
            ("X-Tenant-Data-State", "本次响应所依据的租户业务数据状态"),
        ] {
            let mut header = utoipa::openapi::header::Header::default();
            header.description = Some(description.to_owned());
            response.headers.entry(name.to_owned()).or_insert(header);
        }
    }
}

fn set_operation_id(
    operation: &mut Option<utoipa::openapi::path::Operation>,
    method: &str,
    path: &str,
) {
    let Some(operation) = operation else {
        return;
    };

    let normalized_path = path
        .strip_prefix(crate::http::API_PREFIX)
        .unwrap_or(path)
        .trim_start_matches('/')
        .split('/')
        .map(|segment| {
            segment
                .strip_prefix('{')
                .and_then(|value| value.strip_suffix('}'))
                .map_or_else(
                    || segment.replace('-', "_"),
                    |parameter| format!("by_{parameter}"),
                )
        })
        .collect::<Vec<_>>()
        .join("_");

    operation.operation_id = Some(format!("{method}_{normalized_path}"));
}

/// 获取 OpenAPI JSON 文档
pub async fn openapi_json() -> impl axum::response::IntoResponse {
    use axum::Json;
    Json(ApiDoc::openapi())
}
