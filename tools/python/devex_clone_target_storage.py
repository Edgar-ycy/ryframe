"""只读核验本次存储启动收据、实际参数和有效目录；不启动或终止存储服务。"""
from __future__ import annotations

import ctypes
import hashlib
import json
import os
from pathlib import Path
import subprocess
import uuid
from urllib.parse import urlsplit

from devex_clone import read_json
from devex_clone_capture import write_json
from devex_clone_model import exact, local_path
from devex_clone_source_proof import bound_file
from full_stack_process import process_identity
from process_sockets import verify_listener
from restore_reference_plan import plan_hash


def windows_argv(command_line: str) -> list[str]:
    if os.name != "nt":
        raise ValueError("RustFS 原生进程参数核验仅用于本机 Windows")
    shell = ctypes.WinDLL("shell32", use_last_error=True)
    kernel = ctypes.WinDLL("kernel32", use_last_error=True)
    shell.CommandLineToArgvW.argtypes = [ctypes.c_wchar_p, ctypes.POINTER(ctypes.c_int)]
    shell.CommandLineToArgvW.restype = ctypes.POINTER(ctypes.c_wchar_p)
    kernel.LocalFree.argtypes = [ctypes.c_void_p]
    kernel.LocalFree.restype = ctypes.c_void_p
    count = ctypes.c_int()
    pointer = shell.CommandLineToArgvW(command_line, ctypes.byref(count))
    if not pointer:
        raise ctypes.WinError(ctypes.get_last_error())
    try:
        return [pointer[index] for index in range(count.value)]
    finally:
        kernel.LocalFree(pointer)


def actual_windows_argv(resources, identity: dict) -> list[str]:
    if os.name != "nt" or process_identity(identity["pid"]) != identity:
        raise ValueError("读取参数前 RustFS 内核身份不符")
    executable = Path(os.environ["SystemRoot"]) / "System32/WindowsPowerShell/v1.0/powershell.exe"
    script = ("$ErrorActionPreference='Stop'; [Console]::OutputEncoding=[System.Text.UTF8Encoding]::new($false); "
              f"(Get-CimInstance Win32_Process -Filter 'ProcessId = {identity['pid']}').CommandLine | ConvertTo-Json -Compress")
    result = resources.runner([str(executable), "-NoProfile", "-NonInteractive", "-Command", script],
                              stdout=subprocess.PIPE, stderr=subprocess.PIPE, check=True, timeout=15,
                              creationflags=subprocess.CREATE_NO_WINDOW if os.name == "nt" else 0)
    value = json.loads(result.stdout.decode("utf-8-sig"))
    if not isinstance(value, str) or not value or process_identity(identity["pid"]) != identity:
        raise ValueError("RustFS 参数读取窗口发生进程变化")
    # 不保存可能属于错误进程的原始命令行，只在与明确参数相等后保存摘要。
    return windows_argv(value)


def verify_storage_generation(expected: dict, actual: dict, runtime: dict | None,
                              cache_runtime: dict | None) -> None:
    """历史初始化保持原值；仅接受显式恢复所登记的存储运行身份变化。"""
    if actual == expected:
        return
    storage = dict(expected["storage"])
    if runtime is not None:
        storage["rustfs"] = runtime["storage"]
    if cache_runtime is not None:
        storage["redis"] = cache_runtime["redis"]
    allowed = {**expected, "storage": storage}
    if actual != allowed:
        raise ValueError("已登记存储恢复之外的目标源码、配置或资源代次发生变化")


def rustfs_layout(resources, rustfs: dict, *, runtime: dict | None = None) -> dict:
    backend = resources.backend
    expected = resources.review["services"]["rustfs"]
    receipt_path = bound_file(backend, rustfs["process_receipt"])
    expected_path = expected["process_receipt"] if runtime is None else runtime["storage"]["process_receipt"]["path"]
    if str(receipt_path) != expected_path:
        raise ValueError("RustFS 进程收据不是审阅计划路径")
    receipt = read_json(receipt_path)
    stable = {"format_version": 1, "role": "rustfs", "scope_id": expected["scope_id"], "lifecycle": "running",
              "identity": rustfs["identity"], "api_url": expected["api"], "console_url": expected["console"], "data_dir": expected["data_dir"]}
    if any(receipt.get(key) != value for key, value in stable.items()):
        raise ValueError("RustFS 实际启动目录或进程收据不同")
    directory = local_path(backend, expected["data_dir"])
    if not directory.is_dir():
        raise ValueError("RustFS 明确数据目录不存在")
    if runtime is not None:
        identity = {"path": str(directory), "device": directory.stat().st_dev, "inode": directory.stat().st_ino}
        if (runtime["storage"] != rustfs or runtime["data_directory"] != identity
                or runtime["api_url"] != expected["api"] or runtime["console_url"] != expected["console"]):
            raise ValueError("已发布存储重启不是原物理目录、端口或本次运行身份")
    launch_path = bound_file(backend, rustfs["launch_receipt"])
    if launch_path != receipt_path.parent / "launch.json":
        raise ValueError("RustFS launch 必须属于同一存储目录")
    launch = read_json(launch_path)
    exact(launch, {"format_version", "scope_id", "identity", "arguments", "environment", "credential_files"})
    exact(launch["credential_files"], {"access_key", "secret_key"})
    for field, binding in launch["credential_files"].items():
        credential = bound_file(backend, binding)
        actual = os.environ.get(resources.request["target"]["s3"][field + "_env"])
        if credential.read_text(encoding="utf-8").strip() != actual:
            raise ValueError("RustFS 启动凭据文件与本次认证不同")
    environment = {"RUSTFS_CONSOLE_ENABLE": "true", **{"RUSTFS_" + field.upper() + "_FILE": value["path"]
                   for field, value in launch["credential_files"].items()}}
    urls = [urlsplit(expected[field]) for field in ("api", "console")]
    args = [rustfs["identity"]["executable"], "server", "--address", f"{urls[0].hostname}:{urls[0].port}",
            "--console-address", f"{urls[1].hostname}:{urls[1].port}", str(directory)]
    if (launch["format_version"] != 1 or launch["scope_id"] != expected["scope_id"] or launch["identity"] != rustfs["identity"]
            or launch["arguments"] != args or launch["environment"] != environment or actual_windows_argv(resources, rustfs["identity"]) != args):
        raise ValueError("RustFS 当前完整参数或受控启动声明不符")
    verify_listener(rustfs["identity"]["pid"], expected["console"])
    result = {"process_receipt": rustfs["process_receipt"], "launch_receipt": rustfs["launch_receipt"],
              "actual_arguments_sha256": plan_hash(args), "data_dir": str(directory),
              "environment_proof": "受控启动收据与凭据文件；不声称从内核枚举进程环境"}
    if runtime is not None:
        result["runtime_transition"] = runtime
    write_json(resources.output / ("storage-layout-" + uuid.uuid4().hex + ".json"), result)
    return result


def wsl_path(path: Path) -> str:
    if len(path.drive) != 2 or path.drive[1] != ":":
        raise ValueError("Redis 配置须是本机 Windows 盘符目录")
    return "/mnt/" + path.drive[0].lower() + str(path)[2:].replace("\\", "/")


def redis_layout(resources, redis: dict, info: dict, observe) -> dict:
    directory = local_path(resources.backend, resources.review["services"]["redis"]["directory"])
    config = bound_file(resources.backend, redis["configuration"])
    if config != directory / "redis.conf":
        raise ValueError("Redis 配置不是审阅目录中的 redis.conf")
    linux_file, linux_directory = wsl_path(config), wsl_path(directory)
    if info.get("config_file") != linux_file or observe(["/usr/bin/readlink", "-f", linux_file]) != linux_file:
        raise ValueError("Redis 运行配置文件或 WSL 实际路径不同")
    actual_hash = observe(["/usr/bin/sha256sum", linux_file]).split()[0]
    if actual_hash != redis["configuration"]["sha256"]:
        raise ValueError("Redis 运行配置字节变化")
    expected = {"bind": "127.0.0.1", "port": str(redis["port"]), "dir": linux_directory, "databases": "1",
                "protected-mode": "yes", "save": "", "appendonly": "no"}
    for key, value in expected.items():
        if resources._redis(["CONFIG", "GET", key]) != [key, value]:
            raise ValueError("Redis 有效目录、端口、database 或持久化配置不同")
    return {"configuration": redis["configuration"], "effective_configuration_sha256": plan_hash(expected),
            "config_file": linux_file, "file_sha256": hashlib.sha256(config.read_bytes()).hexdigest()}
