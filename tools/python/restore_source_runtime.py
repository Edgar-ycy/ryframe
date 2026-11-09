"""在同一 source-generation 锁内复验 C52 派生来源并发布严格运行收据。"""

from __future__ import annotations

import copy
import datetime as dt
import os
from pathlib import Path
import subprocess
import uuid

from devex_clone_capture import read_json, write_json
from process_environment import Environments, configured
from devex_clone_run_state import binding
from devex_clone_source_proof import bound_file
from restore_runtime_evidence import artifact_snapshot
from restore_source_runtime_producer import (
    COMPLETION,
    CONFIGURATION,
    INTENT,
    PROCESS,
    READY,
    STDERR,
    STDOUT,
    run_source_producer,
    _source_policy as _native_source_policy,
    verify_source_producer,
)


RECOVERY_INTENT = "source-verification-recovery-intent.json"
RECOVERY_RECEIPT = "source-verification-recovery.json"
RECOVERY_AUDIT = "recovery-audit"


def _source_policy(operation: str, **arguments):
    from rate_limit_tool import tool_binding

    if tool_binding() is None:
        raise ValueError("来源验收缺少协调器原生工具绑定；先由正式检查入口登记工具，禁止临时编译回退")
    return _native_source_policy(operation, **arguments)


def _runtime_policy(action: str, **arguments):
    return _source_policy("producer-ready", action=action, arguments=arguments)


def _timestamp() -> str:
    return dt.datetime.now(dt.timezone.utc).isoformat()


def _new_file(path: Path, content: bytes) -> dict:
    with path.open("xb") as stream:
        stream.write(content)
        stream.flush()
        os.fsync(stream.fileno())
    return artifact_snapshot(path).descriptor()


def _login_query(resources, database: dict, selection: str, upper=None) -> str:
    sql = _runtime_policy("login-query", database=database["database"], selection=selection, upper_id=upper)
    return resources._mysql(database, sql)


def _login_before(resources, database: dict, audit: Path) -> tuple[bytes, int, dict]:
    raw = _login_query(resources, database, "all")
    evidence = _runtime_policy("login-row-evidence", raw=raw)
    content = bytes.fromhex(evidence["hex"])
    descriptor = _new_file(audit / "login-before.tsv", content)
    return content, evidence["upper_id"], descriptor


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
    previous = _login_query(resources, database, "old", upper)
    added = _login_query(resources, database, "new", upper)
    previous_bytes = (previous + ("\n" if previous else "")).encode()
    _new_file(audit / "login-after-old.tsv", previous_bytes)
    _new_file(audit / "login-after-new.tsv", (added + ("\n" if added else "")).encode())
    completed_at = _timestamp()
    receipt = _runtime_policy(
        "login-create", backend=str(_evidence_backend(audit)), directory=str(audit.parent),
        old_before=old.hex(), old_after=previous_bytes.hex(), upper_id=upper, new_raw=added,
        lineage=lineage, subjects=subjects, started_at=started_at, completed_at=completed_at,
    )
    write_json(audit / "login-audit.json", receipt)
    return receipt


def _cache_keys(plan: list[dict]) -> list[str]:
    return _runtime_policy("cache-keys", plan=plan)


def _cache_before(resources, plan: list[dict]) -> tuple[list[dict], list[str]]:
    raw = []
    for item in plan:
        keys = item["keys"]
        raw.append({
            "types": {name: resources._redis(["TYPE", key]) for name, key in keys.items()},
            "tenant_epoch": resources._redis(["GET", keys["tenant_epoch"]]),
            "user_version": resources._redis(["GET", keys["user_version"]]),
        })
    selected = _runtime_policy("cache-read-plans", plan=plan, raw=raw)
    for item, row, read in zip(plan, raw, selected, strict=True):
        fields, payload = [], ""
        if read["snapshot_type"] == "hash":
            fields = resources._redis(["HKEYS", item["keys"]["snapshot"]])
            payload = resources._redis(["HGET", item["keys"]["snapshot"], read["field"]])
        row.update(fields=fields, payload=payload)
    observed = _runtime_policy("cache-observe", plan=plan, raw=raw)
    return observed["before"], observed["comparisons"]


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


def _cache_attempt_state(output: Path) -> dict:
    return _source_policy(
        "runtime-cache-state", backend=str(_evidence_backend(output.parent)),
        directory=str(output.parent), output=str(output),
    )


def _publish_cache_cleanup(resources, observation: dict, output: Path) -> dict:
    after = [{"key": key, "type": resources._redis(["TYPE", key])}
             for key in _cache_keys(observation["plan"])]
    receipt = _runtime_policy(
        "cache-publication", backend=str(_evidence_backend(output.parent)),
        directory=str(output.parent), output=str(output), observation=observation,
        after=after, cleaned_at=_timestamp(),
    )
    write_json(output, receipt)
    return receipt


def _execute_cache_cleanup(
    resources, observation: dict, comparisons: list[str], output: Path, *, checkpoint=None
) -> dict:
    if checkpoint is not None:
        checkpoint()
    verified = resources._cache_cleanup_native(observation, comparisons, output)
    if checkpoint is not None:
        checkpoint()
    if verified.get("status") != "confirmed":
        raise ValueError(verified["message"])
    return _publish_cache_cleanup(resources, observation, output)


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
    return _runtime_policy("control-database", databases=facts["source"]["seed_target"]["target"]["databases"])


def _validate_paths(backend: Path, start: Path, output: Path) -> tuple[Path, Path]:
    selected = _runtime_policy("paths", backend=str(backend), start=str(start), output=str(output))
    return Path(selected["start"]), Path(selected["output"])


def _producer_bindings(directory: Path) -> dict:
    return _runtime_policy("producer-bindings", backend=str(_evidence_backend(directory)), directory=str(directory))


def receipt_fields(*, recovered=False) -> set[str]:
    return set(_runtime_policy("receipt-fields", recovered=recovered))


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
                        _runtime_policy("image-binding", before=before_value["image"], expected=facts["running"]["image"])
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
                        _runtime_policy("cache-effects", cleanup=cleanup, effects=effects, recovered=False)
                    elif stage == "receipt":
                        receipt = _source_runtime_receipt(
                            expected_directory, facts, start, started_at, before, after, effects, cleanup,
                            recovery=None,
                        )
                    elif stage == "publish":
                        write_json(output, receipt)
                    elif stage == "verify-published":
                        verified = verify_source_runtime(backend, binding(output), live=True)
                        _runtime_policy("published", expected=receipt, actual=verified["receipt"])
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
    return _runtime_policy("login-stored", backend=str(backend), directory=str(directory),
                           descriptor=descriptor, lineage=lineage, subjects=subjects, started_at=started_at)


def _verify_recovery_prefix(backend: Path, directory: Path, facts: dict) -> dict:
    from devex_clone_seed_generation_images import verify_image

    _runtime_policy("recovery-prefix", backend=str(backend), directory=str(directory))
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
    _runtime_policy("image-binding", before=before["image"], expected=facts["running"]["image"])
    return {"producer": producer, "producer_intent": producer_intent, "node": verified_node,
            "audit": audit, "before": before, "before_descriptor": before_descriptor,
            "environment": environment}


def _recovery_intent(
    directory: Path, facts: dict, prefix: dict, successor: dict, pre_manifest: list[dict],
    authorization_cache: dict,
) -> dict:
    return _runtime_policy(
        "recovery-intent", backend=str(_evidence_backend(directory)), directory=str(directory),
        start=facts["start_descriptor"], lineage=facts["receipt"]["dataset_lineage"],
        before=prefix["before_descriptor"], successor=successor, pre_manifest=pre_manifest,
        authorization_cache=authorization_cache, started_at=_timestamp(),
    )


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
    state = _cache_attempt_state(output)
    types = [resources._redis(["TYPE", key]) for key in _cache_keys(frozen["plan"])]
    observed, comparisons = None, None
    if state["status"] == "not_attempted" and not all(kind == "none" for kind in types):
        observed, comparisons = _cache_observation(resources, frozen["namespace"], frozen["subjects"])
    decision = _source_policy(
        "runtime-cache-resume", backend=str(_evidence_backend(output.parent)),
        directory=str(output.parent), output=str(output), frozen=frozen, types=types, observed=observed,
    )
    if decision["action"] == "publish":
        return _publish_cache_cleanup(resources, frozen, output)
    return _execute_cache_cleanup(resources, frozen, comparisons, output, checkpoint=checkpoint)


def _recheck_login_rows(resources, database: dict, directory: Path, audit: dict) -> None:
    upper = int(audit["verified"]["upper_id"])
    for selection, filename in (("old", "login-after-old.tsv"), ("new", "login-after-new.tsv")):
        raw = _login_query(resources, database, selection, upper)
        _runtime_policy("login-recheck", backend=str(_evidence_backend(directory)),
                        directory=str(directory), filename=filename, raw=raw)


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
                        state = _cache_attempt_state(directory / "cache-cleanup.json")
                        if state["status"] == "not_attempted":
                            observation, _comparisons = _cache_observation(
                                resources, resources.selected["redis"]["namespace"], prefix["node"]["subjects"]
                            )
                        else:
                            observation = state["observation"]
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
                    _runtime_policy("cache-effects", cleanup=cleanup, effects=effects, recovered=True)
                elif stage == "recovery-receipt":
                    recovery = _runtime_policy(
                        "recovery-receipt", backend=str(backend), directory=str(directory),
                        cleanup=cleanup, after=after_descriptor, completed_at=_timestamp(),
                    )
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
                    _runtime_policy("published", expected=receipt, actual=verified["receipt"])
                elif stage == "checkpoint":
                    checkpoint()
                else:
                    raise ValueError("来源恢复阶段不属于原生固定计划")
        return {"output": str(output), "status": receipt["status"], "restore_success": False}


def _verify_cache_receipt(backend: Path, directory: Path, descriptor: dict, subjects: list[dict]) -> dict:
    path = bound_file(backend, descriptor)
    if path != directory / "cache-cleanup.json":
        raise ValueError("来源授权缓存清理不属于同一 verification 目录")
    return _source_policy("runtime-cache-receipt", backend=str(backend), directory=str(directory),
                          value=read_json(path), subjects=subjects)


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
    _runtime_policy("facts-binding", directory=str(directory), generation=str(facts["output"]),
                    receipt=receipt, lineage=facts["receipt"]["dataset_lineage"])
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
