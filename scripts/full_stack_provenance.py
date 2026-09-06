"""绑定全栈源码、构建产物与运行收据，拒绝跨来源复用。"""

from __future__ import annotations

import hashlib
import json
import os
from collections.abc import Iterable, Mapping
from pathlib import Path, PurePosixPath
import re

from artifact_digests import file_digest
from full_stack_process import write_receipt
from source_inventory import git, snapshot, source_domain


SHA256_PATTERN = re.compile(r"^[a-f0-9]{64}$")
COMMIT_PATTERN = re.compile(r"^[a-f0-9]{40}$")
BUILD_EVIDENCE = "build-evidence.json"
RUNTIME_EVIDENCE = "runtime-evidence.json"
RUNTIME_RECEIPT = "runtime.json"
SOURCE_PAIR_RECEIPT = "source-pair.json"
FIXTURE_RECEIPT = "fixture.json"
BINARY_ROLES = frozenset({"ryframe", "ryframe-worker", "ryframe-reset", "ryframe-migrate"})
DEVICE_FIXTURE = Path("crates/ryframe-generator/tests/fixtures/device.toml")
DEVICE_RESOURCE = Path("catalog/resources/device.toml")
EMPTY_SHA256 = hashlib.sha256(b"").hexdigest()
ARCHIVE_RECEIPT_LIMITS = {
    SOURCE_PAIR_RECEIPT: 16 * 1024,
    BUILD_EVIDENCE: 4 * 1024 * 1024,
    RUNTIME_RECEIPT: 64 * 1024,
    RUNTIME_EVIDENCE: 4 * 1024 * 1024,
    FIXTURE_RECEIPT: 4 * 1024 * 1024,
}


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


def _archive_path(value: object, label: str) -> PurePosixPath:
    if (
        not isinstance(value, str)
        or "\\" in value
        or "\x00" in value
        or value.startswith("//")
    ):
        raise ValueError(f"{label}路径无效")
    path = PurePosixPath(value)
    if not path.is_absolute() or path.as_posix() != value or ".." in path.parts:
        raise ValueError(f"{label}必须是规范绝对 POSIX 路径")
    return path


def _archive_binding(
    value: object,
    label: str,
    *,
    filename: str | None = None,
    raw: bytes | None = None,
) -> tuple[dict, PurePosixPath]:
    binding = _exact(value, {"path", "bytes", "sha256"}, label)
    path = _archive_path(binding["path"], label)
    if filename is not None and path.name != filename:
        raise ValueError(f"{label}文件名无效")
    if type(binding["bytes"]) is not int or binding["bytes"] <= 0:
        raise ValueError(f"{label}字节数无效")
    digest = _sha256(binding["sha256"], f"{label}摘要")
    if raw is not None and (
        binding["bytes"] != len(raw) or digest != hashlib.sha256(raw).hexdigest()
    ):
        raise ValueError(f"{label}与归档中的原始字节不匹配")
    return binding, path


def _archive_members(
    entries: Mapping[str, bytes] | Iterable[tuple[str, bytes]], fixture: str
) -> dict[str, bytes]:
    expected = {SOURCE_PAIR_RECEIPT, BUILD_EVIDENCE, RUNTIME_RECEIPT, RUNTIME_EVIDENCE}
    if fixture == "device":
        expected.add(FIXTURE_RECEIPT)
    elif fixture != "core":
        raise ValueError("全栈归档 fixture 必须是 core 或 device")
    selected: dict[str, bytes] = {}
    items = entries.items() if isinstance(entries, Mapping) else entries
    for entry in items:
        try:
            name, raw = entry
        except (TypeError, ValueError):
            raise ValueError("全栈归档成员格式无效") from None
        if not isinstance(name, str) or "\\" in name or "\x00" in name:
            raise ValueError("全栈归档成员路径无效")
        path = PurePosixPath(name)
        if path.is_absolute() or path.as_posix() != name or ".." in path.parts:
            raise ValueError("全栈归档成员路径无效")
        if path.name not in expected:
            continue
        if path.name in selected:
            raise ValueError(f"全栈归档包含重复 {path.name}")
        if type(raw) is not bytes:
            raise ValueError(f"全栈归档 {path.name} 未保留原始字节")
        selected[path.name] = raw
    if set(selected) != expected:
        raise ValueError("全栈归档缺少必需的唯一来源、构建或运行收据")
    return selected


def _clean_archive_snapshot(value: object, label: str, expected_head: str) -> dict:
    receipt = _snapshot(value, label)
    if (
        receipt["head"] != expected_head
        or receipt["patch_sha256"] != EMPTY_SHA256
        or receipt["files"]
    ):
        raise ValueError(f"{label}必须是目标提交的干净源码")
    return receipt


def _generated_archive_snapshot(value: object, label: str, expected_head: str) -> dict:
    receipt = _snapshot(value, label)
    if receipt["head"] != expected_head or (
        receipt["patch_sha256"] == EMPTY_SHA256 and not receipt["files"]
    ):
        raise ValueError(f"{label}没有绑定目标提交上的实际生成变化")
    return receipt


def _archive_pair(
    source: dict,
    pair: dict,
    raw: bytes,
    *,
    backend_sha: str,
    frontend_sha: str,
    run_id: int,
    attempt: int,
) -> PurePosixPath:
    expected = {
        "backend_sha": backend_sha,
        "frontend_sha": frontend_sha,
        "run_id": run_id,
        "attempt": attempt,
    }
    _exact(pair, {"format_version", *expected, "sources"}, "归档源码组合收据")
    sources = _exact(pair["sources"], {"backend", "frontend"}, "归档源码组合来源")
    for name, expected_head in (("backend", backend_sha), ("frontend", frontend_sha)):
        _clean_archive_snapshot(sources[name], f"归档 {name} 原始来源", expected_head)
    if pair["format_version"] != 1 or any(pair[name] != value for name, value in expected.items()):
        raise ValueError("归档源码组合与目标提交或 CI attempt 不匹配")
    embedded = _exact(
        source.get("source_pair"), {*pair, "receipt"}, "构建来源中的源码组合"
    )
    if {name: embedded[name] for name in pair} != pair:
        raise ValueError("构建来源与归档源码组合收据不匹配")
    expected_original = sources if source["fixture"] == "device" else {"backend": sources["backend"]}
    if source["original"] != expected_original:
        raise ValueError("构建来源与源码组合中的干净来源不匹配")
    _, path = _archive_binding(
        embedded["receipt"],
        "源码组合收据绑定",
        filename=SOURCE_PAIR_RECEIPT,
        raw=raw,
    )
    return path.parent


def _archive_core_source(source: object, backend_root: PurePosixPath, backend_sha: str) -> dict:
    receipt = _exact(
        source,
        {"format_version", "fixture", "backend_root", "source_pair", "original"},
        "core 构建来源",
    )
    if receipt["format_version"] != 1 or receipt["fixture"] != "core":
        raise ValueError("core 构建来源格式无效")
    if _archive_path(receipt["backend_root"], "core 后端根目录") != backend_root:
        raise ValueError("core 构建来源与构建后端根目录不匹配")
    original = _exact(receipt["original"], {"backend"}, "core 原始来源")
    _clean_archive_snapshot(original["backend"], "core 后端原始来源", backend_sha)
    return receipt


def _archive_device_source(
    source: object,
    fixture_receipt: dict,
    fixture_raw: bytes,
    backend_root: PurePosixPath,
    *,
    backend_sha: str,
    frontend_sha: str,
    fixture_sha256: str | None,
) -> dict:
    receipt = _exact(
        source,
        {
            "format_version",
            "fixture",
            "fixture_receipt",
            "fixture_definition",
            "roots",
            "source_pair",
            "original",
            "generated",
        },
        "Device 构建来源",
    )
    fixture_hash = _sha256(fixture_sha256, "发布源码 Device fixture 摘要")
    if fixture_hash == EMPTY_SHA256:
        raise ValueError("发布源码 Device fixture 不能为空")
    if receipt["format_version"] != 1 or receipt["fixture"] != "device":
        raise ValueError("Device 构建来源格式无效")
    _, fixture_path = _archive_binding(
        receipt["fixture_receipt"],
        "Device fixture 收据绑定",
        filename=FIXTURE_RECEIPT,
        raw=fixture_raw,
    )
    fixture_root = fixture_path.parent
    roots = _exact(receipt["roots"], {"backend", "frontend"}, "Device 工作树根目录")
    root_paths = {
        name: _archive_path(roots[name], f"Device {name} 工作树")
        for name in ("backend", "frontend")
    }
    if (
        root_paths["backend"] != backend_root
        or root_paths["backend"] != fixture_root / "backend"
        or root_paths["frontend"] != fixture_root / "frontend"
    ):
        raise ValueError("Device 工作树与 fixture 收据路径不匹配")

    original = _exact(receipt["original"], {"backend", "frontend"}, "Device 原始来源")
    generated = _exact(receipt["generated"], {"backend", "frontend"}, "Device 生成来源")
    for name, expected_head in (("backend", backend_sha), ("frontend", frontend_sha)):
        _clean_archive_snapshot(original[name], f"Device 原始 {name} 来源", expected_head)
        _generated_archive_snapshot(generated[name], f"Device 生成 {name} 来源", expected_head)

    fixture = _exact(
        fixture_receipt,
        {"format_version", "fixture", "status", "fixture_sha256", "sources", "paths", "generated"},
        "归档 Device fixture 收据",
    )
    if (
        fixture["format_version"] != 1
        or fixture["fixture"] != "device"
        or fixture["status"] != "ready"
        or _sha256(fixture["fixture_sha256"], "归档 Device fixture 摘要") != fixture_hash
        or fixture["sources"] != original
        or fixture["paths"] != roots
        or fixture["generated"] != generated
    ):
        raise ValueError("归档 Device fixture 与构建来源或发布源码不匹配")

    definitions = _exact(
        receipt["fixture_definition"], {"fixture", "resource"}, "Device fixture 定义绑定"
    )
    expected_paths = {
        "fixture": backend_root / DEVICE_FIXTURE.as_posix(),
        "resource": backend_root / DEVICE_RESOURCE.as_posix(),
    }
    sizes = set()
    for name, expected_path in expected_paths.items():
        binding, path = _archive_binding(definitions[name], f"Device {name} 定义")
        if path != expected_path or binding["sha256"] != fixture_hash:
            raise ValueError("Device fixture 定义路径或摘要不匹配")
        sizes.add(binding["bytes"])
    if len(sizes) != 1:
        raise ValueError("Device fixture 与生成资源字节数不匹配")
    files = {entry["path"]: entry["sha256"] for entry in generated["backend"]["files"]}
    if files.get(DEVICE_RESOURCE.as_posix()) != fixture_hash:
        raise ValueError("Device 生成快照未绑定当前 fixture 资源")
    return receipt


def validate_archive_evidence(
    entries: Mapping[str, bytes] | Iterable[tuple[str, bytes]],
    *,
    fixture: str,
    backend_sha: str,
    frontend_sha: str,
    run_id: int,
    attempt: int,
    fixture_sha256: str | None = None,
) -> dict:
    """纯校验全栈归档中的源码、构建和运行收据，不读取归档外部状态。"""
    if (
        not isinstance(backend_sha, str)
        or not isinstance(frontend_sha, str)
        or COMMIT_PATTERN.fullmatch(backend_sha) is None
        or COMMIT_PATTERN.fullmatch(frontend_sha) is None
    ):
        raise ValueError("全栈归档目标提交 SHA 无效")
    if type(run_id) is not int or run_id <= 0 or type(attempt) is not int or attempt <= 0:
        raise ValueError("全栈归档 CI 运行身份无效")
    members = _archive_members(entries, fixture)
    receipts = {
        name: _parse_json(raw, f"归档 {name}", ARCHIVE_RECEIPT_LIMITS[name])
        for name, raw in members.items()
    }

    build = _exact(
        receipts[BUILD_EVIDENCE],
        {"format_version", "kind", "backend_root", "source", "artifacts"},
        "归档全栈构建证据",
    )
    if build["format_version"] != 1 or build["kind"] != "full-stack-build":
        raise ValueError("归档全栈构建证据格式无效")
    backend_root = _archive_path(build["backend_root"], "归档构建后端根目录")
    artifacts = _exact(build["artifacts"], set(BINARY_ROLES), "归档全栈构建产物")
    artifact_paths = set()
    for role in sorted(BINARY_ROLES):
        _, path = _archive_binding(artifacts[role], f"归档构建产物 {role}")
        if path.name != role or path in artifact_paths:
            raise ValueError("归档构建产物角色、路径或唯一性无效")
        artifact_paths.add(path)

    source = (
        _archive_core_source(build["source"], backend_root, backend_sha)
        if fixture == "core"
        else _archive_device_source(
            build["source"],
            receipts[FIXTURE_RECEIPT],
            members[FIXTURE_RECEIPT],
            backend_root,
            backend_sha=backend_sha,
            frontend_sha=frontend_sha,
            fixture_sha256=fixture_sha256,
        )
    )
    receipt_root = _archive_pair(
        source,
        receipts[SOURCE_PAIR_RECEIPT],
        members[SOURCE_PAIR_RECEIPT],
        backend_sha=backend_sha,
        frontend_sha=frontend_sha,
        run_id=run_id,
        attempt=attempt,
    )

    runtime = _exact(
        receipts[RUNTIME_RECEIPT],
        {
            "format_version",
            "backend_root",
            "scope_id",
            "configuration_sha256",
            "worker_ready_url",
            "artifacts",
        },
        "归档运行收据",
    )
    if runtime["format_version"] != 1 or runtime["backend_root"] != str(backend_root):
        raise ValueError("归档运行收据格式或后端根目录无效")
    if runtime["scope_id"] != f"ci-{run_id}-{attempt}":
        raise ValueError("归档运行 scope 无效")
    _sha256(runtime["configuration_sha256"], "归档运行配置摘要")
    if not isinstance(runtime["worker_ready_url"], str) or re.fullmatch(
        r"http://127\.0\.0\.1:([1-9][0-9]{0,4})/readyz", runtime["worker_ready_url"]
    ) is None or not 1 <= int(
        runtime["worker_ready_url"].split(":")[-1].split("/")[0]
    ) <= 65535:
        raise ValueError("归档 Worker 就绪地址无效")
    runtime_artifacts = _exact(runtime["artifacts"], {"api", "worker"}, "归档运行产物")
    for runtime_role, build_role in (("api", "ryframe"), ("worker", "ryframe-worker")):
        binding = _exact(
            runtime_artifacts[runtime_role],
            {"path", "sha256"},
            f"归档 {runtime_role} 产物",
        )
        _archive_path(binding["path"], f"归档 {runtime_role} 产物")
        _sha256(binding["sha256"], f"归档 {runtime_role} 产物摘要")
        if binding != {
            "path": artifacts[build_role]["path"],
            "sha256": artifacts[build_role]["sha256"],
        }:
            raise ValueError("归档运行产物与构建角色的路径或摘要不匹配")

    runtime_evidence = _exact(
        receipts[RUNTIME_EVIDENCE],
        {"format_version", "kind", "source", "build_evidence", "runtime"},
        "归档全栈运行来源证据",
    )
    if (
        runtime_evidence["format_version"] != 1
        or runtime_evidence["kind"] != "full-stack-runtime"
        or runtime_evidence["source"] != source
    ):
        raise ValueError("归档运行来源证据格式或源码来源不匹配")
    _, build_path = _archive_binding(
        runtime_evidence["build_evidence"],
        "全栈构建证据收据绑定",
        filename=BUILD_EVIDENCE,
        raw=members[BUILD_EVIDENCE],
    )
    _, runtime_path = _archive_binding(
        runtime_evidence["runtime"],
        "全栈运行收据绑定",
        filename=RUNTIME_RECEIPT,
        raw=members[RUNTIME_RECEIPT],
    )
    if build_path.parent != receipt_root or runtime_path.parent != receipt_root:
        raise ValueError("源码、构建和运行收据不属于同一全栈运行目录")
    return {
        "source_pair": receipts[SOURCE_PAIR_RECEIPT],
        "build_evidence": build,
        "runtime": runtime,
        "runtime_evidence": runtime_evidence,
        **({"fixture_receipt": receipts[FIXTURE_RECEIPT]} if fixture == "device" else {}),
    }


def _stable_snapshots(roots: dict[str, Path], expected: dict) -> dict:
    observed = {name: snapshot(root)[0] for name, root in roots.items()}
    if observed != expected:
        raise ValueError("Device 生成工作树源码快照与 fixture 收据不匹配")
    repeated = {name: snapshot(root)[0] for name, root in roots.items()}
    if repeated != observed:
        raise ValueError("核验 Device 生成工作树期间源码发生变化")
    return observed


def source_pair_receipt(backend: Path, frontend: Path, run_id: int, attempt: int) -> dict:
    """稳定采集两个独立 Git 根目录的干净源码快照与 CI 运行身份。"""
    if type(run_id) is not int or run_id <= 0 or type(attempt) is not int or attempt <= 0:
        raise ValueError("全栈源码组合缺少有效运行身份")
    roots = {"backend": backend.resolve(strict=True), "frontend": frontend.resolve(strict=True)}
    if roots["backend"] == roots["frontend"]:
        raise ValueError("全栈源码组合必须来自两个独立仓库")
    for name, root in roots.items():
        actual = Path(git(root, "rev-parse", "--show-toplevel").decode().strip()).resolve()
        if actual != root:
            raise ValueError(f"全栈 {name} 来源必须是实际 Git 工作树根目录")
    sources = {name: snapshot(root)[0] for name, root in roots.items()}
    for name, value in sources.items():
        _clean_archive_snapshot(value, f"全栈 {name} 来源", value.get("head", ""))
    if {name: snapshot(root)[0] for name, root in roots.items()} != sources:
        raise ValueError("采集全栈源码组合期间源码发生变化")
    return {
        "format_version": 1,
        "backend_sha": sources["backend"]["head"],
        "frontend_sha": sources["frontend"]["head"],
        "run_id": run_id,
        "attempt": attempt,
        "sources": sources,
    }


def verify_source_pair_receipt(
    path: Path,
    backend: Path,
    frontend: Path,
    run_id: int,
    attempt: int,
) -> dict:
    """只读复核已登记源码组合与两个当前工作树仍完全一致。"""
    recorded, _ = _read_json(
        path, "全栈源码组合收据", ARCHIVE_RECEIPT_LIMITS[SOURCE_PAIR_RECEIPT]
    )
    current = source_pair_receipt(backend, frontend, run_id, attempt)
    if recorded != current:
        raise ValueError("全栈源码组合收据与当前干净源码或运行身份不匹配")
    return recorded


def _source_pair(directory: Path, backend_sha: str, frontend_sha: str | None = None) -> dict:
    root = directory.resolve(strict=True)
    pair, _ = _read_json(root / "source-pair.json", "全栈源码组合收据")
    _exact(
        pair,
        {"format_version", "backend_sha", "frontend_sha", "run_id", "attempt", "sources"},
        "全栈源码组合收据",
    )
    if pair["format_version"] != 1:
        raise ValueError("全栈源码组合收据格式不受支持")
    for name in ("backend_sha", "frontend_sha"):
        if not isinstance(pair[name], str) or COMMIT_PATTERN.fullmatch(pair[name]) is None:
            raise ValueError("全栈源码组合收据包含无效提交 SHA")
    sources = _exact(pair["sources"], {"backend", "frontend"}, "全栈源码组合来源")
    for name, field in (("backend", "backend_sha"), ("frontend", "frontend_sha")):
        _clean_archive_snapshot(sources[name], f"全栈 {name} 来源", pair[field])
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
    if pair["sources"] != sources:
        raise ValueError("Device 原始来源与全栈源码组合收据不匹配")
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
    if pair["sources"]["backend"] != observed:
        raise ValueError("core 后端来源与全栈源码组合收据不匹配")
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
