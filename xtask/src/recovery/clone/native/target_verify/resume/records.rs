//! 持锁驱动器只提交观察与绑定，状态标志和收据形状由原生模型决定。
use super::*;

pub(super) fn execute(
    backend: &Path,
    target: &Path,
    transition: &str,
    args: &Value,
) -> Result<Value> {
    match transition {
        "inventory-intent" => inventory(backend, target, args, "intent"),
        "inventory-failure" => inventory(backend, target, args, "failure"),
        "inventory-confirmed" => inventory(backend, target, args, "confirmed"),
        "migration-intent" => migration_intent(backend, target, args),
        "migration-started" => started(backend, target, args),
        "migration-confirmed" => migration_confirmed(backend, target, args),
        "marker-intent" => marker_intent(backend, args),
        "marker-failure" | "marker-confirmed" => {
            marker_result(args, transition == "marker-confirmed")
        }
        _ => Err("目标续作状态请求无效".into()),
    }
}

fn marker_intent(backend: &Path, args: &Value) -> Result<Value> {
    let mut value = header(
        args,
        &[
            "resume",
            "started",
            "restart_generation",
            "request",
            "before",
        ],
        "devex-clone-redis-marker-rebind-intent",
    )?;
    let request = &args["request"];
    let selected = super::super::resume_model::selected(backend, request)?;
    let redis = &selected["redis"];
    value["markers"] = json!({"namespace":redis["namespace"],"ownership_key":redis["ownership_key"],"ownership_value":redis["ownership_value"],
        "sentinel_key":request["reset"]["sentinel_key"],"sentinel_value":request["reset"]["sentinel_value"]});
    value
        .as_object_mut()
        .ok_or("目标续作状态请求无效")?
        .remove("request");
    Ok(value)
}

fn marker_result(args: &Value, confirmed: bool) -> Result<Value> {
    let fields = if confirmed {
        &["intent", "restart_generation", "after", "atomic_result"][..]
    } else {
        &["intent", "restart_generation", "error_type"][..]
    };
    let mut value = header(
        args,
        fields,
        if confirmed {
            "devex-clone-redis-marker-rebind-confirmed"
        } else {
            "devex-clone-redis-marker-rebind-failure"
        },
    )?;
    value[if confirmed {
        "remote_write_operations"
    } else {
        "remote_write_operations_confirmed"
    }] = json!(u8::from(confirmed));
    value["unknown_result"] = json!(!confirmed);
    Ok(value)
}

fn header(args: &Value, fields: &[&str], kind: &str) -> Result<Value> {
    let mut expected = vec!["attempt", "at"];
    expected.extend_from_slice(fields);
    json_io::exact(args, &expected, "续作收据输入")?;
    attempt(&args["attempt"])?;
    timestamp(&args["at"])?;
    let mut value = args.clone();
    value["format_version"] = json!(1);
    value["kind"] = json!(kind);
    Ok(value)
}

fn inventory(backend: &Path, target: &Path, args: &Value, kind: &str) -> Result<Value> {
    let fields = match kind {
        "intent" => &["failure", "prepared_files"][..],
        "failure" => &["intent", "source_failure", "prepared_files", "error_type"][..],
        "confirmed" => &["intent", "failure", "prepared_files", "initialized"][..],
        _ => unreachable!(),
    };
    let mut value = header(
        args,
        fields,
        &format!("devex-clone-initialize-resume-{kind}"),
    )?;
    value["remote_writes"] = json!(0);
    if kind != "intent" {
        let attempt = attempt(&args["attempt"])?;
        check(
            bound(backend, &args["intent"])?
                == target.join(format!("resume-initialize-{attempt}.intent.json")),
            "目标续作收据状态与固定前像不同",
        )?;
    }
    Ok(value)
}

fn migration_intent(backend: &Path, target: &Path, args: &Value) -> Result<Value> {
    let mut value = header(
        args,
        &["request", "state", "protected_binaries"],
        "devex-clone-migration-resume-intent",
    )?;
    let state = &args["state"];
    check(
        state["resumable"] == true
            && matches!(
                state["mode"].as_str(),
                Some("migration" | "migration-successor")
            ),
        "目标续作状态请求无效",
    )?;
    let index = state["next_index"].as_u64().ok_or("目标续作状态请求无效")?;
    let operation = state["operations"]
        .get(index as usize)
        .ok_or("目标续作状态请求无效")?;
    check(
        operation["write"] == false
            && (state["mode"] != "migration-successor"
                || state["protected_binaries"] == args["protected_binaries"]),
        "目标续作状态转换不能越过未确认操作",
    )?;
    value
        .as_object_mut()
        .ok_or("目标续作状态请求无效")?
        .remove("state");
    value["failure"] = state["failure"].clone();
    value["prepare"] = binding(backend, &target.join("prepare.json"))?;
    for key in ["initialize_started", "reset", "creation", "prepared_files"] {
        value[key] = state[key].clone();
    }
    value["migration_prefix"] = state["completed"].clone();
    value["next_operation"] = operation["id"].clone();
    value["remote_write_operations_before_resume"] =
        json!(u8::from(state["mode"] == "migration-successor"));
    if state["mode"] == "migration-successor" {
        value["predecessor"] = state["predecessor"].clone();
    }
    Ok(value)
}

fn started(backend: &Path, target: &Path, args: &Value) -> Result<Value> {
    let mut value = header(
        args,
        &[
            "intent",
            "generation",
            "storage_runtime",
            "cache_runtime",
            "resources_before",
        ],
        "devex-clone-migration-resume-started",
    )?;
    let attempt = attempt(&args["attempt"])?;
    check(
        bound(backend, &args["intent"])?
            == target.join(format!("resume-initialize-{attempt}.intent.json")),
        "目标续作收据状态与固定前像不同",
    )?;
    value
        .as_object_mut()
        .ok_or("目标续作状态请求无效")?
        .remove("generation");
    value["current_generation_sha256"] = json!(digest::plan_hash(&args["generation"])?);
    value["storage"] = args["generation"]["storage"].clone();
    Ok(value)
}

fn migration_confirmed(backend: &Path, target: &Path, args: &Value) -> Result<Value> {
    let mut value = header(
        args,
        &[
            "intent",
            "started",
            "failure",
            "initialized",
            "prepared_files",
            "original_prefix",
            "resumed_operations",
            "redis_marker_rebind",
            "remote_write_operations",
            "predecessor",
        ],
        "devex-clone-migration-resume-confirmed",
    )?;
    let attempt = attempt(&args["attempt"])?;
    for (key, suffix) in [("intent", "intent"), ("started", "started")] {
        check(
            bound(backend, &args[key])?
                == target.join(format!("resume-initialize-{attempt}.{suffix}.json")),
            "目标续作收据状态与固定前像不同",
        )?;
    }
    check(
        bound(backend, &args["initialized"])? == target.join("initialized.json")
            && args["remote_write_operations"].as_u64().is_some(),
        "目标续作收据状态与固定前像不同",
    )?;
    value["restore_qualified"] = json!(false);
    if value["predecessor"].is_null() {
        value
            .as_object_mut()
            .ok_or("目标续作状态请求无效")?
            .remove("predecessor");
    }
    Ok(value)
}
