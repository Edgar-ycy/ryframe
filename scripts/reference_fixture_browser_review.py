"""只读消费 Device 浏览器成功结果及全部绑定证据。"""

from __future__ import annotations

from pathlib import Path

from full_stack_process_monitor import wait_members
from reference_fixture_browser_evidence import (
    device_tests,
    login_budget,
    verify_redacted_log,
    verify_artifact_manifest,
)
from restore_build import file_digest
from restore_frontend_build import validate_frontend_build
from restore_runtime_evidence import exact_fields, process_document, read_json_document


def _bound(path: Path) -> dict:
    return {"path": str(path), **file_digest(path)}


def _descriptor(value: object, path: Path, label: str) -> dict:
    descriptor = exact_fields(value, {"path", "bytes", "sha256"}, label)
    if descriptor != _bound(path):
        raise ValueError(f"{label}与当前文件不一致")
    return descriptor


def _process(value: object, directory: Path, scope_id: str, label: str) -> dict:
    evidence = exact_fields(value, {"directory", "process", "tree", "completion"}, label)
    if evidence["directory"] != str(directory):
        raise ValueError(f"{label}目录不一致")
    process, _ = process_document(directory / "frontend.json", "frontend", scope_id)
    _descriptor(evidence["process"], process.path, label + "进程")
    tree_path = directory / "frontend-tree.json"
    tree_document = read_json_document(tree_path)
    tree = tree_document.value
    _descriptor(evidence["tree"], tree_path, label + "进程树")
    operation = tree.get("operation_id") if isinstance(tree, dict) else None
    if not isinstance(operation, str):
        raise ValueError(f"{label}进程树缺少 operation id")
    completion_path = directory / f"frontend-members-{operation}-stopped.json"
    completion = read_json_document(completion_path)
    _descriptor(evidence["completion"], completion_path, label + "完整成员关闭证明")
    if wait_members(tree, timeout=0) != completion.value:
        raise ValueError(f"{label}完整成员关闭证明与原进程树不一致")
    process.assert_unchanged()
    tree_document.assert_unchanged()
    completion.assert_unchanged()
    return evidence


def verify_browser_result(binding: dict, context: dict, binding_path: Path) -> dict:
    outputs = context["outputs"]
    if outputs["failure"].exists() or not outputs["result"].is_file():
        raise ValueError("Device 浏览器没有唯一成功结果，必须核对失败或不确定状态")
    document = read_json_document(outputs["result"])
    if any(secret.encode("utf-8") in document.raw for secret in context["secrets"]):
        raise ValueError("Device 浏览器成功结果包含未脱敏凭据")
    result = exact_fields(
        document.value,
        {"format_version", "kind", "status", "run_id", "server", "binding", "intent",
         "build", "logs", "processes", "artifacts", "login_budget", "remote_writes"},
        "Device 浏览器成功结果",
    )
    if (
        type(result["format_version"]) is not int
        or result["format_version"] != 1
        or result["kind"] != "reference-fixture-browser-result"
        or result["status"] != "passed"
        or result["run_id"] != binding["run_id"]
        or result["server"] != binding["server"]
        or result["remote_writes"] != {"business_data": True}
    ):
        raise ValueError("Device 浏览器成功结果身份或状态无效")
    _descriptor(result["binding"], binding_path, "Device 浏览器绑定")
    intent = read_json_document(outputs["intent"])
    expected_intent = {"format_version": 1, "kind": "reference-fixture-browser-intent",
                       "binding": result["binding"], "commands": binding["commands"],
                       "login_budget": {"path": str(outputs["login_budget"]),
                                        "existed_before": False}}
    if intent.value != expected_intent:
        raise ValueError("Device 浏览器 intent 与首次写入边界不一致")
    _descriptor(result["intent"], outputs["intent"], "Device 浏览器 intent")
    keys = ["browser"] if binding["server"] == "dev" else [
        "build", "build_verify_before", "browser", "build_verify_after"
    ]
    logs = exact_fields(result["logs"], set(keys), "Device 浏览器日志")
    processes = exact_fields(result["processes"], set(keys), "Device 浏览器进程")
    for name in keys:
        verify_redacted_log(outputs[name + "_log"], logs[name], context["secrets"])
        _process(processes[name], outputs[name + "_process"], binding["scope_id"],
                 f"Device {name} 进程")
    artifacts = exact_fields(result["artifacts"], {"report", "results", "tests"}, "Device 浏览器产物")
    verify_artifact_manifest(
        artifacts["report"], outputs["report"], context["frontend"] /
        ".local-tests/playwright-real/report", "Device 浏览器 HTML 报告"
    )
    verify_artifact_manifest(
        artifacts["results"], outputs["results"], context["frontend"] /
        ".local-tests/playwright-real/results", "Device 浏览器结果"
    )
    if artifacts["tests"] != device_tests(
            outputs["results"] / "device-tests.json", binding["server"], binding["run_id"]):
        raise ValueError("Device 浏览器场景收据与成功结果不一致")
    if result["login_budget"] != login_budget(outputs["login_budget"], binding):
        raise ValueError("Device 登录预算账本与成功结果不一致")
    if binding["server"] == "preview":
        build = exact_fields(result["build"], {"receipt", "dist"}, "Device 前端生产构建")
        _, receipt = validate_frontend_build(context["frontend"])
        _descriptor(build["receipt"], receipt.path, "Device 前端构建收据")
        verify_artifact_manifest(
            build["dist"], context["frontend"] / "dist", context["frontend"],
            "Device 前端生产产物"
        )
    elif result["build"] is not None:
        raise ValueError("Device dev 浏览器结果不得绑定未使用的生产构建")
    intent.assert_unchanged()
    document.assert_unchanged()
    return result
