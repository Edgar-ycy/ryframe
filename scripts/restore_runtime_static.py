"""复用运行与生命周期模型，只读交叉核验浏览器会话前的完整静态权威。"""

from pathlib import Path

from restore_runtime import ROLES, _bind_control_inputs, _descriptor, _receipt_documents
from restore_runtime_evidence import same_path, validate_runtime_receipt
from restore_runtime_launch import _target_matches
from restore_runtime_source import require_bindings, runtime_authority


def verify_static_runtime(backend: Path, receipt: dict, target_document, target: dict,
                          reference: dict, manifest: dict) -> None:
    """不探测服务、不创建锁；输入必须已由完整 target-plan validator 验证。"""
    receipt = validate_runtime_receipt(receipt)
    paths = receipt["paths"]
    bindings, backend_build, frontend_build, launch_document = _receipt_documents(
        receipt, Path(paths["bindings"])
    )
    same_path(paths["launch"], Path(paths["runtime_dir"]) / "runtime-launch.json", "恢复启动收据")
    _root, registration, launch, documents = _bind_control_inputs(backend, launch_document.path)
    if (launch != launch_document.value or registration["target_plan"] != _descriptor(target_document)
            or _descriptor(documents[-1]) != _descriptor(launch_document)):
        raise ValueError("恢复预检的 launch、lifecycle 与显式目标计划不是同一文件快照")
    record, bound_manifest = require_bindings(bindings.value)
    if record["plan"] != target["product_plan"] or bound_manifest != manifest:
        raise ValueError("恢复预检 bindings 与目标产品计划或唯一备份清单不一致")
    execution = target["product_execution"]
    roots = execution["roots"]
    source = {key: execution[key] for key in ("backend_product_sha", "backend_execution_sha", "frontend_sha")}
    source["backend_adapter_contract"] = None if execution["adapter"] is None else execution["adapter"]["contract"]
    resolved = {"roots": {"backend_product": roots["source_backend"],
                          "backend_execution": roots["execution_backend"], "frontend": roots["frontend"]},
                "source": source}
    authority = runtime_authority(record, manifest, source, reference["target"]["frontend_url"])
    endpoints = {role: authority[role + "_endpoint"] for role in ROLES}
    _target_matches(authority, {"product_plan": target["product_plan"], "endpoints": endpoints,
                                "product_execution": execution}, resolved, backend_build, frontend_build)
    expected_restore = {key: authority[source_key] for key, source_key in (
        ("id", "restore_id"), ("backup_id", "backup_id"), ("plan_hash", "plan_hash"),
        ("scope_id", "scope_id"), ("data_verified_at", "data_verified_at"))}
    if (receipt["restore"] != expected_restore
            or receipt["source"] != {"backup_source_sha": manifest["source_sha"], **source}
            or receipt["endpoints"] != endpoints):
        raise ValueError("恢复预检运行收据的恢复身份、来源或端点与权威计划不同")
    for role, path_key in (("backend_product", "backend_product_root"),
                           ("backend_execution", "backend_execution_root"), ("frontend", "frontend_root")):
        same_path(paths[path_key], Path(resolved["roots"][role]), "恢复预检产品来源")
    request = launch["request"]
    if (request["authority"] != authority or request["roots"] != resolved["roots"]
            or request["paths"] != {key: paths[key] for key in ("bindings", "backend_build", "frontend_build")}
            or request["digests"] != {key: receipt["digests"][key] for key in request["paths"]}
            or request["artifacts"] != receipt["backend"]["artifacts"]):
        raise ValueError("恢复预检 launch 的来源、构建或权威与运行收据不同")
    for role in ROLES:
        process = launch["processes"][role]["process_receipt"]
        if receipt["processes"][role] != {"receipt_path": process["path"],
                                         "receipt_sha256": process["sha256"], "identity": process["identity"]}:
            raise ValueError("恢复预检运行收据的进程与完整 launch 进程树不一致")
    for document in (bindings, backend_build, frontend_build, launch_document, target_document, *documents):
        document.assert_unchanged()
