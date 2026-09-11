"""启动并核验唯一来源业务验收 Node 生产者；进程身份先于请求授权持久化。"""

from __future__ import annotations

import datetime as dt
import json
import os
from pathlib import Path
import subprocess

from devex_clone_capture import write_json
from devex_clone_run_state import binding
from devex_clone_source_proof import bound_file
from full_stack_process import process_identity, terminate_owned_process
from restore_reference_plan import plan_hash
from source_fingerprints import require_current_execution_source
from restore_runtime_evidence import (
    artifact_snapshot,
    decode_object,
    exact_fields,
    process_identity_record,
    timestamp,
)


INTENT = "source-producer-intent.json"
PROCESS = "source-producer.json"
STDOUT = "source-producer.stdout.json"
STDERR = "source-producer.stderr.log"
COMPLETION = "source-producer-completion.json"
PRODUCER_FILES = {INTENT, PROCESS, STDOUT, STDERR, COMPLETION}
INTENT_FIELDS = {
    "format_version", "kind", "operation_id", "controller", "source_generation",
    "dataset_lineage", "executable", "script", "argv", "cwd", "environment_sha256",
    "started_at", "coordinator_source", "inputs",
}
PROCESS_FIELDS = {
    "format_version", "kind", "operation_id", "intent", "controller", "process",
    "executable", "argv",
}
COMPLETION_FIELDS = {
    "format_version", "kind", "operation_id", "intent", "producer", "stdout", "stderr",
    "process", "exit_code", "completed_at",
}


def _timestamp() -> str:
    return dt.datetime.now(dt.timezone.utc).isoformat()


def _write_bytes(path: Path, value: bytes) -> dict:
    with path.open("xb") as stream:
        stream.write(value)
        stream.flush()
        os.fsync(stream.fileno())
    return binding(path)


def _validate_operation(value: object) -> str:
    if not isinstance(value, str) or len(value) != 32 or any(char not in "0123456789abcdef" for char in value):
        raise ValueError("来源验收生产者 operation ID 无效")
    return value


def _evidence_files(directory: Path) -> list[dict]:
    from restore_source_runtime import _manifest
    from devex_clone_model import linked

    rows = []
    for path in directory.iterdir():
        if linked(path):
            raise ValueError("来源验收中间证据包含链接或重解析点")
        if path.is_dir():
            if path.name not in {"before", "after", "audit"}:
                raise ValueError("来源验收中间证据包含未知目录")
            # 采集器可能在创建阶段目录后中断；内部未知或空嵌套目录仍由唯一 manifest 拒绝。
            if any(path.iterdir()):
                rows.extend({**row, "path": path.name + "/" + row["path"]} for row in _manifest(path))
        else:
            snapshot = artifact_snapshot(path)
            rows.append({"path": path.name, "bytes": snapshot.bytes, "sha256": snapshot.sha256})
    return sorted(rows, key=lambda row: row["path"])


def _verify_inputs(directory: Path, inputs: object) -> list[dict]:
    """入口授权前的采集文件保持不可变，后续只允许协议已有的精确文件集合。"""
    if not isinstance(inputs, list):
        raise ValueError("来源生产者缺少授权前证据清单")
    previous = ""
    for row in inputs:
        exact_fields(row, {"path", "bytes", "sha256"}, "来源生产者授权前证据")
        if (not isinstance(row["path"], str) or row["path"] <= previous
                or not row["path"].startswith(("before/", "audit/"))):
            raise ValueError("来源生产者授权前证据路径不属于固定采集阶段")
        previous = row["path"]
    observed = _evidence_files(directory)
    by_path = {row["path"]: row for row in observed}
    if any(by_path.get(row["path"]) != row for row in inputs):
        raise ValueError("来源生产者授权前完整证据已变化")
    allowed = {row["path"] for row in inputs} | PRODUCER_FILES | {
        "failed.json", "cache-cleanup.json", "source-runtime.json",
        "audit/login-after-old.tsv", "audit/login-after-new.tsv", "audit/login-audit.json"}
    allowed.update("after/" + row["path"].removeprefix("before/") for row in inputs if row["path"].startswith("before/"))
    if set(by_path) - allowed:
        raise ValueError("来源验收中间证据包含未登记文件")
    return observed


def _producer_command(
    backend: Path,
    execution: Path,
    directory: Path,
    operation_id: str,
    node: Path,
    start: dict,
    lineage: dict,
) -> tuple[Path, list[str]]:
    script = (backend / "scripts/restore_source_existing.mjs").resolve(strict=True)
    arguments = [
        str(node),
        str(script),
        "--backend-dir", str(execution),
        "--lineage", lineage["path"],
        "--run-dir", str(directory),
        "--operation-id", operation_id,
        "--source-generation-sha256", start["sha256"],
        "--write",
    ]
    return script, arguments


def run_source_producer(
    backend: Path,
    execution: Path,
    directory: Path,
    operation_id: str,
    node: Path,
    start: dict,
    lineage: dict,
    environment: dict[str, str],
    *,
    coordinator_source: dict,
    timeout: int = 1800,
    popen=subprocess.Popen,
) -> tuple[dict, dict]:
    """Node 在收到单条 stdin 授权前不请求服务；失败只回收登记的精确进程。"""
    _validate_operation(operation_id)
    if (
        not directory.is_absolute()
        or directory.name != "verification"
        or not directory.is_dir()
        or not node.is_absolute()
        or not node.is_file()
        or any((directory / name).exists() for name in PRODUCER_FILES)
        or any(not isinstance(key, str) or not isinstance(value, str) for key, value in environment.items())
    ):
        raise ValueError("来源验收生产者目录、Node 或环境无效")
    bound_file(backend, start)
    bound_file(backend, lineage)
    require_current_execution_source(backend, coordinator_source)
    controller = process_identity(os.getpid())
    if controller is None:
        raise ValueError("来源验收控制器缺少内核创建身份")
    script, argv = _producer_command(
        backend, execution, directory, operation_id, node.resolve(strict=True), start, lineage
    )
    inputs = _evidence_files(directory)
    _verify_inputs(directory, inputs)
    intent = {
        "format_version": 1,
        "kind": "restore-source-producer-intent",
        "operation_id": operation_id,
        "controller": controller,
        "source_generation": start,
        "dataset_lineage": lineage,
        "executable": str(node.resolve(strict=True)),
        "script": binding(script),
        "argv": argv,
        "cwd": str(backend),
        "environment_sha256": plan_hash(environment),
        "started_at": _timestamp(),
        "coordinator_source": coordinator_source,
        "inputs": inputs,
    }
    write_json(directory / INTENT, intent)
    process = None
    identity = None
    try:
        require_current_execution_source(backend, coordinator_source)
        process = popen(
            argv,
            cwd=backend,
            env=environment,
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            creationflags=subprocess.CREATE_NO_WINDOW if os.name == "nt" else 0,
        )
        identity = process_identity(process.pid)
        if identity is None or Path(identity["executable"]).resolve() != node.resolve(strict=True):
            raise ValueError("来源验收 Node 在登记创建身份前退出或可执行文件不同")
        producer = {
            "format_version": 1,
            "kind": "restore-source-producer",
            "operation_id": operation_id,
            "intent": binding(directory / INTENT),
            "controller": controller,
            "process": identity,
            "executable": str(node.resolve(strict=True)),
            "argv": argv,
        }
        write_json(directory / PROCESS, producer)
        if process_identity(os.getpid()) != controller or process_identity(process.pid) != identity:
            raise ValueError("来源验收在启动授权前控制器或 Node 身份变化")
        require_current_execution_source(backend, coordinator_source)
        authorization = json.dumps(
            {
                "operation": "start",
                "run_dir": str(directory),
                "operation_id": operation_id,
                "source_generation_sha256": start["sha256"],
            },
            ensure_ascii=False,
            sort_keys=True,
            separators=(",", ":"),
        ).encode() + b"\n"
        stdout, stderr = process.communicate(input=authorization, timeout=timeout)
        if len(stdout) > 16 * 1024 * 1024 or len(stderr) > 1024 * 1024:
            raise ValueError("来源验收生产者输出超过固定上限")
        stdout_binding = _write_bytes(directory / STDOUT, stdout)
        stderr_binding = _write_bytes(directory / STDERR, stderr)
        completion = {
            "format_version": 1,
            "kind": "restore-source-producer-completion",
            "operation_id": operation_id,
            "intent": binding(directory / INTENT),
            "producer": binding(directory / PROCESS),
            "stdout": stdout_binding,
            "stderr": stderr_binding,
            "process": identity,
            "exit_code": process.returncode,
            "completed_at": _timestamp(),
        }
        write_json(directory / COMPLETION, completion)
        require_current_execution_source(backend, coordinator_source)
        if process.returncode != 0 or stderr or process_identity(identity["pid"]) == identity:
            raise ValueError("来源验收 Node 失败、写入 stderr 或尚未退出")
        stdout_value = decode_object(stdout.strip(), "来源验收 Node stdout")
        return stdout_value, producer
    except BaseException as error:
        if identity is not None and process_identity(identity["pid"]) == identity:
            try:
                terminate_owned_process(identity, crash=True)
                if process is not None:
                    process.wait(timeout=5)
            except BaseException as cleanup:
                error.add_note("来源验收 Node 精确身份回收失败：" + type(cleanup).__name__)
        raise


def _read_bound(backend: Path, descriptor: dict, expected: Path) -> dict:
    path = bound_file(backend, descriptor)
    if path != expected:
        raise ValueError("来源验收生产者证据不属于同一 verification 目录")
    from devex_clone_capture import read_json

    return read_json(path)


def verify_registered_source_producer(
    backend: Path,
    execution: Path,
    directory: Path,
    start: dict,
    lineage: dict,
    environment: dict[str, str],
    node: Path,
    *,
    coordinator_source: dict,
) -> tuple[dict, dict]:
    """只读复核已登记生产者与同一 start、血缘、源码和环境的绑定。"""
    intent = _read_bound(backend, binding(directory / INTENT), directory / INTENT)
    require_current_execution_source(backend, coordinator_source)
    exact_fields(intent, INTENT_FIELDS, "来源验收生产者 intent")
    _verify_inputs(directory, intent["inputs"])
    producer = _read_bound(backend, binding(directory / PROCESS), directory / PROCESS)
    exact_fields(producer, PROCESS_FIELDS, "来源验收生产者")
    operation = _validate_operation(intent["operation_id"])
    node = node.resolve(strict=True)
    script, argv = _producer_command(
        backend, execution.resolve(strict=True), directory, operation, node, start, lineage
    )
    expected = {
        "format_version": 1,
        "kind": "restore-source-producer-intent",
        "operation_id": operation,
        "source_generation": start,
        "dataset_lineage": lineage,
        "executable": str(node),
        "script": binding(script),
        "argv": argv,
        "cwd": str(backend),
        "environment_sha256": plan_hash(environment),
        "coordinator_source": coordinator_source,
    }
    if any(intent.get(key) != value for key, value in expected.items()):
        raise ValueError("来源验收生产者 intent 没有绑定同一 start、血缘、源码或环境")
    controller = process_identity_record(intent["controller"], "来源验收控制器身份")
    identity = process_identity_record(producer["process"], "来源验收 Node 身份")
    if (
        type(intent["format_version"]) is not int
        or intent["format_version"] != 1
        or type(producer["format_version"]) is not int
        or producer != {
            "format_version": 1,
            "kind": "restore-source-producer",
            "operation_id": operation,
            "intent": binding(directory / INTENT),
            "controller": controller,
            "process": identity,
            "executable": str(node),
            "argv": argv,
        }
        or timestamp(intent["started_at"], "来源验收生产者开始时间")
        != intent["started_at"]
    ):
        raise ValueError("来源验收生产者登记与当前输入不同")
    require_current_execution_source(backend, coordinator_source)
    return intent, producer


def _completion(backend: Path, directory: Path, intent: dict, producer: dict) -> dict:
    completion = _read_bound(backend, binding(directory / COMPLETION), directory / COMPLETION)
    exact_fields(completion, COMPLETION_FIELDS, "来源验收生产者 completion")
    if (type(completion["format_version"]) is not int or type(completion["exit_code"]) is not int
            or completion != {
                "format_version": 1, "kind": "restore-source-producer-completion",
                "operation_id": intent["operation_id"], "intent": binding(directory / INTENT),
                "producer": binding(directory / PROCESS),
                "stdout": artifact_snapshot(directory / STDOUT).descriptor(),
                "stderr": artifact_snapshot(directory / STDERR).descriptor(), "process": producer["process"],
                "exit_code": completion["exit_code"], "completed_at": completion["completed_at"]}):
        raise ValueError("来源验收生产者完成证据或退出状态不可信")
    started = dt.datetime.fromisoformat(timestamp(intent["started_at"], "生产者开始时间").replace("Z", "+00:00"))
    completed = dt.datetime.fromisoformat(timestamp(completion["completed_at"], "生产者完成时间").replace("Z", "+00:00"))
    if not started <= completed <= dt.datetime.now(dt.timezone.utc):
        raise ValueError("来源生产者完成时间不属于同一次执行")
    return completion


def verify_source_producer(
    backend: Path,
    execution: Path,
    directory: Path,
    start: dict,
    lineage: dict,
    environment: dict[str, str],
    node: Path,
    *,
    coordinator_source: dict,
) -> tuple[dict, dict]:
    """只读复核生产者身份、实际参数、stdout/stderr 和自然退出。"""
    intent, producer = verify_registered_source_producer(
        backend, execution, directory, start, lineage, environment, node,
        coordinator_source=coordinator_source,
    )
    completion = _completion(backend, directory, intent, producer)
    identity = producer["process"]
    if (
        completion["exit_code"] != 0
        or (directory / STDERR).read_bytes() != b""
        or process_identity(identity["pid"]) == identity
    ):
        raise ValueError("来源验收生产者收据、退出状态或 stderr 不可信")
    stdout = decode_object((directory / STDOUT).read_bytes().strip(), "来源验收 Node stdout")
    return producer, stdout


def require_source_verifier_stopped(backend: Path, start: dict) -> dict:
    """供 generation recover 只读使用；不猜测、扫描或跨职责终止进程。"""
    from devex_clone_seed_generation import verify_running_source

    facts = verify_running_source(backend, start, live=False)
    directory = facts["output"] / "verification"
    from devex_clone_model import linked

    if linked(directory):
        raise ValueError("来源验收 verification 路径不能是链接或重解析点")
    if not directory.exists():
        return {"status": "not_started", "process": None}
    if not directory.is_dir():
        raise ValueError("来源验收 verification 路径不是普通目录")
    entries = list(directory.iterdir())
    if any(linked(entry) for entry in entries):
        raise ValueError("来源验收包含链接或重解析点")
    names = {entry.name for entry in entries}
    allowed = PRODUCER_FILES | {"before", "after", "audit", "cache-cleanup.json", "source-runtime.json", "failed.json"}
    if names - allowed or PROCESS not in names or INTENT not in names:
        raise ValueError("来源验收缺少已登记进程身份或包含未知证据")
    from devex_clone_factory_context import configured
    from restore_reference_io import ExternalTools

    environment = configured(facts["environment"]["environment"])
    node = Path(
        ExternalTools(facts["source"]["request"], directory).command("node")[0]
    ).resolve(strict=True)
    intent, producer = verify_registered_source_producer(
        backend,
        facts["execution"],
        directory,
        start,
        facts["receipt"]["dataset_lineage"],
        environment,
        node,
        coordinator_source=facts["coordinator_source"],
    )
    identity = producer["process"]
    if process_identity(identity["pid"]) == identity:
        raise ValueError("来源验收 Node 仍在运行，generation recover 必须失败关闭")
    if "source-runtime.json" in names and "failed.json" not in names:
        from restore_source_runtime import verify_source_runtime

        verified = verify_source_runtime(backend, binding(directory / "source-runtime.json"), live=False)
        if verified["receipt"]["source_generation"] != start:
            raise ValueError("来源验收完成收据不属于当前 source generation")
        return {"status": "verified_stopped", "process": identity}
    if STDERR in names and STDOUT not in names or COMPLETION in names and not {STDOUT, STDERR}.issubset(names):
        raise ValueError("来源验收生产者输出发布顺序不成立")
    completion = _completion(backend, directory, intent, producer) if COMPLETION in names else None
    if "failed.json" in names:
        failure = _read_bound(backend, binding(directory / "failed.json"), directory / "failed.json")
        exact_fields(failure, {"status", "error"}, "来源验收失败记录")
        if failure["status"] != "failed" or not isinstance(failure["error"], str) or not failure["error"]:
            raise ValueError("来源验收失败记录无效")
    evidence = _verify_inputs(directory, intent["inputs"])
    if process_identity(identity["pid"]) == identity:
        raise ValueError("来源验收 Node 在中间证据复核期间重新出现")
    directories = sorted(path.relative_to(directory).as_posix() for path in directory.rglob("*") if path.is_dir())
    return {"status": "stopped", "process": identity, "evidence": {"files": evidence, "directories": directories},
            "exit_code": None if completion is None else completion["exit_code"]}
