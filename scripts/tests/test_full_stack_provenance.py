import copy
import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import sys
import unittest
from unittest import mock

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import full_stack_provenance as provenance
from full_stack_process import write_receipt
from reference_fixture_source_pair import write_pair
from workspace_directory import WorkspaceDirectory


class DeviceSourceEvidenceTests(unittest.TestCase):
    def setUp(self):
        local = Path(__file__).resolve().parents[2] / ".local-tests/python-unit"
        local.mkdir(parents=True, exist_ok=True)
        temporary = WorkspaceDirectory(local, "full-stack-provenance-")
        self.addCleanup(temporary.cleanup)
        self.root = temporary.path
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
            "backend": self.source(self.backend_head, provenance.EMPTY_SHA256),
            "frontend": self.source(self.frontend_head, provenance.EMPTY_SHA256),
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
                    "format_version": 1,
                    "backend_sha": self.backend_head,
                    "frontend_sha": self.frontend_head,
                    "run_id": 12,
                    "attempt": 3,
                    "sources": self.sources,
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
        return {
            "head": head,
            "patch_sha256": patch if len(patch) == 64 else patch * 64,
            "files": [],
        }

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

    def test_local_fixture_pair_binds_fixture_without_claiming_ci_identity(self):
        pair = provenance.reference_fixture_source_pair(self.backend)
        self.assertEqual(pair["kind"], provenance.REFERENCE_FIXTURE_SOURCE_PAIR)
        self.assertEqual(pair["sources"], self.sources)
        (self.root / "source-pair.json").write_text(json.dumps(pair) + "\n", encoding="utf-8")
        evidence = provenance.full_stack_source_evidence(self.backend, self.root)
        self.assertEqual(evidence["source_pair"]["kind"], provenance.REFERENCE_FIXTURE_SOURCE_PAIR)

        changed = {**pair, "sources": {**self.sources, "backend": self.source("c" * 40, provenance.EMPTY_SHA256)}}
        (self.root / "source-pair.json").write_text(json.dumps(changed) + "\n", encoding="utf-8")
        with self.assertRaisesRegex(ValueError, "fixture 原始来源"):
            provenance.full_stack_source_evidence(self.backend, self.root)

    def test_local_fixture_pair_writer_only_accepts_new_fixture_runtime_receipt(self):
        output = self.backend / ".local-tests/reference-fixture/runtime/source-pair.json"
        output.parent.mkdir(parents=True)
        result = write_pair(self.backend, output)
        self.assertEqual(result["kind"], provenance.REFERENCE_FIXTURE_SOURCE_PAIR)
        self.assertEqual(json.loads(output.read_text(encoding="utf-8")), result)
        with self.assertRaisesRegex(ValueError, "不得覆盖"):
            write_pair(self.backend, output)

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
            ("frontend", original | {"frontend_sha": "c" * 40}, {}, "来源"),
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
        with mock.patch.dict(os.environ, {"RYFRAME_E2E_FIXTURE": "core"}), mock.patch.object(
            provenance,
            "snapshot",
            return_value=(copy.deepcopy(self.sources["backend"]), b""),
        ):
            evidence = provenance.full_stack_source_evidence(self.backend, self.root)
        self.assertEqual(evidence["fixture"], "core")
        self.assertEqual(evidence["original"], {"backend": self.sources["backend"]})
        self.assertEqual(evidence["source_pair"]["backend_sha"], self.backend_head)
        self.assertEqual(evidence["source_pair"]["frontend_sha"], self.frontend_head)
        self.assertEqual(evidence["source_pair"]["sources"], self.sources)

    def test_source_pair_producer_requires_stable_clean_snapshots_for_both_repositories(self):
        snapshots = {
            self.backend: self.sources["backend"],
            self.frontend: self.sources["frontend"],
        }
        with mock.patch.object(
            provenance,
            "snapshot",
            side_effect=lambda root: (copy.deepcopy(snapshots[Path(root).resolve()]), b""),
        ):
            receipt = provenance.source_pair_receipt(self.backend, self.frontend, 12, 3)
        self.assertEqual(receipt["sources"], self.sources)
        self.assertEqual(receipt["backend_sha"], self.backend_head)
        self.assertEqual(receipt["frontend_sha"], self.frontend_head)

        dirty = copy.deepcopy(self.sources)
        dirty["frontend"]["patch_sha256"] = "f" * 64
        with mock.patch.object(
            provenance,
            "snapshot",
            side_effect=lambda root: (
                copy.deepcopy(dirty["backend" if Path(root) == self.backend else "frontend"]),
                b"",
            ),
        ), self.assertRaisesRegex(ValueError, "干净源码"):
            provenance.source_pair_receipt(self.backend, self.frontend, 12, 3)

    def test_source_pair_verifier_rejects_changed_source_or_run_identity(self):
        path = self.root / "recorded-source-pair.json"
        snapshots = {
            self.backend: self.sources["backend"],
            self.frontend: self.sources["frontend"],
        }
        with mock.patch.object(
            provenance,
            "snapshot",
            side_effect=lambda root: (
                copy.deepcopy(snapshots[Path(root).resolve()]),
                b"",
            ),
        ):
            receipt = provenance.source_pair_receipt(
                self.backend, self.frontend, 12, 3
            )
        path.write_text(json.dumps(receipt), encoding="utf-8")
        with mock.patch.object(
            provenance, "source_pair_receipt", return_value=receipt
        ):
            self.assertEqual(
                provenance.verify_source_pair_receipt(
                    path, self.backend, self.frontend, 12, 3
                ),
                receipt,
            )

        changed = copy.deepcopy(receipt)
        changed["sources"]["frontend"]["head"] = "c" * 40
        cases = (changed, {**receipt, "attempt": 4})
        for current in cases:
            with self.subTest(current=current), mock.patch.object(
                provenance, "source_pair_receipt", return_value=current
            ), self.assertRaisesRegex(ValueError, "当前干净源码或运行身份"):
                provenance.verify_source_pair_receipt(
                    path, self.backend, self.frontend, 12, 3
                )

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


class ArchiveEvidenceTests(unittest.TestCase):
    def setUp(self):
        self.backend_sha = "a" * 40
        self.frontend_sha = "b" * 40
        self.run_id = 123
        self.attempt = 2
        self.receipt_root = "/home/runner/work/_temp/ryframe-full-stack"
        self.fixture_hash = hashlib.sha256(b"device fixture").hexdigest()

    @staticmethod
    def raw(value: dict) -> bytes:
        return (json.dumps(value, sort_keys=True, separators=(",", ":")) + "\n").encode()

    @staticmethod
    def binding(path: str, raw: bytes) -> dict:
        return {
            "path": path,
            "bytes": len(raw),
            "sha256": hashlib.sha256(raw).hexdigest(),
        }

    def snapshot(self, head: str, *, clean: bool, files: list[dict] | None = None) -> dict:
        return {
            "head": head,
            "patch_sha256": provenance.EMPTY_SHA256
            if clean
            else hashlib.sha256(f"generated-{head}".encode()).hexdigest(),
            "files": files or [],
        }

    def archive(self, fixture: str = "core") -> tuple[list[tuple[str, bytes]], dict[str, dict]]:
        sources = {
            "backend": self.snapshot(self.backend_sha, clean=True),
            "frontend": self.snapshot(self.frontend_sha, clean=True),
        }
        pair = {
            "format_version": 1,
            "backend_sha": self.backend_sha,
            "frontend_sha": self.frontend_sha,
            "run_id": self.run_id,
            "attempt": self.attempt,
            "sources": sources,
        }
        pair_raw = self.raw(pair)
        embedded_pair = {
            **pair,
            "receipt": self.binding(f"{self.receipt_root}/source-pair.json", pair_raw),
        }
        fixture_receipt = None
        fixture_raw = None
        if fixture == "core":
            backend_root = "/home/runner/work/ryframe/ryframe/backend"
            source = {
                "format_version": 1,
                "fixture": "core",
                "backend_root": backend_root,
                "source_pair": embedded_pair,
                "original": {"backend": sources["backend"]},
            }
        else:
            fixture_root = "/home/runner/work/ryframe/ryframe/backend/.local-tests/device-fixture"
            backend_root = f"{fixture_root}/backend"
            frontend_root = f"{fixture_root}/frontend"
            original = sources
            generated = {
                "backend": self.snapshot(
                    self.backend_sha,
                    clean=False,
                    files=[
                        {
                            "path": provenance.DEVICE_RESOURCE.as_posix(),
                            "sha256": self.fixture_hash,
                        }
                    ],
                ),
                "frontend": self.snapshot(
                    self.frontend_sha,
                    clean=False,
                    files=[{"path": "src/generated/resources/device/index.ts", "sha256": "c" * 64}],
                ),
            }
            roots = {"backend": backend_root, "frontend": frontend_root}
            fixture_receipt = {
                "format_version": 1,
                "fixture": "device",
                "status": "ready",
                "fixture_sha256": self.fixture_hash,
                "sources": original,
                "paths": roots,
                "generated": generated,
            }
            fixture_raw = self.raw(fixture_receipt)
            definition = {
                "bytes": len(b"device fixture"),
                "sha256": self.fixture_hash,
            }
            source = {
                "format_version": 1,
                "fixture": "device",
                "fixture_receipt": self.binding(f"{fixture_root}/fixture.json", fixture_raw),
                "fixture_definition": {
                    "fixture": {
                        "path": f"{backend_root}/{provenance.DEVICE_FIXTURE.as_posix()}",
                        **definition,
                    },
                    "resource": {
                        "path": f"{backend_root}/{provenance.DEVICE_RESOURCE.as_posix()}",
                        **definition,
                    },
                },
                "roots": roots,
                "source_pair": embedded_pair,
                "original": original,
                "generated": generated,
            }

        artifacts = {}
        for role in provenance.BINARY_ROLES:
            artifacts[role] = {
                "path": f"/home/runner/work/ryframe/ryframe/target/ci/full-stack/{role}",
                "bytes": len(role),
                "sha256": hashlib.sha256(role.encode()).hexdigest(),
            }
        build = {
            "format_version": 1,
            "kind": "full-stack-build",
            "backend_root": backend_root,
            "source": source,
            "artifacts": artifacts,
        }
        build_raw = self.raw(build)
        runtime = {
            "format_version": 1,
            "backend_root": backend_root,
            "scope_id": "ci-123-2",
            "configuration_sha256": "d" * 64,
            "worker_ready_url": "http://127.0.0.1:9091/readyz",
            "artifacts": {
                runtime_role: {
                    "path": artifacts[build_role]["path"],
                    "sha256": artifacts[build_role]["sha256"],
                }
                for runtime_role, build_role in (("api", "ryframe"), ("worker", "ryframe-worker"))
            },
        }
        runtime_raw = self.raw(runtime)
        runtime_evidence = {
            "format_version": 1,
            "kind": "full-stack-runtime",
            "source": source,
            "build_evidence": self.binding(f"{self.receipt_root}/build-evidence.json", build_raw),
            "runtime": self.binding(f"{self.receipt_root}/runtime.json", runtime_raw),
        }
        raw_receipts = {
            provenance.SOURCE_PAIR_RECEIPT: pair_raw,
            provenance.BUILD_EVIDENCE: build_raw,
            provenance.RUNTIME_RECEIPT: runtime_raw,
            provenance.RUNTIME_EVIDENCE: self.raw(runtime_evidence),
        }
        if fixture_raw is not None:
            raw_receipts[provenance.FIXTURE_RECEIPT] = fixture_raw
        entries = [(f"artifact/logs/{name}", raw) for name, raw in raw_receipts.items()]
        values = {
            "pair": pair,
            "build": build,
            "runtime": runtime,
            "runtime_evidence": runtime_evidence,
        }
        if fixture_receipt is not None:
            values["fixture"] = fixture_receipt
        return entries, values

    def validate(self, entries, fixture="core"):
        return provenance.validate_archive_evidence(
            entries,
            fixture=fixture,
            backend_sha=self.backend_sha,
            frontend_sha=self.frontend_sha,
            run_id=self.run_id,
            attempt=self.attempt,
            fixture_sha256=self.fixture_hash if fixture == "device" else None,
        )

    @staticmethod
    def replace(entries, name: str, raw: bytes) -> list[tuple[str, bytes]]:
        return [
            (path, raw if PurePosixPath(path).name == name else value)
            for path, value in entries
        ]

    def test_core_archive_binds_clean_source_all_roles_and_runtime_receipts(self):
        entries, values = self.archive()
        result = self.validate(entries)
        self.assertEqual(result["source_pair"], values["pair"])
        self.assertEqual(result["build_evidence"], values["build"])
        self.assertEqual(result["runtime"], values["runtime"])
        self.assertEqual(result["runtime_evidence"], values["runtime_evidence"])

    def test_device_archive_binds_raw_fixture_clean_sources_and_generated_resource(self):
        entries, values = self.archive("device")
        result = self.validate(entries, "device")
        self.assertEqual(result["fixture_receipt"], values["fixture"])
        self.assertEqual(
            result["build_evidence"]["source"]["original"], values["fixture"]["sources"]
        )
        with self.assertRaisesRegex(ValueError, "fixture"):
            provenance.validate_archive_evidence(
                entries,
                fixture="device",
                backend_sha=self.backend_sha,
                frontend_sha=self.frontend_sha,
                run_id=self.run_id,
                attempt=self.attempt,
                fixture_sha256="e" * 64,
            )

    def test_every_json_receipt_rejects_duplicate_fields_before_binding_checks(self):
        entries, _ = self.archive("device")
        for name in provenance.ARCHIVE_RECEIPT_LIMITS:
            with self.subTest(name=name), self.assertRaisesRegex(ValueError, "重复字段"):
                self.validate(self.replace(entries, name, b'{"field":1,"field":2}'), "device")

    def test_missing_duplicate_and_noncanonical_archive_members_fail_closed(self):
        entries, _ = self.archive()
        with self.assertRaisesRegex(ValueError, "缺少"):
            self.validate(entries[:-1])
        with self.assertRaisesRegex(ValueError, "重复"):
            self.validate([*entries, ("copy/runtime.json", entries[2][1])])
        with self.assertRaisesRegex(ValueError, "路径"):
            self.validate([("../escape.txt", b"ignored"), *entries])

    def test_unknown_schema_missing_build_role_and_dirty_source_fail_closed(self):
        entries, values = self.archive()
        unknown = {**values["runtime_evidence"], "unknown": True}
        with self.assertRaisesRegex(ValueError, "字段"):
            self.validate(self.replace(entries, provenance.RUNTIME_EVIDENCE, self.raw(unknown)))

        missing_role = copy.deepcopy(values["build"])
        del missing_role["artifacts"]["ryframe-reset"]
        with self.assertRaisesRegex(ValueError, "字段"):
            self.validate(self.replace(entries, provenance.BUILD_EVIDENCE, self.raw(missing_role)))

        dirty = copy.deepcopy(values["build"])
        dirty["source"]["original"]["backend"]["patch_sha256"] = "f" * 64
        with self.assertRaisesRegex(ValueError, "干净源码"):
            self.validate(self.replace(entries, provenance.BUILD_EVIDENCE, self.raw(dirty)))

    def test_runtime_must_reference_build_paths_and_hashes(self):
        entries, values = self.archive()
        runtime = copy.deepcopy(values["runtime"])
        runtime["artifacts"]["api"]["path"] = "/tmp/unbound/ryframe"
        runtime_raw = self.raw(runtime)
        evidence = copy.deepcopy(values["runtime_evidence"])
        evidence["runtime"] = self.binding(f"{self.receipt_root}/runtime.json", runtime_raw)
        changed = self.replace(entries, provenance.RUNTIME_RECEIPT, runtime_raw)
        changed = self.replace(changed, provenance.RUNTIME_EVIDENCE, self.raw(evidence))
        with self.assertRaisesRegex(ValueError, "路径或摘要"):
            self.validate(changed)

        runtime = copy.deepcopy(values["runtime"])
        runtime["scope_id"] = "ci-123-1"
        runtime_raw = self.raw(runtime)
        evidence = copy.deepcopy(values["runtime_evidence"])
        evidence["runtime"] = self.binding(f"{self.receipt_root}/runtime.json", runtime_raw)
        changed = self.replace(entries, provenance.RUNTIME_RECEIPT, runtime_raw)
        changed = self.replace(changed, provenance.RUNTIME_EVIDENCE, self.raw(evidence))
        with self.assertRaisesRegex(ValueError, "scope"):
            self.validate(changed)

    def test_receipt_bindings_hash_the_exact_archived_bytes_and_share_one_directory(self):
        entries, values = self.archive()
        changed = self.replace(
            entries,
            provenance.BUILD_EVIDENCE,
            next(
                raw
                for path, raw in entries
                if PurePosixPath(path).name == provenance.BUILD_EVIDENCE
            )
            + b" ",
        )
        with self.assertRaisesRegex(ValueError, "原始字节"):
            self.validate(changed)

        evidence = copy.deepcopy(values["runtime_evidence"])
        evidence["runtime"]["path"] = "/home/runner/work/_temp/another/runtime.json"
        with self.assertRaisesRegex(ValueError, "同一全栈运行目录"):
            self.validate(self.replace(entries, provenance.RUNTIME_EVIDENCE, self.raw(evidence)))


if __name__ == "__main__":
    unittest.main()
