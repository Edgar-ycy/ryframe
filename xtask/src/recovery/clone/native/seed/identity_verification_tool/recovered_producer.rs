//! 绑定曾启动但未取得原身份执行授权的生产者及其正式关闭。
use std::{collections::BTreeSet, ffi::OsStr, process::Stdio};

use super::*;
use crate::{
    local_test_path::{LocalTestPathKind, validate_absolute_path, validate_external_existing_file},
    process::child_command,
    recovery::clone::native::{
        post_process::{command, files},
        target_verify::input::request,
    },
};

const PRODUCER_SOURCE: &str = "xtask/src/data/performance_identities/producer.rs";
const LEGACY_SOURCE_SHA: &str = "49235531fcca04852cbfebcce7f7b7d13ea03ff9ba6ca4f64161ec868b87953d";
const LEGACY_FAILURE: &str =
    "identity_producer_failed：身份生产者未完成；核对原阶段、进程收据及账本，禁止自动重放。\n";

pub(super) struct RecoveredProducer {
    pub(super) attempt: u64,
    pub(super) proof: Value,
    pub(super) descriptors: Vec<Value>,
}

pub(super) fn require(
    backend: &Path,
    run: &Path,
    rows: &[Value],
    number: u64,
    previous: &Value,
    recovered_result: &Value,
    legacy_source: &Value,
) -> Result<RecoveredProducer> {
    let registration = read_at(
        backend,
        previous,
        &run.join("seed-runtime/identity-verification-tool-successor.json"),
    )?;
    let attempt = number_of(&registration["first_attempt"])?;
    let recovery_number = recovery_window(rows, attempt, number)?;
    if registration["format_version"] != 1
        || registration["kind"] != "devex-seed-identity-verification-tool-successor"
        || registration["run_manifest"] != state::manifest_binding(run)?
    {
        return Err("身份生产者关闭未承接原登记的首次失败和相邻正式阶段".into());
    }
    let failed = verify_attempt(rows, attempt)?;
    let (controller, owner) = verify_controller(run, failed, false)?;
    let failure = failed_receipt(backend, run, failed, attempt, &controller)?;
    let session = session(backend, run, attempt, previous, &registration, &controller)?;
    let closed = recovery(
        backend,
        run,
        rows,
        recovery_number,
        recovered_result,
        &session,
    )?;
    let mut descriptors = vec![
        previous.clone(),
        recovered_result.clone(),
        registration["run_manifest"].clone(),
        registration["identity_plan"].clone(),
        failure.clone(),
        controller.clone(),
        session.launch.clone(),
        session.process.clone(),
        session.stderr.clone(),
        session.source.clone(),
        closed.controller.clone(),
        closed.intent.clone(),
    ];
    let build = frozen_source(
        backend,
        &registration,
        &owner,
        legacy_source,
        &mut descriptors,
    )?;
    Ok(RecoveredProducer {
        attempt,
        proof: json!({
            "attempt": attempt,
            "previous": previous,
            "failure": failure,
            "controller": controller,
            "launch": session.launch,
            "producer": session.process,
            "stderr": session.stderr,
            "launch_source": session.source,
            "recovery_result": recovered_result,
            "recovery_controller": closed.controller,
            "recovery_intent": closed.intent,
            "legacy_source": legacy_source,
            "build": build,
        }),
        descriptors,
    })
}

fn recovery_window(rows: &[Value], attempt: u64, number: u64) -> Result<u64> {
    let recovered = attempt
        .checked_add(1)
        .filter(|recovered| *recovered < number)
        .ok_or("身份生产者必须在原失败后的正式关闭阶段退场")?;
    for row in rows {
        if row["number"]
            .as_u64()
            .is_some_and(|prior| prior > recovered && prior < number)
            && (row["stage"] != "seed-runtime" || row["mode"] != MODE || row["status"] != "failed")
        {
            return Err("原身份生产者关闭后的中间阶段不是已收口的身份核验失败".into());
        }
    }
    // 调用方还要逐个核对这些失败，确认没有登记新工具或启动生产者。
    Ok(recovered)
}

fn failed_receipt(
    backend: &Path,
    run: &Path,
    row: &Value,
    attempt: u64,
    controller: &Value,
) -> Result<Value> {
    if row["status"] != "failed"
        || row["error_type"] != "StageError"
        || !row["result"].is_null()
        || row["finished_at"].as_str().is_none_or(str::is_empty)
    {
        return Err("曾启动的身份生产者必须对应已结束且无结果的原失败".into());
    }
    files::absent(&state::result_path(run, attempt))?;
    let path = run.join(format!("failure-{attempt:04}.json"));
    let (descriptor, receipt) = checked_receipt(backend, &path)?;
    if receipt
        != json!({"format_version":2,"kind":"devex-stage-failure","attempt":attempt,
            "stage":"seed-runtime","mode":MODE,"error_type":"StageError",
            "operation_id":"clone.seed-runtime.identities-verify","controller":controller})
    {
        return Err("原身份生产者失败收据未绑定同一控制器".into());
    }
    Ok(descriptor)
}

struct Session {
    launch: Value,
    process: Value,
    identity: Value,
    stderr: Value,
    source: Value,
}

fn session(
    backend: &Path,
    run: &Path,
    attempt: u64,
    previous: &Value,
    registration: &Value,
    controller: &Value,
) -> Result<Session> {
    let output = run
        .join("seed-runtime")
        .join(format!("attempt-{attempt:04}"));
    let launch_path = output.join("session-launch.json");
    let (launch_binding, launch) = checked_receipt(backend, &launch_path)?;
    common::bound_file(backend, &registration["identity_plan"], "原身份核验计划")?;
    validate_external_existing_file(&command::source(backend, "identity-verify")?)?;
    let expected = command::build(&json!({
        "backend":backend,"run_dir":run,"attempt":attempt,"kind":"identity-verify","output":output,
        "tool":registration["tool"],"registration":registration,"configuration":null
    }))?;
    if launch
        != json!({"format_version":1,"kind":"devex-post-producer-launch",
            "run_manifest":registration["run_manifest"],"registration":previous,"controller":controller,
            "attempt":attempt,"producer_kind":"identity-verify","command":expected["argv"],
            "tool":registration["tool"],"source":expected["source"]})
    {
        return Err("原身份生产者启动收据没有绑定实际工具、参数及固定登记".into());
    }
    let process_path = output.join("session-process.json");
    let (process_binding, process) = checked_receipt(backend, &process_path)?;
    let identity = common::validate_process_identity(&process["identity"])?;
    let owner = document::text(&process["identity_lock_owner"], "原身份生产者锁 owner")?;
    if process
        != json!({"format_version":1,"kind":"devex-post-producer","launch":launch_binding,
            "identity":identity,"identity_lock_owner":owner})
        || identity["executable"] != registration["tool"]["path"]
        || !uuid::Uuid::parse_str(owner)
            .is_ok_and(|id| id.get_version_num() == 4 && id.to_string() == owner)
    {
        return Err("原身份进程收据未绑定同一启动、程序和锁 owner".into());
    }
    let stderr = require_legacy_stderr(backend, &output)?;
    Ok(Session {
        launch: launch_binding,
        process: process_binding,
        identity,
        stderr,
        source: launch["source"].clone(),
    })
}

fn checked_receipt(backend: &Path, path: &Path) -> Result<(Value, Value)> {
    let descriptor = common::binding(&common::common_file(backend, path, "原身份生产者证据")?)?;
    let receipt = read_at(backend, &descriptor, path)?;
    Ok((descriptor, receipt))
}

fn require_legacy_stderr(backend: &Path, output: &Path) -> Result<Value> {
    files::absent(&output.join("session.stdout.log"))?;
    files::absent(&output.join("identity-result.json"))?;
    let path = common::common_file(
        backend,
        &output.join("session.stderr.log"),
        "原身份生产者 stderr",
    )?;
    let descriptor = common::binding(&path)?;
    if descriptor["bytes"] != LEGACY_FAILURE.len() as u64
        || fs::read(&path)? != LEGACY_FAILURE.as_bytes()
        || common::binding(&path)? != descriptor
    {
        return Err("原身份生产者 stderr 与冻结的唯一失败消息不同".into());
    }
    Ok(descriptor)
}

struct Closed {
    intent: Value,
    controller: Value,
}

fn recovery(
    backend: &Path,
    run: &Path,
    rows: &[Value],
    number: u64,
    descriptor: &Value,
    session: &Session,
) -> Result<Closed> {
    let row = rows
        .iter()
        .find(|row| row["number"] == number)
        .ok_or("缺少相邻的原身份生产者正式关闭阶段")?;
    if row["stage"] != "seed-runtime"
        || row["mode"] != "recover-session"
        || row["status"] != "passed"
        || row["finished_at"].as_str().is_none_or(str::is_empty)
        || !row["error_type"].is_null()
        || row["result"] != *descriptor
    {
        return Err("原身份生产者关闭未绑定相邻且已通过的正式结果".into());
    }
    let result = read_at(backend, descriptor, &state::result_path(run, number))?;
    if result
        != json!({
            "status":"seed_session_recovered","producer":session.process,"already_stopped":true,
            "copy_requires_reconciliation":true,"remote_writes":0,"restore_qualified":false
        })
    {
        return Err("正式关闭结果未证明同一原身份生产者之前已经退出".into());
    }
    let path = run
        .join("seed-runtime")
        .join(format!("attempt-{number:04}/session-recovery-intent.json"));
    let (intent, value) = checked_receipt(backend, &path)?;
    if value
        != json!({
            "producer":session.process,"identity":session.identity,
            "action":"terminate_exact_producer","copy_requires_reconciliation":true
        })
    {
        return Err("原身份生产者关闭意图与启动进程身份不同".into());
    }
    let (controller, _) = verify_controller(run, row, false)?;
    Ok(Closed { intent, controller })
}

fn frozen_source(
    backend: &Path,
    registration: &Value,
    owner: &Value,
    legacy_source: &Value,
    descriptors: &mut Vec<Value>,
) -> Result<Value> {
    let build = &registration["build"];
    let tool = continuation_tool::verify(backend, build, PURPOSE, owner)?;
    if tool != registration["tool"] {
        return Err("原身份生产者工具不同于原通过的冻结构建".into());
    }
    let declared = common::read_bound(backend, build, "原身份生产者构建请求")?;
    let (tool_path, tool_binding) = external_binding(&tool)?;
    let (validation_path, validation) = external_binding(&declared["validation"])?;
    let (frozen_path, frozen_binding) = external_binding(&declared["frozen"])?;
    let directory = frozen_path.parent().ok_or("原冻结构建缺少证据目录")?;
    if tool_path.parent() != Some(directory) || validation_path.parent() != Some(directory) {
        return Err("原冻结构建的程序、结果与收据不在同一固定目录".into());
    }
    let frozen = verified_external_json(&frozen_path, &frozen_binding)?;
    document::exact(
        &frozen,
        &[
            "Tool",
            "ToolSha",
            "ResultSHA",
            "SourceBase",
            "ArchivedInputs",
            "SourceInputsSHA",
        ],
        "原冻结构建收据",
    )?;
    let source_inputs_path =
        validate_external_existing_file(&directory.join("source-inputs.json"))?;
    let source_inputs_binding = common::binding(&source_inputs_path)?;
    if !same_sha(&source_inputs_binding["sha256"], &frozen["SourceInputsSHA"]) {
        return Err("原冻结构建没有绑定实际的源码归档清单".into());
    }
    let inputs = verified_external_json(&source_inputs_path, &source_inputs_binding)?;
    let archived = archived_delta(directory, &frozen, &inputs)?;
    let legacy = common::bound_file(backend, legacy_source, "原编译来源生产者副本")?;
    if legacy_source["sha256"] != LEGACY_SOURCE_SHA {
        return Err("原生产者源码副本不匹配已核对的旧选取逻辑".into());
    }
    verify_git_source(&inputs, &legacy)?;
    descriptors.extend([
        build.clone(),
        tool_binding.clone(),
        validation.clone(),
        frozen_binding.clone(),
        source_inputs_binding.clone(),
        legacy_source.clone(),
    ]);
    descriptors.extend(archived.iter().cloned());
    Ok(json!({
        "request":build,"tool":tool_binding,"validation":validation,"frozen":frozen_binding,
        "source_inputs":source_inputs_binding,"source_base":inputs["Base"],"archived_inputs":archived,
    }))
}

fn external_binding(descriptor: &Value) -> Result<(PathBuf, Value)> {
    let path = request::external_file(descriptor)?;
    let binding = common::binding(&path)?;
    if !same_sha(&descriptor["sha256"], &binding["sha256"]) {
        return Err("原冻结文件在检查期间变化".into());
    }
    Ok((path, binding))
}

fn verified_external_json(path: &Path, binding: &Value) -> Result<Value> {
    use sha2::{Digest, Sha256};

    let bytes = fs::read(path)?;
    if binding["bytes"] != bytes.len() as u64
        || binding["sha256"] != hex::encode(Sha256::digest(&bytes))
    {
        return Err("读取的原冻结构建元数据与已登记摘要不同".into());
    }
    Ok(serde_json::from_slice(&bytes)?)
}

fn same_sha(left: &Value, right: &Value) -> bool {
    matches!((left.as_str(), right.as_str()), (Some(left), Some(right))
        if left.len() == 64 && left.bytes().all(|byte| byte.is_ascii_hexdigit()) && left.eq_ignore_ascii_case(right))
}

fn archived_delta(directory: &Path, frozen: &Value, inputs: &Value) -> Result<Vec<Value>> {
    document::exact(inputs, &["Base", "Tree", "Inputs"], "原五文件源码归档清单")?;
    let rows = document::array(&inputs["Inputs"], "原五文件源码归档记录")?;
    if inputs["Base"] != frozen["SourceBase"] || frozen["ArchivedInputs"] != 5 || rows.len() != 5 {
        return Err("原冻结构建的来源基线或归档数量不同".into());
    }
    let archive = directory.join("verified-source-delta");
    files::absent(&archive.join(PRODUCER_SOURCE))?;
    let mut seen = BTreeSet::new();
    let mut bindings = Vec::new();
    for row in rows {
        document::exact(row, &["Path", "Sha256"], "原源码归档条目")?;
        let path = document::text(&row["Path"], "原源码归档相对路径")?;
        if !path.starts_with("xtask/src/")
            || !path.ends_with(".rs")
            || path.contains(['\\', ':'])
            || path.split('/').any(|part| matches!(part, "" | "." | ".."))
            || path.eq_ignore_ascii_case(PRODUCER_SOURCE)
            || !seen.insert(path.to_ascii_lowercase())
        {
            return Err("原源码归档有重复或无效路径，或包含生产者源码".into());
        }
        let absolute = validate_external_existing_file(&archive.join(path))?;
        let binding = common::binding(&absolute)?;
        if !same_sha(&binding["sha256"], &row["Sha256"]) {
            return Err("原冻结构建的源码归档摘要变化".into());
        }
        bindings.push(binding);
    }
    Ok(bindings)
}

fn verify_git_source(inputs: &Value, legacy: &Path) -> Result<()> {
    let base = document::text(&inputs["Base"], "原冻结构建 Git 来源")?;
    if base.len() != 40
        || !base
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        return Err("原冻结构建缺少规范完整 Git 提交".into());
    }
    let tree = validate_absolute_path(
        Path::new(document::text(&inputs["Tree"], "原冻结构建检出路径")?),
        LocalTestPathKind::ExistingDirectory,
    )?;
    let compiled_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .ok_or("当前后端源码目录缺失")?;
    let layout_args = [
        "rev-parse",
        "--path-format=absolute",
        "--show-toplevel",
        "--git-common-dir",
    ];
    let layout = String::from_utf8(read_git(&tree, &layout_args)?)?;
    let current = String::from_utf8(read_git(compiled_root, &layout_args)?)?;
    let old_dirs = layout.lines().collect::<Vec<_>>();
    let current_dirs = current.lines().collect::<Vec<_>>();
    let ([old_root, old_common], [current_root, current_common]) =
        (old_dirs.as_slice(), current_dirs.as_slice())
    else {
        return Err("原冻结构建和当前后端缺少完整 Git 工作树归属".into());
    };
    if fs::canonicalize(old_root)? != fs::canonicalize(&tree)?
        || fs::canonicalize(current_root)? != fs::canonicalize(compiled_root)?
        || fs::canonicalize(old_common)? != fs::canonicalize(current_common)?
    {
        return Err("原冻结构建和当前后端不属于同一 Git 对象库".into());
    }
    let object = format!("{base}:{PRODUCER_SOURCE}");
    let bytes = read_git(&tree, &["cat-file", "blob", &object])?;
    if bytes != fs::read(legacy)? {
        return Err("原编译 Git blob 与已绑定的旧生产者源码副本不同".into());
    }
    Ok(())
}

fn read_git(root: &Path, args: &[&str]) -> Result<Vec<u8>> {
    let mut git = child_command(OsStr::new("git"));
    for (key, _) in std::env::vars_os() {
        if key
            .to_string_lossy()
            .to_ascii_uppercase()
            .starts_with("GIT_")
        {
            git.env_remove(key);
        }
    }
    let output = git
        .arg("--no-pager")
        .args(args)
        .current_dir(root)
        .stdin(Stdio::null())
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_NO_REPLACE_OBJECTS", "1")
        .env("GIT_NO_LAZY_FETCH", "1")
        .env("GIT_ALLOW_PROTOCOL", "")
        .output()?;
    if !output.status.success() {
        return Err("无法从原冻结构建的同一 Git 对象库读取源码".into());
    }
    Ok(output.stdout)
}

#[cfg(test)]
#[test]
fn recovered_producer_window_accepts_only_failed_verification_attempts() -> Result<()> {
    let mut rows = [json!({"number":72,"stage":"seed-runtime","mode":MODE,"status":"failed"})];
    assert_eq!(recovery_window(&rows, 70, 73)?, 71);
    assert!(recovery_window(&rows, 70, 71).is_err());
    rows[0]["status"] = json!("running");
    assert!(recovery_window(&rows, 70, 73).is_err());
    rows[0]["status"] = json!("failed");
    rows[0]["mode"] = json!("prepare");
    assert!(recovery_window(&rows, 70, 73).is_err());
    Ok(())
}

#[cfg(test)]
#[test]
fn legacy_early_exit_requires_exact_stderr_without_success_output() -> Result<()> {
    let fixture = tempfile::tempdir()?;
    let backend = fixture.path();
    let output = backend.join(".local-tests/session");
    fs::create_dir_all(&output)?;
    let stderr = output.join("session.stderr.log");
    fs::write(&stderr, LEGACY_FAILURE)?;
    assert_eq!(
        require_legacy_stderr(backend, &output)?,
        common::binding(&stderr)?
    );
    for filename in ["session.stdout.log", "identity-result.json"] {
        let path = output.join(filename);
        fs::write(&path, b"{}")?;
        assert!(require_legacy_stderr(backend, &output).is_err());
        fs::remove_file(path)?;
    }
    fs::write(&stderr, format!("{LEGACY_FAILURE}额外输出\n"))?;
    assert!(require_legacy_stderr(backend, &output).is_err());
    Ok(())
}
