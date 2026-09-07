"""将新目标严格绑定到已审阅单侧资源、实际配置和三份维护工具。"""
from __future__ import annotations

import hashlib
import os
from pathlib import Path
import re

from devex_clone import read_json
from devex_clone_model import digest, exact, linked, local_path, name
from devex_clone_inventory import configuration as inventory_configuration
from devex_clone_source_proof import bound_file, require_closed_port, verify_api_address
from devex_clone_tools import verify as verify_tools
from full_stack_runtime import configuration_digest, worker_ready_url
from full_stack_rate_limit_config import load_app_table
from restore_build import file_digest, source_snapshot
from restore_reference_plan import BUCKETS, identifier, plan_hash
from restore_source_binding import defaults_connection, source_binding

KEYS = {"shared-control": ("combined", "shared"), "shared": ("tenant", "shared"),
        "dedicated-a": ("tenant", "dedicated"), "dedicated-b": ("tenant", "dedicated")}
EXCLUSIVE = {"mysql_exclusive", "redis_exclusive", "object_storage_exclusive"}


def validate_review(review: dict) -> None:
    """拒绝缺字段的审阅计划，避免初始化过程中才发现资源定义不完整。"""
    if not isinstance(review, dict) or not isinstance(review.get("reference"), dict):
        raise ValueError("审阅计划结构无效")
    if review.get("kind") != "review-only-perf-resource-plan-with-readonly-preflight":
        raise ValueError("审阅计划类型无效")
    if review.get("ready_for_execution") is not True:
        raise ValueError("审阅计划尚未就绪")
    scopes, tools, services = review.get("scopes"), review.get("tools"), review.get("services")
    if not isinstance(scopes, dict) or set(scopes) != {"seed", "base", "candidate"}:
        raise ValueError("审阅计划必须完整声明三侧资源")
    if not isinstance(tools, dict) or not isinstance(services, dict):
        raise ValueError("审阅计划缺少工具或服务定义")
    for role in ("mysql", "aws", "rustfs"):
        tool = tools.get(role)
        if not isinstance(tool, dict) or not isinstance(tool.get("path"), str) or not isinstance(tool.get("sha256"), str):
            raise ValueError("审阅计划工具定义无效")
    redis = tools.get("redis_server")
    if not isinstance(redis, dict) or any(not isinstance(redis.get(key), str) for key in ("distribution", "resolved_path", "sha256")):
        raise ValueError("审阅计划 Redis 工具定义无效")
    rustfs, cache = services.get("rustfs"), services.get("redis")
    if not isinstance(rustfs, dict) or not isinstance(cache, dict) or any(
        not isinstance(value, str)
        for value in (rustfs.get("api"), rustfs.get("console"), rustfs.get("data_dir"), cache.get("directory"))
    ):
        raise ValueError("审阅计划服务定义无效")
    for role in ("source", "protected_target"):
        side = review["reference"].get(role)
        if not isinstance(side, dict) or not isinstance(side.get("databases"), list):
            raise ValueError("审阅计划保护资源定义无效")
    for side in scopes.values():
        if not isinstance(side, dict) or not isinstance(side.get("scope_id"), str) or not name(side["scope_id"]):
            raise ValueError("审阅计划 scope 无效")
        if any(not isinstance(side.get(key), str) for key in ("runtime_dir", "identity_ledger", "api_url", "worker_ready_url", "frontend_url")):
            raise ValueError("审阅计划运行路径无效")
        objects, cache = side.get("objects"), side.get("redis")
        if not isinstance(objects, dict) or any(not isinstance(objects.get(key), str) for key in ("endpoint", "region")):
            raise ValueError("审阅计划对象存储定义无效")
        if not isinstance(cache, dict) or any(not isinstance(cache.get(key), str) for key in ("url", "namespace", "ownership_key", "ownership_value")):
            raise ValueError("审阅计划 Redis 定义无效")
        databases = side.get("databases")
        if not isinstance(databases, list) or {item.get("key") for item in databases if isinstance(item, dict)} != set(KEYS):
            raise ValueError("审阅计划数据库定义无效")
        if any(not isinstance(item.get("database"), str) or not isinstance(item.get("expected_server_uuid"), str) for item in databases):
            raise ValueError("审阅计划数据库身份无效")


def external_file(value: dict) -> None:
    exact(value, {"path", "sha256"})
    path = Path(value["path"])
    if (not path.is_absolute() or any(linked(p) for p in (path, *path.parents))
            or file_digest(path)["sha256"] != digest(value["sha256"])):
        raise ValueError("新目标外部工具实际字节或路径已变化")


def request_binding(backend: Path, request: dict) -> tuple[dict, dict]:
    required = {"format_version", "kind", "id", "review", "side", "target", "maintenance_build",
                "configuration_sha256", "tools", "storage", "reset"}
    optional = {"execution_backend"}
    exact(request, required | ({"execution_backend"} if "execution_backend" in request else set()))
    if request["format_version"] != 1 or request["kind"] != "devex-clone-fresh-target":
        raise ValueError("新目标请求类型错误")
    name(request["id"])
    exact(request["review"], {"path", "bytes", "sha256", "canonical_sha256"})
    review = read_json(bound_file(backend, {k: v for k, v in request["review"].items() if k != "canonical_sha256"}))
    validate_review(review)
    if (plan_hash(review) != digest(request["review"]["canonical_sha256"])
            or request["side"] not in review["scopes"]):
        raise ValueError("必须选择已就绪审阅计划的明确单侧")
    selected = review["scopes"][request["side"]]
    target = request["target"]
    exact(target, {"scope_id", "s3", "databases"})
    if name(target["scope_id"]) != selected["scope_id"]:
        raise ValueError("目标 scope 与单侧审阅计划不符")
    exact(target["s3"], {"endpoint", "region", "access_key_env", "secret_key_env"})
    if any(target["s3"][key] != selected["objects"][key] for key in ("endpoint", "region")):
        raise ValueError("对象存储不是计划中的目标端点")
    actual = target["databases"]
    if not isinstance(actual, list) or len(actual) != 4 or {db["key"] for db in actual} != set(KEYS):
        raise ValueError("新目标必须精确包含四个逻辑数据库")
    planned = {db["key"]: db for db in selected["databases"]}
    all_names = [(db["expected_server_uuid"], db["database"].lower())
                 for side in review["scopes"].values() for db in side["databases"]]
    protected = {(db["server_uuid"], db["database"].lower())
                 for role in ("source", "protected_target") for db in review["reference"][role]["databases"]}
    if len(all_names) != 12 or len(set(all_names)) != 12 or set(all_names) & protected:
        raise ValueError("三侧数据库相互重叠或碰到参考资源")
    for db in actual:
        exact(db, {"key", "kind", "mode", "database", "server_uuid", "defaults_file", "defaults_sha256"})
        expected = planned[db["key"]]
        if ((db["kind"], db["mode"]) != KEYS[db["key"]] or identifier(db["database"]) != expected["database"]
                or db["server_uuid"] != expected["expected_server_uuid"]
                or db["defaults_file"] != expected["connection_file"]):
            raise ValueError("数据库物理身份或凭据路径不是精确审阅目标")
        if file_digest(local_path(backend, db["defaults_file"]))["sha256"] != digest(db["defaults_sha256"]):
            raise ValueError("目标 MySQL 凭据文件变化")
    exact(request["tools"], {"mysql", "aws"})
    for role, tool in request["tools"].items():
        external_file(tool)
        if any(tool[key] != review["tools"][role][key] for key in ("path", "sha256")):
            raise ValueError("目标工具不属于审阅版本")
    exact(request["storage"], {"rustfs", "redis"})
    return review, selected


def execution_backend(backend: Path, request: dict) -> tuple[Path, dict]:
    """解析可选的冻结 Device 执行工作树，默认继续使用当前后端。"""
    declared = request.get("execution_backend")
    if declared is None:
        return backend.resolve(strict=True), {"kind": "current-backend", "path": str(backend.resolve(strict=True))}
    exact(declared, {"fixture", "path"})
    fixture_path = bound_file(backend, declared["fixture"])
    fixture = read_json(fixture_path)
    if (fixture.get("format_version") != 1 or fixture.get("fixture") != "device"
            or fixture.get("status") != "ready" or not isinstance(fixture.get("paths"), dict)
            or not isinstance(fixture.get("generated"), dict)):
        raise ValueError("冻结 Device 工作树收据无效")
    root = local_path(backend, declared["path"])
    expected = Path(fixture["paths"].get("backend", ""))
    generated = fixture["generated"].get("backend")
    if (root != expected or not (root / "Cargo.toml").is_file() or not (root / ".git").exists()
            or not isinstance(generated, dict) or source_snapshot(root) != generated):
        raise ValueError("冻结 Device 后端工作树或生成来源已变化")
    return root, {"kind": "device-fixture", "path": str(root), "fixture": declared["fixture"], "source": generated}


def reset_config(backend: Path, request: dict, selected: dict) -> None:
    declared = request["reset"]
    exact(declared, {"legacy_ownership", "credential_version", "sentinel_key", "sentinel_value"})
    exact(declared["legacy_ownership"], EXCLUSIVE)
    if any(value is not True for value in declared["legacy_ownership"].values()):
        raise ValueError("仅本次新空资源显式声明 exclusive 引导；不能隐式注入配置")
    table = load_app_table(backend, os.environ).get("reset", {})
    for key in EXCLUSIVE:
        field = "legacy_" + key
        variable = "APP_RESET_" + field.upper()
        if variable + "_FILE" in os.environ:
            raise ValueError("reset 文件覆盖未被明确绑定")
        actual = os.environ.get(variable, table.get(field))
        if actual is not True and actual != "true":
            raise ValueError("实际 reset exclusive 配置与请求不同")
    for field, value in (("credential_version", declared["credential_version"]),
                         ("redis_outside_sentinel_key", declared["sentinel_key"])):
        variable = "APP_RESET_" + field.upper()
        if variable + "_FILE" in os.environ or os.environ.get(variable, table.get(field)) != value:
            raise ValueError("实际 reset 版本或 sentinel 与显式请求不同")
    scope = request["target"]["scope_id"]
    if (not re.fullmatch(r"[A-Za-z0-9._-]{1,64}", declared["credential_version"])
            or declared["sentinel_key"] != f"ryframe:devex-fresh:{scope}:sentinel"
            or declared["sentinel_value"] != f"devex-fresh:{request['id']}"
            or declared["sentinel_key"].startswith(selected["redis"]["namespace"])):
        raise ValueError("sentinel 必须是仅归本 scope 的精确外部哨兵")
    redis = selected["redis"]
    if redis["namespace"] != f"ryframe:{{{scope}}}:" or redis["ownership_key"] != redis["namespace"] + ".ryframe-owner":
        raise ValueError("Redis 计划 namespace 无效")
    # 该入口限定已登记的本机非 TLS Redis；不复制配置或创建 Redis 服务。
    expected = {"APP_REDIS_HOST": "127.0.0.1", "APP_REDIS_PORT": str(request["storage"]["redis"]["port"]),
                "APP_REDIS_DATABASE": "0", "APP_REDIS_TLS": "false"}
    if any(os.environ.get(key) != value for key, value in expected.items()):
        raise ValueError("实际 Redis 地址或 TLS 未显式匹配本侧配置")


def never_started(backend: Path, selected: dict) -> None:
    runtime = local_path(backend, selected["runtime_dir"])
    for filename in ("producer-history.json", "api.json", "worker.json"):
        evidence = runtime / filename
        if evidence.exists() or linked(evidence):
            raise ValueError("目标已经出现生产者启动意图或进程收据，不能认定为从未启动的 fresh 目标")
    for field in ("runtime_dir", "identity_ledger"):
        if local_path(backend, selected[field]).exists():
            raise ValueError("目标已经出现运行或身份准备历史，不具备本代次 fresh 条件")


def generation(backend: Path, request: dict, run) -> dict:
    review, selected = request_binding(backend, request)
    execution_root, execution = execution_backend(backend, request)
    never_started(backend, selected)
    if configuration_digest(execution_root) != digest(request["configuration_sha256"]):
        raise ValueError("新目标 APP 配置或秘密发生变化")
    maintenance = verify_tools(execution_root, bound_file(backend, request["maintenance_build"]), run)
    physical = source_binding(execution_root, {"source": request["target"]})
    planned = {db["key"]: db for db in selected["databases"]}
    for actual in physical["databases"]:
        if any(actual[key] != planned[actual["key"]][key] for key in ("host", "port")):
            raise ValueError("目标 MySQL host/port 不是审阅计划中的地址")
    configuration = inventory_configuration(execution_root, os.environ, request["target"])
    reset_config(execution_root, request, selected)
    verify_api_address(backend, selected["api_url"])
    if os.environ.get("APP_JOBS_MODE") != "external" or worker_ready_url() != selected["worker_ready_url"]:
        raise ValueError("目标 Worker 必须显式匹配关闭端口与 external 模式")
    for field in ("api_url", "worker_ready_url", "frontend_url"):
        require_closed_port(selected[field])
    # 种子密码不属于 APP_*，同样只保存摘要，防止同代次 reset 凭据变更。
    secrets = {key: os.environ.get(key) for key in ("RYFRAME_RESET_ADMIN_PASSWORD", "RYFRAME_RESET_USER_PASSWORD")}
    if not all(secrets.values()):
        raise ValueError("必须显式提供本侧两项初始化密码")
    return {"execution": execution, "maintenance": maintenance, "physical": physical, "configuration": configuration,
            "configuration_sha256": request["configuration_sha256"],
            "seed_credentials_sha256": plan_hash(secrets), "request_sha256": plan_hash(request),
            "review_sha256": request["review"]["sha256"], "selected": selected,
            "operator_declared_controlled_generation_only": True, "external_writers_discovered": False}


def validate_reset_manifest(manifest: dict, request: dict, original: dict) -> None:
    target, reset = request["target"], request["reset"]
    exact(manifest, {"manifest_version", "environment", "scope_id", "code_sha", "config_sha", "credential_version",
                     "confirmation_phrase", "legacy_ownership", "redis", "object_storage", "databases"})
    scope = target["scope_id"]
    if (manifest["manifest_version"] != 4 or manifest["environment"] != "test" or manifest["scope_id"] != scope
            or manifest["code_sha"] != original["maintenance"]["source"]["snapshot"]["head"]
            or manifest["credential_version"] != reset["credential_version"]
            or manifest["confirmation_phrase"] != f"RESET-RYFRAME-test-{scope}"
            or manifest["legacy_ownership"] != reset["legacy_ownership"]):
        raise ValueError("实际 reset plan 来源或引导范围不符")
    digest(manifest["config_sha"])
    physical = {item["key"]: item for item in original["physical"]["databases"]}
    if len(manifest["databases"]) != 4:
        raise ValueError("reset plan 包含额外或缺失数据库")
    seen = set()
    for item in manifest["databases"]:
        keys = item["target_keys"]
        if len(keys) != 1 or keys[0] not in KEYS or keys[0] in seen:
            raise ValueError("reset plan 数据库合并或重复")
        key = keys[0]
        expected = physical[key]
        seen.add(key)
        kinds = {"control", "tenant-data"} if key == "shared-control" else {"tenant-data"}
        source_db = next(db for db in target["databases"] if db["key"] == key)
        connection = defaults_connection(Path(original["maintenance"]["backend_root"]), source_db)
        if (any(item[field] != expected[field] for field in ("host", "port", "database"))
                or item["control_baseline"] != (key == "shared-control") or item["tenant_baseline"] is not True
                or item["ownership_markers"] != {kind: f"ryframe-owner:v1:{scope}:{kind}" for kind in kinds}
                or item["connection"]["tls_mode"] != expected["tls_mode"]
                or item["connection"]["username"] != connection["username"]
                or any(item["connection"].get(field) is not None for field in ("tls_ca_sha256", "tls_client_cert_sha256", "tls_client_key_ref_sha256"))):
            raise ValueError("reset plan 数据库实际目标、baseline 或 owner 不符")
    storage = manifest["object_storage"]
    expected_storage = original["physical"]["s3"]
    if (any(storage[field] != expected_storage[field] for field in ("backend", "endpoint", "region"))
            or storage["use_ssl"] != expected_storage["endpoint"].startswith("https://")
            or len(storage["prefixes"]) != 5 or {p["bucket"] for p in storage["prefixes"]} != BUCKETS):
        raise ValueError("reset plan 对象端点或五桶不符")
    for item in storage["prefixes"]:
        if item != {"bucket": item["bucket"], "prefix": scope + "/", "ownership_marker_key": scope + "/.ryframe-owner",
                    "ownership_marker": f"ryframe-owner:v1:{scope}:object-storage:{item['bucket']}"}:
            raise ValueError("reset plan 对象前缀越界")
    redis, selected = manifest["redis"], original["selected"]["redis"]
    if (redis["host"] != "127.0.0.1" or redis["port"] != request["storage"]["redis"]["port"]
            or redis["database"] != 0 or redis["tls"] is not False or redis["namespace"] != selected["namespace"]
            or redis["ownership_marker_key"] != selected["ownership_key"] or redis["ownership_marker"] != selected["ownership_value"]
            or redis["outside_sentinel_key_sha256"] != hashlib.sha256(reset["sentinel_key"].encode()).hexdigest()):
        raise ValueError("reset plan Redis namespace、端点或 sentinel 不符")
