"""为真实全栈服务建立可核验的私有进程树边界。"""

from __future__ import annotations

import argparse
import ctypes
import json
import os
import re
import signal
import subprocess
import sys
import time
import uuid
from dataclasses import dataclass
from pathlib import Path

from full_stack_process import (
    assert_identity,
    process_identity,
    read_process,
    record_process,
    terminate_owned_process,
    write_receipt,
)

ROLES = frozenset({"api", "worker"})
OPERATION_ID = re.compile(r"^[a-f0-9]{32}$")


def _valid_identity(value: object, label: str) -> dict:
    if (
        not isinstance(value, dict)
        or set(value) != {"pid", "started", "executable"}
        or type(value["pid"]) is not int
        or value["pid"] <= 1
        or not isinstance(value["started"], str)
        or not value["started"].isdigit()
        or not isinstance(value["executable"], str)
        or not Path(value["executable"]).is_absolute()
    ):
        raise ValueError(f"{label}缺少有效的 PID 创建身份")
    return value


def _tree_path(directory: Path, role: str) -> Path:
    if role not in ROLES:
        raise ValueError("未知全栈进程树角色")
    return directory / f"{role}-tree.json"


def _read_object(path: Path) -> dict:
    def unique(pairs: list[tuple[str, object]]) -> dict:
        value = {}
        for key, item in pairs:
            if key in value:
                raise ValueError("进程树收据包含重复字段")
            value[key] = item
        return value

    if not path.is_file() or path.is_symlink() or path.stat().st_size > 64 * 1024:
        raise ValueError("进程树收据不存在或不是可信普通文件")
    try:
        value = json.loads(path.read_text(encoding="utf-8"), object_pairs_hook=unique)
    except (json.JSONDecodeError, UnicodeError) as error:
        raise ValueError("进程树收据不是有效 JSON") from error
    if not isinstance(value, dict):
        raise ValueError("进程树收据必须是对象")
    return value


def record_process_tree(
    directory: Path,
    role: str,
    scope: str,
    supervisor: dict,
    process: dict,
    operation_id: str,
) -> dict:
    if not isinstance(scope, str) or not scope:
        raise ValueError("进程树必须绑定非空隔离 scope")
    if not isinstance(operation_id, str) or OPERATION_ID.fullmatch(operation_id) is None:
        raise ValueError("进程树操作 ID 无效")
    receipt = {
        "format_version": 1,
        "kind": "full-stack-process-tree",
        "runtime_directory": str(directory.resolve(strict=True)),
        "role": role,
        "scope_id": scope,
        "operation_id": operation_id,
        "supervisor": _valid_identity(supervisor, "监督进程"),
        "process": _valid_identity(process, "产品进程"),
        "group_id": supervisor["pid"],
    }
    write_receipt(_tree_path(directory, role), receipt)
    return receipt


def read_process_tree(directory: Path, role: str, scope: str) -> dict:
    receipt = _read_object(_tree_path(directory, role))
    if set(receipt) != {
        "format_version",
        "kind",
        "runtime_directory",
        "role",
        "scope_id",
        "operation_id",
        "supervisor",
        "process",
        "group_id",
    } or any(
        receipt.get(key) != value
        for key, value in {
            "format_version": 1,
            "kind": "full-stack-process-tree",
            "runtime_directory": str(directory.resolve(strict=True)),
            "role": role,
            "scope_id": scope,
        }.items()
    ):
        raise ValueError("进程树收据与角色或隔离 scope 不匹配")
    supervisor = _valid_identity(receipt["supervisor"], "监督进程")
    process = _valid_identity(receipt["process"], "产品进程")
    if (
        receipt["group_id"] != supervisor["pid"]
        or not isinstance(receipt["operation_id"], str)
        or OPERATION_ID.fullmatch(receipt["operation_id"]) is None
    ):
        raise ValueError("进程树收据缺少可信进程组或操作 ID")
    return {**receipt, "supervisor": supervisor, "process": process}


def _bound_path(receipt: dict, suffix: str) -> Path:
    directory = Path(receipt.get("runtime_directory", ""))
    if not directory.is_absolute() or not directory.is_dir() or directory.is_symlink():
        raise ValueError("进程树收据没有绑定可信运行目录")
    return directory / f"{receipt['role']}-tree-{suffix}.json"


def _write_control(receipt: dict, mode: str) -> None:
    if mode not in {"normal", "crash"}:
        raise ValueError("未知进程树停止模式")
    directory = Path(receipt["runtime_directory"])
    if read_process_tree(directory, receipt["role"], receipt["scope_id"]) != receipt:
        raise ValueError("进程树收据在停止前发生变化")
    write_receipt(
        _bound_path(receipt, "control"),
        {
            "format_version": 1,
            "kind": "full-stack-process-tree-control",
            "operation_id": receipt["operation_id"],
            "mode": mode,
            "requested_at_ns": time.time_ns(),
        },
    )


def _read_control(receipt: dict) -> str | None:
    path = _bound_path(receipt, "control")
    if not path.exists():
        return None
    control = _read_object(path)
    if (
        set(control)
        != {"format_version", "kind", "operation_id", "mode", "requested_at_ns"}
        or control.get("format_version") != 1
        or control.get("kind") != "full-stack-process-tree-control"
        or control.get("operation_id") != receipt["operation_id"]
        or control.get("mode") not in {"normal", "crash"}
        or type(control.get("requested_at_ns")) is not int
        or control["requested_at_ns"] <= 0
    ):
        raise ValueError("进程树停止收据与当前启动操作不匹配")
    return control["mode"]


def _write_result(receipt: dict, exit_code: int, termination: str) -> None:
    write_receipt(
        _bound_path(receipt, "result"),
        {
            "format_version": 1,
            "kind": "full-stack-process-tree-result",
            "operation_id": receipt["operation_id"],
            "process": receipt["process"],
            "exit_code": exit_code,
            "termination": termination,
        },
    )


def _read_result(receipt: dict) -> dict | None:
    path = _bound_path(receipt, "result")
    if not path.exists():
        return None
    result = _read_object(path)
    if (
        set(result)
        != {"format_version", "kind", "operation_id", "process", "exit_code", "termination"}
        or result.get("format_version") != 1
        or result.get("kind") != "full-stack-process-tree-result"
        or result.get("operation_id") != receipt["operation_id"]
        or result.get("process") != receipt["process"]
        or type(result.get("exit_code")) is not int
        or result.get("termination") not in {"natural", "normal", "forced"}
    ):
        raise ValueError("进程树结果收据与当前启动操作不匹配")
    return result


class _IoCounters(ctypes.Structure):
    _fields_ = [(name, ctypes.c_ulonglong) for name in (
        "ReadOperationCount", "WriteOperationCount", "OtherOperationCount",
        "ReadTransferCount", "WriteTransferCount", "OtherTransferCount",
    )]


class _BasicLimitInformation(ctypes.Structure):
    _fields_ = [
        ("PerProcessUserTimeLimit", ctypes.c_longlong),
        ("PerJobUserTimeLimit", ctypes.c_longlong),
        ("LimitFlags", ctypes.c_ulong),
        ("MinimumWorkingSetSize", ctypes.c_size_t),
        ("MaximumWorkingSetSize", ctypes.c_size_t),
        ("ActiveProcessLimit", ctypes.c_ulong),
        ("Affinity", ctypes.c_size_t),
        ("PriorityClass", ctypes.c_ulong),
        ("SchedulingClass", ctypes.c_ulong),
    ]


class _ExtendedLimitInformation(ctypes.Structure):
    _fields_ = [
        ("BasicLimitInformation", _BasicLimitInformation),
        ("IoInfo", _IoCounters),
        ("ProcessMemoryLimit", ctypes.c_size_t),
        ("JobMemoryLimit", ctypes.c_size_t),
        ("PeakProcessMemoryUsed", ctypes.c_size_t),
        ("PeakJobMemoryUsed", ctypes.c_size_t),
    ]


def enter_supervision() -> object:
    """在启动产品代码前建立 Job Object 或独立 Unix session。"""

    if os.name != "nt":
        pid = os.getpid()
        if os.getsid(0) != pid or os.getpgid(0) != pid:
            raise RuntimeError("Unix 全栈监督进程必须先成为独立 session 和进程组")
        return pid
    from ctypes import wintypes

    kernel = ctypes.WinDLL("kernel32", use_last_error=True)
    kernel.CreateJobObjectW.argtypes = (ctypes.c_void_p, wintypes.LPCWSTR)
    kernel.CreateJobObjectW.restype = wintypes.HANDLE
    kernel.SetInformationJobObject.argtypes = (
        wintypes.HANDLE, ctypes.c_int, ctypes.c_void_p, wintypes.DWORD,
    )
    kernel.AssignProcessToJobObject.argtypes = (wintypes.HANDLE, wintypes.HANDLE)
    kernel.GetCurrentProcess.restype = wintypes.HANDLE
    kernel.CloseHandle.argtypes = (wintypes.HANDLE,)
    handle = kernel.CreateJobObjectW(None, None)
    if not handle:
        raise ctypes.WinError(ctypes.get_last_error())
    information = _ExtendedLimitInformation()
    information.BasicLimitInformation.LimitFlags = 0x2000
    if not kernel.SetInformationJobObject(
        handle, 9, ctypes.byref(information), ctypes.sizeof(information)
    ) or not kernel.AssignProcessToJobObject(handle, kernel.GetCurrentProcess()):
        error = ctypes.get_last_error()
        kernel.CloseHandle(handle)
        raise ctypes.WinError(error)
    # 故意由监督进程终止时让系统关闭句柄；提前关闭会触发整棵树退出。
    return handle


def _linux_group(identity: dict) -> tuple[int, int] | None:
    try:
        fields = Path(f"/proc/{identity['pid']}/stat").read_text().rpartition(")")[2].split()
    except FileNotFoundError:
        return None
    return int(fields[2]), int(fields[3])


def _expected_alive(identity: dict) -> bool:
    try:
        return process_identity(identity["pid"]) == identity
    except PermissionError:
        # Windows 正在终止的 Job 成员可能短暂拒绝查询镜像路径；继续等待句柄消失。
        return True


def _wait_stopped(identities: tuple[dict, ...], timeout: float) -> bool:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if not any(_expected_alive(identity) for identity in identities):
            return True
        time.sleep(0.02)
    return not any(_expected_alive(identity) for identity in identities)


def terminate_owned_process_tree(receipt: dict, *, crash: bool = False) -> bool:
    supervisor = _valid_identity(receipt.get("supervisor"), "监督进程")
    process = _valid_identity(receipt.get("process"), "产品进程")
    group_id = receipt.get("group_id")
    if group_id != supervisor["pid"]:
        raise ValueError("进程树收据的进程组与监督进程不匹配")
    actual_supervisor = process_identity(supervisor["pid"])
    actual_process = process_identity(process["pid"])
    supervisor_alive = assert_identity(actual_supervisor, supervisor)
    process_alive = assert_identity(actual_process, process)
    if not supervisor_alive and not process_alive:
        return False
    _write_control(receipt, "crash" if crash else "normal")
    if not crash and supervisor_alive and _wait_stopped((supervisor, process), 3):
        return True
    if os.name == "nt":
        if not supervisor_alive:
            raise ValueError("Job 监督进程已退出但产品进程仍在，拒绝降级为单进程回收")
        terminate_owned_process(supervisor, crash=True)
        if not _wait_stopped((supervisor, process), 5):
            raise TimeoutError("Windows Job Object 未在期限内回收完整进程树")
        return True
    for identity, alive in ((supervisor, supervisor_alive), (process, process_alive)):
        if alive and _linux_group(identity) != (group_id, group_id):
            raise ValueError("Unix 进程已离开登记的 session 或进程组")
    try:
        os.killpg(group_id, signal.SIGKILL if crash else signal.SIGTERM)
    except ProcessLookupError:
        return False
    if not _wait_stopped((supervisor, process), 5):
        os.killpg(group_id, signal.SIGKILL)
        if not _wait_stopped((supervisor, process), 5):
            raise TimeoutError("Unix 进程组未在期限内退出")
    return True


@dataclass
class SupervisedProcess:
    supervisor: subprocess.Popen
    tree: dict

    @property
    def pid(self) -> int:
        return self.tree["process"]["pid"]

    def poll(self) -> int | None:
        result = _read_result(self.tree)
        if result is not None:
            return result["exit_code"]
        if _expected_alive(self.tree["process"]):
            return None
        return self.supervisor.poll()

    def wait(self, timeout: float) -> int:
        supervisor_code = self.supervisor.wait(timeout=timeout)
        result = _read_result(self.tree)
        return result["exit_code"] if result is not None else supervisor_code


def supervise_product(process: subprocess.Popen, receipt: dict, grace: float = 1) -> int:
    requested, forced, deadline = None, False, None
    while process.poll() is None:
        mode = _read_control(receipt)
        if mode is not None and requested is None:
            requested = mode
            if mode == "crash":
                process.kill()
                forced = True
            else:
                # Windows 无可继承的控制台和温和信号通道，Popen.terminate 等价于
                # TerminateProcess；结果必须如实标为 forced。
                process.terminate()
                forced = os.name == "nt"
                deadline = time.monotonic() + grace
        if deadline is not None and time.monotonic() >= deadline and process.poll() is None:
            process.kill()
            forced = True
            deadline = None
        time.sleep(0.02)
    code = process.wait()
    termination = "natural" if requested is None else "forced" if forced else "normal"
    _write_result(receipt, code, termination)
    return code


def launch_supervised_process(
    directory: Path,
    role: str,
    scope: str,
    arguments: list[str],
    cwd: Path,
    environment: dict[str, str],
    output,
    timeout: float = 5,
) -> SupervisedProcess:
    if not arguments or not Path(arguments[0]).is_absolute():
        raise ValueError("监督进程要求明确的绝对产品可执行文件")
    operation_id = uuid.uuid4().hex
    path = _tree_path(directory, role)
    if path.exists():
        raise ValueError("运行目录已有进程树收据；必须先核对并回收原进程树")
    command = [
        sys.executable,
        str(Path(__file__).resolve()),
        "__supervise",
        "--runtime-dir", str(directory),
        "--role", role,
        "--scope", scope,
        "--operation-id", operation_id,
        "--cwd", str(cwd),
        "--",
        *arguments,
    ]
    supervisor = subprocess.Popen(
        command,
        cwd=cwd,
        env=environment,
        stdin=subprocess.DEVNULL,
        stdout=output,
        stderr=subprocess.STDOUT,
        start_new_session=True,
        creationflags=subprocess.CREATE_NO_WINDOW if os.name == "nt" else 0,
    )
    supervisor_identity = process_identity(supervisor.pid)
    if supervisor_identity is None:
        supervisor.wait(timeout=5)
        raise RuntimeError("全栈监督进程在登记创建身份前退出")
    deadline = time.monotonic() + timeout
    try:
        while time.monotonic() < deadline:
            if path.is_file() and (directory / f"{role}.json").is_file():
                tree = read_process_tree(directory, role, scope)
                if tree["operation_id"] != operation_id:
                    raise ValueError("进程树收据属于其他启动操作")
                assert_identity(tree["supervisor"], supervisor_identity)
                assert_identity(process_identity(tree["process"]["pid"]), tree["process"])
                if read_process(directory, role, scope) != tree["process"]:
                    raise ValueError("产品进程与进程树收据不一致")
                return SupervisedProcess(supervisor, tree)
            if supervisor.poll() is not None:
                raise RuntimeError(f"全栈监督进程在登记产品进程前退出，退出码 {supervisor.returncode}")
            time.sleep(0.02)
        raise TimeoutError("全栈监督进程未在期限内登记产品进程")
    except BaseException:
        if process_identity(supervisor_identity["pid"]) == supervisor_identity:
            terminate_owned_process(supervisor_identity, crash=True)
        supervisor.wait(timeout=5)
        raise


def _supervise(arguments: list[str]) -> int:
    parser = argparse.ArgumentParser(add_help=False)
    parser.add_argument("--runtime-dir", required=True, type=Path)
    parser.add_argument("--role", required=True, choices=tuple(sorted(ROLES)))
    parser.add_argument("--scope", required=True)
    parser.add_argument("--operation-id", required=True)
    parser.add_argument("--cwd", required=True, type=Path)
    args, command = parser.parse_known_args(arguments)
    if command[:1] == ["--"]:
        command = command[1:]
    if not command:
        raise ValueError("监督进程缺少产品启动命令")
    membership = enter_supervision()
    try:
        supervisor = process_identity(os.getpid())
        if supervisor is None:
            raise RuntimeError("无法取得全栈监督进程身份")
        process = subprocess.Popen(
            command,
            cwd=args.cwd.resolve(strict=True),
            env=os.environ,
            stdin=subprocess.DEVNULL,
        )
        identity = process_identity(process.pid)
        if identity is None:
            process.wait(timeout=5)
            raise RuntimeError("产品进程在登记创建身份前退出")
        executable = Path(command[0]).resolve(strict=True)
        if Path(identity["executable"]) != executable:
            raise RuntimeError("启动产物与产品进程的实际可执行文件不一致")
        tree = record_process_tree(
            args.runtime_dir,
            args.role,
            args.scope,
            supervisor,
            identity,
            args.operation_id,
        )
        recorded = record_process(
            args.runtime_dir.resolve(strict=True),
            args.role,
            process.pid,
            command[0],
            args.scope,
        )["identity"]
        if identity != recorded:
            raise RuntimeError("产品进程在登记进程树期间身份发生变化")
        code = supervise_product(process, tree)
        return code if 0 <= code <= 255 else 1
    finally:
        # membership 必须一直存活到产品退出；Windows 由进程结束关闭 Job 句柄。
        _ = membership
        if os.name != "nt":
            os.killpg(os.getpgrp(), signal.SIGKILL)


def main() -> None:
    if sys.argv[1:2] != ["__supervise"]:
        raise SystemExit("full_stack_process_tree.py 只供全栈内部监督使用")
    raise SystemExit(_supervise(sys.argv[2:]))


if __name__ == "__main__":
    main()
