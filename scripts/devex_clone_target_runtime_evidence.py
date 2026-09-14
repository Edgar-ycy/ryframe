"""fresh 目标续作时的可信存储与缓存代次证据。"""
from pathlib import Path

from devex_clone import read_json
from devex_clone_model import exact
from devex_clone_source_proof import bound_file
from devex_clone_target_storage import verify_storage_generation
from restore_reference_plan import plan_hash


def _fixture_runtime_evidence(backend: Path, expected: dict,
                              generation: dict) -> tuple[dict, dict]:
    """从夹具追加账本重新派生指定历史重启代次，不信任 started 的内嵌值。"""
    from devex_clone_run_state import load_state
    from reference_fixture_service_context import runtime_transition
    from reference_fixture_service_history import validate_history

    result_path = bound_file(backend, generation)
    if result_path.parent.name != "results":
        raise ValueError("夹具服务代次结果不属于统一账本")
    run = result_path.parent.parent
    state = load_state(run)
    validate_history(run, state)
    matches = [item for item in state["attempts"]
               if (item["stage"], item["mode"], item["status"], item["result"])
               == ("fixture-services", "restart", "passed", generation)]
    if len(matches) != 1:
        raise ValueError("夹具服务代次不是唯一成功 restart")
    storage, cache = runtime_transition(backend, run, expected)
    if storage is None or cache is None \
            or storage.get("generation") != generation or cache.get("generation") != generation:
        raise ValueError("夹具重启代次不是当前可信服务代次")
    return storage, cache


def _registered_runtime_evidence(backend: Path, expected: dict, storage: dict | None,
                                 cache: dict | None) -> tuple[dict | None, dict | None]:
    """从绑定的发布结果重算普通 storage/cache 重启证据。"""
    if storage is not None:
        exact(storage, {"request", "storage", "attempt", "data_directory", "api_url",
                        "console_url", "restart_result"})
        request = read_json(bound_file(backend, storage["request"]))
        result = read_json(bound_file(backend, storage["restart_result"]))
        previous = request["previous"]
        original = expected["rustfs"]
        if ({key: previous[key] for key in ("identity", "process_receipt", "launch_receipt")}
                != {key: original[key] for key in ("identity", "process_receipt", "launch_receipt")}
                or request["executable"]["sha256"] != original["sha256"]):
            raise ValueError("RustFS 重启请求没有绑定原 fresh 代次")
        published = {key: storage[key] for key in ("request", "storage", "attempt",
                                                    "data_directory", "api_url", "console_url")}
        if result.get("status") != "storage_restarted" or result.get("side") != "target" \
                or any(result.get(key) != value for key, value in published.items()):
            raise ValueError("RustFS started 证据不同于已发布重启结果")
        bound_file(backend, result["ready"])
    if cache is not None:
        exact(cache, {"request", "runtime", "redis", "owner", "controller", "attempt",
                      "ready", "restart_result"})
        request = read_json(bound_file(backend, cache["request"]))
        result = read_json(bound_file(backend, cache["restart_result"]))
        if request["previous"] != expected["redis"]:
            raise ValueError("Redis 重启请求没有绑定原 fresh 代次")
        published = {key: cache[key] for key in ("request", "runtime", "redis", "owner",
                                                 "controller", "attempt", "ready")}
        if result.get("status") != "cache_ready" \
                or any(result.get(key) != value for key, value in published.items()):
            raise ValueError("Redis started 证据不同于已发布重启结果")
        for field in ("runtime", "owner", "controller", "ready"):
            bound_file(backend, cache[field])
    return storage, cache


def _trusted_runtime_evidence(backend: Path, expected: dict, storage: dict | None,
                              cache: dict | None) -> tuple[dict | None, dict | None]:
    if storage is not None and not isinstance(storage, dict) \
            or cache is not None and not isinstance(cache, dict):
        raise ValueError("迁移续作的存储恢复代次无效")
    fixture = [item is not None and "generation" in item for item in (storage, cache)]
    if any(fixture):
        if fixture != [True, True] or storage["generation"] != cache["generation"]:
            raise ValueError("夹具存储与缓存恢复不是同一服务代次")
        exact(storage, {"storage", "data_directory", "api_url", "console_url", "generation"})
        exact(cache, {"redis", "generation"})
        return _fixture_runtime_evidence(backend, expected, storage["generation"])
    return _registered_runtime_evidence(backend, expected, storage, cache)


def validate_started_generation(backend: Path, expected: dict, started: dict) -> None:
    runtime, cache = started["storage_runtime"], started["cache_runtime"]
    if (runtime, cache) != _trusted_runtime_evidence(backend, expected["storage"], runtime, cache):
        raise ValueError("迁移续作 started 的存储代次不同于可信发布证据")
    actual = {**expected, "storage": started["storage"]}
    verify_storage_generation(expected, actual, runtime, cache)
    if actual == expected and (runtime is not None or cache is not None):
        raise ValueError("存储恢复证据没有产生新的实际进程代次")
    if started["current_generation_sha256"] != plan_hash(actual):
        raise ValueError("迁移续作 started 未绑定允许的当前存储代次")
