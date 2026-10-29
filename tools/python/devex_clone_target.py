"""目标历史、预检失败收尾和只读复核；初始化与显式续作由正式Rust入口执行。"""
from __future__ import annotations

from pathlib import Path
import subprocess

from devex_clone import read_json
from devex_clone_capture import read_bound_json, write_json
from devex_clone_model import exact, local_path
from devex_clone_source_proof import bound_file
from devex_clone_target_binding import execution_backend, generation, request_binding
from devex_clone_target_resources import Resources
from devex_clone_target_state import generation_lock
from devex_clone_target_evidence import evidence
from restore_build import file_digest

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


def context(backend: Path, request: dict, output: Path, run, *, request_descriptor: dict, storage_run: Path | None = None,
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
                          lock_output=lock_output, request_descriptor=request_descriptor)
    original["storage"] = resources.storage_identity()
    return original, resources


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
            owned_lock_identity=lock_identity, request_descriptor=expected
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
