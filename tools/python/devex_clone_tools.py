"""只读核验开发复制维护工具的来源、Cargo 事件与冻结产物。"""
from __future__ import annotations

import os
from pathlib import Path
import subprocess

from devex_clone_model import local_path
from devex_provenance import worktree_fingerprint
from restore_build import _native as native_build, file_digest, source_snapshot
from source_fingerprints import checked_source, reusable_artifact_source
from source_inventory import _native as native_source

TOOLS = {"reset": ("bin-reset", "ryframe-reset"), "migrate": ("bin-migrate", "ryframe-migrate"),
         "tenant-data": ("bin-tenant-data", "ryframe-tenant-data")}


def command(backend: Path, role: str, target: Path) -> list[str]:
    return native_build("maintenance-command", backend, {"role": role, "target": str(target)})


def source_binding(backend: Path) -> dict:
    current = checked_source(backend)
    if current is not None:
        return {key: current[key] for key in ("snapshot", "worktree_fingerprint")}
    source = source_snapshot(backend)
    return {"snapshot": source, "worktree_fingerprint": worktree_fingerprint(backend, source["head"])}


def toolchain(backend: Path, run=subprocess.run) -> dict:
    result = {}
    for name, args in (("rustc", ["rustc", "-Vv"]), ("cargo", ["cargo", "-V"])):
        output = run(args, cwd=backend, check=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                     creationflags=subprocess.CREATE_NO_WINDOW if os.name == "nt" else 0)
        result[name] = output.stdout.decode("utf-8").strip()
    return result


def target_directory(backend: Path) -> Path:
    return Path(native_source("maintenance-target-directory", root=str(backend)))


def verify_evidence(backend: Path, filename: Path, receipt: dict | None = None) -> dict:
    return native_build("maintenance-evidence", backend, {"filename": str(filename), "receipt": receipt})


def verify(backend: Path, filename: Path, run=subprocess.run) -> dict:
    """只读重新校验当前开发源码、工具链、Cargo 事件及冻结的维护 CLI 字节。"""
    filename = local_path(backend, str(filename))
    before = file_digest(filename)
    receipt = verify_evidence(backend, filename)
    jobs = native_build("jobs", backend)
    current = source_binding(backend)
    original = reusable_artifact_source(backend, receipt)
    if (receipt["source"] != (original or current)
            or receipt["toolchain"] != toolchain(backend, run)
            or receipt["artifacts"]["reset"]["command"][11] != str(jobs)):
        raise ValueError("维护工具未绑定当前完整开发源码和工具链")
    if file_digest(filename) != before or source_binding(backend) != current or native_build("jobs", backend) != jobs:
        raise ValueError("校验维护工具期间源码或收据变化")
    return receipt
