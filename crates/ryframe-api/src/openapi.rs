use utoipa::OpenApi;

/// RyFrame API 文档
///
/// 访问 `/swagger-ui` 查看交互式 API 文档
#[derive(OpenApi)]
#[openapi(
    info(
        title = "RyFrame API",
        version = "0.5.0",
        description = "RyFrame —— 基于 Rust + Axum 的企业级后端框架，对标若依（RuoYi-Vue）",
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
        crate::handlers::auth_handler::login,
        crate::handlers::auth_handler::logout,
        crate::handlers::auth_handler::refresh,
        crate::handlers::auth_handler::me,
    ),
    components(schemas(
        crate::dto::auth_dto::LoginRequest,
        crate::dto::auth_dto::RefreshRequest,
        crate::dto::auth_dto::LoginResponse,
        ryframe_service::UserInfo,
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
