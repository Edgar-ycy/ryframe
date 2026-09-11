"""从统一运行的固定 export attempt 中只读核对并显式发布完整源导出。"""
from __future__ import annotations

import copy
from pathlib import Path
import subprocess

from devex_clone_capture import read_json, regular_file, write_json
from devex_clone_model import digest, exact, linked, local_path
from devex_clone_run_state import binding, load_state
from devex_clone_source_proof import bound_file, verify_generation
from restore_reference_plan import plan_hash


INTENT_FIELDS = {"format_version", "kind", "attempt", "output", "manifest", "controller",
                 "source_request", "source_storage", "execution_source", "remote_operations"}
SUMMARY_FIELDS = {"export_sha256", "generation_sha256", "proof_files_sha256",
                  "logical_inventory_sha256", "databases", "objects"}
RECONCILED_FIELDS = {"status", "origin_attempt", "origin_controller", "origin_intent", "export",
                     "source_request", "source_storage", "summary", "origin_execution_source",
                     "reconciled_execution_source", "remote_writes", "restore_qualified"}


def _initial(attempt: dict) -> dict:
    return {**copy.deepcopy(attempt), "finished_at": None, "status": "running", "result": None,
            "error_type": None}


def _controller(backend: Path, directory: Path, attempt: dict) -> dict:
    path = local_path(backend, str(directory / f"controller-{attempt['number']:04d}.json"))
    value = read_json(regular_file(path))
    exact(value, {"format_version", "kind", "owner", "attempt", "attempt_sha256"})
    owner = value["owner"]
    if (value["format_version"] != 1 or value["kind"] != "devex-stage-controller"
            or value["attempt"] != attempt["number"] or value["attempt_sha256"] != plan_hash(_initial(attempt))
            or owner.get("directory") != str(directory)
            or owner.get("manifest_sha256") != binding(directory / "manifest.json")["sha256"]):
        raise ValueError("源导出恢复控制器不属于固定运行的原始 attempt")
    return binding(path)


def prepare_attempt(backend: Path, directory: Path, value: dict, number: int,
                    sources: dict, storage: dict | None, *, seed: bool = False) -> Path:
    """远端读取开始前固定原控制器、源码和 storage provenance；不预建 export 输出目录。"""
    state = load_state(directory)
    if not state["attempts"] or state["attempts"][-1]["number"] != number:
        raise ValueError("只能为当前 export attempt 固定恢复意图")
    attempt = state["attempts"][-1]
    if (attempt["stage"], attempt["mode"], attempt["status"], attempt["sources"]) != (
            "seed-runtime" if seed else "export", "source-export" if seed else "run", "running", sources):
        raise ValueError("源导出恢复意图与当前 attempt 或执行源码不同")
    output = local_path(backend, str(directory / f"e{number:04d}"), new=True)
    intent = {"format_version": 1, "kind": "devex-clone-export-attempt", "attempt": number,
              "output": str(output), "manifest": binding(directory / "manifest.json"),
              "controller": _controller(backend, directory, attempt),
              "source_request": copy.deepcopy(value["source_request"]),
              "source_storage": copy.deepcopy(storage), "execution_source": copy.deepcopy(sources),
              "remote_operations": "read_only"}
    write_json(directory / f"export-{number:04d}.intent.json", intent)
    return output


def _result(backend: Path, attempt: dict) -> dict | None:
    return read_json(bound_file(backend, attempt["result"])) if attempt["result"] is not None else None


def _candidate_attempts(backend: Path, directory: Path, *, before: int, seed: bool = False) -> list[dict]:
    candidates = []
    for attempt in load_state(directory)["attempts"]:
        if (attempt["number"] >= before
                or attempt["stage"] != ("seed-runtime" if seed else "export")
                or attempt["mode"] != ("source-export" if seed else "run")):
            continue
        output = local_path(backend, str(directory / f"e{attempt['number']:04d}"))
        if output.exists() and (linked(output) or not output.is_dir()):
            raise ValueError("源导出 attempt 输出不是固定普通目录")
        exported = output / "export.json"
        if exported.exists() or linked(exported):
            candidates.append(attempt)
    return candidates


def reject_reexport(backend: Path, directory: Path, number: int) -> None:
    """完整 export 一旦出现在既有固定 attempt，普通 run 不得再次读取整个源。"""
    candidates = _candidate_attempts(backend, directory, before=number)
    if candidates:
        raise ValueError("既有 export attempt 已写出完整候选；必须先显式 reconcile，禁止重新完整导出")


def _intent(backend: Path, directory: Path, value: dict, attempt: dict) -> tuple[dict, dict]:
    if attempt["status"] not in {"passed", "failed"}:
        raise ValueError("只能核对已经明确收尾的原 export attempt")
    number = attempt["number"]
    path = local_path(backend, str(directory / f"export-{number:04d}.intent.json"))
    saved = read_json(regular_file(path))
    exact(saved, INTENT_FIELDS)
    output = local_path(backend, str(directory / f"e{number:04d}"))
    expected = {"format_version": 1, "kind": "devex-clone-export-attempt", "attempt": number,
                "output": str(output), "manifest": binding(directory / "manifest.json"),
                "controller": _controller(backend, directory, attempt),
                "source_request": value["source_request"], "execution_source": attempt["sources"],
                "remote_operations": "read_only"}
    if any(saved.get(key) != item for key, item in expected.items()):
        raise ValueError("源导出恢复意图与固定 manifest、controller 或原执行源码矛盾")
    return saved, binding(path)


def _product(source: dict) -> dict:
    fingerprints = source.get("fingerprints") if isinstance(source, dict) else None
    product = fingerprints.get("product") if isinstance(fingerprints, dict) else None
    if (not isinstance(product, dict) or set(product) != {"sha256", "files"}
            or digest(product.get("sha256")) != product["sha256"]
            or type(product.get("files")) is not int or product["files"] < 0):
        raise ValueError("export 恢复必须绑定完整产品源码指纹")
    return product


def complete_summary(verified: dict) -> dict:
    value = verified["export"]
    result = {"export_sha256": plan_hash(value), "generation_sha256": plan_hash(verified["generation"]),
              "proof_files_sha256": plan_hash(verified["proof_files"]),
              "logical_inventory_sha256": value["logical_inventory_sha256"],
              "databases": len(value["databases"]),
              "objects": sum(len(bucket["entries"]) for bucket in value["objects"])}
    exact(result, SUMMARY_FIELDS)
    digest(result["logical_inventory_sha256"])
    return result


def record_verified(directory: Path, number: int, intent: dict, exported: dict,
                    verified: dict) -> None:
    """完整本地复核一结束即封存摘要；storage 二次观察或外层发布中断不会丢失它。"""
    write_json(directory / f"export-{number:04d}.verified.json", {
        "format_version": 1, "kind": "devex-clone-export-verified", "attempt": number,
        "intent": binding(directory / f"export-{number:04d}.intent.json"), "export": exported,
        "source_storage": copy.deepcopy(intent["source_storage"]), "summary": complete_summary(verified),
        "remote_writes": 0,
    })


def _verified_seal(directory: Path, attempt: dict, intent: dict, intent_binding: dict,
                   exported: dict, summary: dict) -> None:
    path = directory / f"export-{attempt['number']:04d}.verified.json"
    if not path.exists() and not linked(path):
        return
    value = read_json(regular_file(path))
    expected = {"format_version": 1, "kind": "devex-clone-export-verified",
                "attempt": attempt["number"], "intent": intent_binding, "export": exported,
                "source_storage": intent["source_storage"], "summary": summary, "remote_writes": 0}
    if value != expected:
        raise ValueError("原 export attempt 的完整复核摘要与当前候选矛盾")


def _only_candidate(backend: Path, directory: Path, value: dict, number: int, *, seed: bool = False) -> tuple[dict, dict, dict, dict]:
    attempts = _candidate_attempts(backend, directory, before=number, seed=seed)
    if len(attempts) != 1 or attempts[0]["status"] != "failed":
        raise ValueError("export reconcile 必须恰有一个由原 failed attempt 推导的完整候选")
    attempt = attempts[0]
    intent, intent_binding = _intent(backend, directory, value, attempt)
    path = local_path(backend, str(directory / f"e{attempt['number']:04d}/export.json"))
    exported = binding(regular_file(path))
    return attempt, intent, intent_binding, exported


def _adoptions(backend: Path, directory: Path, *, before: int) -> list[tuple[dict, dict]]:
    results = []
    for attempt in load_state(directory)["attempts"]:
        if attempt["number"] >= before or attempt["stage"] != "export" or attempt["result"] is None:
            continue
        result = _result(backend, attempt)
        if result.get("status") == "export_reconciled":
            exact(result, RECONCILED_FIELDS)
            if attempt["mode"] != "reconcile" or attempt["status"] not in {"failed", "passed"}:
                raise ValueError("export 采用结果不属于已经收尾的 reconcile attempt")
            results.append((attempt, result))
    if len({plan_hash(item[1]) for item in results}) > 1:
        raise ValueError("同一 run 出现多个不同 export 采用结果")
    return results


def _verify_candidate(backend: Path, directory: Path, value: dict, number: int,
                      sources: dict, environment, *, seed: bool = False) -> tuple[dict, dict, dict, dict, dict]:
    from devex_clone_export_verify import verify_source_export
    from devex_clone_storage import current_storage_binding

    attempt, intent, intent_binding, exported = _only_candidate(backend, directory, value, number, seed=seed)
    if _product(intent["execution_source"]) != _product(sources):
        raise ValueError("原 export attempt 与当前恢复执行的产品源码指纹不同")
    with environment.use("source"):
        storage_before = current_storage_binding(backend, directory, "target" if seed else "source")
        if storage_before != intent["source_storage"]:
            raise ValueError("源 storage provenance 与原 export attempt 不同")
        verified = verify_source_export(backend, exported)
        request = read_json(bound_file(backend, value["source_request"]))
        current_generation = verify_generation(backend, request, subprocess.run)
        storage_after = current_storage_binding(backend, directory, "target" if seed else "source")
    if (verified["binding"] != exported or verified["request"] != request
            or verified["export"]["request"] != value["source_request"]
            or verified["generation"] != current_generation or storage_after != storage_before):
        raise ValueError("候选 export 的请求、generation 或 storage 前后复核不一致")
    summary = complete_summary(verified)
    _verified_seal(directory, attempt, intent, intent_binding, exported, summary)
    adopted = [] if seed else _adoptions(backend, directory, before=number)
    if adopted and any(item[1]["export"] != exported for item in adopted):
        raise ValueError("同一 run 已采用另一份 export")
    return attempt, intent, intent_binding, exported, summary


def reconcile(backend: Path, directory: Path, value: dict, environment, number: int,
              sources: dict) -> dict:
    attempt, intent, intent_binding, exported, summary = _verify_candidate(
        backend, directory, value, number, sources, environment)
    result = {"status": "export_reconciled", "origin_attempt": attempt["number"],
              "origin_controller": intent["controller"], "origin_intent": intent_binding,
              "export": exported, "source_request": value["source_request"],
              "source_storage": intent["source_storage"], "summary": summary,
              "origin_execution_source": intent["execution_source"],
              "reconciled_execution_source": copy.deepcopy(sources), "remote_writes": 0,
              "restore_qualified": False}
    if any(adopted != result for _, adopted in _adoptions(backend, directory, before=number)):
        raise ValueError("本次 export 采用结果与同一 run 的既有结果不同")
    return result


def resume(backend: Path, directory: Path, value: dict, environment, number: int,
           sources: dict) -> dict:
    adoptions = [(attempt, result) for attempt, result in _adoptions(backend, directory, before=number)
                 if attempt["status"] == "passed" and attempt["mode"] == "reconcile"]
    if not adoptions:
        raise ValueError("export resume 前必须先完成显式只读 reconcile")
    adopted_attempt, adopted = adoptions[-1]
    attempt, intent, intent_binding, exported, summary = _verify_candidate(
        backend, directory, value, number, sources, environment)
    expected = {"status": "export_reconciled", "origin_attempt": attempt["number"],
                "origin_controller": intent["controller"], "origin_intent": intent_binding,
                "export": exported, "source_request": value["source_request"],
                "source_storage": intent["source_storage"], "summary": summary,
                "origin_execution_source": intent["execution_source"],
                "reconciled_execution_source": adopted["reconciled_execution_source"],
                "remote_writes": 0, "restore_qualified": False}
    if adopted != expected or adopted_attempt["sources"] != adopted["reconciled_execution_source"]:
        raise ValueError("export resume 的已发布 reconcile 收据与当前固定候选矛盾")
    return {"status": "export_verified", "export": exported,
            "logical_inventory_sha256": summary["logical_inventory_sha256"],
            "reconciliation": binding(Path(adopted_attempt["result"]["path"])),
            "remote_writes": 0, "restore_qualified": False}
