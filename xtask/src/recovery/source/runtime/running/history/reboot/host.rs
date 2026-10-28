//! 当前主机启动刻度只来自内核；保留固定余量，不按 PID 猜测历史进程退出。
use crate::{Result, recovery::clone::native::resources::common};
use serde_json::{Value, json};

const TICKS: u64 = 10_000_000;
const MARGIN: u64 = 300 * TICKS;

pub(super) fn exited(identity: &Value, boot: u64) -> Result<bool> {
    let identity = common::validate_process_identity(identity)?;
    let started = identity["started"]
        .as_str()
        .filter(|text| !text.is_empty() && text.bytes().all(|b| b.is_ascii_digit()))
        .ok_or("重启证明缺少完整创建时间")?
        .parse::<u64>()?;
    if started == 0 {
        return Err("重启证明创建时间必须为正数".into());
    }
    Ok(boot
        .checked_sub(MARGIN)
        .is_some_and(|limit| started < limit))
}

#[cfg(windows)]
fn collect() -> Result<(u64, u64)> {
    let mut observed = winsafe::FILETIME::default();
    winsafe::GetSystemTimeAsFileTime(&mut observed);
    let observed = (u64::from(observed.dwHighDateTime) << 32) | u64::from(observed.dwLowDateTime);
    let uptime = winsafe::GetTickCount64()
        .checked_mul(10_000)
        .ok_or("主机运行刻度溢出")?;
    Ok((observed, uptime))
}

#[cfg(not(windows))]
fn collect() -> Result<(u64, u64)> {
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
    Ok((observed, uptime))
}

/// 公开字段与原 host-boot-proof 相同；采集只接受当前内核，不接受协议注入。
pub(super) fn proof() -> Result<Value> {
    let (observed, uptime) = collect()?;
    let value = recorded(observed, uptime)?;
    let (current, _) = collect()?;
    fresh(&value, current)?;
    Ok(value)
}

fn recorded(observed: u64, uptime: u64) -> Result<Value> {
    let boot = checked(observed, uptime)?;
    let seconds = i64::try_from(boot / TICKS)? - 11_644_473_600;
    let micros = (boot % TICKS) / 10;
    let date = chrono::DateTime::from_timestamp(seconds, 0).ok_or("主机启动时间超出范围")?;
    let mut time = date.format("%Y-%m-%dT%H:%M:%S").to_string();
    if micros != 0 {
        time.push_str(&format!(".{micros:06}"));
    }
    time.push_str("+00:00");
    Ok(json!({"format_version":1,"kind":"host-boot-proof",
        "source":if cfg!(windows) {"GetTickCount64"} else {"/proc/stat"},
        "margin_seconds":300,"observed_ticks":observed,"uptime_ticks":uptime,
        "boot_ticks":boot,"boot_time":time}))
}

pub(super) fn fresh(proof: &Value, current: u64) -> Result<()> {
    let observed = proof["observed_ticks"]
        .as_u64()
        .ok_or("主机证明缺少采集时刻")?;
    if current.saturating_sub(observed) > 1800 * TICKS
        || observed.saturating_sub(current) > 60 * TICKS
    {
        return Err("主机启动证明已经过期或不属于当前主机时刻".into());
    }
    Ok(())
}

pub(super) fn require_fresh(proof: &Value) -> Result<()> {
    fresh(proof, collect()?.0)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn public_boot_proof_retains_time_precision_margin_and_freshness() {
        let observed = 130_000_000_000_000_019;
        let proof = recorded(observed, 3600 * TICKS).unwrap();
        assert_eq!(proof["margin_seconds"], 300);
        assert_eq!(proof["boot_ticks"], observed - 3600 * TICKS);
        assert_eq!(proof["boot_time"], "2012-12-14T22:06:40.000001+00:00");
        assert_eq!(proof.as_object().unwrap().len(), 8);
        assert!(fresh(&proof, observed + 1800 * TICKS).is_ok());
        assert!(fresh(&proof, observed + 1800 * TICKS + 1).is_err());
        assert!(fresh(&proof, observed - 60 * TICKS - 1).is_err());
        assert!(recorded(observed, 0).is_err());
        assert!(recorded(observed, observed).is_err());
    }
}
