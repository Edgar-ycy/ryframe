"""启动并核验唯一来源业务验收原生生产者；进程身份先于请求授权持久化。"""

from __future__ import annotations

from native_s3_client import _invoke as native_invoke, _read_response as native_response

from collections import Counter
import datetime as dt
import hashlib
import json
import os
from pathlib import Path
import queue
import re
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
    process_identity_record,
    timestamp,
)


INTENT = "source-producer-intent.json"
PROCESS = "source-producer.json"
READY = "source-producer-ready.json"
STDOUT = "source-producer.stdout.json"
STDERR = "source-producer.stderr.log"
COMPLETION = "source-producer-completion.json"
CONFIGURATION = "source-producer-configuration.json"
PRODUCER_FILES = {INTENT, PROCESS, READY, STDOUT, STDERR, COMPLETION, CONFIGURATION}
INTENT_FIELDS = {
    "format_version", "kind", "operation_id", "controller", "source_generation",
    "dataset_lineage", "executable", "source", "configuration", "argv", "cwd", "environment_sha256",
    "started_at", "coordinator_source", "execution_sha", "staging", "inputs",
}
PROCESS_FIELDS = {
    "format_version", "kind", "operation_id", "intent", "controller", "process",
    "executable", "argv",
}
COMPLETION_FIELDS = {
    "format_version", "kind", "operation_id", "intent", "producer", "ready", "stdout", "stderr",
    "process", "exit_code", "completed_at",
}
READY_FIELDS = {
    "format_version", "kind", "operation_id", "source_generation_sha256",
    "staging_manifest_sha256", "runtime_files_sha256", "contract_file_sha256",
    "lineage_file_sha256",
}


def native_tool() -> Path:
    """读取协调器安装的原生工具绑定；不从 PATH 或环境猜测。"""
    from rate_limit_tool import tool_binding

    value = tool_binding()
    if value is None:
        raise ValueError("来源验收缺少协调器原生工具绑定")
    from devex_clone_target_binding import external_file

    external_file(value)
    return Path(value["path"]).resolve(strict=True)


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
    from restore_source_runtime import RECOVERY_AUDIT, _manifest
    from devex_clone_model import linked

    rows = []
    for path in directory.iterdir():
        if linked(path):
            raise ValueError("来源验收中间证据包含链接或重解析点")
        if path.is_dir():
            if path.name not in {"before", "after", "audit", "source-tools", RECOVERY_AUDIT}:
                raise ValueError("来源验收中间证据包含未知目录")
            # 采集器可能在创建阶段目录后中断；内部未知或空嵌套目录仍由唯一 manifest 拒绝。
            if any(path.iterdir()):
                rows.extend({**row, "path": path.name + "/" + row["path"]} for row in _manifest(path))
        else:
            snapshot = artifact_snapshot(path)
            rows.append({"path": path.name, "bytes": snapshot.bytes, "sha256": snapshot.sha256})
    return sorted(rows, key=lambda row: row["path"])


def _query_audit_paths(directory: Path, inputs: list[dict], observed: list[dict]) -> set[str]:
    """后续与恢复审计只复核同一控制库；不接受任意新增文件。"""
    from restore_source_runtime import RECOVERY_AUDIT, RECOVERY_INTENT

    if (directory / RECOVERY_AUDIT).exists() and not (directory / RECOVERY_INTENT).is_file():
        raise ValueError("恢复登录审计必须继承明确恢复 intent")
    pattern = r"mysql-shared-control-[a-f0-9]{32}\.command\.json"
    candidates = [row for row in observed if re.fullmatch("audit/" + pattern, row["path"])
                  or row["path"].startswith(RECOVERY_AUDIT + "/")]
    if not candidates:
        return set()
    originals = [row for row in inputs if re.fullmatch("audit/" + pattern, row["path"])]
    commands = [decode_object((directory / row["path"]).read_bytes(), "原控制库查询记录")["command"]
                for row in originals]
    if not commands or any(command != commands[0] for command in commands):
        raise ValueError("恢复登录审计缺少原同一控制库查询命令")
    allowed = set()
    for row in candidates:
        if re.fullmatch("(?:audit|" + RECOVERY_AUDIT + ")/" + pattern, row["path"]) is None:
            raise ValueError("恢复登录审计包含未知文件或子目录")
        value = decode_object((directory / row["path"]).read_bytes(), "恢复控制库查询记录")
        exact_fields(value, {"command", "returncode", "error_type", "stdout", "stderr"}, "恢复控制库查询记录")
        if (value["command"] != commands[0]
                or not isinstance(value["stdout"], str) or not isinstance(value["stderr"], str)
                or value["returncode"] is not None and type(value["returncode"]) is not int
                or value["error_type"] is not None and not isinstance(value["error_type"], str)):
            raise ValueError("恢复登录审计改变了原查询命令或记录格式")
        allowed.add(row["path"])
    return allowed


def _capture_receipt_identity(path: Path, category: str) -> str:
    value = decode_object(path.read_bytes(), "完整像存储采集记录")
    if category == "storage-layout":
        fields = {"process_receipt", "launch_receipt", "actual_arguments_sha256", "data_dir", "environment_proof"}
        if "runtime_transition" in value:
            fields.add("runtime_transition")
        exact_fields(value, fields, "完整像存储布局记录")
    else:
        exact_fields(value, {"command", "returncode", "error_type", "stdout", "stderr"}, "完整像缓存身份命令")
        command = value["command"]
        if (not isinstance(command, list) or len(command) < 2
                or any(not isinstance(part, str) or not part for part in command)
                or type(value["returncode"]) is not int or value["returncode"] != 0
                or value["error_type"] is not None or value["stderr"] != ""
                or not isinstance(value["stdout"], str)):
            raise ValueError("完整像缓存身份采集命令未成功或字段无效")
        if command[-2] == "/usr/bin/cat" and re.fullmatch(r"/proc/[1-9][0-9]*/stat", command[-1]):
            process, closing, suffix = value["stdout"].strip().rpartition(")")
            fields = suffix.split()
            if (not closing or len(fields) < 20 or not fields[19].isdigit()
                    or not process.startswith(command[-1].split("/")[2] + " (")):
                raise ValueError("完整像缓存内核身份输出不完整")
            # CPU 等运行计数自然变化，进程号、名称及创建时刻必须保持。
            value["stdout"] = {"process": process + closing, "started": fields[19]}
    return json.dumps(value, ensure_ascii=False, sort_keys=True, separators=(",", ":"))


def _image_capture_paths(directory: Path, inputs: list[dict], observed: list[dict]) -> set[str]:
    """采集器每次使用新 UUID；按原类别、命令与次数核验同一像的有限动态记录。"""
    patterns = {"redis-kernel": r"redis-kernel-[a-f0-9]{32}\.command\.json",
                "storage-layout": r"storage-layout-[a-f0-9]{32}\.json"}
    allowed = set()
    complete = any(row["path"] == "after/image.json" for row in observed)
    for category, pattern in patterns.items():
        original = [row for row in inputs if re.fullmatch("before/" + pattern, row["path"])]
        current = [row for row in observed if re.fullmatch("after/" + pattern, row["path"])]
        if not original and not current:
            continue
        if len(original) != {"redis-kernel": 10, "storage-layout": 2}[category]:
            raise ValueError("完整像动态采集记录缺少原始完整类别与数量")
        expected = Counter(_capture_receipt_identity(directory / row["path"], category) for row in original)
        actual = Counter(_capture_receipt_identity(directory / row["path"], category) for row in current)
        if actual - expected or complete and actual != expected:
            raise ValueError("完整像动态采集记录改变了原类别、命令、身份或数量")
        allowed.update(row["path"] for row in current)
    return allowed


def _database_capture_identity(path: Path, category: str) -> tuple[str, str | None]:
    value = decode_object(path.read_bytes(), "完整像数据库采集记录")
    stdout = value.get("stdout_file")
    stdout_path = None
    if stdout is not None:
        exact_fields(stdout, {"path", "bytes", "sha256"}, "完整像数据库标准输出")
        stdout_path = Path(stdout["path"])
        match = re.fullmatch(r"mysql-(identity|ownership)-[a-f0-9]{32}\.stdout", stdout_path.name)
        if (stdout_path.parent != path.parent or match is None
                or artifact_snapshot(stdout_path).descriptor() != stdout):
            raise ValueError("完整像数据库标准输出未绑定同目录明确采集文件")
        value["stdout_file"] = {**stdout, "path": "mysql-" + match[1] + "-UUID.stdout"}
    if category == "command":
        fields = {"command", "returncode", "error_type", "stdin", "stdout", "stderr"}
        if stdout is not None:
            fields.add("stdout_file")
        exact_fields(value, fields, "完整像数据库命令")
        command = value["command"]
        if (not isinstance(command, list) or not command
                or any(not isinstance(part, str) or not part for part in command)
                or not isinstance(value["stdout"], str) or value["stderr"] != ""):
            raise ValueError("完整像数据库命令格式无效")
        if "--output" in command:
            index = command.index("--output") + 1
            output = Path(command[index]) if index < len(command) else Path()
            if (output.parent != path.parent or re.fullmatch(
                    r"(?:before|after)-target-(?:shared-control|shared|dedicated-a|dedicated-b)\.json", output.name) is None):
                raise ValueError("完整像数据库命令输出越出明确目标目录")
            command[index] = "<databases>/" + output.name
    else:
        exact_fields(value, {"format_version", "kind", "check", "target_key", "returncode",
                            "error_type", "stderr_bytes", "stderr_sha256", "stdout_file"}, "完整像数据库验证输出")
        if (type(value["format_version"]) is not int or value["format_version"] != 1
                or value["kind"] != "mysql-verification-output" or value["check"] != category
                or value["target_key"] not in {"shared-control", "shared", "dedicated-a", "dedicated-b"}
                or type(value["stderr_bytes"]) is not int or value["stderr_bytes"] != 0
                or value["stderr_sha256"] != hashlib.sha256(b"").hexdigest()
                or stdout_path != path.with_suffix(".stdout")):
            raise ValueError("完整像数据库验证输出类别、目标或错误状态无效")
    if type(value["returncode"]) is not int or value["returncode"] != 0 or value["error_type"] is not None:
        raise ValueError("完整像数据库采集未成功")
    return json.dumps(value, ensure_ascii=False, sort_keys=True, separators=(",", ":")), None if stdout_path is None else stdout_path.name


def _database_capture_paths(directory: Path, inputs: list[dict], observed: list[dict]) -> set[str]:
    patterns = {"command": (r"command-[a-f0-9]{32}\.json", 48),
                "identity": (r"mysql-identity-[a-f0-9]{32}\.json", 16),
                "ownership": (r"mysql-ownership-[a-f0-9]{32}\.json", 24)}
    complete = any(row["path"] == "after/image.json" for row in observed)
    allowed = set()
    references = {}
    for category, (pattern, count) in patterns.items():
        original = [row for row in inputs if re.fullmatch("before/databases/" + pattern, row["path"])]
        current = [row for row in observed if re.fullmatch("after/databases/" + pattern, row["path"])]
        if not original and not current:
            continue
        if len(original) != count:
            raise ValueError("完整像数据库动态采集缺少原始完整类别与数量")
        expected = Counter(_database_capture_identity(directory / row["path"], category)[0] for row in original)
        actual = Counter()
        references[category] = Counter()
        for row in current:
            identity, stdout = _database_capture_identity(directory / row["path"], category)
            actual[identity] += 1
            allowed.add(row["path"])
            if stdout is not None:
                allowed.add("after/databases/" + stdout)
                references[category][stdout] += 1
        if actual - expected or complete and actual != expected:
            raise ValueError("完整像数据库动态采集改变了原命令、响应或数量")
    commands = references.get("command", Counter())
    diagnostics = references.get("identity", Counter()) + references.get("ownership", Counter())
    if (any(count != 1 for count in commands.values()) or any(count != 1 for count in diagnostics.values())
            or complete and commands != diagnostics):
        raise ValueError("完整像数据库命令与诊断未一一绑定同一标准输出文件")
    for category, count in (("identity", 16), ("ownership", 24)):
        pattern = "after/databases/mysql-" + category + r"-[a-f0-9]{32}\.stdout"
        current = {row["path"] for row in observed if re.fullmatch(pattern, row["path"])}
        referenced = {path for path in allowed if re.fullmatch(pattern, path)}
        if current != referenced or complete and current and len(current) != count:
            raise ValueError("完整像数据库动态标准输出集合或数量不完整")
    return allowed


def _verify_inputs(directory: Path, inputs: object) -> list[dict]:
    """入口授权前的采集文件保持不可变，后续只允许协议已有的精确文件集合。"""
    if not isinstance(inputs, list):
        raise ValueError("来源生产者缺少授权前证据清单")
    previous = ""
    for row in inputs:
        exact_fields(row, {"path", "bytes", "sha256"}, "来源生产者授权前证据")
        if (not isinstance(row["path"], str) or row["path"] <= previous
                or not row["path"].startswith(("before/", "audit/", "source-tools" + "/"))):
            raise ValueError("来源生产者授权前证据路径不属于固定采集阶段")
        previous = row["path"]
    observed = _evidence_files(directory)
    by_path = {row["path"]: row for row in observed}
    if any(by_path.get(row["path"]) != row for row in inputs):
        raise ValueError("来源生产者授权前完整证据已变化")
    allowed = {row["path"] for row in inputs} | PRODUCER_FILES | {
        "failed.json", "cache-cleanup.json", "source-runtime.json",
        "source-verification-recovery-intent.json", "source-verification-recovery.json",
        "audit/login-after-old.tsv", "audit/login-after-new.tsv", "audit/login-audit.json"}
    allowed.update("after/" + row["path"].removeprefix("before/") for row in inputs if row["path"].startswith("before/"))
    allowed.update(_query_audit_paths(directory, inputs, observed))
    allowed.update(_image_capture_paths(directory, inputs, observed))
    allowed.update(_database_capture_paths(directory, inputs, observed))
    if set(by_path) - allowed:
        raise ValueError("来源验收中间证据包含未登记文件")
    return observed


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
    manifest = (staging / "manifest.json").resolve(strict=True)
    contract = (staging / "openapi/openapi.json").resolve(strict=True)
    arguments = [
        str(tool),
        "__recovery-existing-source",
        "--backend-dir", str(backend),
        "--lineage", lineage["path"],
        "--contract", str(contract),
        "--staging-manifest", str(manifest),
        "--configuration", str(configuration),
        "--configuration-root", str(execution),
        "--run-dir", str(directory),
        "--operation-id", operation_id,
        "--source-generation-sha256", start["sha256"],
        "--write",
    ]
    return manifest, arguments


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
    value = decode_object(raw.strip(), "来源验收原生生产者 ready")
    exact_fields(value, READY_FIELDS, "来源验收原生生产者 ready")
    contract = next(
        (row for row in staging["manifest"]["files"] if row["path"] == staging["manifest"]["contract"]),
        None,
    )
    modules = staging["manifest"]["runtime_modules"]
    expected = {
        "format_version": 1,
        "kind": "restore-source-producer-ready",
        "operation_id": operation,
        "source_generation_sha256": start["sha256"],
        "staging_manifest_sha256": staging["descriptor"]["sha256"],
        "runtime_files_sha256": plan_hash(modules),
        "contract_file_sha256": None if contract is None else contract["sha256"],
        "lineage_file_sha256": lineage["sha256"],
    }
    if type(value["format_version"]) is not int or value != expected:
        raise ValueError("来源验收原生 ready 未绑定工具、契约、血缘或运行身份")
    return value


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
    staging = native_response(*native_invoke(
        "__source-evidence", {
            "operation": 'staging-verify',
            "write": False,
            "arguments": {
                'coordinator': str(backend),
                'execution': str(execution),
                'directory': str(directory),
                'descriptor': staging_descriptor,
                'coordinator_source': coordinator_source,
                'execution_sha': execution_sha,
                'tool': tool,
            },
        }, subprocess.run, may_write=False,
    ))[0]["value"]
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
        native_response(*native_invoke(
            "__source-evidence", {
                "operation": 'staging-verify',
                "write": False,
                "arguments": {
                    'coordinator': str(backend),
                    'execution': str(execution),
                    'directory': str(directory),
                    'descriptor': staging_descriptor,
                    'coordinator_source': coordinator_source,
                    'execution_sha': execution_sha,
                    'tool': tool,
                },
            }, subprocess.run, may_write=False,
        ))[0]["value"]
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
        native_response(*native_invoke(
            "__source-evidence", {
                "operation": 'staging-verify',
                "write": False,
                "arguments": {
                    'coordinator': str(backend),
                    'execution': str(execution),
                    'directory': str(directory),
                    'descriptor': staging_descriptor,
                    'coordinator_source': coordinator_source,
                    'execution_sha': execution_sha,
                    'tool': tool,
                },
            }, subprocess.run, may_write=False,
        ))[0]["value"]
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
        native_response(*native_invoke(
            "__source-evidence", {
                "operation": 'staging-verify',
                "write": False,
                "arguments": {
                    'coordinator': str(backend),
                    'execution': str(execution),
                    'directory': str(directory),
                    'descriptor': staging_descriptor,
                    'coordinator_source': coordinator_source,
                    'execution_sha': execution_sha,
                    'tool': tool,
                },
            }, subprocess.run, may_write=False,
        ))[0]["value"]
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
    exact_fields(intent, INTENT_FIELDS, "来源验收生产者 intent")
    tool_snapshot = artifact_snapshot(node.resolve(strict=True)).descriptor()
    tool = {key: tool_snapshot[key] for key in ("path", "sha256")}
    staging = native_response(*native_invoke(
        "__source-evidence", {
            "operation": 'staging-verify',
            "write": False,
            "arguments": {
                'coordinator': str(backend),
                'execution': str(execution),
                'directory': str(directory),
                'descriptor': intent["staging"],
                'coordinator_source': coordinator_source,
                'execution_sha': execution_sha,
                'tool': tool,
            },
        }, subprocess.run, may_write=False,
    ))[0]["value"]
    _verify_inputs(directory, intent["inputs"])
    producer = _read_bound(backend, binding(directory / PROCESS), directory / PROCESS)
    exact_fields(producer, PROCESS_FIELDS, "来源验收生产者")
    operation = _validate_operation(intent["operation_id"])
    node = node.resolve(strict=True)
    configuration = bound_file(backend, intent["configuration"])
    if configuration != directory / CONFIGURATION:
        raise ValueError("来源原生配置绑定不属于同一verification目录")
    from restore_reference_plan import configuration_binding
    expected_configuration = configuration_binding(execution, execution / "config", "test")
    if read_json(configuration) != expected_configuration:
        raise ValueError("来源原生配置文件绑定发生变化")
    _source, argv = _producer_command(
        Path(staging["root"]), backend, execution, directory, operation, node, start, lineage,
        configuration,
    )
    expected = {
        "format_version": 2,
        "kind": "restore-source-producer-intent",
        "operation_id": operation,
        "source_generation": start,
        "dataset_lineage": lineage,
        "executable": str(node),
        "source": intent["source"],
        "configuration": intent["configuration"],
        "argv": argv,
        "cwd": str(backend),
        "environment_sha256": plan_hash(_native_environment(environment)),
        "coordinator_source": coordinator_source,
        "execution_sha": execution_sha,
        "staging": intent["staging"],
    }
    if any(intent.get(key) != value for key, value in expected.items()):
        raise ValueError("来源验收生产者 intent 没有绑定同一 start、血缘、源码或环境")
    controller = process_identity_record(intent["controller"], "来源验收控制器身份")
    identity = process_identity_record(producer["process"], "来源验收原生生产者身份")
    if (
        type(intent["format_version"]) is not int
        or intent["format_version"] != 2
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
    if historical_coordinator:
        verify_execution_source(coordinator_source, "历史来源验收协调器")
    else:
        require_current_execution_source(backend, coordinator_source)
    native_response(*native_invoke(
        "__source-evidence", {
            "operation": 'staging-verify',
            "write": False,
            "arguments": {
                'coordinator': str(backend),
                'execution': str(execution),
                'directory': str(directory),
                'descriptor': intent["staging"],
                'coordinator_source': coordinator_source,
                'execution_sha': execution_sha,
                'tool': tool,
            },
        }, subprocess.run, may_write=False,
    ))[0]["value"]
    return intent, producer


def _completion(backend: Path, directory: Path, intent: dict, producer: dict) -> dict:
    completion = _read_bound(backend, binding(directory / COMPLETION), directory / COMPLETION)
    exact_fields(completion, COMPLETION_FIELDS, "来源验收生产者 completion")
    if (type(completion["format_version"]) is not int or type(completion["exit_code"]) is not int
            or completion != {
                "format_version": 1, "kind": "restore-source-producer-completion",
                "operation_id": intent["operation_id"], "intent": binding(directory / INTENT),
                "producer": binding(directory / PROCESS),
                "ready": artifact_snapshot(directory / READY).descriptor(),
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
    staging = native_response(*native_invoke(
        "__source-evidence", {
            "operation": 'staging-verify',
            "write": False,
            "arguments": {
                'coordinator': str(backend),
                'execution': str(execution),
                'directory': str(directory),
                'descriptor': intent["staging"],
                'coordinator_source': coordinator_source,
                'execution_sha': execution_sha,
                'tool': tool,
            },
        }, subprocess.run, may_write=False,
    ))[0]["value"]
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
        staging = native_response(*native_invoke(
            "__source-evidence", {
                "operation": 'staging-verify',
                "write": False,
                "arguments": {
                    'coordinator': str(backend),
                    'execution': str(facts["execution"]),
                    'directory': str(directory),
                    'descriptor': intent["staging"],
                    'coordinator_source': facts["coordinator_source"],
                    'execution_sha': facts["request"]["expected_backend_sha"],
                    'tool': tool,
                },
            }, subprocess.run, may_write=False,
        ))[0]["value"]
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
