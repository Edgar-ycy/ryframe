"""单侧 fresh 目标的实际初始化 API；无复制执行 CLI，不启动 API、Worker 或存储服务。

prepare/resume-prepare 已由原生 Rust 入口承担；initialize_target 只执行该一次性
intent 的四库 CREATE、同 hash reset、迁移和完整库存。任何未知结果均停止，禁止
自动重放或清理；verify_target 只读复核原证据及当前资源，不授予正式恢复资格。
"""
from __future__ import annotations

from dataclasses import asdict
import json
from pathlib import Path
import subprocess

from devex_clone import read_json
from devex_clone_capture import read_bound_json, write_json
from devex_clone_export import cli_executable
from devex_clone_inventory import capture_side_inventory
from devex_clone_model import exact, local_path
from devex_clone_source_proof import bound_file
from devex_clone_target_binding import execution_backend, generation, request_binding
from devex_clone_target_resources import Resources
from devex_clone_target_state import confirmed, failure, generation_lock, intent, now
from devex_clone_target_storage import verify_storage_generation
from devex_clone_target_evidence import evidence
from restore_build import file_digest
from restore_reference_io import ExternalTools
from restore_reference_plan import plan_hash

PREPARE_FIELDS = {"format_version", "status", "prepared_at", "request", "request_sha256",
                  "generation", "absence", "fresh_target_initialized", "controlled_generation_only",
                  "external_writers_discovered", "create_intents"}
FAILURE_FIELDS = {"status", "stage", "at", "error_type", "reason", "automatic_retry",
                  "automatic_resource_cleanup", "fresh_target_initialized", "clone_verified",
                  "restore_qualified"}


def _prepare_failure_resolved(backend: Path, output: Path, request_descriptor: dict) -> bool:
    return evidence("prepare-failure-resolved", backend, output,
                    arguments={"request_descriptor": request_descriptor})["resolved"]


def initialization_failure_path(backend: Path, output: Path, request_descriptor: dict) -> Path:
    return Path(evidence("initialization-failure", backend, output, request_descriptor=request_descriptor)["path"])


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
    side = {**request["target"], "s3": {"configuration": resources.object_configuration},
            "runtime_dir": resources.selected["runtime_dir"], "api_url": resources.selected["api_url"]}
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
