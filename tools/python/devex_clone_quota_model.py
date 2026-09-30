"""固定 seed 身份之前的配额计划和完整前后像；不承担 HTTP 或自动恢复写入。"""
import re

from devex_clone_capture import read_json
from devex_clone_model import exact, linked, local_path
from devex_clone_post import registration as post_registration
from devex_clone_post_model import exact_directory
from devex_clone_run_state import binding, load_state
from devex_clone_seed import latest
from devex_clone_source_proof import bound_file
from restore_reference_plan import plan_hash

FIELDS = {"name", "domain", "expire_at", "max_users", "max_roles", "max_storage_mb", "max_requests_per_min"}
MODES = {"quotas-plan", "quotas-apply", "quotas-reconcile"}


def targets(identity):
    groups, environment = identity["groups"], identity["environment"]
    quota = environment["quota"]
    exact(quota, {"system_max_users", "tenant_max_users", "import_headroom_per_tenant"})
    if any(type(item) is not int or not 0 < item <= 2147483647 for item in quota.values()):
        raise ValueError("配额和导入余量必须由身份环境明确登记正整数")
    expected = [("system", "system", "system", 100)] + [
        (f"tenant-{index:02d}", item["tenant_id"], "tenant", 10)
        for index, item in enumerate(environment["tenants"], 1)]
    if (len(expected) != 11 or len(groups) != 11 or len({item[1] for item in expected}) != 11
            or any(not re.fullmatch(r"[a-zA-Z0-9_-]{1,64}", item[1]) for item in expected)):
        raise ValueError("配额只允许系统及计划明确的十个租户")
    result = []
    for group, (slot, tenant, kind, count) in zip(groups, expected):
        if (any(group.get(key) != value for key, value in
                {"slot": slot, "tenant_id": tenant, "kind": kind, "count": count}.items())
                or len(group.get("users", [])) != count):
            raise ValueError("身份组必须保持固定 100 加 10×10 用户")
        result.append({"slot": slot, "tenant_id": tenant, "new_users": count,
                       "import_headroom": quota["import_headroom_per_tenant"] if kind == "tenant" else 0,
                       "max_users": quota[f"{kind}_max_users"]})
    return result


def before_identities(backend, directory, request):
    state = local_path(backend, request["identity_state"])
    if (state.exists() or linked(state) or (directory / "seed-runtime/runtime").exists()
            or linked(directory / "seed-runtime/runtime")):
        raise ValueError("身份准备或新 runtime 已开始，不能重新规划或改变配额")
    for item in load_state(directory)["attempts"]:
        if item["stage"] != "seed-runtime" or item["mode"] not in {"identities-apply", "identities-verify"}:
            continue
        output = directory / "seed-runtime" / f'attempt-{item["number"]:04d}'
        if any((output / name).exists() or linked(output / name) for name in ("session-launch.json", "session-process.json")):
            raise ValueError("身份生产者已启动或存在未知启动意图，不再开放配额写入")


def data(response):
    if not isinstance(response, dict) or response.get("code") != 200 or not isinstance(response.get("data"), dict):
        raise ValueError("配额证据缺少成功的原始 API envelope")
    return response["data"]


def authorization(post, auth):
    admin = post["source_admin"]
    if (any(auth.get(key) != admin[key] for key in ("subject_id", "tenant_id", "username"))
            or auth.get("tenant_id") != "system" or not isinstance(auth.get("permissions"), list)
            or auth.get("is_super_admin") is not True and "*" not in auth["permissions"]
            and not {"tenant:list", "tenant:edit", "tenant:usage:list"}.issubset(auth["permissions"])):
        raise ValueError("配额动作必须保持登记的系统管理员及全部必需权限")


def image(tenant_id, observed):
    exact(observed, {"tenant", "usage"})
    detail, usage = data(observed["tenant"]), data(observed["usage"])
    exact(detail, FIELDS | {"tenant_id", "status", "usage", "expiration_status", "capacity_status"})
    tenant = {key: value for key, value in detail.items() if key not in {"usage", "expiration_status", "capacity_status"}}
    exact(tenant, FIELDS | {"tenant_id", "status"})
    if (tenant["tenant_id"] != tenant_id or usage.get("tenant_id") != tenant_id
            or tenant["status"] != "enabled" or not isinstance(tenant["name"], str) or not tenant["name"]
            or any(tenant[key] is not None and not isinstance(tenant[key], str) for key in ("domain", "expire_at"))
            or any(type(tenant[key]) is not int or tenant[key] < 0 for key in FIELDS if key.startswith("max_"))
            or tenant["max_users"] <= 0 or detail["expiration_status"] not in {"never", "active", "expiring"}):
        raise ValueError("租户身份、启用状态或正配额不完整")
    counts = {}
    for resource, maximum in (("users", "max_users"), ("roles", "max_roles")):
        item = usage.get(resource, {})
        limit = tenant[maximum] if tenant[maximum] > 0 else None
        embedded = (detail.get("usage") or {}).get(resource, {})
        if (type(item.get("used")) is not int or item["used"] < 0 or item.get("limit") != limit
                or any(embedded.get(key) != item.get(key) for key in ("used", "limit"))):
            raise ValueError("租户用量与当前配额不一致")
        counts[resource] = item["used"]
    return {"tenant": tenant, **counts}


def action(target, observed):
    before = image(target["tenant_id"], observed)
    tenant = before["tenant"]
    required = before["users"] + target["new_users"] + target["import_headroom"]
    if (target["max_users"] < tenant["max_users"] or target["max_users"] < required
            or tenant["max_roles"] > 0 and before["roles"] + 1 > tenant["max_roles"]):
        raise ValueError("配额不能缩减，必须覆盖实际用量、普通身份、导入余量及新增角色")
    after = {**before, "tenant": {**tenant, "max_users": target["max_users"]}}
    return {"target": target, "before": before, "after": after, "observed": observed}


def classification(item, observed):
    current = image(item["target"]["tenant_id"], observed)
    if current == item["after"]:
        return "after"
    if current == item["before"]:
        return "before"
    return "mismatch"


def plan_path(directory):
    return directory / "seed-runtime/quota-plan.json"


def load_plan(backend, directory, request, *, current=None):
    record = latest(directory, "seed-runtime", {"quotas-plan"}, current=current)
    path = plan_path(directory)
    result = read_json(bound_file(backend, record["result"]))
    if result.get("status") != "seed_quotas_planned" or result.get("plan") != binding(path):
        raise ValueError("配额计划缺少当前登记的外层成功证明")
    return validate_plan(backend, directory, request)


def validate_plan(backend, directory, request):
    plan = read_json(plan_path(directory))
    exact(plan, {"format_version", "kind", "registration", "identity_plan", "actions", "plan_sha256"})
    identity = read_json(bound_file(backend, request["identity_plan"]))
    expected = targets(identity)
    if (plan["format_version"] != 1 or plan["kind"] != "devex-seed-quotas"
            or plan["registration"] != binding(directory / "seed-runtime.json")
            or plan["identity_plan"] != request["identity_plan"]
            or plan["plan_sha256"] != plan_hash({k: v for k, v in plan.items() if k != "plan_sha256"})
            or len(plan["actions"]) != 11):
        raise ValueError("配额计划缺少当前登记的外层成功证明")
    for target, item in zip(expected, plan["actions"]):
        if item != action(target, item["observed"]):
            raise ValueError("配额动作与原完整前像或明确身份计划不同")
    return plan


def directories(directory, item):
    root = directory / "seed-runtime/quotas" / item["target"]["slot"]
    exact_directory(root)
    children = sorted(root.iterdir()) if root.exists() else []
    if any(not child.is_dir() or child.name != f"{index:04d}" for index, child in enumerate(children, 1)):
        raise ValueError("配额动作目录不连续或含未知文件")
    for child in children:
        exact_directory(child)
    return root, children


def intent(backend, directory, plan, item, attempt):
    value = read_json(attempt / "intent.json")
    expected = {"plan": binding(plan_path(directory)), "target": item["target"],
                "before": binding(attempt / "before.json"), "body": {k: item["after"]["tenant"][k] for k in FIELDS},
                "operation": "put_platform_tenants_by_tenant_id", "automatic_retry": False}
    if value != expected or classification(item, read_json(bound_file(backend, value["before"]))) != "before":
        raise ValueError("配额写入意图或完整前像不同于明确计划")
    return value


def confirmation(backend, directory, plan, item, attempt):
    value = read_json(attempt / "confirmed.json")
    exact(value, {"plan", "target", "observed", "api_response_proven", "put_attempted", "evidence"})
    if (value["plan"] != binding(plan_path(directory)) or value["target"] != item["target"]
            or type(value["api_response_proven"]) is not bool or type(value["put_attempted"]) is not bool
            or classification(item, value["observed"]) != "after"):
        raise ValueError("配额确认缺少完整后像")
    for name, descriptor in value["evidence"].items():
        path = bound_file(backend, descriptor)
        allowed = (path.name == name + ".json" if name in {"intent", "before", "api-response"}
                   else re.fullmatch(rf"{name}-[0-9]{{4}}\.json", path.name))
        if path.parent != attempt or not allowed:
            raise ValueError("配额证据越界")
    if value["put_attempted"]:
        intent(backend, directory, plan, item, attempt)
        required = {"intent", "before", "observed"}
        if value["api_response_proven"]:
            required.add("api-response")
            response = read_json(attempt / "api-response.json")
            if data(response["response"]) != item["after"]["tenant"]:
                raise ValueError("配额 API 响应与完整后像不符")
            registration = read_json(bound_file(backend, plan["registration"]))
            post = post_registration(backend, directory, descriptor=registration["post_copy"])
            authorization(post.request, response["authorization"])
        else:
            required.add("reconcile")
            proof = read_json(bound_file(backend, value["evidence"]["reconcile"]))
            if proof != {"intent": binding(attempt / "intent.json"), "observed": value["evidence"]["observed"], "state": "after"}:
                raise ValueError("未知配额写入缺少当前意图的后像核对")
    else:
        required = {"observed"}
        if value["api_response_proven"] or item["before"] != item["after"]:
            raise ValueError("只读确认不能伪造 PUT 成功")
    if (set(value["evidence"]) != required
            or read_json(bound_file(backend, value["evidence"]["observed"])) != value["observed"]):
        raise ValueError("配额确认的证据集合或原始观察不同")
    return value


def capacity_evidence(backend, directory, request, *, current=None):
    plan = load_plan(backend, directory, request, current=current)
    record = latest(directory, "seed-runtime", MODES, current=current)
    result = read_json(bound_file(backend, record["result"]))
    confirmed = []
    for item in plan["actions"]:
        _, attempts = directories(directory, item)
        if not attempts:
            raise ValueError("配额尚未覆盖全部十一租户")
        confirmation(backend, directory, plan, item, attempts[-1])
        confirmed.append(binding(attempts[-1] / "confirmed.json"))
    if (result.get("status") != "seed_quotas_verified" or result.get("plan") != binding(plan_path(directory))
            or result.get("confirmed") != confirmed):
        raise ValueError("配额必须全部确认且最新外层阶段通过后才能创建身份")
    return {"stage": record["result"], "plan": binding(plan_path(directory)), "confirmed": confirmed}
