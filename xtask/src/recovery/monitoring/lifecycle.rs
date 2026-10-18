use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

use chrono::{SecondsFormat, Utc};
use serde_json::{Map, Value, json};
use uuid::Uuid;

use crate::{
    Result,
    recovery::{
        clone::native::{json as json_io, resources::common},
        monitoring::{
            bind::verify_current,
            config,
            contract::{
                BASE_NAMES, ROLES, exact, file_descriptor as descriptor, process_identity,
                read_binding, read_document, ready_endpoints, sorted_ports, timestamp,
            },
            process,
            receipts::{read_process_trees, validate_close, validate_intent, validate_start},
            supervisor,
        },
    },
};

pub(crate) fn start(backend: &Path, binding_path: &Path) -> Result<Value> {
    let binding = read_binding(backend, binding_path)?;
    let inputs = super::execution::ProtectedInputs::binding(&binding.value)?;
    verify_current(backend, &binding.value)?;
    expect_root(&binding.run, BASE_NAMES.iter().copied().collect())?;
    binding.document.assert_unchanged()?;
    inputs.finish(start_runtime(&binding, &super::runtime::LocalRuntime))
}

pub(crate) fn start_runtime(
    binding: &crate::recovery::monitoring::contract::Binding,
    runtime: &impl super::runtime::Runtime,
) -> Result<Value> {
    let run = &binding.run;
    let operations = ROLES
        .iter()
        .map(|role| {
            (
                (*role).to_owned(),
                Value::String(Uuid::new_v4().simple().to_string()),
            )
        })
        .collect::<Map<_, _>>();
    let intent = json!({
        "format_version": 1,
        "kind": "restore-monitoring-start-intent",
        "binding": binding.document.descriptor(),
        "run_id": binding.value["run_id"],
        "scope_id": binding.value["scope_id"],
        "operations": operations,
        "requested_at": utc_now(),
    });
    let intent_path = run.join("start-intent.json");
    json_io::write_new(&intent_path, &intent)?;
    let mut stage = "validate-intent".to_owned();
    let mut processes = BTreeMap::new();
    let result = start_after_intent(
        binding,
        &intent_path,
        &operations,
        &mut stage,
        &mut processes,
        runtime,
    );
    match result {
        Ok(start) => Ok(start),
        Err(error) => {
            super::failure::finish(binding, &format!("start-{stage}"), error, Ok(processes))
        }
    }
}

fn start_after_intent(
    binding: &crate::recovery::monitoring::contract::Binding,
    intent_path: &Path,
    operations: &Map<String, Value>,
    stage: &mut String,
    processes: &mut BTreeMap<String, Value>,
    runtime: &impl super::runtime::Runtime,
) -> Result<Value> {
    let intent_document = read_document(intent_path, "监控启动意图")?;
    validate_intent(
        &intent_document.value,
        &binding.value,
        &binding.document.descriptor(),
    )?;
    *stage = "create-directories".into();
    let paths = config::create_runtime_directories(&binding.run)?;
    *stage = "write-configs".into();
    let configs = config::write_configs(&binding.value, &paths)?;
    let environment = config::safe_environment(&binding.run)?;
    *stage = "validate-configs".into();
    let validations = runtime.validate(&binding.value, &paths, &environment)?;
    *stage = "launch".into();
    let commands = runtime.commands(&binding.value, &paths)?;
    launch_commands(
        binding,
        &LaunchOptions {
            paths: &paths,
            environment: &environment,
            operations,
            commands: &commands,
        },
        stage,
        processes,
        runtime,
    )?;
    *stage = "finalize-start".into();
    finalize_start(
        binding,
        &paths,
        operations,
        processes,
        intent_document,
        configs,
        validations,
    )
}

struct LaunchOptions<'a> {
    paths: &'a super::runtime::Paths,
    environment: &'a super::runtime::Environment,
    operations: &'a Map<String, Value>,
    commands: &'a super::runtime::Commands,
}

fn launch_commands(
    binding: &crate::recovery::monitoring::contract::Binding,
    options: &LaunchOptions<'_>,
    stage: &mut String,
    processes: &mut BTreeMap<String, Value>,
    runtime: &impl super::runtime::Runtime,
) -> Result<()> {
    let LaunchOptions {
        paths,
        environment,
        operations,
        commands,
    } = options;
    let endpoints = ready_endpoints(&binding.value);
    for role in ROLES {
        *stage = match role {
            "webhook" => "launch-webhook",
            "alertmanager" => "launch-alertmanager",
            _ => "launch-prometheus",
        }
        .into();
        let operation = operations[role].as_str().ok_or("监控启动操作 ID 无效")?;
        let tree = launch_role(
            &binding.value,
            paths,
            environment,
            role,
            operation,
            &commands[role],
            runtime,
        )?;
        processes.insert(role.to_owned(), tree.clone());
        let url = endpoints[role].as_str().ok_or("监控就绪端点无效")?;
        runtime.ready(&tree, url)?;
    }
    Ok(())
}

fn finalize_start(
    binding: &crate::recovery::monitoring::contract::Binding,
    paths: &BTreeMap<String, PathBuf>,
    operations: &Map<String, Value>,
    processes: &BTreeMap<String, Value>,
    intent_document: crate::recovery::monitoring::contract::Document,
    configs: Value,
    validations: Value,
) -> Result<Value> {
    for process in processes.values() {
        let identity = process_identity(&process["process"], "监控产品进程")?.1;
        if common::process_identity(identity["pid"].as_u64().unwrap_or_default() as u32)?.as_ref()
            != Some(&identity)
        {
            return Err("监控进程在启动收据发布前退出或身份变化".into());
        }
    }
    let receipts = processes
        .iter()
        .map(|(role, tree)| {
            Ok((
                role.clone(),
                json!({
                    "operation_id": tree["operation_id"],
                    "tree": descriptor(&supervisor::tree_path_for(&paths["processes"], role))?,
                    "identity": tree["process"],
                }),
            ))
        })
        .collect::<Result<Map<_, _>>>()?;
    let start = json!({
        "format_version": 1,
        "kind": "restore-monitoring-start",
        "status": "running",
        "binding": binding.document.descriptor(),
        "intent": intent_document.descriptor(),
        "run_id": binding.value["run_id"],
        "scope_id": binding.value["scope_id"],
        "configs": configs,
        "validations": validations,
        "processes": receipts,
        "endpoints": ready_endpoints(&binding.value),
        "started_at": utc_now(),
    });
    validate_start(
        &start,
        &binding.value,
        &binding.document.descriptor(),
        &intent_document.descriptor(),
    )?;
    super::contract::validate_runtime_layout(&binding.run, operations, false)?;
    intent_document.assert_unchanged()?;
    json_io::write_new(&binding.run.join("start.json"), &start)?;
    Ok(start)
}

fn launch_role(
    binding: &Value,
    paths: &BTreeMap<String, PathBuf>,
    environment: &BTreeMap<String, String>,
    role: &str,
    operation: &str,
    command: &[String],
    runtime: &impl super::runtime::Runtime,
) -> Result<Value> {
    let protocol = supervisor::Protocol {
        runtime_directory: paths["processes"].clone(),
        role: role.to_owned(),
        scope_id: binding["scope_id"]
            .as_str()
            .ok_or("监控 scope 无效")?
            .to_owned(),
        operation_id: operation.to_owned(),
        command: command.to_vec(),
        cwd: paths["configs"].clone(),
        environment: environment.clone(),
        log: paths["processes"].join(format!("{role}.log")),
    };
    let invocation = runtime.invocation(&protocol)?;
    let (executable, args, env) = invocation;
    let mut process = Command::new(executable);
    process
        .args(args)
        .env_clear()
        .envs(env)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let mut supervisor_process = process.spawn()?;
    let tree_path = supervisor::tree_path_for(&paths["processes"], role);
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        if tree_path.exists() {
            break;
        }
        if let Some(status) = supervisor_process.try_wait()? {
            return Err(format!("监控 supervisor 在登记进程树前退出：{status}").into());
        }
        if Instant::now() >= deadline {
            return Err(format!("监控 supervisor 未在期限内登记 {role} 进程树").into());
        }
        thread::sleep(Duration::from_millis(25));
    }
    let tree = json_io::read(&tree_path)?;
    drop(supervisor_process);
    Ok(tree)
}

pub(crate) fn close(backend: &Path, binding_path: &Path) -> Result<Value> {
    let binding = read_binding(backend, binding_path)?;
    let inputs = super::execution::ProtectedInputs::binding(&binding.value)?;
    verify_current(backend, &binding.value)?;
    inputs.finish(close_runtime(&binding))
}

pub(crate) fn close_runtime(
    binding: &crate::recovery::monitoring::contract::Binding,
) -> Result<Value> {
    let run = &binding.run;
    let observed = super::contract::validate_root_shape(run)?;
    for forbidden in ["result.json", "close.json", "close-failure.json"] {
        if observed.contains(forbidden) {
            return Err(format!("监控运行已有 {forbidden}，拒绝重放关闭").into());
        }
    }
    let intent_document = read_document(&run.join("start-intent.json"), "监控启动意图")?;
    let intent = validate_intent(
        &intent_document.value,
        &binding.value,
        &binding.document.descriptor(),
    )?;
    let operations = intent["operations"]
        .as_object()
        .cloned()
        .ok_or("监控启动操作无效")?;
    let source_name = [
        "observation.json",
        "observation-failure.json",
        "start.json",
        "start-failure.json",
    ]
    .into_iter()
    .find(|name| observed.contains(*name))
    .unwrap_or("start-intent.json");
    let source_document = read_document(&run.join(source_name), "监控关闭来源")?;
    let close_intent = ensure_close_intent(binding, &source_document)?;
    let result = close_after_intent(
        binding,
        source_name,
        &source_document,
        &close_intent,
        &operations,
    );
    match result {
        Ok(close) => Ok(close),
        Err(error) => {
            record_failure(
                run,
                "close",
                &error.to_string(),
                Value::Object(Map::new()),
                false,
                binding,
            )?;
            Err(error)
        }
    }
}

fn close_after_intent(
    binding: &crate::recovery::monitoring::contract::Binding,
    source_name: &str,
    source_document: &crate::recovery::monitoring::contract::Document,
    close_intent: &crate::recovery::monitoring::contract::Document,
    operations: &Map<String, Value>,
) -> Result<Value> {
    let trees = read_process_trees(
        &binding.run,
        binding.value["scope_id"]
            .as_str()
            .ok_or("监控 scope 无效")?,
        operations,
    )?;
    if matches!(
        source_name,
        "start.json" | "observation.json" | "observation-failure.json"
    ) && trees.len() != ROLES.len()
    {
        return Err("已启动的监控运行缺少完整进程树，拒绝形成关闭成功收据".into());
    }
    let completions = process::stop_trees(&trees, false)?;
    config::verify_ports_closed(&binding.value)?;
    let close = json!({
        "format_version": 1,
        "kind": "restore-monitoring-close",
        "status": "closed",
        "binding": binding.document.descriptor(),
        "source": source_document.descriptor(),
        "close_intent": close_intent.descriptor(),
        "run_id": binding.value["run_id"],
        "scope_id": binding.value["scope_id"],
        "completions": completions,
        "ports_closed": sorted_ports(&binding.value),
        "closed_at": utc_now(),
    });
    validate_close(
        &close,
        &binding.value,
        &binding.document.descriptor(),
        &source_document.descriptor(),
        &close_intent.descriptor(),
        operations,
    )?;
    source_document.assert_unchanged()?;
    close_intent.assert_unchanged()?;
    json_io::write_new(&binding.run.join("close.json"), &close)?;
    Ok(close)
}

fn ensure_close_intent(
    binding: &crate::recovery::monitoring::contract::Binding,
    source: &crate::recovery::monitoring::contract::Document,
) -> Result<crate::recovery::monitoring::contract::Document> {
    let path = binding.run.join("close-intent.json");
    if path.exists() {
        let document = read_document(&path, "监控关闭意图")?;
        let object = exact(
            &document.value,
            &[
                "format_version",
                "kind",
                "binding",
                "source",
                "run_id",
                "scope_id",
                "requested_at",
            ],
            "监控关闭意图",
        )?;
        if object.get("format_version").and_then(Value::as_u64) != Some(1)
            || object.get("kind").and_then(Value::as_str) != Some("restore-monitoring-close-intent")
            || object.get("binding") != Some(&binding.document.descriptor())
            || object.get("source") != Some(&source.descriptor())
            || object.get("run_id") != binding.value.get("run_id")
            || object.get("scope_id") != binding.value.get("scope_id")
        {
            return Err("监控关闭意图与当前运行不同".into());
        }
        timestamp(object.get("requested_at"), "监控关闭请求时间")?;
        return Ok(document);
    }
    json_io::write_new(
        &path,
        &json!({
            "format_version": 1,
            "kind": "restore-monitoring-close-intent",
            "binding": binding.document.descriptor(),
            "source": source.descriptor(),
            "run_id": binding.value["run_id"],
            "scope_id": binding.value["scope_id"],
            "requested_at": utc_now(),
        }),
    )?;
    read_document(&path, "监控关闭意图")
}

pub(crate) fn assert_running(
    run: &Path,
    binding: &Value,
    start: &Value,
    operations: &Map<String, Value>,
) -> Result<()> {
    let scope = binding["scope_id"].as_str().ok_or("监控 scope 无效")?;
    let trees = crate::recovery::monitoring::receipts::process_trees(run, scope, operations)?;
    if trees.len() != ROLES.len() {
        return Err("监控运行缺少完整进程树".into());
    }
    for role in ROLES {
        let item = &start["processes"][role];
        if trees[role] != item["identity"]
            || common::process_identity(item["identity"]["pid"].as_u64().unwrap_or_default() as u32)?
                .as_ref()
                != Some(&item["identity"])
        {
            return Err("监控进程身份已经退出或变化".into());
        }
        config::require_listening(
            item["identity"]["pid"].as_u64().unwrap_or_default(),
            start["endpoints"][role]
                .as_str()
                .ok_or("监控就绪端点无效")?,
        )?;
    }
    Ok(())
}

pub(crate) fn record_failure(
    run: &Path,
    stage: &str,
    _error: &str,
    completions: Value,
    ports_closed: bool,
    binding: &crate::recovery::monitoring::contract::Binding,
) -> Result<()> {
    let kind_stage = if stage.starts_with("start-") {
        "start"
    } else {
        stage
    };
    let filename = if matches!(kind_stage, "start" | "observation" | "close") {
        format!("{kind_stage}-failure.json")
    } else {
        format!("{kind_stage}failure.json")
    };
    let path = run.join(filename);
    if path.exists() {
        return Ok(());
    }
    json_io::write_new(
        &path,
        &json!({
            "format_version": 1,
            "kind": format!("restore-monitoring-{kind_stage}-failure"),
            "status": "failed",
            "binding": binding.document.descriptor(),
            "run_id": binding.value["run_id"],
            "scope_id": binding.value["scope_id"],
            "stage": stage,
            "error_type": "MonitoringStageError",
            "completions": completions,
            "ports_closed": ports_closed,
            "failed_at": utc_now(),
            "external_contacts": 0,
        }),
    )
}

pub(crate) fn expect_root(run: &Path, expected: BTreeSet<&str>) -> Result<()> {
    let actual = super::contract::validate_root_shape(run)?;
    let expected = expected.into_iter().map(str::to_owned).collect();
    if actual != expected {
        return Err("监控 run 不处于当前操作要求的精确状态".into());
    }
    Ok(())
}

fn utc_now() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Micros, true)
}
