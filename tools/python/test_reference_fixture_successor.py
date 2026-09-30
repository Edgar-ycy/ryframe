"""seed review successor 的纯关系模型必须可重算并拒绝任何资源碰撞。"""

from __future__ import annotations

import copy
from contextlib import redirect_stderr
import io
from pathlib import Path
import sys
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parent))
from workspace_directory import WorkspaceDirectory

sys.path.insert(0, str(Path(__file__).resolve().parent))
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

    def review(self, token: str, port: int) -> dict:
        root = self.local / token
        scopes = {}
        for index, side in enumerate(successor.SIDES):
            scope = f"fixture-{side}-{token}"
            scopes[side] = {
                "scope_id": scope,
                "runtime_dir": str(root / f"runtime-{side}"),
                "identity_ledger": str(root / f"identity-{side}"),
                "api_url": f"http://127.0.0.1:{port + index}",
                "worker_ready_url": f"http://127.0.0.1:{port + 10 + index}/readyz",
                "frontend_url": f"http://127.0.0.1:{port + 20 + index}",
                "objects": {
                    "endpoint": f"http://127.0.0.1:{port + 30}",
                    "region": "us-east-1",
                },
                "redis": {
                    "url": f"redis://127.0.0.1:{port + 32}/0",
                    "namespace": f"ryframe:{{{scope}}}:",
                    "ownership_key": f"ryframe:{{{scope}}}:.ryframe-owner",
                    "ownership_value": f"ryframe-owner:v1:{scope}:redis",
                },
                "databases": [
                    {
                        "key": key,
                        "database": f"{token}_{side}_{key.replace('-', '_')}",
                        "expected_server_uuid": f"00000000-0000-0000-{index:04x}-{position:012x}",
                    }
                    for position, key in enumerate(
                        ("shared-control", "shared", "dedicated-a", "dedicated-b"),
                        start=1,
                    )
                ],
            }
        return {
            "future_root": str(root),
            "scopes": scopes,
            "services": {
                "rustfs": {
                    "api": f"http://127.0.0.1:{port + 30}",
                    "console": f"http://127.0.0.1:{port + 31}",
                    "data_dir": str(root / "rustfs"),
                },
                "redis": {
                    "endpoint": f"127.0.0.1:{port + 32}",
                    "directory": str(root / "redis"),
                },
            },
        }

    def test_collision_projection_covers_all_resource_classes_and_rejects_overlap(self):
        predecessor = self.review("old", 11000)
        current = self.review("new", 12000)
        projection = successor.collision_projection(predecessor, current)
        self.assertEqual(set(projection), {"predecessor", "successor"})
        self.assertEqual(
            set(projection["successor"]),
            {"scopes", "databases", "redis", "objects", "paths", "sockets"},
        )
        self.assertEqual(len(projection["successor"]["databases"]), 12)
        self.assertEqual(len(projection["successor"]["objects"]), 15)

        mutations = {
            "database": lambda value: value["scopes"]["base"]["databases"][0].update(
                database=predecessor["scopes"]["seed"]["databases"][0]["database"],
                expected_server_uuid=predecessor["scopes"]["seed"]["databases"][0][
                    "expected_server_uuid"
                ],
            ),
            "scope": lambda value: value["scopes"]["base"].update(
                scope_id=predecessor["scopes"]["seed"]["scope_id"]
            ),
            "path": lambda value: value["scopes"]["base"].update(
                runtime_dir=predecessor["scopes"]["seed"]["runtime_dir"]
            ),
            "socket": lambda value: value["scopes"]["base"].update(
                api_url=predecessor["scopes"]["seed"]["api_url"]
            ),
        }
        for label, mutate in mutations.items():
            changed = copy.deepcopy(current)
            mutate(changed)
            with self.subTest(label=label), self.assertRaises(ValueError):
                successor.collision_projection(predecessor, changed)

    def test_ready_review_binds_its_pending_preflight_parent(self):
        pending = {
            "ready_for_execution": False,
            "semantic": "same",
            "tools": {"old": True},
        }
        pending_path, pending_binding = self.file("pending.json", pending)
        ready = {
            "ready_for_execution": True,
            "semantic": "same",
            "tools": {"current": True},
            "preflight": {"supersedes": pending_binding},
        }
        ready_path, _ = self.file("ready.json", ready)

        def validate_preflight(previous, current):
            if previous["semantic"] != current["semantic"]:
                raise ValueError("semantic drift")

        with (
            patch.object(successor, "validate_review"),
            patch.object(
                successor,
                "validate_preflight_successor",
                side_effect=validate_preflight,
            ),
        ):
            observed, descriptor = successor._ready_review(self.backend, ready_path)
        self.assertEqual(observed, ready)
        self.assertEqual(descriptor["canonical_sha256"], plan_hash(ready))

        ready["semantic"] = "changed"
        ready_path.unlink()
        write_json(ready_path, ready)
        with (
            patch.object(successor, "validate_review"),
            patch.object(
                successor,
                "validate_preflight_successor",
                side_effect=validate_preflight,
            ),
            self.assertRaisesRegex(ValueError, "语义不同"),
        ):
            successor._ready_review(self.backend, ready_path)

        ready["semantic"] = "same"
        ready_path.unlink()
        write_json(ready_path, ready)
        pending["binding_drift"] = True
        pending_path.unlink()
        write_json(pending_path, pending)
        with (
            patch.object(successor, "validate_review"),
            patch.object(successor, "validate_preflight_successor"),
            self.assertRaisesRegex(ValueError, "输入绑定已变化"),
        ):
            successor._ready_review(self.backend, ready_path)

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
        observed, selected = target_binding.pending_request_binding(
            fixture.root, fixture.request, fixture.request["review"]
        )
        self.assertFalse(observed["ready_for_execution"])
        self.assertEqual(selected["scope_id"], fixture.scope)
        with self.assertRaisesRegex(ValueError, "pending predecessor"):
            target_binding.pending_request_binding(
                fixture.root,
                fixture.request,
                {**fixture.request["review"], "sha256": "f" * 64},
            )

    def test_build_bridges_distinct_historical_and_current_reviews(self):
        _, inner_registration = self.file(
            "source-registration.json",
            {"kind": "devex-clone-seed-source-registration"},
        )
        source_path, source_result_binding = self.file(
            "0052.json",
            {
                "status": "seed_source_registered",
                "registration": inner_registration,
                "remote_writes": 0,
                "outbox_drained": True,
                "restore_qualified": False,
            },
        )
        predecessor = self.review("old", 11000)
        successor_pending = {
            **self.review("new", 12000),
            "ready_for_execution": False,
            "semantic": "same",
        }
        _, successor_pending_file = self.file(
            "successor-pending.json", successor_pending
        )
        successor_review = {
            **successor_pending,
            "ready_for_execution": True,
            "preflight": {"supersedes": successor_pending_file},
        }
        predecessor_path, predecessor_file = self.file("predecessor.json", predecessor)
        successor_path, successor_file = self.file("successor.json", successor_review)
        predecessor_descriptor = {
            **predecessor_file,
            "canonical_sha256": plan_hash(predecessor),
        }
        successor_descriptor = {
            **successor_file,
            "canonical_sha256": plan_hash(successor_review),
        }
        predecessor_request_path, predecessor_request_file = self.file(
            "predecessor-request.json",
            {"id": "historical-seed", "side": "seed", "review": predecessor_descriptor},
        )
        requests = {}
        for side in successor.SIDES:
            request = {
                "id": f"fresh-{side}",
                "side": side,
                "review": successor_descriptor,
                "storage": {"generation": "seed-service-r1"},
            }
            requests[side] = self.file(f"{side}.json", request)[0]
        with (
            patch.object(
                successor,
                "_pending_review",
                return_value=(predecessor, predecessor_descriptor),
            ),
            patch.object(successor, "validate_preflight_successor"),
            patch.object(successor, "request_binding"),
            patch.object(successor, "pending_request_binding") as pending,
        ):
            result = successor.build(
                self.backend,
                source_path,
                predecessor_path,
                predecessor_request_path,
                successor_path,
                requests,
                "successor-r1",
            )
        pending.assert_called_once()
        self.assertNotEqual(predecessor_file, successor_pending_file)
        self.assertEqual(result["predecessor_review"], predecessor_descriptor)
        self.assertEqual(result["successor_review"], successor_descriptor)
        self.assertNotEqual(
            result["predecessor_review"]["path"],
            successor_review["preflight"]["supersedes"]["path"],
        )
        self.assertEqual(result["source_result"], source_result_binding)
        self.assertEqual(result["source_registration"], inner_registration)
        self.assertEqual(
            result["predecessor_request"]["path"], predecessor_request_file["path"]
        )
        self.assertEqual(set(result["requests"]), set(successor.SIDES))
        self.assertEqual(
            successor.relationship_hash(result), result["relationship_sha256"]
        )

        changed = copy.deepcopy(result)
        changed["collision_projection"]["successor"]["scopes"][0]["identity"] = (
            "tampered"
        )
        self.assertNotEqual(
            successor.relationship_hash(changed), result["relationship_sha256"]
        )

        candidate = requests["candidate"]
        value = {
            "id": "fresh-candidate",
            "side": "candidate",
            "review": successor_descriptor,
            "storage": {"generation": "other"},
        }
        candidate.unlink()
        write_json(candidate, value)
        with (
            patch.object(
                successor,
                "_pending_review",
                return_value=(predecessor, predecessor_descriptor),
            ),
            patch.object(successor, "validate_preflight_successor"),
            patch.object(successor, "request_binding"),
            patch.object(successor, "pending_request_binding"),
            self.assertRaisesRegex(ValueError, "服务代次"),
        ):
            successor.build(
                self.backend,
                source_path,
                predecessor_path,
                predecessor_request_path,
                successor_path,
                requests,
                "successor-r1",
            )

    def test_cli_requires_explicit_write_before_reading_inputs(self):
        arguments = [
            "reference_fixture_successor.py",
            "relationship",
            "--backend-dir",
            str(self.backend),
            "--source-result",
            "missing-source.json",
            "--predecessor-review",
            "missing-predecessor.json",
            "--predecessor-request",
            "missing-request.json",
            "--successor-review",
            "missing-successor.json",
            "--seed-request",
            "missing-seed.json",
            "--base-request",
            "missing-base.json",
            "--candidate-request",
            "missing-candidate.json",
            "--id",
            "successor-r1",
            "--output",
            "missing-output.json",
        ]
        with (
            redirect_stderr(io.StringIO()),
            self.assertRaises(SystemExit) as raised,
        ):
            successor.main(arguments[1:])
        self.assertEqual(raised.exception.code, 2)

    def test_published_source_uses_pending_validator_and_rechecks_relationship(self):
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
                "_validated_relationship",
                side_effect=[relationship, relationship],
            ) as validate_relationship,
            patch.object(successor, "_deep_published_source", side_effect=deep_source),
            patch.object(successor, "pending_request_binding") as pending,
        ):
            observed = successor.published_source(
                self.backend, successor_binding, live_storage=True
            )
        pending.assert_called_once_with(self.backend, historical, predecessor_review)
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
                successor, "_validated_relationship", return_value=relationship
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
                successor, "_validated_relationship", return_value=relationship
            ),
            patch.object(
                successor, "_deep_published_source", side_effect=wrong_registration
            ),
            patch.object(successor, "pending_request_binding"),
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
                "_validated_relationship",
                side_effect=[relationship, changed],
            ),
            patch.object(successor, "_deep_published_source", side_effect=valid_source),
            patch.object(successor, "pending_request_binding"),
            self.assertRaisesRegex(ValueError, "核对期间变化"),
        ):
            successor.published_source(self.backend, successor_binding)


if __name__ == "__main__":
    unittest.main()
