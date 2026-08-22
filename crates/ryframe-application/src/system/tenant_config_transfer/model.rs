use super::*;

#[derive(Clone, Debug, Serialize)]
pub struct TenantConfigBundleVo {
    pub id: String,
    pub origin: String,
    pub source_tenant_key: String,
    pub source_tenant_name: String,
    pub package_schema_version: String,
    pub source_app_version: String,
    pub sha256: Option<String>,
    pub resource_counts: BTreeMap<String, u64>,
    pub item_count: i32,
    pub status: String,
    pub error_summary: Option<String>,
    pub expires_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl From<TenantConfigBundleRecord> for TenantConfigBundleVo {
    fn from(value: TenantConfigBundleRecord) -> Self {
        Self {
            id: value.id.to_string(),
            origin: value.origin,
            source_tenant_key: value.source_tenant_key,
            source_tenant_name: value.source_tenant_name_snapshot,
            package_schema_version: value.package_schema_version,
            source_app_version: value.source_app_version,
            sha256: value.sha256,
            resource_counts: json_counts(&value.resource_counts),
            item_count: value.item_count,
            status: value.status,
            error_summary: value.error_summary,
            expires_at: value.expires_at,
            created_at: value.created_at,
            updated_at: value.updated_at,
        }
    }
}

/// 配置迁移公开视图中的配置包摘要，不包含数据库关联标识或文件信息。
#[derive(Clone, Debug, Serialize)]
pub struct TenantConfigBundleSummaryVo {
    pub origin: String,
    pub source_tenant_key: String,
    pub source_tenant_name: String,
    pub package_schema_version: String,
    pub source_app_version: String,
    pub sha256: Option<String>,
    pub resource_counts: BTreeMap<String, u64>,
    pub item_count: i32,
    pub status: String,
    pub expires_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

impl From<&TenantConfigBundleRecord> for TenantConfigBundleSummaryVo {
    fn from(value: &TenantConfigBundleRecord) -> Self {
        Self {
            origin: value.origin.clone(),
            source_tenant_key: value.source_tenant_key.clone(),
            source_tenant_name: value.source_tenant_name_snapshot.clone(),
            package_schema_version: value.package_schema_version.clone(),
            source_app_version: value.source_app_version.clone(),
            sha256: value.sha256.clone(),
            resource_counts: json_counts(&value.resource_counts),
            item_count: value.item_count,
            status: value.status.clone(),
            expires_at: value.expires_at,
            created_at: value.created_at,
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct TenantConfigTransferVo {
    pub id: String,
    pub bundle_summary: TenantConfigBundleSummaryVo,
    pub status: String,
    pub target_configuration_version: i64,
    pub target_authorization_epoch: i32,
    pub plan_hash: Option<String>,
    pub preview_calculated_at: Option<DateTime<Utc>>,
    pub change_counts: BTreeMap<String, u64>,
    pub error_summary: Option<String>,
    pub applied_configuration_version: Option<i64>,
    pub applied_authorization_epoch: Option<i32>,
    pub rollback_expires_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl TenantConfigTransferVo {
    pub(super) fn from_models(
        value: TenantConfigTransferRecord,
        bundle: &TenantConfigBundleRecord,
    ) -> AppResult<Self> {
        if value.tenant_id != bundle.tenant_id || value.bundle_id != bundle.id {
            return Err(AppError::Internal("配置迁移关联的配置包无效".into()));
        }
        Ok(Self {
            id: value.id.to_string(),
            bundle_summary: bundle.into(),
            status: value.status,
            target_configuration_version: value.target_configuration_version,
            target_authorization_epoch: value.target_authorization_epoch,
            plan_hash: value.plan_hash,
            preview_calculated_at: value.preview_calculated_at,
            change_counts: json_counts(&value.change_counts),
            error_summary: value.error_summary,
            applied_configuration_version: value.applied_configuration_version,
            applied_authorization_epoch: value.applied_authorization_epoch,
            rollback_expires_at: value.rollback_expires_at,
            created_at: value.created_at,
            updated_at: value.updated_at,
        })
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct TenantConfigTransferItemVo {
    pub resource_type: String,
    pub stable_key: String,
    pub display_name: String,
    pub action: String,
    pub outcome: String,
    pub detail_code: Option<String>,
    pub detail: Option<String>,
}

impl From<TenantConfigTransferItemRecord> for TenantConfigTransferItemVo {
    fn from(value: TenantConfigTransferItemRecord) -> Self {
        Self {
            resource_type: value.resource_type,
            stable_key: value.stable_key,
            display_name: value.display_name,
            action: value.action,
            outcome: value.outcome,
            detail_code: value.detail_code,
            detail: value.detail,
        }
    }
}

pub struct RequestTenantConfigBundleOutcome {
    pub bundle: TenantConfigBundleVo,
    pub inserted: bool,
}

pub struct RequestTenantConfigTransferOutcome {
    pub transfer: TenantConfigTransferVo,
    pub inserted: bool,
}

#[derive(Clone, Debug)]
pub struct ApplyTenantConfigTransferCommand {
    pub plan_hash: String,
    pub target_configuration_version: i64,
    pub target_authorization_epoch: i32,
    pub idempotency_key_hash: String,
}

/// 当前二进制实际支持的页面路由与 API 权限目录。
///
/// 目录由 API crate 的编译期路由注册表构造，并在 API、Embedded Worker、
/// External Worker 与 `--once` 之间共享。配置迁移不得使用数据库菜单或权限记录
/// 反向证明一个路由或 API 权限受当前版本支持。
#[derive(Clone, Debug)]
pub struct TenantConfigTargetCatalog {
    pub(super) page_routes: BTreeMap<String, (String, String)>,
    pub(super) api_permission_codes: BTreeMap<String, String>,
}

impl TenantConfigTargetCatalog {
    pub fn new(
        page_routes: impl IntoIterator<Item = (String, String)>,
        api_permission_codes: impl IntoIterator<Item = String>,
    ) -> AppResult<Self> {
        let page_routes = validate_route_catalog(page_routes)?;
        let api_permission_codes = validate_catalog_values(api_permission_codes, "API 权限")?;
        if api_permission_codes
            .values()
            .any(|code| !code.contains(':'))
        {
            return Err(AppError::Config(
                "编译期 API 权限目录包含格式无效的权限码".into(),
            ));
        }
        Ok(Self {
            page_routes,
            api_permission_codes,
        })
    }

    #[doc(hidden)]
    pub fn page_routes(&self) -> &BTreeMap<String, (String, String)> {
        &self.page_routes
    }

    #[doc(hidden)]
    pub fn api_permission_codes(&self) -> &BTreeMap<String, String> {
        &self.api_permission_codes
    }
}

fn json_counts(value: &Value) -> BTreeMap<String, u64> {
    value
        .as_object()
        .into_iter()
        .flatten()
        .filter_map(|(key, value)| value.as_u64().map(|value| (key.clone(), value)))
        .collect()
}

fn validate_catalog_values(
    values: impl IntoIterator<Item = String>,
    label: &str,
) -> AppResult<BTreeMap<String, String>> {
    let values = values.into_iter().collect::<Vec<_>>();
    if values.is_empty() {
        return Err(AppError::Config(format!("编译期{label}目录不能为空")));
    }
    let mut catalog = BTreeMap::new();
    for value in values {
        if value.is_empty() || value.trim() != value {
            return Err(AppError::Config(format!(
                "编译期{label}目录包含空值或首尾空白"
            )));
        }
        if catalog
            .insert(normalize_stable_key(&value), value)
            .is_some()
        {
            return Err(AppError::Config(format!("编译期{label}目录包含重复项目")));
        }
    }
    Ok(catalog)
}

fn validate_route_catalog(
    values: impl IntoIterator<Item = (String, String)>,
) -> AppResult<BTreeMap<String, (String, String)>> {
    let mut catalog = BTreeMap::new();
    for (route_key, menu_type) in values {
        if route_key.is_empty() || route_key.trim() != route_key {
            return Err(AppError::Config(
                "编译期页面路由目录包含空值或首尾空白".into(),
            ));
        }
        if !matches!(menu_type.as_str(), "M" | "C") {
            return Err(AppError::Config(format!(
                "编译期页面路由 {route_key} 的菜单类型无效"
            )));
        }
        if catalog
            .insert(normalize_stable_key(&route_key), (route_key, menu_type))
            .is_some()
        {
            return Err(AppError::Config("编译期页面路由目录包含重复项目".into()));
        }
    }
    if catalog.is_empty() {
        return Err(AppError::Config("编译期页面路由目录不能为空".into()));
    }
    Ok(catalog)
}
