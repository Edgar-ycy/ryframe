"""单侧 fresh 目标的实际初始化 API；无复制执行 CLI，不启动 API、Worker 或存储服务。

prepare_target 只读核验后保存本地创建 intent；initialize_target 只执行该一次性
intent 的四库 CREATE、同 hash reset、迁移和完整库存。任何未知结果均停止，禁止
自动重放或清理；verify_target 只读复核原证据及当前资源，不授予正式恢复资格。
"""
from __future__ import annotations

from dataclasses import asdict
import json
import os
from pathlib import Path
import subprocess
import uuid

from devex_clone import read_json
from devex_clone_capture import read_bound_json, write_json
from devex_clone_export import cli_executable
from devex_clone_inventory import capture_side_inventory
from devex_clone_model import exact, local_path
from devex_clone_run_state import binding
from devex_clone_source_proof import bound_file
from devex_clone_target_binding import (KEYS, execution_backend, generation, request_binding,
                                         target_files)
from devex_clone_target_resources import Resources
from devex_clone_target_state import confirmed, failure, generation_lock, intent, now
from devex_clone_target_storage import verify_storage_generation
from devex_clone_target_evidence import evidence
from restore_build import file_digest
from restore_reference_io import ExternalTools, redact_object_diagnostic
from restore_reference_plan import plan_hash

PREPARE_FIELDS = {"format_version", "status", "prepared_at", "request", "request_sha256",
                  "generation", "absence", "fresh_target_initialized", "controlled_generation_only",
                  "external_writers_discovered", "create_intents"}
FAILURE_FIELDS = {"status", "stage", "at", "error_type", "reason", "automatic_retry",
                  "automatic_resource_cleanup", "fresh_target_initialized", "clone_verified",
                  "restore_qualified"}


def _prepare_failure(backend: Path, output: Path) -> bool:
    return evidence("prepare-failure", backend, output, arguments={})["present"]


def _resume_records(backend: Path, output: Path, request_descriptor: dict) -> dict:
    return evidence("prepare-resume-records", backend, output, request_descriptor=request_descriptor)


def _prepare_failure_resolved(backend: Path, output: Path, request_descriptor: dict) -> bool:
    return evidence("prepare-failure-resolved", backend, output,
                    arguments={"request_descriptor": request_descriptor})["resolved"]


def initialization_failure_path(backend: Path, output: Path, request_descriptor: dict) -> Path:
    return Path(evidence("initialization-failure", backend, output, request_descriptor=request_descriptor)["path"])


def prepare_resume_state(backend: Path, output: Path, request_descriptor: dict) -> dict:
    return evidence("prepare-resume-state", backend, output,
                    arguments={"request_descriptor": request_descriptor})


def _prepare_document(backend: Path, output: Path, descriptor: dict, original: dict, empty: dict) -> dict:
    return evidence("prepare-document", backend, output, arguments={"at": now(),
                    "request_descriptor": descriptor, "generation": original, "absence": empty})


def _validate_prepared(backend: Path, request_file: Path, output: Path, run, storage_run: Path | None,
                       request_descriptor: dict) -> dict:
    result = read_json(output / "prepare.json")
    exact(result, PREPARE_FIELDS)
    request = read_bound_json(request_file, request_descriptor)
    original, resources = context(backend, request, output, run, storage_run=storage_run)
    empty = empty_resources(resources)
    unchanged(backend, request, original, resources, run)
    bound_file(backend, request_descriptor)
    evidence("prepare-current", backend, output, arguments={"at": result["prepared_at"],
             "request_descriptor": request_descriptor, "generation": original, "absence": empty,
             "observed": result})
    return result


def resume_prepare_target(backend: Path, output: Path, run=subprocess.run, *, storage_run: Path | None,
                          request_descriptor: dict) -> dict:
    """显式重做无资源写入的 prepare；原请求、失败和诊断全部保留。"""
    backend = backend.resolve(strict=True)
    request_file = bound_file(backend, request_descriptor)
    output = local_path(backend, str(output), new=not output.exists())
    state = prepare_resume_state(backend, output, request_descriptor)
    if not state["resumable"]:
        raise ValueError("fresh 目标 prepare 不能续作：" + state["reason"])
    if not output.exists():
        output.mkdir()
    attempt = uuid.uuid4().hex
    intent_path = output / f"resume-prepare-{attempt}.intent.json"
    write_json(intent_path, {"format_version": 1, "kind": "devex-clone-prepare-resume-intent",
               "attempt": attempt, "at": now(), "request": request_descriptor,
               "files_before": {name: binding(output / name) for name in state["files"]}})
    try:
        if state["prepared"]:
            result = _validate_prepared(backend, request_file, output, run, storage_run,
                                        request_descriptor)
        else:
            request = read_bound_json(request_file, request_descriptor)
            request_path = output / "request.json"
            if request_path.exists():
                if read_json(request_path) != request:
                    raise ValueError("fresh 目标中断请求不同于固定 registration")
            else:
                write_json(request_path, request)
            original, resources = context(backend, request, output, run, storage_run=storage_run)
            empty = empty_resources(resources)
            unchanged(backend, request, original, resources, run)
            bound_file(backend, request_descriptor)
            if read_json(request_path) != request:
                raise ValueError("fresh 目标请求副本在 prepare 续作期间变化")
            result = _prepare_document(backend, output, request_descriptor, original, empty)
            write_json(output / "prepare.json", result)
        write_json(output / f"resume-prepare-{attempt}.confirmed.json", {
            "format_version": 1, "kind": "devex-clone-prepare-resume-confirmed",
            "attempt": attempt, "at": now(), "intent": binding(intent_path),
            "prepare": binding(output / "prepare.json"), "request": binding(output / "request.json"),
            "remote_writes": 0})
        return result
    except BaseException as error:
        try:
            write_json(output / f"resume-prepare-{attempt}.failure.json", {
                "format_version": 1, "kind": "devex-clone-prepare-resume-failure",
                "attempt": attempt, "at": now(), "intent": binding(intent_path),
                "error_type": type(error).__name__,
                "reason": redact_object_diagnostic(str(error), os.environ)
                if isinstance(error, ValueError) else "只读 prepare 未完成，核对同目录诊断",
                "remote_writes": 0})
        except BaseException as diagnostic_error:
            error.add_note(f"prepare 续作失败证据保存失败：{type(diagnostic_error).__name__}")
        raise


def unresolved_failure(backend: Path, output: Path) -> bool:
    """仅显式确认的 prepare、inventory 或迁移前缀续作解除对应旧失败。"""
    failures = sorted(output.glob("failure*.json"))
    if not failures:
        return False
    try:
        prepared = read_json(output / "prepare.json")
        exact(prepared, PREPARE_FIELDS)
        request_descriptor = prepared["request"]
        active = initialization_failure_path(backend, output, request_descriptor)
        if (output / "initialized.json").is_file():
            from devex_clone_target_inventory_resume import inventory_resume_confirmed

            initialized = read_json(output / "initialized.json")
            inventory = inventory_resume_confirmed(backend, output, initialized)
            from devex_clone_target_resume_evidence import migration_resume_confirmed

            migration = (migration_resume_confirmed(
                         backend, output, initialized))
            if inventory or migration:
                return False
            return not (active == output / "failure.json"
                        and _prepare_failure_resolved(
                            backend, output, request_descriptor))
        if active != output / "failure.json":
            return True
        return not _prepare_failure_resolved(backend, output, request_descriptor)
    except (AttributeError, IndexError, KeyError, OSError, TypeError, ValueError):
        return True


def context(backend: Path, request: dict, output: Path, run, *, storage_run: Path | None = None,
            owned_lock_identity: int | None = None,
            lock_output: Path | None = None) -> tuple[dict, Resources]:
    if owned_lock_identity is not None:
        from devex_clone_target_state import generation_checkpoint

        generation_checkpoint(lock_output or output, owned_lock_identity)
    original = generation(backend, request, run)
    review, selected = request_binding(backend, request)
    execution_root, _ = execution_backend(backend, request)
    resources = Resources(backend, request, output, selected, review, run, storage_run=storage_run,
                          execution_backend=execution_root, owned_lock_identity=owned_lock_identity,
                          lock_output=lock_output)
    original["storage"] = resources.storage_identity()
    return original, resources


def unchanged(backend: Path, request: dict, original: dict, resources: Resources, run) -> None:
    actual = generation(backend, request, run)
    actual["storage"] = resources.storage_identity()
    if actual != original:
        raise ValueError("目标源码、配置、凭据、维护产物或存储进程代次变化")


def empty_resources(resources: Resources) -> dict:
    return {"databases": [resources.database_state(db, exists=False) for db in resources.request["target"]["databases"]],
            "objects": resources.objects(initialized=False), "redis": resources.redis_state(initialized=False, sentinel=False)}


def prepare_target(backend: Path, request_file: Path, output: Path, run=subprocess.run,
                   *, storage_run: Path | None = None, request_descriptor: dict | None = None) -> dict:
    """只读精确目标；新目录中的 intent 不代表资源已创建。"""
    backend = backend.resolve(strict=True)
    request_file = local_path(backend, str(request_file))
    output = local_path(backend, str(output), new=True)
    if not output.parent.is_dir():
        raise ValueError("初始化证据须位于已有父目录中的新目录")
    output.mkdir()
    stage = "request"
    try:
        request_digest = {"path": str(request_file), **file_digest(request_file)}
        expected_request = request_digest if request_descriptor is None else request_descriptor
        if expected_request != request_digest:
            raise ValueError("准备请求不同于调用方固定 descriptor")
        request = read_bound_json(request_file, expected_request)
        write_json(output / "request.json", request)
        stage = "prepare_binding"
        original, resources = context(backend, request, output, run, storage_run=storage_run)
        stage = "prepare_absence"
        empty = empty_resources(resources)
        unchanged(backend, request, original, resources, run)
        if {"path": str(request_file), **file_digest(request_file)} != expected_request:
            raise ValueError("准备期间请求文件变化")
        result = _prepare_document(backend, output, expected_request, original, empty)
        write_json(output / "prepare.json", result)
        return result
    except BaseException as error:
        failure(output, stage, error)
        raise


def reset_plan(resources: Resources, maintenance: dict, request: dict, original: dict, stage: str) -> tuple[dict, str]:
    reset_execution_configuration(resources, maintenance)
    raw = resources.command(
        stage,
        [cli_executable(maintenance, "reset"), "plan"],
        env=resources.environment,
    ).stdout
    value = evidence("reset-plan-output", backend=resources.backend, target=resources.output,
                     arguments={"text": raw.decode("utf-8"), "request": request, "original": original})
    write_json(resources.output / f"{stage}.json", value)
    return value["manifest"], value["plan_hash"]


def reset_execution_configuration(resources: Resources, maintenance: dict) -> Path:
    result = evidence("reset-execution-configuration", resources.backend, resources.output,
                      arguments={"execution_backend": str(resources.execution_backend),
                                 "app_env": resources.environment.get("APP_ENV"), "maintenance": maintenance})
    return Path(result["path"])


def initialize_databases(backend: Path, request: dict, original: dict, resources: Resources, run) -> dict:
    for db in sorted(request["target"]["databases"], key=lambda item: item["key"]):
        unchanged(backend, request, original, resources, run)
        stage = "create-" + db["key"]
        intent(resources.output, stage, {key: db[key] for key in ("key", "server_uuid", "database")})
        resources.create_database(db)
        observed = resources.database_state(db, exists=True, empty=True)
        unchanged(backend, request, original, resources, run)
        confirmed(resources.output, stage, observed)
    resources.objects(initialized=False)
    resources.redis_state(initialized=False, sentinel=False)
    intent(resources.output, "sentinel", {"key": request["reset"]["sentinel_key"]})
    resources.create_sentinel()
    confirmed(resources.output, "sentinel", resources.redis_state(initialized=False, sentinel=True))
    for db in request["target"]["databases"]:
        resources.database_state(db, exists=True, empty=True)
    maintenance = original["maintenance"]
    first = reset_plan(resources, maintenance, request, original, "reset-plan")
    unchanged(backend, request, original, resources, run)
    if first != reset_plan(resources, maintenance, request, original, "reset-plan-confirm"):
        raise ValueError("reset plan 即时复核变化")
    manifest, sha = first
    state = reset_execution_configuration(resources, maintenance)
    state.mkdir()
    unchanged(backend, request, original, resources, run)
    for db in request["target"]["databases"]:
        resources.database_state(db, exists=True, empty=True)
    resources.objects(initialized=False)
    resources.redis_state(initialized=False, sentinel=True)
    intent(resources.output, "reset", {"plan_hash": sha})
    resources.command("reset-execute", [cli_executable(maintenance, "reset"), "execute", "--plan-hash", sha,
                      "--confirm-reset", manifest["confirmation_phrase"]],
                      env=resources.environment, timeout=1800)
    completed = evidence("reset-completed", backend, resources.output, manifest=manifest, sha=sha)
    confirmed(resources.output, "reset", completed)
    for operation in evidence("migration-operations", backend, resources.output,
                              executable=cli_executable(maintenance, "migrate"))["operations"]:
        unchanged(backend, request, original, resources, run)
        resources.command(operation["stage"], operation["command"], timeout=1800)
    return completed


def reconcile_preflight_failure(backend: Path, output: Path, run=subprocess.run, *, storage_run: Path | None = None,
                                request_descriptor: dict | None = None) -> dict:
    """只清理 reset preflight 失败前创建的空 fresh 资源，绝不重放初始化。"""
    backend, output = backend.resolve(strict=True), local_path(backend, str(output))
    failure_path = output / "failure.json"
    failure_value = read_json(failure_path)
    exact(failure_value, FAILURE_FIELDS)
    if (failure_value["stage"] != "create_and_reset" or failure_value["error_type"] != "CalledProcessError"
            or not (output / "initialize.started.json").is_file()):
        raise ValueError("仅允许收尾已记录的 reset preflight 失败")
    reset_state = output / "reset-state"
    reports = sorted(reset_state.glob("*.report.json"))
    if len(reports) != 1:
        raise ValueError("reset preflight 收尾需要唯一报告")
    report = read_json(reports[0])
    preflight = report.get("phases", {}).get("preflight", {})
    if (report.get("status") != "failed" or report.get("failed_phase") != "preflight"
            or preflight.get("status") != "failed"
            or any(item.get("status") != "pending" for name, item in report["phases"].items()
                   if name not in {"preflight", "release"})
            or report["phases"].get("release", {}).get("status") != "complete"):
        raise ValueError("reset 已进入资源写入或报告不属于可收尾的 preflight 失败")
    with generation_lock(output) as lock_identity:
        prepared = read_json(output / "prepare.json")
        expected = prepared["request"] if request_descriptor is None else request_descriptor
        if prepared["request"] != expected:
            raise ValueError("收尾请求不同于固定 registration")
        request = read_bound_json(local_path(backend, expected["path"]), expected)
        original, resources = context(
            backend, request, output, run, storage_run=storage_run,
            owned_lock_identity=lock_identity
        )
        if original != prepared["generation"]:
            raise ValueError("收尾前来源、工具或服务代次发生变化")
        for db in request["target"]["databases"]:
            resources.database_state(db, exists=True, empty=True)
        resources.objects(initialized=False)
        resources.redis_state(initialized=False, sentinel=True)
        before = {"databases": [resources.database_state(db, exists=True, empty=True)
                                for db in request["target"]["databases"]],
                  "objects": resources.objects(initialized=False),
                  "redis": resources.redis_state(initialized=False, sentinel=True)}
        write_json(output / "reconciliation-plan.json", {"status": "preflight_failure_cleanup_planned",
                   "failure": file_digest(failure_path), "reset_report": file_digest(reports[0]), "before": before})
        resources.remove_sentinel()
        for db in sorted(request["target"]["databases"], key=lambda item: item["key"], reverse=True):
            resources.drop_empty_database(db)
        after = {"databases": [resources.database_state(db, exists=False) for db in request["target"]["databases"]],
                 "objects": resources.objects(initialized=False),
                 "redis": resources.redis_state(initialized=False, sentinel=False)}
        result = {"status": "preflight_failure_reconciled", "before": before, "after": after,
                  "failure": file_digest(failure_path), "reset_report": file_digest(reports[0]),
                  "automatic_retry": False, "restore_qualified": False}
        write_json(output / "reconciliation-completed.json", result)
        return result


def inventory(backend: Path, request: dict, resources: Resources,
              output: Path) -> tuple[dict, tuple[dict, ...]]:
    side = {**request["target"], "runtime_dir": resources.selected["runtime_dir"], "api_url": resources.selected["api_url"]}
    tools = ExternalTools({"target": side, "tools": request["tools"]}, resources.output,
                          resources.runner, before_execute=resources.checkpoint)
    arguments = {"environment": resources.environment}
    if resources.execution_backend != backend:
        arguments["evidence_root"] = backend
    captured = capture_side_inventory(resources.execution_backend, "target", tools,
                                      bound_file(backend, request["maintenance_build"]), output, **arguments)
    images = {item.resource["database"]: json.loads(json.dumps(asdict(item))) for item in captured.observations}
    evidence("initial-inventory-shape", backend, resources.output,
             arguments={"databases": request["target"]["databases"], "images": images})
    return {"receipt": captured.receipt_file, "observations": images}, captured.evidence_files


def initialize_target(backend: Path, output: Path, run=subprocess.run, *, storage_run: Path | None = None,
                      request_descriptor: dict | None = None, publish_files=None) -> dict:
    """仅首次调用可写；中断、失败、已初始化均不自动重放。"""
    backend = backend.resolve(strict=True)
    output = local_path(backend, str(output))
    if unresolved_failure(backend, output) or (output / "initialize.started.json").exists():
        raise ValueError("初始化已有尝试；只能只读核对，禁止重放或自动清理")
    stage, claimed = "initialize_binding", False
    after_unlock = [] if publish_files is not None else None
    try:
        with generation_lock(output, after_initialize_unlock=after_unlock) as lock_identity:
            if unresolved_failure(backend, output) or (output / "initialize.started.json").exists():
                raise ValueError("本代次已有执行记录，不能重复初始化")
            claimed = True
            prepared_digest = file_digest(output / "prepare.json")
            prepared = read_json(output / "prepare.json")
            expected_request = prepared["request"] if request_descriptor is None else request_descriptor
            if prepared["request"] != expected_request:
                raise ValueError("initialize 请求不同于固定 registration")
            request = read_bound_json(local_path(backend, expected_request["path"]), expected_request)
            if request != read_json(output / "request.json") or plan_hash(request) != prepared["request_sha256"]:
                raise ValueError("prepare 请求文件已变化")
            from devex_clone_target_tree_claim import prepared_baseline, validate_complete
            baseline = prepared_baseline(backend, output, expected_request, request)
            original, resources = context(
                backend, request, output, run, storage_run=storage_run,
                owned_lock_identity=lock_identity
            )
            from devex_clone_target_prepared_files import validate_prepared_tree
            validate_prepared_tree(
                backend, output, baseline, expected_request, request, locked_guard=True)
            if prepared["generation"] != original or prepared["absence"] != empty_resources(resources):
                raise ValueError("准备后来源或目标缺失状态变化")
            write_json(output / "initialize.started.json", {"at": now(), "generation_sha256": plan_hash(original)})
            stage = "create_and_reset"
            reset = initialize_databases(backend, request, original, resources, run)
            stage = "inventory"
            initial, inventory_files = inventory(backend, request, resources, output / "inventory-initial")
            objects = resources.objects(initialized=True)
            redis = resources.redis_state(initialized=True, sentinel=True)
            unchanged(backend, request, original, resources, run)
            bound_file(backend, prepared["request"])
            if file_digest(output / "prepare.json") != prepared_digest:
                raise ValueError("初始化期间 prepare 证据变化")
            result = {"format_version": 1, "status": "fresh_target_initialized", "id": request["id"],
                      "scope_id": request["target"]["scope_id"], "completed_at": now(), "generation": original,
                      "prepare_sha256": file_digest(output / "prepare.json")["sha256"], "reset": reset,
                      "inventory": initial, "objects": objects, "redis": redis, "history": evidence("history", backend, output),
                      "controlled_generation_never_started": True, "external_writers_discovered": False,
                      "target_ready": False, "clone_verified": False, "restore_qualified": False}
            write_json(output / "initialized-candidate.json", result)
            write_json(output / "initialized.json", result)
            files = validate_complete(backend, output, baseline, request, original, initial,
                                      inventory_files, result)
            if after_unlock is not None:
                after_unlock.append(lambda: publish_files(files))
        return result
    except BaseException as error:
        if claimed:
            failure(output, stage, error)
        raise


def read_target_authority(backend: Path, directory: Path, run=subprocess.run) -> dict:
    """仅复用完整历史、恢复谱系及配置来源；资源观察由 Rust 调度，不写入任何文件。"""
    from devex_clone_factory_context import initialization_history
    from devex_clone_run import read_manifest, environments
    from devex_clone_target_binding import initialized_target_files
    from source_fingerprints import artifact_sources

    original = read_manifest(backend, directory)
    if original["copy_stage"] != "source_to_seed":
        raise ValueError("原生目标权威只接受 source_to_seed，其他阶段继续使用完整状态机")
    filename = bound_file(backend, original["initialized"])
    if filename.name != "initialized.json":
        raise ValueError("目标必须绑定本代次 initialized.json")
    output = filename.parent
    with artifact_sources(backend, original["build_bridges"], []):
        initialized, request = initialization_history(backend, filename)
        with environments(backend, original).use("target"):
            observed = generation(backend, request, run)
        expected = {key: value for key, value in initialized["generation"].items() if key != "storage"}
        if observed != expected:
            raise ValueError("目标当前源码、维护工具、配置或物理连接不同于初始化代次")
        tree = target_files(output)
        authority = {
            "format_version": 1, "kind": "devex-clone-target-authority",
            "initialized": binding(filename), "request": read_json(output / "prepare.json")["request"],
            "initialized_files": initialized_target_files(backend, output),
            "files": tree, "generation": observed, "remote_writes": 0,
            "target_ready": False, "clone_verified": False, "restore_qualified": False,
        }
        if (initialization_history(backend, filename) != (initialized, request)
                or target_files(output) != tree or read_manifest(backend, directory) != original):
            raise ValueError("目标只读权威核验期间输入或历史发生变化")
        return authority


def verify_target(backend: Path, output: Path, observation_dir: Path, run=subprocess.run,
                  *, storage_run: Path | None = None, request_descriptor: dict | None = None) -> dict:
    """只用于复制前的完整初像复验；复制中不得用它代替逐步骤 live guard。"""
    backend = backend.resolve(strict=True)
    output = local_path(backend, str(output))
    observation_dir = local_path(backend, str(observation_dir), new=True)
    if (not observation_dir.parent.is_dir() or unresolved_failure(backend, output)
            or (output / "initialize.lock").exists()):
        raise ValueError("初始化失败、仍持锁或观察目录无效")
    observation_dir.mkdir()
    stage = "verify_existing"
    try:
        with generation_lock(output) as lock_identity:
            original_result = read_json(output / "initialized.json")
            prepared = read_json(output / "prepare.json")
            expected_request = prepared["request"] if request_descriptor is None else request_descriptor
            if prepared["request"] != expected_request:
                raise ValueError("verify 请求不同于固定 registration")
            request = read_bound_json(local_path(backend, expected_request["path"]), expected_request)
            if (file_digest(output / "prepare.json")["sha256"] != original_result["prepare_sha256"]
                    or read_json(output / "initialized-candidate.json") != original_result
                    or request != read_json(output / "request.json") or original_result["history"] != evidence("history", backend, output)):
                raise ValueError("初始化发布或请求证据变化")
            evidence("initial-inventory", backend, output, receipt=original_result["inventory"]["receipt"],
                                     resumed=(output / "failure.json").is_file())
            original, resources = context(
                backend, request, observation_dir, run, storage_run=storage_run,
                owned_lock_identity=lock_identity, lock_output=output
            )
            verify_storage_generation(original_result["generation"], original, resources.storage_runtime_binding, resources.cache_runtime_binding)
            reset = read_json(output / "reset-plan.json")
            if evidence("reset-completed", backend, output, manifest=reset["manifest"], sha=reset["plan_hash"]) != original_result["reset"]:
                raise ValueError("原 reset 完成证据已变化")
            observed, _ = inventory(backend, request, resources, observation_dir / "inventory")
            if (observed["observations"] != original_result["inventory"]["observations"]
                    or resources.objects(initialized=True) != original_result["objects"]
                    or resources.redis_state(initialized=True, sentinel=True) != original_result["redis"]):
                raise ValueError("目标当前完整前像与初始化像不同")
            unchanged(backend, request, original, resources, run)
        result = {"status": "fresh_target_reverified", "initialized": {"path": str(output / "initialized.json"), **file_digest(output / "initialized.json")},
                  "inventory": observed, "remote_writes": 0, "target_ready": False, "clone_verified": False, "restore_qualified": False}
        write_json(observation_dir / "verify.json", result)
        return result
    except BaseException as error:
        failure(observation_dir, stage, error)
        raise
