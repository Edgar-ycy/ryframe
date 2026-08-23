#!/usr/bin/env python3
"""校验前端正式契约记录的后端来源提交。"""

from __future__ import annotations

import argparse
import hashlib
import json
import re
import subprocess
import sys
from pathlib import Path
from typing import Any, NamedTuple


COMMIT_PATTERN = re.compile(r"[0-9a-f]{40}")
SHA256_PATTERN = re.compile(r"[0-9a-f]{64}")
REPOSITORY_PATTERN = re.compile(r"[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+")
SOURCE_KEYS = {
    "schema_version",
    "backend_repository",
    "backend_commit",
    "openapi_path",
    "openapi_version",
    "sha256",
}
EXPECTED_OPENAPI_PATH = "openapi/openapi.json"


class ContractSourceError(ValueError):
    """正式契约来源不可信或与候选不一致。"""


class ContractSource(NamedTuple):
    backend_repository: str
    backend_commit: str
    openapi_path: str
    openapi_version: str
    sha256: str


def _load_json(path: Path, label: str) -> Any:
    try:
        return json.loads(path.read_text(encoding="utf-8"))
    except (OSError, UnicodeError, json.JSONDecodeError) as error:
        raise ContractSourceError(f"无法读取{label} {path}: {error}") from error


def _load_bytes(path: Path, label: str) -> bytes:
    try:
        return path.read_bytes()
    except OSError as error:
        raise ContractSourceError(f"无法读取{label} {path}: {error}") from error


def parse_source_metadata(path: Path, expected_repository: str) -> ContractSource:
    document = _load_json(path, "前端正式契约来源元数据")
    if not isinstance(document, dict):
        raise ContractSourceError("前端正式契约来源元数据必须是对象")
    actual_keys = set(document)
    if actual_keys != SOURCE_KEYS:
        missing = sorted(SOURCE_KEYS - actual_keys)
        extra = sorted(actual_keys - SOURCE_KEYS)
        raise ContractSourceError(
            f"前端正式契约来源元数据字段不匹配，缺少={missing}，多余={extra}"
        )
    if document["schema_version"] != 1:
        raise ContractSourceError("前端正式契约来源 schema_version 必须为 1")

    repository = document["backend_repository"]
    if not isinstance(repository, str) or REPOSITORY_PATTERN.fullmatch(repository) is None:
        raise ContractSourceError("backend_repository 必须是 owner/repository")
    if repository != expected_repository:
        raise ContractSourceError(
            f"正式契约后端仓库不匹配：期望 {expected_repository}，实际 {repository}"
        )

    commit = document["backend_commit"]
    if not isinstance(commit, str) or COMMIT_PATTERN.fullmatch(commit) is None:
        raise ContractSourceError("backend_commit 必须是小写 40 位 Git SHA")

    openapi_path = document["openapi_path"]
    if not isinstance(openapi_path, str):
        raise ContractSourceError("openapi_path 必须是字符串")
    segments = openapi_path.split("/")
    if (
        not openapi_path.endswith(".json")
        or openapi_path.startswith("/")
        or "\\" in openapi_path
        or any(not segment or segment in {".", ".."} for segment in segments)
    ):
        raise ContractSourceError("openapi_path 必须是不含路径穿越的相对 JSON 路径")
    if openapi_path != EXPECTED_OPENAPI_PATH:
        raise ContractSourceError(
            f"openapi_path 必须是 {EXPECTED_OPENAPI_PATH}，实际 {openapi_path}"
        )

    openapi_version = document["openapi_version"]
    if not isinstance(openapi_version, str) or not openapi_version.startswith("3."):
        raise ContractSourceError("openapi_version 必须是 OpenAPI 3 版本")

    digest = document["sha256"]
    if not isinstance(digest, str) or SHA256_PATTERN.fullmatch(digest) is None:
        raise ContractSourceError("sha256 必须是小写 64 位十六进制摘要")

    return ContractSource(repository, commit, openapi_path, openapi_version, digest)


def _git(worktree: Path, arguments: list[str]) -> bytes:
    completed = subprocess.run(
        ["git", "-C", str(worktree), *arguments],
        check=False,
        capture_output=True,
    )
    if completed.returncode != 0:
        message = completed.stderr.decode("utf-8", errors="replace").strip()
        raise ContractSourceError(
            f"Git 命令失败（git {' '.join(arguments)}）：{message or completed.returncode}"
        )
    return completed.stdout


def _verified_commit(worktree: Path, value: str, label: str) -> str:
    if COMMIT_PATTERN.fullmatch(value) is None:
        raise ContractSourceError(f"{label}必须是小写 40 位 Git SHA")
    resolved = _git(worktree, ["rev-parse", "--verify", f"{value}^{{commit}}"])
    actual = resolved.decode("ascii", errors="strict").strip()
    if actual != value:
        raise ContractSourceError(f"{label}未解析到指定提交：期望 {value}，实际 {actual}")
    return actual


def verify_contract_source(
    *,
    backend_worktree: Path,
    backend_head: str,
    expected_repository: str,
    source_metadata: Path,
    frontend_openapi: Path,
    candidate_openapi: Path,
) -> str:
    if REPOSITORY_PATTERN.fullmatch(expected_repository) is None:
        raise ContractSourceError("期望后端仓库必须是 owner/repository")
    source = parse_source_metadata(source_metadata, expected_repository)
    head = _verified_commit(backend_worktree, backend_head, "后端 HEAD")
    checkout_head = _git(backend_worktree, ["rev-parse", "--verify", "HEAD^{commit}"])
    if checkout_head.decode("ascii", errors="strict").strip() != head:
        raise ContractSourceError("后端工作树 HEAD 与待校验提交不一致")
    _verified_commit(backend_worktree, source.backend_commit, "正式契约来源提交")

    ancestor = subprocess.run(
        [
            "git",
            "-C",
            str(backend_worktree),
            "merge-base",
            "--is-ancestor",
            source.backend_commit,
            head,
        ],
        check=False,
        capture_output=True,
    )
    if ancestor.returncode == 1:
        raise ContractSourceError("正式契约来源提交不是当前后端 HEAD 的祖先")
    if ancestor.returncode != 0:
        message = ancestor.stderr.decode("utf-8", errors="replace").strip()
        raise ContractSourceError(f"无法验证正式契约来源祖先关系：{message}")

    source_bytes = _git(
        backend_worktree,
        ["show", f"{source.backend_commit}:{source.openapi_path}"],
    )
    frontend_bytes = _load_bytes(frontend_openapi, "前端正式 OpenAPI")
    candidate_bytes = _load_bytes(candidate_openapi, "后端候选 OpenAPI")
    if source_bytes != frontend_bytes:
        raise ContractSourceError("来源提交中的 OpenAPI 与前端正式 OpenAPI 不一致")
    if source_bytes != candidate_bytes:
        raise ContractSourceError("来源提交中的 OpenAPI 与本次后端候选 OpenAPI 不一致")
    actual_digest = hashlib.sha256(frontend_bytes).hexdigest()
    if source.sha256 != actual_digest:
        raise ContractSourceError(
            f"前端正式契约摘要不匹配：元数据 {source.sha256}，实际 {actual_digest}"
        )

    try:
        openapi = json.loads(frontend_bytes.decode("utf-8"))
    except (UnicodeError, json.JSONDecodeError) as error:
        raise ContractSourceError(f"前端正式 OpenAPI 不是有效 UTF-8 JSON：{error}") from error
    if not isinstance(openapi, dict) or openapi.get("openapi") != source.openapi_version:
        raise ContractSourceError("前端正式 OpenAPI 版本与来源元数据不一致")
    return source.backend_commit


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--backend-worktree", type=Path, required=True)
    parser.add_argument("--backend-head", required=True)
    parser.add_argument("--backend-repository", required=True)
    parser.add_argument("--source-metadata", type=Path, required=True)
    parser.add_argument("--frontend-openapi", type=Path, required=True)
    parser.add_argument("--candidate-openapi", type=Path, required=True)
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    try:
        commit = verify_contract_source(
            backend_worktree=args.backend_worktree,
            backend_head=args.backend_head,
            expected_repository=args.backend_repository,
            source_metadata=args.source_metadata,
            frontend_openapi=args.frontend_openapi,
            candidate_openapi=args.candidate_openapi,
        )
    except ContractSourceError as error:
        print(f"正式契约来源校验失败：{error}", file=sys.stderr)
        return 1
    print(commit)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
