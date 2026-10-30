"""共用目标历史与资源上下文；fresh生命周期由正式Rust入口执行。"""
from __future__ import annotations

from pathlib import Path

from devex_clone import read_json
from devex_clone_model import exact
from devex_clone_target_binding import execution_backend, generation, request_binding
from devex_clone_target_resources import Resources
from devex_clone_target_evidence import evidence

PREPARE_FIELDS = {"format_version", "status", "prepared_at", "request", "request_sha256",
                  "generation", "absence", "fresh_target_initialized", "controlled_generation_only",
                  "external_writers_discovered", "create_intents"}


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
