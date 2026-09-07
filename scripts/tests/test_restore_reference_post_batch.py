import hashlib
import json
import tempfile
import unittest
from pathlib import Path
from unittest.mock import Mock, patch

import sys

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import restore_reference_post_batch as batch


def plan(root: Path) -> dict:
    return {
        "id": "restore-case",
        "work_dir": str(root),
        "source": {"scope_id": "fixture-seed-r1", "databases": [{"key": "control", "kind": "combined"}]},
        "dataset": {"records": 110, "api_validation_posts": 2, "post_batch_rows": 25},
    }


class RestoreReferencePostBatchTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        (self.root / "dataset").mkdir()
        self.input = self.root / "dataset/post-batch.ndjson"
        value = plan(self.root)
        rows = []
        for tenant_index, tenant in enumerate(["system", *[f"fixture-seed-r1-{index:02d}" for index in range(1, 11)]]):
            count = 10
            for index in range(2, count):
                rows.append({"tenant_id": tenant, "index": index, "code": f"restore-case-{index}",
                             "name": f"恢复样本{index}", "sort": index % 1000})
        self.input.write_text("\n".join(json.dumps(row, ensure_ascii=False) for row in rows) + "\n", encoding="utf-8")
        self.value = value

    def test_rows_require_complete_deterministic_tenant_ranges(self):
        rows = batch._rows(self.input, self.value)
        self.assertEqual(len(rows), 88)
        changed = self.input.read_text(encoding="utf-8").replace('"index": 2', '"index": 1', 1)
        self.input.write_text(changed, encoding="utf-8")
        with self.assertRaisesRegex(ValueError, "岗位批次"):
            batch._rows(self.input, self.value)

    def test_prepare_uses_fixed_size_transactions_and_returns_id_range(self):
        tools = Mock()
        tools.mysql.side_effect = ["9", "", "", "", ""]
        with patch.object(batch, "ExternalTools", return_value=tools):
            result = batch.prepare(self.value, self.input)
        self.assertEqual(result["rows"], 88)
        self.assertEqual(result["batches"], 4)
        self.assertEqual(result["first_id"], "10")
        self.assertEqual(result["last_id"], "97")
        self.assertEqual(result["input_sha256"], hashlib.sha256(self.input.read_bytes()).hexdigest())
        statements = [call.args[1] for call in tools.mysql.call_args_list[1:]]
        self.assertTrue(all(statement.startswith("SET SESSION time_zone") for statement in statements))
        self.assertTrue(all("START TRANSACTION" in statement and statement.endswith("COMMIT;\n") for statement in statements))
        self.assertNotIn("恢复样本2", statements[0])
        self.assertIn("0x", statements[0])

    def test_prepare_rejects_mysql_output_and_unsafe_id_range(self):
        tools = Mock()
        tools.mysql.side_effect = ["9", "unexpected"]
        with patch.object(batch, "ExternalTools", return_value=tools), self.assertRaisesRegex(ValueError, "标准输出"):
            batch.prepare(self.value, self.input)
        tools.mysql.side_effect = [str(batch.I64_MAX)]
        with patch.object(batch, "ExternalTools", return_value=tools), self.assertRaisesRegex(ValueError, "64 位"):
            batch.prepare(self.value, self.input)


if __name__ == "__main__":
    unittest.main()
