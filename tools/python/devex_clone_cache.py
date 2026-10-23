"""缓存阶段的锁、资源和进程 I/O 适配；阶段判断由 __cache-process 执行。"""
from __future__ import annotations
from pathlib import Path
from types import SimpleNamespace

from devex_clone_capture import read_json
from devex_clone_cache_native import stage_session
from devex_clone_cache_request import environment, successor_process, validate_request
from devex_clone_storage_request import producers_stopped
from process_guard import process_guard

STAGE = "cache-target"


def transport(backend, request, private, checkpoint, *, password=None):
    from devex_clone_target_resources import Resources
    password = request.get("process", {}).get("password") if password is None else password
    if not isinstance(password, str) or not password:
        raise ValueError("Redis 标记传输必须使用已由配置文件校验的密码")
    resource = SimpleNamespace(request={"storage": {"redis": request["previous"]}},
        environment=private, checkpoint=checkpoint, execution_backend=backend,
        redis_password=password)
    resource._response = Resources._response
    return lambda parts: Resources._redis(resource, parts)


def _port(backend, directory, operation, arguments):
    from devex_clone_cache_process import inspect_start, observe, predecessor_status, start, status, stop
    manifest = lambda: read_json(directory / "manifest.json")
    checkpoint = lambda spec: _invoke(backend, directory, "guard", **spec)
    if operation == "validate":
        return validate_request(backend, directory, manifest(), arguments["request"],
                                owned_lock_identity=arguments["owned_lock_identity"])
    if operation == "successor":
        return successor_process(backend, directory, manifest(), arguments["request"], arguments["successor"],
            owned_lock_identity=arguments["owned_lock_identity"],
            observe_current_tools=arguments["observe_current_tools"])
    if operation == "producers-stopped":
        producers_stopped(backend, directory, manifest())
        return None
    if operation == "inspect":
        return inspect_start(arguments["request"], Path(arguments["output"]))
    if operation == "status":
        return status(arguments["request"], arguments["runtime"])
    if operation == "predecessor":
        return predecessor_status(arguments["request"], arguments["runtime"], arguments["effective"])
    if operation == "stop":
        return stop(arguments["request"], arguments["private"], arguments["runtime"], Path(arguments["output"]))
    if operation == "observe":
        return observe(arguments["request"], arguments["private"], arguments["runtime"])
    if operation == "start":
        return start(arguments["request"], arguments["private"], Path(arguments["output"]),
                     lambda: checkpoint(arguments["guard"]))
    if operation == "redis":
        call = transport(backend, arguments["request"], environment(backend, arguments["request"]),
                         lambda: checkpoint(arguments["guard"]))
        return call(arguments["parts"])
    if operation == "quiet":
        from devex_clone_seed_rebind import quiet_producers
        source = dict(arguments)
        # 当前来源端口使用 Path；stdin 中使用显式路径字符串。
        if "directory" in source:
            source["directory"] = Path(source["directory"])
        quiet_producers(backend, source)
        return None
    if operation == "restart-authorization":
        from devex_clone_seed_generation_reboot import restart_authorization
        restart_authorization(backend, directory,
            **({"allow_attempt": arguments["current"]} if arguments["current"] is not None else {}), live_cache=False)
        return None
    if operation == "seed-history":
        from devex_clone_seed_rebind import history
        from devex_clone_run_state import load_state
        current = arguments["current"]
        return history(directory, load_state(directory), arguments["source"], backend=backend,
            **({"current": current} if current is not None else
               {"cache_recovery": arguments["mode"] in {"resume", "reconcile", "stop", "recover"}}))
    if operation == "seed-segment":
        from devex_clone_seed_generation_prelaunch import closed
        from devex_clone_seed_segment import segmented_resume
        from devex_clone_run_state import load_state
        state = load_state(directory)
        archive = closed(backend, directory, state["attempts"])
        current = arguments["current"]
        return (segmented_resume(backend, directory, state["attempts"], archive, arguments["source"],
                   **({"current": current} if current is not None else {})) if archive is not None else None)
    raise ValueError("未知缓存资源端口；禁止执行")


def _invoke(backend, directory, action, **parameters):
    backend, directory = Path(backend), Path(directory)
    return stage_session(action, directory, parameters,
                         lambda op, args: _port(backend, directory, op, args))


def execute_cache(backend, directory, value, mode, number, request_file=None):
    _invoke(backend, directory, "execution-preflight", value=value, mode=mode, number=number, request_file=request_file)
    with process_guard(directory, STAGE + ".guard"):
        return _invoke(backend, directory, "execute", value=value, mode=mode, number=number, request_file=request_file)


def registration(backend, directory):
    return tuple(_invoke(backend, directory, "registration"))


def starts(backend, directory, request, descriptor, *, before=None, frozen=None):
    return [(attempt, Path(output), runtime) for attempt, output, runtime in
            _invoke(backend, directory, "starts", request=request, descriptor=descriptor, before=before, frozen=frozen)]


def generation_request(backend, directory, request, output, *, owned_lock_identity=None, observe_current_tools=True):
    return tuple(_invoke(backend, directory, "generation", request=request, output=output,
                 owned_lock_identity=owned_lock_identity, observe_current_tools=observe_current_tools))


def registered_cache_binding(backend, directory, *, owned_lock_identity=None, before=None, frozen=None):
    return _invoke(backend, directory, "binding", owned_lock_identity=owned_lock_identity, before=before, frozen=frozen)


def cache_status(backend, directory, *, allow_close_attempt=False):
    return _invoke(backend, directory, "status", allow_close_attempt=allow_close_attempt)


def preflight_successor(backend, directory, value, filename=None, *, mode="restart"):
    return _invoke(backend, directory, "preflight", filename=filename, mode=mode)


def seed_history_proof(backend, directory, attempt, source_result, *, frozen_successor=None):
    return _invoke(backend, directory, "history-proof", attempt=attempt, source_result=source_result,
                   frozen_successor=frozen_successor)


def published_restart_guard(backend, directory, number):
    return _invoke(backend, directory, "restart-guard", number=number)
