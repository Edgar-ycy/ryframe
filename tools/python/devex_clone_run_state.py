"""统一验收目录的持久阶段记录；中断保留 intent，不自动重放写入。"""
from __future__ import annotations

from contextlib import contextmanager
import datetime as dt
import hashlib
import json
import os
from pathlib import Path
import uuid

from devex_clone_capture import read_json, write_json
from devex_clone_model import digest, exact, linked, local_path
from full_stack_process import process_identity, write_receipt
from process_guard import process_guard
from restore_build import file_digest
from restore_reference_plan import plan_hash

STAGES = {"export", "target-verify", "copy", "runtime-source", "runtime-target", "post-copy", "seed-runtime",
          "storage-source", "storage-target", "cache-target", "fixture-buckets", "fixture-services"}


def now() -> str:
    return dt.datetime.now(dt.timezone.utc).isoformat()


def binding(path: Path) -> dict:
    return {"path": str(path), **file_digest(path)}


def _serialized_state(value: dict) -> tuple[bytes, ...]:
    """重建两个既有写入器可能发布的字节，供历史 state 前缀核验。"""
    values = {
        (json.dumps(value, ensure_ascii=False, sort_keys=True, indent=2) + "\n").encode("utf-8"),
        (json.dumps(value, indent=2) + "\n").encode("utf-8"),
        (json.dumps(value, indent=2).replace("\n", "\r\n") + "\r\n").encode("utf-8"),
    }
    return tuple(values)


def historical_state(directory: Path, descriptor: dict, *, verify_results: bool = True) -> dict:
    """证明旧 state 是当前追加式账本的精确前缀，并返回当前稳定绑定。"""
    exact(descriptor, {"path", "bytes", "sha256"})
    path = directory / "state.json"
    if (not isinstance(descriptor["path"], str) or Path(descriptor["path"]) != path
            or type(descriptor["bytes"]) is not int
            or descriptor["bytes"] <= 0):
        raise ValueError("历史阶段记录描述不属于固定运行目录")
    digest(descriptor["sha256"])
    current_binding = binding(path)
    value = load_state(directory, verify_results=verify_results)
    if current_binding == descriptor:
        return {"state": value, "historical_attempts": len(value["attempts"]),
                "current": current_binding}
    matches = []
    for count in range(len(value["attempts"]) + 1):
        candidate = {**value, "attempts": value["attempts"][:count]}
        if any(len(raw) == descriptor["bytes"]
               and hashlib.sha256(raw).hexdigest() == descriptor["sha256"]
               for raw in _serialized_state(candidate)):
            matches.append(count)
    if len(matches) != 1:
        raise ValueError("历史阶段记录不是当前账本的精确追加前缀")
    if binding(path) != current_binding:
        raise ValueError("阶段记录在历史前缀核对期间变化")
    return {"state": value, "historical_attempts": matches[0], "current": current_binding}


def record_failure(backend: Path, directory: Path, number: int, stage: str, mode: str,
                   error: BaseException) -> None:
    """只记录项目代码位置，不保存可能含秘密的异常正文、源码行或局部变量。"""
    frames = []
    scripts = (backend / "tools" / "python").resolve()
    trace = error.__traceback__
    while trace is not None:
        code = trace.tb_frame.f_code
        filename = Path(code.co_filename).resolve()
        if filename.is_relative_to(scripts):
            frames.append({"file": "tools/python/" + filename.relative_to(scripts).as_posix(),
                           "function": code.co_name, "line": trace.tb_lineno})
        trace = trace.tb_next
    write_json(directory / f"failure-{number:04d}.json", {
        "format_version": 1, "kind": "devex-stage-failure", "attempt": number,
        "stage": stage, "mode": mode, "error_type": type(error).__name__, "frames": frames,
        "controller": binding(directory / f"controller-{number:04d}.json"),
    })


def load_state(directory: Path, *, verify_results: bool = True) -> dict:
    value = read_json(directory / "state.json")
    exact(value, {"format_version", "manifest", "attempts"})
    if value["format_version"] != 1 or not isinstance(value["attempts"], list):
        raise ValueError("验收阶段记录类型无效")
    if value["manifest"] != binding(directory / "manifest.json"):
        raise ValueError("固定验收清单发生变化")
    for index, attempt in enumerate(value["attempts"], 1):
        exact(attempt, {"number", "stage", "mode", "started_at", "finished_at", "status", "sources", "result", "error_type"})
        if (attempt["number"] != index or attempt["stage"] not in STAGES
                or attempt["status"] not in {"running", "passed", "failed"}
                or attempt["status"] == "passed" and attempt["result"] is None):
            raise ValueError("验收阶段顺序或状态无效")
        if attempt["result"] is not None:
            exact(attempt["result"], {"path", "bytes", "sha256"})
            digest(attempt["result"]["sha256"])
            path = Path(attempt["result"]["path"])
            if (path != directory / "results" / f"{index:04d}.json"
                    or type(attempt["result"]["bytes"]) is not int or attempt["result"]["bytes"] <= 0
                    or verify_results and binding(path) != attempt["result"]):
                raise ValueError("验收阶段结果缺失或被修改")
    return value


def initialize_state(directory: Path) -> None:
    (directory / "results").mkdir()
    write_json(directory / "state.json", {"format_version": 1,
               "manifest": binding(directory / "manifest.json"), "attempts": []})


def begin(directory: Path, stage: str, mode: str, sources: dict, *, verify_results: bool = True) -> int:
    value = load_state(directory, verify_results=verify_results)
    if stage not in STAGES:
        raise ValueError("未知验收阶段")
    if any(item["status"] == "running" for item in value["attempts"]):
        raise ValueError("存在未收尾阶段，须先核实运行进程并显式恢复控制锁")
    number = len(value["attempts"]) + 1
    value["attempts"].append({"number": number, "stage": stage, "mode": mode,
                            "started_at": now(), "finished_at": None, "status": "running",
                            "sources": sources, "result": None, "error_type": None})
    write_receipt(directory / "state.json", value)
    return number


def finish(directory: Path, number: int, *, result: dict | None = None, error: BaseException | None = None,
           verify_results: bool = True) -> None:
    value = load_state(directory, verify_results=verify_results)
    if not value["attempts"] or value["attempts"][-1]["number"] != number or value["attempts"][-1]["status"] != "running":
        raise ValueError("不能收尾其他执行者或已经结束的阶段")
    attempt = value["attempts"][-1]
    if result is not None:
        path = directory / "results" / f"{number:04d}.json"
        if path.exists():
            if linked(path) or read_json(path) != result:
                raise ValueError("已保存的阶段结果不同，拒绝覆盖失败证据")
        else:
            write_json(path, result)
        attempt["result"] = binding(path)
    attempt.update(status="failed" if error else "passed", finished_at=now(),
                   error_type=type(error).__name__ if error else None)
    if error is None and result is None:
        raise ValueError("成功阶段必须发布实际结果")
    write_receipt(directory / "state.json", value)


@contextmanager
def claim_run_lock(directory: Path):
    path = directory / "run.lock"
    owner = process_identity(os.getpid())
    if owner is None:
        raise ValueError("无法取得验收控制进程的内核身份")
    path.mkdir()
    identity = path.stat().st_ino
    receipt = {"format_version": 1, "identity": owner,
               "directory": str(directory), "manifest_sha256": file_digest(directory / "manifest.json")["sha256"]}
    write_json(path / "owner.json", receipt)
    try:
        yield receipt
    finally:
        if linked(path) or not path.is_dir() or path.stat().st_ino != identity:
            raise ValueError("验收控制锁身份变化，拒绝删除其他执行者的锁")
        if {item.name for item in path.iterdir()} != {"owner.json"} or read_json(path / "owner.json") != receipt:
            raise ValueError("验收控制锁内容变化，保留现场")
        remove_owned_lock(path, identity, receipt)


def remove_owned_lock(path: Path, inode: int, owner: dict) -> None:
    """目录删除失败时保留同 inode 的已知 owner，不把可核验锁退化为空锁。"""
    if (linked(path) or not path.is_dir() or path.stat().st_ino != inode
            or {item.name for item in path.iterdir()} != {"owner.json"}
            or read_json(path / "owner.json") != owner):
        raise ValueError("控制锁清理身份变化，保留现场")
    (path / "owner.json").unlink()
    try:
        path.rmdir()
    except BaseException:
        if not linked(path) and path.is_dir() and path.stat().st_ino == inode and not any(path.iterdir()):
            write_json(path / "owner.json", owner)
        raise


@contextmanager
def run_lock(directory: Path):
    with process_guard(directory, "run-control.guard"), claim_run_lock(directory) as owner:
        yield owner


def bind_controller_attempt(directory: Path, number: int, owner: dict, *, verify_results: bool = True) -> dict:
    """阶段操作前固定持久控制器证据，目录锁清理后仍可证明中断归属。"""
    value = load_state(directory, verify_results=verify_results)
    if (not value["attempts"] or value["attempts"][-1]["number"] != number
            or value["attempts"][-1]["status"] != "running"
            or read_json(directory / "run.lock/owner.json") != owner
            or process_identity(os.getpid()) != owner["identity"]):
        raise ValueError("只能绑定当前持锁控制器刚开始的阶段")
    path = directory / f"controller-{number:04d}.json"
    write_json(path, {"format_version": 1, "kind": "devex-stage-controller", "owner": owner,
                      "attempt": number, "attempt_sha256": plan_hash(value["attempts"][-1])})
    return binding(path)


def controller_record(directory: Path, number: int, attempt: dict) -> tuple[dict, dict]:
    path = directory / f"controller-{number:04d}.json"
    descriptor, value = binding(path), read_json(path)
    exact(value, {"format_version", "kind", "owner", "attempt", "attempt_sha256"})
    if (value["format_version"] != 1 or value["kind"] != "devex-stage-controller" or value["attempt"] != number
            or value["attempt_sha256"] != plan_hash(attempt)):
        raise ValueError("持久控制器与当前阶段完整记录不同")
    return descriptor, value["owner"]


def pending_controller(directory: Path, *, verify_results: bool = True) -> dict | None:
    """返回明确阶段的恢复绑定，不扫描或猜测最近一次持有人。"""
    value = load_state(directory, verify_results=verify_results)
    running = [item for item in value["attempts"] if item["status"] == "running"]
    if not running:
        return None
    if len(running) != 1:
        raise ValueError("不能把多个运行中阶段归属给同一控制器")
    attempt = running[0]
    path = directory / f"controller-{attempt['number']:04d}.json"
    if path.exists():
        return controller_record(directory, attempt["number"], attempt)[0]
    path = directory / "run.lock/owner.json"
    return binding(path) if path.exists() else None


def controller_observation(directory: Path) -> dict | None:
    """只读核对锁或未收尾阶段的实际控制器；观察本身不取得锁或回收资源。"""
    state_before = binding(directory / "state.json")
    value = load_state(directory)
    lock = directory / "run.lock"
    lock_identity = None
    if lock.exists():
        if linked(lock) or not lock.is_dir() or {item.name for item in lock.iterdir()} != {"owner.json"}:
            raise ValueError("控制锁目录或内容无法归属")
        lock_identity = (lock.stat().st_dev, lock.stat().st_ino)
        descriptor = binding(lock / "owner.json")
        owner = read_json(lock / "owner.json")
    else:
        descriptor = pending_controller(directory)
        if descriptor is None:
            if any(item["status"] == "running" for item in value["attempts"]):
                raise ValueError("未收尾阶段缺少控制器收据，须核对原始现场")
            if lock.exists() or binding(directory / "state.json") != state_before:
                raise ValueError("控制状态在只读观察期间变化")
            return None
        owner = read_json(Path(descriptor["path"]))["owner"]
    exact(owner, {"format_version", "identity", "directory", "manifest_sha256"})
    exact(owner["identity"], {"pid", "started", "executable"})
    if (owner["format_version"] != 1 or owner["directory"] != str(directory)
            or owner["manifest_sha256"] != binding(directory / "manifest.json")["sha256"]
            or type(owner["identity"]["pid"]) is not int or owner["identity"]["pid"] <= 1
            or any(not isinstance(owner["identity"][key], str) or not owner["identity"][key]
                   for key in ("started", "executable"))):
        raise ValueError("控制器收据不属于固定运行目录或缺少完整进程身份")
    observed = process_identity(owner["identity"]["pid"])
    if (binding(Path(descriptor["path"])) != descriptor
            or binding(directory / "state.json") != state_before
            or lock_identity is None and lock.exists()
            or lock_identity is not None and
            (linked(lock) or not lock.is_dir() or (lock.stat().st_dev, lock.stat().st_ino) != lock_identity
             or {item.name for item in lock.iterdir()} != {"owner.json"})):
        raise ValueError("控制状态在只读观察期间变化")
    result = {"owner": descriptor, "process_matches": observed == owner["identity"],
              "process_missing": observed is None}
    if lock_identity is None:
        result["phase"] = "publication_pending"
    return result


def recovery_owner(backend: Path, directory: Path, descriptor: dict, value: dict) -> tuple[dict, int | None]:
    from devex_clone_source_proof import bound_file

    path = bound_file(backend, descriptor)
    if path == directory / "run.lock/owner.json":
        return read_json(path), None
    running = [item for item in value["attempts"] if item["status"] == "running"]
    if len(running) != 1:
        raise ValueError("持久控制器恢复必须对应唯一运行中阶段")
    attempt = running[0]
    if path != directory / f"controller-{attempt['number']:04d}.json":
        raise ValueError("旧控制器不能恢复新的运行阶段")
    current, owner = controller_record(directory, attempt["number"], attempt)
    if current != descriptor:
        raise ValueError("持久控制器绑定变化")
    return owner, attempt["number"]


def recover_run_lock(backend: Path, directory: Path, owner_binding: dict, *, verify_results: bool) -> dict:
    path = local_path(backend, str(directory / "run.lock"))
    value = load_state(directory, verify_results=verify_results)
    owner, number = recovery_owner(backend, directory, owner_binding, value)
    exact(owner, {"format_version", "identity", "directory", "manifest_sha256"})
    if (owner["format_version"] != 1 or owner["directory"] != str(directory)
            or owner["manifest_sha256"] != file_digest(directory / "manifest.json")["sha256"]):
        raise ValueError("控制锁来源与固定验收目录不符")
    if process_identity(owner["identity"]["pid"]) is not None:
        raise ValueError("控制进程仍存在或 PID 已复用，拒绝回收")
    owner_file = path / "owner.json"
    lock_binding = binding(owner_file) if path.exists() else None
    inode = path.stat().st_ino if path.exists() else None
    if lock_binding is not None and read_json(owner_file) != owner:
        raise ValueError("现有控制锁属于其他控制器，不能接管")
    for attempt in value["attempts"]:
        if attempt["status"] == "running":
            persistent = directory / f"controller-{attempt['number']:04d}.json"
            if persistent.exists() and controller_record(directory, attempt["number"], attempt)[1] != owner:
                continue
            if number is None or attempt["number"] == number:
                attempt.update(status="failed", finished_at=now(), error_type="ControllerInterrupted")
    result = {"status": "controller_recovered", "owner": owner, "remote_writes": 0,
              "copy_requires_reconciliation": True, "state_sha256": plan_hash(value)}
    attempt_id = uuid.uuid4().hex
    write_json(directory / ("recovery-" + attempt_id + ".intent.json"),
               {"status": "controller_recovery_started", "owner": owner_binding,
                "state_before": binding(directory / "state.json"), "state_after_sha256": plan_hash(value)})
    if (binding(Path(owner_binding["path"])) != owner_binding
            or linked(path) or (inode is None and path.exists()) or (inode is not None and
            (path.stat().st_ino != inode or binding(owner_file) != lock_binding
             or {item.name for item in path.iterdir()} != {"owner.json"}))):
        raise ValueError("回收前控制锁发生变化")
    write_receipt(directory / "state.json", value)
    if (binding(Path(owner_binding["path"])) != owner_binding
            or linked(path) or (inode is None and path.exists()) or (inode is not None and
            (path.stat().st_ino != inode or binding(owner_file) != lock_binding
             or {item.name for item in path.iterdir()} != {"owner.json"}))):
        raise ValueError("保存阶段状态后控制锁发生变化，保留现场")
    if inode is not None:
        remove_owned_lock(path, inode, owner)
    write_json(directory / ("recovered-" + attempt_id + ".json"), result)
    return result


def recover_lock(backend: Path, directory: Path, owner_binding: dict, *, verify_results: bool = True) -> dict:
    """只回收明确死亡控制器的本地锁；与新控制器全程互斥，写入仍须 reconcile。"""
    local_path(backend, str(directory))
    with process_guard(directory, "run-control.guard"):
        return recover_run_lock(backend, directory, owner_binding, verify_results=verify_results)
