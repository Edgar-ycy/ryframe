"""隔离监控投递只接受当前 run 的唯一精确 probe。"""

from __future__ import annotations

import copy
import json
import os
from pathlib import Path
import sys
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

import restore_monitoring_observation as observation
import restore_monitoring_publish as publisher
from workspace_directory import WorkspaceDirectory

ROOT = Path(__file__).resolve().parents[2]


class MonitoringProbeTests(unittest.TestCase):
    def setUp(self):
        self.binding = {
            "scope_id": "restore-scope",
            "endpoints": {
                "prometheus": "http://127.0.0.1:29090",
                "alertmanager": "http://127.0.0.1:29093",
                "webhook": "http://127.0.0.1:29094",
            },
        }
        self.probe = "a" * 32
        self.started = "2026-09-12T00:00:00Z"
        self.event = {
            "received_at": "2026-09-12T00:00:01Z",
            "remote": "127.0.0.1",
            "payload": {
                "receiver": "ryframe-local-webhook",
                "status": "firing",
                "alerts": [
                    {
                        "status": "firing",
                        "labels": {
                            "alertname": "RyFrameMonitoringDeliveryProbe",
                            "service": "ryframe",
                            "scope_id": "restore-scope",
                            "probe_id": self.probe,
                        },
                        "annotations": {"summary": "RyFrame 隔离监控投递验收"},
                        "startsAt": self.started,
                        "endsAt": "2026-09-12T00:10:00Z",
                        "generatorURL": self.binding["endpoints"]["prometheus"],
                        "fingerprint": "fixture",
                    }
                ],
            },
        }

    def test_exact_probe_is_accepted(self):
        self.assertEqual(
            observation._probe_event(self.event, self.binding, self.probe, self.started),
            "firing",
        )

    def test_wrong_receiver_mixed_alert_duplicate_and_wrong_probe_are_rejected(self):
        mutations = []
        wrong_receiver = copy.deepcopy(self.event)
        wrong_receiver["payload"]["receiver"] = "external"
        mutations.append(wrong_receiver)
        mixed = copy.deepcopy(self.event)
        mixed["payload"]["alerts"].append({"labels": {"alertname": "OtherAlert"}})
        mutations.append(mixed)
        duplicate = copy.deepcopy(self.event)
        duplicate["payload"]["alerts"].append(copy.deepcopy(duplicate["payload"]["alerts"][0]))
        mutations.append(duplicate)
        wrong_probe = copy.deepcopy(self.event)
        wrong_probe["payload"]["alerts"][0]["labels"]["probe_id"] = "b" * 32
        mutations.append(wrong_probe)
        for value in mutations:
            with self.subTest(value=value), self.assertRaisesRegex(ValueError, "probe"):
                observation._probe_event(value, self.binding, self.probe, self.started)


class MonitoringTerminalPublisherTests(unittest.TestCase):
    def setUp(self):
        self.directory = WorkspaceDirectory(ROOT / ".local-tests/python-unit")
        self.addCleanup(self.directory.cleanup)
        self.root = Path(self.directory.name).resolve()
        self.value = {"format_version": 1, "kind": "restore-monitoring-result", "status": "passed"}

    def test_success_is_create_only_and_leaves_no_pending(self):
        publisher.publish_terminal_success(self.root, self.value)
        self.assertEqual(json.loads((self.root / "result.json").read_bytes()), self.value)
        self.assertFalse(any(publisher.is_result_pending(item.name) for item in self.root.iterdir()))
        with self.assertRaisesRegex(ValueError, "拒绝覆盖"):
            publisher.publish_terminal_success(self.root, self.value)

    def test_failed_pending_cleanup_preserves_ambiguous_state_and_blocks_replay(self):
        original = Path.unlink

        def fail_pending(path, *args, **kwargs):
            if publisher.is_result_pending(path.name):
                raise OSError("fixture unlink failure")
            return original(path, *args, **kwargs)

        with patch.object(Path, "unlink", fail_pending), self.assertRaisesRegex(OSError, "fixture"):
            publisher.publish_terminal_success(self.root, self.value)
        self.assertTrue((self.root / "result.json").is_file())
        self.assertTrue(any(publisher.is_result_pending(item.name) for item in self.root.iterdir()))
        with self.assertRaisesRegex(ValueError, "pending"):
            publisher.publish_terminal_success(self.root, self.value)


if __name__ == "__main__":
    unittest.main()
