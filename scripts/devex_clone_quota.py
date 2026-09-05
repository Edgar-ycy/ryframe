"""原 API 存活时显式规划、一次应用和核对配额；未知结果不自动重放。"""
from types import SimpleNamespace
import re

from devex_clone_capture import read_json, write_json
from devex_clone_model import linked
from devex_clone_post_actions import Bridge
from devex_clone_post_process import cleanup_failure
from devex_clone_run_state import binding
from devex_clone_source_proof import bound_file
from devex_clone_quota_model import (targets, before_identities, action, classification, data,
    plan_path, load_plan, validate_plan, directories, intent, confirmation, authorization)
from restore_reference_plan import plan_hash


def observe(bridge, item):
    tenant = item["target"]["tenant_id"] if "target" in item else item["tenant_id"]
    return {"tenant": bridge.call("get", tenant_id=tenant), "usage": bridge.call("usage", tenant_id=tenant)}


def guard(context, request):
    from devex_clone_seed_runtime import current_guard

    before_identities(context.backend, context.directory_root, request)
    current_guard(context, request, old_api=True)


def create_plan(context, request, bridge):
    filename = plan_path(context.directory_root)
    if linked(filename):
        raise ValueError("配额计划不能是链接")
    previous = validate_plan(context.backend, context.directory_root, request) if filename.exists() else None
    identity = read_json(bound_file(context.backend, request["identity_plan"]))
    actions = []
    for target in targets(identity):
        guard(context, request)
        observed = observe(bridge, target)
        write_json(context.output / (target["slot"] + "-before.json"), observed)
        actions.append(action(target, observed))
    guard(context, request)
    plan = {"format_version": 1, "kind": "devex-seed-quotas",
            "registration": binding(context.directory_root / "seed-runtime.json"),
            "identity_plan": request["identity_plan"], "actions": actions}
    plan["plan_sha256"] = plan_hash(plan)
    if previous is not None:
        if any(old["target"] != new["target"] or old["before"] != new["before"] or old["after"] != new["after"]
               for old, new in zip(previous["actions"], actions)):
            raise ValueError("已有配额计划的完整前像变化，不能覆盖或自动重新规划")
    else:
        write_json(filename, plan)
    return {"status": "seed_quotas_planned", "plan": binding(filename), "remote_writes": 0,
            "worker_authorized": False, "restore_qualified": False}


def reconciled_before(context, plan, item, attempt):
    intent(context.backend, context.directory_root, plan, item, attempt)
    files = sorted(attempt.glob("reconcile-*.json"))
    if not files:
        raise ValueError("存在未知配额写入，必须先显式核对，不能重放")
    proof = read_json(files[-1])
    path = bound_file(context.backend, proof["observed"])
    if path != attempt / files[-1].name.replace("reconcile-", "observed-"):
        raise ValueError("原前像观察必须属于同次核对和当前写入意图")
    observed = read_json(path)
    if (proof != {"intent": binding(attempt / "intent.json"), "observed": proof["observed"], "state": "before"}
            or classification(item, observed) != "before"):
        raise ValueError("上次核对未证明原完整前像，禁止新写入")


def save_observed(context, attempt, observed):
    filename = attempt / ("observed-" + context.output.name.removeprefix("attempt-") + ".json")
    write_json(filename, observed)
    return binding(filename)


def confirm(context, item, attempt, observed, evidence, *, attempted, proven):
    if classification(item, observed) != "after":
        raise ValueError("配额完整后像不符，必须核对未知写入")
    write_json(attempt / "confirmed.json", {"plan": binding(plan_path(context.directory_root)),
        "target": item["target"], "observed": observed, "put_attempted": attempted,
        "api_response_proven": proven, "evidence": evidence})


def apply_one(context, request, plan, item, bridge):
    root, previous = directories(context.directory_root, item)
    if previous and (previous[-1] / "confirmed.json").exists():
        confirmation(context.backend, context.directory_root, plan, item, previous[-1])
        if classification(item, observe(bridge, item)) != "after":
            raise ValueError("已确认配额或用户角色数量发生变化")
        return
    if previous and (previous[-1] / "intent.json").exists():
        reconciled_before(context, plan, item, previous[-1])
    elif previous:
        for entry in previous[-1].iterdir():
            if entry.name != "before.json" and not re.fullmatch(r"observed-[0-9]{4}\.json", entry.name):
                raise ValueError("未登记 PUT 的配额目录含未知证据，必须核实")
            if classification(item, read_json(entry)) == "mismatch":
                raise ValueError("未登记 PUT 的部分本地前像已经不符，必须核实")
    root.mkdir(parents=True, exist_ok=True)
    attempt = root / f"{len(previous) + 1:04d}"
    attempt.mkdir()
    guard(context, request)
    before = observe(bridge, item)
    write_json(attempt / "before.json", before)
    if item["before"] == item["after"]:
        guard(context, request)
        confirm(context, item, attempt, before, {"observed": save_observed(context, attempt, before)}, attempted=False, proven=False)
        return
    if classification(item, before) != "before":
        raise ValueError("PUT 前完整租户及用户角色用量不同于计划")
    guard(context, request)
    body = {key: value for key, value in item["after"]["tenant"].items() if key not in {"tenant_id", "status"}}
    write_json(attempt / "intent.json", {"plan": binding(plan_path(context.directory_root)), "target": item["target"],
        "before": binding(attempt / "before.json"), "body": body,
        "operation": "put_platform_tenants_by_tenant_id", "automatic_retry": False})
    response = bridge.call("update", tenant_id=item["target"]["tenant_id"], body=body)
    write_json(attempt / "api-response.json", response)
    authorization(context.request, response["authorization"])
    if data(response["response"]) != item["after"]["tenant"]:
        raise ValueError("PUT 返回不匹配完整后像；不重放")
    observed = observe(bridge, item)
    evidence = {"observed": save_observed(context, attempt, observed),
                **{name: binding(attempt / (name + ".json")) for name in ("intent", "before", "api-response")}}
    guard(context, request)
    confirm(context, item, attempt, observed, evidence, attempted=True, proven=True)


def reconcile_one(context, request, plan, item, bridge):
    _, attempts = directories(context.directory_root, item)
    if not attempts:
        return
    attempt = attempts[-1]
    if (attempt / "confirmed.json").exists():
        confirmation(context.backend, context.directory_root, plan, item, attempt)
        if classification(item, observe(bridge, item)) != "after":
            raise ValueError("已确认配额后像漂移")
    elif (attempt / "intent.json").exists():
        intent(context.backend, context.directory_root, plan, item, attempt)
        observed = observe(bridge, item)
        descriptor = save_observed(context, attempt, observed)
        state = classification(item, observed)
        filename = attempt / ("reconcile-" + context.output.name.removeprefix("attempt-") + ".json")
        write_json(filename, {"intent": binding(attempt / "intent.json"), "observed": descriptor, "state": state})
        guard(context, request)
        if state == "after":
            confirm(context, item, attempt, observed, {"observed": descriptor, "reconcile": binding(filename),
                    **{name: binding(attempt / (name + ".json")) for name in ("intent", "before")}}, attempted=True, proven=False)
        elif state != "before":
            raise ValueError("未知写入既非完整前像也非完整后像，需要核实现场")


def execute_body(context, request, bridge, mode):
    plan = None if mode == "quotas-plan" else load_plan(context.backend, context.directory_root, request)
    if mode == "quotas-apply":
        for item in plan["actions"]:
            _, attempts = directories(context.directory_root, item)
            if attempts and (attempts[-1] / "intent.json").exists() and not (attempts[-1] / "confirmed.json").exists():
                reconciled_before(context, plan, item, attempts[-1])
    identity = read_json(bound_file(context.backend, request["identity_plan"]))
    guard(context, request)
    context.administrator()
    auth = bridge.login(post_request=context.request, tenant_ids=[item["tenant_id"] for item in targets(identity)])
    authorization(context.request, auth)
    write_json(context.output / "authentication.json", auth)
    if mode == "quotas-plan":
        return create_plan(context, request, bridge)
    for item in plan["actions"]:
        guard(context, request)
        (apply_one if mode == "quotas-apply" else reconcile_one)(context, request, plan, item, bridge)
    confirmations = []
    for item in plan["actions"]:
        _, attempts = directories(context.directory_root, item)
        if attempts and (attempts[-1] / "confirmed.json").exists():
            confirmation(context.backend, context.directory_root, plan, item, attempts[-1])
            if classification(item, observe(bridge, item)) != "after":
                raise ValueError("最终容量复验发现完整后像变化")
            confirmations.append(binding(attempts[-1] / "confirmed.json"))
    return {"status": "seed_quotas_verified" if len(confirmations) == 11 else "seed_quotas_reconciled",
            "plan": binding(plan_path(context.directory_root)), "confirmed": confirmations,
            "remote_writes": 0 if mode == "quotas-reconcile" else None,
            "worker_authorized": False, "restore_qualified": False}


def execute_quotas(context, request, mode):
    guard(context, request)
    producer_context = SimpleNamespace(**{**vars(context), "request": request,
        "request_binding": binding(context.directory_root / "seed-runtime.json")})
    bridge = Bridge(producer_context, kind=mode.replace("quotas-", "quota-"))
    try:
        result = execute_body(context, request, bridge, mode)
        bridge.call("close", timeout=10)
        guard(context, request)
    except BaseException as original:
        for name, cleanup in (("quota-logout", lambda: bridge.call("close", timeout=10)),
                              ("quota-final-guard", lambda: guard(context, request)), ("quota-producer-close", bridge.close)):
            try:
                cleanup()
            except Exception as error:
                cleanup_failure(context, name, error, original)
        raise
    else:
        bridge.close()
    return result
