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
from devex_clone_factory_context import Environments, configured
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


FORMAT_VERSION = 2
RECEIPT_FIELDS = {
    "format_version", "kind", "status", "source_generation", "dataset_lineage",
    "before", "after", "producer", "verification", "login_audit", "cache_cleanup",
    "evidence", "write_effects", "started_at", "verified_at", "remote_writes",
    "restore_qualified",
}
PRODUCER_BINDING_FIELDS = {"intent", "process", "ready", "stdout", "stderr", "completion"}
ROOT_ENTRIES = {
    "before", "after", "audit", STAGING_DIRECTORY, INTENT, PROCESS, READY, STDOUT, STDERR, COMPLETION,
    "cache-cleanup.json", "source-runtime.json",
}
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


def _cache_before(resources, plan: list[dict]) -> list[dict]:
    result = []
    for item in plan:
        keys = item["keys"]
        types = {name: resources._redis(["TYPE", key]) for name, key in keys.items()}
        if types != {"tenant_epoch": "string", "user_version": "string", "snapshot": "hash"}:
            raise ValueError("来源授权缓存缺少本次精确三键")
        tenant_epoch = resources._redis(["GET", keys["tenant_epoch"]])
        user_version = resources._redis(["GET", keys["user_version"]])
        fields = resources._redis(["HKEYS", keys["snapshot"]])
        if (
            not isinstance(tenant_epoch, str)
            or re.fullmatch(r"[1-9][0-9]{0,9}", tenant_epoch) is None
            or not isinstance(user_version, str)
            or user_version != str(item["user_authorization_version"])
            or fields != [f"{tenant_epoch}:{user_version}"]
        ):
            raise ValueError("来源授权缓存版本或快照字段不同")
        payload = resources._redis(["HGET", keys["snapshot"], fields[0]])
        if not isinstance(payload, str):
            raise ValueError("来源授权缓存快照为空")
        result.append({
            **{key: item[key] for key in ("tenant_id", "user_id", "user_authorization_version")},
            "tenant_epoch": {"key": keys["tenant_epoch"], "value": tenant_epoch},
            "user_version": {"key": keys["user_version"], "value": user_version},
            "snapshot": {"key": keys["snapshot"], "field": fields[0],
                         **_snapshot_identity(payload, item, tenant_epoch)},
        })
    return result


def _clean_authorization_cache(resources, namespace: str, subjects: list[dict], output: Path) -> dict:
    plan = authorization_cache_plan(namespace, subjects)
    before = _cache_before(resources, plan)
    keys = [key for item in plan for key in item["keys"].values()]
    deleted = resources._redis(["UNLINK", *keys])
    after = [{"key": key, "type": resources._redis(["TYPE", key])} for key in keys]
    if deleted != 33 or any(item["type"] != "none" for item in after):
        raise ValueError("来源授权缓存没有精确删除本次 33 个键")
    receipt = {
        "format_version": 1,
        "kind": "restore-source-authorization-cache-cleanup",
        "status": "cleaned",
        "namespace": namespace,
        "before": before,
        "deleted": deleted,
        "after": after,
        "cleaned_at": _timestamp(),
    }
    write_json(output, receipt)
    return receipt


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
                )
                after_value = verify_image(
                    backend, after, facts["selected"], facts["source"]["request"],
                    source_registration=facts["receipt"]["source_registration"],
                )
            effects = image_write_effects(before_value["image"], after_value["image"])
            if cleanup["deleted"] != effects["authorization_cache"]["created_and_removed_keys"]:
                raise ValueError("来源验收授权缓存清理与完整前后像合同不同")
            receipt = {
                "format_version": FORMAT_VERSION,
                "kind": "restore-source-runtime",
                "status": "source_runtime_verified",
                "source_generation": start,
                "dataset_lineage": facts["receipt"]["dataset_lineage"],
                "before": before,
                "after": after,
                "producer": _producer_bindings(expected_directory),
                "verification": binding(expected_directory / STDOUT),
                "login_audit": binding(expected_directory / "audit/login-audit.json"),
                "cache_cleanup": binding(expected_directory / "cache-cleanup.json"),
                "evidence": {
                    "before": _manifest(expected_directory / "before"),
                    "after": _manifest(expected_directory / "after"),
                    "audit": _manifest(expected_directory / "audit"),
                },
                "write_effects": effects,
                "started_at": started_at,
                "verified_at": _timestamp(),
                "remote_writes": 23,
                "restore_qualified": False,
            }
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


def _verify_cache_receipt(backend: Path, directory: Path, descriptor: dict, subjects: list[dict]) -> dict:
    path = bound_file(backend, descriptor)
    if path != directory / "cache-cleanup.json":
        raise ValueError("来源授权缓存清理不属于同一 verification 目录")
    value = read_json(path)
    exact_fields(value, {"format_version", "kind", "status", "namespace", "before", "deleted", "after", "cleaned_at"},
                 "来源授权缓存清理")
    plan = authorization_cache_plan(value["namespace"], subjects)
    expected_keys = [key for item in plan for key in item["keys"].values()]
    if (
        type(value["format_version"]) is not int
        or value["format_version"] != 1
        or value["kind"] != "restore-source-authorization-cache-cleanup"
        or value["status"] != "cleaned"
        or type(value["deleted"]) is not int
        or value["deleted"] != 33
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
    for actual, expected in zip(value["before"], plan, strict=True):
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
        snapshot = exact_fields(
            actual["snapshot"], {"key", "field", "bytes", "sha256"},
            "来源授权缓存快照",
        )
        if (
            tenant_epoch["key"] != keys["tenant_epoch"]
            or not isinstance(tenant_epoch["value"], str)
            or re.fullmatch(r"[1-9][0-9]{0,9}", tenant_epoch["value"]) is None
            or user_version != {
                "key": keys["user_version"],
                "value": str(expected["user_authorization_version"]),
            }
            or snapshot["key"] != keys["snapshot"]
            or snapshot["field"]
            != f"{tenant_epoch['value']}:{user_version['value']}"
            or type(snapshot["bytes"]) is not int
            or snapshot["bytes"] <= 0
            or not isinstance(snapshot["sha256"], str)
            or re.fullmatch(r"[a-f0-9]{64}", snapshot["sha256"]) is None
        ):
            raise ValueError("来源授权缓存清理键或版本不同")
    timestamp(value["cleaned_at"], "来源授权缓存清理时间")
    return value


def verify_source_runtime(backend: Path, descriptor: dict, *, live: bool) -> dict:
    """唯一只读 validator；live 只控制 source generation 当前进程断言，返回静态事实。"""
    from devex_clone_seed_generation import verify_running_source
    from devex_clone_seed_generation_images import verify_image

    path = bound_file(backend, descriptor)
    directory = path.parent
    if path != directory / "source-runtime.json" or directory.name != "verification":
        raise ValueError("source-runtime 不属于固定同代 verification 目录")
    receipt = read_json(path)
    exact_fields(receipt, RECEIPT_FIELDS, "来源运行收据")
    if (
        type(receipt["format_version"]) is not int
        or receipt["format_version"] != FORMAT_VERSION
        or receipt["kind"] != "restore-source-runtime"
        or receipt["status"] != "source_runtime_verified"
        or receipt["remote_writes"] != 23
        or receipt["restore_qualified"] is not False
        or set(receipt["producer"]) != PRODUCER_BINDING_FIELDS
        or {entry.name for entry in directory.iterdir()} != ROOT_ENTRIES
    ):
        raise ValueError("来源运行收据版本、状态或证据集合无效")
    facts = verify_running_source(backend, receipt["source_generation"], live=live)
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
    audit = _read_login_audit(backend, directory, receipt["login_audit"])
    old = (directory / "audit/login-before.tsv").read_bytes()
    old_after = (directory / "audit/login-after-old.tsv").read_bytes()
    new_rows = _login_rows((directory / "audit/login-after-new.tsv").read_text(encoding="utf-8").strip())
    verified_audit = validate_login_audit(
        old, old_after, int(audit["verified"]["upper_id"]), new_rows, facts["lineage"],
        verified_node["subjects"], receipt["started_at"], audit["completed_at"],
    )
    if audit["verified"] != verified_audit:
        raise ValueError("来源登录审计派生结果发生变化")
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
    if receipt["write_effects"] != effects or cleanup["deleted"] != 33:
        raise ValueError("来源运行收据的业务副作用与完整像不同")
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
    }
