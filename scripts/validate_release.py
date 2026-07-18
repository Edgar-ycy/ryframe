#!/usr/bin/env python3
"""Validate immutable inputs for coordinated RyFrame RC and stable releases."""

from __future__ import annotations

import argparse
import datetime as dt
import json
import re
import subprocess
import sys
import tomllib
from dataclasses import dataclass
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
MINIMUM_RC_HOURS = 48
RELEASE_TAG = re.compile(
    r"^v(?P<version>(?:0|[1-9]\d*)\.(?:0|[1-9]\d*)\.(?:0|[1-9]\d*))"
    r"(?P<rc>-rc\.[1-9]\d*)?$"
)


@dataclass(frozen=True)
class ReleaseIdentity:
    tag: str
    version: str
    stable_tag: str
    prerelease: bool


@dataclass(frozen=True)
class RcRelease:
    tag: str
    published_at: dt.datetime


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
        fail(
            "release tag must be canonical vMAJOR.MINOR.PATCH or "
            f"vMAJOR.MINOR.PATCH-rc.N: {tag}"
        )
    version = match.group("version")
    return ReleaseIdentity(
        tag=tag,
        version=version,
        stable_tag=f"v{version}",
        prerelease=match.group("rc") is not None,
    )


def workspace_version() -> str:
    manifest = tomllib.loads((ROOT / "Cargo.toml").read_text(encoding="utf-8"))
    try:
        return str(manifest["workspace"]["package"]["version"])
    except KeyError as error:
        fail(f"workspace.package.version is missing: {error}")


def validate_changelog(stable_tag: str) -> None:
    changelog = (ROOT / "CHANGELOG.md").read_text(encoding="utf-8")
    if (
        re.search(
            rf"^## \[{re.escape(stable_tag)}\](?:\s|$)", changelog, re.MULTILINE
        )
        is None
    ):
        fail(f"CHANGELOG.md has no exact section for {stable_tag}")


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
        fail(
            "workspace package versions differ from "
            f"{expected}: {', '.join(mismatches)}"
        )


def validate_openapi(path: Path, expected: str, label: str) -> object:
    document = load_json(path)
    if not isinstance(document, dict):
        fail(f"{label} OpenAPI document must be a JSON object")
    actual = document.get("info", {}).get("version")
    if actual != expected:
        fail(f"{label} OpenAPI version is {actual!r}, expected {expected!r}")
    return document


def validate_frontend(frontend: Path, tag: str, expected: str) -> None:
    package = load_json(frontend / "package.json")
    if not isinstance(package, dict) or package.get("version") != expected:
        actual = package.get("version") if isinstance(package, dict) else None
        fail(f"frontend package version is {actual!r}, expected {expected!r}")

    backend_contract = validate_openapi(
        ROOT / "openapi" / "openapi.json", expected, "backend"
    )
    frontend_contract = validate_openapi(
        frontend / "openapi" / "openapi.json", expected, "frontend"
    )
    if backend_contract != frontend_contract:
        fail("frontend and backend OpenAPI snapshots differ")

    if git_commit(frontend, tag) != git_commit(frontend, "HEAD"):
        fail(f"frontend checkout does not point to expected tag {tag!r}")


def parse_timestamp(value: str) -> dt.datetime:
    parsed = dt.datetime.fromisoformat(value.replace("Z", "+00:00"))
    if parsed.tzinfo is None:
        fail(f"release timestamp has no timezone: {value}")
    return parsed.astimezone(dt.timezone.utc)


def latest_published_rc(
    releases_path: Path,
    stable_tag: str,
    minimum_hours: int,
    *,
    now: dt.datetime | None = None,
) -> RcRelease:
    if minimum_hours < MINIMUM_RC_HOURS:
        fail(
            f"minimum RC observation cannot be less than {MINIMUM_RC_HOURS} hours"
        )
    releases = load_json(releases_path)
    if not isinstance(releases, list):
        fail("GitHub releases response must be a JSON array")

    rc_pattern = re.compile(re.escape(stable_tag) + r"-rc\.[1-9]\d*$")
    eligible: list[tuple[dt.datetime, str]] = []
    for release in releases:
        if not isinstance(release, dict):
            continue
        tag_name = str(release.get("tag_name", ""))
        published_at = release.get("published_at")
        if (
            release.get("prerelease") is True
            and release.get("draft") is not True
            and published_at
            and rc_pattern.fullmatch(tag_name)
        ):
            eligible.append((parse_timestamp(str(published_at)), tag_name))

    if not eligible:
        fail(f"no published prerelease matching {stable_tag}-rc.N was found")

    # Only the newest published candidate may authorize stable. An older RC cannot
    # satisfy the window after a replacement candidate has been published.
    published, tag_name = max(eligible)
    current_time = now or dt.datetime.now(dt.timezone.utc)
    observed_hours = (current_time - published).total_seconds() / 3600
    if observed_hours < 0:
        fail(f"{tag_name} has a future publication timestamp")
    if observed_hours < minimum_hours:
        fail(
            f"{tag_name} has only {observed_hours:.1f} observed hours; "
            f"stable requires at least {minimum_hours}"
        )
    return RcRelease(tag=tag_name, published_at=published)


def latest_eligible_rc(
    releases_path: Path,
    stable_tag: str,
    minimum_hours: int,
    *,
    now: dt.datetime | None = None,
) -> str:
    """Return the newest qualified tag (compatibility wrapper for callers)."""
    return latest_published_rc(
        releases_path, stable_tag, minimum_hours, now=now
    ).tag


def _evidence_url(status: dict[str, object]) -> str | None:
    for field in ("environment_url", "log_url", "target_url"):
        value = status.get(field)
        if isinstance(value, str) and value.startswith("https://"):
            return value
    return None


def validate_rc_observation(
    deployments_path: Path,
    rc_tag: str,
    rc_commit: str,
    minimum_hours: int,
    *,
    not_before: dt.datetime,
    now: dt.datetime | None = None,
) -> int:
    """Validate a commit-bound, uninterrupted RC Deployment observation."""
    if minimum_hours < MINIMUM_RC_HOURS:
        fail(
            f"minimum RC observation cannot be less than {MINIMUM_RC_HOURS} hours"
        )
    deployments = load_json(deployments_path)
    if not isinstance(deployments, list):
        fail("GitHub deployments evidence must be a JSON array")

    current_time = now or dt.datetime.now(dt.timezone.utc)
    matching: list[tuple[dt.datetime, int, dict[str, object]]] = []
    for deployment in deployments:
        if not isinstance(deployment, dict):
            continue
        if (
            deployment.get("environment") != "release-candidate"
            or deployment.get("ref") != rc_tag
            or deployment.get("sha") != rc_commit
        ):
            continue
        deployment_id = deployment.get("id")
        statuses = deployment.get("statuses")
        created_at = deployment.get("created_at")
        if (
            not isinstance(deployment_id, int)
            or not isinstance(statuses, list)
            or not created_at
        ):
            continue
        matching.append(
            (parse_timestamp(str(created_at)), deployment_id, deployment)
        )

    # A superseded deployment cannot authorize stable after a newer attempt has
    # failed or not yet completed its own observation window.
    candidates = (
        [max(matching, key=lambda item: (item[0], item[1]))]
        if matching
        else []
    )
    qualified: list[tuple[dt.datetime, int]] = []
    for _, deployment_id, deployment in candidates:
        statuses = deployment["statuses"]
        assert isinstance(statuses, list)

        ordered: list[tuple[dt.datetime, dict[str, object]]] = []
        for status in statuses:
            if not isinstance(status, dict) or not status.get("created_at"):
                continue
            ordered.append((parse_timestamp(str(status["created_at"])), status))
        ordered.sort(key=lambda item: item[0])
        if not ordered or ordered[-1][1].get("state") != "success":
            continue

        active_since: dt.datetime | None = None
        for timestamp, status in ordered:
            state = status.get("state")
            if timestamp > current_time:
                active_since = None
                continue
            if state == "in_progress":
                # A new in-progress status starts (or restarts) the auditable
                # continuous window. It must follow publication of this RC.
                active_since = timestamp if timestamp >= not_before else None
                continue
            if active_since is None:
                continue
            if state != "success":
                # pending/queued/inactive/failure/error all interrupt the window.
                active_since = None
                continue

            creator = status.get("creator")
            creator_login = (
                creator.get("login") if isinstance(creator, dict) else None
            )
            creator_type = (
                creator.get("type") if isinstance(creator, dict) else None
            )
            description = status.get("description")
            observed_hours = (timestamp - active_since).total_seconds() / 3600
            if (
                observed_hours >= minimum_hours
                and _evidence_url(status) is not None
                and isinstance(creator_login, str)
                and creator_login.strip()
                and creator_type == "User"
                and isinstance(description, str)
                and description.strip()
            ):
                qualified.append((timestamp, deployment_id))
            active_since = None

    if not qualified:
        fail(
            f"no release-candidate deployment proves a continuous {minimum_hours}-hour "
            f"observation for {rc_tag} at commit {rc_commit}"
        )
    return max(qualified)[1]


def validate_stable_environment(
    environment_path: Path, expected_name: str = "stable-release"
) -> None:
    """Ensure the publishing job has independent required reviewers."""
    environment = load_json(environment_path)
    if not isinstance(environment, dict):
        fail("stable GitHub environment response must be a JSON object")
    if environment.get("name") != expected_name:
        fail(f"stable GitHub environment must be named {expected_name!r}")
    rules = environment.get("protection_rules")
    if not isinstance(rules, list):
        fail("stable-release has no protection rules")
    reviewer_rules = [
        rule
        for rule in rules
        if isinstance(rule, dict)
        and rule.get("type") == "required_reviewers"
        and rule.get("prevent_self_review") is True
        and isinstance(rule.get("reviewers"), list)
        and bool(rule["reviewers"])
    ]
    if not reviewer_rules:
        fail(
            "stable-release must have at least one required reviewer and "
            "prevent self-review"
        )


def git_commit(repository: Path, revision: str) -> str:
    return subprocess.run(
        ["git", "-C", str(repository), "rev-parse", f"{revision}^{{commit}}"],
        check=True,
        capture_output=True,
        text=True,
        encoding="utf-8",
    ).stdout.strip()


def validate_rc_commit_identity(frontend: Path, rc_tag: str) -> str:
    backend_head = git_commit(ROOT, "HEAD")
    backend_rc = git_commit(ROOT, rc_tag)
    if backend_head != backend_rc:
        fail(
            f"stable backend commit {backend_head} differs from observed "
            f"candidate {rc_tag} commit {backend_rc}"
        )

    frontend_head = git_commit(frontend, "HEAD")
    frontend_rc = git_commit(frontend, rc_tag)
    if frontend_head != frontend_rc:
        fail(
            f"stable frontend commit {frontend_head} differs from observed "
            f"candidate {rc_tag} commit {frontend_rc}"
        )
    return backend_rc


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--tag", required=True)
    parser.add_argument("--frontend-dir", type=Path, required=True)
    parser.add_argument("--github-releases-json", type=Path)
    parser.add_argument("--github-deployments-json", type=Path)
    parser.add_argument("--stable-environment-json", type=Path)
    parser.add_argument(
        "--minimum-rc-hours", type=int, default=MINIMUM_RC_HOURS
    )
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
        validate_workspace_packages(identity.version)
        frontend = args.frontend_dir.resolve()
        validate_frontend(frontend, identity.tag, identity.version)

        if identity.prerelease:
            if any(
                path is not None
                for path in (
                    args.github_releases_json,
                    args.github_deployments_json,
                    args.stable_environment_json,
                )
            ):
                fail("RC validation must not use stable promotion evidence")
        else:
            if args.github_releases_json is None:
                fail("stable validation requires --github-releases-json")
            if args.github_deployments_json is None:
                fail("stable validation requires --github-deployments-json")
            if args.stable_environment_json is None:
                fail("stable validation requires --stable-environment-json")
            rc_release = latest_published_rc(
                args.github_releases_json,
                identity.stable_tag,
                args.minimum_rc_hours,
            )
            rc_commit = validate_rc_commit_identity(frontend, rc_release.tag)
            deployment_id = validate_rc_observation(
                args.github_deployments_json,
                rc_release.tag,
                rc_commit,
                args.minimum_rc_hours,
                not_before=rc_release.published_at,
            )
            validate_stable_environment(args.stable_environment_json)
            print(
                f"Validated RC observation deployment {deployment_id} "
                f"for {rc_release.tag}"
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

    kind = "prerelease" if identity.prerelease else "stable"
    print(
        f"Release inputs are consistent: {identity.tag} ({kind}), "
        f"backend/frontend/OpenAPI {identity.version}"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
