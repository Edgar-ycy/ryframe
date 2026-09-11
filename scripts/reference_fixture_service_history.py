"""参考夹具首代及服务生命周期的唯一账本规则。"""
from __future__ import annotations

from pathlib import Path

from devex_clone_capture import read_json
from devex_clone_model import exact, linked
from devex_clone_run_state import binding, historical_state

INITIAL = (("storage-target", "initial"), ("cache-target", "initial"), ("fixture-buckets", "prepare"))
LIFECYCLE_STAGE = "fixture-services"


def validate_manifest(value: dict) -> None:
    exact(value, {"format_version", "kind", "review", "bootstrap", "execution_backend", "scope_id",
                  "data_directory_was_empty"})
    if (value["format_version"] != 1 or value["kind"] != "reference-fixture-service-run"
            or not isinstance(value["execution_backend"], str) or not isinstance(value["scope_id"], str)
            or value["data_directory_was_empty"] is not True):
        raise ValueError("夹具服务清单无效")


def validate_history(run: Path, state: dict, *, ready: bool = True, settled: bool = True) -> dict:
    attempts = state["attempts"]
    boundary = next((index for index, item in enumerate(attempts) if item["stage"] == LIFECYCLE_STAGE), len(attempts))
    initial = attempts[:boundary]
    if len(initial) > len(INITIAL):
        raise ValueError("夹具首代账本包含多余阶段")
    if any((item["stage"], item["mode"]) != INITIAL[index] for index, item in enumerate(initial)):
        raise ValueError("夹具首代账本包含未知阶段或顺序")
    if ready and (len(initial) != len(INITIAL) or any(item["status"] != "passed" for item in initial)):
        raise ValueError("夹具首代服务尚未完整就绪")
    closed = False
    for index, item in enumerate(attempts[boundary:], boundary):
        if item["stage"] != LIFECYCLE_STAGE or item["mode"] not in {"close", "recover"}:
            raise ValueError("夹具服务仅允许明确 close/recover 生命周期追加")
        if settled and item["status"] != "passed":
            raise ValueError("夹具服务存在未成功收尾的生命周期阶段")
        if item["status"] != "passed":
            continue
        sources = item["sources"]
        if sources.get("manifest") != state["manifest"]:
            raise ValueError("夹具生命周期未绑定原始服务清单")
        previous = historical_state(run, sources.get("state_before"))
        if previous["historical_attempts"] != index:
            raise ValueError("夹具生命周期未绑定完整先行账本")
        result = read_json(Path(item["result"]["path"]))
        if (result.get("kind") != "reference-fixture-service-lifecycle"
                or result.get("operation") != item["mode"] or result.get("run") != str(run)
                or result.get("remote_writes") != 0 or result.get("resources_deleted") is not False):
            raise ValueError("夹具生命周期结果未证明原运行目录及零资源删除")
        if item["mode"] == "close":
            if closed or result.get("status") != "services_closed" or result.get("services") != {"redis": "stopped", "rustfs": "stopped"}:
                raise ValueError("夹具关闭结果重复或未证明两项服务退出")
            evidence = result.get("evidence", {})
            if set(evidence) != {"redis", "rustfs"}:
                raise ValueError("夹具关闭缺少两项服务收据")
            for role, filename in (("redis", "stopped.json"), ("rustfs", "rustfs-stopped.json")):
                path = run / f"lifecycle-{index + 1:04d}" / filename
                if evidence[role].get("path") != str(path) or linked(path) or binding(path) != evidence[role]:
                    raise ValueError("夹具关闭收据发生变化或不属于本阶段")
                proof = read_json(path)
                if role == "redis" and (proof.get("status") != "redis_process_stopped"
                                        or proof.get("alive") is not False or proof.get("resources_deleted") is not False):
                    raise ValueError("Redis 关闭收据未确认精确退出")
                if role == "rustfs":
                    _validate_members(proof, run)
            closed = True
        else:
            receipt = result.get("recovery", {})
            path = Path(receipt.get("path", ""))
            if (result.get("status") != "controller_recovered" or path.parent != run
                    or not path.name.startswith("recovered-") or not path.name.endswith(".json")
                    or linked(path) or binding(path) != receipt
                    or read_json(path).get("status") != "controller_recovered"):
                raise ValueError("夹具控制器恢复结果不完整")
    return {"closed": closed, "initial_complete": len(initial) == len(INITIAL)
            and all(item["status"] == "passed" for item in initial),
            "unsettled": [item["number"] for item in attempts if item["status"] != "passed"]}


def _validate_members(proof: dict, run: Path) -> None:
    from full_stack_process_monitor import receipt_path

    tree = proof.get("tree", {})
    completion = proof.get("completion", {})
    if proof.get("state") != "stopped" or tree.get("format_version") != 2:
        raise ValueError("RustFS 关闭收据未绑定当前进程树")
    path = receipt_path(run / "rustfs", "rustfs", tree["operation_id"], "stopped")
    if completion.get("path") != str(path) or linked(path) or binding(path) != completion:
        raise ValueError("RustFS 完整成员关闭证明缺失或变化")
    members = read_json(path)
    if (members.get("status") != "stopped" or members.get("error_type") is not None
            or any(members.get(key) != tree.get(key) for key in ("operation_id", "role", "scope_id", "supervisor", "monitor"))
            or members.get("directory") != tree.get("runtime_directory")
            or not isinstance(members.get("members"), list) or tree.get("supervisor") not in members["members"]):
        raise ValueError("RustFS 完整成员关闭证明不属于原进程树或未收尾")
