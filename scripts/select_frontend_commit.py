#!/usr/bin/env python3
"""为 CI 选择配套前端提交，并按需生成后端候选契约。"""

from __future__ import annotations

import json
import os
import re
import subprocess
import sys
from pathlib import Path


COMMIT_PATTERN = re.compile(r"[0-9a-fA-F]{40}")
RELEASE_REF_PATTERN = re.compile(r"refs/tags/(v[0-9]+\.[0-9]+\.[0-9]+)")
MARKER_PATTERN = re.compile(
    r"^[ \t]*Frontend-Commit:[ \t]*([0-9a-fA-F]{40})[ \t]*$",
    re.MULTILINE,
)
MARKER_MENTION_PATTERN = re.compile(r"Frontend-Commit", re.IGNORECASE)
OPENAPI_PATH = Path("openapi/openapi.json")
REQUEST_ENV = "RYFRAME_CI_FRONTEND_SOURCE_REQUEST"
REQUEST_FIELDS = {
    "event_name",
    "event",
    "backend_worktree",
    "base_sha",
    "prefer_marker",
    "candidate_openapi",
    "release_ref",
    "fallback_main_on_invalid_base",
}


class FrontendSelectionError(ValueError):
    """前端引用或候选契约无法安全确定。"""


def select_frontend_ref(
    body: str,
    contract_changed: bool,
    *,
    prefer_marker: bool = False,
) -> str:
    """选择前端提交；静态门禁可优先使用显式 marker。"""

    body = body.replace("\r\n", "\n").replace("\r", "\n")
    if not contract_changed and not prefer_marker:
        return "main"

    matches = MARKER_PATTERN.findall(body)
    mentions = MARKER_MENTION_PATTERN.findall(body)
    if not contract_changed and not mentions:
        return "main"
    if len(matches) != 1 or len(mentions) != 1:
        if contract_changed:
            raise FrontendSelectionError(
                "OpenAPI 已变化；PR 正文必须且只能包含一行 "
                "Frontend-Commit: <40 位 SHA>"
            )
        raise FrontendSelectionError(
            "Frontend-Commit marker 无效；PR 正文必须且只能包含一行 "
            "Frontend-Commit: <40 位 SHA>"
        )
    return matches[0].lower()


def _checked_commit(value: str, label: str) -> str:
    if COMMIT_PATTERN.fullmatch(value) is None:
        raise FrontendSelectionError(f"{label}必须是 40 位 Git SHA")
    return value.lower()


def _run(
    arguments: list[str],
    *,
    cwd: Path,
    capture_output: bool = False,
) -> subprocess.CompletedProcess[bytes]:
    completed = subprocess.run(
        arguments,
        cwd=cwd,
        check=False,
        capture_output=capture_output,
    )
    if completed.returncode != 0:
        stderr = completed.stderr.decode("utf-8", errors="replace").strip()
        raise FrontendSelectionError(
            f"命令失败（{' '.join(arguments)}）：{stderr or completed.returncode}"
        )
    return completed


def contract_changed_from_git(backend_worktree: Path, base_sha: str) -> bool:
    """只根据受信任基线和正式快照路径判断契约是否变化。"""

    base_sha = _checked_commit(base_sha, "后端基线提交")
    completed = subprocess.run(
        ["git", "diff", "--quiet", base_sha, "HEAD", "--", OPENAPI_PATH.as_posix()],
        cwd=backend_worktree,
        check=False,
        capture_output=True,
    )
    if completed.returncode not in (0, 1):
        stderr = completed.stderr.decode("utf-8", errors="replace").strip()
        raise FrontendSelectionError(f"无法比较后端 OpenAPI 变更：{stderr}")
    return completed.returncode == 1


def commit_exists_in_worktree(backend_worktree: Path, commit: str) -> bool:
    if COMMIT_PATTERN.fullmatch(commit) is None:
        return False
    completed = subprocess.run(
        ["git", "cat-file", "-e", f"{commit}^{{commit}}"],
        cwd=backend_worktree,
        check=False,
        capture_output=True,
    )
    return completed.returncode == 0


def generate_and_classify_candidate(
    backend_worktree: Path,
    base_sha: str,
    candidate_path: Path,
) -> bool:
    """生成候选契约、校验已提交快照，并与基线提交分类。"""

    base_sha = _checked_commit(base_sha, "后端基线提交")
    candidate_path.parent.mkdir(parents=True, exist_ok=True)
    _run(
        [
            "cargo",
            "run",
            "--locked",
            "--target-dir",
            "target/ci/backend",
            "-p",
            "ryframe-api",
            "--bin",
            "export_openapi",
            "--",
            str(candidate_path),
        ],
        cwd=backend_worktree,
    )
    committed = (backend_worktree / OPENAPI_PATH).read_bytes()
    candidate = candidate_path.read_bytes()
    if committed != candidate:
        raise FrontendSelectionError("后端提交未包含当前代码生成的最新 OpenAPI 快照")
    baseline = _run(
        ["git", "show", f"{base_sha}:{OPENAPI_PATH.as_posix()}"],
        cwd=backend_worktree,
        capture_output=True,
    ).stdout
    return baseline != candidate


def _pull_request_body(event_path: Path) -> str:
    try:
        event = json.loads(event_path.read_text(encoding="utf-8"))
    except (OSError, UnicodeError, json.JSONDecodeError) as error:
        raise FrontendSelectionError(f"无法读取 CI 事件 {event_path}：{error}") from error
    pull_request = event.get("pull_request")
    if not isinstance(pull_request, dict):
        raise FrontendSelectionError("CI 事件缺少 pull_request 对象")
    body = pull_request.get("body")
    if body is None:
        return ""
    if not isinstance(body, str):
        raise FrontendSelectionError("pull_request.body 必须是字符串或 null")
    return body


def select_ci_frontend_ref(
    *,
    event_name: str,
    event_path: Path | None,
    backend_worktree: Path,
    base_sha: str | None,
    prefer_marker: bool,
    candidate_path: Path | None,
    release_ref: str | None,
    fallback_main_on_invalid_base: bool = False,
) -> tuple[str, bool]:
    """返回前端引用与 OpenAPI 是否变化。"""

    if event_name != "pull_request":
        if release_ref:
            match = RELEASE_REF_PATTERN.fullmatch(release_ref)
            if match is not None:
                return match.group(1), False
        return "main", False

    if event_path is None:
        raise FrontendSelectionError("pull_request 选择缺少事件文件或基线提交")
    body = _pull_request_body(event_path)
    if base_sha is None:
        if fallback_main_on_invalid_base:
            return (
                select_frontend_ref(body, False, prefer_marker=prefer_marker),
                False,
            )
        raise FrontendSelectionError("pull_request 选择缺少事件文件或基线提交")
    if not commit_exists_in_worktree(backend_worktree, base_sha):
        if fallback_main_on_invalid_base:
            return (
                select_frontend_ref(body, False, prefer_marker=prefer_marker),
                False,
            )
        raise FrontendSelectionError("pull_request 后端基线提交无法解析")
    changed = (
        generate_and_classify_candidate(backend_worktree, base_sha, candidate_path)
        if candidate_path is not None
        else contract_changed_from_git(backend_worktree, base_sha)
    )
    ref = select_frontend_ref(body, changed, prefer_marker=prefer_marker)
    return ref, changed


def _write_outputs(ref: str, changed: bool) -> None:
    output = os.environ.get("GITHUB_OUTPUT")
    text = f"ref={ref}\nchanged={str(changed).lower()}\n"
    if output:
        with Path(output).open("a", encoding="utf-8", newline="\n") as stream:
            stream.write(text)
    print(text, end="")


def _private_request() -> dict[str, object]:
    if len(sys.argv) != 1:
        raise FrontendSelectionError("此脚本是 xtask 私有实现，不接受命令行参数")
    encoded = os.environ.get(REQUEST_ENV)
    if encoded is None:
        raise FrontendSelectionError(f"缺少 xtask 私有请求环境 {REQUEST_ENV}")
    try:
        request = json.loads(encoded)
    except json.JSONDecodeError as error:
        raise FrontendSelectionError("xtask 私有请求不是有效 JSON") from error
    if not isinstance(request, dict) or set(request) != REQUEST_FIELDS:
        raise FrontendSelectionError("xtask 私有请求字段不完整或包含未知字段")
    return request


def _optional_path(request: dict[str, object], name: str) -> Path | None:
    value = request[name]
    if value is None:
        return None
    if not isinstance(value, str) or not value:
        raise FrontendSelectionError(f"xtask 私有请求 {name} 必须是非空路径")
    return Path(value)


def _optional_text(request: dict[str, object], name: str) -> str | None:
    value = request[name]
    if value is None:
        return None
    if not isinstance(value, str) or not value:
        raise FrontendSelectionError(f"xtask 私有请求 {name} 必须是非空字符串")
    return value


def _required_text(request: dict[str, object], name: str) -> str:
    value = _optional_text(request, name)
    if value is None:
        raise FrontendSelectionError(f"xtask 私有请求缺少 {name}")
    return value


def _boolean(request: dict[str, object], name: str) -> bool:
    value = request[name]
    if not isinstance(value, bool):
        raise FrontendSelectionError(f"xtask 私有请求 {name} 必须是布尔值")
    return value


def main() -> None:
    request = _private_request()
    ref, changed = select_ci_frontend_ref(
        event_name=_required_text(request, "event_name"),
        event_path=_optional_path(request, "event"),
        backend_worktree=Path(_required_text(request, "backend_worktree")),
        base_sha=_optional_text(request, "base_sha"),
        prefer_marker=_boolean(request, "prefer_marker"),
        candidate_path=_optional_path(request, "candidate_openapi"),
        release_ref=_optional_text(request, "release_ref"),
        fallback_main_on_invalid_base=_boolean(
            request, "fallback_main_on_invalid_base"
        ),
    )
    _write_outputs(ref, changed)


if __name__ == "__main__":
    main()
