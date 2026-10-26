//! START 的运行时证据实际复核；历史文件不能替代存活身份、监听和 ready 观察。
use std::{collections::BTreeSet, path::Path};

use serde_json::{Value, json};

use crate::{
    Result,
    recovery::{
        clone::native::{digest, json as document, resources::common},
        fixture_control,
    },
};

#[path = "running/builds.rs"]
pub(super) mod builds;
#[path = "running/facts.rs"]
mod facts;
#[path = "running/history.rs"]
pub(super) mod history;
pub(super) use facts::load;
#[path = "running/roles.rs"]
pub(super) mod roles;

pub(super) fn runtime(
    backend: &Path,
    output: &Path,
    request: &Value,
    selected: &Value,
    physical: &Value,
    live: bool,
) -> Result<Value> {
    directory(backend, output)?;
    let execution = Path::new(document::text(&request["execution_backend"], "来源执行根")?);
    let runtime = output.join("runtime");
    let evidence = document::read(&output.join("running-evidence.json"))?;
    document::exact(
        &evidence,
        &[
            "format_version",
            "kind",
            "intent",
            "runtime",
            "roles",
            "physical_binding",
        ],
        "运行中源证据",
    )?;
    let intent = common::read_bound(backend, &evidence["intent"], "运行中源意图")?;
    document::exact(
        &intent,
        &[
            "format_version",
            "kind",
            "request",
            "runtime_directory",
            "operations",
        ],
        "运行中源意图",
    )?;
    if evidence["format_version"].as_u64() != Some(1)
        || evidence["kind"] != "seed-source-runtime-running"
        || evidence["intent"] != common::binding(&output.join("intent.json"))?
        || evidence["runtime"] != common::binding(&runtime.join("runtime.json"))?
        || evidence["physical_binding"] != *physical
        || intent["format_version"].as_u64() != Some(1)
        || intent["kind"] != "seed-source-runtime-intent"
        || intent["request"] != common::binding(&output.join("request.json"))?
        || intent["runtime_directory"] != json!(runtime)
    {
        return Err("运行中源证据必须绑定同一完整代次、请求及物理来源".into());
    }
    roles::directory(&runtime, &intent["operations"])?;
    let contract = fixture_control::verify_full_stack_runtime(execution, &runtime)?;
    let build = builds::verify(backend, request)?;
    let artifacts = document::object(&build["artifacts"], "API/Worker 构建产物")?
        .iter()
        .map(|(role, artifact)| {
            (
                role.clone(),
                json!({"path":artifact["executable"],"sha256":artifact["sha256"]}),
            )
        })
        .collect::<serde_json::Map<_, _>>();
    if contract["backend_root"] != json!(execution)
        || contract["artifacts"] != json!(artifacts)
        || super::binding::physical(&json!({"backend":execution,"evidence_root":backend,
            "source":selected,"app_env":"test","config_dir":contract["api_config_dir"]}))?
            != *physical
    {
        return Err("运行中源配置、物理资源或产品产物变化".into());
    }
    roles::observe(&runtime, &evidence, &intent, selected, &contract, live)?;
    roles::directory(&runtime, &intent["operations"])?;
    if document::read(&output.join("running-evidence.json"))? != evidence {
        return Err("运行中源实际观察期间角色证据变化".into());
    }
    recheck_bindings(backend, request, &evidence)?;
    Ok(contract)
}

fn recheck_bindings(backend: &Path, request: &Value, evidence: &Value) -> Result<()> {
    for role in ["api", "worker"] {
        for field in ["process", "tree"] {
            common::bound_file(backend, &evidence["roles"][role][field], "运行中源角色绑定")?;
        }
    }
    common::bound_file(backend, &evidence["intent"], "运行中源意图")?;
    common::bound_file(backend, &evidence["runtime"], "运行中源配置收据")?;
    common::bound_file(backend, &request["backend_build"], "运行中源产品构建")?;
    Ok(())
}

pub(super) fn directory(backend: &Path, output: &Path) -> Result<()> {
    common::local_directory(backend, output, "来源运行代次")?;
    let files = BTreeSet::from([
        "request.json",
        "intent.json",
        "producers.json",
        "source-request.json",
        "generation-verified.json",
        "runtime-evidence.json",
        "running-evidence.json",
        "dataset-lineage.json",
    ]);
    let directories = BTreeSet::from([
        "runtime",
        "before",
        "running",
        "after",
        "stop-before",
        "verification",
        "recover-before",
        "recover-after",
        "reboot-close",
        "runtime-configuration",
    ]);
    for entry in std::fs::read_dir(output)? {
        let entry = entry?;
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| "来源代次文件名必须为 UTF-8")?;
        let metadata = std::fs::symlink_metadata(entry.path())?;
        if digest::linked(&metadata)
            || !(files.contains(name.as_str()) && metadata.is_file()
                || directories.contains(name.as_str()) && metadata.is_dir())
        {
            return Err("来源代次存在未知、链接或类型不符的项".into());
        }
    }
    Ok(())
}
