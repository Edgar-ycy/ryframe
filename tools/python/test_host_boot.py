"""主机启动证明的纯单元测试；不查询真实进程或系统启动时刻。"""
from datetime import timedelta
from pathlib import Path
import sys
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parent))

import host_boot

OBSERVED = 130_000_000_000_000_000
BOOT = OBSERVED - 3_600 * host_boot.TICKS_PER_SECOND


class HostBootTests(unittest.TestCase):
    def proof(self):
        return host_boot.host_boot_proof(margin_seconds=300, observed=OBSERVED, uptime=3600.0)

    def identity(self, started, pid=4242):
        return {"pid": pid, "started": str(started), "executable": "C:/bin/ryframe.exe"}

    def test_proof_derives_boot_tick_and_time(self):
        proof = self.proof()
        self.assertEqual(proof["boot_ticks"], BOOT)
        self.assertEqual(proof["margin_seconds"], 300)
        self.assertEqual(proof["uptime_ticks"], 3_600 * host_boot.TICKS_PER_SECOND)
        self.assertEqual(proof["boot_time"],
                         (host_boot.EPOCH + timedelta(microseconds=BOOT // 10)).isoformat())
        self.assertEqual(host_boot.verify_host_boot_proof(proof, observed=OBSERVED), proof)

    def test_proof_rejects_invalid_injected_evidence(self):
        for margin, observed, uptime in ((True, OBSERVED, 3600.0), (-1, OBSERVED, 3600.0),
                                        (300, 0, 3600.0), (300, OBSERVED, 0),
                                        (300, OBSERVED, "3600"), (300, 1000, 10 ** 9)):
            with self.subTest(margin=margin, observed=observed, uptime=uptime):
                with self.assertRaises(ValueError):
                    host_boot.host_boot_proof(margin_seconds=margin, observed=observed, uptime=uptime)

    def test_proof_revalidation_rejects_tampering_and_stale_age(self):
        proof = self.proof()
        tampered = dict(proof, boot_ticks=proof["boot_ticks"] + 1)
        with self.assertRaises(ValueError):
            host_boot.verify_host_boot_proof(tampered, observed=OBSERVED)
        with self.assertRaises(ValueError):
            host_boot.verify_host_boot_proof(dict(proof, uptime_ticks="3600"), observed=OBSERVED)
        with self.assertRaises(ValueError):
            host_boot.verify_host_boot_proof(dict(proof, margin_seconds=-1), observed=OBSERVED)
        with self.assertRaises(ValueError):
            host_boot.verify_host_boot_proof(proof, max_age_seconds=600,
                                             observed=OBSERVED + 601 * host_boot.TICKS_PER_SECOND)
        self.assertEqual(
            host_boot.verify_host_boot_proof(proof, max_age_seconds=600,
                                             observed=OBSERVED + 599 * host_boot.TICKS_PER_SECOND),
            proof)

    def test_provably_exited_requires_margin(self):
        proof = self.proof()
        margin = proof["margin_seconds"] * host_boot.TICKS_PER_SECOND
        self.assertTrue(host_boot.provably_exited(self.identity(BOOT - margin - 1), proof))
        self.assertFalse(host_boot.provably_exited(self.identity(BOOT - margin), proof))
        self.assertFalse(host_boot.provably_exited(self.identity(BOOT + margin), proof))
        self.assertTrue(host_boot.provably_started_after_boot(self.identity(BOOT + margin + 1), proof))
        self.assertFalse(host_boot.provably_started_after_boot(self.identity(BOOT + margin), proof))

    def test_identity_requires_exact_kernel_evidence(self):
        for identity in ({**self.identity(BOOT), "pid": True},
                         {**self.identity(BOOT), "started": "12.5"},
                         {**self.identity(BOOT), "started": 12},
                         {**self.identity(BOOT), "executable": ""},
                         {"pid": 42, "started": str(BOOT)}):
            with self.subTest(identity=identity):
                with self.assertRaises(ValueError):
                    host_boot.identity_ticks(identity)


if __name__ == "__main__":
    unittest.main()
