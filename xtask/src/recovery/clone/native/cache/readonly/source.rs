//! 缓存工具后继消费已发布来源；只验证原登记，不重新签发来源或执行资源操作。
use crate::{
    Result,
    recovery::clone::native::{
        digest, export, input, json as document, lifecycle::state, resources::common,
        s3::app_configuration, target_verify,
    },
};
use serde_json::{Value, json};
use std::path::Path;

#[path = "source/relationship.rs"]
mod relationship;

/// 无工具换代的正式已发布来源仍共用六文件、历史和初始化证明。
pub(crate) fn published(backend: &Path, descriptor: &Value) -> Result<Value> {
    let result = common::read_bound(backend, descriptor, "已发布源结果")?;
    let registration = common::read_bound(backend, &result["registration"], "已发布源登记")?;
    let manifest = common::read_bound(backend, &registration["run_manifest"], "已发布源原清单")?;
    let initialized = common::bound_file(backend, &manifest["initialized"], "已发布源初始化")?;
    let (_, target) = target_verify::published::evidence_history(backend, &initialized, None)?;
    registered(backend, descriptor, &target, &target["review"])
}

pub(crate) fn load(backend: &Path, descriptor: &Value) -> Result<Value> {
    let relation = relationship::load(backend, descriptor)?;
    let historical = canonical(backend, &relation["predecessor_request"])?;
    let mut source = registered(
        backend,
        &relation["source_result"],
        &historical,
        &relation["predecessor_review"],
    )?;
    if source["result"]["registration"] != relation["source_registration"]
        || source["seed_target"] != historical
    {
        return Err("发布源与 seed review successor 关系不一致".into());
    }
    if relationship::load(backend, descriptor)? != relation {
        return Err("seed review successor 或其完整输入在核对期间变化".into());
    }
    source["review_successor"] = relation;
    source["review_successor_binding"] = descriptor.clone();
    Ok(source)
}

fn registered(
    backend: &Path,
    descriptor: &Value,
    target: &Value,
    predecessor: &Value,
) -> Result<Value> {
    let filename = common::bound_file(backend, descriptor, "seed 来源结果")?;
    let run = result_directory(backend, &filename)?;
    let run = run.as_path();
    let saved = state::load(run, true)?;
    let attempt = state::attempts(&saved)?
        .iter()
        .rfind(|value| value["stage"] == "seed-runtime" && value["mode"] == "source-register")
        .ok_or("seed 来源缺少 source-register 阶段")?;
    let number = attempt["number"].as_u64().ok_or("来源登记序号无效")?;
    if attempt["status"] != "passed"
        || attempt["result"] != *descriptor
        || filename != run.join(format!("results/{number:04}.json"))
    {
        return Err("seed 来源必须绑定最新已通过的 source-register 结果".into());
    }
    let result = common::read_bound(backend, descriptor, "seed 来源结果")?;
    document::exact(
        &result,
        &[
            "status",
            "registration",
            "source_request",
            "source_storage",
            "generation_verified",
            "remote_writes",
            "outbox_drained",
            "restore_qualified",
        ],
        "seed 来源结果",
    )?;
    qualified(&result)?;
    if result["status"] != "seed_source_registered" {
        return Err("seed 来源结果尚未发布".into());
    }
    let path = common::bound_file(backend, &result["registration"], "seed 来源登记")?;
    if path
        .file_name()
        .is_none_or(|name| name != "source-registration.json")
        || path.parent().and_then(Path::parent) != Some(run.join("seed-runtime").as_path())
    {
        return Err("seed 来源登记不属于固定 attempt 目录".into());
    }
    let registration = document::read(&path)?;
    let request = registration_documents(backend, run, &registration, &result)?;
    let manifest = common::read_bound(backend, &registration["run_manifest"], "seed 原清单")?;
    if manifest["copy_stage"] != "source_to_seed" {
        return Err("seed 来源只能继承 source_to_seed".into());
    }
    let initialized_path = common::bound_file(backend, &manifest["initialized"], "seed 初始化")?;
    let (initialization, seed_target) =
        target_verify::published::evidence_history(backend, &initialized_path, None)?;
    if &seed_target != target {
        return Err("seed 初始化不是 successor 绑定的历史请求".into());
    }
    target_verify::binding_request::validate(backend, &seed_target, predecessor)?;
    let mut source = source_documents(
        backend,
        run,
        &registration,
        &request,
        &manifest,
        &seed_target,
        &initialization,
    )?;
    source["result"] = result;
    if state::load(run, true)? != saved {
        return Err("seed 来源阶段历史在核验期间变化".into());
    }
    recursive_bindings(backend, &registration)?;
    common::bound_file(backend, descriptor, "seed 来源结果")?;
    Ok(source)
}

fn registration_documents(
    backend: &Path,
    run: &Path,
    registration: &Value,
    result: &Value,
) -> Result<Value> {
    document::exact(
        registration,
        &[
            "format_version",
            "kind",
            "run_directory",
            "run_manifest",
            "source_to_seed",
            "post_copy",
            "post_verify",
            "seed_registration",
            "seed_registration_stage",
            "capacity",
            "departments",
            "identity",
            "seed_handoff",
            "seed_close",
            "producer_lineage",
            "source_storage",
            "source_request",
            "generation_verified",
            "source_environment",
            "remote_writes",
            "outbox_drained",
            "restore_qualified",
        ],
        "seed 来源登记",
    )?;
    qualified(registration)?;
    if registration["format_version"].as_u64() != Some(1)
        || registration["kind"] != "devex-clone-seed-source-registration"
        || registration["run_directory"] != json!(run)
        || registration["run_manifest"] != common::binding(&run.join("manifest.json"))?
    {
        return Err("seed 内层登记与原 run 不一致".into());
    }
    for key in ["source_request", "source_storage", "generation_verified"] {
        if registration[key] != result[key] {
            return Err("seed 内层登记与外层发布不一致".into());
        }
    }
    recursive_bindings(backend, registration)?;
    let request = common::read_bound(backend, &registration["source_request"], "seed 来源请求")?;
    export::input::validate_request(backend, &request)?;
    let generation = common::read_bound(
        backend,
        &registration["generation_verified"],
        "seed 来源代次",
    )?;
    if generation["request_sha256"] != digest::plan_hash(&request)?
        || generation["worktree_fingerprint"] != request["worktree_fingerprint"]
        || generation["external_writers_discovered"] != false
        || generation["clone_verified"] != false
        || generation["restore_qualified"] != false
    {
        return Err("seed 来源 generation 不属于已发布请求".into());
    }
    Ok(request)
}

fn source_documents(
    backend: &Path,
    run: &Path,
    registration: &Value,
    request: &Value,
    manifest: &Value,
    target: &Value,
    initialization: &Value,
) -> Result<Value> {
    let storage = common::read_bound(backend, &registration["source_storage"], "seed 来源存储")?;
    document::exact(
        &storage,
        &[
            "format_version",
            "kind",
            "run_directory",
            "run_manifest",
            "side",
            "storage",
        ],
        "seed 来源存储",
    )?;
    if storage["format_version"].as_u64() != Some(1)
        || storage["kind"] != "devex-clone-seed-source-storage"
        || storage["run_directory"] != json!(run)
        || storage["run_manifest"] != registration["run_manifest"]
        || storage["side"] != "target"
    {
        return Err("seed 来源存储不属于原 run 的 target 代次".into());
    }
    let handoff = common::read_bound(backend, &registration["seed_handoff"], "seed handoff")?;
    let runtime = common::read_bound(backend, &request["runtime"], "seed runtime")?;
    let configuration = app_configuration::configuration_value(
        backend,
        Path::new(document::text(
            &runtime["api_config_dir"],
            "seed API 配置目录",
        )?),
        "test",
    )?;
    let objects = app_configuration::private_configuration(backend, &configuration)?;
    let mut expected = target["target"].clone();
    expected["s3"] = json!({"configuration":configuration});
    expected["runtime_dir"] = request["source"]["runtime_dir"].clone();
    expected["api_url"] = handoff["api_url"].clone();
    if target["side"] != "seed"
        || request["source"] != expected
        || request["maintenance_build"] != target["maintenance_build"]
        || json!({"mysql":request["tools"]["mysql"]}) != target["tools"]
        || storage["storage"]["api_url"] != objects["endpoint"]
    {
        return Err("seed 来源没有精确派生自原 seed 初始化".into());
    }
    for field in ["endpoint", "region", "access_key", "secret_key"] {
        if objects[field] != target["target"]["s3"][field] {
            return Err("seed 对象配置不同于初始化绑定".into());
        }
    }
    let environment = common::read_bound(
        backend,
        &registration["source_environment"],
        "seed 来源环境",
    )?;
    document::exact(&environment, &["environment"], "seed 来源环境")?;
    for value in document::object(&environment["environment"], "seed 来源环境映射")?.values()
    {
        document::text(value, "seed 来源环境值")?;
    }
    Ok(
        json!({"directory":run,"registration":registration,"request":request,"storage":storage,
        "generation":common::read_bound(backend,&registration["generation_verified"],"seed 来源代次")?,
        "manifest":manifest,"seed_target":target,"initialization":initialization,"environment":environment}),
    )
}

fn qualified(value: &Value) -> Result<()> {
    if value["remote_writes"].as_u64() != Some(0)
        || value["outbox_drained"] != true
        || value["restore_qualified"] != false
    {
        return Err("seed 来源没有完整排空并发布".into());
    }
    Ok(())
}

fn recursive_bindings(backend: &Path, value: &Value) -> Result<()> {
    match value {
        Value::Object(fields)
            if fields.len() == 3
                && ["path", "bytes", "sha256"]
                    .iter()
                    .all(|field| fields.contains_key(*field)) =>
        {
            common::bound_file(backend, value, "seed 来源完整证据")?;
        }
        Value::Object(fields) => {
            for item in fields.values() {
                recursive_bindings(backend, item)?;
            }
        }
        Value::Array(items) => {
            for item in items {
                recursive_bindings(backend, item)?;
            }
        }
        _ => (),
    }
    Ok(())
}

fn canonical(backend: &Path, descriptor: &Value) -> Result<Value> {
    document::exact(
        descriptor,
        &["path", "bytes", "sha256", "canonical_sha256"],
        "successor 规范绑定",
    )?;
    let file = json!({"path":descriptor["path"],"bytes":descriptor["bytes"],"sha256":descriptor["sha256"]});
    let value = common::read_bound(backend, &file, "successor 规范输入")?;
    input::digest_shape(&descriptor["canonical_sha256"])?;
    if digest::plan_hash(&value)? != descriptor["canonical_sha256"] {
        return Err("successor 输入规范摘要无效".into());
    }
    Ok(value)
}

fn result_directory(backend: &Path, filename: &Path) -> Result<std::path::PathBuf> {
    let result_dir = filename
        .parent()
        .filter(|p| p.file_name().is_some_and(|name| name == "results"))
        .ok_or("seed 来源结果不属于统一 results 目录")?;
    let run = result_dir.parent().ok_or("seed 来源结果缺少统一目录")?;
    common::local_directory(backend, run, "seed 来源目录")
}
