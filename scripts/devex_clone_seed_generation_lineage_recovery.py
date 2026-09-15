"""封存 C68 谱系校验失败，并只授权相邻 C70 重试一次。"""
from __future__ import annotations

import copy
import hashlib
import os
from pathlib import Path
import re
import subprocess

from devex_clone_capture import read_json, write_json
from devex_clone_model import exact, linked, local_path, name
from devex_clone_run_state import binding, controller_record, load_state
from devex_clone_source_proof import bound_file, require_closed_port, require_recorded_producer_stopped
from devex_clone_storage_request import identity
from restore_reference_plan import plan_hash
from source_fingerprints import require_current_execution_source, verify_execution_source
from source_inventory import git


FAILED_ATTEMPT = 68
RECOVERY_ATTEMPT = 69
RETRY_ATTEMPT = 70
FAILED_HEAD = "6b58001aa320e3fe7f1de62b100654b0f2a51edc"
FAILED_TREE = "2c63b19a658f0b0a69bf3855f0cb343046c7539c"
FAILED_FILES = 10_233
FAILED_BYTES = 1_081_069_032
FAILED_MANIFEST = "4cffdac03ff9d83566f56f7ea3114766fdc3acf86e566cd24944a8378567899b"
FAILED_FRAMES = [
    {"file": "scripts/" + file + ".py", "function": function, "line": line}
    for file, function, line in (
        ("devex_clone_run", "execute", 717),
        ("devex_clone_seed_runtime", "execute_seed", 341),
        ("devex_clone_seed_generation", "execute_generation", 369),
        ("restore_source_lineage", "derive_dataset_lineage", 405),
        ("restore_source_lineage", "_dataset_facts", 107),
        ("restore_reference_plan", "validate_plan", 68),
    )
]
STATUS = "seed_source_generation_replay_authorized"
FIELDS = {
    "status", "request", "failed_start", "failure_proof", "history_length", "history_sha256",
    "coordinator_source", "source_registration", "source_rebind", "review_successor", "current_storage",
    "runtime", "before", "after", "evidence_manifest", "source_generation_published", "replay_allowed",
    "remote_writes", "restore_qualified",
}
_TAIL = (
    (61, "cache-target", "stop", "passed", None),
    (62, "storage-target", "stop", "failed", "ValueError"),
    (63, "storage-target", "stop", "passed", None),
    (64, "storage-target", "restart", "failed", "CalledProcessError"),
    (65, "storage-target", "restart", "passed", None),
    (66, "cache-target", "restart", "passed", None),
    (67, "seed-runtime", "source-generation-start", "failed", "ValueError"),
    (68, "seed-runtime", "source-generation-start", "failed", "ValueError"),
)


def pending(attempts: list) -> bool:
    """只识别等待 C69 的固定 C68；不据此证明可重试。"""
    return bool(attempts and attempts[-1].get("number") == FAILED_ATTEMPT
                and tuple(attempts[-1].get(key) for key in ("stage", "mode", "status", "result", "error_type"))
                == ("seed-runtime", "source-generation-start", "failed", None, "ValueError"))


def failed_tail(row: dict, previous: int) -> bool:
    return (previous == FAILED_ATTEMPT - 1 and row.get("number") == FAILED_ATTEMPT
            and tuple(row.get(key) for key in ("stage", "mode", "status", "result", "error_type"))
            == ("seed-runtime", "source-generation-start", "failed", None, "ValueError"))


def _manifest_summary(directory: Path) -> dict:
    from restore_source_runtime import _manifest

    files = _manifest(directory)
    return {"files": len(files), "bytes": sum(item["bytes"] for item in files), "sha256": plan_hash(files)}


def _history(backend: Path, directory: Path, attempts: list) -> tuple[dict, tuple[dict, ...]]:
    from devex_clone_seed_generation_prelaunch import closed, receipt_reregistration_failure

    archive = closed(backend, directory, attempts)
    if archive is None or archive["recovery"].get("number") != 60:
        raise ValueError("C68 谱系失败缺少固定 C60 封存边界")
    index = attempts.index(archive["recovery"])
    tail = attempts[index + 1:index + 1 + len(_TAIL)]
    if (len(tail) != len(_TAIL)
            or any(tuple(row.get(key) for key in ("number", "stage", "mode", "status", "error_type")) != shape
                   for row, shape in zip(tail, _TAIL, strict=True))):
        raise ValueError("C68 谱系失败不属于 C60 后的固定相邻资源续作")
    if any(row.get("result") is not None for row in tail if row["status"] == "failed"):
        raise ValueError("C68 谱系失败历史包含意外发布结果")
    reference = archive["recovery"]["sources"]
    verify_execution_source(reference, "C60 封存控制器")
    for row in tail:
        verify_execution_source(row["sources"], "C68 续作控制器")
        snapshot = row["sources"]["snapshot"]
        if (not snapshot["clean"] or snapshot["files"]
                or snapshot["patch_sha256"] != hashlib.sha256(b"").hexdigest()
                or row["sources"]["fingerprints"]["product"] != reference["fingerprints"]["product"]):
            raise ValueError("C68 续作改变了产品来源或不是干净提交")
    receipt_reregistration_failure(backend, directory, tail[-2], tail[-3])
    return archive, tuple(tail)


def _source(backend: Path, directory: Path, attempts: list, request: dict, tail: tuple[dict, ...]) -> dict:
    from devex_clone_cache import registered_cache_binding
    from devex_clone_seed_rebind import FIELDS as REBIND_FIELDS, transition
    from devex_clone_seed_source import _registered_source
    from devex_clone_storage import registered_storage_binding
    from reference_fixture_successor import _source_with_loader

    registrations = [row for row in attempts if (row["stage"], row["mode"], row["status"])
                     == ("seed-runtime", "source-register", "passed")]
    rebinds = [row for row in attempts if (row["stage"], row["mode"], row["status"])
               == ("seed-runtime", "source-rebind", "passed")]
    if (len(registrations) != 1 or registrations[0]["result"] != request["source_registration"]
            or len(rebinds) != 1 or rebinds[0]["result"] != request["source_rebind"]):
        raise ValueError("C68 谱系失败没有继承唯一 C52 与重绑定")
    rebound_path = bound_file(backend, rebinds[0]["result"])
    if rebound_path != directory / "results" / f"{rebinds[0]['number']:04d}.json":
        raise ValueError("C68 重绑定收据不属于同一复制运行")
    rebound = read_json(rebound_path)
    exact(rebound, REBIND_FIELDS)
    prefix = [row for row in attempts if row["number"] < rebinds[0]["number"]]
    if (rebound["status"] != "seed_source_rebound" or rebound["source_registration"] != request["source_registration"]
            or rebound["review_successor"] != request["review_successor"]
            or rebound["history_length"] != len(prefix) or rebound["history_sha256"] != plan_hash(prefix)
            or rebound["remote_writes"] != 0 or rebound["restore_qualified"] is not False):
        raise ValueError("C68 重绑定没有冻结相同 C52、successor 与完整历史")
    source = _source_with_loader(backend, request["review_successor"], live_storage=False, loader=_registered_source)
    if (source["directory"] != directory
            or source["review_successor"]["source_result"] != request["source_registration"]
            or source["review_successor_binding"] != request["review_successor"]
            or rebound["original_storage"] != source["storage"]["storage"]):
        raise ValueError("C68 请求与原 C52 或 successor 不同")
    transition(backend, rebound["original_storage"], rebound["current_storage"])
    storage = registered_storage_binding(backend, directory, "target", before=FAILED_ATTEMPT)
    cache = registered_cache_binding(
        backend, directory, before=FAILED_ATTEMPT,
        frozen=(request["source_registration"], request["review_successor"]),
    )
    if (storage != request["current_storage"] or storage is None or storage["attempt"] != 65
            or cache is None or cache["attempt"] != tail[-3]["number"]
            or cache["restart_result"] != tail[-3]["result"]):
        raise ValueError("C68 请求没有绑定固定 C65 RustFS 与 C66 Redis 代次")
    transition(backend, rebound["current_storage"], storage)
    result = copy.deepcopy(source)
    result["storage"]["storage"] = copy.deepcopy(storage)
    result["source_rebind"] = request["source_rebind"]
    return result


def _request(backend: Path, output: Path, request_path: Path | None) -> tuple[dict, dict, dict, dict]:
    from devex_clone_seed_generation import REQUEST_FIELDS

    internal_path = output / "request.json"
    internal_descriptor = binding(internal_path)
    internal = read_json(bound_file(backend, internal_descriptor))
    if binding(internal_path) != internal_descriptor:
        raise ValueError("C68 内部请求在读取期间变化")
    exact(internal, REQUEST_FIELDS)
    name(internal["id"])
    selected = internal_path if request_path is None else local_path(backend, str(request_path))
    if request_path is not None and selected == internal_path:
        raise ValueError("C69 必须绑定当前重新登记的外部正式请求，不能复用 C68 内部副本")
    descriptor = binding(selected)
    request = read_json(bound_file(backend, descriptor))
    if binding(selected) != descriptor:
        raise ValueError("C69 当前请求在读取期间变化")
    exact(request, REQUEST_FIELDS)
    if (type(request["format_version"]) is not int or request["format_version"] != 1
            or request["kind"] != "devex-clone-seed-source-generation"):
        raise ValueError("C68 谱系恢复请求与失败代次不同")
    for field in ("source_registration", "source_rebind", "review_successor", "backend_build", "source_environment"):
        bound_file(backend, request[field])
    bound_file(Path(request["execution_backend"]), request["maintenance_build"])
    changes = {key for key in request if request[key] != internal[key]}
    if request_path is not None:
        if changes != {"backend_build", "maintenance_build"}:
            raise ValueError("C69 当前请求必须成对重新登记 API/Worker 与维护构建收据")
        from devex_clone_seed_generation import _reregistered_builds

        _reregistered_builds(backend, internal, request)
    elif changes:
        raise ValueError("C68 内部请求在读取期间变化")
    return request, descriptor, internal_descriptor, internal


def pending_lineage_request(
    backend: Path,
    directory: Path,
    successor: dict,
    execution: Path,
    expected_head: str,
    backend_build: Path,
    maintenance_build: Path,
    environment: Path,
    request_id: str,
    *,
    adapter_contract=None,
    product_backend=None,
) -> tuple[dict, dict] | None:
    """为固定 C68 失败重登等价构建；不放宽普通 published-source。"""
    backend = backend.resolve(strict=True)
    directory = local_path(backend, str(directory))
    execution = local_path(backend, str(execution))
    state_path = directory / "state.json"
    state_descriptor = binding(state_path)
    state = load_state(directory)
    if binding(state_path) != state_descriptor:
        raise ValueError("C69 请求构造期间账本发生变化")
    if not pending(state["attempts"]):
        return None
    _, tail = _history(backend, directory, state["attempts"])
    output = local_path(backend, str(directory / "g0068"))
    previous, _, previous_descriptor, previous_value = _request(backend, output, None)
    if previous != previous_value:
        raise ValueError("C68 内部请求读取结果不一致")

    runtime_path = local_path(backend, str(backend_build))
    maintenance_path = local_path(execution, str(maintenance_build))
    environment_path = local_path(backend, str(environment))
    unchanged = {
        "id": request_id,
        "review_successor": successor,
        "execution_backend": str(execution),
        "expected_backend_sha": expected_head,
        "adapter_contract": adapter_contract,
        "product_backend": None if product_backend is None else str(product_backend),
        "source_environment": binding(environment_path),
    }
    if any(previous[field] != value for field, value in unchanged.items()):
        raise ValueError("C69 重新登记请求改变了构建收据以外的字段")
    request = copy.deepcopy(previous)
    request.update(
        backend_build=binding(runtime_path),
        maintenance_build=binding(maintenance_path),
    )
    if (request["backend_build"] == previous["backend_build"]
            or request["maintenance_build"] == previous["maintenance_build"]):
        raise ValueError("C69 必须成对重新登记新的 API/Worker 与维护构建收据")
    from devex_clone_seed_generation import _reregistered_builds

    _reregistered_builds(backend, previous, request)
    source = _source(backend, directory, state["attempts"], request, tail)
    _verify_request(backend, request, source)
    current_bindings = (
        binding(runtime_path),
        binding(maintenance_path),
        binding(environment_path),
        binding(Path(successor["path"])),
    )
    if (current_bindings != (
            request["backend_build"], request["maintenance_build"],
            request["source_environment"], request["review_successor"])
            or binding(output / "request.json") != previous_descriptor
            or binding(state_path) != state_descriptor
            or load_state(directory) != state):
        raise ValueError("C69 重新登记请求的账本或输入在核验期间变化")
    return request, source


def _verify_request(backend: Path, request: dict, source: dict) -> None:
    """用已证明的 C52/C56/C65 来源重算正式请求，避开失败尾的普通读取策略。"""
    from devex_clone_seed_generation_runtime import registered_inputs
    from devex_clone_tools import verify_evidence
    from process_environment import configured
    from restore_source_binding import source_binding

    expected = {"format_version": 1, "kind": "devex-clone-seed-source-generation", "id": request["id"],
                "source_registration": source["review_successor"]["source_result"],
                "review_successor": source["review_successor_binding"], "source_rebind": source["source_rebind"],
                "current_storage": source["storage"]["storage"], "execution_backend": request["execution_backend"],
                "expected_backend_sha": request["expected_backend_sha"], "adapter_contract": request["adapter_contract"],
                "product_backend": request["product_backend"], "backend_build": request["backend_build"],
                "maintenance_build": request["maintenance_build"], "source_environment": request["source_environment"]}
    if request != expected or source.get("source_generation") is not None:
        raise ValueError("C68 谱系恢复请求不是重绑定后的首个正式生成请求")
    execution, build = registered_inputs(backend, request, reconstruct=False)
    maintenance = verify_evidence(execution, bound_file(execution, request["maintenance_build"]))
    private = read_json(bound_file(backend, request["source_environment"]))
    exact(private, {"environment"})
    if (maintenance["source"] != build["sources"]["full"]["source"]
            or source_binding(execution, {"source": source["request"]["source"]},
                              configured(private["environment"]), evidence_root=backend)
            != source["generation"]["physical_binding"]):
        raise ValueError("C68 正式请求的构建、维护工具或环境与 C52 物理来源不同")


def _runtime(backend: Path, directory: Path, output: Path, request: dict, source: dict, intent: dict,
             *, run, require_idle: bool) -> tuple[object, dict]:
    from devex_clone_seed_generation_runtime import EXTRA_FILES, GenerationRuntime, registered_inputs
    from devex_clone_seed_rebind import quiet_producers
    from devex_clone_storage import current_storage_binding
    from full_stack_process_tree import validate_process_tree_directory
    from full_stack_runtime import verify_runtime
    from restore_build import verify_build
    from restore_source_binding import source_binding
    from devex_clone_source_proof import verify_api_address

    runtime = GenerationRuntime(backend, directory, output, request, source, run)
    runtime.operations = intent["operations"]
    execution, build = registered_inputs(backend, request, reconstruct=True)
    if execution != runtime.execution or build != runtime.build:
        raise ValueError("C68 运行配置没有绑定同一执行来源和构建")
    verify_build(runtime.execution, runtime.build, request["expected_backend_sha"], run)
    with runtime.environment():
        if any(key in os.environ for key in ("RYFRAME_E2E_FIXTURE", "RYFRAME_FULL_STACK_SOURCE")):
            raise ValueError("C68 运行环境混入独立 fixture")
        if source_binding(runtime.execution, {"source": runtime.selected}, evidence_root=backend) != source["generation"]["physical_binding"]:
            raise ValueError("C68 运行配置改变了 C52 物理来源")
        contract = verify_runtime(runtime.execution, runtime.runtime)
        verify_api_address(runtime.execution, runtime.selected["api_url"])
        if require_idle:
            if current_storage_binding(backend, directory, "target") != request["current_storage"]:
                raise ValueError("C68 谱系恢复前 RustFS 代次已变化")
            quiet_producers(backend, source)
            require_closed_port(contract["worker_ready_url"])
    names = validate_process_tree_directory(runtime.runtime, runtime.operations, extra_files=EXTRA_FILES)
    if names != tuple(sorted(EXTRA_FILES)):
        raise ValueError("C68 runtime 已出现产品进程、树、日志或其他未知文件")
    runtime.contract = contract
    return runtime, contract


def lineage_failure(backend: Path, directory: Path, attempts: list, *, request_path: Path | None = None,
                    run=subprocess.run, require_idle: bool = True) -> dict:
    """重算 C68 固定失败、完整前像和零启动事实；不创建文件。"""
    from devex_clone_seed_generation_images import verify_image
    _, tail = _history(backend, directory, attempts)
    failed = tail[-1]
    source_info = failed["sources"]
    if (source_info["snapshot"]["head"] != FAILED_HEAD
            or git(backend, "rev-parse", FAILED_HEAD + "^{tree}").decode().strip() != FAILED_TREE):
        raise ValueError("C68 谱系失败不属于已审计提交和整树")
    running = {**failed, "status": "running", "finished_at": None, "error_type": None}
    controller, owner = controller_record(directory, FAILED_ATTEMPT, running)
    exact(owner, {"format_version", "identity", "directory", "manifest_sha256"})
    owner_identity = identity(owner["identity"])
    if (type(owner["format_version"]) is not int or owner["format_version"] != 1
            or owner["directory"] != str(directory)
            or owner["manifest_sha256"] != binding(directory / "manifest.json")["sha256"]):
        raise ValueError("C68 控制器不属于固定复制运行")
    failure_path = local_path(backend, str(directory / "failure-0068.json"))
    failure = binding(failure_path)
    expected_failure = {"format_version": 1, "kind": "devex-stage-failure", "attempt": FAILED_ATTEMPT,
                        "stage": "seed-runtime", "mode": "source-generation-start",
                        "error_type": "ValueError", "frames": FAILED_FRAMES, "controller": controller}
    failure_value = read_json(failure_path)
    if (type(failure_value.get("format_version")) is not int
            or failure_value != expected_failure):
        raise ValueError("C68 失败栈没有证明 runtime.start 前的固定谱系校验位置")
    require_recorded_producer_stopped(owner_identity)
    output = local_path(backend, str(directory / "g0068"))
    if ({item.name for item in output.iterdir()} != {"before", "runtime", "intent.json", "request.json"}
            or any(linked(item) for item in output.iterdir())
            or not all((output / item).is_dir() for item in ("before", "runtime"))
            or not all((output / item).is_file() for item in ("intent.json", "request.json"))):
        raise ValueError("C68 generation 目录不是固定的启动前文件集合")
    from restore_source_runtime import _manifest

    files = _manifest(output)
    summary = {"files": len(files), "bytes": sum(item["bytes"] for item in files), "sha256": plan_hash(files)}
    if summary != {"files": FAILED_FILES, "bytes": FAILED_BYTES, "sha256": FAILED_MANIFEST}:
        raise ValueError("C68 generation 完整文件清单变化")
    for absent in (directory / "results/0068.json", directory / "seed-runtime/attempt-0068"):
        local_path(backend, str(absent), new=True)
    intent = read_json(output / "intent.json")
    exact(intent, {"format_version", "kind", "request", "runtime_directory", "operations"})
    operations = intent["operations"]
    if (type(intent["format_version"]) is not int or intent["format_version"] != 1
            or intent["kind"] != "seed-source-runtime-intent"
            or intent["request"] != binding(output / "request.json")
            or intent["runtime_directory"] != str(output / "runtime")
            or set(operations) != {"api", "worker"}
            or any(not isinstance(value, str) or len(value) != 32 for value in operations.values())
            or len(set(operations.values())) != 2):
        raise ValueError("C68 启动意图与固定请求、目录或双角色 operation 不同")
    request, request_descriptor, internal_request, historical_request = _request(
        backend, output, request_path
    )
    source = _source(backend, directory, attempts, request, tail)
    _verify_request(backend, request, source)
    runtime, contract = _runtime(
        backend, directory, output, request, source, intent, run=run, require_idle=require_idle)
    before = binding(output / "before/image.json")
    image = verify_image(backend, before, runtime.selected, source["request"],
                         source_registration=request["source_registration"])
    proof = {"attempt": plan_hash(failed), "failure": failure, "controller": controller,
             "source": source_info, "head": FAILED_HEAD, "tree": FAILED_TREE, "manifest": summary,
             "request": internal_request, "intent": binding(output / "intent.json"),
             "runtime": binding(output / "runtime/runtime.json"), "before": before}
    if (controller_record(directory, FAILED_ATTEMPT, running) != (controller, owner)
            or binding(failure_path) != failure or _manifest(output) != files
            or binding(output / "request.json") != internal_request
            or binding(output / "intent.json") != proof["intent"]
            or binding(output / "runtime/runtime.json") != proof["runtime"]
            or binding(output / "before/image.json") != before
            or git(backend, "rev-parse", FAILED_HEAD + "^{tree}").decode().strip() != FAILED_TREE):
        raise ValueError("C68 失败、源码、目录或完整前像在核验期间变化")
    return {"proof": proof, "failed": failed, "request": request,
            "historical_request": historical_request, "request_descriptor": request_descriptor,
            "source": source, "runtime": runtime, "contract": contract, "selected": runtime.selected,
            "before": before, "image": image["image"], "records": tail}


def authorize(backend: Path, directory: Path, request_path: Path, number: int, prefix: list,
              *, run=subprocess.run) -> dict:
    """C69 只观察当前完整像；不创建启动意图，不启动或停止产品进程。"""
    from devex_clone_seed_generation_control import _active
    from devex_clone_seed_generation_images import capture_image, verify_image
    from devex_clone_seed_generation_runtime import GenerationRuntime
    from full_stack_runtime import register_runtime

    if number != RECOVERY_ATTEMPT or not pending(prefix):
        raise ValueError("谱系恢复只接受 C68 后唯一相邻 C69")
    facts = lineage_failure(backend, directory, prefix, request_path=request_path, run=run)
    coordinator = require_current_execution_source(backend, load_state(directory)["attempts"][-1]["sources"])
    if coordinator["fingerprints"]["product"] != facts["failed"]["sources"]["fingerprints"]["product"]:
        raise ValueError("C69 恢复改变了 seed 产品来源")
    output = local_path(backend, str(directory / "seed-runtime/attempt-0069"), new=True)
    runtime = GenerationRuntime(backend, directory, output, facts["request"], facts["source"], run)
    runtime.preflight()

    def checkpoint():
        if (_active(directory, number, "source-generation-recover") != prefix
                or binding(request_path) != facts["request_descriptor"]
                or require_current_execution_source(backend, coordinator) != coordinator):
            raise ValueError("C69 恢复期间账本、请求或协调器来源变化")
        runtime.checkpoint()

    output.mkdir()
    runtime.runtime.mkdir()
    maintenance = read_json(bound_file(runtime.execution, facts["request"]["maintenance_build"]))
    binaries = {"ryframe": runtime.build["artifacts"]["api"]["executable"],
                "ryframe-worker": runtime.build["artifacts"]["worker"]["executable"],
                **{"ryframe-" + role: maintenance["artifacts"][role]["executable"] for role in ("reset", "migrate")}}
    historical_binaries = binaries
    if facts["historical_request"] != facts["request"]:
        historical_request = facts["historical_request"]
        historical_execution = local_path(backend, historical_request["execution_backend"])
        historical_build = read_json(bound_file(backend, historical_request["backend_build"]))
        historical_maintenance = read_json(
            bound_file(historical_execution, historical_request["maintenance_build"])
        )
        historical_binaries = {
            "ryframe": historical_build["artifacts"]["api"]["executable"],
            "ryframe-worker": historical_build["artifacts"]["worker"]["executable"],
            **{
                "ryframe-" + role: historical_maintenance["artifacts"][role]["executable"]
                for role in ("reset", "migrate")
            },
        }
    if historical_binaries != read_json(Path(facts["proof"]["runtime"]["path"]).parent / "binaries.json"):
        raise ValueError("C69 观察运行使用的产品或维护二进制与 C68 不同")
    write_json(runtime.runtime / "binaries.json", binaries)
    with runtime.environment() as environment:
        runtime.contract = register_runtime(runtime.execution, runtime.runtime)
        if runtime.contract != facts["contract"]:
            raise ValueError("C69 观察运行配置与 C68 不同")
        runtime.urls = {"api": runtime.selected["api_url"].rstrip("/") + "/readyz",
                        "worker": runtime.contract["worker_ready_url"]}
        for url in runtime.urls.values():
            require_closed_port(url)
        checkpoint()
        after = capture_image(backend, runtime.execution, runtime.selected, facts["request"], facts["source"],
                              environment, output / "after", run,
                              control_environment=runtime.control_environment)
        verified = verify_image(backend, after, runtime.selected, facts["source"]["request"],
                                source_registration=facts["request"]["source_registration"])
        if verified["image"] != facts["image"]:
            raise ValueError("C69 当前完整像与 C68 启动前像不同，禁止重试")
        checkpoint()
        repeated = lineage_failure(backend, directory, prefix, request_path=request_path, run=run)
        if repeated["proof"] != facts["proof"] or repeated["image"] != facts["image"]:
            raise ValueError("C69 观察期间 C68 失败或完整前像变化")
    if ({item.name for item in output.iterdir()} != {"runtime", "after"}
            or tuple(sorted(item.name for item in runtime.runtime.iterdir())) != ("binaries.json", "runtime.json")):
        raise ValueError("C69 恢复目录包含启动意图、进程证据或未知文件")
    manifest = _manifest_summary(output)
    checkpoint()
    return {"status": STATUS, "request": facts["request_descriptor"], "failed_start": FAILED_ATTEMPT,
            "failure_proof": facts["proof"], "history_length": len(prefix), "history_sha256": plan_hash(prefix),
            "coordinator_source": coordinator,
            **{key: facts["request"][key] for key in ("source_registration", "source_rebind", "review_successor", "current_storage")},
            "runtime": binding(runtime.runtime / "runtime.json"), "before": facts["before"], "after": after,
            "evidence_manifest": manifest,
            "source_generation_published": False, "replay_allowed": True, "remote_writes": 0,
            "restore_qualified": False}


def replay_authority(backend: Path, directory: Path, attempts: list, *, run=subprocess.run) -> dict | None:
    """验证唯一 C69 授权；其后只能出现 C70 实际 START。"""
    from devex_clone_seed_generation_runtime import GenerationRuntime

    matches = [(index, row) for index, row in enumerate(attempts)
               if row.get("number") == RECOVERY_ATTEMPT
               and (row.get("stage"), row.get("mode")) == ("seed-runtime", "source-generation-recover")]
    if not matches:
        return None
    if len(matches) != 1:
        raise ValueError("C69 谱系恢复不能重复")
    position, record = matches[0]
    if position + 1 < len(attempts):
        successor = attempts[position + 1]
        if (type(successor.get("number")) is not int
                or successor["number"] != RETRY_ATTEMPT
                or (successor.get("stage"), successor.get("mode"))
                != ("seed-runtime", "source-generation-start")):
            raise ValueError("C69 后首个阶段必须是唯一 C70 seed-runtime/source-generation-start")
    if record.get("status") == "running":
        return None
    if record.get("status") != "passed" or record.get("error_type") is not None or record.get("result") is None:
        raise ValueError("C69 谱系恢复失败或未发布，不能授权重试")
    path = bound_file(backend, record["result"])
    if path != directory / "results/0069.json":
        raise ValueError("C69 谱系恢复结果不属于固定相邻阶段")
    receipt = read_json(path)
    exact(receipt, FIELDS)
    if (type(receipt["failed_start"]) is not int
            or type(receipt["history_length"]) is not int
            or type(receipt["source_generation_published"]) is not bool
            or type(receipt["replay_allowed"]) is not bool
            or type(receipt["remote_writes"]) is not int
            or type(receipt["restore_qualified"]) is not bool):
        raise ValueError("C69 谱系恢复结果的计数或布尔字段类型无效")
    manifest = receipt["evidence_manifest"]
    if (not isinstance(manifest, dict) or set(manifest) != {"files", "bytes", "sha256"}
            or type(manifest["files"]) is not int or manifest["files"] < 1
            or type(manifest["bytes"]) is not int or manifest["bytes"] < 1
            or not isinstance(manifest["sha256"], str)
            or re.fullmatch(r"[a-f0-9]{64}", manifest["sha256"]) is None):
        raise ValueError("C69 谱系恢复完整目录摘要无效")
    prefix = [row for row in attempts if row["number"] < RECOVERY_ATTEMPT]
    facts = lineage_failure(
        backend, directory, attempts, request_path=Path(receipt["request"]["path"]),
        run=run, require_idle=False)
    verify_execution_source(record["sources"], "C69 谱系恢复控制器")
    request = facts["request"]
    expected = {"status": STATUS, "request": facts["request_descriptor"], "failed_start": FAILED_ATTEMPT,
                "failure_proof": facts["proof"], "history_length": len(prefix), "history_sha256": plan_hash(prefix),
                "coordinator_source": record["sources"],
                **{key: request[key] for key in ("source_registration", "source_rebind", "review_successor", "current_storage")},
                "runtime": receipt["runtime"], "before": facts["before"], "after": receipt["after"],
                "evidence_manifest": receipt["evidence_manifest"],
                "source_generation_published": False, "replay_allowed": True, "remote_writes": 0,
                "restore_qualified": False}
    if receipt != expected or record["sources"]["fingerprints"]["product"] != facts["failed"]["sources"]["fingerprints"]["product"]:
        raise ValueError("C69 谱系恢复没有绑定 C68、完整历史或同一产品来源")
    output = local_path(backend, str(directory / "seed-runtime/attempt-0069"))
    if ({item.name for item in output.iterdir()} != {"runtime", "after"}
            or any(linked(item) for item in output.iterdir())
            or bound_file(backend, receipt["runtime"]) != output / "runtime/runtime.json"
            or bound_file(backend, receipt["after"]) != output / "after/image.json"
            or _manifest_summary(output) != receipt["evidence_manifest"]):
        raise ValueError("C69 谱系恢复目录或证据绑定不同")
    runtime = GenerationRuntime(backend, directory, output, request, facts["source"], run)
    runtime.contract = read_json(bound_file(backend, receipt["runtime"]))
    runtime.operations = {"api": "0" * 32, "worker": "1" * 32}
    if tuple(sorted(item.name for item in runtime.runtime.iterdir())) != ("binaries.json", "runtime.json"):
        raise ValueError("C69 runtime 包含产品进程、树、日志或未知文件")
    with runtime.environment():
        from full_stack_runtime import verify_runtime

        if verify_runtime(runtime.execution, runtime.runtime) != facts["contract"]:
            raise ValueError("C69 runtime 与 C68 运行配置不同")
    from devex_clone_seed_generation_images import verify_image

    after = verify_image(backend, receipt["after"], runtime.selected, facts["source"]["request"],
                         source_registration=request["source_registration"])
    if after["image"] != facts["image"] or binding(path) != record["result"]:
        raise ValueError("C69 完整后像与 C68 启动前像或外层收据不同")
    return {"record": record, "failed": facts["failed"], "records": (facts["failed"], record),
            "receipt": receipt, "image": after["image"], "failure": facts}
