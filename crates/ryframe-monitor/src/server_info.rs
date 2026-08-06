use std::{thread, time::Duration};

use ryframe_kernel::{AppError, AppResult};
use serde::Serialize;
use sysinfo::System;
use tokio::sync::watch;
use utoipa::ToSchema;

const SAMPLE_INTERVAL: Duration = Duration::from_secs(1);

#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct ServerInfo {
    /// 操作系统。
    pub os: String,
    /// 主机名。
    pub hostname: String,
    /// CPU 核心数。
    pub cpu_cores: usize,
    /// CPU 使用率（百分比）。
    pub cpu_usage: f32,
    /// 总内存（GB）。
    pub total_memory: f64,
    /// 已用内存（GB）。
    pub used_memory: f64,
    /// 内存使用率（百分比）。
    pub memory_usage: f32,
    /// 进程 PID。
    pub pid: u32,
    /// 系统运行时长（秒）。
    pub uptime: u64,
}

/// 进程级服务器信息快照读取器。
#[derive(Clone)]
pub struct ServerInfoSampler {
    receiver: watch::Receiver<ServerInfo>,
}

impl ServerInfoSampler {
    /// 启动复用同一个 `System` 的后台采样器，并在返回前完成首个有效样本。
    pub async fn spawn(
        shutdown: watch::Receiver<bool>,
    ) -> AppResult<(Self, tokio::task::JoinHandle<()>)> {
        let collector = tokio::task::spawn_blocking(ServerInfoCollector::initialize)
            .await
            .map_err(|error| AppError::Internal(format!("服务器信息初始化任务失败: {error}")))?;
        let initial = collector.snapshot();
        let (sender, receiver) = watch::channel(initial);
        let handle = tokio::task::spawn_blocking(move || {
            run_sampler(collector, sender, shutdown);
        });
        Ok((Self { receiver }, handle))
    }

    /// 返回最近一次完整采样结果，不在请求线程中刷新系统信息。
    pub fn latest(&self) -> ServerInfo {
        self.receiver.borrow().clone()
    }
}

struct ServerInfoCollector {
    system: System,
    os: String,
    hostname: String,
    cpu_cores: usize,
    pid: u32,
}

impl ServerInfoCollector {
    fn initialize() -> Self {
        let mut system = System::new_all();
        system.refresh_all();
        thread::sleep(sysinfo::MINIMUM_CPU_UPDATE_INTERVAL);
        system.refresh_cpu_all();
        Self {
            cpu_cores: system.cpus().len(),
            system,
            os: std::env::consts::OS.to_owned(),
            hostname: System::host_name().unwrap_or_default(),
            pid: std::process::id(),
        }
    }

    fn refresh(&mut self) {
        self.system.refresh_memory();
        self.system.refresh_cpu_all();
    }

    fn snapshot(&self) -> ServerInfo {
        let total_memory = bytes_to_gigabytes(self.system.total_memory());
        let used_memory = bytes_to_gigabytes(self.system.used_memory());
        let memory_usage = if total_memory > 0.0 {
            round_percent(used_memory / total_memory * 100.0)
        } else {
            0.0
        };
        ServerInfo {
            os: self.os.clone(),
            hostname: self.hostname.clone(),
            cpu_cores: self.cpu_cores,
            cpu_usage: round_percent(f64::from(self.system.global_cpu_usage())),
            total_memory: round_two_decimals(total_memory),
            used_memory: round_two_decimals(used_memory),
            memory_usage,
            pid: self.pid,
            uptime: System::uptime(),
        }
    }
}

fn run_sampler(
    mut collector: ServerInfoCollector,
    sender: watch::Sender<ServerInfo>,
    shutdown: watch::Receiver<bool>,
) {
    while !*shutdown.borrow() {
        thread::sleep(SAMPLE_INTERVAL);
        if *shutdown.borrow() {
            break;
        }
        collector.refresh();
        if sender.send(collector.snapshot()).is_err() {
            break;
        }
    }
}

fn bytes_to_gigabytes(value: u64) -> f64 {
    value as f64 / 1024.0 / 1024.0 / 1024.0
}

fn round_two_decimals(value: f64) -> f64 {
    (value * 100.0).round() / 100.0
}

fn round_percent(value: f64) -> f32 {
    round_two_decimals(value) as f32
}
