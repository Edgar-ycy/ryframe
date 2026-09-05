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
from devex_clone_target_binding import KEYS, generation, request_binding, validate_reset_manifest
from devex_clone_target_resources import Resources
from devex_clone_target_state import confirmed, failure, generation_lock, intent, now
from devex_clone_target_storage import verify_storage_generation
from restore_build import file_digest
from restore_reference_io import ExternalTools, redact_object_diagnostic
from restore_reference_plan import plan_hash

PHASES = {"preflight", "object_storage", "redis", "databases", "control_baseline", "tenant_baselines", "verification", "release"}
PREPARE_STAGES = {"request", "prepare_binding", "prepare_absence"}
PREPARE_FIELDS = {"format_version", "status", "prepared_at", "request", "request_sha256",
                  "generation", "absence", "fresh_target_initialized", "controlled_generation_only",
                  "external_writers_discovered", "create_intents"}
FAILURE_FIELDS = {"status", "stage", "at", "error_type", "reason", "automatic_retry",
                  "automatic_resource_cleanup", "fresh_target_initialized", "clone_verified",
                  "restore_qualified"}
RESUME_FIELDS = {"format_version", "kind", "attempt", "at", "request", "files_before"}


def _prepare_failure(output: Path) -> dict | None:
    path = output / "failure.json"
    if not path.exists():
        return None
    value = read_json(path)
    exact(value, FAILURE_FIELDS)
    if (value["status"] != "needs_reconciliation" or not isinstance(value["stage"], str)
            or value["stage"] not in PREPARE_STAGES
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
    intents, terminals = {}, {}
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
                    or not isinstance(value["files_before"], dict)):
                raise ValueError("fresh 目标 prepare 续作 intent 无效")
            for name, descriptor in value["files_before"].items():
                if not isinstance(name, str) or Path(name).name != name \
                        or binding(output / name) != descriptor:
                    raise ValueError("fresh 目标 prepare 续作前像变化")
            intents[attempt] = binding(path)
            continue
        expected = {"format_version", "kind", "attempt", "at", "intent", "remote_writes"}
        expected |= ({"prepare", "request"} if kind == "confirmed" else {"error_type", "reason"})
        exact(value, expected)
        if (value["format_version"] != 1 or value["kind"] != f"devex-clone-prepare-resume-{kind}"
                or value["attempt"] != attempt or value["remote_writes"] != 0):
            raise ValueError("fresh 目标 prepare 续作结果无效")
        if attempt in terminals:
            raise ValueError("fresh 目标同一 prepare 续作同时存在多个结果")
        terminals[attempt] = (kind, value, binding(path))
    if set(terminals) - set(intents):
        raise ValueError("fresh 目标 prepare 续作结果缺少原 intent")
    for attempt, (_, value, _) in terminals.items():
        if value["intent"] != intents[attempt]:
            raise ValueError("fresh 目标 prepare 续作结果不属于原 intent")
    return {"intents": intents, "terminals": terminals,
            "pending": sorted(set(intents) - set(terminals))}


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
    """成功续作只解除旧 prepare 失败；任何初始化失败仍保持 fail closed。"""
    failures = sorted(output.glob("failure*.json"))
    if not failures:
        return False
    if any(path.name != "failure.json" for path in failures):
        return True
    try:
        if _prepare_failure(output) is None or not (output / "prepare.json").is_file():
            return True
        prepared_value = read_json(output / "prepare.json")
        exact(prepared_value, PREPARE_FIELDS)
        prepared = binding(output / "prepare.json")
        records = _resume_records(backend, output, prepared_value["request"])
        return not any(value[0] == "confirmed" and value[1]["prepare"] == prepared
                       and value[1]["request"] == binding(output / "request.json")
                       for value in records["terminals"].values())
    except (OSError, TypeError, ValueError):
        return True


def history_files() -> set[str]:
    stages = ["sentinel", "reset", *("create-" + key for key in KEYS)]
    return {"initialize.started.json", "prepare.json", "request.json", "reset-plan.json", "reset-plan-confirm.json",
            *(f"{stage}.{state}.json" for stage in stages for state in ("intent", "confirmed"))}


def history(output: Path) -> dict:
    names = history_files()
    if (output / "failure.json").exists():
        names.add("failure.json")
    names.update(path.name for path in output.glob("resume-prepare-*.json"))
    return {name: file_digest(output / name) for name in sorted(names)}


def verify_initial_inventory(backend: Path, output: Path, receipt: dict) -> None:
    filename = bound_file(backend, receipt)
    if filename != output / "inventory-initial/inventory.json":
        raise ValueError("初始库存文件不属于本代次")
    value = read_json(filename)
    for name, expected in value["inventories"].items():
        path = local_path(backend, str(filename.parent / name))
        if path.parent != filename.parent or file_digest(path) != expected:
            raise ValueError("原始初始库存文件发生变化")


def context(backend: Path, request: dict, output: Path, run, *, storage_run: Path | None = None,
            owned_lock_identity: int | None = None) -> tuple[dict, Resources]:
    original = generation(backend, request, run)
    review, selected = request_binding(backend, request)
    resources = Resources(backend, request, output, selected, review, run, storage_run=storage_run,
                          owned_lock_identity=owned_lock_identity)
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


def reset_completed(output: Path, manifest: dict, sha: str) -> dict:
    stem = f"test-{manifest['scope_id']}-{sha}"
    directory = output / "reset-state"
    report_path, ledger_path = directory / (stem + ".report.json"), directory / (stem + ".ledger.json")
    file_digest(report_path)
    file_digest(ledger_path)
    report, ledger = read_json(report_path), read_json(ledger_path)
    identity = {key: manifest[key] for key in ("environment", "scope_id", "code_sha", "config_sha", "credential_version")}
    for value in (report, ledger):
        if value.get("plan_hash") != sha or any(value.get(key) != item for key, item in identity.items()):
            raise ValueError("reset 报告或账本不属于当前精确 plan")
        if set(value["phases"]) != PHASES or any(item["status"] != "complete" or not item["completed_at"] for item in value["phases"].values()):
            raise ValueError("reset 有未完成阶段或锁释放证据缺失")
        if not value["resources"] or any(item["status"] != "complete" for item in value["resources"].values()):
            raise ValueError("reset 存在未确认资源")
    if (report.get("report_version") != 2 or ledger.get("ledger_version") != 4
            or report.get("status") != "completed" or report.get("failed_phase") is not None
            or report.get("completed_at") != ledger["phases"]["release"]["completed_at"]
            or report["phases"] != ledger["phases"] or report["resources"] != ledger["resources"]):
        raise ValueError("fresh 初始化必须本次 completed；reused、失败或释放中断均拒绝")
    if set(report["databases"]) != {f"{db['host']}:{db['port']}/{db['database']}" for db in manifest["databases"]}:
        raise ValueError("reset 完成报告数据库集合不符")
    if (report["redis_namespace"] != manifest["redis"]["namespace"]
            or set(report["object_prefixes"]) != {item["bucket"] + ":" + item["prefix"] for item in manifest["object_storage"]["prefixes"]}):
        raise ValueError("reset 完成报告 Redis 或对象集合不符")
    return {"report": {"file": str(report_path.relative_to(output)), **file_digest(report_path)},
            "ledger": {"file": str(ledger_path.relative_to(output)), **file_digest(ledger_path)},
            "completed_at": report["completed_at"], "status": "completed"}


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
    completed = reset_completed(resources.output, manifest, sha)
    confirmed(resources.output, "reset", completed)
    for args in (["control"], *(["tenant-data", "--target", key] for key in sorted(KEYS))):
        for operation in ("up", "verify"):
            unchanged(backend, request, original, resources, run)
            command = [cli_executable(maintenance, "migrate"), args[0], operation, *args[1:]]
            resources.command("migrate-" + "-".join(args) + "-" + operation, command, timeout=1800)
    return completed


def inventory(backend: Path, request: dict, resources: Resources, output: Path) -> dict:
    side = {**request["target"], "runtime_dir": resources.selected["runtime_dir"], "api_url": resources.selected["api_url"]}
    tools = ExternalTools({"target": side, "tools": request["tools"]}, resources.output, resources.runner)
    captured = capture_side_inventory(backend, "target", tools, bound_file(backend, request["maintenance_build"]),
                                      output, environment=resources.environment)
    images = {item.resource["database"]: json.loads(json.dumps(asdict(item))) for item in captured.observations}
    for db in request["target"]["databases"]:
        image = images[db["database"]]
        for table, value in image["tables"].items():
            if table.startswith("biz_") and table not in {"biz_tenant_target_slot", "biz_tenant_fence"} and value["rows"] != 0:
                raise ValueError("fresh 目标已出现业务数据")
        if db["kind"] != "combined" and (image["tables"]["biz_tenant_fence"]["rows"] != 0 or image["tables"]["biz_tenant_target_slot"]["rows"] != 1):
            raise ValueError("外部新租户库 fence 或空槽行不符")
    return {"receipt": captured.receipt_file, "observations": images}


def initialize_target(backend: Path, output: Path, run=subprocess.run, *, storage_run: Path | None = None,
                      request_descriptor: dict | None = None) -> dict:
    """仅首次调用可写；中断、失败、已初始化均不自动重放。"""
    backend = backend.resolve(strict=True)
    output = local_path(backend, str(output))
    if unresolved_failure(backend, output) or (output / "initialize.started.json").exists():
        raise ValueError("初始化已有尝试；只能只读核对，禁止重放或自动清理")
    stage, claimed = "initialize_binding", False
    try:
        with generation_lock(output):
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
            original, resources = context(backend, request, output, run, storage_run=storage_run)
            if prepared["generation"] != original or prepared["absence"] != empty_resources(resources):
                raise ValueError("准备后来源或目标缺失状态变化")
            write_json(output / "initialize.started.json", {"at": now(), "generation_sha256": plan_hash(original)})
            stage = "create_and_reset"
            reset = initialize_databases(backend, request, original, resources, run)
            stage = "inventory"
            initial = inventory(backend, request, resources, output / "inventory-initial")
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
        stage = "publish_after_lock_release"
        write_json(output / "initialized.json", result)
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
            verify_initial_inventory(backend, output, original_result["inventory"]["receipt"])
            original, resources = context(backend, request, observation_dir, run, storage_run=storage_run,
                                          owned_lock_identity=lock_identity)
            verify_storage_generation(original_result["generation"], original, resources.storage_runtime_binding, resources.cache_runtime_binding)
            reset = read_json(output / "reset-plan.json")
            if reset_completed(output, reset["manifest"], reset["plan_hash"]) != original_result["reset"]:
                raise ValueError("原 reset 完成证据已变化")
            observed = inventory(backend, request, resources, observation_dir / "inventory")
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
