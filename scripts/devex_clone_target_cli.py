"""fresh 目标的统一生命周期入口；登记只保存请求与私有环境的文件绑定。"""
from __future__ import annotations

import copy
from contextlib import contextmanager
from pathlib import Path

from artifact_digests import protect_binaries
from devex_clone import read_json
from devex_clone_capture import read_bound_json, write_json
from devex_clone_factory_context import initialization_history
from process_environment import Environments, configured
from devex_clone_model import exact, linked, local_path
from devex_clone_run_state import binding, controller_observation, historical_state, run_lock
from devex_clone_source_proof import bound_file
import devex_clone_target_cli_fixture as fixture_context
import devex_clone_target_cli_snapshot as snapshot_evidence
from process_guard import process_guard

FIELDS = {"format_version", "kind", "request", "environment", "storage_run", "target_directory"}
PRIVATE_NAMES = {"TEMP", "TMP"}
PRIVATE_PREFIXES = ("APP_", "RYFRAME_")
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
    expected = copy.deepcopy(descriptor)
    path, document = _bound_json(backend, descriptor)
    exact(document, {"environment"})
    private = document["environment"]
    if (not isinstance(private, dict)
            or any(not isinstance(key, str) or not isinstance(value, str) or not value
                   for key, value in private.items())
            or any(key.upper() not in PRIVATE_NAMES and not key.upper().startswith(PRIVATE_PREFIXES)
                   for key in private)):
        raise ValueError("fresh 目标私有环境仅允许 APP_、RYFRAME_、TEMP 和 TMP 文本字段")
    # configured 同时拒绝 Windows 上会冲突的大小写重复键。
    configured(private, {})
    folded = {key.upper(): value for key, value in private.items()}
    if (not PRIVATE_NAMES.issubset(folded) or folded["TEMP"] != folded["TMP"]
            or not Path(folded["TEMP"]).is_absolute()):
        raise ValueError("fresh 目标 TEMP 与 TMP 必须绑定同一绝对私有目录")
    temporary = local_path(backend, folded["TEMP"])
    if not temporary.is_dir():
        raise ValueError("fresh 目标 TEMP 与 TMP 必须是已存在的普通私有目录")
    if descriptor != expected or binding(path) != expected:
        raise ValueError("fresh 目标私有环境在读取期间变化")
    return path, private


def _storage_run(backend: Path, descriptor: dict) -> Path:
    exact(descriptor, {"path", "manifest", "state"})
    directory = local_path(backend, descriptor["path"])
    if not directory.is_dir():
        raise ValueError("fresh 目标 storage run 目录缺失")
    manifest_path, manifest = _bound_json(backend, descriptor["manifest"])
    if manifest_path != directory / "manifest.json":
        raise ValueError("fresh 目标 storage manifest 不属于固定统一目录")
    historical = historical_state(directory, descriptor["state"])
    if fixture_context.fixture_service_run(manifest, historical["state"]):
        from reference_fixture_service_history import validate_history

        validate_history(directory, historical["state"])
        if historical["historical_attempts"] < 3 or binding(directory / "state.json") != historical["current"]:
            raise ValueError("夹具服务登记未包含完整首代或核验期间账本变化")
        return directory
    appended = historical["state"]["attempts"][historical["historical_attempts"]:]
    allowed = {"storage-target": {"restart", "stop", "recover"},
               "cache-target": {"restart", "stop", "recover", "reconcile", "resume"}}
    if any(item["stage"] not in allowed or item["mode"] not in allowed[item["stage"]]
           or item["status"] == "running" for item in appended):
        raise ValueError("fresh 目标登记后仅允许已收尾的受控目标存储阶段")
    if any(item["stage"] == "storage-target" for item in appended):
        from devex_clone_storage import registered_storage_binding

        if registered_storage_binding(backend, directory, "target") is None:
            raise ValueError("fresh 目标当前 RustFS 重启尚未完整发布")
    if any(item["stage"] == "cache-target" for item in appended):
        from devex_clone_cache import registered_cache_binding

        if registered_cache_binding(backend, directory) is None:
            raise ValueError("fresh 目标当前 Redis 重启尚未完整发布")
    if binding(directory / "state.json") != historical["current"]:
        raise ValueError("fresh 目标 storage run 在历史绑定核对期间变化")
    return directory


def _disjoint(*paths: Path) -> None:
    for index, path in enumerate(paths):
        for other in paths[index + 1:]:
            if path == other or path.is_relative_to(other) or other.is_relative_to(path):
                raise ValueError("fresh 目标 workspace、storage run 与观察目录必须互不包含")


def _preflight_reconciliation(target: Path) -> dict:
    """验证只允许清空 preflight 失败资源的收尾收据，且不把它当作可重放初始化。"""
    completed = target / "reconciliation-completed.json"
    reports = sorted((target / "reset-state").glob("*.report.json"))
    value = read_json(completed)
    exact(value, {"status", "before", "after", "failure", "reset_report", "automatic_retry", "restore_qualified"})
    if (value["status"] != "preflight_failure_reconciled" or value["automatic_retry"] is not False
            or value["restore_qualified"] is not False or len(reports) != 1
            or value["failure"] != snapshot_evidence.digest_binding(target / "failure.json")
            or value["reset_report"] != snapshot_evidence.digest_binding(reports[0])):
        raise ValueError("fresh 目标预检失败收尾收据无效")
    for name, exists in (("before", True), ("after", False)):
        state = value[name]
        if (set(state) != {"databases", "objects", "redis"} or not isinstance(state["databases"], list)
                or not state["databases"] or any(item.get("exists") is not exists for item in state["databases"])):
            raise ValueError("fresh 目标预检失败收尾资源前后像无效")
    return binding(completed)


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
    path = workspace / "registration.json"
    before = binding(path)
    value = read_bound_json(path, before)
    exact(value, FIELDS)
    if value["format_version"] != 1 or value["kind"] != "devex-clone-fresh-target-registration":
        raise ValueError("fresh 目标登记类型无效")
    target = local_path(backend, value["target_directory"])
    if target != workspace / "target":
        raise ValueError("fresh 目标目录必须固定在登记 workspace")
    bound_file(backend, value["request"])
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
    fixture_services = fixture_context.fixture_service_generation(storage_run, storage_binding)
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
    _disjoint(workspace, storage)
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
            _disjoint(observation, existing)
        initialized, _ = _initialized(backend, workspace, value)
        result = _run_registered(backend, workspace, verify_target, observation)
        if snapshot_evidence.snapshot(workspace, value, "initialized") != initialized:
            raise ValueError("fresh 目标 verify 期间初始化文件快照变化")
        return {"status": result["status"], "registration": binding(workspace / "registration.json"),
                "observation": binding(observation / "verify.json"), "restore_qualified": False}


def status(backend: Path, workspace: Path) -> dict:
    def report(current: str, last: str | None, pending: str, next_action: str | None,
               *, valid: bool, reason: str | None, registration: dict | None = None) -> dict:
        result = {"status": current, "last_successful_stage": last,
                  "pending_stage": pending, "next_action": next_action,
                  "evidence_valid": valid, "blocking_reason": reason,
                  "restore_qualified": False}
        if registration is not None:
            result["registration"] = registration
        return result

    backend = backend.resolve(strict=True)
    workspace = local_path(backend, str(workspace))
    if not workspace.exists():
        return report("fresh_target_unregistered", None, "registration", "prepare",
                      valid=True, reason=None)
    if linked(workspace) or not workspace.is_dir() or not (workspace / "registration.json").is_file():
        return report("fresh_target_needs_reconciliation", None, "reconciliation", None,
                      valid=False, reason="workspace 或 registration 不完整")
    try:
        path, value, private = _registration(backend, workspace)
    except (OSError, TypeError, ValueError):
        return report("fresh_target_needs_reconciliation", None, "reconciliation", None,
                      valid=False, reason="registration、私有环境或受控存储绑定无效")
    try:
        _validate_registered_request(backend, value["request"])
    except (OSError, TypeError, ValueError):
        _unchanged(backend, path, value, private)
        return report("fresh_target_needs_reconciliation", "registered", "reconciliation", None,
                      valid=False, reason="固定请求未达到可执行条件", registration=binding(path))
    registration = binding(path)
    storage = _storage_run(backend, value["storage_run"])
    storage_state = binding(storage / "state.json")
    try:
        fixture_services = fixture_context.fixture_service_generation(storage, value["storage_run"])
    except (OSError, TypeError, ValueError):
        _unchanged(backend, path, value, private, storage_state)
        return report("fresh_target_needs_reconciliation", "registered", "reconciliation", None,
                      valid=False, reason="夹具服务账本或当前代次无效", registration=registration)
    target = Path(value["target_directory"])
    target_existed = target.exists()
    try:
        target_files = snapshot_evidence.target_files(target) if target_existed else None
    except (OSError, ValueError):
        _unchanged(backend, path, value, private, storage_state)
        return report("fresh_target_needs_reconciliation", "registered", "reconciliation", None,
                      valid=False, reason="fresh 目标目录结构无效", registration=registration)
    from devex_clone_target import prepare_resume_state, unresolved_failure
    from devex_clone_target_resume import initialize_resume_state

    try:
        if (target / "reconciliation-completed.json").is_file():
            reconciliation = _preflight_reconciliation(target)
            result = report("fresh_target_preflight_reconciled", "preflight_reconciled", "new_registration",
                            "prepare-new-workspace", valid=True,
                            reason="原 workspace 已记录收尾，必须使用新的私有环境和工作区重新登记",
                            registration=registration)
            result["reconciliation"] = reconciliation
        elif (target / "initialized.json").is_file():
            if unresolved_failure(backend, target):
                result = report("fresh_target_needs_reconciliation", "prepared", "reconciliation", None,
                                valid=False, reason="初始化存在未解决失败", registration=registration)
            else:
                _initialized(backend, workspace, value)
                result = report("fresh_target_initialized", "initialized", "verification", "verify",
                                valid=True, reason=None, registration=registration)
        elif ((target / "initialize.lock").exists() or (target / "initialize.started.json").exists()):
            resume = initialize_resume_state(backend, target, value["request"],
                                             snapshot_evidence.resume_prepared_files(workspace, value))
            if resume["resumable"]:
                if resume["mode"] == "migration":
                    completed = resume["completed"]
                    operation = resume["operations"][resume["next_index"]]
                    result = report("fresh_target_migration_resume_pending",
                                    "migration:" + completed[-1]["id"], "migration_resume",
                                    "resume-initialize", valid=True, reason=None,
                                    registration=registration)
                    result["resume"] = {"mode": "migration",
                                        "completed_operations": [item["id"] for item in completed],
                                        "next_operation": operation["id"],
                                        "next_operation_is_read_only": not operation["write"]}
                else:
                    result = report("fresh_target_inventory_resume_pending", "reset", "inventory_resume",
                                    "resume-initialize", valid=True, reason=None,
                                    registration=registration)
            else:
                result = report("fresh_target_needs_reconciliation", "prepared" if
                                (workspace / "prepared-files.json").exists() else "registered",
                                "reconciliation", None, valid=False,
                                reason="初始化已有 intent、锁或未解决失败", registration=registration)
        elif (target / "prepare.json").is_file():
            if (workspace / "prepared-files.json").is_file():
                if unresolved_failure(backend, target):
                    raise ValueError("prepare 失败尚未由显式续作确认")
                _prepared(backend, workspace, value)
                result = report("fresh_creation_prepared", "prepared", "initialization", "initialize",
                                valid=True, reason=None, registration=registration)
            else:
                resumable = prepare_resume_state(backend, target, value["request"])
                if not resumable["resumable"]:
                    raise ValueError(resumable["reason"])
                result = report("fresh_target_prepare_publication_pending", "prepared",
                                "prepared_snapshot", "resume-prepare", valid=True, reason=None,
                                registration=registration)
        else:
            resumable = prepare_resume_state(backend, target, value["request"])
            if resumable["resumable"]:
                result = report("fresh_target_prepare_interrupted", "registered", "prepare",
                                "resume-prepare", valid=True, reason=None, registration=registration)
            else:
                result = report("fresh_target_needs_reconciliation", "registered", "reconciliation", None,
                                valid=False, reason=resumable["reason"], registration=registration)
    except (OSError, TypeError, ValueError):
        result = report("fresh_target_needs_reconciliation", "registered", "reconciliation", None,
                        valid=False, reason="fresh 目标本地阶段证据无效", registration=registration)
    if (result["next_action"] == "resume-initialize" and fixture_services is not None
            and not fixture_services["available"]):
        result.update(evidence_valid=False, pending_stage="service_restart", next_action=None,
                      blocking_reason=fixture_services["reason"])
    try:
        controller = controller_observation(storage)
        if controller is not None:
            result["controller"] = {"run_directory": str(storage), **controller}
            if result["evidence_valid"]:
                result["pending_stage"] = "controller"
                result["next_action"] = "recover" if controller["process_missing"] else None
                result["blocking_reason"] = ("原控制器已退出，须通过原 recover 入口核对并回收锁"
                    if controller["process_missing"] else "原控制器仍在运行" if controller["process_matches"]
                    else "控制器 PID 已复用或进程身份不同，不能回收锁")
    except (OSError, TypeError, ValueError):
        result.update(evidence_valid=False, pending_stage="reconciliation", next_action=None,
                      blocking_reason="控制器收据或进程身份无法核实")
    if (target.exists() != target_existed
            or target_existed and snapshot_evidence.target_files(target) != target_files):
        raise ValueError("fresh 目标文件在 status 只读观察期间变化")
    _unchanged(backend, path, value, private, storage_state)
    return result
