//! 没有等价安全成员句柄的平台必须在启动前拒绝。
use super::*;
pub(super) fn supported() -> Result<()> {
    Err("RustFS 完整监督当前只支持 Windows/Linux；不降级裸 PID".into())
}
pub(super) struct Group;
impl Group {
    pub(super) fn spawn(_command: std::process::Command) -> Result<Self> {
        supported()?;
        Ok(Self)
    }
    pub(super) fn identity(&self) -> &Value {
        &Value::Null
    }
    pub(super) fn members(&self) -> Vec<Value> {
        vec![]
    }
    pub(super) fn capture(&mut self) -> Result<()> {
        supported()
    }
    pub(super) fn supervisor_exited(&self) -> Result<bool> {
        supported()?;
        Ok(false)
    }
    pub(super) fn close(&mut self) -> Result<()> {
        supported()
    }
}
