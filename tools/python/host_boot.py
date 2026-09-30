"""主机启动证明：用系统启动时刻判定已登记进程代次是否必然已经退出。

证明只使用内核提供的运行时长或启动时间，不按 PID 猜测进程是否存在。留出固定
余量后仍早于主机启动的创建身份，必然随重启退出；晚于主机启动的身份才是当前
代次。余量用于吸收时钟校正和休眠计时差异，缺失余量一律按未证明处理。
"""

from __future__ import annotations

import ctypes
from datetime import datetime, timedelta, timezone
import os
from pathlib import Path

from devex_clone_model import exact

TICKS_PER_SECOND = 10_000_000
EPOCH = datetime(1601, 1, 1, tzinfo=timezone.utc)
DEFAULT_MARGIN_SECONDS = 300
PROOF_FIELDS = {"format_version", "kind", "source", "margin_seconds", "observed_ticks",
                "uptime_ticks", "boot_ticks", "boot_time"}
IDENTITY_FIELDS = {"pid", "started", "executable"}
SOURCES = {"GetTickCount64", "/proc/stat"}


def _kernel32():
    kernel = ctypes.WinDLL("kernel32", use_last_error=True)
    kernel.GetSystemTimeAsFileTime.argtypes = (ctypes.c_void_p,)
    kernel.GetSystemTimeAsFileTime.restype = None
    kernel.GetTickCount64.argtypes = ()
    kernel.GetTickCount64.restype = ctypes.c_ulonglong
    return kernel


def now_ticks() -> int:
    """当前 UTC 时刻，按登记身份使用的 100 纳秒刻度表示。"""
    if os.name == "nt":
        value = ctypes.c_ulonglong()
        _kernel32().GetSystemTimeAsFileTime(ctypes.byref(value))
        return int(value.value)
    unix_seconds = datetime.now(timezone.utc).timestamp()
    return int((unix_seconds + 11_644_473_600) * TICKS_PER_SECOND)


def uptime_seconds() -> float:
    """系统运行时长；Windows 计时包含休眠，Linux 由内核记录启动时刻推导。"""
    if os.name == "nt":
        return float(_kernel32().GetTickCount64()) / 1000
    return _linux_uptime()


def _linux_uptime() -> float:
    for line in Path("/proc/stat").read_text(encoding="utf-8").splitlines():
        if line.startswith("btime "):
            fields = line.split()
            return datetime.now(timezone.utc).timestamp() - float(fields[1])
    raise ValueError("主机启动证明缺少 /proc/stat 启动时刻")


def host_boot_proof(margin_seconds: int = DEFAULT_MARGIN_SECONDS, *,
                    observed: int | None = None, uptime: float | None = None) -> dict:
    """采集一次主机启动证明；参数只供测试注入确定值。"""
    if type(margin_seconds) is not int or margin_seconds < 0:
        raise ValueError("主机启动证明余量必须是非负整数")
    observed_ticks = now_ticks() if observed is None else observed
    running = uptime_seconds() if uptime is None else uptime
    if type(observed_ticks) is not int or observed_ticks <= 0:
        raise ValueError("主机启动证明的当前时刻无效")
    if isinstance(running, bool) or not isinstance(running, (int, float)) or running <= 0:
        raise ValueError("主机启动证明的系统运行时长无效")
    running_ticks = int(float(running) * TICKS_PER_SECOND)
    boot = observed_ticks - running_ticks
    if boot <= 0 or boot >= observed_ticks:
        raise ValueError("主机启动证明推导出的启动时刻超出有效范围")
    return {"format_version": 1, "kind": "host-boot-proof",
            "source": "GetTickCount64" if os.name == "nt" else "/proc/stat",
            "margin_seconds": margin_seconds, "observed_ticks": observed_ticks,
            "uptime_ticks": running_ticks, "boot_ticks": boot,
            "boot_time": (EPOCH + timedelta(microseconds=boot // 10)).isoformat()}


def verify_host_boot_proof(value: dict, *, max_age_seconds: int | None = None,
                           observed: int | None = None) -> dict:
    """复核已记录的主机启动证明；默认要求它来自当前主机并仍然新鲜。"""
    exact(value, PROOF_FIELDS)
    margin = value["margin_seconds"]
    if (type(value["format_version"]) is not int or value["format_version"] != 1
            or value["kind"] != "host-boot-proof" or value["source"] not in SOURCES
            or type(margin) is not int or margin < 0
            or type(value["observed_ticks"]) is not int or value["observed_ticks"] <= 0
            or type(value["boot_ticks"]) is not int or value["boot_ticks"] <= 0
            or not 0 < value["boot_ticks"] < value["observed_ticks"]
            or type(value["uptime_ticks"]) is not int or value["uptime_ticks"] <= 0
            or value["observed_ticks"] - value["boot_ticks"] != value["uptime_ticks"]
            or value["boot_time"] != (EPOCH + timedelta(microseconds=value["boot_ticks"] // 10)).isoformat()):
        raise ValueError("主机启动证明字段、类型或时刻推导不一致")
    current = now_ticks() if observed is None else observed
    if max_age_seconds is not None:
        if type(max_age_seconds) is not int or max_age_seconds < 0:
            raise ValueError("主机启动证明有效期必须是非负整数")
        age = current - value["observed_ticks"]
        if age > max_age_seconds * TICKS_PER_SECOND or age < -60 * TICKS_PER_SECOND:
            raise ValueError("主机启动证明已经过期或不属于当前主机时刻")
    return value


def identity_ticks(identity: dict) -> int:
    """登记创建身份的刻度；拒绝缺失、浮点或非数字证据。"""
    exact(identity, IDENTITY_FIELDS)
    started = identity["started"]
    if (type(identity["pid"]) is not int or identity["pid"] <= 1
            or not isinstance(started, str) or not started.isdigit()
            or not isinstance(identity["executable"], str) or not identity["executable"]):
        raise ValueError("登记进程身份缺少完整创建证据")
    return int(started)


def provably_exited(identity: dict, proof: dict) -> bool:
    """创建身份早于主机启动（含余量）时，该进程必然已随重启退出。"""
    verify_host_boot_proof(proof)
    margin = proof["margin_seconds"] * TICKS_PER_SECOND
    return identity_ticks(identity) < proof["boot_ticks"] - margin


def provably_started_after_boot(identity: dict, proof: dict) -> bool:
    """创建身份晚于主机启动（含余量）时，该进程只能是重启后的新代次。"""
    verify_host_boot_proof(proof)
    margin = proof["margin_seconds"] * TICKS_PER_SECOND
    return identity_ticks(identity) > proof["boot_ticks"] + margin
