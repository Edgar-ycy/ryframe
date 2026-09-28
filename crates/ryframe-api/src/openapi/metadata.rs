use utoipa::OpenApi;

/// 文档元信息只在这里维护；路径和 DTO 由各领域文档拥有。
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
        (name = "租户配置迁移", description = "导出、上传、预览、应用和回滚不含数据库 ID 与敏感凭据的租户配置包。"),
        (name = "产品能力", description = "编译期能力目录与租户有效产品上下文。"),
        (name = "产品套餐", description = "产品套餐元数据、不可变发布版本及租户产品变更。"),
        (name = "租户数据放置", description = "安全目标元数据、租户 placement 与迁移资格。"),
        (name = "租户数据迁移", description = "停写复制、校验、切换、取消与保留期清理。"),
        (name = "租户数据备份", description = "数据库平台 opaque 备份恢复点登记结果。")
    ),
    paths(crate::router::api_version),
    components(schemas(
        crate::router::ApiVersionInfo,
        crate::router::ApiVersionEndpoints
    ))
)]
struct MetadataDoc;

pub(super) fn document() -> utoipa::openapi::OpenApi {
    MetadataDoc::openapi()
}
