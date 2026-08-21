//! 权限诊断和授权镜像所需的持久化端口。

mod diagnostic;
mod mirror;

pub use diagnostic::{
    AuthorizationDiagnosticReadPort, DiagnosticDepartmentRecord, DiagnosticMenuRecord,
    DiagnosticPermissionRecord, DiagnosticRoleRecord,
};
pub use mirror::{AuthorizationMirrorEvent, AuthorizationMirrorTransaction};
