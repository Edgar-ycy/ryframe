"""从正式恢复目标与仍存活的运行代次推导监控验收权威。"""

from __future__ import annotations

from pathlib import Path

from restore_build import repository
from restore_business_proof import _verified_target
from restore_runtime import verify_live_generation
from restore_runtime_evidence import (
    artifact_snapshot,
    canonical_endpoint,
    exact_fields,
    read_json_document,
    validate_runtime_receipt,
)
from restore_runtime_static import verify_static_runtime


def _descriptor(document) -> dict:
    snapshot = artifact_snapshot(document.path)
    if snapshot.sha256 != document.sha256:
        raise ValueError("监控权威输入在摘要采集期间发生变化")
    return snapshot.descriptor()


def _metrics_endpoints(runtime: dict) -> dict:
    api_ready = canonical_endpoint(runtime["endpoints"]["api"], "/readyz", "监控 API 探针")
    worker_ready = canonical_endpoint(runtime["endpoints"]["worker"], "/readyz", "监控 Worker 探针")
    frontend = canonical_endpoint(runtime["endpoints"]["frontend"], "", "监控前端端点")
    return {
        "api_ready": api_ready,
        "worker_ready": worker_ready,
        "frontend": frontend,
        "api_metrics": canonical_endpoint(
            api_ready.removesuffix("/readyz") + "/api/v1/monitor/metrics",
            "/api/v1/monitor/metrics",
            "API 指标端点",
        ),
        "worker_metrics": canonical_endpoint(
            worker_ready.removesuffix("/readyz") + "/metrics",
            "/metrics",
            "Worker 指标端点",
        ),
    }


def monitoring_preflight(backend: Path, runtime_path: Path, target_path: Path) -> dict:
    """在任何监控进程启动前，主动复核正式来源和仍运行的三端代次。"""
    backend = repository(backend, "监控验收协调后端")
    runtime_document = read_json_document(runtime_path)
    runtime = validate_runtime_receipt(runtime_document.value)
    target_document, target, reference, manifest, dataset = _verified_target(
        backend, target_path, runtime
    )
    authority = verify_static_runtime(
        backend, runtime, target_document, target, reference, manifest
    )
    roots = target["product_execution"]["roots"]
    product = Path(roots["source_backend"])
    execution = Path(roots["execution_backend"])
    frontend = Path(roots["frontend"])
    live = verify_live_generation(
        runtime,
        backend,
        execution,
        frontend,
        Path(runtime["paths"]["bindings"]),
        authority,
        runtime_document.sha256,
        product_backend=product if product != execution else None,
    )
    if live["runtime_receipt_sha256"] != runtime_document.sha256:
        raise ValueError("监控预检没有主动核验当前运行收据")

    launch_document = read_json_document(Path(runtime["paths"]["launch"]))
    if (
        _descriptor(launch_document)
        != {
            "path": runtime["paths"]["launch"],
            "bytes": len(launch_document.raw),
            "sha256": runtime["digests"]["launch"],
        }
    ):
        raise ValueError("监控预检的启动收据与运行绑定不同")
    request = exact_fields(
        launch_document.value["request"],
        {
            "format_version",
            "kind",
            "authority",
            "registration",
            "roots",
            "paths",
            "digests",
            "artifacts",
            "tools",
            "environment",
        },
        "监控预检启动请求",
    )
    environment = exact_fields(
        request["environment"], {"document", "variables", "sha256"}, "监控预检运行环境"
    )
    result = {
        "format_version": 1,
        "kind": "restore-monitoring-authority",
        "runtime": _descriptor(runtime_document),
        "target_plan": _descriptor(target_document),
        "launch": _descriptor(launch_document),
        "environment": environment["document"],
        "restore": runtime["restore"],
        "source": runtime["source"],
        "endpoints": _metrics_endpoints(runtime),
        "maintenance_execution": target["maintenance_execution"],
        "product_execution": target["product_execution"],
        "processes": {
            role: runtime["processes"][role]["identity"]
            for role in ("api", "worker", "frontend")
        },
        "source_generation": dataset["source_generation"],
        "dataset_lineage": dataset["dataset_lineage"],
    }
    for document in (runtime_document, target_document, launch_document):
        document.assert_unchanged()
    return result
