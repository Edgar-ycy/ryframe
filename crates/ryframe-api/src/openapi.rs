use utoipa::OpenApi;

/// RyFrame API 文档
///
/// 访问 `/swagger-ui` 查看交互式 API 文档
#[derive(OpenApi)]
#[openapi(
    info(
        title = "RyFrame API",
        version = "0.5.0",
        description = "RyFrame —— 基于 Rust + Axum 的现代化企业级后端框架",
        license(name = "MIT")
    ),
    tags(
        (name = "认证", description = "登录/登出/刷新令牌/获取当前用户"),
        (name = "用户管理", description = "用户 CRUD、导入导出、密码重置"),
        (name = "角色管理", description = "角色 CRUD、权限分配、菜单分配"),
        (name = "菜单管理", description = "菜单树 CRUD"),
        (name = "部门管理", description = "部门树 CRUD"),
        (name = "岗位管理", description = "岗位 CRUD"),
        (name = "字典管理", description = "字典类型 + 字典数据 CRUD"),
        (name = "参数配置", description = "系统参数配置 CRUD"),
        (name = "通知公告", description = "通知公告 CRUD"),
        (name = "操作日志", description = "操作日志查询/清空"),
        (name = "登录日志", description = "登录日志查询/清空"),
        (name = "定时任务", description = "定时任务 CRUD、暂停/恢复/触发"),
        (name = "在线用户", description = "在线用户列表/强退"),
        (name = "服务器监控", description = "服务器信息/健康检查"),
        (name = "代码生成", description = "根据数据库表生成代码"),
        (name = "个人中心", description = "个人信息/密码修改/头像上传"),
        (name = "通用", description = "文件上传下载")
    ),
    paths(
        // 认证接口
        crate::handlers::auth_handler::login,
        crate::handlers::auth_handler::logout,
        crate::handlers::auth_handler::refresh,
        crate::handlers::auth_handler::me,
        // 用户管理
        crate::handlers::user_handler::list,
        crate::handlers::user_handler::detail,
        crate::handlers::user_handler::create,
        crate::handlers::user_handler::update,
        crate::handlers::user_handler::remove,
        // 角色管理
        crate::handlers::role_handler::list,
        crate::handlers::role_handler::detail,
        crate::handlers::role_handler::create,
        crate::handlers::role_handler::update,
        crate::handlers::role_handler::remove,
        // 部门管理
        crate::handlers::dept_handler::tree,
        crate::handlers::dept_handler::create,
        crate::handlers::dept_handler::update,
        crate::handlers::dept_handler::remove,
        // 岗位管理
        crate::handlers::post_handler::list,
        crate::handlers::post_handler::create,
        crate::handlers::post_handler::update,
        crate::handlers::post_handler::remove,
        // 菜单管理
        crate::handlers::menu_handler::tree,
        crate::handlers::menu_handler::create,
        crate::handlers::menu_handler::update,
        crate::handlers::menu_handler::remove,
        // 参数配置
        crate::handlers::config_handler::list,
        crate::handlers::config_handler::create,
        crate::handlers::config_handler::update,
        crate::handlers::config_handler::remove,
        crate::handlers::config_handler::get_by_key,
        // 字典管理
        crate::handlers::dict_handler::list_types,
        crate::handlers::dict_handler::create_type,
        crate::handlers::dict_handler::list_data_by_type_path,
        crate::handlers::dict_handler::create_data,
        // 通知公告
        crate::handlers::notice_handler::list,
        crate::handlers::notice_handler::create,
        crate::handlers::notice_handler::remove,
        // 操作日志
        crate::handlers::oper_log_handler::list,
        crate::handlers::oper_log_handler::clean,
        // 登录日志
        crate::handlers::login_log_handler::list,
        crate::handlers::login_log_handler::clean,
        // 定时任务
        crate::handlers::job_handler::create_job,
        crate::handlers::job_handler::list_no_page,
        crate::handlers::job_handler::pause_job,
        crate::handlers::job_handler::resume_job,
        // 在线用户
        crate::handlers::online_user_handler::list_online_users,
        crate::handlers::online_user_handler::force_logout,
        // 个人中心
        crate::handlers::profile_handler::get_profile,
        crate::handlers::profile_handler::update_profile,
        crate::handlers::profile_handler::change_password,
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
        // 任务 DTO
        crate::dto::job_dto::CreateJobDto,
        crate::dto::job_dto::UpdateJobDto,
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
