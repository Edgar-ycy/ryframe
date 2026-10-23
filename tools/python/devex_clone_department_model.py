"""固定 seed 部门计划、逐租户确认和只读发布证据。"""
import re
from pathlib import Path
from workspace_directory import WorkspaceDirectory

from devex_clone_capture import write_json
from devex_clone_model import local_path
from devex_clone_post_model import exact_directory
from devex_clone_run_state import binding


MODES = {"departments-plan", "departments-apply", "departments-reconcile", "departments-verify"}
TEMPLATE_PERMISSION = "system:user-import:add"


def snowflake(value):
    if (not isinstance(value, str) or not re.fullmatch(r"[1-9][0-9]{0,18}", value)
            or int(value) > 9223372036854775807):
        raise ValueError("部门准备资源必须返回明确字符串 ID")
    return value


def native_evidence(backend, directory, arguments):
    import subprocess
    from native_s3_client import _invoke

    payload = {"format_version": 1, "kind": "ryframe-seed-evidence", "run_dir": str(directory),
               "operation": "department-guard",
               "arguments": {"backend_root": str(backend), "parameters": arguments}}
    try:
        process, response = _invoke("__seed-evidence", payload, subprocess.run, may_write=False)
    except RuntimeError:
        raise ValueError("部门原生证据核验未完成；保留文件、锁和未知写入现场，核对绑定工具后显式恢复") from None
    if (process.returncode != 0 or not isinstance(response, dict)
            or set(response) != {"ok", "result"} or response["ok"] is not True):
        raise ValueError("部门文件证据、只读闭集或身份账本无效；保留现场，禁止自动重放")
    return response["result"]


def identity_projection(backend, request, *, allow_absent):
    root = local_path(backend, request["identity_state"])
    return native_evidence(backend, root.parent.parent, {"operation": "identity-projection",
        "request": {key: request[key] for key in ("identity_plan", "identity_state")},
        "allow_absent": allow_absent})


def prerequisite_path(directory):
    return directory / "seed-runtime/departments/identity-prerequisite.json"


def plan_path(directory):
    return directory / "seed-runtime/department-plan.json"


def principal(identity):
    return {key: identity[key] for key in ("subject_id", "tenant_id", "username")}


def directories(directory, action):
    root = directory / "seed-runtime/departments" / action["target"]["slot"]
    exact_directory(root)
    children = sorted(root.iterdir()) if root.exists() else []
    if any(not child.is_dir() or child.name != f"{index:04d}" for index, child in enumerate(children, 1)):
        raise ValueError("部门动作目录不连续或含未知文件")
    for child in children:
        exact_directory(child)
    return root, children


def _native_model(method, arguments, *, backend=None, directory=None):
    backend = Path(__file__).resolve().parents[2] if backend is None else backend
    if directory is None:
        raise ValueError("部门模型必须绑定明确运行目录")
    temporary_root = local_path(backend, str(backend / ".local-tests/tmp"))
    exact_directory(temporary_root)
    with WorkspaceDirectory(prefix="department-model-", dir=temporary_root) as temporary:
        path = Path(temporary) / "input.json"
        write_json(path, arguments)
        return native_evidence(backend, directory, {
            "operation": "model", "method": method, "input": binding(path)})


def page_data(response, *, directory):
    return _native_model("page-data", {"response": response}, directory=directory)


def authorization(target, value, *, directory, write=False):
    return _native_model("authorization", {"target": target, "value": value, "write": write}, directory=directory)


def template_authorization(target, value, expected, *, directory):
    return _native_model("template-authorization", {"target": target, "value": value, "principal": expected}, directory=directory)


def created_response(action, target, value, *, directory):
    return _native_model("created-response", {"action": action, "target": target, "value": value}, directory=directory)


def image(action, observed, target, *, directory):
    return _native_model("image", {"action": action, "observed": observed, "target": target}, directory=directory)


def classify(action, observed, target, *, directory):
    return _native_model("classify", {"action": action, "observed": observed, "target": target}, directory=directory)


def intent(backend, directory, plan, action, attempt):
    return _native_model("intent", {"plan": plan, "action": action, "attempt": str(attempt)},
                         backend=backend, directory=directory)


def reconciliation(backend, directory, plan, action, target, attempt, filename):
    return _native_model("reconciliation", {"plan": plan, "action": action, "target": target,
        "attempt": str(attempt), "filename": str(filename)}, backend=backend, directory=directory)


def action_history(backend, directory, plan, action, target, attempts):
    return _native_model("action-history", {"plan": plan, "action": action, "target": target,
        "attempts": [str(path) for path in attempts]}, backend=backend, directory=directory)


def confirmation(backend, directory, plan, action, target, attempt):
    return _native_model("confirmation", {"plan": plan, "action": action, "target": target,
        "attempt": str(attempt)}, backend=backend, directory=directory)


def _identity_request(request):
    return {key: request[key] for key in ("identity_plan", "identity_state")}


def identity_plan(backend, request):
    return _native_model("identity-plan", {"request": {"identity_plan": request["identity_plan"]}},
                         backend=backend, directory=Path(request["identity_plan"]["path"]).parent)


def targets(plan):
    return _native_model("targets", {"plan": plan}, directory=Path(plan["configuration"]["path"]).parent)


def template_identities(backend, request):
    root = local_path(backend, request["identity_state"])
    return _native_model("template-identities", {"request": _identity_request(request)},
                         backend=backend, directory=root.parent.parent)


def read_prerequisite(backend, directory):
    return _native_model("read-prerequisite", {}, backend=backend, directory=directory)


def require_prerequisite(backend, directory, request, value):
    return _native_model("require-prerequisite", {"request": _identity_request(request), "value": value},
                         backend=backend, directory=directory)


def validate_plan(backend, directory, request):
    return _native_model("validate-plan", {"request": _identity_request(request)},
                         backend=backend, directory=directory)


def department_evidence(backend, directory, request, *, current=None):
    from devex_clone_seed import latest
    record = latest(directory, "seed-runtime", MODES, current=current)
    return _native_model("department-evidence", {"request": _identity_request(request), "record": record},
                         backend=backend, directory=directory)


def authorize_identity(backend, directory, request, mode, *, current=None):
    from devex_clone_seed import latest
    record = latest(directory, "seed-runtime", MODES, current=current)
    return _native_model("authorize-identity", {"request": _identity_request(request),
        "record": record, "mode": mode}, backend=backend, directory=directory)
