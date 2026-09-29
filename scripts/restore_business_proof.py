"""在最终登记前独立复核恢复运行、测试明细与双端源码。"""

from __future__ import annotations

import sys

# 只读入口即使未传 -B，也不能在加载当前及动态验证模块时生成字节码。
sys.dont_write_bytecode = True

import argparse
import json
from pathlib import Path

from restore_build import repository
from restore_reference_backup import bound_document, validate_backup_result
from restore_reference_target import FIELDS as TARGET_FIELDS, verify_target_plan
from restore_runtime import validate_launch, verify_live_generation
from restore_runtime_registration import registration_binding
from restore_runtime_static import verify_static_runtime
from restore_runtime_evidence import (
    HEX_40,
    HEX_64,
    MAX_JSON_BYTES,
    artifact_snapshot,
    decode_object,
    exact_fields,
    read_json_document,
    timestamp,
    validate_runtime_receipt,
)
from source_inventory import source_snapshot

REQUIRED_SCENARIOS = [
    "login",
    "session",
    "post",
    "notice",
    "tenant",
    "export",
    "message",
    "schedule",
    "restored-data",
]
AUTHORITY_FIELDS = {"format_version", "kind", "record", "backup_manifest", "tools"}
MANIFEST_FIELDS = {
    "id",
    "scope_id",
    "source_sha",
    "quiesced_at",
    "captured_at",
    "completed_at",
    "retention_until",
    "control_schema_fingerprint",
    "tenant_schema_fingerprint",
    "databases",
    "objects",
    "artifacts",
}
RECORD_FIELDS = {
    "plan",
    "plan_hash",
    "status",
    "started_at",
    "data_verified_at",
    "completed_at",
    "recovered_at",
    "failure",
}
PLAN_FIELDS = {
    "id",
    "backup_id",
    "scope_id",
    "fault_at",
    "databases",
    "object_endpoint",
    "object_prefix",
    "api_ready_url",
    "worker_ready_url",
    "frontend_sha",
}
PROOF_FIELDS = {
    "restore_id",
    "plan_hash",
    "backup_source_sha",
    "backend_product_sha",
    "backend_execution_sha",
    "backend_adapter_contract",
    "frontend_sha",
    "runner_sha",
    "verifier_sha",
    "scope_id",
    "frontend_url",
    "runtime_receipt_sha256",
    "tests_receipt_sha256",
    "target_plan_sha256",
    "source_generation_sha256",
    "dataset_lineage_sha256",
    "started_at",
    "completed_at",
    "scenarios",
    "unexpected_console_messages",
    "unexpected_network_failures",
    "axe_serious_or_critical",
}


def _descriptor(document) -> dict:
    snapshot = artifact_snapshot(document.path)
    if snapshot.sha256 != document.sha256:
        raise ValueError("恢复业务证明输入在摘要采集期间发生变化")
    return snapshot.descriptor()


def _receipt_descriptor(value: object, label: str) -> dict:
    value = exact_fields(value, {"path", "bytes", "sha256"}, label)
    if (
        not isinstance(value["path"], str)
        or not value["path"]
        or type(value["bytes"]) is not int
        or value["bytes"] <= 0
        or not isinstance(value["sha256"], str)
        or HEX_64.fullmatch(value["sha256"]) is None
    ):
        raise ValueError(f"{label}不是精确文件描述")
    return value


def _authority(value: object) -> tuple[dict, dict, dict, dict]:
    value = exact_fields(value, AUTHORITY_FIELDS, "恢复业务权威上下文")
    if (
        type(value["format_version"]) is not int
        or value["format_version"] != 1
        or value["kind"] != "restore-business-authority"
    ):
        raise ValueError("恢复业务权威上下文版本或类型无效")
    record = exact_fields(value["record"], RECORD_FIELDS, "权威恢复记录")
    plan = exact_fields(record["plan"], PLAN_FIELDS, "权威恢复计划")
    backup = exact_fields(value["backup_manifest"], MANIFEST_FIELDS, "权威备份清单")
    tools = exact_fields(value["tools"], {"runner", "verifier", "python"}, "权威核验工具")
    for name in ("runner", "verifier"):
        source = exact_fields(tools[name], {"root", "sha"}, f"权威核验工具 {name}")
        if (
            not isinstance(source["root"], str)
            or not Path(source["root"]).is_absolute()
            or not isinstance(source["sha"], str)
            or HEX_40.fullmatch(source["sha"]) is None
        ):
            raise ValueError(f"权威核验工具 {name} 来源无效")
    _receipt_descriptor(tools["python"], "权威 Python 解释器")
    if (
        record["status"] != "data_verified"
        or record["completed_at"] is not None
        or record["failure"] is not None
        or not isinstance(record["data_verified_at"], str)
        or not isinstance(record["plan_hash"], str)
        or HEX_64.fullmatch(record["plan_hash"]) is None
        or backup["id"] != plan["backup_id"]
        or not isinstance(backup["source_sha"], str)
        or HEX_40.fullmatch(backup["source_sha"]) is None
    ):
        raise ValueError("恢复业务权威记录尚未完成数据验证或来源无效")
    for field in ("started_at", "data_verified_at", "recovered_at"):
        timestamp(record[field], f"权威恢复记录 {field}")
    return value, record, backup, tools


def _verified_target(backend: Path, path: Path, runtime: dict) -> tuple[object, dict, dict, dict, dict]:
    document = read_json_document(path)
    value = exact_fields(document.value, TARGET_FIELDS, "正式恢复目标计划")
    backup = bound_document(backend, value["backup_receipt"])
    receipt = exact_fields(
        backup.value,
        {"command", "plan_sha256", "started_at", "status", "completed_at", "result"},
        "正式备份收据",
    )
    result = receipt["result"]
    validate_backup_result(result)
    manifest = bound_document(backend, result["manifest"])
    reference = _runtime_reference(backend, runtime, document)
    verified = verify_target_plan(backend, reference, document.path)
    if verified != value:
        raise ValueError("正式恢复目标计划复核结果不同")
    dataset = _dataset_authority(backend, result)
    document.assert_unchanged()
    backup.assert_unchanged()
    manifest.assert_unchanged()
    return document, verified, reference, manifest.value, dataset


def _dataset_authority(backend: Path, backup: dict) -> dict:
    """从已完整核验的目标备份链提取唯一数据来源，不接受浏览器指定的独立血缘。"""
    from devex_clone_seed_generation import RESULT_FIELDS, START_FIELDS
    from restore_source_runtime import RECEIPT_FIELDS, RECOVERED_RECEIPT_FIELDS

    stop = bound_document(backend, backup["source_generation"])
    value = exact_fields(stop.value, RESULT_FIELDS, "备份来源停止代次")
    start = bound_document(backend, value["start"])
    runtime = bound_document(backend, value["source_runtime"])
    lineage = bound_document(backend, value["dataset_lineage"])
    running = exact_fields(start.value, START_FIELDS, "备份来源启动代次")
    runtime_fields = set(runtime.value) if isinstance(runtime.value, dict) else set()
    expected_fields = (RECOVERED_RECEIPT_FIELDS
                       if runtime_fields == RECOVERED_RECEIPT_FIELDS else RECEIPT_FIELDS)
    verified = exact_fields(runtime.value, expected_fields, "备份来源运行验收")
    if expected_fields == RECOVERED_RECEIPT_FIELDS:
        _receipt_descriptor(verified["recovery"], "备份来源运行验收恢复绑定")
    if (
        backup["source_export"]["source_generation"] != _descriptor(stop)
        or value["status"] != "seed_source_generation_published"
        or running["status"] != "seed_source_generation_running"
        or verified["source_generation"] != _descriptor(start)
        or any(item["dataset_lineage"] != _descriptor(lineage) for item in (value, running, verified))
    ):
        raise ValueError("备份停止、启动、运行验收与数据血缘不是同一不可变来源")
    for document in (stop, start, runtime, lineage):
        document.assert_unchanged()
    return {"source_generation": _descriptor(stop), "dataset_lineage": _descriptor(lineage)}


def _adapter(execution: dict) -> str | None:
    value = execution["adapter"]
    if value is None:
        return None
    value = exact_fields(
        value,
        {
            "contract",
            "base_backend_sha",
            "base_frontend_sha",
            "reference_adapter_sha",
            "adapter_tree",
            "reconstructed_tree",
            "adapter_paths",
            "patch",
        },
        "恢复产品适配绑定",
    )
    if value["contract"] != "legacy-stable-readiness-b0-v1":
        raise ValueError("恢复产品适配合同无效")
    return value["contract"]


def _runtime_authority(record: dict, backup: dict, target: dict, reference: dict) -> dict:
    plan = record["plan"]
    if target["product_plan"] != plan:
        raise ValueError("恢复目标计划与登记库权威产品计划不同")
    execution = target["product_execution"]
    source = {
        "backup_source_sha": backup["source_sha"],
        "backend_product_sha": execution["backend_product_sha"],
        "backend_execution_sha": execution["backend_execution_sha"],
        "backend_adapter_contract": _adapter(execution),
        "frontend_sha": execution["frontend_sha"],
    }
    if source["frontend_sha"] != plan["frontend_sha"]:
        raise ValueError("恢复产品前端来源与登记库计划不同")
    return {
        "format_version": 2,
        "kind": "restore-runtime-authority",
        "restore_id": plan["id"],
        "backup_id": plan["backup_id"],
        "plan_hash": record["plan_hash"],
        "scope_id": plan["scope_id"],
        "data_verified_at": record["data_verified_at"],
        **source,
        "api_endpoint": plan["api_ready_url"],
        "worker_endpoint": plan["worker_ready_url"],
        "frontend_endpoint": reference["target"]["frontend_url"],
    }


def _runtime_reference(backend: Path, runtime: dict, target_document) -> dict:
    launch_document = read_json_document(Path(runtime["paths"]["launch"]))
    launch = validate_launch(launch_document.value, launch_document.path.parent)
    registered = launch["request"]["registration"]
    if registered["target_plan"] != _descriptor(target_document):
        raise ValueError("恢复运行代次没有绑定显式目标计划")
    registration = bound_document(backend, registered["registration"])
    _value, _facts, documents = registration_binding(backend, registration.path, _descriptor(target_document))
    reference = documents[1].value
    registration.assert_unchanged()
    launch_document.assert_unchanged()
    return reference


def _clean_source(root: Path, expected: str, label: str) -> Path:
    root = repository(root, label)
    snapshot = source_snapshot(root)
    if (
        not isinstance(expected, str)
        or HEX_40.fullmatch(expected) is None
        or snapshot["head"] != expected
        or not snapshot["clean"]
    ):
        raise ValueError(f"{label}必须是证明绑定 SHA 的干净工作树")
    return root


def _source_fields(authority: dict, runner: Path, runner_sha: str, verifier: Path, verifier_sha: str) -> dict:
    return {
        **{key: authority[key] for key in (
            "backup_source_sha",
            "backend_product_sha",
            "backend_execution_sha",
            "backend_adapter_contract",
            "frontend_sha",
        )},
        "runner": {"root": str(runner), "sha": runner_sha},
        "verifier": {"root": str(verifier), "sha": verifier_sha},
    }


def _validate_runs(value: object) -> list[dict]:
    if not isinstance(value, list) or not value:
        raise ValueError("恢复浏览器测试明细不能为空")
    observed = []
    for item in value:
        run = exact_fields(item, {"title", "status", "retry", "scenarios"}, "恢复测试明细")
        if (
            not isinstance(run["title"], list)
            or not run["title"]
            or any(not isinstance(part, str) or not part.strip() for part in run["title"])
            or run["status"] != "passed"
            or type(run["retry"]) is not int
            or run["retry"] != 0
            or not isinstance(run["scenarios"], list)
            or any(not isinstance(name, str) or not name for name in run["scenarios"])
        ):
            raise ValueError("恢复浏览器测试包含失败、重试或无效标题")
        observed.extend(run["scenarios"])
    if set(observed) != set(REQUIRED_SCENARIOS) or len(set(observed)) != len(observed):
        raise ValueError("恢复浏览器测试场景缺失、额外或重复")
    return value


def _evidence_root(runner: Path, restore_id: str, proof, tests, runtime) -> None:
    expected = runner / ".local-tests" / "playwright-real"
    names = (
        (proof, f"restore-{restore_id}.json"),
        (tests, f"restore-{restore_id}-tests.json"),
        (runtime, f"restore-{restore_id}-runtime.json"),
    )
    for document, name in names:
        if document.path.parent != expected or document.path.name != name:
            raise ValueError("恢复业务证明、运行副本与测试明细必须位于 runner 的隔离证据目录")


def _validate_evidence(
    record: dict,
    authority: dict,
    proof_document,
    tests_document,
    runtime_document,
    target_document,
    runner: Path,
    verifier: Path,
    dataset_authority: dict,
) -> dict:
    proof = exact_fields(proof_document.value, PROOF_FIELDS, "恢复业务证明")
    runner_sha, verifier_sha = proof["runner_sha"], proof["verifier_sha"]
    if any(not isinstance(value, str) or HEX_40.fullmatch(value) is None for value in (runner_sha, verifier_sha)):
        raise ValueError("恢复业务证明缺少精确 runner 或 verifier SHA")
    expected_source = _source_fields(authority, runner, runner_sha, verifier, verifier_sha)
    receipt = exact_fields(
        tests_document.value,
        {
            "format_version",
            "kind",
            "restore",
            "runtime",
            "target_plan",
            "source_generation",
            "dataset_lineage",
            "sources",
            "frontend_url",
            "started_at",
            "completed_at",
            "runs",
        },
        "恢复浏览器测试收据",
    )
    _validate_runs(receipt["runs"])
    runtime_binding = _receipt_descriptor(receipt["runtime"], "恢复测试运行收据描述")
    target_binding = _receipt_descriptor(receipt["target_plan"], "恢复测试目标计划描述")
    dataset = {key: _receipt_descriptor(receipt[key], f"恢复测试来源 {key}")
               for key in ("source_generation", "dataset_lineage")}
    expected_restore = {
        "id": record["plan"]["id"],
        "plan_hash": record["plan_hash"],
        "scope_id": record["plan"]["scope_id"],
    }
    if (
        type(receipt["format_version"]) is not int
        or receipt["format_version"] != 1
        or receipt["kind"] != "restore-browser-tests"
        or receipt["restore"] != expected_restore
        or runtime_binding != _descriptor(runtime_document)
        or target_binding != _descriptor(target_document)
        or dataset != dataset_authority
        or receipt["sources"] != expected_source
        or receipt["frontend_url"] != authority["frontend_endpoint"]
    ):
        raise ValueError("恢复浏览器测试收据与权威运行、目标或源码不一致")
    expected_proof = {
        "restore_id": expected_restore["id"],
        "plan_hash": expected_restore["plan_hash"],
        **{key: authority[key] for key in (
            "backup_source_sha",
            "backend_product_sha",
            "backend_execution_sha",
            "backend_adapter_contract",
            "frontend_sha",
        )},
        "runner_sha": runner_sha,
        "verifier_sha": verifier_sha,
        "scope_id": expected_restore["scope_id"],
        "frontend_url": authority["frontend_endpoint"],
        "runtime_receipt_sha256": runtime_document.sha256,
        "tests_receipt_sha256": tests_document.sha256,
        "target_plan_sha256": target_document.sha256,
        **{key + "_sha256": descriptor["sha256"] for key, descriptor in dataset_authority.items()},
        "started_at": receipt["started_at"],
        "completed_at": receipt["completed_at"],
        "scenarios": [{"name": name, "succeeded": True} for name in REQUIRED_SCENARIOS],
        "unexpected_console_messages": 0,
        "unexpected_network_failures": 0,
        "axe_serious_or_critical": 0,
    }
    for field in ("started_at", "completed_at"):
        timestamp(receipt[field], f"恢复浏览器测试 {field}")
    if proof != expected_proof:
        raise ValueError("恢复业务证明与测试明细或权威来源不一致")
    return proof


def verify_business_proof(
    backend: Path,
    runner_root: Path,
    proof_path: Path,
    tests_path: Path,
    runtime_path: Path,
    target_path: Path,
    authority_value: object,
) -> dict:
    backend = repository(backend, "恢复证明协调后端")
    _value, record, backup, tools = _authority(authority_value)
    proof = read_json_document(proof_path)
    tests = read_json_document(tests_path)
    runtime = read_json_document(runtime_path)
    runtime_value = validate_runtime_receipt(runtime.value)
    target, target_value, reference, target_manifest, dataset = _verified_target(backend, target_path, runtime_value)
    if target_manifest != backup:
        raise ValueError("正式恢复目标引用的备份清单与登记库权威备份不同")
    runtime_authority = _runtime_authority(record, backup, target_value, reference)
    runner_source = exact_fields(tools["runner"], {"root", "sha"}, "权威测试 runner")
    verifier_source = exact_fields(tools["verifier"], {"root", "sha"}, "权威恢复核验器")
    runner = _clean_source(runner_root, runner_source["sha"], "恢复测试 runner")
    verifier = _clean_source(backend, verifier_source["sha"], "恢复证明协调后端")
    if str(runner) != runner_source["root"] or str(verifier) != verifier_source["root"]:
        raise ValueError("恢复核验源码根与 Rust 权威上下文不同")
    python = artifact_snapshot(Path(sys.executable))
    if python.descriptor() != tools["python"]:
        raise ValueError("恢复证明核验器没有使用权威上下文绑定的 Python 解释器")
    _evidence_root(runner, record["plan"]["id"], proof, tests, runtime)
    roots = target_value["product_execution"]["roots"]
    product, execution, frontend = map(
        Path,
        (roots["source_backend"], roots["execution_backend"], roots["frontend"]),
    )
    result = verify_live_generation(
        runtime_value,
        backend,
        execution,
        frontend,
        Path(runtime_value["paths"]["bindings"]),
        runtime_authority,
        runtime.sha256,
        product_backend=product if product != execution else None,
    )
    if result["runtime_receipt_sha256"] != runtime.sha256:
        raise ValueError("恢复运行主动核验没有返回当前收据摘要")
    verified = _validate_evidence(
        record, runtime_authority, proof, tests, runtime, target, runner, verifier, dataset
    )
    for document in (proof, tests, runtime, target):
        document.assert_unchanged()
    _clean_source(runner, verified["runner_sha"], "恢复测试 runner")
    _clean_source(verifier, verified["verifier_sha"], "恢复证明协调后端")
    python.assert_unchanged()
    return verified


def dataset_preflight(backend: Path, runtime_path: Path, target_path: Path) -> dict:
    """在浏览器首次会话写入前完整只读重验来源链，仅返回内存投影。"""
    backend = repository(backend, "恢复数据核验后端")
    runtime = read_json_document(runtime_path)
    value = validate_runtime_receipt(runtime.value)
    target, verified, reference, manifest, dataset = _verified_target(backend, target_path, value)
    verify_static_runtime(backend, value, target, verified, reference, manifest)
    selected = reference["target"]
    result = {
        "format_version": 1, "kind": "restore-dataset-authority",
        "runtime": _descriptor(runtime), "target_plan": _descriptor(target), **dataset,
        "target": {key: selected[key] for key in ("scope_id", "api_url", "frontend_url")},
        "execution_backend": verified["product_execution"]["roots"]["execution_backend"],
    }
    runtime.assert_unchanged()
    target.assert_unchanged()
    return result


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__, allow_abbrev=False)
    parser.add_argument("--backend-dir", type=Path, required=True)
    parser.add_argument("--preflight", action="store_true",
                        help="浏览器首次会话前完整只读重验目标来源，仅向标准输出返回数据血缘绑定")
    parser.add_argument("--runner-root", type=Path)
    parser.add_argument("--proof", type=Path)
    parser.add_argument("--tests-receipt", type=Path)
    parser.add_argument("--runtime-receipt", type=Path, required=True)
    parser.add_argument("--target-plan", type=Path, required=True)
    options = [value.partition("=")[0] for value in sys.argv[1:] if value.startswith("--")]
    if len(options) != len(set(options)):
        parser.error("恢复业务证明或数据预检选项不能重复")
    args = parser.parse_args()
    provided = (args.runner_root, args.proof, args.tests_receipt)
    if args.preflight:
        if any(value is not None for value in provided):
            parser.error("只读数据预检不能接收 runner、proof 或 tests-receipt")
        print(json.dumps(dataset_preflight(args.backend_dir, args.runtime_receipt, args.target_plan),
                         ensure_ascii=False, sort_keys=True, separators=(",", ":")))
        return
    if any(value is None for value in provided):
        parser.error("最终业务证明必须同时提供 runner-root、proof 和 tests-receipt")
    raw = sys.stdin.buffer.read(MAX_JSON_BYTES + 1)
    if not raw or len(raw) > MAX_JSON_BYTES:
        raise ValueError("恢复业务权威上下文必须是 16 MiB 内的非空 JSON")
    authority = decode_object(raw, "恢复业务权威上下文")
    result = verify_business_proof(
        args.backend_dir,
        args.runner_root,
        args.proof,
        args.tests_receipt,
        args.runtime_receipt,
        args.target_plan,
        authority,
    )
    print(json.dumps(result, ensure_ascii=False, sort_keys=True, separators=(",", ":")))


if __name__ == "__main__":
    main()
