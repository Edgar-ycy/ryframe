"""fresh-target CLI 对夹具服务账本的只读判定。"""
from pathlib import Path

from devex_clone_capture import read_bound_json
from devex_clone_run_state import historical_state, load_state


def fixture_service_run(manifest: dict, state: dict) -> bool:
    """识别只提供首代服务的参考夹具，不能把它伪装成复制重启。"""
    if manifest.get("kind") != "reference-fixture-service-run":
        return False
    from reference_fixture_service_history import INITIAL, validate_manifest

    validate_manifest(manifest)
    initial = state["attempts"][:len(INITIAL)]
    actual = tuple((item["stage"], item["mode"]) for item in initial)
    if actual != INITIAL or any(item["status"] != "passed" for item in initial):
        raise ValueError("夹具服务账本必须完整包含 RustFS、Redis 与对象桶初始化")
    return True


def fixture_service_generation(directory: Path, descriptor: dict) -> dict | None:
    """返回执行入口与 status 共用的夹具服务代次可用性。"""
    manifest = read_bound_json(directory / "manifest.json", descriptor["manifest"])
    historical_state(directory, descriptor["state"])
    state = load_state(directory)
    if not fixture_service_run(manifest, state):
        return None
    from reference_fixture_service_history import validate_history

    history = validate_history(directory, state)
    if history["closed"]:
        return {"available": False, "active_generation": history["active_generation"],
                "reason": "夹具服务已正常关闭，当前生命周期没有可执行重启；须先扩展生命周期或登记新目标"}
    if history["external_recovery"] is not None:
        return {"available": False, "active_generation": history["active_generation"],
                "reason": "夹具服务外部终止已核对但尚未重启"}
    return {"available": True, "active_generation": history["active_generation"],
            "reason": None}
