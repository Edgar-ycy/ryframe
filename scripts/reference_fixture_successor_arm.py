"""从已发布 successor 与已初始化 fresh target 派生唯一 arm-input 请求。"""

from __future__ import annotations

import copy
import json
import os
from pathlib import Path
import uuid

from devex_clone_capture import read_json
from devex_clone_model import linked, local_path, name
from devex_clone_run_state import binding
from devex_clone_seed_arm import validate_arm_request
from devex_clone_source_proof import bound_file
from restore_reference_plan import plan_hash


SIDES = ("base", "candidate")


def _controlled_directory(backend: Path, value: Path, label: str) -> Path:
    requested = value if value.is_absolute() else backend / value
    directory = local_path(backend, str(requested))
    if linked(directory) or not directory.is_dir():
        raise ValueError(f"{label}必须是受控无链接目录")
    return directory


def _descriptor(backend: Path, path: Path, label: str) -> dict:
    descriptor = binding(path)
    if bound_file(backend, descriptor) != path:
        raise ValueError(f"{label}绑定无效")
    return descriptor


def _target_descriptors(backend: Path, workspace: Path) -> tuple[dict, dict, dict, dict]:
    registration_path = workspace / "registration.json"
    initialized_path = workspace / "target/initialized.json"
    files_path = workspace / "initialized-files.json"
    registration = read_json(bound_file(backend, binding(registration_path)))
    environment = registration.get("environment")
    if not isinstance(environment, dict):
        raise ValueError("fresh target registration 缺少环境绑定")
    bound_file(backend, environment)
    return (
        _descriptor(backend, registration_path, "fresh target registration"),
        _descriptor(backend, initialized_path, "fresh target initialized"),
        _descriptor(backend, files_path, "fresh target 文件快照"),
        copy.deepcopy(environment),
    )


def build(
    backend: Path,
    successor_path: Path,
    workspace_path: Path,
    request_id: str,
    side: str,
    copy_directory: Path,
) -> dict:
    """只读构造请求，并用复制流程自身的校验器证明全部绑定。"""
    backend = backend.resolve(strict=True)
    name(request_id)
    if side not in SIDES:
        raise ValueError("successor arm 只能选择 base 或 candidate")
    successor_file = local_path(
        backend,
        str(successor_path if successor_path.is_absolute() else backend / successor_path),
    )
    if linked(successor_file) or not successor_file.is_file():
        raise ValueError("successor arm 必须绑定受控普通 relationship 文件")
    successor_binding = _descriptor(backend, successor_file, "successor relationship")
    from reference_fixture_successor import published_source

    source = published_source(backend, successor_binding, live_storage=False)
    relationship = source["review_successor"]
    workspace = _controlled_directory(backend, workspace_path, "fresh target workspace")
    registration, initialized, initialized_files, environment = _target_descriptors(
        backend, workspace
    )
    target_copy = local_path(
        backend,
        str(copy_directory if copy_directory.is_absolute() else backend / copy_directory),
        new=True,
    )
    if not target_copy.parent.is_dir() or linked(target_copy):
        raise ValueError("arm 复制目录必须位于已有的受控父目录且尚不存在")
    request = {
        "format_version": 1,
        "kind": "devex-clone-seed-successor-arm-input",
        "source_registration": copy.deepcopy(relationship["source_result"]),
        "review_successor": successor_binding,
        "id": request_id,
        "initialized": initialized,
        "target_registration": registration,
        "target_initialized_files": initialized_files,
        "target_environment": environment,
        "copy_directory": str(target_copy),
        "build_bridges": copy.deepcopy(source["manifest"]["build_bridges"]),
    }
    inputs = validate_arm_request(backend, request, live_storage=False)
    if inputs["target_side"] != side:
        raise ValueError("fresh target workspace 与请求 side 不一致")
    if (
        binding(successor_file) != successor_binding
        or published_source(backend, successor_binding, live_storage=False) != source
    ):
        raise ValueError("successor relationship 或其来源在请求构造期间变化")
    for descriptor in (registration, initialized, initialized_files, environment):
        bound_file(backend, descriptor)
    for descriptor in request["build_bridges"]:
        bound_file(backend, descriptor)
    return request


def _publish_json(path: Path, value: dict) -> None:
    pending = path.with_name(f".{path.name}.{uuid.uuid4().hex}.pending")
    try:
        with pending.open("x", encoding="utf-8", newline="\n") as stream:
            json.dump(value, stream, ensure_ascii=False, sort_keys=True, indent=2)
            stream.write("\n")
            stream.flush()
            os.fsync(stream.fileno())
        os.link(pending, path)
    finally:
        pending.unlink(missing_ok=True)


def publish(
    backend: Path,
    successor_path: Path,
    workspace_path: Path,
    request_id: str,
    side: str,
    copy_directory: Path,
    output: Path,
) -> dict:
    """以同目录独占链接发布完整 JSON，发布前后都重算全部输入。"""
    backend = backend.resolve(strict=True)
    requested = output if output.is_absolute() else backend / output
    target = local_path(backend, str(requested), new=True)
    if not target.parent.is_dir():
        raise ValueError("successor arm 请求输出父目录不存在")
    first = build(
        backend, successor_path, workspace_path, request_id, side, copy_directory
    )
    if build(
        backend, successor_path, workspace_path, request_id, side, copy_directory
    ) != first:
        raise ValueError("successor arm 请求输入在发布前发生变化")
    _publish_json(target, first)
    if read_json(target) != first or build(
        backend, successor_path, workspace_path, request_id, side, copy_directory
    ) != first:
        raise ValueError("successor arm 请求发布后无法重算；保留文件供现场核对")
    return first


def summary(request: dict, *, side: str, written: bool) -> dict:
    return {
        "status": "successor_arm_request_published" if written else "successor_arm_request_planned",
        "request_sha256": plan_hash(request),
        "side": side,
        "remote_writes": 0,
    }
