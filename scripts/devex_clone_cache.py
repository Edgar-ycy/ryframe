"""固定 cache-target 阶段恢复原 Redis 及两项验收标记，未知写入不重放。"""
from __future__ import annotations

import os
from pathlib import Path

from devex_clone_cache_owner import (apply, capture, classify, confirmation, reconcile, require_resumable, transport)
from devex_clone_cache_request import environment, successor_process, validate_request
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


def successor_plan(backend, directory, value, successor, *, owned_lock_identity=None,
                   observe_current_tools=True):
    request, descriptor = registration(backend, directory)
    effective, private, source = successor_process(backend, directory, value, request, successor,
                                                  owned_lock_identity=owned_lock_identity,
                                                  observe_current_tools=observe_current_tools)
    published = [item for item in load_state(directory)["attempts"]
                 if item["result"] == source["review_successor"]["source_result"] and item["status"] == "passed"]
    if len(published) != 1:
        raise ValueError("缓存后继缺少唯一已发布 seed 来源")
    previous = [item for item in records(directory) if item["number"] < published[0]["number"]]
    if (not previous or previous[-1]["status"] != "passed"
            or previous[-1]["mode"] not in {"restart", "resume", "reconcile"}
            or read_json(bound_file(backend, previous[-1]["result"])).get("status") != "cache_ready"):
        raise ValueError("缓存工具后继必须继承 seed 发布前已确认的缓存代次")
    prior = previous[-1]
    created = starts(backend, directory, request, descriptor, before=published[0]["number"])
    if not created:
        raise ValueError("seed 原缓存缺少已登记的创建收据")
    _, process_output, base = created[-1]
    runtime, runtime_binding = owned_runtime(backend, process_output, base)
    prior_result = read_json(bound_file(backend, prior["result"]))
    ready_path = directory / STAGE / f"a{prior['number']:04d}" / "ready.json"
    ready = read_json(ready_path)
    expected = {"request": descriptor, "runtime": runtime_binding, "redis": runtime["redis"],
                "owner": ready["owner"], "controller": controller_binding(backend, directory, prior), "attempt": prior["number"]}
    if (ready != expected or prior_result.get("ready") != binding(ready_path)
            or any(prior_result.get(key) != item for key, item in expected.items())):
        raise ValueError("seed 原缓存发布结果与实际创建代次不符")
    confirmation(backend, bound_file(backend, ready["owner"]).parent, {**request, "binding": descriptor}, runtime_binding)
    proof = {"request": descriptor, "successor": successor, "source_result": source["review_successor"]["source_result"],
             "previous_result": previous[-1]["result"], "process": effective}
    return proof, private, source


def generation_request(backend, directory, request, output, *, owned_lock_identity=None,
                       observe_current_tools=True):
    path = output / "tool-successor.json"
    if not path.exists():
        return request["process"], None
    saved = read_json(path)
    expected, _, _ = successor_plan(backend, directory, read_json(directory / "manifest.json"), saved["successor"],
                                    owned_lock_identity=owned_lock_identity,
                                    observe_current_tools=observe_current_tools)
    if saved != expected:
        raise ValueError("缓存工具后继收据与原请求、来源或当前工具不符")
    return expected["process"], binding(path)


def frozen_generation_request(backend, directory, request, descriptor, output, source_result, successor):
    """核验历史 tool-successor 及完整缓存代次，不观察当前 WSL/Python。"""
    process, tools = generation_request(backend, directory, request, output, observe_current_tools=False)
    if tools is not None:
        saved = read_json(bound_file(backend, tools))
        if (saved["request"] != descriptor or saved["source_result"] != source_result
                or saved["successor"] != successor):
            raise ValueError("冻结缓存工具后继不属于同一发布来源和请求")
    return process, tools


def preflight_successor(backend, directory, value, filename=None, *, mode="restart"):
    """在创建统一阶段前验证明确后继；普通缓存调用不获得来源交接例外。"""
    if not (directory / STAGE / "registration.json").exists():
        return None
    request, _ = registration(backend, directory)
    if filename is not None:
        supplied = read_json(local_path(backend, str(filename)))
        if supplied == request:
            if any((directory / STAGE).glob("a*/tool-successor.json")):
                raise ValueError("缓存工具已换代，不能重新使用原工具请求")
            return None
        if supplied.get("kind") != "devex-clone-seed-review-successor":
            raise ValueError("缓存请求只接受原登记或正式 seed successor")
        selected = binding(local_path(backend, str(filename)))
    else:
        paths = sorted((directory / STAGE).glob("a*/tool-successor.json"))
        if not paths:
            return None
        selected = read_json(paths[-1])["successor"]
    proof, _, source = successor_plan(backend, directory, value, selected)
    from devex_clone_seed_generation_prelaunch import closed
    from devex_clone_seed_rebind import history, quiet_producers
    from devex_clone_seed_segment import segmented_resume

    state = load_state(directory)
    later = history(directory, state, proof["source_result"], backend=backend,
                    cache_recovery=mode in {"resume", "reconcile", "stop", "recover"})
    for item in state["attempts"]:
        if item["stage"] == STAGE and (directory / STAGE / f"a{item['number']:04d}" / "tool-successor.json").exists():
            if seed_history_proof(backend, directory, item, proof["source_result"]) != selected:
                raise ValueError("缓存恢复必须继续已经登记的同一 successor")
    published = next(row["number"] for row in state["attempts"] if row["result"] == proof["source_result"])
    handed_off = any(item["stage"] == "seed-runtime" and item["mode"] in {"source-rebind", "arm-input"}
                     and item["number"] > published for item in state["attempts"])
    archive = closed(backend, directory, state["attempts"])
    segment = (segmented_resume(backend, directory, state["attempts"], archive,
                                proof["source_result"]) if archive is not None else None)
    if (handed_off and (mode != "restart" or segment is None or segment["phase"] != "storage-ready")
            or any(item["mode"] == "arm-input" for item in later)):
        raise ValueError("已交接 seed 来源只能从封存存储后执行唯一缓存重启")
    quiet_producers(backend, source)
    return proof


def seed_history_proof(backend, directory, attempt, source_result, *, frozen_successor=None):
    """只识别本次 successor 的缓存阶段，不放行其他发布后缓存操作。"""
    output = directory / STAGE / f"a{attempt['number']:04d}"
    request, descriptor = registration(backend, directory)
    _, tools = (generation_request(backend, directory, request, output)
                if frozen_successor is None else frozen_generation_request(
                    backend, directory, request, descriptor, output,
                    source_result, frozen_successor))
    if tools is None or read_json(bound_file(backend, tools))["source_result"] != source_result:
        raise ValueError("seed 发布后缓存操作缺少同一来源的工具后继证明")
    return read_json(bound_file(backend, tools))["successor"]


def published_restart_guard(backend, directory, number):
    from devex_clone_seed_generation_prelaunch import closed
    from devex_clone_seed_rebind import history, quiet_producers
    from devex_clone_seed_segment import segmented_resume

    attempt = load_state(directory)["attempts"][-1]
    if attempt["number"] != number or attempt["stage"] != STAGE or attempt["status"] != "running":
        raise ValueError("seed 缓存恢复必须处于当前唯一持锁阶段")
    output = directory / STAGE / f"a{number:04d}"
    saved = read_json(output / "tool-successor.json")
    proof, _, source = successor_plan(backend, directory, read_json(directory / "manifest.json"), saved["successor"])
    if proof != saved:
        raise ValueError("当前缓存工具后继登记变化")
    state = load_state(directory)
    history(directory, state, proof["source_result"], current=number, backend=backend)
    archive = closed(backend, directory, state["attempts"])
    segment = (segmented_resume(backend, directory, state["attempts"], archive,
                                proof["source_result"], current=number) if archive is not None else None)
    handed_off = any(item["stage"] == "seed-runtime" and item["mode"] == "source-rebind"
                     for item in state["attempts"])
    if handed_off and (segment is None or segment["phase"] != "cache-running"):
        raise ValueError("缓存重启不属于封存来源的唯一分段续作")
    quiet_producers(backend, source)


def starts(backend, directory, request, descriptor, *, before=None, frozen=None):
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
        process, tools = (generation_request(backend, directory, request, output) if frozen is None else
                          frozen_generation_request(backend, directory, request, descriptor, output, *frozen))
        expected = {"request": descriptor, "controller": controller_binding(backend, directory, attempt),
                    "attempt": attempt["number"], "process_request_sha256": plan_hash(process),
                    "output": str(output / "process")}
        if tools is not None:
            expected["tools"] = tools
            expected["predecessors"] = []
            for earlier, earlier_output, earlier_runtime in result:
                proof_path = output / f"predecessor-{earlier['number']:04d}.json"
                proof = read_json(proof_path)
                old_process = (generation_request(backend, directory, request, earlier_output)[0]
                               if frozen is None else frozen_generation_request(
                                   backend, directory, request, descriptor, earlier_output, *frozen)[0])
                if (set(proof) != {"process_sha256", "runtime_sha256", "observation"}
                        or proof["process_sha256"] != plan_hash(old_process) or proof["runtime_sha256"] != plan_hash(earlier_runtime)
                        or proof["observation"].get("state") != "stopped"
                        or old_process != process and proof["observation"].get("tools_sha256") != plan_hash(
                            {key: process[key] for key in ("wsl", "python")})):
                    raise ValueError("缓存工具后继缺少每个旧代次的死亡证明")
                expected["predecessors"].append(binding(proof_path))
        if read_json(path) != expected:
            raise ValueError("缓存启动记录与原控制器、请求或进程目录不符")
        runtime = inspect_start(process, output / "process")
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


def publish(backend, output, request, descriptor, runtime, runtime_binding, owner, controller, number, process, tools):
    proof = confirmation(backend, owner, {**request, "binding": descriptor}, runtime_binding)
    redis = runtime["redis"]
    if redis != {**request["previous"], "wsl": process["wsl"], **{key: redis[key] for key in ("pid", "started", "run_id")}}:
        raise ValueError("缓存重启改变原配置、工具或监听端点")
    ready = {"request": descriptor, "runtime": runtime_binding, "redis": redis,
             "owner": proof, "controller": controller, "attempt": number}
    if tools is not None:
        ready["tools"] = tools
        ready["start"] = binding(bound_file(backend, runtime_binding).parent / "start.json")
    write_json(output / "ready.json", ready)
    return {"status": "cache_ready", **ready, "ready": binding(output / "ready.json"), "side": "target",
            "restored_markers": 2, "sessions_restored": False, "locks_restored": False,
            "resources_deleted": False, "restore_qualified": False}


def registered_cache_binding(backend: Path, directory: Path, *, owned_lock_identity=None,
                             before=None, frozen=None):
    directory = local_path(backend, str(directory))
    selected = [item for item in records(directory) if before is None or item["number"] < before]
    if not (directory / STAGE).exists() and not selected:
        return None
    request, descriptor = registration(backend, directory)
    if not selected or selected[-1]["status"] != "passed" or selected[-1]["mode"] not in {"restart", "resume", "reconcile"}:
        raise ValueError("缓存最新阶段不是成功发布的可用代次，不能回退旧成功")
    attempt = selected[-1]
    result = read_json(bound_file(backend, attempt["result"]))
    if result.get("status") != "cache_ready":
        raise ValueError("缓存最新成功阶段尚未确认可用后像，必须显式继续")
    output = directory / STAGE / f"a{attempt['number']:04d}"
    process, tools = (generation_request(backend, directory, request, output,
                                         owned_lock_identity=owned_lock_identity) if frozen is None else
                      frozen_generation_request(backend, directory, request, descriptor, output, *frozen))
    if tools is None:
        validate_request(backend, directory, read_json(directory / "manifest.json"), request,
                         owned_lock_identity=owned_lock_identity)
    ready = read_json(output / "ready.json")
    exact(ready, {"request", "runtime", "redis", "owner", "controller", "attempt"} | ({"tools", "start"} if tools else set()))
    generation = starts(backend, directory, request, descriptor, before=attempt["number"] + 1,
                        frozen=frozen)
    if not generation:
        raise ValueError("可用缓存缺少实际创建记录")
    _, process_output, base = generation[-1]
    actual_process = (generation_request(backend, directory, request, process_output)[0]
                      if frozen is None else frozen_generation_request(
                          backend, directory, request, descriptor, process_output, *frozen)[0])
    if actual_process != process:
        raise ValueError("缓存发布的工具与实际启动代次不同")
    if "state" in base:
        raise ValueError("未完成启动或已清理的 Redis 不能作为可用缓存")
    runtime, runtime_binding = owned_runtime(backend, process_output, base)
    owner = bound_file(backend, ready["owner"])
    if owner.name != "confirmed.json" or owner.parent.parent.parent != directory / STAGE:
        raise ValueError("缓存标记确认不属于本运行")
    confirmation(backend, owner.parent, {**request, "binding": descriptor}, runtime_binding)
    expected = {"request": descriptor, "runtime": runtime_binding, "redis": runtime["redis"],
                "owner": ready["owner"], "controller": controller_binding(backend, directory, attempt), "attempt": attempt["number"]}
    if tools is not None:
        expected["tools"] = tools
        expected["start"] = binding(process_output / "start.json")
    if (ready != expected or result.get("status") != "cache_ready" or result.get("ready") != binding(output / "ready.json")
            or any(result.get(key) != item for key, item in expected.items())):
        raise ValueError("缓存代次尚未取得同一外层阶段的成功发布")
    redis = runtime["redis"]
    if redis != {**request["previous"], "wsl": process["wsl"], **{key: redis[key] for key in ("pid", "started", "run_id")}}:
        raise ValueError("已发布 Redis 改变原配置或物理目标")
    return {**expected, "ready": binding(output / "ready.json"), "restart_result": attempt["result"]}


def published_start(backend, directory, output):
    """成功阶段锚定历史死亡证明；失败现场只能重新观察，不能借未发布证明跳过。"""
    expected_runtime = str(output / "runtime.json")
    found = False
    for attempt in records(directory):
        if attempt["status"] != "passed" or attempt["result"] is None:
            continue
        result = read_json(bound_file(backend, attempt["result"]))
        if result.get("status") != "cache_ready" or result.get("runtime", {}).get("path") != expected_runtime:
            continue
        ready = read_json(bound_file(backend, result["ready"]))
        if result.get("start") != binding(output / "start.json") or ready.get("start") != result["start"]:
            raise ValueError("缓存历史死亡证明未被已发布 start 摘要锚定")
        found = True
    return found


def observe_or_stop(backend, directory, request, descriptor, output, mode, number):
    from devex_clone_cache_process import predecessor_status, status, stop

    previous = starts(backend, directory, request, descriptor, before=number)
    candidates = sorted((directory / STAGE).glob("a*/tool-successor.json"))
    selected = candidates[-1].parent if candidates else (previous[-1][1] if previous else output)
    effective = generation_request(backend, directory, request, selected)[0]
    result = []
    ordered = previous if mode == "status" else list(reversed(previous))
    for attempt, process_output, runtime in ordered:
        original, _ = generation_request(backend, directory, request, process_output)
        proof_path = (previous[-1][1] / f"predecessor-{attempt['number']:04d}.json") if previous else None
        if proof_path is not None and proof_path.exists() and published_start(backend, directory, previous[-1][1]):
            observed = {**read_json(proof_path)["observation"], "evidence": binding(proof_path)}
        elif original != effective:
            observed = predecessor_status(original, runtime, effective)
        elif mode == "status":
            observed = status(effective, runtime)
        else:
            item_output = output / f"p{attempt['number']:04d}"
            item_output.mkdir()
            observed = stop(effective, {}, runtime, item_output)
        result.append({"attempt": attempt["number"], "process": observed})
    return {"status": "cache_status" if mode == "status" else "cache_stopped", "side": "target",
            "request": descriptor, "processes": sorted(result, key=lambda item: item["attempt"]), "copy_usable": False,
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
    call = transport(request, environment(backend, request), guard)
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
    from devex_clone_cache_process import observe, predecessor_status, start, status

    if mode not in {"restart", "stop", "recover", "reconcile", "resume"}:
        raise ValueError("缓存动作仅支持 restart/stop/recover/reconcile/resume")
    directory = local_path(backend, str(directory))
    controller = active(backend, directory, mode, number)
    with process_guard(directory, STAGE + ".guard"):
        successor = None
        if request_file is not None:
            supplied = read_json(local_path(backend, str(request_file)))
            if supplied.get("kind") == "devex-clone-seed-review-successor":
                successor = binding(local_path(backend, str(request_file)))
        elif (directory / STAGE / "registration.json").exists():
            paths = sorted((directory / STAGE).glob("a*/tool-successor.json"))
            if paths:
                successor = read_json(paths[-1])["successor"]
        if mode == "restart":
            request, descriptor = (registration(backend, directory) if successor is not None else
                                   register(backend, directory, value, request_file))
        else:
            if request_file is not None:
                raise ValueError("继续、核对或清理不能替换固定缓存请求")
            request, descriptor = registration(backend, directory)
        output = local_path(backend, str(directory / STAGE / f"a{number:04d}"), new=True)
        output.mkdir()
        tools = None
        if successor is not None:
            proof, private, _ = successor_plan(backend, directory, value, successor)
            write_json(output / "tool-successor.json", proof)
            process, tools = proof["process"], binding(output / "tool-successor.json")
        else:
            process = request["process"]
        if mode in {"stop", "recover"}:
            return observe_or_stop(backend, directory, request, descriptor, output, mode, number)
        if tools is None:
            private = validate_request(backend, directory, value, request)
        def guard():
            checked = (validate_request(backend, directory, value, request) if tools is None else
                       successor_plan(backend, directory, value, successor)[1])
            if (active(backend, directory, mode, number) != controller or registration(backend, directory) != (request, descriptor)
                    or checked != private or generation_request(backend, directory, request, output) != (process, tools)):
                raise ValueError("缓存操作期间控制器、固定请求或环境发生变化")
            producers_stopped(backend, directory, value)
        guard()
        previous = starts(backend, directory, request, descriptor, before=number)
        if mode == "restart":
            owner_paths(backend, directory, request, descriptor, None)
            predecessors = []
            for attempt, previous_output, runtime in previous:
                original = generation_request(backend, directory, request, previous_output)[0]
                observed = (status(process, runtime) if original == process else
                            predecessor_status(original, runtime, process))
                if observed.get("state") != "stopped":
                    raise ValueError("已有 Redis 代次仍存活，须显式精确回收")
                if tools is not None:
                    path = output / f"predecessor-{attempt['number']:04d}.json"
                    write_json(path, {"process_sha256": plan_hash(original), "runtime_sha256": plan_hash(runtime),
                                      "observation": observed})
                    predecessors.append(binding(path))
            process_output = output / "process"
            started = {"request": descriptor, "controller": controller, "attempt": number,
                       "process_request_sha256": plan_hash(process), "output": str(process_output)}
            if tools is not None:
                started["tools"] = tools
                started["predecessors"] = predecessors
            write_json(output / "start.json", started)
            process_output.mkdir()
            runtime = start(process, private, process_output, guard)
            write_json(output / "runtime.json", runtime)
            runtime_binding = binding(output / "runtime.json")
        else:
            if not previous:
                raise ValueError("没有本运行已登记的 Redis 进程可供核对或继续")
            _, process_output, base = previous[-1]
            if "state" in base:
                raise ValueError("该 Redis 启动已精确清理；使用显式 restart 创建新代次")
            if generation_request(backend, directory, request, process_output)[0] != process:
                raise ValueError("继续缓存必须使用同一已登记工具代次")
            runtime = observe(process, private, base)
            if (process_output / "runtime.json").exists():
                runtime, runtime_binding = owned_runtime(backend, process_output, base)
            else:
                write_json(process_output / "runtime.json", runtime)
                runtime_binding = binding(process_output / "runtime.json")
        def marker_guard():
            guard()
            observe(process, private, runtime)
        marker_guard()
        owner = restore_markers(backend, directory, request, descriptor, output, runtime, runtime_binding, mode, number, marker_guard)
        marker_guard()
        if owner is None:
            return {"status": "cache_reconciled_before", "request": descriptor, "runtime": runtime_binding,
                    "copy_usable": False, "remote_writes": 0, "resume_required": True}
        return publish(backend, output, request, descriptor, runtime, runtime_binding, owner, controller, number, process, tools)
