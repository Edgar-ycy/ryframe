"""将已排空的 seed 运行登记为下一轮复制源；仅写本地不可变证据。"""
from __future__ import annotations

from collections.abc import Callable
import copy
import hashlib
from pathlib import Path
import subprocess

from devex_clone_capture import read_json, write_json
from devex_clone_factory_context import Environments, initialization_history
from devex_clone_model import exact, linked, local_path, name
from devex_clone_post import registration as post_registration
from devex_clone_post_process import PHASES
from devex_clone_producer_lineage import collect_producer_lineage
from devex_clone_run_state import binding, load_state
from devex_clone_seed import (history as seed_history, latest, registered,
                              verified_identity_stage)
from devex_clone_seed_close import validate_result as validate_close_result
from devex_clone_seed_runtime import runtime_inputs
from devex_clone_quota_model import capacity_evidence
from devex_clone_source_proof import (bound_file, require_recorded_producer_stopped,
                                      validate_request, verify_generation)
from devex_clone_storage import current_storage_binding
from devex_clone_target_binding import request_binding
from restore_reference_plan import plan_hash

REGISTRATION_FIELDS = {
    "format_version", "kind", "run_directory", "run_manifest", "source_to_seed",
    "post_copy", "post_verify", "seed_registration", "seed_registration_stage",
    "capacity", "departments", "identity", "seed_handoff", "seed_close",
    "producer_lineage", "source_storage", "source_request", "generation_verified",
    "source_environment", "remote_writes", "outbox_drained", "restore_qualified",
}
STORAGE_FIELDS = {
    "format_version", "kind", "run_directory", "run_manifest", "side", "storage",
}
RESULT_FIELDS = {
    "status", "registration", "source_request", "source_storage", "generation_verified",
    "remote_writes", "outbox_drained", "restore_qualified",
}
EVIDENCE_SEQUENCE = (
    "producer-lineage.json", "source-storage.json", "producers.json",
    "source-request.json", "generation-verified.json", "source-registration.json",
)
EVIDENCE_FILES = set(EVIDENCE_SEQUENCE)
ALLOWED_AFTER_CLOSE = {
    ("seed-runtime", "source-register"),
    ("seed-runtime", "source-rebind"),
    ("seed-runtime", "source-export"),
    ("seed-runtime", "source-export-reconcile"),
    ("seed-runtime", "arm-input"),
    ("seed-runtime", "stop"),
    ("seed-runtime", "recover"),
    ("seed-runtime", "recover-session"),
    ("storage-target", "restart"),
    ("storage-target", "status"),
    ("storage-target", "stop"),
    ("storage-target", "recover"),
    ("cache-target", "restart"),
    ("cache-target", "status"),
    ("cache-target", "stop"),
    ("cache-target", "recover"),
    ("cache-target", "reconcile"),
    ("cache-target", "resume"),
}


def _validate_evidence_bindings(backend: Path, value) -> None:
    """递归复核登记中所有显式文件绑定，不把普通业务对象误作收据。"""
    if isinstance(value, dict):
        if set(value) == {"path", "bytes", "sha256"}:
            bound_file(backend, value)
            return
        for item in value.values():
            _validate_evidence_bindings(backend, item)
    elif isinstance(value, list):
        for item in value:
            _validate_evidence_bindings(backend, item)


def _registered_source(backend: Path, descriptor: dict, *, live_storage: bool,
                      validate_seed_target: Callable[[Path, dict], tuple[dict, dict]]) -> dict:
    """恢复发布源，并由调用方选择 seed 初始化请求的执行资格规则。"""
    backend = backend.resolve(strict=True)
    result_path = bound_file(backend, descriptor)
    if result_path.parent.name != "results":
        raise ValueError("seed 源外层结果不属于统一运行 results 目录")
    directory = local_path(backend, str(result_path.parent.parent))
    records = [item for item in load_state(directory)["attempts"]
               if item["stage"] == "seed-runtime" and item["mode"] == "source-register"]
    if (not records or records[-1]["status"] != "passed" or records[-1]["result"] != descriptor
            or result_path != directory / "results" / f"{records[-1]['number']:04d}.json"):
        raise ValueError("seed 源必须绑定最新已通过的 source-register 外层结果")

    result = read_json(result_path)
    exact(result, RESULT_FIELDS)
    if (result["status"] != "seed_source_registered" or result["remote_writes"] != 0
            or result["outbox_drained"] is not True or result["restore_qualified"] is not False):
        raise ValueError("seed 源外层结果没有完整排空并发布")
    registration_path = bound_file(backend, result["registration"])
    if (registration_path.name != "source-registration.json"
            or registration_path.parent.parent != directory / "seed-runtime"):
        raise ValueError("seed 源登记文件不属于原统一运行的固定 attempt")
    registration = read_json(registration_path)
    exact(registration, REGISTRATION_FIELDS)
    if (registration["format_version"] != 1
            or registration["kind"] != "devex-clone-seed-source-registration"
            or registration["run_directory"] != str(directory)
            or registration["run_manifest"] != binding(directory / "manifest.json")
            or registration["source_request"] != result["source_request"]
            or registration["source_storage"] != result["source_storage"]
            or registration["generation_verified"] != result["generation_verified"]
            or registration["remote_writes"] != 0 or registration["outbox_drained"] is not True
            or registration["restore_qualified"] is not False):
        raise ValueError("seed 源内层登记与外层发布结果不一致")
    _validate_evidence_bindings(backend, registration)

    request = read_json(bound_file(backend, registration["source_request"]))
    validate_request(backend, request)
    storage = read_json(bound_file(backend, registration["source_storage"]))
    exact(storage, STORAGE_FIELDS)
    if (storage["format_version"] != 1 or storage["kind"] != "devex-clone-seed-source-storage"
            or storage["run_directory"] != str(directory)
            or storage["run_manifest"] != registration["run_manifest"] or storage["side"] != "target"):
        raise ValueError("seed 源存储登记不属于原 run 的 target 代次")
    generation = read_json(bound_file(backend, registration["generation_verified"]))
    if (generation.get("request_sha256") != plan_hash(request)
            or generation.get("worktree_fingerprint") != request["worktree_fingerprint"]
            or generation.get("external_writers_discovered") is not False
            or generation.get("clone_verified") is not False
            or generation.get("restore_qualified") is not False):
        raise ValueError("seed 源 generation 收据不属于已发布请求")

    original_manifest = read_json(bound_file(backend, registration["run_manifest"]))
    if original_manifest.get("copy_stage") != "source_to_seed":
        raise ValueError("seed 源只能继承完整 source_to_seed 运行")
    _, seed_target = initialization_history(
        backend, bound_file(backend, original_manifest["initialized"]))
    validate_seed_target(backend, seed_target)
    handoff = read_json(bound_file(backend, registration["seed_handoff"]))
    expected_source = {**copy.deepcopy(seed_target["target"]),
                       "runtime_dir": request["source"]["runtime_dir"],
                       "api_url": handoff["api_url"]}
    if (seed_target["side"] != "seed" or request["source"] != expected_source
            or request["maintenance_build"] != seed_target["maintenance_build"]
            or {role: request["tools"][role] for role in ("mysql", "aws")} != seed_target["tools"]
            or storage["storage"]["api_url"] != request["source"]["s3"]["endpoint"]):
        raise ValueError("seed 源请求没有精确派生自原 seed 初始化")
    environment = read_json(bound_file(backend, registration["source_environment"]))
    exact(environment, {"environment"})
    if (not isinstance(environment["environment"], dict)
            or any(not isinstance(key, str) or not isinstance(value, str)
                   for key, value in environment["environment"].items())):
        raise ValueError("seed 源环境必须是固定文本映射")
    return {"directory": directory, "result": result, "registration": registration,
            "request": request, "storage": storage, "generation": generation,
            "manifest": original_manifest, "seed_target": seed_target,
            "environment": environment}


def _published_source(backend: Path, descriptor: dict, *, live_storage: bool,
                      validate_seed_target: Callable[[Path, dict], tuple[dict, dict]]) -> dict:
    from devex_clone_seed_rebind import resolve_storage

    source = _registered_source(backend, descriptor, live_storage=False,
                                validate_seed_target=validate_seed_target)
    state = load_state(source["directory"])
    result = resolve_storage(backend, descriptor, source, state, live_storage=live_storage,
                              current_storage=current_storage_binding)
    if load_state(source["directory"]) != state:
        raise ValueError("seed 发布源核验期间阶段历史变化")
    return result


def published_source(backend: Path, descriptor: dict, *, live_storage: bool = False) -> dict:
    """从已发布外层结果恢复唯一 ready seed 源；可选复核其当前 RustFS 代次。"""
    return _published_source(backend, descriptor, live_storage=live_storage,
                             validate_seed_target=request_binding)


def _attempt_binding(backend: Path, directory: Path, attempt: dict, label: str) -> dict:
    descriptor = attempt.get("result")
    if not isinstance(descriptor, dict):
        raise ValueError(f"{label}缺少外层结果绑定")
    path = bound_file(backend, descriptor)
    if path != directory / "results" / f"{attempt['number']:04d}.json":
        raise ValueError(f"{label}外层结果不属于固定 run attempt")
    return descriptor


def _latest_close(backend: Path, directory: Path, state: dict, runtime: Path,
                  handoff: dict, current: int) -> dict:
    records = [item for item in state["attempts"]
               if item["stage"] == "seed-runtime" and item["mode"] == "close"
               and item["number"] != current]
    if not records or records[-1]["status"] != "passed":
        raise ValueError("最新 seed close 尚未成功发布")
    record = records[-1]
    descriptor = _attempt_binding(backend, directory, record, "seed close")
    value = read_json(bound_file(backend, descriptor))
    validate_close_result(value, runtime, handoff)
    if value["status"] != "seed_runtime_closed" or value["outbox_drained"] is not True:
        raise ValueError("seed close 未取得完整 outbox 排空终态")
    later = [item for item in state["attempts"] if record["number"] < item["number"] < current
             and (item["stage"], item["mode"]) not in ALLOWED_AFTER_CLOSE]
    if later:
        raise ValueError("seed close 后出现未重新排空的阶段，不能登记复制源")
    return descriptor


def _lineage_inputs(backend: Path, directory: Path, state: dict) -> tuple[list[Path], list[dict]]:
    active = post_registration(backend, directory, cleanup=True)
    runtimes = [directory / "post-copy/runtime",
                *(directory / "post-copy" / f"runtime-{number:04d}"
                  for number in range(1, active.sequence + 1)),
                directory / "seed-runtime/runtime"]
    phases = {(stage, mode) for stage, modes in PHASES.values() for mode in modes}
    attempts = []
    for item in state["attempts"]:
        if (item["stage"], item["mode"]) in phases:
            controller = directory / f"controller-{item['number']:04d}.json"
            attempts.append({"attempt": copy.deepcopy(item), "controller": binding(controller)})
    return runtimes, attempts


def _runtime_identities(lineage: dict, runtime: Path) -> dict[str, dict]:
    matches = [item for item in lineage["runtimes"]
               if item["runtime_directory"] == str(runtime)]
    if len(matches) != 1:
        raise ValueError("producer lineage 未唯一覆盖 seed runtime")
    receipts = matches[0]["role_receipts"]
    result = {item["role"]: item["identity"] for item in receipts}
    if set(result) != {"api", "worker"}:
        raise ValueError("seed runtime 最新 API/Worker 收据不完整")
    return result


def producer_registry(lineage: dict, runtime: Path, runtime_binding: dict,
                      scope_id: str) -> dict:
    """把 lineage 的每个创建代次恰好登记一次，并保留最新角色名。"""
    roles = _runtime_identities(lineage, runtime)
    identities = [item["identity"] for item in lineage["identities"]]
    keys = [(item["pid"], item["started"]) for item in identities]
    if len(keys) != len(set(keys)) or len(identities) < 3:
        raise ValueError("producer lineage 创建代次重复或不足")
    role_keys = {(value["pid"], value["started"]): role for role, value in roles.items()}
    processes, sequence = [], 0
    for identity in identities:
        key = (identity["pid"], identity["started"])
        role = role_keys.get(key)
        if role is None:
            sequence += 1
            role = f"history-{sequence:04d}"
        processes.append({"name": role, "identity": copy.deepcopy(identity)})
    if len({item["name"] for item in processes}) != len(processes):
        raise ValueError("producer registry 名称重复")
    return {"format_version": 1, "scope_id": scope_id,
            "runtime_sha256": runtime_binding["sha256"], "processes": processes}


def _source_id(value: str) -> str:
    """保留短 ID 可读性，并为达到上限的 run ID 派生稳定名称。"""
    original = name(value)
    candidate = f"{original}-seed"
    if len(candidate) <= 48:
        return name(candidate)
    return name(f"seed-{hashlib.sha256(original.encode('utf-8')).hexdigest()[:32]}")


def _source_request(backend: Path, value: dict, post: dict, runtime: Path,
                    handoff: dict, registry: dict, registry_binding: dict) -> dict:
    original = read_json(bound_file(backend, value["source_request"]))
    validate_request(backend, original)
    _, target = initialization_history(backend, bound_file(backend, value["initialized"]))
    request_binding(backend, target)
    maintenance = read_json(bound_file(backend, target["maintenance_build"]))
    source_info = maintenance.get("source")
    if (not isinstance(source_info, dict)
            or not isinstance(source_info.get("worktree_fingerprint"), str)):
        raise ValueError("目标维护构建缺少产品来源指纹")
    source = {**copy.deepcopy(target["target"]), "runtime_dir": str(runtime),
              "api_url": handoff["api_url"]}
    runtime_binding = binding(runtime / "runtime.json")
    result = {
        "format_version": 1,
        "kind": "devex-clone-source-export",
        "id": _source_id(value["id"]),
        "source": source,
        "tools": {"mysql": copy.deepcopy(target["tools"]["mysql"]),
                  "mysqldump": copy.deepcopy(original["tools"]["mysqldump"]),
                  "aws": copy.deepcopy(target["tools"]["aws"]),
                  "node": copy.deepcopy(post["node"])},
        "backend_build": copy.deepcopy(handoff["backend_build"]),
        "maintenance_build": copy.deepcopy(target["maintenance_build"]),
        "worktree_fingerprint": source_info["worktree_fingerprint"],
        "runtime": runtime_binding,
        "processes": {role: binding(runtime / f"{role}.json") for role in ("api", "worker")},
        "producers_registry": copy.deepcopy(registry_binding),
        "max_object_bytes": original["max_object_bytes"],
    }
    validate_request(backend, result)
    if (registry["scope_id"] != source["scope_id"]
            or registry["runtime_sha256"] != runtime_binding["sha256"]):
        raise ValueError("producer registry 不属于派生 seed 源运行")
    return result


def _storage(backend: Path, directory: Path, value: dict) -> dict:
    observed = current_storage_binding(backend, directory, "target")
    if observed is None:
        raise ValueError("seed 源必须绑定本 run 实际重启的 target RustFS")
    _, target = initialization_history(backend, bound_file(backend, value["initialized"]))
    request_binding(backend, target)
    expected = target["storage"]["rustfs"]
    if (observed["api_url"] != target["target"]["s3"]["endpoint"]
            or observed["storage"]["sha256"] != expected["sha256"]
            or Path(observed["storage"]["identity"]["executable"]).resolve()
            != Path(expected["identity"]["executable"]).resolve()):
        raise ValueError("seed 源 RustFS 端点或二进制不属于旧 run 的 target")
    return observed


def _anchors(backend: Path, directory: Path, value: dict, current: int, *, run) -> dict:
    if value["copy_stage"] != "source_to_seed":
        raise ValueError("只有完整 source_to_seed run 可登记 seed 复制源")
    from devex_clone_run import require_target_copy

    verified = require_target_copy(backend, directory, value, ("api",))
    if verified.plan["copy_stage"] != "source_to_seed":
        raise ValueError("原复制计划不是 source_to_seed")
    seed = registered(backend, directory)
    post = seed_history(backend, directory, seed)
    identity = verified_identity_stage(backend, directory, seed)
    capacity = capacity_evidence(backend, directory, seed)
    runtime, handoff, private = runtime_inputs(backend, directory)
    if identity != handoff["identity"] or capacity != handoff["capacity"]:
        raise ValueError("seed handoff 未绑定当前身份、部门及容量验证")
    state = load_state(directory)
    close = _latest_close(backend, directory, state, runtime, handoff, current)
    runtimes, attempts = _lineage_inputs(backend, directory, state)
    lineage = collect_producer_lineage(backend, directory, runtimes, attempts)
    for item in lineage["identities"]:
        require_recorded_producer_stopped(item["identity"], run=run)
    register_stage = latest(directory, "seed-runtime", {"register"})
    source_to_seed = {"copy_stage_receipt": post["copy_stage_receipt"],
                      "copy_result": post["copy_result"], "ledger_head": post["ledger_head"]}
    return {
        "run_manifest": binding(directory / "manifest.json"),
        "source_to_seed": source_to_seed,
        "post_copy": seed["post_copy"],
        "post_verify": seed["post_verify"],
        "seed_registration": binding(directory / "seed-runtime.json"),
        "seed_registration_stage": _attempt_binding(backend, directory, register_stage,
                                                     "seed registration"),
        "capacity": capacity,
        "departments": identity["departments"],
        "identity": identity,
        "seed_handoff": binding(runtime / "handoff.json"),
        "seed_close": close,
        "lineage": lineage,
        "storage": _storage(backend, directory, value),
        "source_environment": post["api_environment"],
        "post": post,
        "runtime": runtime,
        "handoff": handoff,
        "private": private,
    }


def _storage_document(directory: Path, anchors: dict) -> dict:
    return {"format_version": 1, "kind": "devex-clone-seed-source-storage",
            "run_directory": str(directory), "run_manifest": anchors["run_manifest"],
            "side": "target", "storage": anchors["storage"]}


def _registration_document(directory: Path, anchors: dict, *, lineage: dict,
                           source_storage: dict, source_request: dict,
                           generation_verified: dict) -> dict:
    return {
        "format_version": 1,
        "kind": "devex-clone-seed-source-registration",
        "run_directory": str(directory),
        **{field: copy.deepcopy(anchors[field]) for field in (
            "run_manifest", "source_to_seed", "post_copy", "post_verify",
            "seed_registration", "seed_registration_stage", "capacity", "departments",
            "identity", "seed_handoff", "seed_close", "source_environment")},
        "producer_lineage": copy.deepcopy(lineage),
        "source_storage": copy.deepcopy(source_storage),
        "source_request": copy.deepcopy(source_request),
        "generation_verified": copy.deepcopy(generation_verified),
        "remote_writes": 0,
        "outbox_drained": True,
        "restore_qualified": False,
    }


def _write_or_match(path: Path, value: dict) -> None:
    if path.exists() or linked(path):
        if linked(path) or not path.is_file() or read_json(path) != value:
            raise ValueError("source-register 已有局部证据不同，须保留现场并显式核对")
        return
    write_json(path, value)


def _existing_output(directory: Path, state: dict, number: int) -> Path | None:
    found = []
    for attempt in state["attempts"]:
        if (attempt["number"] >= number or attempt["stage"] != "seed-runtime"
                or attempt["mode"] != "source-register"):
            continue
        path = directory / "seed-runtime" / f"attempt-{attempt['number']:04d}"
        if not path.exists() and not linked(path):
            continue
        if linked(path) or not path.is_dir() or any(item.is_dir() or linked(item) for item in path.iterdir()):
            raise ValueError("历史 source-register 输出不是无链接普通文件目录")
        names = {item.name for item in path.iterdir()}
        if names - EVIDENCE_FILES:
            raise ValueError("历史 source-register 含未知局部证据，须显式核对")
        prefixes = {frozenset(EVIDENCE_SEQUENCE[:index])
                    for index in range(len(EVIDENCE_SEQUENCE) + 1)}
        if frozenset(names) not in prefixes:
            raise ValueError("历史 source-register 局部证据不是可恢复的写入前缀")
        found.append(path)
    if len(found) > 1:
        raise ValueError("多个历史 source-register 目录可能含登记，不能猜测唯一来源")
    return found[0] if found else None


def _materialize(backend: Path, directory: Path, value: dict, number: int,
                 output: Path, *, run) -> dict:
    before = _anchors(backend, directory, value, number, run=run)
    _write_or_match(output / "producer-lineage.json", before["lineage"])
    lineage_binding = binding(output / "producer-lineage.json")
    storage_document = _storage_document(directory, before)
    exact(storage_document, STORAGE_FIELDS)
    _write_or_match(output / "source-storage.json", storage_document)
    storage_binding = binding(output / "source-storage.json")
    runtime_binding = binding(before["runtime"] / "runtime.json")
    registry = producer_registry(before["lineage"], before["runtime"], runtime_binding,
                                 before["handoff"]["scope_id"])
    _write_or_match(output / "producers.json", registry)
    registry_binding = binding(output / "producers.json")
    request = _source_request(backend, value, before["post"], before["runtime"],
                              before["handoff"], registry, registry_binding)
    _write_or_match(output / "source-request.json", request)
    request_binding_value = binding(output / "source-request.json")
    with Environments(before["private"], before["private"]).use("target"):
        generation = verify_generation(backend, request, run)
    _write_or_match(output / "generation-verified.json", generation)
    generation_binding = binding(output / "generation-verified.json")
    after = _anchors(backend, directory, value, number, run=run)
    comparable = {key: item for key, item in before.items()
                  if key not in {"post", "runtime", "handoff", "private"}}
    if comparable != {key: item for key, item in after.items()
                      if key not in {"post", "runtime", "handoff", "private"}}:
        raise ValueError("seed 源登记期间原证据、lineage 或存储代次发生变化")
    expected_registry = producer_registry(after["lineage"], after["runtime"], runtime_binding,
                                          after["handoff"]["scope_id"])
    expected_request = _source_request(backend, value, after["post"], after["runtime"],
                                       after["handoff"], expected_registry, registry_binding)
    if read_json(output / "producers.json") != expected_registry or request != expected_request:
        raise ValueError("seed 源请求或 producer registry 在生成期间变化")
    with Environments(after["private"], after["private"]).use("target"):
        if verify_generation(backend, request, run) != generation:
            raise ValueError("seed 源 generation 验证在登记期间变化")
    registration = _registration_document(
        directory, after, lineage=lineage_binding, source_storage=storage_binding,
        source_request=request_binding_value, generation_verified=generation_binding)
    exact(registration, REGISTRATION_FIELDS)
    _write_or_match(output / "source-registration.json", registration)
    descriptor = binding(output / "source-registration.json")
    result = {"status": "seed_source_registered", "registration": descriptor,
              "source_request": request_binding_value, "source_storage": storage_binding,
              "generation_verified": generation_binding, "remote_writes": 0,
              "outbox_drained": True, "restore_qualified": False}
    exact(result, RESULT_FIELDS)
    return result


def register_source(backend: Path, directory: Path, value: dict, number: int, *,
                    run=subprocess.run) -> dict:
    """生成或续接唯一源登记；只观察进程/端口/资源绑定，不写远端资源。"""
    backend, directory = backend.resolve(strict=True), local_path(backend, str(directory))
    state = load_state(directory)
    prior = [item for item in state["attempts"]
             if item["stage"] == "seed-runtime" and item["mode"] == "source-register"
             and item["number"] != number and item["status"] == "passed"]
    if prior:
        raise ValueError("同一 seed run 已发布源登记，不能生成第二份")
    output = _existing_output(directory, state, number)
    if output is None:
        output = directory / "seed-runtime" / f"attempt-{number:04d}"
        if output.exists() or linked(output) or not output.parent.is_dir():
            raise ValueError("seed source-register 输出目录不为空或父目录无效")
        output.mkdir()
    return _materialize(backend, directory, value, number, output, run=run)
