"""参考恢复演练的显式资源计划与文件边界；只描述已初始化的隔离资源。"""

from __future__ import annotations

import subprocess
from pathlib import Path

from native_s3_client import _invoke, _read_response

BUCKETS = {"uploads", "avatar", "exports", "imports", "config-packages"}
EXCLUDED_TABLES = {"ryframe_resource_ownership", "sys_backup_set", "sys_backup_resource", "sys_restore_run"}


def _native(operation: str, arguments: dict):
    response, _ = _read_response(*_invoke(
        "__source-evidence",
        {"operation": operation, "arguments": arguments, "write": False},
        subprocess.run,
        may_write=False,
    ))
    return response["value"]


def identifier(value: str) -> str:
    return _native("reference-identifier", {"value": value})


def scope_identifier(value: str) -> str:
    return _native("reference-scope-identifier", {"value": value})


def safe_file(root: Path, relative: str, *, exists: bool = True) -> Path:
    return Path(_native("reference-safe-file", {
        "root": str(root), "relative": relative, "exists": exists,
    }))


def plan_hash(plan: dict) -> str:
    return _native("reference-plan-hash", {"value": plan})


def dataset_timeout_seconds(plan: dict) -> int:
    return _native("reference-dataset-timeout", {"plan": plan})


def validate_configuration_binding(binding: dict, backend: Path) -> None:
    _native("reference-configuration-binding", {
        "backend": str(backend), "binding": binding, "directory": None, "app_env": None,
    })


def configuration_binding(backend: Path, directory: Path, app_env: str = "test") -> dict:
    """构造不含凭据值的应用配置文件绑定。"""
    return _native("reference-configuration-binding", {
        "backend": str(backend), "binding": None, "directory": str(directory), "app_env": app_env,
    })


def source_reference_projection(backend: Path, request: dict) -> dict:
    """从原始私有请求绑定取得对象端口的公开来源与工具投影。"""
    return _native("source-reference-projection", {"backend": str(backend), "request": request})


def validate_plan(plan: dict, backend: Path) -> None:
    _native("reference-plan-validate", {"plan": plan, "backend": str(backend)})


def validate_inventory(plan: dict, inventory: dict) -> None:
    _native("reference-inventory-validate", {"plan": plan, "inventory": inventory})


def verify_artifacts(root: Path, manifest: dict) -> None:
    _native("reference-artifacts-verify", {"root": str(root), "manifest": manifest})
