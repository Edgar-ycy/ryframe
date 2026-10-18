use std::collections::BTreeMap;

use serde_json::{Map, Value};

use crate::{
    Result,
    recovery::{
        clone::native::resources::common,
        monitoring::{
            contract::{ROLES, file_descriptor, process_identity},
            supervisor,
        },
    },
};

const STOP_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);

pub(crate) fn stop_trees(trees: &BTreeMap<String, Value>, crash: bool) -> Result<Value> {
    let mut completions = Map::new();
    let mut failures = Vec::new();
    for role in ROLES.iter().rev() {
        let Some(tree) = trees.get(*role) else {
            continue;
        };
        match stop_tree(tree, crash) {
            Ok(receipt) => {
                completions.insert((*role).to_owned(), receipt);
            }
            Err(error) => failures.push(format!("{role}: {error}")),
        }
    }
    if failures.is_empty() {
        Ok(Value::Object(completions))
    } else {
        Err(format!("监控进程树回收失败：{}", failures.join("；")).into())
    }
}

fn stop_tree(tree: &Value, crash: bool) -> Result<Value> {
    let supervisor_id = process_identity(&tree["supervisor"], "监控 supervisor")?;
    let process_id = process_identity(&tree["process"], "监控产品进程")?;
    let supervisor_alive =
        common::process_identity(supervisor_id.0)?.as_ref() == Some(&supervisor_id.1);
    let process_alive = common::process_identity(process_id.0)?.as_ref() == Some(&process_id.1);
    if supervisor_alive || process_alive {
        if !supervisor_alive {
            return Err("监控 supervisor 已退出但产品进程仍在，拒绝降级为单进程回收".into());
        }
        supervisor::request_stop(tree, crash)?;
    }
    let proof = supervisor::wait_stopped(tree, STOP_TIMEOUT)?;
    let deadline = std::time::Instant::now() + STOP_TIMEOUT;
    supervisor::identity_stopped(&supervisor_id.1, deadline, "监控 supervisor")?;
    supervisor::identity_stopped(&process_id.1, deadline, "监控产品进程")?;
    file_descriptor(
        &std::path::PathBuf::from(proof["directory"].as_str().ok_or("监控停止证明缺少目录")?).join(
            format!(
                "{}-members-{}-stopped.json",
                proof["role"].as_str().ok_or("监控停止证明缺少角色")?,
                proof["operation_id"]
                    .as_str()
                    .ok_or("监控停止证明缺少操作")?
            ),
        ),
    )
}
