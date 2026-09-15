"""C68 谱系失败的只读封存、C69 授权和唯一 C70 重试。"""
from contextlib import ExitStack, contextmanager
import copy
import hashlib
from pathlib import Path
import sys
from types import SimpleNamespace
import unittest
from unittest.mock import Mock, patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
sys.path.insert(0, str(Path(__file__).resolve().parent))

from workspace_directory import WorkspaceDirectory
import devex_clone_seed_generation as generation
import devex_clone_seed_generation_control as control
import devex_clone_seed_generation_lineage_recovery as recovery
import devex_clone_seed_generation_prelaunch as prelaunch
import devex_clone_seed_segment as segment
from devex_clone_capture import read_json, write_json
from devex_clone_run_state import binding
from restore_reference_plan import plan_hash


class LineageRecoveryTests(unittest.TestCase):
    def setUp(self):
        self.backend = Path(__file__).resolve().parents[2]
        temporary = WorkspaceDirectory(
            dir=self.backend / ".local-tests/tmp", prefix="lineage-recovery-"
        )
        self.addCleanup(temporary.cleanup)
        self.directory = Path(temporary.name).resolve()
        (self.directory / "results").mkdir()
        write_json(self.directory / "manifest.json", {"copy_stage": "source_to_seed"})
        self.output = self.directory / "g0068"
        (self.output / "before").mkdir(parents=True)
        (self.output / "runtime").mkdir()
        write_json(self.output / "before/image.json", {"image": {"frozen": True}})
        write_json(self.output / "runtime/runtime.json", {"worker_ready_url": "http://127.0.0.1:1"})
        write_json(self.output / "runtime/binaries.json", {"placeholder": True})
        self.request = {
            "source_registration": {"path": "source"},
            "source_rebind": {"path": "rebind"},
            "review_successor": {"path": "successor"},
            "current_storage": {"attempt": 65},
        }
        write_json(self.output / "request.json", self.request)
        self.operations = {"api": "0" * 32, "worker": "1" * 32}
        write_json(
            self.output / "intent.json",
            {
                "format_version": 1,
                "kind": "seed-source-runtime-intent",
                "request": binding(self.output / "request.json"),
                "runtime_directory": str(self.output / "runtime"),
                "operations": self.operations,
            },
        )
        self.source_info = {
            "snapshot": {
                "head": recovery.FAILED_HEAD,
                "clean": True,
                "files": [],
                "patch_sha256": hashlib.sha256(b"").hexdigest(),
            },
            "fingerprints": {"product": {"sha256": "a" * 64, "files": 1}},
        }
        self.failed = self.record(
            68, "seed-runtime", "source-generation-start", "failed", error="ValueError"
        )
        self.failed["sources"] = self.source_info
        self.tail = (self.failed,)
        self.controller_path = self.directory / "controller-0068.json"
        write_json(self.controller_path, {"controller": "fixed"})
        self.controller = binding(self.controller_path)
        self.owner = {
            "format_version": 1,
            "identity": {"pid": 6800},
            "directory": str(self.directory),
            "manifest_sha256": binding(self.directory / "manifest.json")["sha256"],
        }
        write_json(
            self.directory / "failure-0068.json",
            {
                "format_version": 1,
                "kind": "devex-stage-failure",
                "attempt": 68,
                "stage": "seed-runtime",
                "mode": "source-generation-start",
                "error_type": "ValueError",
                "frames": recovery.FAILED_FRAMES,
                "controller": self.controller,
            },
        )
        self.manifest_files = [{"path": "fixed", "bytes": 7, "sha256": "b" * 64}]
        self.source = {"request": {"source": {"scope_id": "source"}}}
        self.runtime = SimpleNamespace(selected={"api_url": "http://127.0.0.1:2"})
        self.contract = {"worker_ready_url": "http://127.0.0.1:3"}

    @staticmethod
    def record(number, stage, mode, status, result=None, *, error=None):
        return {
            "number": number,
            "stage": stage,
            "mode": mode,
            "status": status,
            "result": result,
            "error_type": error,
            "started_at": "before",
            "finished_at": "after",
            "sources": {"fixed": True},
        }

    @contextmanager
    def lineage_patches(
        self,
        *,
        git_values=None,
        controller_values=None,
        manifest_values=None,
        runtime_effect=None,
    ):
        git_values = git_values or [recovery.FAILED_TREE, recovery.FAILED_TREE]
        controller_values = controller_values or [
            (self.controller, self.owner),
            (self.controller, self.owner),
        ]
        manifest_values = manifest_values or [self.manifest_files, self.manifest_files]

        def runtime(*_args, **_kwargs):
            if runtime_effect is not None:
                runtime_effect()
            return self.runtime, self.contract

        with ExitStack() as stack:
            stack.enter_context(patch.object(recovery, "_history", return_value=({}, self.tail)))
            stack.enter_context(
                patch.object(
                    recovery,
                    "git",
                    side_effect=[(value + "\n").encode() for value in git_values],
                )
            )
            stack.enter_context(
                patch.object(recovery, "controller_record", side_effect=controller_values)
            )
            stack.enter_context(patch.object(recovery, "identity", side_effect=lambda value: value))
            stack.enter_context(patch.object(recovery, "require_recorded_producer_stopped"))
            stack.enter_context(
                patch("restore_source_runtime._manifest", side_effect=manifest_values)
            )
            stack.enter_context(patch.object(recovery, "FAILED_FILES", len(self.manifest_files)))
            stack.enter_context(
                patch.object(
                    recovery,
                    "FAILED_BYTES",
                    sum(item["bytes"] for item in self.manifest_files),
                )
            )
            stack.enter_context(
                patch.object(recovery, "FAILED_MANIFEST", plan_hash(self.manifest_files))
            )
            stack.enter_context(
                patch.object(
                    recovery,
                    "_request",
                    return_value=(
                        self.request,
                        binding(self.output / "request.json"),
                        binding(self.output / "request.json"),
                    ),
                )
            )
            stack.enter_context(patch.object(recovery, "_source", return_value=self.source))
            stack.enter_context(patch.object(recovery, "_verify_request"))
            stack.enter_context(patch.object(recovery, "_runtime", side_effect=runtime))
            stack.enter_context(
                patch(
                    "devex_clone_seed_generation_images.verify_image",
                    return_value={"image": {"frozen": True}},
                )
            )
            yield

    def test_pending_and_failed_tail_only_match_exact_c68_shape(self):
        self.assertTrue(recovery.pending([self.failed]))
        self.assertTrue(recovery.failed_tail(self.failed, 67))
        for field, value in (
            ("number", 69),
            ("stage", "other"),
            ("mode", "source-generation-stop"),
            ("status", "running"),
            ("result", {"published": True}),
            ("error_type", "RuntimeError"),
        ):
            changed = {**self.failed, field: value}
            with self.subTest(field=field):
                self.assertFalse(recovery.pending([changed]))
                self.assertFalse(recovery.failed_tail(changed, 67))
        self.assertFalse(recovery.pending([]))
        self.assertFalse(recovery.failed_tail(self.failed, 66))

    def test_lineage_failure_accepts_exact_readonly_evidence(self):
        before = {str(path): path.read_bytes() for path in self.directory.rglob("*") if path.is_file()}
        with self.lineage_patches():
            facts = recovery.lineage_failure(self.backend, self.directory, [self.failed])
        self.assertEqual(facts["failed"], self.failed)
        self.assertEqual(facts["image"], {"frozen": True})
        self.assertEqual(facts["proof"]["tree"], recovery.FAILED_TREE)
        self.assertEqual(
            before,
            {str(path): path.read_bytes() for path in self.directory.rglob("*") if path.is_file()},
        )

    def test_lineage_failure_rejects_tree_frames_and_controller_drift(self):
        with self.subTest("tree"):
            with self.lineage_patches(git_values=[recovery.FAILED_TREE, "c" * 40]), \
                    self.assertRaisesRegex(ValueError, "核验期间变化"):
                recovery.lineage_failure(self.backend, self.directory, [self.failed])

        failure_path = self.directory / "failure-0068.json"
        original = read_json(failure_path)
        changed = copy.deepcopy(original)
        changed["frames"][0]["line"] += 1
        failure_path.unlink()
        write_json(failure_path, changed)
        with self.subTest("frames"), self.lineage_patches(), \
                self.assertRaisesRegex(ValueError, "失败栈"):
            recovery.lineage_failure(self.backend, self.directory, [self.failed])
        failure_path.unlink()
        write_json(failure_path, original)

        changed = copy.deepcopy(original)
        changed["format_version"] = True
        failure_path.unlink()
        write_json(failure_path, changed)
        with self.subTest("format type"), self.lineage_patches(), \
                self.assertRaisesRegex(ValueError, "失败栈"):
            recovery.lineage_failure(self.backend, self.directory, [self.failed])
        failure_path.unlink()
        write_json(failure_path, original)

        other_path = self.directory / "controller-other.json"
        write_json(other_path, {"controller": "changed"})
        with self.subTest("controller"), self.lineage_patches(
            controller_values=[
                (self.controller, self.owner),
                (binding(other_path), self.owner),
            ]
        ), self.assertRaisesRegex(ValueError, "核验期间变化"):
            recovery.lineage_failure(self.backend, self.directory, [self.failed])

    def test_explicit_recovery_request_must_be_external_to_c68(self):
        request = {"id": "fixed-request"}
        request_path = self.output / "request.json"
        request_path.unlink()
        write_json(request_path, request)
        with patch.object(generation, "REQUEST_FIELDS", set(request)), \
                self.assertRaisesRegex(ValueError, "外部正式请求"):
            recovery._request(self.backend, self.output, request_path)

    def test_request_recalculation_uses_verified_source_without_published_history(self):
        maintenance_path = self.directory / "maintenance.json"
        environment_path = self.directory / "environment.json"
        write_json(maintenance_path, {"source": {"frozen": True}})
        write_json(environment_path, {"environment": {"SAFE": "value"}})
        source = {
            "review_successor": {"source_result": {"path": "registration"}},
            "review_successor_binding": {"path": "successor"},
            "source_rebind": {"path": "rebind"},
            "storage": {"storage": {"attempt": 65}},
            "generation": {"physical_binding": {"sha256": "physical"}},
            "request": {"source": {"scope_id": "source"}},
        }
        request = {
            "format_version": 1,
            "kind": "devex-clone-seed-source-generation",
            "id": "fixed-request",
            "source_registration": source["review_successor"]["source_result"],
            "review_successor": source["review_successor_binding"],
            "source_rebind": source["source_rebind"],
            "current_storage": source["storage"]["storage"],
            "execution_backend": str(self.backend),
            "expected_backend_sha": "a" * 40,
            "adapter_contract": None,
            "product_backend": None,
            "backend_build": {"path": "build"},
            "maintenance_build": binding(maintenance_path),
            "source_environment": binding(environment_path),
        }
        build = {"sources": {"full": {"source": {"frozen": True}}}}
        with patch(
            "devex_clone_seed_generation_runtime.registered_inputs",
            return_value=(self.backend, build),
        ), patch(
            "devex_clone_tools.verify_evidence",
            return_value={"source": {"frozen": True}},
        ), patch("process_environment.configured", return_value={"SAFE": "value"}), patch(
            "restore_source_binding.source_binding",
            return_value=source["generation"]["physical_binding"],
        ), patch("reference_fixture_successor_generation.rebuild") as rebuild:
            recovery._verify_request(self.backend, request, source)
        rebuild.assert_not_called()

    def test_lineage_failure_rejects_output_request_and_manifest_drift(self):
        extra = self.output / "unexpected.json"
        write_json(extra, {"unexpected": True})
        with self.subTest("output"), self.lineage_patches(), \
                self.assertRaisesRegex(ValueError, "启动前文件集合"):
            recovery.lineage_failure(self.backend, self.directory, [self.failed])
        extra.unlink()

        request_path = self.output / "request.json"

        def mutate_request():
            request_path.unlink()
            write_json(request_path, {"changed": True})

        with self.subTest("request"), self.lineage_patches(runtime_effect=mutate_request), \
                self.assertRaisesRegex(ValueError, "核验期间变化"):
            recovery.lineage_failure(self.backend, self.directory, [self.failed])
        request_path.unlink()
        write_json(request_path, self.request)

        changed_files = self.manifest_files + [
            {"path": "changed", "bytes": 1, "sha256": "d" * 64}
        ]
        with self.subTest("manifest"), self.lineage_patches(
            manifest_values=[self.manifest_files, changed_files]
        ), self.assertRaisesRegex(ValueError, "核验期间变化"):
            recovery.lineage_failure(self.backend, self.directory, [self.failed])


class AuthorizationTests(unittest.TestCase):
    def setUp(self):
        self.backend = Path(__file__).resolve().parents[2]
        temporary = WorkspaceDirectory(
            dir=self.backend / ".local-tests/tmp", prefix="lineage-authorize-"
        )
        self.addCleanup(temporary.cleanup)
        self.directory = Path(temporary.name).resolve()
        (self.directory / "results").mkdir()
        (self.directory / "seed-runtime").mkdir()
        self.request_path = self.directory / "request.json"
        self.maintenance = self.file(
            "maintenance.json",
            {
                "artifacts": {
                    "reset": {"executable": "reset.exe"},
                    "migrate": {"executable": "migrate.exe"},
                }
            },
        )
        self.request = {
            "source_registration": {"path": "source"},
            "source_rebind": {"path": "rebind"},
            "review_successor": {"path": "successor"},
            "current_storage": {"attempt": 65},
            "maintenance_build": self.maintenance,
        }
        write_json(self.request_path, self.request)
        self.failed_sources = {"fingerprints": {"product": {"sha256": "a" * 64}}}
        self.failed = {
            "number": 68,
            "stage": "seed-runtime",
            "mode": "source-generation-start",
            "status": "failed",
            "result": None,
            "error_type": "ValueError",
            "sources": self.failed_sources,
        }
        self.prefix = [self.failed]
        self.coordinator = copy.deepcopy(self.failed_sources)
        self.c68_runtime = self.directory / "g0068/runtime"
        self.c68_runtime.mkdir(parents=True)
        write_json(self.c68_runtime / "runtime.json", {"worker_ready_url": "http://127.0.0.1:4"})
        self.binaries = {
            "ryframe": "api.exe",
            "ryframe-worker": "worker.exe",
            "ryframe-reset": "reset.exe",
            "ryframe-migrate": "migrate.exe",
        }
        write_json(self.c68_runtime / "binaries.json", self.binaries)
        self.before = self.file("g0068/before/image.json", {"image": {"frozen": True}})
        self.contract = {"worker_ready_url": "http://127.0.0.1:4"}
        self.source = {"request": {"source": {"scope_id": "source"}}}
        self.facts = {
            "proof": {
                "runtime": binding(self.c68_runtime / "runtime.json"),
                "intent": {"path": "intent"},
            },
            "failed": self.failed,
            "request": self.request,
            "request_descriptor": binding(self.request_path),
            "source": self.source,
            "contract": self.contract,
            "before": self.before,
            "image": {"frozen": True},
        }
        self.runtimes = []

    def file(self, name, value):
        path = self.directory / name
        path.parent.mkdir(parents=True, exist_ok=True)
        write_json(path, value)
        return binding(path)

    def runtime_factory(self, _backend, _directory, output, request, source, _run):
        runtime = SimpleNamespace(
            execution=self.backend,
            selected={"api_url": "http://127.0.0.1:5"},
            build={
                "artifacts": {
                    "api": {"executable": "api.exe"},
                    "worker": {"executable": "worker.exe"},
                }
            },
            runtime=output / "runtime",
            request=request,
            source=source,
            control_environment={},
            preflight=Mock(),
            checkpoint=Mock(),
            prepare=Mock(),
            start=Mock(),
            stop=Mock(),
            retain=Mock(),
            finish=Mock(),
        )

        @contextmanager
        def environment():
            yield {"RYFRAME_TEST": "1"}

        runtime.environment = environment
        self.runtimes.append(runtime)
        return runtime

    @contextmanager
    def authorization_patches(self, *, image=None):
        image = {"frozen": True} if image is None else image

        def register(_execution, runtime_path):
            write_json(runtime_path / "runtime.json", self.contract)
            return self.contract

        def capture(_backend, _execution, _selected, _request, _source, _environment,
                    output, _run, **_kwargs):
            output.mkdir()
            write_json(output / "image.json", {"image": image})
            return binding(output / "image.json")

        with ExitStack() as stack:
            lineage = stack.enter_context(
                patch.object(recovery, "lineage_failure", side_effect=[self.facts, self.facts])
            )
            stack.enter_context(
                patch.object(recovery, "load_state", return_value={"attempts": [{"sources": self.coordinator}]})
            )
            stack.enter_context(
                patch.object(recovery, "require_current_execution_source", return_value=self.coordinator)
            )
            stack.enter_context(patch.object(control, "_active", return_value=self.prefix))
            stack.enter_context(
                patch("devex_clone_seed_generation_runtime.GenerationRuntime", side_effect=self.runtime_factory)
            )
            stack.enter_context(patch("full_stack_runtime.register_runtime", side_effect=register))
            ports = stack.enter_context(patch.object(recovery, "require_closed_port"))
            capture_mock = stack.enter_context(
                patch("devex_clone_seed_generation_images.capture_image", side_effect=capture)
            )
            stack.enter_context(
                patch(
                    "devex_clone_seed_generation_images.verify_image",
                    side_effect=lambda _backend, descriptor, *_args, **_kwargs: read_json(
                        Path(descriptor["path"])
                    ),
                )
            )
            yield lineage, capture_mock, ports

    def test_authorize_only_captures_and_never_controls_product_processes(self):
        with self.authorization_patches() as (lineage, capture, ports):
            value = recovery.authorize(
                self.backend, self.directory, self.request_path, 69, self.prefix
            )
        self.assertEqual(value["status"], recovery.STATUS)
        self.assertTrue(value["replay_allowed"])
        self.assertFalse(value["source_generation_published"])
        self.assertEqual(value["remote_writes"], 0)
        self.assertEqual(lineage.call_count, 2)
        capture.assert_called_once()
        self.assertEqual(ports.call_count, 2)
        runtime = self.runtimes[-1]
        runtime.preflight.assert_called_once()
        self.assertEqual(runtime.checkpoint.call_count, 3)
        for method in (runtime.prepare, runtime.start, runtime.stop, runtime.retain, runtime.finish):
            method.assert_not_called()
        self.assertEqual(
            {path.name for path in (self.directory / "seed-runtime/attempt-0069").iterdir()},
            {"runtime", "after"},
        )

    def test_authorize_rejects_image_drift_without_start_stop_or_publication(self):
        with self.authorization_patches(image={"changed": True}) as (_, capture, _), \
                self.assertRaisesRegex(ValueError, "完整像"):
            recovery.authorize(self.backend, self.directory, self.request_path, 69, self.prefix)
        capture.assert_called_once()
        runtime = self.runtimes[-1]
        for method in (runtime.prepare, runtime.start, runtime.stop, runtime.retain, runtime.finish):
            method.assert_not_called()
        self.assertFalse((self.directory / "results/0069.json").exists())

    def build_authority(self):
        output = self.directory / "seed-runtime/attempt-0069"
        (output / "runtime").mkdir(parents=True)
        (output / "after").mkdir()
        write_json(output / "runtime/binaries.json", self.binaries)
        write_json(output / "runtime/runtime.json", self.contract)
        write_json(output / "after/image.json", {"image": {"frozen": True}})
        record = {
            "number": 69,
            "stage": "seed-runtime",
            "mode": "source-generation-recover",
            "status": "passed",
            "result": None,
            "error_type": None,
            "sources": self.coordinator,
        }
        receipt = {
            "status": recovery.STATUS,
            "request": self.facts["request_descriptor"],
            "failed_start": 68,
            "failure_proof": self.facts["proof"],
            "history_length": 1,
            "history_sha256": plan_hash([self.failed]),
            "coordinator_source": self.coordinator,
            **{
                key: self.request[key]
                for key in (
                    "source_registration",
                    "source_rebind",
                    "review_successor",
                    "current_storage",
                )
            },
            "runtime": binding(output / "runtime/runtime.json"),
            "before": self.before,
            "after": binding(output / "after/image.json"),
            "evidence_manifest": recovery._manifest_summary(output),
            "source_generation_published": False,
            "replay_allowed": True,
            "remote_writes": 0,
            "restore_qualified": False,
        }
        result_path = self.directory / "results/0069.json"
        write_json(result_path, receipt)
        record["result"] = binding(result_path)
        return record, receipt

    @contextmanager
    def authority_patches(self):
        with ExitStack() as stack:
            stack.enter_context(patch.object(recovery, "lineage_failure", return_value=self.facts))
            stack.enter_context(patch.object(recovery, "verify_execution_source"))
            stack.enter_context(
                patch("devex_clone_seed_generation_runtime.GenerationRuntime", side_effect=self.runtime_factory)
            )
            stack.enter_context(
                patch("full_stack_runtime.verify_runtime", return_value=self.contract)
            )
            stack.enter_context(
                patch(
                    "devex_clone_seed_generation_images.verify_image",
                    side_effect=lambda _backend, descriptor, *_args, **_kwargs: read_json(
                        Path(descriptor["path"])
                    ),
                )
            )
            yield

    def test_replay_authority_rejects_bad_records_and_accepts_exact_receipt(self):
        self.assertIsNone(recovery.replay_authority(self.backend, self.directory, [self.failed]))
        failed_recovery = {
            "number": 69,
            "stage": "seed-runtime",
            "mode": "source-generation-recover",
            "status": "failed",
            "result": None,
            "error_type": "ValueError",
        }
        with self.assertRaisesRegex(ValueError, "失败或未发布"):
            recovery.replay_authority(
                self.backend, self.directory, [self.failed, failed_recovery]
            )
        with self.assertRaisesRegex(ValueError, "不能重复"):
            recovery.replay_authority(
                self.backend, self.directory, [self.failed, failed_recovery, failed_recovery]
            )

        record, _ = self.build_authority()
        bad_successor = {
            "number": 70,
            "stage": "seed-runtime",
            "mode": "source-generation-stop",
            "status": "passed",
        }
        with self.assertRaisesRegex(ValueError, "C69 后首个阶段"):
            recovery.replay_authority(
                self.backend, self.directory, [self.failed, record, bad_successor]
            )
        with self.authority_patches():
            value = recovery.replay_authority(
                self.backend, self.directory, [self.failed, record]
            )
        self.assertEqual(value["records"], (self.failed, record))
        self.assertEqual(value["image"], {"frozen": True})
        good_successor = {**bad_successor, "mode": "source-generation-start"}
        with self.authority_patches():
            value = recovery.replay_authority(
                self.backend, self.directory, [self.failed, record, good_successor]
            )
        self.assertEqual(value["record"], record)

        result_path = Path(record["result"]["path"])
        changed = read_json(result_path)
        changed["replay_allowed"] = False
        result_path.unlink()
        write_json(result_path, changed)
        record["result"] = binding(result_path)
        with self.authority_patches(), self.assertRaisesRegex(ValueError, "完整历史"):
            recovery.replay_authority(self.backend, self.directory, [self.failed, record])

    def test_replay_authority_rejects_loose_scalar_types_and_unknown_files(self):
        record, receipt = self.build_authority()
        result_path = Path(record["result"]["path"])
        invalid = {
            "failed_start": 68.0,
            "history_length": 1.0,
            "source_generation_published": 0,
            "replay_allowed": 1,
            "remote_writes": False,
            "restore_qualified": 0,
        }
        for field, value in invalid.items():
            changed = copy.deepcopy(receipt)
            changed[field] = value
            result_path.unlink()
            write_json(result_path, changed)
            record["result"] = binding(result_path)
            with self.subTest(field=field), self.authority_patches(), \
                    self.assertRaisesRegex(ValueError, "字段类型"):
                recovery.replay_authority(self.backend, self.directory, [self.failed, record])

        result_path.unlink()
        write_json(result_path, receipt)
        record["result"] = binding(result_path)
        write_json(self.directory / "seed-runtime/attempt-0069/after/unreferenced.json", {"extra": True})
        with self.authority_patches(), self.assertRaisesRegex(ValueError, "目录或证据绑定"):
            recovery.replay_authority(self.backend, self.directory, [self.failed, record])


class IntegrationTests(unittest.TestCase):
    def setUp(self):
        self.backend = Path(__file__).resolve().parents[2]
        temporary = WorkspaceDirectory(
            dir=self.backend / ".local-tests/tmp", prefix="lineage-integration-"
        )
        self.addCleanup(temporary.cleanup)
        self.directory = Path(temporary.name).resolve()
        self.source = {"fingerprints": {"product": {"sha256": "a" * 64}}}

    def row(self, number, stage, mode, status="passed", result=None, error=None):
        return {
            "number": number,
            "stage": stage,
            "mode": mode,
            "status": status,
            "result": result,
            "error_type": error,
            "sources": self.source,
        }

    def test_generation_preflight_requires_c69_and_closes_after_c70(self):
        rebound = self.row(52, "seed-runtime", "source-rebind")
        closed = self.row(60, "seed-runtime", "source-generation-recover")
        c68 = self.row(
            68,
            "seed-runtime",
            "source-generation-start",
            "failed",
            error="ValueError",
        )
        c69 = self.row(69, "seed-runtime", "source-generation-recover")
        c70 = self.row(70, "seed-runtime", "source-generation-start", "failed", error="ValueError")
        archive = {
            "records": (closed,),
            "recovery": closed,
            "receipt": {"source_registration": {"path": "C52"}},
        }

        def check(attempts, segment_value, failure=False):
            with patch.object(generation, "load_state", return_value={"attempts": attempts}), \
                    patch("devex_clone_seed_generation_prelaunch.closed", return_value=archive), \
                    patch("devex_clone_seed_segment.segmented_resume", return_value=segment_value):
                if failure:
                    with self.assertRaises(ValueError):
                        generation.preflight(self.directory, backend=self.backend)
                else:
                    generation.preflight(self.directory, backend=self.backend)

        ready = {"phase": "ready", "records": (), "storage": {"attempt": 65},
                 "cache": {"attempt": 66}, "replay": None}
        check([rebound, closed, c68], ready, failure=True)
        authorized = {**ready, "records": (c68, c69), "replay": {"record": c69}}
        check([rebound, closed, c68, c69], authorized)
        check([rebound, closed, c68, c69, c70], authorized, failure=True)

    def test_starts_only_hides_c68_after_valid_authority_and_keeps_c70(self):
        c60 = self.row(60, "seed-runtime", "source-generation-recover")
        c66 = self.row(66, "cache-target", "restart")
        c67 = self.row(
            67, "seed-runtime", "source-generation-start", "failed", error="ValueError"
        )
        c68 = self.row(
            68, "seed-runtime", "source-generation-start", "failed", error="ValueError"
        )
        c69 = self.row(69, "seed-runtime", "source-generation-recover")
        c70 = self.row(70, "seed-runtime", "source-generation-start")
        attempts = [c60, c66, c67, c68, c69, c70]
        archive = {"start": {"number": 1}}
        with patch.object(prelaunch, "closed", return_value=archive), \
                patch.object(prelaunch, "receipt_reregistration_failure") as old_failure, \
                patch.object(recovery, "replay_authority", return_value=None):
            self.assertEqual(prelaunch.starts(self.backend, self.directory, attempts), [c68, c70])
        old_failure.assert_called_once_with(self.backend, self.directory, c67, c66)
        replay = {"failed": c68, "record": c69, "records": (c68, c69)}
        with patch.object(prelaunch, "closed", return_value=archive), \
                patch.object(prelaunch, "receipt_reregistration_failure"), \
                patch.object(recovery, "replay_authority", return_value=replay):
            self.assertEqual(prelaunch.starts(self.backend, self.directory, attempts), [c70])

    def test_segment_consumes_c68_c69_and_exposes_replay_to_c70(self):
        recovery60 = self.row(60, "seed-runtime", "source-generation-recover")
        cache_stop = self.row(61, "cache-target", "stop")
        storage_failed = self.row(62, "storage-target", "stop", "failed", error="ValueError")
        storage_stop = self.row(63, "storage-target", "stop")
        storage_restart_failed = self.row(
            64, "storage-target", "restart", "failed", error="CalledProcessError"
        )
        storage_restart = self.row(65, "storage-target", "restart", result={"storage": 65})
        cache_restart = self.row(66, "cache-target", "restart", result={"cache": 66})
        c67 = self.row(
            67, "seed-runtime", "source-generation-start", "failed", error="ValueError"
        )
        c68 = self.row(
            68, "seed-runtime", "source-generation-start", "failed", error="ValueError"
        )
        c69 = self.row(69, "seed-runtime", "source-generation-recover", result={"replay": 69})
        attempts = [
            recovery60,
            cache_stop,
            storage_failed,
            storage_stop,
            storage_restart_failed,
            storage_restart,
            cache_restart,
            c67,
            c68,
            c69,
        ]
        (self.directory / "g0068").mkdir()
        evidence_a = self.file("failure.json", {"fixed": "failure"})
        evidence_b = self.file("controller.json", {"fixed": "controller"})
        storage = {"attempt": 65, "restart_result": storage_restart["result"]}
        cache = {
            "attempt": 66,
            "restart_result": cache_restart["result"],
            "request": {"cache": True},
        }
        archive = {
            "recovery": recovery60,
            "receipt": {
                "current_storage": {"attempt": 53},
                "review_successor": {"path": "successor"},
            },
        }
        replay = {
            "failed": c68,
            "record": c69,
            "records": (c68, c69),
            "receipt": {"request": {"path": "request"}},
            "image": {"frozen": True},
        }
        tail = Mock(return_value=None)
        with ExitStack() as stack:
            stack.enter_context(patch.object(segment, "_same_product_source"))
            stack.enter_context(patch.object(segment, "_require_clean_source"))
            stack.enter_context(
                patch.object(
                    segment,
                    "_verify_cache_stop",
                    return_value={"request": cache["request"], "result": {}},
                )
            )
            stack.enter_context(
                patch.object(
                    segment,
                    "_verify_storage_stop",
                    return_value={
                        "request": {"storage": True},
                        "result": {"processes": [{"attempt": 1}]},
                    },
                )
            )
            stack.enter_context(
                patch.object(
                    segment,
                    "_verify_failed_storage_stop",
                    return_value=(evidence_a, evidence_b),
                )
            )
            stack.enter_context(
                patch.object(
                    segment,
                    "_verify_failed_storage_restart",
                    return_value=(evidence_a, evidence_b),
                )
            )
            stack.enter_context(
                patch.object(segment, "registered_storage_binding", return_value=storage)
            )
            stack.enter_context(patch("devex_clone_seed_rebind.transition"))
            stack.enter_context(
                patch("devex_clone_cache.seed_history_proof", return_value={"path": "successor"})
            )
            stack.enter_context(
                patch("devex_clone_cache.registered_cache_binding", return_value=cache)
            )
            stack.enter_context(
                patch("devex_clone_seed_generation_prelaunch.receipt_reregistration_failure")
            )
            stack.enter_context(patch.object(recovery, "replay_authority", return_value=replay))
            stack.enter_context(patch.object(segment, "_verify_generation_tail", tail))
            value = segment.segmented_resume(
                self.backend,
                self.directory,
                attempts,
                archive,
                {"path": "C52"},
            )
        self.assertEqual(value["replay"], replay)
        self.assertIn(c68, value["records"])
        self.assertIn(c69, value["records"])
        tail.assert_called_once_with([], 69)

    def file(self, name, value):
        path = self.directory / name
        write_json(path, value)
        return binding(path)

    def test_control_delegates_exact_c69_without_running_old_recovery(self):
        c68 = self.row(
            68,
            "seed-runtime",
            "source-generation-start",
            "failed",
            error="ValueError",
        )
        expected = {"status": recovery.STATUS}
        with patch.object(control, "_active", return_value=[c68]), \
                patch.object(recovery, "authorize", return_value=expected) as authorize, \
                patch.object(control, "status") as status:
            value = control.execute_recover(
                self.backend, self.directory, self.directory / "request.json", 69
            )
        self.assertEqual(value, expected)
        authorize.assert_called_once()
        status.assert_not_called()


if __name__ == "__main__":
    unittest.main()
