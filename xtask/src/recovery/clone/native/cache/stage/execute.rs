//! 固定缓存阶段的执行与重复守卫；I/O 端口不决定动作顺序。
use super::super::super::{digest, json as doc, resources::common};
use super::super::rejected;
use super::super::{history, markers};
use super::{Context, number, path, successor};
use crate::Result;
use serde_json::{Value, json};

pub(in super::super) fn guard(context: &mut Context<'_>, p: &Value) -> Result<()> {
    doc::exact(
        p,
        &[
            "request",
            "descriptor",
            "controller",
            "mode",
            "number",
            "process",
            "tools",
            "successor",
            "private",
            "output",
            "runtime",
        ],
        "缓存执行守卫",
    )?;
    let request = &p["request"];
    let mode = doc::text(&p["mode"], "缓存动作")?;
    let number = p["number"].as_u64().ok_or("缓存序号无效")?;
    let checked = if p["tools"].is_null() {
        context.validate(request, &Value::Null)?
    } else {
        successor::plan(context, &p["successor"], &Value::Null, true)?.1
    };
    if context.active(mode, number)? != p["controller"]
        || context.registration()? != (request.clone(), p["descriptor"].clone())
        || checked != p["private"]
        || successor::generation(context, request, &path(&p["output"])?, &Value::Null, true)?
            != (p["process"].clone(), p["tools"].clone())
    {
        return Err(rejected("缓存操作期间控制器、固定请求或环境发生变化"));
    }
    context.call("producers-stopped", Value::Null)?;
    if !p["runtime"].is_null() {
        context.call(
            "observe",
            json!({"request":p["process"],"private":p["private"],"runtime":p["runtime"]}),
        )?;
    }
    Ok(())
}
fn prepare(context: &mut Context<'_>, p: &Value) -> Result<Value> {
    let controller = preflight(context, p)?;
    let mode = doc::text(&p["mode"], "缓存动作")?;
    let num = p["number"].as_u64().ok_or("缓存序号无效")?;
    let filename = if p["request_file"].is_null() {
        None
    } else {
        Some(path(&p["request_file"])?)
    };
    let successor = if let Some(file) = &filename {
        common::common_file(context.backend, file, "缓存阶段输入")?;
        if doc::read(file)?["kind"] == "devex-clone-seed-review-successor" {
            common::binding(file)?
        } else {
            Value::Null
        }
    } else if context
        .directory
        .join(super::STAGE)
        .join("registration.json")
        .exists()
    {
        let files = successor::tool_paths(context)?;
        if let Some(file) = files.last() {
            doc::read(file)?["successor"].clone()
        } else {
            Value::Null
        }
    } else {
        Value::Null
    };
    let (request, descriptor) = if mode == "restart" {
        if successor.is_null() {
            context.register(filename.as_deref())?
        } else {
            context.registration()?
        }
    } else {
        if filename.is_some() {
            return Err(rejected("继续、核对或清理不能替换固定缓存请求"));
        }
        context.registration()?
    };
    let output = context.output(num);
    common::ensure_new_directory(context.backend, &output, "缓存阶段目录")?;
    let (process, mut private, tools) = if !successor.is_null() {
        let (proof, private, _, process) =
            successor::plan(context, &successor, &Value::Null, true)?;
        doc::write_new(&output.join("tool-successor.json"), &proof)?;
        (
            process,
            private,
            common::binding(&output.join("tool-successor.json"))?,
        )
    } else {
        (request["process"].clone(), Value::Null, Value::Null)
    };
    if tools.is_null() && !matches!(mode, "stop" | "recover") {
        private = context.validate(&request, &Value::Null)?;
    }
    let guard_spec = json!({"request":request,"descriptor":descriptor,"controller":controller,"mode":mode,"number":num,"process":process,"tools":tools,"successor":successor,"private":private,"output":output,"runtime":null});
    Ok(guard_spec)
}
fn start(
    context: &mut Context<'_>,
    p: &Value,
    previous: &[history::Generation],
) -> Result<(Value, Value)> {
    let request = &p["request"];
    let descriptor = &p["descriptor"];
    let process = &p["process"];
    let private = &p["private"];
    let tools = &p["tools"];
    let controller = &p["controller"];
    let num = p["number"].as_u64().ok_or("缓存序号无效")?;
    let output = path(&p["output"])?;
    markers::require_owners(context, request, descriptor)?;
    if process["format_version"] == 3 {
        require_windows_predecessor_stopped(context, request)?;
    }
    let mut predecessors = Vec::new();
    for (attempt, earlier, runtime) in previous {
        let original = successor::generation(context, request, earlier, &Value::Null, true)?.0;
        let observed = if original == *process {
            context.call("status", json!({"request":process,"runtime":runtime}))?
        } else {
            context.call(
                "predecessor",
                json!({"request":original,"runtime":runtime,"effective":process}),
            )?
        };
        if observed["state"] != "stopped" {
            return Err(rejected("已有 Redis 代次仍存活，须显式精确回收"));
        }
        if !tools.is_null() {
            let file = output.join(format!("predecessor-{:04}.json", number(attempt)?));
            doc::write_new(
                &file,
                &json!({"process_sha256":digest::plan_hash(&original)?,"runtime_sha256":digest::plan_hash(runtime)?,"observation":observed}),
            )?;
            predecessors.push(common::binding(&file)?);
        }
    }
    let process_output = output.join("process");
    let mut started = json!({"request":descriptor,"controller":controller,"attempt":num,"process_request_sha256":digest::plan_hash(process)?,"output":process_output});
    if !tools.is_null() {
        started["tools"] = tools.clone();
        started["predecessors"] = json!(predecessors);
    }
    doc::write_new(&output.join("start.json"), &started)?;
    common::ensure_new_directory(context.backend, &process_output, "Redis 进程目录")?;
    let runtime = context.call(
        "start",
        json!({"request":process,"private":private,"output":process_output,"guard":p}),
    )?;
    doc::write_new(&output.join("runtime.json"), &runtime)?;
    Ok((runtime, common::binding(&output.join("runtime.json"))?))
}

fn require_windows_predecessor_stopped(context: &mut Context<'_>, request: &Value) -> Result<()> {
    #[cfg(not(windows))]
    {
        let _ = (context, request);
        Err(rejected("Windows 缓存前代只能在原 Windows 主机核验"))
    }
    #[cfg(windows)]
    {
        let descriptor = &request["process_receipt"];
        let path = common::bound_file(context.backend, descriptor, "Windows 缓存原运行收据")?;
        let runtime = common::read_bound(context.backend, descriptor, "Windows 缓存原运行收据")?;
        let original =
            super::super::windows::predecessor_request(context.backend, &path, &runtime)?;
        let observed = context.call("status", json!({"request":original,"runtime":runtime}))?;
        if observed["state"] != "stopped" {
            return Err(rejected("Windows 缓存原登记代次尚未完整停止"));
        }
        Ok(())
    }
}
fn observe(
    context: &mut Context<'_>,
    p: &Value,
    previous: &[history::Generation],
) -> Result<(Value, Value)> {
    let request = &p["request"];
    let process = &p["process"];
    let private = &p["private"];
    let (_, earlier, base) = previous
        .last()
        .ok_or("没有本运行已登记的 Redis 进程可供核对或继续")?;
    if base.get("state").is_some() {
        return Err(rejected(
            "该 Redis 启动已精确清理；使用显式 restart 创建新代次",
        ));
    }
    if successor::generation(context, request, earlier, &Value::Null, true)?.0 != *process {
        return Err(rejected("继续缓存必须使用同一已登记工具代次"));
    }
    let observed = context.call(
        "observe",
        json!({"request":process,"private":private,"runtime":base}),
    )?;
    if earlier.join("runtime.json").exists() {
        history::owned(context, earlier, base)
    } else {
        doc::write_new(&earlier.join("runtime.json"), &observed)?;
        Ok((observed, common::binding(&earlier.join("runtime.json"))?))
    }
}
pub(in super::super) fn run(context: &mut Context<'_>, p: &Value) -> Result<Value> {
    let mut guard_spec = prepare(context, p)?;
    let request = guard_spec["request"].clone();
    let descriptor = guard_spec["descriptor"].clone();
    let process = guard_spec["process"].clone();
    let tools = guard_spec["tools"].clone();
    let controller = guard_spec["controller"].clone();
    let num = guard_spec["number"].as_u64().ok_or("缓存序号无效")?;
    let output = path(&guard_spec["output"])?;
    let mode = guard_spec["mode"]
        .as_str()
        .ok_or("缓存动作无效")?
        .to_owned();
    if matches!(mode.as_str(), "stop" | "recover") {
        return history::observe_or_stop(context, &request, &descriptor, &output, &mode, num);
    }
    guard(context, &guard_spec)?;
    let previous = history::starts(context, &request, &descriptor, Some(num), &Value::Null)?;
    let (runtime, runtime_binding) = if mode == "restart" {
        start(context, &guard_spec, &previous)?
    } else {
        observe(context, &guard_spec, &previous)?
    };
    guard_spec["runtime"] = runtime.clone();
    guard(context, &guard_spec)?;
    let restore = json!({"request":request,"descriptor":descriptor,"runtime_binding":runtime_binding,"mode":mode,"number":num,"output":output});
    let owner = markers::restore(context, &restore, &guard_spec)?;
    guard(context, &guard_spec)?;
    let Some(owner) = owner else {
        return Ok(
            json!({"status":"cache_reconciled_before","request":descriptor,"runtime":runtime_binding,"copy_usable":false,"remote_writes":0,"resume_required":true}),
        );
    };
    history::publish(
        context,
        &json!({"request":request,"descriptor":descriptor,"runtime_binding":runtime_binding,"runtime":runtime,"process":process,"tools":tools,"controller":controller,"number":num,"owner":owner,"output":output}),
    )
}
pub(super) fn preflight(context: &Context<'_>, p: &Value) -> Result<Value> {
    doc::exact(
        p,
        &["value", "mode", "number", "request_file"],
        "缓存执行参数",
    )?;
    let mode = doc::text(&p["mode"], "缓存动作")?;
    if !matches!(
        mode,
        "restart" | "stop" | "recover" | "reconcile" | "resume"
    ) {
        return Err(rejected(
            "缓存动作仅支持 restart/stop/recover/reconcile/resume",
        ));
    }
    if p["value"] != context.manifest()? {
        return Err(rejected("缓存执行清单与固定运行不同"));
    }
    let num = p["number"]
        .as_u64()
        .filter(|n| *n > 0)
        .ok_or("缓存阶段序号无效")?;
    context.active(mode, num)
}
