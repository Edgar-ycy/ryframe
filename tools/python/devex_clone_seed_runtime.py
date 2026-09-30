"""身份 CLI 及新 seed API/Worker 的窄交接；不改变身份业务、配额或正常任务状态。"""
import json
from pathlib import Path
from types import SimpleNamespace

from devex_clone_capture import read_json, write_json
from process_environment import Environments
from devex_clone_post import registration as post_registration, target_lock, require_schedule_stage
from devex_clone_post_actions import verify_confirmations
from devex_clone_post_context import Context, absent
from devex_clone_post_model import exact_directory
from devex_clone_post_process import Producer, require_quiet, recover_session, cleanup_failure, inspect_producer
from devex_clone_run_state import binding, load_state
from devex_clone_seed import (registered, register, history, identity_inputs, identity_evidence,
                              verified_identity_stage, latest)
from devex_clone_quota_model import capacity_evidence
from devex_clone_source_proof import bound_file, require_closed_port, verify_api_address
from devex_clone_target_resources import Resources
from full_stack_process import process_identity, read_process
from full_stack_runtime import register_runtime, verify_runtime, worker_ready_url
from restore_source_binding import source_binding


def runtime_inputs(backend, directory):
    """清理仅绑定已发布的新运行登记及环境，不要求后来工具和业务证据仍有效。"""
    request = registered(backend, directory, cleanup=True)
    post_active = post_registration(backend, directory, descriptor=request["post_copy"], cleanup=True)
    post = post_active.request
    runtime = directory / "seed-runtime/runtime"
    exact_directory(runtime)
    handoff = read_json(runtime / "handoff.json")
    expected = {"registration": binding(directory / "seed-runtime.json"),
                "runtime": binding(runtime / "runtime.json"), "binaries": binding(runtime / "binaries.json")}
    if any(handoff.get(key) != item for key, item in expected.items()):
        raise ValueError("seed runtime 文件不同于固定交接")
    record = latest(directory, "seed-runtime", {"prepare"}, cleanup=True)
    result = read_json(bound_file(backend, record["result"]))
    if result.get("status") != "seed_runtime_prepared" or result.get("handoff") != binding(runtime / "handoff.json"):
        raise ValueError("seed runtime 尚未通过外层准备收尾")
    private = read_json(bound_file(backend, post["api_environment"]))["environment"]
    return runtime, handoff, private


def resources_guard(context):
    """当前物理身份和所有桶 ownership；允许已授权身份和 outbox 数据正常演进。"""
    context.bindings()
    with context.environments.use("target"):
        if source_binding(context.backend, {"source": context.target_config}) != context.initial["generation"]["physical"]:
            raise ValueError("seed 物理目标不属于原初始化资源")
        verify_api_address(context.backend, context.selected["api_url"])
        if worker_ready_url() != context.selected["worker_ready_url"]:
            raise ValueError("seed Worker 地址变化")
        absent(Path(context.selected["runtime_dir"]))
        absent(context.runtime / "worker.json")
        resources = Resources(context.backend, context.target, context.output, context.selected, context.review, context.run,
                              storage_run=context.directory_root)
        resources.storage_identity()
        resources.tools.verify_databases("target")
        resources.tools.verify_objects("target")
    return resources


def schedules_guard(context):
    confirmations = verify_confirmations(context)
    require_schedule_stage(context, confirmations)
    with context.environments.use("target"):
        resources = Resources(context.backend, context.target, context.output, context.selected, context.review, context.run,
                              storage_run=context.directory_root)
        database = next(item for item in context.target_config["databases"] if item["key"] == "shared-control")
        raw = resources.tools.mysql(database, "SELECT COUNT(*) FROM sys_job_schedule WHERE enabled=1 AND del_flag='0';")
        if raw != "0":
            raise ValueError("seed 仍有启用或未明确核清的调度，禁止 Worker")
    return confirmations


def old_api_stopped(context, request):
    bound_file(context.backend, request["api_process"])
    identity = read_process(context.runtime, "api", context.target_config["scope_id"])
    if process_identity(identity["pid"]) is not None:
        raise ValueError("原 API 尚未停止或 PID 已复用，不能启动新运行对")


def current_guard(context, request, *, old_api=False):
    if registered(context.backend, context.directory_root) != request:
        raise ValueError("seed 登记发生变化")
    post = history(context.backend, context.directory_root, request)
    identity_inputs(context.backend, context.directory_root, request, post)
    resources = resources_guard(context)
    if old_api:
        with context.environments.use("target"):
            context.api_guard(resources)
            require_closed_port(context.selected["worker_ready_url"])
    return schedules_guard(context)


def current_mutation_guard(context, request, prerequisite, confirmations):
    """完整阶段守卫之间的写入临界区核验；不重复遍历全部库、桶和源码。"""
    from devex_clone_department_model import require_prerequisite

    if registered(context.backend, context.directory_root) != request:
        raise ValueError("seed 登记发生变化")
    post = history(context.backend, context.directory_root, request)
    identity_inputs(context.backend, context.directory_root, request, post)
    require_prerequisite(context.backend, context.directory_root, request, prerequisite)
    require_schedule_stage(context, confirmations)
    with context.environments.use("target"):
        if source_binding(context.backend, {"source": context.target_config}) != context.initial["generation"]["physical"]:
            raise ValueError("seed 写入目标不属于原初始化资源")
        verify_api_address(context.backend, context.selected["api_url"])
        if worker_ready_url() != context.selected["worker_ready_url"]:
            raise ValueError("seed Worker 地址变化")
        absent(Path(context.selected["runtime_dir"]))
        absent(context.runtime / "worker.json")
        resources = Resources(context.backend, context.target, context.output, context.selected, context.review,
                              context.run, storage_run=context.directory_root)
        context.api_guard(resources)
        require_closed_port(context.selected["worker_ready_url"])
        database = next(item for item in context.target_config["databases"] if item["key"] == "shared-control")
        if resources.tools.mysql(
                database, "SELECT COUNT(*) FROM sys_job_schedule WHERE enabled=1 AND del_flag='0';") != "0":
            raise ValueError("seed 写入期间出现启用调度，拒绝继续")
    return confirmations


def identity_run(context, request, mode):
    from devex_clone_department_model import authorize_identity, identity_projection

    if mode not in {"apply", "verify"}:
        raise ValueError("未知身份阶段")
    departments = (authorize_identity(context.backend, context.directory_root, request, "verify")
                   if mode == "verify" else None)
    capacity = capacity_evidence(context.backend, context.directory_root, request)
    current_guard(context, request, old_api=True)
    _, private = identity_inputs(context.backend, context.directory_root, request, context.request)
    producer_context = SimpleNamespace(backend=context.backend, directory_root=context.directory_root,
        output=context.output, request=request, request_binding=binding(context.directory_root / "seed-runtime.json"),
        private={"target_api": private})
    producer = Producer(producer_context, "identity-" + mode)
    try:
        stdout = producer.communicate(timeout=7200)
    except BaseException as original:
        try:
            producer.close()
        except Exception as cleanup_error:
            cleanup_failure(context, "identity-cleanup", cleanup_error, original)
        raise
    else:
        producer.close()
    result = json.loads(stdout)
    plan = read_json(bound_file(context.backend, request["identity_plan"]))
    expected = ({"status": "prepared", "plan_sha256": plan["plan_sha256"], "entries": 622,
                 "roles": 11, "users": 200} if mode == "apply" else
                {"status": "verified", "plan_sha256": plan["plan_sha256"], "users": 200,
                 "tenants": 10, "message_audience": 10})
    if result != expected:
        raise ValueError("身份 CLI 结果不属于完整计划")
    evidence = (identity_projection(context.backend, request, allow_absent=False)
                if mode == "apply" else identity_evidence(context.backend, context.directory_root, request))
    if mode == "apply" and evidence["state"] != "prepared":
        raise ValueError("身份 apply 未形成完整 prepared 账本")
    current_guard(context, request, old_api=True)
    final_evidence = (identity_projection(context.backend, request, allow_absent=False)
                      if mode == "apply" else identity_evidence(
                          context.backend, context.directory_root, request))
    if final_evidence != evidence:
        raise ValueError("身份阶段收尾期间账本证据变化")
    if mode == "verify" and authorize_identity(
            context.backend, context.directory_root, request, "verify") != departments:
        raise ValueError("身份 verify 期间部门验证证据变化")
    if capacity_evidence(context.backend, context.directory_root, request) != capacity:
        raise ValueError("身份准备期间已发布容量证明变化")
    observed = inspect_producer(context.backend, context.directory_root, load_state(context.directory_root)["attempts"][-1])
    if observed is None or observed["alive"]:
        raise ValueError("身份生产者尚未完整退出")
    write_json(context.output / "identity-result.json", result)
    response = {"status": "seed_identities_verified" if mode == "verify" else "seed_identities_prepared",
            "registration": producer_context.request_binding, "identity": evidence, "producer": observed["receipt"],
            "capacity": capacity,
            "worker_authorized": False, "restore_qualified": False}
    if mode == "verify":
        response["departments"] = departments
    return response


def prepare(context, request):
    proof = verified_identity_stage(context.backend, context.directory_root, request)
    capacity = capacity_evidence(context.backend, context.directory_root, request)
    confirmations = current_guard(context, request)
    with context.environments.use("target"):
        require_closed_port(context.selected["worker_ready_url"])
        old = verify_runtime(context.backend, context.runtime)
    runtime = context.directory_root / "seed-runtime/runtime"
    exact_directory(runtime)
    if runtime.exists() and ({item.name for item in runtime.iterdir()} - {"binaries.json", "runtime.json", "handoff.json"}):
        raise ValueError("seed runtime 已有生产者历史，不能重写准备收据")
    runtime.mkdir(exist_ok=True)
    binaries = read_json(context.runtime / "binaries.json")
    filename = runtime / "binaries.json"
    if filename.exists() and read_json(filename) != binaries:
        raise ValueError("seed 产物与原已审计产物不同")
    if not filename.exists():
        write_json(filename, binaries)
    with context.environments.use("target"):
        if register_runtime(context.backend, runtime) != old:
            raise ValueError("seed 配置、scope 或产品产物变化")
    handoff = {"registration": binding(context.directory_root / "seed-runtime.json"), "identity": proof, "capacity": capacity,
        "schedules": confirmations, "api_environment": context.request["api_environment"],
        "backend_build": context.request["backend_build"], "original_api": request["api_process"],
        "runtime": binding(runtime / "runtime.json"), "binaries": binding(filename),
        "api_url": context.selected["api_url"], "scope_id": context.target_config["scope_id"]}
    filename = runtime / "handoff.json"
    if filename.exists() and read_json(filename) != handoff:
        raise ValueError("已登记 seed 交接不同，不能覆盖历史")
    if not filename.exists():
        write_json(filename, handoff)
    current_guard(context, request)
    if verified_identity_stage(context.backend, context.directory_root, request) != proof:
        raise ValueError("准备期间身份发布证据变化")
    if capacity_evidence(context.backend, context.directory_root, request) != capacity:
        raise ValueError("准备期间已发布容量证明变化")
    return {"status": "seed_runtime_prepared", "handoff": binding(filename), "remote_writes": 0,
            "outbox_drained": False, "restore_qualified": False}


def start(context, request, number):
    from devex_clone_runtime import control

    directory = context.directory_root
    prior = [item for item in load_state(directory)["attempts"] if item["number"] != number
             and item["mode"] not in {"stop", "status", "recover", "recover-session"}]
    runtime, handoff, private = runtime_inputs(context.backend, directory)
    if not prior:
        raise ValueError("最新阶段未通过，不能启动 Worker")
    if prior[-1]["status"] != "passed":
        restart_after_stop(context.backend, directory, runtime, handoff, prior[-1], number)
    proof = verified_identity_stage(context.backend, directory, request)
    if proof != handoff["identity"]:
        raise ValueError("seed 交接不是最新明确身份验证结果")
    if capacity_evidence(context.backend, directory, request) != handoff["capacity"]:
        raise ValueError("seed 交接的容量证明发生变化")
    if current_guard(context, request) != handoff["schedules"]:
        raise ValueError("seed 调度确认变化")
    old_api_stopped(context, request)
    with Environments(private, private).use("target"):
        result = control(context.backend, runtime, "start", ("api", "worker"), handoff["api_url"])
    current_guard(context, request)
    if verified_identity_stage(context.backend, directory, request) != proof:
        raise ValueError("启动期间身份发布证据变化")
    return {"status": "seed_runtime_started", "runtime": result, "handoff": binding(runtime / "handoff.json"),
            "outbox_drained": False, "restore_qualified": False}


def stopped_evidence(runtime, handoff):
    """仅为重新启动提供已闭合生产者历史；不能据此宣称业务任务已排空。"""
    path = runtime / "producer-history.json"
    history = read_json(path)
    if (history.get("kind") != "devex-clone-producer-history" or history.get("scope_id") != handoff["scope_id"]
            or history.get("runtime_directory") != str(runtime) or history.get("format_version") != 1):
        raise ValueError("生产者历史不属于固定 seed runtime")
    events = history["events"]
    if [item["sequence"] for item in events] != list(range(len(events))):
        raise ValueError("生产者历史不连续")
    intents = [item for item in events if item["event"] == "start-intent"]
    if len({item["token"] for item in intents}) != len(intents):
        raise ValueError("生产者启动意图重复")
    for intent in intents:
        completed = [item for item in events if item.get("token") == intent["token"]
                     and item["event"] in {"started", "start-failed"} and item["role"] == intent["role"]
                     and item["sequence"] > intent["sequence"]]
        if not completed:
            raise ValueError("启动意图缺少实际身份或失败收尾，需核实未知进程")
        for item in completed:
            if item["event"] != "started":
                continue
            recorded = item["identity"]
            if (not isinstance(recorded, dict) or set(recorded) != {"pid", "started", "executable"}
                    or type(recorded["pid"]) is not int or recorded["pid"] <= 1
                    or not isinstance(recorded["started"], str) or not recorded["started"].isdigit()
                    or not isinstance(recorded["executable"], str) or not recorded["executable"]):
                raise ValueError("历史生产者身份无效")
            current = process_identity(recorded["pid"])
            if current is None:
                continue
            if (not isinstance(current, dict) or set(current) != {"pid", "started", "executable"}
                    or type(current["pid"]) is not int or current["pid"] != recorded["pid"]
                    or not isinstance(current["started"], str) or not current["started"].isdigit()
                    or not isinstance(current["executable"], str) or not current["executable"]
                    or int(current["started"]) <= int(recorded["started"])):
                raise ValueError("历史生产者仍存在或 PID 代次无法证明更新")
    return {"history": binding(path), "processes": {role: binding(runtime / (role + ".json"))
            if (runtime / (role + ".json")).exists() else None for role in ("api", "worker")}}


def restart_after_stop(backend, directory, runtime, handoff, failed, number):
    if failed["stage"] != "seed-runtime" or failed["mode"] != "start":
        raise ValueError("仅明确停止可解除失败 start 的运行状态，其余失败仍需对应核验")
    stopped = latest(directory, "seed-runtime", {"stop"}, current=number)
    result = read_json(bound_file(backend, stopped["result"]))
    if (stopped["number"] <= failed["number"] or result.get("restart_ready") is not True
            or result.get("handoff") != binding(runtime / "handoff.json")
            or result.get("stopped_evidence") != stopped_evidence(runtime, handoff)
            or result["runtime"]["operation"] != "stop"
            or result["runtime"]["runtime_directory"] != str(runtime)
            or result["runtime"]["scope_id"] != handoff["scope_id"]
            or result["runtime"]["processes"] != {role: {"state": "stopped", "identity": None, "ready": False}
                                                   for role in ("api", "worker")}):
        raise ValueError("缺少失败之后同一 runtime 的精确双角色停止证明")


def execute_seed(backend, directory, value, mode, number, request_file=None, producer_binding=None):
    from devex_clone_runtime import control, observe, reconcile_lock

    if mode == "recover-session":
        if producer_binding is None:
            raise ValueError("身份 Node 回收需要明确收据")
        return recover_session(backend, directory, number, producer_binding)
    if mode in {"stop", "status", "recover"}:
        runtime, handoff, private = runtime_inputs(backend, directory)
        with Environments(private, private).use("target"):
            result = (reconcile_lock(runtime) if mode == "recover"
                      else observe(backend, runtime, ("api", "worker"), handoff["api_url"])
                      if mode == "status"
                      else control(backend, runtime, mode, ("api", "worker"), handoff["api_url"]))
        observed = {"status": "seed_runtime_" + mode, "runtime": result, "restore_qualified": False}
        if mode == "stop":
            observed["handoff"] = binding(runtime / "handoff.json")
            try:
                observed["stopped_evidence"] = stopped_evidence(runtime, handoff)
                observed["restart_ready"] = True
            except (ValueError, OSError, KeyError, TypeError) as error:
                observed.update(restart_ready=False, stopped_evidence_error_type=type(error).__name__)
        return observed
    if mode in {"source-generation-start", "source-generation-stop", "source-generation-recover"}:
        if request_file is None:
            raise ValueError("source-generation 需要明确 --request")
        from devex_clone_seed_generation import execute_generation
        from devex_clone_seed_generation_control import execute_stop, execute_recover

        operation = {"source-generation-start": execute_generation, "source-generation-stop": execute_stop,
                     "source-generation-recover": execute_recover}[mode]
        return operation(backend, directory, request_file, number)
    if mode == "source-generation-reboot-close":
        if request_file is None:
            raise ValueError("主机重启收尾需要明确 --request")
        from devex_clone_seed_generation_reboot import execute_close

        return execute_close(backend, directory, request_file, number)
    require_quiet(backend, directory, number)
    if mode in {"source-export", "source-export-reconcile"}:
        from devex_clone_seed_export import execute_export

        return execute_export(backend, directory, number, reconcile=mode == "source-export-reconcile")
    if mode == "source-rebind":
        if request_file is None:
            raise ValueError("source-rebind 需要明确 successor --request")
        from devex_clone_seed_rebind import register_rebind

        return register_rebind(backend, directory, request_file, number)
    if mode == "arm-input":
        if request_file is None:
            raise ValueError("arm-input 需要明确 --request")
        from devex_clone_seed_arm import publish_arm_input

        return publish_arm_input(backend, directory, request_file, number)
    with target_lock(backend, value):
        if mode == "register":
            if request_file is None:
                raise ValueError("seed 登记需要明确 --request")
            return register(backend, directory, request_file)
        if mode == "source-register":
            from devex_clone_seed_source import register_source

            return register_source(backend, directory, value, number)
        request = registered(backend, directory)
        context = Context(backend, directory, value, number, stage="seed-runtime")
        if mode in {"quotas-plan", "quotas-apply", "quotas-reconcile"}:
            from devex_clone_quota import execute_quotas

            return execute_quotas(context, request, mode)
        if mode in {"departments-plan", "departments-apply", "departments-reconcile", "departments-verify"}:
            from devex_clone_department import execute_departments

            return execute_departments(context, request, mode)
        if mode in {"identities-apply", "identities-verify"}:
            return identity_run(context, request, mode.removeprefix("identities-"))
        if mode == "prepare":
            return prepare(context, request)
        if mode == "start":
            return start(context, request, number)
        if mode == "close":
            from devex_clone_seed_close import execute_close

            return execute_close(context, request, number)
        raise ValueError("未知 seed 运行阶段")
