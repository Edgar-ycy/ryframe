"""fresh 目标库存失败的只读续作判定、执行与最终确认。"""
from pathlib import Path
import subprocess
import uuid

from devex_clone import read_json
from devex_clone_capture import read_bound_json, write_json
from devex_clone_export import cli_executable
from devex_clone_model import exact, local_path
from devex_clone_run_state import binding
from devex_clone_source_proof import bound_file
from devex_clone_target_state import generation_lock, now
from devex_clone_target_evidence import evidence
import devex_clone_target_tree_claim as tree_claim


def inventory_resume_confirmed(backend: Path, output: Path, initialized: dict) -> bool:
    """成功确认及只读重试谱系全部由受管 Rust 证据协议核对。"""
    try:
        return evidence("inventory-resume-confirmed", backend, output,
                        initialized=initialized)["confirmed"] is True
    except (AttributeError, IndexError, KeyError, OSError, TypeError, ValueError):
        return False


def inventory_resume_state(backend: Path, output: Path, request_descriptor: dict,
                           prepared_files: dict, *,
                           owned_lock_identity: int | None = None) -> dict:
    """Python 仅复核持锁句柄身份，纯证据与恢复边界由 Rust 判定。"""
    from devex_clone_target_state import generation_checkpoint

    backend, output = backend.resolve(strict=True), local_path(backend, str(output))
    def checkpoint():
        if owned_lock_identity is not None:
            generation_checkpoint(output)
            lock = output / "initialize.lock"
            if not lock.is_dir() or lock.stat().st_ino != owned_lock_identity:
                raise ValueError("目标初始化锁身份变化")
    checkpoint()
    result = evidence("inventory-resume-state", backend, output,
                      request_descriptor=request_descriptor, prepared_files=prepared_files,
                      owned_lock=owned_lock_identity is not None)
    checkpoint()
    return result


def resume_inventory_target(backend: Path, output: Path, run=subprocess.run, *, storage_run: Path | None = None,
                            request_descriptor: dict | None = None,
                            prepared_files: dict, publish_files=None) -> dict:
    """只恢复 reset 已完成后的库存采集与发布，绝不重放任何资源写入。"""
    from devex_clone_target import (PREPARE_FIELDS, context, inventory, reset_plan,
                                    unchanged)

    backend, output = backend.resolve(strict=True), local_path(backend, str(output))
    prepared = read_json(output / "prepare.json")
    exact(prepared, PREPARE_FIELDS)
    expected = prepared["request"] if request_descriptor is None else request_descriptor
    state = inventory_resume_state(backend, output, expected, prepared_files)
    if not state["resumable"]:
        raise ValueError("库存恢复不可继续：" + state["reason"])
    failure_path = bound_file(backend, state["failure"])
    if failure_path.parent != output:
        raise ValueError("库存恢复失败证据不属于当前 fresh 目标")
    attempt = uuid.uuid4().hex
    resume_intent = output / f"resume-initialize-{attempt}.intent.json"
    after_unlock = [] if publish_files is not None else None
    try:
        with generation_lock(output, after_initialize_unlock=after_unlock) as lock_identity:
            locked_state = inventory_resume_state(
                backend, output, expected, prepared_files,
                owned_lock_identity=lock_identity
            )
            if locked_state != state:
                raise ValueError("取得初始化锁前库存续作证据树发生变化")
            intent = evidence("resume-transition", backend, output,
                transition="inventory-intent", arguments={"attempt": attempt, "at": now(),
                    "failure": binding(failure_path), "prepared_files": state["prepared_files"]})
            write_json(resume_intent, intent)
            trusted = tree_claim.claim_one(output, state["files"], resume_intent, intent)
            prepared = read_json(output / "prepare.json")
            exact(prepared, PREPARE_FIELDS)
            expected = prepared["request"] if request_descriptor is None else request_descriptor
            if prepared["request"] != expected:
                raise ValueError("库存恢复请求不同于固定 registration")
            request = read_bound_json(local_path(backend, expected["path"]), expected)
            if request != read_json(output / "request.json"):
                raise ValueError("库存恢复请求文件已变化")
            tree_claim.unchanged(output, trusted)
            original, resources = context(
                backend, request, output, run, storage_run=storage_run,
                owned_lock_identity=lock_identity
            )
            trusted = tree_claim.claim_runtime(backend, output, trusted, state["baseline_files"])
            if original != prepared["generation"]:
                raise ValueError("库存恢复前来源、工具或服务代次变化")
            tree_claim.unchanged(output, trusted)
            manifest, sha = reset_plan(resources, original["maintenance"], request, original,
                                       f"resume-reset-plan-{attempt}")
            plan_path = output / f"resume-reset-plan-{attempt}.json"
            command_paths = list(output.glob(
                f"resume-reset-plan-{attempt}-*.command.json"
            ))
            if len(command_paths) != 1:
                raise ValueError("库存续作没有产生唯一 reset plan 命令收据")
            trusted = tree_claim.claim_many(
                output, trusted, [plan_path, command_paths[0]]
            )
            evidence("reset-plan", backend, output, path=str(command_paths[0]),
                     executable=cli_executable(original["maintenance"], "reset"),
                     plan={"manifest": manifest, "plan_hash": sha})
            reset = evidence("reset-completed", backend, output, manifest=manifest, sha=sha)
            if read_json(output / "reset.confirmed.json").get("observed") != reset:
                raise ValueError("库存恢复 reset 完成收据不同于当前受控计划")
            inventory_directory = output / f"inventory-resume-{attempt}"
            tree_claim.unchanged(output, trusted)
            initial, inventory_files = inventory(backend, request, resources, inventory_directory)
            trusted = tree_claim.claim_inventory(
                output, trusted, inventory_directory, initial, inventory_files
            )
            tree_claim.unchanged(output, trusted)
            objects = resources.objects(initialized=True)
            trusted = tree_claim.claim_objects(
                backend, output, trusted, state["baseline_files"], request
            )
            tree_claim.unchanged(output, trusted)
            redis = resources.redis_state(initialized=True, sentinel=True)
            tree_claim.unchanged(output, trusted)
            unchanged(backend, request, original, resources, run)
            trusted = tree_claim.claim_runtime(backend, output, trusted, state["baseline_files"])
            result = evidence("resume-transition", backend, output, transition="initialized",
                              arguments={"request": request, "generation": original, "reset": reset,
                                         "inventory": initial, "objects": objects, "redis": redis,
                                         "at": now()})
            candidate_path = output / "initialized-candidate.json"
            write_json(candidate_path, result)
            trusted = tree_claim.claim_one(output, trusted, candidate_path, result)
            initialized_path = output / "initialized.json"
            write_json(initialized_path, result)
            trusted = tree_claim.claim_one(output, trusted, initialized_path, result)
            confirmation_path = output / f"resume-initialize-{attempt}.confirmed.json"
            confirmation = evidence("resume-transition", backend, output,
                transition="inventory-confirmed", arguments={"attempt": attempt, "at": now(),
                    "intent": binding(resume_intent), "failure": binding(failure_path),
                    "initialized": binding(initialized_path), "prepared_files": state["prepared_files"]})
            write_json(confirmation_path, confirmation)
            trusted = tree_claim.claim_one(
                output, trusted, confirmation_path, confirmation
            )
            tree_claim.unchanged(output, trusted)
            if after_unlock is not None:
                after_unlock.append(lambda: publish_files(trusted))
        return result
    except BaseException as error:
        if resume_intent.is_file():
            failure = evidence("resume-transition", backend, output,
                transition="inventory-failure", arguments={"attempt": attempt, "at": now(),
                    "intent": binding(resume_intent), "source_failure": binding(failure_path),
                    "prepared_files": state["prepared_files"], "error_type": type(error).__name__})
            write_json(output / f"resume-initialize-{attempt}.failure.json", failure)
        raise
