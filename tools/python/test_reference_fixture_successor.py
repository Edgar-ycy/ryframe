"""核验已发布 seed 来源消费后继关系与前后文件绑定。"""

from __future__ import annotations

from pathlib import Path
import sys
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parent))
from workspace_directory import WorkspaceDirectory
import reference_fixture_successor as successor
import devex_clone_target_binding as target_binding
from devex_clone_capture import write_json
from devex_clone_run_state import binding
from devex_clone_target_fixture import Fixture
from restore_reference_plan import plan_hash


class SuccessorTests(unittest.TestCase):
    def setUp(self):
        base = Path(__file__).resolve().parents[2] / ".local-tests/python-unit"
        temporary = WorkspaceDirectory(dir=base)
        self.addCleanup(temporary.cleanup)
        self.backend = Path(temporary.name)
        self.local = self.backend / ".local-tests"
        self.local.mkdir()

    def file(self, name: str, value: dict) -> tuple[Path, dict]:
        path = self.local / name
        write_json(path, value)
        return path, binding(path)

    def test_pending_request_has_a_dedicated_non_executable_validator(self):
        base = Path(__file__).resolve().parents[2] / ".local-tests/python-unit"
        temporary = WorkspaceDirectory(dir=base)
        self.addCleanup(temporary.cleanup)
        fixture = Fixture(Path(temporary.name), self)
        fixture.review["ready_for_execution"] = False
        fixture.request["review"] = {
            **fixture.bound(fixture.local / "review.json", fixture.review),
            "canonical_sha256": plan_hash(fixture.review),
        }
        with self.assertRaisesRegex(ValueError, "尚未就绪"):
            target_binding.request_binding(fixture.root, fixture.request)
        observed, selected = target_binding.predecessor_request_binding(
            fixture.root, fixture.request, fixture.request["review"]
        )
        self.assertFalse(observed["ready_for_execution"])
        self.assertEqual(selected["scope_id"], fixture.scope)
        with self.assertRaisesRegex(ValueError, "指定 predecessor"):
            target_binding.predecessor_request_binding(
                fixture.root,
                fixture.request,
                {**fixture.request["review"], "sha256": "f" * 64},
            )

    def test_published_source_uses_predecessor_validator_and_rechecks_relationship(self):
        historical = {"id": "historical-seed", "side": "seed"}
        historical_path, historical_binding = self.file(
            "historical-request.json", historical
        )
        predecessor_request = {
            **historical_binding,
            "canonical_sha256": plan_hash(historical),
        }
        successor_path, successor_binding = self.file(
            "successor-evidence.json", {"fixture": True}
        )
        self.assertTrue(successor_path.is_file())
        source_result = {"path": "source", "bytes": 1, "sha256": "a" * 64}
        source_registration = {
            "path": "registration",
            "bytes": 1,
            "sha256": "b" * 64,
        }
        predecessor_review = {
            "path": "review",
            "bytes": 1,
            "sha256": "c" * 64,
            "canonical_sha256": "d" * 64,
        }
        relationship = {
            "source_result": source_result,
            "source_registration": source_registration,
            "predecessor_review": predecessor_review,
            "predecessor_request": predecessor_request,
        }

        def deep_source(backend, descriptor, *, live_storage, validate_seed_target):
            self.assertEqual(backend, self.backend)
            self.assertEqual(descriptor, source_result)
            self.assertTrue(live_storage)
            validate_seed_target(backend, historical)
            return {
                "result": {"registration": source_registration},
                "seed_target": historical,
            }

        with (
            patch.object(
                successor,
                "review_successor_binding",
                side_effect=[relationship, relationship],
            ) as validate_relationship,
            patch.object(successor, "_deep_published_source", side_effect=deep_source),
            patch.object(successor, "predecessor_request_binding") as history,
        ):
            observed = successor.published_source(
                self.backend, successor_binding, live_storage=True
            )
        history.assert_called_once_with(self.backend, historical, predecessor_review)
        self.assertEqual(validate_relationship.call_count, 2)
        self.assertEqual(observed["review_successor"], relationship)
        self.assertEqual(observed["review_successor_binding"], successor_binding)

    def test_published_source_rejects_seed_or_registration_relationship_drift(self):
        historical = {"id": "historical-seed", "side": "seed"}
        _, historical_binding = self.file("historical-request.json", historical)
        _, successor_binding = self.file("successor-evidence.json", {"fixture": True})
        relationship = {
            "source_result": {"path": "source", "bytes": 1, "sha256": "a" * 64},
            "source_registration": {
                "path": "registration",
                "bytes": 1,
                "sha256": "b" * 64,
            },
            "predecessor_review": {
                "path": "review",
                "bytes": 1,
                "sha256": "c" * 64,
                "canonical_sha256": "d" * 64,
            },
            "predecessor_request": {
                **historical_binding,
                "canonical_sha256": plan_hash(historical),
            },
        }

        def wrong_seed(backend, descriptor, *, live_storage, validate_seed_target):
            changed = {**historical, "id": "different-seed"}
            validate_seed_target(backend, changed)

        with (
            patch.object(
                successor, "review_successor_binding", return_value=relationship
            ),
            patch.object(successor, "_deep_published_source", side_effect=wrong_seed),
            self.assertRaisesRegex(ValueError, "初始化历史"),
        ):
            successor.published_source(self.backend, successor_binding)

        def wrong_registration(
            backend, descriptor, *, live_storage, validate_seed_target
        ):
            validate_seed_target(backend, historical)
            return {
                "result": {"registration": {"wrong": True}},
                "seed_target": historical,
            }

        with (
            patch.object(
                successor, "review_successor_binding", return_value=relationship
            ),
            patch.object(
                successor, "_deep_published_source", side_effect=wrong_registration
            ),
            patch.object(successor, "predecessor_request_binding"),
            self.assertRaisesRegex(ValueError, "关系不一致"),
        ):
            successor.published_source(self.backend, successor_binding)

        changed = {**relationship, "source_result": {"changed": True}}

        def valid_source(backend, descriptor, *, live_storage, validate_seed_target):
            validate_seed_target(backend, historical)
            return {
                "result": {"registration": relationship["source_registration"]},
                "seed_target": historical,
            }

        with (
            patch.object(
                successor,
                "review_successor_binding",
                side_effect=[relationship, changed],
            ),
            patch.object(successor, "_deep_published_source", side_effect=valid_source),
            patch.object(successor, "predecessor_request_binding"),
            self.assertRaisesRegex(ValueError, "核对期间变化"),
        ):
            successor.published_source(self.backend, successor_binding)


if __name__ == "__main__":
    unittest.main()
