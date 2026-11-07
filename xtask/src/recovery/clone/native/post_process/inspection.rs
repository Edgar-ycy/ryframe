use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use super::{
    super::{
        digest, json as json_io,
        lifecycle::{manifest, state},
        resources::common,
    },
    command, registration,
};
use crate::Result;
#[cfg(windows)]
use crate::recovery::fixture_control::services::lifecycle::readonly::process;

/// 未登记生产者的历史尝试必须确实没有启动、进程或输出证据；不采纳缺失一半的未知启动。
pub(super) fn require_unlaunched(
    backend: &Path,
    run: &Path,
    stage: &str,
    number: u64,
) -> Result<()> {
    use crate::local_test_path::{LocalTestPathKind, validate_local_test_path};
    let output = run.join(stage).join(format!("attempt-{number:04}"));
    validate_local_test_path(&output, backend, LocalTestPathKind::StateDirectory)?;
    for name in [
        "session-launch.json",
        "session-process.json",
        "session.stderr.log",
        "session.stdout.log",
    ] {
        super::files::absent(&output.join(name))?;
    }
    Ok(())
}

pub(super) fn inspect(request: &Value) -> Result<Value> {
    let root = json_io::exact(
        request,
        &[
            "backend",
            "run_dir",
            "attempt",
            "registration",
            "registration_binding",
        ],
        "生产者核验请求",
    )?;
    let backend = path(&root["backend"], "后端目录")?;
    let run_dir = path(&root["run_dir"], "运行目录")?;
    let attempt = &root["attempt"];
    let number = attempt["number"]
        .as_u64()
        .filter(|v| *v > 0)
        .ok_or("生产者 attempt 无效")?;
    let stage = json_io::text(&attempt["stage"], "生产者阶段")?;
    if !matches!(stage, "post-copy" | "seed-runtime") {
        return Err("原生生产者历史不属于明确阶段".into());
    }
    let output = run_dir.join(stage).join(format!("attempt-{number:04}"));
    if registration::resolve(
        &backend,
        &run_dir,
        stage,
        &root["registration_binding"],
        Some(number),
        true,
    )? != root["registration"]
    {
        return Err("生产者登记内容或绑定已经变化".into());
    }
    let launch_path = output.join("session-launch.json");
    let process_path = output.join("session-process.json");
    if !launch_path.exists() && !process_path.exists() {
        if output.exists()
            && ["session.stderr.log", "session.stdout.log"]
                .iter()
                .any(|name| output.join(name).exists())
        {
            return Err("原生生产者证据不完整，缺少明确启动收据".into());
        }
        return Ok(Value::Null);
    }
    let launch = json_io::read(&launch_path)?;
    let process = json_io::read(&process_path)?;
    let kind = json_io::text(&launch["producer_kind"], "生产者类型")?;
    let mut launch_fields = vec![
        "format_version",
        "kind",
        "run_manifest",
        "registration",
        "controller",
        "attempt",
        "producer_kind",
        "command",
        "tool",
        "source",
    ];
    if kind == "existing" {
        launch_fields.push("configuration");
    }
    json_io::exact(&launch, &launch_fields, "生产者启动收据")?;
    let owner = if kind.starts_with("identity-") {
        "identity_lock_owner"
    } else {
        "producer_owner"
    };
    let owner_required = kind != "existing";
    let mut process_fields = vec!["format_version", "kind", "launch", "identity"];
    if owner_required {
        process_fields.push(owner);
    }
    json_io::exact(&process, &process_fields, "生产者进程收据")?;
    if launch["format_version"] != 1
        || launch["kind"] != "devex-post-producer-launch"
        || process["format_version"] != 1
        || process["kind"] != "devex-post-producer"
        || launch["attempt"].as_u64() != Some(number)
        || command::stage(kind)? != stage
        || !command::accepts(kind, json_io::text(&attempt["mode"], "生产者模式")?)
        || launch["run_manifest"] != common::binding(&run_dir.join("manifest.json"))?
        || process["launch"] != common::binding(&launch_path)?
        || launch["registration"] != root["registration_binding"]
    {
        return Err("原生生产者不属于本 run 的明确阶段和固定登记".into());
    }
    if owner_required && !uuid(&process[owner]) {
        return Err("原生生产者收据缺少固定 owner".into());
    }
    let mut command_request = json!({
        "backend":backend,"run_dir":run_dir,"attempt":number,"kind":kind,"output":output,
        "tool":launch["tool"],"registration":root["registration"],"configuration":Value::Null
    });
    if kind == "existing" {
        command_request["configuration"] = json!(manifest::bound_file(
            &backend,
            &launch["configuration"],
            "已有数据配置",
            true
        )?);
    }
    let expected = command::build(&command_request)?;
    if launch["command"] != expected["argv"] || launch["source"] != expected["source"] {
        return Err("原生参数或工具不属于固定 run 和 attempt".into());
    }
    controller(&backend, &run_dir, number, attempt, &launch)?;
    let identity = common::validate_process_identity(&process["identity"])?;
    if identity["executable"] != launch["tool"]["path"] {
        return Err("原生内核 executable 与登记工具不同".into());
    }
    let pid = u32::try_from(identity["pid"].as_u64().ok_or("生产者 PID 无效")?)
        .map_err(|_| "生产者 PID 超出范围")?;
    let alive = match common::process_identity(pid)? {
        Some(current) if current == identity => {
            if actual_argv(&identity)? != values(&launch["command"])? {
                return Err("原生实际参数不属于固定 run 和 attempt".into());
            }
            true
        }
        Some(current) if newer(&identity, &current) => false,
        Some(_) => return Err("原生内核身份不能证明为已结束的原创建代次".into()),
        None => false,
    };
    Ok(
        json!({"receipt":common::binding(&process_path)?,"value":process,"controller":json_io::read(&state::controller_path(&run_dir,number))?,"alive":alive,"producer_kind":kind,"request":root["registration"]}),
    )
}

pub(super) fn quiet(request: &Value) -> Result<Value> {
    let root = json_io::exact(
        request,
        &["backend", "run_dir", "current_attempt", "registrations"],
        "生产者静默请求",
    )?;
    let run = path(&root["run_dir"], "运行目录")?;
    let backend = path(&root["backend"], "后端目录")?;
    let current = root["current_attempt"].as_u64();
    let state_value = state::load(&run, false)?;
    let attempts = state::attempts(&state_value)?;
    let registrations = root["registrations"]
        .as_object()
        .ok_or("生产者登记集合必须为对象")?;
    for attempt in attempts {
        let number = attempt["number"].as_u64().ok_or("生产者 attempt 无效")?;
        if current == Some(number)
            || !matches!(
                attempt["stage"].as_str(),
                Some("post-copy" | "seed-runtime")
            )
        {
            continue;
        }
        let supplied = registrations
            .get(&number.to_string())
            .ok_or("生产者静默核验缺少历史登记")?;
        let stage = json_io::text(&attempt["stage"], "生产者阶段")?;
        let launch_path = run
            .join(stage)
            .join(format!("attempt-{number:04}/session-launch.json"));
        let registration = if launch_path.exists() {
            let before = common::binding(&launch_path)?;
            let launch = common::read_bound(&backend, &before, "历史生产者启动收据")?;
            let descriptor = &launch["registration"];
            let request =
                registration::resolve(&backend, &run, stage, descriptor, Some(number), true)?;
            if common::binding(&launch_path)? != before {
                return Err("历史生产者启动收据在读取期间变化".into());
            }
            json!({"request":request,"binding":descriptor})
        } else {
            supplied.clone()
        };
        let observed = inspect(
            &json!({"backend":root["backend"],"run_dir":root["run_dir"],"attempt":attempt,"registration":registration["request"],"registration_binding":registration["binding"]}),
        )?;
        if observed.get("alive").and_then(Value::as_bool) == Some(true) {
            return Err("旧原生生产者仍在运行，先显式 recover-session，再核对未知意图".into());
        }
    }
    if state::load(&run, false)? != state_value {
        return Err("生产者历史在静默核验期间变化".into());
    }
    Ok(json!({"quiet":true}))
}

fn controller(
    backend: &Path,
    run: &Path,
    number: u64,
    attempt: &Value,
    launch: &Value,
) -> Result<()> {
    let path = manifest::bound_file(backend, &launch["controller"], "生产者控制器", true)?;
    let value = json_io::read(&path)?;
    let mut original = attempt.clone();
    original["status"] = json!("running");
    original["finished_at"] = Value::Null;
    original["result"] = Value::Null;
    original["error_type"] = Value::Null;
    if path != state::controller_path(run, number)
        || value["kind"] != "devex-stage-controller"
        || value["attempt"].as_u64() != Some(number)
        || value["attempt_sha256"] != digest::plan_hash(&original)?
        || value["owner"]["directory"] != json!(run)
        || value["owner"]["manifest_sha256"] != launch["run_manifest"]["sha256"]
    {
        return Err("原生生产者控制器不属于原始阶段".into());
    }
    Ok(())
}

fn values(value: &Value) -> Result<Vec<String>> {
    value
        .as_array()
        .ok_or("生产者 argv 必须为数组")?
        .iter()
        .map(|v| json_io::text(v, "生产者 argv").map(ToOwned::to_owned))
        .collect()
}

pub(super) fn require_actual_argv(identity: &Value, expected: &Value) -> Result<()> {
    if actual_argv(identity)? != values(expected)? {
        return Err("生产者实际参数不同于已授权启动命令".into());
    }
    Ok(())
}
pub(super) fn uuid(value: &Value) -> bool {
    value
        .as_str()
        .and_then(|v| uuid::Uuid::parse_str(v).ok())
        .is_some_and(|v| v.get_version_num() == 4)
}
pub(super) fn newer(old: &Value, current: &Value) -> bool {
    old["pid"] == current["pid"]
        && old["started"]
            .as_str()
            .and_then(|v| v.parse::<u128>().ok())
            .zip(
                current["started"]
                    .as_str()
                    .and_then(|v| v.parse::<u128>().ok()),
            )
            .is_some_and(|(a, b)| b > a)
}
fn path(value: &Value, label: &str) -> Result<PathBuf> {
    let value = PathBuf::from(json_io::text(value, label)?);
    if !value.is_absolute() {
        return Err(format!("{label}必须为绝对路径").into());
    }
    Ok(value)
}

#[cfg(unix)]
fn actual_argv(identity: &Value) -> Result<Vec<String>> {
    let identity = common::validate_process_identity(identity)?;
    let pid = u32::try_from(identity["pid"].as_u64().ok_or("生产者 PID 无效")?)
        .map_err(|_| "生产者 PID 超出范围")?;
    let bytes = std::fs::read(format!("/proc/{pid}/cmdline"))?;
    Ok(bytes
        .split(|b| *b == 0)
        .filter(|v| !v.is_empty())
        .map(|v| String::from_utf8(v.to_vec()))
        .collect::<std::result::Result<_, _>>()?)
}
#[cfg(windows)]
fn actual_argv(identity: &Value) -> Result<Vec<String>> {
    process::arguments(identity)
}
#[cfg(not(any(unix, windows)))]
fn actual_argv(_: &Value) -> Result<Vec<String>> {
    Err("当前平台不支持进程参数核验".into())
}
