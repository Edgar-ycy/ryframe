include!(concat!(env!("OUT_DIR"), "/permission_catalog.rs"));

/// Permission codes embedded from all compiled HTTP route attributes.
pub fn route_permission_codes() -> &'static [&'static str] {
    ROUTE_PERMISSION_CODES
}
