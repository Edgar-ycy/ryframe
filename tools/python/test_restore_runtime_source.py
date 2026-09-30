import copy
import json
import sys
import unittest
from pathlib import Path
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parent))
import restore_runtime_evidence
import restore_runtime_source
from source_inventory import build_source_domains
from workspace_directory import WorkspaceDirectory

ROOT = Path(__file__).resolve().parents[2]


def inventory(head):
    return {
        "source": {
            "snapshot": {"head": head, "patch_sha256": "a" * 64, "files": [], "clean": True},
            "worktree_fingerprint": "sha256:" + "b" * 64,
        },
        "files": [],
        "guard": {"head": head, "index_sha256": "c" * 64, "modes_sha256": "d" * 64},
    }


class RestoreRuntimeSourceTests(unittest.TestCase):
    def setUp(self):
        self.directory = WorkspaceDirectory(ROOT / ".local-tests/python-unit")
        self.addCleanup(self.directory.cleanup)
        root = Path(self.directory.name).resolve()
        self.coordinator = root / "coordinator"
        self.product = root / "product"
        self.execution = root / "execution"
        self.frontend = root / "frontend"
        for path in (self.coordinator, self.product, self.execution, self.frontend):
            path.mkdir()
        self.product_inventory = inventory("1" * 40)
        self.execution_inventory = inventory("2" * 40)
        self.frontend_inventory = inventory("3" * 40)
        self.frontend_document = self.document(root / "frontend.json", {"kind": "frontend"})

    @staticmethod
    def document(path, value):
        path.write_text(json.dumps(value), encoding="utf-8")
        return restore_runtime_evidence.read_json_document(path)

    def backend_document(self, product_inventory=None, execution_inventory=None):
        product_inventory = product_inventory or self.product_inventory
        execution_inventory = execution_inventory or self.execution_inventory
        value = {
            "sources": {
                "product": build_source_domains(product_inventory, "backend")["product"],
                "tools": {"files": [], "sha256": "e" * 64},
                "full": copy.deepcopy(execution_inventory),
            }
        }
        return self.document(self.execution.parent / "backend.json", value)

    def patches(self):
        inventories = {
            self.product: self.product_inventory,
            self.execution: self.execution_inventory,
            self.frontend: self.frontend_inventory,
        }
        stack = self.enterContext(patch.object(restore_runtime_source, "repository", side_effect=lambda path, _label: Path(path)))
        _ = stack
        self.enterContext(
            patch.object(
                restore_runtime_source,
                "capture_inventory",
                side_effect=lambda path: copy.deepcopy(inventories[Path(path)]),
            )
        )
        self.enterContext(patch.object(restore_runtime_source, "verify_build"))
        self.enterContext(
            patch.object(
                restore_runtime_source,
                "validate_registered_frontend",
                return_value=(copy.deepcopy(self.frontend_inventory), self.frontend_document),
            )
        )

    def test_b0_keeps_product_and_adapter_execution_identity_separate(self):
        self.patches()
        backend = self.backend_document()
        with patch.object(
            restore_runtime_source,
            "registered_source",
            return_value=(
                self.execution,
                copy.deepcopy(self.execution_inventory),
                (self.product, copy.deepcopy(self.product_inventory)),
            ),
        ) as registered:
            result = restore_runtime_source.resolve_runtime_sources(
                self.coordinator,
                self.execution,
                self.frontend,
                backend,
                self.frontend_document,
                "3" * 40,
                adapter_contract="legacy-stable-readiness-b0-v1",
                product_backend=self.product,
            )
        self.assertEqual(
            result["source"],
            {
                "backend_product_sha": "1" * 40,
                "backend_execution_sha": "2" * 40,
                "backend_adapter_contract": "legacy-stable-readiness-b0-v1",
                "frontend_sha": "3" * 40,
            },
        )
        self.assertEqual(result["roots"]["backend_product"], str(self.product))
        registered.assert_called_once_with(
            self.coordinator,
            self.execution,
            "2" * 40,
            adapter_contract="legacy-stable-readiness-b0-v1",
            product_backend=self.product,
        )

    def test_b1_requires_execution_and_product_to_be_the_same_source(self):
        self.product = self.execution
        self.product_inventory = self.execution_inventory
        self.patches()
        result = restore_runtime_source.resolve_runtime_sources(
            self.coordinator,
            self.execution,
            self.frontend,
            self.backend_document(self.execution_inventory, self.execution_inventory),
            self.frontend_document,
            "3" * 40,
        )
        self.assertEqual(result["source"]["backend_product_sha"], "2" * 40)
        self.assertEqual(result["source"]["backend_execution_sha"], "2" * 40)
        self.assertIsNone(result["source"]["backend_adapter_contract"])

    def test_b1_rejects_a_separate_product_argument_before_runtime_use(self):
        self.patches()
        with self.assertRaisesRegex(ValueError, "不得另行指定产品来源"):
            restore_runtime_source.resolve_runtime_sources(
                self.coordinator,
                self.execution,
                self.frontend,
                self.backend_document(self.execution_inventory, self.execution_inventory),
                self.frontend_document,
                "3" * 40,
                product_backend=self.product,
            )


if __name__ == "__main__":
    unittest.main()
