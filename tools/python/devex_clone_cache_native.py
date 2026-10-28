"""缓存进程证据和控制调用的有界 Rust 传输；不重试，不读取环境凭据。"""
from __future__ import annotations

import json
import os
import subprocess
from subprocess import run as run_native

from devex_clone_capture import unique_object
from rate_limit_client import ROOT, _child_environment
from rate_limit_tool import tool_binding, verified_tool
from process_environment import configured


def invoke(operation, **arguments):
    try:
        payload = json.dumps({"format_version": 1, "kind": "ryframe-cache-process",
                              "operation": operation, "arguments": arguments},
                             ensure_ascii=False, separators=(",", ":")).encode("utf-8")
    except (TypeError, ValueError, UnicodeError):
        raise ValueError("缓存原生请求必须是有效 UTF-8 JSON") from None
    if len(payload) > 1024 * 1024:
        raise ValueError("缓存原生请求超过 1 MiB")
    options = {"creationflags": subprocess.CREATE_NO_WINDOW} if os.name == "nt" else {}
    try:
        with verified_tool(tool_binding()) as executable:
            if executable is None:
                raise ValueError("缓存控制必须由正式入口绑定原生工具；不能临时编译或猜测工具")
            result = run_native([executable, "__cache-process"], cwd=ROOT,
                                    env=configured({}, inherited=_child_environment()), input=payload,
                                    stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
                                    timeout={"preflight-tools": 110, "observe": 100, "stop": 80}.get(operation, 30), check=False, **options)
        if len(result.stdout) > 1024 * 1024:
            raise ValueError("缓存原生响应超过 1 MiB")
        response = json.loads(result.stdout.decode("utf-8"), object_pairs_hook=unique_object)
    except (OSError, subprocess.SubprocessError, UnicodeError, json.JSONDecodeError):
        raise RuntimeError("缓存原生调用未完成；保留意图与锁，不重放任何写入") from None
    if (result.returncode == 1 and isinstance(response, dict)
            and set(response) == {"ok", "error", "message", "exit_code"} and response["ok"] is False
            and response["error"] in {"CacheProcessRejected", "CacheControlFailed", "CacheConnectionRefused", "CacheObservationTimeout"}
            and isinstance(response["message"], str)):
        code = response["exit_code"]
        if (code is not None and (type(code) is not int or code == 0 or not -(2 ** 31) <= code < 2 ** 31)
                or response["error"] != "CacheControlFailed" and code is not None):
            raise RuntimeError("缓存原生退出状态无效；保留现场，禁止重放")
        if response["error"] == "CacheConnectionRefused":
            raise ConnectionRefusedError(response["message"])
        if response["error"] == "CacheObservationTimeout":
            raise TimeoutError(response["message"])
        message = response["message"] + (" 原始退出码：" + str(code) if code is not None else "")
        if response["error"] == "CacheControlFailed":
            raise RuntimeError(message)
        raise ValueError(message)
    if (result.returncode != 0 or not isinstance(response, dict)
            or set(response) != {"ok", "result"} or response["ok"] is not True):
        raise RuntimeError("缓存原生响应无效；保留现场，不自动重放或清理")
    return response["result"]


def _wire(value):
    from pathlib import Path
    if isinstance(value, Path):
        return str(value)
    if isinstance(value, dict):
        return {key: _wire(item) for key, item in value.items()}
    if isinstance(value, (tuple, list)):
        return [_wire(item) for item in value]
    return value


def stage_session(action, directory, parameters, port):
    """复用缓存协议，在原锁存续期间按 Rust 请求执行单次 I/O。"""
    payload = {"format_version": 1, "kind": "ryframe-cache-process", "operation": "stage",
               "arguments": {"action": action, "directory": str(directory), "caller": os.getpid(),
                             "parameters": parameters}}
    return exchange(payload, port)


def exchange(payload, port):
    """仅转发原生阶段消息；授权与生命周期由 Rust 决定。"""
    import queue
    import threading
    def send(child, value):
        data = json.dumps(_wire(value), ensure_ascii=False, separators=(",", ":")).encode("utf-8") + b"\n"
        if len(data) > 1024 * 1024:
            raise ValueError("缓存阶段消息超过 1 MiB")
        child.stdin.write(data)
        child.stdin.flush()
    options = {"creationflags": subprocess.CREATE_NO_WINDOW} if os.name == "nt" else {}
    with verified_tool(tool_binding()) as executable:
        if executable is None:
            raise ValueError("缓存阶段必须使用正式入口绑定的原生工具")
        child = subprocess.Popen([executable, "__cache-process"], cwd=ROOT,
            env=configured({}, inherited=_child_environment()), stdin=subprocess.PIPE,
            stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, **options)
        messages = queue.Queue(maxsize=1)
        def receive():
            try:
                while True:
                    line = child.stdout.readline(1024 * 1024 + 1)
                    messages.put(line)
                    if not line:
                        return
            except OSError:
                messages.put(b"")
        reader = threading.Thread(target=receive, daemon=True)
        reader.start()
        pending = None
        sequence = 0
        wait_seconds = 100 if payload["operation"] == "stage" else 50
        try:
            send(child, payload)
            while True:
                try:
                    line = messages.get(timeout=wait_seconds)
                except queue.Empty:
                    raise TimeoutError("缓存阶段无响应；保留意图、锁及未知写入，禁止重放") from None
                if not line or len(line) > 1024 * 1024:
                    raise RuntimeError("缓存阶段响应缺失或超限；保留现场")
                response = json.loads(line.decode("utf-8"), object_pairs_hook=unique_object)
                if isinstance(response, dict) and set(response) == {"port", "arguments", "sequence"}:
                    sequence += 1
                    if type(response["sequence"]) is not int or response["sequence"] != sequence or pending is not None:
                        raise RuntimeError("缓存阶段端口失序或失败后仍请求执行")
                    try:
                        result = port(response["port"], response["arguments"])
                    except BaseException as error:
                        pending = error
                        send(child, {"sequence": sequence, "ok": False, "result": None})
                    else:
                        send(child, {"sequence": sequence, "ok": True, "result": result})
                    if response["port"] == "checkpoint":
                        request = (response["arguments"]["process"] if response["arguments"] is not None
                                   else payload["arguments"]["request"])
                        timeout = request["timeout_seconds"]
                        if type(timeout) is not int or not 1 <= timeout <= 180:
                            raise ValueError("缓存阶段期限不是已核对的请求预算")
                        # 启动期限 + 一次完整观察(20+30+20) + 原失败回收较大分支 + 原30s传输余量。
                        wait_seconds = timeout + 70 + max(50, timeout + 20) + 30
                    else:
                        wait_seconds = 100
                    continue
                child.stdin.close()
                code = child.wait(timeout=5)
                if pending is not None:
                    raise pending
                if code == 0 and isinstance(response, dict) and set(response) == {"ok", "result"} and response["ok"] is True:
                    return response["result"]
                if code == 1 and isinstance(response, dict) and set(response) == {"ok", "error", "message", "exit_code"} and response["ok"] is False and response["error"] in {"CacheProcessRejected", "CacheObservationTimeout", "CacheControlFailed"} and isinstance(response["message"], str):
                    status = response["exit_code"]
                    if status is not None and (type(status) is not int or status == 0 or not -(2 ** 31) <= status < 2 ** 31):
                        raise RuntimeError("缓存阶段退出码无效；保留现场")
                    if response["error"] != "CacheControlFailed" and status is not None:
                        raise RuntimeError("缓存阶段错误类别与退出码不同")
                    if response["error"] == "CacheControlFailed":
                        raise RuntimeError(response["message"] + " 原始退出码：" + str(status))
                    if response["error"] == "CacheObservationTimeout":
                        raise TimeoutError(response["message"])
                    raise ValueError(response["message"])
                raise RuntimeError("缓存阶段响应或退出状态无效；禁止重放")
        finally:
            if child.poll() is None:
                child.kill()
            child.wait(timeout=5)
            if not child.stdin.closed:
                child.stdin.close()
            child.stdout.close()
            reader.join(timeout=1)


def images(markers):
    return tuple(invoke('images', markers=markers))
