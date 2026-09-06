import copy
import hashlib
import json
import os
from pathlib import Path
import sys
import tempfile
import unittest
from unittest import mock

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import full_stack_provenance as provenance
from full_stack_process import write_receipt


class DeviceSourceEvidenceTests(unittest.TestCase):
    def setUp(self):
        local = Path(__file__).resolve().parents[2] / ".local-tests/python-unit"
        local.mkdir(parents=True, exist_ok=True)
        temporary = tempfile.TemporaryDirectory(dir=local)
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name).resolve()
        self.backend = self.root / "backend"
        self.frontend = self.root / "frontend"
        self.backend.mkdir()
        self.frontend.mkdir()
        definition = b'[resource]\nname = "device"\n'
        for relative in (provenance.DEVICE_FIXTURE, provenance.DEVICE_RESOURCE):
            path = self.backend / relative
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(definition)
        self.backend_head = "a" * 40
        self.frontend_head = "b" * 40
        self.sources = {
            "backend": self.source(self.backend_head, "1"),
            "frontend": self.source(self.frontend_head, "2"),
        }
        self.generated = {
            "backend": self.source(self.backend_head, "3"),
            "frontend": self.source(self.frontend_head, "4"),
        }
        self.receipt = {
            "format_version": 1,
            "fixture": "device",
            "status": "ready",
            "fixture_sha256": hashlib.sha256(definition).hexdigest(),
            "sources": self.sources,
            "paths": {"backend": str(self.backend), "frontend": str(self.frontend)},
            "generated": self.generated,
        }
        self.receipt_path = self.root / "fixture.json"
        self.write_fixture()
        (self.root / "source-pair.json").write_text(
            json.dumps(
                {
                    "backend_sha": self.backend_head,
                    "frontend_sha": self.frontend_head,
                    "run_id": 12,
                    "attempt": 3,
                }
            )
            + "\n",
            encoding="utf-8",
        )
        environment = mock.patch.dict(
            os.environ,
            {"RYFRAME_E2E_FIXTURE": "device", "RYFRAME_CODE_SHA": self.backend_head},
            clear=True,
        )
        environment.start()
        self.addCleanup(environment.stop)
        git = mock.patch.object(
            provenance,
            "git",
            side_effect=lambda root, *_: str(Path(root).resolve()).encode(),
        )
        git.start()
        self.addCleanup(git.stop)
        snapshots = {
            self.backend: self.generated["backend"],
            self.frontend: self.generated["frontend"],
        }
        capture = mock.patch.object(
            provenance,
            "snapshot",
            side_effect=lambda root: (copy.deepcopy(snapshots[Path(root).resolve()]), b""),
        )
        capture.start()
        self.addCleanup(capture.stop)

    @staticmethod
    def source(head: str, patch: str) -> dict:
        return {"head": head, "patch_sha256": patch * 64, "files": []}

    def write_fixture(self) -> None:
        self.receipt_path.write_text(
            json.dumps(self.receipt, ensure_ascii=False, indent=2) + "\n",
            encoding="utf-8",
        )

    def test_ready_fixture_binds_commits_generated_snapshots_paths_and_receipt_bytes(self):
        evidence = provenance.full_stack_source_evidence(self.backend, self.root)
        self.assertEqual(evidence["original"], self.sources)
        self.assertEqual(evidence["generated"], self.generated)
        self.assertEqual(
            evidence["roots"],
            {"backend": str(self.backend), "frontend": str(self.frontend)},
        )
        self.assertEqual(evidence["source_pair"]["backend_sha"], self.backend_head)
        self.assertEqual(evidence["source_pair"]["frontend_sha"], self.frontend_head)
        self.assertEqual(
            evidence["fixture_receipt"],
            {
                "path": str(self.receipt_path),
                "bytes": len(self.receipt_path.read_bytes()),
                "sha256": hashlib.sha256(self.receipt_path.read_bytes()).hexdigest(),
            },
        )
        self.assertEqual(
            {value["sha256"] for value in evidence["fixture_definition"].values()},
            {self.receipt["fixture_sha256"]},
        )

    def test_unready_wrong_commit_noncanonical_path_and_changed_definition_fail_closed(self):
        mutations = (
            ("status", lambda: self.receipt.update(status="preparing"), "尚未完成"),
            ("commit", lambda: self.receipt["sources"]["backend"].update(head="c" * 40), "SHA"),
            (
                "path",
                lambda: self.receipt["paths"].update(backend=str(self.backend / ".." / "backend")),
                "规范",
            ),
            (
                "definition",
                lambda: (self.backend / provenance.DEVICE_RESOURCE).write_bytes(b"changed"),
                "摘要",
            ),
        )
        original_receipt = copy.deepcopy(self.receipt)
        original_definition = (self.backend / provenance.DEVICE_RESOURCE).read_bytes()
        for name, mutate, message in mutations:
            with self.subTest(name=name):
                self.receipt = copy.deepcopy(original_receipt)
                (self.backend / provenance.DEVICE_RESOURCE).write_bytes(original_definition)
                mutate()
                self.write_fixture()
                with self.assertRaisesRegex(ValueError, message):
                    provenance.full_stack_source_evidence(self.backend, self.root)

    def test_wrong_frontend_pair_and_stale_attempt_fail_closed(self):
        pair_path = self.root / "source-pair.json"
        original = json.loads(pair_path.read_text(encoding="utf-8"))
        for name, changed, environment, message in (
            ("frontend", original | {"frontend_sha": "c" * 40}, {}, "源码组合"),
            ("attempt", original, {"GITHUB_RUN_ATTEMPT": "4"}, "attempt"),
        ):
            with self.subTest(name=name), mock.patch.dict(os.environ, environment):
                pair_path.write_text(json.dumps(changed) + "\n", encoding="utf-8")
                with self.assertRaisesRegex(ValueError, message):
                    provenance.full_stack_source_evidence(self.backend, self.root)
        pair_path.write_text(json.dumps(original) + "\n", encoding="utf-8")

    def test_receipts_with_duplicate_json_fields_fail_closed(self):
        pair_path = self.root / "source-pair.json"
        pair_path.write_text(
            '{"backend_sha":"'
            + self.backend_head
            + '","backend_sha":"'
            + self.backend_head
            + '","frontend_sha":"'
            + self.frontend_head
            + '","run_id":12,"attempt":3}\n',
            encoding="utf-8",
        )
        with self.assertRaisesRegex(ValueError, "重复字段"):
            provenance.full_stack_source_evidence(self.backend, self.root)

    def test_core_source_also_binds_selected_pair_and_current_backend_snapshot(self):
        with mock.patch.dict(os.environ, {"RYFRAME_E2E_FIXTURE": "core"}):
            evidence = provenance.full_stack_source_evidence(self.backend, self.root)
        self.assertEqual(evidence["fixture"], "core")
        self.assertEqual(evidence["original"], {"backend": self.generated["backend"]})
        self.assertEqual(evidence["source_pair"]["backend_sha"], self.backend_head)
        self.assertEqual(evidence["source_pair"]["frontend_sha"], self.frontend_head)

    def test_build_and_runtime_evidence_reject_rebound_or_changed_fixture(self):
        binaries = {}
        for name in ("ryframe", "ryframe-worker", "ryframe-reset", "ryframe-migrate"):
            path = self.root / f"{name}.exe"
            path.write_bytes(name.encode())
            binaries[name] = str(path)
        source = provenance.full_stack_source_evidence(self.backend, self.root)
        with self.assertRaisesRegex(ValueError, "恰好包含"):
            provenance.build_evidence(
                self.backend,
                self.root,
                {name: path for name, path in binaries.items() if name != "ryframe-worker"},
                source,
            )
        build = provenance.build_evidence(self.backend, self.root, binaries, source)
        runtime = self.root / "runtime.json"
        runtime.write_text('{"fixture":true}\n', encoding="utf-8")
        write_receipt(self.root / "binaries.json", binaries)
        write_receipt(self.root / provenance.BUILD_EVIDENCE, build)
        registered = provenance.register_runtime_evidence(self.backend, self.root, runtime)
        self.assertEqual(registered["source"], source)
        self.assertEqual(provenance.verify_build_evidence(self.backend, self.root), build)
        self.assertEqual(
            provenance.verify_runtime_evidence(self.backend, self.root, runtime),
            registered,
        )
        with self.receipt_path.open("a", encoding="utf-8") as output:
            output.write(" \n")
        with self.assertRaises(ValueError):
            provenance.verify_build_evidence(self.backend, self.root)


if __name__ == "__main__":
    unittest.main()
