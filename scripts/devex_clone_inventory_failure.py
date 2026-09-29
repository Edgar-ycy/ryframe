"""库存采集中断目录的闭合集与只读失败证据核验。"""
from pathlib import Path
import re

from devex_clone import read_json
from devex_clone_model import exact, linked
from devex_clone_run_state import binding

KEYS = ("shared-control", "shared", "dedicated-a", "dedicated-b")
FAILURE_FIELDS = {"format_version", "status", "stage", "error_type", "remote_writes",
                  "target_ready", "clone_verified"}
STAGES = {"inputs", "binding_before", "inventory_before", "inventory_after",
          "binding_after", "publish"}
_UUID = r"[a-f0-9]{32}"


def _failure(directory: Path) -> None:
    value = read_json(directory / "failure.json")
    allowed = FAILURE_FIELDS | ({"database_verification"}
                                if "database_verification" in value else set())
    exact(value, allowed)
    if (value["format_version"] != 1 or value["status"] != "side_inventory_failed"
            or value["stage"] not in STAGES or not isinstance(value["error_type"], str)
            or not value["error_type"] or value["remote_writes"] != 0
            or value["target_ready"] is not False or value["clone_verified"] is not False
            or "database_verification" in value
            and not isinstance(value["database_verification"], dict)):
        raise ValueError("库存失败收据类型、阶段或只读资格字段无效")


def _raw_target(path: Path, request: dict, key: str) -> None:
    value = read_json(path)
    exact(value, {"scope_id", "control_schema_fingerprint", "tenant_schema_fingerprint", "target"})
    exact(value["target"], {"database", "preserved_tables"})
    database = value["target"]["database"]
    exact(database, {"key", "kind", "server_uuid", "database", "shared", "placements", "tables"})
    declared = next(item for item in request["target"]["databases"] if item["key"] == key)
    if (value["scope_id"] != request["target"]["scope_id"]
            or any(database[field] != declared[field]
                   for field in ("key", "kind", "server_uuid", "database"))
            or database["shared"] is not (declared["mode"] == "shared")
            or not isinstance(database["placements"], list)
            or not isinstance(database["tables"], list)
            or not isinstance(value["target"]["preserved_tables"], list)):
        raise ValueError("库存失败原始目标输出不属于固定四库")


def _command(path: Path, directory: Path) -> str | None:
    value = read_json(path)
    optional = ({"stdout_file"} if "stdout_file" in value else set()) | (
        {"stdout_capture_error"} if "stdout_capture_error" in value else set())
    exact(value, {"command", "returncode", "error_type", "stdin", "stdout", "stderr"} | optional)
    command = value["command"]
    if (not isinstance(command, list) or not command
            or any(not isinstance(item, str) or not item for item in command)
            or value["returncode"] is not None and type(value["returncode"]) is not int
            or value["error_type"] is not None and not isinstance(value["error_type"], str)
            or value["stdin"] is not None and (not isinstance(value["stdin"], dict)
                or set(value["stdin"]) != {"bytes", "sha256"})
            or value["stdout"] is not None and not isinstance(value["stdout"], str)
            or not isinstance(value["stderr"], str)):
        raise ValueError("库存失败命令收据字段或退出状态无效")
    capture_failed = "stdout_capture_error" in value
    if capture_failed and ("stdout_file" not in value
                           or not isinstance(value["stdout_capture_error"], str)
                           or not value["stdout_capture_error"]
                           or value["stdout"] is not None):
        raise ValueError("库存失败 stdout 采集错误字段无效")
    if "stdout_file" not in value:
        return None
    descriptor = value["stdout_file"]
    fields = {"path"} if capture_failed else {"path", "bytes", "sha256"}
    if not isinstance(descriptor, dict) or set(descriptor) != fields \
            or not isinstance(descriptor.get("path"), str):
        raise ValueError("库存失败 stdout 文件描述无效")
    stdout = Path(descriptor["path"])
    if (stdout.parent != directory or not stdout.name or stdout.suffix != ".stdout"
            or linked(stdout) or not stdout.is_file()):
        raise ValueError("库存失败 stdout 文件越出采集目录或名称无效")
    if not capture_failed and binding(stdout) != descriptor:
        raise ValueError("库存失败 stdout 文件绑定变化")
    return stdout.name


def inventory_failure_files(target: Path, relative: str, request: dict,
                            available: set[str]) -> set[str]:
    """验证失败目录内每项内容，并返回必须从待核增量消费的闭合集。"""
    directory = target / relative
    prefix = relative + "/"
    paths = {path for path in available if path.startswith(prefix)}
    if (relative not in available or prefix + "failure.json" not in paths
            or prefix + "inventory.json" in paths):
        raise ValueError("库存失败目录缺少唯一失败收据或错误包含成功收据")
    _failure(directory)
    before, after = [], []
    for phase, result in (("before", before), ("after", after)):
        for key in KEYS:
            name = f"{phase}-target-{key}.json"
            if prefix + name in paths:
                _raw_target(directory / name, request, key)
                result.append(key)
        if result != list(KEYS[:len(result)]):
            raise ValueError("库存失败原始四库输出不是连续固定前缀")
    if after and before != list(KEYS):
        raise ValueError("库存 after 输出不能越过未完成的 before")
    bindings = [name for name in ("binding-before.json", "binding-after.json")
                if prefix + name in paths]
    if bindings:
        first = read_json(directory / bindings[0])
        if bindings == ["binding-after.json"] or len(bindings) == 2 \
                and read_json(directory / bindings[1]) != first:
            raise ValueError("库存失败来源绑定缺失或前后变化")
    known = {"failure.json", *(f"{phase}-target-{key}.json"
             for phase in ("before", "after") for key in KEYS), *bindings}
    stdout_files, stdout_references = set(), []
    for item in sorted(paths):
        name = item.removeprefix(prefix)
        if re.fullmatch(rf"command-{_UUID}\.json", name):
            stdout = _command(directory / name, directory)
            if stdout is not None:
                stdout_references.append(stdout)
        elif "/" not in name and name.endswith(".stdout"):
            stdout_files.add(name)
        elif name not in known:
            raise ValueError("库存失败目录包含未知证据")
    if len(stdout_references) != len(set(stdout_references)):
        raise ValueError("库存失败 stdout 文件被多个命令收据重复引用")
    if stdout_files != set(stdout_references):
        raise ValueError("库存失败 stdout 文件集合与命令收据引用不匹配")
    return paths | {relative}
