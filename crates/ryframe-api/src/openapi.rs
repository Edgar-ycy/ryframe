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
- `F`（按钮）：页面内的操作按钮，显示与授权由独立权限码控制
"#,
        license(name = "MIT")
    ),
    tags(
        (name = "认证", description = "登录/登出/刷新令牌/验证码获取。登录需验证码（可通过配置关闭），支持暴力破解防护。"),
        (name = "用户管理", description = "用户 CRUD、分页查询、详情、导入导出、密码重置请求、状态变更。"),
        (name = "角色管理", description = "角色 CRUD、权限分配(role_permission)、数据权限设置(data_scope + sys_role_dept)。"),
        (name = "菜单管理", description = "菜单树管理（含目录M/菜单C/按钮F）。管理端只允许维护上级菜单、名称、图标、排序、可见和状态。"),
        (name = "权限管理", description = "权限码树查询，用于角色分配权限时展示可选权限列表。"),
        (name = "部门管理", description = "部门树 CRUD，支持祖级列表(ancestors)快速查询子部门。"),
        (name = "岗位管理", description = "岗位 CRUD，用户可关联岗位。"),
        (name = "字典管理", description = "字典类型 + 字典数据 CRUD，前端可据此渲染下拉选项。"),
        (name = "参数配置", description = "系统参数键值对 CRUD，支持按 key 精确查询。"),
        (name = "通知公告", description = "通知公告 CRUD，支持草稿/发布/关闭状态。"),
        (name = "操作日志", description = "POST/PUT/DELETE 请求自动记录，支持分页查询、详情和导出；业务管理端不提供清空入口。"),
        (name = "登录日志", description = "登录成功/失败记录，含 IP、浏览器、操作系统信息。"),
        (name = "在线用户", description = "查看当前在线用户列表，支持强制下线(token 加入黑名单)。"),
        (name = "服务器监控", description = "/health(健康检查) + /metrics(Prometheus) 公开；/server、/cache、/db-pool、/runtime 需认证。"),
        (name = "代码生成", description = "读取数据库表结构，生成 Entity/Repository/Service/Handler/DTO 五层代码。"),
        (name = "个人中心", description = "当前用户信息查看/修改、密码修改、头像更新（全部需认证）。"),
        (name = "通用", description = "/upload、/upload/image、/upload/avatar、/file/download 均需认证。上传链路包含魔数校验、去重和熔断保护。"),
        (name = "租户管理", description = "系统租户管理租户生命周期、配额和管理员初始化。")
    ),
    paths(
        // 认证接口
        crate::handlers::auth_handler::login,
        crate::handlers::auth_handler::logout,
        crate::handlers::auth_handler::refresh,
        crate::handlers::auth_handler::complete_password_reset,
        crate::handlers::auth_handler::me,
        crate::handlers::captcha_handler::generate_captcha_handler,
        crate::handlers::captcha_handler::captcha_image_handler,
        crate::handlers::captcha_handler::verify_captcha_handler,
        crate::handlers::captcha_handler::get_captcha_config_handler,
        // 用户管理
        crate::handlers::user_handler::list,
        crate::handlers::user_handler::list_no_page,
        crate::handlers::user_handler::detail,
        crate::handlers::user_handler::create,
        crate::handlers::user_handler::update,
        crate::handlers::user_handler::remove,
        crate::handlers::user_handler::batch_remove,
        crate::handlers::user_handler::request_password_reset,
        crate::handlers::user_handler::update_status,
        crate::handlers::user_handler::replace_roles,
        crate::handlers::user_handler::export_users,
        crate::handlers::user_handler::import_users,
        crate::handlers::user_handler::download_import_template,
        // 角色管理
        crate::handlers::role_handler::list,
        crate::handlers::role_handler::list_no_page,
        crate::handlers::role_handler::detail,
        crate::handlers::role_handler::create,
        crate::handlers::role_handler::update,
        crate::handlers::role_handler::remove,
        crate::handlers::role_handler::batch_remove,
        crate::handlers::role_handler::replace_permissions,
        crate::handlers::role_handler::get_role_perms,
        crate::handlers::role_handler::replace_data_scope,
        crate::handlers::role_handler::export_roles,
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
        crate::handlers::post_handler::export_posts,
        // 菜单管理
        crate::handlers::menu_handler::tree,
        crate::handlers::menu_handler::user_tree,
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
        crate::handlers::config_handler::refresh_cache,
        crate::handlers::config_handler::export_configs,
        // 字典管理
        crate::handlers::dict_handler::list_types,
        crate::handlers::dict_handler::list_types_no_page,
        crate::handlers::dict_handler::create_type,
        crate::handlers::dict_handler::update_type,
        crate::handlers::dict_handler::delete_type,
        crate::handlers::dict_handler::list_data,
        crate::handlers::dict_handler::list_data_by_type_path,
        crate::handlers::dict_handler::create_data,
        crate::handlers::dict_handler::update_data,
        crate::handlers::dict_handler::delete_data,
        crate::handlers::dict_handler::export_dict_types,
        // 通知公告
        crate::handlers::notice_handler::list,
        crate::handlers::notice_handler::list_no_page,
        crate::handlers::notice_handler::detail,
        crate::handlers::notice_handler::create,
        crate::handlers::notice_handler::update,
        crate::handlers::notice_handler::remove,
        // 操作日志
        crate::handlers::oper_log_handler::list,
        crate::handlers::oper_log_handler::list_no_page,
        crate::handlers::oper_log_handler::export_oper_logs,
        // 登录日志
        crate::handlers::login_log_handler::list,
        crate::handlers::login_log_handler::list_no_page,
        crate::handlers::login_log_handler::export_login_logs,
        // 在线用户
        crate::handlers::online_user_handler::list_online_users,
        crate::handlers::online_user_handler::list_online_users_page,
        crate::handlers::online_user_handler::force_logout,
        // 监控、生成器、通用上传下载和导出导入
        ryframe_monitor::health_check_handler,
        ryframe_monitor::metrics_handler,
        ryframe_monitor::server_info_handler,
        ryframe_monitor::cache_info_handler,
        ryframe_monitor::cache_commands_handler,
        ryframe_monitor::db_pool_handler,
        crate::router::runtime_status,
        crate::handlers::generator_handler::list_tables,
        crate::handlers::generator_handler::preview,
        crate::handlers::generator_handler::generate,
        crate::handlers::generator_handler::download,
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
        crate::handlers::tenant_handler::create,
        crate::handlers::tenant_handler::update,
        crate::handlers::tenant_handler::update_status,
    ),
    components(schemas(
        // 认证 DTO
        crate::dto::auth_dto::LoginRequest,
        crate::dto::auth_dto::RefreshRequest,
        crate::dto::auth_dto::CompletePasswordResetRequest,
        crate::dto::auth_dto::LoginResponse,
        crate::handlers::captcha_handler::CaptchaQuery,
        crate::handlers::captcha_handler::CaptchaResponse,
        crate::handlers::captcha_handler::CaptchaVerifyRequest,
        crate::handlers::captcha_handler::CaptchaVerifyResponse,
        crate::handlers::captcha_handler::CaptchaConfigResponse,
        ryframe_service::UserInfo,
        // 用户 DTO
        crate::dto::user_dto::CreateUserDto,
        crate::dto::user_dto::UpdateUserDto,
        crate::dto::user_dto::PasswordResetRequestDto,
        crate::dto::user_dto::PasswordResetRequestResponse,
        crate::dto::user_dto::UpdateUserStatusDto,
        crate::dto::user_dto::ReplaceUserRolesDto,
        crate::dto::user_import_dto::UserImportResult,
        crate::dto::multipart_dto::FileUploadForm,
        ryframe_service::system::UserVo,
        ryframe_service::system::UserDetailVo,
        ryframe_service::system::RoleBriefVo,
        // 角色 DTO
        crate::dto::role_dto::CreateRoleDto,
        crate::dto::role_dto::UpdateRoleDto,
        crate::dto::role_dto::ReplaceRolePermissionsDto,
        crate::dto::role_dto::ReplaceRoleDataScopeDto,
        ryframe_service::system::RoleVo,
        ryframe_service::system::PermissionType,
        // 部门 DTO
        crate::dto::dept_dto::CreateDeptDto,
        crate::dto::dept_dto::UpdateDeptDto,
        ryframe_service::system::DeptVo,
        ryframe_service::system::DeptTreeNode,
        // 岗位 DTO
        crate::dto::post_dto::CreatePostDto,
        crate::dto::post_dto::UpdatePostDto,
        ryframe_service::system::PostVo,
        // 菜单 DTO
        crate::dto::menu_dto::CreateMenuDto,
        crate::dto::menu_dto::UpdateMenuDto,
        crate::dto::permission_dto::CreatePermissionDto,
        crate::dto::permission_dto::UpdatePermissionDto,
        ryframe_service::system::MenuVo,
        ryframe_service::system::MenuTreeNode,
        ryframe_service::system::PermissionVo,
        ryframe_service::system::PermissionTreeNode,
        ryframe_service::system::PermissionSyncReport,
        // 参数配置 DTO
        crate::dto::config_dto::CreateConfigDto,
        crate::dto::config_dto::UpdateConfigDto,
        ryframe_service::system::ConfigVo,
        // 字典 DTO
        crate::dto::dict_dto::CreateDictTypeDto,
        crate::dto::dict_dto::UpdateDictTypeDto,
        crate::dto::dict_dto::CreateDictDataDto,
        crate::dto::dict_dto::UpdateDictDataDto,
        crate::dto::dict_dto::DictOptionDto,
        ryframe_service::system::DictTypeVo,
        ryframe_service::system::DictDataVo,
        // 通知 DTO
        crate::dto::notice_dto::CreateNoticeDto,
        crate::dto::notice_dto::UpdateNoticeDto,
        ryframe_service::system::NoticeVo,
        // 日志 DTO
        crate::dto::oper_log_dto::OperLogPageQuery,
        crate::dto::login_log_dto::LoginLogPageQuery,
        ryframe_service::system::OperLogVo,
        ryframe_service::system::LoginInfoVo,
        ryframe_service::system::OnlineUserVo,
        // 个人中心 DTO
        crate::dto::profile_dto::UpdateProfileRequest,
        crate::dto::profile_dto::ChangePasswordRequest,
        crate::dto::profile_dto::AvatarResponse,
        ryframe_service::system::profile_service::UserProfileResponse,
        crate::dto::generator_dto::GenerateOptionsDto,
        crate::dto::generator_dto::GenerateRequestDto,
        ryframe_service::system::generator_service::TableInfo,
        ryframe_service::system::generator_service::ColumnInfo,
        ryframe_service::system::generator_service::GeneratedFile,
        ryframe_service::system::generator_service::WriteReport,
        crate::dto::tenant_dto::CreateTenantDto,
        crate::dto::tenant_dto::UpdateTenantDto,
        crate::dto::tenant_dto::UpdateTenantStatusDto,
        ryframe_service::system::TenantVo,
        ryframe_service::system::UploadResponse,
        ryframe_monitor::ServerInfo,
        ryframe_monitor::HealthInfo,
        ryframe_monitor::CacheInfo,
        ryframe_monitor::CacheKeysInfo,
        ryframe_monitor::RedisServerInfo,
        ryframe_monitor::RedisMemoryInfo,
        ryframe_monitor::DbPoolInfo,
    )),
    modifiers(&ApiDocModifier)
)]
pub struct ApiDoc;

/// Render the OpenAPI document with deterministic object-key ordering.
///
/// Utoipa stores extensions in a hash map, so serializing `OpenApi` directly
/// can produce byte-wise differences between otherwise identical processes.
pub fn render_openapi_json(
    document: &utoipa::openapi::OpenApi,
) -> Result<String, serde_json::Error> {
    let canonical = serde_json::to_value(document)?;
    Ok(format!("{}\n", serde_json::to_string_pretty(&canonical)?))
}

/// Bearer Token 安全方案
struct ApiDocModifier;

const DEFAULT_MENU_ROUTES: &[(&str, &str)] = &[
    ("home", "C"),
    ("system", "M"),
    ("monitor", "M"),
    ("tools", "M"),
    ("system.user", "C"),
    ("system.role", "C"),
    ("system.menu", "C"),
    ("system.dept", "C"),
    ("system.post", "C"),
    ("system.dict", "C"),
    ("system.config", "C"),
    ("system.notice", "C"),
    ("system.perm", "C"),
    ("system.operlog", "C"),
    ("system.logininfor", "C"),
    ("monitor.online", "C"),
    ("monitor.server", "C"),
    ("monitor.runtime", "C"),
    ("monitor.cache", "C"),
    ("monitor.db-pool", "C"),
    ("tools.gen", "C"),
];

fn menu_route_contract() -> serde_json::Value {
    let routes = DEFAULT_MENU_ROUTES
        .iter()
        .map(|(route_key, menu_type)| {
            serde_json::json!({
                "route_key": route_key,
                "menu_type": menu_type,
            })
        })
        .collect::<Vec<_>>();

    serde_json::json!({
        "version": 1,
        "routes": routes,
    })
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
        }

        openapi
            .extensions
            .get_or_insert_default()
            .insert("x-ryframe-menu-routes".into(), menu_route_contract());
        openapi.extensions.get_or_insert_default().insert(
            "x-ryframe-password-policy".into(),
            password_policy_contract(),
        );

        for (path, item) in &mut openapi.paths.paths {
            set_operation_id(&mut item.get, "get", path);
            set_operation_id(&mut item.post, "post", path);
            set_operation_id(&mut item.put, "put", path);
            set_operation_id(&mut item.delete, "delete", path);
            set_operation_id(&mut item.patch, "patch", path);
            set_operation_id(&mut item.options, "options", path);
            set_operation_id(&mut item.head, "head", path);
            set_operation_id(&mut item.trace, "trace", path);
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
        .trim_start_matches('/')
        .split('/')
        .skip_while(|segment| matches!(*segment, "api" | "v1"))
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

#[cfg(test)]
mod tests {
    use std::collections::{HashMap, HashSet};

    use super::*;

    #[test]
    fn operations_are_unique_and_use_canonical_paths() {
        let document = serde_json::to_value(ApiDoc::openapi()).unwrap();
        let paths = document["paths"].as_object().unwrap();
        let schemas = document["components"]["schemas"].as_object().unwrap();
        let mut operation_ids = HashMap::new();
        let mut query_operation_count = 0;

        assert!(
            paths.len() >= 89,
            "OpenAPI path coverage unexpectedly shrank: found {}",
            paths.len()
        );
        assert!(
            schemas.len() >= 150,
            "OpenAPI schema coverage unexpectedly shrank: found {}",
            schemas.len()
        );

        for required_path in [
            "/api/v1/system/users/{id}/roles",
            "/api/v1/system/users/{id}/status",
            "/api/v1/system/roles/{id}/permissions",
            "/api/v1/system/roles/{id}/data-scope",
        ] {
            assert!(
                paths.contains_key(required_path),
                "required OpenAPI path is missing: {required_path}"
            );
        }
        for legacy_path in [
            concat!("/api/v1/system/users/assign-", "role"),
            "/api/v1/system/users/status",
            concat!("/api/v1/system/roles/assign-", "perm"),
            concat!("/api/v1/system/roles/assign-", "dept"),
            concat!("/api/v1/system/roles/update-", "data-scope"),
        ] {
            assert!(
                !paths.contains_key(legacy_path),
                "legacy OpenAPI path is still registered: {legacy_path}"
            );
        }
        for (path, item) in paths {
            assert!(
                path.starts_with("/api/v1/"),
                "unversioned OpenAPI path: {path}"
            );
            assert!(!path.contains("listNoPage"), "legacy OpenAPI path: {path}");
            assert!(
                !path.contains("changeStatus"),
                "legacy OpenAPI path: {path}"
            );
            assert!(!path.contains("configKey"), "legacy OpenAPI path: {path}");

            for method in ["get", "post", "put", "delete", "patch"] {
                let Some(operation) = item.get(method) else {
                    continue;
                };
                let operation_id = operation["operationId"]
                    .as_str()
                    .expect("every operation must have an operationId");
                if let Some(previous) = operation_ids.insert(operation_id, path) {
                    panic!("duplicate operationId '{operation_id}' for {previous} and {path}");
                }
                if operation["parameters"]
                    .as_array()
                    .is_some_and(|parameters| {
                        parameters
                            .iter()
                            .any(|parameter| parameter["in"] == "query")
                    })
                {
                    query_operation_count += 1;
                }
                if path.ends_with("/all") || path.ends_with("/export") {
                    let documents_pagination = operation["parameters"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .any(|parameter| {
                            parameter["in"] == "query"
                                && matches!(parameter["name"].as_str(), Some("page" | "page_size"))
                        });
                    assert!(
                        !documents_pagination,
                        "full-record operation must not document pagination parameters: \
                         {method} {path}"
                    );
                }

                let success_has_schema = operation["responses"]
                    .as_object()
                    .into_iter()
                    .flatten()
                    .filter(|(status, _)| status.starts_with('2'))
                    .any(|(_, response)| {
                        response["content"]
                            .as_object()
                            .is_some_and(|content| !content.is_empty())
                    });
                assert!(
                    success_has_schema,
                    "{method} {path} must document its success response schema"
                );

                let allows_empty_request = matches!(
                    (method, path.as_str()),
                    ("post", "/api/v1/auth/logout") | ("post", "/api/v1/system/perms/sync")
                );
                if matches!(method, "post" | "put" | "patch") && !allows_empty_request {
                    assert!(
                        operation.get("requestBody").is_some(),
                        "{method} {path} must document its request body"
                    );
                }
            }
        }
        assert!(
            query_operation_count >= 34,
            "OpenAPI query parameter coverage unexpectedly shrank: found {query_operation_count}"
        );
    }

    #[test]
    fn default_menu_routes_are_exported_as_a_stable_contract() {
        let document = serde_json::to_value(ApiDoc::openapi()).unwrap();
        let contract = &document["x-ryframe-menu-routes"];
        assert_eq!(contract, &menu_route_contract());

        let routes = contract["routes"].as_array().unwrap();
        assert_eq!(routes.len(), 21);
        let mut route_keys = HashSet::new();
        for route in routes {
            let route_key = route["route_key"].as_str().unwrap();
            let menu_type = route["menu_type"].as_str().unwrap();
            assert!(
                route_keys.insert(route_key),
                "duplicate route_key: {route_key}"
            );
            assert!(matches!(menu_type, "M" | "C"));
        }
    }

    #[test]
    fn new_password_inputs_use_the_exported_policy() {
        let document = serde_json::to_value(ApiDoc::openapi()).unwrap();
        assert_eq!(
            &document["x-ryframe-password-policy"],
            &password_policy_contract()
        );

        for (schema, field) in [
            ("ChangePasswordRequest", "new_password"),
            ("CompletePasswordResetRequest", "new_password"),
            ("CreateTenantDto", "admin_password"),
        ] {
            let property = &document["components"]["schemas"][schema]["properties"][field];
            assert_eq!(property["minLength"], 8, "{schema}.{field}");
            assert_eq!(property["maxLength"], 72, "{schema}.{field}");
            assert_eq!(
                property["pattern"],
                ryframe_auth::password::COMPLEXITY_PATTERN,
                "{schema}.{field}"
            );
        }
    }

    #[test]
    fn checked_in_contract_snapshot_is_current() {
        let actual = render_openapi_json(&ApiDoc::openapi()).unwrap();
        let expected = include_str!("../../../openapi/openapi.json");
        assert_eq!(
            actual, expected,
            "run `cargo run -p ryframe-api --bin export_openapi -- openapi/openapi.json`"
        );
    }
}
