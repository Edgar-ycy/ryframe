"""原生数据集的直接调用协议；请求与凭据不进入 argv 或项目环境。"""
import contextlib
import io
import json
from pathlib import Path
import sys
import unittest
from unittest.mock import patch

import restore_reference as reference


class DatasetCliTests(unittest.TestCase):
    def test_dataset_protocol_has_exact_modes_and_no_inline_credentials(self):
        root = Path(__file__).resolve().parents[2]
        payload = json.loads(reference.dataset_protocol(
            root, root / ".local-tests/plan.json", root / ".local-tests/preflight.json"))
        self.assertEqual(set(payload), {"backend_dir", "format_version", "kind", "plan",
                                        "preflight", "side", "verify_existing", "write"})
        self.assertTrue(payload["write"])
        self.assertEqual(payload["side"], "source")
        self.assertIsNone(payload["verify_existing"])

    def test_reference_stdin_is_exact_and_does_not_consume_environment_protocol(self):
        root = Path(__file__).resolve().parents[2]
        value = {"format_version": 1, "kind": "ryframe-xtask-recovery-reference", "request": {
            "backend_dir": str(root), "format_version": 1, "operation": "check-existing",
            "plan": str(root / ".local-tests/plan.json"), "side": "source", "write": False}}
        environment = {reference.PROTOCOL_KEY: "untrusted"}
        with contextlib.ExitStack() as stack:
            stack.enter_context(patch.object(sys, "stdin", io.TextIOWrapper(io.BytesIO(json.dumps(value).encode()))))
            request = reference.private_protocol_request(["--stdin"], environment)
        self.assertEqual(request.command, "check-existing")
        self.assertEqual(request.side, "source")
        self.assertEqual(environment[reference.PROTOCOL_KEY], "untrusted")
        for raw in (b'{"write":true,"write":false}', b'\xff', b'x' * 65537):
            with patch.object(sys, "stdin", io.TextIOWrapper(io.BytesIO(raw))), self.assertRaises(reference.ReferenceProtocolError):
                reference.private_protocol_request(["--stdin"], {})


if __name__ == "__main__":
    unittest.main()
