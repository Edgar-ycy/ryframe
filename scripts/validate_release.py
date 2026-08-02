#!/usr/bin/env python3
"""校验不可变输入并生成确定性的联合发布清单。"""

from __future__ import annotations

import argparse
import hashlib
import json
import re
import subprocess
import sys
import tomllib
from dataclasses import dataclass
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
RELEASE_TAG = re.compile(
    r"^v(?P<version>(?:0|[1-9]\d*)\.(?:0|[1-9]\d*)\.(?:0|[1-9]\d*))$"
)
REPOSITORY_SLUG = re.compile(
    r"^[A-Za-z0-9][A-Za-z0-9_.-]*/[A-Za-z0-9][A-Za-z0-9_.-]*$"
)
COMMIT_SHA = re.compile(r"^[0-9a-f]{40}$")


@dataclass(frozen=True)
class ReleaseIdentity:
    tag: str
    version: str
    stable_tag: str


@dataclass(frozen=True)
class RepositoryRef:
    repository: str
    tag_object: str
    commit: str


def fail(message: str) -> None:
    raise ValueError(message)


def load_json(path: Path) -> object:
    try:
        return json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        fail(f"cannot read JSON from {path}: {error}")


def json_object(path: Path, label: str) -> dict[str, object]:
    value = load_json(path)
    if not isinstance(value, dict):
        fail(f"{label} must contain a JSON object: {path}")
    return value


def release_identity(tag: str) -> ReleaseIdentity:
    match = RELEASE_TAG.fullmatch(tag)
    if match is None:
        fail(
            "release tag must be canonical vMAJOR.MINOR.PATCH "
            "(prerelease tags are not supported)"
        )
    version = match.group("version")
    return ReleaseIdentity(tag=tag, version=version, stable_tag=tag)


def repository_slug(value: str, label: str) -> str:
    if REPOSITORY_SLUG.fullmatch(value) is None:
        fail(f"{label} repository must be an owner/name slug, got {value!r}")
    return value


def commit_sha(value: str, label: str) -> str:
    if COMMIT_SHA.fullmatch(value) is None:
        fail(f"{label} commit must be a lowercase 40-character SHA, got {value!r}")
    return value


def workspace_version() -> str:
    manifest = tomllib.loads((ROOT / "Cargo.toml").read_text(encoding="utf-8"))
    try:
        return str(manifest["workspace"]["package"]["version"])
    except KeyError as error:
        fail(f"workspace.package.version is missing: {error}")


def normalize_markdown(value: str) -> str:
    """规范化换行符和无意义的行尾空白。"""
    lines = value.replace("\r\n", "\n").replace("\r", "\n").split("\n")
    normalized = [line.rstrip() for line in lines]
    while normalized and not normalized[0]:
        normalized.pop(0)
    while normalized and not normalized[-1]:
        normalized.pop()
    return "\n".join(normalized)


def changelog_section(path: Path, stable_tag: str, label: str) -> str:
    """返回一个精确且非空的 Keep a Changelog 版本章节。"""
    changelog = (
        path.read_text(encoding="utf-8")
        .replace("\r\n", "\n")
        .replace("\r", "\n")
    )
    heading = re.compile(
        rf"^## \[{re.escape(stable_tag)}\](?:[ \t]+.*)?$", re.MULTILINE
    )
    match = heading.search(changelog)
    if match is None:
        fail(f"{label} CHANGELOG has no exact section for {stable_tag}")

    remainder = changelog[match.end() :]
    next_heading = re.search(
        r"^## \[[^\]\r\n]+\](?:[ \t]+.*)?$", remainder, re.MULTILINE
    )
    end = match.end() + (next_heading.start() if next_heading else len(remainder))
    section = normalize_markdown(changelog[match.start() : end])
    if re.search(r"^-[ \t]+\S", section, re.MULTILINE) is None:
        fail(
            f"{label} CHANGELOG section {stable_tag} must contain at least "
            "one update item"
        )
    return section


def validate_changelog(stable_tag: str) -> str:
    return changelog_section(ROOT / "CHANGELOG.md", stable_tag, "backend")


def git_text(repository: Path, *args: str) -> str:
    return subprocess.run(
        ["git", "-C", str(repository), *args],
        check=True,
        capture_output=True,
        text=True,
        encoding="utf-8",
    ).stdout.strip()


def git_commit(repository: Path, revision: str) -> str:
    return git_text(repository, "rev-parse", f"{revision}^{{commit}}")


def git_tag_object(repository: Path, tag: str) -> str:
    return git_text(repository, "rev-parse", f"refs/tags/{tag}")


def validate_annotated_tag_notes(
    repository: Path,
    tag: str,
    changelog_path: Path,
    stable_tag: str,
    label: str,
) -> str:
    """要求带注释标签与仓库中的精确发布说明一致。"""
    tag_ref = f"refs/tags/{tag}"
    object_type = git_text(repository, "cat-file", "-t", tag_ref)
    if object_type != "tag":
        fail(f"{label} tag {tag} must be an annotated tag, found {object_type}")

    notes = normalize_markdown(
        git_text(repository, "for-each-ref", "--format=%(contents)", tag_ref)
    )
    expected = changelog_section(changelog_path, stable_tag, label)
    if not notes:
        fail(f"{label} tag {tag} annotation must not be empty")
    if notes != expected:
        fail(
            f"{label} tag {tag} annotation must equal the exact "
            f"{stable_tag} CHANGELOG section"
        )
    return expected


def validate_workspace_packages(expected: str) -> None:
    completed = subprocess.run(
        ["cargo", "metadata", "--format-version", "1", "--no-deps", "--locked"],
        cwd=ROOT,
        check=True,
        capture_output=True,
        text=True,
        encoding="utf-8",
    )
    metadata = json.loads(completed.stdout)
    members = set(metadata["workspace_members"])
    mismatches = sorted(
        f"{package['name']}={package['version']}"
        for package in metadata["packages"]
        if package["id"] in members and package["version"] != expected
    )
    if mismatches:
        fail(f"workspace packages must all be {expected}: {', '.join(mismatches)}")


def validate_openapi(path: Path, expected: str, label: str) -> str:
    contract = json_object(path, f"{label} OpenAPI document")
    info = contract.get("info")
    if not isinstance(info, dict):
        fail(f"{label} OpenAPI info must be an object")
    contract_version = str(info.get("version", ""))
    if contract_version != expected:
        fail(f"{label} OpenAPI info.version is {contract_version!r}, expected {expected!r}")
    try:
        return hashlib.sha256(path.read_bytes()).hexdigest()
    except OSError as error:
        fail(f"cannot hash {label} OpenAPI document {path}: {error}")


def validate_package_version(frontend: Path, expected: str) -> None:
    package = json_object(frontend / "package.json", "frontend package.json")
    package_version = str(package.get("version", ""))
    if package_version != expected:
        fail(
            f"frontend package.json version is {package_version!r}, "
            f"expected {expected!r}"
        )


def validate_repository_ref(
    repository: Path,
    tag: str,
    expected_commit: str,
    label: str,
) -> RepositoryRef:
    actual_commit = git_commit(repository, tag)
    if actual_commit != expected_commit:
        fail(
            f"{label} tag {tag} resolves to {actual_commit}, "
            f"expected fixed commit {expected_commit}"
        )
    return RepositoryRef(
        repository="",
        tag_object=git_tag_object(repository, tag),
        commit=actual_commit,
    )


def validate_frontend(
    frontend: Path,
    tag: str,
    version: str,
    stable_tag: str,
    expected_commit: str,
) -> tuple[RepositoryRef, str]:
    if not frontend.is_dir():
        fail(f"frontend directory does not exist: {frontend}")

    validate_package_version(frontend, version)
    openapi_hash = validate_openapi(
        frontend / "openapi" / "openapi.json", version, "frontend"
    )
    validate_annotated_tag_notes(
        frontend,
        tag,
        frontend / "CHANGELOG.md",
        stable_tag,
        "frontend",
    )
    return (
        validate_repository_ref(frontend, tag, expected_commit, "frontend"),
        openapi_hash,
    )


def release_manifest(
    identity: ReleaseIdentity,
    backend: RepositoryRef,
    frontend: RepositoryRef,
    backend_openapi_hash: str,
    frontend_openapi_hash: str,
) -> dict[str, object]:
    if backend_openapi_hash != frontend_openapi_hash:
        fail(
            "backend and frontend OpenAPI hashes differ: "
            f"{backend_openapi_hash} != {frontend_openapi_hash}"
        )
    return {
        "backend": {
            "commit": backend.commit,
            "openapi_sha256": backend_openapi_hash,
            "repository": backend.repository,
            "tag_object": backend.tag_object,
            "version": identity.version,
        },
        "contract": {"openapi_sha256": backend_openapi_hash},
        "frontend": {
            "commit": frontend.commit,
            "openapi_sha256": frontend_openapi_hash,
            "repository": frontend.repository,
            "tag_object": frontend.tag_object,
            "version": identity.version,
        },
        "release": {"tag": identity.tag, "version": identity.version},
        "schema_version": 1,
    }


def serialize_manifest(manifest: dict[str, object]) -> str:
    return json.dumps(manifest, ensure_ascii=False, indent=2, sort_keys=True) + "\n"


def write_manifest(path: Path, manifest: dict[str, object]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(serialize_manifest(manifest), encoding="utf-8")


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--tag", required=True)
    parser.add_argument("--frontend-dir", type=Path, required=True)
    parser.add_argument("--backend-repository", required=True)
    parser.add_argument("--backend-commit", required=True)
    parser.add_argument("--frontend-repository", required=True)
    parser.add_argument("--frontend-commit", required=True)
    parser.add_argument("--manifest-path", type=Path, required=True)
    args = parser.parse_args()

    try:
        identity = release_identity(args.tag)
        backend_repository = repository_slug(args.backend_repository, "backend")
        frontend_repository = repository_slug(args.frontend_repository, "frontend")
        backend_commit = commit_sha(args.backend_commit, "backend")
        frontend_commit = commit_sha(args.frontend_commit, "frontend")
        root_version = workspace_version()
        if root_version != identity.version:
            fail(
                f"workspace version is {root_version!r}, "
                f"tag requires {identity.version!r}"
            )
        validate_changelog(identity.stable_tag)
        validate_annotated_tag_notes(
            ROOT,
            identity.tag,
            ROOT / "CHANGELOG.md",
            identity.stable_tag,
            "backend",
        )
        validate_workspace_packages(identity.version)
        backend_ref = validate_repository_ref(
            ROOT, identity.tag, backend_commit, "backend"
        )
        backend_ref = RepositoryRef(
            repository=backend_repository,
            tag_object=backend_ref.tag_object,
            commit=backend_ref.commit,
        )
        backend_openapi_hash = validate_openapi(
            ROOT / "openapi" / "openapi.json", identity.version, "backend"
        )
        frontend = args.frontend_dir.resolve()
        frontend_ref, frontend_openapi_hash = validate_frontend(
            frontend,
            identity.tag,
            identity.version,
            identity.stable_tag,
            frontend_commit,
        )
        frontend_ref = RepositoryRef(
            repository=frontend_repository,
            tag_object=frontend_ref.tag_object,
            commit=frontend_ref.commit,
        )
        manifest = release_manifest(
            identity,
            backend_ref,
            frontend_ref,
            backend_openapi_hash,
            frontend_openapi_hash,
        )
        write_manifest(args.manifest_path, manifest)
    except subprocess.CalledProcessError as error:
        detail = (error.stderr or error.stdout or "").strip()
        print(f"Release validation command failed: {error.cmd}", file=sys.stderr)
        if detail:
            print(detail, file=sys.stderr)
        return 1
    except (OSError, KeyError, ValueError, tomllib.TOMLDecodeError) as error:
        print(f"Release validation failed: {error}", file=sys.stderr)
        return 1

    print(
        f"Release inputs are consistent: {identity.tag}, "
        f"backend/frontend/OpenAPI {identity.version}; "
        f"manifest {args.manifest_path}"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
