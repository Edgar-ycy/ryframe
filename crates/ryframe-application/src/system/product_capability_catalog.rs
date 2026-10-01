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
pub const CAPABILITY_CATALOG: &[CapabilityDescriptor] = &[CapabilityDescriptor {
    code: "system.post",
    name: "岗位管理",
    description: "租户岗位的查询、创建、修改、删除和导出能力。",
    affects_authorization: true,
    dependencies: &[],
    conflicts: &[],
    route_keys: &["system.post"],
    permission_codes: &[
        "system:post:add",
        "system:post:edit",
        "system:post:export",
        "system:post:list",
        "system:post:remove",
    ],
    default_admin_permissions: &[
        "system:post:add",
        "system:post:edit",
        "system:post:export",
        "system:post:list",
        "system:post:remove",
    ],
    deployment_dependencies: &[],
    client_config_fields: &[],
    variants: &[STANDARD_VARIANT],
}];

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
