use std::{
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
    pub(super) fn isolated() -> Result<Self> {
        let (api, worker) = super::health::available_ports()?;
        Ok(Self { api, worker })
    }

    pub(super) fn from_config(config: &ryframe_config::AppConfig) -> Result<Self> {
        let ports = Self {
            api: config.app.port,
            worker: config.jobs.health_port,
        };
        if ports.api == 0 || ports.worker == 0 || ports.api == ports.worker {
            return Err("app.port 与 jobs.health_port 必须是互异的非零端口；请修正配置文件".into());
        }
        Ok(ports)
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
    pub(super) fn from_config(
        config: &ryframe_config::AppConfig,
        previous: Option<&super::model::Binaries>,
    ) -> Result<Self> {
        let api = u16::try_from(config.snowflake_worker_id)
            .ok()
            .filter(|id| *id <= 1023)
            .ok_or("snowflake_worker_id 必须为 0..=1023；请修正配置文件")?;
        let worker = (api + 1) % 1024;
        let mut reserved = super::inputs::reserved_ids(previous)?;
        reserved.extend([api, worker]);
        let mut available = (0..=1023).filter(|id| !reserved.contains(id));
        let probe_api = available.next().ok_or("没有可用的 probe API 实例编号")?;
        let probe_worker = available.next().ok_or("没有可用的 probe Worker 实例编号")?;
        Ok(Self {
            api,
            worker,
            probe_api,
            probe_worker,
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
