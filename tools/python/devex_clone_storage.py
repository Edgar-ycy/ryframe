"""读取统一复制目录中的 RustFS 登记、历史代次与已发布绑定。"""
from __future__ import annotations

import copy
from pathlib import Path

from devex_clone_capture import read_json
from devex_clone_model import exact, local_path
from devex_clone_run_state import binding, load_state
from devex_clone_source_proof import bound_file
from devex_clone_storage_request import arguments, validate_request
from devex_clone_storage_process import inspect_attempt, running
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


def registered_storage_binding(backend: Path, directory: Path, side: str, *, before: int | None = None) -> dict | None:
    """仅验证已发布代次及本地输入；调用方继续独立执行原有实时进程/参数/端口核验。"""
    local_path(backend, str(directory))
    root = local_path(backend, str(directory / stage(side)))
    selected = [item for item in records(directory, side)
                if before is None or item["number"] < before]
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
        if not running(backend, result["storage"]["process_receipt"], result["storage"]["identity"], arguments(request), (result["api_url"], result["console_url"])):
            raise ValueError("已登记存储进程已退出，必须显式同目录重启")
    return result
