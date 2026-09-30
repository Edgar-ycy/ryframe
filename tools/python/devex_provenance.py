"""只读绑定 DevEx 样本与正式恢复 v3 运行代次；不授予恢复或写入权限。"""

from __future__ import annotations

import hashlib
import json
import re
import sys
from pathlib import Path

import restore_runtime
from restore_build import source_snapshot
from restore_runtime import _bind_control_inputs, _descriptor
from restore_runtime_evidence import (
    artifact_snapshot,
    canonical_endpoint,
    decode_object,
    directory,
    exact_fields,
    read_json_document,
    same_path,
    validate_authority,
    validate_runtime_receipt,
)
from restore_runtime_source import require_bindings
from source_fingerprints import (
    build_source,
    checked_source,
    current_execution_source,
    reusable_artifact_source,
)
from source_inventory import git


class ProvenanceError(ValueError):
    def __init__(self, stage: str):
        super().__init__(f"运行来源核验失败：{stage}")
        self.stage = stage


def checked(stage: str, operation):
    try:
        return operation()
    except Exception:
        # 原始异常可能带配置路径、连接信息或响应内容，报告只保留确定阶段。
        raise ProvenanceError(stage) from None


def absolute_directory(value: object, label: str) -> Path:
    if not isinstance(value, str) or not Path(value).is_absolute():
        raise ValueError(f"{label}必须是显式绝对目录")
    return directory(Path(value), label)


def request_roots(request: dict) -> tuple[Path, Path, Path, Path]:
    expected_fields = {
        "backend", "frontend", "driver", "runner_frontend", "driver_fingerprint",
        "runner_frontend_fingerprint", "scope_id", "source_fingerprints", "api_url",
        "frontend_url", "metrics_urls", "environment_sha256", "provenance",
    }
    fingerprints = request.get("source_fingerprints") if isinstance(request, dict) else None
    if (
        not isinstance(request, dict)
        or set(request) != expected_fields
        or not isinstance(request.get("driver_fingerprint"), str)
        or re.fullmatch(r"sha256:[a-f0-9]{64}", request["driver_fingerprint"]) is None
        or not isinstance(request.get("runner_frontend_fingerprint"), str)
        or re.fullmatch(r"sha256:[a-f0-9]{64}", request["runner_frontend_fingerprint"]) is None
        or not isinstance(fingerprints, dict)
        or set(fingerprints) != {"backend", "frontend", "runner_frontend"}
        or any(not isinstance(value, str) or re.fullmatch(r"sha256:[a-f0-9]{64}", value) is None
               for value in fingerprints.values())
        or fingerprints["runner_frontend"] != request["runner_frontend_fingerprint"]
        or not isinstance(request.get("environment_sha256"), str)
        or re.fullmatch(r"[a-f0-9]{64}", request["environment_sha256"]) is None
    ):
        raise ValueError("运行来源请求字段不完整")
    return tuple(absolute_directory(request[name], f"{name} 源码") for name in (
        "backend", "frontend", "driver", "runner_frontend"))


def bound_document(binding: object, allowed: Path, label: str):
    binding = exact_fields(binding, {"path", "bytes", "sha256"}, label)
    path_value = binding["path"]
    path = Path(path_value).absolute() if isinstance(path_value, str) else None
    if (
        path is None
        or not Path(path_value).is_absolute()
        or type(binding["bytes"]) is not int
        or binding["bytes"] <= 0
        or not isinstance(binding["sha256"], str)
        or re.fullmatch(r"[a-f0-9]{64}", binding["sha256"]) is None
        or not path.is_relative_to(allowed.absolute())
    ):
        raise ValueError(f"{label}没有绑定受控目录内的完整文件描述")
    document = read_json_document(path)
    if _descriptor(document) != binding:
        raise ValueError(f"{label}的路径、字节数或摘要已经变化")
    return document


def provenance_documents(request: dict, driver: Path) -> dict:
    bindings = exact_fields(
        request["provenance"],
        {"runtime_receipt", "target_plan", "runtime_registration", "runtime_launch",
         "environment_document"},
        "DevEx 运行来源绑定",
    )
    controlled = driver / ".local-tests"
    documents = {
        name: bound_document(bindings[name], controlled, label)
        for name, label in (
            ("runtime_receipt", "restore-runtime v3 收据"),
            ("target_plan", "正式恢复目标计划"),
            ("runtime_registration", "恢复运行登记"),
            ("runtime_launch", "恢复运行 launch"),
        )
    }
    validate_runtime_receipt(documents["runtime_receipt"].value)
    return documents


def environment_document(request: dict, backend: Path):
    value = request["provenance"]["environment_document"]
    path = Path(value).absolute() if isinstance(value, str) else None
    allowed = (backend / ".local-tests" / "devex").absolute()
    if path is None or not Path(value).is_absolute() or not path.is_relative_to(allowed):
        raise ValueError("环境说明必须位于被测后端的 .local-tests/devex")
    snapshot = artifact_snapshot(path)
    if (
        snapshot.bytes == 0
        or snapshot.bytes > 16 * 1024 * 1024
        or snapshot.sha256 != request["environment_sha256"]
    ):
        raise ValueError("环境说明必须是 16 MiB 内且摘要匹配的非空文件")
    try:
        content = path.read_bytes().decode("utf-8", errors="strict")
    except UnicodeDecodeError as error:
        raise ValueError("环境说明必须是 UTF-8") from error
    if not content.strip():
        raise ValueError("环境说明不能为空白文件")
    snapshot.assert_unchanged()
    return snapshot


def worktree_fingerprint(root: Path, commit: str) -> str:
    # 与 xtask metadata::worktree_fingerprint 使用完全相同的 Git 参数及长度前缀。
    digest = hashlib.sha256()

    def update(value: bytes) -> None:
        digest.update(len(value).to_bytes(8, "little"))
        digest.update(value)

    update(commit.encode())
    update(git(root, "diff", "--binary", "--no-ext-diff", "HEAD", "--", "."))
    for raw in git(root, "ls-files", "--others", "--exclude-standard", "-z").split(b"\0"):
        if not raw:
            continue
        relative = Path(raw.decode("utf-8"))
        path = root / relative
        if (
            relative.is_absolute()
            or ".." in relative.parts
            or path.is_symlink()
            or not path.resolve(strict=True).is_relative_to(root.resolve())
        ):
            raise ValueError("源码包含越界的未跟踪路径")
        update(raw)
        update(path.read_bytes())
    return "sha256:" + digest.hexdigest()


def verify_source(root: Path, receipt: dict, fingerprint: str) -> dict:
    original = reusable_artifact_source(root, receipt)
    if original is not None:
        if original["worktree_fingerprint"] != fingerprint:
            raise ValueError("请求没有绑定构建时的完整工作区指纹")
        return original["snapshot"]
    current = checked_source(root)
    source = current["snapshot"] if current is not None else source_snapshot(root)
    actual = (
        current["worktree_fingerprint"]
        if current is not None
        else worktree_fingerprint(root, source["head"])
    )
    expected = build_source(receipt)["snapshot"]
    if (
        expected != source
        or type(source.get("clean")) is not bool
        or not re.fullmatch(r"[a-f0-9]{40}", source["head"])
        or actual != fingerprint
    ):
        raise ValueError("实际源码与本侧精确构建快照或 DevEx 指纹不一致")
    return source


def runtime_authority(receipt: dict) -> dict:
    restore, source, endpoints = receipt["restore"], receipt["source"], receipt["endpoints"]
    return validate_authority({
        "format_version": 2,
        "kind": "restore-runtime-authority",
        "restore_id": restore["id"],
        "backup_id": restore["backup_id"],
        "plan_hash": restore["plan_hash"],
        "scope_id": restore["scope_id"],
        "data_verified_at": restore["data_verified_at"],
        **source,
        "api_endpoint": endpoints["api"],
        "worker_endpoint": endpoints["worker"],
        "frontend_endpoint": endpoints["frontend"],
    })


def verify_endpoints(request: dict, receipt: dict) -> None:
    api = canonical_endpoint(request["api_url"], "", "DevEx API 端点")
    frontend = canonical_endpoint(request["frontend_url"], "", "DevEx 前端端点")
    metrics = exact_fields(request["metrics_urls"], {"api", "worker"}, "DevEx 指标端点")
    ready = receipt["endpoints"]
    expected_api = ready["api"].removesuffix("/readyz")
    expected_worker = ready["worker"].removesuffix("/readyz")
    expected_metrics = {"api": expected_api + "/metrics", "worker": expected_worker + "/metrics"}
    actual_metrics = {
        role: canonical_endpoint(metrics[role], "/metrics", f"{role} 指标端点")
        for role in ("api", "worker")
    }
    if (
        request["scope_id"] != receipt["restore"]["scope_id"]
        or api != expected_api
        or frontend != ready["frontend"]
        or actual_metrics != expected_metrics
    ):
        raise ValueError("DevEx scope、业务地址或指标地址与 restore-runtime 不同")


def control_snapshot(driver: Path, documents: dict) -> tuple[dict, tuple]:
    launch_document = documents["runtime_launch"]
    root, binding, launch, observed = _bind_control_inputs(driver, launch_document.path)
    expected = {
        "registration": _descriptor(documents["runtime_registration"]),
        "target_plan": _descriptor(documents["target_plan"]),
    }
    receipt = documents["runtime_receipt"].value
    if (
        binding != expected
        or launch != launch_document.value
        or _descriptor(observed[-1]) != _descriptor(launch_document)
    ):
        raise ValueError("显式登记、目标计划或 launch 与当前运行代次不同")
    same_path(receipt["paths"]["runtime_dir"], Path(launch["runtime_directory"]), "运行代次")
    same_path(receipt["paths"]["launch"], launch_document.path, "launch")
    if receipt["digests"]["launch"] != launch_document.sha256:
        raise ValueError("restore-runtime 没有绑定显式 launch 摘要")
    by_path = {item.path: item for item in observed}
    for name in ("target_plan", "runtime_registration", "runtime_launch"):
        explicit = documents[name]
        current = by_path.get(explicit.path)
        if current is None or current.raw != explicit.raw or current.state != explicit.state:
            raise ValueError("运行控制链没有读取显式绑定的同一文件快照")
    lifecycle = [
        item for item in observed if item.value.get("kind") == "restore-runtime-lifecycle"
    ]
    if len(lifecycle) != 1:
        raise ValueError("运行控制链缺少唯一 lifecycle")
    snapshot = {
        "root": str(root),
        "binding": binding,
        "generation": launch["generation"],
        "launch": _descriptor(launch_document),
        "lifecycle": _descriptor(lifecycle[0]),
        "documents": [_descriptor(item) for item in observed],
    }
    return snapshot, observed


def verify_runtime_chain(
    driver: Path, backend: Path, frontend: Path, documents: dict
) -> tuple[dict, dict]:
    from restore_runtime_static import verify_static_runtime

    receipt = documents["runtime_receipt"].value
    first, first_documents = control_snapshot(driver, documents)
    bindings = read_json_document(Path(receipt["paths"]["bindings"]))
    _record, manifest = require_bindings(bindings.value)
    references = [
        document
        for document in first_documents
        if document.value.get("kind") == "restore-reference-plan"
    ]
    if len(references) != 1:
        raise ValueError("运行控制链缺少唯一参考恢复计划")
    reference = references[0].value
    verify_static_runtime(
        driver,
        receipt,
        documents["target_plan"],
        documents["target_plan"].value,
        reference,
        manifest,
    )
    authority = runtime_authority(receipt)
    verification = restore_runtime.verify(
        receipt,
        driver,
        backend,
        frontend,
        Path(receipt["paths"]["bindings"]),
        authority,
        documents["runtime_receipt"].sha256,
        product_backend=(
            Path(receipt["paths"]["backend_product_root"])
            if receipt["source"]["backend_adapter_contract"] is not None
            else None
        ),
    )
    second, second_documents = control_snapshot(driver, documents)
    if second != first:
        raise ValueError("核验期间恢复运行 lifecycle 或 generation 发生变化")
    for document in (*first_documents, *second_documents, bindings, *documents.values()):
        document.assert_unchanged()
    return first, verification


def source_state(root: Path, expected_sha: str | None = None) -> dict:
    state = current_execution_source(root)
    snapshot = state["snapshot"]
    if (
        expected_sha is not None
        and (snapshot.get("head") != expected_sha or snapshot.get("clean") is not True)
    ):
        raise ValueError("restore-runtime 产品来源不是精确干净 SHA")
    return state


def verify(request: dict) -> dict:
    backend, frontend, driver, runner_frontend = checked(
        "bindings", lambda: request_roots(request)
    )
    documents = checked(
        "runtime_receipts", lambda: provenance_documents(request, driver)
    )
    receipt = documents["runtime_receipt"].value
    checked(
        "runtime_roots",
        lambda: same_path(
            receipt["paths"]["backend_execution_root"], backend, "后端执行源码"
        ),
    )
    checked(
        "runtime_roots",
        lambda: same_path(receipt["paths"]["frontend_root"], frontend, "前端源码"),
    )
    checked("runtime_endpoints", lambda: verify_endpoints(request, receipt))
    environment = checked(
        "environment_document", lambda: environment_document(request, backend)
    )
    sources = {
        "backend_execution": checked(
            "backend_source",
            lambda: verify_source(
                backend,
                receipt["backend"],
                request["source_fingerprints"]["backend"],
            ),
        ),
        "frontend": checked(
            "frontend_source",
            lambda: verify_source(
                frontend,
                receipt["frontend"],
                request["source_fingerprints"]["frontend"],
            ),
        ),
    }
    execution_sources = {
        "driver": checked("driver_source", lambda: source_state(driver)),
        "runner_frontend": checked(
            "runner_frontend_source", lambda: source_state(runner_frontend)
        ),
    }
    if (
        execution_sources["driver"]["worktree_fingerprint"]
        != request["driver_fingerprint"]
    ):
        raise ProvenanceError("driver_source")
    if (
        execution_sources["runner_frontend"]["worktree_fingerprint"]
        != request["runner_frontend_fingerprint"]
    ):
        raise ProvenanceError("runner_frontend_source")
    product_backend = Path(receipt["paths"]["backend_product_root"])
    product_source = checked(
        "backend_product_source",
        lambda: source_state(
            product_backend, receipt["source"]["backend_product_sha"]
        ),
    )
    sources.update({
        "backend_product": product_source["snapshot"],
        "driver": execution_sources["driver"]["snapshot"],
        "runner_frontend": execution_sources["runner_frontend"]["snapshot"],
    })
    control, runtime = checked(
        "runtime_generation",
        lambda: verify_runtime_chain(driver, backend, frontend, documents),
    )
    for role, root, embedded in (
        ("backend", backend, receipt["backend"]),
        ("frontend", frontend, receipt["frontend"]),
    ):
        checked(
            f"{role}_source_stable",
            lambda root=root, embedded=embedded, role=role: verify_source(
                root, embedded, request["source_fingerprints"][role]
            ),
        )
    for role, root, fingerprint in (
        ("driver", driver, request["driver_fingerprint"]),
        (
            "runner_frontend",
            runner_frontend,
            request["runner_frontend_fingerprint"],
        ),
    ):
        repeated = checked(
            f"{role}_source_stable", lambda root=root: source_state(root)
        )
        if (
            repeated != execution_sources[role]
            or repeated["worktree_fingerprint"] != fingerprint
        ):
            raise ProvenanceError(f"{role}_source_stable")
    repeated_product = checked(
        "backend_product_source_stable",
        lambda: source_state(
            product_backend, receipt["source"]["backend_product_sha"]
        ),
    )
    if repeated_product != product_source:
        raise ProvenanceError("backend_product_source_stable")
    environment.assert_unchanged()
    for document in documents.values():
        document.assert_unchanged()
    return {
        "format_version": 3,
        "kind": "devex-runtime-provenance",
        "scope_id": request["scope_id"],
        "roots": {
            "backend_product": str(product_backend),
            "backend_execution": str(backend),
            "frontend": str(frontend),
            "driver": str(driver),
            "runner_frontend": str(runner_frontend),
        },
        "sources": sources,
        "source_fingerprints": request["source_fingerprints"],
        "execution_sources": execution_sources,
        "receipts": {
            name: _descriptor(document) for name, document in documents.items()
        },
        "runtime": {
            "generation": control["generation"],
            "lifecycle": control["lifecycle"],
            "verification": runtime,
        },
        "processes": runtime["processes"],
        "environment_document": environment.descriptor(),
        "environment_evidence": "operator_declared_document",
        "api_url": request["api_url"],
        "frontend_url": request["frontend_url"],
        "metrics_urls": request["metrics_urls"],
    }


def main() -> None:
    try:
        content = sys.stdin.buffer.read(1024 * 1024 + 1)
        if len(content) > 1024 * 1024:
            raise ProvenanceError("bindings")
        request = checked("bindings", lambda: decode_object(content, "DevEx 运行来源请求"))
        result = {"ok": True, "receipt": verify(request)}
    except ProvenanceError as error:
        result = {"ok": False, "stage": error.stage}
    except Exception:
        result = {"ok": False, "stage": "bindings"}
    print(json.dumps(result, ensure_ascii=False))
    if not result["ok"]:
        raise SystemExit(1)


if __name__ == "__main__":
    main()
