"""在原服务账本中显式关闭服务或恢复死亡控制器，未知结果不自动重放。"""
from __future__ import annotations

from pathlib import Path
import time

from devex_clone_cache_process import stop as stop_cache
from devex_clone_capture import read_json, write_json
from devex_clone_factory_context import configured
from devex_clone_model import local_path
from devex_clone_run_state import (begin, bind_controller_attempt, binding, claim_run_lock,
                                   controller_observation, finish, load_state, recover_run_lock, run_lock)
from full_stack_process_tree import terminate_owned_process_tree
from full_stack_process import process_identity
from process_guard import process_guard
from reference_fixture_service_context import (context, guard, observe_services, registered_services,
                                               unknown_recoveries)
from reference_fixture_service_history import LIFECYCLE_STAGE, validate_history


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
            actual = process_identity(expected["pid"])
            if actual is not None and actual != expected:
                raise ValueError("关闭等待期间 RustFS 进程 PID 已复用")
            pending |= actual is not None
        if not pending:
            observed = observe_services(services)
            if observed != {"redis": "stopped", "rustfs": "stopped"}:
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
    observe_services(services)
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
            _stable_services(value, services, running_state)
            storage_file = output / "rustfs-stopped.json"
            write_json(storage_file, {"tree": services["tree"], "terminated": terminated,
                                      "state": "stopped", "process_tree_confirmed": True})
            result = _result(run, "close", "services_closed", services=observations, controller=controller,
                             evidence={"redis": binding(output / "stopped.json"), "rustfs": binding(storage_file)})
            if cache.get("status") != "redis_process_stopped":
                raise ValueError("Redis 关闭未返回明确结果")
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
    owner_binding = read_json(local_path(backend, str(owner_file if owner_file.is_absolute() else backend / owner_file)))
    observed = controller_observation(run)
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
