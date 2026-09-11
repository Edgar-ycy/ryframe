"""服务生命周期的只读输入快照与精确进程观察。"""
from __future__ import annotations

from pathlib import Path

from devex_clone_capture import read_json
from devex_clone_model import local_path
from devex_clone_run_state import binding, controller_observation, load_state
from devex_clone_source_proof import bound_file, require_closed_port
from devex_clone_storage_process import inspect_attempt, running
from devex_clone_storage_request import arguments
from full_stack_process import process_identity
from full_stack_process_tree import read_process_tree
from reference_fixture_paths import service_run
from reference_fixture_service_history import validate_history, validate_manifest


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
                if item["stage"] == "fixture-services" and item["mode"] == "close"}
    unknown.update(path.name for path in run.glob("lifecycle-*") if path.name not in expected)
    return sorted(unknown)


def registered_services(value: dict) -> dict:
    run, backend, state = value["run"], value["backend"], value["state"]
    validate_history(run, state)
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
    return {"requests": requests, "tree": tree, "runtime": cache["runtime"], "storage": observed}


def observe_services(services: dict) -> dict:
    from devex_clone_cache_process import status as cache_status

    cache = cache_status(services["requests"]["redis"], services["runtime"])
    request, tree = services["requests"]["rustfs"], services["tree"]
    alive = running(tree["process"], arguments(request), (request["api_url"], request["console_url"]), listeners=False)
    supervisor = process_identity(tree["supervisor"]["pid"])
    if supervisor is not None and supervisor != tree["supervisor"]:
        raise ValueError("RustFS 监督器 PID 已复用")
    if alive and supervisor is None:
        raise ValueError("RustFS 产品仍存活但原进程树监督器缺失")
    if alive and process_identity(tree["monitor"]["pid"]) != tree["monitor"]:
        raise ValueError("RustFS 完整成员监督器缺失或身份变化")
    if not alive:
        if supervisor is not None:
            raise ValueError("RustFS 产品已退出但监督进程尚未退出")
        from full_stack_process_monitor import wait_members
        wait_members(tree, timeout=0)
        require_closed_port(request["api_url"])
        require_closed_port(request["console_url"])
    return {"redis": cache["state"], "rustfs": "running" if alive else "stopped"}


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
        next_operation = "reconcile-evidence"
    elif not value["history"]["initial_complete"]:
        attempts = value["state"]["attempts"]
        count = next((index for index, item in enumerate(attempts) if item["stage"] == "fixture-services"), len(attempts))
        operation = ("rustfs", "redis", "buckets")[count]
        next_operation = (operation if count and count == len(attempts)
                          and not (value["run"] / operation).exists() else "reconcile-evidence")
    else:
        observations = observe_services(registered_services(value))
        if value["history"]["closed"]:
            if observations != {"redis": "stopped", "rustfs": "stopped"}:
                raise ValueError("已关闭夹具服务又出现存活进程")
            next_operation = "none"
    guard(value)
    return {"status": "service_status", "run": str(value["run"]), "controller": controller,
            "state": value["sources"]["state_before"], "services": observations, "unsettled": unsettled,
            "unknown_recoveries": unknown, "next_operation": next_operation, "remote_writes": 0}
