//! 只读取原监督器发布的完整树与成员关闭证据，不补造或修复历史。
use crate::{
    Result,
    recovery::clone::native::{json as document, resources::common},
};
use serde_json::{Value, json};
use std::path::Path;

pub(super) fn read(backend: &Path, output: &Path, scope: &str) -> Result<Value> {
    common::local_directory(backend, output, "夹具进程目录")?;
    let filename = output.join("rustfs-tree.json");
    let binding = common::binding(&filename)?;
    if binding["bytes"]
        .as_u64()
        .is_none_or(|bytes| bytes > 64 * 1024)
    {
        return Err("进程树收据超过既定读取上限".into());
    }
    let tree = common::read_bound(backend, &binding, "夹具进程树")?;
    document::exact(
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
        "夹具进程树",
    )?;
    if tree["format_version"].as_u64() != Some(2)
        || tree["kind"] != "full-stack-process-tree"
        || tree["runtime_directory"] != json!(output)
        || tree["role"] != "rustfs"
        || tree["scope_id"] != scope
    {
        return Err("进程树收据与角色、目录或 scope 不匹配".into());
    }
    for role in ["supervisor", "process", "monitor"] {
        common::validate_process_identity(&tree[role])?;
    }
    let operation = document::text(&tree["operation_id"], "进程树操作 ID")?;
    if operation.len() != 32
        || !operation
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        || tree["group_id"] != tree["supervisor"]["pid"]
        || tree["monitor"]["pid"] == tree["supervisor"]["pid"]
        || tree["monitor"]["pid"] == tree["process"]["pid"]
    {
        return Err("进程树收据缺少可信进程组与操作 ID".into());
    }
    common::bound_file(backend, &binding, "夹具进程树")?;
    Ok(tree)
}

pub(super) fn members(backend: &Path, tree: &Value, timeout: std::time::Duration) -> Result<Value> {
    let deadline = std::time::Instant::now() + timeout;
    loop {
        match members_once(backend, tree) {
            Ok(Some(binding)) => return Ok(binding),
            Ok(None) => (),
            Err(error)
                if (crate::recovery::clone::native::lifecycle::identity::is_access_denied(
                    error.as_ref(),
                ) || error
                    .downcast_ref::<std::io::Error>()
                    .is_some_and(|error| error.kind() == std::io::ErrorKind::PermissionDenied))
                    && std::time::Instant::now() < deadline => {}
            Err(error) => return Err(error),
        }
        if std::time::Instant::now() >= deadline {
            return Err("成员监督器未在原定期限内退出并完成整树回收；保留现场".into());
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

fn members_once(backend: &Path, tree: &Value) -> Result<Option<Value>> {
    let root = Path::new(document::text(&tree["runtime_directory"], "进程树目录")?);
    common::local_directory(backend, root, "进程树目录")?;
    let role = document::text(&tree["role"], "进程树角色")?;
    let operation = document::text(&tree["operation_id"], "进程树操作")?;
    if !matches!(role, "rustfs" | "api" | "worker")
        || operation.len() != 32
        || !operation
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err("进程树角色或操作 ID 无效".into());
    }
    let monitor = common::validate_process_identity(&tree["monitor"])?;
    let pid = u32::try_from(monitor["pid"].as_u64().ok_or("成员监督器 PID 无效")?)?;
    let before = common::process_identity(pid)?;
    if before.as_ref().is_some_and(|actual| actual != &monitor) {
        return Err("成员监督器 PID 已复用，拒绝认定关闭成功".into());
    }
    let filename = root.join(format!("{role}-members-{operation}-stopped.json"));
    match std::fs::symlink_metadata(&filename) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            if before.is_none() {
                return Err("成员监督器退出但没有完整成员关闭证明".into());
            }
            return Ok(None);
        }
        Err(error) => return Err(error.into()),
        Ok(_) => (),
    }
    let binding = common::binding(&filename)?;
    if binding["bytes"]
        .as_u64()
        .is_none_or(|bytes| bytes > 4 * 1024 * 1024)
    {
        return Err("进程树成员关闭证明超过既定上限".into());
    }
    let proof = common::read_bound(backend, &binding, "进程树成员关闭证明")?;
    validate_members(tree, &proof)?;
    if before.is_some() || common::process_identity(pid)?.is_some() {
        return Ok(None);
    }
    common::bound_file(backend, &binding, "进程树成员关闭证明")?;
    Ok(Some(binding))
}

fn validate_members(tree: &Value, proof: &Value) -> Result<()> {
    document::exact(
        proof,
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
        "成员关闭证明",
    )?;
    for field in ["operation_id", "role", "scope_id", "supervisor", "monitor"] {
        if proof[field] != tree[field] {
            return Err("进程树成员关闭证明不属于本次进程树".into());
        }
    }
    if proof["directory"] != tree["runtime_directory"]
        || proof["status"] != "stopped"
        || !proof["error_type"].is_null()
    {
        return Err("进程树成员回收未完成，保留原始证明".into());
    }
    let members = document::array(&proof["members"], "实际成员")?;
    if members.is_empty() || !members.contains(&tree["supervisor"]) {
        return Err("进程树成员关闭证明缺少原监督进程".into());
    }
    for member in members {
        common::validate_process_identity(member)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn complete_tree_is_read_without_rewriting_and_foreign_scope_is_rejected() {
        let backend = crate::workspace::root_dir();
        let parent = backend.join(".local-tests/fixture-service-tree");
        std::fs::create_dir_all(&parent).unwrap();
        let directory = tempfile::tempdir_in(parent).unwrap();
        let identity =
            |pid| json!({"pid":pid,"started":"1001","executable":backend.join("fixture.exe")});
        let original = json!({"format_version":2,"kind":"full-stack-process-tree",
            "runtime_directory":directory.path(),"role":"rustfs","scope_id":"tree-scope",
            "operation_id":"a".repeat(32),"supervisor":identity(101),"process":identity(102),
            "monitor":identity(103),"group_id":101});
        let filename = directory.path().join("rustfs-tree.json");
        document::write_new(&filename, &original).unwrap();
        let before = std::fs::read(&filename).unwrap();
        assert_eq!(
            read(&backend, directory.path(), "tree-scope").unwrap(),
            original
        );
        assert!(read(&backend, directory.path(), "foreign-scope").is_err());
        assert_eq!(std::fs::read(&filename).unwrap(), before);
        let mut forged = original.clone();
        forged["monitor"] = forged["process"].clone();
        std::fs::write(&filename, serde_json::to_vec(&forged).unwrap()).unwrap();
        assert!(read(&backend, directory.path(), "tree-scope").is_err());
    }

    #[test]
    fn member_proof_requires_original_supervisor_and_no_failed_or_foreign_completion() {
        let identity = |pid| json!({"pid":pid,"started":"1001","executable":crate::workspace::root_dir().join("fixture.exe")});
        let tree = json!({"operation_id":"a".repeat(32),"runtime_directory":"fixed",
            "role":"rustfs","scope_id":"tree-scope","supervisor":identity(101),"monitor":identity(103)});
        let proof = json!({"format_version":1,"operation_id":tree["operation_id"],"directory":"fixed",
            "role":"rustfs","scope_id":"tree-scope","supervisor":tree["supervisor"],
            "monitor":tree["monitor"],"status":"stopped","members":[identity(101)],"error_type":null});
        validate_members(&tree, &proof).unwrap();
        for (field, replacement) in [
            ("members", json!([identity(104)])),
            ("error_type", json!("UnknownMembers")),
            ("operation_id", json!("b".repeat(32))),
        ] {
            let mut changed = proof.clone();
            changed[field] = replacement;
            assert!(validate_members(&tree, &changed).is_err(), "{field}");
        }
    }
}
