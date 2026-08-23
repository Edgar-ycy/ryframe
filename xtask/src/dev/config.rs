use std::{
    env,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
};

use crate::Result;

#[derive(Debug, Clone, Copy)]
pub(super) struct DevPorts {
    pub(super) api: u16,
    pub(super) worker: u16,
}

impl DevPorts {
    pub(super) fn from_environment() -> Result<Self> {
        let ports = Self {
            api: environment_port("APP_APP_PORT", 8080)?,
            worker: environment_port("APP_JOBS_HEALTH_PORT", 9091)?,
        };
        if ports.api == ports.worker {
            return Err("APP_APP_PORT 与 APP_JOBS_HEALTH_PORT 不能使用同一端口".into());
        }
        Ok(ports)
    }
}

fn environment_port(name: &str, default: u16) -> Result<u16> {
    match env::var(name) {
        Ok(value) => value
            .parse::<u16>()
            .ok()
            .filter(|port| *port > 0)
            .ok_or_else(|| format!("{name} 必须是 1~65535 的端口").into()),
        Err(env::VarError::NotPresent) => Ok(default),
        Err(env::VarError::NotUnicode(_)) => Err(format!("{name} 不是有效 UTF-8").into()),
    }
}

#[derive(Debug, Clone, Copy)]
pub(super) struct WorkerIds {
    pub(super) api: u16,
    pub(super) worker: u16,
    pub(super) probe_api: u16,
    pub(super) probe_worker: u16,
}

impl WorkerIds {
    pub(super) fn from_environment() -> Result<Self> {
        let base = match env::var("SNOWFLAKE_WORKER_ID") {
            Ok(value) => value
                .parse::<u16>()
                .ok()
                .filter(|value| *value <= 1023)
                .ok_or("SNOWFLAKE_WORKER_ID 必须是 0~1023 的整数")?,
            Err(env::VarError::NotPresent) => 1,
            Err(env::VarError::NotUnicode(_)) => {
                return Err("SNOWFLAKE_WORKER_ID 不是有效 UTF-8".into());
            }
        };
        Ok(Self {
            api: base,
            worker: (base + 1) % 1024,
            probe_api: (base + 2) % 1024,
            probe_worker: (base + 3) % 1024,
        })
    }
}

pub(super) fn spawn_shutdown_listener(
    shutdown_requested: Arc<AtomicBool>,
) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        let runtime = match tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        {
            Ok(runtime) => runtime,
            Err(error) => {
                eprintln!("无法安装 Ctrl+C 监听器：{error}");
                return;
            }
        };
        if runtime.block_on(tokio::signal::ctrl_c()).is_ok() {
            shutdown_requested.store(true, Ordering::Release);
        }
    })
}
