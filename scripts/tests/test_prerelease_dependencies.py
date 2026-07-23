from __future__ import annotations

import unittest
from datetime import date
from pathlib import Path

from scripts import check_prerelease_dependencies as check


ROOT = Path(__file__).resolve().parents[2]


class PrereleaseDependencyTest(unittest.TestCase):
    def test_only_semver_prereleases_are_classified(self) -> None:
        for version in (
            "2.0.0-rc.3",
            "1.0.0-beta.8",
            "3.1.4-alpha.1",
            "v4.0.0-rc.2",
        ):
            self.assertTrue(check.is_prerelease_version(version))
        for version in ("2.0.0", "0.11.1+wasi-snapshot-preview1", "bookworm"):
            self.assertFalse(check.is_prerelease_version(version))

    def test_deployment_parser_handles_quoted_ports_platforms_and_actions(self) -> None:
        source = """
services:
  quoted:
    image: "repo/app:1.0.0-rc.1"
  private:
    image: registry.example:5000/team/app:2.0.0-beta.2 # reviewed separately
  dynamic-registry:
    image: ${REGISTRY}/team/worker:2.1.0-rc.3
env:
  IMAGE: repo/tools:2.2.0-beta.1
steps:
  - uses: 'owner/action@v3.0.0-rc.4'
  - uses: docker://registry.example:5000/tools/check:4.0.0-preview.1
  - uses: docker://${{ env.IMAGE }}
"""
        self.assertEqual(
            check.deployment_source_findings(source, ".github/workflows/example.yml"),
            {
                check.Finding(
                    "container", "repo/app", "1.0.0-rc.1", ".github/workflows/example.yml"
                ),
                check.Finding(
                    "container",
                    "registry.example:5000/team/app",
                    "2.0.0-beta.2",
                    ".github/workflows/example.yml",
                ),
                check.Finding(
                    "container",
                    "${REGISTRY}/team/worker:2.1.0-rc.3",
                    "unresolved",
                    ".github/workflows/example.yml",
                ),
                check.Finding(
                    "deployment-literal",
                    ".github/workflows/example.yml",
                    "2.1.0-rc.3",
                    ".github/workflows/example.yml",
                ),
                check.Finding(
                    "container", "repo/tools", "2.2.0-beta.1", ".github/workflows/example.yml"
                ),
                check.Finding(
                    "tool", "owner/action", "v3.0.0-rc.4", ".github/workflows/example.yml"
                ),
                check.Finding(
                    "container",
                    "registry.example:5000/tools/check",
                    "4.0.0-preview.1",
                    ".github/workflows/example.yml",
                ),
            },
        )

        dockerfile = """
ARG BASE=repo/base:4.9.0-beta.2
FROM ${BASE} AS base
FROM --platform=linux/amd64 registry.example:5000/team/app:5.0.0-rc.1 AS build
FROM registry.example:5000/team/runtime:5.0.0
"""
        self.assertEqual(
            check.deployment_source_findings(dockerfile, "deploy/Dockerfile"),
            {
                check.Finding(
                    "container", "repo/base", "4.9.0-beta.2", "deploy/Dockerfile"
                ),
                check.Finding(
                    "container",
                    "registry.example:5000/team/app",
                    "5.0.0-rc.1",
                    "deploy/Dockerfile",
                )
            },
        )

    def test_dynamic_unpinned_deployment_references_fail_closed(self) -> None:
        source = """
services:
  whole-reference:
    image: ${IMAGE}
  dynamic-tag:
    image: repo/app:${TAG}
steps:
  - uses: docker://repo/check:${TAG}
"""
        self.assertEqual(
            check.deployment_source_findings(source, "compose.yml"),
            {
                check.Finding("container", "${IMAGE}", "unresolved", "compose.yml"),
                check.Finding(
                    "container", "repo/app:${TAG}", "unresolved", "compose.yml"
                ),
                check.Finding(
                    "container", "repo/check:${TAG}", "unresolved", "compose.yml"
                ),
            },
        )

    def test_yaml_aliases_and_shadowed_variables_fail_closed(self) -> None:
        source = """
x-image: &bad repo/anchored:7.0.0-rc.1
env:
  IMAGE: repo/shadowed:8.0.0-beta.2
services:
  anchored:
    image: *bad
  shadowed:
    image: ${IMAGE}
later:
  IMAGE: repo/stable:8.0.0
"""
        self.assertEqual(
            check.deployment_source_findings(source, "compose.yml"),
            {
                check.Finding("container", "*bad", "unresolved", "compose.yml"),
                check.Finding(
                    "deployment-literal",
                    "compose.yml",
                    "7.0.0-rc.1",
                    "compose.yml",
                ),
                check.Finding(
                    "deployment-literal",
                    "compose.yml",
                    "8.0.0-beta.2",
                    "compose.yml",
                ),
            },
        )

    def test_hidden_alias_cannot_reuse_an_allowlisted_structured_version(self) -> None:
        source = """
services:
  allowed:
    image: rustfs/rustfs:1.0.0-beta.8
x-image: &bad evil/app:1.0.0-beta.8
  hidden:
    image: *bad
"""
        self.assertEqual(
            check.deployment_source_findings(source, "compose.yml"),
            {
                check.Finding("container", "*bad", "unresolved", "compose.yml"),
                check.Finding(
                    "container",
                    "rustfs/rustfs",
                    "1.0.0-beta.8",
                    "compose.yml",
                ),
                check.Finding(
                    "deployment-literal",
                    "compose.yml",
                    "1.0.0-beta.8",
                    "compose.yml",
                ),
            },
        )

    def test_escaped_and_multiline_deployment_references_fail_closed(self) -> None:
        yaml_source = r'''
services:
  hex-escaped:
    image: "evil/app:1.0.0-\x62eta.8"
  unicode-escaped:
    image: "evil/app:1.0.0-\u0062eta.8"
  folded:
    image: >-
      evil/app:1.0.0-beta.8
'''
        yaml_findings = check.deployment_source_findings(yaml_source, "compose.yml")
        self.assertIn(
            check.Finding(
                "container",
                r"evil/app:1.0.0-\x62eta.8",
                "unresolved",
                "compose.yml",
            ),
            yaml_findings,
        )
        self.assertIn(
            check.Finding(
                "container",
                r"evil/app:1.0.0-\u0062eta.8",
                "unresolved",
                "compose.yml",
            ),
            yaml_findings,
        )
        self.assertIn(
            check.Finding("container", ">-", "unresolved", "compose.yml"),
            yaml_findings,
        )

        docker_source = "FROM evil/app:1.0.0-b\\\neta.8\n"
        self.assertEqual(
            check.deployment_source_findings(docker_source, "deploy/Dockerfile"),
            {
                check.Finding(
                    "container",
                    "evil/app:1.0.0-b\\",
                    "unresolved",
                    "deploy/Dockerfile",
                )
            },
        )

    def test_repository_exceptions_are_exact_and_current(self) -> None:
        unexpected, stale, expired = check.evaluate(
            check.collect_findings(), check.load_allowlist()
        )
        self.assertEqual(unexpected, [])
        self.assertEqual(stale, [])
        self.assertEqual(expired, [])

    def test_allowlist_review_date_expires_an_existing_exception(self) -> None:
        findings = {
            check.Finding("cargo", "example", "1.0.0-rc.1", "Cargo.lock")
        }
        key = next(iter(findings)).key()
        allowed = {
            key: {
                "review_after": "2026-08-23",
            }
        }

        self.assertEqual(check.evaluate(findings, allowed, date(2026, 8, 22))[2], [])
        self.assertEqual(check.evaluate(findings, allowed, date(2026, 8, 23))[2], [key])

    def test_ci_and_release_run_the_dependency_gate(self) -> None:
        command = "python scripts/check_prerelease_dependencies.py"
        for relative in (".github/workflows/ci.yml", ".github/workflows/release.yml"):
            source = (ROOT / relative).read_text(encoding="utf-8")
            self.assertIn(command, source, relative)


if __name__ == "__main__":
    unittest.main()
