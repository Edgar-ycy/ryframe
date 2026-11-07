//! 首代服务沿固定 RustFS → Redis → 五桶顺序推进，失败保留原账本。
use super::storage::{self, check};
use crate::{
    Result,
    recovery::clone::native::{
        digest, json as document,
        lifecycle::{
            lock::{ProcessGuard, RunLock},
            state,
        },
        resources::common,
    },
};
use serde_json::{Value, json};
use std::{
    fs,
    path::{Path, PathBuf},
};

#[path = "initial/buckets.rs"]
mod buckets;
#[path = "initial/inputs.rs"]
mod inputs;
#[path = "initial/redis.rs"]
mod redis;
use inputs::Inputs;

pub(in crate::recovery::fixture_control) fn run(
    backend: &Path,
    review: &Path,
    environment: &Path,
    operation: &str,
) -> Result<Value> {
    let inputs = Inputs::load(backend, review, environment)?;
    match operation {
        "rustfs" => rustfs(&inputs),
        "redis" => redis::run(&inputs),
        "buckets" => buckets::run(&inputs),
        _ => Err("该首代服务阶段尚未接入原生编排".into()),
    }
}

fn rustfs(inputs: &Inputs) -> Result<Value> {
    let service = &inputs.review["services"]["rustfs"];
    let run = &inputs.run;
    let root = run.parent().ok_or("首代服务缺少运行根")?;
    empty_directory(&inputs.backend, root, true)?;
    check(!run.try_exists()?, "首代服务账本已经存在，禁止覆盖或重放")?;
    for key in ["api", "console"] {
        common::require_closed_port(document::text(&service[key], "RustFS 端点")?)?;
    }
    let data = PathBuf::from(document::text(&service["data_dir"], "RustFS 数据目录")?);
    empty_directory(&inputs.backend, &data, false)?;
    inputs.guard()?;
    common::ensure_directory(&inputs.backend, root, "首代服务根")?;
    common::ensure_new_directory(&inputs.backend, run, "首代服务账本")?;
    let output = run.join("rustfs");
    common::ensure_new_directory(&inputs.backend, &output, "首代 RustFS 输出")?;
    if let Some(parent) = data.parent() {
        common::ensure_directory(&inputs.backend, parent, "RustFS 数据父目录")?;
    }
    common::ensure_directory(&inputs.backend, &data, "RustFS 数据目录")?;
    let manifest = json!({"format_version":1,"kind":"reference-fixture-service-run",
        "review":inputs.sources["review"],"bootstrap":inputs.sources["bootstrap"],
        "execution_backend":inputs.bootstrap["execution_backend"],"scope_id":service["scope_id"],
        "data_directory_was_empty":true});
    document::write_new(&run.join("manifest.json"), &manifest)?;
    state::initialize(run)?;
    let request = json!({"scope_id":service["scope_id"],"executable":inputs.review["tools"]["rustfs"],
        "data_directory":directory_identity(&data)?,"api_url":service["api"],"console_url":service["console"],
        "credential_files":{"access_key":inputs.bootstrap["secret_files"]["rustfs-access-key.txt"],
            "secret_key":inputs.bootstrap["secret_files"]["rustfs-secret-key.txt"]},"timeout_seconds":60});
    document::write_new(&output.join("request.json"), &request)?;
    let manifest = common::binding(&run.join("manifest.json"))?;
    let request_binding = common::binding(&output.join("request.json"))?;
    let sources = json!({"review":inputs.sources["review"],"bootstrap":inputs.sources["bootstrap"],
        "request":request_binding});
    let _guard = ProcessGuard::acquire(run)?;
    let lock = RunLock::claim(run)?;
    let outcome = initial_attempt(inputs, &lock, &output, &request, &manifest, &sources);
    let released = lock.release();
    let result = outcome?;
    released?;
    Ok(
        json!({"status":"rustfs_started","storage":result,"run":run,"remote_writes":0,
        "services_started":true,"request_sha256":digest::plan_hash(&request)?}),
    )
}

fn initial_attempt(
    inputs: &Inputs,
    lock: &RunLock,
    output: &Path,
    request: &Value,
    manifest: &Value,
    sources: &Value,
) -> Result<Value> {
    inputs.guard()?;
    lock.verify()?;
    let (number, attempt) = state::begin(&inputs.run, "storage-target", "initial", sources)?;
    state::bind_controller(&inputs.run, number, &lock.owner, &attempt)?;
    let controller = common::binding(&state::controller_path(&inputs.run, number))?;
    let result = storage::start(
        storage::Start {
            backend: &inputs.backend,
            request,
            private: &inputs.private,
            output,
            manifest,
            controller: &controller,
            number,
        },
        || {
            lock.verify()?;
            inputs.guard()?;
            common::bound_file(&inputs.backend, manifest, "RustFS 固定清单")?;
            common::bound_file(&inputs.backend, &sources["request"], "RustFS 固定请求")?;
            Ok(())
        },
    );
    match result {
        Ok(result) => {
            state::finish(&inputs.run, number, Some(&result), None)?;
            Ok(result)
        }
        Err(error) => {
            state::finish(&inputs.run, number, None, Some("NativeStorageStartFailed"))?;
            Err(error)
        }
    }
}

fn empty_directory(backend: &Path, path: &Path, require_parent: bool) -> Result<()> {
    crate::local_test_path::validate_local_test_path(
        path,
        backend,
        crate::local_test_path::LocalTestPathKind::StateDirectory,
    )?;
    check(
        !digest::linked_path(path)?.contains(&true),
        "首代服务目录经过链接",
    )?;
    if path.try_exists()? {
        check(
            path.is_dir() && fs::read_dir(path)?.next().is_none(),
            "首代服务目录不是明确空目录",
        )?;
    } else if require_parent {
        check(
            path.parent().is_some_and(Path::is_dir),
            "首代服务根父目录缺失",
        )?;
    }
    Ok(())
}

fn directory_identity(path: &Path) -> Result<Value> {
    let metadata = fs::symlink_metadata(path)?;
    let (device, inode) = common::device_inode_for(path, &metadata)?;
    Ok(json!({"path":path,"device":device,"inode":inode}))
}

fn prefix(inputs: &Inputs, stages: &[(&str, &str)]) -> Result<Value> {
    let saved = state::load(&inputs.run, true)?;
    let attempts = state::attempts(&saved)?;
    check(
        attempts.len() == stages.len()
            && attempts.iter().zip(stages).all(|(attempt, (stage, mode))| {
                attempt["stage"] == *stage
                    && attempt["mode"] == *mode
                    && attempt["status"] == "passed"
            }),
        "首代服务只能接续同一账本唯一完整成功前缀；未知或失败禁止重放",
    )?;
    let manifest = document::read(&inputs.run.join("manifest.json"))?;
    super::history::validate_manifest(&manifest)?;
    check(
        manifest["bootstrap"] == inputs.sources["bootstrap"],
        "首代服务不属于原始 bootstrap",
    )?;
    super::history::validate(&inputs.run, &saved, false, false)?;
    Ok(saved)
}

fn finish(inputs: &Inputs, number: u64, result: Result<Value>, failure: &str) -> Result<Value> {
    match result {
        Ok(value) => {
            state::finish(&inputs.run, number, Some(&value), None)?;
            Ok(value)
        }
        Err(error) => {
            state::finish(&inputs.run, number, None, Some(failure))?;
            Err(error)
        }
    }
}
