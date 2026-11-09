"""读取已登记的 seed 后继关系和深层来源。"""

from __future__ import annotations

import copy
from pathlib import Path

from devex_clone_capture import read_bound_json
from devex_clone_model import digest, exact, linked, local_path
from devex_clone_run_state import binding
from devex_clone_seed_source import _published_source as _deep_published_source
from devex_clone_target_binding import predecessor_request_binding, review_successor_binding
from restore_reference_plan import plan_hash


def _document(
    backend: Path, value: Path, expected: dict | None = None
) -> tuple[Path, dict, dict]:
    requested = value if value.is_absolute() else backend / value
    path = local_path(backend, str(requested))
    if linked(path) or not path.is_file():
        raise ValueError("successor 输入必须是受控普通文件")
    descriptor = binding(path)
    if expected is not None and descriptor != expected:
        raise ValueError("successor 输入绑定已变化")
    return path, read_bound_json(path, descriptor), descriptor


def _descriptor_path(value: dict, label: str, *, canonical: bool = False) -> Path:
    fields = {"path", "bytes", "sha256"}
    if canonical:
        fields.add("canonical_sha256")
    if not isinstance(value, dict):
        raise ValueError(f"successor {label} 绑定结构无效")
    exact(value, fields)
    if not isinstance(value["path"], str):
        raise ValueError(f"successor {label} 路径无效")
    digest(value["sha256"])
    if canonical:
        digest(value["canonical_sha256"])
    return Path(value["path"])


def _source_with_loader(
    backend: Path, descriptor: dict, *, live_storage: bool, loader
) -> dict:
    """通过 successor 复核明确绑定的历史 seed，普通发布源仍要求 ready。"""
    backend = backend.resolve(strict=True)
    expected_descriptor = copy.deepcopy(descriptor)
    successor = review_successor_binding(backend, expected_descriptor)
    expected = successor["predecessor_request"]
    expected_binding = {key: expected[key] for key in ("path", "bytes", "sha256")}
    _, historical_request, _ = _document(
        backend,
        _descriptor_path(expected, "predecessor request", canonical=True),
        expected_binding,
    )
    if plan_hash(historical_request) != expected["canonical_sha256"]:
        raise ValueError("successor 历史 seed 请求规范摘要无效")
    validation_count = 0

    def validate_predecessor(root: Path, seed_target: dict) -> tuple[dict, dict]:
        nonlocal validation_count
        validation_count += 1
        if seed_target != historical_request:
            raise ValueError("发布源初始化历史不是 successor 绑定的 seed 请求")
        return predecessor_request_binding(
            root, seed_target, successor["predecessor_review"]
        )

    source = loader(
        backend,
        successor["source_result"],
        live_storage=live_storage,
        validate_seed_target=validate_predecessor,
    )
    if (
        validation_count != 1
        or source["result"].get("registration") != successor["source_registration"]
        or source["seed_target"] != historical_request
    ):
        raise ValueError("发布源与 seed review successor 关系不一致")
    if source.get("source_rebind") is not None:
        _, rebind, _ = _document(backend, Path(source["source_rebind"]["path"]), source["source_rebind"])
        if rebind.get("review_successor") != expected_descriptor:
            raise ValueError("存储重绑定不属于当前 successor 关系")
    if (
        descriptor != expected_descriptor
        or review_successor_binding(backend, expected_descriptor) != successor
    ):
        raise ValueError("seed review successor 或其输入在核对期间变化")
    return {
        **source,
        "review_successor": successor,
        "review_successor_binding": expected_descriptor,
    }


def published_source(backend: Path, descriptor: dict, *, live_storage: bool = False) -> dict:
    return _source_with_loader(backend, descriptor, live_storage=live_storage,
                               loader=_deep_published_source)
