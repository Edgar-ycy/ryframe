/// HTTP 路由的编译期访问策略。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AccessPolicy {
    Public,
    Authenticated,
    Permission,
    Capability,
}

/// 菜单、页面与访问约束的稳定映射。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MenuRouteDescriptor {
    pub route_key: &'static str,
    pub name: &'static str,
    pub title_key: &'static str,
    pub menu_type: &'static str,
    pub page_key: Option<&'static str>,
    pub permission_code: Option<&'static str>,
    pub capability_code: Option<&'static str>,
}

/// 产品能力对菜单、页面和权限的闭合集合。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CapabilityDescriptor {
    pub code: &'static str,
    pub route_keys: &'static [&'static str],
    pub page_keys: &'static [&'static str],
    pub permission_codes: &'static [&'static str],
}

/// 每个编译路由对应的显式访问策略。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RoutePolicyDescriptor {
    pub source: &'static str,
    pub handler: &'static str,
    pub method: &'static str,
    pub path: &'static str,
    pub policy: AccessPolicy,
    pub permission_code: Option<&'static str>,
    pub capability_code: Option<&'static str>,
}

/// 能力门禁路由的 OpenAPI 契约信息。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RouteCapabilityBinding {
    pub source: &'static str,
    pub handler: &'static str,
    pub method: &'static str,
    pub path: &'static str,
    pub capability_code: &'static str,
    pub permission_code: Option<&'static str>,
}

include!(concat!(env!("OUT_DIR"), "/permission_catalog.rs"));

pub const SUPPORTED_HTTP_METHODS: &[&str] = &[
    "GET", "POST", "PUT", "PATCH", "DELETE", "HEAD", "OPTIONS", "TRACE",
];

/// 返回机器目录声明的全部权限码，包括不直接绑定路由的治理权限。
pub fn permission_codes() -> &'static [&'static str] {
    PERMISSION_CODES
}

pub fn menu_routes() -> &'static [MenuRouteDescriptor] {
    MENU_ROUTES
}

pub fn capabilities() -> &'static [CapabilityDescriptor] {
    CAPABILITIES
}

pub fn route_policies() -> &'static [RoutePolicyDescriptor] {
    ROUTE_POLICIES
}

pub fn route_capability_bindings() -> &'static [RouteCapabilityBinding] {
    ROUTE_CAPABILITY_BINDINGS
}
