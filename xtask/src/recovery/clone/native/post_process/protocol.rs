use std::{
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
};

use serde_json::{Value, json};

use super::{
    super::{
        digest, json as json_io, lifecycle::state, resources::common, target_verify::guard::Inputs,
    },
    command, inspection, lock, registration,
};
use crate::{
    Result,
    local_test_path::{LocalTestPathKind, validate_local_test_path},
};

const ENTRY: &str = "__clone-post-process";

pub(crate) fn run_hidden_if_requested() -> Option<i32> {
    if std::env::args().nth(1).as_deref() != Some(ENTRY) {
        return None;
    }
    let result = run();
    let response = match &result {
        Ok(value) => json!({"ok":true,"result":value}),
        Err(error) => json!({"ok":false,"error_type":category(error.as_ref())}),
    };
    let written = (|| -> Result<()> {
        let mut output = std::io::stdout().lock();
        serde_json::to_writer(&mut output, &response)?;
        output.write_all(b"\n")?;
        output.flush()?;
        Ok(())
    })();
    Some(if result.is_ok() && written.is_ok() {
        0
    } else {
        1
    })
}

fn run() -> Result<Value> {
    let mut bytes = Vec::new();
    std::io::stdin()
        .take(1024 * 1024 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > 1024 * 1024 {
        return Err("后处理请求超过 1 MiB".into());
    }
    let text = std::str::from_utf8(&bytes).map_err(|_| "后处理请求不是 UTF-8")?;
    let request = json_io::parse_fragment(text, "后处理请求")?;
    let root = json_io::exact(&request, &["operation", "payload"], "后处理请求")?;
    match json_io::text(&root["operation"], "后处理操作")? {
        "command" => command::build(&root["payload"]),
        "inspect" => inspection::inspect(&root["payload"]),
        "quiet" => inspection::quiet(&root["payload"]),
        "recover-session" => recover_session(&root["payload"]),
        "prepare-launch" => prepare_launch(&root["payload"]),
        "record-process" => record_process(&root["payload"]),
        "terminate-process" => terminate_process(&root["payload"]),
        "record-cleanup-failure" => record_cleanup_failure(&root["payload"]),
        _ => Err("未知后处理操作".into()),
    }
}

pub(super) fn terminate_process(request: &Value) -> Result<Value> {
    let root = json_io::exact(request, &["identity"], "生产者异常清理请求")?;
    let identity = common::validate_process_identity(&root["identity"])?;
    let pid = u32::try_from(identity["pid"].as_u64().ok_or("生产者 PID 无效")?)
        .map_err(|_| "生产者 PID 超出范围")?;
    match common::process_identity(pid)? {
        Some(current) if current == identity => {
            common::terminate_tree(&identity)?;
        }
        Some(_) => return Err("生产者 PID 已复用，保留现场，不终止其他进程".into()),
        None => return Ok(json!({"terminated":false,"already_stopped":true})),
    }
    if common::process_identity(pid)?.as_ref() == Some(&identity) {
        return Err("生产者异常清理后原创建身份仍在运行".into());
    }
    Ok(json!({"terminated":true,"already_stopped":false}))
}

pub(super) fn record_cleanup_failure(request: &Value) -> Result<Value> {
    let root = json_io::exact(
        request,
        &["backend", "output", "label", "error_type"],
        "生产者清理诊断请求",
    )?;
    let backend = path(&root["backend"], "后端目录")?;
    let output = path(&root["output"], "生产者输出目录")?;
    if !output.starts_with(&backend) || !output.is_dir() {
        return Err("生产者清理诊断目录不属于后端工作区".into());
    }
    let label = json_io::text(&root["label"], "清理诊断标签")?;
    let error_type = json_io::text(&root["error_type"], "清理错误类型")?;
    let valid = |value: &str| {
        !value.is_empty()
            && value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
    };
    if !valid(label) || !valid(error_type) {
        return Err("清理诊断标签或错误类型无效".into());
    }
    let destination = output.join(format!("{label}-error.json"));
    json_io::write_new(&destination, &json!({"error_type":error_type}))?;
    common::binding(&destination)
}

pub(super) fn record_process(request: &Value) -> Result<Value> {
    let root = json_io::exact(
        request,
        &["output", "identity", "kind", "owner"],
        "生产者进程登记请求",
    )?;
    let output = path(&root["output"], "attempt 目录")?;
    let identity = common::validate_process_identity(&root["identity"])?;
    let pid = u32::try_from(identity["pid"].as_u64().ok_or("生产者 PID 无效")?)
        .map_err(|_| "生产者 PID 超出范围")?;
    if common::process_identity(pid)?.as_ref() != Some(&identity) {
        return Err("生产者创建身份已结束、被复用或发生变化".into());
    }
    let launch = json_io::read(&output.join("session-launch.json"))?;
    if identity["executable"] != launch["tool"]["path"] {
        return Err("生产者 executable 与已授权启动工具不同".into());
    }
    inspection::require_actual_argv(&identity, &launch["command"])?;
    let kind = json_io::text(&root["kind"], "生产者类型")?;
    let mut receipt = json!({"format_version":1,"kind":"devex-post-producer","launch":common::binding(&output.join("session-launch.json"))?,"identity":identity});
    if kind != "existing" {
        let owner = json_io::text(&root["owner"], "生产者 owner")?;
        let parsed = uuid::Uuid::parse_str(owner).map_err(|_| "生产者 owner 无效")?;
        if parsed.get_version_num() != 4 {
            return Err("生产者 owner 必须为 UUID v4".into());
        }
        let field = if kind.starts_with("identity-") {
            "identity_lock_owner"
        } else {
            "producer_owner"
        };
        receipt[field] = json!(owner);
    }
    json_io::write_new(&output.join("session-process.json"), &receipt)?;
    Ok(receipt)
}

pub(super) fn prepare_launch(request: &Value) -> Result<Value> {
    let root = json_io::exact(
        request,
        &[
            "backend",
            "run_dir",
            "attempt",
            "kind",
            "output",
            "tool",
            "registration",
            "registration_binding",
            "configuration",
            "owner",
            "controller_identity",
        ],
        "生产者启动请求",
    )?;
    let run = path(&root["run_dir"], "运行目录")?;
    let output = path(&root["output"], "attempt 目录")?;
    let number = root["attempt"]
        .as_u64()
        .filter(|v| *v > 0)
        .ok_or("生产者 attempt 无效")?;
    let state_value = state::load(&run, true)?;
    let attempt = state::attempts(&state_value)?
        .last()
        .ok_or("生产者启动缺少阶段")?;
    if attempt["number"].as_u64() != Some(number)
        || attempt["status"] != "running"
        || output
            != run
                .join(attempt["stage"].as_str().ok_or("阶段无效")?)
                .join(format!("attempt-{number:04}"))
    {
        return Err("原生启动必须属于当前运行阶段".into());
    }
    verify_controller(&run, &root["controller_identity"])?;
    let backend = path(&root["backend"], "后端目录")?;
    if registration::resolve(
        &backend,
        &run,
        json_io::text(&attempt["stage"], "生产者阶段")?,
        &root["registration_binding"],
        Some(number),
        false,
    )? != root["registration"]
    {
        return Err("生产者请求不同于当前绑定登记".into());
    }
    let plan = command::build(
        &json!({"backend":root["backend"],"run_dir":root["run_dir"],"attempt":number,"kind":root["kind"],"output":root["output"],"tool":root["tool"],"registration":root["registration"],"configuration":root["configuration"]}),
    )?;
    let launch_path = output.join("session-launch.json");
    let launch = json!({"format_version":1,"kind":"devex-post-producer-launch","run_manifest":common::binding(&run.join("manifest.json"))?,"registration":root["registration_binding"],"controller":common::binding(&state::controller_path(&run,number))?,"attempt":number,"producer_kind":root["kind"],"command":plan["argv"],"tool":plan["tool"],"source":plan["source"]});
    let mut launch = launch;
    if !root["configuration"].is_null() {
        launch["configuration"] = common::binding(&path(&root["configuration"], "已有数据配置")?)?;
    }
    json_io::write_new(&launch_path, &launch)?;
    Ok(json!({"launch":launch,"command":plan["argv"],"owner":root["owner"]}))
}

fn recovery_stage(request: &Value) -> Result<(PathBuf, PathBuf, u64)> {
    let root = json_io::exact(
        request,
        &[
            "backend",
            "run_dir",
            "current_attempt",
            "descriptor",
            "attempt",
            "registration",
            "registration_binding",
            "identity_lock",
            "controller_identity",
        ],
        "会话恢复请求",
    )?;
    let backend = path(&root["backend"], "后端目录")?;
    let run = path(&root["run_dir"], "运行目录")?;
    let current = root["current_attempt"]
        .as_u64()
        .filter(|v| *v > 0)
        .ok_or("恢复 attempt 无效")?;
    let state_value = state::load(&run, false)?;
    let latest = state::attempts(&state_value)?
        .last()
        .ok_or("恢复阶段缺失")?;
    if latest["number"].as_u64() != Some(current)
        || latest["mode"] != "recover-session"
        || latest["status"] != "running"
        || latest["stage"] != root["attempt"]["stage"]
        || root["attempt"]["number"]
            .as_u64()
            .is_none_or(|number| number >= current)
        || !state::attempts(&state_value)?.contains(&root["attempt"])
    {
        return Err("原生回收必须属于明确恢复阶段".into());
    }
    verify_controller(&run, &root["controller_identity"])?;
    Ok((backend, run, current))
}

pub(super) fn recover_session(root: &Value) -> Result<Value> {
    let (backend, run, current) = recovery_stage(root)?;
    let inspection = json!({"backend":backend,"run_dir":run,"attempt":root["attempt"],
        "registration":root["registration"],"registration_binding":root["registration_binding"]});
    let observed = inspection::inspect(&inspection)?;
    if observed.is_null() || observed["receipt"] != root["descriptor"] {
        return Err("明确生产者收据发生变化".into());
    }
    let identity_lock = identity_lock_request(
        &backend,
        &run,
        &root["attempt"],
        &root["registration"],
        &observed["value"],
    )?;
    if identity_lock != root["identity_lock"] {
        return Err("回收的身份锁与历史生产者和登记不同".into());
    }
    let identity_plan = protect_identity_plan(&backend, &root["registration"], &identity_lock)?;
    let controller = &observed["controller"]["owner"]["identity"];
    let controller_pid = u32::try_from(controller["pid"].as_u64().ok_or("控制器 PID 无效")?)
        .map_err(|_| "控制器 PID 超出范围")?;
    if common::process_identity(controller_pid)?.is_some() {
        return Err("原控制进程仍存在或 PID 已复用，不能回收其原生".into());
    }
    let output = run
        .join(root["attempt"]["stage"].as_str().ok_or("历史阶段无效")?)
        .join(format!("attempt-{current:04}"));
    fs::create_dir(&output)?;
    json_io::write_new(
        &output.join("session-recovery-intent.json"),
        &json!({"producer":root["descriptor"],"identity":observed["value"]["identity"],"action":"terminate_exact_producer","copy_requires_reconciliation":true}),
    )?;
    let current_observed = inspection::inspect(&inspection)?;
    if current_observed != observed {
        return Err("回收窗口内生产者证据或内核身份发生变化".into());
    }
    if current_observed["alive"] == true {
        common::terminate_tree(&current_observed["value"]["identity"])?;
    }
    if common::process_identity(
        u32::try_from(
            current_observed["value"]["identity"]["pid"]
                .as_u64()
                .unwrap_or_default(),
        )
        .map_err(|_| "生产者 PID 超出范围")?,
    )?
    .is_some()
    {
        return Err("原生生产者回收后身份仍不确定".into());
    }
    if !identity_lock.is_null() {
        release_identity_lock(&backend, &run, &identity_lock)?;
    }
    if let Some(identity_plan) = identity_plan {
        identity_plan.check()?;
    }
    Ok(
        json!({"status":if root["attempt"]["stage"]=="seed-runtime"{"seed_session_recovered"}else{"post_session_recovered"},"producer":root["descriptor"],"already_stopped":current_observed["alive"]==false,"copy_requires_reconciliation":true,"remote_writes":0,"restore_qualified":false}),
    )
}

pub(super) fn identity_lock_request(
    backend: &Path,
    run: &Path,
    attempt: &Value,
    registration: &Value,
    process: &Value,
) -> Result<Value> {
    if attempt["stage"] == "seed-runtime"
        && matches!(
            attempt["mode"].as_str(),
            Some("identities-apply" | "identities-verify")
        )
        && process.get("identity_lock_owner").is_some()
    {
        identity_state(backend, run, &registration["identity_state"])?;
        let mut plan = common::read_bound(backend, &registration["identity_plan"], "原身份计划")?;
        if plan["format_version"] != 1 || plan["kind"] != "devex-identities" {
            return Err("回收身份锁需要原身份计划版本和类型".into());
        }
        let declared = plan
            .as_object_mut()
            .and_then(|fields| fields.remove("plan_sha256"))
            .ok_or("回收身份锁缺少原计划摘要")?;
        if declared != digest::plan_hash(&plan)? {
            return Err("原身份计划正文摘要不同，拒绝回收锁".into());
        }
        Ok(
            json!({"backend":backend,"identity_state":registration["identity_state"],
            "identity":process["identity"],"owner_token":process["identity_lock_owner"],
            "plan_sha256":declared}),
        )
    } else {
        Ok(Value::Null)
    }
}

fn protect_identity_plan(
    backend: &Path,
    registration: &Value,
    request: &Value,
) -> Result<Option<Inputs>> {
    if request.is_null() {
        return Ok(None);
    }
    let bound = &registration["identity_plan"];
    let input = Inputs::open([common::bound_file(backend, bound, "回收身份计划")?])?;
    common::bound_file(backend, bound, "回收原身份计划")?;
    Ok(Some(input))
}

fn identity_state(backend: &Path, run: &Path, value: &Value) -> Result<PathBuf> {
    let path = Path::new(json_io::text(value, "回收身份账本路径")?);
    if path != run.join("seed-runtime/identities") {
        return Err("身份锁必须属于本运行的固定账本".into());
    }
    Ok(validate_local_test_path(
        path,
        backend,
        LocalTestPathKind::StateDirectory,
    )?)
}

pub(super) fn release_identity_lock(backend: &Path, run: &Path, request: &Value) -> Result<()> {
    let path = identity_state(backend, run, &request["identity_state"])?;
    match fs::symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
        Ok(_) => lock::recover(request).map(|_| ()),
    }
}

fn verify_controller(run: &std::path::Path, value: &Value) -> Result<()> {
    let expected = common::validate_process_identity(value)?;
    let pid = u32::try_from(expected["pid"].as_u64().ok_or("控制器 PID 无效")?)
        .map_err(|_| "控制器 PID 超出范围")?;
    if common::process_identity(pid)?.as_ref() != Some(&expected) {
        return Err("请求控制器身份已结束、被复用或发生变化".into());
    }
    if json_io::read(&run.join("run.lock/owner.json"))?["identity"] != expected {
        return Err("请求控制器未持当前 run 锁".into());
    }
    Ok(())
}

fn path(value: &Value, label: &str) -> Result<PathBuf> {
    let path = PathBuf::from(json_io::text(value, label)?);
    if !path.is_absolute() {
        return Err(format!("{label}必须为绝对路径").into());
    }
    Ok(path)
}
fn category(error: &dyn std::error::Error) -> String {
    let value = error.to_string();
    if value.contains("PID") || value.contains("进程") {
        "ProcessEvidenceError"
    } else if value.contains("锁") {
        "IdentityLockError"
    } else if value.contains("运行") {
        "RuntimeProofError"
    } else {
        "PostProcessError"
    }
    .into()
}
