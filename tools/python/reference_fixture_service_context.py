"""服务生命周期的只读输入快照与精确进程观察。"""
from __future__ import annotations

import copy
from pathlib import Path
import time

from devex_clone_capture import read_json
from devex_clone_run_state import binding
from devex_clone_source_proof import bound_file, require_closed_port
from devex_clone_storage_process import inspect_attempt, running
from devex_clone_storage_request import arguments, directory_identity
from full_stack_process import process_identity
from full_stack_process_tree import read_process_tree
from rate_limit_client import load_app_table
from fixture_service_client import _invoke, lifecycle, validate_history


def service_run(review: dict) -> Path:
    return Path(_invoke("service-run", {"review": review}))


def application_settings(bootstrap_file: Path, execution: Path) -> dict:
    """从已绑定 bootstrap 相邻的唯一明文配置目录读取应用值。"""
    return load_app_table(execution, {"APP_ENV": "test"}, config_dir=bootstrap_file.parent / "config")


def load_prepared_environment(backend: Path, value: Path, secret_files: tuple[str, ...]) -> tuple[Path, Path, dict, dict]:
    receipt, execution, private, document = _invoke("service-prepared", {"path": value, "secret_files": secret_files}, backend=backend)
    return Path(receipt), Path(execution), private, document


def context(backend: Path, review_path: Path, bootstrap_path: Path) -> dict:
    value = _invoke("service-context", {"review": review_path, "environment": bootstrap_path}, backend=backend)
    value["backend"], value["run"] = Path(value["backend"]), Path(value["run"])
    return value


def guard(value: dict, state_binding: dict | None = None) -> None:
    _invoke("service-guard", {"value": value, "running": state_binding}, backend=value["backend"])




def registered_services(value: dict, *, settled: bool = True) -> dict:
    run, backend, state = value["run"], value["backend"], value["state"]
    history = validate_history(run, state, settled=settled)
    first, second = state["attempts"][:2]
    requests = {}
    for role, attempt in (("rustfs", first), ("redis", second)):
        descriptor = attempt["sources"].get("request")
        if not isinstance(descriptor, dict):
            raise ValueError("历史首代缺少冻结服务请求，不能补造完整生命周期证据")
        path = bound_file(backend, descriptor)
        if path != run / role / "request.json":
            raise ValueError("冻结服务请求不属于原启动目录")
        requests[role] = read_json(path)
    directory_identity(backend, requests["rustfs"]["data_directory"])
    storage = read_json(bound_file(backend, first["result"]))
    tree_binding = storage.get("tree")
    if not isinstance(tree_binding, dict) or bound_file(backend, tree_binding) != run / "rustfs/rustfs-tree.json":
        raise ValueError("历史首代缺少启动前进程树归属，不能宣称完整后代可回收")
    tree = read_process_tree(run / "rustfs", "rustfs", requests["rustfs"]["scope_id"])
    if tree["process"] != storage["identity"]:
        raise ValueError("RustFS 进程树与首代真实创建身份不同")
    controller = binding(run / "controller-0001.json")
    observed = inspect_attempt(backend, run / "rustfs", value["sources"]["manifest"], controller, 1,
                               request_binding=first["sources"]["request"])
    if observed is None or observed["state"] != "recorded" or observed["identity"] != tree["process"]:
        raise ValueError("RustFS 首代意图、进程记录与树归属不一致")
    if (any(storage.get(key) != observed.get(key) for key in ("process_receipt", "launch_receipt"))
            or storage.get("sha256") != requests["rustfs"]["executable"]["sha256"]):
        raise ValueError("RustFS 成功结果与冻结启动来源不同")
    cache = read_json(bound_file(backend, second["result"]))
    if (cache.get("service") != "redis" or not isinstance(cache.get("runtime"), dict)
            or read_json(run / "redis/runtime.json") != cache["runtime"]):
        raise ValueError("Redis 首代缺少冻结运行收据")
    initial = {"storage": {key: storage[key] for key in ("identity", "sha256", "process_receipt", "launch_receipt")},
               "redis": cache["runtime"]["redis"]}
    if history["active_generation"].get("kind") != "initial":
        outer = read_json(bound_file(backend, history["active_generation"]))
        generation = read_json(bound_file(backend, outer["generation"]))
        requests = {role: read_json(bound_file(backend, generation[role]["request"]))
                    for role in ("rustfs", "redis")}
        storage, cache = generation["rustfs"]["storage"], {"runtime": generation["redis"]["runtime"]}
        tree_binding = storage["tree"]
        tree_path = bound_file(backend, tree_binding)
        tree = read_process_tree(tree_path.parent, "rustfs", requests["rustfs"]["scope_id"])
        observed = inspect_attempt(backend, tree_path.parent, value["sources"]["manifest"],
                                   generation["controller"], generation["attempt"],
                                   request_binding=generation["rustfs"]["request"])
        if observed is None or observed["state"] != "recorded" or observed["identity"] != tree["process"]:
            raise ValueError("RustFS 当前重启代次的意图、进程和树归属不一致")
        if (any(storage.get(key) != observed.get(key) for key in ("identity", "process_receipt", "launch_receipt"))
                or storage.get("sha256") != requests["rustfs"]["executable"]["sha256"]):
            raise ValueError("RustFS 当前重启结果与冻结请求不同")
        if read_json(Path(cache["runtime"]["output"]) / "runtime.json") != cache["runtime"]:
            raise ValueError("Redis 当前重启运行收据变化")
    runtime_path = (run / "redis/runtime.json" if history["active_generation"].get("kind") == "initial"
                    else Path(cache["runtime"]["output"]) / "runtime.json")
    cache_effective = None
    current_review = value["documents"]["review"]
    if value["manifest"]["review"] != value["sources"]["review"]:
        tools = current_review["tools"]
        redis, python = tools["redis_server"], tools["redis_python"]
        original = requests["redis"]
        if (redis["distribution"] != original["distribution"]
                or redis["resolved_path"] != original["executable"]
                or redis["sha256"] != original["sha256"]
                or python["distribution"] != original["distribution"]
                or python["path"] != original["python"]["path"]
                or python["resolved_path"] != original["python"]["executable"]):
            raise ValueError("夹具工具续签只能替换 WSL 或同一 Python 入口的字节代次")
        cache_effective = {**copy.deepcopy(original), "wsl": copy.deepcopy(tools["wsl"]),
                           "python": {"path": python["path"], "executable": python["resolved_path"],
                                      "sha256": python["sha256"]}}
        if cache_effective == original:
            cache_effective = None
    return {"requests": requests, "tree": tree, "runtime": cache["runtime"], "storage": storage,
            "cache_effective": cache_effective,
            "storage_observation": observed,
            "origin": initial,
            "evidence": {"rustfs_tree": tree_binding,
                         "redis_runtime": binding(runtime_path)}}


def _identity_observations(tree: dict) -> dict:
    observations = {}
    deadline = time.monotonic() + 5
    for role in ("supervisor", "monitor", "process"):
        expected = tree[role]
        while True:
            try:
                actual = process_identity(expected["pid"])
                break
            except PermissionError:
                if time.monotonic() >= deadline:
                    raise
                time.sleep(0.05)
        if actual is not None and actual != expected:
            # 身份不同说明被记录进程已经退出，PID 现由无关进程复用：按“被记录身份已消失”
            # 记账；任何终止或等待仍由 full_stack_process 的身份校验拒绝复用 PID。
            observations[role] = "missing"
            continue
        observations[role] = "running" if actual is not None else "missing"
    return observations


def _external_termination(services: dict, identities: dict) -> dict:
    from full_stack_process_monitor import receipt_path

    request, tree = services["requests"]["rustfs"], services["tree"]
    directory = Path(tree["runtime_directory"])
    operation = tree["operation_id"]
    ready_path = receipt_path(directory, tree["role"], operation, "ready")
    ready = read_json(ready_path)
    expected = {"format_version": 1, "operation_id": operation, "directory": str(directory),
                "role": tree["role"], "scope_id": tree["scope_id"],
                "supervisor": tree["supervisor"], "monitor": tree["monitor"]}
    if ready != expected:
        raise ValueError("RustFS 成员监督器启动归属证据缺失或变化")
    unexpected = [path.name for path in (
        directory / f"{tree['role']}-tree-{operation}-control.json",
        directory / f"{tree['role']}-tree-{operation}-result.json",
    ) if path.exists()]
    if unexpected:
        raise ValueError("RustFS 外部终止现场包含未完成的控制或关闭证据")
    for url in (request["api_url"], request["console_url"]):
        require_closed_port(url)
    if _identity_observations(tree) != identities:
        raise ValueError("RustFS 外部终止核对期间进程身份变化")
    return {"status": "external-termination-unreconciled", "identities": identities,
            "normal_shutdown_proof": None,
            "evidence": {**services["evidence"], "rustfs_monitor_ready": binding(ready_path)}}


def observe_services(services: dict) -> dict:
    from devex_clone_cache_process import predecessor_status, status as cache_status

    effective = services.get("cache_effective")
    cache = (cache_status(services["requests"]["redis"], services["runtime"])
             if effective is None else predecessor_status(
                 services["requests"]["redis"], services["runtime"], effective))
    request, tree = services["requests"]["rustfs"], services["tree"]
    identities = _identity_observations(tree)
    if set(identities.values()) == {"running"}:
        if not running(tree["process"], arguments(request),
                       (request["api_url"], request["console_url"])):
            raise ValueError("RustFS 完整进程树观察期间产品进程退出")
        rustfs, termination = "running", None
    elif set(identities.values()) == {"missing"}:
        from full_stack_process_monitor import receipt_path, wait_members

        stopped = receipt_path(
            Path(tree["runtime_directory"]), tree["role"], tree["operation_id"], "stopped")
        if stopped.exists():
            wait_members(tree, timeout=0)
            for url in (request["api_url"], request["console_url"]):
                require_closed_port(url)
            rustfs, termination = "stopped", None
        else:
            termination = _external_termination(services, identities)
            rustfs = termination["status"]
    else:
        raise ValueError("RustFS 进程树仅部分存活，不能判定服务状态")
    if termination is not None and cache["state"] != "stopped":
        raise ValueError("RustFS 外部终止时 Redis 仍存活或状态未知")
    return {"redis": cache["state"], "rustfs": rustfs, "termination": termination}


def runtime_transition(backend: Path, run: Path, expected: dict) -> tuple[dict | None, dict | None]:
    return tuple(_invoke("service-transition", {"run": run, "expected": expected}, backend=backend))




def status(backend: Path, review: Path, bootstrap: Path) -> dict:
    return lifecycle(backend, "status", review, bootstrap, None, write=False)
