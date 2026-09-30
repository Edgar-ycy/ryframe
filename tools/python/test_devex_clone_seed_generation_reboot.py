"""主机重启观察的纯单元测试；不启动服务、不读取真实账本。"""
from pathlib import Path
import shutil
import sys
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parent))

import devex_clone_seed_generation_reboot as reboot
import host_boot
import restore_source_runtime as runtime

OBSERVED = 130_000_000_000_000_000
BOOT = OBSERVED - 3_600 * host_boot.TICKS_PER_SECOND


def proof():
    return host_boot.host_boot_proof(margin_seconds=300, observed=OBSERVED, uptime=3600.0)


def identity(started, pid=4242):
    return {"pid": pid, "started": str(started), "executable": "C:/bin/ryframe.exe"}


class RebootObservationTests(unittest.TestCase):
    def setUp(self):
        parent = Path(__file__).resolve().parents[2] / ".local-tests/python-temp"
        parent.mkdir(parents=True, exist_ok=True)
        self.root = Path(parent / f"reboot-observation-{id(self)}")
        self.root.mkdir()
        self.addCleanup(shutil.rmtree, self.root, ignore_errors=True)
        self.verification = self.root / "verification"
        for name in ("before", "audit", runtime.STAGING_DIRECTORY):
            (self.verification / name).mkdir(parents=True, exist_ok=True)
        for name in (runtime.INTENT, runtime.PROCESS, runtime.READY, runtime.STDOUT,
                     runtime.STDERR, runtime.COMPLETION):
            (self.verification / name).write_text("{}", encoding="utf-8")
        (self.verification / "failed.json").write_text(
            '{"status": "failed", "error": "ValueError"}', encoding="utf-8")
        (self.verification / "audit/login-audit.json").write_text("{}", encoding="utf-8")
        (self.verification / "before/image.json").write_text("{}", encoding="utf-8")

    def test_failed_prefix_accepts_only_the_exact_interrupted_shape(self):
        value = reboot.failed_prefix(self.verification)
        self.assertEqual(value["manifest"], sorted(runtime.FAILED_ROOT_ENTRIES))
        self.assertEqual(value["producer"]["intent"]["bytes"], 2)
        (self.verification / "after").mkdir()
        with self.assertRaises(ValueError):
            reboot.failed_prefix(self.verification)
        shutil.rmtree(self.verification / "after")
        (self.verification / "failed.json").write_text(
            '{"status": "failed", "error": "CalledProcessError"}', encoding="utf-8")
        with self.assertRaises(ValueError):
            reboot.failed_prefix(self.verification)

    def test_role_evidence_requires_every_member_before_boot(self):
        margin = 300 * host_boot.TICKS_PER_SECOND
        tree = {"operation_id": "0" * 32,
                "process": identity(BOOT - margin - 1, pid=11),
                "supervisor": identity(BOOT - margin - 1, pid=12),
                "monitor": identity(BOOT - margin - 1, pid=13)}
        self.assertTrue(reboot.role_evidence(tree, proof())["all_exited"])
        tree["monitor"] = identity(BOOT + margin + 1, pid=13)
        self.assertFalse(reboot.role_evidence(tree, proof())["all_exited"])

    def test_port_evidence_only_reports_explicit_closures(self):
        def closed(url):
            if url.endswith("9999"):
                raise ValueError("端口仍在监听")

        with patch.object(reboot, "require_closed_port", closed):
            rows = reboot.port_evidence(["http://127.0.0.1:18210", "http://127.0.0.1:9999"])
        self.assertEqual(rows[0], {"url": "http://127.0.0.1:18210", "closed": True})
        self.assertEqual(rows[1], {"url": "http://127.0.0.1:9999", "closed": False,
                                   "error_type": "ValueError"})

    def test_storage_generation_reports_exit_and_ports(self):
        created = BOOT - 300 * host_boot.TICKS_PER_SECOND - 1
        facts = {"request": {"current_storage": {
            "attempt": 65, "api_url": "http://127.0.0.1:29200", "console_url": "http://127.0.0.1:29201",
            "identity": identity(created, pid=7388),
            "storage": {"identity": identity(created, pid=7388)},
            "request": {"path": "request.json", "bytes": 1, "sha256": "0" * 64}}}}
        with patch.object(reboot, "require_closed_port", lambda url: None):
            value, blockers = reboot.storage_generation(facts, proof())
        self.assertTrue(value["exited"])
        self.assertEqual(blockers, [])

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

    def test_start_record_rejects_running_stage(self):
        with patch.object(reboot, "load_state",
                          return_value={"attempts": [{"status": "running", "number": 71,
                                                      "stage": "storage-target", "mode": "restart"}]}):
            with self.assertRaises(ValueError):
                reboot.start_record(Path("."), self.root)

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
