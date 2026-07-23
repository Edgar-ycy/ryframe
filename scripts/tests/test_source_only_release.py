from __future__ import annotations

import re
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
WORKFLOWS = ROOT / ".github" / "workflows"


class SourceOnlyReleaseWorkflowTest(unittest.TestCase):
    def publishing_workflows(self) -> list[Path]:
        paths = {WORKFLOWS / "release.yml"}
        paths.update(WORKFLOWS.glob("*nightly*.yml"))
        paths.update(WORKFLOWS.glob("*nightly*.yaml"))
        return sorted(paths)

    def test_release_and_nightly_workflows_publish_no_custom_assets(self) -> None:
        forbidden = (
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
            "\n          files:",
            "\n          body:",
            "generate_release_notes:",
            "git tag -f nightly",
        )
        for path in self.publishing_workflows():
            source = path.read_text(encoding="utf-8")
            for fragment in forbidden:
                with self.subTest(path=path.name, fragment=fragment):
                    self.assertNotIn(fragment, source)

    def test_release_rerun_removes_assets_only_from_target_tag(self) -> None:
        source = (WORKFLOWS / "release.yml").read_text(encoding="utf-8")
        lookup = "releases/tags/${RELEASE_TAG}"
        deletion = "releases/assets/${asset_id}"
        publisher = "softprops/action-gh-release@v3"
        self.assertIn(lookup, source)
        self.assertIn(deletion, source)
        self.assertLess(source.index(lookup), source.index(deletion))
        self.assertLess(source.index(deletion), source.index(publisher))

    def test_release_keeps_quality_and_promotion_gates(self) -> None:
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
        self.assertIn("--minimum-rc-hours 48", source)

    def test_production_dockerfile_propagates_build_identity(self) -> None:
        source = (ROOT / "deploy" / "Dockerfile").read_text(encoding="utf-8")
        for fragment in (
            "ARG RYFRAME_BUILD_COMMIT",
            'RYFRAME_BUILD_COMMIT="${RYFRAME_BUILD_COMMIT}"',
            'org.opencontainers.image.revision="${RYFRAME_BUILD_COMMIT}"',
        ):
            with self.subTest(fragment=fragment):
                self.assertIn(fragment, source)
        self.assertNotIn("ARG RYFRAME_BUILD_COMMIT=", source)

    def test_automatic_promotion_preserves_rc_and_release_gates(self) -> None:
        promotion = (WORKFLOWS / "auto-promote.yml").read_text(encoding="utf-8")
        release = (WORKFLOWS / "release.yml").read_text(encoding="utf-8")

        for fragment in (
            "RYFRAME_AUTO_PROMOTE_STABLE == 'true'",
            "github.ref == 'refs/heads/main'",
            "RYFRAME_AUTO_PROMOTE_RC_TAG",
            'RC_MINIMUM_SECONDS: "172800"',
            'RC_MAX_HEARTBEAT_GAP_SECONDS: "7200"',
            'RELEASE_RETRY_LIMIT: "3"',
            'RELEASE_RETRY_DELAY_SECONDS: "600"',
            "RYFRAME_RELEASE_TOKEN",
            "RYFRAME_RELEASE_BOT",
            '"$admin_url/"',
            '"$admin_url/build-identity.json?run=$GITHUB_RUN_ID"',
            '"$api_url/livez"',
            '"$api_url/readyz"',
            '"$api_url/api/v1/version?run=$GITHUB_RUN_ID"',
            ".frontend_commit == $commit",
            ".source_commit == $commit",
            "--minimum-rc-hours 48",
            "Automated RC observation heartbeat was interrupted",
            "Probe the public release-candidate environment",
            "persist-credentials: false",
            "git tag -a --cleanup=verbatim",
            'git -C frontend push origin "refs/tags/${STABLE_TAG}"',
            'git push origin "refs/tags/${STABLE_TAG}"',
            'gh workflow run release.yml --ref main -f tag="$STABLE_TAG"',
            '"repos/$GITHUB_REPOSITORY/actions/runs/${run_id}/rerun"',
        ):
            with self.subTest(fragment=fragment):
                self.assertIn(fragment, promotion)

        self.assertNotIn("sleep 48", promotion)
        self.assertNotIn("git tag -f", promotion)
        self.assertNotIn("git push --force", promotion)
        self.assertNotIn(
            "RYFRAME_RELEASE_TOKEN",
            promotion.split("    steps:", maxsplit=1)[0],
        )
        self.assertLess(
            promotion.index('git -C frontend push origin "refs/tags/${STABLE_TAG}"'),
            promotion.index('git push origin "refs/tags/${STABLE_TAG}"'),
        )
        self.assertIn("github.actor == vars.RYFRAME_RELEASE_BOT", release)
        self.assertIn("needs.stable-approval.result == 'skipped'", release)
        self.assertIn("workflow_dispatch:", release)
        self.assertIn("inputs.tag || github.ref_name", release)

    def test_ci_parses_the_automatic_promotion_workflow(self) -> None:
        for workflow in ("ci.yml", "release.yml"):
            source = (WORKFLOWS / workflow).read_text(encoding="utf-8")
            with self.subTest(workflow=workflow):
                self.assertIn(".github/workflows/auto-promote.yml", source)

    def test_pnpm_action_uses_node24_runtime(self) -> None:
        references: list[tuple[str, str]] = []
        pattern = re.compile(r"pnpm/action-setup@([^\s#]+)")
        for path in sorted(WORKFLOWS.glob("*.y*ml")):
            source = path.read_text(encoding="utf-8")
            references.extend((path.name, match) for match in pattern.findall(source))

        self.assertTrue(references, "expected a pnpm/action-setup reference")
        for path, version in references:
            with self.subTest(path=path):
                self.assertEqual(version, "v6")

    def test_release_uses_both_non_empty_changelogs(self) -> None:
        source = (WORKFLOWS / "release.yml").read_text(encoding="utf-8")
        for fragment in (
            "Check out matching frontend Changelog",
            "frontend/CHANGELOG.md",
            "backend-release-notes.md",
            "frontend-release-notes.md",
            "CHANGELOG section has no update items",
            "body_path: release_body.md",
            "Verify published notes and zero custom assets",
            "'.assets | length == 0'",
            'diff --unified release_body.md "$published_body"',
        ):
            with self.subTest(fragment=fragment):
                self.assertIn(fragment, source)

    def test_release_revalidates_tag_objects_before_publishing(self) -> None:
        source = (WORKFLOWS / "release.yml").read_text(encoding="utf-8")
        for fragment in (
            "backend_tag_oid:",
            "frontend_tag_oid:",
            "Revalidate coordinated tag objects",
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
