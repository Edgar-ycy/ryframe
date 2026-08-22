//! RyFrame 组合根库，集中暴露进程装配与受控维护能力。

pub mod app;
pub mod boot;

#[cfg(feature = "destructive-reset")]
#[path = "bin/ryframe_reset.rs"]
pub mod reset;
