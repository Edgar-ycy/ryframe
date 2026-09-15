"""固定 C69 最终复核失败的只读授权回归。"""

from __future__ import annotations

import copy
from contextlib import ExitStack, contextmanager
import hashlib
from pathlib import Path
import sys
from types import SimpleNamespace
import unittest
from unittest import TestCase
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
sys.path.insert(0, str(Path(__file__).resolve().parent))

from workspace_directory import WorkspaceDirectory
from devex_clone_capture import read_json, write_json
from devex_clone_run_state import binding
import devex_clone_seed_generation_control as control
import devex_clone_seed_generation_lineage_recovery as recovery
import devex_clone_seed_generation_lineage_retry as retry


class FailedAuthorizationTests(TestCase):
    def setUp(self):
        self.backend = Path(__file__).resolve().parents[2]
        temporary = WorkspaceDirectory(
            dir=self.backend / ".local-tests/tmp", prefix="lineage-retry-"
        )
        self.addCleanup(temporary.cleanup)
        self.directory = Path(temporary.name).resolve()
        (self.directory / "results").mkdir()
        output = self.directory / "seed-runtime/attempt-0069"
        (output / "runtime").mkdir(parents=True)
        (output / "after").mkdir()
        write_json(self.directory / "manifest.json", {"copy_stage": "source_to_seed"})

        self.build = self.file(
            "build.json",
            {
                "artifacts": {
                    "api": {"executable": "api.exe"},
                    "worker": {"executable": "worker.exe"},
                }
            },
        )
        self.execution = self.directory / "execution"
        maintenance_path = self.execution / ".local-tests/maintenance.json"
        maintenance_path.parent.mkdir(parents=True)
        write_json(maintenance_path, {"artifacts": {
            "reset": {"executable": "reset.exe"},
            "migrate": {"executable": "migrate.exe"},
        }})
        self.maintenance = binding(maintenance_path)
        self.request = {
            "execution_backend": str(self.execution),
            "backend_build": self.build,
            "maintenance_build": self.maintenance,
            "source_registration": {"path": "source"},
        }
        self.request_path = self.directory / "request.json"
        write_json(self.request_path, self.request)
        write_json(
            output / "runtime/binaries.json",
            {
                "ryframe": "api.exe",
                "ryframe-worker": "worker.exe",
                "ryframe-reset": "reset.exe",
                "ryframe-migrate": "migrate.exe",
            },
        )
        self.contract = {"worker_ready_url": "http://127.0.0.1:4"}
        write_json(output / "runtime/runtime.json", self.contract)
        write_json(output / "after/image.json", {"image": {"frozen": True}})

        self.product = {"sha256": "a" * 64}
        self.c68 = self.row(
            68,
            "source-generation-start",
            "failed",
            {"snapshot": {}, "fingerprints": {"product": self.product}},
        )
        self.sources = {
            "snapshot": {
                "head": retry.HEAD,
                "clean": True,
                "files": [],
                "patch_sha256": hashlib.sha256(b"").hexdigest(),
            },
            "fingerprints": {"product": self.product},
        }
        self.c69 = self.row(69, "source-generation-recover", "failed", self.sources)
        self.attempts = [self.c68, self.c69]
        write_json(
            self.directory / "state.json",
            {
                "format_version": 1,
                "manifest": binding(self.directory / "manifest.json"),
                "attempts": self.attempts,
            },
        )
        self.controller_path = self.directory / "controller-0069.json"
        write_json(self.controller_path, {"controller": True})
        self.controller = binding(self.controller_path)
        self.owner = {
            "format_version": 1,
            "identity": {
                "pid": 9009,
                "started": "1",
                "executable": str(self.backend / "python.exe"),
            },
            "directory": str(self.directory),
            "manifest_sha256": binding(self.directory / "manifest.json")["sha256"],
        }
        write_json(
            self.directory / "failure-0069.json",
            {
                "format_version": 1,
                "kind": "devex-stage-failure",
                "attempt": 69,
                "stage": "seed-runtime",
                "mode": "source-generation-recover",
                "error_type": "ValueError",
                "frames": retry.FRAMES,
                "controller": self.controller,
            },
        )
        self.facts = {
            "proof": {"fixed": "c68-proof"},
            "failed": self.c68,
            "request": self.request,
            "historical_request": self.request,
            "request_descriptor": binding(self.request_path),
            "source": {"request": {"source": {}}},
            "contract": self.contract,
            "before": {"path": "c68-before"},
            "image": {"frozen": True},
            "records": (self.c68,),
        }

    def file(self, name, value):
        path = self.directory / name
        write_json(path, value)
        return binding(path)

    @staticmethod
    def row(number, mode, status, sources):
        return {
            "number": number,
            "stage": "seed-runtime",
            "mode": mode,
            "started_at": "2026-09-16T00:00:00Z",
            "finished_at": "2026-09-16T00:01:00Z",
            "status": status,
            "sources": sources,
            "result": None,
            "error_type": "ValueError" if status == "failed" else None,
        }

    def runtime(self, *_args):
        @contextmanager
        def environment():
            yield {}

        return SimpleNamespace(
            execution=self.backend,
            selected={},
            operations={},
            contract=None,
            environment=environment,
        )

    @contextmanager
    def authority_patches(
        self, *, image=None, manifest=None, state_attempts=None, repeated=None,
        owner_error=None,
    ):
        image = self.facts["image"] if image is None else image
        state_attempts = self.attempts if state_attempts is None else state_attempts
        request_binding = binding(self.request_path)
        with ExitStack() as stack:
            stack.enter_context(patch.object(retry, "REQUEST_RELATIVE", self.request_path.relative_to(self.backend)))
            stack.enter_context(patch.object(retry, "REQUEST_BYTES", request_binding["bytes"]))
            stack.enter_context(patch.object(retry, "REQUEST_SHA256", request_binding["sha256"]))
            stack.enter_context(patch.object(retry, "TREE", "b" * 40))
            stack.enter_context(
                patch.object(
                    retry,
                    "load_state",
                    return_value={"format_version": 1, "attempts": state_attempts},
                )
            )
            stack.enter_context(patch.object(retry, "verify_execution_source"))
            stack.enter_context(patch.object(retry, "git", return_value=b"b" * 40 + b"\n"))
            stack.enter_context(
                patch.object(retry, "controller_record", return_value=(self.controller, self.owner))
            )
            stopped = stack.enter_context(patch.object(retry, "require_recorded_producer_stopped"))
            lineage = stack.enter_context(
                patch.object(
                    recovery,
                    "lineage_failure",
                    side_effect=[self.facts, repeated or self.facts],
                )
            )
            owned = stack.enter_context(
                patch("devex_clone_run._require_owned_run", side_effect=owner_error)
            )
            stack.enter_context(
                patch.object(recovery, "_manifest_summary", return_value=manifest or retry.MANIFEST)
            )
            stack.enter_context(
                patch("devex_clone_seed_generation_runtime.GenerationRuntime", side_effect=self.runtime)
            )
            stack.enter_context(patch("full_stack_runtime.verify_runtime", return_value=self.contract))
            stack.enter_context(
                patch("devex_clone_seed_generation_images.verify_image", return_value={"image": image})
            )
            yield lineage, stopped, owned

    def test_derives_read_only_authority_from_exact_failed_evidence(self):
        before = binding(self.directory / "state.json")
        with self.authority_patches() as (lineage, stopped, owned):
            value = retry.authority(self.backend, self.directory, self.attempts)
        self.assertEqual(value["records"], (self.c68, self.c69))
        self.assertEqual(value["request"], binding(self.request_path))
        self.assertEqual(value["receipt"], None)
        self.assertEqual(value["image"], self.facts["image"])
        self.assertEqual(binding(self.directory / "state.json"), before)
        self.assertFalse((self.directory / "results/0069.json").exists())
        self.assertEqual(lineage.call_count, 2)
        stopped.assert_called_once()
        owned.assert_not_called()

    def test_accepts_only_exact_owned_running_c70_successor(self):
        c70 = self.row(70, "source-generation-start", "running", self.sources)
        c70.update(finished_at=None, result=None, error_type=None)
        current = [*self.attempts, c70]
        with self.authority_patches(state_attempts=current) as (_, _, owned):
            value = retry.authority(self.backend, self.directory, current)
        self.assertEqual(value["records"], (self.c68, self.c69))
        self.assertEqual(owned.call_count, 2)
        self.assertTrue(all(call.args == (self.directory,) for call in owned.call_args_list))

        for field, changed in (
            ("finished_at", "done"),
            ("result", {"path": "result"}),
            ("error_type", "ValueError"),
        ):
            invalid = copy.deepcopy(current)
            invalid[-1][field] = changed
            with self.subTest(field=field), self.authority_patches(state_attempts=invalid), \
                    self.assertRaisesRegex(ValueError, "C69 后只允许"):
                retry.authority(self.backend, self.directory, invalid)

        with self.authority_patches(
            state_attempts=current, owner_error=ValueError("wrong owner")
        ), self.assertRaisesRegex(ValueError, "wrong owner"):
            retry.authority(self.backend, self.directory, current)

    def test_completed_c70_reuses_its_request_without_rescanning_c69(self):
        result_path = self.directory / "results/0070.json"
        write_json(result_path, {"request": binding(self.request_path)})
        c70 = self.row(70, "source-generation-start", "passed", self.sources)
        c70.update(result=binding(result_path), error_type=None)
        current = [*self.attempts, c70]
        with self.authority_patches(state_attempts=current) as (lineage, _, owned):
            value = retry.completed(self.backend, self.directory, current)
        self.assertEqual(value["records"], (self.c68, self.c69))
        self.assertEqual(value["completed_successor"], c70)
        self.assertEqual(value["request"], binding(self.request_path))
        lineage.assert_not_called()
        owned.assert_not_called()

        changed = read_json(result_path)
        changed["request"] = {"path": "wrong", "bytes": 1, "sha256": "0" * 64}
        result_path.unlink()
        write_json(result_path, changed)
        c70["result"] = binding(result_path)
        current = [*self.attempts, c70]
        with self.authority_patches(state_attempts=current), \
                self.assertRaisesRegex(ValueError, "同一请求"):
            retry.completed(self.backend, self.directory, current)

    def test_rejects_repeated_lineage_drift(self):
        changed = copy.deepcopy(self.facts)
        changed["image"] = {"frozen": False}
        with self.authority_patches(repeated=changed), \
                self.assertRaisesRegex(ValueError, "核验期间"):
            retry.authority(self.backend, self.directory, self.attempts)

    def test_rejects_failure_manifest_image_and_unknown_file_drift(self):
        failure_path = self.directory / "failure-0069.json"
        changed = read_json(failure_path)
        changed["frames"] = changed["frames"][:-1]
        failure_path.unlink()
        write_json(failure_path, changed)
        with self.authority_patches(), self.assertRaisesRegex(ValueError, "失败栈"):
            retry.authority(self.backend, self.directory, self.attempts)

        changed["frames"] = retry.FRAMES
        failure_path.unlink()
        write_json(failure_path, changed)
        with self.authority_patches(manifest={**retry.MANIFEST, "files": 1}), \
                self.assertRaisesRegex(ValueError, "失败目录"):
            retry.authority(self.backend, self.directory, self.attempts)

        extra = self.directory / "seed-runtime/attempt-0069/unknown"
        extra.mkdir()
        with self.authority_patches(), self.assertRaisesRegex(ValueError, "失败目录"):
            retry.authority(self.backend, self.directory, self.attempts)
        extra.rmdir()

        with self.authority_patches(image={"changed": True}), \
                self.assertRaisesRegex(ValueError, "完整后像"):
            retry.authority(self.backend, self.directory, self.attempts)

        unexpected = self.directory / "g0069"
        unexpected.mkdir()
        with self.authority_patches(), self.assertRaises(ValueError):
            retry.authority(self.backend, self.directory, self.attempts)

    def test_failure_numbers_must_be_json_integers(self):
        failure_path = self.directory / "failure-0069.json"
        original = read_json(failure_path)
        for field in ("format_version", "attempt"):
            changed = copy.deepcopy(original)
            changed[field] = float(changed[field])
            failure_path.unlink()
            write_json(failure_path, changed)
            with self.subTest(field=field), self.authority_patches(), \
                    self.assertRaisesRegex(ValueError, "失败栈"):
                retry.authority(self.backend, self.directory, self.attempts)
        failure_path.unlink()
        write_json(failure_path, original)

    def test_rejects_request_and_state_drift(self):
        with self.authority_patches():
            self.request_path.write_text("{}", encoding="utf-8")
            with self.assertRaisesRegex(ValueError, "正式请求变化"):
                retry.authority(self.backend, self.directory, self.attempts)

        self.request_path.unlink()
        write_json(self.request_path, self.request)
        changed = copy.deepcopy(self.attempts)
        changed[0]["status"] = "passed"
        with self.authority_patches(state_attempts=self.attempts), \
                self.assertRaisesRegex(ValueError, "当前账本"):
            retry.authority(self.backend, self.directory, changed)


class RoutingTests(TestCase):
    def setUp(self):
        self.backend = Path(__file__).resolve().parents[2]
        self.directory = self.backend / ".local-tests/copy-run"
        self.c68 = {"number": 68, "stage": "seed-runtime", "mode": "source-generation-start"}
        self.c69 = {
            "number": 69,
            "stage": "seed-runtime",
            "mode": "source-generation-recover",
            "status": "failed",
            "result": None,
            "error_type": "ValueError",
        }

    def test_replay_routes_exact_failure_and_only_allows_c70_start(self):
        expected = {"record": self.c69, "records": (self.c68, self.c69)}
        with patch.object(retry, "authority", return_value=expected) as authority:
            self.assertIs(
                recovery.replay_authority(self.backend, self.directory, [self.c68, self.c69]),
                expected,
            )
        authority.assert_called_once()

        wrong = {"number": 70, "stage": "seed-runtime", "mode": "source-generation-stop"}
        with patch.object(retry, "authority", return_value=expected), \
                self.assertRaisesRegex(ValueError, "C69 后首个阶段"):
            recovery.replay_authority(self.backend, self.directory, [self.c68, self.c69, wrong])

        completed = {
            "number": 70,
            "stage": "seed-runtime",
            "mode": "source-generation-start",
            "status": "passed",
            "finished_at": "2026-09-16T01:00:00Z",
            "result": {"path": "result"},
            "error_type": None,
        }
        with patch.object(retry, "completed", return_value=expected) as terminal, \
                patch.object(retry, "authority") as live:
            self.assertIs(
                recovery.replay_authority(
                    self.backend, self.directory, [self.c68, self.c69, completed]
                ),
                expected,
            )
        terminal.assert_called_once()
        live.assert_not_called()

    def test_second_recover_is_rejected_before_status_or_capture(self):
        request = self.directory / "request.json"
        with patch.object(control, "load_state", return_value={"attempts": [self.c68, self.c69]}), \
                patch.object(retry, "pending", return_value=True), \
                patch("devex_clone_seed_generation_prelaunch.starts") as starts, \
                self.assertRaisesRegex(ValueError, "只能执行 C70 START"):
            control.preflight(self.directory, "source-generation-recover", backend=self.backend, request_path=request)
        starts.assert_not_called()

        with patch.object(control, "_active", return_value=[self.c68, self.c69]), \
                patch.object(recovery, "pending", return_value=False), \
                patch.object(retry, "pending", return_value=True), \
                patch.object(control, "status") as status, \
                self.assertRaisesRegex(ValueError, "禁止重复采集"):
            control.execute_recover(self.backend, self.directory, request, 70)
        status.assert_not_called()


if __name__ == "__main__":
    unittest.main()
