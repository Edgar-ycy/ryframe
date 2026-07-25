#!/usr/bin/env python3
"""Validate immutable inputs for coordinated RyFrame releases."""

from __future__ import annotations

import argparse
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


@dataclass(frozen=True)
class ReleaseIdentity:
    tag: str
    version: str
    stable_tag: str


def fail(message: str) -> None:
    raise ValueError(message)


def load_json(path: Path) -> object:
    try:
        return json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        fail(f"cannot read JSON from {path}: {error}")


def release_identity(tag: str) -> ReleaseIdentity:
    match = RELEASE_TAG.fullmatch(tag)
    if match is None:
        fail("release tag must be canonical vMAJOR.MINOR.PATCH (RC tags are not supported)")
    version = match.group("version")
    return ReleaseIdentity(
        tag=tag,
        version=version,
        stable_tag=f"v{version}",
    )


def workspace_version() -> str:
    manifest = tomllib.loads((ROOT / "Cargo.toml").read_text(encoding="utf-8"))
    try:
        return str(manifest["workspace"]["package"]["version"])
    except KeyError as error:
        fail(f"workspace.package.version is missing: {error}")


def normalize_markdown(value: str) -> str:
    """Normalize line endings and insignificant trailing whitespace."""
    lines = value.replace("\r\n", "\n").replace("\r", "\n").split("\n")
    normalized = [line.rstrip() for line in lines]
    while normalized and not normalized[0]:
        normalized.pop(0)
    while normalized and not normalized[-1]:
        normalized.pop()
    return "\n".join(normalized)


def changelog_section(path: Path, stable_tag: str, label: str) -> str:
    """Return one exact, non-empty Keep a Changelog version section."""
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


def validate_annotated_tag_notes(
    repository: Path,
    tag: str,
    changelog_path: Path,
    stable_tag: str,
    label: str,
) -> str:
    """Require an annotated tag equal to the repository's exact notes."""
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


def validate_openapi(path: Path, expected: str, label: str) -> object:
    contract = load_json(path)
    contract_version = str(contract.get("info", {}).get("version", ""))
    if contract_version != expected:
        fail(f"{label} OpenAPI info.version is {contract_version!r}, expected {expected!r}")
    return contract


def validate_frontend(
    frontend: Path,
    tag: str,
    version: str,
    stable_tag: str,
) -> None:
    if not frontend.is_dir():
        fail(f"frontend directory does not exist: {frontend}")

    openapi_path = frontend / "openapi" / "openapi.json"
    validate_openapi(openapi_path, version, "frontend")

    validate_annotated_tag_notes(
        frontend,
        tag,
        frontend / "CHANGELOG.md",
        stable_tag,
        "frontend",
    )


def git_commit(repository: Path, revision: str) -> str:
    return git_text(repository, "rev-parse", f"{revision}^{{commit}}")


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--tag", required=True)
    parser.add_argument("--frontend-dir", type=Path, required=True)
    args = parser.parse_args()

    try:
        identity = release_identity(args.tag)
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
        frontend = args.frontend_dir.resolve()
        validate_frontend(
            frontend,
            identity.tag,
            identity.version,
            identity.stable_tag,
        )
    except subprocess.CalledProcessError as error:
        detail = (error.stderr or error.stdout or "").strip()
        print(f"Release validation command failed: {error.cmd}", file=sys.stderr)
        if detail:
            print(detail, file=sys.stderr)
        return 1
    except (OSError, KeyError, ValueError) as error:
        print(f"Release validation failed: {error}", file=sys.stderr)
        return 1

    print(
        f"Release inputs are consistent: {identity.tag}, "
        f"backend/frontend/OpenAPI {identity.version}"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())