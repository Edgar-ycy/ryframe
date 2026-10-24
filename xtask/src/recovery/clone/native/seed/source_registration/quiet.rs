//! 历史创建代次的停止证明；CIM 失败必须拒绝，不发送任何进程信号。
use std::path::Path;

use serde_json::Value;

use crate::{Result, recovery::clone::native::lifecycle::identity};

use observation::CimObservation;

#[path = "quiet/cim.rs"]
pub(crate) mod cim;
#[path = "quiet/observation.rs"]
pub(crate) mod observation;
#[path = "quiet/timing.rs"]
pub(crate) mod timing;

enum Inspection {
    Gone,
    Present(Value),
    AccessDenied,
}

#[derive(Debug)]
pub(crate) struct CimUnavailable {
    pub(crate) code: Option<i32>,
}

impl std::fmt::Display for CimUnavailable {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let code = self
            .code
            .map(|code| format!("0x{:08X}", code as u32))
            .unwrap_or_else(|| "未提供 HRESULT".into());
        write!(
            formatter,
            "Windows 内核进程查询拒绝访问（错误码 5），原生 CIM 查询失败（{code}）；未证明历史进程已停止，也未发送进程信号。请检查 Winmgmt 服务和本机 ROOT\\CIMV2 读取权限，修复后重新执行原核验命令；禁止伪造停止状态或重放数据写入。"
        )
    }
}

impl std::error::Error for CimUnavailable {}

pub(crate) fn require_stopped(
    backend: &Path,
    run: &Path,
    expected: &Value,
    tool: &Path,
) -> Result<()> {
    verify_with(
        expected,
        &mut |pid| match identity::process(pid) {
            Ok(None) => Ok(Inspection::Gone),
            Ok(Some(value)) => Ok(Inspection::Present(value)),
            Err(error) if cfg!(windows) && identity::is_access_denied(error.as_ref()) => {
                Ok(Inspection::AccessDenied)
            }
            Err(error) => Err(error),
        },
        &mut |pid| observation::cim(backend, run, pid, tool),
    )
}

fn verify_with(
    expected: &Value,
    inspect: &mut dyn FnMut(u32) -> Result<Inspection>,
    cim: &mut dyn FnMut(u32) -> Result<CimObservation>,
) -> Result<()> {
    let (pid, original) = timing::recorded(expected)?;
    match inspect(pid)? {
        Inspection::Gone => Ok(()),
        Inspection::Present(current) => {
            let (observed_pid, created) = timing::recorded(&current)?;
            if observed_pid != pid || created <= original {
                return Err("登记进程尚未停止或当前创建代次不能证明更新；不得继续登记".into());
            }
            Ok(())
        }
        Inspection::AccessDenied => match cim(pid)? {
            CimObservation::Observed(creation) => timing::require_newer(expected, &creation),
            CimObservation::QueryFailed { code } => Err(Box::new(CimUnavailable { code })),
        },
    }
}

#[cfg(test)]
#[path = "../../../../../../tests/unit/recovery/clone/native/seed/source_registration/quiet.rs"]
mod tests;
