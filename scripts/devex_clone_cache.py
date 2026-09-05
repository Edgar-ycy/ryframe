"""固定 cache-target 阶段恢复原 Redis 及两项验收标记，未知写入不重放。"""
from __future__ import annotations

import os
from pathlib import Path

from devex_clone_cache_owner import (apply, capture, classify, confirmation, reconcile, require_resumable, transport)
from devex_clone_cache_request import environment, validate_request
from devex_clone_capture import read_json, write_json
from devex_clone_model import exact, local_path
from devex_clone_run_state import binding, load_state
from devex_clone_source_proof import bound_file
from devex_clone_storage import controller_binding
from devex_clone_storage_request import producers_stopped
from full_stack_process import process_identity
from process_guard import process_guard
from restore_reference_plan import plan_hash

STAGE = "cache-target"


def records(directory, *, cleanup=False):
    return [item for item in load_state(directory, verify_results=not cleanup)["attempts"] if item["stage"] == STAGE]


def active(backend, directory, mode, number):
    state = load_state(directory, verify_results=mode not in {"stop", "recover"})
    attempt = state["attempts"][-1] if state["attempts"] else {}
    if any(attempt.get(key) != value for key, value in {"number": number, "stage": STAGE, "mode": mode, "status": "running"}.items()):
        raise ValueError("缓存操作不是统一运行中当前唯一已登记阶段")
    descriptor = controller_binding(backend, directory, attempt)
    controller = read_json(bound_file(backend, descriptor))
    if (controller["owner"]["identity"] != process_identity(os.getpid())
            or controller["owner"] != read_json(directory / "run.lock/owner.json")):
        raise ValueError("缓存操作必须由原控制器持有统一运行锁")
    return descriptor


def registration(backend, directory):
    root = local_path(backend, str(directory / STAGE))
    value = read_json(root / "registration.json")
    exact(value, {"request", "manifest"})
    if value["manifest"] != binding(directory / "manifest.json") or Path(value["request"]["path"]) != root / "request.json":
        raise ValueError("缓存登记不属于固定运行")
    request = read_json(bound_file(backend, value["request"]))
    if request.get("manifest") != value["manifest"] or request.get("side") != "target":
        raise ValueError("缓存请求不属于原目标")
    return request, value["request"]


def register(backend, directory, value, filename):
    root = local_path(backend, str(directory / STAGE))
    root.mkdir(exist_ok=True)
    if (root / "registration.json").exists():
        request, descriptor = registration(backend, directory)
        if filename is not None and read_json(local_path(backend, str(filename))) != request:
            raise ValueError("不能替换同一缓存运行的固定请求")
    else:
        if filename is None:
            raise ValueError("首次缓存恢复必须显式提供固定请求")
        descriptor = binding(local_path(backend, str(filename)))
        request = read_json(bound_file(backend, descriptor))
        validate_request(backend, directory, value, request)
        bound_file(backend, descriptor)
        write_json(root / "request.json", request)
        descriptor = binding(root / "request.json")
        write_json(root / "registration.json", {"request": descriptor, "manifest": binding(directory / "manifest.json")})
    return request, descriptor


def starts(backend, directory, request, descriptor, *, before=None):
    from devex_clone_cache_process import inspect_start

    result = []
    for attempt in records(directory, cleanup=True):
        if attempt["mode"] != "restart" or before is not None and attempt["number"] >= before:
            continue
        output = local_path(backend, str(directory / STAGE / f"a{attempt['number']:04d}"))
        path = output / "start.json"
        if not path.exists():
            if (output / "process").exists():
                raise ValueError("缓存进程目录缺少持久控制阶段绑定")
            continue
        expected = {"request": descriptor, "controller": controller_binding(backend, directory, attempt),
                    "attempt": attempt["number"], "process_request_sha256": plan_hash(request["process"]),
                    "output": str(output / "process")}
        if read_json(path) != expected:
            raise ValueError("缓存启动记录与原控制器、请求或进程目录不符")
        runtime = inspect_start(request["process"], output / "process")
        if runtime is not None:
            result.append((attempt, output, runtime))
    return result


def owner_paths(backend, directory, request, descriptor, runtime):
    groups = {}
    for attempt in records(directory, cleanup=True):
        path = local_path(backend, str(directory / STAGE / f"a{attempt['number']:04d}" / "owner"))
        if (path / "intent.json").exists():
            intent = read_json(path / "intent.json")
            if intent.get("request") != descriptor:
                raise ValueError("历史缓存标记意图不属于固定请求")
            reference = intent["runtime"]
            generation = bound_file(backend, reference)
            if generation.name != "runtime.json" or generation.parent.parent != directory / STAGE:
                raise ValueError("标记意图引用其他运行的缓存代次")
            groups.setdefault(plan_hash(reference), (reference, []))[1].append(path)
    selected = []
    for reference, paths in groups.values():
        owner_request = {**request, "binding": descriptor}
        for earlier in paths[:-1]:
            require_resumable(backend, earlier, owner_request, reference)
        if reference == runtime:
            selected = paths
        else:
            confirmation(backend, paths[-1], owner_request, reference)
    return selected


def owned_runtime(backend, output, base):
    path = output / "runtime.json"
    runtime = read_json(path)
    if any(runtime.get(key) != item for key, item in base.items()):
        raise ValueError("缓存完整运行记录与实际创建收据不符")
    bound_file(backend, runtime["process_receipt"])
    return runtime, binding(path)


def publish(backend, output, request, descriptor, runtime, runtime_binding, owner, controller, number):
    proof = confirmation(backend, owner, {**request, "binding": descriptor}, runtime_binding)
    redis = runtime["redis"]
    if redis != {**request["previous"], **{key: redis[key] for key in ("pid", "started", "run_id")}}:
        raise ValueError("缓存重启改变原配置、工具或监听端点")
    ready = {"request": descriptor, "runtime": runtime_binding, "redis": redis,
             "owner": proof, "controller": controller, "attempt": number}
    write_json(output / "ready.json", ready)
    return {"status": "cache_ready", **ready, "ready": binding(output / "ready.json"), "side": "target",
            "restored_markers": 2, "sessions_restored": False, "locks_restored": False,
            "resources_deleted": False, "restore_qualified": False}


def registered_cache_binding(backend: Path, directory: Path, *, owned_lock_identity=None):
    directory = local_path(backend, str(directory))
    selected = records(directory)
    if not (directory / STAGE).exists() and not selected:
        return None
    request, descriptor = registration(backend, directory)
    if not selected or selected[-1]["status"] != "passed" or selected[-1]["mode"] not in {"restart", "resume", "reconcile"}:
        raise ValueError("缓存最新阶段不是成功发布的可用代次，不能回退旧成功")
    attempt = selected[-1]
    validate_request(backend, directory, read_json(directory / "manifest.json"), request,
                     owned_lock_identity=owned_lock_identity)
    result = read_json(bound_file(backend, attempt["result"]))
    if result.get("status") != "cache_ready":
        raise ValueError("缓存最新成功阶段尚未确认可用后像，必须显式继续")
    output = directory / STAGE / f"a{attempt['number']:04d}"
    ready = read_json(output / "ready.json")
    exact(ready, {"request", "runtime", "redis", "owner", "controller", "attempt"})
    generation = starts(backend, directory, request, descriptor, before=attempt["number"] + 1)
    if not generation:
        raise ValueError("可用缓存缺少实际创建记录")
    _, process_output, base = generation[-1]
    if "state" in base:
        raise ValueError("未完成启动或已清理的 Redis 不能作为可用缓存")
    runtime, runtime_binding = owned_runtime(backend, process_output, base)
    owner = bound_file(backend, ready["owner"])
    if owner.name != "confirmed.json" or owner.parent.parent.parent != directory / STAGE:
        raise ValueError("缓存标记确认不属于本运行")
    confirmation(backend, owner.parent, {**request, "binding": descriptor}, runtime_binding)
    expected = {"request": descriptor, "runtime": runtime_binding, "redis": runtime["redis"],
                "owner": ready["owner"], "controller": controller_binding(backend, directory, attempt), "attempt": attempt["number"]}
    if (ready != expected or result.get("status") != "cache_ready" or result.get("ready") != binding(output / "ready.json")
            or any(result.get(key) != item for key, item in expected.items())):
        raise ValueError("缓存代次尚未取得同一外层阶段的成功发布")
    redis = runtime["redis"]
    if redis != {**request["previous"], **{key: redis[key] for key in ("pid", "started", "run_id")}}:
        raise ValueError("已发布 Redis 改变原配置或物理目标")
    return {**expected, "ready": binding(output / "ready.json"), "restart_result": attempt["result"]}


def observe_or_stop(backend, directory, request, descriptor, output, mode, number):
    from devex_clone_cache_process import status, stop

    previous = starts(backend, directory, request, descriptor, before=number)
    result = []
    for attempt, _, runtime in previous:
        if mode == "status":
            observed = status(request["process"], runtime)
        else:
            item_output = output / f"p{attempt['number']:04d}"
            item_output.mkdir()
            observed = stop(request["process"], {}, runtime, item_output)
        result.append({"attempt": attempt["number"], "process": observed})
    return {"status": "cache_status" if mode == "status" else "cache_stopped", "side": "target",
            "request": descriptor, "processes": result, "copy_usable": False,
            "remote_writes": 0, "resources_deleted": False}


def cache_status(backend: Path, directory: Path):
    directory = local_path(backend, str(directory))
    initial = binding(directory / "state.json")
    state = load_state(directory, verify_results=False)
    if any(item["status"] == "running" for item in state["attempts"]):
        raise ValueError("统一运行中还有未完成阶段，不能形成稳定观察")
    if not records(directory, cleanup=True) and not (directory / STAGE).exists():
        return {"status": "cache_unregistered", "copy_usable": False, "processes": []}
    request, descriptor = registration(backend, directory)
    result = observe_or_stop(backend, directory, request, descriptor, directory, "status", len(state["attempts"]) + 1)
    if binding(directory / "state.json") != initial or registration(backend, directory) != (request, descriptor):
        raise ValueError("缓存只读观察期间阶段或登记变化")
    return result


def restore_markers(backend, directory, request, descriptor, output, runtime, runtime_binding, mode, number, guard):
    call = transport(request, environment(backend, request))
    owner_request = {**request, "binding": descriptor}
    previous = owner_paths(backend, directory, request, descriptor, runtime_binding)
    if previous and (previous[-1] / "confirmed.json").exists():
        confirmation(backend, previous[-1], owner_request, runtime_binding)
        guard()
        if classify(request["markers"], capture(call, request["markers"])) != "after":
            raise ValueError("已确认缓存标记的完整后像发生漂移")
        return previous[-1]
    if mode == "reconcile":
        if not previous:
            raise ValueError("该缓存代次尚无标记写入意图，使用显式 resume")
        state = reconcile(backend, previous[-1], owner_request, runtime_binding, call, guard, number)
        return previous[-1] if state == "after" else None
    if previous:
        if mode != "resume":
            raise ValueError("未知缓存写入不能由 restart 自动重放")
        require_resumable(backend, previous[-1], owner_request, runtime_binding)
    apply(backend, output / "owner", owner_request, runtime_binding, call, guard)
    return output / "owner"


def execute_cache(backend: Path, directory: Path, value: dict, mode: str, number: int, request_file: Path | None = None):
    from devex_clone_cache_process import observe, start, status

    if mode not in {"restart", "stop", "recover", "reconcile", "resume"}:
        raise ValueError("缓存动作仅支持 restart/stop/recover/reconcile/resume")
    directory = local_path(backend, str(directory))
    controller = active(backend, directory, mode, number)
    with process_guard(directory, STAGE + ".guard"):
        if mode == "restart":
            request, descriptor = register(backend, directory, value, request_file)
        else:
            if request_file is not None:
                raise ValueError("继续、核对或清理不能替换固定缓存请求")
            request, descriptor = registration(backend, directory)
        output = local_path(backend, str(directory / STAGE / f"a{number:04d}"), new=True)
        output.mkdir()
        if mode in {"stop", "recover"}:
            return observe_or_stop(backend, directory, request, descriptor, output, mode, number)
        private = validate_request(backend, directory, value, request)
        def guard():
            if (active(backend, directory, mode, number) != controller or registration(backend, directory) != (request, descriptor)
                    or validate_request(backend, directory, value, request) != private):
                raise ValueError("缓存操作期间控制器、固定请求或环境发生变化")
            producers_stopped(backend, directory, value)
        guard()
        previous = starts(backend, directory, request, descriptor, before=number)
        if mode == "restart":
            owner_paths(backend, directory, request, descriptor, None)
            for _, _, runtime in previous:
                if status(request["process"], runtime).get("state") != "stopped":
                    raise ValueError("已有 Redis 代次仍存活，须显式精确回收")
            process_output = output / "process"
            write_json(output / "start.json", {"request": descriptor, "controller": controller, "attempt": number,
                       "process_request_sha256": plan_hash(request["process"]), "output": str(process_output)})
            process_output.mkdir()
            runtime = start(request["process"], private, process_output, guard)
            write_json(output / "runtime.json", runtime)
            runtime_binding = binding(output / "runtime.json")
        else:
            if not previous:
                raise ValueError("没有本运行已登记的 Redis 进程可供核对或继续")
            _, process_output, base = previous[-1]
            if "state" in base:
                raise ValueError("该 Redis 启动已精确清理；使用显式 restart 创建新代次")
            runtime = observe(request["process"], private, base)
            if (process_output / "runtime.json").exists():
                runtime, runtime_binding = owned_runtime(backend, process_output, base)
            else:
                write_json(process_output / "runtime.json", runtime)
                runtime_binding = binding(process_output / "runtime.json")
        def marker_guard():
            guard()
            observe(request["process"], private, runtime)
        marker_guard()
        owner = restore_markers(backend, directory, request, descriptor, output, runtime, runtime_binding, mode, number, marker_guard)
        marker_guard()
        if owner is None:
            return {"status": "cache_reconciled_before", "request": descriptor, "runtime": runtime_binding,
                    "copy_usable": False, "remote_writes": 0, "resume_required": True}
        return publish(backend, output, request, descriptor, runtime, runtime_binding, owner, controller, number)
