"""绑定全栈源码、构建产物与运行收据，拒绝跨来源复用。"""

from __future__ import annotations

import hashlib
import json
import os
from pathlib import Path
import re
import subprocess

from artifact_digests import file_digest
from full_stack_process import write_receipt
from source_inventory import git, source_domain
import native_s3_client as native


SHA256_PATTERN = re.compile(r"^[a-f0-9]{64}$")
COMMIT_PATTERN = re.compile(r"^[a-f0-9]{40}$")
BUILD_EVIDENCE = "build-evidence.json"
RUNTIME_EVIDENCE = "runtime-evidence.json"
RUNTIME_RECEIPT = "runtime.json"
SOURCE_PAIR_RECEIPT = "source-pair.json"
FIXTURE_RECEIPT = "fixture.json"
REFERENCE_FIXTURE_SOURCE_PAIR = "reference-fixture-source-pair"
BINARY_ROLES = frozenset({"ryframe", "ryframe-worker", "ryframe-reset", "ryframe-migrate"})
EMPTY_SHA256 = hashlib.sha256(b"").hexdigest()

class _DuplicateJsonKey(ValueError):
    pass


def _unique_object(pairs: list[tuple[str, object]]) -> dict:
    value = {}
    for key, item in pairs:
        if key in value:
            raise _DuplicateJsonKey(key)
        value[key] = item
    return value


def _exact(value: object, fields: set[str], label: str) -> dict:
    if not isinstance(value, dict) or set(value) != fields:
        raise ValueError(f"{label}字段不完整或包含未知内容")
    return value


def _sha256(value: object, label: str) -> str:
    if not isinstance(value, str) or SHA256_PATTERN.fullmatch(value) is None:
        raise ValueError(f"{label}不是有效 SHA-256")
    return value


def _snapshot(value: object, label: str) -> dict:
    receipt = _exact(value, {"head", "patch_sha256", "files"}, label)
    if not isinstance(receipt["head"], str) or COMMIT_PATTERN.fullmatch(receipt["head"]) is None:
        raise ValueError(f"{label} HEAD 不是有效提交 SHA")
    _sha256(receipt["patch_sha256"], f"{label}补丁摘要")
    if not isinstance(receipt["files"], list):
        raise ValueError(f"{label}未跟踪文件清单无效")
    previous = ""
    for entry in receipt["files"]:
        item = _exact(entry, {"path", "sha256"}, f"{label}未跟踪文件")
        if not isinstance(item["path"], str):
            raise ValueError(f"{label}未跟踪文件路径无效")
        source_domain(item["path"])
        if item["path"] <= previous:
            raise ValueError(f"{label}未跟踪文件必须无重复并按路径排序")
        previous = item["path"]
        _sha256(item["sha256"], f"{label}未跟踪文件摘要")
    return receipt


def _canonical_root(value: object, expected_parent: Path, label: str) -> Path:
    if not isinstance(value, str):
        raise ValueError(f"{label}路径无效")
    path = Path(value)
    try:
        resolved = path.resolve(strict=True)
    except (OSError, RuntimeError):
        raise ValueError(f"{label}路径不存在或无法解析") from None
    if (
        not path.is_absolute()
        or path != resolved
        or resolved.parent != expected_parent
        or not resolved.is_dir()
    ):
        raise ValueError(f"{label}必须是 fixture 目录内的规范绝对目录")
    actual_git_root = Path(git(resolved, "rev-parse", "--show-toplevel").decode().strip()).resolve()
    if actual_git_root != resolved:
        raise ValueError(f"{label}必须是实际 Git 工作树根目录")
    return resolved


def _binding(path: Path) -> dict:
    if not path.is_absolute():
        raise ValueError("证据文件必须使用规范绝对路径")
    try:
        resolved = path.resolve(strict=True)
    except (OSError, RuntimeError):
        raise ValueError("证据文件不存在或无法解析") from None
    if path != resolved:
        raise ValueError("证据文件必须使用规范绝对路径")
    return {"path": str(resolved), **file_digest(resolved)}


def _parse_json(raw: bytes, label: str, limit: int) -> dict:
    if type(raw) is not bytes:
        raise ValueError(f"{label}必须保留原始字节")
    if not raw or len(raw) > limit:
        raise ValueError(f"{label}大小无效")
    try:
        value = json.loads(raw, object_pairs_hook=_unique_object)
    except _DuplicateJsonKey as error:
        raise ValueError(f"{label}包含重复字段：{error}") from None
    except (UnicodeDecodeError, json.JSONDecodeError):
        raise ValueError(f"{label}不是有效 JSON") from None
    if not isinstance(value, dict):
        raise ValueError(f"{label}必须是 JSON 对象")
    return value


def _read_json(path: Path, label: str, limit: int = 4 * 1024 * 1024) -> tuple[dict, bytes]:
    if path.is_symlink() or not path.is_file():
        raise ValueError(f"{label}必须是普通文件")
    raw = path.read_bytes()
    return _parse_json(raw, label, limit), raw


def _clean_archive_snapshot(value: object, label: str, expected_head: str) -> dict:
    receipt = _snapshot(value, label)
    if (
        receipt["head"] != expected_head
        or receipt["patch_sha256"] != EMPTY_SHA256
        or receipt["files"]
    ):
        raise ValueError(f"{label}必须是目标提交的干净源码")
    return receipt


def reference_fixture_source_pair(backend: Path) -> dict:
    """为已完成的 业务 crate fixture 签发本地来源组合，不伪装为 CI 运行。"""
    root = backend.resolve(strict=True).parent
    receipt_path = root / FIXTURE_RECEIPT
    receipt, _ = _read_json(receipt_path, "业务 crate fixture 收据")
    _exact(
        receipt,
        {"format_version", "fixture", "status", "fixture_sha256", "sources", "paths", "generated"},
        "业务 crate fixture 收据",
    )
    if receipt["format_version"] != 1 or receipt["fixture"] != "business" or receipt["status"] != "ready":
        raise ValueError("业务 crate fixture 尚未完成或格式不受支持")
    paths = _exact(receipt["paths"], {"backend", "frontend"}, "业务 crate 工作树路径")
    if _canonical_root(paths["backend"], root, "业务 crate 后端工作树") != backend.resolve():
        raise ValueError("业务 crate fixture 与当前后端工作树不匹配")
    sources = _exact(receipt["sources"], {"backend", "frontend"}, "业务 crate 原始来源")
    for name in ("backend", "frontend"):
        _clean_archive_snapshot(sources[name], f"业务 crate {name} 原始来源", sources[name].get("head", ""))
    return {
        "format_version": 1,
        "kind": REFERENCE_FIXTURE_SOURCE_PAIR,
        "fixture_receipt": _binding(receipt_path),
        "sources": sources,
    }


def full_stack_source_evidence(backend: Path, directory: Path | None = None) -> dict | None:
    """复用全栈生产者的原生来源事实；非全栈入口仍可不提供来源组合。"""
    if directory is None or not (directory / SOURCE_PAIR_RECEIPT).is_file():
        return None
    response, _ = native._read_response(*native._invoke(
        "__source-evidence", {"operation": "full-stack-source-evidence", "write": False,
            "arguments": {"backend": str(backend.resolve(strict=True)),
                          "directory": str(directory.resolve(strict=True)),
                          "ci": {"run_id": os.environ.get("GITHUB_RUN_ID"),
                                 "attempt": os.environ.get("GITHUB_RUN_ATTEMPT")}}},
        subprocess.run, may_write=False,
    ))
    return _exact(response, {"value"}, "全栈原生来源响应")["value"]


def build_evidence(
    backend: Path,
    directory: Path,
    binaries: dict[str, str],
    expected_source: dict | None,
) -> dict:
    if set(binaries) != BINARY_ROLES:
        raise ValueError("全栈构建必须恰好包含 API、Worker、reset 和 migrate")
    source = full_stack_source_evidence(backend, directory)
    if source != expected_source:
        raise ValueError("构建期间全栈源码或 fixture 来源发生变化")
    artifacts = {name: _binding(Path(path)) for name, path in sorted(binaries.items())}
    if full_stack_source_evidence(backend, directory) != source:
        raise ValueError("采集构建产物期间全栈源码或 fixture 来源发生变化")
    return {
        "format_version": 1,
        "kind": "full-stack-build",
        "backend_root": str(backend.resolve()),
        "source": source,
        "artifacts": artifacts,
    }


def verify_build_evidence(backend: Path, directory: Path) -> dict:
    receipt, _ = _read_json(directory / BUILD_EVIDENCE, "全栈构建证据")
    _exact(receipt, {"format_version", "kind", "backend_root", "source", "artifacts"}, "全栈构建证据")
    if (
        receipt["format_version"] != 1
        or receipt["kind"] != "full-stack-build"
        or receipt["backend_root"] != str(backend.resolve())
        or not isinstance(receipt["artifacts"], dict)
        or not receipt["artifacts"]
    ):
        raise ValueError("全栈构建证据格式或后端路径无效")
    binaries = {}
    for name, value in receipt["artifacts"].items():
        binding = _exact(value, {"path", "bytes", "sha256"}, f"全栈构建产物 {name}")
        if not isinstance(name, str) or not name or type(binding["bytes"]) is not int or binding["bytes"] < 0:
            raise ValueError("全栈构建产物名称或大小无效")
        _sha256(binding["sha256"], f"全栈构建产物 {name}")
        if not isinstance(binding["path"], str):
            raise ValueError("全栈构建产物路径无效")
        binaries[name] = binding["path"]
    manifest, _ = _read_json(directory / "binaries.json", "全栈二进制清单")
    if set(manifest) != set(binaries) or any(
        not isinstance(value, str) or value != binaries[name] for name, value in manifest.items()
    ):
        raise ValueError("全栈构建证据与二进制清单不匹配")
    expected = build_evidence(backend, directory, binaries, receipt["source"])
    if receipt != expected:
        raise ValueError("全栈构建证据与当前源码或产物不匹配")
    return receipt


def _runtime_evidence(backend: Path, directory: Path, runtime: Path) -> dict | None:
    source = full_stack_source_evidence(backend, directory)
    if source is None:
        return None
    build = verify_build_evidence(backend, directory)
    if build["source"] != source:
        raise ValueError("运行时来源与构建来源不匹配")
    return {
        "format_version": 1,
        "kind": "full-stack-runtime",
        "source": source,
        "build_evidence": _binding(directory / BUILD_EVIDENCE),
        "runtime": _binding(runtime),
    }


def register_runtime_evidence(backend: Path, directory: Path, runtime: Path) -> dict | None:
    receipt = _runtime_evidence(backend, directory, runtime)
    path = directory / RUNTIME_EVIDENCE
    if receipt is None:
        if path.exists():
            raise ValueError("非全栈复用入口存在意外的运行来源证据")
        return None
    if path.exists():
        existing, _ = _read_json(path, "全栈运行来源证据")
        if existing != receipt:
            raise ValueError("已登记运行来源证据与当前源码、构建或 runtime 不匹配")
    else:
        write_receipt(path, receipt)
    return receipt


def verify_runtime_evidence(backend: Path, directory: Path, runtime: Path) -> dict | None:
    expected = _runtime_evidence(backend, directory, runtime)
    path = directory / RUNTIME_EVIDENCE
    if expected is None:
        if path.exists():
            raise ValueError("非全栈复用入口存在意外的运行来源证据")
        return None
    receipt, _ = _read_json(path, "全栈运行来源证据")
    if receipt != expected:
        raise ValueError("全栈运行来源证据与当前来源不匹配")
    return receipt
