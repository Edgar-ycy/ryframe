"""同一复制 run 的固定后续登记：API 准备、调度核对和既有数据读取。"""
from contextlib import contextmanager, nullcontext
from pathlib import Path
import subprocess

from devex_clone_capture import read_bound_json, read_json, write_json
from devex_clone_copy_locks import GUARDS
from devex_clone_model import exact, linked, local_path
from devex_clone_post_model import administrator, environment_delta, exact_directory, native_invoke, native_response
from devex_clone_run_state import binding, load_state
from devex_clone_source_proof import bound_file
from devex_clone_target_binding import external_file
from process_guard import process_guard
from restore_reference_plan import plan_hash
from devex_clone_post_registration import resolve_post_registration

FIELDS = {"format_version", "kind", "run_manifest", "copy_stage_receipt", "copy_result", "ledger_head",
          "api_environment", "backend_build", "node", "source_admin", "frontend_root", "pacing",
          "reference_plan", "dataset", "administrator_credentials"}


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

    from devex_clone_run import _published_seed_source

    descriptor = value.get("source_registration")
    if not isinstance(descriptor, dict):
        raise ValueError("seed_to_arm 缺少已发布 seed 来源登记")
    inherited = _published_seed_source(backend, value, live_storage=False)
    registration = inherited["registration"]
    if (inherited["source_request"] != value.get("source_request")
            or inherited["source_environment"] != value.get("source_environment")
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


def validate_registration(backend: Path, directory: Path, value: dict, request: dict,
                          owned_lock_identity: int | None = None):
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
    for field in FIELDS - {"format_version", "kind", "node", "source_admin", "frontend_root", "pacing", "administrator_credentials"}:
        bound_file(backend, request[field])
    external_file(request["node"])
    bridge_builds = [read_json(Path(item["path"]))["build"] for item in value["build_bridges"]]
    if {key: request["backend_build"][key] for key in ("path", "sha256")} not in bridge_builds:
        raise ValueError("API 产物必须属于本 run 已审计的产品构建")
    # 初始化文件树把控制互斥计入快照；只有确实持有 generation lock 时才能按“已持锁”复核。
    initial, _ = initialization_history(backend, bound_file(backend, value["initialized"]), owned_lock_identity)
    selected = initial["generation"]["selected"]
    private = read_json(Path(request["api_environment"]["path"]))
    exact(private, {"environment"})
    api = private["environment"]
    if not isinstance(api, dict) or any(not isinstance(k, str) or not isinstance(v, str) for k, v in api.items()):
        raise ValueError("派生 API 环境必须是文本映射")
    old = read_json(bound_file(backend, value["target_environment"]))["environment"]
    environment_delta(old, api, selected["frontend_url"])
    administrator(request["source_admin"], backend, request["administrator_credentials"])
    pacing_binding(backend, directory, request)
    _validate_business_lineage(backend, value, request, verified.plan)
    administrator(request["source_admin"], backend, request["administrator_credentials"])
    return verified


def pacing_binding(backend, directory, request):
    native_response(*native_invoke(
        "__source-evidence", {"operation": "copy-pacing-binding", "write": False,
            "arguments": {"backend": str(backend), "directory": str(directory),
                          "frontend_root": request["frontend_root"], "pacing": request["pacing"]}},
        subprocess.run, may_write=False,
    ))








def runtime_inputs(backend: Path, directory: Path, *, descriptor=None, cleanup=False) -> tuple[dict, dict]:
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
    environment = _read_runtime_environment(backend, request["api_environment"])
    runtime = active.runtime
    exact_directory(runtime)
    handoff = read_json(runtime / "handoff.json")
    if (prepared["handoff"] != binding(runtime / "handoff.json")
            or handoff.get("registration") != active.descriptor or handoff.get("worker_started") is not False):
        raise ValueError("API runtime 交接不属于固定登记")
    if handoff.get("runtime") != binding(runtime / "runtime.json") or handoff.get("binaries") != binding(runtime / "binaries.json"):
        raise ValueError("API runtime 文件发生变化")
    if cleanup:
        if environment != {"APP_ENV": "test"}:
            raise ValueError("历史 API 清理环境只能选择 APP_ENV=test")
    else:
        _verify_runtime_environment(backend, directory, state["manifest"], request, environment, handoff["api_url"])
    return {"runtime_dir": str(runtime), "api_url": handoff["api_url"]}, environment


def _read_runtime_environment(backend, descriptor):
    path = bound_file(backend, descriptor)
    private = read_bound_json(path, {**descriptor, "path": str(path)})
    exact(private, {"environment"})
    return private["environment"]


def _verify_runtime_environment(backend, directory, manifest, request, environment, api_url):
    if request["run_manifest"] != manifest:
        raise ValueError("API 运行环境不属于本 run 的冻结清单")
    original = read_bound_json(directory / "manifest.json", manifest)["target_environment"]
    environment_delta(_read_runtime_environment(backend, original), environment, api_url)
    if set(environment) != {"APP_ENV"}:
        temporary = environment.get("TEMP")
        try:
            valid = (set(environment) == {"APP_ENV", "TEMP", "TMP"} and temporary == environment["TMP"]
                     and not any(character in temporary for character in "\r\n")
                     and ".." not in Path(temporary).parts and local_path(backend, temporary).is_dir())
        except (OSError, ValueError):
            valid = False
        if not valid:
            raise ValueError("API 运行环境 TEMP/TMP 必须指向同一现有受管私有目录")
    for field in (manifest, original, request["api_environment"]):
        bound_file(backend, field)


@contextmanager
def target_lock(backend: Path, value: dict, current_run: Path | None = None):
    target = bound_file(backend, value["initialized"]).parent
    ledger = local_path(backend, value["copy_directory"]) / "ledger"
    storage = nullcontext()
    if current_run is not None:
        from devex_clone_run import target_storage_control

        storage = target_storage_control(backend, current_run, value)
    from devex_clone_target_state import generation_lock

    # 初始化文件树的快照包含控制互斥；post-copy 必须与复制阶段一样持有目标 generation lock
    # （guard + initialize.lock），否则初始化历史复核会同时遇到未登记且被锁定的 guard 文件。
    if any(path.exists() or linked(path) for path in (target / "initialize.lock", ledger / "clone.lock")):
        raise ValueError("复制锁尚未回收，不能开始 API 交接")
    with storage, generation_lock(target) as identity, process_guard(ledger, GUARDS["ledger"]):
        yield identity


def execute_post(backend: Path, directory: Path, value: dict, mode: str, number: int,
                 producer_binding: dict | None = None) -> dict:
    from devex_clone_post_context import Context
    from devex_clone_post_actions import Bridge, execute_actions
    from devex_clone_post_process import require_quiet, recover_session, cleanup_failure

    if mode in {"register", "prepare", "amend"}:
        raise ValueError("登记、准备和环境修订必须使用原生持锁正式入口")
    if mode == "recover-session":
        if producer_binding is None:
            raise ValueError("Node 回收需要明确 --producer-binding")
        return recover_session(backend, directory, number, producer_binding)
    require_quiet(backend, directory, number)
    with target_lock(backend, value, current_run=directory) as owned_lock_identity:
        if mode not in {"schedules", "reconcile", "verify"}:
            raise ValueError("登记、准备和环境修订必须使用原生持锁正式入口")
        context = Context(backend, directory, value, number, owned_lock_identity=owned_lock_identity)
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
