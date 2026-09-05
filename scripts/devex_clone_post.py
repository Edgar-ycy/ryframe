"""同一复制 run 的固定后续登记：API 准备、调度核对和既有数据读取。"""
from contextlib import contextmanager, nullcontext
from pathlib import Path

from devex_clone_capture import read_json, write_json
from devex_clone_copy_locks import GUARDS
from devex_clone_model import exact, linked, local_path
from devex_clone_post_model import administrator, environment_delta, exact_directory
from devex_clone_run_state import binding, load_state
from devex_clone_source_proof import bound_file, require_closed_port
from devex_clone_target_binding import external_file
from process_guard import process_guard
from restore_reference_plan import plan_hash
from devex_clone_post_registration import resolve_post_registration

FIELDS = {"format_version", "kind", "run_manifest", "copy_stage_receipt", "copy_result", "ledger_head",
          "api_environment", "backend_build", "node", "source_admin", "frontend_root", "pacing",
          "reference_plan", "dataset"}


def _validate_business_lineage(backend: Path, value: dict, request: dict, plan: dict) -> None:
    """将直接复制源还原到最初的数据集；arm 阶段不得伪造新的业务来源。"""
    stage = plan.get("copy_stage")
    if stage != value.get("copy_stage"):
        raise ValueError("复制计划阶段与固定 run 清单不同")
    reference = read_json(bound_file(backend, request["reference_plan"]))
    dataset = read_json(bound_file(backend, request["dataset"]))
    source = reference.get("source")
    origin_scope = source.get("scope_id") if isinstance(source, dict) else None
    if (not isinstance(origin_scope, str)
            or dataset.get("plan_sha256") != plan_hash(reference)
            or dataset.get("source_scope_id") != origin_scope):
        raise ValueError("原 plan 与 dataset 身份不一致")
    if stage == "source_to_seed":
        if plan.get("source_scope") != origin_scope:
            raise ValueError("既有业务数据不属于复制计划的直接来源")
        return
    if stage != "seed_to_arm":
        raise ValueError("post-copy 不支持当前复制阶段")

    from devex_clone_seed_source import published_source

    descriptor = value.get("source_registration")
    if not isinstance(descriptor, dict):
        raise ValueError("seed_to_arm 缺少已发布 seed 来源登记")
    inherited = published_source(backend, descriptor)
    registration = inherited["registration"]
    if (registration["source_request"] != value.get("source_request")
            or registration["source_environment"] != value.get("source_environment")
            or inherited["request"].get("source", {}).get("scope_id") != plan.get("source_scope")):
        raise ValueError("seed_to_arm 直接来源不属于当前复制计划")
    previous = resolve_post_registration(
        backend, inherited["directory"], descriptor=registration["post_copy"], cleanup=True)
    if (previous.request["reference_plan"] != request["reference_plan"]
            or previous.request["dataset"] != request["dataset"]):
        raise ValueError("seed_to_arm 必须继承原 post-copy 的 plan 与 dataset 绑定")
    original = read_json(bound_file(backend, inherited["manifest"]["source_request"]))
    if original.get("source", {}).get("scope_id") != origin_scope:
        raise ValueError("seed_to_arm 原始来源 scope 与数据集不一致")


def registered(backend: Path, directory: Path, *, cleanup: bool = False) -> dict:
    request = resolve_post_registration(backend, directory, cleanup=cleanup).request
    exact(request, FIELDS)
    if request["run_manifest"] != binding(directory / "manifest.json"):
        raise ValueError("复制后登记属于其他 run")
    return request


def registration(backend: Path, directory: Path, *, descriptor=None, current=None, cleanup=False):
    return resolve_post_registration(backend, directory, descriptor=descriptor, current=current, cleanup=cleanup)


def validate_registration(backend: Path, directory: Path, value: dict, request: dict):
    from devex_clone_factory_context import initialization_history
    from devex_clone_run import require_target_copy

    exact(request, FIELDS)
    if request["format_version"] != 1 or request["kind"] != "devex-clone-post-copy":
        raise ValueError("复制后登记类型无效")
    verified = require_target_copy(backend, directory, value, ("api",))
    attempts = [item for item in load_state(directory)["attempts"] if item["stage"] == "copy"]
    output = local_path(backend, value["copy_directory"])
    expected = {"run_manifest": binding(directory / "manifest.json"),
                "copy_stage_receipt": attempts[-1]["result"], "copy_result": binding(output / "result.json"),
                "ledger_head": binding(output / "ledger/head.json")}
    if any(request[field] != item for field, item in expected.items()):
        raise ValueError("复制后登记必须绑定当前复制成功阶段及账本 head")
    for field in FIELDS - {"format_version", "kind", "node", "source_admin", "frontend_root", "pacing"}:
        bound_file(backend, request[field])
    external_file(request["node"])
    bridge_builds = [read_json(Path(item["path"]))["build"] for item in value["build_bridges"]]
    if {key: request["backend_build"][key] for key in ("path", "sha256")} not in bridge_builds:
        raise ValueError("API 产物必须属于本 run 已审计的产品构建")
    initial, _ = initialization_history(backend, bound_file(backend, value["initialized"]))
    selected = initial["generation"]["selected"]
    private = read_json(Path(request["api_environment"]["path"]))
    exact(private, {"environment"})
    api = private["environment"]
    if not isinstance(api, dict) or any(not isinstance(k, str) or not isinstance(v, str) for k, v in api.items()):
        raise ValueError("派生 API 环境必须是文本映射")
    old = read_json(bound_file(backend, value["target_environment"]))["environment"]
    environment_delta(old, api, selected["frontend_url"])
    administrator(request["source_admin"], api)
    frontend = Path(request["frontend_root"])
    exact_directory(frontend)
    if frontend != backend.parent / "ryframe-vue3" or not frontend.is_dir():
        raise ValueError("pacing 只允许读取明确的当前前端项目")
    pacing_binding(backend, directory, request)
    _validate_business_lineage(backend, value, request, verified.plan)
    return verified


def pacing_binding(backend, directory, request):
    pacing = request["pacing"]
    exact(pacing, {"contract", "bindings", "python_sha256", "login_budget_module_sha256"})
    exact(pacing["bindings"], {"python", "login_budget_state"})
    state = local_path(backend, pacing["bindings"]["login_budget_state"])
    if state != directory / "post-copy/login-budget.json":
        raise ValueError("登录预算写入必须限定当前 run 的明确文件")
    external_file({"path": pacing["bindings"]["python"], "sha256": pacing["python_sha256"]})
    external_file({"path": str(Path(request["frontend_root"]) / "scripts/browser-login-budget.mjs"),
                   "sha256": pacing["login_budget_module_sha256"]})


def register(backend: Path, directory: Path, value: dict, filename: Path) -> dict:
    filename = local_path(backend, str(filename))
    original = binding(filename)
    request = read_json(filename)
    validate_registration(backend, directory, value, request)
    if binding(filename) != original:
        raise ValueError("复制后请求读取期间变化")
    path = directory / "post-copy.json"
    if path.exists() or linked(path):
        if linked(path) or read_json(path) != request:
            raise ValueError("固定复制后登记已有不同内容，拒绝覆盖")
    else:
        write_json(path, request)
    return {"status": "post_copy_registered", "registration": binding(path), "remote_writes": 0,
            "worker_must_remain_stopped": True, "restore_qualified": False}


def _invalidated_runtime(backend: Path, directory: Path, active) -> dict:
    from full_stack_process import process_identity, read_process

    state = load_state(directory)
    prepares = [item for item in state["attempts"] if item["stage"] == "post-copy" and item["mode"] == "prepare"
                and item["status"] == "passed"]
    if not prepares:
        raise ValueError("amendment 必须绑定已发布的旧 prepare")
    prepared = prepares[-1]
    prepare_result = read_json(bound_file(backend, prepared["result"]))
    handoff_path = active.runtime / "handoff.json"
    handoff = read_json(handoff_path)
    if (prepare_result.get("status") != "post_copy_api_prepared"
            or prepare_result.get("handoff") != binding(handoff_path)
            or handoff.get("registration") != active.descriptor or handoff.get("worker_started") is not False):
        raise ValueError("旧 prepare 与活动登记或 API-only 交接不符")
    starts = [item for item in state["attempts"] if item["stage"] == "runtime-target" and item["mode"] == "start"
              and item["number"] > prepared["number"]]
    if not starts or starts[-1]["status"] != "failed":
        raise ValueError("amendment 只处理已明确失败的旧 API 启动")
    failed = starts[-1]
    stops = [item for item in state["attempts"] if item["stage"] == "runtime-target" and item["mode"] == "stop"
             and item["number"] > failed["number"] and item["status"] == "passed"]
    if not stops or any(item["number"] > stops[-1]["number"] and item["stage"] == "runtime-target"
                        for item in state["attempts"]):
        raise ValueError("失败 API 必须由最新的精确 stop 收尾")
    stopped = stops[-1]
    stop_result = read_json(bound_file(backend, stopped["result"]))
    expected_processes = {"api": {"state": "stopped", "identity": None, "ready": False}}
    history_path, api_path = active.runtime / "producer-history.json", active.runtime / "api.json"
    history, contract = read_json(history_path), read_json(active.runtime / "runtime.json")
    identity = read_process(active.runtime, "api", stop_result["scope_id"])
    events = history.get("events", [])
    if (stop_result.get("format_version") != 1 or stop_result.get("kind") != "devex-clone-runtime-control"
            or stop_result.get("operation") != "stop" or stop_result.get("runtime_directory") != str(active.runtime)
            or stop_result.get("processes") != expected_processes
            or stop_result.get("producer_history") != str(history_path)
            or stop_result.get("scope_id") != contract.get("scope_id")
            or history.get("format_version") != 1 or history.get("kind") != "devex-clone-producer-history"
            or history.get("scope_id") != stop_result.get("scope_id")
            or history.get("runtime_directory") != str(active.runtime)
            or [item.get("event") for item in events] != ["start-intent", "started", "start-failed"]
            or [item.get("sequence") for item in events] != [0, 1, 2]
            or any(item.get("role") != "api" for item in events)
            or len({item.get("token") for item in events}) != 1 or events[1].get("identity") != identity
            or events[2].get("error_type") != failed["error_type"]
            or process_identity(identity["pid"]) is not None
            or (active.runtime / "worker.json").exists() or linked(active.runtime / "worker.json")):
        raise ValueError("旧 API 启动、失败与停止证据没有完整闭合")
    for url in (handoff["api_url"], contract["worker_ready_url"]):
        require_closed_port(url)
    failure, controller = directory / f"failure-{failed['number']:04d}.json", directory / f"controller-{failed['number']:04d}.json"
    diagnosis = read_json(failure)
    if (diagnosis.get("attempt") != failed["number"] or diagnosis.get("stage") != "runtime-target"
            or diagnosis.get("mode") != "start" or diagnosis.get("error_type") != failed["error_type"]
            or diagnosis.get("controller") != binding(controller)
            or not any(frame.get("function") == "_start_one" for frame in diagnosis.get("frames", []))):
        raise ValueError("旧启动失败诊断不属于已停止 API")
    return {"prepare": prepared["result"], "handoff": binding(handoff_path),
            "runtime": binding(active.runtime / "runtime.json"), "binaries": binding(active.runtime / "binaries.json"),
            "api_process": binding(api_path), "producer_history": binding(history_path),
            "failed_start": {"attempt": failed["number"], "controller": binding(controller), "failure": binding(failure)},
            "stopped": {"attempt": stopped["number"], "result": stopped["result"]}}


def amend(backend: Path, directory: Path, value: dict, filename: Path, number: int) -> dict:
    active = registration(backend, directory, current=number)
    candidate_path = local_path(backend, str(filename))
    candidate_binding, candidate = binding(candidate_path), read_json(candidate_path)
    validate_registration(backend, directory, value, candidate)
    if any(candidate[field] != active.request[field] for field in FIELDS - {"api_environment"}):
        raise ValueError("post-copy amendment 只能修正 API 环境绑定")
    old = read_json(bound_file(backend, active.request["api_environment"]))["environment"]
    new = read_json(bound_file(backend, candidate["api_environment"]))["environment"]
    changed = {key for key in set(old) | set(new) if old.get(key) != new.get(key)}
    if changed != {"APP_CORS_ALLOW_ORIGINS"}:
        raise ValueError("amendment 私有环境只能修正单一 CORS 字段")
    state = load_state(directory)
    if ((directory / "seed-runtime.json").exists()
            or any(item["stage"] == "seed-runtime" for item in state["attempts"])
            or any(item["stage"] == "post-copy" and item["mode"] in {"schedules", "reconcile", "verify"}
                   for item in state["attempts"])):
        raise ValueError("业务动作或 seed 已开始，不能修订 API 配置")
    evidence = _invalidated_runtime(backend, directory, active)
    sequence = active.sequence + 1
    payload = {"format_version": 1, "kind": "devex-clone-post-copy-amendment", "sequence": sequence,
               "run_manifest": binding(directory / "manifest.json"), "predecessor": active.descriptor,
               "request_sha256": plan_hash(candidate), "api_environment": candidate["api_environment"],
               "invalidated_runtime": evidence}
    amendments = directory / "post-copy/amendments"
    amendments.mkdir(exist_ok=active.sequence > 0)
    path = amendments / f"{sequence:04d}.json"
    if path.exists() or linked(path):
        raise ValueError("post-copy amendment 目标已存在，拒绝覆盖")
    if binding(candidate_path) != candidate_binding:
        raise ValueError("完整修正请求在核验期间变化")
    write_json(path, payload)
    return {"status": "post_copy_amended", "registration": binding(path), "sequence": sequence,
            "predecessor": active.descriptor, "runtime_directory": str(directory / "post-copy" / f"runtime-{sequence:04d}"),
            "remote_writes": 0, "worker_must_remain_stopped": True, "restore_qualified": False}


def runtime_inputs(backend: Path, directory: Path, *, descriptor=None) -> tuple[dict, dict]:
    state = load_state(directory, verify_results=False)
    prepares = [item for item in state["attempts"] if item["stage"] == "post-copy" and item["mode"] == "prepare"
                and item["status"] == "passed"]
    if not prepares:
        raise ValueError("API runtime 尚未取得已发布的准备结果")
    if descriptor is not None:
        matched = []
        for item in prepares:
            candidate = read_json(bound_file(backend, item["result"]))
            if read_json(bound_file(backend, candidate["handoff"])).get("registration") == descriptor:
                matched.append(candidate)
        if not matched:
            raise ValueError("活动 post-copy 登记没有对应的新 prepare")
        prepared = matched[-1]
    else:
        prepared = read_json(bound_file(backend, prepares[-1]["result"]))
    if prepared.get("status") != "post_copy_api_prepared" or prepared.get("worker_started") is not False:
        raise ValueError("API runtime 准备结果未保持 Worker 停止")
    handoff_path = bound_file(backend, prepared["handoff"])
    handoff = read_json(handoff_path)
    active = registration(backend, directory, descriptor=handoff["registration"], cleanup=True)
    request = active.request
    private = read_json(bound_file(backend, request["api_environment"]))
    exact(private, {"environment"})
    runtime = active.runtime
    exact_directory(runtime)
    handoff = read_json(runtime / "handoff.json")
    if (prepared["handoff"] != binding(runtime / "handoff.json")
            or handoff.get("registration") != active.descriptor or handoff.get("worker_started") is not False):
        raise ValueError("API runtime 交接不属于固定登记")
    if handoff.get("runtime") != binding(runtime / "runtime.json") or handoff.get("binaries") != binding(runtime / "binaries.json"):
        raise ValueError("API runtime 文件发生变化")
    return {"runtime_dir": str(runtime), "api_url": handoff["api_url"]}, private["environment"]


@contextmanager
def target_lock(backend: Path, value: dict, current_run: Path | None = None):
    target = bound_file(backend, value["initialized"]).parent
    ledger = local_path(backend, value["copy_directory"]) / "ledger"
    storage = nullcontext()
    if current_run is not None:
        from devex_clone_run import target_storage_control

        storage = target_storage_control(backend, current_run, value)
    with storage, process_guard(target, GUARDS["target"]), process_guard(ledger, GUARDS["ledger"]):
        if any(path.exists() or linked(path) for path in (target / "initialize.lock", ledger / "clone.lock")):
            raise ValueError("复制锁尚未回收，不能开始 API 交接")
        yield


def execute_post(backend: Path, directory: Path, value: dict, mode: str, number: int,
                 request_file: Path | None = None, producer_binding: dict | None = None) -> dict:
    from devex_clone_post_context import Context
    from devex_clone_post_actions import Bridge, execute_actions
    from devex_clone_post_process import require_quiet, recover_session, cleanup_failure

    if mode == "recover-session":
        if producer_binding is None:
            raise ValueError("Node 回收需要明确 --producer-binding")
        return recover_session(backend, directory, number, producer_binding)
    require_quiet(backend, directory, number)
    with target_lock(backend, value, current_run=directory):
        if mode == "register":
            if request_file is None:
                raise ValueError("登记必须提供明确请求文件")
            return register(backend, directory, value, request_file)
        if mode == "amend":
            if request_file is None:
                raise ValueError("amend 必须提供完整修正请求")
            return amend(backend, directory, value, request_file, number)
        if mode not in {"prepare", "schedules", "reconcile", "verify"}:
            raise ValueError("未知复制后阶段")
        context = Context(backend, directory, value, number)
        if mode == "prepare":
            return context.prepare()
        context.guard()
        if mode in {"schedules", "reconcile"}:
            bridge = Bridge(context)
            try:
                result = execute_actions(context, bridge, mode)
            except BaseException as original:
                try:
                    bridge.close()
                except Exception as cleanup_error:
                    cleanup_failure(context, "session-cleanup", cleanup_error, original)
                raise
            else:
                bridge.close()
                return result
        return verify_existing(context)


def verify_existing(context) -> dict:
    from devex_clone_post_actions import verify_confirmations
    from devex_clone_post_process import Producer, cleanup_failure

    confirmations = verify_confirmations(context)
    require_schedule_stage(context, confirmations)
    context.guard()
    context.business_objects()
    target = context.business_binding()
    path = context.output / "business-target.json"
    write_json(path, target)
    request = context.request
    producer = Producer(context, "existing")
    try:
        stdout = producer.communicate(timeout=1800)
    except BaseException as original:
        try:
            producer.close()
        except Exception as cleanup_error:
            cleanup_failure(context, "session-cleanup", cleanup_error, original)
        raise
    else:
        producer.close()
    import json
    observed = json.loads(stdout)
    if (observed.get("status") != "copy_existing_data_verified" or observed.get("target_binding_sha256") != plan_hash(target)
            or observed.get("plan_sha256") != target["source_plan_sha256"] or observed.get("copy") != target["copy"]
            or observed.get("scope_id") != target["target"]["scope_id"] or observed.get("source_scope_id") != target["source_scope_id"]
            or observed.get("side") != "copy_target" or observed.get("clone_verified") is not False
            or observed.get("restore_success") is not False
            or observed.get("input_files") != {"plan_sha256": request["reference_plan"]["sha256"],
                "dataset_sha256": request["dataset"]["sha256"], "target_binding_sha256": binding(path)["sha256"]}):
        raise ValueError("业务读取结果未绑定原 plan 与当前目标")
    write_json(context.output / "business-result.json", observed)
    objects = context.business_objects(remaining=True)
    require_schedule_stage(context, verify_confirmations(context))
    context.guard()
    return {"status": "post_copy_existing_data_verified", "evidence": binding(context.output / "business-result.json"),
            "objects": objects, "registration": context.request_binding, "worker_must_remain_stopped": True, "restore_qualified": False}


def require_schedule_stage(context, confirmations):
    attempts = [item for item in load_state(context.directory_root)["attempts"]
                if item["stage"] == "post-copy" and item["mode"] in {"schedules", "reconcile"}]
    if not attempts or attempts[-1]["mode"] != "schedules" or attempts[-1]["status"] != "passed":
        raise ValueError("最新调度阶段尚未通过完整来源和收尾核验，先继续 schedules")
    result = read_json(bound_file(context.backend, attempts[-1]["result"]))
    if (result.get("status") != "target_schedule_actions_verified" or result.get("registration") != context.request_binding
            or result.get("confirmed") != confirmations):
        raise ValueError("调度阶段发布结果与当前完整确认不同")
