"""将既有启动请求接到原生代次模型；不保留状态判定或发布的 Python 实现。"""

from __future__ import annotations

import subprocess
from pathlib import Path

from native_s3_client import _invoke, _read_response
from rate_limit_tool import tool_binding
from restore_runtime_evidence import read_json_document
from restore_runtime_launch import commands, validate_request

STATE = "lifecycle.json"
LAUNCH = "runtime-launch.json"
FINAL_STATES = {"stopped", "start_failed"}


def descriptor(document) -> dict:
    return {"path": str(document.path), "bytes": len(document.raw), "sha256": document.sha256}


def creation_path(root: Path) -> Path:
    return root.parent / f"{root.name}.runtime-create.json"


def _commands(request: object) -> dict:
    return commands(validate_request(request))


def _native(action, root, binding, value=None, *, previous=None, role_commands=None, number=1):
    if tool_binding() is None:
        raise ValueError("恢复代次模型缺少协调器工具绑定；请通过正式 xtask 入口执行")
    write = action in {"create-intent", "save-state", "write-launch"}
    result, response = _invoke("__source-evidence", {
        "operation": "runtime-generation", "write": write,
        "arguments": {"action": action, "root": str(root), "binding": binding,
                      "value": value, "previous": previous,
                      "commands": role_commands, "number": number},
    }, subprocess.run, may_write=False)
    return _read_response(result, response)[0]["value"]


def validate_creation(value, root, binding):
    validate_request(value.get("request"))
    return _native("validate-creation", root, binding, value)


def create_intent(root, binding, request):
    validate_request(request)
    published = _native("create-intent", root, binding, request)
    document = read_json_document(creation_path(root))
    if descriptor(document) != published:
        raise ValueError("首次运行 intent 在原生发布后发生变化；保留现场，禁止重放")
    return document


def read_creation(root, binding, expected=None):
    document = read_json_document(creation_path(root))
    validate_request(document.value.get("request"))
    value = _native("read-creation", root, binding, previous=expected)
    document.assert_unchanged()
    if value != document.value:
        raise ValueError("首次运行 intent 的原生读取与冻结文档不一致")
    return document, value


def new_generation(root, number, request):
    return _native("new-generation", root, request["registration"], request,
                   number=number, role_commands=_commands(request))


def validate_generation(generation, root, number, binding):
    return _native("validate-generation", root, binding, generation, number=number,
                   role_commands=_commands(generation.get("request")))


def state_document(root):
    path = root / STATE
    return read_json_document(path) if path.exists() else None


def validate_state(value, root, binding):
    role_commands = [_commands(item.get("request")) for item in value.get("generations", [])]
    return _native("validate-state", root, binding, value, role_commands=role_commands)


def save_state(root, state, binding, previous):
    if previous is not None:
        previous.assert_unchanged()
    published = _native("save-state", root, binding, state,
            previous=None if previous is None else descriptor(previous),
            role_commands=[_commands(item.get("request")) for item in state["generations"]])
    document = read_json_document(root / STATE)
    if descriptor(document) != published:
        raise ValueError("运行代次状态在原生发布后发生变化；保留现场，禁止重放")
    return document


def initial_state(root, binding, creation, request):
    creation.assert_unchanged()
    return _native("initial-state", root, binding, request, previous=descriptor(creation),
                   role_commands=_commands(request))


def validate_launch(value, generation):
    request = validate_request(value.get("request"))
    return _native("validate-launch", generation, request["registration"], value,
                   role_commands=_commands(request))


def write_launch(generation):
    request = generation["request"]
    return _native("write-launch", Path(generation["directory"]), request["registration"], generation,
                   role_commands=_commands(request))
