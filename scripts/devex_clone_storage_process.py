"""RustFS 精确启动和回收；原始进程收据保持只读，失败不按 PID 猜测归属。"""
from __future__ import annotations

import os
from pathlib import Path
import subprocess
import time
from types import SimpleNamespace

from devex_clone_capture import read_json, write_json
from devex_clone_model import exact
from devex_clone_run_state import binding
from devex_clone_source_proof import bound_file
from devex_clone_storage_request import arguments, configuration, identity
from devex_clone_target_storage import actual_windows_argv
from full_stack_process import process_identity, terminate_owned_process
from process_sockets import verify_listener


def actual_arguments(expected: dict, args: list[str]) -> None:
    observed = actual_windows_argv(SimpleNamespace(runner=subprocess.run), expected)
    if observed != args:
        raise ValueError("存储进程当前参数不是已登记的完整命令")


def running(expected: dict, args: list[str], urls: tuple[str, str], *, listeners=True) -> bool:
    current = process_identity(identity(expected)["pid"])
    if current is None:
        return False
    if current != expected:
        raise ValueError("存储 PID 已被复用，拒绝操作其他内核代次")
    actual_arguments(expected, args)
    if listeners:
        for url in urls:
            verify_listener(expected["pid"], url)
    if process_identity(expected["pid"]) != expected:
        raise ValueError("存储参数和监听核对期间内核代次变化")
    return True


def inspect_attempt(backend: Path, output: Path, registration: dict, controller: dict, number: int,
                    *, request_binding: dict | None = None) -> dict | None:
    path = output / "intent.json"
    if not path.exists():
        if any((output / name).exists() for name in ("process.json", "launch.json", "ready.json", "spawned.json")):
            raise ValueError("存储进程证据缺少启动意图，拒绝猜测来源")
        return None
    intent = read_json(path)
    exact(intent, {"format_version", "kind", "request", "controller", "attempt", "arguments", "environment"})
    request_path = bound_file(backend, request_binding or registration)
    if request_binding is not None and request_path != output / "request.json":
        raise ValueError("服务账本嵌入请求必须位于原始启动目录")
    request = read_json(request_path)
    if (intent["format_version"] != 1 or intent["kind"] != "devex-clone-storage-intent"
            or intent["request"] != registration or intent["controller"] != controller or intent["attempt"] != number
            or intent["arguments"] != arguments(request) or intent["environment"] != configuration(request)):
        raise ValueError("存储意图不属于原注册与对应控制阶段")
    bound_file(backend, controller)
    receipt = output / "process.json"
    if not receipt.exists():
        closed = output / "not-started.json"
        if closed.exists() and read_json(closed) == {"intent": binding(path), "popen_called": False}:
            return {"state": "not_started", "intent": binding(path)}
        cleanup_path = output / "failure-cleanup.json"
        if cleanup_path.exists():
            cleanup = read_json(cleanup_path)
            exact(cleanup, {"intent", "identity", "pid", "returncode"})
            if (cleanup["intent"] != binding(path) or type(cleanup["pid"]) is not int or cleanup["pid"] <= 1
                    or type(cleanup["returncode"]) is not int or cleanup["identity"] is not None
                    and identity(cleanup["identity"])["pid"] != cleanup["pid"]
                    or process_identity(cleanup["pid"]) is not None):
                raise ValueError("启动失败回收证据不完整或退出进程 PID 已被复用")
            return {"state": "not_started", "intent": binding(path), "cleanup": binding(cleanup_path)}
        raise ValueError("存储启动意图缺少完整创建身份；必须核实现场，不能重启或按 PID 回收")
    process = read_json(receipt)
    exact(process, {"format_version", "role", "scope_id", "lifecycle", "identity", "api_url", "console_url", "data_dir", "intent"})
    stable = {"format_version": 1, "role": "rustfs", "scope_id": request["scope_id"], "lifecycle": "running",
              "api_url": request["api_url"], "console_url": request["console_url"],
              "data_dir": request["data_directory"]["path"], "intent": binding(path)}
    if any(process[key] != item for key, item in stable.items()) or identity(process["identity"])["executable"] != request["executable"]["path"]:
        raise ValueError("存储进程收据不属于该启动意图或实际工具")
    # 启动早期失败可能只有 durable process，清理不能依赖稍后才会出现的 launch/ready。
    launch_path = output / "launch.json"
    if launch_path.exists():
        launch = read_json(launch_path)
        expected = {"format_version": 1, "scope_id": request["scope_id"], "identity": process["identity"],
                    "arguments": arguments(request), "environment": configuration(request), "credential_files": request["credential_files"]}
        if launch != expected:
            raise ValueError("存储启动参数收据变化")
    return {"state": "recorded", "identity": process["identity"], "process_receipt": binding(receipt),
            "launch_receipt": binding(launch_path) if launch_path.exists() else None, "intent": binding(path)}


def stop_record(request: dict, observed: dict, output: Path) -> dict:
    if observed["state"] == "not_started":
        return {"state": "stopped", "identity": None, "terminated": False, "process_receipt": None}
    expected = observed["identity"]
    urls = request["api_url"], request["console_url"]
    alive = running(expected, arguments(request), urls, listeners=False)
    terminated = terminate_owned_process(expected) if alive else False
    if process_identity(expected["pid"]) is not None:
        raise ValueError("存储回收后仍存在进程或 PID 已被复用")
    result = {"state": "stopped", "identity": expected, "terminated": terminated,
              "process_receipt": observed["process_receipt"]}
    write_json(output / "stopped.json", result)
    return result


def wait_ready(child, expected: dict, request: dict) -> None:
    deadline = time.monotonic() + request["timeout_seconds"]
    while time.monotonic() < deadline:
        if child.poll() is not None or process_identity(child.pid) != expected:
            raise RuntimeError("RustFS 就绪前退出或实际身份变化")
        try:
            for key in ("api_url", "console_url"):
                verify_listener(child.pid, request[key])
            return
        except ValueError:
            time.sleep(0.1)
    raise TimeoutError("RustFS 未在明确时限内监听登记端口")


def start(backend: Path, request: dict, environment: dict, output: Path, registration: dict,
          controller: dict, number: int, guard, *, supervised: bool = False) -> dict:
    args, config = arguments(request), configuration(request)
    write_json(output / "intent.json", {"format_version": 1, "kind": "devex-clone-storage-intent", "request": registration,
               "controller": controller, "attempt": number, "arguments": args, "environment": config})
    child, expected, popen_called = None, None, False
    try:
        # 不继承任何未登记 RUSTFS 开关；凭据只以原文件路径传入子进程。
        private = {key: item for key, item in environment.items() if not key.upper().startswith("RUSTFS_")}
        private.update(config)
        with (output / "stdout.log").open("xb") as stdout, (output / "stderr.log").open("xb") as stderr:
            popen_called = True
            if supervised:
                from full_stack_process_tree import launch_supervised_process

                child = launch_supervised_process(output, "rustfs", request["scope_id"], args, output, private, stdout,
                                                  timeout=request["timeout_seconds"])
            else:
                child = subprocess.Popen(args, cwd=output, env=private, stdin=subprocess.DEVNULL, stdout=stdout, stderr=stderr,
                                         creationflags=subprocess.CREATE_NO_WINDOW if os.name == "nt" else 0)
        write_json(output / "spawned.json", {"pid": child.pid, "identity_pending": True})
        actual = process_identity(child.pid)
        if actual is None or actual["executable"] != request["executable"]["path"]:
            raise ValueError("新存储实际可执行文件与注册产物不符")
        expected = actual
        write_json(output / "process.json", {"format_version": 1, "role": "rustfs", "scope_id": request["scope_id"],
                   "lifecycle": "running", "identity": expected, "api_url": request["api_url"], "console_url": request["console_url"],
                   "data_dir": request["data_directory"]["path"], "intent": binding(output / "intent.json")})
        write_json(output / "launch.json", {"format_version": 1, "scope_id": request["scope_id"], "identity": expected,
                   "arguments": args, "environment": config, "credential_files": request["credential_files"]})
        actual_arguments(expected, args)
        wait_ready(child, expected, request)
        guard()
        if not running(expected, args, (request["api_url"], request["console_url"])):
            raise ValueError("存储最终就绪观察已退出")
        result = {"identity": expected, "sha256": request["executable"]["sha256"],
                  "process_receipt": binding(output / "process.json"), "launch_receipt": binding(output / "launch.json")}
        if supervised:
            result["tree"] = binding(output / "rustfs-tree.json")
        write_json(output / "ready.json", {"storage": result, "request": registration, "controller": controller,
                   "attempt": number, "listeners": [request["api_url"], request["console_url"]]})
        return result
    except BaseException as original:
        try:
            if child is None:
                if not popen_called:
                    write_json(output / "not-started.json", {"intent": binding(output / "intent.json"), "popen_called": False})
            else:
                if supervised:
                    from full_stack_process_tree import terminate_owned_process_tree

                    terminate_owned_process_tree(child.tree, crash=True)
                elif expected is not None:
                    terminate_owned_process(expected)
                elif child.poll() is None:
                    # 仅使用刚创建的 Popen 内核句柄；不能从未确认的 PID 重新寻找进程。
                    child.kill()
                code = child.wait(timeout=5)
                write_json(output / "failure-cleanup.json", {"intent": binding(output / "intent.json"),
                           "identity": expected, "pid": child.pid, "returncode": code})
        except BaseException as cleanup:
            original.add_note("存储启动精确回收失败：" + type(cleanup).__name__)
        raise
