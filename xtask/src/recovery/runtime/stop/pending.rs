//! 只验证已登记的无产品树启动切点，不对其进程发信号。
use std::path::Path;

use serde_json::{Value, json};

use super::{Inputs, check_restore_completion, optional_file, restore_completion, tree};
use crate::{
    Result,
    recovery::clone::native::{json as strict, resources::common},
};

pub(super) fn verify(
    backend: &Path,
    directory: &Path,
    generation: &Value,
    role: &str,
    inputs: &mut Inputs,
) -> Result<()> {
    let operation = strict::text(&generation["roles"][role]["operation_id"], "原角色启动操作")?;
    let ready_path = directory.join(format!("{role}-members-{operation}-ready.json"));
    let completion_path = directory.join(format!("{role}-members-{operation}-stopped.json"));
    let completed = optional_file(&completion_path)?;
    if !optional_file(&ready_path)? {
        if completed
            || optional_file(&directory.join(format!("{role}-tree-{operation}-control.json")))?
            || optional_file(&directory.join(format!("{role}-tree-{operation}-result.json")))?
        {
            return Err("原角色存在启动或关闭收据但缺少完整树和原 ready，保留现场".into());
        }
        return Ok(());
    }
    if !completed {
        return Err("原角色已有 ready 但没有完整树或原成员完成收据，保留待定状态".into());
    }
    if std::fs::symlink_metadata(&ready_path)?.len() > 64 * 1024 {
        return Err("原成员 ready 收据超过 64 KiB，保留现场".into());
    }
    let ready = inputs.read(&ready_path)?;
    let ready = &ready.value;
    strict::exact(
        ready,
        &[
            "format_version",
            "operation_id",
            "directory",
            "role",
            "scope_id",
            "supervisor",
            "monitor",
        ],
        "原恢复角色成员 ready",
    )?;
    let scope = &generation["request"]["authority"]["scope_id"];
    if ready["format_version"] != 1
        || ready["operation_id"] != operation
        || ready["directory"] != json!(directory)
        || ready["role"] != role
        || ready["scope_id"] != *scope
        || ready["supervisor"]["pid"] == ready["monitor"]["pid"]
    {
        return Err("原成员 ready 与当代角色的操作或身份不同，保留现场".into());
    }
    common::validate_process_identity(&ready["supervisor"])?;
    common::validate_process_identity(&ready["monitor"])?;
    // 只投影现有完成核验器需要的证据字段。没有产品树，不能建立进程控制权。
    let authority = json!({
        "runtime_directory":directory, "role":role, "operation_id":operation, "scope_id":scope,
        "supervisor":ready["supervisor"], "monitor":ready["monitor"],
    });
    let bound = restore_completion(backend, &authority)?;
    let proof = inputs.bound(&bound)?;
    check_restore_completion(&authority, &proof.value)?;
    for member in strict::array(&proof.value["members"], "原已停止成员")? {
        tree::require_exited(member)?;
    }
    Ok(())
}
