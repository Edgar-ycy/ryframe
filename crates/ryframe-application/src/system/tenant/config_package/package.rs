use std::{collections::BTreeSet, sync::Arc};

use chrono::{DateTime, Utc};
use ryframe_kernel::{AppError, AppResult};
use sha2::{Digest, Sha256};

use super::super::super::{CAPABILITY_CATALOG, CapabilityRequirement};
use super::{
    NAME_MAX_CHARS, STABLE_CODE_MAX_BYTES, TRANSFER_STABLE_KEY_MAX_CHARS,
    TenantConfigCatalogSummary, TenantConfigPackageLimits, TenantConfigPackageManifest,
    TenantConfigPackageResources,
};

/// 成功生成的配置包及其规范资源表示。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GeneratedTenantConfigPackage {
    pub manifest: TenantConfigPackageManifest,
    pub resources: TenantConfigPackageResources,
    pub canonical_resources: Vec<u8>,
    pub data: Vec<u8>,
    pub package_sha256: String,
}

/// 成功解析并完成安全校验的配置包。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ParsedTenantConfigPackage {
    pub manifest: TenantConfigPackageManifest,
    pub resources: TenantConfigPackageResources,
    pub canonical_resources: Vec<u8>,
    pub package_sha256: String,
}

/// 生成租户配置包时写入清单的来源信息。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TenantConfigPackageSource {
    pub tenant_key: String,
    pub tenant_name: String,
    pub app_version: String,
    pub generated_at: DateTime<Utc>,
}

/// 在阻塞线程中构造只含两个受控文件的配置包。
pub async fn build_tenant_config_package(
    archive: Arc<dyn crate::ports::tenant_config::TenantConfigArchivePort>,
    resources: TenantConfigPackageResources,
    required_capabilities: Vec<CapabilityRequirement>,
    source: TenantConfigPackageSource,
    limits: TenantConfigPackageLimits,
) -> AppResult<GeneratedTenantConfigPackage> {
    tokio::task::spawn_blocking(move || {
        super::format::build_package_blocking(
            archive.as_ref(),
            resources,
            required_capabilities,
            source,
            limits,
        )
    })
    .await
    .map_err(|error| {
        tracing::error!(%error, "租户配置包生成阻塞任务失败");
        AppError::Internal("租户配置包生成任务失败".into())
    })?
}

/// 在阻塞线程中解析并校验受控配置包。
pub async fn parse_tenant_config_package(
    archive: Arc<dyn crate::ports::tenant_config::TenantConfigArchivePort>,
    data: Vec<u8>,
    limits: TenantConfigPackageLimits,
) -> AppResult<ParsedTenantConfigPackage> {
    let (parsed, _) = super::parse_tenant_config_package_with_source(archive, data, limits).await?;
    Ok(parsed)
}

pub(super) fn sha256_hex(data: &[u8]) -> String {
    hex::encode(Sha256::digest(data))
}

fn catalog_summary<'a>(values: impl Iterator<Item = &'a str>) -> TenantConfigCatalogSummary {
    let values = values.collect::<BTreeSet<_>>();
    let canonical = values.iter().copied().collect::<Vec<_>>().join("\n");
    TenantConfigCatalogSummary {
        count: values.len(),
        sha256: sha256_hex(canonical.as_bytes()),
    }
}

pub(super) fn required_permission_summary(
    resources: &TenantConfigPackageResources,
) -> TenantConfigCatalogSummary {
    catalog_summary(resources.permissions.iter().map(|item| item.code.as_str()))
}

pub(super) fn required_route_summary(
    resources: &TenantConfigPackageResources,
) -> TenantConfigCatalogSummary {
    catalog_summary(
        resources
            .menus
            .iter()
            .filter_map(|item| item.route_key.as_deref()),
    )
}

pub(super) fn validate_required_capabilities(
    requirements: &[CapabilityRequirement],
    resources: &TenantConfigPackageResources,
) -> AppResult<()> {
    let mut canonical = requirements.to_vec();
    canonical.sort();
    if canonical != requirements {
        return Err(AppError::Validation(
            "配置包 required_capabilities 必须按 code/variant/schema_version 排序".into(),
        ));
    }
    let mut declared_codes = BTreeSet::new();
    for requirement in requirements {
        validate_stable_code(&requirement.code, STABLE_CODE_MAX_BYTES, "能力代码")?;
        validate_stable_code(&requirement.variant, STABLE_CODE_MAX_BYTES, "能力 variant")?;
        if requirement.schema_version <= 0 {
            return Err(AppError::Validation(
                "能力 schema_version 必须是正整数".into(),
            ));
        }
        if !declared_codes.insert(requirement.code.as_str()) {
            return Err(AppError::Validation(format!(
                "配置包重复声明能力 {}",
                requirement.code
            )));
        }
        let descriptor = CAPABILITY_CATALOG
            .iter()
            .find(|descriptor| descriptor.code == requirement.code)
            .ok_or_else(|| {
                AppError::Validation(format!(
                    "配置包声明了当前版本未知的能力 {}",
                    requirement.code
                ))
            })?;
        if !descriptor.variants.iter().any(|variant| {
            variant.code == requirement.variant
                && variant.schema_version == requirement.schema_version
        }) {
            return Err(AppError::Validation(format!(
                "配置包能力 {} 的 variant/schema 不受当前版本支持",
                requirement.code
            )));
        }
    }

    let mut involved_codes = BTreeSet::new();
    for descriptor in CAPABILITY_CATALOG {
        let uses_permission = resources.permissions.iter().any(|permission| {
            descriptor
                .permission_codes
                .contains(&permission.code.as_str())
        }) || resources.roles.iter().any(|role| {
            role.permission_codes
                .iter()
                .any(|permission| descriptor.permission_codes.contains(&permission.as_str()))
        }) || resources.menus.iter().any(|menu| {
            menu.permission_code
                .as_deref()
                .is_some_and(|permission| descriptor.permission_codes.contains(&permission))
        });
        let uses_route = resources.menus.iter().any(|menu| {
            menu.route_key
                .as_deref()
                .is_some_and(|route| descriptor.route_keys.contains(&route))
        });
        if uses_permission || uses_route {
            involved_codes.insert(descriptor.code);
        }
    }
    if involved_codes != declared_codes {
        return Err(AppError::Validation(
            "配置包 required_capabilities 与实际权限/菜单资源不一致".into(),
        ));
    }
    Ok(())
}

pub(super) fn route_menu_stable_key(route_key: &str) -> String {
    format!("route:{}:{route_key}", route_key.len())
}

pub(super) fn action_menu_stable_key(parent_stable_key: &str, permission_code: &str) -> String {
    format!(
        "action:{}:{parent_stable_key}:{}:{permission_code}",
        parent_stable_key.len(),
        permission_code.len()
    )
}

/// 保守识别不得跨环境迁移的敏感参数键。
pub fn is_sensitive_config_key(key: &str) -> bool {
    let normalized = key
        .chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect::<String>();
    const SENSITIVE_MARKERS: [&str; 10] = [
        "password",
        "passwd",
        "passphrase",
        "secret",
        "token",
        "credential",
        "privatekey",
        "apikey",
        "accesskey",
        "signingkey",
    ];
    SENSITIVE_MARKERS
        .iter()
        .any(|marker| normalized.contains(marker))
}

pub(super) fn validate_path(path: &[String], label: &str) -> AppResult<()> {
    if path.is_empty() {
        return Err(AppError::Validation(format!("{label}不能为空")));
    }
    for segment in path {
        validate_text(segment, NAME_MAX_CHARS, label)?;
        if segment.trim() != segment {
            return Err(AppError::Validation(format!("{label}格式无效")));
        }
    }
    Ok(())
}

pub(super) fn validate_department_stable_key(path: &[String]) -> AppResult<()> {
    let stable_key = path
        .iter()
        .map(|part| format!("{}:{part}", part.len()))
        .collect::<Vec<_>>()
        .join("/");
    if stable_key.chars().count() > TRANSFER_STABLE_KEY_MAX_CHARS {
        return Err(AppError::Validation(format!(
            "部门完整路径生成的稳定键不能超过 {TRANSFER_STABLE_KEY_MAX_CHARS} 个字符"
        )));
    }
    Ok(())
}

pub(super) fn validate_stable_text(value: &str, max_bytes: usize, label: &str) -> AppResult<()> {
    if value.trim().is_empty()
        || value.trim() != value
        || value.len() > max_bytes
        || value.contains('\0')
    {
        return Err(AppError::Validation(format!("{label}格式无效")));
    }
    Ok(())
}

pub(super) fn validate_stable_code(value: &str, max_bytes: usize, label: &str) -> AppResult<()> {
    validate_stable_text(value, max_bytes, label)?;
    if value.bytes().any(|byte| !(0x21..=0x7e).contains(&byte)) {
        return Err(AppError::Validation(format!(
            "{label}只能使用 ASCII 可见字符"
        )));
    }
    Ok(())
}

pub(super) fn validate_text(value: &str, max_chars: usize, label: &str) -> AppResult<()> {
    if value.trim().is_empty() || value.contains('\0') || value.chars().count() > max_chars {
        return Err(AppError::Validation(format!(
            "{label}不能为空且不能超过 {max_chars} 个字符"
        )));
    }
    Ok(())
}

pub(super) fn validate_optional_text(
    value: &Option<String>,
    max_chars: usize,
    label: &str,
) -> AppResult<()> {
    if value
        .as_deref()
        .is_some_and(|value| value.contains('\0') || value.chars().count() > max_chars)
    {
        return Err(AppError::Validation(format!(
            "{label}不能超过 {max_chars} 个字符"
        )));
    }
    Ok(())
}

pub(super) fn collation_key(value: &str) -> String {
    // 便携业务代码均受 ASCII 校验约束，小写化后与目标端的不区分大小写键语义一致。
    value.to_ascii_lowercase()
}

pub(super) fn normalized_path_key(path: &[String]) -> Vec<String> {
    // 部门路径采用明确的逐段二进制语义，不使用 Rust 近似数据库排序规则。
    path.to_vec()
}

pub(super) fn validate_status(value: &str, label: &str) -> AppResult<()> {
    if matches!(value, "0" | "1") {
        Ok(())
    } else {
        Err(AppError::Validation(format!("{label}无效")))
    }
}

pub(super) fn unique_by<T>(values: impl IntoIterator<Item = T>, message: &str) -> AppResult<()>
where
    T: Ord,
{
    let mut unique = BTreeSet::new();
    for value in values {
        if !unique.insert(value) {
            return Err(AppError::Validation(message.to_owned()));
        }
    }
    Ok(())
}

pub(super) fn validate_parent_graph(
    parents: &std::collections::BTreeMap<String, Option<String>>,
    label: &str,
) -> AppResult<()> {
    for node in parents.keys() {
        let mut current = Some(node.as_str());
        let mut visiting = BTreeSet::new();
        while let Some(value) = current {
            if !visiting.insert(value) {
                return Err(AppError::Validation(format!("{label}存在循环引用")));
            }
            current = parents.get(value).and_then(Option::as_deref);
        }
    }
    Ok(())
}
