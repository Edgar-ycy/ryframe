"""普通恢复构建的共享读侧适配；来源、上下文和产物规则由Rust唯一实现。"""
from __future__ import annotations

import json
from pathlib import Path
import subprocess

from artifact_digests import file_digest
from build_source import build_process_environment
# 维护工具仍直接消费Cargo事件；其原生迁移不属于普通runtime职责。
from full_stack_build import cargo_artifact
from source_inventory import git, source_snapshot

ROLES = {"api": ("bin-api", "ryframe"), "worker": ("bin-worker", "ryframe-worker")}


def _native(action: str, root: Path, value=None, *, environment=None):
    from native_s3_client import _invoke, _read_response

    def controlled(command, **kwargs):
        # 固定原生读侧需要实际native构建环境；复用既有过滤，不改变全局RPC环境。
        kwargs["env"] = build_process_environment(environment)
        return subprocess.run(command, **kwargs)

    result, response = _invoke("__source-evidence", {
        "operation": "runtime-build-read", "write": False,
        "arguments": {"action": action, "root": str(root), "value": value},
    }, controlled, may_write=False)
    return _read_response(result, response)[0]["value"]


def repository(path: Path, label: str) -> Path:
    return Path(_native("repository", path))


def registered_source(coordinator: Path, source_backend: Path, expected_head: str, *,
                      adapter_contract=None, product_backend=None):
    value = _native("registered", source_backend, {
        "coordinator": str(coordinator), "expected_head": expected_head,
        "adapter_contract": adapter_contract,
        "product_backend": str(product_backend) if product_backend is not None else None,
    })
    product = value["product"]
    return Path(value["execution"]), value["inventory"], (
        (Path(product["path"]), product["inventory"]) if product is not None else None)


def build_command(role: str) -> list[str]:
    return _native("command", Path(__file__).resolve().parents[2], role)


def build_context(root: Path, *, environment=None) -> dict:
    return _native("context", root, environment=environment)


def validate_build_context(value: object) -> dict:
    return _native("context-model", Path(__file__).resolve().parents[2], value)


def verify_build(root: Path, receipt: dict, sha: str) -> None:
    _native("verify", root, {"receipt": receipt, "expected_head": sha})


def verify_build_artifacts(receipt: dict) -> None:
    _native("artifacts", Path(__file__).resolve().parents[2], receipt)


def validate_new_output(path: Path, root: Path) -> Path:
    path = path.resolve()
    local = (root / ".local-tests").resolve()
    if path == local or not path.is_relative_to(local):
        raise ValueError("运行收据只能写入当前仓库忽略的 .local-tests")
    if path.exists():
        raise FileExistsError(f"运行收据已经存在：{path}")
    if not path.parent.is_dir():
        raise ValueError("运行收据的父目录必须已经存在")
    git(root, "check-ignore", path.relative_to(root).as_posix())
    return path


def write_new(path: Path, value: dict, root: Path) -> None:
    path = validate_new_output(path, root)
    with path.open("x", encoding="utf-8", newline="\n") as stream:
        json.dump(value, stream, ensure_ascii=False, indent=2)
        stream.write("\n")
