"""迁移续作锁内目标树的精确增量声明；未知文件不能被后续阶段吸收。"""
from collections import Counter
from pathlib import Path
import hashlib
import json
import re

from devex_clone import read_json
from devex_clone_capture import read_bound_json, write_json
from devex_clone_model import exact
from devex_clone_run_state import binding
from devex_clone_target_binding import target_files, validate_initialization_delta
from devex_clone_target_prepared_files import validate_prepared_tree
from restore_build import file_digest
from restore_reference_plan import BUCKETS

COMMAND_FIELDS = {"command", "returncode", "error_type", "stdout", "stderr"}
_UUID = r"[a-f0-9]{32}"


def digest_binding(path: Path) -> dict:
    return {key: value for key, value in binding(path).items() if key != "path"}


def snapshot(workspace: Path, value: dict, stage: str, *, locked_guard: bool = False,
             initialize_lock_released: bool = False) -> dict:
    path = workspace / f"{stage}-files.json"
    before = binding(path)
    document = read_bound_json(path, before)
    exact(document, {"format_version", "kind", "registration", "predecessor", "files"})
    predecessor = binding(workspace / "prepared-files.json") if stage == "initialized" else None
    ignored = {"initialize.lock"} if locked_guard else None
    if (document["format_version"] != 1
            or document["kind"] != f"devex-clone-fresh-target-{stage}-files"
            or document["registration"] != binding(workspace / "registration.json")
            or document["predecessor"] != predecessor
            or document["files"] != target_files(
                Path(value["target_directory"]), ignored=ignored, locked_guard=locked_guard,
                initialize_lock_released=initialize_lock_released)
            or binding(path) != before):
        raise ValueError("fresh 目标阶段文件集合、内容或登记发生变化")
    return before


def write_snapshot(backend: Path, workspace: Path, value: dict, stage: str,
                   expected_files: list[dict] | None = None, *, expected_document: dict | None = None,
                   locked_guard: bool = False) -> dict:
    target = Path(value["target_directory"])
    first = target_files(target, locked_guard=locked_guard,
                         initialize_lock_released=locked_guard)
    if expected_files is not None and first != expected_files:
        raise ValueError("fresh 目标初始化锁释放后的最终文件树不同于执行结果")
    if target_files(target, locked_guard=locked_guard,
                    initialize_lock_released=locked_guard) != first:
        raise ValueError("fresh 目标目录在阶段快照期间变化")
    if stage == "prepared":
        registration = read_bound_json(
            workspace / "registration.json", binding(workspace / "registration.json"))
        request_descriptor = registration["request"]
        request = read_bound_json(Path(request_descriptor["path"]), request_descriptor)
        validate_prepared_tree(backend, target, first, request_descriptor, request)
        if expected_document is not None and read_bound_json(
                target / "prepare.json", binding(target / "prepare.json")) != expected_document:
            raise ValueError("fresh 目标 prepare 文件不同于叶子执行结果")
    predecessor = binding(workspace / "prepared-files.json") if stage == "initialized" else None
    result = {
        "format_version": 1,
        "kind": f"devex-clone-fresh-target-{stage}-files",
        "registration": binding(workspace / "registration.json"),
        "predecessor": predecessor,
        "files": first,
    }
    path = workspace / f"{stage}-files.json"
    write_json(path, result)
    descriptor = binding(path)
    if snapshot(workspace, value, stage, locked_guard=locked_guard,
                initialize_lock_released=locked_guard) != descriptor:
        raise ValueError("fresh 目标目录快照发布期间变化")
    return descriptor


def resume_prepared_files(workspace: Path, value: dict) -> dict:
    """返回完整 prepare 快照绑定；叶子执行器会再次核对登记和前像。"""
    path = workspace / "prepared-files.json"
    before = binding(path)
    document = read_bound_json(path, before)
    exact(document, {"format_version", "kind", "registration", "predecessor", "files"})
    if (document["format_version"] != 1
            or document["kind"] != "devex-clone-fresh-target-prepared-files"
            or document["registration"] != binding(workspace / "registration.json")
            or document["predecessor"] is not None
            or not isinstance(document["files"], list)
            or binding(path) != before):
        raise ValueError("fresh 目标 prepare 文件快照绑定无效")
    return {"descriptor": before, "registration": document["registration"],
            "predecessor": document["predecessor"]}


def prepared_baseline(backend: Path, output: Path, request_descriptor: dict,
                      request: dict) -> list[dict]:
    """在首次资源访问前确认 prepare 树恰好是已声明的只读证据。"""
    files = target_files(output, ignored={"initialize.lock"}, locked_guard=True)
    validate_prepared_tree(
        backend, output, files, request_descriptor, request, locked_guard=True)
    return files


def _inventory_paths(output: Path, result: dict,
                     evidence_files: tuple[dict, ...]) -> set[str]:
    directory = output / "inventory-initial"
    if not isinstance(evidence_files, tuple) or result.get("receipt") != binding(
            directory / "inventory.json"):
        raise ValueError("首次初始化库存没有返回精确文件闭合集")
    paths = {"inventory-initial"}
    seen = set()
    for item in evidence_files:
        exact(item, {"path", "type", "bytes", "sha256"})
        name = item["path"]
        if (item["type"] != "file" or not isinstance(name, str)
                or Path(name).name != name or name in seen
                or file_digest(directory / name) != {key: item[key] for key in ("bytes", "sha256")}):
            raise ValueError("首次初始化库存文件描述无效")
        seen.add(name)
        paths.add("inventory-initial/" + name)
    if "inventory.json" not in seen:
        raise ValueError("首次初始化库存闭合集缺少成功收据")
    return paths


def validate_complete(backend: Path, output: Path, baseline: list[dict], request: dict,
                      original: dict, initial: dict, inventory_files: tuple[dict, ...],
                      result: dict) -> list[dict]:
    """只接受固定创建、reset、迁移、库存和最终发布组成的完整树。"""
    from devex_clone_target import target_evidence_hooks
    from devex_clone_target_resume_evidence import (
        _creation_evidence, _initialization_paths, _reset_evidence,
        migration_operations, migration_prefix)

    creation = _creation_evidence(output, request)
    reset = _reset_evidence(target_evidence_hooks(), output, request, original)
    operations = migration_operations(original["maintenance"])
    completed = migration_prefix(output, operations, complete=True)["completed"]
    required = _initialization_paths(output, creation, reset, completed)
    required.discard("failure.json")
    required.update(_inventory_paths(output, initial, inventory_files))
    required.update({"initialized-candidate.json", "initialized.json"})
    if result != read_json(output / "initialized.json"):
        raise ValueError("首次初始化最终收据内容变化")
    current = target_files(output, ignored={"initialize.lock"}, locked_guard=True)
    validate_initialization_delta(output, current, baseline, request, required,
                                  observation_rounds=12 + len(completed),
                                  initialized_objects=result["objects"])
    return current


def _maps(files: list[dict]) -> dict[str, dict]:
    return {item["path"]: item for item in files}


def _delta(output: Path, before: list[dict]) -> tuple[list[dict], dict[str, dict]]:
    from devex_clone_target_state import generation_checkpoint

    generation_checkpoint(output)
    current = target_files(output, ignored={"initialize.lock"}, locked_guard=True)
    old, new = _maps(before), _maps(current)
    if any(new.get(path) != descriptor for path, descriptor in old.items()):
        raise ValueError("迁移续作锁内已有目标证据缺失或变化")
    return current, {path: descriptor for path, descriptor in new.items() if path not in old}


def unchanged(output: Path, expected: list[dict]) -> None:
    from devex_clone_target_state import generation_checkpoint

    generation_checkpoint(output)
    if target_files(output, ignored={"initialize.lock"}, locked_guard=True) != expected:
        raise ValueError("迁移续作下次资源访问前目标证据树发生变化")


def claim_one(output: Path, before: list[dict], path: Path, value: dict | None = None) -> list[dict]:
    current, additions = _delta(output, before)
    relative = path.relative_to(output).as_posix()
    if set(additions) != {relative} or additions[relative].get("type") != "file":
        raise ValueError("迁移续作阶段没有产生唯一声明文件")
    if value is not None and read_json(path) != value:
        raise ValueError("迁移续作阶段声明文件内容变化")
    return current


def claim_many(output: Path, before: list[dict], paths: list[Path]) -> list[dict]:
    current, additions = _delta(output, before)
    expected = {path.relative_to(output).as_posix() for path in paths}
    if set(additions) != expected or any(item.get("type") != "file"
                                         for item in additions.values()):
        raise ValueError("迁移续作阶段文件集合不同于精确声明")
    return current


def _receipt(output: Path, name: str) -> dict:
    value = read_json(output / name)
    exact(value, COMMAND_FIELDS)
    if (not isinstance(value["command"], list) or not value["command"]
            or any(not isinstance(item, str) or not item for item in value["command"])
            or value["returncode"] != 0 or type(value["returncode"]) is not int
            or value["error_type"] is not None or not isinstance(value["stdout"], str)
            or value["stderr"] != ""):
        raise ValueError("迁移续作只读观察命令收据无效")
    value["stdout"] = value["stdout"].replace("\r\n", "\n")
    return value


def _stage(additions: dict[str, dict], stage: str, count: int) -> list[str]:
    pattern = re.compile(re.escape(stage) + rf"-{_UUID}\.command\.json")
    found = sorted(path for path in additions if pattern.fullmatch(path))
    if len(found) != count:
        raise ValueError(f"迁移续作 {stage} 观察收据数量无效")
    return found


def _baseline_stage(output: Path, prepared: list[dict], stage: str) -> list[dict]:
    pattern = re.compile(re.escape(stage) + rf"-{_UUID}\.command\.json")
    return [_receipt(output, item["path"]) for item in prepared
            if pattern.fullmatch(item["path"])]


def _process_identity(value: str) -> tuple[str, str]:
    head, marker, fields = value.strip().rpartition(")")
    parts = fields.split()
    if not marker or not head or len(parts) < 20 or not parts[19].isdigit():
        raise ValueError("迁移续作 Redis 内核身份输出无效")
    return head + marker, parts[19]


def _fixture_redis_observations(baseline: list[dict], observed: list[dict], cache: dict) -> bool:
    redis, expected = cache["redis"], {}
    for item in baseline:
        command = list(item["command"])
        match = re.fullmatch(r"/proc/(\d+)/(stat|exe)", command[-1])
        kind = match.group(2) if match else None
        if match:
            command[-1] = f"/proc/{redis['pid']}/{kind}"
        expected[tuple(command)] = (item, kind)
    if len(expected) != 5 or {tuple(item["command"]) for item in observed} != set(expected):
        return False
    for item in observed:
        baseline_item, kind = expected[tuple(item["command"])]
        if kind == "stat":
            old_head, _ = _process_identity(baseline_item["stdout"])
            _, separator, comm = old_head.partition(" ")
            if not separator or _process_identity(item["stdout"]) \
                    != (f"{redis['pid']} {comm}", redis["started"]):
                return False
        elif item["stdout"] != baseline_item["stdout"]:
            return False
    return True


def _fixture_storage_layout(output: Path, prepared: list[dict], observed: str,
                            runtime: dict | None) -> bool:
    layouts = [read_json(output / item["path"]) for item in prepared
               if re.fullmatch(rf"storage-layout-{_UUID}\.json", item["path"])]
    if len(layouts) != 2 or layouts[0] != layouts[1]:
        return False
    expected = layouts[0]
    if runtime is not None:
        expected = {**expected,
                    "process_receipt": runtime["storage"]["process_receipt"],
                    "launch_receipt": runtime["storage"]["launch_receipt"],
                    "runtime_transition": runtime}
    return read_json(output / observed) == expected


def claim_runtime(output: Path, before: list[dict], prepared: list[dict], *,
                  storage_runtime: dict | None = None,
                  cache_runtime: dict | None = None) -> list[dict]:
    """声明 context/unchanged 各自唯一的一轮存储与 Redis 内核观察。"""
    current, additions = _delta(output, before)
    redis_paths = _stage(additions, "redis-kernel", 5)
    layout_paths = [path for path in additions
                    if re.fullmatch(rf"storage-layout-{_UUID}\.json", path)]
    if set(additions) != set(redis_paths) | set(layout_paths) or len(layout_paths) != 1:
        raise ValueError("迁移续作存储代次观察包含未知或缺失文件")
    baseline = _baseline_stage(output, prepared, "redis-kernel")
    commands = {tuple(item["command"]): item["stdout"] for item in baseline}
    observed = [_receipt(output, path) for path in redis_paths]
    from devex_clone_target_runtime_evidence import fixture_restart_generation

    fixture = fixture_restart_generation(storage_runtime, cache_runtime)
    exact_baseline = (len(baseline) == 10 and len(commands) == 5
                      and Counter(tuple(item["command"]) for item in baseline)
                      == Counter({command: 2 for command in commands}))
    exact_observed = (fixture is not None and _fixture_redis_observations(
        baseline, observed, cache_runtime
    ) or fixture is None and Counter(tuple(item["command"]) for item in observed)
        == Counter({command: 1 for command in commands}))
    if not exact_baseline or not exact_observed:
        raise ValueError("迁移续作 Redis 内核观察不是唯一完整轮次")
    for item in (() if fixture is not None else observed):
        expected = commands[tuple(item["command"])]
        if "/proc/" in item["command"][-1] and item["command"][-1].endswith("/stat"):
            if _process_identity(item["stdout"]) != _process_identity(expected):
                raise ValueError("迁移续作 Redis 进程创建身份变化")
        elif item["stdout"] != expected:
            raise ValueError("迁移续作 Redis 工具、配置或路径观察变化")
    if not _fixture_storage_layout(
            output, prepared, layout_paths[0], storage_runtime if fixture is not None else None):
        raise ValueError("迁移续作存储目录观察不属于 prepare 固定代次")
    return current


def _static_receipts(output: Path, additions: dict[str, dict], prepared: list[dict],
                     request: dict) -> set[str]:
    names = set()
    for stage in ("bucket-readiness", "bucket-versioning", "scoped-list"):
        paths = _stage(additions, stage, 5)
        baseline = _baseline_stage(output, prepared, stage)
        observed = [_receipt(output, path) for path in paths]
        expected_commands = Counter(tuple(value["command"]) for value in baseline)
        if (len(baseline) != 5
                or Counter(tuple(value["command"]) for value in observed) != expected_commands):
            raise ValueError("迁移续作对象观察不是逐桶唯一完整轮次")
        if stage != "scoped-list":
            if Counter((tuple(value["command"]), value["stdout"]) for value in observed) \
                    != Counter((tuple(value["command"]), value["stdout"]) for value in baseline):
                raise ValueError("迁移续作对象端点或版本观察变化")
        else:
            owner = request["target"]["scope_id"] + "/.ryframe-owner"
            for value in observed:
                page = json.loads(value["stdout"] or "{}")
                keys = [item.get("Key") for item in page.get("Contents", [])]
                if page.get("IsTruncated") is not False or keys != [owner]:
                    raise ValueError("迁移续作对象前缀不是唯一 owner")
        names.update(paths)
    return names


def _owner_files(output: Path, additions: dict[str, dict], request: dict) -> set[str]:
    receipt_paths = _stage(additions, "owner-read", 5)
    body_paths = sorted(path for path in additions
                        if re.fullmatch(rf"owner-[a-z0-9-]+-{_UUID}\.bin", path))
    if len(body_paths) != 5:
        raise ValueError("迁移续作对象 owner 文件数量无效")
    scope, seen = request["target"]["scope_id"], set()
    for path in receipt_paths:
        receipt = _receipt(output, path)
        command = receipt["command"]
        if "s3api" not in command or "get-object" not in command or "--bucket" not in command:
            raise ValueError("迁移续作对象 owner 命令无效")
        bucket = command[command.index("--bucket") + 1]
        body = Path(command[-1])
        relative = body.relative_to(output).as_posix()
        if (bucket in seen or relative not in body_paths or "--key" not in command
                or command[command.index("--key") + 1] != f"{scope}/.ryframe-owner"):
            raise ValueError("迁移续作对象 owner 命令与目标文件不一致")
        expected = f"ryframe-owner:v1:{scope}:object-storage:{bucket}".encode()
        if file_digest(body) != {"bytes": len(expected),
                                 "sha256": hashlib.sha256(expected).hexdigest()}:
            raise ValueError("迁移续作对象 owner 字节无效")
        seen.add(bucket)
    if seen != set(BUCKETS):
        raise ValueError("迁移续作对象 owner 未完整覆盖五桶")
    return set(receipt_paths) | set(body_paths)


def claim_preimage(output: Path, before: list[dict], prepared: list[dict], request: dict,
                   started_path: Path, started: dict) -> list[dict]:
    """声明 started 前像只包含四库、五桶 owner、Redis 与当前代次。"""
    current, additions = _delta(output, before)
    expected = _static_receipts(output, additions, prepared, request)
    expected |= _owner_files(output, additions, request)
    for database in request["target"]["databases"]:
        paths = _stage(additions, "mysql-" + database["key"], 1)
        baseline = _baseline_stage(output, prepared, "mysql-" + database["key"])
        observed = _receipt(output, paths[0])
        if (len(baseline) != 1 or observed["command"] != baseline[0]["command"]
                or observed["stdout"] != database["server_uuid"] + "\n"
                + database["database"] + "\n"):
            raise ValueError("迁移续作 MySQL 前像命令或输出无效")
        expected.update(paths)
    relative = started_path.relative_to(output).as_posix()
    expected.add(relative)
    if set(additions) != expected or read_json(started_path) != started:
        raise ValueError("迁移续作 started 前像包含未知文件或内容变化")
    return current


def claim_objects(output: Path, before: list[dict], prepared: list[dict],
                  request: dict) -> list[dict]:
    current, additions = _delta(output, before)
    expected = _static_receipts(output, additions, prepared, request)
    expected |= _owner_files(output, additions, request)
    if set(additions) != expected:
        raise ValueError("迁移续作最终对象观察包含未知文件")
    return current


def claim_inventory(output: Path, before: list[dict], directory: Path,
                    result: dict, evidence_files: tuple[dict, ...]) -> list[dict]:
    """库存成功目录只能新增其结果绑定、原始像、绑定和命令收据。"""
    current, additions = _delta(output, before)
    prefix = directory.name + "/"
    if not isinstance(evidence_files, tuple):
        raise ValueError("迁移续作库存生产者未返回不可变文件闭合集")
    expected = {directory.name: {"path": directory.name, "type": "directory"}}
    for item in evidence_files:
        exact(item, {"path", "type", "bytes", "sha256"})
        relative = item["path"]
        if (item["type"] != "file" or not isinstance(relative, str)
                or Path(relative).name != relative or prefix + relative in expected):
            raise ValueError("迁移续作库存生产者文件描述无效")
        expected[prefix + relative] = {**item, "path": prefix + relative}
    if additions != expected:
        raise ValueError("迁移续作库存阶段文件不同于生产者精确闭合集")
    receipt_path = directory / "inventory.json"
    if result.get("receipt") != binding(receipt_path):
        raise ValueError("迁移续作库存结果没有绑定唯一成功收据")
    receipt = read_json(receipt_path)
    inventories = receipt.get("inventories")
    if not isinstance(inventories, dict):
        raise ValueError("迁移续作库存收据缺少原始像清单")
    known = {"inventory.json", *inventories}
    for name, descriptor in inventories.items():
        if Path(name).name != name or file_digest(directory / name) != descriptor:
            raise ValueError("迁移续作库存原始像绑定变化")
    bindings = {name for name in ("binding-before.json", "binding-after.json")
                if (directory / name).is_file()}
    if bindings and bindings != {"binding-before.json", "binding-after.json"}:
        raise ValueError("迁移续作库存来源绑定不完整")
    if bindings and read_json(directory / "binding-before.json") != read_json(
            directory / "binding-after.json"):
        raise ValueError("迁移续作库存期间来源绑定变化")
    known |= bindings
    produced = {item["path"] for item in evidence_files}
    if not known.issubset(produced):
        raise ValueError("迁移续作库存核心收据不属于生产者文件闭合集")
    return current
