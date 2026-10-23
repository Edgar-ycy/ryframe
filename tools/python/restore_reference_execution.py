"""恢复执行前固定产品运行记录、严格目标计划与同一备份文件快照。"""

from dataclasses import dataclass
from pathlib import Path

from devex_clone_model import local_path
from restore_reference_backup import bound_document, document_binding
from restore_reference_plan import _native
from restore_reference_target import verify_target_plan
from restore_runtime_evidence import JsonDocument, read_json_document
from restore_runtime_registration import verify_registration


def product_plan_hash(value: dict) -> str:
    return _native("reference-product-hash", {"value": value})


def validate_restore_record(plan: dict, manifest: dict, record: dict, product_plan: dict) -> None:
    _native("reference-restore-record", {
        "plan": plan, "manifest": manifest, "record": record, "product_plan": product_plan,
    })


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
