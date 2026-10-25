"""fresh 目标的统一生命周期入口；登记只保存请求与私有环境的文件绑定。"""
from __future__ import annotations

import copy
from contextlib import contextmanager
from pathlib import Path

from artifact_digests import protect_binaries
from devex_clone_capture import read_bound_json, write_json
from devex_clone_factory_context import initialization_history
from process_environment import Environments, configured
from devex_clone_model import exact, linked, local_path
from devex_clone_run_state import binding, controller_observation, historical_state, load_state, run_lock
from devex_clone_source_proof import bound_file
import devex_clone_target_tree_claim as snapshot_evidence
from process_guard import process_guard
from devex_clone_target_evidence import evidence

WORKSPACE_GUARD = "fresh-target.guard"


def _bound_json(backend: Path, descriptor: dict) -> tuple[Path, dict]:
    path = bound_file(backend, descriptor)
    return path, read_bound_json(path, descriptor)


def _directory_identity(path: Path) -> tuple[int, int]:
    if linked(path) or not path.is_dir():
        raise ValueError("fresh 目标 workspace 缺失或经过链接")
    state = path.stat()
    return state.st_dev, state.st_ino


@contextmanager
def _workspace_control(workspace: Path):
    identity = _directory_identity(workspace)
    with process_guard(workspace, WORKSPACE_GUARD):
        if _directory_identity(workspace) != identity:
            raise ValueError("fresh 目标 workspace 在取得控制锁前被替换")
        try:
            yield
        finally:
            if _directory_identity(workspace) != identity:
                raise ValueError("fresh 目标 workspace 在阶段期间被替换")


def _private_environment(backend: Path, descriptor: dict) -> tuple[Path, dict]:
    from devex_clone_target_binding import _native

    result = _native("private-environment", backend, descriptor=descriptor)
    return Path(result["path"]), result["environment"]


def _fixture_service_run(manifest: dict, state: dict) -> bool:
    """识别只提供首代服务的参考夹具，不能把它伪装成复制重启。"""
    if manifest.get("kind") != "reference-fixture-service-run":
        return False
    from fixture_service_client import validate_initial, validate_manifest

    validate_manifest(manifest)
    validate_initial(state)
    return True


def _fixture_service_generation(directory: Path, descriptor: dict) -> dict | None:
    """返回执行入口与 status 共用的夹具服务代次可用性。"""
    manifest = read_bound_json(directory / "manifest.json", descriptor["manifest"])
    historical_state(directory, descriptor["state"])
    state = load_state(directory)
    if not _fixture_service_run(manifest, state):
        return None
    from fixture_service_client import validate_history

    history = validate_history(directory, state)
    if history["closed"]:
        return {"available": False, "active_generation": history["active_generation"],
                "reason": "夹具服务已正常关闭；须先执行 fixture services restart 再续作 fresh target"}
    if history["external_recovery"] is not None:
        return {"available": False, "active_generation": history["active_generation"],
                "reason": "夹具服务外部终止已核对但尚未重启"}
    return {"available": True, "active_generation": history["active_generation"], "reason": None}


def _storage_run(backend: Path, descriptor: dict) -> Path:
    exact(descriptor, {"path", "manifest", "state"})
    directory = local_path(backend, descriptor["path"])
    if not directory.is_dir():
        raise ValueError("fresh 目标 storage run 目录缺失")
    manifest_path, manifest = _bound_json(backend, descriptor["manifest"])
    if manifest_path != directory / "manifest.json":
        raise ValueError("fresh 目标 storage manifest 不属于固定统一目录")
    historical = historical_state(directory, descriptor["state"])
    if _fixture_service_run(manifest, historical["state"]):
        from fixture_service_client import validate_history

        validate_history(directory, historical["state"])
        if historical["historical_attempts"] < 3 or binding(directory / "state.json") != historical["current"]:
            raise ValueError("夹具服务登记未包含完整首代或核验期间账本变化")
        return directory
    appended = historical["state"]["attempts"][historical["historical_attempts"]:]
    stages = evidence("cli-storage-append", backend, directory, arguments={"attempts": appended})
    if stages["storage"]:
        from devex_clone_storage import registered_storage_binding

        if registered_storage_binding(backend, directory, "target") is None:
            raise ValueError("fresh 目标当前 RustFS 重启尚未完整发布")
    if stages["cache"]:
        from devex_clone_cache import registered_cache_binding

        if registered_cache_binding(backend, directory) is None:
            raise ValueError("fresh 目标当前 Redis 重启尚未完整发布")
    if binding(directory / "state.json") != historical["current"]:
        raise ValueError("fresh 目标 storage run 在历史绑定核对期间变化")
    return directory


def _disjoint(backend: Path, *paths: Path) -> None:
    evidence("cli-disjoint", backend, paths[0], arguments={"paths": [str(path) for path in paths]})


def _preflight_reconciliation(backend: Path, target: Path) -> dict:
    from devex_clone_target_binding import _native

    return _native("preflight-reconciliation", backend, target=str(target))["binding"]


def _prepared(backend: Path, workspace: Path, value: dict) -> dict:
    path = Path(value["target_directory"]) / "prepare.json"
    descriptor = binding(path)
    prepared = read_bound_json(path, descriptor)
    if prepared.get("request") != value["request"]:
        raise ValueError("fresh 目标 prepare 请求不属于固定 registration")
    proof = snapshot_evidence.snapshot(workspace, value, "prepared")
    if binding(path) != descriptor:
        raise ValueError("fresh 目标 prepare 证据在快照核对期间变化")
    bound_file(backend, value["request"])
    return proof


def _prepare_binding(backend: Path, value: dict) -> dict:
    path = Path(value["target_directory"]) / "prepare.json"
    descriptor = binding(path)
    if read_bound_json(path, descriptor).get("request") != value["request"]:
        raise ValueError("fresh 目标 prepare 请求不属于固定 registration")
    bound_file(backend, value["request"])
    return descriptor


def _initialized(backend: Path, workspace: Path, value: dict) -> tuple[dict, dict]:
    prepare_path = Path(value["target_directory"]) / "prepare.json"
    prepare_binding = _prepare_binding(backend, value)
    prepared = binding(workspace / "prepared-files.json")
    proof = snapshot_evidence.snapshot(workspace, value, "initialized")
    if binding(workspace / "prepared-files.json") != prepared or binding(prepare_path) != prepare_binding:
        raise ValueError("fresh 目标 prepare 文件快照发生变化")
    initialized, _ = initialization_history(backend, Path(value["target_directory"]) / "initialized.json")
    return proof, initialized


def _registration(backend: Path, workspace: Path) -> tuple[Path, dict, dict]:
    workspace = local_path(backend, str(workspace))
    observed = evidence("cli-registration", backend, workspace, arguments={})
    path, before, value = Path(observed["path"]), observed["binding"], observed["value"]
    _, private = _private_environment(backend, value["environment"])
    _storage_run(backend, value["storage_run"])
    if binding(path) != before:
        raise ValueError("fresh 目标登记在读取期间变化")
    return path, copy.deepcopy(value), copy.deepcopy(private)


def _validate_registered_request(backend: Path, descriptor: dict) -> None:
    """确认固定请求当前仍满足执行前置条件，不把仅有文件绑定当成可续作。"""
    from devex_clone_target_binding import request_binding

    _, request = _bound_json(backend, descriptor)
    request_binding(backend, request)


def _unchanged(backend: Path, path: Path, value: dict, private: dict,
               storage_state: dict | None = None) -> None:
    before = binding(path)
    _, current_private = _private_environment(backend, value["environment"])
    if (read_bound_json(path, before) != value or binding(path) != before
            or current_private != private):
        raise ValueError("fresh 目标登记或私有环境在阶段期间变化")
    bound_file(backend, value["request"])
    storage = _storage_run(backend, value["storage_run"])
    if storage_state is not None and binding(storage / "state.json") != storage_state:
        raise ValueError("fresh 目标 storage run 在只读阶段期间变化")


def _run_registered(backend: Path, workspace: Path, operation, *args) -> dict:
    path, value, private = _registration(backend, workspace)
    registered = binding(path)
    storage_run = _storage_run(backend, value["storage_run"])
    storage_binding = copy.deepcopy(value["storage_run"])
    storage_state = binding(storage_run / "state.json")
    fixture_services = _fixture_service_generation(storage_run, storage_binding)
    active_storage_run = storage_run
    if fixture_services is not None:
        if not fixture_services["available"]:
            raise ValueError(fixture_services["reason"])
        if fixture_services["active_generation"].get("kind") == "initial":
            active_storage_run = None
    from devex_clone_target_binding import execution_binary_bindings

    _, request = _bound_json(backend, value["request"])
    binaries = execution_binary_bindings(backend, request)
    environment = configured(private)
    with protect_binaries(binaries), run_lock(storage_run):
        if (_registration(backend, workspace)[1] != value
                or _storage_run(backend, storage_binding) != storage_run
                or binding(storage_run / "state.json") != storage_state):
            raise ValueError("fresh 目标登记或 storage run 在取得控制锁前变化")
        try:
            with Environments(environment, environment).use("target"):
                result = operation(backend, Path(value["target_directory"]), *args,
                                   storage_run=active_storage_run)
        finally:
            _, current_private = _private_environment(backend, value["environment"])
            if (binding(path) != registered or read_bound_json(path, registered) != value
                    or current_private != private):
                raise ValueError("fresh 目标登记或私有环境在阶段期间变化")
            bound_file(backend, value["request"])
            if (value["storage_run"] != storage_binding
                    or _storage_run(backend, storage_binding) != storage_run
                    or binding(storage_run / "state.json") != storage_state):
                raise ValueError("fresh 目标 storage run 在阶段期间变化")
    return result


def prepare(backend: Path, workspace: Path, request_file: Path, environment_file: Path, storage_run: Path) -> dict:
    from devex_clone_target import prepare_target

    backend = backend.resolve(strict=True)
    workspace = local_path(backend, str(workspace), new=True)
    if not workspace.parent.is_dir():
        raise ValueError("fresh 目标 workspace 必须位于已有父目录")
    request = local_path(backend, str(request_file))
    environment = local_path(backend, str(environment_file))
    storage = local_path(backend, str(storage_run))
    _disjoint(backend, workspace, storage)
    request_binding, environment_binding = binding(request), binding(environment)
    bound_file(backend, request_binding)
    _, private = _private_environment(backend, environment_binding)
    storage_binding = {"path": str(storage), "manifest": binding(storage / "manifest.json"),
                       "state": binding(storage / "state.json")}
    _storage_run(backend, storage_binding)
    workspace.mkdir()
    with _workspace_control(workspace):
        bound_file(backend, request_binding)
        _, current_private = _private_environment(backend, environment_binding)
        if current_private != private:
            raise ValueError("fresh 目标私有环境在 workspace 取得控制锁前变化")
        _storage_run(backend, storage_binding)
        registration = {"format_version": 1, "kind": "devex-clone-fresh-target-registration",
                        "request": request_binding, "environment": environment_binding,
                        "storage_run": storage_binding,
                        "target_directory": str(workspace / "target")}
        write_json(workspace / "registration.json", registration)
        def invoke(root, target, *, storage_run):
            return prepare_target(root, request, target, storage_run=storage_run,
                                  request_descriptor=request_binding)
        result = _run_registered(backend, workspace, invoke)
        snapshot = snapshot_evidence.write_snapshot(
            backend, workspace, registration, "prepared", expected_document=result)
        return {"status": result["status"], "registration": binding(workspace / "registration.json"),
                "target": binding(workspace / "target/prepare.json"), "files": snapshot,
                "restore_qualified": False}


def resume_prepare(backend: Path, workspace: Path) -> dict:
    from devex_clone_target import prepare_resume_state, resume_prepare_target

    backend = backend.resolve(strict=True)
    workspace = local_path(backend, str(workspace))
    path, registered, private = _registration(backend, workspace)
    if (workspace / "prepared-files.json").exists():
        _prepared(backend, workspace, registered)
        raise ValueError("fresh 目标 prepare 已完整发布，不能重复续作")
    resumable = prepare_resume_state(backend, Path(registered["target_directory"]), registered["request"])
    if not resumable["resumable"]:
        raise ValueError("fresh 目标 prepare 不能续作：" + resumable["reason"])
    storage = _storage_run(backend, registered["storage_run"])
    if controller_observation(storage) is not None:
        raise ValueError("fresh 目标控制锁尚未收尾；先核对 status，再使用原 recover 入口")
    with _workspace_control(workspace):
        _unchanged(backend, path, registered, private)
        _, value, _ = _registration(backend, workspace)
        if (workspace / "prepared-files.json").exists():
            _prepared(backend, workspace, value)
            raise ValueError("fresh 目标 prepare 已完整发布，不能重复续作")

        def invoke(root, target, *, storage_run):
            return resume_prepare_target(root, target, storage_run=storage_run,
                                         request_descriptor=value["request"])

        result = _run_registered(backend, workspace, invoke)
        snapshot = snapshot_evidence.write_snapshot(
            backend, workspace, value, "prepared", expected_document=result)
        return {"status": result["status"], "registration": binding(workspace / "registration.json"),
                "target": binding(workspace / "target/prepare.json"), "files": snapshot,
                "resumed": True, "restore_qualified": False}


def initialize(backend: Path, workspace: Path) -> dict:
    from devex_clone_target import initialize_target

    backend = backend.resolve(strict=True)
    workspace = local_path(backend, str(workspace))
    with _workspace_control(workspace):
        _, value, _ = _registration(backend, workspace)
        prepared = _prepared(backend, workspace, value)
        published = []

        def invoke(root, target, *, storage_run):
            return initialize_target(
                root, target, storage_run=storage_run,
                publish_files=lambda files: published.append(
                    snapshot_evidence.write_snapshot(
                        backend, workspace, value, "initialized", files, locked_guard=True))
            )

        result = _run_registered(backend, workspace, invoke)
        if binding(workspace / "prepared-files.json") != prepared:
            raise ValueError("fresh 目标 initialize 期间 prepare 文件快照变化")
        if (len(published) != 1
                or snapshot_evidence.snapshot(workspace, value, "initialized") != published[0]):
            raise ValueError("fresh 目标 initialize 未在目标控制锁内发布唯一文件快照")
        return {"status": result["status"], "registration": binding(workspace / "registration.json"),
                "target": binding(workspace / "target/initialized.json"), "files": published[0],
                "restore_qualified": False}


def resume_initialize(backend: Path, workspace: Path) -> dict:
    from devex_clone_target_resume import initialize_resume_state, resume_initialize_target

    backend, workspace = backend.resolve(strict=True), local_path(backend, str(workspace))
    with _workspace_control(workspace):
        _, value, _ = _registration(backend, workspace)
        _prepare_binding(backend, value)
        prepared_files = snapshot_evidence.resume_prepared_files(workspace, value)
        state = initialize_resume_state(backend, Path(value["target_directory"]), value["request"],
                                        prepared_files)
        if not state["resumable"]:
            raise ValueError("fresh 目标 initialize 不能续作：" + state["reason"])
        published = []

        def invoke(root, target, *, storage_run):
            return resume_initialize_target(root, target, storage_run=storage_run,
                                            request_descriptor=value["request"],
                                            prepared_files=prepared_files,
                                            publish_files=lambda files: published.append(
                                                snapshot_evidence.write_snapshot(
                                                    backend, workspace, value, "initialized", files,
                                                    locked_guard=True)))

        result = _run_registered(backend, workspace, invoke)
        if (len(published) != 1
                or snapshot_evidence.snapshot(workspace, value, "initialized") != published[0]):
            raise ValueError("fresh 目标续作未在目标控制锁内发布唯一文件快照")
        return {"status": result["status"], "registration": binding(workspace / "registration.json"),
                "target": binding(workspace / "target/initialized.json"), "files": published[0],
                "resumed": True, "restore_qualified": False}


def reconcile_preflight(backend: Path, workspace: Path) -> dict:
    from devex_clone_target import reconcile_preflight_failure

    backend, workspace = backend.resolve(strict=True), local_path(backend, str(workspace))
    with _workspace_control(workspace):
        _, value, _ = _registration(backend, workspace)
        result = _run_registered(backend, workspace, reconcile_preflight_failure)
        return {"status": result["status"], "registration": binding(workspace / "registration.json"),
                "reconciliation": binding(workspace / "target/reconciliation-completed.json"),
                "restore_qualified": False}


def verify(backend: Path, workspace: Path, observation_dir: Path) -> dict:
    from devex_clone_target import verify_target

    backend = backend.resolve(strict=True)
    workspace = local_path(backend, str(workspace))
    observation = local_path(backend, str(observation_dir), new=True)
    with _workspace_control(workspace):
        _, value, _ = _registration(backend, workspace)
        storage = _storage_run(backend, value["storage_run"])
        for existing in (workspace, Path(value["target_directory"]), storage):
            _disjoint(backend, observation, existing)
        initialized, _ = _initialized(backend, workspace, value)
        result = _run_registered(backend, workspace, verify_target, observation)
        if snapshot_evidence.snapshot(workspace, value, "initialized") != initialized:
            raise ValueError("fresh 目标 verify 期间初始化文件快照变化")
        return {"status": result["status"], "registration": binding(workspace / "registration.json"),
                "observation": binding(observation / "verify.json"), "restore_qualified": False}


def validate_dispatch(backend: Path, workspace: Path, args) -> None:
    evidence("cli-dispatch", backend, workspace, arguments={"operation": args.operation,
             "write": args.write, "supplied": [args.request is not None, args.environment is not None,
                                                args.storage_run is not None, args.observation_dir is not None]})
