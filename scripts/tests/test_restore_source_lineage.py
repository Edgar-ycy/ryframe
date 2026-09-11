"""C52 原始数据集到当前 seed 完整像的严格派生血缘。"""

import copy
import hashlib
from pathlib import Path
import sys
from types import SimpleNamespace
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
sys.path.insert(0, str(Path(__file__).resolve().parent))

from devex_clone_capture import write_json
from devex_clone_run_state import binding
from restore_reference_plan import plan_hash
import restore_source_lineage as lineage
from workspace_directory import WorkspaceDirectory


class SourceDatasetLineageTests(unittest.TestCase):
    def setUp(self):
        repository = Path(__file__).resolve().parents[2]
        temporary = WorkspaceDirectory(
            dir=repository / ".local-tests/python-unit", prefix="source-lineage-"
        )
        self.addCleanup(temporary.cleanup)
        self.backend = Path(temporary.name)
        self.run = self.backend / ".local-tests/copy-run"
        self.run.mkdir(parents=True)
        self.origin = "restore-source-unit"
        self.current = "perf-seed-unit"
        self._build_evidence()
        self.addCleanup(patch.stopall)
        patch.object(lineage, "validate_plan").start()
        patch(
            "devex_clone_post_registration.resolve_post_registration",
            return_value=self.post,
        ).start()
        patch(
            "devex_clone_run.require_target_copy",
            return_value=SimpleNamespace(plan=self.copy_plan),
        ).start()
        patch("devex_clone_post._validate_business_lineage").start()
        patch.object(
            lineage,
            "load_state",
            return_value={
                "attempts": [
                    {
                        "number": 29,
                        "stage": "post-copy",
                        "mode": "verify",
                        "status": "passed",
                        "result": self.c29_descriptor,
                    }
                ]
            },
        ).start()

    def _write(self, relative: str, value: dict) -> tuple[Path, dict]:
        path = self.backend / ".local-tests" / relative
        path.parent.mkdir(parents=True, exist_ok=True)
        write_json(path, value)
        return path, binding(path)

    @staticmethod
    def _sha(value: str) -> str:
        return hashlib.sha256(value.encode()).hexdigest()

    def _tenants(self) -> tuple[list[dict], list[tuple[str, dict]]]:
        tenant_ids = ["system", *[f"{self.origin}-{index:02d}" for index in range(1, 11)]]
        tenants = [
            {
                "tenant_id": tenant_id,
                "username": f"user-{index}",
                "password_env": f"PASSWORD_{index}",
                "records": 9_091 if index < 10 else 9_090,
                "posts": [
                    {"id": f"{index}-{post}", "code": f"P{post}", "name": f"帖子 {post}"}
                    for post in range(3)
                ],
                "files": [],
            }
            for index, tenant_id in enumerate(tenant_ids)
        ]
        files = []
        for index in range(256):
            tenant = tenants[index % len(tenants)]
            path = f"{tenant['tenant_id']}/object-{index:03d}.bin"
            item = {
                "file_id": str(index),
                "file_name": f"object-{index:03d}.bin",
                "file_path": path,
                "file_url": f"/{path}",
                "bytes": 4 * 1024**2,
                "sha256": self._sha(path),
            }
            tenant["files"].append(item)
            files.append((tenant["tenant_id"], item))
        return tenants, files

    def _build_evidence(self) -> None:
        tenants, files = self._tenants()
        targets = ["shared"] * 4 + ["shared-control"] * 4 + ["dedicated-a", "dedicated-b"]
        self.reference_plan = {
            "format_version": 1,
            "source": {"scope_id": self.origin},
            "dataset": {"tenant_targets": targets},
        }
        self.dataset = {
            "format_version": 1,
            "plan_sha256": plan_hash(self.reference_plan),
            "source_scope_id": self.origin,
            "started_at": "2026-09-01T00:00:00Z",
            "records": 100_000,
            "object_bytes": 1024**3,
            "tenants": tenants,
            "request_interval_ms": 1000,
            "post_concurrency": 4,
            "completed_at": "2026-09-03T00:00:00Z",
        }
        _, plan_descriptor = self._write("origin/plan.json", self.reference_plan)
        self.dataset_path, dataset_descriptor = self._write("origin/result.json", self.dataset)
        objects = []
        buckets = ("uploads", "avatar", "exports", "imports", "config-packages")
        for index, (_, item) in enumerate(files):
            objects.append(
                {
                    "bucket": buckets[index % len(buckets)],
                    "source_key": f"{self.origin}/{item['file_path']}",
                    "target_key": f"{self.current}/{item['file_path']}",
                    "artifact": {"bytes": item["bytes"], "sha256": item["sha256"]},
                    "metadata": {"tenant-id": item["file_path"].split("/", 1)[0]},
                }
            )
        self.probe = {
            "bucket": "uploads",
            "source_key": f"{self.origin}/system/probe.txt",
            "target_key": f"{self.current}/system/probe.txt",
            "artifact": {"bytes": 7, "sha256": self._sha("probe")},
            "metadata": {},
        }
        objects.append(self.probe)
        self.copy_plan = {
            "plan_sha256": "1" * 64,
            "copy_stage": "source_to_seed",
            "source_scope": self.origin,
            "target_scope": self.current,
            "objects": objects,
            "databases": [
                {"key": key, "tables": {"sys_post": 100_044 if key == "shared-control" else 0}}
                for key in ("shared-control", "shared", "dedicated-a", "dedicated-b")
            ],
        }
        copy_directory = self.run / "copy"
        copy_directory.mkdir()
        write_json(copy_directory / "plan.json", self.copy_plan)
        _, stage_descriptor = self._write("copy/stage.json", {"stage": "copy"})
        _, ledger_descriptor = self._write("copy/ledger-head.json", {"sequence": 52})
        copy_result = {
            "status": "data_steps_verified",
            "plan_sha256": self.copy_plan["plan_sha256"],
            "fresh_target_sha256": "2" * 64,
            "generation_sha256": "3" * 64,
            "source_export_sha256": "4" * 64,
        }
        write_json(copy_directory / "result.json", copy_result)
        copy_descriptor = binding(copy_directory / "result.json")
        _, post_descriptor = self._write("post-copy/amendments/0001.json", {"post": "registered"})
        post_request = {
            "reference_plan": plan_descriptor,
            "dataset": dataset_descriptor,
            "copy_result": copy_descriptor,
            "copy_stage_receipt": stage_descriptor,
            "ledger_head": ledger_descriptor,
        }
        expected_copy = {
            "fresh_target_sha256": copy_result["fresh_target_sha256"],
            "generation_sha256": copy_result["generation_sha256"],
            "ledger_head_sha256": ledger_descriptor["sha256"],
            "plan_sha256": self.copy_plan["plan_sha256"],
            "source_export_sha256": copy_result["source_export_sha256"],
            "stage_receipt_sha256": stage_descriptor["sha256"],
        }
        target = {
            "format_version": 1,
            "kind": "devex-copy-business-target",
            "source_plan_sha256": self.dataset["plan_sha256"],
            "source_scope_id": self.origin,
            "target": {
                "scope_id": self.current,
                "api_url": "http://127.0.0.1:18210",
                "frontend_url": "http://127.0.0.1:4190",
            },
            "copy": expected_copy,
        }
        evidence_dir = self.run / "post-copy/attempt-0029"
        target_path, target_descriptor = self._write(
            "copy-run/post-copy/attempt-0029/business-target.json", target
        )
        self.target_path = target_path
        self.assertEqual(target_path, evidence_dir / "business-target.json")
        evidence = {
            "format_version": 1,
            "status": "copy_existing_data_verified",
            "side": "copy_target",
            "scope_id": self.current,
            "source_scope_id": self.origin,
            "plan_sha256": self.dataset["plan_sha256"],
            "target_binding_sha256": plan_hash(target),
            "copy": expected_copy,
            "clone_verified": False,
            "restore_success": False,
            "tenants": 11,
            "posts": 33,
            "files": 256,
            "actions": {"business": "read_only", "objects": "read_only", "session": "login_logout"},
            "input_files": {
                "plan_sha256": plan_descriptor["sha256"],
                "dataset_sha256": dataset_descriptor["sha256"],
                "target_binding_sha256": target_descriptor["sha256"],
            },
        }
        _, evidence_descriptor = self._write(
            "copy-run/post-copy/attempt-0029/business-result.json", evidence
        )
        c29 = {
            "status": "post_copy_existing_data_verified",
            "evidence": evidence_descriptor,
            "objects": {
                "business_objects": 256,
                "additional_objects": 1,
                "additional_objects_downloaded": True,
                "all_scoped_keys_verified": True,
            },
            "registration": post_descriptor,
            "restore_qualified": False,
            "worker_must_remain_stopped": True,
        }
        _, self.c29_descriptor = self._write("copy-run/results/0029.json", c29)
        registration = self._registration(post_descriptor, self.c29_descriptor, plan_descriptor)
        _, registration_descriptor = self._write(
            "copy-run/seed-runtime/attempt-0052/source-registration.json", registration
        )
        c52 = {
            "status": "seed_source_registered",
            "registration": registration_descriptor,
            "source_request": plan_descriptor,
            "source_storage": plan_descriptor,
            "generation_verified": plan_descriptor,
            "remote_writes": 0,
            "outbox_drained": True,
            "restore_qualified": False,
        }
        _, self.c52_descriptor = self._write("copy-run/results/0052.json", c52)
        self.post = SimpleNamespace(request=post_request, descriptor=post_descriptor)
        self.source = {
            "review_successor": {"source_result": self.c52_descriptor},
            "result": c52,
            "registration": registration,
            "directory": self.run,
            "manifest": {"copy_directory": str(copy_directory)},
            "request": {
                "source": {
                    "scope_id": self.current,
                    "api_url": "http://127.0.0.1:18210",
                    "frontend_url": "http://127.0.0.1:4190",
                }
            },
        }
        self.image = self._image(targets, objects)
        self.image_path, self.image_descriptor = self._write("generation/before/image.json", self.image)

    def _registration(self, post: dict, verify: dict, filler: dict) -> dict:
        from devex_clone_seed_source import REGISTRATION_FIELDS

        value = {field: filler for field in REGISTRATION_FIELDS}
        value.update(
            {
                "format_version": 1,
                "kind": "devex-clone-seed-source-registration",
                "post_copy": post,
                "post_verify": verify,
                "source_request": filler,
                "remote_writes": 0,
                "outbox_drained": True,
                "restore_qualified": False,
            }
        )
        return value

    def _image(self, targets: list[str], objects: list[dict]) -> dict:
        placements = {key: [] for key in ("shared-control", "shared", "dedicated-a", "dedicated-b")}
        placements["shared-control"].append({"tenant_id": "system"})
        for index, target in enumerate(targets, 1):
            placements[target].append({"tenant_id": f"{self.origin}-{index:02d}"})
        databases = {}
        for entry in self.copy_plan["databases"]:
            key = entry["key"]
            databases[key] = {
                "target": {
                    "database": {
                        "key": key,
                        "placements": placements[key],
                        "tables": [
                            {"table": table, "rows": rows, "sha256": self._sha(f"{key}:{table}")}
                            for table, rows in entry["tables"].items()
                        ],
                    }
                }
            }
        buckets = {key: {} for key in ("uploads", "avatar", "exports", "imports", "config-packages")}
        for item in objects:
            buckets[item["bucket"]][item["target_key"]] = {
                **item["artifact"],
                "metadata": item["metadata"],
                "identity": {"ETag": item["artifact"]["sha256"]},
            }
        return {
            "format_version": 1,
            "kind": "seed-source-generation-image",
            "source_registration": self.c52_descriptor,
            "image": {"databases": databases, "objects": buckets},
        }

    def _rewrite_image(self, change) -> None:
        change(self.image)
        self.image_path.unlink()
        write_json(self.image_path, self.image)
        self.image_descriptor = binding(self.image_path)

    def test_derives_exact_scale_and_preserves_origin_tenant_ids(self):
        result = lineage.derive_dataset_lineage(
            self.backend, self.source, self.image_descriptor, self.image
        )
        self.assertEqual(result["scale"]["records"], 100_000)
        self.assertEqual(result["scale"]["current_post_rows"], 100_044)
        self.assertEqual(result["scale"]["post_samples"], 33)
        self.assertEqual(result["scale"]["business_objects"], 256)
        self.assertEqual(result["scale"]["verified_objects"], 257)
        self.assertEqual(result["scale"]["object_bytes"], 1024**3)
        self.assertEqual(result["scopes"]["origin_tenant_scope_id"], self.origin)
        self.assertEqual(result["scopes"]["current_object_scope_id"], self.current)
        self.assertEqual(
            result["verification"],
            {
                "scope_id": self.current,
                "api_url": "http://127.0.0.1:18210",
                "frontend_url": "http://127.0.0.1:4190",
                "request_interval_ms": 1000,
            },
        )
        self.assertEqual(result["tenants"][1]["tenant_id"], f"{self.origin}-01")
        self.assertEqual(result["tenants"][1]["username"], "user-1")
        self.assertEqual(result["tenants"][1]["password_env"], "PASSWORD_1")
        self.assertEqual(result["tenants"][1]["records"], 9_091)
        self.assertEqual(len(result["tenants"][1]["posts"]), 3)
        self.assertTrue(result["tenants"][1]["files"])
        self.assertEqual(
            result["objects"]["probe"],
            {
                "bucket": self.probe["bucket"],
                "source_key": self.probe["source_key"],
                "target_key": self.probe["target_key"],
                "bytes": self.probe["artifact"]["bytes"],
                "sha256": self.probe["artifact"]["sha256"],
                "metadata": self.probe["metadata"],
            },
        )
        _, descriptor = self._write("generation/dataset-lineage.json", result)
        self.assertEqual(
            lineage.verify_dataset_lineage(
                self.backend, self.source, descriptor, self.image_descriptor, self.image
            ),
            result,
        )

    def test_sample_count_is_not_mistaken_for_total_records(self):
        changed = copy.deepcopy(self.dataset)
        changed["records"] = 33
        changed["tenants"][0]["records"] -= 99_967
        self.dataset_path.unlink()
        write_json(self.dataset_path, changed)
        post = SimpleNamespace(
            request={
                "reference_plan": self.post.request["reference_plan"],
                "dataset": binding(self.dataset_path),
            }
        )
        with self.assertRaisesRegex(ValueError, "正式恢复规模"):
            lineage._dataset_facts(self.backend, post)

    def test_wrong_tenant_placement_or_object_digest_fails_closed(self):
        self._rewrite_image(
            lambda value: value["image"]["databases"]["shared"]["target"]["database"][
                "placements"
            ].append({"tenant_id": "unknown"})
        )
        with self.assertRaisesRegex(ValueError, "placement"):
            lineage.derive_dataset_lineage(
                self.backend, self.source, self.image_descriptor, self.image
            )
        self.image = self._image(self.reference_plan["dataset"]["tenant_targets"], self.copy_plan["objects"])
        key = self.copy_plan["objects"][0]["target_key"]
        bucket = self.copy_plan["objects"][0]["bucket"]
        self._rewrite_image(lambda value: value["image"]["objects"][bucket][key].update(sha256="f" * 64))
        with self.assertRaisesRegex(ValueError, "复制计划对象"):
            lineage.derive_dataset_lineage(
                self.backend, self.source, self.image_descriptor, self.image
            )

    def test_unknown_object_or_second_probe_fails_closed(self):
        self._rewrite_image(
            lambda value: value["image"]["objects"]["uploads"].update(
                {
                    f"{self.current}/unknown.bin": {
                        "bytes": 1,
                        "sha256": self._sha("unknown"),
                        "metadata": {},
                        "identity": {"ETag": "unknown"},
                    }
                }
            )
        )
        with self.assertRaisesRegex(ValueError, "未知对象"):
            lineage.derive_dataset_lineage(
                self.backend, self.source, self.image_descriptor, self.image
            )

        self.image = self._image(
            self.reference_plan["dataset"]["tenant_targets"], self.copy_plan["objects"]
        )
        second = {
            "bucket": "exports",
            "source_key": f"{self.origin}/system/second-probe.txt",
            "target_key": f"{self.current}/system/second-probe.txt",
            "artifact": {"bytes": 1, "sha256": self._sha("second-probe")},
            "metadata": {},
        }
        self.copy_plan["objects"].append(second)
        self.image["image"]["objects"]["exports"][second["target_key"]] = {
            **second["artifact"], "metadata": {}, "identity": {"ETag": "second"}
        }
        self._rewrite_image(lambda value: None)
        with self.assertRaisesRegex(ValueError, "唯一探针"):
            lineage.derive_dataset_lineage(
                self.backend, self.source, self.image_descriptor, self.image
            )

    def test_unknown_lineage_field_is_rejected(self):
        result = lineage.derive_dataset_lineage(
            self.backend, self.source, self.image_descriptor, self.image
        )
        result["unknown"] = True
        _, descriptor = self._write("generation/dataset-lineage.json", result)
        with self.assertRaisesRegex(ValueError, "字段"):
            lineage.verify_dataset_lineage(
                self.backend, self.source, descriptor, self.image_descriptor, self.image
            )

    def test_verification_endpoints_and_pacing_come_from_bound_evidence(self):
        changed = copy.deepcopy(self.dataset)
        changed["request_interval_ms"] = -1
        self.dataset_path.unlink()
        write_json(self.dataset_path, changed)
        post = SimpleNamespace(
            request={
                "reference_plan": self.post.request["reference_plan"],
                "dataset": binding(self.dataset_path),
            }
        )
        with self.assertRaisesRegex(ValueError, "正式恢复规模"):
            lineage._dataset_facts(self.backend, post)

        self.dataset_path.unlink()
        write_json(self.dataset_path, self.dataset)
        target = lineage.read_json(self.target_path)
        target["target"]["api_url"] = "http://127.0.0.1:19999"
        self.target_path.unlink()
        write_json(self.target_path, target)
        with self.assertRaisesRegex(ValueError, "C29"):
            lineage.derive_dataset_lineage(
                self.backend, self.source, self.image_descriptor, self.image
            )


if __name__ == "__main__":
    unittest.main()
