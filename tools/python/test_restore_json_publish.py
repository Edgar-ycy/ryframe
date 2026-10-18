"""浏览器证据仍使用的 JSON 发布工具必须保留独占写入和写后复核。"""

import json
from pathlib import Path
import sys
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parent))
import restore_runtime_evidence as evidence
from workspace_directory import WorkspaceDirectory


class JsonPublicationTests(unittest.TestCase):
    def test_create_only_and_truncated_reread(self):
        with WorkspaceDirectory(dir=Path(__file__).parents[2] / ".local-tests") as work:
            output = Path(work) / "evidence.json"
            value = {"message": "中文"}
            evidence.publish_json(output, value)
            original = output.read_bytes()
            self.assertEqual(json.loads(original), value)
            with self.assertRaises(ValueError):
                evidence.publish_json(output, value)
            self.assertEqual(output.read_bytes(), original)
            output.write_bytes(original[:-1])
            with self.assertRaisesRegex(ValueError, "写后字节"):
                evidence._verify_published_bytes(output, value, original)
            self.assertEqual(list(Path(work).glob("*.pending")), [])

    def test_racing_writer_is_preserved(self):
        with WorkspaceDirectory(dir=Path(__file__).parents[2] / ".local-tests") as work:
            output = Path(work) / "evidence.json"
            original_link = evidence.os.link

            def race(source, destination):
                Path(destination).write_bytes(b"unrelated writer")
                return original_link(source, destination)

            with patch.object(evidence.os, "link", side_effect=race):
                with self.assertRaisesRegex(ValueError, "并发创建"):
                    evidence.publish_json(output, {"id": "new"})
            self.assertEqual(output.read_bytes(), b"unrelated writer")
            self.assertEqual(list(Path(work).glob("*.pending")), [])


if __name__ == "__main__":
    unittest.main()
