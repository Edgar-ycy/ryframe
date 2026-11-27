//! 只读取固定 attempt 的生产者历史，不查询运行中的 PID。
use crate::{
    Result,
    recovery::clone::native::{
        digest, json as json_io,
        post_process::{command, registration},
        resources::common,
    },
};
use serde_json::{Value, json};
use std::path::Path;
#[path = "producer/attempt.rs"]
mod attempt;
#[path = "producer/source.rs"]
mod source;
use source::{bound, identity};

pub(crate) fn collect(
    backend: &Path,
    run: &Path,
    registered: &Value,
    coordinator_tool: &Value,
) -> Result<Option<Value>> {
    context(backend, run, coordinator_tool)?;
    let attempt = attempt::validate(registered)?;
    let number = attempt["number"].as_u64().ok_or("生产者 attempt 无效")?;
    let stage = json_io::text(&attempt["stage"], "生产者阶段")?;
    if !attempt["result"].is_null() {
        bound(
            &run.join("results").join(format!("{number:04}.json")),
            run,
            Some(&attempt["result"]),
        )?;
    }
    let output = run.join(stage).join(format!("attempt-{number:04}"));
    let launch_path = output.join("session-launch.json");
    let process_path = output.join("session-process.json");
    let present = [
        source::present(&launch_path)?,
        source::present(&process_path)?,
    ];
    if !present[0] && !present[1] {
        for name in ["session.stdout.log", "session.stderr.log"] {
            if source::present(&output.join(name))? {
                return Err("Node 日志缺少明确启动收据".into());
            }
        }
        return Ok(None);
    }
    if !matches!(stage, "post-copy" | "seed-runtime") || !present.iter().all(|v| *v) {
        return Err("Node 生产者不属于明确阶段或缺少收据".into());
    }
    source::directory(&output, run)?;
    let launch_binding = bound(&launch_path, run, None)?;
    let receipt_binding = bound(&process_path, run, None)?;
    let (controller_binding, controller_identity) =
        attempt::controller(run, attempt, &registered["controller"])?;
    let launch = json_io::read(&launch_path)?;
    let process = json_io::read(&process_path)?;
    let manifest = bound(&run.join("manifest.json"), run, None)?;
    let kind = receipts(
        attempt,
        &launch,
        &process,
        &controller_binding,
        &manifest,
        &launch_binding,
    )?;
    let registration_path = source::binding_path(&launch["registration"])?;
    let registration_binding = bound(&registration_path, run, Some(&launch["registration"]))?;
    let tool_path = source::tool(&launch["tool"])?;
    source::script(attempt, backend, kind, &launch["source"])?;
    let configuration = if kind == "existing" {
        if controller_identity["executable"] != json!(tool_path) {
            return Err("已有数据生产者工具与原控制器可执行文件不同".into());
        }
        let path = output.join("existing-configuration.json");
        bound(&path, &output, Some(&launch["configuration"]))?;
        Some(path)
    } else {
        None
    };
    let request = registration::resolve(
        backend,
        run,
        stage,
        &launch["registration"],
        Some(number),
        true,
    )?;
    verify_command(
        backend,
        run,
        attempt,
        &output,
        &launch,
        &request,
        configuration.as_deref(),
    )?;
    let identity = identity(&process["identity"], Some(&tool_path))?;
    // 读取完成后重新核对绑定，不把半次读取登记为历史。
    for (path, binding) in [
        (&launch_path, &launch_binding),
        (&process_path, &receipt_binding),
        (&registration_path, &registration_binding),
    ] {
        if common::binding(path)? != *binding {
            return Err("生产者历史读取期间变化".into());
        }
    }
    if let Some(path) = configuration {
        bound(&path, &output, Some(&launch["configuration"]))?;
    }
    Ok(Some(
        json!({"attempt":number,"stage":stage,"mode":attempt["mode"],"producer_kind":kind,
        "receipt":receipt_binding,"launch":launch_binding,"controller":controller_binding,"registration":registration_binding,"identity":identity}),
    ))
}

fn receipts<'a>(
    attempt: &Value,
    launch: &'a Value,
    process: &Value,
    controller_binding: &Value,
    manifest: &Value,
    launch_binding: &Value,
) -> Result<&'a str> {
    let number = attempt["number"].as_u64().ok_or("生产者 attempt 无效")?;
    let stage = json_io::text(&attempt["stage"], "生产者阶段")?;
    let kind = json_io::text(&launch["producer_kind"], "生产者类型")?;
    let mut fields = vec![
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
        fields.push("configuration");
    }
    json_io::exact(launch, &fields, "生产者启动收据")?;
    attempt::owner(process, kind)?;
    if command::stage(kind)? != stage
        || !command::accepts(kind, json_io::text(&attempt["mode"], "生产者模式")?)
        || launch["format_version"] != 1
        || launch["kind"] != "devex-post-producer-launch"
        || launch["attempt"] != number
        || launch["controller"] != *controller_binding
        || launch["run_manifest"] != *manifest
        || process["format_version"] != 1
        || process["kind"] != "devex-post-producer"
        || process["launch"] != *launch_binding
    {
        return Err("原生生产者启动或进程收据不属于固定 attempt".into());
    }
    Ok(kind)
}

fn verify_command(
    backend: &Path,
    run: &Path,
    attempt: &Value,
    output: &Path,
    launch: &Value,
    request: &Value,
    configuration: Option<&Path>,
) -> Result<()> {
    let number = attempt["number"].as_u64().ok_or("生产者 attempt 无效")?;
    let kind = json_io::text(&launch["producer_kind"], "生产者类型")?;
    // session 使用收据中的工具，existing 已与原控制器绑定。
    if !matches!(kind, "session" | "existing") && launch["tool"] != request["identity_tool"] {
        return Err("生产者工具不同于固定登记".into());
    }
    let expected = command::build(
        &json!({"backend":backend,"run_dir":run,"attempt":number,"kind":kind,
        "output":output,"tool":launch["tool"],"registration":request,"configuration":configuration}),
    )?;
    if launch["command"] != expected["argv"]
        || expected["argv"].as_array().is_none_or(|items| {
            items
                .iter()
                .any(|v| v.as_str().is_none_or(|s| s.is_empty() || s.contains('\0')))
        })
    {
        return Err("生产者命令未绑定明确 run 和 attempt".into());
    }
    Ok(())
}

fn context(backend: &Path, run: &Path, coordinator_tool: &Value) -> Result<()> {
    if !backend.is_absolute()
        || !backend.is_dir()
        || digest::linked_path(backend)?.iter().any(|v| *v)
    {
        return Err("后端根目录必须是无链接的绝对目录".into());
    }
    source::directory(&backend.join(".local-tests"), backend)?;
    source::directory(run, &backend.join(".local-tests"))?;
    json_io::exact(coordinator_tool, &["path", "sha256"], "协调器工具")?;
    source::sha(&coordinator_tool["sha256"])?;
    if !Path::new(json_io::text(&coordinator_tool["path"], "协调器路径")?).is_absolute() {
        return Err("协调器路径必须为绝对路径".into());
    }
    Ok(())
}

#[cfg(test)]
#[path = "../../../../../../../tests/unit/recovery/clone/native/seed/source_registration/lineage/producer.rs"]
mod tests;
