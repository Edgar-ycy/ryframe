"""统一复制目录中的 RustFS 显式重启、观察和精确回收，不执行数据操作。"""
from __future__ import annotations

import copy
import os
from pathlib import Path
import subprocess

from artifact_digests import protect_binaries
from devex_clone_capture import read_json, write_json
from devex_clone_model import exact, local_path
from devex_clone_run_state import binding, load_state
from devex_clone_source_proof import bound_file, require_closed_port, require_recorded_producer_stopped
from devex_clone_storage_request import (arguments, directory_identity, producers_stopped, validate_request)
from devex_clone_storage_process import inspect_attempt, running, start, stop_record
from full_stack_process import process_identity
from process_guard import process_guard
from restore_reference_plan import plan_hash


def stage(side: str) -> str:
    if side not in {"source", "target"}:
        raise ValueError("必须明确选择源或目标存储")
    return "storage-" + side


def records(directory: Path, side: str, *, cleanup=False) -> list[dict]:
    return [item for item in load_state(directory, verify_results=not cleanup)["attempts"] if item["stage"] == stage(side)]


def controller_binding(backend: Path, directory: Path, attempt: dict) -> dict:
    path = local_path(backend, str(directory / f"controller-{attempt['number']:04d}.json"))
    value = read_json(path)
    exact(value, {"format_version", "kind", "owner", "attempt", "attempt_sha256"})
    initial = {**copy.deepcopy(attempt), "status": "running", "finished_at": None, "result": None, "error_type": None}
    expected = {"format_version": 1, "kind": "devex-stage-controller", "attempt": attempt["number"], "attempt_sha256": plan_hash(initial)}
    if (any(value[key] != item for key, item in expected.items())
            or value["owner"].get("directory") != str(directory)
            or value["owner"].get("manifest_sha256") != binding(directory / "manifest.json")["sha256"]):
        raise ValueError("存储控制器证据不属于该固定运行和完整阶段")
    return binding(path)


def active(backend: Path, directory: Path, mode: str, number: int, side: str) -> dict:
    state = load_state(directory, verify_results=mode not in {"status", "stop", "recover"})
    if not state["attempts"]:
        raise ValueError("存储控制必须通过统一阶段入口执行")
    attempt = state["attempts"][-1]
    if (attempt["number"] != number or attempt["stage"] != stage(side) or attempt["mode"] != mode
            or attempt["status"] != "running"):
        raise ValueError("存储控制不是当前唯一已登记阶段")
    descriptor = controller_binding(backend, directory, attempt)
    controller = read_json(bound_file(backend, descriptor))
    if (controller["owner"]["identity"] != process_identity(os.getpid())
            or read_json(directory / "run.lock/owner.json") != controller["owner"]):
        raise ValueError("存储动作必须由原登记控制器持有统一运行锁")
    return descriptor


def registration(backend: Path, directory: Path, side: str) -> tuple[dict, dict]:
    root = local_path(backend, str(directory / stage(side)))
    saved = read_json(root / "registration.json")
    exact(saved, {"request", "manifest", "side"})
    if (saved["manifest"] != binding(directory / "manifest.json") or saved["side"] != side
            or Path(saved["request"]["path"]) != root / "request.json"):
        raise ValueError("存储登记不属于固定运行和明确侧")
    request = read_json(bound_file(backend, saved["request"]))
    if request.get("manifest") != saved["manifest"] or request.get("side") != side:
        raise ValueError("存储请求与登记不符")
    return request, saved["request"]


def register(backend: Path, directory: Path, value: dict, side: str, filename: Path | None) -> tuple[dict, dict, dict]:
    root = local_path(backend, str(directory / stage(side)))
    root.mkdir(exist_ok=True)
    if (root / "registration.json").exists():
        request, descriptor = registration(backend, directory, side)
        if filename is not None and read_json(local_path(backend, str(filename))) != request:
            raise ValueError("不能替换同一存储运行的固定请求")
    else:
        if filename is None:
            raise ValueError("首次存储重启必须显式提供请求")
        filename = local_path(backend, str(filename))
        descriptor = binding(filename)
        request = read_json(bound_file(backend, descriptor))
        validate_request(backend, directory, value, request, side)
        bound_file(backend, descriptor)
        write_json(root / "request.json", request)
        descriptor = binding(root / "request.json")
        write_json(root / "registration.json", {"request": descriptor, "manifest": binding(directory / "manifest.json"), "side": side})
    private = validate_request(backend, directory, value, request, side)
    return request, descriptor, private


def previous_attempts(backend: Path, directory: Path, side: str, descriptor: dict, *, before: int | None = None) -> list[tuple]:
    result = []
    for attempt in records(directory, side, cleanup=True):
        if attempt["mode"] != "restart" or before is not None and attempt["number"] >= before:
            continue
        output = local_path(backend, str(directory / stage(side) / f"a{attempt['number']:04d}"))
        observed = inspect_attempt(backend, output, descriptor, controller_binding(backend, directory, attempt), attempt["number"])
        if observed is not None:
            result.append((attempt, observed))
    return result


def require_previous_stopped(backend: Path, directory: Path, request: dict, descriptor: dict,
                             before: int | None) -> None:
    require_recorded_producer_stopped(request["previous"]["identity"], run=subprocess.run,
                                      counter_run=subprocess.run)
    for _, observed in previous_attempts(backend, directory, request["side"], descriptor, before=before):
        if observed["state"] == "recorded":
            require_recorded_producer_stopped(observed["identity"], run=subprocess.run,
                                              counter_run=subprocess.run)
    for key in ("api_url", "console_url"):
        require_closed_port(request[key])


def preflight_restart(backend: Path, directory: Path, value: dict, side: str,
                      request_file: Path | None) -> None:
    """在创建 attempt 前只读证明固定请求和所有历史存储代次均可安全重启。"""
    root = local_path(backend, str(directory / stage(side)))
    if (root / "registration.json").exists():
        request, descriptor = registration(backend, directory, side)
        if request_file is not None and read_json(local_path(backend, str(request_file))) != request:
            raise ValueError("不能替换同一存储运行的固定请求")
    else:
        if request_file is None:
            raise ValueError("首次存储重启必须显式提供请求")
        path = local_path(backend, str(request_file))
        descriptor = binding(path)
        request = read_json(bound_file(backend, descriptor))
    validate_request(backend, directory, value, request, side)
    require_previous_stopped(backend, directory, request, descriptor, None)


def registered_storage_binding(backend: Path, directory: Path, side: str) -> dict | None:
    """仅验证已发布代次及本地输入；调用方继续独立执行原有实时进程/参数/端口核验。"""
    local_path(backend, str(directory))
    root = local_path(backend, str(directory / stage(side)))
    selected = records(directory, side)
    if not root.exists() and not selected:
        return None
    request, descriptor = registration(backend, directory, side)
    changes = [item for item in selected if item["mode"] in {"restart", "stop", "recover"}]
    if not changes or changes[-1]["mode"] != "restart" or changes[-1]["status"] != "passed":
        raise ValueError("存储最新改变状态的阶段不是已发布成功重启")
    attempt = changes[-1]
    if any(item["status"] != "passed" for item in selected if item["number"] > attempt["number"]):
        raise ValueError("存储有后续失败或未完成观察，不能回退旧成功")
    value = read_json(directory / "manifest.json")
    validate_request(backend, directory, value, request, side)
    output = root / f"a{attempt['number']:04d}"
    observed = inspect_attempt(backend, output, descriptor, controller_binding(backend, directory, attempt), attempt["number"])
    if observed is None or observed["state"] != "recorded" or observed["launch_receipt"] is None:
        raise ValueError("成功存储阶段缺少完整实际进程和启动收据")
    storage = {"identity": observed["identity"], "sha256": request["executable"]["sha256"],
               "process_receipt": observed["process_receipt"], "launch_receipt": observed["launch_receipt"]}
    ready = read_json(output / "ready.json")
    if ready != {"storage": storage, "request": descriptor, "controller": controller_binding(backend, directory, attempt),
                 "attempt": attempt["number"], "listeners": [request["api_url"], request["console_url"]]}:
        raise ValueError("存储就绪证据与实际启动收据不符")
    result = read_json(bound_file(backend, attempt["result"]))
    expected = {"status": "storage_restarted", "side": side, "request": descriptor, "storage": storage,
                "ready": binding(output / "ready.json"), "attempt": attempt["number"],
                "data_directory": request["data_directory"], "api_url": request["api_url"], "console_url": request["console_url"]}
    if any(result.get(key) != item for key, item in expected.items()):
        raise ValueError("存储启动尚未取得同一外层发布结果")
    return {key: expected[key] for key in ("request", "storage", "attempt", "data_directory", "api_url", "console_url")} | {"restart_result": attempt["result"]}


def current_storage_binding(backend: Path, directory: Path, side: str) -> dict | None:
    result = registered_storage_binding(backend, directory, side)
    if result is not None:
        request = read_json(bound_file(backend, result["request"]))
        if not running(result["storage"]["identity"], arguments(request), (result["api_url"], result["console_url"])):
            raise ValueError("已登记存储进程已退出，必须显式同目录重启")
    return result


def storage_status(backend: Path, directory: Path, side: str) -> dict:
    """只读观察，不创建阶段或文件，也不把未知发布状态包装成复制许可。"""
    directory = local_path(backend, str(directory))
    initial = binding(directory / "state.json")
    selected = records(directory, side, cleanup=True)
    if any(item["status"] == "running" for item in load_state(directory, verify_results=False)["attempts"]):
        raise ValueError("统一运行仍有控制器动作，暂不能形成稳定存储状态")
    if not (directory / stage(side)).exists() and not selected:
        return {"status": "storage_unregistered", "side": side, "copy_usable": False, "processes": []}
    request, descriptor = registration(backend, directory, side)
    result = observe_or_stop(backend, directory, request, descriptor, directory, "status", len(load_state(directory, verify_results=False)["attempts"]) + 1)
    if binding(directory / "state.json") != initial or registration(backend, directory, side) != (request, descriptor):
        raise ValueError("存储只读观察期间登记或阶段记录变化")
    return {**result, "copy_usable": False}


def observe_or_stop(backend: Path, directory: Path, request: dict, descriptor: dict, output: Path, mode: str, number: int) -> dict:
    previous = previous_attempts(backend, directory, request["side"], descriptor, before=number)
    if not previous:
        raise ValueError("不存在本统一目录实际启动的存储；不能回收原始服务")
    # 所有历史启动意图都已证明身份，不能用最近一次掩盖更早未知生产者。
    if mode in {"stop", "recover"}:
        for _, observed in previous:
            if observed["state"] == "recorded":
                running(observed["identity"], arguments(request), (request["api_url"], request["console_url"]), listeners=False)
    results = []
    for attempt, observed in previous:
        if observed["state"] == "not_started":
            results.append({"attempt": attempt["number"], "state": "stopped", "identity": None})
            continue
        if mode in {"stop", "recover"}:
            item_output = output / f"p{attempt['number']:04d}"
            item_output.mkdir()
            result = stop_record(request, observed, item_output)
        else:
            alive = running(observed["identity"], arguments(request), (request["api_url"], request["console_url"]))
            result = {"state": "running" if alive else "stopped", "identity": observed["identity"],
                      "process_receipt": observed["process_receipt"]}
        results.append({"attempt": attempt["number"], **result})
    if mode in {"stop", "recover"}:
        # 历史代次复用登记端口；先回收所有已证明归属的进程，再拒绝任何残留监听。
        for key in ("api_url", "console_url"):
            require_closed_port(request[key])
    return {"status": "storage_status" if mode == "status" else "storage_recovered" if mode == "recover" else "storage_stopped",
            "side": request["side"], "request": descriptor, "processes": results,
            "reconciliation_required": True, "object_api_calls": 0, "database_calls": 0, "resources_deleted": False}


def execute_storage(backend: Path, directory: Path, value: dict, mode: str, number: int, side: str,
                    request_file: Path | None = None) -> dict:
    if mode not in {"restart", "stop", "recover"}:
        raise ValueError("存储动作必须是 restart/stop/recover；只读状态使用 storage_status")
    directory = local_path(backend, str(directory))
    controller = active(backend, directory, mode, number, side)
    # 父入口持整个 run 锁；此文件锁同时挡住绕过入口的同侧控制重入。
    with process_guard(directory, stage(side) + ".guard"):
        if mode == "restart":
            request, descriptor, private = register(backend, directory, value, side, request_file)
        else:
            if request_file is not None:
                raise ValueError("清理和观察不能重新登记或更换存储请求")
            request, descriptor = registration(backend, directory, side)
            private = None
        output = local_path(backend, str(directory / stage(side) / f"a{number:04d}"), new=True)
        output.mkdir()
        if mode != "restart":
            return observe_or_stop(backend, directory, request, descriptor, output, mode, number)
        def guard():
            if active(backend, directory, mode, number, side) != controller or registration(backend, directory, side) != (request, descriptor):
                raise ValueError("存储控制器或固定登记变化")
            if validate_request(backend, directory, value, request, side) != private:
                raise ValueError("存储启动期间私有环境变化")
            producers_stopped(backend, directory, value)
            directory_identity(backend, request["data_directory"])
        guard()
        require_previous_stopped(backend, directory, request, descriptor, number)
        with protect_binaries([{key: request["executable"][key] for key in ("path", "sha256")}]):
            storage = start(backend, request, private, output, descriptor, controller, number, guard)
        return {"status": "storage_restarted", "side": side, "request": descriptor, "attempt": number,
                "storage": storage, "ready": binding(output / "ready.json"), "data_directory": request["data_directory"],
                "api_url": request["api_url"], "console_url": request["console_url"], "object_api_calls": 0,
                "database_calls": 0, "storage_startup_may_write_files": True, "reconciliation_required": True,
                "resources_deleted": False, "restore_qualified": False}
