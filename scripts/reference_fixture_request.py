"""从已准备的夹具环境和首代服务账本签发 fresh-target 请求。"""

from __future__ import annotations

import argparse
import json
from pathlib import Path
import re

from devex_clone_capture import read_json, write_json
from devex_clone_model import linked, local_path
from devex_clone_target_binding import KEYS, request_binding, validate_review
from restore_build import file_digest
from restore_reference_plan import plan_hash
from reference_fixture_paths import service_run as expected_service_run


def _read(backend: Path, value: Path) -> tuple[Path, dict]:
    requested = value if value.is_absolute() else backend / value
    path = local_path(backend, str(requested))
    if linked(path) or not path.is_file():
        raise ValueError("fresh-target 请求输入必须是受控普通文件")
    return path, read_json(path)


def _bound(path: Path) -> dict:
    return {"path": str(path), **file_digest(path)}


def _service(backend: Path, run: Path, review_binding: dict, bootstrap_binding: dict) -> dict:
    manifest_file, manifest = _read(backend, run / "manifest.json")
    state_file, state = _read(backend, run / "state.json")
    if (manifest.get("kind") != "reference-fixture-service-run" or manifest.get("review") != review_binding
            or manifest.get("bootstrap") != bootstrap_binding):
        raise ValueError("首代服务账本未绑定当前审阅计划和私有环境")
    attempts = tuple((item.get("stage"), item.get("mode"), item.get("status")) for item in state.get("attempts", []))
    if attempts != (("storage-target", "initial", "passed"), ("cache-target", "initial", "passed"),
                    ("fixture-buckets", "prepare", "passed")):
        raise ValueError("首代服务账本阶段不完整或不是同一代次")
    process_file, process = _read(backend, run / "rustfs/process.json")
    launch_file, launch = _read(backend, run / "rustfs/launch.json")
    runtime_file, runtime = _read(backend, run / "redis/runtime.json")
    identity = process.get("identity")
    redis = runtime.get("redis")
    if (process.get("role") != "rustfs" or process.get("lifecycle") != "running"
            or not isinstance(identity, dict) or not isinstance(redis, dict)):
        raise ValueError("首代 RustFS 或 Redis 运行收据无效")
    return {
        "run": {"path": str(run), "manifest": _bound(manifest_file), "state": _bound(state_file)},
        "rustfs": {"identity": identity, "launch_receipt": _bound(launch_file),
                   "process_receipt": _bound(process_file), "sha256": identity.get("sha256")},
        "redis": redis,
    }


def build(backend: Path, environment_path: Path, service_run: Path, request_id: str) -> dict:
    """构造并校验请求；调用方决定是否显式写入新文件。"""
    backend = backend.resolve(strict=True)
    bootstrap_file, bootstrap = _read(backend, environment_path)
    if (bootstrap.get("kind") != "reference-fixture-environment" or bootstrap.get("status") != "prepared"
            or bootstrap.get("services_started") is not False or bootstrap.get("remote_writes") != 0):
        raise ValueError("私有环境收据无效或已包含服务副作用")
    plan = bootstrap.get("plan")
    if not isinstance(plan, dict) or plan.get("side") != "seed":
        raise ValueError("fresh-target 只能从 seed 私有环境签发")
    review_binding = plan.get("review")
    if not isinstance(review_binding, dict):
        raise ValueError("私有环境缺少审阅计划绑定")
    review_file, review = _read(backend, Path(review_binding["path"]))
    if _bound(review_file) != {key: review_binding[key] for key in ("path", "bytes", "sha256")}:
        raise ValueError("私有环境审阅计划摘要已变化")
    validate_review(review)
    selected = review["scopes"]["seed"]
    execution = Path(bootstrap["execution_backend"])
    if execution != Path(selected["backend_dir"]):
        raise ValueError("私有环境执行工作树与 seed 审阅侧不符")
    requested_run = service_run if service_run.is_absolute() else backend / service_run
    expected_run = expected_service_run(review)
    actual_run = local_path(backend, str(requested_run))
    if actual_run != expected_run:
        raise ValueError("fresh-target 请求必须绑定本审阅计划的唯一服务账本")
    service = _service(backend, actual_run, _bound(review_file), _bound(bootstrap_file))
    rustfs = service["rustfs"]
    rustfs["sha256"] = review["tools"]["rustfs"]["sha256"]
    if rustfs["identity"].get("executable") != review["tools"]["rustfs"]["path"]:
        raise ValueError("RustFS 首代二进制与审阅工具不符")
    if not re.fullmatch(r"[A-Za-z0-9._-]{1,64}", request_id):
        raise ValueError("fresh-target 请求 ID 无效")
    databases = []
    for key in ("shared-control", "shared", "dedicated-a", "dedicated-b"):
        item = next(value for value in selected["databases"] if value["key"] == key)
        defaults = Path(item["connection_file"])
        databases.append({"key": key, "kind": KEYS[key][0], "mode": KEYS[key][1], "database": item["database"],
                          "server_uuid": item["expected_server_uuid"], "defaults_file": str(defaults),
                          "defaults_sha256": file_digest(defaults)["sha256"]})
    credential_version = read_json(bootstrap_file.parent / "environment.json")["environment"].get("APP_RESET_CREDENTIAL_VERSION")
    scope = selected["scope_id"]
    request = {
        "format_version": 1, "kind": "devex-clone-fresh-target", "id": request_id,
        "review": {**_bound(review_file), "canonical_sha256": plan["review"]["canonical_sha256"]}, "side": "seed",
        "target": {"scope_id": scope, "s3": {"endpoint": selected["objects"]["endpoint"], "region": selected["objects"]["region"],
                   "access_key_env": "APP_OBJECT_STORAGE_ACCESS_KEY", "secret_key_env": "APP_OBJECT_STORAGE_SECRET_KEY"}, "databases": databases},
        "maintenance_build": plan["maintenance_build"], "configuration_sha256": bootstrap["configuration_sha256"],
        "tools": {name: {key: review["tools"][name][key] for key in ("path", "sha256")} for name in ("mysql", "aws")},
        "storage": {"rustfs": rustfs, "redis": service["redis"]},
        "reset": {"legacy_ownership": {key: True for key in ("mysql_exclusive", "redis_exclusive", "object_storage_exclusive")},
                  "credential_version": credential_version, "sentinel_key": f"ryframe:devex-fresh:{scope}:sentinel",
                  "sentinel_value": f"devex-fresh:{request_id}"},
        "execution_backend": {"fixture": plan["fixture"], "path": str(execution)},
    }
    request_binding(backend, request)
    return request


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--backend-dir", type=Path, required=True)
    parser.add_argument("--environment", type=Path, required=True)
    parser.add_argument("--service-run", type=Path, required=True)
    parser.add_argument("--id", required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--write", action="store_true")
    args = parser.parse_args()
    if not args.write:
        parser.error("签发 fresh-target 请求需要显式 --write")
    backend = args.backend_dir.resolve()
    request = build(backend, args.environment, args.service_run, args.id)
    requested = args.output if args.output.is_absolute() else backend / args.output
    output = local_path(backend, str(requested), new=True)
    if not output.parent.is_dir():
        raise ValueError("fresh-target 请求输出父目录不存在")
    write_json(output, request)
    print(json.dumps({"status": "requested", "request_sha256": plan_hash(request), "remote_writes": 0}, ensure_ascii=False))


if __name__ == "__main__":
    main()
