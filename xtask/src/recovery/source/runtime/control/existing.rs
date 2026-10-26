//! 回收只读取同一原 START 和已登记树；失败 START 不伪造成功收据或不可变运行基线。
use super::frame::Frame;
use crate::{
    Result,
    recovery::{
        clone::native::{
            json as document, maintenance, resources::common, seed::generation::coordinator,
        },
        fixture_control::{self, services::lifecycle::readonly::NativeRuntime},
        source::evidence::{
            binding,
            running::{self, builds, history, roles},
        },
    },
};
use serde_json::{Value, json};
use std::path::Path;

pub(super) fn unpublished(frame: &Frame<'_>, start: &Value, descriptor: &Value) -> Result<Value> {
    if start["stage"] != "seed-runtime"
        || start["mode"] != "source-generation-start"
        || start["status"] != "failed"
        || !start["result"].is_null()
        || !frame.prefix.contains(start)
    {
        return Err("未发布源回收必须绑定当前完整历史中的原失败 START".into());
    }
    let number = start["number"]
        .as_u64()
        .filter(|n| *n > 0)
        .ok_or("失败 START 序号无效")?;
    let output = frame.run.join(format!("g{number:04}"));
    running::directory(frame.backend, &output)?;
    let request = common::read_bound(frame.backend, descriptor, "源回收显式请求")?;
    coordinator::request(&request)?;
    let internal = common::binding(&output.join("request.json"))?;
    if common::read_bound(frame.backend, &internal, "源原始请求")? != request {
        return Err("回收必须显式绑定原 source-generation 请求，不能替换来源或目标".into());
    }
    let source = history::predecessor(frame.backend, frame.run, frame.state())?;
    matches_source(&request, &source)?;
    let build = builds::verify(frame.backend, &request)?;
    let execution = Path::new(document::text(&request["execution_backend"], "源执行根")?);
    let runtime = output.join("runtime");
    let contract = fixture_control::verify_full_stack_runtime(execution, &runtime)?;
    let intent = intent(frame, &output, &internal)?;
    roles::directory(&runtime, &intent["operations"])?;
    verify_binaries(frame, &request, &build, &runtime, &contract)?;
    let mut selected = source["request"]["source"].clone();
    document::object(&selected, "来源物理目标")?;
    selected["runtime_dir"] = json!(runtime);
    let private = common::read_bound(
        frame.backend,
        &request["source_environment"],
        "源私有运行环境",
    )?;
    document::exact(&private, &["environment"], "源私有运行环境")?;
    let physical = binding::physical(&json!({"backend":execution,"evidence_root":frame.backend,
        "source":selected,"app_env":private["environment"]["APP_ENV"],"config_dir":contract["api_config_dir"]}))?;
    if physical != source["generation"]["physical_binding"] {
        return Err("失败 START 的实际冻结配置改变了 C52 完整物理来源".into());
    }
    Ok(
        json!({"request":request,"request_binding":descriptor,"internal_request":internal,
        "directory":frame.run,"number":number,"output":output,"execution":execution,
        "source":source,"selected":selected,"runtime":contract,"environment":private,
        "intent":intent,"intent_binding":common::binding(&output.join("intent.json"))?}),
    )
}

pub(super) fn matches_source(request: &Value, source: &Value) -> Result<()> {
    if request["source_registration"] != source["review_successor"]["source_result"]
        || request["source_rebind"] != source["source_rebind"]
        || request["review_successor"] != source["review_successor_binding"]
        || request["current_storage"] != source["storage"]["storage"]
    {
        return Err("回收请求改变了原 C52、存储、重绑定或 successor".into());
    }
    Ok(())
}

fn intent(frame: &Frame<'_>, output: &Path, request: &Value) -> Result<Value> {
    let descriptor = common::binding(&output.join("intent.json"))?;
    let intent = common::read_bound(frame.backend, &descriptor, "原源启动意图")?;
    document::exact(
        &intent,
        &[
            "format_version",
            "kind",
            "request",
            "runtime_directory",
            "operations",
        ],
        "原源启动意图",
    )?;
    if intent["format_version"].as_u64() != Some(1)
        || intent["kind"] != "seed-source-runtime-intent"
        || intent["request"] != *request
        || intent["runtime_directory"] != json!(output.join("runtime"))
    {
        return Err("源回收意图未绑定原请求和同代 runtime".into());
    }
    roles::directory(&output.join("runtime"), &intent["operations"])?;
    common::bound_file(frame.backend, &descriptor, "原源启动意图")?;
    Ok(intent)
}

fn verify_binaries(
    frame: &Frame<'_>,
    request: &Value,
    build: &Value,
    runtime: &Path,
    contract: &Value,
) -> Result<()> {
    let execution = Path::new(document::text(&request["execution_backend"], "源执行根")?);
    let maintenance = maintenance::verify_evidence(
        execution,
        &common::bound_file(execution, &request["maintenance_build"], "源维护构建")?,
        None,
    )?;
    let expected = json!({"ryframe":build["artifacts"]["api"]["executable"],
        "ryframe-worker":build["artifacts"]["worker"]["executable"],
        "ryframe-reset":maintenance["artifacts"]["reset"]["executable"],
        "ryframe-migrate":maintenance["artifacts"]["migrate"]["executable"]});
    if document::read(&runtime.join("binaries.json"))? != expected
        || contract["backend_root"] != json!(execution)
    {
        return Err("源回收 runtime 与完整产品、维护构建的实际文件绑定不同".into());
    }
    for role in ["api", "worker"] {
        if contract["artifacts"][role]
            != json!({"path":build["artifacts"][role]["executable"],"sha256":build["artifacts"][role]["sha256"]})
        {
            return Err("源回收角色产物与原构建不同".into());
        }
    }
    frame.checkpoint()
}

pub(super) fn status(frame: &Frame<'_>, start: &Value) -> Result<Value> {
    frame.checkpoint()?;
    let number = start["number"]
        .as_u64()
        .filter(|n| *n > 0)
        .ok_or("来源启动序号无效")?;
    let output = frame.run.join(format!("g{number:04}"));
    running::directory(frame.backend, &output)?;
    let intent = document::read(&output.join("intent.json"))?;
    let runtime = output.join("runtime");
    roles::directory(&runtime, &intent["operations"])?;
    let source = history::predecessor(frame.backend, frame.run, frame.state())?;
    let scope = &source["request"]["source"]["scope_id"];
    let mut observed = serde_json::Map::new();
    for role in ["api", "worker"] {
        let tree_path = runtime.join(format!("{role}-tree.json"));
        if !tree_path.try_exists()? {
            observed.insert(
                role.into(),
                json!({"state":"unknown","tree":null,"completion":null}),
            );
            continue;
        }
        let tree_binding = common::binding(&tree_path)?;
        let tree = roles::tree(&runtime, role, scope)?;
        if tree["operation_id"] != intent["operations"][role] {
            return Err("源树 operation 不属于同一启动意图".into());
        }
        let alive = ["process", "supervisor", "monitor"]
            .map(|field| observe(&tree[field]))
            .into_iter()
            .collect::<Result<Vec<_>>>()?;
        let completion = if alive.iter().all(|value| !*value) {
            NativeRuntime::new(frame.backend).complete_registered_tree(&tree)?
        } else {
            Value::Null
        };
        let state = if !completion.is_null() {
            "stopped"
        } else if alive.iter().all(|value| *value) {
            "running"
        } else {
            "degraded"
        };
        common::bound_file(frame.backend, &tree_binding, "源角色树")?;
        observed.insert(
            role.into(),
            json!({"state":state,"tree":tree_binding,"completion":completion}),
        );
    }
    frame.checkpoint()?;
    Ok(
        json!({"status":"source_generation_observed","attempt":number,"start_status":start["status"],
        "intent":common::binding(&output.join("intent.json"))?,"roles":observed,"remote_writes":0,"restore_qualified":false}),
    )
}

fn observe(expected: &Value) -> Result<bool> {
    common::validate_process_identity(expected)?;
    let pid = u32::try_from(expected["pid"].as_u64().ok_or("原源 PID 无效")?)?;
    match common::process_identity(pid)? {
        None => Ok(false),
        Some(actual) if actual == *expected => Ok(true),
        Some(_) => Err("源代次存在 PID 复用或创建身份变化".into()),
    }
}
