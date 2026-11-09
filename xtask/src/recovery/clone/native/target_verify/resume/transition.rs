//! 续作收据转换统一由 Rust 建模；只返回候选值，由持锁控制器发布。
use super::*;

pub(in super::super) fn execute(
    backend: &Path,
    target: &Path,
    transition: &str,
    args: &Value,
) -> Result<Value> {
    match transition {
        "initialized" => initialized(backend, target, args),
        "operation-intent" => operation_intent(backend, target, args),
        "operation-confirmed" => operation_confirmed(backend, target, args),
        "migration-failure" => migration_failure(backend, target, args),
        "os-error" => os_error(args),
        "session-start"
        | "session-operation"
        | "session-completed"
        | "session-markers"
        | "session-markers-completed" => super::session::execute(backend, target, transition, args),
        _ => super::records::execute(backend, target, transition, args),
    }
}

fn os_error(args: &Value) -> Result<Value> {
    json_io::exact(
        args,
        &["errno", "winerror", "filename", "receipt_path"],
        "续作操作系统错误",
    )?;
    for key in ["errno", "winerror"] {
        check(
            args[key].is_null() || args[key].as_i64().is_some(),
            "目标续作状态请求无效",
        )?;
    }
    let normalized = if args["filename"].is_null() {
        None
    } else {
        let filename = json_io::text(&args["filename"], "续作错误文件")?;
        Some(
            if let Some(filename) = filename.strip_prefix("\\\\?\\UNC\\") {
                format!("\\\\{filename}")
            } else {
                filename
                    .strip_prefix("\\\\?\\")
                    .unwrap_or(filename)
                    .to_owned()
            },
        )
    };
    let role = normalized.as_ref().map(|filename| {
        if args["receipt_path"].as_str() == Some(filename.as_str()) {
            "command_receipt"
        } else {
            "other"
        }
    });
    Ok(
        json!({"errno":args["errno"],"winerror":args["winerror"],"filename_role":role,
        "filename_length":normalized.map(|filename| filename.chars().count())}),
    )
}

fn initialized(backend: &Path, target: &Path, args: &Value) -> Result<Value> {
    json_io::exact(
        args,
        &[
            "request",
            "generation",
            "reset",
            "inventory",
            "objects",
            "redis",
            "at",
        ],
        "初始化结果输入",
    )?;
    timestamp(&args["at"])?;
    let request = &args["request"];
    check(
        read(backend, &target.join("request.json"))? == *request,
        "目标续作收据状态与固定前像不同",
    )?;
    Ok(
        json!({"format_version":1,"status":"fresh_target_initialized","id":request["id"],"scope_id":request["target"]["scope_id"],"completed_at":args["at"],"generation":args["generation"],
        "prepare_sha256":binding(backend,&target.join("prepare.json"))?["sha256"],"reset":args["reset"],"inventory":args["inventory"],"objects":args["objects"],"redis":args["redis"],
        "history":super::super::proof::history(backend,target)?,"controlled_generation_never_started":true,"external_writers_discovered":false,"target_ready":false,"clone_verified":false,"restore_qualified":false}),
    )
}

fn operation_intent(backend: &Path, target: &Path, args: &Value) -> Result<Value> {
    json_io::exact(
        args,
        &["attempt", "index", "at", "resume", "failure", "operation"],
        "迁移操作 intent 输入",
    )?;
    let attempt = attempt(&args["attempt"])?;
    timestamp(&args["at"])?;
    let index = args["index"].as_u64().ok_or("目标续作状态请求无效")?;
    let resume = bound(backend, &args["resume"])?;
    check(
        resume == target.join(format!("resume-initialize-{attempt}.intent.json")),
        "目标续作收据状态与固定前像不同",
    )?;
    let operation = &args["operation"];
    let operations = super::super::resume_prefix::operations(
        json_io::text(&operation["command"][0], "迁移程序")?,
        &read(backend, &target.join("request.json"))?,
    )?;
    check(
        operations["operations"].get(index as usize) == Some(operation),
        "目标续作收据状态与固定前像不同",
    )?;
    Ok(
        json!({"format_version":1,"kind":"devex-clone-migration-operation-intent","attempt":attempt,"index":index,"at":args["at"],"resume":args["resume"],"failure":args["failure"],
        "operation":super::super::resume_model::operation(operation)}),
    )
}

fn operation_confirmed(backend: &Path, target: &Path, args: &Value) -> Result<Value> {
    json_io::exact(
        args,
        &["attempt", "index", "at", "intent", "receipt", "write"],
        "迁移操作确认输入",
    )?;
    let attempt = attempt(&args["attempt"])?;
    timestamp(&args["at"])?;
    let index = args["index"].as_u64().ok_or("目标续作状态请求无效")?;
    let intent_path = bound(backend, &args["intent"])?;
    let intent = read(backend, &intent_path)?;
    json_io::exact(
        &intent,
        super::super::resume_model::OP_INTENT,
        "续作操作 intent",
    )?;
    check(
        intent_path == target.join(format!("resume-migrate-{attempt}-{index:02}.intent.json"))
            && intent["attempt"] == attempt
            && intent["index"] == index
            && args["write"].is_boolean()
            && intent["operation"]["write"] == args["write"]
            && ordered(&[&intent["at"], &args["at"]]),
        "目标续作收据状态与固定前像不同",
    )?;
    let receipt_path = bound(backend, &args["receipt"])?;
    let operations = super::super::resume_prefix::operations(
        json_io::text(&intent["operation"]["command"][0], "迁移程序")?,
        &read(backend, &target.join("request.json"))?,
    )?;
    let operation = operations["operations"]
        .get(index as usize)
        .ok_or("目标续作状态请求无效")?;
    check(
        receipt_path.parent() == Some(target)
            && super::super::resume_model::operation(operation) == intent["operation"]
            && super::super::resume_prefix::operation_record(backend, target, operation)?
                == args["receipt"],
        "目标续作状态转换不能越过未确认操作",
    )?;
    Ok(
        json!({"format_version":1,"kind":"devex-clone-migration-operation-confirmed","attempt":attempt,"index":index,"at":args["at"],"intent":args["intent"],"receipt":args["receipt"],"write":args["write"]}),
    )
}

fn migration_failure(backend: &Path, target: &Path, args: &Value) -> Result<Value> {
    json_io::exact(
        args,
        &[
            "attempt",
            "at",
            "intent",
            "source_failure",
            "prepared_files",
            "started",
            "error_type",
            "confirmed_operations",
            "remote_write_operations_confirmed",
            "active_write",
            "active_operation",
            "os_error",
            "predecessor",
        ],
        "迁移失败输入",
    )?;
    let attempt = attempt(&args["attempt"])?;
    timestamp(&args["at"])?;
    check(
        bound(backend, &args["intent"])?
            == target.join(format!("resume-initialize-{attempt}.intent.json"))
            && args["remote_write_operations_confirmed"].as_u64().is_some()
            && args["confirmed_operations"].is_array()
            && args["error_type"]
                .as_str()
                .is_some_and(|error| !error.is_empty()),
        "目标续作状态请求无效",
    )?;
    let mut active = args["active_operation"].clone();
    if let Some(active) = active.as_object_mut() {
        active.retain(|key, _| !key.starts_with('_'));
    }
    let unknown = if !args["active_write"].is_null() {
        args["active_write"].clone()
    } else if active["write_started"] == true {
        json!("redis-marker-rebind")
    } else {
        Value::Null
    };
    let mut value = json!({"format_version":1,"kind":"devex-clone-migration-resume-failure","attempt":attempt,"at":args["at"],"intent":args["intent"],"source_failure":args["source_failure"],"prepared_files":args["prepared_files"],"started":args["started"],
        "error_type":args["error_type"],"confirmed_operations":args["confirmed_operations"],"remote_write_operations_confirmed":args["remote_write_operations_confirmed"],"unknown_write_operation":unknown,"phase":active.get("phase").cloned().unwrap_or(Value::Null),"active_operation":active,"os_error":args["os_error"]});
    if !args["predecessor"].is_null() {
        value["predecessor"] = args["predecessor"].clone();
    }
    Ok(value)
}
