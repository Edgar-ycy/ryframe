import http.client
import json
import sys
import threading
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import restore_frontend_server
from restore_frontend_build import FRONTEND_RECEIPT, frontend_files
from source_inventory import build_source_domains, canonical_digest
from workspace_directory import WorkspaceDirectory

ROOT = Path(__file__).resolve().parents[2]


def inventory():
    return {
        "source": {
            "snapshot": {"head": "1" * 40, "patch_sha256": "2" * 64, "files": [], "clean": True},
            "worktree_fingerprint": "sha256:" + "3" * 64,
        },
        "files": [],
        "guard": {"head": "1" * 40, "index_sha256": "4" * 64, "modes_sha256": "5" * 64},
    }


class RestoreFrontendServerTests(unittest.TestCase):
    def setUp(self):
        self.directory = WorkspaceDirectory(ROOT / ".local-tests/python-unit")
        self.addCleanup(self.directory.cleanup)
        self.frontend = Path(self.directory.name) / "中文 空格"
        dist = self.frontend / "dist"
        (dist / ".vite").mkdir(parents=True)
        (dist / "assets").mkdir()
        (dist / "index.html").write_bytes(b"<main>RyFrame</main>")
        (dist / "assets/app.js").write_bytes(b"globalThis.ryframe = true")
        (dist / ".vite/manifest.json").write_bytes(b"{}")
        receipt = {
            "format_version": 2,
            "kind": "restore-frontend-build",
            "sources": build_source_domains(inventory(), "frontend"),
            "build": {
                "command": ["vite", "build"],
                "mode": "production",
                "target": "vite-default",
                "toolchain": {
                    "node": "v24.0.0",
                    "pnpm": {"pinned": "11.0.0", "observed": "11.0.0"},
                    "vite": "7.0.0",
                },
                "environment": {"variables": [], "sha256": canonical_digest([])},
                "environment_files": [],
            },
            "files": frontend_files(self.frontend),
        }
        self.receipt = dist / FRONTEND_RECEIPT
        self.receipt.write_text(json.dumps(receipt), encoding="utf-8")

    def test_site_loads_only_the_receipt_whitelist_and_detects_changes(self):
        content = restore_frontend_server.load_site(self.frontend, self.receipt)
        self.assertEqual(set(content), {".vite/manifest.json", "assets/app.js", "index.html"})
        (self.frontend / "dist/assets/app.js").write_bytes(b"changed")
        with self.assertRaisesRegex(ValueError, "不一致"):
            restore_frontend_server.load_site(self.frontend, self.receipt)

    def test_handler_serves_assets_and_spa_without_exposing_receipt_or_paths(self):
        content = restore_frontend_server.load_site(self.frontend, self.receipt)
        server = restore_frontend_server.ThreadingHTTPServer(
            ("127.0.0.1", 0), restore_frontend_server._handler(content)
        )
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()
        self.addCleanup(server.server_close)
        self.addCleanup(server.shutdown)
        connection = http.client.HTTPConnection("127.0.0.1", server.server_port, timeout=2)
        self.addCleanup(connection.close)

        connection.request("GET", "/assets/app.js")
        response = connection.getresponse()
        self.assertEqual((response.status, response.read()), (200, b"globalThis.ryframe = true"))
        connection.request("GET", "/post/42", headers={"Accept": "text/html"})
        response = connection.getresponse()
        self.assertEqual((response.status, response.read()), (200, b"<main>RyFrame</main>"))
        for target in ("/.vite/restore-build.json", "/assets/missing.js", "/%2e%2e/secret"):
            connection.request("GET", target, headers={"Accept": "application/json"})
            response = connection.getresponse()
            with self.subTest(target=target):
                self.assertIn(response.status, {400, 404})
                response.read()

    def test_invalid_bindings_are_rejected_before_listening(self):
        for host, port in (("0.0.0.0", 4173), ("127.0.0.1", 0), ("127.0.0.1", True)):
            with self.subTest(host=host, port=port), self.assertRaises(ValueError):
                restore_frontend_server.serve(self.frontend, self.receipt, host, port)


if __name__ == "__main__":
    unittest.main()
