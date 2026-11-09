"""主机重启观察的纯单元测试；不启动服务、不读取真实账本。"""
from pathlib import Path
import shutil
import sys
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parent))

import devex_clone_seed_generation_reboot as reboot


class RebootObservationTests(unittest.TestCase):
    def setUp(self):
        parent = Path(__file__).resolve().parents[2] / ".local-tests/python-temp"
        parent.mkdir(parents=True, exist_ok=True)
        self.root = Path(parent / f"reboot-observation-{id(self)}")
        self.root.mkdir()
        self.addCleanup(shutil.rmtree, self.root, ignore_errors=True)

    def test_native_role_facts_survive_build_drift_and_keep_reboot_blockers_separate(self):
        start = {"number": 70, "result": {"sha256": "original-start"}}
        native = {"start": start, "later": [], "facts": {"output": str(self.root / "g0070")},
                  "prefix": {"producer": "original"}, "proof": {"source": "GetTickCount64"},
                  "roles": {"api": {"all_exited": True}, "worker": {"all_exited": True}},
                  "storage": {"exited": True}, "blockers": []}
        response = {"ok": True, "result": {"value": native}}
        process = reboot.subprocess.CompletedProcess([], 0)
        with patch.object(reboot, "native_invoke", return_value=(process, response)) as invoke, \
                patch.object(reboot, "build_context_state", return_value={"backend": {"matches": False}}), \
                patch.object(reboot, "plan_hash", return_value="frozen-history-hash"), \
                patch.object(reboot, "load_state", return_value={"attempts": [start]}):
            value = reboot._observation(self.root, self.root, live_cache=False)
        self.assertTrue(value["reboot_verified"])
        self.assertFalse(value["value"]["closure_ready"])
        self.assertEqual(value["value"]["roles"], native["roles"])
        self.assertEqual(value["proof"], native["proof"])
        self.assertEqual(value["value"]["storage"], native["storage"])
        self.assertEqual(value["reboot_blockers"], [])
        self.assertEqual(invoke.call_count, 1)
        self.assertEqual(invoke.call_args.args[:2], (
            "__source-evidence", {"operation": "source-generation-reboot-observation", "write": False,
                "arguments": {"backend": str(self.root.resolve()), "directory": str(self.root.resolve())}}))
        self.assertEqual(invoke.call_args.kwargs, {"may_write": False})

    def test_native_observation_failure_and_cancellation_propagate_without_replay(self):
        for error in (ValueError("原身份缺失"), TimeoutError("观察未知"), KeyboardInterrupt()):
            with self.subTest(error=type(error).__name__), \
                    patch.object(reboot, "native_invoke", side_effect=error) as invoke, \
                    patch.object(reboot, "cache_generations") as cache, \
                    patch.object(reboot, "build_context_state") as build:
                with self.assertRaises(type(error)) as caught:
                    reboot.observe(self.root, self.root)
                self.assertIs(caught.exception, error)
                self.assertEqual(invoke.call_count, 1)
                cache.assert_not_called()
                build.assert_not_called()

    def test_cache_generations_require_stopped_linux_identity(self):
        status = {"status": "cache_status", "request": {"path": "cache.json"},
                  "processes": [
                      {"attempt": 66, "process": {"state": "stopped",
                                                  "linux": {"alive": False,
                                                            "identity": {"boot_id": "old-boot"},
                                                            "boot_id": "new-boot"}}},
                      {"attempt": 67, "process": {"state": "running",
                                                  "linux": {"alive": True,
                                                            "identity": {"boot_id": "new-boot"},
                                                            "boot_id": "new-boot"}}}]}
        with patch("devex_clone_cache.cache_status", return_value=status):
            value, blockers = reboot.cache_generations(Path("."), Path("."))
        self.assertEqual([row["attempt"] for row in value["generations"]], [66, 67])
        self.assertTrue(value["generations"][0]["boot_id_changed"])
        self.assertEqual(blockers, ["缓存代次 67 没有明确退出证据"])

    def test_closure_preflight_requires_verified_reboot_and_same_request(self):
        request = self.root / "generation-request.json"
        request.write_text("{}", encoding="utf-8")
        descriptor = reboot.binding(request)
        facts = {"receipt": {"request": descriptor}}
        verified = {"reboot_verified": True, "reboot_blockers": [], "value": {"service_attempts": []},
                    "facts": facts}
        with patch.object(reboot, "load_state", return_value={"attempts": []}), \
                patch.object(reboot, "closure_record", return_value=None), \
                patch.object(reboot, "_observation", return_value=verified):
            self.assertEqual(reboot.preflight(Path("."), self.root, request), verified)
        blocked = dict(verified, reboot_verified=False, reboot_blockers=["角色仍在世"])
        with patch.object(reboot, "load_state", return_value={"attempts": []}), \
                patch.object(reboot, "closure_record", return_value=None), \
                patch.object(reboot, "_observation", return_value=blocked):
            with self.assertRaises(ValueError):
                reboot.preflight(Path("."), self.root, request)
        other = self.root / "other-request.json"
        other.write_text("{}", encoding="utf-8")
        with patch.object(reboot, "load_state", return_value={"attempts": []}), \
                patch.object(reboot, "closure_record", return_value=None), \
                patch.object(reboot, "_observation", return_value=verified):
            with self.assertRaises(ValueError):
                reboot.preflight(Path("."), self.root, other)
        with self.assertRaises(ValueError):
            reboot.preflight(Path("."), self.root, None)

    def test_closure_preflight_rejects_service_generations(self):
        request = self.root / "generation-request.json"
        request.write_text("{}", encoding="utf-8")
        value = {"reboot_verified": True, "reboot_blockers": [],
                 "value": {"service_attempts": [71]},
                 "facts": {"receipt": {"request": reboot.binding(request)}}}
        with patch.object(reboot, "load_state", return_value={"attempts": []}), \
                patch.object(reboot, "closure_record", return_value=None), \
                patch.object(reboot, "_observation", return_value=value):
            with self.assertRaises(ValueError):
                reboot.preflight(Path("."), self.root, request)


if __name__ == "__main__":
    unittest.main()
