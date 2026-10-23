"""在同一 source-generation 锁内复验 C52 派生来源并发布严格运行收据。"""

from __future__ import annotations

import copy
import datetime as dt
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
from restore_reference_plan import identifier
from restore_runtime_evidence import artifact_snapshot, exact_fields
from restore_source_runtime_producer import (
    COMPLETION,
    CONFIGURATION,
    INTENT,
    PROCESS,
    READY,
    STDERR,
    STDOUT,
    run_source_producer,
    _runtime_policy,
    _source_policy,
    verify_source_producer,
)


RECEIPT_FIELDS = {
    "format_version", "kind", "status", "source_generation", "dataset_lineage",
    "before", "after", "producer", "verification", "login_audit", "cache_cleanup",
    "evidence", "write_effects", "started_at", "verified_at", "remote_writes",
    "restore_qualified",
}
RECOVERED_RECEIPT_FIELDS = RECEIPT_FIELDS | {"recovery"}
RECOVERY_INTENT = "source-verification-recovery-intent.json"
RECOVERY_RECEIPT = "source-verification-recovery.json"
RECOVERY_AUDIT = "recovery-audit"
FAILED_ROOT_ENTRIES = {
    "before", "audit", "source-tools", INTENT, PROCESS, READY, STDOUT, STDERR, COMPLETION,
    CONFIGURATION, "failed.json",
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


def _timestamp() -> str:
    return dt.datetime.now(dt.timezone.utc).isoformat()


def _new_file(path: Path, content: bytes) -> dict:
    with path.open("xb") as stream:
        stream.write(content)
        stream.flush()
        os.fsync(stream.fileno())
    return artifact_snapshot(path).descriptor()


def _login_rows(raw: str) -> list[dict]:
    return _runtime_policy("login-rows", raw=raw)


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
    verified = _source_policy(
        'login-audit',
        old_before=old.hex(),
        old_after=previous_bytes.hex(),
        upper_id=upper,
        new_rows=_login_rows(added),
        lineage=lineage,
        subjects=subjects,
        started_at=started_at,
        completed_at=completed_at,
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
    return _runtime_policy("snapshot", payload=payload, subject=subject, tenant_epoch=tenant_epoch)


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
    plan = _source_policy(
        'cache-plan',
        namespace=namespace,
        subjects=subjects,
    )
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
    after = [{"key": key, "type": resources._redis(["TYPE", key])}
             for key in _cache_keys(observation["plan"])]
    receipt = _runtime_policy(
        "cache-publication", observation=observation, deleted=deleted, expired=expired,
        execution=execution, after=after, cleaned_at=_timestamp(),
    )
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


def _evidence_backend(directory: Path) -> Path:
    for parent in directory.parents:
        if parent.name == ".local-tests":
            return parent.parent
    raise ValueError("来源验收证据必须位于后端忽略目录")


def _manifest(root: Path) -> list[dict]:
    return _source_policy(
        'runtime-manifest',
        backend=str(_evidence_backend(root)),
        directory=str(root),
    )


def _resources(backend: Path, facts: dict, output: Path, run):
    from devex_clone_target_binding import redis_configuration
    from devex_clone_target_resources import Resources

    target = facts["source"]["seed_target"]
    review_binding = {key: value for key, value in target["review"].items() if key != "canonical_sha256"}
    review = read_json(bound_file(backend, review_binding))
    selected = review["scopes"]["seed"]
    redis_configuration(facts["execution"], target, selected,
                        environment=facts["environment"]["environment"])
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
    return _runtime_policy(
        "runtime-receipt", backend=str(_evidence_backend(directory)), directory=str(directory),
        lineage=facts["receipt"]["dataset_lineage"], start=start, started_at=started_at,
        before=before, after=after, effects=effects, cleanup=cleanup, recovery=recovery,
    )


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
            if dict(os.environ) != control_environments.values["source"]:
                raise ValueError("源验证控制环境在安装服务配置前变化")
            environment = configured(facts["environment"]["environment"])
            with Environments(environment, {}).use("source"):
                for stage in _runtime_policy("flow", mode="verify"):
                    if stage == "prepare":
                        expected_directory.mkdir()
                        (expected_directory / "audit").mkdir()
                        resources = _resources(backend, facts, expected_directory / "audit", run)
                        database = _control_database(facts)
                    elif stage == "login-before":
                        old_rows, upper_id, _ = _login_before(
                            resources, database, expected_directory / "audit"
                        )
                    elif stage == "capture-before":
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
                    elif stage == "producer":
                        from restore_source_runtime_producer import native_tool
                        node = native_tool()
                        operation = uuid.uuid4().hex
                        node_result, _ = run_source_producer(
                            backend, facts["execution"], expected_directory, operation, node, start,
                            facts["receipt"]["dataset_lineage"], environment,
                            coordinator_source=facts["coordinator_source"],
                            execution_sha=facts["request"]["expected_backend_sha"], popen=popen,
                        )
                        verified_node = _source_policy(
                            'result',
                            value=node_result,
                            lineage=facts['lineage'],
                            start=start,
                            lineage_descriptor=facts['receipt']['dataset_lineage'],
                        )
                    elif stage == "login-after":
                        audit = _login_after(
                            resources, database, expected_directory / "audit", old_rows, upper_id,
                            facts["lineage"], verified_node["subjects"], started_at,
                        )
                    elif stage == "cache-cleanup":
                        cleanup = _clean_authorization_cache(
                            resources, resources.selected["redis"]["namespace"],
                            verified_node["subjects"], expected_directory / "cache-cleanup.json",
                        )
                    elif stage == "capture-after":
                        after = capture_image(
                            backend, facts["execution"], facts["selected"], facts["request"],
                            facts["source"], environment, expected_directory / "after", run,
                            control_environment=control_environment,
                        )
                        after_value = verify_image(
                            backend, after, facts["selected"], facts["source"]["request"],
                            source_registration=facts["receipt"]["source_registration"],
                        )
                    elif stage == "effects":
                        effects = _source_policy(
                            'write-effects',
                            before=before_value['image'],
                            after=after_value['image'],
                        )
                        if (
                            cleanup["deleted"] + len(cleanup["expired_snapshot_keys"])
                            != effects["authorization_cache"]["created_and_removed_keys"]
                        ):
                            raise ValueError("来源验收授权缓存清理与完整前后像合同不同")
                    elif stage == "receipt":
                        receipt = _source_runtime_receipt(
                            expected_directory, facts, start, started_at, before, after, effects, cleanup,
                            recovery=None,
                        )
                    elif stage == "publish":
                        write_json(output, receipt)
                    elif stage == "verify-published":
                        verified = verify_source_runtime(backend, binding(output), live=True)
                        if verified["receipt"] != receipt:
                            raise ValueError("来源运行收据写入后复核不同")
                    elif stage == "checkpoint":
                        checkpoint()
                    else:
                        raise ValueError("来源运行阶段不属于原生固定计划")
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
    verified = _source_policy(
        'login-audit',
        old_before=old.hex(),
        old_after=old_after.hex(),
        upper_id=int(audit['verified']['upper_id']),
        new_rows=_login_rows(added),
        lineage=lineage,
        subjects=subjects,
        started_at=started_at,
        completed_at=audit['completed_at'],
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
    from restore_source_runtime_producer import native_tool
    node = native_tool()
    producer, stdout = verify_source_producer(
        backend, facts["execution"], directory, facts["start_descriptor"],
        facts["receipt"]["dataset_lineage"], environment, node,
        coordinator_source=facts["coordinator_source"],
        execution_sha=facts["request"]["expected_backend_sha"], historical_coordinator=True,
    )
    verified_node = _source_policy(
        'result',
        value=stdout,
        lineage=facts['lineage'],
        start=facts['start_descriptor'],
        lineage_descriptor=facts['receipt']['dataset_lineage'],
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

    intent = _runtime_policy(
        "recovery-pending", backend=str(backend), directory=str(directory),
        start=facts["start_descriptor"], lineage=facts["receipt"]["dataset_lineage"],
        before=prefix["before_descriptor"], successor=successor, subjects=prefix["node"]["subjects"],
    )
    require_current_execution_source(backend, intent["successor"])
    return intent


def _resume_or_execute_cache_cleanup(resources, intent: dict, output: Path, *, checkpoint=None) -> dict:
    frozen = intent["authorization_cache"]
    keys = _cache_keys(frozen["plan"])
    types = [resources._redis(["TYPE", key]) for key in keys]
    observed, comparisons = (None, None) if all(kind == "none" for kind in types) else _cache_observation(
        resources, frozen["namespace"], frozen["subjects"]
    )
    decision = _source_policy("runtime-cache-resume", frozen=frozen, types=types, observed=observed)
    if decision["action"] == "reconcile":
        return _publish_cache_cleanup(
            resources, frozen, output, None, decision["expired"], "reconciled_unknown_lua_result",
        )
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
            for stage in _runtime_policy("flow", mode="recover"):
                if stage == "recovery-intent":
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
                elif stage == "login-recheck":
                    recovery_audit.mkdir(exist_ok=True)
                    _recheck_login_rows(resources, _control_database(facts), directory, prefix["audit"])
                elif stage == "cache-resume":
                    cleanup = _resume_or_execute_cache_cleanup(
                        resources, intent, directory / "cache-cleanup.json", checkpoint=checkpoint
                    )
                elif stage == "capture-after":
                    after_descriptor = capture_image(
                        backend, facts["execution"], facts["selected"], facts["request"], facts["source"],
                        prefix["environment"], directory / "after", run,
                        control_environment=control_environment,
                    )
                    after = verify_image(
                        backend, after_descriptor, facts["selected"], facts["source"]["request"],
                        source_registration=facts["receipt"]["source_registration"],
                    )
                elif stage == "effects":
                    effects = _source_policy(
                        'write-effects',
                        before=prefix['before']['image'],
                        after=after['image'],
                    )
                    if len(cleanup["after"]) != 33:
                        raise ValueError("来源验收恢复缓存清理没有完整解释 33 个计划键")
                elif stage == "recovery-receipt":
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
                elif stage == "receipt":
                    receipt = _source_runtime_receipt(
                        directory, facts, start, prefix["producer_intent"]["started_at"],
                        prefix["before_descriptor"], after_descriptor, effects, cleanup,
                        recovery=binding(directory / RECOVERY_RECEIPT),
                    )
                elif stage == "publish":
                    write_json(output, receipt)
                elif stage == "verify-published":
                    verified = verify_source_runtime(backend, binding(output), live=True)
                    if verified["receipt"] != receipt:
                        raise ValueError("恢复后的来源运行收据写入后复核不同")
                elif stage == "checkpoint":
                    checkpoint()
                else:
                    raise ValueError("来源恢复阶段不属于原生固定计划")
        return {"output": str(output), "status": receipt["status"], "restore_success": False}


def _read_login_audit(backend: Path, directory: Path, descriptor: dict) -> dict:
    return _runtime_policy(
        "login-evidence", backend=str(backend), directory=str(directory), descriptor=descriptor,
    )


def _verify_cache_receipt(backend: Path, directory: Path, descriptor: dict, subjects: list[dict]) -> dict:
    path = bound_file(backend, descriptor)
    if path != directory / "cache-cleanup.json":
        raise ValueError("来源授权缓存清理不属于同一 verification 目录")
    return _source_policy("runtime-cache-receipt", value=read_json(path), subjects=subjects)


def _verify_recovery_chain(
    backend: Path, directory: Path, descriptor: dict, receipt: dict, cleanup: dict
) -> dict:
    from source_fingerprints import verify_execution_source

    verified = _runtime_policy(
        "recovery-chain", backend=str(backend), directory=str(directory),
        descriptor=descriptor, receipt=receipt, cleanup=cleanup,
    )
    verify_execution_source(verified["successor"], "来源验收恢复工具")
    return verified["recovery"]


def verify_source_runtime_facts(backend: Path, receipt: dict, *, live: bool) -> dict:
    """按已完整核验的普通或恢复收据选择事实 validator；普通路径不降级。"""
    from devex_clone_seed_generation import verify_historical_running_source, verify_running_source

    selected = _runtime_policy("runtime-facts", backend=str(backend), receipt=receipt)
    if selected["recovered"]:
        from source_fingerprints import require_current_execution_source

        if live:
            require_current_execution_source(backend, selected["successor"])
        verifier = verify_historical_running_source
    else:
        verifier = verify_running_source
    return verifier(backend, receipt["source_generation"], live=live)


def verify_source_runtime(backend: Path, descriptor: dict, *, live: bool) -> dict:
    """唯一只读 validator；live 只控制 source generation 当前进程断言，返回静态事实。"""
    from devex_clone_seed_generation_images import verify_image

    path = bound_file(backend, descriptor)
    directory = path.parent
    if path != directory / "source-runtime.json" or directory.name != "verification":
        raise ValueError("source-runtime 不属于固定同代 verification 目录")
    receipt = read_json(path)
    recovered = _runtime_policy(
        "runtime-schema", backend=str(backend), directory=str(directory), value=receipt,
    )["recovered"]
    facts = verify_source_runtime_facts(backend, receipt, live=live)
    if (
        directory != facts["output"] / "verification"
        or receipt["dataset_lineage"] != facts["receipt"]["dataset_lineage"]
    ):
        raise ValueError("来源运行收据没有绑定同一 generation 与数据血缘")
    environment = configured(facts["environment"]["environment"])
    from restore_source_runtime_producer import native_tool
    node = native_tool()
    producer, stdout = verify_source_producer(
        backend, facts["execution"], directory, receipt["source_generation"],
        receipt["dataset_lineage"], environment, node,
        coordinator_source=facts["coordinator_source"],
        execution_sha=facts["request"]["expected_backend_sha"],
        historical_coordinator=recovered,
    )
    verified_node = _source_policy(
        'result',
        value=stdout,
        lineage=facts['lineage'],
        start=receipt['source_generation'],
        lineage_descriptor=receipt['dataset_lineage'],
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
    effects = _source_policy(
        'write-effects',
        before=before['image'],
        after=after['image'],
    )
    if recovered:
        _verify_recovery_chain(backend, directory, receipt["recovery"], receipt, cleanup)
    _runtime_policy(
        "runtime-final", backend=str(backend), directory=str(directory), descriptor=descriptor,
        receipt=receipt, before=before["image"], expected_before=facts["running"]["image"],
        effects=effects, cleanup=cleanup, recovered=recovered,
    )
    return {
        "receipt": copy.deepcopy(receipt),
        "before": copy.deepcopy(before),
        "after": copy.deepcopy(after),
        "producer": copy.deepcopy(producer),
        "lineage": copy.deepcopy(facts["lineage"]),
        "facts": copy.deepcopy(facts),
    }
