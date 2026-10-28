//! 状态查询只观察，不创建控制锁、不回收进程、不重放操作。
use super::*;

pub(super) fn run(runtime: &mut dyn Runtime, context: &Context) -> Result<Value> {
    let history = history::validate(&context.run, &context.value["state"], false, false)?;
    let controller = controller(runtime, context)?;
    let unknown = context.unknown()?;
    let mut observations = Value::Null;
    let mut reconciliation = Value::Null;
    let mut partial_close_rejection = None;
    let next = if !unknown.is_empty() {
        "reconcile-evidence"
    } else if !controller.is_null() {
        check(
            controller["process_matches"] == true || controller["process_missing"] == true,
            "控制器 PID 已复用，不能等待或恢复无关进程",
        )?;
        if controller["process_missing"] == true {
            "recover"
        } else {
            "wait-controller"
        }
    } else if !history["unsettled"].as_array().is_some_and(Vec::is_empty) {
        if reconcilable(runtime, context, &history) {
            "reconcile"
        } else {
            "reconcile-evidence"
        }
    } else if history["initial_complete"] != true && history["closed"] == true {
        observations = partial::verify_closed(runtime, context)?;
        "new-scenario"
    } else if history["initial_complete"] != true {
        initial(runtime, context, &mut partial_close_rejection)?
    } else {
        let services = registered(runtime, context, true)?;
        observations = observe(runtime, &services)?;
        let mut next = "close";
        if !observations["termination"].is_null() {
            let reconciled = &history["external_recovery"];
            let mut proof = observations["termination"]
                .as_object()
                .ok_or("外部终止观察必须为对象")?
                .clone();
            proof.insert(
                "owner".into(),
                context.value["sources"]["state_before"].clone(),
            );
            proof.insert("receipt".into(), reconciled.clone());
            reconciliation = Value::Object(proof);
            next = if reconciled.is_null() {
                "recover"
            } else {
                "restart"
            };
            observations = json!(if reconciled.is_null() {
                "external-termination-unreconciled"
            } else {
                "external-termination-reconciled"
            });
        }
        if history["closed"] == true {
            check(
                observations == expected_stopped(),
                "已关闭夹具服务又出现存活进程",
            )?;
            next = "restart";
        }
        next
    };
    context.guard(None)?;
    let mut status = json!({"status":"service_status","run":context.run,"controller":controller,"state":context.value["sources"]["state_before"],
        "services":observations,"unsettled":history["unsettled"],"unknown_recoveries":unknown,"reconciliation":reconciliation,
        "next_operation":next,"remote_writes":0});
    if let Some(rejection) = partial_close_rejection {
        status["partial_close_rejection"] = rejection;
    }
    Ok(status)
}

fn initial(
    runtime: &mut dyn Runtime,
    context: &Context,
    rejection: &mut Option<Value>,
) -> Result<&'static str> {
    let next = initial_operation(context)?;
    if !matches!(next, "reconcile-evidence" | "buckets") {
        return Ok(next);
    }
    match partial::can_close(runtime, context) {
        Ok("running") if next == "buckets" => Ok("buckets"),
        Ok(_) => Ok("close"),
        Err(error) => {
            let reason = error.downcast_ref::<model::LifecycleError>().map_or(
                "部分关闭资格核验未完成，原始证据、文件或进程身份无法安全确认",
                |error| error.message,
            );
            *rejection = Some(
                json!({"category":model::category(error.as_ref()),"reason":reason,
                "advice":"核对原始首代账本、冻结请求、控制器、启动收据与完整进程树；保留现场，不重签绑定、不重放启动，不绕过状态执行关闭"}),
            );
            Ok("reconcile-evidence")
        }
    }
}

fn reconcilable(runtime: &mut dyn Runtime, context: &Context, history: &Value) -> bool {
    let Some(failed) = history::reconcilable(&context.value["state"], history, "close") else {
        return false;
    };
    let proof = (|| {
        let services = registered(runtime, context, false)?;
        history::evidence::close_evidence(
            &context.run,
            failed,
            &history["active_generation"],
            &context.sources(),
            &services,
        )
    })();
    proof.is_ok()
}

fn initial_operation(context: &Context) -> Result<&'static str> {
    let attempts = state::attempts(&context.value["state"])?;
    let count = attempts
        .iter()
        .position(|item| item["stage"] == "fixture-services")
        .unwrap_or(attempts.len());
    if count == 0
        || count != attempts.len()
        || !attempts[..count]
            .iter()
            .all(|attempt| attempt["status"] == "passed")
    {
        return Ok("reconcile-evidence");
    }
    let Some(operation) = ["rustfs", "redis", "buckets"].get(count).copied() else {
        return Ok("reconcile-evidence");
    };
    let exists = match std::fs::symlink_metadata(context.run.join(operation)) {
        Ok(_) => true,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
        Err(error) => return Err(error.into()),
    };
    Ok(if exists {
        "reconcile-evidence"
    } else {
        operation
    })
}
