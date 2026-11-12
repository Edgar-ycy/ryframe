"""核对已有消费者使用的工作树指纹。"""

import hashlib
import struct
import sys
import unittest
from pathlib import Path
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parent))
sys.path.insert(0, str(Path(__file__).resolve().parent))
import devex_provenance as provenance
from workspace_directory import WorkspaceDirectory

ROOT = Path(__file__).resolve().parents[2]
SHA = "a" * 40
SNAPSHOT = {"head": SHA, "patch_sha256": "d" * 64, "files": [], "clean": True}


class FingerprintTests(unittest.TestCase):
    def test_xtask_length_framing_and_raw_git_order_include_untracked_content(self):
        with WorkspaceDirectory(ROOT / ".local-tests/python-unit") as directory:
            root = Path(directory).resolve()
            (root / "z.rs").write_bytes(b"z content")
            (root / "a.rs").write_bytes(b"a content")
            chunks = [b"a" * 40, b"binary patch\0", b"z.rs", b"z content", b"a.rs", b"a content"]
            expected = hashlib.sha256(
                b"".join(struct.pack("<Q", len(value)) + value for value in chunks)
            ).hexdigest()
            with patch.object(
                provenance, "git", side_effect=[chunks[1], b"z.rs\0a.rs\0"]
            ) as git:
                self.assertEqual(
                    provenance.worktree_fingerprint(root, SNAPSHOT["head"]),
                    "sha256:" + expected,
                )
            self.assertEqual(
                git.call_args_list[0].args[1:],
                ("diff", "--binary", "--no-ext-diff", "HEAD", "--", "."),
            )
            with patch.object(
                provenance, "git", side_effect=[b"", b"../outside\0"]
            ), self.assertRaises(ValueError):
                provenance.worktree_fingerprint(root, SNAPSHOT["head"])


if __name__ == "__main__":
    unittest.main()
