from __future__ import annotations

import importlib.util
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
SCRIPT = ROOT / "scripts" / "check_architecture.py"
SPEC = importlib.util.spec_from_file_location("check_architecture", SCRIPT)
assert SPEC is not None and SPEC.loader is not None
CHECK_ARCHITECTURE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(CHECK_ARCHITECTURE)


class ArchitectureSecurityTest(unittest.TestCase):
    def test_current_sources_do_not_expose_unsigned_replay_headers(self) -> None:
        errors: list[str] = []
        CHECK_ARCHITECTURE.check_unsigned_replay_contract(errors)
        self.assertEqual(errors, [])

    def test_unsigned_replay_header_contract_is_rejected(self) -> None:
        self.assertTrue(
            CHECK_ARCHITECTURE.exposes_unsigned_replay_contract(
                'headers.get("X-Nonce")'
            )
        )
        self.assertTrue(
            CHECK_ARCHITECTURE.exposes_unsigned_replay_contract(
                "headers.get('x-timestamp')"
            )
        )

    def test_standard_message_signature_fields_are_not_blocked(self) -> None:
        self.assertFalse(
            CHECK_ARCHITECTURE.exposes_unsigned_replay_contract(
                'headers.get("Signature-Input"); headers.get("Content-Digest");'
            )
        )


if __name__ == "__main__":
    unittest.main()
