"""Device preview 静态响应必须逐项匹配浏览器绑定的完整 dist。"""

from __future__ import annotations

import json
from pathlib import Path
import sys
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parent))
from devex_clone_capture import write_json
from reference_fixture_browser_evidence import artifact_manifest
from reference_fixture_browser_responses import preview_responses
from restore_build import file_digest
from workspace_directory import WorkspaceDirectory


class ReferenceFixtureBrowserResponseTests(unittest.TestCase):
    def setUp(self):
        backend = Path(__file__).resolve().parents[2]
        self.temporary = WorkspaceDirectory(dir=backend / ".local-tests", prefix="browser-response-")
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.dist = self.root / "frontend/dist"
        (self.dist / "assets").mkdir(parents=True)
        (self.dist / "index.html").write_text("index-a", encoding="utf-8")
        (self.dist / "assets/app.js").write_text("script-a", encoding="utf-8")
        self.manifest = artifact_manifest(self.dist, self.dist.parent, "Device 前端生产产物")
        self.path = self.root / "responses.json"
        self.binding = {"run_id": "r24-device-preview", "scope_id": "fixture-source"}

    def entry(self, sequence: int, path: str, target: Path, destination: str) -> dict:
        return {"sequence": sequence, "method": "GET", "path": path,
                "destination": destination, "status": 200, **file_digest(target),
                "representation": "identity"}

    def write(self, entries: list[dict]) -> None:
        write_json(self.path, {
            "format_version": 1, "kind": "device-preview-static-responses",
            "status": "complete", **{"run_id": self.binding["run_id"],
                                      "scope_id": self.binding["scope_id"]},
            "limits": {"entries": 10_000, "bytes": 8 * 1024 * 1024 * 1024},
            "total_entries": len(entries), "total_bytes": sum(item["bytes"] for item in entries),
            "entries": entries,
        })

    def test_accepts_spa_fallback_and_repeated_exact_static_responses(self):
        entries = [
            self.entry(1, "/system/device", self.dist / "index.html", "document"),
            self.entry(2, "/assets/app.js", self.dist / "assets/app.js", "script"),
            self.entry(3, "/assets/app.js", self.dist / "assets/app.js", "script"),
        ]
        self.write(entries)
        result = preview_responses(self.path, self.binding, self.manifest)
        self.assertEqual(result["receipt"]["entries"], entries)
        self.assertEqual(result["descriptor"]["path"], str(self.path))

    def test_rejects_a_to_b_to_a_bytes_unknown_files_and_query_paths(self):
        cases = []
        changed = self.entry(1, "/login", self.dist / "index.html", "document")
        changed["sha256"] = "b" * 64
        cases.append(changed)
        cases.append(self.entry(1, "/assets/missing.js", self.dist / "assets/app.js", "script"))
        cases.append(self.entry(1, "/assets/app.js?v=1", self.dist / "assets/app.js", "script"))
        for entry in cases:
            with self.subTest(path=entry["path"]):
                if self.path.exists():
                    self.path.unlink()
                self.write([entry, self.entry(2, "/assets/app.js", self.dist / "assets/app.js", "script")])
                with self.assertRaises(ValueError):
                    preview_responses(self.path, self.binding, self.manifest)

    def test_response_receipt_never_contains_file_bodies(self):
        self.write([
            self.entry(1, "/", self.dist / "index.html", "document"),
            self.entry(2, "/assets/app.js", self.dist / "assets/app.js", "script"),
        ])
        self.assertNotIn("script-a", json.dumps(preview_responses(
            self.path, self.binding, self.manifest
        ), ensure_ascii=False))


if __name__ == "__main__":
    unittest.main()
