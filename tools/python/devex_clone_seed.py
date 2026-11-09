"""同一复制 run 的身份准备与新 seed 运行登记；历史数据证明不要求目标永远静止。"""
import json
from pathlib import Path

from devex_clone_capture import read_json
from devex_clone_model import exact, local_path
from devex_clone_post import registration as post_registration
from devex_clone_run_state import binding, load_state
from devex_clone_source_proof import bound_file
from restore_reference_plan import plan_hash

FIELDS = {"format_version", "kind", "run_manifest", "post_copy", "post_verify", "identity_plan",
          "identity_state", "identity_tool", "api_process", "node"}


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
    exact(plan, {"format_version", "kind", "configuration", "environment", "groups", "plan_sha256"})
    payload = {key: value for key, value in plan.items() if key != "plan_sha256"}
    if (plan["format_version"] != 1 or plan["kind"] != "devex-identities"
            or plan_hash(payload) != plan["plan_sha256"]):
        raise ValueError("身份计划原摘要变化")
    environment = plan["environment"]
    active = post_registration(backend, directory, descriptor=request["post_copy"])
    if active.request != post:
        raise ValueError("身份计划引用的 post-copy 登记内容变化")
    runtime = active.runtime
    contract = read_json(runtime / "runtime.json")
    from rate_limit_client import load_app_table
    from devex_clone_target_binding import external_file

    api = load_app_table(backend, {"APP_ENV": "test"}, config_dir=Path(contract["api_config_dir"]))
    external_file(request["identity_tool"])
    if (environment["scope_id"] != api.get("scope_id") or contract["scope_id"] != environment["scope_id"]
            or Path(environment["backend_dir"]) != backend or environment["frontend_dir"] != post["frontend_root"]
            or Path(environment["runtime"]["directory"]) != runtime
            or environment["runtime"]["receipt_sha256"] != binding(runtime / "runtime.json")["sha256"]
            or request["api_process"] != binding(runtime / "api.json")
            or environment["runtime"]["api_process_sha256"] != request["api_process"]["sha256"]):
        raise ValueError("身份计划必须保留原 API 的 scope、配置与创建身份")
    handoff = read_json(runtime / "handoff.json")
    if (environment["api_url"].rstrip("/") != handoff["api_url"].rstrip("/")
            or api["cors"]["allow_origins"] != [environment["frontend_url"].rstrip("/")]):
        raise ValueError("身份计划 API 或前端地址不同")
    document = environment["environment_document"]
    if binding(local_path(backend, document["path"]))["sha256"] != document["sha256"]:
        raise ValueError("身份环境说明变化")
    state = local_path(backend, request["identity_state"])
    if state != directory / "seed-runtime/identities":
        raise ValueError("身份账本必须是本 run 固定 seed 目录")
    private = private_configuration(backend, plan)
    exact(private["passwords"], {"system", "tenant"})
    for password in private["passwords"].values():
        if not isinstance(password, str) or not password or "\0" in password:
            raise ValueError("身份计划缺少明确的文件密码")
    if private["database"]["control"] != {
            key: api["database"]["primary"][key]
            for key in ("host", "port", "username", "password", "database", "tls_mode")}:
        raise ValueError("身份控制库必须与固定文件配置一致")
    return plan, {"APP_ENV": "test"}


def private_configuration(backend, plan):
    """只在内存恢复已绑定私有文件；公开计划不保存任何凭据。"""
    import copy

    descriptor = plan["configuration"]
    path = bound_file(backend, descriptor)
    private = read_json(path)
    public = copy.deepcopy(private)
    public.pop("passwords")
    public["system_admin"].pop("password")
    public["database"]["control"].pop("password")
    for target in public["database"]["targets"]:
        target["connection"].pop("password")
    for tenant in public["tenants"]:
        tenant["admin"].pop("password")
    if public != plan["environment"] or binding(path) != descriptor:
        raise ValueError("身份私有配置文件或非敏感绑定变化")
    return private


def identity_evidence(backend, directory, request):
    return seed_evidence(backend, directory, "identity-evidence", {"request": request})


def seed_evidence(backend, directory, operation, arguments):
    return invoke_seed_tool(backend, "__seed-evidence", {
        "format_version": 1, "kind": "ryframe-seed-evidence", "run_dir": str(directory),
        "operation": operation, "arguments": arguments,
    }, timeout=180)


def invoke_seed_tool(backend, command, value, *, timeout):
    """通过正式工具绑定传递有界 seed 请求，原生端完成校验和执行。"""
    import os
    import subprocess
    from devex_clone_capture import unique_object
    from rate_limit_client import _child_environment
    from rate_limit_tool import tool_binding, verified_tool

    payload = json.dumps(value,
                         ensure_ascii=False, separators=(",", ":")).encode("utf-8")
    if len(payload) > 65536:
        raise ValueError("seed 请求超过 64 KiB")
    options = {"creationflags": subprocess.CREATE_NO_WINDOW} if os.name == "nt" else {}
    try:
        with verified_tool(tool_binding()) as executable:
            if executable is None:
                raise ValueError("seed 核验缺少当前原生工具绑定，禁止临时编译")
            process = subprocess.run([executable, command], cwd=backend,
                env=_child_environment(), input=payload, stdout=subprocess.PIPE,
                stderr=subprocess.DEVNULL, timeout=timeout, check=False, **options)
        if len(process.stdout) > 1024 * 1024:
            raise ValueError("seed 响应超过 1 MiB")
        response = json.loads(process.stdout.decode("utf-8"), object_pairs_hook=unique_object)
    except (OSError, subprocess.SubprocessError, UnicodeError, ValueError):
        raise ValueError("seed 原生请求未完成；保留现场，核对当前绑定工具，禁止自动重放") from None
    if (process.returncode != 0 or not isinstance(response, dict)
            or set(response) != {"ok", "result"} or response["ok"] is not True
            or not isinstance(response["result"], dict)):
        raise ValueError("seed 文件证据无效；保留原始文件与阶段，禁止自动重放")
    return response["result"]


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
