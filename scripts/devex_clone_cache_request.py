"""缓存恢复仅绑定原目标、配置与初始化验收标记，不修改原初始化历史。"""
from __future__ import annotations

import copy
from pathlib import Path
import shlex

from devex_clone_cache_owner import images
from devex_clone_capture import read_json
from devex_clone_factory_context import initialization_history
from devex_clone_model import exact, local_path
from devex_clone_run_state import binding
from devex_clone_source_proof import bound_file
from devex_clone_storage_request import directory_identity
from devex_clone_target_binding import external_file, pending_request_binding, request_binding

FIELDS = {"format_version", "kind", "side", "manifest", "initialized", "target_environment",
          "previous", "process_receipt", "process", "markers"}
REDIS_FIELDS = {"port", "wsl", "distribution", "pid", "started", "executable", "sha256", "run_id", "configuration"}


def environment(backend, request):
    private = read_json(bound_file(backend, request["target_environment"]))
    exact(private, {"environment"})
    value = private["environment"]
    if not isinstance(value, dict) or any(type(key) is not str or type(item) is not str for key, item in value.items()):
        raise ValueError("缓存恢复须使用固定目标完整文本环境")
    expected = {"APP_REDIS_HOST": "127.0.0.1", "APP_REDIS_PORT": str(request["previous"]["port"]),
                "APP_REDIS_DATABASE": "0", "APP_REDIS_TLS": "false"}
    if any(value.get(key) != item for key, item in expected.items()) or not value.get("APP_REDIS_PASSWORD"):
        raise ValueError("缓存恢复环境不是原本机认证端点")
    return copy.deepcopy(value)


def process_binding(backend, request, review, private, *, effective=None):
    from devex_clone_cache_process import validate

    previous, process = request["previous"], request["process"]
    validate(process, files=effective is None)
    if effective is not None:
        validate(effective)
    receipt = read_json(bound_file(backend, request["process_receipt"]))
    old_identity = {key: previous[key] for key in ("pid", "started", "executable")}
    if (receipt.get("format_version") != 1 or receipt.get("role") != "redis"
            or receipt.get("lifecycle") != "running" or receipt.get("request_storage") != previous
            or receipt.get("scope_id") != review["services"]["redis"]["scope_id"]):
        raise ValueError("原 Redis 进程收据与本次初始化或服务身份不同")
    old_linux = receipt["linux_identity"]
    exact(old_linux, {"pid", "started", "executable", "boot_id"})
    if {key: old_linux[key] for key in old_identity} != old_identity:
        raise ValueError("原 Redis Linux 创建身份不符")
    directory = directory_identity(backend, process["directory"])
    if directory != local_path(backend, review["services"]["redis"]["directory"]):
        raise ValueError("缓存恢复只能使用原服务物理目录")
    config = bound_file(backend, previous["configuration"])
    if config != directory / "redis.conf":
        raise ValueError("缓存恢复配置不属于原目录")
    if effective is None:
        external_file(previous["wsl"])
    expected = {key: previous[key] for key in ("wsl", "distribution", "executable", "sha256", "configuration", "port")}
    expected.update(scope_id=receipt["scope_id"], launcher=review["tools"]["redis_server"]["path"],
                    previous_identity=old_identity, previous_boot_id=old_linux["boot_id"],
                    previous_run_id=previous["run_id"], password_env="APP_REDIS_PASSWORD")
    if any(process.get(key) != item for key, item in expected.items()):
        raise ValueError("缓存进程请求改变原服务、配置、工具或创建身份")
    options = [shlex.split(line) for line in config.read_text(encoding="utf-8").splitlines() if line.strip()]
    passwords = [item[1:] for item in options if item[0] == "requirepass"]
    if passwords != [[private["APP_REDIS_PASSWORD"]]]:
        raise ValueError("原 Redis 配置认证与固定目标环境不同")


def _validate_request(backend, directory, value, request, bind_request, *, owned_lock_identity=None, effective=None):
    exact(request, FIELDS)
    if (request["format_version"] != 1 or request["kind"] != "devex-clone-cache-restart"
            or request["side"] != "target" or request["manifest"] != binding(directory / "manifest.json")
            or request["initialized"] != value["initialized"] or request["target_environment"] != value["target_environment"]):
        raise ValueError("缓存恢复必须属于固定运行的原目标和明确环境")
    exact(request["previous"], REDIS_FIELDS)
    initial, original = initialization_history(backend, bound_file(backend, request["initialized"]), owned_lock_identity)
    review, selected = bind_request(backend, original)
    if request["previous"] != original["storage"]["redis"] or request["previous"] != initial["generation"]["storage"]["redis"]:
        raise ValueError("缓存恢复原代次与完整初始化历史不符")
    markers = {key: selected["redis"][key] for key in ("namespace", "ownership_key", "ownership_value")}
    markers.update({key: original["reset"][key] for key in ("sentinel_key", "sentinel_value")})
    scope = original["target"]["scope_id"]
    if (request["markers"] != markers or markers["namespace"] != f"ryframe:{{{scope}}}:"
            or markers["ownership_key"] != markers["namespace"] + ".ryframe-owner"
            or markers["sentinel_key"].startswith(markers["namespace"])
            or markers["sentinel_key"] != f"ryframe:devex-fresh:{scope}:sentinel"
            or markers["sentinel_value"] != "devex-fresh:" + original["id"]
            or initial["redis"] != images(markers)[1]
            or selected["redis"]["url"] != f"redis://127.0.0.1:{request['previous']['port']}/0"):
        raise ValueError("只能原子恢复原初始化明确记录的两个验收标记")
    private = environment(backend, request)
    process_binding(backend, request, review, private, effective=effective)
    return private


def validate_request(backend: Path, directory: Path, value: dict, request: dict, *, owned_lock_identity=None) -> dict:
    return _validate_request(backend, directory, value, request, request_binding,
                             owned_lock_identity=owned_lock_identity)


def successor_process(backend, directory, value, request, successor, *, owned_lock_identity=None):
    """只从已发布 seed 的正式 successor 推导 WSL/Python；原资源与登记始终不变。"""
    from devex_clone_seed_source import _registered_source
    from reference_fixture_environment import validate_current_review_tools
    from reference_fixture_successor import _source_with_loader

    source = _source_with_loader(backend, successor, live_storage=False, loader=_registered_source)
    if source["directory"] != directory or source["manifest"] != value:
        raise ValueError("缓存工具后继必须属于当前已发布 seed 的原 run")
    relationship = source["review_successor"]
    review = read_json(bound_file(backend, {key: item for key, item in relationship["successor_review"].items()
                                          if key != "canonical_sha256"}))
    tools = validate_current_review_tools(review)
    original = request["process"]
    redis, python = tools["redis_server"], tools["redis_python"]
    if (redis["distribution"] != original["distribution"] or redis["resolved_path"] != original["executable"]
            or redis["sha256"] != original["sha256"] or python["distribution"] != original["distribution"]
            or Path(tools["wsl"]["path"]).resolve() != Path(original["wsl"]["path"]).resolve()
            or python["path"] != original["python"]["path"] or python["resolved_path"] != original["python"]["executable"]):
        raise ValueError("缓存工具后继不得改变 Redis、发行版或解释器入口")
    effective = {**copy.deepcopy(original), "wsl": copy.deepcopy(tools["wsl"]),
                 "python": {"path": python["path"], "executable": python["resolved_path"], "sha256": python["sha256"]}}
    if effective == original:
        raise ValueError("缓存工具没有变化，无需登记 successor")
    def bind_pending(root, target):
        if target != source["seed_target"]:
            raise ValueError("缓存初始化不是 successor 冻结的原 seed")
        return pending_request_binding(root, target, relationship["predecessor_review"])
    private = _validate_request(backend, directory, value, request, bind_pending,
                                 owned_lock_identity=owned_lock_identity, effective=effective)
    return effective, private, source
