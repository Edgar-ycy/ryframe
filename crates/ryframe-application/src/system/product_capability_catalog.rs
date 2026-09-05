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

/// 当前部署没有可选业务能力；套餐和租户始终支持合法的空能力集合。
pub const CAPABILITY_CATALOG: &[CapabilityDescriptor] = &[];

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
