"""两侧与历史重绑定共同核对唯一已发布共享导出。"""
from __future__ import annotations

from pathlib import Path

from devex_clone_capture import read_json
from devex_clone_export_recovery import SUMMARY_FIELDS, _intent, _verified_seal
from devex_clone_model import exact
from devex_clone_run_state import binding, load_state
from devex_clone_source_proof import bound_file
from reference_fixture_successor import published_source

MODES = {"source-export", "source-export-reconcile"}
FIELDS = {"status", "origin_attempt", "origin_intent", "source_registration", "source_rebind", "source_generation",
          "review_successor", "source_request", "source_storage", "export", "summary",
          "remote_writes", "restore_qualified"}


def _source(backend: Path, directory: Path, *, live: bool) -> dict:
    """既有历史 generation 输入继续读取唯一已发布重绑定。"""
    records = [item for item in load_state(directory)["attempts"]
               if (item["stage"], item["mode"]) == ("seed-runtime", "source-rebind")]
    if len(records) != 1 or records[0]["status"] != "passed":
        raise ValueError("seed 导出必须先完成唯一显式 source-rebind")
    rebound = read_json(bound_file(backend, records[0]["result"]))
    source = published_source(backend, rebound["review_successor"], live_storage=live)
    if source["directory"] != directory or source.get("source_rebind") != records[0]["result"]:
        raise ValueError("seed 导出来源不属于当前 run 的冻结重绑定")
    return source


def _reconcile_results(backend: Path, directory: Path, records: list[dict]) -> list[dict]:
    results = []
    for record in records:
        if record["mode"] != "source-export-reconcile" or record["result"] is None:
            continue
        path = bound_file(backend, record["result"])
        if path != directory / "results" / f"{record['number']:04d}.json":
            raise ValueError("seed export 重试结果不属于原固定 attempt")
        value = read_json(path)
        exact(value, FIELDS)
        exact(value["summary"], SUMMARY_FIELDS)
        results.append(value)
    return results


def published_export(backend: Path, descriptor: dict, source: dict) -> dict:
    """核对唯一外层发布及导出清单；实际消费阶段仍完整核验所有 payload。"""
    path = bound_file(backend, descriptor)
    directory = source["directory"]
    state = load_state(directory)
    records = [item for item in state["attempts"] if item["stage"] == "seed-runtime" and item["mode"] in MODES]
    successful = [item for item in records if item["status"] == "passed"]
    if (len(successful) != 1 or successful[0]["result"] != descriptor
            or path != directory / "results" / f"{successful[0]['number']:04d}.json"):
        raise ValueError("arm 必须绑定唯一已发布 seed source-export 外层结果")
    record = successful[0]
    if record["mode"] == "source-export":
        valid_history = records == [record]
    else:
        valid_history = (len(records) >= 2 and records[-1] == record
                         and records[0]["mode"] == "source-export"
                         and records[0]["status"] == "failed"
                         and all(item["mode"] == "source-export-reconcile" and item["status"] == "failed"
                                 for item in records[1:-1]))
    if not valid_history:
        raise ValueError("seed 导出存在重复、未收尾或非法采用历史")
    value = read_json(path)
    exact(value, FIELDS)
    exact(value["summary"], SUMMARY_FIELDS)
    expected = {"status": "seed_source_export_published", "source_registration": source["review_successor"]["source_result"],
                "source_rebind": source.get("source_rebind"), "review_successor": source["review_successor_binding"],
                "source_generation": source.get("source_generation"),
                "source_request": source["source_request"], "source_storage": source["storage"]["storage"],
                "remote_writes": 0, "restore_qualified": False}
    if (source.get("source_rebind") is None or source.get("source_generation") is None
            or any(value.get(key) != item for key, item in expected.items())):
        raise ValueError("seed 导出不属于当前冻结的 C52、重绑定和 successor")
    origins = [item for item in records if item["number"] == value["origin_attempt"] and item["mode"] == "source-export"]
    if len(origins) != 1 or (origins[0] != record and (origins[0]["status"] != "failed" or record["mode"] != "source-export-reconcile")):
        raise ValueError("seed 导出发布没有唯一原导出阶段")
    origin = origins[0]
    if (bound_file(backend, value["origin_intent"]) != directory / f"export-{origin['number']:04d}.intent.json"
            or bound_file(backend, value["export"]) != directory / f"e{origin['number']:04d}/export.json"):
        raise ValueError("seed 导出与原固定 attempt 目录不同")
    intent, intent_binding = _intent(backend, directory, {"source_request": value["source_request"]}, origin)
    if intent_binding != value["origin_intent"] or intent["source_storage"] != value["source_storage"]:
        raise ValueError("seed 导出意图的来源、存储或运行清单不同")
    if record["mode"] == "source-export" and not (directory / f"export-{origin['number']:04d}.verified.json").is_file():
        raise ValueError("成功 source-export 缺少原完整核验封存")
    _verified_seal(directory, origin, intent, intent_binding, value["export"], value["summary"])
    exported = read_json(bound_file(backend, value["export"]))
    from restore_reference_plan import plan_hash

    if (exported.get("request") != value["source_request"]
            or exported.get("logical_inventory_sha256") != value["summary"]["logical_inventory_sha256"]
            or plan_hash(exported) != value["summary"]["export_sha256"]):
        raise ValueError("seed 导出清单不属于已完整复核的发布摘要")
    if any(item != value for item in _reconcile_results(backend, directory, records)):
        raise ValueError("seed 导出发布与先前只读采用结果不同")
    if binding(path) != descriptor or load_state(directory) != state:
        raise ValueError("seed 导出发布在只读核验期间变化")
    return value


def require_export_binding(backend: Path, source: dict, value: dict) -> None:
    result = published_export(backend, value["source_export_result"], source)
    if result["export"] != value["source_export"]:
        raise ValueError("arm 导出必须与唯一外层发布绑定完全相同")


def reconciles_failed_export(directory: Path, state: dict, failed: dict, current: int | None, source_registration: dict) -> bool:
    """仅开放当前持锁的显式采用，或已发布且绑定同一 origin 的采用记录。"""
    records = [item for item in state["attempts"] if item["number"] > failed["number"]
               and (item["stage"], item["mode"]) == ("seed-runtime", "source-export-reconcile")]
    if ((failed["stage"], failed["mode"], failed["status"])
            != ("seed-runtime", "source-export", "failed") or not records):
        return False
    active = [item for item in records if item["number"] == current and item["status"] == "running"]
    passed = [item for item in records if item["status"] == "passed"]
    terminal = {item["number"] for item in active + passed}
    if (len(active) + len(passed) != 1 or active and records[-1] != active[0]
            or passed and records[-1] != passed[0]
            or any(item["number"] not in terminal and item["status"] != "failed" for item in records)):
        return False
    values = []
    for record in records:
        if record["result"] is None:
            continue
        path = directory / "results" / f"{record['number']:04d}.json"
        if binding(path) != record["result"]:
            raise ValueError("seed export 采用结果发生变化")
        value = read_json(path)
        exact(value, FIELDS)
        exact(value["summary"], SUMMARY_FIELDS)
        if (value["status"] != "seed_source_export_published"
                or value["origin_attempt"] != failed["number"]
                or value["source_registration"] != source_registration
                or value["remote_writes"] != 0 or value["restore_qualified"] is not False):
            return False
        values.append(value)
    return not values or all(item == values[0] for item in values)
