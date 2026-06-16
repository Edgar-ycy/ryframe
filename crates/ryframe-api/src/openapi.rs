use utoipa::OpenApi;

#[allow(dead_code)]
mod extra_paths {
    #[utoipa::path(
        get,
        path = "/api/v1/monitor/health",
        tag = "服务器监控",
        responses((status = 200, description = "运行健康状态"))
    )]
    pub fn monitor_health() {}

    #[utoipa::path(
        get,
        path = "/api/v1/monitor/metrics",
        tag = "服务器监控",
        responses((status = 200, description = "Prometheus 指标文本"))
    )]
    pub fn monitor_metrics() {}

    #[utoipa::path(
        get,
        path = "/api/v1/monitor/server",
        tag = "服务器监控",
        responses((status = 200, description = "服务器 CPU、内存、磁盘信息")),
        security(("bearer" = []))
    )]
    pub fn monitor_server() {}

    #[utoipa::path(
        get,
        path = "/api/v1/monitor/cache",
        tag = "服务器监控",
        responses((status = 200, description = "缓存运行状态")),
        security(("bearer" = []))
    )]
    pub fn monitor_cache() {}

    #[utoipa::path(
        get,
        path = "/api/v1/monitor/cache/commands",
        tag = "服务器监控",
        responses((status = 200, description = "Redis 命令统计")),
        security(("bearer" = []))
    )]
    pub fn monitor_cache_commands() {}

    #[utoipa::path(
        get,
        path = "/api/v1/monitor/db-pool",
        tag = "服务器监控",
        responses((status = 200, description = "数据库连接池状态")),
        security(("bearer" = []))
    )]
    pub fn monitor_db_pool() {}

    #[utoipa::path(
        get,
        path = "/api/v1/monitor/runtime",
        tag = "服务器监控",
        responses((status = 200, description = "主应用运行时组件状态")),
        security(("bearer" = []))
    )]
    pub fn monitor_runtime() {}

    #[utoipa::path(
        get,
        path = "/api/v1/tools/gen/tables",
        tag = "代码生成",
        responses((status = 200, description = "数据库表结构列表")),
        security(("bearer" = []))
    )]
    pub fn gen_tables() {}

    #[utoipa::path(
        post,
        path = "/api/v1/tools/gen/preview",
        tag = "代码生成",
        responses((status = 200, description = "预览生成文件")),
        security(("bearer" = []))
    )]
    pub fn gen_preview() {}

    #[utoipa::path(
        post,
        path = "/api/v1/tools/gen/generate",
        tag = "代码生成",
        responses((status = 200, description = "生成代码并写入磁盘")),
        security(("bearer" = []))
    )]
    pub fn gen_generate() {}

    #[utoipa::path(
        post,
        path = "/api/v1/tools/gen/download",
        tag = "代码生成",
        responses((status = 200, description = "下载生成代码 zip")),
        security(("bearer" = []))
    )]
    pub fn gen_download() {}

    #[utoipa::path(
        post,
        path = "/api/v1/common/upload",
        tag = "通用",
        responses((status = 200, description = "鉴权文件上传")),
        security(("bearer" = []))
    )]
    pub fn common_upload() {}

    #[utoipa::path(
        post,
        path = "/api/v1/common/upload/image",
        tag = "通用",
        responses((status = 200, description = "鉴权图片上传并压缩")),
        security(("bearer" = []))
    )]
    pub fn common_upload_image() {}

    #[utoipa::path(
        post,
        path = "/api/v1/common/upload/avatar",
        tag = "通用",
        responses((status = 200, description = "鉴权头像上传")),
        security(("bearer" = []))
    )]
    pub fn common_upload_avatar() {}

    #[utoipa::path(
        get,
        path = "/api/v1/common/file/download",
        tag = "通用",
        params(
            ("path" = String, Query, description = "对象存储 key"),
            ("bucket" = Option<String>, Query, description = "对象存储 bucket")
        ),
        responses((status = 200, description = "文件下载")),
        security(("bearer" = []))
    )]
    pub fn common_file_download() {}

    #[utoipa::path(get, path = "/api/v1/system/users/export", tag = "用户管理", responses((status = 200, description = "导出用户 Excel")), security(("bearer" = [])))]
    pub fn users_export() {}

    #[utoipa::path(post, path = "/api/v1/system/users/import", tag = "用户管理", responses((status = 200, description = "导入用户 Excel")), security(("bearer" = [])))]
    pub fn users_import() {}

    #[utoipa::path(get, path = "/api/v1/system/users/import-template", tag = "用户管理", responses((status = 200, description = "下载用户导入模板")), security(("bearer" = [])))]
    pub fn users_import_template() {}

    #[utoipa::path(get, path = "/api/v1/system/roles/export", tag = "角色管理", responses((status = 200, description = "导出角色 Excel")), security(("bearer" = [])))]
    pub fn roles_export() {}

    #[utoipa::path(get, path = "/api/v1/system/posts/export", tag = "岗位管理", responses((status = 200, description = "导出岗位 Excel")), security(("bearer" = [])))]
    pub fn posts_export() {}

    #[utoipa::path(get, path = "/api/v1/system/configs/export", tag = "参数配置", responses((status = 200, description = "导出参数配置 Excel")), security(("bearer" = [])))]
    pub fn configs_export() {}

    #[utoipa::path(get, path = "/api/v1/system/dict/types/export", tag = "字典管理", responses((status = 200, description = "导出字典类型 Excel")), security(("bearer" = [])))]
    pub fn dict_types_export() {}

    #[utoipa::path(get, path = "/api/v1/system/operlogs/export", tag = "操作日志", responses((status = 200, description = "导出操作日志 Excel")), security(("bearer" = [])))]
    pub fn operlogs_export() {}

    #[utoipa::path(get, path = "/api/v1/system/loginlogs/export", tag = "登录日志", responses((status = 200, description = "导出登录日志 Excel")), security(("bearer" = [])))]
    pub fn loginlogs_export() {}
}

/// RyFrame API 文档
///
/// 访问 `/swagger-ui` 查看交互式 API 文档
#[derive(OpenApi)]
#[openapi(
    info(
        title = "RyFrame API",
        version = "0.5.0",
        description = r#"RyFrame —— 基于 Rust + Axum 的现代化企业级后端框架。

## 认证
所有受保护接口需在请求头携带 `Authorization: Bearer <access_token>`。
登录接口返回 `access_token`（短期）和 `refresh_token`（长期）。

## 响应格式
```json
{ "code": 200, "msg": "操作成功", "data": { ... } }
```
分页接口返回 `{ "code": 200, "msg": "查询成功", "rows": [...], "total": 100 }`。

## 菜单类型
菜单管理使用 `menu_type` 字段区分节点类型：
- `M`（目录）：侧边栏一级分组，无实际页面
- `C`（菜单）：可点击的页面路由
- `F`（按钮）：页面内的操作按钮，通过 `perms` 字段关联权限标识
"#,
        license(name = "MIT")
    ),
    tags(
        (name = "认证", description = "登录/登出/刷新令牌/验证码获取。登录需验证码（可通过配置关闭），支持暴力破解防护。"),
        (name = "用户管理", description = "用户 CRUD、分页查询、详情、导入导出、密码重置、状态变更。"),
        (name = "角色管理", description = "角色 CRUD、权限分配(role_permission)、菜单分配(role_menu)、数据权限设置(data_scope + sys_role_dept)。"),
        (name = "菜单管理", description = "菜单树管理（含目录M/菜单C/按钮F），支持路由参数(query)、权限标识(perms)、外链(is_frame)、缓存(is_cache)配置。"),
        (name = "权限管理", description = "权限码树查询，用于角色分配权限时展示可选权限列表。"),
        (name = "部门管理", description = "部门树 CRUD，支持祖级列表(ancestors)快速查询子部门。"),
        (name = "岗位管理", description = "岗位 CRUD，用户可关联岗位。"),
        (name = "字典管理", description = "字典类型 + 字典数据 CRUD，前端可据此渲染下拉选项。"),
        (name = "参数配置", description = "系统参数键值对 CRUD，支持按 key 精确查询。"),
        (name = "通知公告", description = "通知公告 CRUD，支持草稿/发布/关闭状态。"),
        (name = "操作日志", description = "POST/PUT/DELETE 请求自动记录，支持分页查询和批量清空。"),
        (name = "登录日志", description = "登录成功/失败记录，含 IP、浏览器、操作系统信息。"),
        (name = "在线用户", description = "查看当前在线用户列表，支持强制下线(token 加入黑名单)。"),
        (name = "服务器监控", description = "/health(健康检查) + /metrics(Prometheus) 公开；/server、/cache、/db-pool、/runtime 需认证。"),
        (name = "代码生成", description = "读取数据库表结构，生成 Entity/Repository/Service/Handler/DTO 五层代码。"),
        (name = "个人中心", description = "当前用户信息查看/修改、密码修改、头像更新（全部需认证）。"),
        (name = "通用", description = "/upload、/upload/image、/upload/avatar、/file/download 均需认证。上传链路包含魔数校验、去重、审计事件和任务投递。")
    ),
    paths(
        // 认证接口
        crate::handlers::auth_handler::login,
        crate::handlers::auth_handler::logout,
        crate::handlers::auth_handler::refresh,
        crate::handlers::auth_handler::me,
        // 用户管理
        crate::handlers::user_handler::list,
        crate::handlers::user_handler::list_no_page,
        crate::handlers::user_handler::detail,
        crate::handlers::user_handler::create,
        crate::handlers::user_handler::update,
        crate::handlers::user_handler::remove,
        crate::handlers::user_handler::batch_remove,
        crate::handlers::user_handler::reset_password,
        crate::handlers::user_handler::change_status,
        // 角色管理
        crate::handlers::role_handler::list,
        crate::handlers::role_handler::list_no_page,
        crate::handlers::role_handler::detail,
        crate::handlers::role_handler::create,
        crate::handlers::role_handler::update,
        crate::handlers::role_handler::remove,
        crate::handlers::role_handler::batch_remove,
        crate::handlers::role_handler::assign_permissions,
        crate::handlers::role_handler::assign_menus,
        crate::handlers::role_handler::assign_data_scope,
        // 部门管理
        crate::handlers::dept_handler::tree,
        crate::handlers::dept_handler::list_page,
        crate::handlers::dept_handler::list_no_page,
        crate::handlers::dept_handler::detail,
        crate::handlers::dept_handler::create,
        crate::handlers::dept_handler::update,
        crate::handlers::dept_handler::remove,
        // 岗位管理
        crate::handlers::post_handler::list,
        crate::handlers::post_handler::list_no_page,
        crate::handlers::post_handler::detail,
        crate::handlers::post_handler::create,
        crate::handlers::post_handler::update,
        crate::handlers::post_handler::remove,
        // 菜单管理
        crate::handlers::menu_handler::tree,
        crate::handlers::menu_handler::list_page,
        crate::handlers::menu_handler::list_no_page,
        crate::handlers::menu_handler::detail,
        crate::handlers::menu_handler::create,
        crate::handlers::menu_handler::update,
        crate::handlers::menu_handler::remove,
        // 参数配置
        crate::handlers::config_handler::list,
        crate::handlers::config_handler::list_no_page,
        crate::handlers::config_handler::detail,
        crate::handlers::config_handler::create,
        crate::handlers::config_handler::update,
        crate::handlers::config_handler::remove,
        crate::handlers::config_handler::get_by_key,
        // 字典管理
        crate::handlers::dict_handler::list_types,
        crate::handlers::dict_handler::list_types_no_page,
        crate::handlers::dict_handler::create_type,
        crate::handlers::dict_handler::update_type,
        crate::handlers::dict_handler::delete_type,
        crate::handlers::dict_handler::list_data_by_type_path,
        crate::handlers::dict_handler::create_data,
        crate::handlers::dict_handler::update_data,
        crate::handlers::dict_handler::delete_data,
        // 通知公告
        crate::handlers::notice_handler::list,
        crate::handlers::notice_handler::list_no_page,
        crate::handlers::notice_handler::detail,
        crate::handlers::notice_handler::create,
        crate::handlers::notice_handler::update,
        crate::handlers::notice_handler::remove,
        // 操作日志
        crate::handlers::oper_log_handler::list,
        crate::handlers::oper_log_handler::clean,
        // 登录日志
        crate::handlers::login_log_handler::list,
        crate::handlers::login_log_handler::clean,
        // 在线用户
        crate::handlers::online_user_handler::list_online_users,
        crate::handlers::online_user_handler::list_online_users_page,
        crate::handlers::online_user_handler::force_logout,
        // 监控、生成器、通用上传下载和导出导入
        extra_paths::monitor_health,
        extra_paths::monitor_metrics,
        extra_paths::monitor_server,
        extra_paths::monitor_cache,
        extra_paths::monitor_cache_commands,
        extra_paths::monitor_db_pool,
        extra_paths::monitor_runtime,
        extra_paths::gen_tables,
        extra_paths::gen_preview,
        extra_paths::gen_generate,
        extra_paths::gen_download,
        extra_paths::common_upload,
        extra_paths::common_upload_image,
        extra_paths::common_upload_avatar,
        extra_paths::common_file_download,
        extra_paths::users_export,
        extra_paths::users_import,
        extra_paths::users_import_template,
        extra_paths::roles_export,
        extra_paths::posts_export,
        extra_paths::configs_export,
        extra_paths::dict_types_export,
        extra_paths::operlogs_export,
        extra_paths::loginlogs_export,
        // 个人中心
        crate::handlers::profile_handler::get_profile,
        crate::handlers::profile_handler::update_profile,
        crate::handlers::profile_handler::change_password,
        crate::handlers::profile_handler::update_avatar,
        // 权限管理
        crate::handlers::permission_handler::tree,
    ),
    components(schemas(
        // 认证 DTO
        crate::dto::auth_dto::LoginRequest,
        crate::dto::auth_dto::RefreshRequest,
        crate::dto::auth_dto::LoginResponse,
        ryframe_service::UserInfo,
        // 用户 DTO
        crate::dto::user_dto::CreateUserDto,
        crate::dto::user_dto::UpdateUserDto,
        crate::dto::user_dto::ResetPasswordDto,
        crate::dto::user_dto::ChangeStatusDto,
        // 角色 DTO
        crate::dto::role_dto::CreateRoleDto,
        crate::dto::role_dto::UpdateRoleDto,
        crate::dto::role_dto::AssignPermsDto,
        crate::dto::role_dto::AssignMenusDto,
        crate::dto::role_dto::AssignDataScopeDto,
        // 部门 DTO
        crate::dto::dept_dto::CreateDeptDto,
        crate::dto::dept_dto::UpdateDeptDto,
        // 岗位 DTO
        crate::dto::post_dto::CreatePostDto,
        crate::dto::post_dto::UpdatePostDto,
        // 菜单 DTO
        crate::dto::menu_dto::CreateMenuDto,
        crate::dto::menu_dto::UpdateMenuDto,
        // 参数配置 DTO
        crate::dto::config_dto::CreateConfigDto,
        crate::dto::config_dto::UpdateConfigDto,
        // 字典 DTO
        crate::dto::dict_dto::CreateDictTypeDto,
        crate::dto::dict_dto::UpdateDictTypeDto,
        crate::dto::dict_dto::CreateDictDataDto,
        crate::dto::dict_dto::UpdateDictDataDto,
        // 通知 DTO
        crate::dto::notice_dto::CreateNoticeDto,
        crate::dto::notice_dto::UpdateNoticeDto,
        // 日志 DTO
        crate::dto::oper_log_dto::OperLogPageQuery,
        crate::dto::login_log_dto::LoginLogPageQuery,
        // 个人中心 DTO
        crate::dto::profile_dto::UpdateProfileRequest,
        crate::dto::profile_dto::ChangePasswordRequest,
    )),
    modifiers(&SecurityAddon)
)]
pub struct ApiDoc;

/// Bearer Token 安全方案
struct SecurityAddon;

impl utoipa::Modify for SecurityAddon {
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
        }
    }
}

/// 获取 OpenAPI JSON 文档
pub async fn openapi_json() -> impl axum::response::IntoResponse {
    use axum::Json;
    Json(ApiDoc::openapi())
}
