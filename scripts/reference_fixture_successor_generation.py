"""从同一 C52 successor 与注册构建生成只读源后继请求；仅显式发布写入文件。"""
from __future__ import annotations

from pathlib import Path

from devex_clone_capture import read_json
from process_environment import configured
from devex_clone_model import exact, local_path, name
from devex_clone_run_state import binding
from devex_clone_seed_generation_runtime import registered_inputs
from devex_clone_source_proof import bound_file
from devex_clone_tools import verify_evidence
from reference_fixture_successor import published_source
from reference_fixture_successor_arm import _publish_json
from restore_reference_plan import plan_hash
from restore_source_binding import source_binding


def build(backend: Path, successor: Path, execution: Path, expected_head: str, backend_build: Path,
          maintenance_build: Path, environment: Path, request_id: str, *, adapter_contract=None, product_backend=None) -> dict:
    backend = backend.resolve(strict=True)
    name(request_id)
    relationship, build_path, environment_path = [local_path(backend, str(path)) for path in (successor, backend_build, environment)]
    maintenance_path = local_path(execution, str(maintenance_build))
    source = published_source(backend, binding(relationship), live_storage=False)
    if source.get("source_rebind") is None or source.get("source_generation") is not None:
        raise ValueError("generation-request 必须在重绑定后、首个 source-generation 前构造")
    value = {"format_version": 1, "kind": "devex-clone-seed-source-generation", "id": request_id,
             "source_registration": source["review_successor"]["source_result"],
             "review_successor": binding(relationship), "source_rebind": source["source_rebind"],
             "current_storage": source["storage"]["storage"], "execution_backend": str(execution),
             "expected_backend_sha": expected_head, "adapter_contract": adapter_contract,
             "product_backend": None if product_backend is None else str(product_backend),
             "backend_build": binding(build_path), "maintenance_build": binding(maintenance_path),
             "source_environment": binding(environment_path)}
    root, receipt = registered_inputs(backend, value, reconstruct=False)
    maintenance = verify_evidence(root, maintenance_path)
    private = read_json(environment_path)
    exact(private, {"environment"})
    if (maintenance["source"] != receipt["sources"]["full"]["source"]
            or source_binding(root, {"source": source["request"]["source"]}, configured(private["environment"]), evidence_root=backend)
            != source["generation"]["physical_binding"]):
        raise ValueError("后继请求维护构建或环境没有绑定同一源码与 C52 完整物理来源")
    if published_source(backend, binding(relationship), live_storage=False) != source:
        raise ValueError("generation-request 来源在只读构造期间变化")
    for field in ("review_successor", "backend_build", "source_environment"):
        bound_file(backend, value[field])
    bound_file(root, value["maintenance_build"])
    return value


def rebuild(backend: Path, request: dict) -> dict:
    return build(backend, Path(request["review_successor"]["path"]), Path(request["execution_backend"]),
                 request["expected_backend_sha"], Path(request["backend_build"]["path"]),
                 Path(request["maintenance_build"]["path"]), Path(request["source_environment"]["path"]), request["id"],
                 adapter_contract=request["adapter_contract"],
                 product_backend=None if request["product_backend"] is None else Path(request["product_backend"]))


def publish(backend: Path, output: Path, *args, **kwargs) -> dict:
    target = local_path(backend, str(output), new=True)
    if not target.parent.is_dir():
        raise ValueError("generation-request 输出必须使用已有父目录中的新路径")
    first = build(backend, *args, **kwargs)
    if build(backend, *args, **kwargs) != first:
        raise ValueError("generation-request 发布前输入发生变化")
    _publish_json(target, first)
    if read_json(target) != first or build(backend, *args, **kwargs) != first:
        raise ValueError("generation-request 发布后发生变化；保留证据禁止重放")
    return first


def arguments(parser) -> None:
    for field in ("backend-dir", "successor", "source-backend", "backend-build", "maintenance-build", "source-environment"):
        parser.add_argument("--" + field, type=Path, required=True)
    parser.add_argument("--expected-head", required=True)
    parser.add_argument("--id", required=True)
    parser.add_argument("--adapter-contract")
    parser.add_argument("--product-backend", type=Path)
    parser.add_argument("--output", type=Path)
    parser.add_argument("--write", action="store_true")


def execute(args, parser) -> dict:
    if args.write != (args.output is not None):
        parser.error("generation-request 只读预览不落盘；发布必须同时指定 --output 与 --write")
    values = (args.successor, args.source_backend, args.expected_head, args.backend_build,
              args.maintenance_build, args.source_environment, args.id)
    keywords = {"adapter_contract": args.adapter_contract, "product_backend": args.product_backend}
    value = publish(args.backend_dir, args.output, *values, **keywords) if args.write else build(args.backend_dir, *values, **keywords)
    return {"status": "source_generation_request_published" if args.write else "source_generation_request_planned",
            "request": value, "request_sha256": plan_hash(value), "remote_writes": 0}
