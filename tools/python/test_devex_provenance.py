"""DevEx 性能来源只读消费正式恢复 v3 运行代次。"""

import copy
import hashlib
import json
import struct
import subprocess
import sys
import unittest
from pathlib import Path
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parent))
sys.path.insert(0, str(Path(__file__).resolve().parent))
import devex_provenance as provenance
import restore_runtime
import test_restore_runtime as runtime_tests
from restore_runtime import _descriptor
from restore_runtime_evidence import read_json_document
from workspace_directory import WorkspaceDirectory

ROOT = Path(__file__).resolve().parents[2]
SHA = "a" * 40
FINGERPRINT = "sha256:" + "b" * 64
RUNNER_FINGERPRINT = "sha256:" + "c" * 64
SNAPSHOT = {"head": SHA, "patch_sha256": "d" * 64, "files": [], "clean": True}


class ProvenanceTests(unittest.TestCase):
    def setUp(self):
        temporary = WorkspaceDirectory(ROOT / ".local-tests/python-unit", "devex-v3-")
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name).resolve()
        self.backend = self.directory("backend")
        self.frontend = self.directory("frontend")
        self.driver = self.directory("driver")
        self.runner = self.directory("runner")
        self.control = self.driver / ".local-tests/runtime"
        self.control.mkdir(parents=True)
        self.generation = self.control / "generation-0001"
        self.generation.mkdir()
        self.environment = self.backend / ".local-tests/devex/environment.md"
        self.environment.parent.mkdir(parents=True)
        self.environment.write_text("# 已登记环境\n不含凭据。", encoding="utf-8")
        self.reference = self.write("reference.json", {
            "kind": "restore-reference-plan",
            "target": {"frontend_url": "http://127.0.0.1:14174"},
        })
        self.target = self.write("target.json", {"kind": "restore-reference-target-plan"})
        self.registration = self.write("registration.json", {
            "kind": "restore-runtime-registration",
        })
        self.lifecycle = self.write("lifecycle.json", {
            "kind": "restore-runtime-lifecycle",
            "generations": [{"number": 1, "status": "running"}],
        })
        self.bindings = self.write("bindings.json", {
            "record": {"plan": {}},
            "manifest": {"source_sha": "e" * 40},
        })
        self.launch = self.write("runtime-launch.json", {
            "format_version": 1,
            "kind": "restore-runtime-launch",
            "generation": 1,
            "runtime_directory": str(self.generation),
            "request": {
                "registration": {
                    "registration": _descriptor(self.registration),
                    "target_plan": _descriptor(self.target),
                },
            },
        }, parent=self.generation)
        self.runtime_receipt = {
            "format_version": 3,
            "kind": "restore-runtime",
            "restore": {
                "id": "restore-one",
                "backup_id": "backup-one",
                "plan_hash": "f" * 64,
                "scope_id": "perf-isolated",
                "data_verified_at": "2026-09-12T08:00:00+08:00",
            },
            "paths": {
                "backend_product_root": str(self.backend),
                "backend_execution_root": str(self.backend),
                "frontend_root": str(self.frontend),
                "runtime_dir": str(self.generation),
                "bindings": str(self.bindings.path),
                "backend_build": str(self.control / "backend-build.json"),
                "frontend_build": str(self.control / "frontend-build.json"),
                "launch": str(self.launch.path),
            },
            "digests": {
                "bindings": self.bindings.sha256,
                "backend_build": "1" * 64,
                "frontend_build": "2" * 64,
                "launch": self.launch.sha256,
            },
            "source": {
                "backup_source_sha": "e" * 40,
                "backend_product_sha": SHA,
                "backend_execution_sha": SHA,
                "backend_adapter_contract": None,
                "frontend_sha": SHA,
            },
            "endpoints": {
                "api": "http://127.0.0.1:18082/readyz",
                "worker": "http://127.0.0.1:19093/readyz",
                "frontend": "http://127.0.0.1:14174",
            },
            "backend": {"kind": "backend-build"},
            "frontend": {"kind": "frontend-build"},
            "processes": {},
        }
        self.runtime = self.write("restore-runtime.json", self.runtime_receipt)
        self.request = {
            "backend": str(self.backend),
            "frontend": str(self.frontend),
            "driver": str(self.driver),
            "runner_frontend": str(self.runner),
            "driver_fingerprint": FINGERPRINT,
            "runner_frontend_fingerprint": RUNNER_FINGERPRINT,
            "scope_id": "perf-isolated",
            "source_fingerprints": {
                "backend": FINGERPRINT,
                "frontend": FINGERPRINT,
                "runner_frontend": RUNNER_FINGERPRINT,
            },
            "api_url": "http://127.0.0.1:18082",
            "frontend_url": "http://127.0.0.1:14174",
            "metrics_urls": {
                "api": "http://127.0.0.1:18082/metrics",
                "worker": "http://127.0.0.1:19093/metrics",
            },
            "environment_sha256": self.digest(self.environment),
            "provenance": {
                "runtime_receipt": _descriptor(self.runtime),
                "target_plan": _descriptor(self.target),
                "runtime_registration": _descriptor(self.registration),
                "runtime_launch": _descriptor(self.launch),
                "environment_document": str(self.environment),
            },
        }
        self.documents = (
            self.registration,
            self.reference,
            self.target,
            self.target,
            self.lifecycle,
            self.launch,
        )
        self.control_result = (
            self.control,
            self.launch.value["request"]["registration"],
            self.launch.value,
            self.documents,
        )
        self.execution = {
            "snapshot": SNAPSHOT,
            "worktree_fingerprint": FINGERPRINT,
            "fingerprints": {},
        }
        self.runner_execution = {
            **self.execution,
            "worktree_fingerprint": RUNNER_FINGERPRINT,
        }
        self.runtime_verification = {
            "format_version": 1,
            "kind": "restore-runtime-verification",
            "processes": {
                role: {"identity": {"pid": index + 42, "started": role}}
                for index, role in enumerate(("api", "worker", "frontend"))
            },
        }
        self.validate = self.enterContext(patch.object(
            provenance, "validate_runtime_receipt", side_effect=lambda value: value))
        self.bind = self.enterContext(patch.object(
            provenance, "_bind_control_inputs", return_value=self.control_result))
        self.static = self.enterContext(patch("restore_runtime_static.verify_static_runtime"))
        self.require = self.enterContext(patch.object(
            provenance, "require_bindings", return_value=({}, self.bindings.value["manifest"])))
        self.live = self.enterContext(patch.object(
            provenance.restore_runtime, "verify", return_value=self.runtime_verification))
        self.source = self.enterContext(patch.object(
            provenance, "verify_source", return_value=SNAPSHOT))
        self.source_state = self.enterContext(patch.object(
            provenance, "current_execution_source", side_effect=self.execution_source))

    def directory(self, name):
        result = self.root / name
        result.mkdir()
        return result

    def write(self, name, value, *, parent=None):
        path = (parent or self.control) / name
        path.write_text(json.dumps(value), encoding="utf-8")
        return read_json_document(path)

    @staticmethod
    def digest(path):
        return hashlib.sha256(path.read_bytes()).hexdigest()

    def execution_source(self, root):
        return copy.deepcopy(
            self.runner_execution if root == self.runner else self.execution
        )

    def verify(self, request=None):
        return provenance.verify(request or self.request)

    def test_exact_v3_chain_is_read_only_and_distinguishes_every_source(self):
        before = {
            path: path.read_bytes()
            for path in self.root.rglob("*")
            if path.is_file()
        }
        result = self.verify()
        after = {
            path: path.read_bytes()
            for path in self.root.rglob("*")
            if path.is_file()
        }
        self.assertEqual(before, after)
        self.assertEqual(result["format_version"], 3)
        self.assertEqual(result["runtime"]["generation"], 1)
        self.assertEqual(result["roots"], {
            "backend_product": str(self.backend),
            "backend_execution": str(self.backend),
            "frontend": str(self.frontend),
            "driver": str(self.driver),
            "runner_frontend": str(self.runner),
        })
        self.assertEqual(result["receipts"]["runtime_receipt"], _descriptor(self.runtime))
        self.assertEqual(result["environment_document"]["path"], str(self.environment))
        self.assertEqual(self.bind.call_count, 2)
        self.static.assert_called_once()
        self.live.assert_called_once()

    def test_b0_adapter_keeps_product_and_execution_roots_distinct(self):
        product = self.directory("product-backend")
        self.runtime_receipt["paths"]["backend_product_root"] = str(product)
        self.runtime_receipt["source"]["backend_product_sha"] = SHA
        self.runtime_receipt["source"]["backend_execution_sha"] = "9" * 40
        self.runtime_receipt["source"][
            "backend_adapter_contract"
        ] = "legacy-stable-readiness-b0-v1"
        self.runtime.path.write_text(
            json.dumps(self.runtime_receipt), encoding="utf-8"
        )
        self.runtime = read_json_document(self.runtime.path)
        self.request["provenance"]["runtime_receipt"] = _descriptor(self.runtime)
        result = self.verify()
        self.assertEqual(result["roots"]["backend_product"], str(product))
        self.assertEqual(result["roots"]["backend_execution"], str(self.backend))
        self.assertEqual(
            self.live.call_args.kwargs["product_backend"], product
        )

    def test_legacy_or_inexact_provenance_schema_is_rejected_before_control(self):
        legacy = {
            "backend_build": {"path": str(self.control / "build.json"), "sha256": "1" * 64},
            "runtime": {"path": str(self.control / "runtime.json"), "sha256": "2" * 64},
            "processes": {"api": "3" * 64, "worker": "4" * 64},
            "frontend_build_sha256": "5" * 64,
            "environment_document": str(self.environment),
        }
        candidates = [
            legacy,
            {**self.request["provenance"], "extra": True},
            {**self.request["provenance"], "runtime_receipt": {
                **self.request["provenance"]["runtime_receipt"], "bytes": True}},
        ]
        for value in candidates:
            changed = copy.deepcopy(self.request)
            changed["provenance"] = value
            with self.subTest(value=value), self.assertRaisesRegex(
                provenance.ProvenanceError, "runtime_receipts"
            ):
                self.verify(changed)
        self.bind.assert_not_called()

    def test_descriptor_and_control_chain_mismatches_fail_closed(self):
        changed = copy.deepcopy(self.request)
        changed["provenance"]["target_plan"]["sha256"] = "0" * 64
        with self.assertRaisesRegex(provenance.ProvenanceError, "runtime_receipts"):
            self.verify(changed)
        other = self.write("other-target.json", self.target.value)
        changed = copy.deepcopy(self.request)
        changed["provenance"]["target_plan"] = _descriptor(other)
        with self.assertRaisesRegex(provenance.ProvenanceError, "runtime_generation"):
            self.verify(changed)
        self.static.assert_not_called()
        self.live.assert_not_called()

    def test_non_running_or_changed_generation_is_rejected(self):
        self.bind.side_effect = ValueError("generation stopped")
        with self.assertRaisesRegex(provenance.ProvenanceError, "runtime_generation"):
            self.verify()
        changed_lifecycle = self.write(
            "other-lifecycle.json",
            {"kind": "restore-runtime-lifecycle", "generations": [{"number": 2}]},
        )
        second = (*self.control_result[:3], (
            self.registration,
            self.reference,
            self.target,
            self.target,
            changed_lifecycle,
            self.launch,
        ))
        self.bind.side_effect = [self.control_result, second]
        with self.assertRaisesRegex(provenance.ProvenanceError, "runtime_generation"):
            self.verify()

    def test_a_b_a_replacement_during_live_probe_is_rejected(self):
        original = self.target.path.read_bytes()

        def replace_and_restore(*_args, **_kwargs):
            self.target.path.write_bytes(b'{"kind":"B"}')
            self.target.path.write_bytes(original)
            return self.runtime_verification

        self.live.side_effect = replace_and_restore
        with self.assertRaisesRegex(provenance.ProvenanceError, "runtime_generation"):
            self.verify()

    def test_scope_endpoints_and_source_fingerprints_are_bound(self):
        candidates = []
        for field, value in (
            ("scope_id", "other-scope"),
            ("api_url", "http://127.0.0.1:18083"),
            ("frontend_url", "http://127.0.0.1:14175"),
        ):
            changed = copy.deepcopy(self.request)
            changed[field] = value
            candidates.append(changed)
        changed = copy.deepcopy(self.request)
        changed["metrics_urls"]["worker"] = "http://127.0.0.1:19094/metrics"
        candidates.append(changed)
        for candidate in candidates:
            with self.subTest(candidate=candidate), self.assertRaisesRegex(
                provenance.ProvenanceError, "runtime_endpoints"
            ):
                self.verify(candidate)
        changed = copy.deepcopy(self.request)
        changed["source_fingerprints"]["backend"] = "sha256:" + "0" * 64
        self.source.side_effect = ValueError("source changed")
        with self.assertRaisesRegex(provenance.ProvenanceError, "backend_source"):
            self.verify(changed)

    def test_secret_or_uncontrolled_paths_are_rejected_without_reading_them(self):
        secret = self.root / "passwords.json"
        secret.write_text('{"password":"secret"}', encoding="utf-8")
        changed = copy.deepcopy(self.request)
        changed["provenance"]["environment_document"] = str(secret)
        changed["environment_sha256"] = self.digest(secret)
        with self.assertRaisesRegex(provenance.ProvenanceError, "environment_document"):
            self.verify(changed)
        outside = self.write("outside-runtime.json", self.runtime_receipt, parent=self.root)
        changed = copy.deepcopy(self.request)
        changed["provenance"]["runtime_receipt"] = _descriptor(outside)
        with self.assertRaisesRegex(provenance.ProvenanceError, "runtime_receipts"):
            self.verify(changed)

    def test_source_and_document_changes_at_end_are_rejected(self):
        self.source_state.side_effect = [
            self.execution,
            self.runner_execution,
            self.execution,
            {**self.execution, "worktree_fingerprint": "sha256:" + "0" * 64},
        ]
        with self.assertRaisesRegex(provenance.ProvenanceError, "driver_source_stable"):
            self.verify()
        self.source_state.side_effect = self.execution_source
        self.environment.write_text("changed", encoding="utf-8")
        with self.assertRaisesRegex(provenance.ProvenanceError, "environment_document"):
            self.verify()

    def test_cli_failure_does_not_echo_private_input(self):
        result = subprocess.run(
            [sys.executable, "-B", "-X", "utf8", str(ROOT / "tools/python/devex_provenance.py")],
            input=json.dumps({"backend": "private-password=secret"}),
            capture_output=True,
            text=True,
            encoding="utf-8",
            timeout=10,
        )
        self.assertEqual(result.returncode, 1)
        self.assertEqual(json.loads(result.stdout), {"ok": False, "stage": "bindings"})
        self.assertEqual(result.stderr, "")


class FingerprintTests(unittest.TestCase):
    def test_xtask_length_framing_and_raw_git_order_include_untracked_content(self):
        with WorkspaceDirectory(ROOT / ".local-tests/python-unit") as directory:
            root = Path(directory).resolve()
            (root / "z.rs").write_bytes(b"z content")
            (root / "a.rs").write_bytes(b"a content")
            chunks = [b"a" * 40, b"binary patch\0", b"z.rs", b"z content", b"a.rs", b"a content"]
            expected = hashlib.sha256(
                b"".join(struct.pack("<Q", len(value)) + value for value in chunks)
            ).hexdigest()
            with patch.object(
                provenance, "git", side_effect=[chunks[1], b"z.rs\0a.rs\0"]
            ) as git:
                self.assertEqual(
                    provenance.worktree_fingerprint(root, SNAPSHOT["head"]),
                    "sha256:" + expected,
                )
            self.assertEqual(
                git.call_args_list[0].args[1:],
                ("diff", "--binary", "--no-ext-diff", "HEAD", "--", "."),
            )
            with patch.object(
                provenance, "git", side_effect=[b"", b"../outside\0"]
            ), self.assertRaises(ValueError):
                provenance.worktree_fingerprint(root, SNAPSHOT["head"])


class RuntimeChainIntegrationTests(unittest.TestCase):
    def setUp(self):
        self.fixture = runtime_tests.RestoreRuntimeTests("runTest")
        self.fixture.setUp()
        self.addCleanup(self.fixture.doCleanups)
        self.authority, self.receipt, bindings_path = self.fixture.bind_receipt()
        bindings = read_json_document(bindings_path).value
        target = {
            "product_plan": bindings["record"]["plan"],
            "product_execution": {
                "roots": {
                    "source_backend": str(self.fixture.backend),
                    "execution_backend": str(self.fixture.backend),
                    "frontend": str(self.fixture.frontend),
                },
                **{
                    key: self.authority[key]
                    for key in (
                        "backend_product_sha",
                        "backend_execution_sha",
                        "frontend_sha",
                    )
                },
                "adapter": None,
                "builds": {
                    role: _descriptor(
                        read_json_document(Path(self.receipt["paths"][key]))
                    )
                    for role, key in (
                        ("backend", "backend_build"),
                        ("frontend", "frontend_build"),
                    )
                },
            },
        }
        self.target = self.document("target-plan.json", target)
        self.registration = self.document(
            "runtime-registration.json", {"kind": "restore-runtime-registration"}
        )
        self.reference = self.document(
            "reference-plan.json",
            {
                "kind": "restore-reference-plan",
                "target": {"frontend_url": self.authority["frontend_endpoint"]},
            },
        )
        self.lifecycle = self.document(
            "lifecycle.json", {"kind": "restore-runtime-lifecycle"}
        )
        launch = read_json_document(self.fixture.launch_path).value
        launch["generation"] = 1
        launch["request"]["registration"] = {
            "registration": _descriptor(self.registration),
            "target_plan": _descriptor(self.target),
        }
        launch["request"]["artifacts"] = self.receipt["backend"]["artifacts"]
        for role in restore_runtime.ROLES:
            process = self.receipt["processes"][role]
            process_document = read_json_document(Path(process["receipt_path"]))
            launch["processes"][role]["process_receipt"] = {
                **_descriptor(process_document),
                "identity": process["identity"],
            }
        self.fixture.write_json(self.fixture.launch_path, launch)
        self.launch = read_json_document(self.fixture.launch_path)
        self.receipt["digests"]["launch"] = self.launch.sha256
        self.runtime = self.document("restore-runtime.json", self.receipt)
        self.documents = {
            "runtime_receipt": self.runtime,
            "target_plan": self.target,
            "runtime_registration": self.registration,
            "runtime_launch": self.launch,
        }
        self.observed = (
            self.registration,
            self.reference,
            self.target,
            self.target,
            self.lifecycle,
            self.launch,
        )
        self.control = (
            self.fixture.root,
            launch["request"]["registration"],
            launch,
            self.observed,
        )

    def document(self, name, value):
        return read_json_document(
            self.fixture.write_json(self.fixture.root / name, value)
        )

    def test_real_v3_static_and_live_validators_share_the_same_control_chain(self):
        with self.fixture.runtime_patches(fake_probes=True), patch.object(
            provenance, "_bind_control_inputs", return_value=self.control
        ), patch(
            "restore_runtime_static._bind_control_inputs",
            return_value=(
                self.fixture.root,
                self.control[1],
                self.control[2],
                (self.launch,),
            ),
        ):
            control, verified = provenance.verify_runtime_chain(
                self.fixture.backend,
                self.fixture.backend,
                self.fixture.frontend,
                self.documents,
            )
        self.assertEqual(control["generation"], 1)
        self.assertEqual(verified["status"], "verified")
        self.assertEqual(
            verified["runtime_receipt_sha256"], self.runtime.sha256
        )


if __name__ == "__main__":
    unittest.main()
