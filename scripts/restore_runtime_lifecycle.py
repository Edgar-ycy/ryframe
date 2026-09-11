"""用同一 ownership 锁管理正式恢复 API、Worker 与前端的完整进程树。"""

from __future__ import annotations

import argparse
import time
from pathlib import Path

from devex_clone_source_proof import require_closed_port
from full_stack_process import assert_identity, process_identity, read_process
from full_stack_process_monitor import completion_binding
from full_stack_process_tree import (
    launch_supervised_process,
    read_process_tree,
    terminate_owned_process_tree,
)
from process_sockets import verify_listener
from restore_runtime_evidence import read_json_document
from restore_runtime_generation import (
    FINAL_STATES,
    LAUNCH,
    create_intent,
    creation_path,
    descriptor,
    initial_state,
    new_generation,
    read_creation,
    save_state,
    state_document,
    validate_launch,
    validate_state,
    write_launch,
)
from restore_runtime_launch import ROLES, launch_environment, prepare_request
from restore_runtime_registration import (
    REGISTRATION_LOCK,
    registration_binding,
    runtime_control_directory,
    verify_registration,
)
from runtime_control_lock import controller_lock, reconcile_lock


def add_arguments(parser: argparse.ArgumentParser, operation: str) -> None:
    parser.add_argument("--runtime-registration", type=Path, required=True)
    parser.add_argument("--target-plan", type=Path, required=True)
    if operation == "start":
        parser.add_argument("--source-backend", type=Path, required=True)
        parser.add_argument("--source-frontend", type=Path, required=True)
        parser.add_argument("--build-receipt", type=Path, required=True)
        parser.add_argument("--bindings", type=Path, required=True)
        parser.add_argument("--adapter-contract")
        parser.add_argument("--product-backend", type=Path)
        parser.add_argument("--timeout", type=float, default=60)
        parser.add_argument("--write", action="store_true", required=True)
    elif operation in {"stop", "recover"}:
        parser.add_argument("--generation", type=int, required=True)
        parser.add_argument("--write", action="store_true", required=True)
        if operation == "recover":
            parser.add_argument("--owner", type=Path, required=True)


def _descriptor(document) -> dict:
    return {"path": str(document.path), "bytes": len(document.raw), "sha256": document.sha256}


def _binding(coordinator: Path, registration_path: Path, target_plan_path: Path):
    target = read_json_document(target_plan_path)
    value, facts, documents = registration_binding(
        coordinator, registration_path, _descriptor(target)
    )
    if documents[2].path != target.path or documents[2].raw != target.raw:
        raise ValueError("恢复运行 target plan 在调用绑定期间发生变化")
    root = Path(facts["runtime_directory"])
    local = runtime_control_directory(coordinator)
    if root == local or not root.is_relative_to(local):
        raise ValueError("恢复运行目录必须是协调器忽略目录内的明确子目录")
    return root, {
        "registration": _descriptor(documents[0]),
        "target_plan": facts["target_plan"],
    }, value, facts, documents


def _operation(name: str, binding: dict) -> str:
    return (
        f"runtime-{name}:{binding['registration']['sha256']}:"
        f"{binding['target_plan']['sha256']}"
    )


def _wait_ready(process, role: str, probe, url: str, timeout: float) -> None:
    deadline, latest = time.monotonic() + timeout, None
    while time.monotonic() < deadline:
        if process.poll() is not None:
            raise RuntimeError(f"{role} 在就绪前退出")
        try:
            probe()
            verify_listener(process.pid, url)
            return
        except (OSError, ValueError) as error:
            latest = error
            time.sleep(0.1)
    raise TimeoutError(f"{role} 就绪超时：{latest}")


def _observe_role(generation: dict, role: str) -> dict:
    item = generation["roles"][role]
    path = Path(generation["directory"])
    process_path, tree_path = path / f"{role}.json", path / f"{role}-tree.json"
    url = generation["request"]["authority"][f"{role}_endpoint"]
    if not process_path.exists() and not tree_path.exists():
        require_closed_port(url)
        return {"state": "pending", "identity": None, "tree": None, "completion": None}
    if process_path.exists() and not tree_path.is_file():
        raise ValueError(f"{role} 进程收据存在但缺少 Job 或进程组树证据")
    if not tree_path.is_file():
        raise ValueError(f"{role} 进程树证据不是可信普通文件")
    scope = generation["request"]["authority"]["scope_id"]
    tree = read_process_tree(path, role, scope)
    identity = tree["process"]
    if process_path.is_file() and read_process(path, role, scope) != identity:
        raise ValueError(f"{role} 产品进程收据与完整树不一致")
    if tree["operation_id"] != item["operation_id"]:
        raise ValueError(f"{role} 进程树与启动 intent 不一致")
    actual = process_identity(identity["pid"])
    if actual is not None:
        assert_identity(actual, identity)
        if Path(identity["executable"]) != Path(item["command"][0]):
            raise ValueError(f"{role} 进程没有运行登记的可执行文件")
        verify_listener(identity["pid"], url)
        return {"state": "running", "identity": identity, "tree": tree, "completion": None}
    completion = completion_binding(tree)
    require_closed_port(url)
    return {"state": "stopped", "identity": identity, "tree": tree, "completion": completion}


def _observe_generation(generation: dict) -> dict:
    return {role: _observe_role(generation, role) for role in ROLES}


def _derived_status(generation: dict, observed: dict) -> str:
    states = {item["state"] for item in observed.values()}
    if states == {"running"}:
        return "running"
    if "running" in states:
        return "degraded"
    if generation["status"] in FINAL_STATES:
        return generation["status"]
    return "exited" if states == {"stopped"} else "interrupted"


def _stop_roles(generation: dict, observed: dict) -> list[str]:
    errors = []
    for role in reversed(ROLES):
        item, current = generation["roles"][role], observed[role]
        try:
            if current["tree"] is not None:
                if current["state"] == "running":
                    terminate_owned_process_tree(current["tree"])
                item["tree"] = current["tree"]
                item["completion"] = completion_binding(current["tree"])
            require_closed_port(generation["request"]["authority"][f"{role}_endpoint"])
            item["state"] = "stopped"
        except BaseException as error:
            errors.append(f"{role}:{type(error).__name__}:{error}")
    return errors


def start(args, coordinator: Path, probe_api, probe_worker, probe_frontend) -> dict:
    if isinstance(args.timeout, bool) or not 0 < args.timeout <= 180:
        raise ValueError("恢复运行就绪超时必须在 0 到 180 秒之间")
    root, request = prepare_request(
        coordinator,
        args.runtime_registration,
        args.target_plan,
        args.source_backend,
        args.source_frontend,
        args.build_receipt,
        args.bindings,
        adapter_contract=args.adapter_contract,
        product_backend=args.product_backend,
    )
    binding = request["registration"]
    control = runtime_control_directory(coordinator)
    with controller_lock(control, _operation("start", binding), REGISTRATION_LOCK):
        repeated_root, repeated = prepare_request(
            coordinator,
            args.runtime_registration,
            args.target_plan,
            args.source_backend,
            args.source_frontend,
            args.build_receipt,
            args.bindings,
            adapter_contract=args.adapter_contract,
            product_backend=args.product_backend,
        )
        if repeated_root != root or repeated != request:
            raise ValueError("取得运行控制权前启动来源发生变化")
        document = state_document(root) if root.exists() else None
        if document is None:
            registration, live_facts, _documents = verify_registration(
                coordinator, args.runtime_registration, binding["target_plan"]
            )
            if Path(live_facts["runtime_directory"]) != root:
                raise ValueError("fresh target 登记运行目录发生变化")
            creation = create_intent(root, binding, request)
            if registration["observation"]["runtime"]["exists"]:
                if not root.is_dir() or any(root.iterdir()):
                    raise ValueError("登记的空运行目录在 lifecycle 写入前发生变化")
            else:
                root.mkdir()
            state = initial_state(root, binding, creation, request)
            generation = state["generations"][0]
            document = save_state(root, state, binding, None)
            generation_path = Path(generation["directory"])
            generation_path.mkdir()
            document = read_json_document(document.path)
            validate_state(document.value, root, binding)
        else:
            state = validate_state(document.value, root, binding)
            previous = state["generations"][-1]
            observed = _observe_generation(previous)
            if previous["status"] not in FINAL_STATES or any(
                item["state"] == "running" for item in observed.values()
            ):
                raise ValueError("上一恢复运行代次尚未明确停止")
            generation = new_generation(root, len(state["generations"]) + 1, request)
            state["generations"].append(generation)
            document = save_state(root, state, binding, document)
            generation_path = Path(generation["directory"])
            generation_path.mkdir()
            document = read_json_document(document.path)
            validate_state(document.value, root, binding)
        started = {}
        probes = {
            "api": lambda: probe_api(request["authority"]["api_endpoint"]),
            "worker": lambda: probe_worker(request["authority"]["worker_endpoint"]),
            "frontend": lambda: probe_frontend(
                Path(request["roots"]["frontend"]),
                read_json_document(Path(request["paths"]["frontend_build"])).value,
                request["authority"]["frontend_endpoint"],
            ),
        }
        try:
            for role in ROLES:
                item = generation["roles"][role]
                with Path(item["log"]).open("xb") as log:
                    process = launch_supervised_process(
                        generation_path,
                        role,
                        request["authority"]["scope_id"],
                        item["command"],
                        Path(
                            request["roots"][
                                "frontend" if role == "frontend" else "backend_execution"
                            ]
                        ),
                        launch_environment(request, role),
                        log,
                        operation_id=item["operation_id"],
                    )
                started[role] = process
                item["tree"], item["state"] = process.tree, "running"
                document = save_state(root, state, binding, document)
                _wait_ready(
                    process,
                    role,
                    probes[role],
                    request["authority"][f"{role}_endpoint"],
                    args.timeout,
                )
            current_root, current = prepare_request(
                coordinator,
                args.runtime_registration,
                args.target_plan,
                args.source_backend,
                args.source_frontend,
                args.build_receipt,
                args.bindings,
                adapter_contract=args.adapter_contract,
                product_backend=args.product_backend,
            )
            if current_root != root or current != request:
                raise ValueError("恢复运行启动期间来源、配置或工具发生变化")
            generation["launch"] = write_launch(generation)
            generation["status"] = "running"
            document = save_state(root, state, binding, document)
            document.assert_unchanged()
            for process in started.values():
                process.release_controller_handle()
            return {
                "format_version": 1,
                "kind": "restore-runtime-lifecycle-result",
                "operation": "start",
                "status": "running",
                "generation": generation["number"],
                "runtime_directory": generation["directory"],
                "launch": generation["launch"],
            }
        except BaseException as error:
            generation["error_type"] = type(error).__name__
            observed = {}
            for role in ROLES:
                try:
                    observed[role] = _observe_role(generation, role)
                except BaseException as observation:
                    error.add_note(f"{role} 启动失败现场观察失败：{observation}")
                    process = started.get(role)
                    observed[role] = {
                        "state": "running" if process is not None and process.poll() is None else "pending",
                        "tree": None if process is None else process.tree,
                    }
            failures = _stop_roles(generation, observed)
            for role, process in started.items():
                try:
                    process.wait(timeout=10)
                except BaseException as cleanup:
                    failures.append(f"{role}-wait:{type(cleanup).__name__}:{cleanup}")
            generation["status"] = "cleanup_failed" if failures else "start_failed"
            try:
                save_state(root, state, binding, document)
            except BaseException as cleanup:
                failures.append(f"state:{type(cleanup).__name__}:{cleanup}")
            if failures:
                error.add_note("恢复运行启动失败后的完整树回收失败：" + "; ".join(failures))
            raise


def _load_locked(args, coordinator: Path):
    root, binding, _registration, facts, documents = _binding(
        coordinator, args.runtime_registration, args.target_plan
    )
    document = state_document(root) if root.exists() else None
    if document is None:
        return root, binding, facts, documents, None, None
    state = validate_state(document.value, root, binding)
    return root, binding, facts, documents, document, state


def _verify_interrupted_creation(root: Path, binding: dict, facts: dict):
    creation, request = read_creation(root, binding)
    if root.exists():
        if not root.is_dir() or root.is_symlink() or any(root.iterdir()):
            raise ValueError("首次运行目录中存在未登记文件，拒绝自动接管")
    for endpoint in facts["endpoints"].values():
        require_closed_port(endpoint)
    creation.assert_unchanged()
    return creation, request["request"]


def status(args, coordinator: Path) -> dict:
    root, binding, _registration, _facts, _documents = _binding(
        coordinator, args.runtime_registration, args.target_plan
    )
    control = runtime_control_directory(coordinator)
    with controller_lock(control, _operation("status", binding), REGISTRATION_LOCK):
        root, binding, facts, documents, document, state = _load_locked(args, coordinator)
        if state is None:
            if creation_path(root).exists():
                _creation, _request = _verify_interrupted_creation(root, binding, facts)
                return {
                    "format_version": 1,
                    "kind": "restore-runtime-lifecycle-result",
                    "operation": "status",
                    "status": "interrupted_before_first_generation",
                    "generation": 1,
                    "registration": binding,
                }
            verify_registration(coordinator, args.runtime_registration, binding["target_plan"])
            return {
                "format_version": 1,
                "kind": "restore-runtime-lifecycle-result",
                "operation": "status",
                "status": "registered_not_started",
                "registration": binding,
            }
        generation = state["generations"][-1]
        observed = _observe_generation(generation)
        for item in (*documents, document):
            item.assert_unchanged()
        return {
            "format_version": 1,
            "kind": "restore-runtime-lifecycle-result",
            "operation": "status",
            "status": _derived_status(generation, observed),
            "persisted_status": generation["status"],
            "generation": generation["number"],
            "runtime_directory": generation["directory"],
            "processes": observed,
            "launch": generation["launch"],
        }


def _stop_locked(args, coordinator: Path) -> dict:
    root, binding, _facts, documents, document, state = _load_locked(args, coordinator)
    if state is None:
        raise ValueError("恢复运行尚未启动，不能执行停止")
    generation = state["generations"][-1]
    if type(args.generation) is not int or args.generation != generation["number"]:
        raise ValueError("停止操作必须绑定当前明确运行代次")
    observed = _observe_generation(generation)
    if generation["status"] == "stopped" and all(
        item["state"] != "running" for item in observed.values()
    ):
        return {
            "format_version": 1,
            "kind": "restore-runtime-lifecycle-result",
            "operation": "stop",
            "status": "already_stopped",
            "generation": generation["number"],
            "runtime_directory": generation["directory"],
            "processes": observed,
        }
    if generation["launch"] is None:
        launch_path = Path(generation["directory"]) / LAUNCH
        if launch_path.exists():
            launch = read_json_document(launch_path)
            validate_launch(launch.value, Path(generation["directory"]))
            generation["launch"] = descriptor(launch)
            document = save_state(root, state, binding, document)
    generation["status"] = "stopping"
    generation_path = Path(generation["directory"])
    generation_path.mkdir(exist_ok=True)
    document = save_state(root, state, binding, document)
    failures = _stop_roles(generation, observed)
    generation["status"] = "stop_failed" if failures else "stopped"
    generation["error_type"] = "RuntimeStopError" if failures else None
    document = save_state(root, state, binding, document)
    for item in (*documents, document):
        item.assert_unchanged()
    if failures:
        raise ValueError("恢复运行未能回收全部进程树：" + "; ".join(failures))
    return {
        "format_version": 1,
        "kind": "restore-runtime-lifecycle-result",
        "operation": "stop",
        "status": "stopped",
        "generation": generation["number"],
        "runtime_directory": generation["directory"],
        "processes": _observe_generation(generation),
    }


def stop(args, coordinator: Path) -> dict:
    _root, binding, _registration, _facts, _documents = _binding(
        coordinator, args.runtime_registration, args.target_plan
    )
    control = runtime_control_directory(coordinator)
    with controller_lock(control, _operation("stop", binding), REGISTRATION_LOCK):
        return _stop_locked(args, coordinator)


def recover(args, coordinator: Path) -> dict:
    root, binding, _registration, facts, _documents = _binding(
        coordinator, args.runtime_registration, args.target_plan
    )
    control = runtime_control_directory(coordinator)
    expected = control / REGISTRATION_LOCK.lock_name / "owner.json"
    if args.owner.absolute() != expected:
        raise ValueError("控制恢复必须显式绑定全局恢复运行 owner 文件")
    owner = read_json_document(expected)
    allowed = {_operation(name, binding) for name in ("start", "status", "stop", "recover")}
    allowed.add(_operation("bind", binding) + f":{args.generation}")
    if owner.value.get("operation") not in allowed:
        raise ValueError("控制 owner 不属于同一登记的运行生命周期")
    state = state_document(root) if root.exists() else None
    if state is None:
        if args.generation != 1 or owner.value["operation"] != _operation("start", binding):
            raise ValueError("无生命周期状态时只能恢复首代启动控制器")
        creation = creation_path(root)
        if creation.exists():
            _verify_interrupted_creation(root, binding, facts)
        else:
            verify_registration(coordinator, args.runtime_registration, binding["target_plan"])
    else:
        value = validate_state(state.value, root, binding)
        if args.generation != value["generations"][-1]["number"]:
            raise ValueError("控制恢复必须绑定当前明确运行代次")
    reconciliation = reconcile_lock(control, REGISTRATION_LOCK, expected_owner=owner.value)
    with controller_lock(control, _operation("recover", binding), REGISTRATION_LOCK):
        current = state_document(root) if root.exists() else None
        if current is None:
            if not creation_path(root).exists():
                return {
                    "format_version": 1,
                    "kind": "restore-runtime-lifecycle-result",
                    "operation": "recover",
                    "status": "controller_reconciled_not_started",
                    "recovery": reconciliation,
                }
            creation, request = _verify_interrupted_creation(root, binding, facts)
            root.mkdir(exist_ok=True)
            lifecycle = initial_state(root, binding, creation, request)
            save_state(root, lifecycle, binding, None)
        stopped = _stop_locked(args, coordinator)
    return {**stopped, "operation": "recover", "recovery": reconciliation}


def dispatch(args, coordinator: Path, probe_api, probe_worker, probe_frontend) -> dict:
    if args.command == "start":
        return start(args, coordinator, probe_api, probe_worker, probe_frontend)
    if args.command == "status":
        return status(args, coordinator)
    if args.command == "stop":
        return stop(args, coordinator)
    if args.command == "recover":
        return recover(args, coordinator)
    raise ValueError("未知恢复运行生命周期操作")
