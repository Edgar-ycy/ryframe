//! 从原夹具历史核验可独立接续的 RustFS 来源，不观察或控制 Redis。
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use super::{common, model};
use crate::{
    Result,
    recovery::{
        clone::native::{
            json as document,
            lifecycle::{lock, lock::ProcessGuard, state},
            s3::app_configuration,
        },
        fixture_control::services::{history, lifecycle::readonly},
    },
};

pub(super) struct Proof {
    run: PathBuf,
    tree: Value,
    bindings: Value,
}

impl Proof {
    pub(super) fn binding(&self) -> Value {
        self.bindings.clone()
    }

    pub(super) fn lock(&self) -> Result<ProcessGuard> {
        ProcessGuard::acquire(&self.run)
    }

    pub(super) fn check(&self, backend: &Path) -> Result<()> {
        common::local_directory(backend, &self.run, "原来源夹具目录")?;
        check_idle(&self.run)?;
        for descriptor in document::object(&self.bindings, "原来源夹具固定证据")?.values()
        {
            common::bound_file(backend, descriptor, "原来源夹具固定文件")?;
        }
        Ok(())
    }

    pub(super) fn require_stopped(&self, backend: &Path) -> Result<()> {
        self.check(backend)?;
        for role in ["monitor", "supervisor", "process"] {
            common::require_recorded_stopped(&self.tree[role])?;
        }
        if readonly::NativeRuntime::new(backend).complete_registered_tree(&self.tree)?
            != self.bindings["member_stop"]
        {
            return Err("原来源 RustFS 的成员关闭证明与已绑定收据不同".into());
        }
        self.check(backend)
    }
}

pub(super) fn load(backend: &Path, request: &Value, original: &Value) -> Result<Proof> {
    let previous = &request["previous"];
    let mut bindings = json!({});
    let copy = capture(
        backend,
        &request["manifest"],
        &mut bindings,
        "copy_manifest",
    )?;
    if copy["copy_stage"] != "source_to_seed"
        || copy["source_request"] != request["original"]
        || copy["source_environment"] != request["environment"]
        || capture(
            backend,
            &request["original"],
            &mut bindings,
            "source_request",
        )? != *original
    {
        return Err("夹具来源存储未绑定原始复制清单和冻结来源请求".into());
    }
    let rustfs = original_directory(backend, previous)?;
    let lifecycle = rustfs.parent().ok_or("来源 RustFS 缺少代次目录")?;
    let run = lifecycle.parent().ok_or("来源 RustFS 缺少服务账本")?;
    if !run.ends_with("service-run") {
        return Err("来源 RustFS 代次必须属于原夹具 service-run".into());
    }
    common::local_directory(backend, run, "原来源夹具目录")?;
    check_idle(run)?;
    let registered = register(backend, run, &mut bindings)?;
    check_review(backend, run, request, original, &registered.review)?;
    let selected = history::handoff::selected(run, &registered.active)?;
    if selected.redis_runtime["format_version"] != 3
        || selected.redis_runtime["redis"]["platform"] != "windows"
    {
        return Err("来源夹具单独接续只接受原 Windows 服务登记".into());
    }
    if common::bound_file(backend, &bindings["active_restart"], "活跃来源重启")?
        != lifecycle.join("restart.json")
    {
        return Err("来源 RustFS 三份前驱收据不属于活跃服务重启".into());
    }
    check_generation(
        backend,
        request,
        &selected,
        &registered,
        &rustfs,
        &mut bindings,
    )?;
    bindings["redis_runtime_history"] = selected.redis_binding;
    bind_stop(backend, &rustfs, &selected.tree, &mut bindings)?;
    let proof = Proof {
        run: run.to_owned(),
        tree: selected.tree,
        bindings,
    };
    proof.check(backend)?;
    Ok(proof)
}

fn original_directory(backend: &Path, previous: &Value) -> Result<PathBuf> {
    let process = common::bound_file(backend, &previous["process_receipt"], "来源原进程")?;
    let rustfs = process.parent().ok_or("来源原进程缺少 RustFS 目录")?;
    for (key, name) in [
        ("process_receipt", "process.json"),
        ("launch_receipt", "launch.json"),
        ("ready_receipt", "ready.json"),
    ] {
        if common::bound_file(backend, &previous[key], "来源原代次收据")? != rustfs.join(name)
        {
            return Err("来源原代次的进程、启动和就绪收据必须来自同一 RustFS 目录".into());
        }
    }
    if !rustfs.ends_with("rustfs") {
        return Err("来源原代次收据目录不是 RustFS".into());
    }
    Ok(rustfs.to_owned())
}

fn check_idle(run: &Path) -> Result<()> {
    if lock::owner_observation(run)?.is_some() {
        return Err("来源夹具服务账本仍有控制器".into());
    }
    Ok(())
}

struct Registered {
    active: Value,
    restart: Value,
    review: Value,
}

fn register(backend: &Path, run: &Path, bindings: &mut Value) -> Result<Registered> {
    let manifest = record(
        backend,
        &run.join("manifest.json"),
        bindings,
        "service_manifest",
    )?;
    history::validate_manifest(&manifest)?;
    let state_path = run.join("state.json");
    bindings["service_state"] = common::binding(&state_path)?;
    let saved = state::load(run, true)?;
    if saved["manifest"] != bindings["service_manifest"]
        || state::running_attempt(&saved)?.is_some()
    {
        return Err("来源夹具账本未绑定原服务清单或阶段仍在运行".into());
    }
    let history = history::validate(run, &saved, true, true)?;
    let active = history["active_generation"].clone();
    let latest = state::attempts(&saved)?
        .last()
        .ok_or("来源夹具缺少已通过的活跃重启")?;
    if history["closed"] != false
        || !history["external_recovery"].is_null()
        || !history["rustfs_handoff"].is_null()
        || latest["stage"] != "fixture-services"
        || latest["mode"] != "restart"
        || latest["status"] != "passed"
        || latest["result"] != active
    {
        return Err("来源夹具没有唯一已收口的原活跃重启或已有移交".into());
    }
    let outer = capture(backend, &active, bindings, "active_result")?;
    let restart = capture(backend, &outer["generation"], bindings, "active_restart")?;
    let review = capture(backend, &manifest["review"], bindings, "review")?;
    if manifest["scope_id"] != review["services"]["rustfs"]["scope_id"] {
        return Err("来源夹具清单与原审阅的服务 scope 不同".into());
    }
    Ok(Registered {
        active,
        restart,
        review,
    })
}

fn check_review(
    backend: &Path,
    run: &Path,
    request: &Value,
    original: &Value,
    review: &Value,
) -> Result<()> {
    let source = &original["source"];
    let public = app_configuration::public_configuration(backend, &source["s3"]["configuration"])?;
    let seed = &review["scopes"]["seed"];
    let rustfs = &review["services"]["rustfs"];
    let tool = &review["tools"]["rustfs"];
    let root = Path::new(document::text(&review["future_root"], "原来源夹具根")?);
    if review["format_version"] != 1
        || review["kind"] != "review-only-perf-resource-plan-with-readonly-preflight"
        || review["ready_for_execution"] != true
        || root.join("service-run") != run
        || source["scope_id"] != seed["scope_id"]
        || source["api_url"] != seed["api_url"]
        || public != seed["objects"]
        || public["endpoint"] != request["api_url"]
        || rustfs["scope_id"] != request["scope_id"]
        || rustfs["api"] != request["api_url"]
        || rustfs["console"] != request["console_url"]
        || rustfs["data_dir"] != request["data_directory"]["path"]
        || !same_tool(tool, &request["executable"])
    {
        return Err("原来源请求未绑定真实夹具审阅、数据路径、工具和对象端点".into());
    }
    Ok(())
}

fn same_tool(left: &Value, right: &Value) -> bool {
    left["path"].is_string()
        && left["path"] == right["path"]
        && left["sha256"].is_string()
        && left["sha256"] == right["sha256"]
}

fn check_generation(
    backend: &Path,
    request: &Value,
    selected: &history::handoff::Selected,
    registered: &Registered,
    rustfs: &Path,
    bindings: &mut Value,
) -> Result<()> {
    let old = &selected.rustfs_request;
    let storage = &registered.restart["rustfs"]["storage"];
    let previous = &request["previous"];
    let tree = &selected.tree;
    let tool = &request["executable"];
    if [
        "scope_id",
        "data_directory",
        "api_url",
        "console_url",
        "credential_files",
    ]
    .into_iter()
    .any(|key| request[key] != old[key])
        || !same_tool(tool, &old["executable"])
        || tree["process"] != previous["identity"]
        || tree["process"]["executable"] != tool["path"]
        || storage["sha256"] != tool["sha256"]
        || storage["process_receipt"] != previous["process_receipt"]
        || storage["launch_receipt"] != previous["launch_receipt"]
    {
        return Err("来源 RustFS 重启请求与旧物理资源、创建代次或启动收据不同".into());
    }
    common::directory_identity(backend, &old["data_directory"])?;
    check_tree(tree, rustfs, &request["scope_id"])?;
    bindings["tree"] = selected.tree_binding.clone();
    bindings["fixture_request"] = selected.request_binding.clone();
    capture(
        backend,
        &registered.restart["controller"],
        bindings,
        "restart_controller",
    )?;
    check_running_receipts(backend, request, registered, bindings, rustfs)?;
    Ok(())
}

fn check_tree(tree: &Value, rustfs: &Path, scope: &Value) -> Result<()> {
    let operation = document::text(&tree["operation_id"], "来源 RustFS 操作标识")?;
    if operation.len() != 32
        || !operation
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        || tree["format_version"] != 2
        || tree["kind"] != "full-stack-process-tree"
        || tree["role"] != "rustfs"
        || tree["scope_id"] != *scope
        || tree["runtime_directory"] != json!(rustfs)
        || tree["group_id"] != tree["supervisor"]["pid"]
        || tree["supervisor"]["pid"] == tree["monitor"]["pid"]
        || tree["supervisor"]["pid"] == tree["process"]["pid"]
        || tree["monitor"]["pid"] == tree["process"]["pid"]
    {
        return Err("原来源 RustFS 的进程树不是完整登记的 Windows 监督树".into());
    }
    for role in ["process", "monitor", "supervisor"] {
        common::validate_process_identity(&tree[role])?;
    }
    Ok(())
}

fn check_running_receipts(
    backend: &Path,
    request: &Value,
    registered: &Registered,
    bindings: &mut Value,
    rustfs: &Path,
) -> Result<()> {
    let previous = &request["previous"];
    let restart = &registered.restart;
    let storage = &restart["rustfs"]["storage"];
    let identity = &previous["identity"];
    let manifest = bindings["service_manifest"].clone();
    let intent = record(
        backend,
        &rustfs.join("intent.json"),
        bindings,
        "start_intent",
    )?;
    let process = capture(backend, &previous["process_receipt"], bindings, "process")?;
    let launch = capture(backend, &previous["launch_receipt"], bindings, "launch")?;
    let ready = capture(backend, &previous["ready_receipt"], bindings, "ready")?;
    let arguments = model::arguments(request)?;
    if intent
        != json!({"format_version":1,"kind":"devex-clone-storage-intent",
        "request":manifest,"controller":restart["controller"],"attempt":restart["attempt"],"arguments":arguments})
        || process
            != json!({"format_version":1,"role":"rustfs","scope_id":request["scope_id"],
            "lifecycle":"running","identity":identity,"api_url":request["api_url"],
            "console_url":request["console_url"],"data_dir":request["data_directory"]["path"],
            "intent":bindings["start_intent"]})
        || launch
            != json!({"format_version":1,"scope_id":request["scope_id"],"identity":identity,
            "arguments":arguments,"credential_files":request["credential_files"]})
        || ready
            != json!({"storage":storage,"request":manifest,"controller":restart["controller"],
            "attempt":restart["attempt"],"listeners":[request["api_url"],request["console_url"]]})
    {
        return Err("来源 RustFS 的原启动意图、实际进程、启动参数或就绪凭据不一致".into());
    }
    Ok(())
}

fn bind_stop(backend: &Path, rustfs: &Path, tree: &Value, bindings: &mut Value) -> Result<()> {
    let op = document::text(&tree["operation_id"], "来源 RustFS 操作标识")?;
    let stopped = record(
        backend,
        &rustfs.join(format!("rustfs-members-{op}-stopped.json")),
        bindings,
        "member_stop",
    )?;
    readonly::check_restore_completion(tree, &stopped)?;
    if !document::array(&stopped["members"], "来源 RustFS 已回收成员")?.contains(&tree["process"])
    {
        return Err("原来源 RustFS 关闭证明未覆盖受管产品进程".into());
    }
    let control = record(
        backend,
        &rustfs.join(format!("rustfs-tree-{op}-control.json")),
        bindings,
        "stop_control",
    )?;
    let result = record(
        backend,
        &rustfs.join(format!("rustfs-tree-{op}-result.json")),
        bindings,
        "stop_result",
    )?;
    check_stop(tree, &control, &result)
}

fn check_stop(tree: &Value, control: &Value, result: &Value) -> Result<()> {
    let op = &tree["operation_id"];
    if control
        != &json!({"format_version":1,"kind":"full-stack-process-tree-control",
        "operation_id":op,"mode":"normal","requested_at_ns":control["requested_at_ns"]})
        || !control["requested_at_ns"]
            .as_u64()
            .is_some_and(|value| value > 0)
        || result
            != &json!({"format_version":1,"kind":"full-stack-process-tree-result",
            "operation_id":op,"process":tree["process"],"exit_code":result["exit_code"],
            "termination":result["termination"]})
        || !matches!(result["termination"].as_str(), Some("forced"))
        || result["exit_code"].as_i64().is_none()
    {
        return Err("来源 RustFS 必须具备原正常控制请求和同一产品进程的结束收据".into());
    }
    Ok(())
}

fn record(backend: &Path, path: &Path, bindings: &mut Value, key: &str) -> Result<Value> {
    let path = common::common_file(backend, path, "来源夹具登记文件")?;
    capture(backend, &common::binding(&path)?, bindings, key)
}

fn capture(backend: &Path, descriptor: &Value, bindings: &mut Value, key: &str) -> Result<Value> {
    let value = common::read_bound(backend, descriptor, "来源夹具证据")?;
    bindings[key] = descriptor.clone();
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_stop_requires_original_normal_control_and_matching_product_result() {
        let tree = json!({"operation_id":"a".repeat(32),"process":{"pid":752}});
        let control = json!({"format_version":1,"kind":"full-stack-process-tree-control",
            "operation_id":tree["operation_id"],"mode":"normal","requested_at_ns":1});
        let result = json!({"format_version":1,"kind":"full-stack-process-tree-result",
            "operation_id":tree["operation_id"],"process":tree["process"],"exit_code":1,
            "termination":"forced"});
        check_stop(&tree, &control, &result).unwrap();
        let mut changed = control.clone();
        changed["mode"] = json!("crash");
        assert!(check_stop(&tree, &changed, &result).is_err());
        let mut foreign = result.clone();
        foreign["process"]["pid"] = json!(753);
        assert!(check_stop(&tree, &control, &foreign).is_err());
        let mut unknown = result;
        unknown["termination"] = json!("natural");
        assert!(check_stop(&tree, &control, &unknown).is_err());
    }
}
