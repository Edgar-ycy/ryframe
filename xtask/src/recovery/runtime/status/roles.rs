use serde_json::{Value, json};
use std::{fs, path::Path};

use super::{
    super::{
        dataset_authority::{inputs::Inputs, path},
        generation::{ROLES, receipts},
        verify::same_path,
    },
    optional_file,
};
use crate::{
    Result,
    recovery::{
        clone::native::{resources::common, target_verify::authority::require_closed},
        fixture_control::services::lifecycle::readonly::{process, restore_completion},
        inputs::{document, domain},
    },
};

pub(in crate::recovery::runtime) fn observe(
    backend: &Path,
    generation: &Value,
    inputs: &mut Inputs,
) -> Result<Value> {
    let directory = path(&generation["directory"])?;
    let mut result = json!({});
    for role in ROLES {
        result[role] = role_state(backend, &directory, generation, role, inputs)?;
    }
    Ok(result)
}

fn role_state(
    backend: &Path,
    directory: &Path,
    generation: &Value,
    role: &str,
    inputs: &mut Inputs,
) -> Result<Value> {
    let item = &generation["roles"][role];
    let authority = &generation["request"]["authority"];
    let endpoint = &authority[format!("{role}_endpoint")];
    let url = document::text(endpoint, "恢复角色端点")?;
    let has_process = optional_file(&directory.join(format!("{role}.json")))?;
    let tree_path = directory.join(format!("{role}-tree.json"));
    if !optional_file(&tree_path)? {
        if has_process {
            return Err(format!("{role} 进程收据存在但缺少 Job 或进程组树证据").into());
        }
        require_closed(url)?;
        return Ok(observation(
            "pending",
            &Value::Null,
            &Value::Null,
            Value::Null,
        ));
    }
    if fs::symlink_metadata(&tree_path)?.len() > 64 * 1024 {
        return Err(format!("{role} 完整进程树收据超过 64 KiB").into());
    }
    let scope = &authority["scope_id"];
    let tree = receipts::tree(directory, role, scope)?;
    let identity = &tree["process"];
    if has_process && receipts::process(directory, role, scope)?.0 != *identity {
        return Err(format!("{role} 产品进程收据与完整树不一致").into());
    }
    if tree["operation_id"] != item["operation_id"] {
        return Err(format!("{role} 进程树与启动 intent 不一致").into());
    }
    let pid = u32::try_from(document::integer(&identity["pid"], "恢复角色 PID")?)?;
    if let Some(actual) = common::process_identity(pid)? {
        if actual != *identity {
            return Err(format!("{role} PID 已复用，拒绝认定原进程运行").into());
        }
        if !same_path(&path(&identity["executable"])?, &path(&item["command"][0])?) {
            return Err(format!("{role} 进程没有运行登记的可执行文件").into());
        }
        let endpoint = domain::endpoint(endpoint, "恢复角色监听端点", None)?;
        process::listener_at(identity, endpoint.host.parse()?, endpoint.port)?;
        return Ok(observation("running", identity, &tree, Value::Null));
    }
    let completion = restore_completion(backend, &tree)?;
    inputs.bound(&completion)?;
    require_closed(url)?;
    Ok(observation("stopped", identity, &tree, completion))
}

fn observation(state: &str, identity: &Value, tree: &Value, completion: Value) -> Value {
    json!({"state":state,"identity":identity,"tree":tree,"completion":completion})
}
