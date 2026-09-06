"""绑定全栈源码、构建产物与运行收据，拒绝跨来源复用。"""

from __future__ import annotations

import hashlib
import json
import os
from pathlib import Path
import re

from artifact_digests import file_digest
from full_stack_process import write_receipt
from source_inventory import git, snapshot, source_domain


SHA256_PATTERN = re.compile(r"^[a-f0-9]{64}$")
COMMIT_PATTERN = re.compile(r"^[a-f0-9]{40}$")
BUILD_EVIDENCE = "build-evidence.json"
RUNTIME_EVIDENCE = "runtime-evidence.json"
BINARY_ROLES = frozenset({"ryframe", "ryframe-worker", "ryframe-reset", "ryframe-migrate"})
DEVICE_FIXTURE = Path("crates/ryframe-generator/tests/fixtures/device.toml")
DEVICE_RESOURCE = Path("catalog/resources/device.toml")


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


def _read_json(path: Path, label: str, limit: int = 4 * 1024 * 1024) -> tuple[dict, bytes]:
    if path.is_symlink() or not path.is_file():
        raise ValueError(f"{label}必须是普通文件")
    raw = path.read_bytes()
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
    return value, raw


def _stable_snapshots(roots: dict[str, Path], expected: dict) -> dict:
    observed = {name: snapshot(root)[0] for name, root in roots.items()}
    if observed != expected:
        raise ValueError("Device 生成工作树源码快照与 fixture 收据不匹配")
    repeated = {name: snapshot(root)[0] for name, root in roots.items()}
    if repeated != observed:
        raise ValueError("核验 Device 生成工作树期间源码发生变化")
    return observed


def _source_pair(directory: Path, backend_sha: str, frontend_sha: str | None = None) -> dict:
    root = directory.resolve(strict=True)
    pair, _ = _read_json(root / "source-pair.json", "全栈源码组合收据")
    _exact(pair, {"backend_sha", "frontend_sha", "run_id", "attempt"}, "全栈源码组合收据")
    for name in ("backend_sha", "frontend_sha"):
        if not isinstance(pair[name], str) or COMMIT_PATTERN.fullmatch(pair[name]) is None:
            raise ValueError("全栈源码组合收据包含无效提交 SHA")
    if (
        type(pair["run_id"]) is not int
        or pair["run_id"] <= 0
        or type(pair["attempt"]) is not int
        or pair["attempt"] <= 0
    ):
        raise ValueError("全栈源码组合收据包含无效运行身份")
    if pair["backend_sha"] != backend_sha or (
        frontend_sha is not None and pair["frontend_sha"] != frontend_sha
    ):
        raise ValueError("全栈源码组合与实际验收工作树提交不匹配")
    for environment, field in (("GITHUB_RUN_ID", "run_id"), ("GITHUB_RUN_ATTEMPT", "attempt")):
        value = os.environ.get(environment)
        if value is not None and (not value.isdecimal() or int(value) != pair[field]):
            raise ValueError("全栈源码组合收据不属于当前 CI attempt")
    return {**pair, "receipt": _binding(root / "source-pair.json")}


def _device_source_evidence(backend: Path, directory: Path) -> dict:
    fixture_root = backend.parent.resolve(strict=True)
    receipt_path = fixture_root / "fixture.json"
    receipt, raw = _read_json(receipt_path, "Device fixture 收据")
    _exact(
        receipt,
        {"format_version", "fixture", "status", "fixture_sha256", "sources", "paths", "generated"},
        "Device fixture 收据",
    )
    if receipt["format_version"] != 1 or receipt["fixture"] != "device" or receipt["status"] != "ready":
        raise ValueError("Device fixture 尚未完成或格式不受支持")
    sources = _exact(receipt["sources"], {"backend", "frontend"}, "Device 原始来源")
    generated = _exact(receipt["generated"], {"backend", "frontend"}, "Device 生成来源")
    paths = _exact(receipt["paths"], {"backend", "frontend"}, "Device 工作树路径")
    roots = {
        name: _canonical_root(paths[name], fixture_root, f"Device {name} 工作树")
        for name in ("backend", "frontend")
    }
    if roots["backend"] != backend or roots["backend"] == roots["frontend"]:
        raise ValueError("Device fixture 与当前后端工作树不匹配")
    for name in ("backend", "frontend"):
        _snapshot(sources[name], f"Device 原始 {name} 来源")
        _snapshot(generated[name], f"Device 生成 {name} 来源")
        if sources[name]["head"] != generated[name]["head"]:
            raise ValueError(f"Device {name} 原始 SHA 与生成工作树 HEAD 不匹配")
    expected_backend = os.environ.get("RYFRAME_CODE_SHA", "")
    if COMMIT_PATTERN.fullmatch(expected_backend) is None or sources["backend"]["head"] != expected_backend:
        raise ValueError("Device 后端原始 SHA 与本次验收提交不匹配")
    pair = _source_pair(directory, sources["backend"]["head"], sources["frontend"]["head"])
    fixture_sha = _sha256(receipt["fixture_sha256"], "Device fixture 内容摘要")
    definitions = {
        name: _binding(roots["backend"] / path)
        for name, path in (("fixture", DEVICE_FIXTURE), ("resource", DEVICE_RESOURCE))
    }
    if any(value["sha256"] != fixture_sha for value in definitions.values()):
        raise ValueError("Device fixture 定义、生成资源与登记摘要不匹配")
    actual = _stable_snapshots(roots, generated)
    return {
        "format_version": 1,
        "fixture": "device",
        "fixture_receipt": {
            "path": str(receipt_path),
            "bytes": len(raw),
            "sha256": hashlib.sha256(raw).hexdigest(),
        },
        "fixture_definition": definitions,
        "roots": {name: str(root) for name, root in roots.items()},
        "source_pair": pair,
        "original": sources,
        "generated": actual,
    }


def full_stack_source_evidence(backend: Path, directory: Path | None = None) -> dict | None:
    """核对当前全栈源码；非全栈复用入口不隐式要求 fixture 环境。"""
    fixture = os.environ.get("RYFRAME_E2E_FIXTURE")
    if fixture is None:
        return None
    if fixture not in {"core", "device"}:
        raise ValueError("RYFRAME_E2E_FIXTURE 必须是 core 或 device")
    if directory is None:
        raise ValueError("全栈来源核验缺少明确运行目录")
    root = backend.resolve(strict=True)
    if fixture == "device":
        return _device_source_evidence(root, directory)
    expected = os.environ.get("RYFRAME_CODE_SHA", "")
    actual_git_root = Path(git(root, "rev-parse", "--show-toplevel").decode().strip()).resolve()
    if actual_git_root != root:
        raise ValueError("core 后端必须是实际 Git 工作树根目录")
    observed = snapshot(root)[0]
    if COMMIT_PATTERN.fullmatch(expected) is None or observed["head"] != expected:
        raise ValueError("core 后端源码与本次验收提交不匹配")
    if snapshot(root)[0] != observed:
        raise ValueError("核验 core 后端源码期间发生变化")
    pair = _source_pair(directory, observed["head"])
    return {
        "format_version": 1,
        "fixture": "core",
        "backend_root": str(root),
        "source_pair": pair,
        "original": {"backend": observed},
    }


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
