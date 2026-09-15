"""同一源运行代次的停止、状态与精确树回收；不启动第二个代次。"""
from __future__ import annotations

from pathlib import Path
import subprocess

from devex_clone_capture import read_json, write_json
from devex_clone_model import exact, local_path, name
from devex_clone_run_state import binding, load_state
from devex_clone_seed_generation import START, STOP, RECOVER, REQUEST_FIELDS, predecessor, source_request, verify_running_source
from devex_clone_seed_generation_images import capture_image, verify_image
from devex_clone_seed_generation_runtime import GenerationRuntime, generation_directory, ROLES, EXTRA_FILES
from devex_clone_source_proof import bound_file
from full_stack_process import process_identity
from full_stack_process_monitor import completion_binding
from full_stack_process_tree import read_process_tree, validate_process_tree_directory
from restore_reference_plan import plan_hash


def preflight(directory: Path, mode: str, *, backend: Path | None = None, request_path: Path | None = None) -> None:
    from devex_clone_seed_generation_prelaunch import starts, prepare, origin

    state = load_state(directory)
    backend = backend or Path(__file__).resolve().parents[1]
    records = starts(backend, directory, state["attempts"])
    if len(records) != 1 or mode not in {STOP, RECOVER}:
        raise ValueError("源代次停止或回收必须继承唯一已登记 start")
    prior = [row for row in state["attempts"] if row["number"] > records[0]["number"]
             and row["stage"] == "seed-runtime" and row["mode"] in {STOP, RECOVER}]
    if mode == STOP and (records[0]["status"] != "passed" or prior):
        raise ValueError("源代次 stop 只能执行一次；失败只能精确回收，禁止重放")
    if mode == RECOVER and not (directory / f"g{records[0]['number']:04d}").exists():
        if request_path is None:
            raise ValueError("未启动恢复必须显式绑定当前请求及最后失败阶段")
        start, _ = origin(backend, directory, state["attempts"])
        prepare(backend, directory, start, request_path, state["attempts"][-1]["number"] + 1)
    elif mode == RECOVER and any(row["mode"] == RECOVER or row["status"] == "passed" for row in prior):
        raise ValueError("源代次已停止或执行回收，不能重放")


def _active(directory: Path, number: int, mode: str) -> list:
    from devex_clone_run import _require_owned_run

    _require_owned_run(directory)
    state = load_state(directory)
    if not state["attempts"] or tuple(state["attempts"][-1][key] for key in ("number", "stage", "mode", "status")) != (
            number, "seed-runtime", mode, "running"):
        raise ValueError("源代次控制不属于同一账本当前持锁阶段")
    return state["attempts"][:-1]


def existing_runtime(backend: Path, facts: dict, run) -> GenerationRuntime:
    runtime = GenerationRuntime(backend, facts["directory"], facts["output"], facts["request"], facts["source"], run)
    intent = read_json(facts["output"] / "intent.json")
    runtime.operations = intent["operations"]
    runtime.contract = facts["runtime"]
    runtime.ready = dict.fromkeys(ROLES, True)
    runtime.urls = {"api": runtime.selected["api_url"].rstrip("/") + "/readyz",
                    "worker": runtime.contract["worker_ready_url"]}
    return runtime


def execute_stop(backend: Path, directory: Path, receipt_path: Path, number: int, *, run=subprocess.run) -> dict:
    from restore_source_runtime import verify_source_runtime
    from devex_clone_source_proof import verify_generation

    prefix = _active(directory, number, STOP)
    source_runtime = binding(local_path(backend, str(receipt_path)))
    verified = verify_source_runtime(backend, source_runtime, live=True)
    receipt = verified["receipt"]
    facts = verify_running_source(backend, receipt["source_generation"], live=True)
    if facts["directory"] != directory:
        raise ValueError("source-runtime 不属于当前账本与同一代次")
    output, request, source = facts["output"], facts["request"], facts["source"]
    runtime = existing_runtime(backend, facts, run)

    def checkpoint():
        if _active(directory, number, STOP) != prefix or binding(receipt_path) != source_runtime:
            raise ValueError("源停止期间账本或 source-runtime 发生变化")
        runtime.checkpoint()

    with runtime.environment() as environment:
        failure, stop_before = None, None
        try:
            checkpoint()
            stop_before = capture_image(backend, runtime.execution, runtime.selected, request, source,
                                        environment, output / "stop-before", run, control_environment=runtime.control_environment)
            if read_json(Path(stop_before["path"]))["image"] != verified["after"]["image"]:
                raise ValueError("source verify 后出现未披露写入；停止并保留现场，禁止发布或重放")
            checkpoint()
            with runtime.control_environment():
                if verify_source_runtime(backend, source_runtime, live=True) != verified:
                    raise ValueError("停止前 source-runtime 验证事实变化")
        except BaseException as error:
            failure = error
        try:
            runtime.stop()
            checkpoint()
            after = capture_image(backend, runtime.execution, runtime.selected, request, source,
                                  environment, output / "after", run, control_environment=runtime.control_environment)
            if stop_before is None or read_json(Path(stop_before["path"]))["image"] != read_json(Path(after["path"]))["image"]:
                raise ValueError("停止前后完整数据库、对象或 Redis 像变化，禁止发布或重放")
        except BaseException as error:
            if failure is not None:
                failure.add_note("停止或完整后像失败：" + type(error).__name__)
                raise failure from error
            raise
        if failure is not None:
            raise failure
        with runtime.control_environment():
            verified_after = verify_source_runtime(backend, source_runtime, live=False)
        if verified_after != verified:
            raise ValueError("停止后 source-runtime 静态证明与停止前不同")
        runtime.finish()
        effective = source_request(source, request, runtime.runtime)
        write_json(output / "source-request.json", effective)
        write_json(output / "generation-verified.json", verify_generation(
            backend, effective, run, control_environment=runtime.control_environment))
        checkpoint()
    return {"status": "seed_source_generation_published", "request": facts["receipt"]["request"],
            **{key: request[key] for key in ("source_registration", "source_rebind", "review_successor", "current_storage")},
            "history_length": len(prefix), "history_sha256": plan_hash(prefix),
            "start": receipt["source_generation"], "source_runtime": source_runtime,
            "dataset_lineage": facts["receipt"]["dataset_lineage"],
            "source_request": binding(output / "source-request.json"),
            "generation_verified": binding(output / "generation-verified.json"),
            "runtime_evidence": binding(output / "runtime-evidence.json"),
            "before": facts["receipt"]["before"], "running": facts["receipt"]["running"],
            "stop_before": stop_before, "after": after, "remote_writes": 0, "restore_qualified": False}


def status(backend: Path, directory: Path) -> dict:
    """只读检查明确 start 的意图和已发布树；缺失树不推断已停止。"""
    from devex_clone_seed_source import _registered_source
    from reference_fixture_successor import _source_with_loader

    initial = binding(directory / "state.json")
    state = load_state(directory)
    from devex_clone_seed_generation_prelaunch import starts, closed

    records = starts(backend, directory, state["attempts"])
    if not records:
        archive = closed(backend, directory, state["attempts"])
        if archive is not None:
            roles = dict.fromkeys(ROLES, {"state": "not_started", "tree": None, "completion": None})
            return _status_result(directory, initial, archive["start"], None, roles)
    if len(records) != 1:
        raise ValueError("source-generation status 需要唯一 start 记录")
    start = records[0]
    output = directory / f"g{start['number']:04d}"
    unknown = dict.fromkeys(ROLES, {"state": "unknown", "tree": None, "completion": None})
    if not output.exists():
        local_path(backend, str(output), new=True)
        return _status_result(directory, initial, start, None, unknown)
    generation_directory(output)
    if not (output / "intent.json").exists():
        return _status_result(directory, initial, start, None, unknown)
    intent = read_json(output / "intent.json")
    exact(intent, {"format_version", "kind", "request", "runtime_directory", "operations"})
    request = read_json(bound_file(backend, intent["request"]))
    exact(request, REQUEST_FIELDS)
    if type(request["format_version"]) is not int or request["format_version"] != 1 or request["kind"] != "devex-clone-seed-source-generation":
        raise ValueError("源代次状态必须读取完整正式请求")
    name(request["id"])
    runtime = output / "runtime"
    if (type(intent["format_version"]) is not int or intent["format_version"] != 1 or intent["kind"] != "seed-source-runtime-intent"
            or intent["request"] != binding(output / "request.json") or intent["runtime_directory"] != str(runtime)
            or set(intent["operations"]) != set(ROLES)):
        raise ValueError("源代次意图与当前固定目录不同")
    validate_process_tree_directory(runtime, intent["operations"], extra_files=EXTRA_FILES)
    # 只从启动所绑定的 C52 原始 source request 推导 scope，不接受状态命令提供的替代目标。
    source = _source_with_loader(backend, request["review_successor"], live_storage=False, loader=_registered_source)
    if source["directory"] != directory or source["review_successor"]["source_result"] != request["source_registration"]:
        raise ValueError("源代次状态请求与原 C52、successor 或账本不同")
    scope = source["request"]["source"]["scope_id"]
    roles = {}
    for role in ROLES:
        if not (runtime / f"{role}-tree.json").exists():
            roles[role] = {"state": "unknown", "tree": None, "completion": None}
            continue
        tree = read_process_tree(runtime, role, scope)
        if tree["operation_id"] != intent["operations"][role]:
            raise ValueError("源代次树不属于启动意图")
        identities = {key: process_identity(tree[key]["pid"]) for key in ("process", "supervisor", "monitor")}
        if any(item is not None and item != tree[key] for key, item in identities.items()):
            raise ValueError("源代次存在 PID 复用或创建身份变化")
        completion = None
        if all(item is None for item in identities.values()):
            completion = completion_binding(tree)
        observed = "stopped" if completion else "running" if all(identities.values()) else "degraded"
        roles[role] = {"state": observed, "tree": binding(runtime / f"{role}-tree.json"),
                       "completion": completion}
    return _status_result(directory, initial, start, binding(output / "intent.json"), roles)


def _status_result(directory: Path, initial: dict, start: dict, intent: dict | None, roles: dict) -> dict:
    if binding(directory / "state.json") != initial:
        raise ValueError("源代次账本在状态观察期间变化")
    return {"status": "source_generation_observed", "attempt": start["number"], "start_status": start["status"],
            "intent": intent, "roles": roles, "remote_writes": 0, "restore_qualified": False}


def execute_recover(backend: Path, directory: Path, request_path: Path, number: int, *, run=subprocess.run) -> dict:
    prefix = _active(directory, number, RECOVER)
    observed = status(backend, directory)
    if "attempt" in observed and not (directory / f"g{observed['attempt']:04d}").exists():
        from devex_clone_seed_generation_prelaunch import execute

        return execute(backend, directory, request_path, number, prefix, run=run)
    if any(row["state"] == "unknown" for row in observed["roles"].values()):
        raise ValueError("启动在完整树发布前中断，缺少完整归属，不能推断停止或自动回收；禁止重放")
    verifier = _verifier_stopped(backend, directory, prefix)
    request_descriptor = binding(local_path(backend, str(request_path)))
    request = read_json(request_path)
    exact(request, REQUEST_FIELDS)
    output = directory / f"g{observed['attempt']:04d}"
    if read_json(output / "request.json") != request:
        raise ValueError("回收必须显式绑定原 source-generation 请求")
    source = predecessor(backend, directory, load_state(directory))
    for field, expected in {"source_registration": source["review_successor"]["source_result"],
                            "source_rebind": source["source_rebind"], "review_successor": source["review_successor_binding"],
                            "current_storage": source["storage"]["storage"]}.items():
        if request[field] != expected:
            raise ValueError("回收请求改变了原 C52、存储或 successor 绑定")
    runtime = existing_runtime(backend, {"directory": directory, "output": output, "request": request, "source": source,
                                "runtime": read_json(output / "runtime/runtime.json")}, run)
    with runtime.environment() as environment:
        failure, before, baseline = None, None, None
        try:
            before = capture_image(backend, runtime.execution, runtime.selected, request, source,
                                   environment, output / "recover-before", run, control_environment=runtime.control_environment)
            with runtime.control_environment():
                baseline, original = _recovery_baseline(backend, output, prefix, verifier, source, runtime.selected)
            if read_json(Path(before["path"]))["image"] != original["image"]:
                raise ValueError("START 运行像或已验证会话后像之后存在未知写入；仅回收树，禁止声明零漂移或重放")
        except BaseException as error:
            failure = error
        if _active(directory, number, RECOVER) != prefix or binding(request_path) != request_descriptor:
            raise ValueError("回收前控制阶段或请求发生变化")
        with runtime.control_environment():
            if _verifier_stopped(backend, directory, prefix) != verifier:
                raise ValueError("回收前来源验收生产者退出事实变化")
        try:
            runtime.stop()
            after = capture_image(backend, runtime.execution, runtime.selected, request, source,
                                  environment, output / "recover-after", run, control_environment=runtime.control_environment)
            if before is None or read_json(Path(before["path"]))["image"] != read_json(Path(after["path"]))["image"]:
                raise ValueError("源回收前后完整像变化，保留现场并禁止发布或重放")
        except BaseException as error:
            if failure is not None:
                failure.add_note("精确树回收或完整后像失败：" + type(error).__name__)
                raise failure from error
            raise
        if failure is not None:
            raise failure
        stopped = status(backend, directory)
        if any(row["state"] != "stopped" for row in stopped["roles"].values()):
            raise ValueError("源回收后缺少完整双角色退出证明")
        with runtime.control_environment():
            if _verifier_stopped(backend, directory, prefix) != verifier:
                raise ValueError("回收后来源验收生产者退出事实变化")
        bound_file(backend, baseline)
    return {"status": "seed_source_generation_abandoned", "request": request_descriptor, "intent": observed["intent"],
            "baseline": baseline, "source_verifier": verifier,
            "before": before, "after": after, "runtime": stopped, "remote_writes": 0,
            "source_generation_published": False, "replay_allowed": False, "restore_qualified": False}


def _recovery_baseline(backend: Path, output: Path, prefix: list, verifier: dict, source: dict, selected: dict):
    """已完成的独立会话验收才允许替代 START 运行像；失败会话不获得写入白名单。"""
    from devex_clone_seed_generation_prelaunch import starts

    records = starts(backend, output.parent, prefix)
    if len(records) != 1:
        raise ValueError("回收必须绑定唯一实际启动")
    start = records[0]
    if start["status"] != "passed":
        raise ValueError("未发布 START 的失败代次缺少不可变运行前像；只回收树，不能声明零漂移")
    receipt = read_json(bound_file(backend, start["result"]))
    baseline = receipt["running"]
    if verifier["status"] == "verified_stopped":
        from restore_source_runtime import verify_source_runtime

        verified = verify_source_runtime(backend, binding(output / "verification/source-runtime.json"), live=False)
        if verified["receipt"]["source_generation"] != start["result"]:
            raise ValueError("回收会话后像没有绑定同一 START")
        baseline = verified["receipt"]["after"]
    value = verify_image(backend, baseline, selected, source["request"],
                         source_registration=receipt["source_registration"])
    return baseline, value


def _verifier_stopped(backend: Path, directory: Path, prefix: list[dict]) -> dict:
    """来源登录生产者由唯一 validator 证明已退出；失败启动没有合法验收入口。"""
    from restore_source_runtime_producer import require_source_verifier_stopped

    from devex_clone_seed_generation_prelaunch import starts

    records = starts(backend, directory, prefix)
    if len(records) != 1:
        raise ValueError("回收必须绑定唯一来源启动")
    start = records[0]
    if start["status"] == "passed":
        return require_source_verifier_stopped(backend, start["result"])
    output = directory / f"g{start['number']:04d}"
    generation_directory(output)
    if (output / "verification").exists():
        raise ValueError("未成功发布的来源启动存在未知验收生产者证据")
    return {"status": "not_started", "process": None}
