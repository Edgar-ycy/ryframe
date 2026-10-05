//! RyFrame 组合根库，集中暴露进程装配与受控维护能力。

#[cfg(feature = "bin-api")]
pub mod app;
#[cfg(any(feature = "bin-api", feature = "bin-worker"))]
pub mod boot;
#[cfg(any(feature = "bin-api", feature = "bin-worker", feature = "bin-migrate"))]
pub mod business;
#[cfg(any(
    feature = "bin-api",
    feature = "bin-worker",
    feature = "bin-migrate",
    feature = "bin-tenant-data",
    feature = "bin-file-maintenance",
    feature = "bin-reset",
))]
pub mod crypto;
#[cfg(any(feature = "bin-api", feature = "bin-worker"))]
pub mod healthcheck;

#[cfg(feature = "bin-reset")]
pub mod reset;
