use super::{
    documents,
    operation::{self, Operation},
    process,
    role::Role,
};
use crate::{Result, recovery::clone::native::json};
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    thread,
    time::{Duration, Instant},
};

pub(super) fn path(directory: &Path, role: Role, id: &str, suffix: &str) -> PathBuf {
    directory.join(format!("{}-members-{id}-{suffix}.json", role.name()))
}
pub(super) fn read(
    directory: &Path,
    runtime: &Value,
    role: Role,
    required: bool,
) -> Result<Option<Value>> {
    let path = directory.join(format!("{}-tree.json", role.name()));
    if !path.exists() {
        if required {
            return Err("已登记角色缺少进程树收据".into());
        }
        return Ok(None);
    }
    let tree = operation::read(&path)?;
    json::exact(
        &tree,
        &[
            "format_version",
            "kind",
            "runtime_directory",
            "role",
            "scope_id",
            "operation_id",
            "supervisor",
            "process",
            "monitor",
            "group_id",
        ],
        "角色进程树",
    )?;
    if tree["format_version"].as_u64() != Some(2)
        || tree["kind"] != "full-stack-process-tree"
        || tree["runtime_directory"] != json!(directory)
        || tree["role"] != role.name()
        || tree["scope_id"] != runtime["scope_id"]
        || !documents::hex(&tree["operation_id"], 32)
        || tree["group_id"] != tree["supervisor"]["pid"]
        || tree["monitor"]["pid"] == tree["supervisor"]["pid"]
        || tree["monitor"]["pid"] == tree["process"]["pid"]
    {
        return Err("角色进程树与运行目录、scope或成员不匹配".into());
    }
    for field in ["process", "supervisor", "monitor"] {
        documents::identity(&tree[field])?;
    }
    let receipt = directory.join(format!("{}.json", role.name()));
    if receipt.exists() {
        if operation::read(&receipt)?["identity"] != tree["process"] {
            return Err("角色产品和进程树收据不一致".into());
        }
    } else if required {
        return Err("角色进程树缺少产品收据".into());
    }
    Ok(Some(tree))
}
pub(super) fn running(
    directory: &Path,
    runtime: &Value,
    role: Role,
    identity: &Value,
) -> Result<Value> {
    let tree = read(directory, runtime, role, true)?.ok_or("缺少角色进程树")?;
    if tree["process"] != *identity
        || !process::alive(&tree["supervisor"])?
        || !process::alive(&tree["monitor"])?
    {
        return Err("角色完整成员监督器未保持运行".into());
    }
    Ok(tree)
}
pub(super) fn register(
    op: &Operation,
    supervisor: &Value,
    monitor: &Value,
    product: &Value,
) -> Result<Value> {
    let tree = json!({"format_version":2,"kind":"full-stack-process-tree","runtime_directory":op.directory(),"role":op.role.name(),"scope_id":op.request["scope_id"],"operation_id":op.owner["operation_id"],"supervisor":supervisor,"monitor":monitor,"process":product,"group_id":supervisor["pid"]});
    // 新操作只覆盖已确认关闭的上一代；调用者已持有控制锁并核验完整回收。
    json::write_replace(
        &op.directory().join(format!("{}-tree.json", op.role.name())),
        &tree,
    )?;
    json::write_replace(
        &op.directory().join(format!("{}.json", op.role.name())),
        &json!({"format_version":1,"role":op.role.name(),"scope_id":op.request["scope_id"],"identity":product}),
    )?;
    Ok(tree)
}
pub(super) fn closure(tree: &Value, timeout: Duration, external: bool) -> Result<Value> {
    let directory = Path::new(tree["runtime_directory"].as_str().ok_or("缺少运行目录")?);
    let id = tree["operation_id"].as_str().ok_or("缺少操作ID")?;
    let role = Role::parse(&tree["role"])?;
    let proof = path(directory, role, id, "stopped");
    let deadline = Instant::now() + timeout;
    loop {
        let monitor_alive = process::alive(&tree["monitor"])?;
        if proof.exists() {
            let value = operation::read(&proof)?;
            validate_proof(tree, &value)?;
            if !monitor_alive {
                for field in ["supervisor", "process"] {
                    if proof_member_alive(tree, &tree[field])? {
                        return Err("完整成员关闭证明存在但登记进程仍存活".into());
                    }
                }
                return Ok(value);
            }
        } else if !monitor_alive {
            if external {
                return external_termination(tree);
            }
            return Err("成员监督器退出但没有完整成员关闭证明".into());
        }
        if Instant::now() >= deadline {
            return Err("完整成员监督器未在期限内完成回收".into());
        }
        thread::sleep(Duration::from_millis(20));
    }
}
fn validate_proof(tree: &Value, value: &Value) -> Result<()> {
    json::exact(
        value,
        &[
            "format_version",
            "operation_id",
            "directory",
            "role",
            "scope_id",
            "supervisor",
            "monitor",
            "status",
            "members",
            "error_type",
        ],
        "角色完整成员证明",
    )?;
    if value["format_version"].as_u64() != Some(1)
        || value["directory"] != tree["runtime_directory"]
        || ["operation_id", "role", "scope_id", "supervisor", "monitor"]
            .iter()
            .any(|key| value[key] != tree[key])
        || value["status"] != "stopped"
        || !value["error_type"].is_null()
    {
        return Err("角色完整成员关闭证明不匹配或失败".into());
    }
    let members = value["members"].as_array().ok_or("完整成员证明缺少成员")?;
    if !members.contains(&tree["supervisor"]) {
        return Err("完整成员证明缺少原监督进程".into());
    }
    for member in members {
        documents::identity(member)?;
        if proof_member_alive(tree, member)? {
            return Err("登记成员尚未退出".into());
        }
    }
    Ok(())
}

fn proof_member_alive(tree: &Value, member: &Value) -> Result<bool> {
    #[cfg(target_os = "linux")]
    {
        use crate::recovery::process_tree::observe_process_group_member;

        let (Some(pid), Some(group)) = (member["pid"].as_u64(), tree["group_id"].as_u64()) else {
            return Err("整组关闭证明缺少成员 PID 或进程组".into());
        };
        let observed = observe_process_group_member(u32::try_from(pid)?, u32::try_from(group)?)?;
        match observed {
            None => Ok(false),
            Some(actual) if actual.identity() == member => Ok(!actual.exited()?),
            Some(_) => Err("进程PID已复用，拒绝操作其他创建身份".into()),
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = tree;
        process::alive(member)
    }
}

fn external_termination(tree: &Value) -> Result<Value> {
    for field in ["supervisor", "monitor", "process"] {
        if process::alive(&tree[field])? {
            return Err("登记进程仍存活，拒绝登记外部终止".into());
        }
    }
    let directory = Path::new(tree["runtime_directory"].as_str().ok_or("运行目录无效")?);
    let id = tree["operation_id"].as_str().ok_or("操作ID无效")?;
    let role = Role::parse(&tree["role"])?;
    let mut value = json!({"format_version":1,"kind":"full-stack-external-termination","operation_id":id,"directory":directory,"role":role.name(),"scope_id":tree["scope_id"],"status":"external-termination","identities":{"supervisor":"missing","monitor":"missing","process":"missing"},"normal_shutdown_proof":null,"checked":{"supervisor":tree["supervisor"],"monitor":tree["monitor"],"process":tree["process"]},"created_at_ns":operation::now()});
    let path = directory.join(format!("{}-external-termination-{id}.json", role.name()));
    if path.exists() {
        let prior = operation::read(&path)?;
        value["created_at_ns"] = prior["created_at_ns"].clone();
        if value != prior || !documents::positive(&prior["created_at_ns"]) {
            return Err("外部终止记录与当前进程树不一致".into());
        }
        return Ok(prior);
    }
    operation::write_new(&path, &value)?;
    Ok(value)
}
pub(super) fn stop(tree: &Value, crash: bool) -> Result<()> {
    for field in ["supervisor", "process", "monitor"] {
        process::alive(&tree[field])?;
    }
    if !process::alive(&tree["supervisor"])? && !process::alive(&tree["process"])? {
        return closure(tree, Duration::from_secs(10), true).map(|_| ());
    }
    let directory = Path::new(tree["runtime_directory"].as_str().ok_or("缺少运行目录")?);
    let role = Role::parse(&tree["role"])?;
    let recorded = operation::read(&directory.join("runtime.json"))?;
    let backend = Path::new(
        recorded[if role == Role::Frontend {
            "coordinator"
        } else {
            "backend_root"
        }]
        .as_str()
        .ok_or("进程树缺少原协调器根")?,
    );
    let runtime = super::runtime::load(role, backend, directory)?;
    if read(directory, &runtime, role, true)?.as_ref() != Some(tree) {
        return Err("角色进程树在停止前漂移".into());
    }
    let control = directory.join(format!(
        "{}-tree-{}-control.json",
        role.name(),
        tree["operation_id"].as_str().ok_or("操作ID无效")?
    ));
    let value = json!({"format_version":1,"kind":"full-stack-process-tree-control","operation_id":tree["operation_id"],"mode":if crash{"crash"}else{"normal"},"requested_at_ns":operation::now()});
    if control.exists() {
        let prior = operation::read(&control)?;
        if prior["operation_id"] != value["operation_id"] || prior["mode"] != value["mode"] {
            return Err("角色已有不同停止意图，保留现场".into());
        }
    } else {
        operation::write_new(&control, &value)?;
    }
    closure(tree, Duration::from_secs(15), true).map(|_| ())
}
pub(super) fn startup_cleanup(op: &Operation, supervisor: &Value) -> Result<()> {
    let ready = path(op.directory(), op.role, op.id()?, "ready");
    if !ready.exists() {
        return Err("已授权启动缺少成员监督器归属，保留失败现场".into());
    }
    let ready = operation::read(&ready)?;
    json::exact(
        &ready,
        &[
            "format_version",
            "operation_id",
            "directory",
            "role",
            "scope_id",
            "supervisor",
            "monitor",
        ],
        "角色启动成员证明",
    )?;
    if ready["supervisor"] != *supervisor
        || ready["operation_id"] != op.owner["operation_id"]
        || ready["directory"] != json!(op.directory())
        || ready["scope_id"] != op.request["scope_id"]
    {
        return Err("启动成员证据不属于当前授权".into());
    }
    let tree = json!({"runtime_directory":op.directory(),"operation_id":op.owner["operation_id"],"role":op.role.name(),"scope_id":op.request["scope_id"],"supervisor":supervisor,"monitor":ready["monitor"],"process":supervisor});
    closure(&tree, Duration::from_secs(10), false).map(|_| ())
}
