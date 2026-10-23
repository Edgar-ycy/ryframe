"""将新目标严格绑定到已审阅单侧资源、实际配置和三份维护工具。"""
from __future__ import annotations

from native_s3_client import (NativeS3InvocationError, _invoke as native_invoke,
                              _read_response as native_response)
import subprocess

from collections import Counter
import hashlib
import os
from pathlib import Path
import re

from artifact_digests import filesystem_path
from devex_clone import read_json
from devex_clone_capture import read_bound_json
from devex_clone_model import digest, exact, linked, local_path
from devex_clone_inventory import configuration as inventory_configuration
from devex_clone_run_state import binding
from devex_clone_source_proof import bound_file, require_closed_port, verify_api_address
from devex_clone_tools import verify as verify_tools
from full_stack_runtime import configuration_digest, worker_ready_url
from rate_limit_client import load_app_table
from restore_build import file_digest
from restore_reference_plan import BUCKETS, plan_hash
from source_inventory import snapshot

KEYS = {"shared-control": ("combined", "shared"), "shared": ("tenant", "shared"),
        "dedicated-a": ("tenant", "dedicated"), "dedicated-b": ("tenant", "dedicated")}
EXCLUSIVE = {"mysql_exclusive", "redis_exclusive", "object_storage_exclusive"}
REVIEW_FILE_TOOLS = ("mysql", "mysqldump", "rustfs")
REVIEW_SUPERVISOR_TOOLS = ("redis_server", "wsl", "redis_python")
REVIEW_TOOLS = (*REVIEW_FILE_TOOLS, *REVIEW_SUPERVISOR_TOOLS)
_UUID = r"[a-f0-9]{32}"
_REGISTRATION_FIELDS = {"format_version", "kind", "request", "environment", "storage_run",
                        "target_directory"}


def target_files(target: Path, *, ignored: set[str] | None = None,
                 locked_guard: bool = False, initialize_lock_released: bool = False) -> list[dict]:
    """返回普通目录树的稳定摘要；任何链接、重解析点或特殊文件均拒绝。"""
    if initialize_lock_released and not locked_guard:
        raise ValueError("只有持有目标 guard 时才能核对已释放的初始化锁")
    if locked_guard:
        from devex_clone_target_state import generation_checkpoint, generation_guard_checkpoint

        checkpoint = generation_guard_checkpoint if initialize_lock_released else generation_checkpoint
        checkpoint(target)
    ignored = set() if ignored is None else ignored
    if linked(target) or not target.is_dir():
        raise ValueError("fresh 目标根目录缺失或经过链接")
    entries = []
    pending = [target]
    while pending:
        directory = pending.pop()
        for path in directory.iterdir():
            relative = path.relative_to(target).as_posix()
            if linked(path):
                raise ValueError("fresh 目标阶段文件经过链接")
            native = filesystem_path(path)
            if relative in ignored:
                # 控制互斥以文件或含 owner 记录的目录形式存在，均不属于快照集合。
                continue
            item = {"path": relative}
            if relative == "copy-target.guard" and locked_guard:
                expected = {**item, "type": "file", "bytes": 1,
                            "sha256": hashlib.sha256(b"\0").hexdigest()}
                # generation_checkpoint 已经通过持锁句柄核对同一文件的精确 NUL 字节；
                # Windows 不允许再次打开被 msvcrt 锁定的字节区间。
                if not os.path.isfile(native) or os.stat(native).st_size != 1:
                    raise ValueError("fresh 目标已持有的控制互斥文件无效")
                entries.append(expected)
                continue
            if os.path.isdir(native):
                entries.append({**item, "type": "directory"})
                pending.append(path)
            elif os.path.isfile(native):
                try:
                    digest_value = file_digest(path)
                except OSError as error:
                    raise ValueError("fresh 目标阶段文件无法安全读取：" + relative) from error
                entries.append({**item, "type": "file", **digest_value})
            else:
                raise ValueError("fresh 目标阶段包含未知文件类型")
    result = sorted(entries, key=lambda item: item["path"])
    if locked_guard:
        checkpoint(target)
    return result


def _file_map(files: list[dict]) -> dict[str, dict]:
    if not isinstance(files, list):
        raise ValueError("fresh 目标文件快照不是列表")
    result = {}
    for item in files:
        if not isinstance(item, dict) or item.get("type") not in {"file", "directory"}:
            raise ValueError("fresh 目标文件快照条目无效")
        fields = {"path", "type"} | ({"bytes", "sha256"} if item["type"] == "file" else set())
        path = item.get("path")
        if (set(item) != fields or not isinstance(path, str) or not path
                or "\\" in path or Path(path).is_absolute()
                or any(part in {"", ".", ".."} for part in path.split("/"))
                or path in result):
            raise ValueError("fresh 目标文件快照路径重复、越界或字段无效")
        if item["type"] == "file" and (type(item["bytes"]) is not int or item["bytes"] < 0
                or not isinstance(item["sha256"], str)
                or re.fullmatch(r"[a-f0-9]{64}", item["sha256"]) is None):
            raise ValueError("fresh 目标文件快照摘要无效")
        result[path] = item
    return result


def prepared_target_files(backend: Path, target: Path, evidence: dict,
                          request_descriptor: dict) -> list[dict]:
    """重读 CLI 发布的完整 prepare 快照，并绑定 registration 与空 predecessor。"""
    exact(evidence, {"descriptor", "registration", "predecessor"})
    descriptor = evidence["descriptor"]
    path = bound_file(backend, descriptor)
    snapshot = read_bound_json(path, descriptor)
    exact(snapshot, {"format_version", "kind", "registration", "predecessor", "files"})
    registration_path = bound_file(backend, evidence["registration"])
    registration = read_bound_json(registration_path, evidence["registration"])
    exact(registration, _REGISTRATION_FIELDS)
    if (path.name != "prepared-files.json" or registration_path != path.parent / "registration.json"
            or snapshot["format_version"] != 1
            or snapshot["kind"] != "devex-clone-fresh-target-prepared-files"
            or snapshot["registration"] != evidence["registration"]
            or snapshot["predecessor"] is not None or evidence["predecessor"] is not None
            or registration["format_version"] != 1
            or registration["kind"] != "devex-clone-fresh-target-registration"
            or registration["request"] != request_descriptor
            or local_path(backend, registration["target_directory"]) != target
            or path.parent != target.parent):
        raise ValueError("fresh 目标 prepare 文件快照、registration 或 predecessor 绑定无效")
    _file_map(snapshot["files"])
    from devex_clone_target_prepared_files import validate_prepared_tree

    request = read_bound_json(bound_file(backend, request_descriptor), request_descriptor)
    validate_prepared_tree(backend, target, snapshot["files"], request_descriptor, request)
    if binding(path) != descriptor or binding(registration_path) != evidence["registration"]:
        raise ValueError("fresh 目标 prepare 文件快照或 registration 在读取期间变化")
    return snapshot["files"]


def initialized_target_files(backend: Path, target: Path, *, locked_guard: bool = False) -> dict:
    """验证正式消费者所见完整树及 initialized→prepared→registration 谱系。"""
    workspace = target.parent
    initialized_path = workspace / "initialized-files.json"
    prepared_path = workspace / "prepared-files.json"
    registration_path = workspace / "registration.json"
    descriptors = {"initialized": binding(initialized_path),
                   "prepared": binding(prepared_path),
                   "registration": binding(registration_path)}
    initialized = read_bound_json(initialized_path, descriptors["initialized"])
    exact(initialized, {"format_version", "kind", "registration", "predecessor", "files"})
    invalid = []
    if initialized["format_version"] != 1:
        invalid.append("format_version")
    if initialized["kind"] != "devex-clone-fresh-target-initialized-files":
        invalid.append("kind")
    if initialized["registration"] != descriptors["registration"]:
        invalid.append("registration")
    if initialized["predecessor"] != descriptors["prepared"]:
        invalid.append("predecessor")
    if invalid:
        raise ValueError("fresh 目标 initialized 文件快照谱系无效：" + ", ".join(invalid))
    prepared = read_bound_json(prepared_path, descriptors["prepared"])
    evidence = {"descriptor": descriptors["prepared"],
                "registration": prepared.get("registration"),
                "predecessor": prepared.get("predecessor")}
    registration = read_bound_json(registration_path, descriptors["registration"])
    prepared_target_files(backend, target, evidence, registration["request"])
    _file_map(initialized["files"])
    if initialized["files"] != target_files(target, ignored={"initialize.lock"}
                                             if locked_guard else None,
                                             locked_guard=locked_guard) \
            or any(binding(path) != descriptors[key] for key, path in (
                ("initialized", initialized_path), ("prepared", prepared_path),
                ("registration", registration_path))):
        raise ValueError("fresh 目标 initialized 完整文件树或谱系在核验期间变化")
    return descriptors["initialized"]


def _successful_receipt(target: Path, relative: str) -> dict:
    value = read_json(target / relative)
    exact(value, {"command", "returncode", "error_type", "stdout", "stderr"})
    if (not isinstance(value["command"], list)
            or any(not isinstance(item, str) or not item for item in value["command"])
            or type(value["returncode"]) is not int or value["returncode"] != 0
            or value["error_type"] is not None or not isinstance(value["stdout"], str)
            or value["stderr"] != ""):
        raise ValueError("fresh 初始化只读观察命令收据无效")
    value["stdout"] = value["stdout"].replace("\r\n", "\n")
    return value


def _successful_object_receipt(target: Path, relative: str, operation: str,
                               request: dict) -> dict:
    value = read_json(target / relative)
    exact(value, {"transport", "operation", "bucket", "returncode", "error_type",
                  "response", "key", "destination", "remote_writes", "scope_id",
                  "endpoint", "configuration_sha256", "prefix", "continuation"})
    scope = request["target"]["scope_id"]
    configuration = request["target"]["s3"]
    if (value["transport"] != "rust-s3-sigv4" or value["operation"] != operation
            or value["bucket"] not in BUCKETS or value["returncode"] != 0
            or value["error_type"] is not None or not isinstance(value["response"], dict)
            or value["scope_id"] != scope or value["endpoint"] != configuration["endpoint"]
            or value["configuration_sha256"] != plan_hash(configuration)
            or value["prefix"] != (scope + "/" if operation == "list-objects-v2" else None)
            or value["continuation"] is not None
            or value["remote_writes"] != 0):
        raise ValueError("fresh 初始化原生对象只读观察收据无效")
    return value


def _stage_paths(values: set[str], stage: str) -> list[str]:
    pattern = re.compile(re.escape(stage) + rf"-{_UUID}\.command\.json")
    return sorted(path for path in values if pattern.fullmatch(path))


def _static_observations(target: Path, remaining: set[str], prepared: dict,
                         stage: str, repetitions: int, *, request: dict,
                         initialized_scope: str | None) -> None:
    operation = {"bucket-readiness": "head-bucket", "bucket-versioning": "get-bucket-versioning",
                 "scoped-list": "list-objects-v2"}[stage]
    baseline = [_successful_object_receipt(target, path, operation, request)
                for path in _stage_paths(set(prepared), stage)]
    current_paths = _stage_paths(remaining, stage)
    current = [_successful_object_receipt(target, path, operation, request)
               for path in current_paths]
    signature = lambda item: (item["operation"], item["bucket"], plan_hash(item["response"]))
    expected = Counter(signature(item) for item in baseline)
    actual = Counter(signature(item) for item in current)
    wanted = Counter({key: count * repetitions for key, count in expected.items()})
    initialized_lists = stage == "scoped-list" and initialized_scope is not None
    valid = len(baseline) == 5 and len(expected) == 5
    if valid and initialized_lists:
        responses = {item["bucket"]: item["response"] for item in baseline}
        grouped = {bucket: [item["response"] for item in current if item["bucket"] == bucket]
                   for bucket in responses}
        owner = initialized_scope + "/.ryframe-owner"
        for bucket, outputs in grouped.items():
            final = [value for value in outputs if value != responses[bucket]]
            page = final[0] if len(final) == 1 else None
            valid = (valid and len(outputs) == repetitions
                     and outputs.count(responses[bucket]) == repetitions - 1
                     and isinstance(page, dict) and page.get("IsTruncated") is False
                     and isinstance(page.get("Contents"), list) and len(page["Contents"]) == 1
                     and isinstance(page["Contents"][0], dict)
                     and page["Contents"][0].get("Key") == owner)
    elif valid:
        valid = actual == wanted
    if not valid:
        raise ValueError("fresh 初始化对象观察命令不是逐桶精确多重集："
                         f"stage={stage}, baseline={len(baseline)}/{len(expected)}, "
                         f"current={len(current)}/{len(actual)}, "
                         f"counts={sorted(actual.values())}/{sorted(wanted.values())}")
    remaining.difference_update(current_paths)


def _owner_observations(target: Path, remaining: set[str], prepared: dict,
                        request: dict, initialized_objects: dict) -> None:
    """绑定逐桶 owner 读取命令、落盘字节与 initialized 结果。"""
    baseline_paths = _stage_paths(set(prepared), "bucket-readiness")
    baseline = [_successful_object_receipt(target, path, "head-bucket", request)
                for path in baseline_paths]
    if len(baseline) != len(BUCKETS) or {item["bucket"] for item in baseline} != set(BUCKETS):
        raise ValueError("fresh 初始化对象基线未覆盖五桶")
    receipts = _stage_paths(remaining, "owner-read")
    if len(receipts) != len(BUCKETS) or not isinstance(initialized_objects, dict) \
            or set(initialized_objects) != set(BUCKETS):
        raise ValueError("fresh 初始化对象 owner 证据未完整覆盖五桶")
    scope = request["target"]["scope_id"]
    seen = set()
    consumed = set(receipts)
    for receipt_path in receipts:
        receipt = _successful_object_receipt(target, receipt_path, "get-object", request)
        bucket = receipt["bucket"]
        pattern = re.compile(r"owner-" + re.escape(bucket) + rf"-{_UUID}\.bin")
        body_name = receipt["destination"]
        if not isinstance(body_name, str):
            raise ValueError("fresh 初始化对象 owner 收据缺少输出文件")
        body = target / body_name
        owner_key = scope + "/.ryframe-owner"
        expected_bytes = f"ryframe-owner:v1:{scope}:object-storage:{bucket}".encode()
        expected_owner = {"bytes": len(expected_bytes),
                          "sha256": hashlib.sha256(expected_bytes).hexdigest()}
        if (bucket not in BUCKETS or bucket in seen or not pattern.fullmatch(body_name)
                or receipt["key"] != owner_key or body.read_bytes() != expected_bytes
                or initialized_objects[bucket] != {"keys": [owner_key], "owner": expected_owner}):
            raise ValueError("fresh 初始化对象 owner 命令、字节或结果无效")
        seen.add(bucket)
        consumed.add(body_name)
    if seen != set(BUCKETS):
        raise ValueError("fresh 初始化对象 owner 证据未完整覆盖五桶")
    remaining.difference_update(consumed)


def _mysql_observations(target: Path, remaining: set[str], prepared: dict,
                        request: dict) -> None:
    databases = {item["key"]: item for item in request["target"]["databases"]}
    for key, database in databases.items():
        baseline_paths = _stage_paths(set(prepared), "mysql-" + key)
        current_paths = _stage_paths(remaining, "mysql-" + key)
        if len(baseline_paths) != 1 or len(current_paths) != 9:
            raise ValueError("fresh 初始化 MySQL 观察收据数量无效")
        baseline = _successful_receipt(target, baseline_paths[0])
        current = [_successful_receipt(target, path) for path in current_paths]
        identity = database["server_uuid"] + "\n"
        exists = identity + database["database"] + "\n"
        if (baseline["stdout"] != identity
                or any(value["command"] != baseline["command"] for value in current)
                or Counter(value["stdout"] for value in current)
                != Counter({identity: 2, "": 4, exists: 3})):
            raise ValueError("fresh 初始化 MySQL 前后像命令或输出多重集无效："
                             + repr(Counter(value["stdout"] for value in current)))
        remaining.difference_update(current_paths)


def _process_stat_identity(value: str) -> tuple[str, str]:
    head, marker, fields = value.strip().rpartition(")")
    parts = fields.split()
    if not marker or not head or len(parts) < 20 or not parts[19].isdigit():
        raise ValueError("fresh 初始化 Redis 内核身份输出无效")
    return head + marker, parts[19]


def _redis_observations(target: Path, remaining: set[str], prepared: dict,
                        repetitions: int) -> None:
    baseline_paths = _stage_paths(set(prepared), "redis-kernel")
    current_paths = _stage_paths(remaining, "redis-kernel")
    baseline = [_successful_receipt(target, path) for path in baseline_paths]
    current = [_successful_receipt(target, path) for path in current_paths]
    commands = {tuple(value["command"]): value["stdout"] for value in baseline}
    def signature(value):
        stdout = (_process_stat_identity(value["stdout"])
                  if "/proc/" in value["command"][-1]
                  and value["command"][-1].endswith("/stat") else value["stdout"])
        return tuple(value["command"]), stdout

    baseline_signatures = Counter(signature(value) for value in baseline)
    if (len(baseline) != 10 or len(commands) != 5
            or set(baseline_signatures.values()) != {2}
            or Counter(tuple(value["command"]) for value in current) \
            != Counter({command: repetitions for command in commands})):
        raise ValueError("fresh 初始化 Redis 内核观察不是逐轮精确多重集："
                         f"baseline={len(baseline)}, current={len(current)}, rounds={repetitions}")
    for value in current:
        expected = commands[tuple(value["command"])]
        if "/proc/" in value["command"][-1] and value["command"][-1].endswith("/stat"):
            if _process_stat_identity(value["stdout"]) != _process_stat_identity(expected):
                raise ValueError("fresh 初始化 Redis 进程创建身份发生变化")
        elif value["stdout"] != expected:
            raise ValueError("fresh 初始化 Redis 工具、配置或路径观察发生变化")
    remaining.difference_update(current_paths)


def validate_initialization_delta(target: Path, current: list[dict], prepared: list[dict],
                                  request: dict,
                                  required: set[str], *, observation_rounds: int,
                                  inventory_failed: bool = False,
                                  initialized_objects: dict | None = None) -> None:
    """以可信 prepare 快照为基线，只接受固定初始化阶段产生的增量。"""
    current_map, prepared_map = _file_map(current), _file_map(prepared)
    if any(current_map.get(path) != value for path, value in prepared_map.items()):
        raise ValueError("fresh 目标 prepare 基线文件缺失或摘要变化")
    additions = {path: value for path, value in current_map.items() if path not in prepared_map}
    guard = current_map.get("copy-target.guard")
    expected_guard = {"path": "copy-target.guard", "type": "file", "bytes": 1,
                      "sha256": hashlib.sha256(b"\0").hexdigest()}
    if guard != expected_guard:
        raise ValueError("fresh 初始化控制互斥文件无效")
    missing = required - set(current_map) - {"copy-target.guard"}
    if missing:
        raise ValueError("fresh 初始化缺少固定阶段文件：" + ", ".join(sorted(missing)))
    remaining = set(additions) - required - {"copy-target.guard"}
    objects_initialized = initialized_objects is not None
    for stage in ("bucket-readiness", "bucket-versioning", "scoped-list"):
        _static_observations(
            target, remaining, prepared_map, stage,
            3 + int(objects_initialized),
            request=request,
            initialized_scope=request["target"]["scope_id"] if objects_initialized else None,
        )
    if objects_initialized:
        _owner_observations(target, remaining, prepared_map, request, initialized_objects)
    _mysql_observations(target, remaining, prepared_map, request)
    _redis_observations(target, remaining, prepared_map, observation_rounds)
    layout_pattern = re.compile(rf"storage-layout-{_UUID}\.json")
    layouts = sorted(path for path in remaining if layout_pattern.fullmatch(path))
    baseline_layouts = [read_json(target / path) for path in prepared_map
                        if layout_pattern.fullmatch(path)]
    if (len(layouts) != observation_rounds or len(baseline_layouts) != 2
            or baseline_layouts[0] != baseline_layouts[1]
            or any(read_json(target / path) != baseline_layouts[0] for path in layouts)):
        raise ValueError("fresh 初始化存储代次观察收据无效")
    remaining.difference_update(layouts)
    if inventory_failed:
        from devex_clone_inventory_failure import inventory_failure_files

        inventory_paths = inventory_failure_files(target, "inventory-initial", request,
                                                   remaining | {"inventory-initial"})
        remaining.difference_update(inventory_paths)
    if remaining:
        raise ValueError("fresh 初始化包含未登记的普通文件或目录："
                         + ", ".join(sorted(remaining)))


def _native(operation: str, backend: Path, **arguments) -> dict:
    """向已绑定协调器交付只读校验；不使用 PATH 或 Cargo 猜测工具。"""
    from rate_limit_tool import tool_binding

    if tool_binding() is None:
        raise ValueError("目标绑定缺少受管 Rust 工具；请通过 run_checks.py 或正式 xtask 入口执行")
    def invoke(command, **options):
        options["timeout"] = 30
        return subprocess.run(command, **options)

    try:
        result, response = native_invoke(
            "__target-binding", {"format_version": 1, "operation": operation,
                                  "backend": str(backend), "arguments": arguments},
            invoke, may_write=False,
        )
    except NativeS3InvocationError as error:
        raise ValueError("原生目标绑定进程未完成：" + str(error.error_type)
                         + "；请核对受管工具与输入文件，保留现场，不重放任何写入") from None
    if (result.returncode == 1 and isinstance(response, dict)
            and set(response) == {"ok", "error"} and response["ok"] is False
            and isinstance(response["error"], dict)
            and set(response["error"]) == {"category", "message", "remote_writes", "automatic_retry"}
            and response["error"]["category"] == "TargetEvidenceRejected"
            and response["error"]["remote_writes"] == 0
            and response["error"]["automatic_retry"] is False
            and isinstance(response["error"]["message"], str)):
        raise ValueError(response["error"]["message"])
    if (result.returncode != 0 or not isinstance(response, dict)
            or set(response) != {"ok", "result"} or response["ok"] is not True
            or not isinstance(response["result"], dict)):
        raise ValueError("原生目标绑定未确认；请核对当前工具和绑定文件，保留现场，不重放任何写入")
    return response["result"]


def validate_review(review: dict) -> None:
    _native("review", Path(__file__).resolve().parents[2], review=review)


def external_file(value: dict) -> None:
    _native("external-file", Path(__file__).resolve().parents[2], descriptor=value)


def request_binding(backend: Path, request: dict) -> tuple[dict, dict]:
    result = _native("request", backend, request=request, predecessor=None)
    return result["review"], result["selected"]


def execution_binary_bindings(backend: Path, request: dict) -> list[dict]:
    return _native("binaries", backend, request=request)["bindings"]


def pending_request_binding(backend: Path, request: dict, predecessor: dict) -> tuple[dict, dict]:
    result = _native("request", backend, request=request, predecessor=predecessor)
    return result["review"], result["selected"]


def execution_backend(backend: Path, request: dict) -> tuple[Path, dict]:
    """解析可选的冻结 业务 crate 执行工作树，默认继续使用当前后端。"""
    declared = request.get("execution_backend")
    if declared is None:
        return backend.resolve(strict=True), {"kind": "current-backend", "path": str(backend.resolve(strict=True))}
    exact(declared, {"fixture", "path"})
    fixture_path = bound_file(backend, declared["fixture"])
    fixture = read_json(fixture_path)
    if (fixture.get("format_version") != 1 or fixture.get("fixture") != "business"
            or fixture.get("status") != "ready" or not isinstance(fixture.get("paths"), dict)
            or not isinstance(fixture.get("generated"), dict)):
        raise ValueError("冻结 业务 crate 工作树收据无效")
    root = local_path(backend, declared["path"])
    expected = Path(fixture["paths"].get("backend", ""))
    generated = fixture["generated"].get("backend")
    if (root != expected or not (root / "Cargo.toml").is_file() or not (root / ".git").exists()
            # 业务 crate 收据绑定的是生成内容快照；忽略运行目录不应让冻结执行树失效。
            or not isinstance(generated, dict) or snapshot(root)[0] != generated):
        raise ValueError("冻结 业务 crate 后端工作树或生成来源已变化")
    return root, {"kind": "business-fixture", "path": str(root), "fixture": declared["fixture"], "source": generated}


def application_config_directory(backend: Path, request: dict) -> Path:
    return Path(_native("config-directory", backend, request=request)["path"])


def reset_config(backend: Path, request: dict, selected: dict) -> None:
    _native("reset-configuration", backend, request=request, selected=selected,
            app_env=os.environ.get("APP_ENV"))


def redis_configuration(backend: Path, request: dict, selected: dict, *, environment=None) -> None:
    environment = os.environ if environment is None else environment
    _native("redis-configuration", backend, request=request, selected=selected,
            app_env=environment.get("APP_ENV"))


def never_started(backend: Path, selected: dict) -> None:
    _native("never-started", backend, selected=selected)


def generation(backend: Path, request: dict, run) -> dict:
    environment = {"APP_ENV": os.environ.get("APP_ENV")}
    review, selected = request_binding(backend, request)
    execution_root, execution = execution_backend(backend, request)
    config_dir = application_config_directory(backend, request)
    never_started(backend, selected)
    if configuration_digest(execution_root, config_dir) != digest(request["configuration_sha256"]):
        raise ValueError("新目标 APP 配置或秘密发生变化")
    maintenance = verify_tools(execution_root, bound_file(backend, request["maintenance_build"]), run)
    physical = native_response(*native_invoke(
        "__source-evidence", {"operation": "physical-binding", "write": False,
            "arguments": {
                'backend': str(execution_root),
                'evidence_root': str(execution_root),
                'source': request["target"],
                'app_env': environment.get("APP_ENV"),
                'config_dir': str(config_dir),
            },
        }, subprocess.run, may_write=False,
    ))[0]["value"]
    planned = {db["key"]: db for db in selected["databases"]}
    for actual in physical["databases"]:
        if any(actual[key] != planned[actual["key"]][key] for key in ("host", "port")):
            raise ValueError("目标 MySQL host/port 不是审阅计划中的地址")
    configuration = inventory_configuration(execution_root, environment, request["target"], config_dir=config_dir)
    reset_config(execution_root, request, selected)
    verify_api_address(execution_root, selected["api_url"], config_dir=config_dir)
    table = load_app_table(execution_root, environment, config_dir=config_dir)
    if not isinstance(table.get("jobs"), dict) or table["jobs"].get("mode") != "external":
        raise ValueError("所选配置文件的 jobs.mode 必须为 external")
    if worker_ready_url(execution_root, config_dir) != selected["worker_ready_url"]:
        raise ValueError("所选配置文件的 jobs.health_host/health_port 与目标 Worker 地址不同")
    for field in ("api_url", "worker_ready_url", "frontend_url"):
        require_closed_port(selected[field])
    # 只保存文件中种子密码的摘要，防止同代次 reset 凭据变更。
    bootstrap = table.get("bootstrap", {})
    password = bootstrap.get("admin_password") if isinstance(bootstrap, dict) else None
    if not isinstance(password, str) or not password.strip():
        raise ValueError("所选配置文件的 bootstrap.admin_password 必须显式提供初始化管理员密码")
    secrets = {"bootstrap.admin_password": password}
    return {"execution": execution, "maintenance": maintenance, "physical": physical, "configuration": configuration,
            "configuration_sha256": request["configuration_sha256"],
            "seed_credentials_sha256": plan_hash(secrets), "request_sha256": plan_hash(request),
            "review_sha256": request["review"]["sha256"], "selected": selected,
            "operator_declared_controlled_generation_only": True, "external_writers_discovered": False}


def validate_reset_manifest(manifest: dict, request: dict, original: dict) -> None:
    _native("reset-manifest", Path(original["maintenance"]["backend_root"]),
            manifest=manifest, request=request, original=original)
