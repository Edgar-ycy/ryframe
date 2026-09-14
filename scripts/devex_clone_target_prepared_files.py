"""fresh prepare 快照的精确只读产物集合与观察语义。"""
from collections import Counter
import json
from pathlib import Path
import re

from devex_clone import read_json
from devex_clone_capture import read_bound_json
from devex_clone_model import exact
from devex_clone_source_proof import bound_file
from devex_clone_target_time import timestamp
from restore_build import file_digest
from restore_reference_plan import BUCKETS, plan_hash

_UUID = r"[a-f0-9]{32}"
_COMMAND_FIELDS = {"command", "returncode", "error_type", "stdout", "stderr"}
_PREPARE_FIELDS = {"format_version", "status", "prepared_at", "request", "request_sha256",
                   "generation", "absence", "fresh_target_initialized", "controlled_generation_only",
                   "external_writers_discovered", "create_intents"}
_GENERATION_FIELDS = {"execution", "maintenance", "physical", "configuration",
                      "configuration_sha256", "seed_credentials_sha256", "request_sha256",
                      "review_sha256", "selected", "operator_declared_controlled_generation_only",
                      "external_writers_discovered", "storage"}


def _file_map(target: Path, files: list[dict], *, locked_guard: bool = False) -> dict[str, dict]:
    result = {}
    for item in files:
        exact(item, {"path", "type", "bytes", "sha256"})
        name = item["path"]
        if (item["type"] != "file" or not isinstance(name, str) or Path(name).name != name
                or name in result):
            raise ValueError("fresh prepare 快照包含目录、越界路径或变化文件")
        expected = ({"path": name, "type": "file", **file_digest(target / name)}
                    if name != "copy-target.guard" or not locked_guard else item)
        if expected != item:
            raise ValueError("fresh prepare 快照包含目录、越界路径或变化文件")
        result[name] = item
    return result


def _receipt(target: Path, name: str) -> dict:
    value = read_json(target / name)
    exact(value, _COMMAND_FIELDS)
    if (not isinstance(value["command"], list) or not value["command"]
            or any(not isinstance(part, str) or not part for part in value["command"])
            or type(value["returncode"]) is not int or value["returncode"] != 0
            or value["error_type"] is not None or not isinstance(value["stdout"], str)
            or value["stderr"] != ""):
        raise ValueError("fresh prepare 只读观察命令收据无效")
    value["stdout"] = value["stdout"].replace("\r\n", "\n")
    return value


def _stage(names: set[str], stage: str, count: int) -> list[str]:
    pattern = re.compile(re.escape(stage) + rf"-{_UUID}\.command\.json")
    found = sorted(name for name in names if pattern.fullmatch(name))
    if len(found) != count:
        raise ValueError(f"fresh prepare {stage} 观察数量无效")
    return found


def _objects(target: Path, names: set[str], request: dict) -> set[str]:
    consumed = set()
    scope = request["target"]["scope_id"] + "/"
    operations = {"bucket-readiness": "head-bucket",
                  "bucket-versioning": "get-bucket-versioning",
                  "scoped-list": "list-objects-v2"}
    for stage, operation in operations.items():
        paths = _stage(names, stage, len(BUCKETS))
        receipts = [_receipt(target, name) for name in paths]
        buckets = set()
        for receipt in receipts:
            command = receipt["command"]
            if ("s3api" not in command or "--bucket" not in command
                    or command[command.index("s3api") + 1] != operation):
                raise ValueError("fresh prepare 对象观察命令无效")
            buckets.add(command[command.index("--bucket") + 1])
            body = json.loads(receipt["stdout"] or "{}")
            if stage == "bucket-versioning" and body.get("Status") is not None:
                raise ValueError("fresh prepare 对象桶启用了版本")
            if stage == "scoped-list" and (body.get("IsTruncated") is not False
                    or body.get("Contents", []) != []
                    or command[command.index("--prefix") + 1] != scope):
                raise ValueError("fresh prepare 对象前缀并非明确空范围")
        if buckets != set(BUCKETS):
            raise ValueError("fresh prepare 对象观察未完整覆盖五桶")
        consumed.update(paths)
    return consumed


def validate_prepare_document(backend: Path, prepared: dict,
                              request_descriptor: dict, request: dict) -> None:
    """将 prepare 的声明、来源摘要与精确空资源前像绑定到登记请求。"""
    exact(prepared, _PREPARE_FIELDS)
    generation = prepared["generation"]
    exact(generation, _GENERATION_FIELDS)
    maintenance = read_bound_json(
        bound_file(backend, request["maintenance_build"]), request["maintenance_build"])
    review_descriptor = {key: value for key, value in request["review"].items()
                         if key != "canonical_sha256"}
    review = read_bound_json(bound_file(backend, review_descriptor), review_descriptor)
    create_intents = [{"key": item["key"], "server_uuid": item["server_uuid"],
                       "database": item["database"]} for item in request["target"]["databases"]]
    databases = [{"server_uuid": item["server_uuid"], "database": item["database"],
                  "exists": False, "empty_checked": False}
                 for item in request["target"]["databases"]]
    absence = {"databases": databases,
               "objects": {bucket: {"keys": []} for bucket in BUCKETS},
               "redis": {"keys": [], "owner": None, "sentinel": None}}
    if (prepared["format_version"] != 1 or prepared["status"] != "fresh_creation_prepared"
            or not timestamp(prepared["prepared_at"])
            or prepared["request"] != request_descriptor
            or prepared["request_sha256"] != plan_hash(request)
            or prepared["fresh_target_initialized"] is not False
            or prepared["controlled_generation_only"] is not True
            or prepared["external_writers_discovered"] is not False
            or prepared["create_intents"] != create_intents or prepared["absence"] != absence
            or generation["maintenance"] != maintenance
            or generation["configuration_sha256"] != request["configuration_sha256"]
            or generation["request_sha256"] != plan_hash(request)
            or generation["review_sha256"] != request["review"]["sha256"]
            or generation["selected"] != review["scopes"][request["side"]]
            or generation["storage"] != request["storage"]
            or generation["operator_declared_controlled_generation_only"] is not True
            or generation["external_writers_discovered"] is not False
            or any(not isinstance(generation[key], dict)
                   for key in ("execution", "physical", "configuration"))):
        raise ValueError("fresh prepare 声明、来源或空资源前像无效")


def validate_prepared_tree(backend: Path, target: Path, files: list[dict],
                           request_descriptor: dict, request: dict, *,
                           locked_guard: bool = False) -> None:
    """拒绝把未知文件吸收到 prepare 基线，并核验固定观察多重集。"""
    values = _file_map(target, files, locked_guard=locked_guard)
    names = set(values)
    consumed = {"request.json", "prepare.json"}
    if locked_guard:
        if "copy-target.guard" not in names:
            raise ValueError("fresh prepare 快照缺少已持有的控制互斥文件")
        consumed.add("copy-target.guard")
    if not consumed.issubset(names) or read_json(target / "request.json") != request:
        raise ValueError("fresh prepare 快照缺少固定请求或结果")
    validate_prepare_document(
        backend, read_json(target / "prepare.json"), request_descriptor, request)
    for database in request["target"]["databases"]:
        paths = _stage(names, "mysql-" + database["key"], 1)
        receipt = _receipt(target, paths[0])
        if receipt["stdout"] != database["server_uuid"] + "\n":
            raise ValueError("fresh prepare MySQL 缺失观察不属于固定目标")
        consumed.update(paths)
    redis_paths = _stage(names, "redis-kernel", 10)
    redis = [_receipt(target, name) for name in redis_paths]
    signatures = Counter((tuple(item["command"]), item["stdout"]) for item in redis)
    if len(signatures) != 5 or set(signatures.values()) != {2}:
        raise ValueError("fresh prepare Redis 内核观察不是五项两轮")
    consumed.update(redis_paths)
    layout_pattern = re.compile(rf"storage-layout-{_UUID}\.json")
    layouts = sorted(name for name in names if layout_pattern.fullmatch(name))
    if len(layouts) != 2:
        raise ValueError("fresh prepare 存储目录观察数量无效")
    if read_json(target / layouts[0]) != read_json(target / layouts[1]):
        raise ValueError("fresh prepare 存储目录两轮观察不一致")
    consumed.update(layouts)
    consumed.update(_objects(target, names, request))
    if "failure.json" in names or any(name.startswith("resume-prepare-") for name in names):
        from devex_clone_target import _prepare_failure, _resume_records

        failure = _prepare_failure(target)
        records = _resume_records(backend, target, request_descriptor)
        if failure is None or records["pending"] or not any(
                item[0] == "confirmed" for item in records["terminals"].values()):
            raise ValueError("fresh prepare 失败续作谱系没有完整确认")
        consumed.add("failure.json")
        consumed.update(Path(item["path"]).name for item in records["intents"].values())
        consumed.update(Path(item[2]["path"]).name for item in records["terminals"].values())
    if names != consumed:
        raise ValueError("fresh prepare 快照包含未声明的文件")
