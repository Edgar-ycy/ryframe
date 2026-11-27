//! 新代运行合同的准备及已发布 API 代次的历史核验。
use std::{
    fs,
    io::{Read, Write},
    path::Path,
};

use serde_json::{Value, json};

use super::runtime;
#[path = "api_successor/publication.rs"]
mod publication;
use crate::{
    Result,
    local_test_path::{LocalTestPathKind, validate_absolute_path},
    recovery::{
        clone::native::{
            digest, json as document,
            lifecycle::{identity, state},
            resources::common,
            seed::{
                history::{self, Post},
                source_registration::lineage::runtime as lineage_runtime,
            },
            target_verify::guard::Inputs,
        },
        fixture_control,
        full_stack::worker::{self, controller, role::Role},
    },
};

const RECEIPTS: [&str; 2] = ["binaries.json", "runtime.json"];
const MAX_RECEIPT_BYTES: u64 = 64 * 1024;

pub(in crate::recovery::clone::native) use publication::{
    Selected, runtime_history, select, select_before,
};

pub(in crate::recovery::clone::native) fn prepare_frozen_runtime(
    backend: &Path,
    product: &Path,
    original: &Path,
    successor: &Path,
) -> Result<Value> {
    common::local_directory(backend, original, "原复制后运行目录")?;
    common::local_directory(backend, successor, "API 后继运行目录")?;
    let parent = original
        .parent()
        .ok_or("原复制后运行目录缺少父目录")?
        .join("api-successors");
    if !registered_runtime_name(original)
        || successor.parent() != Some(parent.as_path())
        || fixture_control::portable_path(&fs::canonicalize(original)?) != original
        || fixture_control::portable_path(&fs::canonicalize(successor)?) != successor
    {
        return Err("API 后继必须是原复制后 api-successors 下的独立规范目录".into());
    }

    let original_directory = Inputs::directory(original)?;
    let successor_directory = Inputs::directory(successor)?;
    let original_files = RECEIPTS.map(|name| original.join(name));
    let frozen = Inputs::open(original_files.iter().cloned())?;
    let contract = fixture_control::verify_full_stack_runtime(product, original)?;
    for field in ["api_config_dir", "worker_config_dir"] {
        let config = contract[field].as_str().ok_or("原角色合同缺少配置目录")?;
        common::local_directory(backend, Path::new(config), "原角色私有配置目录")?;
    }
    let bytes = [
        read_receipt(&original_files[0])?,
        read_receipt(&original_files[1])?,
    ];
    frozen.check()?;
    original_directory.check()?;
    successor_directory.check()?;
    prepare_receipts(successor, [&bytes[0], &bytes[1]])?;

    let published = Inputs::open(RECEIPTS.map(|name| successor.join(name)))?;
    if prepared_receipts(successor, [&bytes[0], &bytes[1]])? != [true; 2]
        || fixture_control::verify_full_stack_runtime(product, successor)? != contract
    {
        return Err("API 后继发布的原始字节或运行合同不匹配，保留现场".into());
    }
    published.check()?;
    frozen.check()?;
    original_directory.check()?;
    successor_directory.check()?;
    Ok(contract)
}

fn registered_runtime_name(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| {
            name == "runtime"
                || name.strip_prefix("runtime-").is_some_and(|number| {
                    number.len() == 4
                        && number.bytes().all(|byte| byte.is_ascii_digit())
                        && number != "0000"
                })
        })
}

/// 核对发布代次的持久证据。调用方独立观察当前进程或停止状态。
pub(in crate::recovery::clone::native) fn load(
    backend: &Path,
    run: &Path,
    registration: &Value,
) -> Result<Option<Value>> {
    Ok(select(backend, run, registration)?.map(|selected| selected.publication))
}

fn original_executor(
    run: &Path,
    attempts: &[Value],
    completed: &Value,
    post: &Post,
    intent: &Value,
    second: bool,
) -> Result<(u64, Value)> {
    let mut fields = vec![
        "format_version",
        "kind",
        "attempt",
        "registration",
        "marker",
        "runtime_directory",
        "token",
        "executor",
        "incident",
    ];
    if second {
        fields.extend(["predecessor", "request"]);
    }
    document::exact(intent, &fields, "API 后继启动意图")?;
    let number = attempt_number(intent, "attempt")?;
    let format_version = if second { 2 } else { 1 };
    if intent["format_version"] != format_version
        || intent["kind"] != "devex-clone-api-successor-intent"
        || intent["registration"] != post.descriptor
    {
        return Err("API 后继启动意图格式或原登记不匹配".into());
    }
    let restart = attempts
        .iter()
        .find(|row| row["number"] == number)
        .ok_or("API 后继没有原启动阶段")?;
    let completed_number = attempt_number(completed, "number")?;
    if restart["stage"] != "runtime-target"
        || restart["mode"] != "restart-api"
        || match completed["mode"].as_str() {
            Some("restart-api") => completed_number != number || restart["status"] != "passed",
            Some("reconcile-api") => completed_number <= number || restart["status"] != "failed",
            _ => true,
        }
    {
        return Err("API 后继发布与原启动阶段顺序或完成方式不匹配".into());
    }
    let mut initial = restart.clone();
    initial["status"] = json!("running");
    initial["finished_at"] = Value::Null;
    initial["result"] = Value::Null;
    initial["error_type"] = Value::Null;
    let (_, owner) = state::controller_record(run, number, &initial)?;
    identity::validate(&owner, run)?;
    let executor = common::validate_process_identity(&owner["identity"])?;
    let path = Path::new(document::text(&executor["executable"], "API 原启动控制器")?);
    validate_absolute_path(path, LocalTestPathKind::ExistingFile)?;
    if owner["manifest_sha256"] != state::manifest_binding(run)?["sha256"]
        || common::binding(path)? != intent["executor"]
    {
        return Err("API 后继的控制器与原阶段身份或可执行文件不同".into());
    }
    Ok((number, executor))
}

fn verify_incident(
    backend: &Path,
    attempts: &[Value],
    restart: u64,
    post: &Post,
    intent: &Value,
) -> Result<()> {
    let marker = attempts
        .iter()
        .find(|row| {
            row["stage"] == "target-cache-markers"
                && matches!(row["mode"].as_str(), Some("run" | "reconcile"))
                && row["status"] == "passed"
                && row["number"]
                    .as_u64()
                    .is_some_and(|number| number < restart)
                && row["result"] == intent["marker"]
        })
        .ok_or("API 后继没有已通过的原标记阶段")?;
    let outer = common::read_bound(backend, &marker["result"], "API 事故原标记阶段结果")?;
    let observation = common::read_bound(backend, &outer["observation"], "API 事故原标记观察")?;
    let incident = &intent["incident"];
    if outer["status"] != "target_cache_markers_confirmed"
        || observation["binding"]["completed_copy"]["after_api_incident"]["api_incident"]
            != *incident
        || incident["kind"] != "windows-api-incomplete-cleanup-incident"
        || incident.get("normal_job_exit_proof") != Some(&Value::Null)
    {
        return Err("API 后继的事故证据不同于通过的原标记观察".into());
    }
    let operation = token(&incident["operation_id"])?;
    for (field, filename) in [
        ("api", "api.json".to_owned()),
        ("tree", "api-tree.json".to_owned()),
        ("ready", format!("api-members-{operation}-ready.json")),
        ("failure", format!("api-members-{operation}-stopped.json")),
    ] {
        bound_at(backend, &incident[field], &post.runtime.join(filename))?;
    }
    Ok(())
}

fn verify_generation(
    backend: &Path,
    post: &Post,
    directory: &Path,
    publication: &Value,
    intent: &Value,
    executor: &Value,
) -> Result<Value> {
    for (field, filename) in [("runtime", "runtime.json"), ("binaries", "binaries.json")] {
        let successor = &publication[field];
        bound_at(backend, successor, &directory.join(filename))?;
        let predecessor = common::binding(&post.runtime.join(filename))?;
        if successor["bytes"] != predecessor["bytes"]
            || successor["sha256"] != predecessor["sha256"]
        {
            return Err("API 后继运行合同或产物收据原始字节发生变化".into());
        }
    }
    let contract = read_at(
        backend,
        &publication["runtime"],
        &directory.join("runtime.json"),
    )?;
    let product = runtime::product_root(backend, &contract)?;
    let generation =
        lineage_runtime::collect_bound(backend, &product, directory, &publication["runtime"])?;
    let api = read_at(backend, &publication["api"], &directory.join("api.json"))?;
    let role_tree = read_at(
        backend,
        &publication["api_tree"],
        &directory.join("api-tree.json"),
    )?;
    controller::require_api_tree(directory, &contract)?;
    let operation = token(&intent["token"])?;
    let ready = directory.join(format!("api-members-{operation}-ready.json"));
    let binding = common::binding(&ready)?;
    let expected_ready = json!({"format_version":1,"operation_id":operation,"directory":directory,
        "role":"api","scope_id":contract["scope_id"],"supervisor":role_tree["supervisor"],
        "monitor":role_tree["monitor"]});
    if contract["scope_id"] != intent["incident"]["scope_id"]
        || role_tree["operation_id"] != operation
        || role_tree["process"] != api["identity"]
        || role_tree["supervisor"]["pid"] == role_tree["process"]["pid"]
        || role_tree["process"] == intent["incident"]["identities"]["process"]
        || read_at(backend, &binding, &ready)? != expected_ready
        || generation["role_receipts"]
            != json!([{"role":"api","receipt":publication["api"],
            "identity":api["identity"]}])
        || !generation["generations"].as_array().is_some_and(|rows| {
            matches!(rows.as_slice(), [row] if row["role"] == "api"
                && row["token"] == operation && row["identity"] == api["identity"])
        })
    {
        return Err("API 后继进程树或创建历史不同于预留的唯一 API 代次".into());
    }
    verify_archive(
        backend,
        post,
        directory,
        publication,
        &role_tree,
        executor,
        &contract,
    )?;
    Ok(generation)
}

fn verify_archive(
    backend: &Path,
    post: &Post,
    directory: &Path,
    publication: &Value,
    tree: &Value,
    executor: &Value,
    contract: &Value,
) -> Result<()> {
    let id = token(&tree["operation_id"])?;
    for name in ["source-pair.json", "runtime-evidence.json"] {
        super::files::absent(&directory.join(name))?;
    }
    let archive = directory.join(Role::Api.archive()).join(id);
    let paths = [
        "owner.json",
        "request.json",
        "result.json",
        "candidate.json",
        "supervisor.json",
        "progress.json",
    ]
    .map(|name| archive.join(name));
    let guard = Inputs::open(paths.iter().cloned())?;
    let read = |index: usize| {
        document::read(&common::common_file(
            backend,
            &paths[index],
            "API 启动归档收据",
        )?)
    };
    let owner = read(0)?;
    let request = read(1)?;
    let result = read(2)?;
    let candidate = read(3)?;
    let supervisor = read(4)?;
    let progress = read(5)?;
    if owner["kind"] != "full-stack-api-control-owner"
        || owner["identity"] != *executor
        || owner["operation_id"] != id
        || request["operation"] != "start"
        || request["backend_root"] != contract["backend_root"]
        || request["scope_id"] != tree["scope_id"]
        || request["api_artifact"] != contract["artifacts"]["api"]
        || request["environment_binding"]["file"] != post.request["api_environment"]
        || result["outcome"] != "succeeded"
        || result["state"] != "running"
        || result["identity"] != tree["process"]
        || progress["phase"] != "ready"
        || progress["identity"] != tree["process"]
        || supervisor["identity"] != tree["supervisor"]
    {
        return Err("API 启动归档不同于原控制者、受管启动或进程代次".into());
    }
    let runtime_source =
        json!({"bytes":publication["runtime"]["bytes"],"sha256":publication["runtime"]["sha256"]});
    let source = digest::plan_hash(&json!({"runtime":runtime_source,"runtime_evidence":null}))?;
    let mut request_frame = json!({"format_version":1,"kind":"ryframe-full-stack-worker",
        "operation":"validate","arguments":{"mode":"result","directory":directory,"source":source,
        "owner":owner,"request":request,"value":null,"candidate":candidate,"supervisor":supervisor,
        "runtime_sha256":publication["runtime"]["sha256"],"backend":null,"runtime":null}});
    for (mode, evidence) in [("result", result), ("progress", progress)] {
        request_frame["arguments"]["mode"] = json!(mode);
        request_frame["arguments"]["value"] = evidence;
        worker::execute(&serde_json::to_vec(&request_frame)?)?;
    }
    common::bound_file(
        backend,
        &post.request["api_environment"],
        "API 后继原冻结环境",
    )?;
    guard.check()
}

fn attempt_number(value: &Value, field: &str) -> Result<u64> {
    value[field]
        .as_u64()
        .filter(|value| *value > 0)
        .ok_or_else(|| "API 后继阶段序号无效".into())
}

fn token(value: &Value) -> Result<&str> {
    value
        .as_str()
        .filter(|value| {
            value.len() == 32
                && value
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        })
        .ok_or_else(|| "API 后继操作标识无效".into())
}

fn bound_at(backend: &Path, binding: &Value, path: &Path) -> Result<()> {
    if common::bound_file(backend, binding, "API 后继持久证据")? != path {
        return Err("API 后继持久证据不在固定路径".into());
    }
    Ok(())
}

fn read_at(backend: &Path, binding: &Value, path: &Path) -> Result<Value> {
    bound_at(backend, binding, path)?;
    let value = document::read(path)?;
    bound_at(backend, binding, path)?;
    Ok(value)
}

fn read_receipt(path: &Path) -> Result<Vec<u8>> {
    validate_absolute_path(path, LocalTestPathKind::ExistingFile)?;
    let metadata = fs::symlink_metadata(path)?;
    if common::link_count_for(path, &metadata)? != 1 {
        return Err("API 后继准备收据不能有额外硬链接".into());
    }
    let mut bytes = Vec::new();
    fs::File::open(path)?
        .take(MAX_RECEIPT_BYTES + 1)
        .read_to_end(&mut bytes)?;
    if bytes.is_empty() || bytes.len() as u64 > MAX_RECEIPT_BYTES {
        return Err("API 后继准备收据大小无效".into());
    }
    Ok(bytes)
}

fn prepare_receipts(directory: &Path, expected: [&[u8]; 2]) -> Result<()> {
    let present = prepared_receipts(directory, expected)?;
    let native = fs::canonicalize(directory)?;
    for ((name, bytes), exists) in RECEIPTS.into_iter().zip(expected).zip(present) {
        if !exists {
            let mut temporary = tempfile::NamedTempFile::new_in(&native)?;
            temporary.as_file_mut().write_all(bytes)?;
            temporary.as_file().sync_all()?;
            temporary.persist_noclobber(native.join(name))?;
        }
    }
    Ok(())
}

fn prepared_receipts(directory: &Path, expected: [&[u8]; 2]) -> Result<[bool; 2]> {
    let mut present = [false; 2];
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let name = entry.file_name();
        let index = RECEIPTS
            .iter()
            .position(|candidate| name == *candidate)
            .ok_or("API 后继准备目录含未知文件或进程证据")?;
        if read_receipt(&entry.path())?.as_slice() != expected[index] {
            return Err("API 后继准备收据与原始字节不同，拒绝覆盖".into());
        }
        present[index] = true;
    }
    Ok(present)
}

#[cfg(test)]
#[test]
fn api_successor_preparation_resumes_only_exact_receipts() -> Result<()> {
    let temporary = tempfile::tempdir()?;
    let directory = temporary.path();
    let binaries = directory.join(RECEIPTS[0]);
    let runtime = directory.join(RECEIPTS[1]);
    let raw: [&[u8]; 2] = [b"{ \"build\": 1 }\r\n", b"{ \"runtime\": 2 }\r\n"];
    fs::write(&binaries, b"{\"build\":1}\n")?;
    assert!(prepare_receipts(directory, raw).is_err());
    assert_eq!(fs::read(&binaries)?.as_slice(), b"{\"build\":1}\n");
    assert!(!runtime.try_exists()?);

    fs::write(&binaries, raw[0])?;
    prepare_receipts(directory, raw)?;
    assert_eq!(fs::read(&runtime)?.as_slice(), raw[1]);
    prepare_receipts(directory, raw)?;
    assert_eq!(fs::read(&binaries)?.as_slice(), raw[0]);

    fs::write(directory.join("api.json"), b"{}")?;
    assert!(prepare_receipts(directory, raw).is_err());
    assert_eq!(fs::read(&runtime)?.as_slice(), raw[1]);
    Ok(())
}
