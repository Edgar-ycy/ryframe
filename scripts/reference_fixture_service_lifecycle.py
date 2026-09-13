"""在原服务账本中显式关闭服务或恢复死亡控制器，未知结果不自动重放。"""
from __future__ import annotations

import copy
from pathlib import Path
import time

from devex_clone_cache_process import start as start_cache, stop as stop_cache
from devex_clone_capture import read_json, write_json
from process_environment import configured
from devex_clone_model import local_path
from devex_clone_run_state import (begin, bind_controller_attempt, binding, claim_run_lock,
                                   controller_observation, finish, load_state,
                                   recover_run_lock, run_lock)
from full_stack_process_tree import terminate_owned_process_tree
from full_stack_process_monitor import completion_binding
from full_stack_process import process_identity
from process_guard import process_guard
from reference_fixture_service_context import (context, guard, observe_services, registered_services,
                                               unknown_recoveries)
from reference_fixture_service_history import (LIFECYCLE_STAGE, close_reconciliation_evidence,
                                               reconcilable_failed_close, validate_history)


def _result(run: Path, operation: str, status: str, **details) -> dict:
    return {"format_version": 1, "kind": "reference-fixture-service-lifecycle", "operation": operation,
            "status": status, "run": str(run), "remote_writes": 0, "resources_deleted": False, **details}


def _stable_services(value: dict, services: dict, running_state: dict) -> None:
    guard(value, running_state)
    if unknown_recoveries(value["run"]):
        raise ValueError("关闭期间出现未知控制恢复或生命周期证据")
    if registered_services(value) != services:
        raise ValueError("关闭期间服务请求、进程树或运行收据发生变化")


def _closed_services(services: dict) -> dict:
    deadline = time.monotonic() + 5
    while True:
        pending = False
        for expected in (services["tree"]["supervisor"], services["tree"]["process"]):
            try:
                actual = process_identity(expected["pid"])
            except PermissionError:
                # Windows 正在退出的进程可能短暂拒绝查询镜像路径。只把它当作
                # 尚未完成的观察继续等待，绝不据此签发已关闭证明。
                pending = True
                continue
            if actual is not None and actual != expected:
                raise ValueError("关闭等待期间 RustFS 进程 PID 已复用")
            pending |= actual is not None
        if not pending:
            observed = observe_services(services)
            if observed != {"redis": "stopped", "rustfs": "stopped", "termination": None}:
                raise ValueError("夹具服务完整进程树尚未退出")
            return observed
        if time.monotonic() >= deadline:
            raise TimeoutError("关闭夹具服务后原进程身份仍存活")
        time.sleep(0.05)


def close(backend: Path, review: Path, bootstrap: Path, *, write: bool) -> dict:
    if not write:
        raise ValueError("关闭夹具服务必须显式指定 --write")
    value = context(backend, review, bootstrap)
    run = value["run"]
    history = validate_history(run, value["state"])
    if unknown_recoveries(run):
        raise ValueError("存在未知控制恢复结果，必须先核实完整前后像")
    if controller_observation(run) is not None:
        raise ValueError("存在当前或遗留控制器，不能开始服务关闭")
    services = registered_services(value)
    observed = observe_services(services)
    if observed["termination"] is not None:
        raise ValueError("夹具服务遭外部终止，必须先显式 recover 核对现场")
    if history["closed"]:
        return _result(run, "close", "services_already_closed", services=_closed_services(services))
    with run_lock(run) as owner:
        guard(value)
        number = begin(run, LIFECYCLE_STAGE, "close", value["sources"])
        controller = bind_controller_attempt(run, number, owner)
        running_state = binding(run / "state.json")
        try:
            output = local_path(backend, str(run / f"lifecycle-{number:04d}"), new=True)
            output.mkdir()
            _stable_services(value, services, running_state)
            cache = stop_cache(services["requests"]["redis"], configured(value["private"]),
                               services["runtime"], output)
            _stable_services(value, services, running_state)
            # Redis 已经精确退出后才操作 RustFS，始终通过原监督器的树控制合同。
            terminated = terminate_owned_process_tree(services["tree"])
            observations = _closed_services(services)
            completion = completion_binding(services["tree"])
            _stable_services(value, services, running_state)
            storage_file = output / "rustfs-stopped.json"
            write_json(storage_file, {"tree": services["tree"], "terminated": terminated,
                                      "state": "stopped", "completion": completion})
            result = _result(run, "close", "services_closed",
                             services={key: observations[key] for key in ("redis", "rustfs")}, controller=controller,
                             evidence={"redis": binding(output / "stopped.json"), "rustfs": binding(storage_file)})
            if cache.get("status") != "redis_process_stopped":
                raise ValueError("Redis 关闭未返回明确结果")
            finish(run, number, result=result)
        except BaseException as error:
            finish(run, number, error=error)
            raise
    return result


def _reconciliation_evidence(value: dict, failed: dict, services: dict) -> tuple[dict, dict]:
    evidence, failed_controller = close_reconciliation_evidence(
        value["run"], failed, value["history"]["active_generation"], value["sources"], services)
    completion = completion_binding(services["tree"])
    if completion != evidence["rustfs_members"]:
        raise ValueError("RustFS 完整成员关闭证明在进程核对期间变化")
    observations = observe_services(services)
    if observations != {"redis": "stopped", "rustfs": "stopped", "termination": None}:
        raise ValueError("失败 close 和解时服务或端口尚未完整停止")
    return observations, {"evidence": evidence, "failed_controller": failed_controller}


def reconcile(backend: Path, review: Path, bootstrap: Path, owner_file: Path, *, write: bool) -> dict:
    if not write:
        raise ValueError("和解夹具关闭证据必须显式指定 --write")
    value = context(backend, review, bootstrap)
    run = value["run"]
    owner_path = local_path(backend, str(owner_file if owner_file.is_absolute() else backend / owner_file))
    if owner_path != run / "state.json" or binding(owner_path) != value["sources"]["state_before"]:
        raise ValueError("夹具关闭和解必须精确绑定 status 返回的当前 state 文件")
    failed = reconcilable_failed_close(value["state"], value["history"])
    if failed is None or unknown_recoveries(run) or controller_observation(run) is not None:
        raise ValueError("只允许和解唯一未结算的失败 close，且不能有控制器或未知恢复")
    services = registered_services(value, settled=False)
    observations, proof_sources = _reconciliation_evidence(value, failed, services)
    with run_lock(run) as owner:
        guard(value)
        if unknown_recoveries(run):
            raise ValueError("发布和解前出现未知恢复")
        number = begin(run, LIFECYCLE_STAGE, "reconcile", value["sources"])
        controller = bind_controller_attempt(run, number, owner)
        running_state = binding(run / "state.json")
        try:
            output = local_path(backend, str(run / f"lifecycle-{number:04d}"), new=True)
            output.mkdir()
            current_observations, current_sources = _reconciliation_evidence(value, failed, services)
            if current_observations != observations or current_sources != proof_sources:
                raise ValueError("发布和解前关闭证据或服务状态发生变化")
            proof_file = output / "reconciliation.json"
            proof = {"format_version": 1, "kind": "reference-fixture-failed-close-reconciliation",
                     "run": str(run), "attempt": number, "owner": value["sources"]["state_before"],
                     "failed_attempt": failed["number"], "failed_error_type": failed["error_type"],
                     "failed_sources": failed["sources"],
                     "failed_controller": proof_sources["failed_controller"],
                     "services": {key: observations[key] for key in ("redis", "rustfs")},
                     "evidence": proof_sources["evidence"],
                     "remote_writes": 0, "resources_deleted": False}
            write_json(proof_file, proof)
            guard(value, running_state)
            final_observations, final_sources = _reconciliation_evidence(value, failed, services)
            if final_observations != observations or final_sources != proof_sources:
                raise ValueError("和解证据写入后关闭现场发生变化")
            result = _result(run, "reconcile", "failed_close_reconciled", controller=controller,
                             owner=value["sources"]["state_before"], failed_attempt=failed["number"],
                             services=proof["services"], reconciliation=binding(proof_file))
            finish(run, number, result=result)
        except BaseException as error:
            finish(run, number, error=error)
            raise
    return result


def recover(backend: Path, review: Path, bootstrap: Path, owner_file: Path, *, write: bool) -> dict:
    if not write:
        raise ValueError("恢复夹具控制锁必须显式指定 --write")
    value = context(backend, review, bootstrap)
    run = value["run"]
    owner_path = local_path(backend, str(owner_file if owner_file.is_absolute() else backend / owner_file))
    observed = controller_observation(run)
    if observed is None:
        return _recover_external(value, owner_path)
    owner_binding = read_json(owner_path)
    if observed is None or observed["owner"] != owner_binding or observed["process_missing"] is not True:
        raise ValueError("只允许回收用户明确绑定且确已死亡的原控制器；存活或 PID 复用均拒绝")
    if unknown_recoveries(run):
        raise ValueError("旧控制恢复 intent 未完成，不能再次自动恢复")
    with process_guard(run, "run-control.guard"):
        guard(value)
        previous_receipts = set(run.glob("recovered-*.json"))
        recovered = recover_run_lock(backend, run, owner_binding, verify_results=True)
        published = set(run.glob("recovered-*.json")) - previous_receipts
        if len(published) != 1:
            raise ValueError("控制恢复没有发布唯一收尾收据，保留现场")
        recovery_receipt = binding(published.pop())
        guard(value, binding(run / "state.json"))
        # 恢复与追加阶段之间始终持有同一内核互斥，其他控制器不能插入。
        with claim_run_lock(run) as owner:
            sources = {**value["sources"], "state_before": binding(run / "state.json")}
            number = begin(run, LIFECYCLE_STAGE, "recover", sources)
            controller = bind_controller_attempt(run, number, owner)
            state = load_state(run)
            unsettled = [item["number"] for item in state["attempts"][:-1] if item["status"] != "passed"]
            result = _result(run, "recover", "controller_recovered", controller=controller,
                             recovery=recovery_receipt, next_operation="reconcile-evidence" if unsettled else "status")
            if recovered.get("status") != "controller_recovered":
                raise ValueError("控制恢复未返回已收尾状态")
            finish(run, number, result=result)
    return result


def _recover_external(value: dict, owner_path: Path) -> dict:
    run = value["run"]
    history = validate_history(run, value["state"])
    if (owner_path != run / "state.json" or binding(owner_path) != value["sources"]["state_before"]):
        raise ValueError("外部终止恢复必须明确绑定 status 返回的当前 state 文件")
    if (history["closed"] or history["external_recovery"] is not None or history["unsettled"]
            or not history["initial_complete"] or unknown_recoveries(run)):
        raise ValueError("当前服务账本不允许重复或跨阶段核对外部终止")
    services = registered_services(value)
    observed = observe_services(services)
    if observed["termination"] is None:
        raise ValueError("只有全部登记服务明确外部终止时才能执行该恢复")
    with process_guard(run, "run-control.guard"):
        guard(value)
        if controller_observation(run) is not None:
            raise ValueError("核对外部终止前出现其他控制器")
        if observe_services(services) != observed:
            raise ValueError("核对外部终止前服务状态变化")
        with claim_run_lock(run) as owner:
            sources = {**value["sources"], "external_owner": value["sources"]["state_before"]}
            number = begin(run, LIFECYCLE_STAGE, "recover", sources)
            controller = bind_controller_attempt(run, number, owner)
            running_state = binding(run / "state.json")
            try:
                output = local_path(value["backend"], str(run / f"lifecycle-{number:04d}"), new=True)
                output.mkdir()
                guard(value, running_state)
                if observe_services(services) != observed:
                    raise ValueError("发布外部终止证据前服务状态变化")
                evidence_file = output / "external-termination.json"
                write_json(evidence_file, {"format_version": 1, "kind": "reference-fixture-external-termination",
                    "run": str(run), "owner": value["sources"]["state_before"],
                    "observation": observed["termination"], "normal_shutdown_proof": None,
                    "remote_writes": 0, "resources_deleted": False})
                guard(value, running_state)
                if observe_services(services) != observed:
                    raise ValueError("保存外部终止证据后服务状态变化")
                result = _result(run, "recover", "external_termination_reconciled", controller=controller,
                                 owner=value["sources"]["state_before"], evidence=binding(evidence_file),
                                 next_operation="restart")
                finish(run, number, result=result)
            except BaseException as error:
                finish(run, number, error=error)
                raise
    return result


def _restart_requests(services: dict, output: Path) -> dict:
    rustfs = copy.deepcopy(services["requests"]["rustfs"])
    redis = copy.deepcopy(services["requests"]["redis"])
    previous = services["runtime"]
    redis.update(previous_identity={key: previous["linux_identity"][key]
                                    for key in ("pid", "started", "executable")},
                 previous_boot_id=previous["linux_identity"]["boot_id"],
                 previous_run_id=previous["redis"]["run_id"])
    for role, request in (("rustfs", rustfs), ("redis", redis)):
        directory = output / role
        directory.mkdir()
        write_json(directory / "request.json", request)
    return {"rustfs": rustfs, "redis": redis}


def _restart_cleanup(services: dict, private: dict, storage: dict | None,
                     runtime: dict | None, output: Path, original: BaseException) -> None:
    if not output.exists():
        return
    errors = []
    cleanup = output / "failed-start-cleanup"
    try:
        cleanup.mkdir(exist_ok=True)
    except BaseException as error:
        original.add_note("夹具服务重启失败后的回收目录无法创建：" + type(error).__name__)
        return
    if runtime is not None:
        try:
            stop_cache(services["requests"]["redis"], configured(private), runtime, cleanup)
        except BaseException as error:
            errors.append("Redis:" + type(error).__name__)
    if storage is not None:
        try:
            from full_stack_process_tree import read_process_tree

            tree = read_process_tree(output / "rustfs", "rustfs", services["requests"]["rustfs"]["scope_id"])
            terminate_owned_process_tree(tree, crash=True)
            completion_binding(tree)
        except BaseException as error:
            errors.append("RustFS:" + type(error).__name__)
    if errors:
        original.add_note("夹具服务重启失败后的精确回收未完成：" + ",".join(errors))


def restart(backend: Path, review: Path, bootstrap: Path, owner_file: Path, *, write: bool) -> dict:
    if not write:
        raise ValueError("重启夹具服务必须显式指定 --write")
    value = context(backend, review, bootstrap)
    run = value["run"]
    owner_path = local_path(backend, str(owner_file if owner_file.is_absolute() else backend / owner_file))
    history = validate_history(run, value["state"])
    if (owner_path != run / "state.json" or binding(owner_path) != value["sources"]["state_before"]):
        raise ValueError("服务重启必须明确绑定 status 返回的当前 state 文件")
    if (history["closed"] or history["external_recovery"] is None or history["unsettled"]
            or unknown_recoveries(run) or controller_observation(run) is not None):
        raise ValueError("服务重启必须紧接已完成且唯一的外部终止核对")
    previous = registered_services(value)
    stopped = observe_services(previous)
    if stopped["termination"] is None:
        raise ValueError("重启前原服务代次不是已核对的外部终止状态")
    with run_lock(run) as owner:
        guard(value)
        if observe_services(previous) != stopped:
            raise ValueError("重启前原服务状态变化")
        sources = {**value["sources"], "external_recovery": history["external_recovery"]}
        output = local_path(backend, str(run / f"lifecycle-{len(value['state']['attempts']) + 1:04d}"), new=True)
        number = begin(run, LIFECYCLE_STAGE, "restart", sources)
        if output != run / f"lifecycle-{number:04d}":
            raise ValueError("夹具服务重启代次编号与账本不一致")
        controller = bind_controller_attempt(run, number, owner)
        running_state = binding(run / "state.json")
        storage = runtime = None
        try:
            output.mkdir()
            requests = _restart_requests(previous, output)
            guard(value, running_state)
            if observe_services(previous) != stopped:
                raise ValueError("RustFS 重启前原服务状态变化")
            from devex_clone_storage_process import start as start_storage

            storage = start_storage(backend, requests["rustfs"], configured(value["private"]),
                                    output / "rustfs", value["sources"]["manifest"], controller,
                                    number, lambda: guard(value, running_state), supervised=True)
            guard(value, running_state)
            runtime = start_cache(requests["redis"], configured(value["private"]), output / "redis",
                                  lambda: guard(value, running_state))
            from full_stack_process_tree import read_process_tree

            tree = read_process_tree(output / "rustfs", "rustfs", requests["rustfs"]["scope_id"])
            current = {"requests": requests, "tree": tree, "runtime": runtime, "storage": storage,
                       "storage_observation": None, "origin": previous["origin"],
                       "evidence": {"rustfs_tree": storage["tree"],
                                    "redis_runtime": binding(output / "redis/runtime.json")}}
            expected = {"redis": "running", "rustfs": "running", "termination": None}
            if observe_services(current) != expected:
                raise ValueError("夹具服务新代次未同时就绪")
            generation_file = output / "restart.json"
            generation = {"format_version": 1, "kind": "reference-fixture-service-generation",
                          "run": str(run), "attempt": number, "owner": value["sources"]["state_before"],
                          "predecessor": history["external_recovery"],
                          "previous_generation": history["active_generation"], "controller": controller,
                          "services": {"redis": "running", "rustfs": "running"},
                          "rustfs": {"request": binding(output / "rustfs/request.json"), "storage": storage},
                          "redis": {"request": binding(output / "redis/request.json"), "runtime": runtime},
                          "remote_writes": 0, "resources_deleted": False}
            write_json(generation_file, generation)
            guard(value, running_state)
            if observe_services(current) != expected:
                raise ValueError("发布重启代次前服务状态变化")
            result = _result(run, "restart", "services_restarted", controller=controller,
                             services=generation["services"], generation=binding(generation_file),
                             previous_generation=history["active_generation"], next_operation="close")
            finish(run, number, result=result)
        except BaseException as error:
            active = {"requests": requests} if "requests" in locals() else previous
            _restart_cleanup(active, value["private"], storage, runtime, output, error)
            finish(run, number, error=error)
            raise
    return result
