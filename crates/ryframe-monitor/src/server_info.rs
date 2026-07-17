use serde::Serialize;
use sysinfo::System;
use utoipa::ToSchema;

#[derive(Debug, Serialize, ToSchema)]
pub struct ServerInfo {
    /// 操作系统
    pub os: String,
    /// 主机名
    pub hostname: String,
    /// CPU 核心数
    pub cpu_cores: usize,
    /// CPU 使用率（百分比）
    pub cpu_usage: f32,
    /// 总内存（GB）
    pub total_memory: f64,
    /// 已用内存（GB）
    pub used_memory: f64,
    /// 内存使用率（百分比）
    pub memory_usage: f32,
    /// JVM... no, Rust doesn't have JVM
    /// 进程 PID
    pub pid: u32,
    /// 运行时长（秒）
    pub uptime: u64,
}

impl ServerInfo {
    pub fn collect() -> Self {
        let mut sys = System::new_all();
        sys.refresh_all();

        // 需要两次刷新以获取真实 CPU 使用率差值
        // sysinfo::MINIMUM_CPU_UPDATE_INTERVAL 是获取准确数据的最小间隔
        std::thread::sleep(sysinfo::MINIMUM_CPU_UPDATE_INTERVAL);
        sys.refresh_cpu_all();

        let total_mem = sys.total_memory() as f64 / 1024.0 / 1024.0 / 1024.0;
        let used_mem = sys.used_memory() as f64 / 1024.0 / 1024.0 / 1024.0;
        let cpu_cores = sys.cpus().len();
        // sysinfo 0.39 中 global_cpu_usage() 直接返回百分比值（0-100）
        let cpu_usage = sys.global_cpu_usage();
        let cpu_percent = (cpu_usage as f64 * 100.0).round() as f32 / 100.0;

        Self {
            os: std::env::consts::OS.to_string(),
            hostname: System::host_name().unwrap_or_default(),
            cpu_cores,
            cpu_usage: cpu_percent,
            total_memory: (total_mem * 100.0).round() / 100.0,
            used_memory: (used_mem * 100.0).round() / 100.0,
            memory_usage: if total_mem > 0.0 {
                ((used_mem / total_mem) * 10000.0).round() as f32 / 100.0
            } else {
                0.0_f32
            },
            pid: std::process::id(),
            uptime: System::uptime(),
        }
    }
}
