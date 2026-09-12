"""隔离监控生命周期的目录形状与阶段收据验证。"""

from __future__ import annotations

from pathlib import Path
import re

from restore_monitoring_evidence import descriptor
from restore_monitoring_processes import CONFIG_NAMES, ROLES, VALIDATION_NAMES, paths, ready_endpoints
from restore_monitoring_publish import is_result_pending
from restore_monitoring_rules import ALERTS
from restore_monitoring_staging import DIRECTORY as STAGING_DIRECTORY, TOKEN_NAME
from restore_runtime_evidence import (
    HEX_64,
    artifact_snapshot,
    exact_fields,
    process_identity_record,
    read_json_document,
    reject_link_or_reparse,
    timestamp,
)

BASE_NAMES = {"binding.json", TOKEN_NAME, STAGING_DIRECTORY}
RUNTIME_DIRECTORIES = {"configs", "processes", "evidence", "prometheus-data", "alertmanager-data"}
RECEIPT_NAMES = {
    "start-intent.json", "start.json", "start-failure.json", "observation.json",
    "observation-failure.json", "close-intent.json", "close.json", "close-failure.json", "result.json",
}
OBSERVATION_FILES = {
    "api-metrics.txt", "worker-metrics.txt", "prometheus-targets.json",
    "prometheus-rules.json", "promtool-boundaries.log", "webhook-events.jsonl",
}
UUID_HEX = re.compile(r"[a-f0-9]{32}")


def document_descriptor(document, label: str) -> dict:
    current = descriptor(document.path)
    if current["sha256"] != document.sha256 or current["bytes"] != len(document.raw):
        raise ValueError(f"{label}在摘要采集期间变化")
    return current


def binding_descriptor(document) -> dict:
    return document_descriptor(document, "监控绑定")


def artifact(value: object, label: str, expected: Path | None = None) -> dict:
    value = exact_fields(value, {"path", "bytes", "sha256"}, label)
    if (
        not isinstance(value["path"], str) or not Path(value["path"]).is_absolute()
        or type(value["bytes"]) is not int or value["bytes"] < 0
        or not isinstance(value["sha256"], str) or HEX_64.fullmatch(value["sha256"]) is None
    ):
        raise ValueError(f"{label}描述无效")
    path = Path(value["path"])
    if expected is not None and path != expected:
        raise ValueError(f"{label}不属于当前监控 run")
    if artifact_snapshot(path).descriptor() != value:
        raise ValueError(f"{label}在发布后发生变化")
    return value


def validate_root_shape(run_directory: Path) -> set[str]:
    reject_link_or_reparse(run_directory)
    observed = {item.name for item in run_directory.iterdir()}
    if any(is_result_pending(name) for name in observed):
        raise ValueError("监控最终成功收据存在待核对 pending，发布状态不明")
    unknown = observed - BASE_NAMES - RUNTIME_DIRECTORIES - RECEIPT_NAMES
    if unknown or not BASE_NAMES.issubset(observed):
        raise ValueError("监控 run 包含未知写入或缺少固定绑定")
    for item in run_directory.iterdir():
        if item.name in RUNTIME_DIRECTORIES or item.name == STAGING_DIRECTORY:
            reject_link_or_reparse(item)
            if not item.is_dir():
                raise ValueError("监控 run 的登记写入路径不是普通目录")
        elif not item.is_file() or item.is_symlink():
            raise ValueError("监控 run 根成员不是普通文件")
    for success, failure in (
        ("start.json", "start-failure.json"),
        ("observation.json", "observation-failure.json"),
        ("close.json", "close-failure.json"),
    ):
        if {success, failure}.issubset(observed):
            raise ValueError("监控 run 同时存在阶段成功与失败收据")
    if observed & RUNTIME_DIRECTORIES and "start-intent.json" not in observed:
        raise ValueError("监控 run 写入目录缺少启动意图")
    if observed & {"close.json", "close-failure.json"} and "close-intent.json" not in observed:
        raise ValueError("监控 run 关闭收据缺少关闭意图")
    if "result.json" in observed and not {"start.json", "observation.json", "close.json"}.issubset(observed):
        raise ValueError("监控最终结果缺少成功阶段")
    return observed


def expect_root(run_directory: Path, expected: set[str]) -> None:
    if validate_root_shape(run_directory) != expected:
        raise ValueError("监控 run 不处于当前操作要求的精确状态")


def validate_runtime_layout(run_directory: Path, operations: dict[str, str], *, observed: bool) -> None:
    runtime = paths(run_directory)
    for name in RUNTIME_DIRECTORIES:
        path = runtime[name]
        reject_link_or_reparse(path)
        if not path.is_dir():
            raise ValueError("监控 run 缺少登记写入目录")
    config_names = {item.name for item in runtime["configs"].iterdir()}
    expected_configs = set(CONFIG_NAMES.values()) | set(VALIDATION_NAMES.values())
    if config_names != expected_configs or any(
        not item.is_file() or item.is_symlink() for item in runtime["configs"].iterdir()
    ):
        raise ValueError("监控配置目录不是精确固定集合")
    evidence_names = {item.name for item in runtime["evidence"].iterdir()}
    expected_evidence = OBSERVATION_FILES if observed else {"webhook-events.jsonl"}
    if evidence_names != expected_evidence or any(
        not item.is_file() or item.is_symlink() for item in runtime["evidence"].iterdir()
    ):
        raise ValueError("监控证据目录不是当前阶段的精确集合")
    from full_stack_process_tree import validate_process_tree_directory

    validate_process_tree_directory(
        runtime["processes"], operations, extra_files=tuple(f"{role}.log" for role in ROLES)
    )


def validate_intent(value: object, binding: dict, binding_receipt: dict) -> dict:
    value = exact_fields(
        value,
        {"format_version", "kind", "binding", "run_id", "scope_id", "operations", "requested_at"},
        "监控启动意图",
    )
    operations = exact_fields(value["operations"], set(ROLES), "监控启动操作")
    if (
        value["format_version"] != 1 or value["kind"] != "restore-monitoring-start-intent"
        or value["binding"] != binding_receipt or value["run_id"] != binding["run_id"]
        or value["scope_id"] != binding["scope_id"] or len(set(operations.values())) != len(ROLES)
        or any(not isinstance(item, str) or UUID_HEX.fullmatch(item) is None for item in operations.values())
    ):
        raise ValueError("监控启动意图与当前绑定不同")
    timestamp(value["requested_at"], "监控启动请求时间")
    return value


def validate_start(value: object, binding: dict, binding_receipt: dict, intent_receipt: dict) -> dict:
    value = exact_fields(
        value,
        {"format_version", "kind", "status", "binding", "intent", "run_id", "scope_id",
         "configs", "validations", "processes", "endpoints", "started_at"},
        "监控启动收据",
    )
    if (
        value["format_version"] != 1 or value["kind"] != "restore-monitoring-start"
        or value["status"] != "running" or value["binding"] != binding_receipt
        or value["intent"] != intent_receipt or value["run_id"] != binding["run_id"]
        or value["scope_id"] != binding["scope_id"] or value["endpoints"] != ready_endpoints(binding)
    ):
        raise ValueError("监控启动收据与当前绑定不同")
    timestamp(value["started_at"], "监控启动时间")
    run_directory, runtime = Path(binding_receipt["path"]).parent, paths(Path(binding_receipt["path"]).parent)
    configs = exact_fields(value["configs"], set(CONFIG_NAMES), "监控配置收据")
    validations = exact_fields(value["validations"], set(VALIDATION_NAMES), "监控配置校验收据")
    for name, receipt in configs.items():
        artifact(receipt, f"监控配置 {name}", runtime[name])
    for name, receipt in validations.items():
        artifact(receipt, f"监控校验 {name}", runtime["validate-" + name])
    intent_document = read_json_document(Path(intent_receipt["path"]))
    intent = validate_intent(intent_document.value, binding, binding_receipt)
    if document_descriptor(intent_document, "监控启动意图") != intent_receipt:
        raise ValueError("监控启动收据引用的意图已经变化")
    processes = exact_fields(value["processes"], set(ROLES), "监控进程收据")
    for role, item in processes.items():
        item = exact_fields(item, {"operation_id", "tree", "identity"}, f"监控进程 {role}")
        if item["operation_id"] != intent["operations"][role]:
            raise ValueError("监控进程不属于当前启动意图")
        artifact(item["tree"], f"监控进程树 {role}", run_directory / "processes" / f"{role}-tree.json")
        process_identity_record(item["identity"], f"监控进程 {role}")
    intent_document.assert_unchanged()
    return value


def validate_observation(value: object, binding: dict, binding_receipt: dict, start_receipt: dict) -> dict:
    value = exact_fields(
        value,
        {"format_version", "kind", "status", "binding", "start", "run_id", "scope_id",
         "metrics", "prometheus", "boundaries", "delivery", "external_contacts", "observed_at"},
        "监控观测收据",
    )
    if (
        value["format_version"] != 1 or value["kind"] != "restore-monitoring-observation"
        or value["status"] != "passed" or value["binding"] != binding_receipt
        or value["start"] != start_receipt or value["run_id"] != binding["run_id"]
        or value["scope_id"] != binding["scope_id"] or value["external_contacts"] != 0
    ):
        raise ValueError("监控观测收据与当前运行不同")
    evidence = Path(binding_receipt["path"]).parent / "evidence"
    metrics = exact_fields(value["metrics"], {"api", "worker"}, "监控指标证据")
    for role, item in metrics.items():
        item = exact_fields(
            item,
            {"artifact", "missing_status", "invalid_status", "accepted_status", "sample_count", "ryframe_sample_count"},
            f"{role} 指标证据",
        )
        if (
            item["missing_status"] != 401 or item["invalid_status"] != 401 or item["accepted_status"] != 200
            or type(item["sample_count"]) is not int or type(item["ryframe_sample_count"]) is not int
            or not 0 < item["ryframe_sample_count"] <= item["sample_count"]
        ):
            raise ValueError("真实指标或 Bearer 鉴权证据无效")
        artifact(item["artifact"], f"{role} 指标", evidence / f"{role}-metrics.txt")
    prometheus = exact_fields(value["prometheus"], {"targets", "targets_artifact", "alerts", "rules_artifact"}, "Prometheus 证据")
    if set(prometheus["targets"]) != {"ryframe-api", "ryframe-worker"} or set(prometheus["alerts"]) != ALERTS:
        raise ValueError("Prometheus target 或固定恢复告警证据不完整")
    artifact(prometheus["targets_artifact"], "Prometheus target", evidence / "prometheus-targets.json")
    artifact(prometheus["rules_artifact"], "Prometheus rules", evidence / "prometheus-rules.json")
    boundaries = exact_fields(value["boundaries"], {"passed", "cases", "artifact"}, "监控边界证据")
    if boundaries["passed"] is not True or boundaries["cases"] != ["23h", "23h30s", "24h30s"]:
        raise ValueError("23/24 小时边界证据不完整")
    artifact(boundaries["artifact"], "promtool 边界", evidence / "promtool-boundaries.log")
    delivery = exact_fields(value["delivery"], {"probe_id", "firing", "resolved", "receiver", "events"}, "告警投递证据")
    if (
        not isinstance(delivery["probe_id"], str) or UUID_HEX.fullmatch(delivery["probe_id"]) is None
        or delivery["firing"] is not True or delivery["resolved"] is not True
        or delivery["receiver"] != binding["endpoints"]["webhook"] + "/alerts"
    ):
        raise ValueError("Alertmanager firing/resolved 投递证据无效")
    artifact(delivery["events"], "隔离 webhook 事件", evidence / "webhook-events.jsonl")
    timestamp(value["observed_at"], "监控观测时间")
    return value


def validate_close(
    value: object,
    binding: dict,
    binding_receipt: dict,
    source: dict,
    close_intent: dict,
    operations: dict[str, str],
) -> dict:
    value = exact_fields(
        value,
        {"format_version", "kind", "status", "binding", "source", "close_intent", "run_id",
         "scope_id", "completions", "ports_closed", "closed_at"},
        "监控关闭收据",
    )
    if (
        value["format_version"] != 1 or value["kind"] != "restore-monitoring-close"
        or value["status"] != "closed" or value["binding"] != binding_receipt
        or value["source"] != source or value["close_intent"] != close_intent
        or value["run_id"] != binding["run_id"] or value["scope_id"] != binding["scope_id"]
        or value["ports_closed"] != sorted(binding["endpoints"])
    ):
        raise ValueError("监控关闭收据与当前运行不同")
    completions = value["completions"]
    if not isinstance(completions, dict) or not set(completions).issubset(ROLES):
        raise ValueError("监控完整进程树关闭证明包含未知角色")
    directory = Path(binding_receipt["path"]).parent / "processes"
    for role, item in completions.items():
        artifact(
            item,
            f"监控进程 {role} 关闭证明",
            directory / f"{role}-members-{operations[role]}-stopped.json",
        )
    timestamp(value["closed_at"], "监控关闭时间")
    return value


def validate_result(value: object, binding: dict, binding_receipt: dict) -> dict:
    value = exact_fields(
        value,
        {"format_version", "kind", "status", "binding", "start", "observation", "close",
         "run_id", "scope_id", "alerts", "targets", "firing_delivered", "resolved_delivered",
         "boundary_cases", "ports_closed", "external_contacts", "completed_at"},
        "监控最终成功收据",
    )
    if (
        value["format_version"] != 1 or value["kind"] != "restore-monitoring-result"
        or value["status"] != "passed" or value["binding"] != binding_receipt
        or value["run_id"] != binding["run_id"] or value["scope_id"] != binding["scope_id"]
        or value["alerts"] != sorted(ALERTS)
        or value["targets"] != ["ryframe-api", "ryframe-worker"]
        or value["firing_delivered"] is not True or value["resolved_delivered"] is not True
        or value["boundary_cases"] != ["23h", "23h30s", "24h30s"]
        or value["ports_closed"] != sorted(binding["endpoints"])
        or value["external_contacts"] != 0
    ):
        raise ValueError("监控最终成功收据与完整闭环证据不同")
    run_directory = Path(binding_receipt["path"]).parent
    for name in ("start", "observation", "close"):
        artifact(value[name], f"监控最终 {name} 证据", run_directory / f"{name}.json")
    timestamp(value["completed_at"], "监控最终完成时间")
    return value
