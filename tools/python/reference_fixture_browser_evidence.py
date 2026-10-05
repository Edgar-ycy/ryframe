"""严格核验业务 crate 浏览器产物、场景收据与登录预算。"""

from __future__ import annotations

import hashlib
import os
from pathlib import Path, PurePosixPath
import re
import stat
import time
from dataclasses import dataclass

from restore_runtime_evidence import (
    ArtifactSnapshot,
    artifact_snapshot,
    directory,
    exact_fields,
    file_state,
    read_json_document,
    reject_link_or_reparse,
)


MAX_ARTIFACT_FILES = 10_000
MAX_ARTIFACT_BYTES = 8 * 1024 * 1024 * 1024
MAX_LOG_BYTES = 64 * 1024 * 1024
MAX_LOGIN_BUCKETS = 1_024
BUSINESS_SCENARIOS = (
    "shared-migration",
    "dedicated-migration",
    "retention",
    "cancellation",
    "crash-recovery",
)
BUSINESS_TESTS = {
    "真实业务数据从 shared-control 复制校验并切换到 shared": ("shared-migration",),
    "真实业务数据从 dedicated-a 复制校验并切换到 dedicated-b": (
        "dedicated-migration", "retention",
    ),
    "真实排队业务迁移取消恢复源数据，并允许再次迁移": ("cancellation",),
    "真实业务复制阻塞时 Worker 崩溃，重启后同一迁移恢复并完成校验": ("crash-recovery",),
}
RUN_ID = re.compile(r"[a-z0-9][a-z0-9-]{0,63}")
LOGIN_KEY = re.compile(r"(?:principal|ip):[a-f0-9]{64}")
GENERATION = re.compile(r"[a-f0-9-]{36}")


def _relative(path: object, label: str) -> PurePosixPath:
    relative = PurePosixPath(path) if isinstance(path, str) else None
    if (
        relative is None
        or relative.is_absolute()
        or not relative.parts
        or ".." in relative.parts
        or "\\" in path
        or relative.as_posix() != path
    ):
        raise ValueError(f"{label}必须是规范化相对路径")
    return relative


def _scan(root: Path, maximum_files: int, maximum_bytes: int) -> tuple[list[dict], tuple[ArtifactSnapshot, ...]]:
    snapshots = []
    total = 0
    def failed(error: OSError) -> None:
        raise error

    for current, names, filenames in os.walk(root, followlinks=False, onerror=failed):
        base = Path(current)
        names.sort()
        filenames.sort()
        for name in names:
            metadata = (base / name).lstat()
            if stat.S_ISLNK(metadata.st_mode) or not stat.S_ISDIR(metadata.st_mode) \
                    or getattr(metadata, "st_file_attributes", 0) & 0x400:
                raise ValueError("业务 crate 浏览器产物包含链接、重解析点或非普通目录")
        for name in filenames:
            path = base / name
            metadata = path.lstat()
            if len(snapshots) >= maximum_files or total + metadata.st_size > maximum_bytes:
                raise ValueError("业务 crate 浏览器产物超过已登记文件数或字节上限")
            snapshot = artifact_snapshot(path)
            snapshots.append(snapshot)
            total += snapshot.bytes
    snapshots.sort(key=lambda item: item.path.relative_to(root).as_posix())
    files = [
        {"path": item.path.relative_to(root).as_posix(), "bytes": item.bytes,
         "sha256": item.sha256}
        for item in snapshots
    ]
    return files, tuple(snapshots)


@dataclass(frozen=True)
class ArtifactManifestSnapshot:
    manifest: dict
    snapshots: tuple[ArtifactSnapshot, ...]
    root: Path
    parent: Path
    label: str

    def assert_unchanged(self) -> None:
        for snapshot in self.snapshots:
            snapshot.assert_unchanged()
        if verify_artifact_manifest(self.manifest, self.root, self.parent, self.label) != self.manifest:
            raise ValueError(f"{self.label}与运行前完整清单不一致")


def artifact_manifest_snapshot(root: Path, parent: Path, label: str, *,
                               maximum_files: int = MAX_ARTIFACT_FILES,
                               maximum_bytes: int = MAX_ARTIFACT_BYTES) -> ArtifactManifestSnapshot:
    """固定完整有界清单及文件状态，供运行期间检测 A→B→A。"""
    if type(maximum_files) is not int or type(maximum_bytes) is not int \
            or maximum_files < 1 or maximum_bytes < 1:
        raise ValueError("业务 crate 浏览器产物上限无效")
    parent = directory(parent, label + "父目录")
    root = directory(root, label)
    if root == parent or not root.is_relative_to(parent):
        raise ValueError(f"{label}不在已登记产物目录内")
    files, snapshots = _scan(root, maximum_files, maximum_bytes)
    if not files:
        raise ValueError(f"{label}不能为空")
    for snapshot in snapshots:
        snapshot.assert_unchanged()
    repeated, _ = _scan(root, maximum_files, maximum_bytes)
    if repeated != files:
        raise ValueError(f"{label}在生成完整清单期间发生变化")
    manifest = {
        "format_version": 1,
        "kind": "bounded-artifact-manifest",
        "root": str(root),
        "limits": {"files": maximum_files, "bytes": maximum_bytes},
        "total_files": len(files),
        "total_bytes": sum(item["bytes"] for item in files),
        "files": files,
    }
    return ArtifactManifestSnapshot(manifest, snapshots, root, parent, label)


def artifact_manifest(root: Path, parent: Path, label: str, *,
                      maximum_files: int = MAX_ARTIFACT_FILES,
                      maximum_bytes: int = MAX_ARTIFACT_BYTES) -> dict:
    return artifact_manifest_snapshot(
        root, parent, label, maximum_files=maximum_files, maximum_bytes=maximum_bytes
    ).manifest


def verify_artifact_manifest(value: object, root: Path, parent: Path, label: str) -> dict:
    manifest = exact_fields(
        value,
        {"format_version", "kind", "root", "limits", "total_files", "total_bytes", "files"},
        label,
    )
    limits = exact_fields(manifest["limits"], {"files", "bytes"}, label + "上限")
    if (
        type(manifest["format_version"]) is not int
        or manifest["format_version"] != 1
        or manifest["kind"] != "bounded-artifact-manifest"
        or manifest["root"] != str(root.absolute())
        or type(limits["files"]) is not int
        or type(limits["bytes"]) is not int
        or not 1 <= limits["files"] <= MAX_ARTIFACT_FILES
        or not 1 <= limits["bytes"] <= MAX_ARTIFACT_BYTES
        or type(manifest["total_files"]) is not int
        or type(manifest["total_bytes"]) is not int
        or not isinstance(manifest["files"], list)
    ):
        raise ValueError(f"{label}版本、路径或上限无效")
    previous = ""
    total = 0
    for item in manifest["files"]:
        item = exact_fields(item, {"path", "bytes", "sha256"}, label + "文件")
        _relative(item["path"], label + "文件")
        if (
            item["path"] <= previous
            or type(item["bytes"]) is not int
            or item["bytes"] < 0
            or not isinstance(item["sha256"], str)
            or re.fullmatch(r"[a-f0-9]{64}", item["sha256"]) is None
        ):
            raise ValueError(f"{label}文件顺序、大小或摘要无效")
        previous = item["path"]
        total += item["bytes"]
    if (
        not manifest["files"]
        or manifest["total_files"] != len(manifest["files"])
        or manifest["total_bytes"] != total
        or len(manifest["files"]) > limits["files"]
        or total > limits["bytes"]
    ):
        raise ValueError(f"{label}汇总与完整文件清单不一致")
    current = artifact_manifest(
        root, parent, label, maximum_files=limits["files"], maximum_bytes=limits["bytes"]
    )
    if current != manifest:
        raise ValueError(f"{label}与已发布完整清单不一致")
    return manifest


def business_tests(path: Path, server: str, run_id: str) -> dict:
    document = read_json_document(path)
    receipt = exact_fields(
        document.value,
        {"format_version", "kind", "fixture", "server", "run_id", "status", "runs"},
        "业务 crate 浏览器测试收据",
    )
    if (
        type(receipt["format_version"]) is not int
        or receipt["format_version"] != 1
        or receipt["kind"] != "business-browser-tests"
        or receipt["fixture"] != "business"
        or receipt["server"] != server
        or server not in {"dev", "preview"}
        or receipt["run_id"] != run_id
        or RUN_ID.fullmatch(run_id) is None
        or receipt["status"] != "passed"
        or not isinstance(receipt["runs"], list)
        or len(receipt["runs"]) != 4
    ):
        raise ValueError("业务 crate 浏览器测试收据与本次运行不一致")
    titles = set()
    scenarios = []
    for item in receipt["runs"]:
        run = exact_fields(item, {"title", "status", "retry", "scenarios"}, "业务 crate 浏览器测试明细")
        if (
            not isinstance(run["title"], list)
            or len(run["title"]) != 1
            or any(not isinstance(part, str) or not part.strip() for part in run["title"])
            or run["status"] != "passed"
            or type(run["retry"]) is not int
            or run["retry"] != 0
            or not isinstance(run["scenarios"], list)
            or not run["scenarios"]
            or any(not isinstance(item, str) or not item for item in run["scenarios"])
        ):
            raise ValueError("业务 crate 浏览器测试包含失败、跳过、重试或无效标题")
        title = tuple(run["title"])
        if title in titles:
            raise ValueError("业务 crate 浏览器测试标题重复")
        expected_scenarios = BUSINESS_TESTS.get(run["title"][0])
        if expected_scenarios is None or tuple(run["scenarios"]) != expected_scenarios:
            raise ValueError("业务 crate 浏览器测试标题与场景不匹配")
        titles.add(title)
        scenarios.extend(run["scenarios"])
    if {title[0] for title in titles} != set(BUSINESS_TESTS):
        raise ValueError("业务 crate 浏览器测试标题集合不完整")
    if len(scenarios) != len(BUSINESS_SCENARIOS) or set(scenarios) != set(BUSINESS_SCENARIOS):
        raise ValueError("业务 crate 浏览器场景缺失、重复或包含未知值")
    document.assert_unchanged()
    return {"receipt": document.value, "descriptor": {
        "path": str(document.path), "bytes": len(document.raw), "sha256": document.sha256
    }}


def login_budget(path: Path, binding: dict) -> dict:
    document = read_json_document(path)
    ledger = exact_fields(document.value, {"version", "binding", "observedAt", "buckets"}, "登录预算账本")
    expected = {
        "scope": binding["scope_id"],
        "capacity": binding["rate_limits"]["login"]["capacity"],
        "windowMs": binding["rate_limits"]["login"]["window_secs"] * 1_000,
    }
    if (
        ledger["version"] != 1
        or ledger["binding"] != expected
        or type(ledger["observedAt"]) is not int
        or not 0 <= ledger["observedAt"] <= int(time.time() * 1_000)
        or not isinstance(ledger["buckets"], dict)
        or not ledger["buckets"]
        or len(ledger["buckets"]) > MAX_LOGIN_BUCKETS
    ):
        raise ValueError("登录预算账本与 scope、限流或首次浏览器写入不一致")
    for key, value in ledger["buckets"].items():
        bucket = exact_fields(value, {"generation", "count", "reservedAt", "completedAt"}, "登录预算条目")
        completed = bucket["completedAt"]
        if (
            LOGIN_KEY.fullmatch(key) is None
            or not isinstance(bucket["generation"], str)
            or GENERATION.fullmatch(bucket["generation"]) is None
            or type(bucket["count"]) is not int
            or not 1 <= bucket["count"] <= expected["capacity"]
            or type(bucket["reservedAt"]) is not int
            or not 0 <= bucket["reservedAt"] <= ledger["observedAt"]
            or (completed is not None and (
                type(completed) is not int or not bucket["reservedAt"] <= completed <= ledger["observedAt"]
            ))
        ):
            raise ValueError("登录预算账本包含损坏条目")
    document.assert_unchanged()
    return {"ledger": ledger, "descriptor": {
        "path": str(document.path), "bytes": len(document.raw), "sha256": document.sha256
    }}


def verify_redacted_log(path: Path, descriptor: object, secrets: tuple[str, ...]) -> dict:
    value = exact_fields(descriptor, {"path", "bytes", "sha256"}, "业务 crate 前端日志")
    snapshot = artifact_snapshot(path)
    if snapshot.descriptor() != value or snapshot.bytes > MAX_LOG_BYTES:
        raise ValueError("业务 crate 前端日志描述或大小无效")
    reject_link_or_reparse(path)
    before = path.lstat()
    with path.open("rb") as stream:
        opened = os.fstat(stream.fileno())
        if file_state(opened) != snapshot.state:
            raise ValueError("业务 crate 前端日志在脱敏核验前被替换")
        raw = stream.read(MAX_LOG_BYTES + 1)
        after_read = os.fstat(stream.fileno())
    reject_link_or_reparse(path)
    if (file_state(before) != snapshot.state or file_state(after_read) != snapshot.state
            or file_state(path.lstat()) != snapshot.state or len(raw) != snapshot.bytes
            or hashlib.sha256(raw).hexdigest() != snapshot.sha256
            or any(secret.encode("utf-8") in raw for secret in secrets)):
        raise ValueError("业务 crate 前端日志包含未脱敏凭据或读取期间发生变化")
    return value
