"""为新的隔离参考夹具生成首代环境计划；plan 只读，不启动服务或创建资源。"""
from __future__ import annotations

import copy
from pathlib import Path

from reference_fixture_service_context import load_prepared_environment
from restore_build import file_digest
from reference_fixture_environment_native import invoke as environment_native


SECRET_FILES = ("mysql-client.cnf", "rustfs-access-key.txt", "rustfs-secret-key.txt", "redis-password.txt",
                "reset-admin-password.txt", "jwt-secret.txt", "metrics-token.txt")


def bound(path: Path) -> dict:
    return {"path": str(path), **file_digest(path)}


def prepared_environment(backend: Path, value: Path) -> tuple[Path, Path, dict, dict]:
    """读取并复核唯一环境 bootstrap、私有环境摘要及全部秘密文件。"""
    return load_prepared_environment(backend, value, SECRET_FILES)




def _preflight_binding(review: dict) -> dict:
    return environment_native("preflight-binding", review)


def _preflight(review: dict) -> dict:
    from devex_clone_cache_native import invoke
    return invoke("preflight-tools", review=review)


def validate_current_review_tools(review: dict) -> dict:
    """重新观察 ready 审阅绑定的工具，并拒绝本机工具代次漂移。"""
    expected = _preflight_binding(review)
    observed = _preflight(review)
    if observed != expected:
        raise ValueError("夹具审阅计划中的当前工具已经变化")
    return observed


def validated_recorded_review_tools(review: dict) -> dict:
    """只验证已发布预检收据的冻结工具，不声称当前本机工具仍可用。"""
    return copy.deepcopy(_preflight_binding(review))


def validate_preflight_successor(previous: dict, review: dict) -> None:
    environment_native("preflight-successor", {"previous": previous, "review": review})


def plan(backend: Path, review_path: Path, execution_source_path: Path, maintenance_path: Path, side: str = "seed") -> dict:
    return environment_native("plan", {"backend": str(backend.resolve(strict=True)),
        "review": str(review_path), "execution_source": str(execution_source_path),
        "maintenance_build": str(maintenance_path), "side": side})
