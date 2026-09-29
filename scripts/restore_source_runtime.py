"""在同一 source-generation 锁内复验 C52 派生来源并发布严格运行收据。"""

from __future__ import annotations

import copy
import datetime as dt
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import uuid

from devex_clone_capture import read_json, write_json
from process_environment import Environments, configured
from devex_clone_model import linked
from devex_clone_run_state import binding
from devex_clone_source_proof import bound_file
from restore_reference_io import ExternalTools
from restore_reference_plan import identifier, plan_hash
from restore_runtime_evidence import artifact_snapshot, exact_fields, timestamp
from restore_source_runtime_model import (
    authorization_cache_plan,
    image_write_effects,
    validate_login_audit,
    validate_node_result,
)
from restore_source_runtime_producer import (
    COMPLETION,
    INTENT,
    PROCESS,
    READY,
    STDERR,
    STDOUT,
    run_source_producer,
    verify_source_producer,
)
from restore_source_runtime_staging import DIRECTORY as STAGING_DIRECTORY


FORMAT_VERSION = 3
RECEIPT_FIELDS = {
    "format_version", "kind", "status", "source_generation", "dataset_lineage",
    "before", "after", "producer", "verification", "login_audit", "cache_cleanup",
    "evidence", "write_effects", "started_at", "verified_at", "remote_writes",
    "restore_qualified",
}
RECOVERED_RECEIPT_FIELDS = RECEIPT_FIELDS | {"recovery"}
PRODUCER_BINDING_FIELDS = {"intent", "process", "ready", "stdout", "stderr", "completion"}
ROOT_ENTRIES = {
    "before", "after", "audit", STAGING_DIRECTORY, INTENT, PROCESS, READY, STDOUT, STDERR, COMPLETION,
    "cache-cleanup.json", "source-runtime.json",
}
RECOVERY_INTENT = "source-verification-recovery-intent.json"
RECOVERY_RECEIPT = "source-verification-recovery.json"
RECOVERY_AUDIT = "recovery-audit"
RECOVERED_ROOT_ENTRIES = ROOT_ENTRIES | {"failed.json", RECOVERY_INTENT, RECOVERY_RECEIPT, RECOVERY_AUDIT}
FAILED_ROOT_ENTRIES = {
    "before", "audit", STAGING_DIRECTORY, INTENT, PROCESS, READY, STDOUT, STDERR, COMPLETION,
    "failed.json",
}
RECOVERY_INTENT_FIELDS = {
    "format_version", "kind", "status", "source_generation", "dataset_lineage", "failure",
    "producer", "login_audit", "before", "pre_recovery_manifest", "successor",
    "authorization_cache", "started_at",
}
RECOVERY_RECEIPT_FIELDS = {
    "format_version", "kind", "status", "intent", "cache_cleanup", "after",
    "historical_cleanup", "confirmed_cleanup", "login_recheck", "completed_at",
}
AUTHORIZATION_CACHE_CLEANUP_SCRIPT = """local expired = {}
for index = 0, 10 do
  local key_offset = index * 3
  local argument_offset = index * 5
  local tenant_key = KEYS[key_offset + 1]
  local user_key = KEYS[key_offset + 2]
  local snapshot_key = KEYS[key_offset + 3]
  if redis.call('TYPE', tenant_key)['ok'] ~= 'string'
      or redis.call('GET', tenant_key) ~= ARGV[argument_offset + 1]
      or redis.call('TYPE', user_key)['ok'] ~= 'string'
      or redis.call('GET', user_key) ~= ARGV[argument_offset + 2] then
    return redis.error_reply('authorization persistent cache compare failed')
  end
  local expected_type = ARGV[argument_offset + 3]
  local snapshot_type = redis.call('TYPE', snapshot_key)['ok']
  if snapshot_type ~= expected_type then
    return redis.error_reply('authorization snapshot cache type changed')
  elseif snapshot_type == 'none' then
    table.insert(expired, snapshot_key)
  elseif snapshot_type == 'hash' then
    if redis.call('HLEN', snapshot_key) ~= 1
        or redis.call('HGET', snapshot_key, ARGV[argument_offset + 4]) ~= ARGV[argument_offset + 5] then
      return redis.error_reply('authorization snapshot cache compare failed')
    end
  else
    return redis.error_reply('authorization snapshot cache type changed')
  end
end
local deleted = redis.call('UNLINK', unpack(KEYS))
return {deleted, expired}"""
LOGIN_FIELDS = (
    "id", "tenant_id", "user_name", "ipaddr", "login_location", "browser", "os",
    "status", "msg", "login_time",
)


def _timestamp() -> str:
    return dt.datetime.now(dt.timezone.utc).isoformat()


def _new_file(path: Path, content: bytes) -> dict:
    with path.open("xb") as stream:
        stream.write(content)
        stream.flush()
        os.fsync(stream.fileno())
    return artifact_snapshot(path).descriptor()


def _hex_text(value: str, nullable: bool) -> str | None:
    if nullable and value == "NULL":
        return None
    if not isinstance(value, str) or len(value) % 2 or re.fullmatch(r"[A-F0-9]*", value) is None:
        raise ValueError("来源登录日志包含无效十六进制文本")
    try:
        return bytes.fromhex(value).decode("utf-8")
    except (ValueError, UnicodeDecodeError) as error:
        raise ValueError("来源登录日志文本不是有效 UTF-8") from error


def _login_rows(raw: str) -> list[dict]:
    result = []
    for line in raw.splitlines() if raw else []:
        columns = line.split("\t")
        if len(columns) != len(LOGIN_FIELDS) or re.fullmatch(r"[1-9][0-9]{0,18}", columns[0]) is None:
            raise ValueError("来源登录日志查询返回无效列或主键")
        values = [columns[0], *[_hex_text(value, index in {4, 5, 6, 8})
                                for index, value in enumerate(columns[1:-1], 1)], columns[-1]]
        result.append(dict(zip(LOGIN_FIELDS, values, strict=True)))
    if result != sorted(result, key=lambda row: int(row["id"])) or len({row["id"] for row in result}) != len(result):
        raise ValueError("来源登录日志查询没有按唯一主键排序")
    return result


def _login_query(resources, database: dict, condition: str) -> str:
    name = identifier(database["database"])
    sql = (
        "SELECT CAST(`id` AS CHAR),HEX(`tenant_id`),HEX(`user_name`),HEX(`ipaddr`),"
        "IFNULL(HEX(`login_location`),'NULL'),IFNULL(HEX(`browser`),'NULL'),"
        "IFNULL(HEX(`os`),'NULL'),HEX(`status`),IFNULL(HEX(`msg`),'NULL'),"
        "DATE_FORMAT(`login_time`,'%Y-%m-%dT%H:%i:%s.%f') "
        f"FROM `{name}`.`sys_login_info` {condition} ORDER BY `id`;"
    )
    return resources._mysql(database, sql)


def _login_before(resources, database: dict, audit: Path) -> tuple[bytes, int, dict]:
    raw = _login_query(resources, database, "")
    rows = _login_rows(raw)
    content = (raw + ("\n" if raw else "")).encode()
    descriptor = _new_file(audit / "login-before.tsv", content)
    return content, max((int(row["id"]) for row in rows), default=0), descriptor


def _login_after(
    resources,
    database: dict,
    audit: Path,
    old: bytes,
    upper: int,
    lineage: dict,
    subjects: list[dict],
    started_at: str,
) -> dict:
    previous = _login_query(resources, database, f"WHERE `id` <= {upper}")
    added = _login_query(resources, database, f"WHERE `id` > {upper}")
    previous_bytes = (previous + ("\n" if previous else "")).encode()
    old_descriptor = _new_file(audit / "login-after-old.tsv", previous_bytes)
    new_descriptor = _new_file(audit / "login-after-new.tsv", (added + ("\n" if added else "")).encode())
    completed_at = _timestamp()
    verified = validate_login_audit(
        old, previous_bytes, upper, _login_rows(added), lineage, subjects, started_at, completed_at
    )
    receipt = {
        "format_version": 1,
        "kind": "restore-source-login-audit",
        "status": "verified",
        "before_rows": binding(audit / "login-before.tsv"),
        "after_old_rows": old_descriptor,
        "after_new_rows": new_descriptor,
        "verified": verified,
        "completed_at": completed_at,
    }
    write_json(audit / "login-audit.json", receipt)
    return receipt


def _snapshot_identity(payload: str, subject: dict, tenant_epoch: str) -> dict:
    try:
        value = json.loads(payload)
    except json.JSONDecodeError as error:
        raise ValueError("来源授权快照不是有效 JSON") from error
    exact_fields(value, {"versions", "tenant_session_version", "principal"}, "来源授权快照")
    versions = exact_fields(
        value["versions"], {"tenant_authorization_epoch", "user_authorization_version"},
        "来源授权快照版本",
    )
    principal = exact_fields(
        value["principal"],
        {"actor", "tenant_authorization_epoch", "preferred_locale", "roles", "role_ids",
         "permissions", "tenant_request_limit_per_minute"},
        "来源授权快照主体",
    )
    actor = exact_fields(
        principal["actor"],
        {"user_id", "tenant_id", "username", "dept_id", "dept_path", "data_scope",
         "custom_dept_ids", "include_self", "is_super_admin"},
        "来源授权快照 actor",
    )
    if (
        str(actor["user_id"]) != subject["user_id"]
        or actor["tenant_id"] != subject["tenant_id"]
        or versions["tenant_authorization_epoch"] != int(tenant_epoch)
        or versions["user_authorization_version"] != subject["user_authorization_version"]
        or principal["tenant_authorization_epoch"] != int(tenant_epoch)
    ):
        raise ValueError("来源授权快照与登录主体或版本不同")
    encoded = payload.encode()
    return {"bytes": len(encoded), "sha256": hashlib.sha256(encoded).hexdigest()}


def _cache_keys(plan: list[dict]) -> list[str]:
    # JSON 保存会排序对象字段；Redis 的三键顺序必须来自角色合同。
    return [item["keys"][role] for item in plan
            for role in ("tenant_epoch", "user_version", "snapshot")]


def _cache_before(resources, plan: list[dict]) -> tuple[list[dict], list[str]]:
    result, comparisons = [], []
    for item in plan:
        keys = item["keys"]
        types = {name: resources._redis(["TYPE", key]) for name, key in keys.items()}
        if (
            types.get("tenant_epoch") != "string"
            or types.get("user_version") != "string"
            or types.get("snapshot") not in {"hash", "none"}
        ):
            raise ValueError("来源授权缓存持久键缺失或快照类型无效")
        tenant_epoch = resources._redis(["GET", keys["tenant_epoch"]])
        user_version = resources._redis(["GET", keys["user_version"]])
        if (
            not isinstance(tenant_epoch, str)
            or re.fullmatch(r"[1-9][0-9]{0,9}", tenant_epoch) is None
            or not isinstance(user_version, str)
            or user_version != str(item["user_authorization_version"])
        ):
            raise ValueError("来源授权缓存持久版本不同")
        snapshot = {"key": keys["snapshot"], "type": types["snapshot"]}
        field, payload = "", ""
        if types["snapshot"] == "hash":
            fields = resources._redis(["HKEYS", keys["snapshot"]])
            field = f"{tenant_epoch}:{user_version}"
            if fields != [field]:
                raise ValueError("来源授权缓存快照字段不同")
            payload = resources._redis(["HGET", keys["snapshot"], field])
            if not isinstance(payload, str):
                raise ValueError("来源授权缓存快照为空")
            snapshot.update(field=field, **_snapshot_identity(payload, item, tenant_epoch))
        result.append({
            **{key: item[key] for key in ("tenant_id", "user_id", "user_authorization_version")},
            "tenant_epoch": {"key": keys["tenant_epoch"], "value": tenant_epoch},
            "user_version": {"key": keys["user_version"], "value": user_version},
            "snapshot": snapshot,
        })
        comparisons.extend([tenant_epoch, user_version, types["snapshot"], field, payload])
    return result, comparisons


def _cache_observation(resources, namespace: str, subjects: list[dict]) -> tuple[dict, list[str]]:
    plan = authorization_cache_plan(namespace, subjects)
    before, comparisons = _cache_before(resources, plan)
    return {
        "namespace": namespace,
        "subjects": copy.deepcopy(subjects),
        "plan": plan,
        "before": before,
    }, comparisons


def _publish_cache_cleanup(
    resources, observation: dict, output: Path, deleted: int | None, expired: list[str], execution: str
) -> dict:
    plan = observation["plan"]
    keys = _cache_keys(plan)
    planned_snapshots = [item["keys"]["snapshot"] for item in plan]
    expired_set = set(expired)
    if (
        len(expired_set) != len(expired)
        or expired != [key for key in planned_snapshots if key in expired_set]
        or expired != [item["snapshot"]["key"] for item in observation["before"]
                       if item["snapshot"]["type"] == "none"]
        or (execution == "confirmed_lua_result"
            and (type(deleted) is not int or deleted + len(expired) != len(keys)))
        or (execution == "reconciled_unknown_lua_result" and deleted is not None)
        or execution not in {"confirmed_lua_result", "reconciled_unknown_lua_result"}
    ):
        raise ValueError("来源授权缓存原子清理计数或过期快照不同")
    after = [{"key": key, "type": resources._redis(["TYPE", key])} for key in keys]
    if any(item["type"] != "none" for item in after):
        raise ValueError("来源授权缓存没有精确删除本次 33 个键")
    receipt = {
        "format_version": 2,
        "kind": "restore-source-authorization-cache-cleanup",
        "status": "cleaned",
        "namespace": observation["namespace"],
        "before": observation["before"],
        "deleted": deleted,
        "expired_snapshot_keys": expired,
        "after": after,
        "execution": execution,
        "cleaned_at": _timestamp(),
    }
    write_json(output, receipt)
    return receipt


def _execute_cache_cleanup(
    resources, observation: dict, comparisons: list[str], output: Path, *, checkpoint=None
) -> dict:
    keys = _cache_keys(observation["plan"])
    if checkpoint is not None:
        checkpoint()
    result = resources._redis([
        "EVAL", AUTHORIZATION_CACHE_CLEANUP_SCRIPT, str(len(keys)), *keys, *comparisons,
    ])
    if checkpoint is not None:
        checkpoint()
    if (
        not isinstance(result, list)
        or len(result) != 2
        or type(result[0]) is not int
        or not isinstance(result[1], list)
        or any(not isinstance(key, str) for key in result[1])
    ):
        raise ValueError("来源授权缓存原子清理返回无效")
    return _publish_cache_cleanup(
        resources, observation, output, result[0], result[1], "confirmed_lua_result"
    )


def _clean_authorization_cache(resources, namespace: str, subjects: list[dict], output: Path) -> dict:
    observation, comparisons = _cache_observation(resources, namespace, subjects)
    return _execute_cache_cleanup(resources, observation, comparisons, output)


def _manifest(root: Path) -> list[dict]:
    rows = []
    if not root.is_dir() or linked(root):
        raise ValueError("来源验收证据目录不是普通目录")
    for path in sorted(root.rglob("*"), key=lambda item: item.relative_to(root).as_posix()):
        if linked(path):
            raise ValueError("来源验收证据包含链接或重解析点")
        if path.is_dir():
            try:
                next(path.iterdir())
            except StopIteration as error:
                raise ValueError("来源验收证据包含空目录") from error
            continue
        if not path.is_file():
            raise ValueError("来源验收证据包含非普通文件")
        snapshot = artifact_snapshot(path)
        rows.append({"path": path.relative_to(root).as_posix(), "bytes": snapshot.bytes,
                     "sha256": snapshot.sha256})
    if not rows:
        raise ValueError("来源验收证据目录不能为空")
    return rows


def _resources(backend: Path, facts: dict, output: Path, run):
    from devex_clone_target_binding import redis_configuration
    from devex_clone_target_resources import Resources

    target = facts["source"]["seed_target"]
    review_binding = {key: value for key, value in target["review"].items() if key != "canonical_sha256"}
    review = read_json(bound_file(backend, review_binding))
    selected = review["scopes"]["seed"]
    redis_configuration(target, selected)
    return Resources(
        backend,
        target,
        output,
        selected,
        review,
        run,
        execution_backend=facts["execution"],
        storage_run=facts["directory"],
    )


def _control_database(facts: dict) -> dict:
    databases = facts["source"]["seed_target"]["target"]["databases"]
    matches = [item for item in databases if item.get("key") == "shared-control"]
    if len(matches) != 1 or matches[0].get("kind") != "combined":
        raise ValueError("来源验收缺少唯一控制数据库")
    return matches[0]


def _validate_paths(backend: Path, start: Path, output: Path) -> tuple[Path, Path]:
    backend = backend.resolve(strict=True)
    local = (backend / ".local-tests").resolve(strict=True)
    for path, label in ((start, "source generation"), (output, "来源运行收据")):
        if not path.is_absolute() or not path.resolve().is_relative_to(local) or linked(path):
            raise ValueError(f"{label}必须位于当前后端忽略目录且不能经过链接")
    if output.name != "source-runtime.json" or output.parent.name != "verification":
        raise ValueError("来源运行收据必须使用同代 verification/source-runtime.json")
    return start.resolve(strict=True), output


def _producer_bindings(directory: Path) -> dict:
    return {
        "intent": binding(directory / INTENT),
        "process": binding(directory / PROCESS),
        "ready": binding(directory / READY),
        "stdout": binding(directory / STDOUT),
        "stderr": artifact_snapshot(directory / STDERR).descriptor(),
        "completion": binding(directory / COMPLETION),
    }


def _remote_write_receipt(cleanup: dict, *, recovered: bool) -> dict:
    expired = cleanup["expired_snapshot_keys"]
    reconciled = cleanup["execution"] == "reconciled_unknown_lua_result"
    return {
        "api_session_commands": 22,
        "authorization_cache": {
            "confirmed_commands": 0 if reconciled else 1,
            "unknown_result_commands": None if recovered else 0,
            "phase": "recovery" if recovered else "initial_verification",
            "historical_cleanup": "unknown" if recovered else "not_applicable",
            "deleted_keys": cleanup["deleted"],
            "expired_snapshot_keys": len(expired),
            "observed_absent_keys": len(cleanup["after"]),
        },
        "confirmed_total_commands": 22 + (0 if reconciled else 1),
    }


def _source_runtime_receipt(
    directory: Path,
    facts: dict,
    start: dict,
    started_at: str,
    before: dict,
    after: dict,
    effects: dict,
    cleanup: dict,
    *,
    recovery: dict | None,
) -> dict:
    receipt = {
        "format_version": FORMAT_VERSION,
        "kind": "restore-source-runtime",
        "status": "source_runtime_verified",
        "source_generation": start,
        "dataset_lineage": facts["receipt"]["dataset_lineage"],
        "before": before,
        "after": after,
        "producer": _producer_bindings(directory),
        "verification": binding(directory / STDOUT),
        "login_audit": binding(directory / "audit/login-audit.json"),
        "cache_cleanup": binding(directory / "cache-cleanup.json"),
        "evidence": {
            "before": _manifest(directory / "before"),
            "after": _manifest(directory / "after"),
            "audit": _manifest(directory / "audit"),
        },
        "write_effects": effects,
        "started_at": started_at,
        "verified_at": _timestamp(),
        "remote_writes": _remote_write_receipt(cleanup, recovered=recovery is not None),
        "restore_qualified": False,
    }
    if recovery is not None:
        receipt["recovery"] = recovery
    return receipt


def execute_source_verification(
    backend: Path,
    start_path: Path,
    output: Path,
    *,
    run=subprocess.run,
    popen=subprocess.Popen,
) -> dict:
    """唯一写入口；参数先验证，再在 source-generation 全局锁内执行。"""
    start_path, output = _validate_paths(backend, start_path, output)
    from devex_clone_seed_generation import registered_running_source, verify_running_source
    from devex_clone_seed_generation_images import capture_image, verify_image

    start = binding(start_path)
    initial = verify_running_source(backend, start, live=True)
    control_environments = Environments(dict(os.environ), configured(initial["environment"]["environment"]))

    def control_environment():
        if dict(os.environ) not in control_environments.values.values():
            raise ValueError("源验证的控制或服务环境在检查前变化")
        return control_environments.use("source")
    expected_directory = initial["output"] / "verification"
    if output.parent != expected_directory or expected_directory.exists():
        raise ValueError("来源验收必须使用尚不存在的同代 verification 目录")
    started_at = _timestamp()
    try:
        with registered_running_source(backend, start) as checkpoint:
            facts = checkpoint()
            if facts["output"] / "verification" != expected_directory:
                raise ValueError("取得控制锁后 source generation 代次变化")
            expected_directory.mkdir()
            (expected_directory / "audit").mkdir()
            if dict(os.environ) != control_environments.values["source"]:
                raise ValueError("源验证控制环境在安装服务配置前变化")
            environment = configured(facts["environment"]["environment"])
            with Environments(environment, {}).use("source"):
                resources = _resources(backend, facts, expected_directory / "audit", run)
                database = _control_database(facts)
                old_rows, upper_id, _ = _login_before(
                    resources, database, expected_directory / "audit"
                )
                checkpoint()
                before = capture_image(
                    backend, facts["execution"], facts["selected"], facts["request"],
                    facts["source"], environment, expected_directory / "before", run,
                    control_environment=control_environment,
                )
                before_value = verify_image(
                    backend, before, facts["selected"], facts["source"]["request"],
                    source_registration=facts["receipt"]["source_registration"],
                )
                if before_value["image"] != facts["running"]["image"]:
                    raise ValueError("来源验收开始前完整像与同一运行 start 不同")
                checkpoint()
                node = Path(
                    ExternalTools(facts["source"]["request"], expected_directory)
                    .command("node")[0]
                ).resolve(strict=True)
                operation = uuid.uuid4().hex
                node_result, _ = run_source_producer(
                    backend, facts["execution"], expected_directory, operation, node, start,
                    facts["receipt"]["dataset_lineage"], environment,
                    coordinator_source=facts["coordinator_source"],
                    execution_sha=facts["request"]["expected_backend_sha"], popen=popen,
                )
                verified_node = validate_node_result(
                    node_result, facts["lineage"], start, facts["receipt"]["dataset_lineage"]
                )
                checkpoint()
                audit = _login_after(
                    resources, database, expected_directory / "audit", old_rows, upper_id,
                    facts["lineage"], verified_node["subjects"], started_at,
                )
                cleanup = _clean_authorization_cache(
                    resources, resources.selected["redis"]["namespace"],
                    verified_node["subjects"], expected_directory / "cache-cleanup.json",
                )
                checkpoint()
                after = capture_image(
                    backend, facts["execution"], facts["selected"], facts["request"],
                    facts["source"], environment, expected_directory / "after", run,
                    control_environment=control_environment,
                )
                after_value = verify_image(
                    backend, after, facts["selected"], facts["source"]["request"],
                    source_registration=facts["receipt"]["source_registration"],
                )
            effects = image_write_effects(before_value["image"], after_value["image"])
            if (
                cleanup["deleted"] + len(cleanup["expired_snapshot_keys"])
                != effects["authorization_cache"]["created_and_removed_keys"]
            ):
                raise ValueError("来源验收授权缓存清理与完整前后像合同不同")
            receipt = _source_runtime_receipt(
                expected_directory, facts, start, started_at, before, after, effects, cleanup,
                recovery=None,
            )
            checkpoint()
            write_json(output, receipt)
            verified = verify_source_runtime(backend, binding(output), live=True)
            if verified["receipt"] != receipt:
                raise ValueError("来源运行收据写入后复核不同")
            checkpoint()
            return {"output": str(output), "status": receipt["status"], "restore_success": False}
    except BaseException as error:
        failed = output.parent / "failed.json"
        if output.parent.is_dir() and not failed.exists():
            write_json(failed, {"status": "failed", "error": type(error).__name__})
        raise


def _validate_stored_login_audit(
    backend: Path, directory: Path, descriptor: dict, lineage: dict, subjects: list[dict], started_at: str
) -> dict:
    audit = _read_login_audit(backend, directory, descriptor)
    old = (directory / "audit/login-before.tsv").read_bytes()
    old_after = (directory / "audit/login-after-old.tsv").read_bytes()
    added = (directory / "audit/login-after-new.tsv").read_text(encoding="utf-8").strip()
    verified = validate_login_audit(
        old, old_after, int(audit["verified"]["upper_id"]), _login_rows(added), lineage,
        subjects, started_at, audit["completed_at"],
    )
    if audit["verified"] != verified:
        raise ValueError("来源登录审计派生结果发生变化")
    return audit


def _verify_recovery_prefix(backend: Path, directory: Path, facts: dict) -> dict:
    from devex_clone_seed_generation_images import verify_image

    names = frozenset(entry.name for entry in directory.iterdir())
    if names not in {
        frozenset(FAILED_ROOT_ENTRIES), frozenset(FAILED_ROOT_ENTRIES | {RECOVERY_INTENT}),
        frozenset(FAILED_ROOT_ENTRIES | {RECOVERY_INTENT, RECOVERY_AUDIT}),
    }:
        raise ValueError("来源验收恢复只接受未清理、未采集后像且未封口的精确失败前缀")
    failure = read_json(directory / "failed.json")
    exact_fields(failure, {"status", "error"}, "来源验收原失败记录")
    if failure != {"status": "failed", "error": "ValueError"}:
        raise ValueError("来源验收恢复只接受缓存阶段 ValueError 失败记录")
    environment = configured(facts["environment"]["environment"])
    node = Path(ExternalTools(facts["source"]["request"], directory).command("node")[0]).resolve(strict=True)
    producer, stdout = verify_source_producer(
        backend, facts["execution"], directory, facts["start_descriptor"],
        facts["receipt"]["dataset_lineage"], environment, node,
        coordinator_source=facts["coordinator_source"],
        execution_sha=facts["request"]["expected_backend_sha"], historical_coordinator=True,
    )
    verified_node = validate_node_result(
        stdout, facts["lineage"], facts["start_descriptor"], facts["receipt"]["dataset_lineage"]
    )
    producer_intent = read_json(directory / INTENT)
    audit = _validate_stored_login_audit(
        backend, directory, binding(directory / "audit/login-audit.json"), facts["lineage"],
        verified_node["subjects"], producer_intent["started_at"],
    )
    before_descriptor = binding(directory / "before/image.json")
    before = verify_image(
        backend, before_descriptor, facts["selected"], facts["source"]["request"],
        source_registration=facts["receipt"]["source_registration"],
    )
    if before["image"] != facts["running"]["image"]:
        raise ValueError("来源验收恢复前像与同一 start 运行像不同")
    return {"producer": producer, "producer_intent": producer_intent, "node": verified_node,
            "audit": audit, "before": before, "before_descriptor": before_descriptor,
            "environment": environment}


def _recovery_intent(
    directory: Path, facts: dict, prefix: dict, successor: dict, pre_manifest: list[dict],
    authorization_cache: dict,
) -> dict:
    return {
        "format_version": 1,
        "kind": "restore-source-verification-recovery-intent",
        "status": "recovering",
        "source_generation": facts["start_descriptor"],
        "dataset_lineage": facts["receipt"]["dataset_lineage"],
        "failure": binding(directory / "failed.json"),
        "producer": _producer_bindings(directory),
        "login_audit": binding(directory / "audit/login-audit.json"),
        "before": prefix["before_descriptor"],
        "pre_recovery_manifest": pre_manifest,
        "successor": successor,
        "authorization_cache": authorization_cache,
        "started_at": _timestamp(),
    }


def _validate_pending_recovery_intent(
    backend: Path, directory: Path, facts: dict, prefix: dict, successor: dict
) -> dict:
    from source_fingerprints import require_current_execution_source

    path = directory / RECOVERY_INTENT
    descriptor = binding(path)
    intent = read_json(path)
    exact_fields(intent, RECOVERY_INTENT_FIELDS, "待续作来源验收恢复 intent")
    if (
        type(intent["format_version"]) is not int
        or intent["format_version"] != 1
        or intent["kind"] != "restore-source-verification-recovery-intent"
        or intent["status"] != "recovering"
        or intent["source_generation"] != facts["start_descriptor"]
        or intent["dataset_lineage"] != facts["receipt"]["dataset_lineage"]
        or intent["failure"] != binding(directory / "failed.json")
        or intent["producer"] != _producer_bindings(directory)
        or intent["login_audit"] != binding(directory / "audit/login-audit.json")
        or intent["before"] != prefix["before_descriptor"]
        or intent["successor"] != successor
    ):
        raise ValueError("待续作来源验收恢复 intent 与失败前缀或当前 successor 不同")
    current = [row for row in _manifest(directory) if row["path"] != RECOVERY_INTENT
               and not row["path"].startswith(RECOVERY_AUDIT + "/")]
    if current != intent["pre_recovery_manifest"]:
        raise ValueError("待续作来源验收恢复 intent 绑定的原完整 manifest 已变化")
    _validate_cache_observation(intent["authorization_cache"], prefix["node"]["subjects"])
    require_current_execution_source(backend, intent["successor"])
    timestamp(intent["started_at"], "待续作来源验收恢复开始时间")
    if binding(path) != descriptor:
        raise ValueError("待续作来源验收恢复 intent 在核验期间变化")
    return intent


def _resume_or_execute_cache_cleanup(resources, intent: dict, output: Path, *, checkpoint=None) -> dict:
    frozen = intent["authorization_cache"]
    keys = _cache_keys(frozen["plan"])
    types = [resources._redis(["TYPE", key]) for key in keys]
    if all(kind == "none" for kind in types):
        expired = [item["snapshot"]["key"] for item in frozen["before"]
                   if item["snapshot"]["type"] == "none"]
        return _publish_cache_cleanup(
            resources, frozen, output, None, expired,
            "reconciled_unknown_lua_result",
        )
    observed, comparisons = _cache_observation(
        resources, frozen["namespace"], frozen["subjects"]
    )
    if observed != frozen:
        raise ValueError("来源验收恢复 intent 后授权缓存出现混合状态或漂移")
    return _execute_cache_cleanup(resources, frozen, comparisons, output, checkpoint=checkpoint)


def _recheck_login_rows(resources, database: dict, directory: Path, audit: dict) -> None:
    upper = int(audit["verified"]["upper_id"])
    for condition, filename in ((f"WHERE `id` <= {upper}", "login-after-old.tsv"),
                                (f"WHERE `id` > {upper}", "login-after-new.tsv")):
        raw = _login_query(resources, database, condition)
        actual = (raw + ("\n" if raw else "")).encode()
        if actual != (directory / "audit" / filename).read_bytes():
            raise ValueError("来源验收恢复时登录日志出现原审计以外的未知写入")


def execute_source_verification_recovery(
    backend: Path, start_path: Path, output: Path, *, run=subprocess.run
) -> dict:
    """在同一 verification 原地续做缓存清理与后像封口；不重跑 Node 或登录。"""
    start_path, output = _validate_paths(backend, start_path, output)
    from devex_clone_seed_generation import (
        registered_historical_running_source,
        verify_historical_running_source,
    )
    from devex_clone_seed_generation_images import capture_image, verify_image

    start = binding(start_path)
    initial = verify_historical_running_source(backend, start, live=True)
    directory = initial["output"] / "verification"
    if output.parent != directory or not directory.is_dir() or output.exists():
        raise ValueError("来源验收恢复必须使用已失败且尚未封口的同代 verification")
    controls = Environments(dict(os.environ), configured(initial["environment"]["environment"]))

    def control_environment():
        if dict(os.environ) not in controls.values.values():
            raise ValueError("源验证恢复的控制或服务环境在检查前变化")
        return controls.use("source")

    with registered_historical_running_source(backend, start) as registered:
        checkpoint, recovery_tools = registered
        facts = checkpoint()
        prefix = _verify_recovery_prefix(backend, directory, facts)
        with Environments(prefix["environment"], {}).use("source"):
            recovery_audit = directory / RECOVERY_AUDIT
            resources = _resources(backend, facts, recovery_audit, run)
            intent_path = directory / RECOVERY_INTENT
            if intent_path.exists():
                intent = _validate_pending_recovery_intent(
                    backend, directory, facts, prefix, recovery_tools
                )
            else:
                pre_manifest = _manifest(directory)
                observation, _comparisons = _cache_observation(
                    resources, resources.selected["redis"]["namespace"], prefix["node"]["subjects"]
                )
                intent = _recovery_intent(
                    directory, facts, prefix, recovery_tools, pre_manifest, observation
                )
                write_json(intent_path, intent)
            checkpoint()
            recovery_audit.mkdir(exist_ok=True)
            _recheck_login_rows(resources, _control_database(facts), directory, prefix["audit"])
            cleanup = _resume_or_execute_cache_cleanup(
                resources, intent, directory / "cache-cleanup.json", checkpoint=checkpoint
            )
            checkpoint()
            after_descriptor = capture_image(
                backend, facts["execution"], facts["selected"], facts["request"], facts["source"],
                prefix["environment"], directory / "after", run,
                control_environment=control_environment,
            )
            after = verify_image(
                backend, after_descriptor, facts["selected"], facts["source"]["request"],
                source_registration=facts["receipt"]["source_registration"],
            )
            _recheck_login_rows(resources, _control_database(facts), directory, prefix["audit"])
        effects = image_write_effects(prefix["before"]["image"], after["image"])
        if len(cleanup["after"]) != 33:
            raise ValueError("来源验收恢复缓存清理没有完整解释 33 个计划键")
        recovery = {
            "format_version": 1, "kind": "restore-source-verification-recovery",
            "status": "recovered", "intent": binding(directory / RECOVERY_INTENT),
            "cache_cleanup": binding(directory / "cache-cleanup.json"), "after": after_descriptor,
            "historical_cleanup": "unknown",
            "confirmed_cleanup": (
                "recovery_lua_cas_unlink" if cleanup["execution"] == "confirmed_lua_result"
                else "reconciled_unknown_lua_result_all_keys_absent"
            ),
            "login_recheck": _manifest(recovery_audit),
            "completed_at": _timestamp(),
        }
        write_json(directory / RECOVERY_RECEIPT, recovery)
        receipt = _source_runtime_receipt(
            directory, facts, start, prefix["producer_intent"]["started_at"],
            prefix["before_descriptor"], after_descriptor, effects, cleanup,
            recovery=binding(directory / RECOVERY_RECEIPT),
        )
        checkpoint()
        write_json(output, receipt)
        verified = verify_source_runtime(backend, binding(output), live=True)
        if verified["receipt"] != receipt:
            raise ValueError("恢复后的来源运行收据写入后复核不同")
        checkpoint()
        return {"output": str(output), "status": receipt["status"], "restore_success": False}


def _validate_manifest(root: Path, expected: object, label: str) -> None:
    if not isinstance(expected, list) or _manifest(root) != expected:
        raise ValueError(f"来源验收 {label} 证据文件集合或字节发生变化")


def _read_login_audit(backend: Path, directory: Path, descriptor: dict) -> dict:
    path = bound_file(backend, descriptor)
    if path != directory / "audit/login-audit.json":
        raise ValueError("来源登录审计不属于同一 verification 目录")
    value = read_json(path)
    exact_fields(
        value,
        {"format_version", "kind", "status", "before_rows", "after_old_rows",
         "after_new_rows", "verified", "completed_at"},
        "来源登录审计",
    )
    if (
        type(value["format_version"]) is not int
        or value["format_version"] != 1
        or value["kind"] != "restore-source-login-audit"
        or value["status"] != "verified"
    ):
        raise ValueError("来源登录审计类型或状态无效")
    for key, filename in (("before_rows", "login-before.tsv"), ("after_old_rows", "login-after-old.tsv"),
                          ("after_new_rows", "login-after-new.tsv")):
        snapshot = artifact_snapshot(directory / "audit" / filename).descriptor()
        if value[key] != snapshot:
            raise ValueError("来源登录审计原始行证据已变化")
    return value


def _validate_cache_before(before: object, plan: list[dict], expired: list[str]) -> None:
    if not isinstance(before, list) or len(before) != len(plan):
        raise ValueError("来源授权缓存前像没有完整主体集合")
    for actual, expected in zip(before, plan, strict=True):
        exact_fields(
            actual,
            {
                "tenant_id", "user_id", "user_authorization_version", "tenant_epoch",
                "user_version", "snapshot",
            },
            "来源授权缓存清理前像",
        )
        if any(actual[key] != expected[key] for key in ("tenant_id", "user_id", "user_authorization_version")):
            raise ValueError("来源授权缓存清理主体不同")
        keys = expected["keys"]
        tenant_epoch = exact_fields(
            actual["tenant_epoch"], {"key", "value"}, "来源授权缓存 tenant epoch"
        )
        user_version = exact_fields(
            actual["user_version"], {"key", "value"}, "来源授权缓存用户版本"
        )
        snapshot = actual["snapshot"]
        if not isinstance(snapshot, dict) or snapshot.get("type") not in {"hash", "none"}:
            raise ValueError("来源授权缓存快照类型不同")
        fields = ({"key", "type", "field", "bytes", "sha256"}
                  if snapshot["type"] == "hash" else {"key", "type"})
        exact_fields(snapshot, fields, "来源授权缓存快照")
        if (
            tenant_epoch["key"] != keys["tenant_epoch"]
            or not isinstance(tenant_epoch["value"], str)
            or re.fullmatch(r"[1-9][0-9]{0,9}", tenant_epoch["value"]) is None
            or user_version != {
                "key": keys["user_version"],
                "value": str(expected["user_authorization_version"]),
            }
            or snapshot["key"] != keys["snapshot"]
        ):
            raise ValueError("来源授权缓存清理键或版本不同")
        if snapshot["type"] == "none":
            if snapshot["key"] not in expired:
                raise ValueError("来源授权缓存过期快照没有计入原子清理")
        elif (
            snapshot["key"] in expired
            or snapshot["field"] != f"{tenant_epoch['value']}:{user_version['value']}"
            or type(snapshot["bytes"]) is not int
            or snapshot["bytes"] <= 0
            or not isinstance(snapshot["sha256"], str)
            or re.fullmatch(r"[a-f0-9]{64}", snapshot["sha256"]) is None
        ):
            raise ValueError("来源授权缓存清理键或版本不同")


def _validate_cache_observation(value: object, subjects: list[dict]) -> dict:
    cache = exact_fields(value, {"namespace", "subjects", "plan", "before"}, "恢复授权缓存前像")
    plan = authorization_cache_plan(cache["namespace"], subjects)
    if cache["subjects"] != subjects or cache["plan"] != plan:
        raise ValueError("来源验收恢复 intent 的授权缓存计划不同")
    before = cache["before"]
    if not isinstance(before, list) or any(not isinstance(item, dict) for item in before):
        raise ValueError("来源验收恢复缺少完整授权缓存前像")
    expired = [item["snapshot"]["key"] for item in before
               if isinstance(item.get("snapshot"), dict) and item["snapshot"].get("type") == "none"]
    _validate_cache_before(before, plan, expired)
    return cache


def _verify_cache_receipt(backend: Path, directory: Path, descriptor: dict, subjects: list[dict]) -> dict:
    path = bound_file(backend, descriptor)
    if path != directory / "cache-cleanup.json":
        raise ValueError("来源授权缓存清理不属于同一 verification 目录")
    value = read_json(path)
    exact_fields(value, {"format_version", "kind", "status", "namespace", "before", "deleted",
                         "expired_snapshot_keys", "after", "execution", "cleaned_at"},
                 "来源授权缓存清理")
    plan = authorization_cache_plan(value["namespace"], subjects)
    expected_keys = _cache_keys(plan)
    expected_snapshots = [item["keys"]["snapshot"] for item in plan]
    expired = value["expired_snapshot_keys"]
    if (
        type(value["format_version"]) is not int
        or value["format_version"] != 2
        or value["kind"] != "restore-source-authorization-cache-cleanup"
        or value["status"] != "cleaned"
        or value["execution"] not in {"confirmed_lua_result", "reconciled_unknown_lua_result"}
        or (value["execution"] == "confirmed_lua_result"
            and (type(value["deleted"]) is not int or not 22 <= value["deleted"] <= 33))
        or (value["execution"] == "reconciled_unknown_lua_result" and value["deleted"] is not None)
        or not isinstance(expired, list)
        or any(not isinstance(key, str) for key in expired)
        or len(set(expired)) != len(expired)
        or expired != [key for key in expected_snapshots if key in set(expired)]
        or (value["execution"] == "confirmed_lua_result" and value["deleted"] + len(expired) != 33)
        or not isinstance(value["before"], list)
        or not isinstance(value["after"], list)
        or len(value["before"]) != 11
        or len(value["after"]) != 33
    ):
        raise ValueError("来源授权缓存清理身份或结果无效")
    for actual, key in zip(value["after"], expected_keys, strict=True):
        exact_fields(actual, {"key", "type"}, "来源授权缓存清理后像")
        if actual != {"key": key, "type": "none"}:
            raise ValueError("来源授权缓存清理后像不同")
    _validate_cache_before(value["before"], plan, expired)
    timestamp(value["cleaned_at"], "来源授权缓存清理时间")
    return value


def _verify_recovery_chain(
    backend: Path, directory: Path, descriptor: dict, receipt: dict, cleanup: dict
) -> dict:
    from source_fingerprints import verify_execution_source

    path = bound_file(backend, descriptor)
    if path != directory / RECOVERY_RECEIPT:
        raise ValueError("来源验收恢复收据不属于同一 verification 目录")
    recovery = read_json(path)
    exact_fields(recovery, RECOVERY_RECEIPT_FIELDS, "来源验收恢复收据")
    intent_path = bound_file(backend, recovery["intent"])
    if intent_path != directory / RECOVERY_INTENT:
        raise ValueError("来源验收恢复 intent 不属于同一 verification 目录")
    intent = read_json(intent_path)
    exact_fields(intent, RECOVERY_INTENT_FIELDS, "来源验收恢复 intent")
    if (
        type(intent["format_version"]) is not int
        or intent["format_version"] != 1
        or intent["kind"] != "restore-source-verification-recovery-intent"
        or intent["status"] != "recovering"
        or intent["source_generation"] != receipt["source_generation"]
        or intent["dataset_lineage"] != receipt["dataset_lineage"]
        or intent["failure"] != binding(directory / "failed.json")
        or intent["producer"] != _producer_bindings(directory)
        or intent["login_audit"] != receipt["login_audit"]
        or intent["before"] != receipt["before"]
    ):
        raise ValueError("来源验收恢复 intent 没有绑定原失败前缀")
    failure = read_json(directory / "failed.json")
    if failure != {"status": "failed", "error": "ValueError"}:
        raise ValueError("来源验收恢复没有保留原 ValueError 失败记录")
    verify_execution_source(intent["successor"], "来源验收恢复工具")
    if intent["successor"]["snapshot"]["clean"] is not True:
        raise ValueError("来源验收恢复工具不是已提交的干净 successor")
    cache = _validate_cache_observation(
        intent["authorization_cache"], intent["authorization_cache"]["subjects"]
    )
    if cache["namespace"] != cleanup["namespace"] or cache["before"] != cleanup["before"]:
        raise ValueError("来源验收恢复缓存清理没有绑定 intent 的精确前像")
    pre_manifest = intent["pre_recovery_manifest"]
    if not isinstance(pre_manifest, list):
        raise ValueError("来源验收恢复 intent 缺少恢复前完整 manifest")
    current = _manifest(directory)
    post_paths = {RECOVERY_INTENT, RECOVERY_RECEIPT, "cache-cleanup.json", "source-runtime.json"}
    original = [row for row in current if row["path"] not in post_paths
                and not row["path"].startswith(("after/", RECOVERY_AUDIT + "/"))]
    top_level = {row["path"].split("/", 1)[0] for row in pre_manifest}
    if pre_manifest != original or top_level != FAILED_ROOT_ENTRIES:
        raise ValueError("来源验收恢复前完整 manifest 或原文件发生变化")
    if (
        type(recovery["format_version"]) is not int
        or recovery["format_version"] != 1
        or recovery["kind"] != "restore-source-verification-recovery"
        or recovery["status"] != "recovered"
        or recovery["cache_cleanup"] != receipt["cache_cleanup"]
        or recovery["after"] != receipt["after"]
        or recovery["historical_cleanup"] != "unknown"
        or recovery["confirmed_cleanup"] != (
            "recovery_lua_cas_unlink" if cleanup["execution"] == "confirmed_lua_result"
            else "reconciled_unknown_lua_result_all_keys_absent"
        )
        or binding(intent_path) != recovery["intent"]
        or binding(path) != descriptor
    ):
        raise ValueError("来源验收恢复收据没有绑定确认的恢复清理")
    _validate_manifest(directory / RECOVERY_AUDIT, recovery["login_recheck"], "恢复登录日志核对")
    started = dt.datetime.fromisoformat(timestamp(intent["started_at"], "来源验收恢复开始时间").replace("Z", "+00:00"))
    completed = dt.datetime.fromisoformat(timestamp(recovery["completed_at"], "来源验收恢复完成时间").replace("Z", "+00:00"))
    cleaned = dt.datetime.fromisoformat(timestamp(cleanup["cleaned_at"], "来源验收恢复清理时间").replace("Z", "+00:00"))
    if not started <= cleaned <= completed <= dt.datetime.now(dt.timezone.utc):
        raise ValueError("来源验收恢复时间顺序不成立")
    return recovery


def verify_source_runtime_facts(backend: Path, receipt: dict, *, live: bool) -> dict:
    """按已完整核验的普通或恢复收据选择事实 validator；普通路径不降级。"""
    from devex_clone_seed_generation import verify_historical_running_source, verify_running_source

    fields = set(receipt) if isinstance(receipt, dict) else set()
    if fields == RECEIPT_FIELDS:
        verifier = verify_running_source
    elif fields == RECOVERED_RECEIPT_FIELDS and isinstance(receipt.get("recovery"), dict):
        from source_fingerprints import require_current_execution_source

        recovery_path = bound_file(backend, receipt["recovery"])
        recovery = read_json(recovery_path)
        exact_fields(recovery, RECOVERY_RECEIPT_FIELDS, "来源验收恢复收据")
        intent_path = bound_file(backend, recovery["intent"])
        intent = read_json(intent_path)
        exact_fields(intent, RECOVERY_INTENT_FIELDS, "来源验收恢复 intent")
        if (recovery_path.name != RECOVERY_RECEIPT
                or intent_path != recovery_path.parent / RECOVERY_INTENT
                or intent["source_generation"] != receipt["source_generation"]):
            raise ValueError("来源运行恢复事实没有绑定同代 intent")
        if live:
            require_current_execution_source(backend, intent["successor"])
        verifier = verify_historical_running_source
    else:
        raise ValueError("来源运行收据不能选择唯一普通或恢复事实 validator")
    return verifier(backend, receipt["source_generation"], live=live)


def verify_source_runtime(backend: Path, descriptor: dict, *, live: bool) -> dict:
    """唯一只读 validator；live 只控制 source generation 当前进程断言，返回静态事实。"""
    from devex_clone_seed_generation_images import verify_image

    path = bound_file(backend, descriptor)
    directory = path.parent
    if path != directory / "source-runtime.json" or directory.name != "verification":
        raise ValueError("source-runtime 不属于固定同代 verification 目录")
    receipt = read_json(path)
    fields = frozenset(receipt) if isinstance(receipt, dict) else frozenset()
    recovered = fields == frozenset(RECOVERED_RECEIPT_FIELDS)
    if fields not in {frozenset(RECEIPT_FIELDS), frozenset(RECOVERED_RECEIPT_FIELDS)}:
        raise ValueError("来源运行收据字段不属于普通或恢复 schema")
    exact_fields(receipt, RECOVERED_RECEIPT_FIELDS if recovered else RECEIPT_FIELDS, "来源运行收据")
    expected_root = RECOVERED_ROOT_ENTRIES if recovered else ROOT_ENTRIES
    if (
        type(receipt["format_version"]) is not int
        or receipt["format_version"] != FORMAT_VERSION
        or receipt["kind"] != "restore-source-runtime"
        or receipt["status"] != "source_runtime_verified"
        or receipt["restore_qualified"] is not False
        or set(receipt["producer"]) != PRODUCER_BINDING_FIELDS
        or {entry.name for entry in directory.iterdir()} != expected_root
        or recovered and not isinstance(receipt["recovery"], dict)
    ):
        raise ValueError("来源运行收据版本、状态或证据集合无效")
    facts = verify_source_runtime_facts(backend, receipt, live=live)
    if (
        directory != facts["output"] / "verification"
        or receipt["dataset_lineage"] != facts["receipt"]["dataset_lineage"]
    ):
        raise ValueError("来源运行收据没有绑定同一 generation 与数据血缘")
    environment = configured(facts["environment"]["environment"])
    node = Path(ExternalTools(facts["source"]["request"], directory).command("node")[0]).resolve(strict=True)
    producer, stdout = verify_source_producer(
        backend, facts["execution"], directory, receipt["source_generation"],
        receipt["dataset_lineage"], environment, node,
        coordinator_source=facts["coordinator_source"],
        execution_sha=facts["request"]["expected_backend_sha"],
        historical_coordinator=recovered,
    )
    for key, filename in (("intent", INTENT), ("process", PROCESS), ("ready", READY), ("stdout", STDOUT),
                          ("stderr", STDERR), ("completion", COMPLETION)):
        actual = artifact_snapshot(directory / filename).descriptor()
        if receipt["producer"][key] != actual:
            raise ValueError("来源运行收据的生产者文件绑定已变化")
    if receipt["verification"] != binding(directory / STDOUT):
        raise ValueError("来源运行收据没有绑定 Node stdout")
    verified_node = validate_node_result(
        stdout, facts["lineage"], receipt["source_generation"], receipt["dataset_lineage"]
    )
    _validate_stored_login_audit(
        backend, directory, receipt["login_audit"], facts["lineage"],
        verified_node["subjects"], receipt["started_at"],
    )
    cleanup = _verify_cache_receipt(
        backend, directory, receipt["cache_cleanup"], verified_node["subjects"]
    )
    before = verify_image(
        backend, receipt["before"], facts["selected"], facts["source"]["request"],
        source_registration=facts["receipt"]["source_registration"],
    )
    after = verify_image(
        backend, receipt["after"], facts["selected"], facts["source"]["request"],
        source_registration=facts["receipt"]["source_registration"],
    )
    if before["image"] != facts["running"]["image"]:
        raise ValueError("来源验收前像与 start 运行像不同")
    effects = image_write_effects(before["image"], after["image"])
    if (
        receipt["write_effects"] != effects
        or len(cleanup["after"]) != 33
        or (not recovered and cleanup["execution"] != "confirmed_lua_result")
        or receipt["remote_writes"] != _remote_write_receipt(cleanup, recovered=recovered)
    ):
        raise ValueError("来源运行收据的业务副作用与完整像不同")
    if recovered:
        _verify_recovery_chain(backend, directory, receipt["recovery"], receipt, cleanup)
    exact_fields(receipt["evidence"], {"before", "after", "audit"}, "来源验收文件清单")
    for name in ("before", "after", "audit"):
        _validate_manifest(directory / name, receipt["evidence"][name], name)
    time_values = (
        timestamp(receipt["started_at"], "来源验收开始时间"),
        timestamp(read_json(directory / COMPLETION)["completed_at"], "来源生产者完成时间"),
        timestamp(cleanup["cleaned_at"], "来源缓存清理时间"),
        timestamp(receipt["verified_at"], "来源验收完成时间"),
    )
    started, completed, cleaned, verified = (
        dt.datetime.fromisoformat(value.replace("Z", "+00:00")) for value in time_values
    )
    if not started <= completed <= cleaned <= verified <= dt.datetime.now(dt.timezone.utc):
        raise ValueError("来源运行收据时间顺序不成立")
    if binding(path) != descriptor:
        raise ValueError("来源运行收据在核验期间变化")
    return {
        "receipt": copy.deepcopy(receipt),
        "before": copy.deepcopy(before),
        "after": copy.deepcopy(after),
        "producer": copy.deepcopy(producer),
        "lineage": copy.deepcopy(facts["lineage"]),
        "facts": copy.deepcopy(facts),
    }
