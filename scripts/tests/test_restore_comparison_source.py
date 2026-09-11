from pathlib import Path
import sys
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import restore_comparison_source as comparison


class B0AdapterEvidenceTests(unittest.TestCase):
    def test_registered_adapter_is_reconstructed_from_embedded_patch(self):
        root = Path(__file__).resolve().parents[2]
        value = comparison.b0_adapter_evidence(root)
        self.assertEqual(value["base_backend_sha"], comparison.B0_BACKEND_COMMIT)
        self.assertEqual(value["base_frontend_sha"], comparison.B0_FRONTEND_COMMIT)
        self.assertEqual(value["reference_adapter_sha"], comparison.B0_ADAPTER_COMMIT)
        self.assertEqual(value["adapter_tree"], comparison.B0_ADAPTER_TREE)
        self.assertEqual(value["adapter_paths"], ["xtask/src/cli.rs"])
        self.assertEqual(value["patch"]["sha256"], comparison.B0_ADAPTER_PATCH_SHA256)

    def test_changed_embedded_patch_is_rejected(self):
        root = Path(__file__).resolve().parents[2]
        with patch.object(comparison, "_patch_bytes", return_value=b"changed"), \
                self.assertRaisesRegex(ValueError, "不匹配"):
            comparison.b0_adapter_evidence(root)


if __name__ == "__main__":
    unittest.main()
