"""固定清单驱动导出、目标复核、数据复制及环境控制，逐阶段显式继续。"""
from __future__ import annotations

import copy
from contextlib import contextmanager, nullcontext
import os
from pathlib import Path

from artifact_digests import protect_binaries
from devex_clone_capture import read_json, write_json
from devex_clone_factory_context import initialization_history
from process_environment import Environments
from devex_clone_model import exact, linked, local_path, name
from devex_clone_run_state import (begin, bind_controller_attempt, binding, claim_run_lock, controller_observation, finish,
                                   historical_state, initialize_state, load_state,
                                   record_failure, run_lock)
from devex_clone_source_proof import bound_file
from process_guard import process_guard

FIELDS = {"format_version", "kind", "id", "source_request", "source_export", "initialized",
          "source_environment", "target_environment", "copy_directory", "copy_stage", "build_bridges"}
SEED_TO_ARM_FIELDS = FIELDS | {
    "source_registration", "target_registration", "target_initialized_files",
    "target_storage_run",
}
SUCCESSOR_SEED_TO_ARM_FIELDS = SEED_TO_ARM_FIELDS | {"review_successor", "source_export_result", "source_generation"}
TARGET_REGISTRATION_FIELDS = {
    "format_version", "kind", "request", "environment", "storage_run", "target_directory",
}
HISTORICAL_STORAGE_RUN_FIELDS = {"path", "manifest", "state"}
TARGET_STORAGE_RUN_FIELDS = {"path", "manifest", "storage", "cache"}


def manifest(backend: Path, value: dict) -> dict:
    if value.get("copy_stage") == "seed_to_arm":
        fields = (SUCCESSOR_SEED_TO_ARM_FIELDS if "review_successor" in value
                  else SEED_TO_ARM_FIELDS)
    else:
        fields = FIELDS
    if value.get("copy_stage") == "seed_to_arm" and "source_rebind" in value:
        fields = fields | {"source_rebind"}
    exact(value, fields)
    if value["format_version"] != 1 or value["kind"] != "devex-clone-run":
        raise ValueError("统一验收清单类型无效")
    name(value["id"])
    if value["copy_stage"] not in {"source_to_seed", "seed_to_arm"}:
        raise ValueError("复制阶段必须明确为来源到 seed 或 seed 到测量侧")
    if value["copy_stage"] == "seed_to_arm":
        bound_file(backend, value["source_registration"])
        if "source_rebind" in value:
            bound_file(backend, value["source_rebind"])
        if "review_successor" in value:
            bound_file(backend, value["review_successor"])
            _published_seed_source(backend, value, live_storage=False)
        target_lifecycle_binding(backend, value, live_storage=False)
    for key in ("source_request", "initialized", "source_environment", "target_environment"):
        bound_file(backend, value[key])
    if value["source_export"] is not None:
        exported = read_json(bound_file(backend, value["source_export"]))
        if exported.get("request") != value["source_request"]:
            raise ValueError("导出证据不属于固定源请求")
    local_path(backend, value["copy_directory"])
    if not isinstance(value["build_bridges"], list):
        raise ValueError("构建审计证据必须是明确列表")
    for bridge in value["build_bridges"]:
        exact(bridge, {"path", "sha256"})
        path = local_path(backend, bridge["path"])
        if binding(path)["sha256"] != bridge["sha256"]:
            raise ValueError("构建审计证据发生变化")
    return copy.deepcopy(value)


def initialize(backend: Path, filename: Path, directory: Path) -> dict:
    filename = local_path(backend, str(filename))
    directory = local_path(backend, str(directory), new=True)
    original = binding(filename)
    value = manifest(backend, read_json(filename))
    storage = target_storage_run(backend, value, live=False)
    if storage is not None and _overlap(directory, storage):
        raise ValueError("seed_to_arm 运行目录必须与外部目标 storage run 拓扑分离")
    if binding(filename) != original or not directory.parent.is_dir():
        raise ValueError("验收清单读取期间变化或父目录不存在")
    directory.mkdir()
    write_json(directory / "manifest.json", value)
    initialize_state(directory)
    return {"status": "run_registered", "id": value["id"], "directory": str(directory),
            "remote_writes": 0, "stages_passed": [], "restore_qualified": False}


def read_manifest(backend: Path, directory: Path) -> dict:
    local_path(backend, str(directory))
    load_state(directory)
    value = manifest(backend, read_json(directory / "manifest.json"))
    storage = target_storage_run(backend, value, live=False)
    if storage is not None and _overlap(directory, storage):
        raise ValueError("seed_to_arm 运行目录必须与外部目标 storage run 拓扑分离")
    return value


def registered_manifest(backend: Path, directory: Path) -> dict:
    local_path(backend, str(directory))
    load_state(directory, verify_results=False)
    value = read_json(directory / "manifest.json")
    if value.get("copy_stage") == "seed_to_arm":
        fields = (SUCCESSOR_SEED_TO_ARM_FIELDS if "review_successor" in value
                  else SEED_TO_ARM_FIELDS)
    else:
        fields = FIELDS
    if value.get("copy_stage") == "seed_to_arm" and "source_rebind" in value:
        fields = fields | {"source_rebind"}
    exact(value, fields)
    if value["format_version"] != 1 or value["kind"] != "devex-clone-run":
        raise ValueError("固定验收清单类型变化")
    return value


def _overlap(first: Path, second: Path) -> bool:
    return first == second or first.is_relative_to(second) or second.is_relative_to(first)


def _capture_target_storage_run(backend: Path, historical: dict, *, live: bool) -> dict:
    exact(historical, HISTORICAL_STORAGE_RUN_FIELDS)
    directory = local_path(backend, historical["path"])
    if linked(directory) or not directory.is_dir():
        raise ValueError("seed_to_arm 外部目标 storage run 缺失或经过链接")
    if bound_file(backend, historical["manifest"]) != directory / "manifest.json":
        raise ValueError("fresh registration 的 storage manifest 不属于固定 run")
    historical_state(directory, historical["state"])
    from devex_clone_cache import registered_cache_binding
    from devex_clone_storage import current_storage_binding, registered_storage_binding

    storage = (current_storage_binding(backend, directory, "target") if live
               else registered_storage_binding(backend, directory, "target"))
    cache = registered_cache_binding(backend, directory)
    return {"path": str(directory), "manifest": copy.deepcopy(historical["manifest"]),
            "storage": storage, "cache": cache}


def _target_snapshot(backend: Path, registration_descriptor: dict,
                     initialized_descriptor: dict, files_descriptor: dict) -> tuple[dict, dict, dict]:
    registration_path = bound_file(backend, registration_descriptor)
    if registration_path.name != "registration.json":
        raise ValueError("arm 目标 registration 文件名无效")
    workspace = registration_path.parent
    registration = read_json(registration_path)
    exact(registration, TARGET_REGISTRATION_FIELDS)
    if (registration["format_version"] != 1
            or registration["kind"] != "devex-clone-fresh-target-registration"):
        raise ValueError("arm 目标必须来自 fresh-target registration")
    target = local_path(backend, registration["target_directory"])
    if target != workspace / "target" or linked(workspace) or not workspace.is_dir():
        raise ValueError("arm 目标目录不属于 fresh-target workspace")
    initialized_path = bound_file(backend, initialized_descriptor)
    if initialized_path != target / "initialized.json":
        raise ValueError("arm 目标 initialized.json 不属于 fresh-target registration")
    files_path = bound_file(backend, files_descriptor)
    if files_path != workspace / "initialized-files.json":
        raise ValueError("arm 目标文件快照不属于 fresh-target workspace")
    snapshot = read_json(files_path)
    exact(snapshot, {"format_version", "kind", "registration", "predecessor", "files"})
    from devex_clone_target_cli import _target_files

    if (snapshot["format_version"] != 1
            or snapshot["kind"] != "devex-clone-fresh-target-initialized-files"
            or snapshot["registration"] != registration_descriptor
            or snapshot["predecessor"] != binding(workspace / "prepared-files.json")
            or snapshot["files"] != _target_files(target)):
        raise ValueError("arm 目标 initialized-files 未绑定当前完整初始化目录")
    initialized, request = initialization_history(backend, initialized_path)
    prepared = read_json(target / "prepare.json")
    if prepared.get("request") != registration["request"]:
        raise ValueError("arm 目标初始化请求不属于 fresh-target registration")
    return registration, initialized, request


def target_lifecycle_binding(backend: Path, value: dict, *, live_storage: bool) -> dict:
    """把 fresh registration、初始化收据、目录快照和当前存储代次闭合为同一目标。"""
    registration, initialized, request = _target_snapshot(
        backend, value["target_registration"], value["initialized"],
        value["target_initialized_files"],
    )
    if registration["environment"] != value["target_environment"]:
        raise ValueError("arm 目标环境不属于 fresh-target registration")
    storage = _capture_target_storage_run(backend, registration["storage_run"], live=live_storage)
    expected = value.get("target_storage_run")
    if expected is not None:
        exact(expected, TARGET_STORAGE_RUN_FIELDS)
        if expected != storage:
            raise ValueError("seed_to_arm target_storage_run 不属于 fresh 初始化使用的固定代次")
    return {"registration": registration, "initialized": initialized, "target": request,
            "target_storage_run": storage}


def target_storage_run(backend: Path, value: dict, *, live: bool = True) -> Path | None:
    """解析 seed_to_arm 外部目标存储；source_to_seed 继续使用本 run。"""
    if value["copy_stage"] != "seed_to_arm":
        return None
    descriptor = value["target_storage_run"]
    exact(descriptor, TARGET_STORAGE_RUN_FIELDS)
    directory = local_path(backend, descriptor["path"])
    registration = read_json(bound_file(backend, value["target_registration"]))
    exact(registration, TARGET_REGISTRATION_FIELDS)
    historical = registration["storage_run"]
    if _capture_target_storage_run(backend, historical, live=live) != descriptor:
        raise ValueError("seed_to_arm 外部目标存储代次发生变化")
    return directory


def _require_owned_run(directory: Path) -> None:
    from full_stack_process import process_identity

    owner_path = directory / "run.lock/owner.json"
    owner = read_json(owner_path)
    actual = process_identity(os.getpid())
    if (actual is None or owner.get("identity") != actual
            or owner.get("directory") != str(directory)
            or owner.get("manifest_sha256") != binding(directory / "manifest.json")["sha256"]):
        raise ValueError("当前控制器没有持有 target_storage_run 的固定运行锁")


def _target_storage_snapshot(directory: Path) -> tuple[tuple[int, int], dict]:
    """同时固定 storage run 目录文件身份与当前追加式账本。"""
    if linked(directory) or not directory.is_dir():
        raise ValueError("target_storage_run 目录缺失、类型错误或经过链接")
    state = directory.stat()
    identity = state.st_dev, state.st_ino
    state_binding = binding(directory / "state.json")
    current = directory.stat()
    if (linked(directory) or not directory.is_dir()
            or (current.st_dev, current.st_ino) != identity
            or binding(directory / "state.json") != state_binding):
        raise ValueError("target_storage_run 在目录与账本快照期间变化")
    return identity, state_binding


@contextmanager
def target_storage_control(backend: Path, current_run: Path, value: dict):
    """在目标核验或复制期间锁住 seed_to_arm 继承的存储运行，阻止重启与停止竞态。"""
    selected = target_storage_run(backend, value, live=False)
    if selected is None:
        yield current_run
        return
    if _overlap(current_run, selected) and current_run != selected:
        raise ValueError("当前运行与 target_storage_run 不能互相包含")
    expected = copy.deepcopy(value["target_storage_run"])
    observed = _target_storage_snapshot(selected)

    def unchanged(frozen: tuple[tuple[int, int], dict]) -> None:
        if (value["target_storage_run"] != expected
                or _target_storage_snapshot(selected) != frozen
                or target_storage_run(backend, value) != selected
                or _target_storage_snapshot(selected) != frozen):
            raise ValueError("target_storage_run 在阶段期间变化")

    if selected == current_run:
        _require_owned_run(current_run)
        frozen = _target_storage_snapshot(selected)
        if frozen != observed:
            raise ValueError("target_storage_run 在取得控制锁前变化")
        unchanged(frozen)
        try:
            yield selected
        finally:
            unchanged(frozen)
        return
    with run_lock(selected):
        frozen = _target_storage_snapshot(selected)
        if frozen != observed:
            raise ValueError("target_storage_run 在取得控制锁前变化")
        unchanged(frozen)
        try:
            yield selected
        finally:
            unchanged(frozen)


def cleanup_inputs(backend: Path, directory: Path, side: str) -> tuple[dict, Environments]:
    """停止只读取固定清单与该侧运行环境，不依赖导出或另一侧的证据文件。"""
    value = registered_manifest(backend, directory)
    if side == "target" and (directory / "post-copy.json").exists():
        from devex_clone_post import runtime_inputs

        _, environment = runtime_inputs(backend, directory)
        return value, Environments(environment, environment)
    private = read_json(bound_file(backend, value[side + "_environment"]))
    exact(private, {"environment"})
    return value, Environments(private["environment"], private["environment"])


def recover_copy(backend: Path, directory: Path, owner: dict) -> dict:
    from devex_clone_copy_locks import recover_copy_locks

    with run_lock(directory):
        value = registered_manifest(backend, directory)
        return recover_copy_locks(backend, local_path(backend, value["copy_directory"]), owner,
                                  source_export=export_binding(backend, directory, value), initialized=value["initialized"])


def environments(backend: Path, value: dict) -> Environments:
    result = {}
    for side in ("source", "target"):
        path = bound_file(backend, value[side + "_environment"])
        private = read_json(path)
        exact(private, {"environment"})
        result[side] = private["environment"]
    return Environments(result["source"], result["target"])


def source_storage_binding(backend: Path, directory: Path, value: dict | None = None, *,
                           copy_stage: str | None = None) -> dict | None:
    """解析本 run 的直接 storage-source，或 seed_to_arm 唯一继承的旧 target 代次。"""
    from devex_clone_storage import current_storage_binding

    local = current_storage_binding(backend, directory, "source")
    if value is None and copy_stage == "source_to_seed":
        return local
    value = registered_manifest(backend, directory) if value is None else value
    if copy_stage is not None and copy_stage != value["copy_stage"]:
        raise ValueError("复制阶段与固定验收清单不一致")
    if value["copy_stage"] != "seed_to_arm":
        return local
    if local is not None:
        raise ValueError("seed_to_arm 只能继承已发布 seed 存储，不能建立第二套 source authority")
    source = _published_seed_source(backend, value, live_storage=True)
    if source["source_request"] != value["source_request"]:
        raise ValueError("seed_to_arm 清单的源请求不属于继承登记")
    return source["storage"]["storage"]


def inherited_build_bridges(backend: Path, value: dict) -> list[dict]:
    if value["copy_stage"] != "seed_to_arm":
        return []
    from source_fingerprints import read_binding

    source = _published_seed_source(backend, value, live_storage=False)
    if source["source_request"] != value["source_request"]:
        raise ValueError("seed_to_arm 产物来源不属于继承源请求")
    if source.get("source_generation") is not None:
        # 后继代次自身绑定当前 clean 注册来源；历史 C52 的旧产物桥接仍保留在原清单中。
        return []
    bridges = source["manifest"]["build_bridges"]
    expected = {
        (source["request"][field]["path"], source["request"][field]["sha256"])
        for field in ("backend_build", "maintenance_build")
    }
    actual = []
    for descriptor in bridges:
        bridge = read_binding(backend, descriptor)
        build = bridge.get("build")
        if not isinstance(build, dict) or set(build) != {"path", "sha256"}:
            raise ValueError("继承构建桥接缺少明确构建收据")
        actual.append((build["path"], build["sha256"]))
    if len(expected) != 2 or len(actual) != 2 or set(actual) != expected:
        raise ValueError("seed_to_arm 只能继承源请求的 API/Worker 与维护构建桥接")
    return copy.deepcopy(bridges)


def _published_seed_source(backend: Path, value: dict, *, live_storage: bool) -> dict:
    """普通清单保持 ready-only；successor 清单显式采用不可变 pending bridge。"""
    if "review_successor" not in value:
        from devex_clone_seed_source import published_source

        source = published_source(backend, value["source_registration"], live_storage=live_storage)
        if source.get("source_rebind") != value.get("source_rebind"):
            raise ValueError("seed_to_arm 来源的冻结重绑定收据变化")
        return source
    from reference_fixture_successor import published_source

    source = published_source(backend, value["review_successor"],
                              live_storage=live_storage)
    relationship = source.get("review_successor")
    if (source.get("review_successor_binding") != value["review_successor"]
            or not isinstance(relationship, dict)
            or relationship.get("source_result") != value["source_registration"]):
        raise ValueError("seed_to_arm 清单的 successor 与 C52 来源不一致")
    if source.get("source_rebind") != value.get("source_rebind"):
        raise ValueError("seed_to_arm 来源的冻结重绑定收据变化")
    if source.get("source_generation") is None or source["source_generation"] != value["source_generation"]:
        raise ValueError("seed_to_arm 必须绑定当前已发布 source-generation")
    from devex_clone_seed_export import require_export_binding

    require_export_binding(backend, source, value)
    return source


def export_binding(backend: Path, directory: Path, value: dict) -> dict:
    if value["source_export"] is not None:
        return value["source_export"]
    state = load_state(directory)
    for attempt in reversed(state["attempts"]):
        if attempt["stage"] == "export" and attempt["status"] == "passed":
            result = read_json(Path(attempt["result"]["path"]))
            exported = result["export"]
            bound_file(backend, exported)
            return exported
    raise ValueError("复制前必须完成源导出阶段")


def run_export(backend: Path, directory: Path, value: dict, environment: Environments, number: int,
               mode: str, sources: dict) -> dict:
    from devex_clone_export_recovery import (prepare_attempt, reconcile, record_verified,
                                             reject_reexport, resume)
    from devex_clone_export_verify import verify_source_export
    from devex_clone_source import export_source

    controller = Environments(dict(os.environ), {})

    def control_environment():
        if dict(os.environ) not in (controller.values["source"], environment.values["source"]):
            raise ValueError("导出控制或服务环境在检查前变化")
        return controller.use("source")

    if mode == "reconcile":
        if value["source_export"] is not None:
            raise ValueError("固定清单已有外部 export，无需采用 attempt 候选")
        return reconcile(backend, directory, value, environment, number, sources,
                         control_environment=control_environment)
    if mode == "resume":
        if value["source_export"] is not None:
            raise ValueError("固定清单已有外部 export，无需恢复 attempt 候选")
        return resume(backend, directory, value, environment, number, sources,
                      control_environment=control_environment)
    if mode != "run":
        raise ValueError("源导出模式无效")
    with control_environment(), environment.use("source"):
        if value["source_export"] is None:
            reject_reexport(backend, directory, number)
        with control_environment():
            storage = source_storage_binding(backend, directory, value)
        if value["source_export"] is None:
            # 失败仅留下本地证据；新尝试保存独立短目录，固定主清单不换代。
            output = prepare_attempt(backend, directory, value, number, sources, storage)
            export_source(backend, bound_file(backend, value["source_request"]), output,
                          control_environment=control_environment)
            exported = binding(output / "export.json")
        else:
            exported = value["source_export"]
        verified = verify_source_export(backend, exported)
        if value["source_export"] is None:
            intent = read_json(directory / f"export-{number:04d}.intent.json")
            record_verified(directory, number, intent, exported, verified)
        with control_environment():
            if source_storage_binding(backend, directory, value) != storage:
                raise ValueError("源 storage provenance 在导出前后发生变化")
    return {"status": "export_verified", "export": exported,
            "logical_inventory_sha256": verified["export"]["logical_inventory_sha256"],
            "remote_writes": 0, "restore_qualified": False}


def run_target(backend: Path, directory: Path, value: dict, environment: Environments, number: int) -> dict:
    from devex_clone_target import verify_target

    target = bound_file(backend, value["initialized"])
    with target_storage_control(backend, directory, value) as storage_run, environment.use("target"):
        return verify_target(
            backend, target.parent, directory / f"v{number:04d}", storage_run=storage_run,
        )


def copy_binary_bindings(backend: Path, value: dict) -> list[dict]:
    """仅从已绑定请求、初始化历史和构建收据收集明确二进制；业务文件不进入保护集合。"""
    from devex_clone_source_proof import validate_request
    from devex_clone_target_binding import request_binding

    source = read_json(bound_file(backend, value["source_request"]))
    validate_request(backend, source)
    _, target = initialization_history(backend, bound_file(backend, value["initialized"]))
    request_binding(backend, target)
    bindings = [dict(item) for item in source["tools"].values()]
    bindings.extend(dict(item) for item in target["tools"].values())
    builds = [(source["backend_build"], "restore-backend-build", {"api", "worker"}),
              (source["maintenance_build"], "devex-clone-tool-build", {"reset", "migrate", "tenant-data"}),
              (target["maintenance_build"], "devex-clone-tool-build", {"reset", "migrate", "tenant-data"})]
    for descriptor, kind, roles in builds:
        build = read_json(bound_file(backend, descriptor))
        if build.get("kind") != kind or set(build.get("artifacts", {})) != roles:
            raise ValueError("二进制保护必须绑定当前构建类型和完整角色")
        bindings.extend({"path": item["executable"], "sha256": item["sha256"]}
                        for item in build["artifacts"].values())
    storage = target["storage"]
    bindings.append({"path": storage["rustfs"]["identity"]["executable"], "sha256": storage["rustfs"]["sha256"]})
    bindings.append(dict(storage["redis"]["wsl"]))
    return bindings


def _run_copy_locked(backend: Path, directory: Path, value: dict, environment: Environments,
                     mode: str, storage_run: Path) -> dict:
    from devex_clone_factory import copy_to_fresh_target
    from devex_clone_resume import check_continuation, continue_copy

    output = local_path(backend, value["copy_directory"])
    exported = export_binding(backend, directory, value)
    source_storage = source_storage_binding(backend, directory, value)
    if output.exists():
        session = read_json(output / "session.json")
        if session["source_export"] != exported or session["initialized"] != value["initialized"]:
            raise ValueError("已有复制目录不属于本次固定源和目标")
        if mode not in {"reconcile", "resume"}:
            raise ValueError("已有复制目录须显式 reconcile 或 resume，不能重新首次复制")
        check_continuation(backend, output, session, mode)
        with protect_binaries(copy_binary_bindings(backend, value)):
            result = continue_copy(
                backend, output, environment.values["source"], environment.values["target"],
                mode=mode, source_storage_run=directory, target_storage_run=storage_run,
            )
        if source_storage_binding(backend, directory, value) != source_storage:
            raise ValueError("复制期间继承的源 storage provenance 发生变化")
        return result
    if mode != "run":
        raise ValueError("不存在的复制目录不能核对或续跑")
    with protect_binaries(copy_binary_bindings(backend, value)):
        result = copy_to_fresh_target(
            backend, exported, value["initialized"], environment.values["source"],
            environment.values["target"], output, copy_id=value["id"], stage=value["copy_stage"],
            source_storage_run=directory, target_storage_run=storage_run,
        )
    if source_storage_binding(backend, directory, value) != source_storage:
        raise ValueError("复制期间继承的源 storage provenance 发生变化")
    return result


def run_copy(backend: Path, directory: Path, value: dict, environment: Environments, mode: str) -> dict:
    with target_storage_control(backend, directory, value) as storage_run:
        return _run_copy_locked(backend, directory, value, environment, mode, storage_run)


def run_runtime(backend: Path, value: dict, environment: Environments, side: str, operation: str, roles: tuple,
                *, run_directory: Path | None = None) -> dict:
    from devex_clone_runtime import control, observe, reconcile_lock

    if side == "target" and run_directory is not None and (run_directory / "post-copy.json").exists():
        from devex_clone_post import registration, runtime_inputs, target_lock

        if roles != ("api",):
            raise ValueError("复制后交接只允许 API，Worker 必须保持停止")
        if operation == "start":
            if (run_directory / "seed-runtime.json").exists():
                raise ValueError("原 API 已成为身份计划的历史来源，必须通过 seed-runtime 启动新运行对")
            from devex_clone_post import registered
            from devex_clone_post_context import Context

            active = registration(backend, run_directory)
            runtime, private = runtime_inputs(backend, run_directory, descriptor=active.descriptor)
            environment = Environments(private, private)
            registered(backend, run_directory)
            number = load_state(run_directory)["attempts"][-1]["number"]
            with target_lock(backend, value, current_run=run_directory):
                context = Context(backend, run_directory, value, number)
                context.bindings()
                context.target_guard(api=False)
                with environment.use("target"):
                    result = control(backend, Path(runtime["runtime_dir"]), operation, roles, runtime["api_url"])
                context.guard()
                return result
        runtime, private = runtime_inputs(backend, run_directory)
        environment = Environments(private, private)
        with environment.use("target"):
            if operation == "recover":
                return reconcile_lock(Path(runtime["runtime_dir"]))
            if operation == "status":
                return observe(backend, Path(runtime["runtime_dir"]), roles, runtime["api_url"])
            return control(backend, Path(runtime["runtime_dir"]), operation, roles, runtime["api_url"])

    if side == "source":
        request = read_json(bound_file(backend, value["source_request"]))
        runtime = request["source"]
    else:
        initial_file = bound_file(backend, value["initialized"])
        initial = (initialization_history(backend, initial_file)[0] if operation == "start"
                   else read_json(initial_file))
        runtime = initial["generation"]["selected"]
        if operation == "start":
            if run_directory is None:
                raise ValueError("目标启动必须绑定当前统一验收目录")
            require_target_copy(backend, run_directory, value, roles)
    with environment.use(side):
        if operation == "recover":
            return reconcile_lock(Path(runtime["runtime_dir"]))
        if operation == "status":
            return observe(backend, Path(runtime["runtime_dir"]), roles, runtime["api_url"])
        return control(backend, Path(runtime["runtime_dir"]), operation, roles, runtime["api_url"])


def require_target_copy(backend: Path, directory: Path, value: dict, roles: tuple):
    from devex_clone_ledger import CloneLedger

    output = local_path(backend, value["copy_directory"])
    saved, result = read_json(output / "session.json"), read_json(output / "result.json")
    attempts = [attempt for attempt in load_state(directory)["attempts"] if attempt["stage"] == "copy"]
    if not attempts or attempts[-1]["status"] != "passed":
        raise ValueError("最新复制阶段尚未通过完整来源和控制器收尾核验")
    published = read_json(Path(attempts[-1]["result"]["path"]))
    if {key: item for key, item in published.items() if key != "continuation_receipt"} != result:
        raise ValueError("最新复制阶段结果与内部数据完成证据不同")
    if (saved["initialized"] != value["initialized"] or result.get("status") != "data_steps_verified"
            or saved["source_export"] != export_binding(backend, directory, value)
            or result["source_export_sha256"] != saved["source_export"]["sha256"]):
        raise ValueError("目标启动前须完成本目标的数据复制")
    ledger = CloneLedger(backend, output / "ledger", saved["plan_sha256"], saved["generation_sha256"])
    observed = ledger.inspect()
    from devex_clone import verify_plan_result
    from devex_clone_transfer import TransferSteps

    verified = verify_plan_result(backend, str(output / "plan.json"))
    plan = verified.plan
    no_writes = (observed["status"] == "partial" and not observed["steps"] and not plan["objects"]
                 and result["unchanged_databases"] == len(plan["databases"])
                 and set(result["unchanged_database_images"]) == {TransferSteps.db_step(item) for item in plan["databases"]})
    if (not (observed["status"] == "ledger_evidence_complete" or no_writes)
            or result["plan_sha256"] != saved["plan_sha256"]
            or result["plan_sha256"] != plan["plan_sha256"]
            or result["generation_sha256"] != saved["generation_sha256"]
            or result["written_steps"] != len(observed["steps"])
            or result["fresh_target_sha256"] != value["initialized"]["sha256"]):
        raise ValueError("目标数据复制尚未发布有效的完整账本收据")
    if result["pending_target_actions"] != plan["pending_target_actions"]:
        raise ValueError("复制完成证据与当前目标调度处置不同")
    if "worker" in roles and plan["pending_target_actions"]:
        raise ValueError("调度处置尚未完成，仅可启动 API，Worker 必须保持停止")
    return verified


def execute(backend: Path, directory: Path, stage: str, mode: str, roles: tuple = ("api", "worker"),
            *, post_copy_request: Path | None = None, producer_binding: dict | None = None,
            seed_request: Path | None = None, storage_request: Path | None = None,
            cache_request: Path | None = None) -> dict:
    from source_fingerprints import artifact_sources, current_execution_source

    if stage == "target-verify" and mode != "run":
        raise ValueError("target-verify 只支持首次 run，不得创建 reconcile 或 resume 阶段")

    # 停止只依据已登记运行产物及内核身份，不能被后来源码变化阻挡。
    session_cleanup = stage in {"post-copy", "seed-runtime"} and mode == "recover-session"
    seed_cleanup = stage == "seed-runtime" and mode in {"stop", "recover", "source-generation-recover"}
    storage_cleanup = (stage.startswith("storage-") or stage == "cache-target") and mode in {"stop", "recover"}
    cleanup = session_cleanup or seed_cleanup or storage_cleanup or (stage.startswith("runtime-") and mode in {"stop", "recover"})
    evidence_handoff = stage == "seed-runtime" and mode in {
        "arm-input", "source-rebind", "source-generation-start", "source-generation-stop", "source-generation-recover",
        "source-export", "source-export-reconcile",
    }
    cache_handoff = None
    if stage == "cache-target" and not cleanup:
        from devex_clone_cache import preflight_successor

        cache_handoff = preflight_successor(backend, directory, registered_manifest(backend, directory), cache_request, mode=mode)
        evidence_handoff = cache_handoff is not None
    published_storage = stage == "storage-target" and mode == "restart" and (directory / "seed-runtime.json").exists()
    if published_storage:
        from devex_clone_seed_rebind import published_restart_guard

        published_restart_guard(backend, directory, None)
        evidence_handoff = True
    if session_cleanup or seed_cleanup or storage_cleanup or evidence_handoff:
        value, environment = registered_manifest(backend, directory), None
    elif cleanup:
        value, environment = cleanup_inputs(backend, directory, stage.removeprefix("runtime-"))
    else:
        value = read_manifest(backend, directory)
        environment = environments(backend, value)
    source_context = (nullcontext() if cleanup or evidence_handoff else
                      artifact_sources(backend, value["build_bridges"], inherited_build_bridges(backend, value)))
    if stage in {"storage-source", "storage-target"} and mode == "restart":
        from devex_clone_storage import preflight_restart

        preflight_restart(backend, directory, value, stage.removeprefix("storage-"), storage_request)
    number, result = None, None
    with process_guard(directory, "run-control.guard"):
        try:
            with claim_run_lock(directory) as owner, source_context:
                if cache_handoff is not None:
                    if preflight_successor(backend, directory, value, cache_request, mode=mode) != cache_handoff:
                        raise ValueError("缓存工具后继在登记阶段前变化")
                if published_storage:
                    published_restart_guard(backend, directory, None)
                if stage == "seed-runtime" and mode == "source-generation-start":
                    from devex_clone_seed_generation import preflight as generation_preflight

                    generation_preflight(directory)
                if stage == "seed-runtime" and mode in {"source-generation-stop", "source-generation-recover"}:
                    from devex_clone_seed_generation_control import preflight as generation_control_preflight

                    generation_control_preflight(directory, mode)
                if stage == "seed-runtime" and mode in {"source-export", "source-export-reconcile"}:
                    from devex_clone_seed_export import preflight

                    preflight(directory, mode)
                try:
                    sources = current_execution_source(backend)
                except Exception as error:
                    if not cleanup:
                        raise
                    sources = {"source_capture_error_type": type(error).__name__}
                number = begin(directory, stage, mode, sources, verify_results=not cleanup)
                bind_controller_attempt(directory, number, owner, verify_results=not cleanup)
                if stage == "export":
                    result = run_export(backend, directory, value, environment, number, mode, sources)
                elif stage == "target-verify" and mode == "run":
                    result = run_target(backend, directory, value, environment, number)
                elif stage == "copy":
                    result = run_copy(backend, directory, value, environment, mode)
                elif stage in {"storage-source", "storage-target"}:
                    from devex_clone_storage import execute_storage

                    result = execute_storage(backend, directory, value, mode, number,
                                             stage.removeprefix("storage-"), storage_request)
                elif stage == "cache-target":
                    from devex_clone_cache import execute_cache

                    result = execute_cache(backend, directory, value, mode, number, cache_request)
                elif stage == "post-copy":
                    from devex_clone_post import execute_post

                    result = execute_post(backend, directory, value, mode, number, post_copy_request, producer_binding)
                elif stage == "seed-runtime":
                    from devex_clone_seed_runtime import execute_seed

                    result = execute_seed(backend, directory, value, mode, number, seed_request, producer_binding)
                elif stage in {"runtime-source", "runtime-target"} and mode in {"start", "stop", "status", "recover"}:
                    result = run_runtime(backend, value, environment, stage.removeprefix("runtime-"), mode, roles,
                                         run_directory=directory)
                else:
                    raise ValueError("阶段与执行模式不匹配")
                if not cleanup and (read_manifest(backend, directory) != value or current_execution_source(backend) != sources):
                    raise ValueError("阶段运行期间源码、验收工具或输入清单发生变化")
                if result.get("status") == "needs_reconciliation":
                    raise ValueError("现场仍有不匹配结果，保留核对证据并停止")
            # 来源终检和目录锁清理通过后，在同一内核互斥中发布；持久控制器保留到后续恢复。
            finish(directory, number, result=result, verify_results=not cleanup)
        except BaseException as error:
            if number is not None:
                try:
                    record_failure(backend, directory, number, stage, mode, error)
                except Exception as diagnostic_error:
                    error.add_note(f"阶段位置证据保存失败：{type(diagnostic_error).__name__}")
            if number is not None and load_state(directory, verify_results=not cleanup)["attempts"][-1]["status"] == "running":
                finish(directory, number, result=result, error=error, verify_results=not cleanup)
            raise
    return {"status": "stage_finished", "stage": stage, "mode": mode, "attempt": number,
            "result": result, "restore_qualified": False}


def status(backend: Path, directory: Path) -> dict:
    from source_fingerprints import current_execution_source

    value = read_manifest(backend, directory)
    state = load_state(directory)
    current = current_execution_source(backend)
    active = controller_observation(directory)
    attempts = [{key: item[key] for key in ("number", "stage", "mode", "status", "error_type", "result")}
                | {"execution_source_matches": item["sources"] == current} for item in state["attempts"]]
    return {"status": "run_status", "id": value["id"], "controller": active, "attempts": attempts,
            "remote_writes": 0, "restore_qualified": False}
