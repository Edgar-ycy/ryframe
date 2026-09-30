"""树外私有监督器：持有 Job 或 pidfd，完成整树回收后才发布成功证明。"""
from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import subprocess
import sys
import time

from full_stack_process import assert_identity, process_identity, terminate_owned_process, write_receipt
from full_stack_process_members import WindowsMembers, UnixMembers, finish_members, join_windows_job


def receipt_path(directory: Path, role: str, operation_id: str, suffix: str) -> Path:
    return directory / f"{role}-members-{operation_id}-{suffix}.json"


def _read(path: Path) -> dict:
    from full_stack_process_tree import _read_object
    return _read_object(path)


def start_monitor(directory: Path, role: str, scope: str, operation_id: str) -> dict:
    from full_stack_process_tree import OPERATION_ID, ROLES

    if role not in ROLES or OPERATION_ID.fullmatch(operation_id) is None or not scope:
        raise ValueError("完整成员监督请求身份无效")
    directory = directory.resolve(strict=True)
    supervisor = process_identity(os.getpid())
    if supervisor is None:
        raise ValueError("无法绑定原监督进程身份")
    if os.name != "nt" and (os.getsid(0) != os.getpid() or os.getpgid(0) != os.getpid()):
        raise ValueError("Unix 产品监督进程必须拥有独立 session/group")
    ready = receipt_path(directory, role, operation_id, "ready")
    if ready.exists():
        raise ValueError("成员监督器登记已经存在，拒绝接管")
    environment = dict(os.environ)
    environment.pop("__PYVENV_LAUNCHER__", None)
    python = str(Path(getattr(sys, "_base_executable", sys.executable)).resolve(strict=True))
    child = subprocess.Popen([python, "-X", "utf8", str(Path(__file__).resolve()), "__monitor",
                              "--directory", str(directory), "--role", role, "--scope", scope,
                              "--operation-id", operation_id, "--supervisor", json.dumps(supervisor)],
                             env=environment, stdin=subprocess.DEVNULL, start_new_session=True,
                             creationflags=subprocess.CREATE_NO_WINDOW if os.name == "nt" else 0)
    identity = process_identity(child.pid)
    try:
        if identity is None:
            raise ValueError("成员监督器在登记前退出")
        deadline = time.monotonic() + 5
        while not ready.is_file():
            if child.poll() is not None or time.monotonic() >= deadline:
                raise TimeoutError("成员监督器未发布启动前归属")
            time.sleep(0.01)
        expected = {"format_version": 1, "operation_id": operation_id, "directory": str(directory),
                    "role": role, "scope_id": scope, "supervisor": supervisor, "monitor": identity}
        if _read(ready) != expected or process_identity(child.pid) != identity:
            raise ValueError("成员监督器登记与本次实际创建身份不同")
        if os.name == "nt":
            join_windows_job(operation_id)
        return identity
    except BaseException:
        if identity is not None and process_identity(identity["pid"]) == identity:
            terminate_owned_process(identity, crash=True)
        child.wait(timeout=5)
        raise


def wait_members(receipt: dict, timeout: float = 10) -> dict:
    from full_stack_process_tree import _valid_identity

    identity = _valid_identity(receipt.get("monitor"), "树外成员监督器")
    path = receipt_path(Path(receipt["runtime_directory"]), receipt["role"], receipt["operation_id"], "stopped")
    deadline = time.monotonic() + timeout
    while True:
        try:
            actual = process_identity(identity["pid"])
        except PermissionError:
            # 退出中的 Windows 进程镜像查询可能暂时拒绝访问；只等待，绝不因此认定已退出。
            if time.monotonic() >= deadline:
                raise
            time.sleep(0.01)
            continue
        if actual is not None and actual != identity:
            raise ValueError("成员监督器 PID 已复用，拒绝认定关闭成功")
        if path.is_file():
            proof = _read(path)
            expected = {"format_version", "operation_id", "directory", "role", "scope_id", "supervisor",
                        "monitor", "status", "members", "error_type"}
            if (set(proof) != expected or proof["operation_id"] != receipt["operation_id"]
                    or proof["directory"] != receipt["runtime_directory"] or proof["role"] != receipt["role"]
                    or proof["scope_id"] != receipt["scope_id"] or proof["monitor"] != identity
                    or proof["supervisor"] != receipt["supervisor"]):
                raise ValueError("完整成员关闭证明不属于本次进程树")
            if actual is not None:
                if time.monotonic() >= deadline:
                    raise TimeoutError("成员监督器签发收据后未退出")
                time.sleep(0.01)
                continue
            if proof["status"] != "stopped" or proof["error_type"] is not None:
                raise ValueError("完整成员回收失败：" + str(proof["error_type"]))
            if not isinstance(proof["members"], list) or not proof["members"]:
                raise ValueError("完整成员关闭证明缺少实际成员")
            for member in proof["members"]:
                _valid_identity(member, "进程树成员")
            if receipt["supervisor"] not in proof["members"]:
                raise ValueError("完整成员关闭证明缺少原监督进程")
            if actual is None:
                return proof
        elif actual is None:
            raise ValueError("成员监督器退出但没有完整成员关闭证明")
        if time.monotonic() >= deadline:
            raise TimeoutError("成员监督器未在期限内完成整树回收")
        time.sleep(0.01)


def wait_startup_cleanup(directory: Path, role: str, scope: str, operation_id: str, supervisor: dict) -> None:
    """未发布产品树时也等待已经启动的成员监督器，不留下日志句柄和后台回收进程。"""
    path = receipt_path(directory, role, operation_id, "ready")
    if not path.exists():
        return
    ready = _read(path)
    if (ready.get("supervisor") != supervisor or ready.get("operation_id") != operation_id
            or ready.get("directory") != str(directory.resolve()) or ready.get("role") != role
            or ready.get("scope_id") != scope):
        raise ValueError("启动失败后的成员监督证据不属于本次运行")
    wait_members({"runtime_directory": str(directory.resolve()), "role": role, "scope_id": scope,
                  "operation_id": operation_id, "supervisor": supervisor, "monitor": ready["monitor"]})


def completion_binding(receipt: dict) -> dict:
    from artifact_digests import file_digest

    proof = wait_members(receipt)
    path = receipt_path(Path(receipt["runtime_directory"]), receipt["role"], receipt["operation_id"], "stopped")
    descriptor = {"path": str(path), **file_digest(path)}
    if _read(path) != proof:
        raise ValueError("完整成员关闭证明在绑定期间发生变化")
    return descriptor


def monitor(directory: Path, role: str, scope: str, operation_id: str, supervisor: dict) -> None:
    from full_stack_process_tree import OPERATION_ID, ROLES, _valid_identity

    _valid_identity(supervisor, "原监督进程")
    if role not in ROLES or OPERATION_ID.fullmatch(operation_id) is None or not scope:
        raise ValueError("私有成员监督参数无效")
    directory = directory.resolve(strict=True)
    if not assert_identity(process_identity(supervisor["pid"]), supervisor):
        raise ValueError("原监督进程已退出，不能建立新树归属")
    identity = process_identity(os.getpid())
    members = (WindowsMembers if os.name == "nt" else UnixMembers)(operation_id, supervisor)
    ready = {"format_version": 1, "operation_id": operation_id, "directory": str(directory),
             "role": role, "scope_id": scope, "supervisor": supervisor, "monitor": identity}
    result = {**ready, "status": "failed", "members": [], "error_type": None}
    try:
        write_receipt(receipt_path(directory, role, operation_id, "ready"), ready)
        deadline = time.monotonic() + 5
        while not members.joined():
            if members.supervisor_exited() or time.monotonic() >= deadline:
                raise TimeoutError("原监督进程未完成启动前归属")
            time.sleep(0.01)
        while not members.supervisor_exited():
            members.capture()
            time.sleep(0.01)
        result["members"] = finish_members(members)
        result["status"] = "stopped"
    except BaseException as error:
        result["error_type"] = type(error).__name__
        try:
            members.terminate()
        except BaseException as cleanup:
            result["error_type"] += "/" + type(cleanup).__name__
        raise
    finally:
        members.close()
        write_receipt(receipt_path(directory, role, operation_id, "stopped"), result)


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("operation", choices=("__monitor",))
    parser.add_argument("--directory", type=Path, required=True)
    parser.add_argument("--role", required=True)
    parser.add_argument("--scope", required=True)
    parser.add_argument("--operation-id", required=True)
    parser.add_argument("--supervisor", required=True)
    args = parser.parse_args()
    monitor(args.directory, args.role, args.scope, args.operation_id, json.loads(args.supervisor))


if __name__ == "__main__":
    main()
