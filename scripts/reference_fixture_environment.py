"""为新的隔离参考夹具生成首代环境计划；plan 只读，不启动服务或创建资源。"""
from __future__ import annotations

import argparse
import json
from pathlib import Path

from devex_clone_capture import read_json
from devex_clone_model import linked, local_path
from devex_clone_target_binding import KEYS, validate_review
from restore_build import file_digest
from restore_reference_plan import plan_hash


def bound(path: Path) -> dict:
    return {"path": str(path), **file_digest(path)}


def _read(backend: Path, value: Path) -> tuple[Path, dict]:
    path = local_path(backend, str(value))
    if linked(path) or not path.is_file():
        raise ValueError("夹具输入必须是受控目录中的普通文件")
    return path, read_json(path)


def _fixture(value: dict) -> None:
    if (value.get("format_version") != 1 or value.get("fixture") != "device"
            or value.get("status") != "ready" or not isinstance(value.get("sources"), dict)
            or set(value.get("paths", {})) != {"backend", "frontend"}
            or not all(isinstance(item, dict) and isinstance(item.get("head"), str)
                       for item in value["sources"].values())):
        raise ValueError("Device 隔离工作树收据不完整或尚未就绪")


def _maintenance(value: dict) -> None:
    artifacts = value.get("artifacts")
    if (value.get("format_version") != 1 or value.get("status") != "maintenance_build_created"
            or not isinstance(artifacts, dict)
            or not {"reset", "migrate", "tenant-data"}.issubset(artifacts)):
        raise ValueError("维护构建收据不完整")
    for name in ("reset", "migrate", "tenant-data"):
        item = artifacts[name]
        if not isinstance(item, dict) or not isinstance(item.get("executable"), str):
            raise ValueError("维护构建产物描述无效")


def plan(backend: Path, review_path: Path, fixture_path: Path, maintenance_path: Path) -> dict:
    """验证 C52 无关的输入，并返回后续显式创建阶段应消费的不可变描述。"""
    backend = backend.resolve(strict=True)
    review_file, review = _read(backend, review_path)
    fixture_file, fixture = _read(backend, fixture_path)
    maintenance_file, maintenance = _read(backend, maintenance_path)
    validate_review(review)
    _fixture(fixture)
    _maintenance(maintenance)
    reference = review["reference"]
    if any(reference[side]["databases"] for side in ("source", "protected_target")):
        raise ValueError("新的隔离参考夹具不得读取或复用历史来源数据库")
    seed = review["scopes"]["seed"]
    databases = {item["key"]: item for item in seed["databases"]}
    if set(databases) != set(KEYS):
        raise ValueError("seed 侧必须精确声明四个数据库")
    names = [entry["database"].lower() for side in review["scopes"].values()
             for entry in side["databases"]]
    if len(names) != 12 or len(set(names)) != len(names):
        raise ValueError("三侧夹具数据库名称不得重叠")
    result = {
        "format_version": 1,
        "kind": "reference-fixture-environment-plan",
        "review": {**bound(review_file), "canonical_sha256": plan_hash(review)},
        "fixture": bound(fixture_file),
        "maintenance_build": bound(maintenance_file),
        "side": "seed",
        "scope_id": seed["scope_id"],
        "databases": [{"key": key, "database": databases[key]["database"],
                       "mode": KEYS[key][1]} for key in sorted(databases)],
        "object_endpoint": seed["objects"]["endpoint"],
        "redis_url": seed["redis"]["url"],
        "historical_data_used": False,
        "remote_writes": 0,
    }
    return {**result, "sha256": plan_hash(result)}


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("operation", choices=("plan",))
    parser.add_argument("--backend-dir", type=Path, required=True)
    parser.add_argument("--review", type=Path, required=True)
    parser.add_argument("--fixture", type=Path, required=True)
    parser.add_argument("--maintenance-build", type=Path, required=True)
    args = parser.parse_args()
    print(json.dumps(plan(args.backend_dir, args.review, args.fixture, args.maintenance_build), ensure_ascii=False))


if __name__ == "__main__":
    main()
