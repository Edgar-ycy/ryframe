"""固定 seed 部门计划、逐租户确认和只读发布证据。"""
import base64
import hashlib
import re
import xml.etree.ElementTree as ET

from devex_clone_capture import read_json
from devex_clone_model import exact, linked, local_path
from devex_clone_post_model import exact_directory
from devex_clone_run_state import binding
from devex_clone_source_proof import bound_file
from restore_reference_plan import plan_hash
from xlsx_workbook import (
    SPREADSHEET_NS as SHEET_NS,
    cell_text,
    open_workbook,
    shared_strings,
)

MODES = {"departments-plan", "departments-apply", "departments-reconcile", "departments-verify"}
DEPARTMENT_FIELDS = {"id", "name", "parent_id", "ancestors", "sort", "status", "remark", "created_at"}
TEMPLATE_FIELDS = {"base64", "bytes", "media_type", "sha256"}
TEMPLATE_PERMISSION = "system:user-import:add"


def snowflake(value):
    if (not isinstance(value, str) or not re.fullmatch(r"[1-9][0-9]{0,18}", value)
            or int(value) > 9223372036854775807):
        raise ValueError("部门准备资源必须返回明确字符串 ID")
    return value


def identity_plan(backend, request):
    plan = read_json(bound_file(backend, request["identity_plan"]))
    exact(plan, {"format_version", "kind", "environment", "groups", "plan_sha256"})
    payload = {key: value for key, value in plan.items() if key != "plan_sha256"}
    if (plan["format_version"] != 1 or plan["kind"] != "devex-identities"
            or plan_hash(payload) != plan["plan_sha256"]):
        raise ValueError("部门阶段引用的身份计划摘要无效")
    return plan


def targets(plan):
    groups = plan["groups"]
    if (len(groups) != 11 or groups[0].get("slot") != "system"
            or any(group.get("slot") != f"tenant-{index:02d}" or group.get("kind") != "tenant"
                   for index, group in enumerate(groups[1:], 1))):
        raise ValueError("部门阶段只允许身份计划中的十个普通租户")
    name = f"dv-{plan['environment']['plan_id']}-import"
    if len(name) > 50:
        raise ValueError("固定部门名称超过当前契约")
    return [{"slot": group["slot"], "tenant_id": group["tenant_id"], "admin": group["admin"],
             "body": {"name": name, "parent_id": None, "sort": 0},
             "expected": {"name": name, "parent_id": None, "ancestors": "0", "sort": 0,
                          "status": "1", "remark": None}}
            for group in groups[1:]]


def expected_entries(plan):
    entries = []
    for group in plan["groups"]:
        entries.extend(({"operation": "post_system_roles", "slot": group["slot"], "phase": "confirmed"},
                        {"operation": "put_system_roles_by_id_permissions", "slot": group["slot"],
                         "phase": "confirmed"}))
        for user in group["users"]:
            entries.extend(({"operation": "post_system_users", "slot": user["user_slot"], "phase": "confirmed"},
                            {"operation": "post_system_users_by_id_password_reset_requests",
                             "slot": user["user_slot"], "phase": "confirmed"},
                            {"operation": "post_auth_password_reset_complete", "slot": user["user_slot"],
                             "phase": "confirmed"}))
    if len(entries) != 622:
        raise ValueError("身份计划不能形成固定 622 项写入序列")
    return entries


def validate_entries(plan, ledger):
    expected = expected_entries(plan)
    entries = ledger["entries"]
    if not isinstance(entries, list) or len(entries) != len(expected):
        raise ValueError("身份账本写入序列不完整")
    reset_ids = []
    for actual, base in zip(entries, expected):
        reset = base["operation"] == "post_system_users_by_id_password_reset_requests"
        exact(actual, set(base) | {"id"} | ({"request_id"} if reset else set()))
        identifier = ledger["roles"].get(base["slot"], ledger["users"].get(base["slot"]))
        if any(actual[key] != value for key, value in base.items()) or actual["id"] != identifier:
            raise ValueError("身份账本序列与已确认角色或用户不一致")
        if reset:
            reset_ids.append(snowflake(actual["request_id"]))
    if len(reset_ids) != 200 or len(set(reset_ids)) != len(reset_ids):
        raise ValueError("身份账本密码请求 ID 不完整或重复")


def identity_projection(backend, request, *, allow_absent):
    plan = identity_plan(backend, request)
    root = local_path(backend, request["identity_state"])
    if not root.exists():
        if linked(root) or not allow_absent:
            raise ValueError("身份账本缺失或路径不是明确未创建状态")
        return {"state": "absent", "identity_state": str(root), "plan_sha256": plan["plan_sha256"],
                "ledger": None, "projection_sha256": None, "entries": 0, "roles": 0, "users": 0}
    exact_directory(root)
    if (root / "lock").exists() or linked(root / "lock"):
        raise ValueError("身份账本仍持锁或锁状态未知")
    ledger = read_json(root / "ledger.json")
    fields = {"format_version", "plan_sha256", "status", "entries", "roles", "users"}
    if ledger.get("status") in {"verifying", "verified"}:
        fields.add("publication_nonce")
    if ledger.get("status") == "verified":
        fields.add("verified_receipt")
    exact(ledger, fields)
    groups = {group["slot"] for group in plan["groups"]}
    users = {user["user_slot"] for group in plan["groups"] for user in group["users"]}
    if (ledger["format_version"] != 1 or ledger["plan_sha256"] != plan["plan_sha256"]
            or ledger["status"] not in {"prepared", "verifying", "verified"}
            or set(ledger["roles"]) != groups or set(ledger["users"]) != users):
        raise ValueError("身份账本不是全部确认的固定准备结果")
    if (ledger["status"] in {"verifying", "verified"}
            and (not isinstance(ledger["publication_nonce"], str)
                 or not re.fullmatch(r"[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}",
                                     ledger["publication_nonce"]))):
        raise ValueError("身份账本发布 nonce 无效")
    ids = list(ledger["roles"].values()) + list(ledger["users"].values())
    if len(ids) != 211 or len(set(ids)) != len(ids) or any(snowflake(value) != value for value in ids):
        raise ValueError("身份账本角色或用户 ID 不完整或重复")
    validate_entries(plan, ledger)
    stable = {key: ledger[key] for key in ("format_version", "plan_sha256", "entries", "roles", "users")}
    return {"state": ledger["status"], "identity_state": str(root), "plan_sha256": plan["plan_sha256"],
            "ledger": binding(root / "ledger.json"), "projection_sha256": plan_hash(stable),
            "entries": 622, "roles": 11, "users": 200}


def template_identities(backend, request):
    """从已确认身份账本选择每租户第一个固定用户，不接受计划外主体。"""
    plan = identity_plan(backend, request)
    projection = identity_projection(backend, request, allow_absent=False)
    if projection["state"] not in {"prepared", "verified"}:
        raise ValueError("部门模板需要已确认的固定身份账本")
    ledger = read_json(bound_file(backend, projection["ledger"]))
    identities = []
    for index, group in enumerate(plan["groups"][1:], 1):
        slot = f"tenant-{index:02d}"
        permissions = group.get("permissions")
        users = group.get("users")
        if (group.get("slot") != slot or group.get("kind") != "tenant"
                or not isinstance(permissions, list) or "*" in permissions
                or TEMPLATE_PERMISSION not in permissions or not isinstance(users, list) or not users):
            raise ValueError("部门模板用户不属于具备导入权限的固定租户身份组")
        user = users[0]
        exact(user, {"user_slot", "username", "password_env", "client_address"})
        subject_id = ledger["users"].get(user["user_slot"])
        identity = {"subject_id": snowflake(subject_id), "tenant_id": group["tenant_id"],
                    "username": user["username"], "password_env": user["password_env"],
                    "client_address": user["client_address"]}
        identities.append(identity)
    if len(identities) != 10 or len({item["subject_id"] for item in identities}) != 10:
        raise ValueError("部门模板固定主体数量不足或重复")
    if binding(bound_file(backend, projection["ledger"])) != projection["ledger"]:
        raise ValueError("部门模板主体推导期间身份账本变化")
    return identities


def prerequisite_path(directory):
    return directory / "seed-runtime/departments/identity-prerequisite.json"


def read_prerequisite(backend, directory):
    path = prerequisite_path(directory)
    value = read_json(path)
    exact(value, {"format_version", "kind", "registration", "identity"})
    identity = value["identity"]
    exact(identity, {"state", "identity_state", "plan_sha256", "ledger", "projection_sha256",
                     "entries", "roles", "users"})
    if (value["format_version"] != 1 or value["kind"] != "devex-seed-identity-prerequisite"
            or value["registration"] != binding(directory / "seed-runtime.json")
            or identity["state"] not in {"absent", "prepared"}):
        raise ValueError("部门计划的身份前提文件无效")
    bound_file(backend, binding(path))
    return value


def require_prerequisite(backend, directory, request, value):
    expected = value["identity"]
    actual = identity_projection(backend, request, allow_absent=expected["state"] == "absent")
    if actual != expected:
        raise ValueError("部门阶段期间身份账本前提发生变化")
    return value


def plan_path(directory):
    return directory / "seed-runtime/department-plan.json"


def validate_plan(backend, directory, request):
    plan = read_json(plan_path(directory))
    exact(plan, {"format_version", "kind", "registration", "identity_plan", "prerequisite", "actions",
                 "plan_sha256"})
    source = identity_plan(backend, request)
    expected_targets = targets(source)
    expected_templates = template_identities(backend, request)
    payload = {key: value for key, value in plan.items() if key != "plan_sha256"}
    if (plan["format_version"] != 1 or plan["kind"] != "devex-seed-departments"
            or plan["registration"] != binding(directory / "seed-runtime.json")
            or plan["identity_plan"] != request["identity_plan"]
            or plan["prerequisite"] != binding(prerequisite_path(directory))
            or plan["plan_sha256"] != plan_hash(payload) or len(plan["actions"]) != 10):
        raise ValueError("部门计划不属于当前 seed 登记或原身份计划")
    for expected, template_identity, action in zip(expected_targets, expected_templates, plan["actions"]):
        exact(action, {"target", "body", "expected", "before", "before_evidence",
                       "before_template_paths", "principal", "template_principal"})
        if (action["target"] != {key: expected[key] for key in ("slot", "tenant_id")}
                or action["body"] != expected["body"] or action["expected"] != expected["expected"]
                or action["template_principal"] != principal(template_identity)):
            raise ValueError("部门动作不是十租户固定根部门")
        evidence = bound_file(backend, action["before_evidence"])
        if (evidence.name != f'{expected["slot"]}-before.json'
                or evidence.parent.parent != directory / "seed-runtime"
                or not re.fullmatch(r"attempt-[0-9]{4,}", evidence.parent.name)):
            raise ValueError("部门计划前像证据不属于当前 seed 阶段")
        observed = image(action, read_json(evidence), expected)
        if (observed["principal"] != action["principal"]
                or observed["template_principal"] != action["template_principal"]
                or observed["directory"] != action["before"] or observed["department"] is not None
                or observed["template"]["paths"] != action["before_template_paths"]
                or expected["body"]["name"] in observed["template"]["paths"]):
            raise ValueError("部门计划未绑定完整目录且固定名称不存在的前像")
    read_prerequisite(backend, directory)
    return plan


def page_data(response):
    if not isinstance(response, dict) or response.get("code") != 200 or not isinstance(response.get("data"), dict):
        raise ValueError("部门观察缺少成功 API envelope")
    data = response["data"]
    exact(data, {"items", "page", "page_size", "total", "total_pages", "max_page_size"})
    expected_pages = 1 if data["total"] else 0
    if (data["page"] != 1 or data["page_size"] != 100 or data["max_page_size"] != 100
            or type(data["total"]) is not int or type(data["total_pages"]) is not int
            or data["total_pages"] != expected_pages or data["total"] < 0
            or data["total"] != len(data["items"]) or data["total"] > 100):
        raise ValueError("部门列表不是完整第一页或数量不可证明")
    return data


def authorization(target, value, *, write=False):
    exact(value, {"subject_id", "tenant_id", "username", "is_super_admin", "permissions"})
    admin = target["admin"]
    if (value["tenant_id"] != target["tenant_id"] or value["username"] != admin["username"]
            or snowflake(value["subject_id"]) != value["subject_id"] or not isinstance(value["permissions"], list)
            or "*" in value["permissions"]
            or not (value["is_super_admin"] is True
                    or ({"system:dept:list", "system:dept:add"} if write else {"system:dept:list"})
                    .issubset(value["permissions"]))):
        raise ValueError("部门动作租户管理员或权限不符")
    return {key: value[key] for key in ("subject_id", "tenant_id", "username")}


def principal(identity):
    return {key: identity[key] for key in ("subject_id", "tenant_id", "username")}


def template_authorization(target, value, expected):
    exact(value, {"subject_id", "tenant_id", "username", "is_super_admin", "permissions"})
    if (principal(value) != expected or value["tenant_id"] != target["tenant_id"]
            or snowflake(value["subject_id"]) != value["subject_id"]
            or value["is_super_admin"] is not False or not isinstance(value["permissions"], list)
            or "*" in value["permissions"] or TEMPLATE_PERMISSION not in value["permissions"]):
        raise ValueError("部门模板固定用户或导入权限不符")
    return principal(value)


def department_record(value):
    exact(value, DEPARTMENT_FIELDS)
    parent = value["parent_id"]
    if (snowflake(value["id"]) != value["id"] or not isinstance(value["name"], str)
            or not value["name"].strip() or (parent is not None and snowflake(parent) != parent)
            or not isinstance(value["ancestors"], str) or type(value["sort"]) is not int
            or not isinstance(value["status"], str)
            or (value["remark"] is not None and not isinstance(value["remark"], str))
            or not isinstance(value["created_at"], str) or not value["created_at"]):
        raise ValueError("部门目录对象字段无效")
    return value


def department(action, value):
    department_record(value)
    if any(value[key] != item for key, item in action["expected"].items()):
        raise ValueError("部门对象不是计划中的唯一启用根部门")
    return value


def created_response(action, target, value):
    exact(value, {"response", "authorization"})
    if authorization(target, value["authorization"], write=True) != action["principal"]:
        raise ValueError("部门创建响应主体不同于计划主体")
    response = value["response"]
    if not isinstance(response, dict) or response.get("code") != 200:
        raise ValueError("创建部门缺少成功 API envelope")
    return department(action, response.get("data"))


def directory_image(data):
    records = [department_record(item) for item in data["items"]]
    if len({item["id"] for item in records}) != len(records):
        raise ValueError("部门完整目录包含重复 ID")
    records.sort(key=lambda item: int(item["id"]))
    return {"total": len(records), "departments": records}


def _workbook_paths(content):
    try:
        with open_workbook(
            content, required_members=frozenset({"xl/worksheets/sheet2.xml"})
        ) as archive:
            strings = shared_strings(archive)
            sheet = ET.fromstring(archive.read("xl/worksheets/sheet2.xml"))
            paths = []
            for cell in sheet.findall(f".//{{{SHEET_NS}}}c"):
                match = re.fullmatch(r"A([1-9][0-9]*)", cell.get("r", ""))
                if match and int(match.group(1)) >= 2:
                    path = cell_text(cell, strings).strip()
                    if path:
                        paths.append(path)
    except (ET.ParseError, KeyError, OSError, ValueError) as error:
        raise ValueError("部门模板不是受限且可解析的当前工作簿") from error
    if len(paths) != len(set(paths)) or paths != sorted(paths):
        raise ValueError("部门模板可用路径重复或未按当前契约排序")
    return paths


def template_image(value, target, expected_principal):
    exact(value, {"response", "authorization"})
    template_authorization(target, value["authorization"], expected_principal)
    response = value["response"]
    exact(response, TEMPLATE_FIELDS)
    if (type(response["bytes"]) is not int or not 0 < response["bytes"] <= 512 * 1024
            or not isinstance(response["media_type"], str)
            or not response["media_type"].lower().startswith(
                "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet")
            or not isinstance(response["base64"], str)
            or not isinstance(response["sha256"], str)
            or not re.fullmatch(r"[a-f0-9]{64}", response["sha256"])):
        raise ValueError("部门模板响应字段或边界无效")
    try:
        content = base64.b64decode(response["base64"], validate=True)
    except (ValueError, TypeError) as error:
        raise ValueError("部门模板 base64 无效") from error
    if (len(content) != response["bytes"]
            or hashlib.sha256(content).hexdigest() != response["sha256"]):
        raise ValueError("部门模板长度或摘要不符")
    return {"bytes": len(content), "sha256": response["sha256"],
            "paths": _workbook_paths(content)}


def image(action, observed, target):
    exact(observed, {"list", "detail", "template"})
    listed = observed["list"]
    exact(listed, {"response", "authorization"})
    principal = authorization(target, listed["authorization"])
    if action.get("principal") is not None and principal != action["principal"]:
        raise ValueError("部门目录主体不同于固定计划主体")
    data = page_data(listed["response"])
    directory = directory_image(data)
    matches = [item for item in directory["departments"] if item["name"] == action["body"]["name"]]
    if len(matches) > 1:
        raise ValueError("部门目录含多个固定名称对象")
    item = None if not matches else department(action, matches[0])
    detail = observed["detail"]
    if item is None:
        if detail is not None:
            raise ValueError("固定部门不存在时不能带详情")
        return {"directory": directory, "department": None,
                "template": template_image(observed["template"], target, action["template_principal"]),
                "principal": principal, "template_principal": action["template_principal"]}
    exact(detail, {"response", "authorization"})
    if authorization(target, detail["authorization"]) != principal:
        raise ValueError("部门详情主体不同于目录观察主体")
    if detail["response"].get("code") != 200 or detail["response"].get("data") != item:
        raise ValueError("部门详情与完整列表对象不同")
    return {"directory": directory, "department": item,
            "template": template_image(observed["template"], target, action["template_principal"]),
            "principal": principal, "template_principal": action["template_principal"]}


def after_directory(action, item):
    records = [dict(value) for value in action["before"]["departments"]] + [item]
    records.sort(key=lambda value: int(value["id"]))
    return {"total": action["before"]["total"] + 1, "departments": records}


def classify(action, observed, target):
    current = image(action, observed, target)
    name = action["body"]["name"]
    if (current["department"] is None and current["directory"] == action["before"]
            and current["template"]["paths"] == action["before_template_paths"]
            and name not in current["template"]["paths"]):
        return "before"
    if (current["department"] is not None
            and current["directory"] == after_directory(action, current["department"])
            and current["template"]["paths"] == sorted(action["before_template_paths"] + [name])):
        return "after"
    return "mismatch"


def directories(directory, action):
    root = directory / "seed-runtime/departments" / action["target"]["slot"]
    exact_directory(root)
    children = sorted(root.iterdir()) if root.exists() else []
    if any(not child.is_dir() or child.name != f"{index:04d}" for index, child in enumerate(children, 1)):
        raise ValueError("部门动作目录不连续或含未知文件")
    for child in children:
        exact_directory(child)
    return root, children


def intent(backend, directory, plan, action, attempt):
    value = read_json(attempt / "intent.json")
    expected = {"plan": binding(plan_path(directory)), "target": action["target"],
                "before": binding(attempt / "before.json"), "body": action["body"],
                "operation": "post_system_depts", "idempotency_key": value.get("idempotency_key"),
                "automatic_retry": False}
    if value != expected:
        raise ValueError("部门写入意图不同于固定计划")
    if (not isinstance(value["idempotency_key"], str)
            or not re.fullmatch(r"[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}",
                                value["idempotency_key"])):
        raise ValueError("部门写入意图缺少固定幂等键")
    bound_file(backend, value["before"])
    return value


def reconciliation(backend, directory, plan, action, target, attempt, filename):
    proof = read_json(filename)
    exact(proof, {"intent", "observed", "state"})
    suffix = filename.name.removeprefix("reconcile-")
    observed_path = bound_file(backend, proof["observed"])
    observed = read_json(observed_path)
    state = classify(action, observed, target)
    if (not re.fullmatch(r"[0-9]{4,}\.json", suffix)
            or proof["intent"] != binding(attempt / "intent.json")
            or observed_path != attempt / ("observed-" + suffix)
            or proof["state"] != state or state not in {"before", "after"}):
        raise ValueError("部门核对不属于当前意图的完整前后像")
    intent(backend, directory, plan, action, attempt)
    return state


def action_history(backend, directory, plan, action, target, attempts):
    key = None
    for index, attempt in enumerate(attempts):
        names = {item.name for item in attempt.iterdir()}
        allowed = {"before.json", "intent.json", "api-response.json", "confirmed.json"}
        if any(name not in allowed and not re.fullmatch(r"(?:observed|reconcile)-[0-9]{4,}\.json", name)
               for name in names):
            raise ValueError("部门动作链含未登记证据")
        if "before.json" in names and classify(action, read_json(attempt / "before.json"), target) != "before":
            raise ValueError("部门动作链的 POST 前像不再为空")
        if "intent.json" not in names:
            if names - {"before.json"} or index == len(attempts) - 1:
                raise ValueError("部门动作链有未完成的本地尝试")
            continue
        current = intent(backend, directory, plan, action, attempt)
        if "api-response.json" in names:
            created_response(action, target, read_json(attempt / "api-response.json"))
        key = current["idempotency_key"] if key is None else key
        if current["idempotency_key"] != key:
            raise ValueError("部门重试未复用首个已登记幂等键")
        reconciles = sorted(attempt.glob("reconcile-*.json"))
        states = [reconciliation(backend, directory, plan, action, target, attempt, item)
                  for item in reconciles]
        if "confirmed.json" in names:
            if index != len(attempts) - 1:
                raise ValueError("已确认部门后不得增加后续尝试")
            confirmed = confirmation(backend, directory, plan, action, target, attempt)
            if ("api-response.json" in names) != confirmed["api_response_proven"]:
                raise ValueError("部门成功响应未被唯一确认消费")
        elif "api-response.json" in names:
            raise ValueError("部门成功响应尚未与完整后像确认")
        elif index == len(attempts) - 1 or not states or any(state != "before" for state in states):
            raise ValueError("未确认部门意图未经完整空前像核对")
    if not attempts or not (attempts[-1] / "confirmed.json").exists():
        raise ValueError("部门动作链未以唯一后像确认收尾")
    return binding(attempts[-1] / "confirmed.json")


def confirmation(backend, directory, plan, action, target, attempt):
    value = read_json(attempt / "confirmed.json")
    exact(value, {"plan", "target", "observed", "api_response_proven", "post_attempted", "evidence"})
    if (value["plan"] != binding(plan_path(directory)) or value["target"] != action["target"]
            or type(value["api_response_proven"]) is not bool or value["post_attempted"] is not True
            or classify(action, value["observed"], target) != "after"):
        raise ValueError("部门确认缺少唯一完整后像")
    intent(backend, directory, plan, action, attempt)
    required = {"observed", "intent", "before"}
    required.add("api-response" if value["api_response_proven"] else "reconcile")
    if set(value["evidence"]) != required:
        raise ValueError("部门确认的证据集合不完整")
    names = {"intent": "intent.json", "before": "before.json", "api-response": "api-response.json"}
    for name, descriptor in value["evidence"].items():
        path = bound_file(backend, descriptor)
        expected_name = names.get(name)
        if (path.parent != attempt or (expected_name is not None and path.name != expected_name)
                or (name == "observed" and not re.fullmatch(r"observed-[0-9]{4,}\.json", path.name))
                or (name == "reconcile" and not re.fullmatch(r"reconcile-[0-9]{4,}\.json", path.name))):
            raise ValueError("部门确认引用其他动作证据")
    if read_json(bound_file(backend, value["evidence"]["observed"])) != value["observed"]:
        raise ValueError("部门确认内嵌后像与原始观察不同")
    before = read_json(bound_file(backend, value["evidence"]["before"]))
    if classify(action, before, target) != "before":
        raise ValueError("部门确认缺少原空目录前像")
    if value["api_response_proven"]:
        created = created_response(action, target, read_json(bound_file(backend, value["evidence"]["api-response"])))
        if created != image(action, value["observed"], target)["department"]:
            raise ValueError("部门创建响应与最终唯一对象不同")
    else:
        proof = read_json(bound_file(backend, value["evidence"]["reconcile"]))
        if proof != {"intent": binding(attempt / "intent.json"),
                     "observed": value["evidence"]["observed"], "state": "after"}:
            raise ValueError("未知部门写入缺少当前意图的后像核对")
    return value


def department_evidence(backend, directory, request, *, current=None):
    from devex_clone_seed import latest

    plan = validate_plan(backend, directory, request)
    record = latest(directory, "seed-runtime", MODES, current=current)
    result = read_json(bound_file(backend, record["result"]))
    exact(result, {"status", "plan", "confirmed", "observed", "remote_writes",
                   "worker_authorized", "restore_qualified"})
    source_targets = targets(identity_plan(backend, request))
    confirmed = []
    for action, target in zip(plan["actions"], source_targets):
        _, attempts = directories(directory, action)
        confirmed.append(action_history(backend, directory, plan, action, target, attempts))
    if (record["mode"] != "departments-verify" or result["status"] != "seed_departments_verified"
            or result.get("plan") != binding(plan_path(directory)) or result.get("confirmed") != confirmed
            or not isinstance(result.get("observed"), list) or len(result["observed"]) != 10
            or result["remote_writes"] != 0 or result["worker_authorized"] is not False
            or result["restore_qualified"] is not False):
        raise ValueError("部门必须在最新外层只读验证中全部通过")
    for descriptor, action, target in zip(result["observed"], plan["actions"], source_targets):
        path = bound_file(backend, descriptor)
        if path.parent != directory / "seed-runtime" / f"attempt-{record['number']:04d}" or classify(
                action, read_json(path), target) != "after":
            raise ValueError("部门最终观察不属于当前只读验证或完整后像")
    return {"stage": record["result"], "plan": binding(plan_path(directory)), "confirmed": confirmed,
            "prerequisite": plan["prerequisite"]}


def authorize_identity(backend, directory, request, mode, *, current=None):
    """身份 apply 只从未创建账本开始；verify 只消费完整 622 项稳定投影。"""
    proof = department_evidence(backend, directory, request, current=current)
    saved = read_prerequisite(backend, directory)["identity"]
    if mode == "apply":
        if saved["state"] != "absent" or identity_projection(backend, request, allow_absent=True) != saved:
            raise ValueError("身份 apply 只允许在部门验证后从明确空账本开始，不能重放")
    elif mode == "verify":
        actual = identity_projection(backend, request, allow_absent=False)
        if actual["state"] not in {"prepared", "verifying", "verified"}:
            raise ValueError("身份 verify 需要全部 622 项已确认")
        if saved["state"] == "prepared" and actual["projection_sha256"] != saved["projection_sha256"]:
            raise ValueError("补部门前后的身份稳定投影不同")
    else:
        raise ValueError("未知身份模式")
    return proof
