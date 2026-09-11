"""恢复运行代次、首次目录 intent 与三进程启动收据的严格模型。"""

from __future__ import annotations

import json
import uuid
from pathlib import Path

from full_stack_process import read_process, write_receipt
from full_stack_process_tree import read_process_tree, validate_process_tree_directory
from restore_runtime_evidence import HEX_64, exact_fields, read_json_document
from restore_runtime_launch import ROLES, commands, validate_request

STATE = "lifecycle.json"
LAUNCH = "runtime-launch.json"
ACTIVE_STATES = {"starting", "running", "stopping", "stop_failed", "cleanup_failed"}
FINAL_STATES = {"stopped", "start_failed"}


def descriptor(document) -> dict:
    return {"path": str(document.path), "bytes": len(document.raw), "sha256": document.sha256}


def creation_path(root: Path) -> Path:
    return root.parent / f"{root.name}.runtime-create.json"


def _write_exclusive(path: Path, value: dict) -> object:
    with path.open("x", encoding="utf-8", newline="\n") as stream:
        json.dump(value, stream, ensure_ascii=False, indent=2)
        stream.write("\n")
    return read_json_document(path)


def validate_creation(value: object, root: Path, binding: dict) -> dict:
    value = exact_fields(
        value,
        {"format_version", "kind", "runtime_directory", "generation", "registration", "request"},
        "恢复运行首次目录 intent",
    )
    if (
        value["format_version"] != 1
        or value["kind"] != "restore-runtime-create-intent"
        or value["runtime_directory"] != str(root)
        or value["generation"] != 1
        or value["registration"] != binding
    ):
        raise ValueError("恢复运行首次目录 intent 与登记或目录不匹配")
    request = validate_request(value["request"])
    if request["registration"] != binding:
        raise ValueError("恢复运行首次目录 intent 的启动请求未绑定同一登记")
    return value


def create_intent(root: Path, binding: dict, request: dict) -> object:
    path = creation_path(root)
    if path.exists():
        raise ValueError("恢复运行首次目录已有未收尾 intent；必须先 recover")
    document = _write_exclusive(
        path,
        {
            "format_version": 1,
            "kind": "restore-runtime-create-intent",
            "runtime_directory": str(root),
            "generation": 1,
            "registration": binding,
            "request": request,
        },
    )
    validate_creation(document.value, root, binding)
    return document


def read_creation(root: Path, binding: dict, expected: dict | None = None):
    document = read_json_document(creation_path(root))
    value = validate_creation(document.value, root, binding)
    if expected is not None and descriptor(document) != expected:
        raise ValueError("恢复运行首次目录 intent 与生命周期描述不一致")
    return document, value


def new_generation(root: Path, number: int, request: dict) -> dict:
    path = root / f"generation-{number:04d}"
    role_commands = commands(request)
    return {
        "number": number,
        "directory": str(path),
        "status": "starting",
        "request": request,
        "roles": {
            role: {
                "state": "pending",
                "operation_id": uuid.uuid4().hex,
                "command": role_commands[role],
                "log": str(path / f"{role}.log"),
                "tree": None,
                "completion": None,
            }
            for role in ROLES
        },
        "launch": None,
        "error_type": None,
    }


def validate_generation(generation: object, root: Path, number: int, binding: dict) -> dict:
    generation = exact_fields(
        generation,
        {"number", "directory", "status", "request", "roles", "launch", "error_type"},
        "恢复运行代次",
    )
    expected = root / f"generation-{number:04d}"
    if (
        generation["number"] != number
        or generation["directory"] != str(expected)
        or generation["status"] not in ACTIVE_STATES | FINAL_STATES
        or generation["request"].get("registration") != binding
        or (
            generation["error_type"] is not None
            and not isinstance(generation["error_type"], str)
        )
    ):
        raise ValueError("恢复运行代次编号、状态或登记绑定无效")
    request = validate_request(generation["request"])
    roles = exact_fields(generation["roles"], set(ROLES), "恢复运行角色")
    expected_commands = commands(request)
    for role, item in roles.items():
        item = exact_fields(
            item,
            {"state", "operation_id", "command", "log", "tree", "completion"},
            f"{role} 运行状态",
        )
        if (
            item["state"] not in {"pending", "running", "stopped"}
            or not isinstance(item["operation_id"], str)
            or len(item["operation_id"]) != 32
            or any(character not in "0123456789abcdef" for character in item["operation_id"])
            or item["command"] != expected_commands[role]
            or item["log"] != str(expected / f"{role}.log")
            or (item["tree"] is not None and not isinstance(item["tree"], dict))
            or (item["completion"] is not None and not isinstance(item["completion"], dict))
        ):
            raise ValueError(f"{role} 运行状态无效")
    files = (
        validate_process_tree_directory(
            expected,
            {role: item["operation_id"] for role, item in roles.items()},
            extra_files=(LAUNCH,),
        )
        if expected.exists()
        else ()
    )
    launch_binding = generation["launch"]
    if launch_binding is not None:
        launch_binding = exact_fields(
            launch_binding, {"path", "bytes", "sha256"}, "恢复运行启动收据描述"
        )
        launch_path = expected / LAUNCH
        if (
            launch_binding["path"] != str(launch_path)
            or type(launch_binding["bytes"]) is not int
            or launch_binding["bytes"] <= 0
            or not isinstance(launch_binding["sha256"], str)
            or HEX_64.fullmatch(launch_binding["sha256"]) is None
        ):
            raise ValueError("恢复运行启动收据描述与当前代次不一致")
        launch = read_json_document(launch_path)
        if descriptor(launch) != launch_binding:
            raise ValueError("恢复运行启动收据在成功登记后发生变化")
        validate_launch(launch.value, expected)
    elif LAUNCH in files:
        interrupted_publication = (
            generation["status"] == "starting"
            and generation["error_type"] is None
            and all(
                item["state"] == "running" and item["tree"] is not None
                for item in roles.values()
            )
        )
        if not interrupted_publication:
            raise ValueError("恢复运行代次存在未登记的启动成功收据")
        validate_launch(read_json_document(expected / LAUNCH).value, expected)
    for role, item in roles.items():
        tree_path = expected / f"{role}-tree.json"
        process_path = expected / f"{role}.json"
        if process_path.exists() and not tree_path.exists():
            raise ValueError(f"{role} 产品进程收据缺少完整树证据")
        if item["tree"] is not None:
            tree = read_process_tree(expected, role, request["authority"]["scope_id"])
            if (
                tree != item["tree"]
                or tree["operation_id"] != item["operation_id"]
                or not process_path.is_file()
                or read_process(expected, role, request["authority"]["scope_id"])
                != tree["process"]
                or not (expected / f"{role}.log").is_file()
            ):
                raise ValueError(f"{role} 运行状态没有绑定当前进程树文件")
        completion = item["completion"]
        if completion is not None:
            completion = exact_fields(
                completion, {"path", "bytes", "sha256"}, f"{role} 完整树关闭证明"
            )
            stopped = expected / f"{role}-members-{item['operation_id']}-stopped.json"
            proof = read_json_document(stopped)
            if completion != descriptor(proof):
                raise ValueError(f"{role} 完整树关闭证明与当前 operation 不一致")
    if generation["status"] == "running" and launch_binding is None:
        raise ValueError("运行代次缺少成功启动收据")
    return generation


def state_document(root: Path):
    path = root / STATE
    return read_json_document(path) if path.exists() else None


def _missing_latest_directory(state: dict, root: Path) -> str | None:
    generation = state["generations"][-1]
    roles = generation["roles"].values()
    if (
        generation["status"] == "starting"
        and generation["launch"] is None
        and generation["error_type"] is None
        and all(item["state"] == "pending" and item["tree"] is None for item in roles)
    ):
        return Path(generation["directory"]).name
    return None


def validate_state(value: object, root: Path, binding: dict) -> dict:
    state = exact_fields(
        value,
        {
            "format_version",
            "kind",
            "runtime_directory",
            "registration",
            "creation_intent",
            "generations",
        },
        "恢复运行生命周期",
    )
    if (
        state["format_version"] != 1
        or state["kind"] != "restore-runtime-lifecycle"
        or state["runtime_directory"] != str(root)
        or state["registration"] != binding
        or not isinstance(state["generations"], list)
        or not state["generations"]
    ):
        raise ValueError("恢复运行生命周期与当前目录或登记不匹配")
    read_creation(root, binding, state["creation_intent"])
    for number, generation in enumerate(state["generations"], 1):
        validate_generation(generation, root, number, binding)
    expected = {STATE} | {
        f"generation-{number:04d}" for number in range(1, len(state["generations"]) + 1)
    }
    actual = {entry.name for entry in root.iterdir()} if root.is_dir() else set()
    missing = _missing_latest_directory(state, root)
    if missing is not None and missing not in actual:
        expected.remove(missing)
    if not root.is_dir() or actual != expected:
        raise ValueError("恢复运行目录包含缺失、额外或未知的生命周期文件")
    return state


def save_state(root: Path, state: dict, binding: dict, previous) -> object:
    path = root / STATE
    if previous is None:
        if path.exists():
            raise ValueError("恢复运行生命周期在创建前被替换")
    else:
        previous.assert_unchanged()
    write_receipt(path, state)
    document = read_json_document(path)
    validate_state(document.value, root, binding)
    return document


def initial_state(root: Path, binding: dict, creation: object, request: dict) -> dict:
    return {
        "format_version": 1,
        "kind": "restore-runtime-lifecycle",
        "runtime_directory": str(root),
        "registration": binding,
        "creation_intent": descriptor(creation),
        "generations": [new_generation(root, 1, request)],
    }


def process_binding(path: Path, role: str, scope: str) -> dict:
    document = read_json_document(path / f"{role}.json")
    identity = read_process(path, role, scope)
    return {
        "path": str(document.path),
        "bytes": len(document.raw),
        "sha256": document.sha256,
        "identity": identity,
    }


def validate_launch(value: object, generation: Path) -> dict:
    launch = exact_fields(
        value,
        {"format_version", "kind", "generation", "runtime_directory", "request", "processes"},
        "恢复运行启动收据",
    )
    if (
        launch["format_version"] != 1
        or launch["kind"] != "restore-runtime-launch"
        or launch["runtime_directory"] != str(generation)
        or type(launch["generation"]) is not int
        or launch["generation"] <= 0
    ):
        raise ValueError("恢复运行启动收据版本、代次或目录不匹配")
    request = validate_request(launch["request"])
    processes = exact_fields(launch["processes"], set(ROLES), "恢复运行启动进程")
    expected_commands = commands(request)
    for role, item in processes.items():
        item = exact_fields(
            item, {"tree", "command", "log", "process_receipt"}, f"{role} 启动进程"
        )
        binding = exact_fields(
            item["process_receipt"], {"path", "bytes", "sha256", "identity"}, f"{role} 进程收据"
        )
        if (
            not isinstance(item["tree"], dict)
            or item["command"] != expected_commands[role]
            or item["log"] != str(generation / f"{role}.log")
            or binding["path"] != str(generation / f"{role}.json")
            or type(binding["bytes"]) is not int
            or binding["bytes"] <= 0
            or not isinstance(binding["sha256"], str)
            or HEX_64.fullmatch(binding["sha256"]) is None
            or not isinstance(binding["identity"], dict)
        ):
            raise ValueError(f"{role} 启动收据与请求不匹配")
        process_document = read_json_document(Path(binding["path"]))
        identity = read_process(generation, role, request["authority"]["scope_id"])
        tree = read_process_tree(generation, role, request["authority"]["scope_id"])
        if (
            binding
            != {
                "path": str(process_document.path),
                "bytes": len(process_document.raw),
                "sha256": process_document.sha256,
                "identity": identity,
            }
            or tree != item["tree"]
            or tree["process"] != identity
            or Path(identity["executable"]) != Path(item["command"][0])
        ):
            raise ValueError(f"{role} 启动收据没有绑定当前完整进程树")
    return launch


def write_launch(generation: dict) -> dict:
    path = Path(generation["directory"])
    value = {
        "format_version": 1,
        "kind": "restore-runtime-launch",
        "generation": generation["number"],
        "runtime_directory": str(path),
        "request": generation["request"],
        "processes": {
            role: {
                "tree": generation["roles"][role]["tree"],
                "command": generation["roles"][role]["command"],
                "log": generation["roles"][role]["log"],
                "process_receipt": process_binding(
                    path, role, generation["request"]["authority"]["scope_id"]
                ),
            }
            for role in ROLES
        },
    }
    validate_launch(value, path)
    output = path / LAUNCH
    if output.exists():
        raise ValueError("恢复运行启动成功收据已经存在")
    return descriptor(_write_exclusive(output, value))
