use ryframe_kernel::{AppError, AppResult};
use serde_json::Value;

pub type CapabilityConfigValidator = fn(&Value) -> AppResult<()>;

#[derive(Clone, Copy)]
pub struct CapabilityVariantDescriptor {
    pub code: &'static str,
    pub schema_version: i32,
    pub validate: CapabilityConfigValidator,
}

#[derive(Clone, Copy)]
pub struct CapabilityDescriptor {
    pub code: &'static str,
    pub name: &'static str,
    pub description: &'static str,
    pub affects_authorization: bool,
    pub dependencies: &'static [&'static str],
    pub conflicts: &'static [&'static str],
    pub route_keys: &'static [&'static str],
    pub permission_codes: &'static [&'static str],
    pub default_admin_permissions: &'static [&'static str],
    pub deployment_dependencies: &'static [&'static str],
    pub client_config_fields: &'static [&'static str],
    pub variants: &'static [CapabilityVariantDescriptor],
}

fn validate_empty_config(config: &Value) -> AppResult<()> {
    if config.as_object().is_some_and(|object| object.is_empty()) {
        Ok(())
    } else {
        Err(AppError::Validation("能力配置必须是空对象".into()))
    }
}

const STANDARD_VARIANT: CapabilityVariantDescriptor = CapabilityVariantDescriptor {
    code: "standard",
    schema_version: 1,
    validate: validate_empty_config,
};

/// 编译期能力目录。能力必须同时在前端 manifest、访问目录和资源路由中闭合。
const fn standard_capability(
    code: &'static str,
    name: &'static str,
    routes: &'static [&'static str],
    permissions: &'static [&'static str],
    deployment_dependencies: &'static [&'static str],
) -> CapabilityDescriptor {
    CapabilityDescriptor {
        code,
        name,
        description: name,
        affects_authorization: true,
        dependencies: &[],
        conflicts: &[],
        route_keys: routes,
        permission_codes: permissions,
        default_admin_permissions: permissions,
        deployment_dependencies,
        client_config_fields: &[],
        variants: &[STANDARD_VARIANT],
    }
}

pub const CAPABILITY_CATALOG: &[CapabilityDescriptor] = &[
    standard_capability(
        "system.post",
        "岗位管理",
        &["system.post"],
        &[
            "system:post:add",
            "system:post:edit",
            "system:post:export",
            "system:post:list",
            "system:post:remove",
        ],
        &[],
    ),
    standard_capability(
        "system.user",
        "用户管理",
        &["system.user"],
        &[
            "system:user-import:add",
            "system:user-import:cancel",
            "system:user-import:list",
            "system:user:add",
            "system:user:edit",
            "system:user:export",
            "system:user:list",
            "system:user:remove",
        ],
        &[],
    ),
    standard_capability(
        "system.role",
        "角色管理",
        &["system.role"],
        &[
            "system:role:add",
            "system:role:edit",
            "system:role:export",
            "system:role:list",
            "system:role:remove",
        ],
        &[],
    ),
    standard_capability(
        "system.menu",
        "菜单管理",
        &["system.menu"],
        &[
            "system:menu:add",
            "system:menu:edit",
            "system:menu:list",
            "system:menu:remove",
        ],
        &[],
    ),
    standard_capability(
        "system.dept",
        "部门管理",
        &["system.dept"],
        &[
            "system:dept:add",
            "system:dept:edit",
            "system:dept:list",
            "system:dept:remove",
        ],
        &[],
    ),
    standard_capability(
        "system.dict",
        "字典管理",
        &["system.dict"],
        &[
            "system:dict:add",
            "system:dict:edit",
            "system:dict:export",
            "system:dict:list",
            "system:dict:remove",
        ],
        &[],
    ),
    standard_capability(
        "system.config",
        "参数设置",
        &["system.config"],
        &[
            "system:config:add",
            "system:config:edit",
            "system:config:export",
            "system:config:list",
            "system:config:remove",
        ],
        &[],
    ),
    standard_capability(
        "system.perm",
        "权限管理",
        &["system.perm"],
        &[
            "system:perm:add",
            "system:perm:edit",
            "system:perm:list",
            "system:perm:remove",
            "system:perm:sync",
        ],
        &[],
    ),
    standard_capability(
        "system.authorization-diagnostics",
        "权限诊断",
        &["system.authorization-diagnostics"],
        &["system:authorization-diagnostic:list"],
        &[],
    ),
    standard_capability(
        "system.operlog",
        "操作日志",
        &["system.operlog"],
        &["system:operlog:export", "system:operlog:list"],
        &[],
    ),
    standard_capability(
        "system.logininfor",
        "登录日志",
        &["system.logininfor"],
        &["system:logininfor:export", "system:logininfor:list"],
        &[],
    ),
    standard_capability(
        "system.message",
        "消息发布",
        &[],
        &["system:message:publish"],
        &["messaging"],
    ),
    standard_capability(
        "monitor.online",
        "在线会话",
        &["monitor.online"],
        &["monitor:online:force-logout", "monitor:online:list"],
        &["redis"],
    ),
    standard_capability(
        "monitor.jobs",
        "后台任务",
        &["monitor.jobs"],
        &["monitor:job:list", "monitor:job:retry"],
        &[],
    ),
    standard_capability(
        "monitor.schedules",
        "定时任务",
        &["monitor.schedules"],
        &[
            "monitor:schedule:add",
            "monitor:schedule:edit",
            "monitor:schedule:list",
            "monitor:schedule:remove",
            "monitor:schedule:run",
        ],
        &["scheduler"],
    ),
    standard_capability(
        "system.notice",
        "通知公告",
        &["system.notice"],
        &[
            "system:notice:add",
            "system:notice:edit",
            "system:notice:list",
            "system:notice:remove",
        ],
        &[],
    ),
];

pub fn capability_descriptor(code: &str) -> AppResult<&'static CapabilityDescriptor> {
    CAPABILITY_CATALOG
        .iter()
        .find(|descriptor| descriptor.code == code)
        .ok_or_else(|| AppError::Config(format!("数据库引用了未编译的能力代码 {code}")))
}

/// 校验完整能力快照。`config` 是所选 schema 的完整值，不执行深合并。
pub fn validate_capability_snapshot(
    code: &str,
    variant_code: &str,
    schema_version: i32,
    config: &Value,
) -> AppResult<&'static CapabilityDescriptor> {
    let descriptor = capability_descriptor(code)?;
    if !descriptor
        .variants
        .iter()
        .any(|variant| variant.code == variant_code && variant.schema_version == schema_version)
    {
        return Err(AppError::Validation(format!(
            "能力 {code} 不支持变体 {variant_code} 的 schema v{schema_version}"
        )));
    }
    (descriptor
        .variants
        .iter()
        .find(|variant| variant.code == variant_code && variant.schema_version == schema_version)
        .expect("variant was checked above")
        .validate)(config)?;
    Ok(descriptor)
}

/// 只向客户端投影 descriptor 明确允许的字段，绝不回传服务端私有配置。
pub fn project_client_config(descriptor: &CapabilityDescriptor, config: &Value) -> Value {
    let Some(object) = config.as_object() else {
        return Value::Object(Default::default());
    };
    Value::Object(
        descriptor
            .client_config_fields
            .iter()
            .filter_map(|field| {
                object
                    .get(*field)
                    .cloned()
                    .map(|value| ((*field).into(), value))
            })
            .collect(),
    )
}
