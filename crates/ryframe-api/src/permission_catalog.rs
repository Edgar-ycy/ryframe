include!(concat!(env!("OUT_DIR"), "/permission_catalog.rs"));

/// 从所有已编译 HTTP 路由属性中嵌入的权限码。
pub fn route_permission_codes() -> &'static [&'static str] {
    ROUTE_PERMISSION_CODES
}
