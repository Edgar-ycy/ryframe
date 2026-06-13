/// 权限码常量
///
/// 此模块是所有 API 权限码的唯一定义来源。
/// Handler 路由守卫和数据库 sys_permission 表应使用相同的权限码值。
///
/// 命名规范: `MODULE_RESOURCE_ACTION`
/// 示例: `USER_LIST` → `"system:user:list"`
pub struct PermissionCode;

// ============================================================
// 用户管理 (system:user)
// ============================================================
impl PermissionCode {
    pub const USER_LIST: &'static str = "system:user:list";
    pub const USER_ADD: &'static str = "system:user:add";
    pub const USER_EDIT: &'static str = "system:user:edit";
    pub const USER_REMOVE: &'static str = "system:user:remove";
    pub const USER_EXPORT: &'static str = "system:user:export";

    // ============================================================
    // 角色管理 (system:role)
    // ============================================================
    pub const ROLE_LIST: &'static str = "system:role:list";
    pub const ROLE_ADD: &'static str = "system:role:add";
    pub const ROLE_EDIT: &'static str = "system:role:edit";
    pub const ROLE_REMOVE: &'static str = "system:role:remove";
    pub const ROLE_EXPORT: &'static str = "system:role:export";

    // ============================================================
    // 菜单管理 (system:menu)
    // ============================================================
    pub const MENU_LIST: &'static str = "system:menu:list";
    pub const MENU_ADD: &'static str = "system:menu:add";
    pub const MENU_EDIT: &'static str = "system:menu:edit";
    pub const MENU_REMOVE: &'static str = "system:menu:remove";

    // ============================================================
    // 权限管理 (system:permission)
    // ============================================================
    pub const PERMISSION_LIST: &'static str = "system:permission:list";

    // ============================================================
    // 部门管理 (system:dept)
    // ============================================================
    pub const DEPT_LIST: &'static str = "system:dept:list";
    pub const DEPT_ADD: &'static str = "system:dept:add";
    pub const DEPT_EDIT: &'static str = "system:dept:edit";
    pub const DEPT_REMOVE: &'static str = "system:dept:remove";

    // ============================================================
    // 岗位管理 (system:post)
    // ============================================================
    pub const POST_LIST: &'static str = "system:post:list";
    pub const POST_ADD: &'static str = "system:post:add";
    pub const POST_EDIT: &'static str = "system:post:edit";
    pub const POST_REMOVE: &'static str = "system:post:remove";
    pub const POST_EXPORT: &'static str = "system:post:export";

    // ============================================================
    // 参数配置 (system:config)
    // ============================================================
    pub const CONFIG_LIST: &'static str = "system:config:list";
    pub const CONFIG_ADD: &'static str = "system:config:add";
    pub const CONFIG_EDIT: &'static str = "system:config:edit";
    pub const CONFIG_REMOVE: &'static str = "system:config:remove";
    pub const CONFIG_EXPORT: &'static str = "system:config:export";

    // ============================================================
    // 字典管理 (system:dict)
    // ============================================================
    pub const DICT_LIST: &'static str = "system:dict:list";
    pub const DICT_ADD: &'static str = "system:dict:add";
    pub const DICT_EDIT: &'static str = "system:dict:edit";
    pub const DICT_REMOVE: &'static str = "system:dict:remove";
    pub const DICT_EXPORT: &'static str = "system:dict:export";

    // ============================================================
    // 通知公告 (system:notice)
    // ============================================================
    pub const NOTICE_LIST: &'static str = "system:notice:list";
    pub const NOTICE_ADD: &'static str = "system:notice:add";
    pub const NOTICE_EDIT: &'static str = "system:notice:edit";
    pub const NOTICE_REMOVE: &'static str = "system:notice:remove";

    // ============================================================
    // 操作日志 (system:operlog)
    // ============================================================
    pub const OPERLOG_LIST: &'static str = "system:operlog:list";
    pub const OPERLOG_EXPORT: &'static str = "system:operlog:export";
    pub const OPERLOG_REMOVE: &'static str = "system:operlog:remove";

    // ============================================================
    // 登录日志 (system:logininfor)
    // ============================================================
    pub const LOGININFOR_LIST: &'static str = "system:logininfor:list";
    pub const LOGININFOR_EXPORT: &'static str = "system:logininfor:export";
    pub const LOGININFOR_REMOVE: &'static str = "system:logininfor:remove";

    // ============================================================
    // 定时任务 (system:job)
    // ============================================================
    pub const JOB_LIST: &'static str = "system:job:list";
    pub const JOB_ADD: &'static str = "system:job:add";
    pub const JOB_EDIT: &'static str = "system:job:edit";
    pub const JOB_REMOVE: &'static str = "system:job:remove";

    // ============================================================
    // 在线用户 (monitor:online)
    // ============================================================
    pub const ONLINE_LIST: &'static str = "monitor:online:list";
    pub const ONLINE_FORCE_LOGOUT: &'static str = "monitor:online:force-logout";

    // ============================================================
    // 代码生成 (tools:gen)
    // ============================================================
    pub const GEN_LIST: &'static str = "tools:gen:list";
    pub const GEN_ADD: &'static str = "tools:gen:add";

    // ============================================================
    // 全部权限 (通配符)
    // ============================================================
    pub const ALL: &'static str = "*:*:*";

    // ============================================================
    // 收集所有 API 权限码（用于启动时校验 DB 一致性）
    // ============================================================
    pub fn all_api_permissions() -> &'static [&'static str] {
        &[
            Self::USER_LIST,
            Self::USER_ADD,
            Self::USER_EDIT,
            Self::USER_REMOVE,
            Self::USER_EXPORT,
            Self::ROLE_LIST,
            Self::ROLE_ADD,
            Self::ROLE_EDIT,
            Self::ROLE_REMOVE,
            Self::ROLE_EXPORT,
            Self::MENU_LIST,
            Self::MENU_ADD,
            Self::MENU_EDIT,
            Self::MENU_REMOVE,
            Self::PERMISSION_LIST,
            Self::DEPT_LIST,
            Self::DEPT_ADD,
            Self::DEPT_EDIT,
            Self::DEPT_REMOVE,
            Self::POST_LIST,
            Self::POST_ADD,
            Self::POST_EDIT,
            Self::POST_REMOVE,
            Self::POST_EXPORT,
            Self::CONFIG_LIST,
            Self::CONFIG_ADD,
            Self::CONFIG_EDIT,
            Self::CONFIG_REMOVE,
            Self::CONFIG_EXPORT,
            Self::DICT_LIST,
            Self::DICT_ADD,
            Self::DICT_EDIT,
            Self::DICT_REMOVE,
            Self::DICT_EXPORT,
            Self::NOTICE_LIST,
            Self::NOTICE_ADD,
            Self::NOTICE_EDIT,
            Self::NOTICE_REMOVE,
            Self::OPERLOG_LIST,
            Self::OPERLOG_EXPORT,
            Self::OPERLOG_REMOVE,
            Self::LOGININFOR_LIST,
            Self::LOGININFOR_EXPORT,
            Self::LOGININFOR_REMOVE,
            Self::JOB_LIST,
            Self::JOB_ADD,
            Self::JOB_EDIT,
            Self::JOB_REMOVE,
            Self::ONLINE_LIST,
            Self::ONLINE_FORCE_LOGOUT,
            Self::GEN_LIST,
            Self::GEN_ADD,
        ]
    }
}
