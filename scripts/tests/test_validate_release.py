from __future__ import annotations

import datetime as dt
import importlib.util
import sys
import unittest
from pathlib import Path
from unittest import mock


MODULE_PATH = Path(__file__).resolve().parents[1] / "validate_release.py"
SPEC = importlib.util.spec_from_file_location("validate_release", MODULE_PATH)
assert SPEC is not None and SPEC.loader is not None
validate_release = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = validate_release
SPEC.loader.exec_module(validate_release)


class ReleaseIdentityTests(unittest.TestCase):
    def test_accepts_stable_and_canonical_rc(self) -> None:
        stable = validate_release.release_identity("v0.5.0")
        candidate = validate_release.release_identity("v0.5.0-rc.2")

        self.assertFalse(stable.prerelease)
        self.assertEqual(candidate.version, "0.5.0")
        self.assertEqual(candidate.stable_tag, "v0.5.0")
        self.assertTrue(candidate.prerelease)

    def test_rejects_ambiguous_rc_tags(self) -> None:
        for tag in (
            "0.5.0",
            "v0.5.0-rc",
            "v0.5.0-rc.0",
            "v0.5.0-rc1",
            "v0.5.0-beta.1",
        ):
            with self.subTest(tag=tag), self.assertRaises(ValueError):
                validate_release.release_identity(tag)


class RcObservationTests(unittest.TestCase):
    def test_observation_floor_cannot_be_weakened(self) -> None:
        with self.assertRaisesRegex(ValueError, "cannot be less than 48"):
            validate_release.latest_eligible_rc(
                Path("releases.json"), "v0.5.0", 47
            )

    def test_newest_rc_must_complete_window(self) -> None:
        now = dt.datetime(2026, 7, 18, 12, tzinfo=dt.timezone.utc)
        releases = [
            {
                "tag_name": "v0.5.0-rc.1",
                "prerelease": True,
                "draft": False,
                "published_at": "2026-07-15T12:00:00Z",
            },
            {
                "tag_name": "v0.5.0-rc.2",
                "prerelease": True,
                "draft": False,
                "published_at": "2026-07-17T12:00:00Z",
            },
        ]
        with mock.patch.object(validate_release, "load_json", return_value=releases):
            with self.assertRaisesRegex(ValueError, "24.0 observed hours"):
                validate_release.latest_eligible_rc(
                    Path("releases.json"), "v0.5.0", 48, now=now
                )

    def test_returns_newest_qualified_rc(self) -> None:
        now = dt.datetime(2026, 7, 18, 12, tzinfo=dt.timezone.utc)
        releases = [
            {
                "tag_name": "v0.5.0-rc.1",
                "prerelease": True,
                "draft": False,
                "published_at": "2026-07-14T12:00:00Z",
            },
            {
                "tag_name": "v0.5.0-rc.2",
                "prerelease": True,
                "draft": False,
                "published_at": "2026-07-16T12:00:00Z",
            },
        ]
        with mock.patch.object(validate_release, "load_json", return_value=releases):
            self.assertEqual(
                validate_release.latest_eligible_rc(
                    Path("releases.json"), "v0.5.0", 48, now=now
                ),
                "v0.5.0-rc.2",
            )

    def test_stable_requires_both_commits_to_match_rc(self) -> None:
        commits = iter(("backend", "backend", "frontend", "different"))
        with mock.patch.object(
            validate_release, "git_commit", side_effect=lambda *_args: next(commits)
        ):
            with self.assertRaisesRegex(ValueError, "stable frontend commit"):
                validate_release.validate_rc_commit_identity(
                    Path("frontend"), "v0.5.0-rc.2"
                )

    @staticmethod
    def deployment(statuses: list[dict[str, object]]) -> dict[str, object]:
        return {
            "id": 782,
            "created_at": "2026-07-16T11:30:00Z",
            "environment": "release-candidate",
            "ref": "v0.5.0-rc.2",
            "sha": "abc123",
            "statuses": statuses,
        }

    @staticmethod
    def status(state: str, created_at: str) -> dict[str, object]:
        return {
            "state": state,
            "created_at": created_at,
            "creator": {"login": "release-operator", "type": "User"},
            "description": "RC observation dashboard reviewed",
            "environment_url": "https://monitoring.example/rc/v0.5.0-rc.2",
        }

    def test_accepts_commit_bound_continuous_deployment_observation(self) -> None:
        statuses = [
            self.status("success", "2026-07-18T13:00:00Z"),
            self.status("in_progress", "2026-07-16T12:00:00Z"),
        ]
        evidence = [self.deployment(statuses)]
        with mock.patch.object(
            validate_release, "load_json", return_value=evidence
        ):
            self.assertEqual(
                validate_release.validate_rc_observation(
                    Path("deployments.json"),
                    "v0.5.0-rc.2",
                    "abc123",
                    48,
                    not_before=dt.datetime(
                        2026, 7, 16, 11, tzinfo=dt.timezone.utc
                    ),
                    now=dt.datetime(2026, 7, 18, 14, tzinfo=dt.timezone.utc),
                ),
                782,
            )

    def test_failure_interrupts_deployment_observation(self) -> None:
        statuses = [
            self.status("in_progress", "2026-07-16T12:00:00Z"),
            self.status("failure", "2026-07-17T12:00:00Z"),
            self.status("success", "2026-07-19T12:00:00Z"),
        ]
        with mock.patch.object(
            validate_release,
            "load_json",
            return_value=[self.deployment(statuses)],
        ):
            with self.assertRaisesRegex(ValueError, "continuous 48-hour"):
                validate_release.validate_rc_observation(
                    Path("deployments.json"),
                    "v0.5.0-rc.2",
                    "abc123",
                    48,
                    not_before=dt.datetime(
                        2026, 7, 16, 11, tzinfo=dt.timezone.utc
                    ),
                    now=dt.datetime(2026, 7, 19, 13, tzinfo=dt.timezone.utc),
                )

    def test_newer_failed_deployment_supersedes_old_success(self) -> None:
        old = self.deployment(
            [
                self.status("in_progress", "2026-07-16T12:00:00Z"),
                self.status("success", "2026-07-18T13:00:00Z"),
            ]
        )
        newer = self.deployment(
            [
                self.status("in_progress", "2026-07-18T14:00:00Z"),
                self.status("failure", "2026-07-18T15:00:00Z"),
            ]
        )
        newer["id"] = 783
        newer["created_at"] = "2026-07-18T13:30:00Z"
        with mock.patch.object(
            validate_release, "load_json", return_value=[newer, old]
        ):
            with self.assertRaisesRegex(ValueError, "continuous 48-hour"):
                validate_release.validate_rc_observation(
                    Path("deployments.json"),
                    "v0.5.0-rc.2",
                    "abc123",
                    48,
                    not_before=dt.datetime(
                        2026, 7, 16, 11, tzinfo=dt.timezone.utc
                    ),
                    now=dt.datetime(2026, 7, 18, 16, tzinfo=dt.timezone.utc),
                )

    def test_observation_requires_evidence_link_and_attestor(self) -> None:
        success = self.status("success", "2026-07-18T13:00:00Z")
        success["environment_url"] = ""
        success["creator"] = {}
        statuses = [
            self.status("in_progress", "2026-07-16T12:00:00Z"),
            success,
        ]
        with mock.patch.object(
            validate_release,
            "load_json",
            return_value=[self.deployment(statuses)],
        ):
            with self.assertRaisesRegex(ValueError, "continuous 48-hour"):
                validate_release.validate_rc_observation(
                    Path("deployments.json"),
                    "v0.5.0-rc.2",
                    "abc123",
                    48,
                    not_before=dt.datetime(
                        2026, 7, 16, 11, tzinfo=dt.timezone.utc
                    ),
                    now=dt.datetime(2026, 7, 18, 14, tzinfo=dt.timezone.utc),
                )


class StableEnvironmentTests(unittest.TestCase):
    def test_accepts_independent_required_reviewers(self) -> None:
        environment = {
            "name": "stable-release",
            "protection_rules": [
                {
                    "type": "required_reviewers",
                    "prevent_self_review": True,
                    "reviewers": [{"type": "Team", "id": 42}],
                }
            ],
        }
        with mock.patch.object(
            validate_release, "load_json", return_value=environment
        ):
            validate_release.validate_stable_environment(Path("environment.json"))

    def test_rejects_self_reviewable_or_unreviewed_environment(self) -> None:
        for environment in (
            {
                "name": "stable-release",
                "protection_rules": [
                    {
                        "type": "required_reviewers",
                        "prevent_self_review": False,
                        "reviewers": [{"id": 42}],
                    }
                ],
            },
            {
                "name": "stable-release",
                "protection_rules": [],
            },
        ):
            with self.subTest(environment=environment), mock.patch.object(
                validate_release, "load_json", return_value=environment
            ), self.assertRaises(ValueError):
                validate_release.validate_stable_environment(
                    Path("environment.json")
                )


if __name__ == "__main__":
    unittest.main()
