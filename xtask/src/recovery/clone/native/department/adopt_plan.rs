//! 接纳紧邻的已失败部门计划证据，不启动生产者或重放业务动作。
use std::path::Path;

use serde_json::{Map, Value, json};

use super::{adoption, common, document};
use crate::{
    Result,
    recovery::clone::native::{
        lifecycle::state,
        post_process::{inspect_historical, registration},
        seed::department_successor,
    },
};

#[allow(
    clippy::too_many_arguments,
    reason = "显式绑定同一阶段与两个调用方守卫"
)]
pub(in crate::recovery::clone::native) fn run(
    backend: &Path,
    run: &Path,
    output: &Path,
    number: u64,
    request: &Value,
    target: &Value,
    guard: &mut impl FnMut(Option<&Value>) -> Result<()>,
    administrator: &mut impl FnMut() -> Result<()>,
) -> Result<Option<Value>> {
    let saved = state::load(run, true)?;
    let [.., candidate, current] = state::attempts(&saved)? else {
        return Ok(None);
    };
    if candidate["stage"] != "seed-runtime"
        || candidate["mode"] != "departments-plan"
        || candidate["status"] != "failed"
    {
        return Ok(None);
    }
    if current["stage"] != "seed-runtime"
        || current["mode"] != "departments-plan"
        || current["status"] != "running"
        || current["number"].as_u64() != Some(number)
        || output != run.join(format!("seed-runtime/attempt-{number:04}"))
    {
        return Err("部门计划采用必须属于紧邻的当前重试 attempt".into());
    }
    guard(None)?;
    super::execute(
        backend,
        run,
        &json!({"operation":"identity-projection","allow_absent":true,
            "request":{"identity_plan":request["identity_plan"],"identity_state":request["identity_state"]}}),
    )?;
    administrator()?;
    let original = observe(backend, run, request, &saved, target)?;
    guard(Some(&original["prerequisite"]))?;
    if observe(backend, run, request, &saved, target)? != original {
        return Err("部门计划采用复核期间证据发生变化".into());
    }
    let mut evidence = original["evidence"].clone();
    evidence["guard"] = super::execute(
        backend,
        run,
        &json!({"operation":"publish","output":output,"target":target,"attempt":number}),
    )?;
    common::bound_file(backend, &evidence["guard"], "当前部门采用守卫")?;
    if state::load(run, true)? != saved {
        return Err("部门计划采用发布期间阶段历史发生变化".into());
    }
    Ok(Some(
        json!({"status":"seed_departments_planned","plan":evidence["plan"],
        "prerequisite":evidence["prerequisite"],"adopted":evidence,"remote_writes":0,
        "worker_authorized":false,"restore_qualified":false}),
    ))
}

fn observe(
    backend: &Path,
    run: &Path,
    request: &Value,
    saved: &Value,
    target: &Value,
) -> Result<Value> {
    if state::load(run, true)? != *saved {
        return Err("部门计划采用期间阶段历史发生变化".into());
    }
    let inputs = adoption::dispatch(backend, run, "adoption-inputs", &json!({"state":saved}))?;
    let mut processes = Map::new();
    for pid in document::array(&inputs["pids"], "部门采用控制器 PID")? {
        let pid = pid
            .as_u64()
            .and_then(|pid| u32::try_from(pid).ok())
            .filter(|pid| *pid > 0)
            .ok_or("部门采用控制器 PID 无效")?;
        let identity = common::process_identity(pid)?.unwrap_or(Value::Null);
        if processes
            .insert(pid.to_string(), identity.clone())
            .is_some_and(|old| old != identity)
        {
            return Err("部门计划采用期间控制器 PID 观察变化".into());
        }
    }
    let mut producers = Map::new();
    let [prior @ .., _] = document::array(&inputs["attempts"], "部门采用阶段")? else {
        return Err("部门采用缺少当前和原阶段".into());
    };
    for attempt in prior {
        let number = attempt["number"].as_u64().ok_or("历史部门阶段序号无效")?;
        let mut producer = inspect_historical(backend, run, attempt)?;
        original_request(backend, run, number, request, &mut producer)?;
        producers.insert(number.to_string(), producer);
    }
    let observed = adoption::dispatch(
        backend,
        run,
        "adoption-evidence",
        &json!({"request":request,"state":saved,"processes":processes,"producers":producers,"target":target}),
    )?;
    document::exact(&observed, &["evidence", "prerequisite"], "部门采用证据")?;
    Ok(observed)
}

fn original_request(
    backend: &Path,
    run: &Path,
    number: u64,
    request: &Value,
    producer: &mut Value,
) -> Result<()> {
    if producer.is_null() || producer["request"] == *request {
        return Ok(());
    }
    let launch = common::read_bound(backend, &producer["value"]["launch"], "部门采用的原启动")?;
    let descriptor = &launch["registration"];
    if common::bound_file(backend, descriptor, "部门采用的原生产者登记")?
        != department_successor::path(run)
    {
        return Ok(());
    }
    let projected =
        registration::resolve(backend, run, "seed-runtime", descriptor, Some(number), true)?;
    let mut expected = request.clone();
    expected["identity_tool"] = projected["identity_tool"].clone();
    if producer["producer_kind"] != "department-plan"
        || producer["request"] != projected
        || projected != expected
        || launch["tool"] != projected["identity_tool"]
    {
        return Err("部门采用不能改变原业务登记或接受其他生产者工具".into());
    }
    // 业务验证仍使用原 seed 登记，原进程、启动及后继工具的文件绑定原样保留。
    producer["request"] = request.clone();
    Ok(())
}
