"""C70 源代次因主机重启中断的只读核验；不启动、停止或重放任何真实资源。

观察只接受唯一已收尾的 C70 成功 START 且其后没有停止、恢复或其他源阶段，并核对
失败验收前缀仍然完整。进程代次是否退出只由主机启动证明判定，不按 PID 猜测；重启
丢失的缓存状态不会被记为清理成功，构建环境漂移只作为阻塞项如实报告。
"""

from __future__ import annotations

from pathlib import Path
import subprocess
from native_s3_client import _invoke as native_invoke, _read_response as native_response

from devex_clone_capture import read_json
from devex_clone_model import exact
from devex_clone_run_state import binding, load_state
from devex_clone_source_proof import bound_file
from restore_reference_plan import plan_hash

CLOSE = "source-generation-reboot-close"
CLOSURE_DIRECTORY = "reboot-close"
CLOSURE_FIELDS = {"format_version", "kind", "status", "start", "request", "failed_prefix",
                  "reboot", "roles", "storage", "cache", "baseline", "running", "declared_deltas",
                  "verification_qualified", "replay_allowed", "remote_writes", "restore_qualified"}
CLOSE_RESULT_FIELDS = {"status", "start", "request", "closure", "baseline", "reboot", "failed_prefix",
                       "declared_deltas", "producer", "verification_qualified", "replay_allowed",
                       "remote_writes", "restore_qualified"}


def cache_generations(backend: Path, directory: Path, *, live: bool = True) -> tuple[dict, list[str]]:
    """复用只读缓存状态；内核启动标识变化才证明重启前代次已经退出。"""
    if not live:
        return {"status": "frozen_by_closure"}, []
    from devex_clone_cache import cache_status

    try:
        status = cache_status(backend, directory, allow_close_attempt=True)
    except ValueError as error:
        return {"status": "error", "error_type": type(error).__name__}, ["缓存代次无法完成只读观察"]
    observed, blockers = [], []
    for row in status.get("processes", []):
        linux = row["process"].get("linux", {})
        recorded, current = linux.get("identity", {}).get("boot_id"), linux.get("boot_id")
        changed = bool(recorded) and bool(current) and recorded != current
        observed.append({"attempt": row["attempt"], "state": row["process"]["state"],
                         "linux_alive": linux.get("alive"), "recorded_boot_id": recorded,
                         "current_boot_id": current, "boot_id_changed": changed})
        if row["process"]["state"] != "stopped" or linux.get("alive") is not False or not changed:
            blockers.append(f"缓存代次 {row['attempt']} 没有明确退出证据")
    if not observed:
        blockers.append("缓存登记为空，无法核对重启前代次")
    return {"status": status["status"], "request": status["request"], "generations": observed}, blockers


def build_context_state(backend: Path, facts: dict) -> dict:
    """如实报告构建环境漂移：漂移不是重启事实，但会阻塞后续收口。"""
    from devex_clone_tools import verify_evidence
    from restore_build import build_context

    execution = Path(facts["request"]["execution_backend"])
    expected = facts["request"]["expected_backend_sha"]
    observed = {}
    for name, field, root in (("backend", "backend_build", backend),
                              ("maintenance", "maintenance_build", execution)):
        path = bound_file(root, facts["request"][field])
        try:
            receipt = read_json(path) if name == "backend" else verify_evidence(execution, path)
            matches = (receipt["build"] == build_context(execution) if name == "backend"
                       else receipt["source"]["snapshot"]["head"] == expected
                       and receipt["backend_root"] == str(execution))
            observed[name] = {"matches": matches, "evidence": binding(path)}
        except ValueError as error:
            observed[name] = {"matches": False, "error_type": type(error).__name__, "evidence": binding(path)}
    return observed


def observe(backend: Path, directory: Path) -> dict:
    """完整只读观察：绑定重启中断现场并列出继续收口前的阻塞项。"""
    return _observation(backend, directory)["value"]


def _observation(backend: Path, directory: Path, *, live_cache: bool = True) -> dict:
    """观察实现；返回完整结果与只依据重启事实的授权判定。"""
    directory = directory.resolve()
    result, _ = native_response(*native_invoke(
        "__source-evidence", {"operation": "source-generation-reboot-observation", "write": False,
            "arguments": {"backend": str(backend.resolve()), "directory": str(directory)}},
        subprocess.run, may_write=False))
    exact(result, {"value"})
    native = result["value"]
    exact(native, {"start", "later", "facts", "prefix", "proof", "roles", "storage", "blockers"})
    tail = {"start": native["start"], "later": native["later"]}
    facts = dict(native["facts"], output=Path(native["facts"]["output"]))
    prefix, proof = native["prefix"], native["proof"]
    role_facts, storage = native["roles"], native["storage"]
    blockers = list(native["blockers"])
    cache, cache_blockers = cache_generations(backend, directory, live=live_cache)
    blockers += cache_blockers
    reboot_blockers = list(blockers)
    build = build_context_state(backend, facts)
    if not all(row["matches"] for row in build.values()):
        blockers.append("构建环境与登记收据不一致，收口前必须成对重新登记构建收据")
    history = [row for row in load_state(directory)["attempts"] if row["number"] < tail["start"]["number"]]
    value = {"status": "source_generation_reboot_observed", "attempt": tail["start"]["number"],
             "start": tail["start"]["result"], "history_length": len(history),
             "history_sha256": plan_hash(history),
             "service_attempts": [row["number"] for row in tail["later"]],
             "verification": prefix, "reboot": proof, "roles": role_facts,
             "storage": storage, "cache": cache,
             "build_context": build,
             "blockers": blockers, "closure_ready": not blockers,
             "remote_writes": 0, "restore_qualified": False}
    return {"value": value, "facts": facts, "prefix": prefix, "proof": proof,
            "reboot_blockers": reboot_blockers, "reboot_verified": not reboot_blockers,
            "start": tail["start"]}


def preflight(backend: Path, directory: Path, request_path: Path | None) -> dict:
    """收尾前只读前置：要求已核实重启事实，且请求仍是 C70 冻结的同一正式请求。"""
    directory = directory.resolve()
    if request_path is None:
        raise ValueError("重启收尾必须显式提供 C70 冻结的正式请求")
    request_path = request_path.resolve()
    if closure_record(directory) is not None:
        raise ValueError("主机重启收尾已经发布，不能重复收尾")
    settled = [row for row in load_state(directory)["attempts"]
               if (row["stage"], row["mode"]) == ("seed-runtime", CLOSE) and row["status"] != "running"]
    if settled and any(row["result"] is not None for row in settled) \
            or (directory / "g0070" / CLOSURE_DIRECTORY).exists():
        raise ValueError("主机重启收尾已发布或留下输出，不能直接重放")
    observation = _observation(backend, directory)
    if not observation["reboot_verified"]:
        raise ValueError("主机重启事实尚未完整核实：" + "；".join(observation["reboot_blockers"]))
    if observation["value"]["service_attempts"]:
        raise ValueError("重启收尾只能在登记新服务代次之前执行")
    receipt = observation["facts"]["receipt"]
    if binding(request_path) != receipt["request"]:
        raise ValueError("重启收尾必须绑定 C70 冻结的同一正式请求")
    return observation


def closure_record(directory: Path) -> dict | None:
    """唯一已发布的重启收尾记录；不存在返回 None，重复或未收尾即拒绝。"""
    directory = directory.resolve()
    records = [row for row in load_state(directory)["attempts"]
               if (row["stage"], row["mode"]) == ("seed-runtime", CLOSE)]
    settled = [row for row in records if row["status"] != "running"]
    published = [row for row in settled if row["status"] == "passed"]
    if len(published) > 1 or any(row["status"] != "failed" for row in settled if row not in published):
        raise ValueError("主机重启收尾不能重复、失败或未收尾")
    records = published
    if not records:
        return None
    if len(records) != 1 or records[0]["result"] is None:
        raise ValueError("主机重启收尾不能重复、失败或未收尾")
    return records[0]


def closure_receipt(backend: Path, directory: Path) -> dict:
    """复核已发布收尾收据并返回它的绑定与内容。"""
    record = closure_record(directory)
    if record is None:
        raise ValueError("缺少唯一主机重启收尾收据，不能登记新的服务代次")
    result_path = bound_file(backend, record["result"])
    result = read_json(result_path)
    exact(result, CLOSE_RESULT_FIELDS)
    if result["status"] != "seed_source_generation_reboot_closed":
        raise ValueError("主机重启收尾阶段结果状态无效")
    path = bound_file(backend, result["closure"])
    if path != directory / "g0070" / CLOSURE_DIRECTORY / "closure.json":
        raise ValueError("主机重启收尾收据不属于固定代次目录")
    closure = read_json(path)
    exact(closure, CLOSURE_FIELDS)
    if (type(closure["format_version"]) is not int or closure["format_version"] != 1
            or closure["kind"] != "seed-source-generation-reboot-closure"
            or closure["status"] != "seed_source_generation_reboot_closed"
            or closure["verification_qualified"] is not False
            or closure["replay_allowed"] is not False
            or type(closure["remote_writes"]) is not int or closure["remote_writes"] != 0
            or closure["restore_qualified"] is not False
            or closure["declared_deltas"] != {"sys_login_info": 11, "sys_oper_log": 0,
                                              "sys_outbox_event": 0}):
        raise ValueError("主机重启收尾收据的状态、计数或声明增量无效")
    generations = closure["cache"].get("generations") if isinstance(closure["cache"], dict) else None
    if not generations or any(row.get("state") != "stopped" for row in generations):
        raise ValueError("主机重启收尾收据没有记录已完成退出的缓存代次")
    return {"record": record, "binding": binding(path), "result": binding(result_path), "receipt": closure}


def restart_authorization(backend: Path, directory: Path, *, allow_attempt: str | None = None,
                          live_cache: bool = True) -> dict:
    """登记新服务代次的唯一授权：收据绑定加实时重启事实复核。"""
    issued = closure_receipt(backend, directory)
    observation = _observation(backend, directory, live_cache=live_cache)
    if not observation["reboot_verified"]:
        raise ValueError("主机重启事实未通过复核：" + "；".join(observation["reboot_blockers"]))
    if (issued["receipt"]["start"] != observation["start"]["result"]
            or issued["receipt"]["request"] != observation["facts"]["receipt"]["request"]):
        raise ValueError("重启收尾收据没有绑定当前源代次或冻结请求")
    later = [row for row in load_state(directory)["attempts"] if row["number"] > issued["record"]["number"]]
    allowed = {("storage-target", "restart"), ("cache-target", "restart")}
    for row in later:
        if (row["stage"], row["mode"]) not in allowed or row["status"] not in {"passed", "running"}:
            raise ValueError("重启收尾之后存在其他阶段或未收尾的服务登记")
        if row["status"] == "running" and f"{row['stage']}:{row['mode']}" != allow_attempt:
            raise ValueError("当前持锁服务阶段与授权请求不一致")
    return {"closure": issued, "observation": observation, "later": later}


def restart_tail(backend: Path, directory: Path, attempts: list, tail: list, archive: dict,
                 descriptor: dict, current: int | None) -> dict | None:
    """识别收尾后唯一的目标存储与缓存重启；不匹配返回 None，不完整即拒绝。"""
    from devex_clone_seed_generation_lineage_retry import completed

    closures = [row for row in attempts if (row["stage"], row["mode"]) == ("seed-runtime", CLOSE)]
    if not closures:
        return None
    published = [row for row in closures if row["status"] == "passed" and row["result"] is not None]
    if len(published) != 1 or any(row["status"] != "failed" for row in closures if row not in published):
        raise ValueError("主机重启收尾不能重复、失败或未收尾")
    closures = published
    authority = completed(backend, directory, attempts)
    if authority is None:
        raise ValueError("存在重启收尾收据但缺少已收尾的 C70 源代次")
    start = authority["completed_successor"]
    rows = [row for row in tail if row["number"] > start["number"]]
    if not rows:
        return None
    shapes = (("seed-runtime", CLOSE, "passed"), ("storage-target", "restart", "passed"),
              ("cache-target", "restart", "passed"))
    if len(rows) != len(shapes) or any(
            row["number"] != start["number"] + offset
            or tuple(row.get(key) for key in ("stage", "mode", "status")) != shape
            for offset, (row, shape) in enumerate(zip(rows, shapes), 1)):
        raise ValueError("主机重启收尾后的目标存储与缓存重启不完整或不相邻")
    observation = _observation(backend, directory, live_cache=False)
    if not observation["reboot_verified"]:
        raise ValueError("重启段现场未通过复核：" + "；".join(observation["reboot_blockers"]))
    issued = closure_receipt(backend, directory)
    if issued["record"]["number"] != rows[0]["number"]:
        raise ValueError("重启段没有采用当前唯一收尾收据")
    from devex_clone_cache import registered_cache_binding
    from devex_clone_storage import registered_storage_binding

    storage_row, cache_row = rows[1], rows[2]
    storage = registered_storage_binding(backend, directory, "target", before=cache_row["number"] + 1)
    if (storage is None or storage["attempt"] != storage_row["number"]
            or storage["restart_result"] != storage_row["result"]):
        raise ValueError("收尾后的对象存储重启没有绑定同一登记代次")
    cache = registered_cache_binding(
        backend, directory, before=cache_row["number"] + 1,
        frozen=(descriptor, archive["receipt"]["review_successor"]))
    if (cache is None or cache["attempt"] != cache_row["number"]
            or cache["restart_result"] != cache_row["result"]):
        raise ValueError("收尾后的缓存重启没有绑定同一登记代次")
    return {"records": tuple(rows), "storage": storage, "cache": cache,
            "closure": issued["binding"], "baseline": issued["receipt"]["baseline"],
            "deltas": issued["receipt"]["declared_deltas"]}


def execute_close(backend: Path, directory: Path, request_path: Path, number: int) -> dict:
    """发布一次重启收尾收据；不重启服务、不采集新像、不重放任何源阶段。"""
    from devex_clone_run import _require_owned_run
    from devex_clone_run_state import load_state as _load
    from restore_source_runtime import _producer_bindings

    observation = preflight(backend, directory, request_path)
    state = _load(directory)
    if (not state["attempts"] or state["attempts"][-1]["number"] != number
            or tuple(state["attempts"][-1][key] for key in ("stage", "mode", "status"))
            != ("seed-runtime", CLOSE, "running")):
        raise ValueError("重启收尾不属于当前唯一持锁阶段")
    _require_owned_run(directory)
    facts = observation["facts"]
    output = facts["output"] / CLOSURE_DIRECTORY
    if output.exists():
        raise ValueError("重启收尾目录已存在，不能覆盖或重放")
    output.mkdir()
    closure = {"format_version": 1, "kind": "seed-source-generation-reboot-closure",
               "status": "seed_source_generation_reboot_closed",
               "start": observation["start"]["result"], "request": facts["receipt"]["request"],
               "failed_prefix": observation["prefix"], "reboot": observation["proof"],
               "roles": observation["value"]["roles"], "storage": observation["value"]["storage"],
               "cache": observation["value"]["cache"], "baseline": facts["receipt"]["before"],
               "running": facts["receipt"]["running"],
               "declared_deltas": {"sys_login_info": 11, "sys_oper_log": 0, "sys_outbox_event": 0},
               "verification_qualified": False, "replay_allowed": False,
               "remote_writes": 0, "restore_qualified": False}
    from devex_clone_capture import write_json

    write_json(output / "closure.json", closure)
    return {"status": closure["status"], "start": closure["start"], "request": closure["request"],
            "closure": binding(output / "closure.json"), "baseline": closure["baseline"],
            "reboot": closure["reboot"], "failed_prefix": closure["failed_prefix"],
            "declared_deltas": closure["declared_deltas"], "producer": _producer_bindings(
                facts["output"] / "verification"),
            "verification_qualified": False, "replay_allowed": False,
            "remote_writes": 0, "restore_qualified": False}
