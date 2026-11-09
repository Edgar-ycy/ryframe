"""冻结 seed 发布后的存储代次交接；仅追加收据，不改写来源或执行数据操作。"""
from __future__ import annotations

import copy
from pathlib import Path

from devex_clone_seed_guards import guard
from devex_clone_capture import read_json
from devex_clone_run_state import binding, load_state
from devex_clone_seed_segment import segmented_resume
from devex_clone_source_proof import bound_file
from devex_clone_storage import current_storage_binding, registered_storage_binding
from devex_clone_storage_request import directory_identity

def _model(backend: Path, action: str, arguments: dict) -> dict:
    value = guard("rebind-model", {"action": action, "input": arguments}, backend=backend,
                  label="seed 重绑定历史与代次")
    if not isinstance(value, dict):
        raise ValueError("seed 重绑定模型结果无效")
    if "rejected" in value:
        message = value["rejected"]
        if (set(value) != {"rejected"} or not isinstance(message, str) or not message
                or len(message) > 512 or any(char in message for char in "\r\n\0")):
            raise ValueError("seed 重绑定拒绝结果无效")
        raise ValueError(message)
    return value


def history(directory: Path, state: dict, descriptor: dict, *, current: int | None = None,
            backend: Path | None = None, cache_recovery: bool = False) -> list[dict]:
    from devex_clone_seed_generation_prelaunch import closed, proof, origin

    root = backend or Path(__file__).resolve().parents[2]
    attempts = state["attempts"]
    archive = closed(root, directory, attempts)
    segment = (segmented_resume(root, directory, attempts, archive, descriptor, current=current)
               if archive is not None else None)
    pending = []
    prefix = attempts[:-1] if attempts and current == attempts[-1]["number"] else attempts
    if (archive is None and prefix and tuple(prefix[-1][key] for key in ("stage", "mode", "status"))
            == ("seed-runtime", "source-generation-recover", "failed")):
        start, _ = origin(root, directory, prefix)
        pending = prefix[prefix.index(start):]
    frozen_successor = archive["receipt"]["review_successor"] if archive is not None else None
    selected_archive = None if archive is None else {key: archive[key] for key in ("receipt", "records")}
    selected_segment = None if segment is None else {"records": segment["records"]}
    last_output = bool(attempts and (directory / f"g{attempts[-1]['number']:04d}").exists())
    planned = _model(root, "history", {"attempts": attempts, "descriptor": descriptor,
        "current": current, "archive": selected_archive, "segment": selected_segment,
        "pending": pending, "backend_available": backend is not None,
        "cache_recovery": cache_recovery, "generation_output_exists": last_output})
    proofs = []
    for action in planned["actions"]:
        kind = action["kind"]
        if kind == "cache":
            from devex_clone_cache import seed_history_proof

            successor = seed_history_proof(root, directory, action["record"], descriptor,
                                           frozen_successor=frozen_successor)
            proofs.append({"kind": kind, "successor": successor})
            _model(root, "history-proofs", {"proofs": proofs})
            if action["owner"]:
                from devex_clone_run import _require_owned_run

                _require_owned_run(directory)
        elif kind == "registered-cache":
            from devex_clone_cache import registered_cache_binding

            registered_cache_binding(root, directory,
                before=archive["recovery"]["number"] if archive is not None else None,
                frozen=(descriptor, frozen_successor) if frozen_successor is not None else None)
        elif kind == "rebind-cache":
            successor = read_json(bound_file(root, action["record"]["result"])).get("review_successor")
            proofs.append({"kind": kind, "successor": successor})
            _model(root, "history-proofs", {"proofs": proofs})
        elif kind == "owner":
            from devex_clone_run import _require_owned_run

            _require_owned_run(directory)
        elif kind == "zero-output":
            proof(root, directory, action["record"])
        elif kind == "export-reconcile":
            from devex_clone_seed_export import reconciles_failed_export

            if not reconciles_failed_export(directory, state, action["origin"], current, descriptor):
                raise ValueError("seed 发布后存在失败或未收尾阶段")
        elif kind == "arm-prefix":
            guard("arm-prefix-history", {"run": str(directory), "record": action["record"], "source": descriptor},
                  backend=root, label="已发布 ARM 失败前缀")
        else:
            raise ValueError("seed 重绑定模型返回未知证明任务")
    return planned["later"]

def quiet_producers(backend: Path, source: dict) -> None:
    guard("rebind-quiet-producers", {"registration": source["registration"], "request": source["request"]},
          backend=backend, label="发布源生产者谱系与双端口静默")


def transition(backend: Path, original: dict, current: dict | None) -> None:
    directory_identity(backend, original["data_directory"])
    guard("rebind-transition-files", {"original": original, "current": current}, backend=backend,
          label="重绑定固定请求、目录、端点与后续存储代次")
    directory_identity(backend, original["data_directory"])


def _active_read(state: dict, backend: Path) -> int | None:
    return _model(backend, "active-read", {
        "attempts": state["attempts"]})["current"]


def resolve_storage(backend: Path, descriptor: dict, source: dict, state: dict, *,
                    live_storage: bool, current_storage=current_storage_binding) -> dict:
    directory = source["directory"]
    later = history(directory, state, descriptor, current=_active_read(state, backend), backend=backend)
    from devex_clone_seed_generation_prelaunch import closed

    archive = closed(backend, directory, state["attempts"])
    segment = (segmented_resume(backend, directory, state["attempts"], archive, descriptor,
                                current=_active_read(state, backend)) if archive is not None else None)
    selected = _model(backend, "resolve-plan", {"attempts": state["attempts"], "later": later})
    record = selected["record"]
    original = source["storage"]["storage"]
    rebound, receipt, registered = None, None, None
    if record is not None:
        receipt = record["result"]
        if bound_file(backend, receipt) != directory / "results" / f"{record['number']:04d}.json":
            raise ValueError("重绑定结果不属于固定 run attempt")
        value = read_json(bound_file(backend, receipt))
        guard("rebind-receipt", {"value": value, "descriptor": descriptor,
                                 "original": original, "prefix": selected["prefix"]}, backend=backend,
              label="重绑定来源及完整历史前缀")
        bound_file(backend, value["review_successor"])
        rebound = value["current_storage"]
        transition(backend, original, rebound)
        if segment is None:
            registered = registered_storage_binding(backend, directory, "target")
    parameters = {"original": original, "rebound": rebound, "registered": registered,
        "segment": None if segment is None else {key: segment[key] for key in ("storage", "phase")},
        "live": False, "current": None}
    effective = _model(backend, "resolve-storage", parameters)["effective"]
    if live_storage:
        # 先验证封存边界，再观察实时资源；不提前接触未授权的监听代次。
        _model(backend, "resolve-storage", {**parameters, "live": True, "current": effective})
        _model(backend, "resolve-storage", {**parameters, "live": True,
            "current": current_storage(backend, directory, "target")})
        quiet_producers(backend, source)
    result = copy.deepcopy(source)
    result["storage"]["storage"] = copy.deepcopy(effective)
    result["source_rebind"] = receipt
    return result


def published_restart_guard(backend: Path, directory: Path, number: int | None) -> None:
    from devex_clone_seed_source import _registered_source
    from devex_clone_target_binding import predecessor_request_binding, request_binding

    state = load_state(directory)
    selected = _model(backend, "restart-stage", {"attempts": state["attempts"], "number": number})
    if selected["reboot"]:
        from devex_clone_seed_generation_reboot import restart_authorization

        restart_authorization(backend, directory,
                              allow_attempt=None if number is None else "storage-target:restart")
        return
    descriptor = selected["descriptor"]
    later = history(directory, state, descriptor, current=number, backend=backend)
    from devex_clone_seed_generation_prelaunch import closed

    archive = closed(backend, directory, state["attempts"])
    segment = (segmented_resume(backend, directory, state["attempts"], archive, descriptor,
                                current=number) if archive is not None else None)
    _model(backend, "restart-boundary", {"later": later,
        "segment": None if segment is None else {"phase": segment["phase"]}, "number": number})

    def validate_history(root: Path, target: dict) -> tuple[dict, dict]:
        review = read_json(bound_file(root, {key: value for key, value in target["review"].items()
                                          if key != "canonical_sha256"}))
        return (request_binding(root, target) if review["ready_for_execution"] else
                predecessor_request_binding(root, target, target["review"]))

    source = _registered_source(backend, descriptor, live_storage=False, validate_seed_target=validate_history)
    quiet_producers(backend, source)
    original = source["storage"]["storage"]
    directory_identity(backend, original["data_directory"])
    guard("rebind-restart-files", {"original": original}, backend=backend,
          label="发布源固定存储请求、目录与旧代次")
    directory_identity(backend, original["data_directory"])
