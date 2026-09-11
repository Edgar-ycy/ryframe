"""绑定备份前实际运行的干净构建与已有数据复验；不准备数据或执行备份。"""
from __future__ import annotations

import argparse
import datetime as dt
import json
import os
from pathlib import Path
import subprocess
import sys
import urllib.request
from urllib.parse import urlsplit

from full_stack_process import process_identity, read_process
from full_stack_runtime import verify_runtime
from process_sockets import endpoint, verify_listener
from restore_build import file_digest, validate_new_output, verify_build, write_new
from restore_comparison_source import capture_comparison_sources, verify_comparison_sources
from restore_reference_io import ExternalTools
from restore_reference_plan import plan_hash, validate_plan
from restore_runtime import NoRedirect, read_json
from restore_runtime_evidence import read_json_document
from restore_source_binding import source_binding
from restore_source_runtime import execute_source_verification


def build_sha(build: dict) -> str:
    return build["sources"]["full"]["source"]["snapshot"]["head"]


def instant(value: str) -> dt.datetime:
    result = dt.datetime.fromisoformat(value.replace("Z", "+00:00"))
    if result.tzinfo is None:
        raise ValueError("来源证据时间必须包含时区")
    return result


def source_state(backend: Path, plan: dict, build: dict, *, stopped=False) -> dict:
    verify_build(backend, build, build_sha(build))
    side = plan["source"]
    directory = Path(side["runtime_dir"])
    runtime = verify_runtime(backend, directory)
    if runtime["scope_id"] != side["scope_id"]:
        raise ValueError("来源运行 scope 与参考计划不同")
    physical = source_binding(backend, plan)
    processes = {}
    urls = {"api": side["api_url"].rstrip("/") + "/readyz", "worker": runtime["worker_ready_url"]}
    for role in ("api", "worker"):
        identity = read_process(directory, role, side["scope_id"])
        artifact = build["artifacts"][role]
        if (Path(identity["executable"]).resolve() != Path(artifact["executable"]).resolve()
                or runtime["artifacts"][role] != {"path": artifact["executable"], "sha256": artifact["sha256"]}):
            raise ValueError("来源 API/Worker 并非所声明干净构建的实际产物")
        actual = process_identity(identity["pid"])
        if stopped:
            if actual is not None:
                raise ValueError("来源进程尚未停止或 PID 被重用")
        else:
            if actual != identity:
                raise ValueError("来源进程创建身份已变化")
            endpoint(urls[role])
            if urlsplit(urls[role]).path != "/readyz":
                raise ValueError("来源探针必须为明确的 readyz")
            verify_listener(identity["pid"], urls[role])
            opener = urllib.request.build_opener(urllib.request.ProxyHandler({}), NoRedirect())
            with opener.open(urls[role], timeout=10) as response:
                if response.status != 200:
                    raise ValueError("来源 API/Worker 尚未就绪")
        processes[role] = identity
    return {"runtime": runtime, "processes": processes, "physical_binding": physical}


def expected_result(plan: dict, dataset: dict, digest: str) -> dict:
    if (dataset.get("format_version") != 1 or dataset.get("plan_sha256") != plan_hash(plan)
            or dataset.get("source_scope_id") != plan["source"]["scope_id"]
            or type(dataset.get("records")) is not int or dataset["records"] < 100_000
            or type(dataset.get("object_bytes")) is not int or dataset["object_bytes"] < 1024**3):
        raise ValueError("来源数据必须绑定原计划且达到参考规模")
    tenants = dataset["tenants"]
    expected = {"system", *[f"{plan['source']['scope_id']}-{index:02d}" for index in range(1, 11)]}
    if (len(tenants) != 11 or {tenant["tenant_id"] for tenant in tenants} != expected
            or sum(tenant["records"] for tenant in tenants) != dataset["records"]
            or sum(file["bytes"] for tenant in tenants for file in tenant["files"]) != dataset["object_bytes"]
            or any(len(tenant["posts"]) < 3 or not tenant["files"] for tenant in tenants)):
        raise ValueError("来源数据样本、租户或声明规模不一致")
    return {"format_version": 1, "status": "existing_data_verified", "side": "source",
            "scope_id": plan["source"]["scope_id"], "plan_sha256": plan_hash(plan),
            "source_scope_id": dataset["source_scope_id"], "dataset_sha256": digest,
            "actions": {"business": "read_only", "objects": "read_only", "session": "login_logout"},
            "restore_success": False, "tenants": len(tenants),
            "posts": sum(len(tenant["posts"]) for tenant in tenants),
            "files": sum(len(tenant["files"]) for tenant in tenants)}


def verify_source(backend: Path, plan_path: Path, build_path: Path, dataset_path: Path) -> dict:
    plan, build, dataset = map(read_json, (plan_path, build_path, dataset_path))
    validate_plan(plan, backend)
    work = Path(plan["work_dir"])
    owner = {"format_version": 1, "id": plan["id"], "plan_sha256": plan_hash(plan)}
    if read_json(work / "reference-owner.json") != owner:
        raise ValueError("来源复验目录 ownership 不匹配")
    inputs = {str(path): file_digest(path) for path in (plan_path, build_path, dataset_path)}
    expected = expected_result(plan, dataset, inputs[str(dataset_path)]["sha256"])
    started = dt.datetime.now(dt.timezone.utc)
    if instant(dataset["completed_at"]) > started:
        raise ValueError("数据准备尚未完成或完成时间位于未来")
    before = source_state(backend, plan, build)
    tools = ExternalTools(plan, work)
    command = [*tools.command("node"), str(backend / "scripts/restore_reference_dataset.mjs"),
               "--plan", str(plan_path), "--backend-dir", str(backend), "--verify-existing",
               str(dataset_path), "--side", "source", "--write"]
    result = tools.execute(command, timeout=1800, env={**os.environ, "RYFRAME_PYTHON": sys.executable})
    verified = json.loads(result.stdout)
    if verified != expected:
        raise ValueError("来源复验结果未绑定同一计划、数据摘要、范围或检查侧")
    if source_state(backend, plan, build) != before:
        raise ValueError("来源复验期间构建、配置或进程代次发生变化")
    if any(file_digest(Path(path)) != digest for path, digest in inputs.items()):
        raise ValueError("来源复验期间输入证据发生变化")
    return {"format_version": 1, "kind": "restore-source-runtime", "plan_sha256": plan_hash(plan),
            "backend_root": str(backend), "scope_id": plan["source"]["scope_id"], "build": build,
            **before, "inputs": inputs, "dataset_path": str(dataset_path), "verified": verified,
            "started_at": started.isoformat(), "verified_at": dt.datetime.now(dt.timezone.utc).isoformat()}


def stopped_source_evidence(backend: Path, plan: dict, receipt_path: Path) -> tuple[dict, dict, str]:
    original = file_digest(receipt_path)
    receipt = read_json(receipt_path)
    expected = {"format_version": 1, "kind": "restore-source-runtime", "plan_sha256": plan_hash(plan),
                "backend_root": str(backend), "scope_id": plan["source"]["scope_id"]}
    if any(receipt.get(key) != value for key, value in expected.items()):
        raise ValueError("备份来源运行证明与本次计划、源码目录或 scope 不匹配")
    verify_build(backend, receipt["build"], build_sha(receipt["build"]))
    for filename, digest in receipt["inputs"].items():
        if file_digest(Path(filename)) != digest:
            raise ValueError("来源复验证据已变化")
    dataset_path = Path(receipt["dataset_path"])
    dataset = read_json(dataset_path)
    if receipt["verified"] != expected_result(plan, dataset, file_digest(dataset_path)["sha256"]):
        raise ValueError("来源复验收据被替换或混入恢复目标结果")
    state = source_state(backend, plan, receipt["build"], stopped=True)
    if any(receipt.get(key) != value for key, value in state.items()):
        raise ValueError("来源复验后进程曾重启或运行配置被替换")
    if not (instant(dataset["completed_at"]) <= instant(receipt["started_at"])
            <= instant(receipt["verified_at"]) <= dt.datetime.now(dt.timezone.utc)):
        raise ValueError("来源数据与复验时间顺序不成立")
    if file_digest(receipt_path) != original:
        raise ValueError("备份核验期间来源运行收据发生变化")
    return receipt, state, original["sha256"]


def quiesce_source(backend: Path, plan: dict, receipt_path: Path) -> dict:
    receipt, state, digest = stopped_source_evidence(backend, plan, receipt_path)
    return {"format_version": 1, "kind": "restore-source-quiescence", "plan_sha256": plan_hash(plan),
            "scope_id": plan["source"]["scope_id"], "source_runtime_sha256": digest,
            "source_sha": build_sha(receipt["build"]), "processes": state["processes"],
            "observed_stopped_at": dt.datetime.now(dt.timezone.utc).isoformat()}


def verify_stopped_source(backend: Path, plan: dict, inventory: dict, receipt_path: Path,
                          quiescence_path: Path) -> dict:
    receipt, state, digest = stopped_source_evidence(backend, plan, receipt_path)
    verify_build(backend, receipt["build"], inventory["source_sha"])
    observed_digest = file_digest(quiescence_path)
    observed = read_json(quiescence_path)
    expected = {"format_version": 1, "kind": "restore-source-quiescence", "plan_sha256": plan_hash(plan),
                "scope_id": plan["source"]["scope_id"], "source_runtime_sha256": digest,
                "source_sha": inventory["source_sha"], "processes": state["processes"]}
    if set(observed) != set(expected) | {"observed_stopped_at"} or any(observed.get(key) != value for key, value in expected.items()):
        raise ValueError("停止观察收据与同一来源、构建及进程代次不匹配")
    if not (instant(receipt["verified_at"]) <= instant(observed["observed_stopped_at"])
            <= instant(inventory["quiesced_at"]) <= instant(inventory["captured_at"])
            <= dt.datetime.now(dt.timezone.utc)):
        raise ValueError("来源复验、实际停止观察与清单采集时间顺序不成立")
    if file_digest(quiescence_path) != observed_digest:
        raise ValueError("备份核验期间停止观察收据发生变化")
    return {"source_runtime_sha256": digest, "source_quiescence_sha256": observed_digest["sha256"]}


def _add_comparison_capture_parser(commands) -> None:
    command = commands.add_parser("comparison-capture")
    for name in (
        "backend-dir", "b0-backend", "b0-adapter-backend", "b0-frontend",
        "b0-backend-build", "b0-frontend-build", "b1-backend", "b1-frontend",
        "b1-backend-build", "b1-frontend-build", "source-export-result", "output",
    ):
        command.add_argument("--" + name, type=Path, required=True)
    command.add_argument(
        "--write", action="store_true", required=True,
        help="显式写入新的双版本来源清单；只读取源码、构建收据和已发布 source-export",
    )


def _add_comparison_verify_parser(commands) -> None:
    command = commands.add_parser("comparison-verify")
    command.add_argument("--backend-dir", type=Path, required=True)
    command.add_argument("--receipt", type=Path, required=True)


def _comparison_result_binding(path: Path) -> tuple[dict, object]:
    document = read_json_document(path)
    return {
        "path": str(document.path),
        "bytes": len(document.raw),
        "sha256": document.sha256,
    }, document


def _capture_comparison(args, backend: Path) -> dict:
    output = validate_new_output(args.output, backend)
    export, document = _comparison_result_binding(args.source_export_result)
    result = capture_comparison_sources(
        backend,
        b0_backend=args.b0_backend,
        b0_adapter_backend=args.b0_adapter_backend,
        b0_frontend=args.b0_frontend,
        b0_backend_build=args.b0_backend_build,
        b0_frontend_build=args.b0_frontend_build,
        b1_backend=args.b1_backend,
        b1_frontend=args.b1_frontend,
        b1_backend_build=args.b1_backend_build,
        b1_frontend_build=args.b1_frontend_build,
        source_export_result=export,
    )
    document.assert_unchanged()
    write_new(output, result, backend)
    return {"output": str(output), "status": "comparison_sources_captured", "restore_success": False}


def _verify_comparison(args, backend: Path) -> dict:
    document = read_json_document(args.receipt)
    result = verify_comparison_sources(backend, document.value)
    document.assert_unchanged()
    return {
        "receipt": str(document.path),
        "status": "comparison_sources_verified",
        "source_export_identity_sha256": result["source_export"]["identity_sha256"],
        "restore_success": False,
    }


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    command = commands.add_parser("verify")
    command.add_argument("--backend-dir", type=Path, required=True)
    command.add_argument("--source-generation", type=Path, required=True)
    command.add_argument("--output", type=Path, required=True)
    command.add_argument(
        "--write",
        action="store_true",
        required=True,
        help="显式执行同代来源只读业务验收、登记会话副作用并写入严格收据",
    )
    _add_comparison_capture_parser(commands)
    _add_comparison_verify_parser(commands)
    args = parser.parse_args()
    backend = args.backend_dir.resolve()
    if args.command == "comparison-capture":
        print(json.dumps(_capture_comparison(args, backend)))
        return
    if args.command == "comparison-verify":
        print(json.dumps(_verify_comparison(args, backend)))
        return
    print(json.dumps(execute_source_verification(
        backend, args.source_generation, args.output
    )))


if __name__ == "__main__":
    main()
