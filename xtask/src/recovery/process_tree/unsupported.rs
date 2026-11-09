//! 没有等价安全成员句柄的平台必须在启动前拒绝。
use crate::Result;
use serde_json::Value;
use std::time::Duration;
pub(in crate::recovery) fn supported() -> Result<()> {
    Err("RustFS 完整监督当前只支持 Windows/Linux；不降级裸 PID".into())
}
pub(in crate::recovery) struct Group;
impl Group {
    pub(in crate::recovery) fn spawn(
        _command: std::process::Command,
        _session_timeout: Duration,
    ) -> Result<Self> {
        supported()?;
        Ok(Self)
    }
    pub(in crate::recovery) fn identity(&self) -> &Value {
        &Value::Null
    }
    pub(in crate::recovery) fn members(&self) -> Vec<Value> {
        vec![]
    }
    pub(in crate::recovery) fn capture(&mut self) -> Result<()> {
        supported()
    }
    pub(in crate::recovery) fn supervisor_exited(&self) -> Result<bool> {
        supported()?;
        Ok(false)
    }
    pub(in crate::recovery) fn close(&mut self) -> Result<()> {
        supported()
    }
}
