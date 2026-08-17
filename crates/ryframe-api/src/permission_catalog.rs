pub type RouteCapabilityBinding = (
    &'static str,
    &'static str,
    &'static str,
    &'static str,
    &'static str,
    Option<&'static str>,
);

include!(concat!(env!("OUT_DIR"), "/permission_catalog.rs"));

/// 从所有已编译 HTTP 路由属性中嵌入的权限码。
pub fn route_permission_codes() -> &'static [&'static str] {
    ROUTE_PERMISSION_CODES
}

/// 由 `#[capability]` 与可选 `#[perm]` 属性生成的编译期路由约束。
pub fn route_capability_bindings() -> &'static [RouteCapabilityBinding] {
    ROUTE_CAPABILITY_BINDINGS
}
