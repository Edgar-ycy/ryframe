//! 租户、产品与授权诊断领域。

pub use super::authorization_diagnostic::*;
pub use super::platform_boundary::{is_platform_permission, is_platform_route};
pub use super::product::*;
pub use super::product_capability_catalog::*;
pub use super::tenant::*;
pub use super::tenant::{config_package::*, config_transfer::*, data_migration::*, usage::*};
