use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

use serde_json::{Map, Value};

use super::contract::{
    CONFIG_NAMES, ROLES, VALIDATION_NAMES, absolute_text, artifact, exact, process_identity,
    read_document, rule_alerts, sorted_ports, timestamp, valid_operation_id,
};
use crate::Result;

pub(crate) fn validate_intent(
    value: &Value,
    binding: &Value,
    binding_receipt: &Value,
) -> Result<Value> {
    let intent = exact(
        value,
        &[
            "format_version",
            "kind",
            "binding",
            "run_id",
            "scope_id",
            "operations",
            "requested_at",
        ],
        "监控启动意图",
    )?;
    let operations = exact(
        intent.get("operations").ok_or("监控启动意图缺少操作")?,
        &ROLES,
        "监控启动操作",
    )?;
    let mut operation_ids = BTreeSet::new();
    for value in operations.values() {
        let operation = value
            .as_str()
            .filter(|value| valid_operation_id(value))
            .ok_or("监控启动操作 ID 无效")?;
        operation_ids.insert(operation);
    }
    if intent.get("format_version").and_then(Value::as_u64) != Some(1)
        || intent.get("kind").and_then(Value::as_str) != Some("restore-monitoring-start-intent")
        || intent.get("binding") != Some(binding_receipt)
        || intent.get("run_id") != binding.get("run_id")
        || intent.get("scope_id") != binding.get("scope_id")
        || operation_ids.len() != ROLES.len()
    {
        return Err("监控启动意图与当前绑定不同".into());
    }
    timestamp(intent.get("requested_at"), "监控启动请求时间")?;
    Ok(value.clone())
}

pub(crate) fn validate_start(
    value: &Value,
    binding: &Value,
    binding_receipt: &Value,
    intent_receipt: &Value,
) -> Result<Value> {
    let start = exact(
        value,
        &[
            "format_version",
            "kind",
            "status",
            "binding",
            "intent",
            "run_id",
            "scope_id",
            "configs",
            "validations",
            "processes",
            "endpoints",
            "started_at",
        ],
        "监控启动收据",
    )?;
    if start.get("format_version").and_then(Value::as_u64) != Some(1)
        || start.get("kind").and_then(Value::as_str) != Some("restore-monitoring-start")
        || start.get("status").and_then(Value::as_str) != Some("running")
        || start.get("binding") != Some(binding_receipt)
        || start.get("intent") != Some(intent_receipt)
        || start.get("run_id") != binding.get("run_id")
        || start.get("scope_id") != binding.get("scope_id")
        || start.get("endpoints") != Some(&super::contract::ready_endpoints(binding))
    {
        return Err("监控启动收据与当前绑定不同".into());
    }
    timestamp(start.get("started_at"), "监控启动时间")?;
    let run = absolute_text(binding_receipt.get("path"), "监控绑定路径")?
        .parent()
        .ok_or("监控绑定缺少运行目录")?
        .to_path_buf();
    validate_start_artifacts(start, &run)?;
    let intent_path = absolute_text(intent_receipt.get("path"), "监控启动意图")?;
    let intent_document = read_document(&intent_path, "监控启动意图")?;
    let intent = validate_intent(&intent_document.value, binding, binding_receipt)?;
    if intent_document.descriptor() != *intent_receipt {
        return Err("监控启动收据引用的意图已经变化".into());
    }
    let processes = exact(
        start.get("processes").ok_or("监控启动收据缺少进程")?,
        &ROLES,
        "监控进程收据",
    )?;
    for role in ROLES {
        let item = processes.get(role).ok_or("监控启动收据缺少固定进程")?;
        let item = exact(
            item,
            &["operation_id", "tree", "identity"],
            &format!("监控进程 {role}"),
        )?;
        if item.get("operation_id") != intent["operations"].get(role) {
            return Err("监控进程不属于当前启动意图".into());
        }
        artifact(
            item.get("tree").ok_or("监控进程缺少树收据")?,
            &format!("监控进程树 {role}"),
            Some(&run.join("processes").join(format!("{role}-tree.json"))),
        )?;
        process_identity(
            item.get("identity").ok_or("监控进程缺少身份")?,
            &format!("监控进程 {role}"),
        )?;
    }
    intent_document.assert_unchanged()?;
    Ok(value.clone())
}

pub(crate) fn validate_observation(
    value: &Value,
    binding: &Value,
    binding_receipt: &Value,
    start_receipt: &Value,
) -> Result<Value> {
    let observation = exact(
        value,
        &[
            "format_version",
            "kind",
            "status",
            "binding",
            "start",
            "run_id",
            "scope_id",
            "metrics",
            "prometheus",
            "boundaries",
            "delivery",
            "external_contacts",
            "observed_at",
        ],
        "监控观测收据",
    )?;
    if observation.get("format_version").and_then(Value::as_u64) != Some(1)
        || observation.get("kind").and_then(Value::as_str) != Some("restore-monitoring-observation")
        || observation.get("status").and_then(Value::as_str) != Some("passed")
        || observation.get("binding") != Some(binding_receipt)
        || observation.get("start") != Some(start_receipt)
        || observation.get("run_id") != binding.get("run_id")
        || observation.get("scope_id") != binding.get("scope_id")
        || observation.get("external_contacts").and_then(Value::as_u64) != Some(0)
    {
        return Err("监控观测收据与当前运行不同".into());
    }
    let run = absolute_text(binding_receipt.get("path"), "监控绑定路径")?
        .parent()
        .ok_or("监控绑定缺少运行目录")?
        .to_path_buf();
    let evidence = run.join("evidence");
    let metrics = exact(
        observation.get("metrics").ok_or("监控观测缺少指标")?,
        &["api", "worker"],
        "监控指标证据",
    )?;
    for role in ["api", "worker"] {
        let item = exact(
            metrics.get(role).ok_or("监控指标缺少固定角色")?,
            &[
                "artifact",
                "missing_status",
                "invalid_status",
                "accepted_status",
                "sample_count",
                "ryframe_sample_count",
            ],
            &format!("{role} 指标证据"),
        )?;
        let accepted = item
            .get("accepted_status")
            .and_then(Value::as_u64)
            .ok_or("指标 accepted 状态无效")?;
        let samples = item
            .get("sample_count")
            .and_then(Value::as_u64)
            .ok_or("指标样本数无效")?;
        let ryframe = item
            .get("ryframe_sample_count")
            .and_then(Value::as_u64)
            .ok_or("指标 RyFrame 样本数无效")?;
        if item.get("missing_status").and_then(Value::as_u64) != Some(401)
            || item.get("invalid_status").and_then(Value::as_u64) != Some(401)
            || accepted != 200
            || ryframe == 0
            || ryframe > samples
        {
            return Err("真实指标或 Bearer 鉴权证据无效".into());
        }
        artifact(
            item.get("artifact").ok_or("指标缺少证据文件")?,
            &format!("{role} 指标"),
            Some(&evidence.join(format!("{role}-metrics.txt"))),
        )?;
    }
    let prometheus = exact(
        observation
            .get("prometheus")
            .ok_or("监控观测缺少 Prometheus 证据")?,
        &["targets", "targets_artifact", "alerts", "rules_artifact"],
        "Prometheus 证据",
    )?;
    let targets = string_set(prometheus.get("targets"), "Prometheus target")?;
    let expected_targets = BTreeSet::from(["ryframe-api".to_owned(), "ryframe-worker".to_owned()]);
    let alerts = string_set(prometheus.get("alerts"), "Prometheus 告警")?;
    if targets != expected_targets || alerts != rule_alerts(binding)? {
        return Err("Prometheus target 或固定恢复告警证据不完整".into());
    }
    artifact(
        prometheus
            .get("targets_artifact")
            .ok_or("Prometheus target 缺少证据")?,
        "Prometheus target",
        Some(&evidence.join("prometheus-targets.json")),
    )?;
    artifact(
        prometheus
            .get("rules_artifact")
            .ok_or("Prometheus rules 缺少证据")?,
        "Prometheus rules",
        Some(&evidence.join("prometheus-rules.json")),
    )?;
    let boundaries = exact(
        observation
            .get("boundaries")
            .ok_or("监控观测缺少边界证据")?,
        &["passed", "cases", "artifact"],
        "监控边界证据",
    )?;
    if boundaries.get("passed").and_then(Value::as_bool) != Some(true)
        || !string_array_equals(boundaries.get("cases"), &["23h", "23h30s", "24h30s"])
    {
        return Err("23/24 小时边界证据不完整".into());
    }
    artifact(
        boundaries.get("artifact").ok_or("监控边界缺少证据文件")?,
        "promtool 边界",
        Some(&evidence.join("promtool-boundaries.log")),
    )?;
    validate_delivery(observation, binding, &evidence)?;
    timestamp(observation.get("observed_at"), "监控观测时间")?;
    Ok(value.clone())
}

fn validate_delivery(
    observation: &Map<String, Value>,
    binding: &Value,
    evidence: &Path,
) -> Result<()> {
    let delivery = exact(
        observation.get("delivery").ok_or("监控观测缺少投递证据")?,
        &["probe_id", "firing", "resolved", "receiver", "events"],
        "告警投递证据",
    )?;
    let probe = delivery
        .get("probe_id")
        .and_then(Value::as_str)
        .ok_or("告警投递 probe 无效")?;
    let webhook = binding["endpoints"]["webhook"]
        .as_str()
        .ok_or("监控绑定缺少 webhook 端点")?;
    if !valid_operation_id(probe)
        || delivery.get("firing").and_then(Value::as_bool) != Some(true)
        || delivery.get("resolved").and_then(Value::as_bool) != Some(true)
        || delivery.get("receiver").and_then(Value::as_str) != Some(&format!("{webhook}/alerts"))
    {
        return Err("Alertmanager firing/resolved 投递证据无效".into());
    }
    artifact(
        delivery.get("events").ok_or("告警投递缺少事件文件")?,
        "隔离 webhook 事件",
        Some(&evidence.join("webhook-events.jsonl")),
    )?;
    Ok(())
}

pub(crate) fn validate_close(
    value: &Value,
    binding: &Value,
    binding_receipt: &Value,
    source: &Value,
    close_intent: &Value,
    operations: &Map<String, Value>,
) -> Result<Value> {
    let close = exact(
        value,
        &[
            "format_version",
            "kind",
            "status",
            "binding",
            "source",
            "close_intent",
            "run_id",
            "scope_id",
            "completions",
            "ports_closed",
            "closed_at",
        ],
        "监控关闭收据",
    )?;
    if close.get("format_version").and_then(Value::as_u64) != Some(1)
        || close.get("kind").and_then(Value::as_str) != Some("restore-monitoring-close")
        || close.get("status").and_then(Value::as_str) != Some("closed")
        || close.get("binding") != Some(binding_receipt)
        || close.get("source") != Some(source)
        || close.get("close_intent") != Some(close_intent)
        || close.get("run_id") != binding.get("run_id")
        || close.get("scope_id") != binding.get("scope_id")
        || close.get("ports_closed").and_then(Value::as_array)
            != Some(
                &sorted_ports(binding)
                    .into_iter()
                    .map(Value::from)
                    .collect::<Vec<_>>(),
            )
    {
        return Err("监控关闭收据与当前运行不同".into());
    }
    let completions = close
        .get("completions")
        .and_then(Value::as_object)
        .ok_or("监控完整进程树关闭证明无效")?;
    let binding_path = absolute_text(binding_receipt.get("path"), "监控绑定路径")?;
    let run = binding_path.parent().ok_or("监控绑定缺少运行目录")?;
    for (role, item) in completions {
        if !ROLES.contains(&role.as_str()) {
            return Err("监控完整进程树关闭证明包含未知角色".into());
        }
        let operation = operations
            .get(role)
            .and_then(Value::as_str)
            .ok_or("监控关闭证明缺少操作 ID")?;
        artifact(
            item,
            &format!("监控进程 {role} 关闭证明"),
            Some(
                &run.join("processes")
                    .join(format!("{role}-members-{operation}-stopped.json")),
            ),
        )?;
    }
    timestamp(close.get("closed_at"), "监控关闭时间")?;
    Ok(value.clone())
}

pub(crate) fn validate_result(
    value: &Value,
    binding: &Value,
    binding_receipt: &Value,
) -> Result<Value> {
    let result = exact(
        value,
        &[
            "format_version",
            "kind",
            "status",
            "binding",
            "start",
            "observation",
            "close",
            "run_id",
            "scope_id",
            "alerts",
            "targets",
            "firing_delivered",
            "resolved_delivered",
            "boundary_cases",
            "ports_closed",
            "external_contacts",
            "completed_at",
        ],
        "监控最终成功收据",
    )?;
    let alerts = result
        .get("alerts")
        .and_then(Value::as_array)
        .ok_or("监控最终告警列表无效")?;
    let expected_alerts = rule_alerts(binding)?
        .into_iter()
        .map(Value::from)
        .collect::<Vec<_>>();
    if result.get("format_version").and_then(Value::as_u64) != Some(1)
        || result.get("kind").and_then(Value::as_str) != Some("restore-monitoring-result")
        || result.get("status").and_then(Value::as_str) != Some("passed")
        || result.get("binding") != Some(binding_receipt)
        || result.get("run_id") != binding.get("run_id")
        || result.get("scope_id") != binding.get("scope_id")
        || alerts != &expected_alerts
        || !string_array_equals(result.get("targets"), &["ryframe-api", "ryframe-worker"])
        || result.get("firing_delivered").and_then(Value::as_bool) != Some(true)
        || result.get("resolved_delivered").and_then(Value::as_bool) != Some(true)
        || !string_array_equals(result.get("boundary_cases"), &["23h", "23h30s", "24h30s"])
        || result.get("ports_closed").and_then(Value::as_array)
            != Some(
                &sorted_ports(binding)
                    .into_iter()
                    .map(Value::from)
                    .collect::<Vec<_>>(),
            )
        || result.get("external_contacts").and_then(Value::as_u64) != Some(0)
    {
        return Err("监控最终成功收据与完整闭环证据不同".into());
    }
    let binding_path = absolute_text(binding_receipt.get("path"), "监控绑定路径")?;
    let run = binding_path.parent().ok_or("监控绑定缺少运行目录")?;
    for name in ["start", "observation", "close"] {
        artifact(
            result.get(name).ok_or("监控最终收据缺少阶段证据")?,
            &format!("监控最终 {name} 证据"),
            Some(&run.join(format!("{name}.json"))),
        )?;
    }
    timestamp(result.get("completed_at"), "监控最终完成时间")?;
    Ok(value.clone())
}

pub(crate) fn read_start(
    run: &Path,
    binding: &Value,
    binding_receipt: &Value,
) -> Result<(super::contract::Document, Value, Value, Map<String, Value>)> {
    let intent_document = read_document(&run.join("start-intent.json"), "监控启动意图")?;
    let intent_receipt = intent_document.descriptor();
    let intent = validate_intent(&intent_document.value, binding, binding_receipt)?;
    let start_document = read_document(&run.join("start.json"), "监控启动收据")?;
    let start = validate_start(
        &start_document.value,
        binding,
        binding_receipt,
        &intent_receipt,
    )?;
    start_document.assert_unchanged()?;
    intent_document.assert_unchanged()?;
    let operations = intent
        .get("operations")
        .and_then(Value::as_object)
        .cloned()
        .ok_or("监控启动操作无效")?;
    Ok((start_document, start, intent, operations))
}

fn validate_start_artifacts(start: &Map<String, Value>, run: &Path) -> Result<()> {
    let configs = exact(
        start.get("configs").ok_or("监控启动收据缺少配置")?,
        &CONFIG_NAMES.map(|(name, _)| name),
        "监控配置收据",
    )?;
    for (name, filename) in CONFIG_NAMES {
        artifact(
            configs.get(name).ok_or("监控配置收据缺少固定项")?,
            &format!("监控配置 {name}"),
            Some(&run.join("configs").join(filename)),
        )?;
    }
    let validations = exact(
        start.get("validations").ok_or("监控启动收据缺少配置校验")?,
        &VALIDATION_NAMES.map(|(name, _)| name),
        "监控配置校验收据",
    )?;
    for (name, filename) in VALIDATION_NAMES {
        artifact(
            validations.get(name).ok_or("监控配置校验收据缺少固定项")?,
            &format!("监控校验 {name}"),
            Some(&run.join("configs").join(filename)),
        )?;
    }
    Ok(())
}

fn string_set(value: Option<&Value>, label: &str) -> Result<BTreeSet<String>> {
    value
        .and_then(Value::as_array)
        .ok_or_else(|| format!("{label}必须是数组"))?
        .iter()
        .map(|value| {
            value
                .as_str()
                .map(ToOwned::to_owned)
                .ok_or_else(|| format!("{label}项必须是字符串").into())
        })
        .collect()
}

fn string_array_equals(value: Option<&Value>, expected: &[&str]) -> bool {
    value.and_then(Value::as_array).is_some_and(|values| {
        values.len() == expected.len()
            && values
                .iter()
                .zip(expected)
                .all(|(value, expected)| value.as_str() == Some(*expected))
    })
}

pub(crate) fn process_trees(
    run: &Path,
    scope: &str,
    operations: &Map<String, Value>,
) -> Result<BTreeMap<String, Value>> {
    Ok(read_process_trees(run, scope, operations)?
        .into_iter()
        .map(|(role, receipt)| (role, receipt.get("process").cloned().unwrap_or(Value::Null)))
        .collect())
}

pub(crate) fn read_process_trees(
    run: &Path,
    scope: &str,
    operations: &Map<String, Value>,
) -> Result<BTreeMap<String, Value>> {
    let mut result = BTreeMap::new();
    for role in ROLES {
        let path = run.join("processes").join(format!("{role}-tree.json"));
        let document = read_document(&path, &format!("监控进程树 {role}"))?;
        let receipt = exact(
            &document.value,
            &[
                "format_version",
                "kind",
                "runtime_directory",
                "role",
                "scope_id",
                "operation_id",
                "supervisor",
                "process",
                "group_id",
            ],
            &format!("监控进程树 {role}"),
        )?;
        if receipt.get("format_version").and_then(Value::as_u64) != Some(3)
            || receipt.get("kind").and_then(Value::as_str)
                != Some("restore-monitoring-process-tree")
            || receipt.get("role").and_then(Value::as_str) != Some(role)
            || receipt.get("scope_id").and_then(Value::as_str) != Some(scope)
            || receipt.get("operation_id") != operations.get(role)
            || receipt.get("runtime_directory").and_then(Value::as_str)
                != Some(&run.join("processes").to_string_lossy())
            || receipt.get("group_id") != receipt.get("process").and_then(|value| value.get("pid"))
        {
            return Err("监控进程树收据与角色、scope 或操作不匹配".into());
        }
        process_identity(
            receipt
                .get("supervisor")
                .ok_or("监控进程树缺少 supervisor")?,
            &format!("监控进程监督器 {role}"),
        )?;
        let process = receipt.get("process").ok_or("监控进程树缺少产品进程")?;
        process_identity(process, &format!("监控进程 {role}"))?;
        result.insert(role.to_owned(), document.value);
    }
    Ok(result)
}
