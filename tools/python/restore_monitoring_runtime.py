"""隔离监控的启动、观测、关闭与最终成功收据状态机。"""

from __future__ import annotations

import subprocess
import uuid
from pathlib import Path

from devex_clone_source_proof import require_closed_port
from full_stack_process import assert_identity, process_identity
from full_stack_process_monitor import completion_binding
from full_stack_process_tree import launch_supervised_process, read_process_tree
from process_sockets import verify_listener
from restore_build import repository, write_new
from restore_monitoring_evidence import descriptor, utc_now, validate_binding
from restore_monitoring_observation import verify_boundaries, verify_delivery, verify_metrics, verify_prometheus
from restore_monitoring_processes import (
    ROLES,
    commands,
    create_directories,
    paths,
    ready_endpoints,
    safe_environment,
    stop_processes,
    validate_configs,
    wait_ready,
    write_configs,
)
from restore_monitoring_publish import publish_terminal_success
from restore_monitoring_receipts import (
    BASE_NAMES,
    RUNTIME_DIRECTORIES,
    binding_descriptor,
    document_descriptor,
    expect_root,
    validate_close,
    validate_intent,
    validate_observation,
    validate_result,
    validate_root_shape,
    validate_runtime_layout,
    validate_start,
)
from restore_runtime_evidence import (
    read_json_document,
    timestamp,
)


def read_binding(backend: Path, path: Path) -> tuple[object, dict]:
    backend = repository(backend, "监控验收协调后端")
    if not path.is_absolute() or path.name != "binding.json":
        raise ValueError("监控绑定必须使用绝对 binding.json")
    document = read_json_document(path)
    if not document.path.parent.resolve(strict=True).is_relative_to((backend / ".local-tests").resolve(strict=True)):
        raise ValueError("监控绑定必须位于协调器忽略目录")
    binding = validate_binding(document.value)
    if Path(binding["staging"]["path"]).parent.parent != document.path.parent:
        raise ValueError("监控绑定与 lifecycle run 目录不同")
    validate_root_shape(document.path.parent)
    document.assert_unchanged()
    return document, binding


def _intent(binding_document, binding: dict, operations: dict[str, str]) -> dict:
    if set(operations) != set(ROLES) or len(set(operations.values())) != len(ROLES):
        raise ValueError("监控启动操作 ID 不完整或重复")
    return {
        "format_version": 1, "kind": "restore-monitoring-start-intent",
        "binding": binding_descriptor(binding_document), "run_id": binding["run_id"],
        "scope_id": binding["scope_id"], "operations": operations, "requested_at": utc_now(),
    }


def _read_intent(run_directory: Path, binding_document, binding: dict) -> tuple[object, dict]:
    document = read_json_document(run_directory / "start-intent.json")
    return document, validate_intent(document.value, binding, binding_descriptor(binding_document))


def _process_receipts(run_directory: Path, binding: dict, intent: dict) -> dict:
    directory = run_directory / "processes"
    if not directory.is_dir():
        return {}
    result = {}
    for role in ROLES:
        if not (directory / f"{role}-tree.json").exists():
            continue
        tree = read_process_tree(directory, role, binding["scope_id"])
        if tree["operation_id"] != intent["operations"][role]:
            raise ValueError("监控进程树不属于当前启动意图")
        result[role] = tree
    return result


def _failure(binding_document, binding: dict, stage: str, error: BaseException, completions: dict) -> dict:
    return {
        "format_version": 1, "kind": f"restore-monitoring-{stage}-failure", "status": "failed",
        "binding": binding_descriptor(binding_document), "run_id": binding["run_id"],
        "scope_id": binding["scope_id"], "error_type": type(error).__name__,
        "completions": completions, "failed_at": utc_now(), "external_contacts": 0,
    }


def _record_failure(path: Path, value: dict, backend: Path, error: BaseException) -> None:
    if path.exists():
        error.add_note("原失败证据已存在，未覆盖")
        return
    try:
        write_new(path, value, backend)
    except BaseException as write_error:
        error.add_note("失败证据 create-only 发布同时失败：" + type(write_error).__name__)


def start_runtime(
    backend: Path, binding_document, binding: dict, execution: dict, *, run=subprocess.run,
    launcher=launch_supervised_process, ready=wait_ready, listener=verify_listener,
    port_check=require_closed_port,
) -> dict:
    backend = repository(backend, "监控验收协调后端")
    run_directory = binding_document.path.parent
    expect_root(run_directory, BASE_NAMES)
    intent_path = run_directory / "start-intent.json"
    operations = {role: uuid.uuid4().hex for role in ROLES}
    write_new(intent_path, _intent(binding_document, binding, operations), backend)
    processes, stage = {}, "validate-intent"
    try:
        intent_document = read_json_document(intent_path)
        validate_intent(intent_document.value, binding, binding_descriptor(binding_document))
        intent_receipt = document_descriptor(intent_document, "监控启动意图")
        stage = "create-directories"
        runtime = create_directories(backend, run_directory)
        stage = "write-configs"
        configs = write_configs(backend, binding, execution, runtime)
        environment = safe_environment(run_directory)
        stage = "validate-configs"
        validations = validate_configs(execution, runtime, environment, run)
        launch_commands, endpoints = commands(binding, execution, runtime), ready_endpoints(binding)
        for role in ROLES:
            stage = "launch-" + role
            with (runtime["processes"] / f"{role}.log").open("xb") as output:
                process = launcher(
                    runtime["processes"], role, binding["scope_id"], launch_commands[role],
                    run_directory, environment, output, operation_id=operations[role],
                )
            processes[role] = process
            ready(endpoints[role])
            listener(process.pid, endpoints[role])
        if any(process.poll() is not None for process in processes.values()):
            raise RuntimeError("监控进程在启动收据发布前退出")
        process_receipts = {
            role: {"operation_id": operations[role],
                   "tree": descriptor(runtime["processes"] / f"{role}-tree.json"),
                   "identity": processes[role].tree["process"]}
            for role in ROLES
        }
        start = {
            "format_version": 1, "kind": "restore-monitoring-start", "status": "running",
            "binding": binding_descriptor(binding_document), "intent": intent_receipt,
            "run_id": binding["run_id"], "scope_id": binding["scope_id"], "configs": configs,
            "validations": validations, "processes": process_receipts, "endpoints": endpoints,
            "started_at": utc_now(),
        }
        validate_start(start, binding, binding_descriptor(binding_document), intent_receipt)
        validate_runtime_layout(run_directory, operations, observed=False)
        for process in processes.values():
            process.release_controller_handle()
        intent_document.assert_unchanged()
        write_new(run_directory / "start.json", start, backend)
        return start
    except BaseException as error:
        completions = {}
        try:
            completions = stop_processes(processes, crash=True)
            for endpoint in binding["endpoints"].values():
                port_check(endpoint)
        except BaseException as cleanup:
            error.add_note("启动失败后的完整进程树回收同时失败：" + type(cleanup).__name__)
        _record_failure(
            run_directory / "start-failure.json",
            _failure(binding_document, binding, "start-" + stage, error, completions), backend, error,
        )
        raise


def read_start(run_directory: Path, binding_document, binding: dict) -> tuple[object, dict, dict]:
    intent_document, intent = _read_intent(run_directory, binding_document, binding)
    intent_receipt = document_descriptor(intent_document, "监控启动意图")
    document = read_json_document(run_directory / "start.json")
    value = validate_start(document.value, binding, binding_descriptor(binding_document), intent_receipt)
    document_descriptor(document, "监控启动收据")
    intent_document.assert_unchanged()
    document.assert_unchanged()
    return document, value, intent


def _assert_running(run_directory: Path, binding: dict, start: dict, intent: dict, listener=verify_listener) -> None:
    trees = _process_receipts(run_directory, binding, intent)
    if set(trees) != set(ROLES):
        raise ValueError("监控运行缺少进程树")
    for role, item in start["processes"].items():
        tree = trees[role]
        if tree["process"] != item["identity"] or not assert_identity(
            process_identity(item["identity"]["pid"]), item["identity"]
        ):
            raise ValueError("监控进程身份已经退出或变化")
        listener(item["identity"]["pid"], start["endpoints"][role])


def observe_runtime(
    backend: Path, binding_document, binding: dict, execution: dict, secret: str, *,
    run=subprocess.run, fetch=None, listener=verify_listener, port_check=require_closed_port,
) -> dict:
    backend = repository(backend, "监控验收协调后端")
    run_directory = binding_document.path.parent
    expect_root(run_directory, BASE_NAMES | RUNTIME_DIRECTORIES | {"start-intent.json", "start.json"})
    start_document, start, intent = read_start(run_directory, binding_document, binding)
    try:
        validate_runtime_layout(run_directory, intent["operations"], observed=False)
        _assert_running(run_directory, binding, start, intent, listener)
        runtime, options = paths(run_directory), {} if fetch is None else {"fetch": fetch}
        metrics = verify_metrics(backend, binding, secret, runtime["evidence"], **options)
        prometheus = verify_prometheus(backend, binding, runtime["evidence"], **options)
        boundaries = verify_boundaries(backend, execution, runtime, safe_environment(run_directory), run=run)
        delivery = verify_delivery(binding, runtime["events"], **options)
        _assert_running(run_directory, binding, start, intent, listener)
        observation = {
            "format_version": 1, "kind": "restore-monitoring-observation", "status": "passed",
            "binding": binding_descriptor(binding_document),
            "start": document_descriptor(start_document, "监控启动收据"),
            "run_id": binding["run_id"], "scope_id": binding["scope_id"], "metrics": metrics,
            "prometheus": prometheus, "boundaries": boundaries, "delivery": delivery,
            "external_contacts": 0, "observed_at": utc_now(),
        }
        validate_observation(
            observation,
            binding,
            binding_descriptor(binding_document),
            document_descriptor(start_document, "监控启动收据"),
        )
        validate_runtime_layout(run_directory, intent["operations"], observed=True)
        write_new(run_directory / "observation.json", observation, backend)
        return observation
    except BaseException as error:
        completions = {}
        try:
            completions = stop_processes(
                _process_receipts(run_directory, binding, intent),
                crash=True,
            )
            for endpoint in binding["endpoints"].values():
                port_check(endpoint)
        except BaseException as cleanup:
            error.add_note("监控观测失败后的完整进程树回收同时失败：" + type(cleanup).__name__)
        _record_failure(
            run_directory / "observation-failure.json",
            _failure(binding_document, binding, "observation", error, completions), backend, error,
        )
        raise


def cleanup_interrupted_runtime(
    binding_document,
    binding: dict,
    error: BaseException,
    *,
    port_check=require_closed_port,
) -> None:
    """staged runner 自身失败时尽力回收已经登记的三棵进程树。"""
    run_directory = binding_document.path.parent
    try:
        if not (run_directory / "start-intent.json").exists():
            return
        _intent_document, intent = _read_intent(run_directory, binding_document, binding)
        stop_processes(_process_receipts(run_directory, binding, intent), crash=True)
        for endpoint in binding["endpoints"].values():
            port_check(endpoint)
    except BaseException as cleanup:
        error.add_note("staged runner 失败后的完整进程树回收同时失败：" + type(cleanup).__name__)


def _close_intent(binding_document, binding: dict, source: dict, requested_at: str | None = None) -> dict:
    return {
        "format_version": 1, "kind": "restore-monitoring-close-intent",
        "binding": binding_descriptor(binding_document), "source": source,
        "run_id": binding["run_id"], "scope_id": binding["scope_id"],
        "requested_at": utc_now() if requested_at is None else requested_at,
    }


def close_runtime(
    backend: Path, binding_document, binding: dict, *, terminate=None,
    completion=completion_binding, port_check=require_closed_port,
) -> dict:
    backend = repository(backend, "监控验收协调后端")
    run_directory = binding_document.path.parent
    observed_names = validate_root_shape(run_directory)
    if "result.json" in observed_names or "close.json" in observed_names:
        raise ValueError("监控运行已经关闭或形成最终结果")
    if "close-failure.json" in observed_names:
        raise ValueError("监控关闭已有失败证据，必须先核对现场，禁止重放")
    intent_document, intent = _read_intent(run_directory, binding_document, binding)
    source_name = next(
        (name for name in ("observation.json", "observation-failure.json", "start.json", "start-failure.json") if name in observed_names),
        "start-intent.json",
    )
    source_document = read_json_document(run_directory / source_name)
    source = document_descriptor(source_document, "监控关闭来源")
    close_intent_path = run_directory / "close-intent.json"
    if close_intent_path.exists():
        close_document = read_json_document(close_intent_path)
        expected = _close_intent(binding_document, binding, source, close_document.value.get("requested_at"))
        if close_document.value != expected:
            raise ValueError("监控关闭意图与当前运行不同")
        timestamp(expected["requested_at"], "监控关闭请求时间")
    else:
        write_new(close_intent_path, _close_intent(binding_document, binding, source), backend)
    completions = {}
    try:
        kwargs = {"crash": False, "completion": completion}
        if terminate is not None:
            kwargs["terminate"] = terminate
        trees = _process_receipts(run_directory, binding, intent)
        if source_name in {"start.json", "observation.json", "observation-failure.json"} and set(trees) != set(ROLES):
            raise ValueError("已启动的监控运行缺少完整进程树，拒绝形成关闭成功收据")
        completions = stop_processes(trees, **kwargs)
        for endpoint in binding["endpoints"].values():
            port_check(endpoint)
        close = {
            "format_version": 1, "kind": "restore-monitoring-close", "status": "closed",
            "binding": binding_descriptor(binding_document), "source": source,
            "close_intent": descriptor(close_intent_path), "run_id": binding["run_id"],
            "scope_id": binding["scope_id"], "completions": completions,
            "ports_closed": sorted(binding["endpoints"]), "closed_at": utc_now(),
        }
        close_intent_document = read_json_document(close_intent_path)
        validate_close(
            close,
            binding,
            binding_descriptor(binding_document),
            source,
            document_descriptor(close_intent_document, "监控关闭意图"),
            intent["operations"],
        )
        write_new(run_directory / "close.json", close, backend)
        source_document.assert_unchanged()
        close_intent_document.assert_unchanged()
        return close
    except BaseException as error:
        _record_failure(
            run_directory / "close-failure.json",
            _failure(binding_document, binding, "close", error, completions), backend, error,
        )
        raise


def prepare_result(binding_document, binding: dict, *, port_check=require_closed_port) -> dict:
    run_directory = binding_document.path.parent
    expected = BASE_NAMES | RUNTIME_DIRECTORIES | {
        "start-intent.json", "start.json", "observation.json", "close-intent.json", "close.json",
    }
    expect_root(run_directory, expected)
    start_document, _start, intent = read_start(run_directory, binding_document, binding)
    observation_document = read_json_document(run_directory / "observation.json")
    observation = validate_observation(
        observation_document.value,
        binding,
        binding_descriptor(binding_document),
        document_descriptor(start_document, "监控启动收据"),
    )
    close_intent_document = read_json_document(run_directory / "close-intent.json")
    close_document = read_json_document(run_directory / "close.json")
    close = validate_close(
        close_document.value, binding, binding_descriptor(binding_document),
        document_descriptor(observation_document, "监控观测收据"),
        document_descriptor(close_intent_document, "监控关闭意图"),
        intent["operations"],
    )
    if set(close["completions"]) != set(ROLES):
        raise ValueError("监控成功结果缺少三棵完整进程树关闭证明")
    validate_runtime_layout(run_directory, intent["operations"], observed=True)
    for tree in _process_receipts(run_directory, binding, intent).values():
        if assert_identity(process_identity(tree["process"]["pid"]), tree["process"]):
            raise ValueError("监控进程仍在运行，不能发布最终结果")
    for endpoint in binding["endpoints"].values():
        port_check(endpoint)
    result = {
        "format_version": 1, "kind": "restore-monitoring-result", "status": "passed",
        "binding": binding_descriptor(binding_document),
        "start": document_descriptor(start_document, "监控启动收据"),
        "observation": document_descriptor(observation_document, "监控观测收据"),
        "close": document_descriptor(close_document, "监控关闭收据"),
        "run_id": binding["run_id"], "scope_id": binding["scope_id"],
        "alerts": observation["prometheus"]["alerts"],
        "targets": sorted(observation["prometheus"]["targets"]),
        "firing_delivered": observation["delivery"]["firing"],
        "resolved_delivered": observation["delivery"]["resolved"],
        "boundary_cases": observation["boundaries"]["cases"],
        "ports_closed": close["ports_closed"], "external_contacts": 0, "completed_at": utc_now(),
    }
    for document in (start_document, observation_document, close_intent_document, close_document, binding_document):
        document.assert_unchanged()
    return result


def publish_result(backend: Path, binding_document, binding: dict, result: dict) -> dict:
    backend = repository(backend, "监控验收协调后端")
    expected = prepare_result(binding_document, binding)
    if ({key: value for key, value in expected.items() if key != "completed_at"}
            != {key: value for key, value in result.items() if key != "completed_at"}):
        raise ValueError("staged runner 返回的监控最终候选与当前证据不同")
    timestamp(result.get("completed_at"), "监控完成时间")
    publish_terminal_success(binding_document.path.parent, result)
    return result


def status(binding_document, binding: dict) -> dict:
    names = validate_root_shape(binding_document.path.parent)
    state, next_action, alive = "bound", "start", {}
    if "result.json" in names:
        result_document = read_json_document(binding_document.path.parent / "result.json")
        validate_result(result_document.value, binding, binding_descriptor(binding_document))
        result_document.assert_unchanged()
        state, next_action = "passed", None
    elif "close-failure.json" in names:
        state, next_action = "close-failed", None
    elif "close.json" in names:
        state = "closed"
        next_action = "result" if "observation.json" in names else None
    elif "close-intent.json" in names:
        state, next_action = "closing", "close"
    elif "observation-failure.json" in names:
        state, next_action = "observation-failed", "close"
    elif "observation.json" in names:
        state, next_action = "observed", "close"
    elif "start-failure.json" in names:
        state, next_action = "start-failed", "close"
    elif "start.json" in names:
        _document, start, _intent_value = read_start(binding_document.path.parent, binding_document, binding)
        alive = {
            role: assert_identity(process_identity(item["identity"]["pid"]), item["identity"])
            for role, item in start["processes"].items()
        }
        state, next_action = ("running", "observe") if all(alive.values()) else ("interrupted", "close")
    elif "start-intent.json" in names:
        state, next_action = "starting-interrupted", "close"
    binding_document.assert_unchanged()
    return {
        "status": state,
        "run_id": binding["run_id"],
        "scope_id": binding["scope_id"],
        "next_action": next_action,
        "processes_alive": alive,
    }
