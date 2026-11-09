//! 正常停止后的后继请求与 generation 证明；复用现有原生证明权威，不自行签简化收据。
use super::window::Window;
use crate::{
    Result,
    recovery::{
        clone::native::{
            export::{proof, transport::Transport},
            json as document,
            resources::common,
            seed::generation::coordinator,
        },
        fixture_control::services::lifecycle::readonly::NativeRuntime,
        source::evidence::running::roles,
    },
};
use serde_json::{Value, json};

pub(super) fn finish<T: Transport + Sync>(window: &Window<'_, '_, T>) -> Result<()> {
    window.checkpoint()?;
    let output = window.output()?;
    let runtime = output.join("runtime");
    let intent = document::read(&output.join("intent.json"))?;
    roles::directory(&runtime, &intent["operations"])?;
    let mut observed_roles = serde_json::Map::new();
    let mut processes = serde_json::Map::new();
    for role in ["api", "worker"] {
        let tree = roles::tree(&runtime, role, &window.facts["selected"]["scope_id"])?;
        if tree["operation_id"] != intent["operations"][role] {
            return Err("源静止证明的树与启动意图不同".into());
        }
        let process = common::binding(&runtime.join(format!("{role}.json")))?;
        processes.insert(role.into(), process.clone());
        observed_roles.insert(role.into(), json!({"process":process,
            "tree":common::binding(&runtime.join(format!("{role}-tree.json")))?,
            "completion":NativeRuntime::new(window.frame.backend).complete_registered_tree(&tree)?, "ready":true}));
    }
    producers(window, &processes)?;
    document::write_new(
        &output.join("runtime-evidence.json"),
        &json!({
            "format_version":1,"kind":"seed-source-runtime-quiescence",
            "intent":common::binding(&output.join("intent.json"))?,
            "runtime":common::binding(&runtime.join("runtime.json"))?,
            "roles":observed_roles,"physical_binding":window.facts["source"]["generation"]["physical_binding"],
            "ports_idle":["api","worker"],"observed_stopped_at":chrono::Utc::now().to_rfc3339(),
        }),
    )?;
    let build = common::read_bound(
        window.frame.backend,
        &window.facts["request"]["backend_build"],
        "源构建",
    )?;
    let request = coordinator::dispatch(&json!({"action":"source-request","input":{
        "source_request":window.facts["source"]["request"],"request":window.facts["request"],
        "fingerprint":build["sources"]["full"]["source"]["worktree_fingerprint"],
        "runtime":common::binding(&runtime.join("runtime.json"))?,"processes":processes,
        "producers_registry":common::binding(&output.join("producers.json"))?,"runtime_dir":runtime,
    }}))?;
    document::write_new(&output.join("source-request.json"), &request)?;
    let mut context = window.context(&output)?;
    context.request = request;
    context.request_path = output.join("source-request.json");
    // generation 的应用配置选择只接收 APP_ENV；OS 运行环境仍保留在实际采集窗口。
    context.environment = std::collections::BTreeMap::from([("APP_ENV".into(), "test".into())]);
    let generation = proof::generation_in(&context, &window.frame.scope)?;
    document::write_new(&output.join("generation-verified.json"), &generation)?;
    window.checkpoint()
}

fn producers<T: Transport + Sync>(
    window: &Window<'_, '_, T>,
    processes: &serde_json::Map<String, Value>,
) -> Result<()> {
    let original = common::read_bound(
        window.frame.backend,
        &window.facts["source"]["request"]["producers_registry"],
        "原始来源生产者",
    )?;
    let mut entries = document::array(&original["processes"], "原始来源生产者集合")?.iter().enumerate()
        .map(|(index,item)|json!({"name":format!("predecessor-{index}"),"identity":item["identity"]}))
        .collect::<Vec<_>>();
    for role in ["api", "worker"] {
        let row = common::read_bound(window.frame.backend, &processes[role], "源角色进程")?;
        entries.push(json!({"name":role,"identity":row["identity"]}));
    }
    let output = window.output()?;
    let value = json!({"format_version":1,"scope_id":window.facts["selected"]["scope_id"],
        "runtime_sha256":common::binding(&output.join("runtime/runtime.json"))?["sha256"],"processes":entries});
    let path = output.join("producers.json");
    if path.try_exists()? {
        if document::read(&path)? != value {
            return Err("源后继生产者登记发生变化，禁止覆盖".into());
        }
    } else {
        document::write_new(&path, &value)?;
    }
    Ok(())
}
