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
    external_recovery = None
    active_generation = {"kind": "initial", "rustfs": attempts[0]["result"] if len(attempts) > 0 else None,
                         "redis": attempts[1]["result"] if len(attempts) > 1 else None}
    for index, item in enumerate(attempts[boundary:], boundary):
        if item["stage"] != LIFECYCLE_STAGE or item["mode"] not in {"close", "recover", "restart"}:
            raise ValueError("夹具服务仅允许明确 close/recover/restart 生命周期追加")
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
            if external_recovery is not None:
                raise ValueError("夹具外部终止核对后必须重启，不能补造正常关闭")
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
        elif item["mode"] == "recover":
            receipt = result.get("recovery", {})
            path = Path(receipt.get("path", ""))
            if result.get("status") == "external_termination_reconciled":
                if closed or external_recovery is not None:
                    raise ValueError("夹具外部终止不能在关闭后或未重启时重复核对")
                evidence = result.get("evidence", {})
                evidence_path = Path(evidence.get("path", ""))
                if (result.get("owner") != sources.get("state_before")
                        or evidence_path != run / f"lifecycle-{index + 1:04d}/external-termination.json"
                        or linked(evidence_path) or binding(evidence_path) != evidence):
                    raise ValueError("夹具外部终止核对未绑定完整先行账本及独立证据")
                _validate_external_termination(read_json(evidence_path), run, sources["state_before"], active_generation)
                external_recovery = item["result"]
                continue
            if (result.get("status") != "controller_recovered" or path.parent != run
                    or not path.name.startswith("recovered-") or not path.name.endswith(".json")
                    or linked(path) or binding(path) != receipt
                    or read_json(path).get("status") != "controller_recovered"):
                raise ValueError("夹具控制器恢复结果不完整")
        else:
            if closed or external_recovery is None:
                raise ValueError("夹具服务重启必须紧接明确的外部终止核对")
            if result.get("status") != "services_restarted" or result.get("services") != {
                    "redis": "running", "rustfs": "running"}:
                raise ValueError("夹具服务重启未证明两项服务的新运行代次")
            generation = result.get("generation", {})
            path = Path(generation.get("path", ""))
            if (path != run / f"lifecycle-{index + 1:04d}/restart.json"
                    or linked(path) or binding(path) != generation):
                raise ValueError("夹具服务重启代次收据缺失或变化")
            _validate_restart(read_json(path), run, index + 1, sources["state_before"],
                              external_recovery, active_generation, result.get("controller"))
            active_generation = item["result"]
            external_recovery = None
            closed = False
    return {"closed": closed, "external_recovery": external_recovery,
            "active_generation": active_generation,
            "initial_complete": len(initial) == len(INITIAL)
            and all(item["status"] == "passed" for item in initial),
            "unsettled": [item["number"] for item in attempts if item["status"] != "passed"]}


def _generation_paths(run: Path, generation: dict) -> tuple[Path, Path]:
    if generation.get("kind") == "initial":
        return run / "rustfs", run / "redis"
    result = read_json(Path(generation["path"]))
    document = read_json(Path(result["generation"]["path"]))
    return Path(document["rustfs"]["request"]["path"]).parent, Path(document["redis"]["request"]["path"]).parent


def _validate_external_termination(proof: dict, run: Path, state_before: dict, generation: dict) -> None:
    exact(proof, {"format_version", "kind", "run", "owner", "observation",
                  "normal_shutdown_proof", "remote_writes", "resources_deleted"})
    observation = proof["observation"]
    evidence = observation.get("evidence", {}) if isinstance(observation, dict) else {}
    rustfs, redis = _generation_paths(run, generation)
    tree_path = rustfs / "rustfs-tree.json"
    runtime_path = redis / "runtime.json"
    if (proof["format_version"] != 1 or proof["kind"] != "reference-fixture-external-termination"
            or proof["run"] != str(run) or proof["owner"] != state_before
            or proof["normal_shutdown_proof"] is not None or proof["remote_writes"] != 0
            or proof["resources_deleted"] is not False
            or observation.get("status") != "external-termination-unreconciled"
            or observation.get("normal_shutdown_proof") is not None
            or observation.get("identities") != {role: "missing" for role in ("supervisor", "monitor", "process")}
            or set(evidence) != {"rustfs_tree", "redis_runtime", "rustfs_monitor_ready"}
            or evidence["rustfs_tree"] != binding(tree_path)
            or evidence["redis_runtime"] != binding(runtime_path)):
        raise ValueError("夹具外部终止证据未证明全部身份消失或原运行来源")
    ready_path = Path(evidence["rustfs_monitor_ready"].get("path", ""))
    if (ready_path.parent != rustfs or not ready_path.name.endswith("-ready.json")
            or binding(ready_path) != evidence["rustfs_monitor_ready"]):
        raise ValueError("夹具外部终止证据未绑定原成员监督启动归属")


def _validate_restart(proof: dict, run: Path, number: int, state_before: dict,
                      predecessor: dict, previous_generation: dict, controller: dict) -> None:
    exact(proof, {"format_version", "kind", "run", "attempt", "owner", "predecessor",
                  "previous_generation", "controller", "services", "rustfs", "redis",
                  "remote_writes", "resources_deleted"})
    root = run / f"lifecycle-{number:04d}"
    rustfs, redis = proof["rustfs"], proof["redis"]
    if (proof["format_version"] != 1 or proof["kind"] != "reference-fixture-service-generation"
            or proof["run"] != str(run) or proof["attempt"] != number or proof["owner"] != state_before
            or proof["predecessor"] != predecessor or proof["previous_generation"] != previous_generation
            or proof["controller"] != controller or proof["services"] != {"redis": "running", "rustfs": "running"}
            or proof["remote_writes"] != 0 or proof["resources_deleted"] is not False
            or set(rustfs) != {"request", "storage"} or set(redis) != {"request", "runtime"}):
        raise ValueError("夹具服务重启代次未绑定先行状态及核对结果")
    rustfs_request = Path(rustfs["request"].get("path", ""))
    redis_request = Path(redis["request"].get("path", ""))
    if (rustfs_request != root / "rustfs/request.json" or redis_request != root / "redis/request.json"
            or binding(rustfs_request) != rustfs["request"] or binding(redis_request) != redis["request"]
            or read_json(rustfs_request) != read_json(run / "rustfs/request.json")):
        raise ValueError("夹具服务重启请求未复用原物理资源或不属于本代次")
    storage = rustfs["storage"]
    if (set(storage) != {"identity", "sha256", "process_receipt", "launch_receipt", "tree"}
            or any(Path(storage[key].get("path", "")).parent != root / "rustfs"
                   or binding(Path(storage[key]["path"])) != storage[key]
                   for key in ("process_receipt", "launch_receipt", "tree"))):
        raise ValueError("夹具 RustFS 重启没有绑定新进程树及启动收据")
    runtime = redis["runtime"]
    if (not isinstance(runtime, dict) or Path(runtime.get("output", "")) != root / "redis"
            or read_json(root / "redis/runtime.json") != runtime):
        raise ValueError("夹具 Redis 重启没有绑定新运行收据")
    original = read_json(run / "redis/request.json")
    current = read_json(redis_request)
    stable = {key: value for key, value in current.items()
              if key not in {"previous_identity", "previous_boot_id", "previous_run_id"}}
    if (stable != {key: value for key, value in original.items()
                  if key not in {"previous_identity", "previous_boot_id", "previous_run_id"}}
            or current.get("previous_identity") is None
            or current.get("previous_boot_id") is None or current.get("previous_run_id") is None):
        raise ValueError("夹具 Redis 重启未绑定前一运行代次及原物理配置")


def _validate_members(proof: dict, run: Path) -> None:
    from full_stack_process_monitor import receipt_path

    tree = proof.get("tree", {})
    completion = proof.get("completion", {})
    if proof.get("state") != "stopped" or tree.get("format_version") != 2:
        raise ValueError("RustFS 关闭收据未绑定当前进程树")
    directory = Path(tree.get("runtime_directory", ""))
    if (not directory.is_absolute() or not directory.is_relative_to(run)
            or directory != run / "rustfs" and not (directory.name == "rustfs"
            and directory.parent.parent == run and directory.parent.name.startswith("lifecycle-"))):
        raise ValueError("RustFS 关闭收据不属于服务账本中的已登记代次")
    path = receipt_path(directory, "rustfs", tree["operation_id"], "stopped")
    if completion.get("path") != str(path) or linked(path) or binding(path) != completion:
        raise ValueError("RustFS 完整成员关闭证明缺失或变化")
    members = read_json(path)
    if (members.get("status") != "stopped" or members.get("error_type") is not None
            or any(members.get(key) != tree.get(key) for key in ("operation_id", "role", "scope_id", "supervisor", "monitor"))
            or members.get("directory") != tree.get("runtime_directory")
            or not isinstance(members.get("members"), list) or tree.get("supervisor") not in members["members"]):
        raise ValueError("RustFS 完整成员关闭证明不属于原进程树或未收尾")
