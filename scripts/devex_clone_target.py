"""单侧 fresh 目标的实际初始化 API；无复制执行 CLI，不启动 API、Worker 或存储服务。

prepare_target 只读核验后保存本地创建 intent；initialize_target 只执行该一次性
intent 的四库 CREATE、同 hash reset、迁移和完整库存。任何未知结果均停止，禁止
自动重放或清理；verify_target 只读复核原证据及当前资源，不授予正式恢复资格。
"""
from __future__ import annotations

from dataclasses import asdict
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import uuid

from devex_clone import read_json
from devex_clone_capture import read_bound_json, unique_object, write_json
from devex_clone_export import cli_executable
from devex_clone_inventory import capture_side_inventory
from devex_clone_model import exact, local_path
from devex_clone_run_state import binding
from devex_clone_source_proof import bound_file
from devex_clone_target_binding import (KEYS, execution_backend, generation, request_binding,
                                         target_files, validate_reset_manifest)
from devex_clone_target_resources import Resources
from devex_clone_target_state import confirmed, failure, generation_lock, intent, now
from devex_clone_target_storage import verify_storage_generation
from devex_clone_target_time import ordered, timestamp
import devex_clone_target_reset_evidence as reset_evidence
from restore_build import file_digest
from restore_reference_io import ExternalTools, redact_object_diagnostic
from restore_reference_plan import plan_hash

PREPARE_STAGES = {"request", "prepare_binding", "prepare_absence"}
PREPARE_FIELDS = {"format_version", "status", "prepared_at", "request", "request_sha256",
                  "generation", "absence", "fresh_target_initialized", "controlled_generation_only",
                  "external_writers_discovered", "create_intents"}
FAILURE_FIELDS = {"status", "stage", "at", "error_type", "reason", "automatic_retry",
                  "automatic_resource_cleanup", "fresh_target_initialized", "clone_verified",
                  "restore_qualified"}
RESUME_FIELDS = {"format_version", "kind", "attempt", "at", "request", "files_before"}


def target_evidence_hooks():
    """为叶子证据模块绑定当前目标实现，不形成反向导入。"""
    from devex_clone_target_resume_evidence import TargetEvidenceHooks

    return TargetEvidenceHooks(resume_records=_resume_records, prepare_artifact=_prepare_artifact,
                               reset_completed=reset_evidence.reset_completed, history=history,
                               verify_initial_inventory=verify_initial_inventory)


def _prepare_failure(output: Path) -> dict | None:
    path = output / "failure.json"
    if not path.exists():
        return None
    value = read_json(path)
    exact(value, FAILURE_FIELDS)
    if (value["status"] != "needs_reconciliation" or not isinstance(value["stage"], str)
            or value["stage"] not in PREPARE_STAGES
            or not timestamp(value["at"])
            or not isinstance(value["error_type"], str) or not value["error_type"]
            or not isinstance(value["reason"], str) or not value["reason"]
            or value["automatic_retry"] is not False or value["automatic_resource_cleanup"] is not False
            or value["fresh_target_initialized"] is not False or value["clone_verified"] is not False
            or value["restore_qualified"] is not False):
        raise ValueError("fresh 目标 failure 不是可续作的只读 prepare 失败")
    return value


def _resume_name(name: str) -> tuple[str, str] | None:
    match = re.fullmatch(r"resume-prepare-([a-f0-9]{32})\.(intent|confirmed|failure)\.json", name)
    return (match.group(1), match.group(2)) if match else None


def _prepare_artifact(name: str) -> bool:
    return (name in {"request.json", "prepare.json", "failure.json"}
            or re.fullmatch(r"[a-z0-9-]+-[a-f0-9]{32}\.command\.json", name) is not None
            or re.fullmatch(r"storage-layout-[a-f0-9]{32}\.json", name) is not None
            or _resume_name(name) is not None)


def _resume_records(backend: Path, output: Path, request_descriptor: dict) -> dict:
    intents, intent_values, terminals = {}, {}, {}
    for path in sorted(output.glob("resume-prepare-*.json")):
        parsed = _resume_name(path.name)
        if parsed is None:
            raise ValueError("fresh 目标包含无法归属的 prepare 续作记录")
        attempt, kind = parsed
        value = read_json(path)
        if kind == "intent":
            exact(value, RESUME_FIELDS)
            if (value["format_version"] != 1 or value["kind"] != "devex-clone-prepare-resume-intent"
                    or value["attempt"] != attempt or value["request"] != request_descriptor
                    or not timestamp(value["at"])
                    or not isinstance(value["files_before"], dict)):
                raise ValueError("fresh 目标 prepare 续作 intent 无效")
            for name, descriptor in value["files_before"].items():
                if not isinstance(name, str) or Path(name).name != name \
                        or binding(output / name) != descriptor:
                    raise ValueError("fresh 目标 prepare 续作前像变化")
            intents[attempt], intent_values[attempt] = binding(path), value
            continue
        expected = {"format_version", "kind", "attempt", "at", "intent", "remote_writes"}
        expected |= ({"prepare", "request"} if kind == "confirmed" else {"error_type", "reason"})
        exact(value, expected)
        if (value["format_version"] != 1 or value["kind"] != f"devex-clone-prepare-resume-{kind}"
                or value["attempt"] != attempt or not timestamp(value["at"])
                or value["remote_writes"] != 0):
            raise ValueError("fresh 目标 prepare 续作结果无效")
        if attempt in terminals:
            raise ValueError("fresh 目标同一 prepare 续作同时存在多个结果")
        terminals[attempt] = (kind, value, binding(path))
    if set(terminals) - set(intents):
        raise ValueError("fresh 目标 prepare 续作结果缺少原 intent")
    causal_failure = None
    if intents and (output / "failure.json").exists():
        causal_failure = read_json(output / "failure.json")
        exact(causal_failure, FAILURE_FIELDS)
        if not timestamp(causal_failure["at"]):
            raise ValueError("fresh 目标既有失败时间无效")
    started_path = output / "initialize.started.json"
    started_at = None
    if intents and started_path.exists():
        started = read_json(started_path)
        exact(started, {"at", "generation_sha256"})
        if not timestamp(started["at"]):
            raise ValueError("fresh 初始化开始时间无效")
        started_at = started["at"]
    for attempt, (_, value, _) in terminals.items():
        intent_value = intent_values[attempt]
        if (value["intent"] != intents[attempt]
                or not ordered(intent_value["at"], value["at"])):
            raise ValueError("fresh 目标 prepare 续作结果不属于原 intent")
    for value in intent_values.values():
        if (causal_failure is not None and not ordered(causal_failure["at"], value["at"])
                or started_at is not None and not ordered(value["at"], started_at)):
            raise ValueError("fresh 目标 prepare 续作时间不属于初始化前因果链")
    if started_at is not None and any(
            not ordered(value[1]["at"], started_at) for value in terminals.values()):
        raise ValueError("fresh 目标 prepare 续作结果晚于初始化开始")
    return {"intents": intents, "terminals": terminals,
            "pending": sorted(set(intents) - set(terminals))}


def _prepare_failure_resolved(backend: Path, output: Path,
                              request_descriptor: dict) -> bool:
    """确认根 failure 是已由显式只读续作收尾的 prepare 失败。"""
    try:
        if _prepare_failure(output) is None or not (output / "prepare.json").is_file():
            return False
        records = _resume_records(backend, output, request_descriptor)
        prepared = binding(output / "prepare.json")
        return not records["pending"] and any(
            value[0] == "confirmed" and value[1]["prepare"] == prepared
            and value[1]["request"] == binding(output / "request.json")
            for value in records["terminals"].values())
    except (AttributeError, IndexError, KeyError, OSError, TypeError, ValueError):
        return False


def initialization_failure_path(backend: Path, output: Path,
                                request_descriptor: dict) -> Path:
    """返回唯一初始化失败；允许其前面存在已收尾的 prepare 失败。"""
    root = output / "failure.json"
    secondary = sorted(path for path in output.glob("failure-*.json")
                       if re.fullmatch(r"failure-[a-f0-9]{32}\.json", path.name))
    all_secondary = sorted(output.glob("failure-*.json"))
    if not root.is_file() or secondary != all_secondary or len(secondary) > 1:
        raise ValueError("fresh 目标没有唯一可归属的初始化失败")
    if not secondary:
        return root
    if not _prepare_failure_resolved(backend, output, request_descriptor):
        raise ValueError("初始化失败之前的 prepare 失败尚未显式收尾")
    return secondary[0]


def prepare_resume_state(backend: Path, output: Path, request_descriptor: dict) -> dict:
    """只读判定 registration/request/prepare 中断能否显式续作。"""
    bound_file(backend, request_descriptor)
    output = local_path(backend, str(output), new=not output.exists())
    if not output.exists():
        return {"resumable": True, "prepared": False, "reason": None,
                "files": {}, "pending_attempts": []}
    if not output.is_dir():
        raise ValueError("fresh 目标 prepare 输出不是普通目录")
    files = {}
    if any((output / name).exists() for name in ("sentinel.intent.json", "reset.intent.json")) \
            or any(output.glob("create-*.intent.json")):
        return {"resumable": False, "prepared": False,
                "reason": "已存在资源写入 intent", "files": files, "pending_attempts": []}
    for item in sorted(output.iterdir()):
        if not item.is_file() or not _prepare_artifact(item.name):
            return {"resumable": False, "prepared": False,
                    "reason": "包含非只读 prepare 证据", "files": files, "pending_attempts": []}
        files[item.name] = file_digest(item)
    request_path = output / "request.json"
    if request_path.exists() and read_json(request_path) != read_bound_json(
            local_path(backend, request_descriptor["path"]), request_descriptor):
        raise ValueError("fresh 目标中断请求不同于固定 registration")
    failure_value = _prepare_failure(output)
    records = _resume_records(backend, output, request_descriptor)
    prepared = output / "prepare.json"
    if prepared.exists():
        value = read_json(prepared)
        exact(value, PREPARE_FIELDS)
        if value["status"] != "fresh_creation_prepared" or value["request"] != request_descriptor:
            raise ValueError("fresh 目标 prepare 结果不属于固定 registration")
        confirmed = [terminal for terminal in records["terminals"].values()
                     if terminal[0] == "confirmed" and terminal[1]["prepare"] == binding(prepared)
                     and terminal[1]["request"] == binding(request_path)]
        if failure_value is not None and not confirmed and not records["pending"]:
            raise ValueError("fresh 目标 prepare 与旧失败之间缺少显式续作绑定")
    elif any(value[0] == "confirmed" for value in records["terminals"].values()):
        raise ValueError("fresh 目标 prepare 续作确认缺少最终 prepare.json")
    return {"resumable": True, "prepared": prepared.exists(), "reason": None,
            "files": files, "pending_attempts": records["pending"]}


def _validate_prepared(backend: Path, request_file: Path, output: Path, run, storage_run: Path | None,
                       request_descriptor: dict) -> dict:
    result = read_json(output / "prepare.json")
    exact(result, PREPARE_FIELDS)
    request = read_bound_json(request_file, request_descriptor)
    original, resources = context(backend, request, output, run, storage_run=storage_run)
    empty = empty_resources(resources)
    unchanged(backend, request, original, resources, run)
    bound_file(backend, request_descriptor)
    if (result["format_version"] != 1 or result["status"] != "fresh_creation_prepared"
            or result["request"] != request_descriptor or result["request_sha256"] != plan_hash(request)
            or result["generation"] != original or result["absence"] != empty):
        raise ValueError("fresh 目标已发布 prepare 与当前只读前像不同")
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
            result = {"format_version": 1, "status": "fresh_creation_prepared", "prepared_at": now(),
                      "request": request_descriptor, "request_sha256": plan_hash(request),
                      "generation": original, "absence": empty, "fresh_target_initialized": False,
                      "controlled_generation_only": True, "external_writers_discovered": False,
                      "create_intents": [{"key": db["key"], "server_uuid": db["server_uuid"],
                                          "database": db["database"]}
                                         for db in request["target"]["databases"]]}
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
                         backend, output, initialized, target_evidence_hooks()))
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


def history_files() -> set[str]:
    stages = ["sentinel", "reset", *("create-" + key for key in KEYS)]
    return {"initialize.started.json", "prepare.json", "request.json", "reset-plan.json", "reset-plan-confirm.json",
            *(f"{stage}.{state}.json" for stage in stages for state in ("intent", "confirmed"))}


def history(output: Path) -> dict:
    names = history_files()
    names.update(path.name for path in output.glob("failure*.json"))
    names.update(path.name for path in output.glob("resume-prepare-*.json"))
    names.update(path.name for path in output.glob("resume-initialize-*.intent.json"))
    names.update(path.name for path in output.glob("resume-initialize-*.started.json"))
    names.update(path.name for path in output.glob("resume-initialize-*.failure.json"))
    names.update(path.name for path in output.glob("resume-migrate-*.json"))
    if (output / "resume-reset-plan.json").exists():
        names.add("resume-reset-plan.json")
    names.update(path.name for path in output.glob("resume-reset-plan-*.json"))
    return {name: file_digest(output / name) for name in sorted(names)}


def verify_initial_inventory(backend: Path, output: Path, receipt: dict, *, resumed: bool = False) -> None:
    filename = bound_file(backend, receipt)
    resumed_directory = filename.parent.name.startswith("inventory-resume-")
    if filename.name != "inventory.json" or (filename.parent != output / "inventory-initial"
                                               and not (resumed and resumed_directory)):
        raise ValueError("初始库存文件不属于本代次")
    value = read_json(filename)
    for name, expected in value["inventories"].items():
        path = local_path(backend, str(filename.parent / name))
        if path.parent != filename.parent or file_digest(path) != expected:
            raise ValueError("原始初始库存文件发生变化")


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
        result = {"format_version": 1, "status": "fresh_creation_prepared", "prepared_at": now(),
                  "request": expected_request, "request_sha256": plan_hash(request),
                  "generation": original, "absence": empty, "fresh_target_initialized": False,
                  "controlled_generation_only": True, "external_writers_discovered": False,
                  "create_intents": [{"key": db["key"], "server_uuid": db["server_uuid"], "database": db["database"]}
                                     for db in request["target"]["databases"]]}
        write_json(output / "prepare.json", result)
        return result
    except BaseException as error:
        failure(output, stage, error)
        raise


def reset_plan(resources: Resources, maintenance: dict, request: dict, original: dict, stage: str) -> tuple[dict, str]:
    env = {**resources.environment, "RYFRAME_CODE_SHA": maintenance["source"]["snapshot"]["head"]}
    raw = resources.command(stage, [cli_executable(maintenance, "reset"), "plan"], env=env).stdout
    text = raw.decode("utf-8").replace("\r\n", "\n")
    match = re.fullmatch(r"(\{[\s\S]*\})\nplan_hash=([a-f0-9]{64})\n?", text)
    if not match or hashlib.sha256(match[1].encode()).hexdigest() != match[2]:
        raise ValueError("实际 reset plan 原文或 hash 缺失、重复、不一致")
    manifest = json.loads(match[1], object_pairs_hook=unique_object)
    validate_reset_manifest(manifest, request, original)
    write_json(resources.output / f"{stage}.json", {"manifest": manifest, "plan_hash": match[2]})
    return manifest, match[2]


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
    state = resources.output / "reset-state"
    state.mkdir()
    env = {**resources.environment, "RYFRAME_CODE_SHA": maintenance["source"]["snapshot"]["head"],
           "RYFRAME_RESET_STATE_DIR": str(state), "RYFRAME_RESET_SERVICES_STOPPED": "YES"}
    unchanged(backend, request, original, resources, run)
    for db in request["target"]["databases"]:
        resources.database_state(db, exists=True, empty=True)
    resources.objects(initialized=False)
    resources.redis_state(initialized=False, sentinel=True)
    intent(resources.output, "reset", {"plan_hash": sha})
    resources.command("reset-execute", [cli_executable(maintenance, "reset"), "execute", "--plan-hash", sha,
                      "--confirm-reset", manifest["confirmation_phrase"]], env=env, timeout=1800)
    completed = reset_evidence.reset_completed(resources.output, manifest, sha)
    confirmed(resources.output, "reset", completed)
    from devex_clone_target_resume_evidence import migration_operations

    for operation in migration_operations(maintenance):
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
    for db in request["target"]["databases"]:
        image = images[db["database"]]
        for table, value in image["tables"].items():
            if table.startswith("biz_") and table not in {"biz_tenant_target_slot", "biz_tenant_fence"} and value["rows"] != 0:
                raise ValueError("fresh 目标已出现业务数据")
        if db["kind"] != "combined" and (image["tables"]["biz_tenant_fence"]["rows"] != 0 or image["tables"]["biz_tenant_target_slot"]["rows"] != 1):
            raise ValueError("外部新租户库 fence 或空槽行不符")
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
            from devex_clone_target_initialization_claim import prepared_baseline, validate_complete
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
                      "inventory": initial, "objects": objects, "redis": redis, "history": history(output),
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
                    or request != read_json(output / "request.json") or original_result["history"] != history(output)):
                raise ValueError("初始化发布或请求证据变化")
            verify_initial_inventory(backend, output, original_result["inventory"]["receipt"],
                                     resumed=(output / "failure.json").is_file())
            original, resources = context(
                backend, request, observation_dir, run, storage_run=storage_run,
                owned_lock_identity=lock_identity, lock_output=output
            )
            verify_storage_generation(original_result["generation"], original, resources.storage_runtime_binding, resources.cache_runtime_binding)
            reset = read_json(output / "reset-plan.json")
            if reset_evidence.reset_completed(output, reset["manifest"], reset["plan_hash"]) != original_result["reset"]:
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
