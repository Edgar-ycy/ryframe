"""seed 十租户固定根部门的计划、显式写入、核对和只读验证。"""
from types import SimpleNamespace
import time

from devex_clone_post_actions import Bridge
from devex_clone_post_process import cleanup_failure, inspect_producer
from devex_clone_run_state import binding, load_state
from devex_clone_seed import identity_inputs, latest
from full_stack_process import process_identity
from restore_reference_plan import plan_hash
from devex_clone_department_model import (authorization, classify, confirmation,
    directories, identity_plan, identity_projection, image, intent, page_data, plan_path,
    read_prerequisite, require_prerequisite, snowflake,
    targets, template_identities, validate_plan)


SESSION_CLOSE_SECONDS = 10
CLOSE_MARGIN_SECONDS = 10


def guard(context, request, prerequisite=None, *, quick=False):
    from devex_clone_seed_runtime import current_guard, current_mutation_guard

    if quick:
        if prerequisite is None:
            raise ValueError("部门写入临界区守卫缺少身份前提")
        confirmations = getattr(context, "_department_schedule_confirmations", None)
        if confirmations is None:
            raise ValueError("部门写入临界区缺少本阶段完整守卫结果")
        current_mutation_guard(context, request, prerequisite, confirmations)
    else:
        context._department_schedule_confirmations = current_guard(context, request, old_api=True)
    if prerequisite is not None:
        require_prerequisite(context.backend, context.directory_root, request, prerequisite)


def observe(context, bridge, action, target):
    listed = bridge.call("list", tenant_id=target["tenant_id"], name=action["body"]["name"])
    authorization(target, listed["authorization"], directory=context.directory_root)
    data = page_data(listed["response"], directory=context.directory_root)
    detail = None
    desired = [item for item in data["items"] if item.get("name") == action["body"]["name"]]
    if len(desired) == 1:
        identifier = snowflake(desired[0].get("id"))
        detail = bridge.call("detail", tenant_id=target["tenant_id"], department_id=identifier)
    template = bridge.call("template", tenant_id=target["tenant_id"])
    observed = {"list": listed, "detail": detail, "template": template}
    image(action, observed, target, directory=context.directory_root)
    return observed


def native(context, method, **arguments):
    from devex_clone_department_model import _native_model
    return _native_model(method, arguments, backend=context.backend, directory=context.directory_root)


def prerequisite(context, request):
    return native(context, "prepare-prerequisite", request=request)


def create_plan(context, request, bridge):
    prior = prerequisite(context, request)
    source = identity_plan(context.backend, request)
    templates = template_identities(context.backend, request)
    actions = []
    for target, template in zip(targets(source), templates):
        action = native(context, "initial-action", target=target, template=template)
        guard(context, request, prior)
        observed = observe(context, bridge, action, target)
        actions.append(native(context, "plan-action", action=action, target=target,
                              observed=observed, output=str(context.output)))
    guard(context, request, prior)
    return native(context, "publish-plan", request=request, actions=actions)


def close_timeout():
    """为十个 owner、十个模板会话及 pacing 关闭保留有限预算。"""
    return SESSION_CLOSE_SECONDS * 21 + CLOSE_MARGIN_SECONDS


def _close_rpc_timeout(deadline):
    remaining = deadline - time.monotonic() - CLOSE_MARGIN_SECONDS
    if remaining <= 0:
        raise TimeoutError("部门会话关闭已没有安全回收预算")
    return min(SESSION_CLOSE_SECONDS * 21, remaining)


def _native_guard(context, operation, output, attempt=None):
    from devex_clone_department_model import native_evidence
    return native_evidence(context.backend, context.directory_root, {
        "operation": operation, "output": str(output), "target": _target(context), "attempt": attempt})


def _target(context):
    target = getattr(context, "target_config", None)
    if isinstance(target, dict) and isinstance(target.get("s3"), dict):
        return {"scope_id": target.get("scope_id"), "endpoint": target["s3"].get("endpoint"),
                "configuration_sha256": plan_hash(target["s3"])}
    return None


def _publish_adoption_guard(context, attempt):
    output = context.directory_root / "seed-runtime" / f"attempt-{attempt['number']:04d}"
    return _native_guard(context, "publish", output, attempt["number"])


def _adoption_evidence(context, request, state):
    inputs = native(context, "adoption-inputs", state=state)
    processes = {str(pid): process_identity(pid) for pid in inputs["pids"]}
    producers = {str(attempt["number"]): inspect_producer(context.backend, context.directory_root, attempt)
                 for attempt in inputs["attempts"][:-1]}
    value = native(context, "adoption-evidence", request=request, state=state,
                   processes=processes, producers=producers, target=_target(context))
    if value is None:
        return None, None
    return value["evidence"], value["prerequisite"]


def adopt_failed_plan(context, request):
    state = load_state(context.directory_root)
    if len(state["attempts"]) < 2:
        return None
    current = state["attempts"][-1]
    candidate = state["attempts"][-2]
    if (candidate["stage"], candidate["mode"], candidate["status"]) != (
            "seed-runtime", "departments-plan", "failed"):
        return None
    number = current["number"]
    if (current["stage"], current["mode"], current["status"]) != (
            "seed-runtime", "departments-plan", "running") or context.output != (
            context.directory_root / "seed-runtime" / f"attempt-{number:04d}"):
        raise ValueError("部门计划采用必须属于紧邻的当前重试 attempt")
    guard(context, request)
    identity_projection(context.backend, request, allow_absent=True)
    context.administrator()
    evidence, prior = _adoption_evidence(context, request, state)
    guard(context, request, prior)
    repeated, _ = _adoption_evidence(context, request, load_state(context.directory_root))
    if repeated != evidence:
        raise ValueError("部门计划采用复核期间证据发生变化")
    evidence["guard"] = _publish_adoption_guard(context, state["attempts"][-1])
    return {"status": "seed_departments_planned", "plan": evidence["plan"],
            "prerequisite": evidence["prerequisite"], "adopted": evidence,
            "remote_writes": 0, "worker_authorized": False, "restore_qualified": False}


def load_plan(backend, directory, request, *, current=None):
    record = latest(directory, "seed-runtime", {"departments-plan"}, current=current)
    from devex_clone_department_model import _native_model
    return _native_model("load-plan", {"request": request, "record": record}, backend=backend, directory=directory)


def reconciled_before(context, plan, action, target, attempt):
    return native(context, "reconciled-before", plan=plan, action=action, target=target, attempt=str(attempt))


def apply_one(context, request, prerequisite, plan, action, target, bridge):
    from pathlib import Path
    step = native(context, "begin-action", action=action, target=target)
    if step["confirmed"]:
        if classify(action, observe(context, bridge, action, target), target, directory=context.directory_root) != "after":
            raise ValueError("已确认部门发生变化")
        return False
    attempt = Path(step["attempt"])
    guard(context, request, prerequisite, quick=True)
    before = observe(context, bridge, action, target)
    guard(context, request, prerequisite, quick=True)
    native(context, "write-intent", action=action, target=target, attempt=str(attempt),
           observed=before, idempotency_key=step["idempotency_key"])
    response = bridge.call("create", tenant_id=target["tenant_id"], body=action["body"],
                           idempotency_key=step["idempotency_key"])
    native(context, "write-response", action=action, target=target, attempt=str(attempt), response=response)
    observed = observe(context, bridge, action, target)
    proof = native(context, "complete-action", action=action, target=target, attempt=str(attempt),
                   observed=observed, output=str(context.output))
    guard(context, request, prerequisite, quick=True)
    native(context, "confirm-action", action=action, target=target, attempt=str(attempt), observed=observed, proof=proof)
    return True


def reconcile_one(context, request, prerequisite, plan, action, target, bridge):
    _, attempts = directories(context.directory_root, action)
    if not attempts:
        return None
    attempt = attempts[-1]
    if (attempt / "confirmed.json").exists():
        confirmation(context.backend, context.directory_root, plan, action, target, attempt)
        if classify(action, observe(context, bridge, action, target), target, directory=context.directory_root) != "after":
            raise ValueError("已确认部门后像漂移")
        return "after"
    if not (attempt / "intent.json").exists():
        return None
    intent(context.backend, context.directory_root, plan, action, attempt)
    observed = observe(context, bridge, action, target)
    proof = native(context, "reconcile-action", action=action, target=target, attempt=str(attempt),
                   observed=observed, output=str(context.output))
    guard(context, request, prerequisite, quick=True)
    if proof["state"] == "after":
        native(context, "confirm-action", action=action, target=target, attempt=str(attempt), observed=observed, proof=proof)
    elif proof["state"] != "before":
        raise ValueError("未知部门写入既非完整空前像也非唯一完整后像")
    return proof["state"]


def verify_all(context, request, prerequisite, plan, source_targets, bridge):
    confirmed, observed = [], []
    for action, target in zip(plan["actions"], source_targets):
        _, attempts = directories(context.directory_root, action)
        if not attempts:
            raise ValueError("部门尚未覆盖全部普通租户")
        confirmation(context.backend, context.directory_root, plan, action, target, attempts[-1])
        guard(context, request, prerequisite, quick=True)
        value = observe(context, bridge, action, target)
        proof = native(context, "verify-action", action=action, target=target, observed=value, output=str(context.output))
        observed.append(proof["observed"])
        confirmed.append(proof["confirmed"])
    return confirmed, observed


def execute_body(context, request, bridge, mode):
    source_targets = targets(identity_plan(context.backend, request))
    templates = template_identities(context.backend, request)
    plan = None
    if mode == "departments-plan":
        guard(context, request)
        identity_projection(context.backend, request, allow_absent=True)
    else:
        plan = load_plan(context.backend, context.directory_root, request, current=None)
        prior = read_prerequisite(context.backend, context.directory_root)
        require_prerequisite(context.backend, context.directory_root, request, prior)
        guard(context, request, prior)
    context.administrator()
    auth = bridge.login(post_request=context.request, tenant_ids=[item["tenant_id"] for item in source_targets],
                        template_identities=templates)
    native(context, "authenticate", request=request, auth=auth, output=str(context.output))
    if mode == "departments-plan":
        return create_plan(context, request, bridge)
    if mode == "departments-apply":
        for action, target in zip(plan["actions"], source_targets):
            _, attempts = directories(context.directory_root, action)
            if attempts and (attempts[-1] / "intent.json").exists() and not (attempts[-1] / "confirmed.json").exists():
                reconciled_before(context, plan, action, target, attempts[-1])
    guard(context, request, prior, quick=True)
    writes, unresolved = 0, []
    if mode == "departments-apply":
        for action, target in zip(plan["actions"], source_targets):
            guard(context, request, prior, quick=True)
            writes += int(apply_one(context, request, prior, plan, action, target, bridge))
    elif mode == "departments-reconcile":
        for action, target in zip(plan["actions"], source_targets):
            guard(context, request, prior, quick=True)
            if reconcile_one(context, request, prior, plan, action, target, bridge) == "before":
                unresolved.append(action["target"]["slot"])
    elif mode == "departments-verify":
        confirmed, observed = verify_all(context, request, prior, plan, source_targets, bridge)
        return {"status": "seed_departments_verified", "plan": binding(plan_path(context.directory_root)),
                "confirmed": confirmed, "observed": observed, "remote_writes": 0,
                "worker_authorized": False, "restore_qualified": False}
    else:
        raise ValueError("未知部门阶段")
    return native(context, "stage-result", plan=plan, targets=source_targets, mode=mode,
                  writes=writes, unresolved=unresolved)


def execute_departments(context, request, mode):
    if mode == "departments-plan":
        adopted = adopt_failed_plan(context, request)
        if adopted is not None:
            return adopted
    source, private = identity_inputs(context.backend, context.directory_root, request, context.request)
    timeout = close_timeout()
    producer_context = SimpleNamespace(**{**vars(context), "request": request,
        "request_binding": binding(context.directory_root / "seed-runtime.json"),
        "private": {**context.private, "target_api": private}})
    bridge = Bridge(producer_context, kind=mode.replace("departments-", "department-"))
    deadline, close_requested, producer_close_requested = None, False, False

    def request_close():
        nonlocal close_requested
        close_requested = True
        return bridge.call("close", timeout=_close_rpc_timeout(deadline))

    def close_producer():
        nonlocal producer_close_requested
        producer_close_requested = True
        bridge.close()

    try:
        result = execute_body(context, request, bridge, mode)
        deadline = time.monotonic() + timeout
        request_close()
        close_producer()
        guard(context, request, read_prerequisite(context.backend, context.directory_root))
    except BaseException as original:
        if deadline is None:
            deadline = time.monotonic() + timeout
        cleanup_steps = [] if close_requested else [("department-logout", request_close)]
        if not producer_close_requested:
            cleanup_steps.append(("department-producer-close", close_producer))
        for name, cleanup in cleanup_steps:
            try:
                cleanup()
            except Exception as error:
                cleanup_failure(context, name, error, original)
        raise
    return result
