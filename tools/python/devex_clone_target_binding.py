"""将新目标严格绑定到已审阅单侧资源、实际配置和三份维护工具。"""
from __future__ import annotations

import copy
from collections import Counter
import hashlib
import json
import os
from pathlib import Path
import re

from artifact_digests import filesystem_path
from devex_clone import read_json
from devex_clone_capture import read_bound_json, unique_object
from devex_clone_model import digest, exact, linked, local_path, name
from devex_clone_inventory import configuration as inventory_configuration
from devex_clone_run_state import binding
from devex_clone_source_proof import bound_file, require_closed_port, verify_api_address
from devex_clone_tools import verify as verify_tools
from full_stack_runtime import configuration_digest, worker_ready_url
from full_stack_rate_limit_config import load_app_table
from restore_build import file_digest
from restore_reference_plan import BUCKETS, identifier, plan_hash
from restore_source_binding import defaults_connection, source_binding
from source_inventory import snapshot

KEYS = {"shared-control": ("combined", "shared"), "shared": ("tenant", "shared"),
        "dedicated-a": ("tenant", "dedicated"), "dedicated-b": ("tenant", "dedicated")}
EXCLUSIVE = {"mysql_exclusive", "redis_exclusive", "object_storage_exclusive"}
REVIEW_FILE_TOOLS = ("mysql", "mysqldump", "aws", "rustfs")
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
                if not os.path.isdir(native) or any(path.iterdir()):
                    raise ValueError("fresh 目标忽略项不是空的普通目录")
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


def _stage_paths(values: set[str], stage: str) -> list[str]:
    pattern = re.compile(re.escape(stage) + rf"-{_UUID}\.command\.json")
    return sorted(path for path in values if pattern.fullmatch(path))


def _static_observations(target: Path, remaining: set[str], prepared: dict,
                         stage: str, repetitions: int, *, initialized_scope: str | None) -> None:
    baseline = [_successful_receipt(target, path) for path in _stage_paths(set(prepared), stage)]
    current_paths = _stage_paths(remaining, stage)
    current = [_successful_receipt(target, path) for path in current_paths]
    signature = lambda item: (tuple(item["command"]), item["stdout"])
    expected = Counter(signature(item) for item in baseline)
    actual = Counter(signature(item) for item in current)
    wanted = Counter({key: count * repetitions for key, count in expected.items()})
    initialized_lists = stage == "scoped-list" and initialized_scope is not None
    valid = len(baseline) == 5 and len(expected) == 5
    if valid and initialized_lists:
        commands = {tuple(item["command"]): item["stdout"] for item in baseline}
        grouped = {command: [item["stdout"] for item in current
                             if tuple(item["command"]) == command]
                   for command in commands}
        owner = initialized_scope + "/.ryframe-owner"
        for command, outputs in grouped.items():
            final = [value for value in outputs if value != commands[command]]
            try:
                page = json.loads(final[0], object_pairs_hook=unique_object) if len(final) == 1 else None
            except (TypeError, ValueError):
                page = None
            valid = (valid and len(outputs) == repetitions
                     and outputs.count(commands[command]) == repetitions - 1
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
    baseline = [_successful_receipt(target, path) for path in baseline_paths]
    prefixes = []
    for receipt in baseline:
        command = receipt["command"]
        if "s3api" not in command:
            raise ValueError("fresh 初始化对象工具前缀缺失")
        prefixes.append(command[:command.index("s3api")])
    if len(prefixes) != len(BUCKETS) or any(value != prefixes[0] for value in prefixes):
        raise ValueError("fresh 初始化对象工具前缀不唯一")
    receipts = _stage_paths(remaining, "owner-read")
    if len(receipts) != len(BUCKETS) or not isinstance(initialized_objects, dict) \
            or set(initialized_objects) != set(BUCKETS):
        raise ValueError("fresh 初始化对象 owner 证据未完整覆盖五桶")
    scope = request["target"]["scope_id"]
    seen = set()
    consumed = set(receipts)
    for receipt_path in receipts:
        receipt = _successful_receipt(target, receipt_path)
        command = receipt["command"]
        if "--bucket" not in command:
            raise ValueError("fresh 初始化对象 owner 命令缺少桶")
        bucket = command[command.index("--bucket") + 1]
        pattern = re.compile(r"owner-" + re.escape(bucket) + rf"-{_UUID}\.bin")
        body_name = Path(command[-1]).name
        body = target / body_name
        owner_key = scope + "/.ryframe-owner"
        expected_command = [*prefixes[0], "s3api", "get-object", "--bucket", bucket,
                            "--key", owner_key, str(body)]
        expected_bytes = f"ryframe-owner:v1:{scope}:object-storage:{bucket}".encode()
        expected_owner = {"bytes": len(expected_bytes),
                          "sha256": hashlib.sha256(expected_bytes).hexdigest()}
        if (bucket not in BUCKETS or bucket in seen or not pattern.fullmatch(body_name)
                or command != expected_command or body.read_bytes() != expected_bytes
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


def validate_review(review: dict) -> None:
    """拒绝缺字段的审阅计划，避免初始化过程中才发现资源定义不完整。"""
    if not isinstance(review, dict) or not isinstance(review.get("reference"), dict):
        raise ValueError("审阅计划结构无效")
    if review.get("kind") != "review-only-perf-resource-plan-with-readonly-preflight":
        raise ValueError("审阅计划类型无效")
    if review.get("ready_for_execution") is not True:
        raise ValueError("审阅计划尚未就绪")
    scopes, tools, services = review.get("scopes"), review.get("tools"), review.get("services")
    if not isinstance(scopes, dict) or set(scopes) != {"seed", "base", "candidate"}:
        raise ValueError("审阅计划必须完整声明三侧资源")
    if not isinstance(tools, dict) or not isinstance(services, dict):
        raise ValueError("审阅计划缺少工具或服务定义")
    for role in REVIEW_FILE_TOOLS:
        tool = tools.get(role)
        if not isinstance(tool, dict) or not isinstance(tool.get("path"), str) or not isinstance(tool.get("sha256"), str):
            raise ValueError("审阅计划工具定义无效")
    redis = tools.get("redis_server")
    if not isinstance(redis, dict) or any(not isinstance(redis.get(key), str) for key in ("distribution", "resolved_path", "sha256")):
        raise ValueError("审阅计划 Redis 工具定义无效")
    rustfs, cache = services.get("rustfs"), services.get("redis")
    if not isinstance(rustfs, dict) or not isinstance(cache, dict) or any(
        not isinstance(value, str)
        for value in (rustfs.get("api"), rustfs.get("console"), rustfs.get("data_dir"), cache.get("directory"))
    ):
        raise ValueError("审阅计划服务定义无效")
    for role in ("source", "protected_target"):
        side = review["reference"].get(role)
        if not isinstance(side, dict) or not isinstance(side.get("databases"), list):
            raise ValueError("审阅计划保护资源定义无效")
    for side in scopes.values():
        if not isinstance(side, dict) or not isinstance(side.get("scope_id"), str) or not name(side["scope_id"]):
            raise ValueError("审阅计划 scope 无效")
        if any(not isinstance(side.get(key), str) for key in ("runtime_dir", "identity_ledger", "api_url", "worker_ready_url", "frontend_url")):
            raise ValueError("审阅计划运行路径无效")
        objects, cache = side.get("objects"), side.get("redis")
        if not isinstance(objects, dict) or any(not isinstance(objects.get(key), str) for key in ("endpoint", "region")):
            raise ValueError("审阅计划对象存储定义无效")
        if not isinstance(cache, dict) or any(not isinstance(cache.get(key), str) for key in ("url", "namespace", "ownership_key", "ownership_value")):
            raise ValueError("审阅计划 Redis 定义无效")
        databases = side.get("databases")
        if not isinstance(databases, list) or {item.get("key") for item in databases if isinstance(item, dict)} != set(KEYS):
            raise ValueError("审阅计划数据库定义无效")
        if any(not isinstance(item.get("database"), str) or not isinstance(item.get("expected_server_uuid"), str) for item in databases):
            raise ValueError("审阅计划数据库身份无效")


def external_file(value: dict) -> None:
    exact(value, {"path", "sha256"})
    path = Path(value["path"])
    if (not path.is_absolute() or any(linked(p) for p in (path, *path.parents))
            or file_digest(path)["sha256"] != digest(value["sha256"])):
        raise ValueError("新目标外部工具实际字节或路径已变化")


def _request_binding(backend: Path, request: dict, validate) -> tuple[dict, dict]:
    required = {"format_version", "kind", "id", "review", "side", "target", "maintenance_build",
                "configuration_sha256", "tools", "storage", "reset"}
    exact(request, required | ({"execution_backend"} if "execution_backend" in request else set()))
    if request["format_version"] != 1 or request["kind"] != "devex-clone-fresh-target":
        raise ValueError("新目标请求类型错误")
    name(request["id"])
    exact(request["review"], {"path", "bytes", "sha256", "canonical_sha256"})
    review = read_json(bound_file(backend, {k: v for k, v in request["review"].items() if k != "canonical_sha256"}))
    validate(review)
    if (plan_hash(review) != digest(request["review"]["canonical_sha256"])
            or request["side"] not in review["scopes"]):
        raise ValueError("必须选择已就绪审阅计划的明确单侧")
    selected = review["scopes"][request["side"]]
    target = request["target"]
    exact(target, {"scope_id", "s3", "databases"})
    if name(target["scope_id"]) != selected["scope_id"]:
        raise ValueError("目标 scope 与单侧审阅计划不符")
    exact(target["s3"], {"endpoint", "region", "access_key_env", "secret_key_env"})
    if any(target["s3"][key] != selected["objects"][key] for key in ("endpoint", "region")):
        raise ValueError("对象存储不是计划中的目标端点")
    actual = target["databases"]
    if not isinstance(actual, list) or len(actual) != 4 or {db["key"] for db in actual} != set(KEYS):
        raise ValueError("新目标必须精确包含四个逻辑数据库")
    planned = {db["key"]: db for db in selected["databases"]}
    all_names = [(db["expected_server_uuid"], db["database"].lower())
                 for side in review["scopes"].values() for db in side["databases"]]
    protected = {(db["server_uuid"], db["database"].lower())
                 for role in ("source", "protected_target") for db in review["reference"][role]["databases"]}
    if len(all_names) != 12 or len(set(all_names)) != 12 or set(all_names) & protected:
        raise ValueError("三侧数据库相互重叠或碰到参考资源")
    for db in actual:
        exact(db, {"key", "kind", "mode", "database", "server_uuid", "defaults_file", "defaults_sha256"})
        expected = planned[db["key"]]
        if ((db["kind"], db["mode"]) != KEYS[db["key"]] or identifier(db["database"]) != expected["database"]
                or db["server_uuid"] != expected["expected_server_uuid"]
                or db["defaults_file"] != expected["connection_file"]):
            raise ValueError("数据库物理身份或凭据路径不是精确审阅目标")
        if file_digest(local_path(backend, db["defaults_file"]))["sha256"] != digest(db["defaults_sha256"]):
            raise ValueError("目标 MySQL 凭据文件变化")
    exact(request["tools"], {"mysql", "aws"})
    for role, tool in request["tools"].items():
        external_file(tool)
        if any(tool[key] != review["tools"][role][key] for key in ("path", "sha256")):
            raise ValueError("目标工具不属于审阅版本")
    exact(request["storage"], {"rustfs", "redis"})
    return review, selected


def request_binding(backend: Path, request: dict) -> tuple[dict, dict]:
    """只接受已经完成预检并明确就绪的普通 fresh-target 请求。"""
    return _request_binding(backend, request, validate_review)


def execution_binary_bindings(backend: Path, request: dict) -> list[dict]:
    """收集 fresh 阶段会执行或持续核验的本机二进制。"""
    review, _ = request_binding(backend, request)
    build = read_json(bound_file(backend, request["maintenance_build"]))
    artifacts = build.get("artifacts")
    if build.get("kind") != "devex-clone-tool-build" or not isinstance(artifacts, dict) \
            or set(artifacts) != {"reset", "migrate", "tenant-data"}:
        raise ValueError("fresh 目标维护构建缺少完整二进制")
    bindings = [dict(value) for value in request["tools"].values()]
    for artifact in artifacts.values():
        if not isinstance(artifact, dict) or any(key not in artifact for key in ("executable", "sha256")):
            raise ValueError("fresh 目标维护构建二进制绑定无效")
        bindings.append({"path": artifact["executable"], "sha256": artifact["sha256"]})
    rustfs, redis = request["storage"]["rustfs"], request["storage"]["redis"]
    rustfs_binding = {"path": rustfs.get("identity", {}).get("executable"), "sha256": rustfs.get("sha256")}
    if rustfs_binding != review["tools"]["rustfs"] or redis.get("wsl") != review["tools"]["wsl"]:
        raise ValueError("fresh 目标存储二进制不属于审阅版本")
    bindings.extend((rustfs_binding, dict(redis["wsl"])))
    return bindings


def pending_request_binding(backend: Path, request: dict, predecessor: dict) -> tuple[dict, dict]:
    """仅供 review successor 核对历史 seed；不授予该请求执行权限。"""
    expected = copy.deepcopy(predecessor)
    if request.get("review") != expected:
        raise ValueError("历史 seed 请求未绑定指定 pending predecessor")

    def validate_pending(review: dict) -> None:
        if review.get("ready_for_execution") is not False:
            raise ValueError("历史 seed predecessor 必须保持 pending")
        structural = copy.deepcopy(review)
        structural["ready_for_execution"] = True
        validate_review(structural)

    result = _request_binding(backend, request, validate_pending)
    if request.get("review") != expected:
        raise ValueError("历史 seed 请求的 predecessor 在核对期间变化")
    return result


def execution_backend(backend: Path, request: dict) -> tuple[Path, dict]:
    """解析可选的冻结 Device 执行工作树，默认继续使用当前后端。"""
    declared = request.get("execution_backend")
    if declared is None:
        return backend.resolve(strict=True), {"kind": "current-backend", "path": str(backend.resolve(strict=True))}
    exact(declared, {"fixture", "path"})
    fixture_path = bound_file(backend, declared["fixture"])
    fixture = read_json(fixture_path)
    if (fixture.get("format_version") != 1 or fixture.get("fixture") != "business"
            or fixture.get("status") != "ready" or not isinstance(fixture.get("paths"), dict)
            or not isinstance(fixture.get("generated"), dict)):
        raise ValueError("冻结 Device 工作树收据无效")
    root = local_path(backend, declared["path"])
    expected = Path(fixture["paths"].get("backend", ""))
    generated = fixture["generated"].get("backend")
    if (root != expected or not (root / "Cargo.toml").is_file() or not (root / ".git").exists()
            # Device 收据绑定的是生成内容快照；忽略运行目录不应让冻结执行树失效。
            or not isinstance(generated, dict) or snapshot(root)[0] != generated):
        raise ValueError("冻结 Device 后端工作树或生成来源已变化")
    return root, {"kind": "device-fixture", "path": str(root), "fixture": declared["fixture"], "source": generated}


def reset_config(backend: Path, request: dict, selected: dict) -> None:
    declared = request["reset"]
    exact(declared, {"legacy_ownership", "credential_version", "sentinel_key", "sentinel_value"})
    exact(declared["legacy_ownership"], EXCLUSIVE)
    if any(value is not True for value in declared["legacy_ownership"].values()):
        raise ValueError("仅本次新空资源显式声明 exclusive 引导；不能隐式注入配置")
    table = load_app_table(backend, os.environ).get("reset", {})
    for key in EXCLUSIVE:
        field = "legacy_" + key
        variable = "APP_RESET_" + field.upper()
        if variable + "_FILE" in os.environ:
            raise ValueError("reset 文件覆盖未被明确绑定")
        actual = os.environ.get(variable, table.get(field))
        if actual is not True and actual != "true":
            raise ValueError("实际 reset exclusive 配置与请求不同")
    for field, value in (("credential_version", declared["credential_version"]),
                         ("redis_outside_sentinel_key", declared["sentinel_key"])):
        variable = "APP_RESET_" + field.upper()
        if variable + "_FILE" in os.environ or os.environ.get(variable, table.get(field)) != value:
            raise ValueError("实际 reset 版本或 sentinel 与显式请求不同")
    scope = request["target"]["scope_id"]
    if (not re.fullmatch(r"[A-Za-z0-9._-]{1,64}", declared["credential_version"])
            or declared["sentinel_key"] != f"ryframe:devex-fresh:{scope}:sentinel"
            or declared["sentinel_value"] != f"devex-fresh:{request['id']}"
            or declared["sentinel_key"].startswith(selected["redis"]["namespace"])):
        raise ValueError("sentinel 必须是仅归本 scope 的精确外部哨兵")
    redis_configuration(request, selected)


def redis_configuration(request: dict, selected: dict) -> None:
    """核对精确 Redis 连接与 namespace；复用于只读来源代次，不执行 reset。"""
    scope = request["target"]["scope_id"]
    redis = selected["redis"]
    if redis["namespace"] != f"ryframe:{{{scope}}}:" or redis["ownership_key"] != redis["namespace"] + ".ryframe-owner":
        raise ValueError("Redis 计划 namespace 无效")
    # 该入口限定已登记的本机非 TLS Redis；不复制配置或创建 Redis 服务。
    expected = {"APP_REDIS_HOST": "127.0.0.1", "APP_REDIS_PORT": str(request["storage"]["redis"]["port"]),
                "APP_REDIS_DATABASE": "0", "APP_REDIS_TLS": "false"}
    if any(os.environ.get(key) != value for key, value in expected.items()):
        raise ValueError("实际 Redis 地址或 TLS 未显式匹配本侧配置")


def never_started(backend: Path, selected: dict) -> None:
    runtime = local_path(backend, selected["runtime_dir"])
    for filename in ("producer-history.json", "api.json", "worker.json"):
        evidence = runtime / filename
        if evidence.exists() or linked(evidence):
            raise ValueError("目标已经出现生产者启动意图或进程收据，不能认定为从未启动的 fresh 目标")
    for field in ("runtime_dir", "identity_ledger"):
        directory = local_path(backend, selected[field])
        if directory.exists() and (linked(directory) or not directory.is_dir() or any(directory.iterdir())):
            raise ValueError("目标已经出现运行或身份准备历史，不具备本代次 fresh 条件")


def generation(backend: Path, request: dict, run) -> dict:
    review, selected = request_binding(backend, request)
    execution_root, execution = execution_backend(backend, request)
    never_started(backend, selected)
    if configuration_digest(execution_root) != digest(request["configuration_sha256"]):
        raise ValueError("新目标 APP 配置或秘密发生变化")
    maintenance = verify_tools(execution_root, bound_file(backend, request["maintenance_build"]), run)
    physical = source_binding(execution_root, {"source": request["target"]})
    planned = {db["key"]: db for db in selected["databases"]}
    for actual in physical["databases"]:
        if any(actual[key] != planned[actual["key"]][key] for key in ("host", "port")):
            raise ValueError("目标 MySQL host/port 不是审阅计划中的地址")
    configuration = inventory_configuration(execution_root, os.environ, request["target"])
    reset_config(execution_root, request, selected)
    verify_api_address(backend, selected["api_url"])
    if os.environ.get("APP_JOBS_MODE") != "external" or worker_ready_url() != selected["worker_ready_url"]:
        raise ValueError("目标 Worker 必须显式匹配关闭端口与 external 模式")
    for field in ("api_url", "worker_ready_url", "frontend_url"):
        require_closed_port(selected[field])
    # 种子密码不属于 APP_*，同样只保存摘要，防止同代次 reset 凭据变更。
    secrets = {key: os.environ.get(key) for key in ("RYFRAME_RESET_ADMIN_PASSWORD", "RYFRAME_RESET_USER_PASSWORD")}
    if not all(secrets.values()):
        raise ValueError("必须显式提供本侧两项初始化密码")
    return {"execution": execution, "maintenance": maintenance, "physical": physical, "configuration": configuration,
            "configuration_sha256": request["configuration_sha256"],
            "seed_credentials_sha256": plan_hash(secrets), "request_sha256": plan_hash(request),
            "review_sha256": request["review"]["sha256"], "selected": selected,
            "operator_declared_controlled_generation_only": True, "external_writers_discovered": False}


def validate_reset_manifest(manifest: dict, request: dict, original: dict) -> None:
    target, reset = request["target"], request["reset"]
    exact(manifest, {"manifest_version", "environment", "scope_id", "code_sha", "config_sha", "credential_version",
                     "confirmation_phrase", "legacy_ownership", "redis", "object_storage", "databases"})
    scope = target["scope_id"]
    if (manifest["manifest_version"] != 4 or manifest["environment"] != "test" or manifest["scope_id"] != scope
            or manifest["code_sha"] != original["maintenance"]["source"]["snapshot"]["head"]
            or manifest["credential_version"] != reset["credential_version"]
            or manifest["confirmation_phrase"] != f"RESET-RYFRAME-test-{scope}"
            or manifest["legacy_ownership"] != reset["legacy_ownership"]):
        raise ValueError("实际 reset plan 来源或引导范围不符")
    digest(manifest["config_sha"])
    physical = {item["key"]: item for item in original["physical"]["databases"]}
    if len(manifest["databases"]) != 4:
        raise ValueError("reset plan 包含额外或缺失数据库")
    seen = set()
    for item in manifest["databases"]:
        keys = item["target_keys"]
        if len(keys) != 1 or keys[0] not in KEYS or keys[0] in seen:
            raise ValueError("reset plan 数据库合并或重复")
        key = keys[0]
        expected = physical[key]
        seen.add(key)
        kinds = {"control", "tenant-data"} if key == "shared-control" else {"tenant-data"}
        source_db = next(db for db in target["databases"] if db["key"] == key)
        connection = defaults_connection(Path(original["maintenance"]["backend_root"]), source_db)
        if (any(item[field] != expected[field] for field in ("host", "port", "database"))
                or item["control_baseline"] != (key == "shared-control") or item["tenant_baseline"] is not True
                or item["ownership_markers"] != {kind: f"ryframe-owner:v1:{scope}:{kind}" for kind in kinds}
                or item["connection"]["tls_mode"] != expected["tls_mode"]
                or item["connection"]["username"] != connection["username"]
                or any(item["connection"].get(field) is not None for field in ("tls_ca_sha256", "tls_client_cert_sha256", "tls_client_key_ref_sha256"))):
            raise ValueError("reset plan 数据库实际目标、baseline 或 owner 不符")
    storage = manifest["object_storage"]
    expected_storage = original["physical"]["s3"]
    if (any(storage[field] != expected_storage[field] for field in ("backend", "endpoint", "region"))
            or storage["use_ssl"] != expected_storage["endpoint"].startswith("https://")
            or len(storage["prefixes"]) != 5 or {p["bucket"] for p in storage["prefixes"]} != BUCKETS):
        raise ValueError("reset plan 对象端点或五桶不符")
    for item in storage["prefixes"]:
        if item != {"bucket": item["bucket"], "prefix": scope + "/", "ownership_marker_key": scope + "/.ryframe-owner",
                    "ownership_marker": f"ryframe-owner:v1:{scope}:object-storage:{item['bucket']}"}:
            raise ValueError("reset plan 对象前缀越界")
    redis, selected = manifest["redis"], original["selected"]["redis"]
    if (redis["host"] != "127.0.0.1" or redis["port"] != request["storage"]["redis"]["port"]
            or redis["database"] != 0 or redis["tls"] is not False or redis["namespace"] != selected["namespace"]
            or redis["ownership_marker_key"] != selected["ownership_key"] or redis["ownership_marker"] != selected["ownership_value"]
            or redis["outside_sentinel_key_sha256"] != hashlib.sha256(reset["sentinel_key"].encode()).hexdigest()):
        raise ValueError("reset plan Redis namespace、端点或 sentinel 不符")
