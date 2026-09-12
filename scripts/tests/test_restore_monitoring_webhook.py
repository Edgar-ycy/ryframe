"""隔离监控 webhook 只在临时 loopback 端口接收登记 JSON。"""

from __future__ import annotations

import json
from pathlib import Path
import socket
import sys
import threading
import unittest
import urllib.error
import urllib.request

sys.path.insert(0, str(Path(__file__).resolve().parent))
sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

import restore_monitoring_webhook as webhook
from workspace_directory import WorkspaceDirectory

ROOT = Path(__file__).resolve().parents[2]


def available_port() -> int:
    with socket.socket() as probe:
        probe.bind(("127.0.0.1", 0))
        return probe.getsockname()[1]


class MonitoringWebhookTests(unittest.TestCase):
    def test_loopback_fixture_records_json_and_rejects_other_inputs(self):
        directory = WorkspaceDirectory(ROOT / ".local-tests/python-unit")
        self.addCleanup(directory.cleanup)
        events = Path(directory.name) / "events.jsonl"
        events.write_bytes(b"")
        port = available_port()
        with events.open("ab", buffering=0) as stream:
            instance = webhook.server("127.0.0.1", port, events, stream)
            thread = threading.Thread(target=instance.serve_forever, kwargs={"poll_interval": 0.01})
            thread.start()
            try:
                with urllib.request.urlopen(f"http://127.0.0.1:{port}/readyz", timeout=2) as response:
                    self.assertEqual(response.status, 200)
                request = urllib.request.Request(
                    f"http://127.0.0.1:{port}/alerts",
                    data=json.dumps({"status": "firing", "alerts": []}).encode(),
                    headers={"Content-Type": "application/json"},
                    method="POST",
                )
                with urllib.request.urlopen(request, timeout=2) as response:
                    self.assertEqual(response.status, 204)
                invalid = urllib.request.Request(
                    f"http://127.0.0.1:{port}/alerts",
                    data=b"not-json",
                    headers={"Content-Type": "application/json"},
                    method="POST",
                )
                with self.assertRaises(urllib.error.HTTPError) as caught:
                    urllib.request.urlopen(invalid, timeout=2)
                self.assertEqual(caught.exception.code, 400)
            finally:
                instance.shutdown()
                instance.server_close()
                thread.join(timeout=5)
            values = [json.loads(line) for line in events.read_text(encoding="utf-8").splitlines()]
            self.assertEqual(len(values), 1)
            self.assertEqual(values[0]["remote"], "127.0.0.1")
            self.assertEqual(values[0]["payload"]["status"], "firing")


if __name__ == "__main__":
    unittest.main()
