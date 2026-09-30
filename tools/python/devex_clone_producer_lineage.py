"""汇总已登记运行代次和 Node 生产者的不可变创建身份历史。"""

from __future__ import annotations

from pathlib import Path
from pathlib import PurePosixPath
import re
from urllib.parse import urlsplit

from devex_clone_capture import read_json
from devex_clone_model import digest, exact, linked
from devex_clone_post import registration as post_registration
from devex_clone_post_process import (
    IDENTITY_RELEASE_ID,
    KINDS,
    PHASES,
    historical_producer_request,
    producer_command,
)
from devex_clone_run_state import STAGES, binding, load_state
from restore_build import file_digest
from restore_reference_plan import plan_hash

ATTEMPT_FIELDS = {
    "number", "stage", "mode", "started_at", "finished_at", "status", "sources", "result", "error_type",
}
IDENTITY_FIELDS = {"pid", "started", "executable"}
ROLES = {"api", "worker"}


def _linked(path: Path) -> bool:
    return any(linked(item) for item in (path, *path.parents))


def _directory(path: Path, parent: Path, label: str) -> Path:
    if (not path.is_absolute() or path == parent or not path.is_relative_to(parent)
            or _linked(path) or not path.is_dir()):
        raise ValueError(f"{label}必须是明确范围内的无链接目录")
    return path.resolve(strict=True)


def _regular(path: Path, parent: Path, label: str, *, min_bytes: int = 1,
             max_bytes: int = 16 * 1024 * 1024) -> Path:
    if (not path.is_absolute() or not path.is_relative_to(parent) or _linked(path)
            or not path.is_file() or path.stat().st_size < min_bytes or path.stat().st_size > max_bytes):
        raise ValueError(f"{label}必须是明确范围内的普通文件")
    return path.resolve(strict=True)


def _bound(path: Path, parent: Path, label: str, expected: dict | None = None) -> dict:
    path = _regular(path, parent, label)
    actual = binding(path)
    if expected is not None:
        exact(expected, {"path", "bytes", "sha256"})
        digest(expected["sha256"])
        if expected != actual:
            raise ValueError(f"{label}绑定已变化")
    return actual


def _binding_path(value: dict, label: str) -> Path:
    exact(value, {"path", "bytes", "sha256"})
    if not isinstance(value["path"], str):
        raise ValueError(f"{label}路径无效")
    return Path(value["path"])


def _identity(value: dict, label: str, *, executable: Path | None = None) -> dict:
    exact(value, IDENTITY_FIELDS)
    path = Path(value["executable"]) if isinstance(value["executable"], str) else Path()
    if (type(value["pid"]) is not int or value["pid"] <= 1
            or not isinstance(value["started"], str)
            or re.fullmatch(r"[1-9][0-9]{0,31}", value["started"]) is None
            or not path.is_absolute() or _linked(path) or not path.is_file()):
        raise ValueError(f"{label}缺少完整进程创建身份")
    resolved = path.resolve(strict=True)
    if executable is not None and resolved != executable.resolve(strict=True):
        raise ValueError(f"{label}可执行文件与登记产物不一致")
    return {"pid": value["pid"], "started": value["started"], "executable": str(resolved)}


def _artifact(backend: Path, value: dict, label: str) -> tuple[dict, Path]:
    exact(value, {"path", "sha256"})
    sha256 = digest(value["sha256"])
    if not isinstance(value["path"], str):
        raise ValueError(f"{label}产物路径无效")
    path = Path(value["path"])
    if (not path.is_absolute() or not path.is_relative_to(backend) or _linked(path)
            or not path.is_file() or file_digest(path)["sha256"] != sha256):
        raise ValueError(f"{label}产物路径或摘要已变化")
    resolved = path.resolve(strict=True)
    return {"path": str(resolved), "sha256": sha256}, resolved


def _runtime_contract(backend: Path, runtime: Path) -> tuple[dict, dict[str, Path]]:
    path = _regular(runtime / "runtime.json", runtime, "runtime 收据")
    value = read_json(path)
    exact(value, {"format_version", "backend_root", "scope_id", "configuration_sha256",
                  "worker_ready_url", "artifacts"})
    if (value["format_version"] != 1 or not isinstance(value["backend_root"], str)
            or Path(value["backend_root"]).resolve() != backend
            or not isinstance(value["scope_id"], str)
            or re.fullmatch(r"[a-z0-9][a-z0-9-]{2,63}", value["scope_id"]) is None):
        raise ValueError("runtime 收据不属于当前后端或隔离 scope")
    digest(value["configuration_sha256"])
    if not isinstance(value["worker_ready_url"], str):
        raise ValueError("runtime Worker 就绪地址无效")
    parsed = urlsplit(value["worker_ready_url"])
    if (parsed.scheme != "http" or parsed.hostname != "127.0.0.1" or parsed.path != "/readyz"
            or parsed.username is not None or parsed.password is not None or parsed.query or parsed.fragment
            or parsed.port is None):
        raise ValueError("runtime Worker 就绪地址无效")
    exact(value["artifacts"], ROLES)
    paths = {}
    for role in sorted(ROLES):
        value["artifacts"][role], paths[role] = _artifact(backend, value["artifacts"][role], role)
    return value, paths


def _event_shape(event: dict) -> set[str]:
    if not isinstance(event, dict):
        return set()
    common = {"sequence", "time_ns", "event", "role"}
    return {
        "start-intent": common | {"token", "artifact", "ready_url", "configuration_sha256", "log"},
        "started": common | {"token", "identity"},
        "start-failed": common | {"token", "error_type", "log"},
        "startup-rollback": common | {"identity"},
        "stopped": common | {"identity"},
    }.get(event.get("event"), set())


def _history(backend: Path, runtime: Path) -> tuple[dict, list[dict], list[dict]]:
    contract, artifacts = _runtime_contract(backend, runtime)
    history_path = _regular(runtime / "producer-history.json", runtime, "生产者历史")
    history = read_json(history_path)
    exact(history, {"format_version", "kind", "scope_id", "runtime_directory", "events"})
    events = history["events"]
    if (history["format_version"] != 1 or history["kind"] != "devex-clone-producer-history"
            or history["scope_id"] != contract["scope_id"]
            or not isinstance(history["runtime_directory"], str) or Path(history["runtime_directory"]) != runtime
            or not isinstance(events, list) or not events):
        raise ValueError("生产者历史与明确 runtime 不一致")
    intents, started, generations, identities, previous_time = {}, {}, [], {}, -1
    for sequence, event in enumerate(events):
        fields = _event_shape(event)
        if not fields:
            raise ValueError("生产者历史包含未知事件")
        exact(event, fields)
        if (event["sequence"] != sequence or type(event["time_ns"]) is not int
                or event["time_ns"] <= 0 or event["time_ns"] < previous_time or event["role"] not in ROLES):
            raise ValueError("生产者历史序号、时间或角色无效")
        previous_time = event["time_ns"]
        role, kind = event["role"], event["event"]
        if kind == "start-intent":
            token = event["token"]
            if not isinstance(token, str) or re.fullmatch(r"[a-f0-9]{32}", token) is None or token in intents:
                raise ValueError("生产者启动 token 无效或重复")
            artifact, _ = _artifact(backend, event["artifact"], f"{role} 启动")
            if (artifact != contract["artifacts"][role]
                    or event["configuration_sha256"] != contract["configuration_sha256"]
                    or event["log"] != f"{role}-{token}.log"):
                raise ValueError("生产者启动意图与 runtime 产物或配置不一致")
            digest(event["configuration_sha256"])
            ready = urlsplit(event["ready_url"])
            if ready.scheme != "http" or ready.hostname != "127.0.0.1" or ready.path != "/readyz" or ready.port is None:
                raise ValueError("生产者启动就绪地址无效")
            if not isinstance(event["log"], str):
                raise ValueError("生产者日志名无效")
            _regular(runtime / event["log"], runtime, "生产者日志", min_bytes=0)
            intents[token] = {"role": role, "started": None, "failed": False, "log": event["log"]}
        elif kind == "started":
            token = event["token"]
            if (token not in intents or intents[token]["role"] != role or intents[token]["started"] is not None
                    or intents[token]["failed"]):
                raise ValueError("started 事件没有唯一对应的启动意图")
            identity = _identity(event["identity"], "runtime 生产者", executable=artifacts[role])
            key = (identity["pid"], identity["started"])
            if key in identities:
                raise ValueError("同一 runtime 创建身份被多个启动 token 重复登记")
            identities[key] = token
            intents[token].update(started=identity, closed=False)
            started.setdefault(role, []).append(identity)
            generations.append({"sequence": sequence, "role": role, "token": token, "identity": identity})
        elif kind == "start-failed":
            token = event["token"]
            if (token not in intents or intents[token]["role"] != role or intents[token]["failed"]
                    or intents[token].get("closed") is True
                    or event["log"] != intents[token]["log"] or not isinstance(event["error_type"], str)
                    or re.fullmatch(r"[A-Za-z_][A-Za-z0-9_.]*", event["error_type"]) is None):
                raise ValueError("启动失败事件没有唯一对应的启动意图")
            intents[token]["failed"] = True
            if intents[token]["started"] is not None:
                intents[token]["closed"] = True
        else:
            identity = _identity(event["identity"], "runtime 结束事件", executable=artifacts[role])
            key = (identity["pid"], identity["started"])
            token = identities.get(key)
            if (token is None or intents[token]["role"] != role or intents[token]["started"] != identity
                    or intents[token]["failed"] or intents[token]["closed"]):
                raise ValueError("停止或回滚事件没有唯一对应的已启动代次")
            intents[token]["closed"] = True
    if any(intent["started"] is None and not intent["failed"] for intent in intents.values()):
        raise ValueError("生产者启动意图没有 started 或 start-failed 收据")
    receipts = []
    for role in sorted(ROLES):
        path = runtime / f"{role}.json"
        if role not in started:
            if path.exists() or linked(path):
                raise ValueError("角色收据没有对应的 started 历史")
            continue
        receipt_path = _regular(path, runtime, f"{role} 收据")
        receipt = read_json(receipt_path)
        exact(receipt, {"format_version", "role", "scope_id", "identity"})
        identity = _identity(receipt["identity"], f"{role} 收据", executable=artifacts[role])
        if (receipt["format_version"] != 1 or receipt["role"] != role
                or receipt["scope_id"] != contract["scope_id"] or identity != started[role][-1]):
            raise ValueError("角色收据不是该 runtime 的最新启动代次")
        receipts.append({"role": role, "receipt": binding(receipt_path), "identity": identity})
    return ({"runtime_directory": str(runtime), "runtime": binding(runtime / "runtime.json"),
             "history": binding(history_path), "scope_id": contract["scope_id"],
             "role_receipts": receipts, "generations": generations}, generations, receipts)


def _attempt(value: dict) -> tuple[dict, dict]:
    exact(value, {"attempt", "controller"})
    attempt = value["attempt"]
    exact(attempt, ATTEMPT_FIELDS)
    if (type(attempt["number"]) is not int or attempt["number"] <= 0 or attempt["stage"] not in STAGES
            or not isinstance(attempt["mode"], str)
            or re.fullmatch(r"[a-z][a-z0-9-]*", attempt["mode"]) is None
            or not isinstance(attempt["started_at"], str) or not attempt["started_at"]
            or not isinstance(attempt["finished_at"], str) or not attempt["finished_at"]
            or attempt["status"] not in {"passed", "failed"} or not isinstance(attempt["sources"], dict)
            or attempt["status"] == "passed" and attempt["error_type"] is not None
            or attempt["status"] == "failed" and not isinstance(attempt["error_type"], str)):
        raise ValueError("Node attempt 必须是已收尾的完整阶段记录")
    if attempt["result"] is not None:
        exact(attempt["result"], {"path", "bytes", "sha256"})
    return attempt, value["controller"]


def _script_source(attempt: dict, backend: Path, script: Path, descriptor: dict) -> None:
    sources = attempt["sources"]
    exact(sources, {"snapshot", "worktree_fingerprint", "fingerprints"})
    snapshot = sources["snapshot"]
    exact(snapshot, {"head", "patch_sha256", "files", "clean"})
    if (not isinstance(snapshot["head"], str) or re.fullmatch(r"[a-f0-9]{40}", snapshot["head"]) is None
            or type(snapshot["clean"]) is not bool
            or not isinstance(sources["worktree_fingerprint"], str)
            or re.fullmatch(r"sha256:[a-f0-9]{64}", sources["worktree_fingerprint"]) is None):
        raise ValueError("Node attempt 来源快照结构无效")
    digest(snapshot["patch_sha256"])
    exact(sources["fingerprints"], {"product", "test_tools", "support"})
    for value in sources["fingerprints"].values():
        exact(value, {"sha256", "files"})
        if type(value["files"]) is not int or value["files"] < 0:
            raise ValueError("Node attempt 分域指纹结构无效")
        digest(value["sha256"])
    if not isinstance(snapshot["files"], list):
        raise ValueError("Node attempt 未跟踪文件清单无效")
    relative = script.relative_to(backend).as_posix()
    found, previous = [], ""
    for item in snapshot["files"]:
        exact(item, {"path", "sha256"})
        path = PurePosixPath(item["path"]) if isinstance(item["path"], str) else PurePosixPath(".")
        if (not isinstance(item["path"], str) or path.is_absolute() or ".." in path.parts
                or path.as_posix() != item["path"] or item["path"] <= previous):
            raise ValueError("Node attempt 未跟踪文件清单未排序、重复或越界")
        previous = item["path"]
        digest(item["sha256"])
        if item["path"] == relative:
            found.append(item)
    if len(found) > 1 or found and found[0]["sha256"] != descriptor["sha256"]:
        raise ValueError("Node 历史脚本与 attempt 来源快照不一致")
    if not found and binding(script) != descriptor:
        raise ValueError("Node 历史脚本既不属于来源快照，也不同于当前固定文件")


def _authoritative_inputs(backend: Path, run: Path, runtime_directories: list[Path],
                          attempts: list[dict]) -> tuple[list[Path], list[dict]]:
    state = load_state(run)
    active = post_registration(backend, run, cleanup=True)
    if type(active.sequence) is not int or not 0 <= active.sequence <= 9999:
        raise ValueError("post-copy runtime 代次无效")
    expected_runtimes = [run / "post-copy/runtime",
                         *(run / "post-copy" / f"runtime-{number:04d}" for number in range(1, active.sequence + 1))]
    seed_prepares = [item for item in state["attempts"]
                     if item["stage"] == "seed-runtime" and item["mode"] == "prepare" and item["status"] == "passed"]
    if len(seed_prepares) > 1:
        raise ValueError("seed runtime 只能有一个已发布准备代次")
    if seed_prepares:
        expected_runtimes.append(run / "seed-runtime/runtime")
    if len(runtime_directories) != len(expected_runtimes) or set(runtime_directories) != set(expected_runtimes):
        raise ValueError("显式 runtime 清单没有覆盖完整登记代次")
    phases = {(stage, mode) for stage, modes in PHASES.values() for mode in modes}
    expected_attempts = [item for item in state["attempts"] if (item["stage"], item["mode"]) in phases]
    supplied = {_attempt(item)[0]["number"]: item for item in attempts}
    if len(supplied) != len(attempts) or set(supplied) != {item["number"] for item in expected_attempts}:
        raise ValueError("显式 Node attempt 清单没有覆盖固定 state")
    ordered = []
    for attempt in expected_attempts:
        registered = supplied[attempt["number"]]
        if registered["attempt"] != attempt:
            raise ValueError("Node attempt 不同于固定 state")
        controller = _bound(run / f"controller-{attempt['number']:04d}.json", run, "Node controller")
        if registered["controller"] != controller:
            raise ValueError("Node attempt.controller 不同于固定控制器")
        ordered.append(registered)
    return expected_runtimes, ordered


def _node(backend: Path, run: Path, registered: dict) -> dict | None:
    attempt, expected_controller = _attempt(registered)
    number, stage = attempt["number"], attempt["stage"]
    if attempt["result"] is not None:
        _bound(run / "results" / f"{number:04d}.json", run, "Node attempt 结果", attempt["result"])
    output = run / stage / f"attempt-{number:04d}"
    launch_path, process_path = output / "session-launch.json", output / "session-process.json"
    evidence = (launch_path.exists() or linked(launch_path), process_path.exists() or linked(process_path))
    if not any(evidence):
        if output.exists() and any((output / name).exists() or linked(output / name)
                                   for name in ("session.stdout.log", "session.stderr.log")):
            raise ValueError("Node 日志缺少明确启动收据")
        return None
    if stage not in {"post-copy", "seed-runtime"} or not all(evidence):
        raise ValueError("Node 生产者不属于明确阶段或缺少收据")
    _directory(output, run, "Node attempt 输出")
    launch_binding = _bound(launch_path, run, "Node launch")
    receipt_binding = _bound(process_path, run, "Node process")
    controller_path = run / f"controller-{number:04d}.json"
    controller_binding = _bound(controller_path, run, "Node controller", expected_controller)
    controller, launch, process = read_json(controller_path), read_json(launch_path), read_json(process_path)
    exact(controller, {"format_version", "kind", "owner", "attempt", "attempt_sha256"})
    original = {**attempt, "status": "running", "finished_at": None, "result": None, "error_type": None}
    owner = controller["owner"]
    exact(owner, {"format_version", "identity", "directory", "manifest_sha256"})
    manifest = _bound(run / "manifest.json", run, "run manifest")
    _identity(owner["identity"], "Node controller")
    if (controller["format_version"] != 1 or controller["kind"] != "devex-stage-controller"
            or controller["attempt"] != number or controller["attempt_sha256"] != plan_hash(original)
            or owner["format_version"] != 1 or owner["directory"] != str(run)
            or owner["manifest_sha256"] != manifest["sha256"]):
        raise ValueError("Node controller 与 attempt 不一致")
    exact(launch, {"format_version", "kind", "run_manifest", "registration", "controller", "attempt",
                   "producer_kind", "command", "node", "script"})
    kind = launch["producer_kind"]
    identity_producer = kind in {"identity-apply", "identity-verify"}
    identity_owner_recorded = "identity_lock_owner" in process
    if identity_owner_recorded and not identity_producer:
        raise ValueError("非身份 Node 收据不能登记 identity lock owner")
    exact(process, {"format_version", "kind", "launch", "identity"}
          | ({"identity_lock_owner"} if identity_owner_recorded else set()))
    if (identity_owner_recorded
            and (not isinstance(process["identity_lock_owner"], str)
                 or IDENTITY_RELEASE_ID.fullmatch(process["identity_lock_owner"]) is None)):
        raise ValueError("身份 Node 收据 identity lock owner 格式无效")
    if (kind not in PHASES or PHASES[kind][0] != stage or attempt["mode"] not in PHASES[kind][1]
            or KINDS.get(kind) is None or launch["format_version"] != 1
            or launch["kind"] != "devex-post-producer-launch" or launch["attempt"] != number
            or launch["controller"] != controller_binding or launch["run_manifest"] != manifest
            or process["format_version"] != 1 or process["kind"] != "devex-post-producer"
            or process["launch"] != launch_binding):
        raise ValueError("Node launch/process 不属于固定 attempt")
    registration_path = _binding_path(launch["registration"], "Node registration")
    registration = _bound(registration_path, run, "Node registration", launch["registration"])
    exact(launch["node"], {"path", "sha256"})
    if not isinstance(launch["node"]["path"], str):
        raise ValueError("Node 工具路径无效")
    node_path = Path(launch["node"]["path"])
    if (not node_path.is_absolute() or _linked(node_path) or not node_path.is_file()
            or file_digest(node_path)["sha256"] != digest(launch["node"]["sha256"])):
        raise ValueError("Node 工具路径或摘要已变化")
    script_path = _binding_path(launch["script"], "Node 历史脚本")
    digest(launch["script"]["sha256"])
    expected_script = backend / "tools" / "js" / KINDS[kind]
    if (script_path != expected_script or type(launch["script"]["bytes"]) is not int
            or launch["script"]["bytes"] <= 0):
        raise ValueError("Node 历史脚本绑定无效")
    _regular(script_path, backend, "Node 历史脚本")
    _script_source(attempt, backend, script_path, launch["script"])
    request = historical_producer_request(backend, run, stage, launch["registration"])
    command = producer_command(backend, request, run, number, kind, output)
    if (launch["node"] != request["node"] or launch["command"] != command
            or any(not isinstance(item, str) or not item or "\0" in item for item in command)):
        raise ValueError("Node 命令未绑定明确 run 和 attempt")
    identity = _identity(process["identity"], "Node 生产者", executable=node_path)
    return {"attempt": number, "stage": stage, "mode": attempt["mode"], "producer_kind": kind,
            "receipt": receipt_binding, "launch": launch_binding, "controller": controller_binding,
            "registration": registration, "identity": identity}


def _occurrence_key(value: dict) -> tuple:
    return (value["kind"], value.get("runtime_directory", ""), value.get("role", ""),
            value.get("sequence", -1), value.get("attempt", -1), value.get("producer_kind", ""))


def collect_producer_lineage(backend: Path, run_directory: Path, runtime_directories: list[Path],
                             attempts: list[dict]) -> dict:
    """只读取调用方明确列出的运行目录和 attempt，不扫描工作区或探测服务。"""
    backend = Path(backend)
    if not backend.is_absolute() or _linked(backend) or not backend.is_dir():
        raise ValueError("后端根目录必须是无链接的绝对目录")
    backend = backend.resolve(strict=True)
    local = _directory(backend / ".local-tests", backend, "本地证据根")
    run = _directory(Path(run_directory), local, "验收 run")
    if (not isinstance(runtime_directories, list) or not runtime_directories
            or not isinstance(attempts, list)):
        raise ValueError("必须显式列出 runtime 目录和 attempt")
    manifest = _bound(run / "manifest.json", run, "run manifest")
    runtimes, seen_runtime, identity_records = [], set(), {}

    def record(identity: dict, occurrence: dict) -> None:
        key = (identity["pid"], identity["started"])
        current = identity_records.get(key)
        if current is None:
            current = {"identity": identity, "occurrences": []}
            identity_records[key] = current
        elif current["identity"] != identity:
            raise ValueError("同一 PID 创建代次登记了不同可执行文件")
        if occurrence not in current["occurrences"]:
            current["occurrences"].append(occurrence)

    try:
        runtime_paths = [Path(item) for item in runtime_directories]
    except TypeError as error:
        raise ValueError("runtime 目录必须是明确路径") from error
    runtime_paths, attempts = _authoritative_inputs(backend, run, runtime_paths, attempts)
    for raw in sorted(runtime_paths, key=lambda item: str(item).casefold()):
        runtime = _directory(raw, run, "runtime")
        if runtime in seen_runtime:
            raise ValueError("runtime 目录重复登记")
        seen_runtime.add(runtime)
        result, generations, _receipts = _history(backend, runtime)
        runtimes.append(result)
        for generation in generations:
            record(generation["identity"], {"kind": "runtime", "runtime_directory": str(runtime),
                   "role": generation["role"], "sequence": generation["sequence"]})
    nodes, numbers = [], set()
    registered_attempts = [(_attempt(registered)[0]["number"], registered) for registered in attempts]
    for _number, registered in sorted(registered_attempts, key=lambda item: item[0]):
        attempt, _controller = _attempt(registered)
        if attempt["number"] in numbers:
            raise ValueError("attempt 重复登记")
        numbers.add(attempt["number"])
        node = _node(backend, run, registered)
        if node is not None:
            nodes.append(node)
            record(node["identity"], {"kind": "node", "attempt": node["attempt"],
                   "producer_kind": node["producer_kind"]})
    identities = sorted(identity_records.values(), key=lambda item: (
        item["identity"]["pid"], int(item["identity"]["started"]), item["identity"]["executable"].casefold()))
    for item in identities:
        item["occurrences"].sort(key=_occurrence_key)
    return {"format_version": 1, "kind": "devex-clone-producer-lineage",
            "run_directory": str(run), "run_manifest": manifest,
            "runtimes": runtimes, "node_producers": nodes, "identities": identities}
