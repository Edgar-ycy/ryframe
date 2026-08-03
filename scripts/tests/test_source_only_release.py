from __future__ import annotations

import re
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
WORKFLOWS = ROOT / ".github" / "workflows"


class ReleaseWorkflowTest(unittest.TestCase):
    def test_release_publishes_only_github_source_archives(self) -> None:
        source = (WORKFLOWS / "release.yml").read_text(encoding="utf-8")
        self.assertIn("Create source-only GitHub release", source)
        self.assertIn("Verify published notes and zero custom assets", source)
        self.assertIn("(.assets | length == 0)", source)
        self.assertIn(".zipball_url", source)
        self.assertIn(".tarball_url", source)
        for fragment in (
            "publish-oci:",
            "docker/build-push-action",
            "docker/login-action",
            "docker/setup-qemu-action",
            "platforms: linux/amd64,linux/arm64",
            "packages: write",
            "id-token: write",
            "anchore/sbom-action@e22c389904149dbc22b58101806040fa8d37a610",
            "format: spdx-json",
            "sigstore/cosign-installer@d7543c93d881b35a8faa02e8e3605f69b7a1ce62",
            "cosign attest --yes --type spdxjson",
            "cosign verify-attestation --type spdxjson",
            "--oci-image-repository",
            "--oci-digest",
            "release-manifest.json",
            "stable-approval:",
            "environment:\n      name: stable-release",
            "contract-snapshots:",
            "backend-gate:",
            "frontend-gate:",
        ):
            with self.subTest(fragment=fragment):
                self.assertNotIn(fragment, source)
        for fragment in ("git archive", "gh release upload", "generate_release_notes:"):
            with self.subTest(fragment=fragment):
                self.assertNotIn(fragment, source)

    def test_release_rerun_removes_assets_only_from_target_tag(self) -> None:
        source = (WORKFLOWS / "release.yml").read_text(encoding="utf-8")
        lookup = "releases/tags/${RELEASE_TAG}"
        deletion = "releases/assets/${asset_id}"
        publisher = "softprops/action-gh-release@b4309332981a82ec1c5618f44dd2e27cc8bfbfda"
        self.assertIn(lookup, source)
        self.assertIn(deletion, source)
        self.assertLess(source.index(lookup), source.index(deletion))
        self.assertLess(source.index(deletion), source.index(publisher))

    def test_release_is_stable_only_and_keeps_only_required_jobs(self) -> None:
        source = (WORKFLOWS / "release.yml").read_text(encoding="utf-8")
        for job in ("validate-release:", "publish-release:"):
            with self.subTest(job=job):
                self.assertIn(job, source)
        jobs_source = source.split("\njobs:\n", maxsplit=1)[1]
        self.assertEqual(
            re.findall(r"^  ([a-z][a-z0-9-]+):$", jobs_source, re.MULTILINE),
            ["validate-release", "publish-release"],
        )
        self.assertIn("Existing coordinated stable tag to validate and publish", source)
        self.assertIn("prerelease: false", source)
        self.assertIsNone(re.search(r"\bRC\b", source))
        self.assertNotIn("release-candidate", source)
        self.assertNotIn("minimum-rc-hours", source)

    def test_contract_snapshots_are_generated_locally_without_ci_recompilation(self) -> None:
        ci_source = (WORKFLOWS / "ci.yml").read_text(encoding="utf-8")
        release_source = (WORKFLOWS / "release.yml").read_text(encoding="utf-8")

        for fragment in (
            "Check OpenAPI contract snapshot",
            "Check generated MySQL schema snapshot",
            "cargo run --locked -p ryframe-api --bin export_openapi",
            "cargo run --locked -p ryframe-db-migration --bin export_mysql_snapshot",
        ):
            with self.subTest(fragment=fragment):
                self.assertNotIn(fragment, ci_source)
        self.assertNotIn("contract-snapshots:", release_source)
        self.assertNotIn("Verify generated snapshots", release_source)
        self.assertNotIn("Upload OpenAPI contract", ci_source)

    def test_release_uses_fixed_frontend_source_without_custom_artifacts(self) -> None:
        source = (WORKFLOWS / "release.yml").read_text(encoding="utf-8")
        for fragment in (
            "FRONTEND_REPOSITORY:",
            "vars.RYFRAME_FRONTEND_REPOSITORY",
            "--backend-repository",
            "--backend-commit",
            "--frontend-repository",
            "--frontend-commit",
            "--manifest-path",
            "ref: ${{ needs.validate-release.outputs.backend_commit }}",
            "ref: ${{ needs.validate-release.outputs.frontend_commit }}",
            "Verify published notes and zero custom assets",
            'select(.event == "push" and .status == "completed" and .conclusion == "success")',
        ):
            with self.subTest(fragment=fragment):
                self.assertIn(fragment, source)
        self.assertNotIn("Edgar-ycy/ryframe-vue3", source)

    def test_ci_runs_pinned_actionlint(self) -> None:
        source = (WORKFLOWS / "ci.yml").read_text(encoding="utf-8")
        self.assertIn(
            "docker://rhysd/actionlint@sha256:887a259a5a534f3c4f36cb02dca341673c6089431057242cdc931e9f133147e9",
            source,
        )
        self.assertNotIn("auto-promote.yml", source)

    def test_ci_uses_linux_static_gates_without_rust_tests(self) -> None:
        source = (WORKFLOWS / "ci.yml").read_text(encoding="utf-8")
        workflow_sources = "\n".join(
            path.read_text(encoding="utf-8")
            for path in sorted(WORKFLOWS.glob("*.yml"))
        )
        self.assertIn("permissions:\n  contents: read", source)
        self.assertIn("Check & Lint (Linux)", source)
        self.assertEqual(
            source.count(
                "cargo clippy --locked --workspace --all-targets -- -D warnings"
            ),
            1,
        )
        for fragment in (
            "cargo run ",
            "cargo check ",
            "cargo build ",
            "cargo test ",
            "cargo nextest",
            "cargo llvm-cov",
            "test-windows:",
            "coverage:",
        ):
            with self.subTest(fragment=fragment):
                self.assertNotIn(fragment, workflow_sources)

    def test_weekly_schedule_runs_only_dependency_security_job(self) -> None:
        source = (WORKFLOWS / "ci.yml").read_text(encoding="utf-8")
        check_job = source.split("\n  check:\n", maxsplit=1)[1].split(
            "\n  security-audit:\n", maxsplit=1
        )[0]
        security_job_header = source.split(
            "\n  security-audit:\n", maxsplit=1
        )[1].split("\n    steps:\n", maxsplit=1)[0]

        self.assertIn("  schedule:\n", source)
        self.assertIn(
            "    if: ${{ github.event_name != 'schedule' }}", check_job
        )
        self.assertNotIn("\n    if:", security_job_header)
        self.assertNotIn("RUSTDOCFLAGS", source)

    def test_ci_runs_only_the_offline_smoke_contract_test(self) -> None:
        source = (WORKFLOWS / "ci.yml").read_text(encoding="utf-8")

        self.assertEqual(source.count("node deploy/tests/smoke-test.test.js"), 1)
        self.assertIsNone(
            re.search(
                r"(?m)^\s*node\s+deploy/tests/smoke-test\.js(?:\s|$)",
                source,
            )
        )

    def test_release_governance_files_are_utf8_lf_without_bom(self) -> None:
        paths = (
            WORKFLOWS / "ci.yml",
            WORKFLOWS / "release.yml",
            ROOT / "docs" / "release-guide.md",
        )
        for path in paths:
            with self.subTest(path=path.name):
                data = path.read_bytes()
                self.assertFalse(data.startswith(b"\xef\xbb\xbf"))
                self.assertNotIn(b"\r", data)
                self.assertTrue(data.endswith(b"\n"))

    def test_production_dockerfile_propagates_build_identity(self) -> None:
        source = (ROOT / "deploy" / "Dockerfile").read_text(encoding="utf-8")
        for fragment in (
            "ARG RYFRAME_BUILD_COMMIT",
            'test -n "${RYFRAME_BUILD_COMMIT}"',
            "grep -Eq '^[0-9a-f]{40}$'",
            'RYFRAME_BUILD_COMMIT="${RYFRAME_BUILD_COMMIT}"',
            'org.opencontainers.image.revision="${RYFRAME_BUILD_COMMIT}"',
        ):
            with self.subTest(fragment=fragment):
                self.assertIn(fragment, source)
        self.assertNotIn("ARG RYFRAME_BUILD_COMMIT=", source)

    def test_production_compose_starts_worker_before_api(self) -> None:
        source = (ROOT / "deploy" / "compose.prod.yml").read_text(encoding="utf-8")
        api_section = source.split("  api:\n", maxsplit=1)[1].split(
            "\n  worker:\n    <<:", maxsplit=1
        )[0]
        self.assertIn("migrate:\n        condition: service_completed_successfully", api_section)
        self.assertIn("worker:\n        condition: service_healthy", api_section)

    def test_release_uses_both_non_empty_changelogs(self) -> None:
        source = (WORKFLOWS / "release.yml").read_text(encoding="utf-8")
        for fragment in (
            "Check out fixed frontend Changelog",
            "frontend/CHANGELOG.md",
            "backend-release-notes.md",
            "frontend-release-notes.md",
            "CHANGELOG section has no update items",
            "body_path: release_body.md",
            "Verify published notes and zero custom assets",
            'diff --unified release_body.md "$published_body"',
        ):
            with self.subTest(fragment=fragment):
                self.assertIn(fragment, source)

    def test_release_revalidates_tag_objects_before_publishing(self) -> None:
        source = (WORKFLOWS / "release.yml").read_text(encoding="utf-8")
        for fragment in (
            "backend_tag_oid:",
            "frontend_tag_oid:",
            "Revalidate tag objects",
            "Confirm tag refs immediately before publishing",
            'git cat-file -t "refs/tags/${RELEASE_TAG}"',
            'git -C frontend cat-file -t "refs/tags/${RELEASE_TAG}"',
            'git ls-remote --refs origin "refs/tags/${RELEASE_TAG}"',
            'git -C frontend ls-remote --refs origin "refs/tags/${RELEASE_TAG}"',
        ):
            with self.subTest(fragment=fragment):
                self.assertIn(fragment, source)

    def test_only_stable_release_workflow_can_publish(self) -> None:
        workflow_paths = sorted(WORKFLOWS.glob("*.y*ml"))
        self.assertFalse(any("nightly" in path.name.lower() for path in workflow_paths))
        for path in workflow_paths:
            source = path.read_text(encoding="utf-8")
            if path.name == "release.yml":
                continue
            for fragment in (
                "softprops/action-gh-release",
                "prerelease: true",
                "refs/tags/nightly",
                "tag_name: nightly",
            ):
                with self.subTest(path=path.name, fragment=fragment):
                    self.assertNotIn(fragment, source)


if __name__ == "__main__":
    unittest.main()
