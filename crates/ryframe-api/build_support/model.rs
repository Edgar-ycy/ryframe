use std::collections::BTreeMap;

use serde::Deserialize;

pub(super) const CATALOG_VERSION: u32 = 1;
pub(super) const SUPPORTED_HTTP_METHODS: &[&str] = &[
    "GET", "POST", "PUT", "PATCH", "DELETE", "HEAD", "OPTIONS", "TRACE",
];

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct AccessCatalog {
    pub(super) version: u32,
    pub(super) permissions: Vec<String>,
    #[serde(default)]
    pub(super) non_route_permissions: Vec<String>,
    #[serde(default)]
    pub(super) permission_names: BTreeMap<String, String>,
    #[serde(default)]
    pub(super) menus: Vec<MenuEntry>,
    #[serde(default)]
    pub(super) capabilities: Vec<CapabilityEntry>,
    #[serde(default)]
    pub(super) route_policies: Vec<ExplicitRoutePolicy>,
    #[serde(default)]
    pub(super) manual_routes: Vec<ManualRoute>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct GeneratedAccessCatalog {
    pub(super) version: u32,
    #[serde(default)]
    pub(super) resources: Vec<GeneratedResource>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct GeneratedResource {
    pub(super) name: String,
    pub(super) module: String,
    #[serde(default)]
    pub(super) capability: Option<String>,
    pub(super) labels: GeneratedLabels,
    pub(super) menu: GeneratedMenu,
    pub(super) route: GeneratedRoute,
    pub(super) permissions: GeneratedPermissions,
    #[serde(default)]
    pub(super) extension_permissions: BTreeMap<String, String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct GeneratedLabels {
    pub(super) zh_cn: String,
    pub(super) en: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct GeneratedMenu {
    pub(super) key: String,
    pub(super) parent: String,
    pub(super) order: u32,
    pub(super) icon: Option<String>,
    pub(super) labels: GeneratedLabels,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct GeneratedRoute {
    pub(super) key: String,
    pub(super) path: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct GeneratedPermissions {
    pub(super) create: String,
    pub(super) read: String,
    pub(super) list: String,
    pub(super) update: String,
    pub(super) delete: String,
}

impl GeneratedPermissions {
    pub(super) fn values(&self) -> [&str; 5] {
        [
            &self.create,
            &self.read,
            &self.list,
            &self.update,
            &self.delete,
        ]
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct MenuEntry {
    pub(super) route_key: String,
    pub(super) order: u32,
    pub(super) name: String,
    pub(super) title_key: String,
    pub(super) menu_type: String,
    pub(super) page_key: Option<String>,
    pub(super) permission: Option<String>,
    pub(super) capability: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct CapabilityEntry {
    pub(super) code: String,
    pub(super) route_keys: Vec<String>,
    pub(super) page_keys: Vec<String>,
    pub(super) permissions: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ExplicitRoutePolicy {
    pub(super) method: String,
    pub(super) path: String,
    pub(super) policy: ExplicitPolicyKind,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ManualRoute {
    pub(super) source: String,
    pub(super) handler: String,
    pub(super) method: String,
    pub(super) path: String,
    pub(super) policy: ExplicitPolicyKind,
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum ExplicitPolicyKind {
    Public,
    Authenticated,
}

#[derive(Clone, Copy, Debug)]
pub(super) enum GeneratedPolicyKind {
    Public,
    Authenticated,
    Permission,
    Capability,
}

impl GeneratedPolicyKind {
    pub(super) const fn rust_name(self) -> &'static str {
        match self {
            Self::Public => "Public",
            Self::Authenticated => "Authenticated",
            Self::Permission => "Permission",
            Self::Capability => "Capability",
        }
    }
}

#[derive(Debug)]
pub(super) struct CompiledRoute {
    pub(super) source: String,
    pub(super) handler: String,
    pub(super) method: String,
    pub(super) path: String,
    pub(super) permission: Option<String>,
    pub(super) capability: Option<String>,
    pub(super) declared_policy: Option<GeneratedPolicyKind>,
}
