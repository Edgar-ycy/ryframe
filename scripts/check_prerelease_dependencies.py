#!/usr/bin/env python3
"""Reject unreviewed prerelease packages, tools, and deployment images."""

from __future__ import annotations

import json
import re
import shlex
import tomllib
from collections import Counter
from dataclasses import dataclass
from datetime import date
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
ALLOWLIST_PATH = ROOT / "scripts" / "prerelease_dependency_allowlist.json"
PRERELEASE_VERSION = re.compile(
    r"^v?\d+\.\d+\.\d+-(?:alpha|beta|rc|pre|preview|nightly|canary)"
    r"(?:[.0-9A-Za-z-]*)$",
    re.IGNORECASE,
)
PRERELEASE_IN_TEXT = re.compile(
    r"(?<![0-9A-Za-z])v?\d+\.\d+\.\d+-(?:alpha|beta|rc|pre|preview|nightly|canary)"
    r"(?:[.0-9A-Za-z-]*)(?![0-9A-Za-z])",
    re.IGNORECASE,
)
YAML_IMAGE = re.compile(r"^\s*(?:-\s*)?[\"']?image[\"']?\s*:\s*(?P<value>.+?)\s*$")
YAML_ACTION = re.compile(r"^\s*(?:-\s*)?[\"']?uses[\"']?\s*:\s*(?P<value>.+?)\s*$")
DOCKER_FROM = re.compile(r"^\s*FROM\s+(?P<value>.+?)\s*$", re.IGNORECASE)
YAML_VARIABLE = re.compile(
    r"^\s*(?:-\s*)?(?P<name>[A-Z_][A-Z0-9_]*)\s*:\s*(?P<value>.+?)\s*$"
)
DOCKER_VARIABLE = re.compile(
    r"^\s*(?:ARG|ENV)\s+(?P<name>[A-Z_][A-Z0-9_]*)"
    r"(?:=|\s+)(?P<value>.+?)\s*$",
    re.IGNORECASE,
)
PRERELEASE_CHANNEL = re.compile(
    r"(?:^|[.-])(?:alpha|beta|rc|pre|preview|nightly|canary)(?:$|[.-])",
    re.IGNORECASE,
)


@dataclass(frozen=True, order=True)
class Finding:
    ecosystem: str
    name: str
    version: str
    source: str

    def key(self) -> tuple[str, str, str, str]:
        return (self.ecosystem, self.name, self.version, self.source)


def is_prerelease_version(version: str) -> bool:
    return PRERELEASE_VERSION.fullmatch(version) is not None


def is_prerelease_reference(version: str) -> bool:
    return is_prerelease_version(version) or PRERELEASE_CHANNEL.search(version) is not None


def is_unresolved_deployment_reference(reference: str) -> bool:
    stripped = reference.strip()
    return (
        "$" in stripped
        or "\\" in stripped
        or "`" in stripped
        or stripped.startswith(("*", "|", ">"))
    )


def yaml_scalar(value: str) -> str:
    """Return a simple YAML scalar without quotes or an unquoted comment."""
    value = value.strip()
    if not value:
        return value
    if value[0] in {'"', "'"}:
        quote = value[0]
        closing = value.find(quote, 1)
        return value[1:closing] if closing >= 0 else value[1:]
    return re.split(r"\s+#", value, maxsplit=1)[0].strip()


def split_container_reference(reference: str) -> tuple[str, str] | None:
    """Split an OCI reference at the tag colon, not a registry-port colon."""
    reference = reference.strip()
    if not reference:
        return None
    without_digest = reference.split("@", maxsplit=1)[0]
    last_slash = without_digest.rfind("/")
    tag_colon = without_digest.rfind(":")
    if tag_colon <= last_slash:
        return None
    name = without_digest[:tag_colon]
    version = without_digest[tag_colon + 1 :]
    if not name or not version:
        return None
    return name, version


def deployment_variables(source: str) -> dict[str, str]:
    variables: dict[str, str] = {}
    for line in source.splitlines():
        match = DOCKER_VARIABLE.match(line) or YAML_VARIABLE.match(line)
        if match:
            variables[match.group("name")] = yaml_scalar(match.group("value"))
    return variables


def resolve_deployment_reference(reference: str, variables: dict[str, str]) -> str:
    github_env = re.compile(r"\$\{\{\s*env\.([A-Z_][A-Z0-9_]*)\s*}}")
    defaulted = re.compile(r"\$\{([A-Z_][A-Z0-9_]*):-([^}]*)}")
    braced = re.compile(r"\$\{([A-Z_][A-Z0-9_]*)}")
    plain = re.compile(r"\$([A-Z_][A-Z0-9_]*)")

    resolved = reference
    for _ in range(8):
        previous = resolved
        resolved = github_env.sub(
            lambda match: variables.get(match.group(1), match.group(0)), resolved
        )
        resolved = defaulted.sub(
            lambda match: variables.get(match.group(1), match.group(2)), resolved
        )
        resolved = braced.sub(
            lambda match: variables.get(match.group(1), match.group(0)), resolved
        )
        resolved = plain.sub(
            lambda match: variables.get(match.group(1), match.group(0)), resolved
        )
        if resolved == previous:
            break
    return resolved


def docker_from_image(value: str) -> str | None:
    """Extract the image from a Docker FROM line, including platform options."""
    try:
        tokens = shlex.split(value, comments=True, posix=True)
    except ValueError:
        return None
    index = 0
    while index < len(tokens) and tokens[index].startswith("--"):
        option = tokens[index]
        index += 1
        if "=" not in option and index < len(tokens):
            index += 1
    return tokens[index] if index < len(tokens) else None


def deployment_source_findings(source: str, relative: str) -> set[Finding]:
    findings: set[Finding] = set()
    structured_versions: Counter[str] = Counter()

    def add_structured(finding: Finding) -> None:
        findings.add(finding)
        structured_versions[finding.version] += 1

    is_dockerfile = Path(relative).name.lower().startswith("dockerfile")
    variables = deployment_variables(source)
    for line in source.splitlines():
        image: str | None = None
        image_match = YAML_IMAGE.match(line)
        if image_match:
            image = yaml_scalar(image_match.group("value"))
        elif is_dockerfile and (from_match := DOCKER_FROM.match(line)):
            from_value = from_match.group("value")
            image = docker_from_image(from_value)
            if image is None:
                add_structured(
                    Finding("container", from_value.strip(), "unresolved", relative)
                )

        if image:
            image = resolve_deployment_reference(image, variables)
            if is_unresolved_deployment_reference(image):
                add_structured(Finding("container", image, "unresolved", relative))
            elif container := split_container_reference(image):
                name, version = container
                if is_prerelease_reference(version):
                    add_structured(Finding("container", name, version, relative))

        action_match = YAML_ACTION.match(line)
        if not action_match:
            continue
        action = resolve_deployment_reference(
            yaml_scalar(action_match.group("value")), variables
        )
        if action.startswith("docker://"):
            reference = action.removeprefix("docker://")
            if is_unresolved_deployment_reference(reference):
                add_structured(Finding("container", reference, "unresolved", relative))
                continue
            container = split_container_reference(reference)
            if container:
                name, version = container
                if is_prerelease_reference(version):
                    add_structured(Finding("container", name, version, relative))
            continue
        if is_unresolved_deployment_reference(action):
            add_structured(Finding("tool", action, "unresolved", relative))
            continue
        name, separator, version = action.rpartition("@")
        if separator and name and is_prerelease_reference(version):
            add_structured(Finding("tool", name, version, relative))

    # Fail closed when YAML features or variable scoping exceed the deliberately
    # small line parser above. This catches anchors, aliases, flow mappings,
    # folded scalars, and shadowed variables without silently accepting a
    # prerelease literal. Versions already attributed to an image/action keep
    # their more useful structured finding.
    uncovered_literals = Counter(PRERELEASE_IN_TEXT.findall(source))
    uncovered_literals.subtract(structured_versions)
    for version, count in uncovered_literals.items():
        if count > 0:
            findings.add(Finding("deployment-literal", relative, version, relative))
    return findings


def load_allowlist(path: Path = ALLOWLIST_PATH) -> dict[tuple[str, str, str, str], dict[str, object]]:
    document = json.loads(path.read_text(encoding="utf-8"))
    if document.get("schema") != 1 or not isinstance(document.get("entries"), list):
        raise ValueError(f"{path}: unsupported prerelease allowlist schema")

    allowed: dict[tuple[str, str, str, str], dict[str, object]] = {}
    for entry in document["entries"]:
        if not isinstance(entry, dict):
            raise ValueError(f"{path}: allowlist entries must be objects")
        required = (
            "ecosystem",
            "name",
            "version",
            "source",
            "review_after",
            "reason",
            "evidence",
        )
        if any(not isinstance(entry.get(field), str) or not entry[field].strip() for field in required):
            raise ValueError(f"{path}: every allowlist entry needs {', '.join(required)}")
        if not is_prerelease_reference(entry["version"]):
            raise ValueError(f"{path}: allowlisted version is not a prerelease: {entry['version']}")
        try:
            date.fromisoformat(entry["review_after"])
        except ValueError as error:
            raise ValueError(
                f"{path}: review_after must be an ISO date: {entry['review_after']}"
            ) from error
        if not entry["evidence"].startswith("https://"):
            raise ValueError(f"{path}: evidence must be an HTTPS primary-source URL")
        key = tuple(entry[field] for field in ("ecosystem", "name", "version", "source"))
        if key in allowed:
            raise ValueError(f"{path}: duplicate allowlist entry: {key}")
        allowed[key] = entry
    return allowed


def cargo_lock_findings(path: Path = ROOT / "Cargo.lock") -> set[Finding]:
    document = tomllib.loads(path.read_text(encoding="utf-8"))
    return {
        Finding("cargo", package["name"], package["version"], path.relative_to(ROOT).as_posix())
        for package in document.get("package", [])
        if is_prerelease_version(package.get("version", ""))
    }


def cargo_manifest_findings() -> set[Finding]:
    findings: set[Finding] = set()
    for path in sorted([ROOT / "Cargo.toml", *(ROOT / "crates").glob("*/Cargo.toml")]):
        source = path.read_text(encoding="utf-8")
        relative = path.relative_to(ROOT).as_posix()
        for version in PRERELEASE_IN_TEXT.findall(source):
            findings.add(Finding("cargo-manifest", relative, version, relative))
    return findings


def deployment_findings() -> set[Finding]:
    paths = [ROOT / "docker-compose.test.yml"]
    paths.extend(sorted((ROOT / "deploy").rglob("Dockerfile*")))
    paths.extend(sorted((ROOT / ".github" / "workflows").glob("*.yml")))
    paths.extend(sorted((ROOT / ".github" / "workflows").glob("*.yaml")))

    findings: set[Finding] = set()
    for path in paths:
        if not path.is_file():
            continue
        source = path.read_text(encoding="utf-8")
        relative = path.relative_to(ROOT).as_posix()
        findings.update(deployment_source_findings(source, relative))
    return findings


def collect_findings() -> set[Finding]:
    return cargo_lock_findings() | cargo_manifest_findings() | deployment_findings()


def evaluate(
    findings: set[Finding],
    allowed: dict[tuple[str, str, str, str], dict[str, object]],
    today: date | None = None,
) -> tuple[
    list[Finding],
    list[tuple[str, str, str, str]],
    list[tuple[str, str, str, str]],
]:
    today = today or date.today()
    finding_keys = {finding.key() for finding in findings}
    unexpected = sorted(finding for finding in findings if finding.key() not in allowed)
    stale = sorted(key for key in allowed if key not in finding_keys)
    expired = sorted(
        key
        for key, entry in allowed.items()
        if key in finding_keys and today >= date.fromisoformat(str(entry["review_after"]))
    )
    return unexpected, stale, expired


def main() -> int:
    try:
        allowed = load_allowlist()
        findings = collect_findings()
    except (OSError, ValueError, json.JSONDecodeError, tomllib.TOMLDecodeError) as error:
        print(f"Prerelease dependency check failed: {error}")
        return 1

    unexpected, stale, expired = evaluate(findings, allowed)
    if unexpected or stale or expired:
        print("Prerelease dependency check failed:")
        for finding in unexpected:
            print(
                "  - unreviewed "
                f"{finding.ecosystem} dependency {finding.name} {finding.version} "
                f"in {finding.source}"
            )
        for ecosystem, name, version, source in stale:
            print(f"  - stale allowlist entry {ecosystem} {name} {version} in {source}")
        for ecosystem, name, version, source in expired:
            review_after = allowed[(ecosystem, name, version, source)]["review_after"]
            print(
                f"  - allowlist review is due for {ecosystem} {name} {version} "
                f"in {source} (review_after {review_after})"
            )
        return 1

    print(
        "Prerelease dependency check passed "
        f"({len(findings)} explicitly reviewed exception(s))"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
