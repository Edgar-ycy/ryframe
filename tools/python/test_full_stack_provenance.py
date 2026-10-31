"""真实隔离 Git 来源、原生全栈事实及冻结 build/runtime 消费回归；不启动服务。"""
from __future__ import annotations

import copy
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parent))
import full_stack_provenance as provenance
import native_s3_client as native
from artifact_digests import file_digest
from full_stack_process import write_receipt
from reference_fixture_source_pair import write_pair
from source_inventory import snapshot
from workspace_directory import WorkspaceDirectory

MODEL = Path("tools/python/fixtures/order-business/src/resources/mod.rs")


class BusinessSourceEvidenceTests(unittest.TestCase):
    def setUp(self):
        self.enterContext(patch.dict(os.environ, {"GITHUB_RUN_ID": "12", "GITHUB_RUN_ATTEMPT": "3"}))
        local = Path(__file__).resolve().parents[2] / ".local-tests/python-unit"
        temporary = WorkspaceDirectory(local, "fs-source-")
        self.addCleanup(temporary.cleanup)
        self.root = temporary.path
        self.backend, self.frontend = self.root / "backend", self.root / "frontend"
        for name, repository in (("backend", self.backend), ("frontend", self.frontend)):
            repository.mkdir()
            (repository / ".gitignore").write_text("/.local-tests/\n", encoding="utf-8")
            (repository / "README.md").write_text(name + "\n", encoding="utf-8")
        self.model = self.backend / MODEL
        self.model.parent.mkdir(parents=True)
        self.model.write_bytes(b"pub struct Order;\n")
        self.sources = {}
        for name, repository in (("backend", self.backend), ("frontend", self.frontend)):
            self.git(repository, "init", "--quiet")
            self.git(repository, "add", ".")
            self.git(repository, "-c", "user.name=RyFrame fixture",
                     "-c", "user.email=fixture@example.invalid", "commit", "--quiet", "-m", "source fixture")
            self.sources[name] = snapshot(repository)[0]
        self.directory = self.root / "runtime"
        self.directory.mkdir()
        self.pair = {"format_version": 1, "backend_sha": self.sources["backend"]["head"],
                     "frontend_sha": self.sources["frontend"]["head"], "run_id": 12, "attempt": 3,
                     "sources": copy.deepcopy(self.sources)}
        self.pair_path = self.directory / "source-pair.json"
        write_receipt(self.pair_path, self.pair)

    def git(self, repository, *arguments):
        return subprocess.run(["git", "-C", str(repository), "-c", "core.autocrlf=false",
                               "-c", "core.hooksPath=" + str(self.root / "no-hooks"),
                               "-c", "commit.gpgSign=false", *arguments],
                              stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=30, check=True)

    def prepare_business(self):
        self.model.write_bytes(b"pub struct Order;\npub const GENERATED: bool = true;\n")
        self.generated = {"backend": snapshot(self.backend)[0], "frontend": snapshot(self.frontend)[0]}
        self.fixture = {"format_version": 1, "fixture": "business", "status": "ready",
                        "fixture_sha256": hashlib.sha256(self.model.read_bytes()).hexdigest(),
                        "sources": copy.deepcopy(self.sources),
                        "paths": {"backend": str(self.backend), "frontend": str(self.frontend)},
                        "generated": copy.deepcopy(self.generated)}
        self.fixture_path = self.root / "fixture.json"
        write_receipt(self.fixture_path, self.fixture)

    def native_fact(self):
        response, _ = native._read_response(*native._invoke(
            "__source-evidence", {"operation": "full-stack-source-evidence", "write": False,
                "arguments": {"backend": str(self.backend), "directory": str(self.directory),
                              "ci": {"run_id": os.environ.get("GITHUB_RUN_ID"),
                                     "attempt": os.environ.get("GITHUB_RUN_ATTEMPT")}}},
            subprocess.run, may_write=False))
        return response["value"]

    def frozen_receipts(self):
        # 直接取得生产者事实并原样放入收据，不让 Python 来源算法签发自身的预期值。
        source = self.native_fact()
        binaries = {}
        artifacts = {}
        for role in ("ryframe", "ryframe-worker", "ryframe-reset", "ryframe-migrate"):
            path = self.root / (role + ".exe")
            path.write_bytes(("frozen " + role).encode())
            binaries[role] = str(path)
            artifacts[role] = {"path": str(path), **file_digest(path)}
        build = {"format_version": 1, "kind": "full-stack-build", "backend_root": str(self.backend),
                 "source": source, "artifacts": artifacts}
        write_receipt(self.directory / "binaries.json", binaries)
        build_path = self.directory / provenance.BUILD_EVIDENCE
        write_receipt(build_path, build)
        runtime = self.directory / "runtime.json"
        write_receipt(runtime, {"fixture": True})
        evidence = {"format_version": 1, "kind": "full-stack-runtime", "source": source,
                    "build_evidence": {"path": str(build_path), **file_digest(build_path)},
                    "runtime": {"path": str(runtime), **file_digest(runtime)}}
        write_receipt(self.directory / provenance.RUNTIME_EVIDENCE, evidence)
        return source, binaries, build, runtime, evidence

    def test_core_source_also_binds_selected_pair_and_current_backend_snapshot(self):
        missing = self.root / "absent-backend"
        self.assertIsNone(provenance.full_stack_source_evidence(missing))
        self.assertIsNone(provenance.full_stack_source_evidence(missing, self.root))
        self.assertIsNone(provenance.full_stack_source_evidence(missing, self.root / "not-full-stack"))
        evidence = provenance.full_stack_source_evidence(self.backend, self.directory)
        self.assertEqual(evidence["fixture"], "core")
        self.assertEqual(evidence["backend_root"], str(self.backend))
        self.assertEqual(evidence["original"], {"backend": self.sources["backend"]})
        self.assertEqual(evidence["source_pair"]["sources"], self.sources)
        self.assertEqual(evidence["source_pair"]["receipt"],
                         {"path": str(self.pair_path), **file_digest(self.pair_path)})
        with patch.dict(os.environ):
            os.environ.pop("GITHUB_RUN_ID", None)
            os.environ.pop("GITHUB_RUN_ATTEMPT", None)
            self.assertEqual(provenance.full_stack_source_evidence(self.backend, self.directory), evidence)
        (self.backend / "README.md").write_text("changed\n", encoding="utf-8")
        with self.assertRaises(ValueError):
            provenance.full_stack_source_evidence(self.backend, self.directory)

    def test_native_business_fact_verifies_frozen_build_and_runtime_without_rewriting_model_path(self):
        self.prepare_business()
        source, binaries, build, runtime, evidence = self.frozen_receipts()
        self.assertEqual(source["fixture"], "business")
        self.assertEqual(source["original"], self.sources)
        self.assertEqual(source["generated"], self.generated)
        self.assertEqual(source["roots"], {"backend": str(self.backend), "frontend": str(self.frontend)})
        self.assertEqual(source["fixture_receipt"],
                         {"path": str(self.fixture_path), **file_digest(self.fixture_path)})
        model = source["fixture_definition"]["model"]
        self.assertEqual({field: model[field] for field in ("bytes", "sha256")}, file_digest(self.model))
        if os.name == "nt":
            self.assertIn("/", model["path"])
            self.assertIn("\\", model["path"])
            self.assertNotEqual(model["path"], str(Path(model["path"])))
        receipts = [self.fixture_path, self.pair_path, self.directory / "binaries.json",
                    self.directory / provenance.BUILD_EVIDENCE,
                    self.directory / provenance.RUNTIME_EVIDENCE, runtime]
        before = {path: path.read_bytes() for path in receipts}
        self.assertEqual(provenance.verify_build_evidence(self.backend, self.directory), build)
        self.assertEqual(provenance.verify_runtime_evidence(self.backend, self.directory, runtime), evidence)
        self.assertEqual(provenance.register_runtime_evidence(self.backend, self.directory, runtime), evidence)
        self.assertEqual(before, {path: path.read_bytes() for path in receipts})
        with self.assertRaisesRegex(ValueError, "恰好包含"):
            provenance.build_evidence(self.backend, self.directory,
                                      {role: path for role, path in binaries.items() if role != "ryframe-worker"}, source)

    def test_local_fixture_pair_writer_and_native_reader_keep_original_identity_and_no_overwrite(self):
        self.prepare_business()
        output = self.backend / ".local-tests/reference-fixture/runtime/source-pair.json"
        output.parent.mkdir(parents=True)
        result = write_pair(self.backend, output)
        self.assertEqual(result["kind"], provenance.REFERENCE_FIXTURE_SOURCE_PAIR)
        self.assertEqual(result["sources"], self.sources)
        self.assertEqual(result["fixture_receipt"],
                         {"path": str(self.fixture_path), **file_digest(self.fixture_path)})
        with patch.dict(os.environ, {"GITHUB_RUN_ID": "different-ci", "GITHUB_RUN_ATTEMPT": "99"}):
            evidence = provenance.full_stack_source_evidence(self.backend, output.parent)
        self.assertEqual(evidence["source_pair"]["kind"], provenance.REFERENCE_FIXTURE_SOURCE_PAIR)
        before = output.read_bytes()
        with self.assertRaisesRegex(ValueError, "不得覆盖"):
            write_pair(self.backend, output)
        self.assertEqual(output.read_bytes(), before)
        wrong = copy.deepcopy(result)
        wrong["sources"]["backend"]["head"] = "c" * 40
        write_receipt(output, wrong)
        with self.assertRaises(ValueError):
            provenance.full_stack_source_evidence(self.backend, output.parent)

    def test_business_fixture_and_generated_snapshot_drift_fail_closed(self):
        self.prepare_business()
        model = self.model.read_bytes()
        readme = self.backend / "README.md"
        original_readme = readme.read_bytes()
        mutations = (
            ("status", lambda: self.fixture.update(status="preparing")),
            ("commit", lambda: self.fixture["sources"]["backend"].update(head="c" * 40)),
            ("path", lambda: self.fixture["paths"].update(backend=str(self.backend) + "/../backend")),
            ("definition", lambda: self.model.write_bytes(b"changed model\n")),
            ("snapshot", lambda: readme.write_bytes(b"changed generated tree\n")),
        )
        baseline = copy.deepcopy(self.fixture)
        for name, mutate in mutations:
            with self.subTest(name=name):
                self.fixture = copy.deepcopy(baseline)
                self.model.write_bytes(model)
                readme.write_bytes(original_readme)
                mutate()
                write_receipt(self.fixture_path, self.fixture)
                with self.assertRaises(ValueError):
                    provenance.full_stack_source_evidence(self.backend, self.directory)

    def test_core_pair_sources_duplicate_fields_and_stale_ci_fail_closed(self):
        raw = self.pair_path.read_bytes()
        wrong = copy.deepcopy(self.pair)
        wrong["frontend_sha"] = "c" * 40
        duplicate = raw.replace(b'"format_version": 1', b'"format_version": 1, "format_version": 1', 1)
        self.assertNotEqual(duplicate, raw)
        for name, content in (("frontend", json.dumps(wrong).encode()), ("duplicate", duplicate)):
            with self.subTest(name=name):
                self.pair_path.write_bytes(content)
                with self.assertRaises(ValueError):
                    provenance.full_stack_source_evidence(self.backend, self.directory)
        self.pair_path.write_bytes(raw)
        for identity in ({"GITHUB_RUN_ID": "13", "GITHUB_RUN_ATTEMPT": "3"},
                         {"GITHUB_RUN_ID": "12", "GITHUB_RUN_ATTEMPT": "4"},
                         {"GITHUB_RUN_ID": "invalid", "GITHUB_RUN_ATTEMPT": "3"}):
            with self.subTest(ci=identity), patch.dict(os.environ, identity), self.assertRaises(ValueError):
                provenance.full_stack_source_evidence(self.backend, self.directory)

    def test_frozen_consumers_reject_changed_fixture_artifact_runtime_and_source_fact(self):
        self.prepare_business()
        _, binaries, build, runtime, _ = self.frozen_receipts()
        artifact = Path(binaries["ryframe"])
        build_path = self.directory / provenance.BUILD_EVIDENCE
        paths = (self.fixture_path, artifact, runtime, build_path)
        original = {path: path.read_bytes() for path in paths}
        forged = copy.deepcopy(build)
        forged["source"]["fixture_definition"]["model"]["path"] += ".wrong"
        for name, path, content, reject_build in (
            ("fixture", self.fixture_path, original[self.fixture_path] + b" \n", True),
            ("artifact", artifact, b"changed frozen artifact", True),
            ("runtime", runtime, b"changed runtime", False),
            ("source", build_path, json.dumps(forged).encode(), True),
        ):
            with self.subTest(name=name):
                for item, raw in original.items():
                    item.write_bytes(raw)
                path.write_bytes(content)
                if reject_build:
                    with self.assertRaises(ValueError):
                        provenance.verify_build_evidence(self.backend, self.directory)
                else:
                    self.assertEqual(provenance.verify_build_evidence(self.backend, self.directory), build)
                with self.assertRaises(ValueError):
                    provenance.verify_runtime_evidence(self.backend, self.directory, runtime)


if __name__ == "__main__":
    unittest.main()
