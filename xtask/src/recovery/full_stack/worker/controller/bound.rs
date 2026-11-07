//! 同进程复制交接从冻结文件取得隔离环境；不改宿主环境，也不重放未完成操作。
use super::super::{environment::Environment, role::Role};
use crate::Result;
use serde_json::Value;
use std::{path::Path, time::Instant};

pub(crate) fn control_bound(
    role: Role,
    action: &str,
    backend: &Path,
    directory: &Path,
    timeout: f64,
    file: &Value,
) -> Result<Value> {
    let started = Instant::now();
    if action != "stop" {
        return Err("绑定交接控制只允许一次明确停止；未知原操作必须显式核对，不重放".into());
    }
    let environment = Environment::capture(file)?;
    super::control_verified(
        role,
        action,
        backend,
        directory,
        timeout,
        started,
        Some(&environment),
    )
}

pub(crate) fn observe_bound(
    role: Role,
    backend: &Path,
    directory: &Path,
    file: &Value,
) -> Result<Value> {
    let environment = Environment::capture(file)?;
    if role == Role::Api {
        require_no_pending(role, directory)?;
    }
    let result = super::observe(role, backend, directory);
    if role == Role::Api {
        require_no_pending(role, directory)?;
    }
    environment.check()?;
    result
}

pub(super) fn require_no_pending(role: Role, directory: &Path) -> Result<()> {
    match std::fs::symlink_metadata(directory.join(role.lock())) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
        Ok(_) => Err("角色尚有未完成控制证据；保留原操作并显式核对，不追加停止".into()),
    }
}

/// 显式回收只消费原已成功结果；不调用会停止中断启动的通用reconcile分支。
pub(crate) fn reconcile_completed_bound(
    backend: &Path,
    directory: &Path,
    file: &Value,
    original_controller: &Value,
) -> Result<Option<Value>> {
    use super::super::operation::{self, Operation};
    let role = Role::Api;
    match std::fs::symlink_metadata(directory.join(role.lock())) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
        Ok(_) => {}
    }
    let environment = Environment::capture(file)?;
    let guard = super::guard(role, directory, std::time::Duration::ZERO)?;
    let runtime = super::super::runtime::load(role, backend, directory)?;
    let source = operation::source(directory)?;
    let op = Operation::read(role, directory, &source)?;
    if op.owner["identity"] != *original_controller
        || op.request["backend_root"] != serde_json::json!(backend)
        || op.request["scope_id"] != runtime["scope_id"]
        || !matches!(op.request["operation"].as_str(), Some("start" | "stop"))
    {
        return Err("角色已完成结果不属于原外层控制器或固定运行合同，保留两层锁".into());
    }
    let pid = u32::try_from(
        original_controller["pid"]
            .as_u64()
            .ok_or("原外层控制器PID无效")?,
    )?;
    if crate::recovery::clone::native::resources::common::process_identity(pid)?.as_ref()
        == Some(original_controller)
    {
        return Err("控制器仍在运行，禁止释放运行控制锁".into());
    }
    if op.request["operation"] == "start"
        && op.request.get("environment_binding") != Some(environment.binding())
    {
        return Err("原API启动环境与冻结交接不同，保留原结果与两层锁".into());
    }
    let result = op.validate("result", operation::read(&op.path.join("result.json"))?)?;
    if result["outcome"] != "succeeded"
        || !matches!(result["state"].as_str(), Some("running" | "stopped"))
    {
        return Err("角色结果仍未知或失败；仅保留原生证据，不补发start或stop".into());
    }
    let before = super::observe(role, backend, directory)?;
    if before["state"] != result["state"] || before["identity"] != result["identity"] {
        return Err("原已成功角色结果与当前完整树不同，保留原结果与两层锁".into());
    }
    environment.check()?;
    guard.check()?;
    // archive复用原字段、进度和来源校验，原样归档；随后父入口还必须核对完整树并精确回收外层锁。
    let archived = op.archive()?;
    if super::observe(role, backend, directory)? != before {
        return Err("已成功角色结果归档后完整树变化，保留归档与原外层锁".into());
    }
    guard.check()?;
    environment.check()?;
    Ok(Some(archived))
}
