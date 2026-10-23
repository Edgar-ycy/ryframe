"""完整来源清单的临时原生协议适配；通用 Git 与 JSON 摘要仍供当前调用方使用。"""
from __future__ import annotations

import hashlib
import json
import os
from pathlib import Path
import subprocess


def git(root: Path, *arguments: str) -> bytes:
    return subprocess.check_output(["git", "-C", str(root), *arguments],
                                   env={**os.environ, "GIT_OPTIONAL_LOCKS": "0"})


def snapshot(root: Path) -> tuple[dict, bytes]:
    value = _native("snapshot", root=str(root))
    return value["snapshot"], bytes(value["patch"])


def source_snapshot(root: Path) -> dict:
    return _native("source-snapshot", root=str(root))


def worktree_fingerprint(root: Path, commit: str) -> str:
    return _native("worktree-fingerprint", root=str(root), commit=commit)


def canonical_digest(value: object) -> str:
    return hashlib.sha256(json.dumps(value, ensure_ascii=False, sort_keys=True,
                                    separators=(",", ":")).encode()).hexdigest()


def _native(action: str, **arguments):
    # 沿用协调器的二进制 identity 校验及有界 stdin，不保留 Python 算法回退。
    from native_s3_client import _invoke, _read_response

    result, response = _invoke("__source-evidence", {
        "operation": "source-inventory", "write": False,
        "arguments": {"action": action, **arguments},
    }, subprocess.run, may_write=False)
    return _read_response(result, response)[0]["value"]


def source_domain(relative: str) -> str:
    return _native("domain", path=relative)


def fingerprints(inventory: dict) -> dict:
    return _native("fingerprints", files=inventory["files"])


def validate_inventory(inventory: object) -> dict:
    _native("validate", inventory=inventory)
    return inventory


def build_source_domains(inventory: dict, repository: str) -> dict:
    return _native("domains", inventory=inventory, repository=repository)


def validate_build_source_domains(value: object, repository: str) -> dict:
    _native("validate-domains", value=value, repository=repository)
    return value


def file_inventory(root: Path) -> dict:
    return _native("file-inventory", root=str(root))


def capture_inventory(root: Path, source: dict | None = None) -> dict:
    return _native("capture", root=str(root), source=source)
