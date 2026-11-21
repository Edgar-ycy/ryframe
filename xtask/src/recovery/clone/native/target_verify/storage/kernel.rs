use serde_json::Value;

use super::common;
use crate::Result;
pub(crate) fn require_observed_identity(
    identity: &Value,
    observer: &impl super::StorageObserver,
) -> Result<()> {
    check_identity(identity, |pid| observer.process_identity(pid))
}

fn check_identity(
    identity: &Value,
    observe: impl FnOnce(u32) -> Result<Option<Value>>,
) -> Result<()> {
    let identity = common::validate_process_identity(identity)?;
    let pid = u32::try_from(identity["pid"].as_u64().ok_or("存储 PID 无效")?)
        .map_err(|_| "存储 PID 超出范围")?;
    if observe(pid)?.as_ref() != Some(&identity) {
        return Err("RustFS 观察窗口中的内核进程身份变化".into());
    }
    Ok(())
}
