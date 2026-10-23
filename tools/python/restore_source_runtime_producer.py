"""启动并核验唯一来源业务验收原生生产者；进程身份先于请求授权持久化。"""

from __future__ import annotations

from native_s3_client import _invoke as native_invoke, _read_response as native_response

import datetime as dt
import json
import os
from pathlib import Path
import queue
import subprocess
import threading

from devex_clone_capture import read_json, write_json
from devex_clone_run_state import binding
from devex_clone_source_proof import bound_file
from full_stack_process import process_identity, terminate_owned_process
from restore_reference_plan import plan_hash
from source_fingerprints import require_current_execution_source, verify_execution_source
from restore_runtime_evidence import (
    artifact_snapshot,
    decode_object,
    exact_fields,
)


INTENT = "source-producer-intent.json"
PROCESS = "source-producer.json"
READY = "source-producer-ready.json"
STDOUT = "source-producer.stdout.json"
STDERR = "source-producer.stderr.log"
COMPLETION = "source-producer-completion.json"
CONFIGURATION = "source-producer-configuration.json"
PRODUCER_FILES = {INTENT, PROCESS, READY, STDOUT, STDERR, COMPLETION, CONFIGURATION}


def native_tool() -> Path:
    """读取协调器安装的原生工具绑定；不从 PATH 或环境猜测。"""
    from rate_limit_tool import tool_binding, verified_tool

    value = tool_binding()
    if value is None:
        raise ValueError("来源验收缺少协调器原生工具绑定")
    with verified_tool(value) as executable:
        return Path(executable).resolve(strict=True)


def _timestamp() -> str:
    return dt.datetime.now(dt.timezone.utc).isoformat()


def _write_bytes(path: Path, value: bytes) -> dict:
    with path.open("xb") as stream:
        stream.write(value)
        stream.flush()
        os.fsync(stream.fileno())
    return binding(path)


def _source_policy(name: str, **arguments):
    return native_response(*native_invoke(
        "__source-evidence", {
            "operation": name, "write": False, "arguments": arguments,
        }, subprocess.run, may_write=False,
    ))[0]["value"]


def _runtime_policy(action: str, **arguments):
    return _source_policy("producer-ready", action=action, arguments=arguments)


def _inventory_request(directory: Path, operation: str, **arguments):
    from restore_source_runtime import _evidence_backend

    return _source_policy(
        operation, backend=str(_evidence_backend(directory)), directory=str(directory), **arguments,
    )


def _evidence_files(directory: Path) -> list[dict]:
    return _inventory_request(directory, "producer-files")


def _verify_inputs(directory: Path, inputs: object) -> list[dict]:
    return _inventory_request(directory, "producer-inputs", inputs=inputs)


def _producer_command(
    staging: Path,
    backend: Path,
    execution: Path,
    directory: Path,
    operation_id: str,
    tool: Path,
    start: dict,
    lineage: dict,
    configuration: Path,
) -> tuple[Path, list[str]]:
    value = _runtime_policy(
        "producer-command", staging=str(staging), backend=str(backend), execution=str(execution),
        directory=str(directory), operation=operation_id, tool=str(tool), start=start,
        lineage=lineage, configuration=str(configuration),
    )
    return Path(value["manifest"]).resolve(strict=True), value["argv"]


def _native_environment(environment: dict[str, str]) -> dict[str, str]:
    """原生核验只继承系统运行变量和唯一 APP_ENV，不传递任何业务配置或凭据。"""
    from process_environment import configured

    if environment.get("APP_ENV") != "test":
        raise ValueError("来源原生核验必须绑定 test 配置环境")
    return configured({"APP_ENV": "test"})


def _ready_line(process, timeout: int) -> bytes:
    """在任何请求授权前有界等待原生生产者完成契约与血缘预载。"""
    if process.stdout is None or type(timeout) is not int or timeout <= 0:
        raise ValueError("来源验收生产者缺少 ready 管道或有效超时")
    result = queue.Queue(maxsize=1)

    def read() -> None:
        try:
            result.put((process.stdout.readline(64 * 1024 + 1), None))
        except BaseException as error:
            result.put((None, error))

    threading.Thread(target=read, name="restore-source-ready", daemon=True).start()
    try:
        line, error = result.get(timeout=min(timeout, 60))
    except queue.Empty as error:
        raise ValueError("来源验收原生生产者未在授权前发布 ready") from error
    if error is not None:
        raise ValueError("来源验收原生生产者 ready 管道读取失败") from error
    if not line or len(line) > 64 * 1024 or not line.endswith(b"\n"):
        raise ValueError("来源验收原生生产者 ready 不是有界单行证据")
    return line


def _verify_ready(raw: bytes, operation: str, start: dict, lineage: dict, staging: dict) -> dict:
    try:
        text = raw.decode("utf-8")
    except UnicodeDecodeError:
        raise ValueError("来源验收原生生产者 ready 不是UTF-8") from None
    return _source_policy(
        'producer-ready',
        raw=text,
        operation=operation,
        start=start,
        lineage=lineage,
        staging=staging,
    )


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
    execution_sha: str,
    timeout: int = 1800,
    popen=subprocess.Popen,
) -> tuple[dict, dict]:
    """原生生产者在收到单条 stdin 授权前不请求服务；失败只回收登记的精确进程。"""
    _runtime_policy("producer-operation", value=operation_id)
    if (
        not directory.is_absolute()
        or directory.name != "verification"
        or not directory.is_dir()
        or not node.is_absolute()
        or not node.is_file()
        or any((directory / name).exists() for name in PRODUCER_FILES)
        or any(not isinstance(key, str) or not isinstance(value, str) for key, value in environment.items())
    ):
        raise ValueError("来源验收生产者目录、原生工具或环境无效")
    bound_file(backend, start)
    bound_file(backend, lineage)
    require_current_execution_source(backend, coordinator_source)
    controller = process_identity(os.getpid())
    if controller is None:
        raise ValueError("来源验收控制器缺少内核创建身份")
    tool_snapshot = artifact_snapshot(node.resolve(strict=True)).descriptor()
    tool = {key: tool_snapshot[key] for key in ("path", "sha256")}
    from restore_reference_plan import configuration_binding
    configuration_path = directory / CONFIGURATION
    configuration_value = configuration_binding(execution, execution / "config", "test")
    configuration_descriptor = _write_bytes(
        configuration_path,
        (json.dumps(configuration_value, ensure_ascii=False, sort_keys=True, indent=2) + "\n").encode("utf-8"),
    )
    staging_descriptor = native_response(*native_invoke(
        "__source-evidence", {
            "operation": 'staging-create',
            "write": True,
            "arguments": {
                'coordinator': str(backend),
                'execution': str(execution),
                'directory': str(directory),
                'coordinator_source': coordinator_source,
                'execution_sha': execution_sha,
                'tool': tool,
            },
        }, subprocess.run, may_write=True,
    ))[0]["value"]
    staging = _source_policy(
        'staging-verify',
        coordinator=str(backend),
        execution=str(execution),
        directory=str(directory),
        descriptor=staging_descriptor,
        coordinator_source=coordinator_source,
        execution_sha=execution_sha,
        tool=tool,
    )
    source, argv = _producer_command(
        Path(staging["root"]), backend, execution, directory, operation_id,
        node.resolve(strict=True), start, lineage, configuration_path,
    )
    native_environment = _native_environment(environment)
    inputs = [
        row for row in _evidence_files(directory)
        if row["path"] not in PRODUCER_FILES
    ]
    _verify_inputs(directory, inputs)
    intent = {
        "format_version": 2,
        "kind": "restore-source-producer-intent",
        "operation_id": operation_id,
        "controller": controller,
        "source_generation": start,
        "dataset_lineage": lineage,
        "executable": str(node.resolve(strict=True)),
        "source": binding(backend / "xtask/src/recovery/dataset_prepare/existing_native.rs"),
        "configuration": configuration_descriptor,
        "argv": argv,
        "cwd": str(backend),
        "environment_sha256": plan_hash(native_environment),
        "started_at": _timestamp(),
        "coordinator_source": coordinator_source,
        "execution_sha": execution_sha,
        "staging": staging_descriptor,
        "inputs": inputs,
    }
    write_json(directory / INTENT, intent)
    process = None
    identity = None
    try:
        require_current_execution_source(backend, coordinator_source)
        _source_policy(
            'staging-verify',
            coordinator=str(backend),
            execution=str(execution),
            directory=str(directory),
            descriptor=staging_descriptor,
            coordinator_source=coordinator_source,
            execution_sha=execution_sha,
            tool=tool,
        )
        process = popen(
            argv,
            cwd=backend,
            env=native_environment,
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            creationflags=subprocess.CREATE_NO_WINDOW if os.name == "nt" else 0,
        )
        identity = process_identity(process.pid)
        if identity is None or Path(identity["executable"]).resolve() != node.resolve(strict=True):
            raise ValueError("来源验收原生生产者在登记创建身份前退出或可执行文件不同")
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
        ready_raw = _ready_line(process, timeout)
        ready_binding = _write_bytes(directory / READY, ready_raw)
        _verify_ready(ready_raw, operation_id, start, lineage, staging)
        require_current_execution_source(backend, coordinator_source)
        _source_policy(
            'staging-verify',
            coordinator=str(backend),
            execution=str(execution),
            directory=str(directory),
            descriptor=staging_descriptor,
            coordinator_source=coordinator_source,
            execution_sha=execution_sha,
            tool=tool,
        )
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
            "ready": ready_binding,
            "stdout": stdout_binding,
            "stderr": stderr_binding,
            "process": identity,
            "exit_code": process.returncode,
            "completed_at": _timestamp(),
        }
        write_json(directory / COMPLETION, completion)
        require_current_execution_source(backend, coordinator_source)
        _source_policy(
            'staging-verify',
            coordinator=str(backend),
            execution=str(execution),
            directory=str(directory),
            descriptor=staging_descriptor,
            coordinator_source=coordinator_source,
            execution_sha=execution_sha,
            tool=tool,
        )
        if process.returncode != 0 or stderr or process_identity(identity["pid"]) == identity:
            raise ValueError("来源验收原生生产者失败、写入 stderr 或尚未退出")
        stdout_value = decode_object(stdout.strip(), "来源验收原生生产者 stdout")
        return stdout_value, producer
    except BaseException as error:
        if identity is not None and process_identity(identity["pid"]) == identity:
            try:
                terminate_owned_process(identity, crash=True)
                if process is not None:
                    process.wait(timeout=5)
            except BaseException as cleanup:
                error.add_note("来源验收原生生产者精确身份回收失败：" + type(cleanup).__name__)
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
    execution_sha: str,
    historical_coordinator: bool = False,
) -> tuple[dict, dict]:
    """只读复核已登记生产者与同一 start、血缘、源码和环境的绑定。"""
    intent = _read_bound(backend, binding(directory / INTENT), directory / INTENT)
    if historical_coordinator:
        verify_execution_source(coordinator_source, "历史来源验收协调器")
        if coordinator_source["snapshot"]["clean"] is not True:
            raise ValueError("历史来源验收协调器不是已登记干净来源")
    else:
        require_current_execution_source(backend, coordinator_source)
    tool_snapshot = artifact_snapshot(node.resolve(strict=True)).descriptor()
    tool = {key: tool_snapshot[key] for key in ("path", "sha256")}
    staging = _source_policy(
        'staging-verify',
        coordinator=str(backend),
        execution=str(execution),
        directory=str(directory),
        descriptor=intent['staging'],
        coordinator_source=coordinator_source,
        execution_sha=execution_sha,
        tool=tool,
    )
    _verify_inputs(directory, intent["inputs"])
    producer = _read_bound(backend, binding(directory / PROCESS), directory / PROCESS)
    operation = intent["operation_id"]
    node = node.resolve(strict=True)
    configuration = bound_file(backend, intent["configuration"])
    if configuration != directory / CONFIGURATION:
        raise ValueError("来源原生配置绑定不属于同一verification目录")
    from restore_reference_plan import configuration_binding
    expected_configuration = configuration_binding(execution, execution / "config", "test")
    _source, argv = _producer_command(
        Path(staging["root"]), backend, execution, directory, operation, node, start, lineage,
        configuration,
    )
    verified = _runtime_policy(
        "producer-registration", backend=str(backend), directory=str(directory), start=start,
        lineage=lineage, tool=str(node), argv=argv,
        environment_sha256=plan_hash(_native_environment(environment)),
        coordinator_source=coordinator_source, execution_sha=execution_sha,
        configuration=expected_configuration,
    )
    if verified != {"intent": intent, "producer": producer}:
        raise ValueError("来源验收生产者在登记复核期间变化")
    if historical_coordinator:
        verify_execution_source(coordinator_source, "历史来源验收协调器")
    else:
        require_current_execution_source(backend, coordinator_source)
    _source_policy(
        'staging-verify',
        coordinator=str(backend),
        execution=str(execution),
        directory=str(directory),
        descriptor=intent['staging'],
        coordinator_source=coordinator_source,
        execution_sha=execution_sha,
        tool=tool,
    )
    return intent, producer


def _completion(backend: Path, directory: Path, intent: dict, producer: dict) -> dict:
    return _runtime_policy(
        "producer-completion", backend=str(backend), directory=str(directory),
        intent=intent, producer=producer,
    )


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
    execution_sha: str,
    historical_coordinator: bool = False,
) -> tuple[dict, dict]:
    """只读复核生产者身份、实际参数、stdout/stderr 和自然退出。"""
    intent, producer = verify_registered_source_producer(
        backend, execution, directory, start, lineage, environment, node,
        coordinator_source=coordinator_source, execution_sha=execution_sha,
        historical_coordinator=historical_coordinator,
    )
    tool_snapshot = artifact_snapshot(node.resolve(strict=True)).descriptor()
    tool = {key: tool_snapshot[key] for key in ("path", "sha256")}
    staging = _source_policy(
        'staging-verify',
        coordinator=str(backend),
        execution=str(execution),
        directory=str(directory),
        descriptor=intent['staging'],
        coordinator_source=coordinator_source,
        execution_sha=execution_sha,
        tool=tool,
    )
    _verify_ready(
        (directory / READY).read_bytes(), intent["operation_id"], start, lineage, staging
    )
    completion = _completion(backend, directory, intent, producer)
    identity = producer["process"]
    if (
        completion["exit_code"] != 0
        or (directory / STDERR).read_bytes() != b""
        or process_identity(identity["pid"]) == identity
    ):
        raise ValueError("来源验收生产者收据、退出状态或 stderr 不可信")
    stdout = decode_object((directory / STDOUT).read_bytes().strip(), "来源验收原生生产者 stdout")
    return producer, stdout


def require_source_verifier_stopped(backend: Path, start: dict) -> dict:
    """供 generation recover 只读使用；不猜测、扫描或跨职责终止进程。"""
    from devex_clone_seed_generation import verify_historical_running_source
    from restore_source_runtime import RECOVERY_AUDIT

    facts = verify_historical_running_source(backend, start, live=False)
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
    allowed = PRODUCER_FILES | {
        "before", "after", "audit", "source-tools", "cache-cleanup.json",
        "source-runtime.json", "failed.json", "source-verification-recovery-intent.json",
        "source-verification-recovery.json", RECOVERY_AUDIT,
    }
    if names - allowed or PROCESS not in names or INTENT not in names:
        raise ValueError("来源验收缺少已登记进程身份或包含未知证据")
    from process_environment import configured
    environment = configured(facts["environment"]["environment"])
    stored_intent = read_json(directory / INTENT)
    node = Path(stored_intent["executable"]).resolve(strict=True)
    intent, producer = verify_registered_source_producer(
        backend,
        facts["execution"],
        directory,
        start,
        facts["receipt"]["dataset_lineage"],
        environment,
        node,
        coordinator_source=facts["coordinator_source"],
        execution_sha=facts["request"]["expected_backend_sha"],
        historical_coordinator=True,
    )
    if READY in names:
        tool_snapshot = artifact_snapshot(node).descriptor()
        tool = {key: tool_snapshot[key] for key in ("path", "sha256")}
        staging = _source_policy(
            'staging-verify',
            coordinator=str(backend),
            execution=str(facts['execution']),
            directory=str(directory),
            descriptor=intent['staging'],
            coordinator_source=facts['coordinator_source'],
            execution_sha=facts['request']['expected_backend_sha'],
            tool=tool,
        )
        _verify_ready(
            (directory / READY).read_bytes(), intent["operation_id"], start,
            facts["receipt"]["dataset_lineage"], staging,
        )
    identity = producer["process"]
    if process_identity(identity["pid"]) == identity:
        raise ValueError("来源验收原生生产者仍在运行，generation recover 必须失败关闭")
    if "source-runtime.json" in names:
        from restore_source_runtime import verify_source_runtime

        verified = verify_source_runtime(backend, binding(directory / "source-runtime.json"), live=False)
        if verified["receipt"]["source_generation"] != start:
            raise ValueError("来源验收完成收据不属于当前 source generation")
        return {"status": "verified_stopped", "process": identity}
    if (
        names & {STDOUT, STDERR, COMPLETION} and READY not in names
        or STDERR in names and STDOUT not in names
        or COMPLETION in names and not {STDOUT, STDERR}.issubset(names)
    ):
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
