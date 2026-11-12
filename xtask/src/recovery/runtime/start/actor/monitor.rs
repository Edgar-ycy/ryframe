//! 树外 monitor 持有本次 Job 或 session，完成整树关闭后才发布原成员证明。
use super::Actor;
use crate::{
    Result,
    local_test_path::{LocalTestPathKind, validate_absolute_path},
    recovery::{
        clone::native::{digest, lifecycle::identity, resources::common},
        process_tree::Group,
        runtime::generation::receipts,
    },
};
use serde_json::{Value, json};
use std::{fs, io::ErrorKind, path::Path, time::Duration};

pub(super) fn run(actor: &Actor) -> Result<()> {
    let monitor = identity::current()?;
    let directory = directory_identity(&actor.directory)?;
    let command = actor.command("supervise")?;
    actor.verify_start()?;
    for path in [
        actor.member_path("ready"),
        actor.member_path("stopped"),
        actor.directory.join(format!("{}-tree.json", actor.role)),
        actor.directory.join(format!("{}.json", actor.role)),
    ] {
        require_absent(&path)?;
    }
    verify_directory(actor, directory)?;
    let mut group = Group::spawn(command, Duration::from_secs(5))?;
    let ready = json!({
        "format_version":1, "operation_id":actor.operation_id, "directory":actor.directory,
        "role":actor.role, "scope_id":actor.scope, "supervisor":group.identity(), "monitor":monitor,
    });
    let registered = publish_member(actor, directory, "ready", &ready);
    let owns_ready = registered.is_ok();
    let observation = registered.and_then(|()| observe(actor, directory, &ready, &mut group));
    // 正常退出和观察失败均先显式回收本次 Group，再用实际捕获成员签发证明。
    let cleanup = group.close();
    let members = group.members();
    let observation = observation.and_then(|registered| {
        verify_directory(actor, directory)?;
        let current = original_tree(actor)?;
        if registered.is_some() && current != registered {
            return Err("已观察的原恢复进程树在成员关闭期间变化".into());
        }
        if let Some(tree) = current {
            verify_tree(&ready, &tree, &members)?;
        }
        Ok(())
    });
    let published = if owns_ready {
        let proof = completion(ready, members, observation.is_ok(), cleanup.is_ok());
        publish_member(actor, directory, "stopped", &proof)
    } else {
        // 不能为另一原 ready 或未确认发布的文件补签新的完成证明。
        Ok(())
    };
    combine(
        combine(observation, cleanup, "原成员整树关闭"),
        published,
        "原成员完成收据发布",
    )
}

fn observe(
    actor: &Actor,
    directory: (u64, u64),
    ready: &Value,
    group: &mut Group,
) -> Result<Option<Value>> {
    let mut registered = None;
    loop {
        group.capture()?;
        let supervisor_exited = group.supervisor_exited()?;
        if registered.is_none() {
            verify_directory(actor, directory)?;
            let mut tree = original_tree(actor)?;
            let controlled_before_tree =
                !supervisor_exited && tree.is_none() && actor.control()?.is_some();
            if controlled_before_tree {
                // 监督器可能已发布树；重新观察后只在尚无树时关闭自有 Group。
                tree = original_tree(actor)?;
            }
            if let Some(tree) = tree {
                group.capture()?;
                verify_tree(ready, &tree, &group.members())?;
                registered = Some(tree);
            } else if controlled_before_tree {
                return Ok(None);
            }
        }
        // 已登记完整树的控制由树内 supervisor 读取和执行。
        if supervisor_exited {
            return Ok(registered);
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn require_absent(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(_) => Err("原角色成员或进程树收据已存在，保留现场且不启动".into()),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

fn original_tree(actor: &Actor) -> Result<Option<Value>> {
    let path = actor.directory.join(format!("{}-tree.json", actor.role));
    match fs::symlink_metadata(&path) {
        Ok(metadata)
            if metadata.is_file()
                && !digest::linked(&metadata)
                && metadata.len() > 0
                && metadata.len() <= 64 * 1024 =>
        {
            receipts::tree(&actor.directory, actor.role, &actor.scope).map(Some)
        }
        Ok(_) => Err("原恢复进程树必须是无链接的非空普通文件且不超过 64 KiB".into()),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

fn verify_tree(ready: &Value, tree: &Value, members: &[Value]) -> Result<()> {
    if tree["runtime_directory"] != ready["directory"]
        || ["role", "scope_id", "operation_id", "supervisor", "monitor"]
            .iter()
            .any(|field| tree[*field] != ready[*field])
        || tree["process"]["pid"] == tree["supervisor"]["pid"]
        || !members.contains(&tree["supervisor"])
        || !members.contains(&tree["process"])
    {
        return Err("恢复进程树与原操作、树外 monitor 或实际持有的成员不同".into());
    }
    Ok(())
}

fn completion(mut ready: Value, members: Vec<Value>, observed: bool, closed: bool) -> Value {
    let error = match (observed, closed) {
        (true, true) => None,
        (false, true) => Some("MemberObservationFailed"),
        (true, false) => Some("MemberCleanupFailed"),
        (false, false) => Some("MemberObservationFailed/MemberCleanupFailed"),
    };
    ready["status"] = json!(if error.is_none() { "stopped" } else { "failed" });
    ready["members"] = json!(members);
    ready["error_type"] = json!(error);
    ready
}

fn publish_member(actor: &Actor, directory: (u64, u64), suffix: &str, value: &Value) -> Result<()> {
    verify_directory(actor, directory)?;
    actor.publish_receipt(&actor.member_path(suffix), value)?;
    verify_directory(actor, directory)
}

fn verify_directory(actor: &Actor, expected: (u64, u64)) -> Result<()> {
    if directory_identity(&actor.directory)? != expected {
        return Err("原恢复运行代次目录身份变化，保留成员现场".into());
    }
    Ok(())
}

fn directory_identity(directory: &Path) -> Result<(u64, u64)> {
    validate_absolute_path(directory, LocalTestPathKind::ExistingDirectory)?;
    common::device_inode_for(directory, &fs::symlink_metadata(directory)?)
}

fn combine(first: Result<()>, second: Result<()>, stage: &str) -> Result<()> {
    match (first, second) {
        (Ok(()), result) | (result, Ok(())) => result,
        (Err(error), Err(after)) => Err(format!("{error}；{stage}同时失败：{after}").into()),
    }
}

#[cfg(test)]
#[path = "../../../../../tests/unit/recovery/runtime/start/actor/monitor.rs"]
mod tests;
