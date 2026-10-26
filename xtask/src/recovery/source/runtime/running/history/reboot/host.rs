//! 当前主机启动刻度只来自内核；保留固定余量，不按 PID 猜测历史进程退出。
use crate::{Result, recovery::clone::native::resources::common};
use serde_json::Value;

const TICKS: u64 = 10_000_000;
const MARGIN: u64 = 300 * TICKS;

pub(super) fn exited(identity: &Value, boot: u64) -> Result<()> {
    let identity = common::validate_process_identity(identity)?;
    let started = identity["started"]
        .as_str()
        .filter(|text| !text.is_empty() && text.bytes().all(|b| b.is_ascii_digit()))
        .ok_or("重启证明缺少完整创建时间")?
        .parse::<u64>()?;
    if !boot
        .checked_sub(MARGIN)
        .is_some_and(|limit| started < limit)
    {
        return Err(
            "登记代次不能证明早于当前主机启动及固定余量；保留现场，禁止重放或推断停止".into(),
        );
    }
    Ok(())
}

#[cfg(windows)]
pub(super) fn boot() -> Result<u64> {
    let mut observed = winsafe::FILETIME::default();
    winsafe::GetSystemTimeAsFileTime(&mut observed);
    let observed = (u64::from(observed.dwHighDateTime) << 32) | u64::from(observed.dwLowDateTime);
    let uptime = winsafe::GetTickCount64()
        .checked_mul(10_000)
        .ok_or("主机运行刻度溢出")?;
    checked(observed, uptime)
}

#[cfg(not(windows))]
pub(super) fn boot() -> Result<u64> {
    let observed = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?;
    let observed = observed
        .as_secs()
        .checked_add(11_644_473_600)
        .and_then(|n| n.checked_mul(TICKS))
        .and_then(|n| n.checked_add(u64::from(observed.subsec_nanos() / 100)))
        .ok_or("主机当前刻度溢出")?;
    let data = std::fs::read_to_string("/proc/stat")
        .map_err(|error| format!("主机启动证明无法读取 /proc/stat：{error}；检查内核只读入口"))?;
    let mut values = data.lines().filter_map(|line| line.strip_prefix("btime "));
    let start = values
        .next()
        .ok_or("主机启动证明缺少 /proc/stat 启动时刻")?
        .trim()
        .parse::<u64>()?;
    if values.next().is_some() {
        return Err("主机启动证明包含重复启动时刻".into());
    }
    let start = start
        .checked_add(11_644_473_600)
        .and_then(|n| n.checked_mul(TICKS))
        .ok_or("主机启动刻度溢出")?;
    let uptime = observed
        .checked_sub(start)
        .ok_or("主机启动时刻晚于当前时刻")?;
    checked(observed, uptime)
}

fn checked(observed: u64, uptime: u64) -> Result<u64> {
    let boot = observed
        .checked_sub(uptime)
        .filter(|boot| *boot > 0 && *boot < observed);
    if uptime == 0 {
        return Err("主机启动证明的系统运行时长无效".into());
    }
    boot.ok_or_else(|| "主机启动证明推导出的启动时刻超出有效范围".into())
}
