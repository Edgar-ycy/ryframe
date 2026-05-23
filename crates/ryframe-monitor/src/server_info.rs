use serde::Serialize;
use sysinfo::System;

#[derive(Debug, Serialize)]
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

        let total_mem = sys.total_memory() as f64 / 1024.0 / 1024.0 / 1024.0;
        let used_mem = sys.used_memory() as f64 / 1024.0 / 1024.0 / 1024.0;
        let cpu_usage = sys.global_cpu_usage();

        Self {
            os: std::env::consts::OS.to_string(),
            hostname: System::host_name().unwrap_or_default(),
            cpu_cores: sys.cpus().len(),
            cpu_usage: (cpu_usage * 100.0 * 100.0).round() / 100.0,
            total_memory: (total_mem * 100.0).round() / 100.0,
            used_memory: (used_mem * 100.0).round() / 100.0,
            memory_usage: if total_mem > 0.0 {
                ((used_mem / total_mem) * 10000.0).round() as f32 / 100.0
            } else { 0.0_f32 },
            pid: std::process::id(),
            uptime: System::uptime(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_server_info() {
        let info = ServerInfo::collect();
        assert!(!info.os.is_empty());
        assert!(info.cpu_cores > 0);
        assert!(info.total_memory > 0.0);
        assert!(info.pid > 0);
        assert!(info.memory_usage >= 0.0 && info.memory_usage <= 100.0);

        let json = serde_json::to_value(&info).unwrap();
        assert!(json.get("os").is_some());
        assert!(json.get("cpu_cores").is_some());
    }
}