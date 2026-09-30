"""在原复制目录显式核对及续跑；不重建目标、不重导来源、不抢占残留锁。"""
from __future__ import annotations

import datetime as dt
from pathlib import Path
import subprocess
import uuid

import devex_clone_factory as factory
from devex_clone import read_json, verify_plan_result
from devex_clone_capture import regular_file, write_json
from devex_clone_factory_plan import compose_declaration
from devex_clone_ledger import LedgerError, project
from devex_clone_copy_locks import register_copy_owner
from devex_clone_model import bound_file as artifact_file, exact, local_path
from devex_clone_source_proof import bound_file
from devex_clone_target_state import generation_lock
from devex_clone_transfer import TransferSteps
from restore_reference_io import ExternalTools
from restore_reference_plan import plan_hash


def session_record(backend: Path, output: Path) -> dict:
    """保存记录只用于定位原证据，不能单独授予远端写入许可。"""
    root = local_path(backend, str(output))
    value = read_json(regular_file(root / "session.json"))
    exact(value, {"source_export", "initialized", "generation_sha256", "plan_sha256",
                  "source_observation", "clone_verified", "restore_qualified"})
    if value["clone_verified"] is not False or value["restore_qualified"] is not False:
        raise ValueError("复制准备记录不能声明克隆或恢复完成")
    return value


def hydrate(session: factory.CloneSession, saved: dict) -> None:
    """重新取得同一来源、初始目标和当前库存，复原 guard，不重复执行 prepare。"""
    session.source_verified = factory.verify_source_export(session.backend, session.export_binding)
    session.exported = session.source_verified["export"]
    session.source_request = session.source_verified["request"]
    _, session.target_request = factory.initialization_history(
        session.backend, session.target_directory / "initialized.json", session.owned_lock_identity)
    session.bind_credentials()
    source_generation = session._source_generation()
    initial, session.target_request, _ = session._target_generation()
    selected = initial["generation"]["selected"]
    target = {**session.target_request["target"], "runtime_dir": selected["runtime_dir"], "api_url": selected["api_url"]}
    tools = {role: session.source_request["tools"][role] for role in ("mysql", "aws")}
    if tools != session.target_request["tools"]:
        raise ValueError("续跑两侧 MySQL 与 S3 工具不同")
    session.tools = ExternalTools({"source": session.exported["source"], "target": target, "tools": tools},
                                  session.output, session.run)
    session.source_initial = session._source_inventory()
    observed = {key: {"tables": item.tables, "preserved": item.preserved} for key, item in session.source_initial.items()}
    if observed != saved["source_observation"]:
        raise ValueError("来源当前完整行像与原复制会话不同")
    by_name = initial["inventory"]["observations"]
    session.initialized = {db["key"]: factory.database_observation(by_name[db["database"]]) for db in target["databases"]}
    session.initial_target = initial
    session.plan_file = regular_file(session.output / "plan.json")
    session.verified_plan = verify_plan_result(session.backend, str(session.plan_file))
    session.plan = session.verified_plan.plan
    wrapper = read_json(session.plan_file)
    if local_path(session.backend, wrapper["input_path"]) != session.output / "input.json":
        raise ValueError("续跑计划必须绑定原执行目录的输入")
    session.declaration = read_json(regular_file(session.output / "input.json"))
    evidence = session.declaration["evidence"]
    root = local_path(session.backend, session.exported["artifact_root"])
    if (read_json(artifact_file(root, evidence["target_initialized"])) != initial
            or evidence["source_stopped"] != session.exported["evidence"]["generation-after"]):
        raise ValueError("原计划未绑定同一来源停止与目标初始化证明")
    stopped = read_json(artifact_file(root, evidence["target_stopped"]))
    if stopped.get("initialized") != session.target_binding:
        raise ValueError("原计划目标停止证明不属于本初始化")
    expected = compose_declaration(session.exported, session.source_initial, target, session.initialized, evidence,
                                   copy_id=session.declaration["copy_id"], stage=session.declaration["copy_stage"])
    if session.declaration != expected or saved["plan_sha256"] != session.plan["plan_sha256"]:
        raise ValueError("原计划与当前完整来源、目标及初始像不同")
    session.generation_sha256 = plan_hash({"source": session.export_binding, "target": session.target_binding,
        "source_generation": source_generation, "target_generation": initial["generation"],
        "environments": session.environment.bindings, "plan": session.plan["plan_sha256"]})
    if saved["generation_sha256"] != session.generation_sha256:
        raise ValueError("原复制代次、秘密环境或来源证据变化")
    with session.environment.use("source") as environment:
        factory.observe_source_objects(session.backend, session.tools, session.source_verified,
                                       session.directory("source-objects-resume"), environment=environment)
    if session_record(session.backend, session.output) != saved:
        raise ValueError("复原期间原会话记录变化")


def reconcile_steps(steps: TransferSteps) -> dict:
    """未知结果只用真实前后像分类；未登记资源也必须保有原前像。"""
    existing = steps.ledger.snapshot()["steps"]
    planned = steps.databases | steps.objects
    if set(existing) - set(planned):
        raise ValueError("账本含有不属于完整计划的步骤")
    observed = steps.check_databases("reconcile", list(steps.databases))
    observed.update(steps.check_objects("reconcile", list(steps.objects)))
    return {"status": "copy_reconciled" if "mismatch" not in observed.values() else "needs_reconciliation",
            "steps": observed, "pending_steps": sorted(set(planned) - set(existing)),
            "remote_writes": 0, "target_ready": False, "restore_success": False}


def resume_steps(steps: TransferSteps) -> None:
    """先复验全部已确认后像，再执行尚未完成的明确步骤。"""
    existing = steps.ledger.snapshot()["steps"]
    planned = steps.databases | steps.objects
    if set(existing) - set(planned) or any(item["phase"] not in {"confirmed", "reconciled_before"} for item in existing.values()):
        raise ValueError("未知或不匹配步骤必须先显式核对")
    steps.check_databases("confirmed", [step for step in steps.databases
                                        if step in existing and existing[step]["phase"] == "confirmed"])
    steps.check_objects("confirmed", [step for step in steps.objects
                                      if step in existing and existing[step]["phase"] == "confirmed"])
    for step, item in planned.items():
        if step in existing:
            if existing[step]["phase"] == "reconciled_before":
                steps.resume(step)
        elif step in steps.databases:
            steps.apply_database(item["key"])
        else:
            steps.apply_object(item["bucket"], item["target_key"])
    steps.finish()


def check_continuation(backend: Path, output: Path, saved: dict, mode: str) -> None:
    """先只读检查账本，已知中断或残留锁不应触发昂贵的全量来源核验。"""
    ledger = factory.CloneLedger(backend, output / "ledger", saved["plan_sha256"], saved["generation_sha256"], mode=mode)
    ledger.inspect()
    if ledger.path("clone.lock").exists():
        raise LedgerError("复制账本有残留或活动锁，须先核对持有进程身份")
    state = project(ledger.events)
    if mode == "resume" and (state["session"]["outcome"] != "partial"
            or any(item["phase"] not in {"confirmed", "reconciled_before"} for item in state["steps"].values())):
        raise LedgerError("上次复制未正常暂停或有未知写入，必须先显式 reconcile")


def continue_copy(backend: Path, output: Path, source_environment: dict, target_environment: dict, *,
                  mode: str, run=subprocess.run, source_storage_run: Path | None = None,
                  target_storage_run: Path | None = None) -> dict:
    """显式 reconcile 不写远端；resume 仅续跑本账本，残留锁须事先另行核验处理。"""
    if mode not in {"reconcile", "resume"}:
        raise ValueError("复制接续模式必须为 reconcile 或 resume")
    backend, output = backend.resolve(strict=True), local_path(backend, str(output))
    saved = session_record(backend, output)
    session = factory.CloneSession(
        backend, saved["source_export"], saved["initialized"], source_environment,
        target_environment, output, run, existing=True,
        source_storage_run=source_storage_run, target_storage_run=target_storage_run,
    )
    session.target_directory = bound_file(backend, session.target_binding).parent
    attempt = "continuation-" + uuid.uuid4().hex
    receipt = output / (attempt + ".json")
    try:
        check_continuation(backend, output, saved, mode)
        owner = register_copy_owner(backend, output)
        session.copy_owner = owner
        with session.environment.use("target"), generation_lock(session.target_directory, copy_owner=owner):
            session.owned_lock_identity = (session.target_directory / "initialize.lock").stat().st_ino
            hydrate(session, saved)
            session.verify_target_owners()
            ledger = factory.CloneLedger(backend, output / "ledger", session.plan["plan_sha256"],
                                          session.generation_sha256, mode=mode, copy_owner=owner)
            with ledger:
                steps = TransferSteps(backend, session.plan_file, session.tools, ledger, session, verified_plan=session.verified_plan)
                if mode == "reconcile":
                    result = reconcile_steps(steps)
                else:
                    resume_steps(steps)
            if mode == "resume":
                result = steps.result()
            snapshot = ledger.inspect()
        session.owned_lock_identity = None
        if mode == "resume":
            final = local_path(backend, str(output / "result.json"))
            if final.exists():
                if read_json(regular_file(final)) != result:
                    raise ValueError("原结果不同，不能覆盖历史完成证据")
            else:
                write_json(final, result)
        write_json(receipt, {"mode": mode, "completed_at": dt.datetime.now(dt.timezone.utc).isoformat(),
                             "plan_sha256": session.plan["plan_sha256"], "generation_sha256": session.generation_sha256,
                             "ledger": snapshot, "result": result})
        return {**result, "continuation_receipt": str(receipt)}
    except BaseException as error:
        session.owned_lock_identity = None
        if not receipt.exists():
            write_json(receipt, {"mode": mode, "status": "needs_reconciliation", "error_type": type(error).__name__,
                                 "automatic_retry": False, "automatic_resource_cleanup": False,
                                 "target_ready": False, "restore_success": False})
        raise
