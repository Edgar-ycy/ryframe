"""参考夹具审阅续签必须仅派生新资源描述，并保持未执行状态。"""

from __future__ import annotations

import json
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import reference_fixture_review as review


class ReferenceFixtureReviewTests(unittest.TestCase):
    def setUp(self):
        self.backend = Path(__file__).resolve().parents[2]
        self.temporary = tempfile.TemporaryDirectory(dir=self.backend / ".local-tests")
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.execution = self.root / "device-backend"
        (self.execution / ".local-tests/reference-fixture").mkdir(parents=True)
        self.generated = {"head": "a" * 40, "patch_sha256": "b" * 64, "files": []}
        self.fixture = {
            "format_version": 1, "fixture": "device", "status": "ready",
            "sources": {"backend": {"head": "a" * 40}, "frontend": {"head": "b" * 40}},
            "paths": {"backend": str(self.execution), "frontend": str(self.root / "device-frontend")},
            "generated": {"backend": self.generated},
        }
        self.template = {
            "format_version": 1,
            "kind": "review-only-perf-resource-plan-with-readonly-preflight",
            "ready_for_execution": True,
            "reference": {"source": {"databases": []}, "protected_target": {"databases": []}},
            "tools": {
                "mysql": {"path": "C:/tools/mysql.exe", "sha256": "a" * 64},
                "mysqldump": {"path": "C:/tools/mysqldump.exe", "sha256": "e" * 64},
                "aws": {"path": "C:/tools/aws.exe", "sha256": "b" * 64},
                "rustfs": {"path": "C:/tools/rustfs.exe", "sha256": "c" * 64},
                "redis_server": {"distribution": "Ubuntu", "resolved_path": "/usr/bin/redis-server", "sha256": "d" * 64},
            },
            "services": {"rustfs": {"api": "http://127.0.0.1:29200", "console": "http://127.0.0.1:29201", "data_dir": str(self.root / "rustfs")},
                         "redis": {"directory": str(self.root / "redis")}},
            "scopes": {},
        }
        for index, side in enumerate(review.SIDES):
            scope = f"fixture-{side}-old"
            self.template["scopes"][side] = {
                "scope_id": scope, "runtime_dir": str(self.root / f"runtime-{side}"),
                "backend_dir": str(self.root / f"old-{side}"), "identity_ledger": str(self.root / f"identities-{side}"),
                "api_url": f"http://127.0.0.1:{18210 + index}",
                "worker_ready_url": f"http://127.0.0.1:{19210 + index}/readyz",
                "frontend_url": f"http://127.0.0.1:{4190 + index}",
                "objects": {"endpoint": "http://127.0.0.1:29200", "region": "us-east-1"},
                "redis": {"url": "redis://127.0.0.1:16390/0", "namespace": f"ryframe:{{{scope}}}:",
                          "ownership_key": f"ryframe:{{{scope}}}:.ryframe-owner", "ownership_value": "old"},
                "databases": [{"key": key, "database": f"old_{side}_{key.replace('-', '_')}",
                               "mode": "shared" if key in ("shared-control", "shared") else "dedicated",
                               "expected_server_uuid": "server-uuid", "connection_file": str(self.root / "old.cnf"),
                               "host": "127.0.0.1", "port": 3306}
                              for key in ("shared-control", "shared", "dedicated-a", "dedicated-b")],
            }

    def write(self, name: str, value: dict) -> Path:
        path = self.root / name
        path.write_text(json.dumps(value), encoding="utf-8")
        return path

    def test_renewal_rebinds_all_sides_and_requires_preflight(self):
        template = self.write("template.json", self.template)
        fixture = self.write("fixture.json", self.fixture)
        with patch.object(review, "snapshot", return_value=(self.generated, b"")):
            result = review.renew(self.backend, template, fixture,
                                  self.execution / ".local-tests/reference-fixture/recovery-r11",
                                  "r11-current", 18230, 19230, 4200, 29210, 29211, 16391)
        self.assertFalse(result["ready_for_execution"])
        self.assertNotIn("preflight", result)
        self.assertEqual(result["sources"]["backend_head"], "a" * 40)
        self.assertEqual(result["scopes"]["seed"]["scope_id"], "fixture-seed-r11-current")
        self.assertEqual(result["scopes"]["candidate"]["api_url"], "http://127.0.0.1:18232")
        self.assertEqual(result["scopes"]["base"]["backend_dir"], str(self.execution))
        self.assertEqual(result["services"]["rustfs"]["data_dir"], str(self.execution / ".local-tests/reference-fixture/recovery-r11/rustfs"))
        self.assertEqual(result["scopes"]["seed"]["objects"]["endpoint"], "http://127.0.0.1:29210")
        self.assertEqual(result["services"]["redis"]["endpoint"], "127.0.0.1:16391")
        self.assertTrue(all(item["connection_file"].replace("\\", "/").endswith("secrets/mysql-client.cnf")
                            for item in result["scopes"]["seed"]["databases"]))

    def test_renewal_rejects_overlapping_port_ranges(self):
        template = self.write("template.json", self.template)
        fixture = self.write("fixture.json", self.fixture)
        with patch.object(review, "snapshot", return_value=(self.generated, b"")):
            with self.assertRaisesRegex(ValueError, "端口不能重叠"):
                review.renew(self.backend, template, fixture,
                             self.execution / ".local-tests/reference-fixture/recovery-r11",
                             "r11-current", 18230, 18230, 4200, 29210, 29211, 16391)

    def test_renewal_rechecks_template_and_fixture_descriptors(self):
        original_scope = review._scope
        for kind in ("template", "fixture"):
            with self.subTest(kind=kind):
                template = self.write(f"{kind}-template.json", self.template)
                fixture = self.write(f"{kind}-fixture.json", self.fixture)
                target = template if kind == "template" else fixture
                changed = False

                def derive(*args, **kwargs):
                    nonlocal changed
                    if not changed:
                        changed = True
                        value = json.loads(target.read_text(encoding="utf-8"))
                        value["descriptor_drift"] = True
                        target.write_text(json.dumps(value), encoding="utf-8")
                    return original_scope(*args, **kwargs)

                with (
                    patch.object(review, "snapshot", return_value=(self.generated, b"")),
                    patch.object(review, "_scope", side_effect=derive),
                    self.assertRaisesRegex(ValueError, "模板、Device 收据"),
                ):
                    review.renew(
                        self.backend, template, fixture,
                        self.execution / f".local-tests/reference-fixture/recovery-{kind}",
                        f"r11-{kind}", 18230, 19230, 4200, 29210, 29211, 16391,
                    )


if __name__ == "__main__":
    unittest.main()
