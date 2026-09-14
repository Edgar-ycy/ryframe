"""服务生命周期的只读输入快照与精确进程观察。"""
from __future__ import annotations

from pathlib import Path
import time

from devex_clone_capture import read_json
from devex_clone_model import local_path
from devex_clone_run_state import binding, controller_observation, load_state
from devex_clone_source_proof import bound_file, require_closed_port
from devex_clone_storage_process import inspect_attempt, running
from devex_clone_storage_request import arguments, directory_identity
from full_stack_process import process_identity
from full_stack_process_tree import read_process_tree
from reference_fixture_paths import service_run
from reference_fixture_service_history import (close_reconciliation_evidence,
                                               reconcilable_failed_close, validate_history,
                                               validate_manifest)


def context(backend: Path, review_path: Path, bootstrap_path: Path) -> dict:
    from reference_fixture_services import document, environment

    review_file, review = document(backend, review_path)
    bootstrap_file, bootstrap, private = environment(backend, review_file, review, bootstrap_path)
    run = local_path(backend, str(service_run(review)))
    manifest_file, manifest = document(backend, run / "manifest.json")
    validate_manifest(manifest)
    if (manifest["review"] != binding(review_file) or manifest["bootstrap"] != binding(bootstrap_file)
            or manifest["execution_backend"] != bootstrap["execution_backend"]
            or manifest["scope_id"] != review["services"]["rustfs"]["scope_id"]):
        raise ValueError("夹具服务生命周期未绑定原始审阅、私有环境与执行目录")
    state_binding = binding(run / "state.json")
    state = load_state(run)
    history = validate_history(run, state, ready=False, settled=False)
    sources = {"review": binding(review_file), "bootstrap": binding(bootstrap_file),
               "manifest": binding(manifest_file), "state_before": state_binding,
               "environment": binding(bootstrap_file.parent / "environment.json")}
    value = {"backend": backend, "run": run, "manifest": manifest, "state": state, "history": history,
             "sources": sources, "private": private,
             "documents": {"review": review, "bootstrap": bootstrap, "manifest": manifest}}
    guard(value)
    return value


def guard(value: dict, state_binding: dict | None = None) -> None:
    for key, descriptor in value["sources"].items():
        expected = state_binding if key == "state_before" and state_binding is not None else descriptor
        if binding(bound_file(value["backend"], expected)) != expected:
            raise ValueError("夹具服务生命周期输入在执行期间发生变化")
        if key in value["documents"] and read_json(Path(expected["path"])) != value["documents"][key]:
            raise ValueError("夹具服务输入内容与采样摘要不一致")
    if state_binding is None and load_state(value["run"]) != value["state"]:
        raise ValueError("夹具服务账本在采样期间变化")
    if read_json(Path(value["sources"]["environment"]["path"])).get("environment") != value["private"]:
        raise ValueError("夹具服务私有环境在采样期间变化")


def unknown_recoveries(run: Path) -> list[str]:
    state = load_state(run)
    consumed = {read_json(Path(item["result"]["path"])).get("recovery", {}).get("path")
                for item in state["attempts"] if item["stage"] == "fixture-services"
                and item["mode"] == "recover" and item["status"] == "passed"}
    unknown = {path.name for path in run.glob("recovery-*.intent.json")
               if not (run / ("recovered-" + path.name[len("recovery-"):-len(".intent.json")] + ".json")).is_file()}
    unknown.update(path.name for path in run.glob("recovered-*.json") if str(path) not in consumed)
    expected = {f"lifecycle-{item['number']:04d}" for item in state["attempts"]
                if item["stage"] == "fixture-services" and item["mode"] in {"close", "reconcile", "restart"}}
    for item in state["attempts"]:
        if (item["stage"], item["mode"], item["status"]) != ("fixture-services", "recover", "passed"):
            continue
        result = read_json(Path(item["result"]["path"]))
        if result.get("status") == "external_termination_reconciled":
            expected.add(f"lifecycle-{item['number']:04d}")
    unknown.update(path.name for path in run.glob("lifecycle-*") if path.name not in expected)
    return sorted(unknown)


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
    return {"requests": requests, "tree": tree, "runtime": cache["runtime"], "storage": storage,
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
            raise ValueError(f"RustFS {role} PID 已复用")
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
    from devex_clone_cache_process import status as cache_status

    cache = cache_status(services["requests"]["redis"], services["runtime"])
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
    """让既有 fresh-target 仅沿已验证的夹具服务 lineage 采用新运行身份。"""
    manifest = read_json(run / "manifest.json")
    value = context(backend, Path(manifest["review"]["path"]), Path(manifest["bootstrap"]["path"]))
    if value["history"]["active_generation"].get("kind") == "initial":
        return None, None
    services = registered_services(value)
    if expected != services["origin"]:
        raise ValueError("夹具服务重启 lineage 未绑定 fresh-target 的原始服务代次")
    storage = {key: services["storage"][key]
               for key in ("identity", "sha256", "process_receipt", "launch_receipt")}
    request = services["requests"]["rustfs"]
    return ({"storage": storage, "data_directory": request["data_directory"],
             "api_url": request["api_url"], "console_url": request["console_url"],
             "generation": value["history"]["active_generation"]},
            {"redis": services["runtime"]["redis"],
             "generation": value["history"]["active_generation"]})


def status(backend: Path, review: Path, bootstrap: Path) -> dict:
    value = context(backend, review, bootstrap)
    controller = controller_observation(value["run"])
    unsettled = value["history"]["unsettled"]
    unknown = unknown_recoveries(value["run"])
    observations = None
    next_operation = "close"
    if unknown:
        next_operation = "reconcile-evidence"
    elif controller is not None:
        if not controller["process_matches"] and not controller["process_missing"]:
            raise ValueError("控制器 PID 已复用，不能等待或恢复无关进程")
        next_operation = "recover" if controller["process_missing"] else "wait-controller"
    elif unsettled:
        failed = reconcilable_failed_close(value["state"], value["history"])
        if failed is not None:
            try:
                services = registered_services(value, settled=False)
                close_reconciliation_evidence(value["run"], failed,
                                              value["history"]["active_generation"],
                                              value["sources"], services)
            except (FileNotFoundError, IsADirectoryError, KeyError, OSError, TypeError, ValueError):
                failed = None
        next_operation = "reconcile" if failed is not None else "reconcile-evidence"
    elif not value["history"]["initial_complete"]:
        attempts = value["state"]["attempts"]
        count = next((index for index, item in enumerate(attempts) if item["stage"] == "fixture-services"), len(attempts))
        operation = ("rustfs", "redis", "buckets")[count]
        next_operation = (operation if count and count == len(attempts)
                          and not (value["run"] / operation).exists() else "reconcile-evidence")
    else:
        observations = observe_services(registered_services(value))
        if observations["termination"] is not None:
            reconciled = value["history"].get("external_recovery")
            next_operation = "restart" if reconciled is not None else "recover"
            reconciliation = {**observations["termination"],
                              "owner": value["sources"]["state_before"], "receipt": reconciled}
            observations = ("external-termination-reconciled" if reconciled is not None
                            else "external-termination-unreconciled")
        else:
            reconciliation = None
        if value["history"]["closed"]:
            if observations != {"redis": "stopped", "rustfs": "stopped", "termination": None}:
                raise ValueError("已关闭夹具服务又出现存活进程")
            next_operation = "restart"
    if (not isinstance(observations, str)
            or observations not in {"external-termination-unreconciled", "external-termination-reconciled"}):
        reconciliation = None
    guard(value)
    return {"status": "service_status", "run": str(value["run"]), "controller": controller,
            "state": value["sources"]["state_before"], "services": observations, "unsettled": unsettled,
            "unknown_recoveries": unknown, "reconciliation": reconciliation,
            "next_operation": next_operation, "remote_writes": 0}
