"""库存失败目录中的命令 stdout 必须由收据完整且唯一地绑定。"""
import copy
import json
from pathlib import Path
import sys
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parent))
from devex_clone_capture import write_json
from devex_clone_inventory_failure import inventory_failure_files
from devex_clone_run_state import binding
from workspace_directory import WorkspaceDirectory


UUIDS = ("a" * 32, "b" * 32, "c" * 32)


class InventoryFailureFilesTests(unittest.TestCase):
    def setUp(self):
        temporary = WorkspaceDirectory(
            Path(__file__).resolve().parents[2] / ".local-tests/python-unit",
            prefix="inventory-failure-",
        )
        self.addCleanup(temporary.cleanup)
        self.target = Path(temporary.name)
        self.relative = "inventory-initial"
        self.directory = self.target / self.relative
        self.directory.mkdir()
        self.request = {"target": {"scope_id": "inventory-failure", "databases": []}}
        write_json(self.directory / "failure.json", {
            "format_version": 1,
            "status": "side_inventory_failed",
            "stage": "inputs",
            "error_type": "FileNotFoundError",
            "remote_writes": 0,
            "target_ready": False,
            "clone_verified": False,
        })

    def available(self) -> set[str]:
        return {self.relative} | {
            self.relative + "/" + path.name for path in self.directory.iterdir()
        }

    def command(self, identifier: str, stdout: Path | None = None, *,
                capture_error: str | None = None, body="captured") -> Path:
        receipt = {
            "command": ["fixed-tool", "verify"],
            "returncode": 0,
            "error_type": None,
            "stdin": None,
            "stdout": body,
            "stderr": "",
        }
        if stdout is not None:
            receipt["stdout_file"] = ({"path": str(stdout)} if capture_error else binding(stdout))
        if capture_error is not None:
            receipt["stdout_capture_error"] = capture_error
        path = self.directory / f"command-{identifier}.json"
        write_json(path, receipt)
        return path

    @staticmethod
    def replace(path: Path, value: dict) -> None:
        path.write_text(
            json.dumps(value, ensure_ascii=False, sort_keys=True, indent=2) + "\n",
            encoding="utf-8",
        )

    def test_valid_stdout_reference_is_the_complete_failure_directory(self):
        stdout = self.directory / f"mysql-identity-{UUIDS[0]}.stdout"
        stdout.write_bytes(b"fixed output\n")
        self.command(UUIDS[1], stdout)

        self.assertEqual(
            inventory_failure_files(
                self.target, self.relative, self.request, self.available()
            ),
            self.available(),
        )

    def test_matching_command_stdout_without_receipt_reference_is_rejected(self):
        (self.directory / f"command-{UUIDS[0]}.stdout").write_bytes(b"unclaimed")

        with self.assertRaisesRegex(ValueError, "引用不匹配"):
            inventory_failure_files(
                self.target, self.relative, self.request, self.available()
            )

    def test_valid_stdout_capture_failure_has_no_decoded_body(self):
        stdout = self.directory / f"mysql-identity-{UUIDS[0]}.stdout"
        stdout.write_bytes(b"partial output")
        self.command(UUIDS[1], stdout, capture_error="OSError", body=None)

        self.assertEqual(
            inventory_failure_files(
                self.target, self.relative, self.request, self.available()
            ),
            self.available(),
        )

    def test_two_command_receipts_cannot_reference_the_same_stdout(self):
        stdout = self.directory / f"mysql-identity-{UUIDS[0]}.stdout"
        stdout.write_bytes(b"fixed output\n")
        self.command(UUIDS[1], stdout)
        self.command(UUIDS[2], stdout)

        with self.assertRaisesRegex(ValueError, "重复引用"):
            inventory_failure_files(
                self.target, self.relative, self.request, self.available()
            )

    def test_stdout_reference_is_checked_before_reading_outside_directory(self):
        stdout = self.target / "outside.stdout"
        stdout.write_bytes(b"outside")
        self.command(UUIDS[0], stdout)

        with self.assertRaisesRegex(ValueError, "越出采集目录"):
            inventory_failure_files(
                self.target, self.relative, self.request, self.available()
            )

    def test_mismatched_stdout_binding_is_rejected(self):
        stdout = self.directory / f"mysql-identity-{UUIDS[0]}.stdout"
        stdout.write_bytes(b"fixed output\n")
        receipt_path = self.command(UUIDS[1], stdout)
        receipt = json.loads(receipt_path.read_text(encoding="utf-8"))
        receipt["stdout_file"] = copy.deepcopy(receipt["stdout_file"])
        receipt["stdout_file"]["sha256"] = "f" * 64
        self.replace(receipt_path, receipt)

        with self.assertRaisesRegex(ValueError, "绑定变化"):
            inventory_failure_files(
                self.target, self.relative, self.request, self.available()
            )

    def test_capture_error_cannot_describe_stdout_directory(self):
        stdout = self.directory / f"command-{UUIDS[0]}.stdout"
        stdout.mkdir()
        self.command(UUIDS[1], stdout, capture_error="OSError", body=None)

        with self.assertRaisesRegex(ValueError, "名称无效"):
            inventory_failure_files(
                self.target, self.relative, self.request, self.available()
            )

    def test_stdout_capture_failure_cannot_retain_decoded_body(self):
        stdout = self.directory / f"mysql-identity-{UUIDS[0]}.stdout"
        stdout.write_bytes(b"partial output")
        self.command(UUIDS[1], stdout, capture_error="OSError", body="partial output")

        with self.assertRaisesRegex(ValueError, "采集错误字段"):
            inventory_failure_files(
                self.target, self.relative, self.request, self.available()
            )


if __name__ == "__main__":
    unittest.main()
