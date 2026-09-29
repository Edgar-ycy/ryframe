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
from reference_fixture_control_protocol import run_private
from reference_fixture_service_history import validate_history
from devex_clone_run_state import load_state


def _read(backend: Path, value: Path) -> tuple[Path, dict]:
    requested = value if value.is_absolute() else backend / value
    path = local_path(backend, str(requested))
    if linked(path) or not path.is_file():
        raise ValueError("fresh-target 请求输入必须是受控普通文件")
    return path, read_json(path)


def _bound(path: Path) -> dict:
    return {"path": str(path), **file_digest(path)}


SIDES = ("seed", "base", "candidate")


def _prepared_environment(
    backend: Path, path: Path, side: str
) -> tuple[Path, dict, Path, dict]:
    bootstrap_file, bootstrap = _read(backend, path)
    if (
        bootstrap.get("kind") != "reference-fixture-environment"
        or bootstrap.get("status") != "prepared"
        or bootstrap.get("services_started") is not False
        or bootstrap.get("remote_writes") != 0
    ):
        raise ValueError("私有环境收据无效或已包含服务副作用")
    plan = bootstrap.get("plan")
    if (
        not isinstance(plan, dict)
        or plan.get("kind") != "reference-fixture-environment-plan"
        or plan.get("side") != side
        or plan.get("scope_id") is None
    ):
        raise ValueError("私有环境未绑定请求指定侧")
    plan_body = {key: value for key, value in plan.items() if key != "sha256"}
    if plan.get("sha256") != plan_hash(plan_body):
        raise ValueError("私有环境计划摘要无效")
    review_binding = plan.get("review")
    if not isinstance(review_binding, dict):
        raise ValueError("私有环境缺少审阅计划绑定")
    review_file, review = _read(backend, Path(review_binding["path"]))
    expected = {key: review_binding[key] for key in ("path", "bytes", "sha256")}
    if _bound(review_file) != expected or review_binding.get(
        "canonical_sha256"
    ) != plan_hash(review):
        raise ValueError("私有环境审阅计划摘要已变化")
    validate_review(review)
    selected = review["scopes"][side]
    if plan["scope_id"] != selected["scope_id"] or Path(
        bootstrap["execution_backend"]
    ) != Path(selected["backend_dir"]):
        raise ValueError("私有环境执行工作树与指定审阅侧不符")
    return bootstrap_file, bootstrap, review_file, review


def _service(backend: Path, run: Path, review_binding: dict, review: dict) -> dict:
    manifest_file, manifest = _read(backend, run / "manifest.json")
    state_file, state = _read(backend, run / "state.json")
    if (
        manifest.get("kind") != "reference-fixture-service-run"
        or manifest.get("review") != review_binding
    ):
        raise ValueError("首代服务账本未绑定当前审阅计划")
    seed_binding = manifest.get("bootstrap")
    if not isinstance(seed_binding, dict):
        raise ValueError("首代服务账本缺少 seed 私有环境绑定")
    seed_file, seed, seed_review_file, seed_review = _prepared_environment(
        backend, Path(seed_binding["path"]), "seed"
    )
    if (
        _bound(seed_file) != seed_binding
        or _bound(seed_review_file) != review_binding
        or seed_review != review
        or manifest.get("execution_backend") != seed["execution_backend"]
        or manifest.get("scope_id") != review["services"]["rustfs"]["scope_id"]
    ):
        raise ValueError("首代服务账本未绑定当前审阅计划的 seed 私有环境")
    history = validate_history(run, state)
    if load_state(run) != state or history["closed"] or history["external_recovery"] is not None:
        raise ValueError("首代服务账本发生变化或已经关闭")
    process_file, process = _read(backend, run / "rustfs/process.json")
    launch_file, launch = _read(backend, run / "rustfs/launch.json")
    runtime_file, runtime = _read(backend, run / "redis/runtime.json")
    identity = process.get("identity")
    redis = runtime.get("redis")
    if (
        process.get("role") != "rustfs"
        or process.get("lifecycle") != "running"
        or not isinstance(identity, dict)
        or not isinstance(redis, dict)
    ):
        raise ValueError("首代 RustFS 或 Redis 运行收据无效")
    result = {
        "run": {
            "path": str(run),
            "manifest": _bound(manifest_file),
            "state": _bound(state_file),
        },
        "bootstrap": _bound(seed_file),
        "rustfs": {
            "identity": identity,
            "launch_receipt": _bound(launch_file),
            "process_receipt": _bound(process_file),
            "sha256": identity.get("sha256"),
        },
        "redis": redis,
    }
    if history["active_generation"].get("kind") != "initial":
        from reference_fixture_service_context import runtime_transition

        storage, cache = runtime_transition(backend, run, {"storage": result["rustfs"], "redis": result["redis"]})
        result["rustfs"], result["redis"] = storage["storage"], cache["redis"]
    if (
        result["run"]["manifest"] != _bound(manifest_file)
        or result["run"]["state"] != _bound(state_file)
        or result["bootstrap"] != _bound(seed_file)
    ):
        raise ValueError("首代服务账本在请求构造期间变化")
    return result


def _require_live_service(backend: Path, review_file: Path, service: dict) -> None:
    """终末核验当前服务代次、完整进程树与三份运行收据。"""
    from reference_fixture_service_context import context, guard, observe_services, registered_services

    seed_bootstrap = Path(service["bootstrap"]["path"])
    value = context(backend, review_file, seed_bootstrap)
    registered = registered_services(value)
    storage = {key: registered["storage"][key]
               for key in ("identity", "sha256", "process_receipt", "launch_receipt")}
    if (service["run"]["path"] != str(value["run"])
            or service["run"]["manifest"] != value["sources"]["manifest"]
            or service["run"]["state"] != value["sources"]["state_before"]
            or service["bootstrap"] != value["sources"]["bootstrap"]
            or service["rustfs"] != storage
            or service["redis"] != registered["runtime"]["redis"]):
        raise ValueError("fresh-target 服务投影与当前登记代次不一致")
    descriptors = (storage["process_receipt"], storage["launch_receipt"],
                   registered["evidence"]["redis_runtime"])
    if any(not isinstance(item, dict) for item in descriptors):
        raise ValueError("fresh-target 服务缺少进程、启动或 Redis 运行绑定")
    if observe_services(registered) != {"redis": "running", "rustfs": "running", "termination": None}:
        raise ValueError("fresh-target 请求签发时 RustFS 或 Redis 未保持存活")
    guard(value)
    for descriptor in descriptors:
        path, _ = _read(backend, Path(descriptor["path"]))
        if _bound(path) != descriptor:
            raise ValueError("fresh-target 服务运行收据在签发前发生变化")


def build(
    backend: Path, environment_path: Path, service_run: Path, request_id: str, side: str
) -> dict:
    """构造并校验请求；调用方决定是否显式写入新文件。"""
    backend = backend.resolve(strict=True)
    if side not in SIDES:
        raise ValueError("fresh-target 必须选择 seed、base 或 candidate")
    bootstrap_file, bootstrap, review_file, review = _prepared_environment(
        backend, environment_path, side
    )
    bootstrap_binding = _bound(bootstrap_file)
    review_binding = _bound(review_file)
    plan = bootstrap["plan"]
    selected = review["scopes"][side]
    execution = Path(bootstrap["execution_backend"])
    requested_run = service_run if service_run.is_absolute() else backend / service_run
    expected_run = expected_service_run(review)
    actual_run = local_path(backend, str(requested_run))
    if actual_run != expected_run:
        raise ValueError("fresh-target 请求必须绑定本审阅计划的唯一服务账本")
    service = _service(backend, actual_run, review_binding, review)
    if side == "seed" and service["bootstrap"] != bootstrap_binding:
        raise ValueError("seed 请求必须使用启动首代服务的同一私有环境")
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
        databases.append(
            {
                "key": key,
                "kind": KEYS[key][0],
                "mode": KEYS[key][1],
                "database": item["database"],
                "server_uuid": item["expected_server_uuid"],
                "defaults_file": str(defaults),
                "defaults_sha256": file_digest(defaults)["sha256"],
            }
        )
    environment_file, private = _read(
        backend, bootstrap_file.parent / "environment.json"
    )
    environment_binding = _bound(environment_file)
    values = private.get("environment")
    if (
        not isinstance(values, dict)
        or values.get("APP_SCOPE_ID") != selected["scope_id"]
    ):
        raise ValueError("fresh-target 私有环境未绑定当前 scope")
    credential_version = values.get("APP_RESET_CREDENTIAL_VERSION")
    scope = selected["scope_id"]
    request = {
        "format_version": 1,
        "kind": "devex-clone-fresh-target",
        "id": request_id,
        "review": {
            **review_binding,
            "canonical_sha256": plan["review"]["canonical_sha256"],
        },
        "side": side,
        "target": {
            "scope_id": scope,
            "s3": {
                "endpoint": selected["objects"]["endpoint"],
                "region": selected["objects"]["region"],
                "access_key_env": "APP_OBJECT_STORAGE_ACCESS_KEY",
                "secret_key_env": "APP_OBJECT_STORAGE_SECRET_KEY",
            },
            "databases": databases,
        },
        "maintenance_build": plan["maintenance_build"],
        "configuration_sha256": bootstrap["configuration_sha256"],
        "tools": {
            name: {key: review["tools"][name][key] for key in ("path", "sha256")}
            for name in ("mysql", "aws")
        },
        "storage": {"rustfs": rustfs, "redis": service["redis"]},
        "reset": {
            "legacy_ownership": {
                key: True
                for key in (
                    "mysql_exclusive",
                    "redis_exclusive",
                    "object_storage_exclusive",
                )
            },
            "credential_version": credential_version,
            "sentinel_key": f"ryframe:devex-fresh:{scope}:sentinel",
            "sentinel_value": f"devex-fresh:{request_id}",
        },
        "execution_backend": {"fixture": plan["fixture"], "path": str(execution)},
    }
    request_binding(backend, request)
    if (
        _bound(bootstrap_file) != bootstrap_binding
        or _bound(review_file) != review_binding
        or _bound(environment_file) != environment_binding
    ):
        raise ValueError("fresh-target 私有环境在请求构造期间变化")
    _require_live_service(backend, review_file, service)
    return request


def publish(
    backend: Path,
    environment_path: Path,
    service_run: Path,
    request_id: str,
    side: str,
    output: Path,
) -> dict:
    """只写一个新的请求文件，并在写入后重算全部输入绑定。"""
    backend = backend.resolve(strict=True)
    requested = output if output.is_absolute() else backend / output
    target = local_path(backend, str(requested), new=True)
    if not target.parent.is_dir():
        raise ValueError("fresh-target 请求输出父目录不存在")
    request = build(backend, environment_path, service_run, request_id, side)
    write_json(target, request)
    if (
        read_json(target) != request
        or build(backend, environment_path, service_run, request_id, side) != request
    ):
        raise ValueError("fresh-target 请求发布期间输入发生变化")
    return request


PROTOCOL_SCHEMAS = {
    "publish": (("environment", "service_run", "id", "side", "output"), (), True),
}


def main(argv: list[str]) -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--backend-dir", type=Path, required=True)
    parser.add_argument("--environment", type=Path, required=True)
    parser.add_argument("--service-run", type=Path, required=True)
    parser.add_argument("--id", required=True)
    parser.add_argument("--side", choices=SIDES, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--write", action="store_true")
    args = parser.parse_args(argv)
    if not args.write:
        parser.error("签发 fresh-target 请求需要显式 --write")
    backend = args.backend_dir.resolve()
    request = publish(
        backend, args.environment, args.service_run, args.id, args.side, args.output
    )
    print(
        json.dumps(
            {
                "status": "requested",
                "request_sha256": plan_hash(request),
                "remote_writes": 0,
            },
            ensure_ascii=False,
        )
    )


if __name__ == "__main__":
    raise SystemExit(run_private("request", PROTOCOL_SCHEMAS, main, positional_operation=False))
