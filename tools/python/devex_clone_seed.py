"""同一复制 run 的身份准备与新 seed 运行登记；历史数据证明不要求目标永远静止。"""
import hashlib
import json
from pathlib import Path
import re

from devex_clone_capture import read_json, write_json
from devex_clone_model import exact, linked, local_path
from devex_clone_post import registration as post_registration
from devex_clone_post_model import exact_directory
from devex_clone_run_state import binding, load_state
from devex_clone_source_proof import bound_file
from restore_reference_plan import plan_hash

FIELDS = {"format_version", "kind", "run_manifest", "post_copy", "post_verify", "identity_plan",
          "identity_state", "identity_environment", "api_process", "node"}


def password_names(value):
    """仅投影计划明确声明的密码变量名，不提供宽泛环境扩展口。"""
    names = set()
    if isinstance(value, dict):
        for key, item in value.items():
            if key == "password_env" and isinstance(item, str):
                names.add(item)
            else:
                names.update(password_names(item))
    elif isinstance(value, list):
        for item in value:
            names.update(password_names(item))
    return names


def latest(directory, stage, modes, *, current=None, cleanup=False):
    records = [item for item in load_state(directory, verify_results=not cleanup)["attempts"]
               if item["stage"] == stage and item["mode"] in modes and item["number"] != current]
    if cleanup:
        records = [item for item in records if item["status"] == "passed"]
    if not records or records[-1]["status"] != "passed":
        raise ValueError("最新必需阶段尚未通过外层来源与清理核验")
    return records[-1]


def registered(backend, directory, *, cleanup=False):
    record = latest(directory, "seed-runtime", {"register"}, cleanup=cleanup)
    value = read_json(bound_file(backend, record["result"]))
    path = directory / "seed-runtime.json"
    if value.get("status") != "seed_runtime_registered" or value.get("registration") != binding(path):
        raise ValueError("seed 登记未绑定已发布的原始文件")
    request = read_json(path)
    exact(request, FIELDS)
    if (request["format_version"] != 1 or request["kind"] != "devex-clone-seed-runtime"
            or request["run_manifest"] != binding(directory / "manifest.json")):
        raise ValueError("seed 登记不属于当前 run")
    return request


def history(backend, directory, request):
    """验证历史成功文件和最新发布状态，不读取目标业务全库行像。"""
    exact(request, FIELDS)
    if (request["format_version"] != 1 or request["kind"] != "devex-clone-seed-runtime"
            or request["run_manifest"] != binding(directory / "manifest.json")):
        raise ValueError("seed 必须绑定同 run 的固定 post-copy")
    active = post_registration(backend, directory)
    if request["post_copy"] != active.descriptor:
        raise ValueError("seed 必须绑定当前 post-copy 登记代次")
    post = active.request
    record = latest(directory, "post-copy", {"register", "prepare", "schedules", "reconcile", "verify"})
    result = read_json(bound_file(backend, record["result"]))
    if (record["mode"] != "verify" or record["result"] != request["post_verify"]
            or result.get("status") != "post_copy_existing_data_verified"
            or result.get("registration") != request["post_copy"]
            or result.get("worker_must_remain_stopped") is not True):
        raise ValueError("seed 必须取得最新同 run 的真实业务读取成功结果")
    copy_record = latest(directory, "copy", {"run", "resume", "reconcile"})
    if copy_record["result"] != post["copy_stage_receipt"]:
        raise ValueError("登记后复制阶段或账本发生变化")
    for key in ("copy_stage_receipt", "copy_result", "ledger_head", "backend_build", "api_environment"):
        bound_file(backend, post[key])
    if request["node"] != post["node"]:
        raise ValueError("身份 Node 必须复用固定 post-copy 工具")
    return post


def identity_inputs(backend, directory, request, post):
    path = bound_file(backend, request["identity_plan"])
    plan = read_json(path)
    exact(plan, {"format_version", "kind", "environment", "groups", "plan_sha256"})
    payload = {key: value for key, value in plan.items() if key != "plan_sha256"}
    if (plan["format_version"] != 1 or plan["kind"] != "devex-identities"
            or plan_hash(payload) != plan["plan_sha256"]):
        raise ValueError("身份计划原摘要变化")
    environment = plan["environment"]
    api = read_json(bound_file(backend, post["api_environment"]))["environment"]
    active = post_registration(backend, directory, descriptor=request["post_copy"])
    if active.request != post:
        raise ValueError("身份计划引用的 post-copy 登记内容变化")
    runtime = active.runtime
    contract = read_json(runtime / "runtime.json")
    if (environment["scope_id"] != api.get("APP_SCOPE_ID") or contract["scope_id"] != environment["scope_id"]
            or Path(environment["backend_dir"]) != backend or environment["frontend_dir"] != post["frontend_root"]
            or Path(environment["runtime"]["directory"]) != runtime
            or environment["runtime"]["receipt_sha256"] != binding(runtime / "runtime.json")["sha256"]
            or request["api_process"] != binding(runtime / "api.json")
            or environment["runtime"]["api_process_sha256"] != request["api_process"]["sha256"]):
        raise ValueError("身份计划必须保留原 API 的 scope、配置与创建身份")
    handoff = read_json(runtime / "handoff.json")
    if (environment["api_url"].rstrip("/") != handoff["api_url"].rstrip("/")
            or api["APP_CORS_ALLOW_ORIGINS"] != environment["frontend_url"].rstrip("/")):
        raise ValueError("身份计划 API 或前端地址不同")
    document = environment["environment_document"]
    if binding(local_path(backend, document["path"]))["sha256"] != document["sha256"]:
        raise ValueError("身份环境说明变化")
    state = local_path(backend, request["identity_state"])
    if state != directory / "seed-runtime/identities":
        raise ValueError("身份账本必须是本 run 固定 seed 目录")
    private = read_json(bound_file(backend, request["identity_environment"]))
    exact(private, {"environment"})
    private = private["environment"]
    if not isinstance(private, dict) or any(not isinstance(k, str) or not isinstance(v, str) for k, v in private.items()):
        raise ValueError("身份私有环境必须是文本映射")
    secrets = {environment["passwords"][key] for key in ("system_env", "tenant_env")}
    secrets.update(password_names(plan))
    if (any(private.get(k) != v for k, v in api.items()) or set(private) - set(api) - secrets
            or any(not re.fullmatch(r"[A-Z][A-Z0-9_]*", key) or not private.get(key) for key in secrets)
            or any(key.startswith("APP_") for key in set(private) - set(api))):
        raise ValueError("身份环境仅可补充计划明确秘密，不能改变原配置或借 shell 环境")
    return plan, private


def register(backend, directory, request_file):
    path = local_path(backend, str(request_file))
    before = binding(path)
    request = read_json(path)
    post = history(backend, directory, request)
    identity_inputs(backend, directory, request, post)
    if binding(path) != before:
        raise ValueError("seed 请求在核验期间变化")
    destination = directory / "seed-runtime.json"
    if destination.exists() or linked(destination):
        if linked(destination) or read_json(destination) != request:
            raise ValueError("已有 seed 登记不同，不能覆盖")
    else:
        write_json(destination, request)
    return {"status": "seed_runtime_registered", "registration": binding(destination),
            "worker_authorized": False, "remote_writes": 0, "restore_qualified": False}


def _publication_names(plan_sha256, root):
    seed = list(hashlib.sha256(
        plan_sha256.encode("utf-8") + b"\0" + str(root.resolve()).encode("utf-8")).hexdigest()[:32])
    seed[12], seed[16] = "5", "8"
    identifier = "-".join(("".join(seed[:8]), "".join(seed[8:12]), "".join(seed[12:16]),
                           "".join(seed[16:20]), "".join(seed[20:])))
    return f"identity-publication-{identifier}.json", f"verified-{identifier}.json"


def _encoded_sha256(value):
    content = json.dumps(value, ensure_ascii=False, indent=2, separators=(",", ": ")) + "\n"
    return hashlib.sha256(content.encode("utf-8")).hexdigest()


def _expected_identities(plan, ledger):
    from devex_clone_department_model import snowflake

    identities = []
    for group in plan["groups"]:
        permissions = group.get("permissions")
        if (not isinstance(group.get("role_code"), str) or not group["role_code"]
                or not isinstance(permissions, list)
                or any(not isinstance(item, str) or not item for item in permissions)
                or len(set(permissions)) != len(permissions)):
            raise ValueError("身份计划角色或权限不能形成固定验证投影")
        for user in group["users"]:
            exact(user, {"user_slot", "username", "password_env", "client_address"})
            identities.append({"tenant_slot": group["slot"], "user_slot": user["user_slot"],
                "tenant_id": group["tenant_id"], "username": user["username"],
                "client_address": user["client_address"], "password_env": user["password_env"],
                "user_id": snowflake(ledger["users"][user["user_slot"]]),
                "roles_sha256": plan_hash([group["role_code"]]),
                "permissions_sha256": plan_hash(sorted(permissions))})
    if len(identities) != 200:
        raise ValueError("身份验证投影不是固定 200 个用户")
    return identities


def _expected_pools(identities):
    system = [item for item in identities if item["tenant_slot"] == "system"]
    tenants = sorted((item for item in identities if item["tenant_slot"] != "system"),
                     key=lambda item: (item["user_slot"][-3:], item["tenant_slot"]))
    contract, bindings = {}, {}
    for name, selected in (("system", system), ("tenants", tenants)):
        if (len(selected) != 100 or len({item["roles_sha256"] for item in selected}) != 1
                or len({item["permissions_sha256"] for item in selected}) != 1):
            raise ValueError("身份准备池数量或授权摘要不一致")
        selections = {}
        for count in (10, 50, 100):
            distribution = {}
            for item in selected[:count]:
                slot = item["tenant_slot"]
                distribution[slot] = distribution.get(slot, 0) + 1
            selections[str(count)] = distribution
        contract[name] = {"roles_sha256": selected[0]["roles_sha256"],
            "permissions_sha256": selected[0]["permissions_sha256"],
            "client_address_model": "fixed-benchmark-per-user",
            "slots": [{key: item[key] for key in ("tenant_slot", "user_slot", "client_address")}
                      for item in selected], "selections": selections}
        bindings[name] = [{key: item[key] for key in ("tenant_id", "username", "password_env")}
                          for item in selected]
    return {"contract": contract, "bindings": bindings}


def _verified_receipt(backend, root, plan, ledger, receipt):
    fields = {"format_version", "plan_sha256", "scope_id", "status", "identities",
              "identity_pools", "templates", "message_audience", "audience_sha256",
              "homepage_user_slots"}
    exact(receipt, fields)
    identities = _expected_identities(plan, ledger)
    for item in receipt["identities"] if isinstance(receipt["identities"], list) else ():
        exact(item, {"tenant_slot", "user_slot", "tenant_id", "username", "client_address",
                     "password_env", "user_id", "roles_sha256", "permissions_sha256"})
    audience = [{"kind": "user", "target_id": item["user_id"]}
                for item in identities if item["tenant_slot"] == "system"][:10]
    homepage = [item["user_slot"] for item in identities
                if item["tenant_slot"] != "system" and item["user_slot"].endswith("-001")]
    if (receipt["format_version"] != 1 or receipt["plan_sha256"] != plan["plan_sha256"]
            or receipt["scope_id"] != plan["environment"]["scope_id"]
            or receipt["status"] != "verified" or receipt["identities"] != identities
            or receipt["identity_pools"] != _expected_pools(identities)
            or receipt["message_audience"] != audience
            or receipt["audience_sha256"] != plan_hash(audience)
            or receipt["homepage_user_slots"] != homepage):
        raise ValueError("身份验证收据投影与完整计划和账本不一致")
    groups = [group for group in plan["groups"] if group.get("kind") == "tenant"]
    if len(groups) != 10 or not isinstance(receipt["templates"], list) or len(receipt["templates"]) != 10:
        raise ValueError("身份验证收据缺少十租户模板")
    templates, paths, departments = [], set(), set()
    for group, item in zip(groups, receipt["templates"]):
        exact(item, {"tenant_slot", "path", "template_sha256", "department_path",
                     "department_sha256"})
        path = local_path(backend, item["path"])
        name = re.fullmatch(
            rf"template-{re.escape(group['slot'])}-[0-9a-f]{{8}}-[0-9a-f]{{4}}-4[0-9a-f]{{3}}-"
            rf"[89ab][0-9a-f]{{3}}-[0-9a-f]{{12}}\.xlsx", path.name)
        department = item["department_path"]
        if (item["tenant_slot"] != group["slot"] or str(path.resolve()) != item["path"]
                or path.parent != root or name is None or not isinstance(department, str) or not department
                or item["department_sha256"] != hashlib.sha256(department.encode("utf-8")).hexdigest()
                or binding(path)["sha256"] != item["template_sha256"]):
            raise ValueError("身份模板未绑定当前根目录、租户、部门路径或实物摘要")
        paths.add(item["path"])
        departments.add(department)
        templates.append(binding(path))
    if len(paths) != 10 or len(departments) != 1:
        raise ValueError("身份模板路径或完整部门路径不唯一")
    return templates


def identity_evidence(backend, directory, request):
    from devex_clone_department_model import identity_plan, identity_projection

    plan = identity_plan(backend, request)
    root = local_path(backend, request["identity_state"])
    exact_directory(root)
    projection = identity_projection(backend, request, allow_absent=False)
    if (projection != {**projection, "state": "verified", "identity_state": str(root),
                       "plan_sha256": plan["plan_sha256"], "entries": 622, "roles": 11,
                       "users": 200}):
        raise ValueError("身份账本不是严格有序的完整 622 项 verified 投影")
    ledger_binding = projection["ledger"]
    ledger = read_json(bound_file(backend, ledger_binding))
    intent_name, receipt_name = _publication_names(plan["plan_sha256"], root)
    publication = sorted(item.name for item in root.iterdir()
                         if item.name.lower().startswith("identity-publication-")
                         or item.name.lower().startswith("verified-"))
    if publication != sorted((intent_name, receipt_name)) or ledger["verified_receipt"] != receipt_name:
        raise ValueError("身份发布必须只有确定性 intent 和 verified 收据且不能残留 pending")
    intent_path, receipt_path = root / intent_name, root / receipt_name
    if linked(intent_path) or linked(receipt_path):
        raise ValueError("身份 publication 证据不能是链接")
    intent_binding, receipt_binding = binding(intent_path), binding(receipt_path)
    intent, receipt = read_json(intent_path), read_json(receipt_path)
    exact(intent, {"format_version", "kind", "plan_sha256", "identity_state", "publication_nonce",
                   "ledger_sha256", "receipt_file", "receipt_sha256", "receipt"})
    prepared = {"format_version": ledger["format_version"], "plan_sha256": ledger["plan_sha256"],
                "status": "prepared", "entries": ledger["entries"], "roles": ledger["roles"],
                "users": ledger["users"]}
    if (intent["format_version"] != 1 or intent["kind"] != "devex-identity-verification-publication"
            or intent["plan_sha256"] != plan["plan_sha256"]
            or intent["identity_state"] != str(root.resolve())
            or intent["publication_nonce"] != ledger["publication_nonce"]
            or intent["ledger_sha256"] != _encoded_sha256(prepared)
            or intent["receipt_file"] != receipt_name
            or intent["receipt_sha256"] != receipt_binding["sha256"] or intent["receipt"] != receipt):
        raise ValueError("身份 publication intent 未完整绑定计划、根目录、账本和收据")
    templates = _verified_receipt(backend, root, plan, ledger, receipt)
    if (binding(root / "ledger.json") != ledger_binding or binding(intent_path) != intent_binding
            or binding(receipt_path) != receipt_binding
            or publication != sorted(item.name for item in root.iterdir()
                                     if item.name.lower().startswith("identity-publication-")
                                     or item.name.lower().startswith("verified-"))
            or templates != [binding(local_path(backend, item["path"])) for item in receipt["templates"]]):
        raise ValueError("身份账本、publication 或模板在读取期间变化")
    return {"ledger": ledger_binding, "publication_intent": intent_binding,
            "verified": receipt_binding, "templates": templates}


def verified_identity_stage(backend, directory, request, *, current=None):
    from devex_clone_department_model import department_evidence

    record = latest(directory, "seed-runtime", {"identities-apply", "identities-verify"}, current=current)
    result = read_json(bound_file(backend, record["result"]))
    evidence = identity_evidence(backend, directory, request)
    departments = department_evidence(backend, directory, request, current=current)
    if (record["mode"] != "identities-verify" or result.get("status") != "seed_identities_verified"
            or result.get("registration") != binding(directory / "seed-runtime.json")
            or result.get("identity") != evidence or result.get("departments") != departments):
        raise ValueError("必须取得当前同账本 identities-verify 的外层通过结果")
    return {"stage": record["result"], "identity": evidence, "producer": result["producer"],
            "departments": departments}
