"""运行时来源分域与产品产物复用的离线边界测试。"""

import copy
from pathlib import Path
import sys
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import source_fingerprints as sources

SNAPSHOT = {"head": "a" * 40, "patch_sha256": "b" * 64, "files": [], "clean": False}


def inventory(*, product: str = "c", tool: str = "d") -> dict:
    return {
        "source": {
            "snapshot": copy.deepcopy(SNAPSHOT),
            "worktree_fingerprint": "sha256:" + tool * 64,
        },
        "files": [
            {"path": "Cargo.toml", "sha256": product * 64},
            {"path": "scripts/check.py", "sha256": tool * 64},
        ],
        "guard": {"head": "a" * 40, "index_sha256": "e" * 64, "modes_sha256": "f" * 64},
    }


class SourceFingerprintsTests(unittest.TestCase):
    def test_execution_source_records_snapshot_worktree_and_domains(self):
        value = sources.execution_source(inventory())
        self.assertEqual(value["snapshot"], SNAPSHOT)
        self.assertEqual(value["worktree_fingerprint"], "sha256:" + "d" * 64)
        self.assertEqual(set(value["fingerprints"]), {"product", "test_tools", "support"})

    def test_current_execution_source_uses_one_protected_inventory(self):
        value = inventory()
        with patch.object(sources, "capture_inventory", return_value=value) as capture:
            self.assertEqual(sources.current_execution_source(Path("repository")), sources.execution_source(value))
        capture.assert_called_once_with(Path("repository"))

    def test_build_source_accepts_product_and_maintenance_receipt_shapes(self):
        self.assertEqual(sources.build_source({"kind": "restore-backend-build", "source": SNAPSHOT}),
                         {"snapshot": SNAPSHOT})
        value = inventory()["source"]
        self.assertEqual(sources.build_source({"kind": "devex-clone-tool-build", "source": value}), value)

    def test_inventory_source_must_match_build_snapshot(self):
        value = inventory()
        sources.verify_inventory_source(value, {"kind": "restore-backend-build", "source": SNAPSHOT})
        changed = copy.deepcopy(value)
        changed["source"]["snapshot"]["head"] = "0" * 40
        with self.assertRaisesRegex(ValueError, "构建时"):
            sources.verify_inventory_source(changed, {"kind": "restore-backend-build", "source": SNAPSHOT})

    def test_maintenance_inventory_must_match_complete_fingerprint(self):
        value = inventory()
        receipt = {"kind": "devex-clone-tool-build", "source": copy.deepcopy(value["source"])}
        sources.verify_inventory_source(value, receipt)
        receipt["source"]["worktree_fingerprint"] = "sha256:" + "0" * 64
        with self.assertRaisesRegex(ValueError, "构建时"):
            sources.verify_inventory_source(value, receipt)

    def test_inventory_rejects_invalid_worktree_fingerprint(self):
        value = inventory()
        value["source"]["worktree_fingerprint"] = "invalid"
        with self.assertRaisesRegex(ValueError, "构建时"):
            sources.verify_inventory_source(value, {"kind": "restore-backend-build", "source": SNAPSHOT})

    def test_receipt_without_inventory_cannot_claim_reusable_source(self):
        with patch.object(sources, "current_execution_source", side_effect=AssertionError("unexpected scan")):
            self.assertIsNone(sources.reusable_artifact_source(Path("repository"), {"source": SNAPSHOT}))

    def test_tool_only_change_reuses_original_product_source(self):
        original = inventory(tool="d")
        current = sources.execution_source(inventory(tool="e"))
        receipt = {"kind": "restore-backend-build", "source": SNAPSHOT, "source_inventory": original}
        with patch.object(sources, "current_execution_source", return_value=current):
            result = sources.reusable_artifact_source(Path("repository"), receipt)
        self.assertEqual(result, original["source"])
        result["snapshot"]["head"] = "0" * 40
        self.assertEqual(original["source"]["snapshot"]["head"], "a" * 40)

    def test_product_change_rejects_existing_artifact(self):
        original = inventory(product="c")
        current = sources.execution_source(inventory(product="0"))
        receipt = {"kind": "restore-backend-build", "source": SNAPSHOT, "source_inventory": original}
        with patch.object(sources, "current_execution_source", return_value=current), \
                self.assertRaisesRegex(ValueError, "重新编译"):
            sources.reusable_artifact_source(Path("repository"), receipt)


if __name__ == "__main__":
    unittest.main()
