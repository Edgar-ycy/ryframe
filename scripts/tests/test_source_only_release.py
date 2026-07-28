from __future__ import annotations

import re
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
WORKFLOWS = ROOT / ".github" / "workflows"


class ReleaseWorkflowTest(unittest.TestCase):
    def publishing_workflows(self) -> list[Path]:
        paths = {WORKFLOWS / "release.yml"}
        paths.update(WORKFLOWS.glob("*nightly*.yml"))
        paths.update(WORKFLOWS.glob("*nightly*.yaml"))
        return sorted(paths)

    def test_release_only_publishes_the_deterministic_manifest_asset(self) -> None:
        source = (WORKFLOWS / "release.yml").read_text(encoding="utf-8")
        self.assertIn("files: release-manifest.json", source)
        self.assertIn("release-manifest.json", source)
        for fragment in (
            "actions/upload-artifact",
            "actions/download-artifact",
            "docker/build-push-action",
            "docker/login-action",
            "docker buildx imagetools",
            "git archive",
            "gh release upload",
            "ghcr.io/",
            "packages: write",
            "SHA256SUMS",
            ".cdx.json",
            "type=oci",
            "generate_release_notes:",
        ):
            with self.subTest(fragment=fragment):
                self.assertNotIn(fragment, source)

    def test_nightly_still_publishes_no_custom_assets(self) -> None:
        source = (WORKFLOWS / "nightly.yml").read_text(encoding="utf-8")
        for fragment in (
            "actions/upload-artifact",
            "actions/download-artifact",
            "docker/build-push-action",
            "docker/login-action",
            "docker buildx imagetools",
            "gh release upload",
            "ghcr.io/",
            "packages: write",
            "SHA256SUMS",
            ".cdx.json",
            "type=oci",
            "\n          files:",
        ):
            with self.subTest(fragment=fragment):
                self.assertNotIn(fragment, source)
        self.assertIn("'.assets | length == 0'", source)

    def test_release_rerun_removes_assets_only_from_target_tag(self) -> None:
        source = (WORKFLOWS / "release.yml").read_text(encoding="utf-8")
        lookup = "releases/tags/${RELEASE_TAG}"
        deletion = "releases/assets/${asset_id}"
        publisher = "softprops/action-gh-release@v3"
        self.assertIn(lookup, source)
        self.assertIn(deletion, source)
        self.assertLess(source.index(lookup), source.index(deletion))
        self.assertLess(source.index(deletion), source.index(publisher))

    def test_release_is_stable_only_and_keeps_quality_gates(self) -> None:
        source = (WORKFLOWS / "release.yml").read_text(encoding="utf-8")
        for job in (
            "validate-release:",
            "backend-gate:",
            "frontend-gate:",
            "stable-approval:",
            "publish-release:",
        ):
            with self.subTest(job=job):
                self.assertIn(job, source)
        self.assertIn("Existing coordinated stable tag to validate and publish", source)
        self.assertIn("prerelease: false", source)
        self.assertIsNone(re.search(r"\bRC\b", source))
        self.assertNotIn("release-candidate", source)
        self.assertNotIn("minimum-rc-hours", source)

    def test_release_uses_fixed_frontend_source_and_manifest(self) -> None:
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
            "Build deterministic release manifest",
            "files: release-manifest.json",
            "Verify published notes and release manifest",
        ):
            with self.subTest(fragment=fragment):
                self.assertIn(fragment, source)
        self.assertNotIn("Edgar-ycy/ryframe-vue3", source)

    def test_ci_runs_pinned_actionlint(self) -> None:
        source = (WORKFLOWS / "ci.yml").read_text(encoding="utf-8")
        self.assertIn("docker://rhysd/actionlint:1.7.7", source)
        self.assertNotIn("auto-promote.yml", source)

    def test_ci_uses_read_only_token_and_runs_windows_library_tests(self) -> None:
        source = (WORKFLOWS / "ci.yml").read_text(encoding="utf-8")
        self.assertIn("permissions:\n  contents: read", source)
        self.assertIn("Run library tests without external services", source)
        self.assertIn("cargo test --locked --workspace --lib", source)

    def test_release_governance_files_are_utf8_lf_without_bom(self) -> None:
        paths = (
            WORKFLOWS / "ci.yml",
            WORKFLOWS / "release.yml",
            WORKFLOWS / "nightly.yml",
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

    def test_release_builds_production_container_with_verified_identity(self) -> None:
        source = (WORKFLOWS / "release.yml").read_text(encoding="utf-8")
        for fragment in (
            "Build production container and verify build identity",
            "DOCKER_BUILDKIT=1 docker build",
            "--file deploy/Dockerfile",
            "--build-arg RYFRAME_BUILD_COMMIT=\"$RYFRAME_BUILD_COMMIT\"",
            "org.opencontainers.image.revision",
            "ryframe:release-gate",
        ):
            with self.subTest(fragment=fragment):
                self.assertIn(fragment, source)

    def test_release_uses_both_non_empty_changelogs(self) -> None:
        source = (WORKFLOWS / "release.yml").read_text(encoding="utf-8")
        for fragment in (
            "Check out matching frontend Changelog",
            "frontend/CHANGELOG.md",
            "backend-release-notes.md",
            "frontend-release-notes.md",
            "CHANGELOG section has no update items",
            "body_path: release_body.md",
            "Verify published notes and release manifest",
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

    def test_nightly_waits_for_successful_main_ci(self) -> None:
        source = (WORKFLOWS / "nightly.yml").read_text(encoding="utf-8")
        for fragment in (
            "workflow_run:",
            "workflows: [ CI ]",
            "github.event.workflow_run.conclusion == 'success'",
            "github.event.workflow_run.event == 'push'",
            "github.event.workflow_run.head_branch == 'main'",
            "ref: ${{ github.event.workflow_run.head_sha }}",
            "gh api --paginate",
            "CHANGELOG.md",
            'git tag -a -f --cleanup=verbatim -F "$RUNNER_TEMP/nightly-release-notes.md" nightly',
            "git cat-file -t refs/tags/nightly",
            "CHANGELOG section has no update items",
            "body_path: ${{ runner.temp }}/nightly-release-notes.md",
            "Verify Nightly notes and zero custom assets",
            "'.assets | length == 0'",
            "make_latest: false",
        ):
            with self.subTest(fragment=fragment):
                self.assertIn(fragment, source)


if __name__ == "__main__":
    unittest.main()
