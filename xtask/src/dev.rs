//! 本地开发监督器的命令入口。

#[path = "dev/build.rs"]
mod build;
#[path = "dev/command.rs"]
mod command;
#[path = "dev/config.rs"]
mod config;
#[path = "dev/health.rs"]
mod health;
#[path = "dev/model.rs"]
mod model;
#[path = "dev/orchestrator.rs"]
mod orchestrator;
#[path = "dev/services.rs"]
mod services;

#[allow(unused_imports)]
pub(crate) use orchestrator::run;

#[cfg(test)]
#[allow(unused_imports)]
pub(crate) use command::{api_command, worker_command};
#[cfg(test)]
#[allow(unused_imports)]
pub(crate) use health::{
    available_ports, combine_failures, start_worker_after_api_ready, wait_services_ready_until,
};
#[cfg(test)]
#[allow(unused_imports)]
pub(crate) use services::switch_services_with_rollback;
