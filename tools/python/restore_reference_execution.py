"""恢复执行前固定产品运行记录、严格目标计划与同一备份文件快照。"""

from dataclasses import dataclass
import datetime as dt
import hashlib
import json
from pathlib import Path

from devex_clone_model import exact, local_path
from restore_reference_backup import bound_document, document_binding
from restore_reference_plan import identifier, scope_identifier
from restore_reference_target import PRODUCT_FIELDS, verify_target_plan
from restore_runtime_evidence import JsonDocument, read_json_document, timestamp
from restore_runtime_registration import verify_registration


def product_plan_hash(value: dict) -> str:
    """与 Rust RestorePlan/RestoreDatabase 的 serde 字段顺序保持一致。"""
    fields = ("id", "backup_id", "scope_id", "fault_at", "databases", "object_endpoint",
              "object_prefix", "api_ready_url", "worker_ready_url", "frontend_sha")
    ordered = {key: value[key] for key in fields}
    ordered["databases"] = [{key: db[key] for key in ("source_key", "target_key", "server_uuid", "database")}
                            for db in value["databases"]]
    return hashlib.sha256(json.dumps(ordered, ensure_ascii=False, separators=(",", ":")).encode()).hexdigest()


def validate_restore_record(plan: dict, manifest: dict, record: dict, product_plan: dict) -> None:
    exact(record, {"plan", "plan_hash", "status", "started_at", "data_verified_at", "completed_at", "recovered_at", "failure"})
    exact(record["plan"], PRODUCT_FIELDS)
    if record["plan"] != product_plan or record["plan_hash"] != product_plan_hash(product_plan):
        raise ValueError("restore-begin 记录与目标计划内嵌产品计划或产品摘要不完全一致")
    target, restore_plan = plan["target"], record["plan"]
    identifier(restore_plan["id"])
    identifier(restore_plan["backup_id"])
    scope_identifier(restore_plan["scope_id"])
    for database in restore_plan["databases"]:
        exact(database, {"source_key", "target_key", "server_uuid", "database"})
    expected = {(db["key"], db["key"], db["server_uuid"], db["database"]) for db in target["databases"]}
    actual = {(db["source_key"], db["target_key"], db["server_uuid"], db["database"]) for db in restore_plan["databases"]}
    if (record["status"] != "running" or restore_plan["backup_id"] != manifest["id"]
            or restore_plan["backup_id"] != plan["id"] or restore_plan["scope_id"] != target["scope_id"]
            or actual != expected or len(restore_plan["databases"]) != len(expected)
            or restore_plan["object_endpoint"] != target["s3"]["endpoint"]
            or restore_plan["object_prefix"] != target["scope_id"] + "/"
            or any(record[key] is not None for key in ("data_verified_at", "completed_at", "failure"))):
        raise ValueError("还原必须绑定 restore-begin 的运行中记录和相同精确目标")
    for field in ("started_at", "recovered_at"):
        timestamp(record[field], "恢复运行记录时间")
    started = dt.datetime.fromisoformat(record["started_at"].replace("Z", "+00:00"))
    recovered = dt.datetime.fromisoformat(record["recovered_at"].replace("Z", "+00:00"))
    captured = dt.datetime.fromisoformat(manifest["captured_at"].replace("Z", "+00:00"))
    if recovered != captured or recovered > started:
        raise ValueError("恢复记录的恢复点与同一备份清单不一致")
    if not 0 <= (dt.datetime.now(dt.timezone.utc) - started).total_seconds() <= 3600:
        raise ValueError("恢复操作尚未开始、时钟回退或已超过 60 分钟")


@dataclass(frozen=True)
class RestoreInputs:
    root: Path
    target: JsonDocument
    backup: JsonDocument
    manifest: JsonDocument
    record: JsonDocument
    registration: JsonDocument

    def assert_unchanged(self) -> None:
        for document in (self.target, self.backup, self.manifest, self.record, self.registration):
            document.assert_unchanged()

    def bindings(self) -> dict:
        return {"target_side": self.target.value["target_side"], "target_plan": document_binding(self.target),
                "backup_receipt": document_binding(self.backup), "record": document_binding(self.record),
                "runtime_registration": document_binding(self.registration)}


def restore_inputs(backend: Path, plan: dict, target_path: Path, root: Path, record_path: Path,
                   registration_path: Path, *, read_only: bool) -> RestoreInputs:
    target = read_json_document(local_path(backend, str(target_path.absolute())))
    value = verify_target_plan(backend, plan, target.path, read_only=read_only)
    if value != target.value:
        raise ValueError("恢复目标计划在读取与核验之间发生变化")
    backup = bound_document(backend, value["backup_receipt"])
    manifest = bound_document(backend, backup.value["result"]["manifest"])
    root = local_path(backend, str(root.absolute()))
    if root != Path(backup.value["result"]["backup_root"]) or manifest.path != root / "manifest.json":
        raise ValueError("恢复只能消费目标计划绑定的同一正式备份目录")
    record = read_json_document(local_path(backend, str(record_path.absolute())))
    validate_restore_record(plan, manifest.value, record.value, value["product_plan"])
    registration = read_json_document(local_path(backend, str(registration_path.absolute())))
    _registered, _facts, documents = verify_registration(backend, registration.path, document_binding(target))
    if document_binding(documents[0]) != document_binding(registration):
        raise ValueError("恢复运行登记在输入读取期间变化")
    result = RestoreInputs(root, target, backup, manifest, record, registration)
    result.assert_unchanged()
    return result


def restore_output(backend: Path, plan: dict, inputs: RestoreInputs) -> Path:
    output = local_path(backend, str(Path(plan["work_dir"]) / f"restore-{plan['target_side']}.json"), new=True)
    if (output.is_relative_to(inputs.root)
            or output.is_relative_to(Path(inputs.target.value["fresh_target"]["initialized"]["path"]).parent)):
        raise ValueError("恢复输出不能覆盖备份或 fresh-target 初始化证据")
    for phase in ("before", "after", "failure-after"):
        local_path(backend, str(output.parent / f"restore-{plan['target_side']}-{phase}"), new=True)
    return output
