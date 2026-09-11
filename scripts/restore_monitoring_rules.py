"""正式恢复监控验收使用的固定告警规则合同。"""

from __future__ import annotations

import hashlib
from pathlib import Path
import subprocess

import yaml

from restore_runtime_evidence import artifact_snapshot, exact_fields
from source_inventory import git


RULE_PATHS = {
    "alerts": "deploy/prometheus/ryframe-alerts.yml",
    "tests": "deploy/prometheus/ryframe-alerts.test.yml",
}
ALERTS = frozenset(
    {
        "RyFrameBackupAging",
        "RyFrameBackupStale",
        "RyFrameBackupMissing",
        "RyFrameBackupExpired",
        "RyFrameBackupInvalid",
        "RyFrameBackupMetricMissing",
        "RyFrameRestoreFailed",
        "RyFrameRestoreOverdue",
    }
)


def expected_paths(backend: Path) -> dict[str, str]:
    return {name: str((backend / relative).absolute()) for name, relative in RULE_PATHS.items()}


def _tracked_head(backend: Path, relative: str, raw: bytes, label: str) -> None:
    try:
        tracked = git(backend, "ls-files", "--error-unmatch", "--", relative)
        committed = git(backend, "show", f"HEAD:{relative}")
    except subprocess.CalledProcessError as error:
        raise ValueError(f"{label}不是协调器 HEAD 跟踪的正式文件") from error
    try:
        names = tracked.decode("utf-8", errors="strict").splitlines()
    except UnicodeDecodeError as error:
        raise ValueError(f"{label}的 Git 跟踪路径不是 UTF-8") from error
    if names != [relative] or committed != raw:
        raise ValueError(f"{label}与协调器 HEAD 跟踪内容不同")


def _read_yaml(backend: Path, relative: str, label: str) -> tuple[dict, object]:
    path = backend / relative
    snapshot = artifact_snapshot(path)
    if snapshot.bytes > 1024 * 1024:
        raise ValueError(f"{label}超过 1 MiB")
    try:
        raw = path.read_bytes()
        value = yaml.safe_load(raw.decode("utf-8", errors="strict"))
    except (OSError, UnicodeError, yaml.YAMLError) as error:
        raise ValueError(f"{label}不是有效 UTF-8 YAML") from error
    if len(raw) != snapshot.bytes or hashlib.sha256(raw).hexdigest() != snapshot.sha256:
        raise ValueError(f"{label}在读取期间发生变化")
    _tracked_head(backend, relative, raw, label)
    snapshot.assert_unchanged()
    if not isinstance(value, dict):
        raise ValueError(f"{label}必须是 YAML 映射")
    return value, snapshot


def _defined_alerts(value: dict) -> list[str]:
    groups = value.get("groups")
    if not isinstance(groups, list):
        raise ValueError("监控规则缺少 groups")
    result = []
    for group in groups:
        if not isinstance(group, dict) or not isinstance(group.get("rules"), list):
            raise ValueError("监控规则组结构无效")
        for rule in group["rules"]:
            if isinstance(rule, dict) and "alert" in rule:
                if not isinstance(rule["alert"], str):
                    raise ValueError("监控告警名称无效")
                result.append(rule["alert"])
    return result


def _test_alerts(value: dict) -> tuple[set[str], set[tuple[str, str, bool]]]:
    if value.get("rule_files") != ["ryframe-alerts.yml"] or not isinstance(value.get("tests"), list):
        raise ValueError("监控规则测试没有绑定唯一正式规则文件")
    names: set[str] = set()
    boundary: set[tuple[str, str, bool]] = set()
    for case in value["tests"]:
        if not isinstance(case, dict) or not isinstance(case.get("alert_rule_test"), list):
            raise ValueError("监控规则测试场景无效")
        for assertion in case["alert_rule_test"]:
            if not isinstance(assertion, dict):
                raise ValueError("监控规则测试断言无效")
            name, evaluated, expected = (
                assertion.get("alertname"),
                assertion.get("eval_time"),
                assertion.get("exp_alerts"),
            )
            if not isinstance(name, str) or not isinstance(evaluated, str) or not isinstance(expected, list):
                raise ValueError("监控规则测试断言字段无效")
            names.add(name)
            boundary.add((evaluated, name, bool(expected)))
    return names, boundary


def _validate_documents(alerts: dict, tests: dict) -> None:
    defined = _defined_alerts(alerts)
    if any(defined.count(name) != 1 for name in ALERTS):
        raise ValueError("正式监控规则没有各定义一次固定的 8 个恢复告警")
    tested, boundary = _test_alerts(tests)
    if tested != ALERTS:
        raise ValueError("正式监控规则测试没有精确覆盖固定的 8 个恢复告警")
    expected_boundary = {
        ("23h", "RyFrameBackupAging", False),
        ("23h", "RyFrameBackupStale", False),
        ("23h30s", "RyFrameBackupAging", True),
        ("23h30s", "RyFrameBackupStale", False),
        ("24h30s", "RyFrameBackupAging", False),
        ("24h30s", "RyFrameBackupStale", True),
    }
    if not expected_boundary.issubset(boundary):
        raise ValueError("正式监控规则测试缺少 23/24 小时边界断言")


def bind_rules(backend: Path) -> tuple[dict, tuple[object, object]]:
    documents, snapshots = {}, {}
    for name, relative in RULE_PATHS.items():
        documents[name], snapshots[name] = _read_yaml(backend, relative, f"监控规则 {name}")
    _validate_documents(documents["alerts"], documents["tests"])
    return (
        {name: snapshots[name].descriptor() for name in RULE_PATHS},
        (snapshots["alerts"], snapshots["tests"]),
    )


def verify_rules(backend: Path, value: object) -> tuple[object, object]:
    value = exact_fields(value, set(RULE_PATHS), "监控规则")
    paths = {name: item.get("path") if isinstance(item, dict) else None for name, item in value.items()}
    if paths != expected_paths(backend):
        raise ValueError("监控规则路径不是协调器内固定的正式文件")
    actual, snapshots = bind_rules(backend)
    if actual != value:
        raise ValueError("监控规则与绑定内容不同")
    return snapshots
