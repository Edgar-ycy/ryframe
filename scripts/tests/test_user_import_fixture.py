"""用户导入 fixture 共用同一模板解析、重写和失败关闭实现。"""

from __future__ import annotations

from concurrent.futures import ThreadPoolExecutor
import json
import os
from pathlib import Path
import stat
import subprocess
import sys
import threading
import unittest
from unittest.mock import patch
import warnings
import xml.etree.ElementTree as ET
import zipfile

SCRIPTS = Path(__file__).resolve().parents[1]
ROOT = SCRIPTS.parent
sys.path.insert(0, str(SCRIPTS))
sys.path.insert(0, str(Path(__file__).resolve().parent))
import user_import_fixture as fixture  # noqa: E402
from user_import_fixture import (  # noqa: E402
    HEADERS,
    MODEL,
    NS,
    canonical,
    cell_text,
    digest,
    prepare_browser,
    prepare_performance,
)
from workspace_directory import WorkspaceDirectory  # noqa: E402


def worksheet(rows: list[list[str]]) -> bytes:
    root = ET.Element(f"{{{NS}}}worksheet")
    ET.SubElement(root, f"{{{NS}}}dimension", {"ref": "A1:E4"})
    data = ET.SubElement(root, f"{{{NS}}}sheetData")
    for number, values in enumerate(rows, 1):
        row = ET.SubElement(data, f"{{{NS}}}row", {"r": str(number)})
        for column, value in zip("ABCDE", values):
            cell = ET.SubElement(
                row, f"{{{NS}}}c", {"r": f"{column}{number}", "t": "inlineStr"}
            )
            inline = ET.SubElement(cell, f"{{{NS}}}is")
            ET.SubElement(inline, f"{{{NS}}}t").text = value
    return ET.tostring(root)


def template(
    path: Path,
    department: str = "测试总部/测试部门",
    headers: list[str] | None = None,
    extra: tuple[str, bytes] | None = None,
) -> str:
    with zipfile.ZipFile(path, "x", compression=zipfile.ZIP_DEFLATED) as archive:
        archive.writestr(
            "xl/worksheets/sheet1.xml",
            worksheet([headers or HEADERS, ["旧示例", "示例", "", "", "错误部门"]]),
        )
        archive.writestr(
            "xl/worksheets/sheet2.xml", worksheet([["部门完整路径"], [department]])
        )
        archive.writestr("unchanged.txt", "必须保留模板其他内容")
        if extra is not None:
            archive.writestr(*extra, compress_type=zipfile.ZIP_STORED)
    return digest(path.read_bytes())


class UserImportFixtureTests(unittest.TestCase):
    def setUp(self) -> None:
        self.directory = WorkspaceDirectory(
            ROOT / ".local-tests/python-unit", "user-import-fixture-"
        )
        self.addCleanup(self.directory.cleanup)
        self.root = self.directory.path
        self.source = self.root / "template.xlsx"
        self.template_sha256 = template(self.source)

    def request(self, namespace: str = "a" * 20, **values: object) -> dict[str, object]:
        return {
            "template": str(self.source),
            "template_sha256": self.template_sha256,
            "model_sha256": digest(canonical(MODEL)),
            "directory": str(self.root / namespace),
            "namespace": namespace,
            "concurrency": 2,
            "cycles": 2,
            **values,
        }

    def directory_alias(self, target: Path, name: str) -> Path:
        alias = self.root / name
        if os.name == "nt":
            completed = subprocess.run(
                ["cmd.exe", "/d", "/c", "mklink", "/J", str(alias), str(target)],
                check=False,
                capture_output=True,
            )
            details = (completed.stdout + completed.stderr).decode(errors="replace")
            self.assertEqual(completed.returncode, 0, details)
            self.addCleanup(lambda: alias.rmdir() if alias.exists() else None)
        else:
            alias.symlink_to(target, target_is_directory=True)
            self.addCleanup(lambda: alias.unlink(missing_ok=True))
        return alias

    def test_performance_rows_are_valid_and_preserve_template_members(self) -> None:
        manifest = prepare_performance(self.request())
        self.assertEqual(manifest["model"], MODEL)
        self.assertEqual(len(manifest["files"]), 4)
        with zipfile.ZipFile(self.source) as original:
            department = original.read("xl/worksheets/sheet2.xml")
        for entry in manifest["files"]:
            path = self.root / ("a" * 20) / entry["filename"]
            self.assertEqual(digest(path.read_bytes()), entry["sha256"])
            self.assertRegex(entry["username"], r"^dv[a-f0-9]{26}$")
            with zipfile.ZipFile(path) as archive:
                sheet = ET.fromstring(archive.read("xl/worksheets/sheet1.xml"))
                rows = sheet.find(f"{{{NS}}}sheetData")
                self.assertIsNotNone(rows)
                self.assertEqual(len(rows), 2)
                self.assertEqual(
                    [cell_text(cell, []) for cell in rows[1]],
                    [
                        entry["username"],
                        "性能导入",
                        entry["username"] + "@example.test",
                        "",
                        "测试总部/测试部门",
                    ],
                )
                self.assertEqual(archive.read("xl/worksheets/sheet2.xml"), department)
                self.assertEqual(
                    archive.read("unchanged.txt").decode(), "必须保留模板其他内容"
                )
        self.assertEqual(digest(self.source.read_bytes()), self.template_sha256)

    def test_abba_samples_never_reuse_usernames(self) -> None:
        used: set[str] = set()
        arms = ["A", "B", "B", "A", "A", "B", "B", "A", "A", "B"]
        self.assertEqual((arms.count("A"), arms.count("B")), (5, 5))
        for index, _arm in enumerate(arms):
            manifest = prepare_performance(self.request(f"{index:020x}"))
            usernames = {row["username"] for row in manifest["files"]}
            self.assertFalse(used & usernames)
            used |= usernames
        self.assertEqual(len(used), 40)

    def test_reproducible_model_rejects_existing_destination(self) -> None:
        first = prepare_performance(self.request())
        second = prepare_performance(
            self.request(directory=str(self.root / "another-explicit-directory"))
        )
        self.assertEqual(first, second)
        with self.assertRaisesRegex(ValueError, "目录已存在"):
            prepare_performance(self.request())

    def test_digest_model_header_and_department_fail_before_output(self) -> None:
        with self.assertRaisesRegex(ValueError, "模板SHA"):
            prepare_performance(self.request(template_sha256="0" * 64))
        with self.assertRaisesRegex(ValueError, "模型SHA"):
            prepare_performance(self.request(model_sha256="0" * 64))
        for suffix, headers, department in [
            ("header", ["旧结构"], "部门"),
            ("department", None, ""),
        ]:
            source = self.root / f"{suffix}.xlsx"
            checksum = template(source, department, headers)
            with self.assertRaises(ValueError):
                prepare_performance(
                    self.request(template=str(source), template_sha256=checksum)
                )
        self.assertFalse((self.root / ("a" * 20)).exists())

    def test_dimensions_and_zip_paths_fail_closed(self) -> None:
        for values in [
            {"concurrency": 0},
            {"concurrency": True},
            {"concurrency": 101},
            {"cycles": 0},
            {"concurrency": 100, "cycles": 101},
            {"namespace": "../../escape"},
        ]:
            with self.subTest(values=values), self.assertRaises(ValueError):
                prepare_performance(self.request(**values))
        for name in ["../outside", "..\\outside", "/absolute"]:
            source = (
                self.root / f"invalid-{len(list(self.root.glob('invalid-*')))}.xlsx"
            )
            with zipfile.ZipFile(source, "x") as archive:
                archive.writestr(name, "data")
            with self.assertRaisesRegex(ValueError, "ZIP"):
                prepare_performance(
                    self.request(
                        template=str(source),
                        template_sha256=digest(source.read_bytes()),
                    )
                )
        duplicate = self.root / "duplicate.xlsx"
        with warnings.catch_warnings():
            warnings.simplefilter("ignore", UserWarning)
            with zipfile.ZipFile(duplicate, "x") as archive:
                archive.writestr("same", "first")
                archive.writestr("same", "second")
        with self.assertRaisesRegex(ValueError, "ZIP"):
            prepare_performance(
                self.request(
                    template=str(duplicate),
                    template_sha256=digest(duplicate.read_bytes()),
                )
            )

        too_many = self.root / "too-many.xlsx"
        with zipfile.ZipFile(too_many, "x") as archive:
            for index in range(1001):
                archive.writestr(f"entries/{index}", b"")
        with self.assertRaisesRegex(ValueError, "ZIP"):
            prepare_performance(
                self.request(
                    template=str(too_many),
                    template_sha256=digest(too_many.read_bytes()),
                )
            )

        linked = self.root / "linked-member.xlsx"
        member = zipfile.ZipInfo("linked")
        member.create_system = 3
        member.external_attr = (stat.S_IFLNK | 0o777) << 16
        with zipfile.ZipFile(linked, "x") as archive:
            archive.writestr(member, "target")
        with self.assertRaisesRegex(ValueError, "ZIP"):
            prepare_performance(
                self.request(
                    template=str(linked),
                    template_sha256=digest(linked.read_bytes()),
                )
            )

    def test_encrypted_zip_member_fails_closed(self) -> None:
        encrypted = self.root / "encrypted.xlsx"
        content = bytearray(self.source.read_bytes())
        for signature, flag_offset in [(b"PK\x03\x04", 6), (b"PK\x01\x02", 8)]:
            header = content.find(signature)
            self.assertGreaterEqual(header, 0)
            offset = header + flag_offset
            flags = int.from_bytes(content[offset : offset + 2], "little") | 0x1
            content[offset : offset + 2] = flags.to_bytes(2, "little")
        encrypted.write_bytes(content)
        with zipfile.ZipFile(encrypted) as archive:
            self.assertTrue(archive.infolist()[0].flag_bits & 0x1)
        with self.assertRaisesRegex(ValueError, "ZIP"):
            prepare_browser(
                encrypted,
                digest(content),
                self.root / "encrypted-output.xlsx",
                "import-1234abcd",
            )

    def test_unicode_space_paths_use_real_exclusive_publication(self) -> None:
        directory = self.root / "中文 空格"
        directory.mkdir()
        source = directory / "真实 模板.xlsx"
        checksum = template(source)
        output = directory / "导入 样本.xlsx"
        receipt = prepare_browser(
            source,
            checksum,
            output,
            "import-1234abcd",
        )
        self.assertEqual(receipt["template_sha256"], checksum)
        self.assertEqual(receipt["sha256"], digest(output.read_bytes()))

    def test_parent_directory_aliases_fail_closed(self) -> None:
        actual = self.root / "真实父目录"
        actual.mkdir()
        source = actual / "template.xlsx"
        checksum = template(source)
        alias = self.directory_alias(actual, "父目录链接")

        with self.assertRaises(ValueError):
            prepare_browser(
                alias / "template.xlsx",
                checksum,
                self.root / "linked-template-output.xlsx",
                "import-1234abcd",
            )
        with self.assertRaises(ValueError):
            prepare_browser(
                self.source,
                self.template_sha256,
                alias / "linked-output.xlsx",
                "import-1234abcd",
            )
        with self.assertRaises(ValueError):
            prepare_performance(
                self.request(directory=str(alias / "performance-output"))
            )
        self.assertEqual(list(actual.glob("*output*")), [])

    def test_template_and_generated_size_limits_fail_before_output(self) -> None:
        oversized_archive = self.root / "oversized-archive.xlsx"
        with zipfile.ZipFile(oversized_archive, "x") as archive:
            archive.writestr("large", bytes(16 * 1024 * 1024 + 1))
        with self.assertRaises(ValueError):
            prepare_performance(
                self.request(
                    template=str(oversized_archive),
                    template_sha256=digest(oversized_archive.read_bytes()),
                )
            )

        oversized_output = self.root / "oversized-output-template.xlsx"
        checksum = template(
            oversized_output,
            extra=("stored.bin", bytes(10 * 1024 * 1024)),
        )
        destination = self.root / "oversized-output.xlsx"
        with self.assertRaisesRegex(ValueError, "产品上限"):
            prepare_browser(
                oversized_output,
                checksum,
                destination,
                "import-1234abcd",
            )
        self.assertFalse(destination.exists())

    def test_large_fixture_rejects_reserved_template_member(self) -> None:
        source = self.root / "reserved-member.xlsx"
        checksum = template(source, extra=("ryframe-upload-fixture.bin", b"occupied"))
        output = self.root / "reserved-output.xlsx"
        with self.assertRaisesRegex(ValueError, "保留成员"):
            prepare_browser(
                source,
                checksum,
                output,
                "import-1234abcd",
                large=True,
            )
        self.assertFalse(output.exists())

    def test_browser_fixture_reuses_core_for_success_duplicate_and_invalid_rows(
        self,
    ) -> None:
        output = self.root / "browser.xlsx"
        receipt = prepare_browser(
            self.source,
            self.template_sha256,
            output,
            "import-1234abcd",
            large=True,
        )
        self.assertGreater(receipt["bytes"], 3 * 1024 * 1024)
        self.assertLess(receipt["bytes"], 10 * 1024 * 1024)
        self.assertEqual(receipt["template_sha256"], self.template_sha256)
        self.assertEqual(receipt["sha256"], digest(output.read_bytes()))
        with zipfile.ZipFile(output) as archive:
            sheet = ET.fromstring(archive.read("xl/worksheets/sheet1.xml"))
            data = sheet.find(f"{{{NS}}}sheetData")
            rows = [[cell_text(cell, []) for cell in row] for row in data]
            self.assertEqual(rows[0], HEADERS)
            self.assertEqual(rows[1][0], rows[2][0])
            self.assertEqual(rows[1][-1], "测试总部/测试部门")
            self.assertEqual(rows[3][-1], "不存在的隔离部门")
            self.assertEqual(
                archive.read("unchanged.txt").decode(), "必须保留模板其他内容"
            )
            padding = archive.getinfo("ryframe-upload-fixture.bin")
            self.assertEqual(padding.compress_type, zipfile.ZIP_STORED)
            self.assertEqual(padding.file_size, 3 * 1024 * 1024)
        with self.assertRaises(ValueError):
            prepare_browser(
                self.source,
                self.template_sha256,
                output,
                "import-1234abcd",
            )

    def test_concurrent_writers_publish_exactly_one_complete_winner(self) -> None:
        output = self.root / "concurrent.xlsx"
        barrier = threading.Barrier(2)
        real_link = fixture.os.link

        def racing_link(source: Path, destination: Path) -> None:
            barrier.wait(timeout=5)
            real_link(source, destination)

        def write(username: str) -> tuple[str, object]:
            try:
                return (
                    "ok",
                    prepare_browser(
                        self.source,
                        self.template_sha256,
                        output,
                        username,
                    ),
                )
            except Exception as error:  # noqa: BLE001 - 测试需要核对竞争输家类型
                return ("error", error)

        with patch.object(fixture.os, "link", side_effect=racing_link):
            with ThreadPoolExecutor(max_workers=2) as executor:
                results = list(
                    executor.map(write, ["import-11111111", "import-22222222"])
                )

        winners = [value for status, value in results if status == "ok"]
        failures = [value for status, value in results if status == "error"]
        self.assertEqual(len(winners), 1)
        self.assertEqual(len(failures), 1)
        self.assertIsInstance(failures[0], FileExistsError)
        winner = winners[0]
        self.assertIsInstance(winner, dict)
        self.assertEqual(winner["sha256"], digest(output.read_bytes()))
        with zipfile.ZipFile(output) as archive:
            sheet = ET.fromstring(archive.read("xl/worksheets/sheet1.xml"))
            data = sheet.find(f"{{{NS}}}sheetData")
            self.assertEqual(cell_text(data[1][0], []), winner["username"])
        self.assertEqual(list(self.root.glob(".concurrent.xlsx.*.tmp")), [])

    def test_atomic_publication_does_not_leave_partial_output(self) -> None:
        output = self.root / "atomic-failure.xlsx"
        with patch.object(fixture.os, "link", side_effect=OSError("injected")):
            with self.assertRaises(OSError):
                prepare_browser(
                    self.source,
                    self.template_sha256,
                    output,
                    "import-1234abcd",
                )
        self.assertFalse(output.exists())
        self.assertEqual(list(self.root.glob(".atomic-failure.xlsx.*.tmp")), [])

    def test_browser_cli_requires_digest_and_hides_parser_details_on_failure(
        self,
    ) -> None:
        script = SCRIPTS / "user_import_fixture.py"
        output = self.root / "cli.xlsx"
        completed = subprocess.run(
            [
                sys.executable,
                "-X",
                "utf8",
                str(script),
                "browser",
                "--template",
                str(self.source),
                "--template-sha256",
                self.template_sha256,
                "--output",
                str(output),
                "--username",
                "import-87654321",
            ],
            check=False,
            capture_output=True,
            text=True,
            encoding="utf-8",
        )
        self.assertEqual(completed.returncode, 0, completed.stderr)
        self.assertTrue(json.loads(completed.stdout)["ok"])
        failed_output = self.root / "failed.xlsx"
        failed = subprocess.run(
            [
                sys.executable,
                "-X",
                "utf8",
                str(script),
                "browser",
                "--template",
                str(self.source),
                "--template-sha256",
                "0" * 64,
                "--output",
                str(failed_output),
                "--username",
                "import-87654321",
            ],
            check=False,
            capture_output=True,
            text=True,
            encoding="utf-8",
        )
        self.assertEqual(failed.returncode, 1)
        self.assertEqual(
            json.loads(failed.stdout),
            {"ok": False, "reason": "import_fixture_preparation_failed"},
        )
        self.assertNotIn(str(self.source), failed.stdout + failed.stderr)
        self.assertFalse(failed_output.exists())
        parameter_error = subprocess.run(
            [sys.executable, "-X", "utf8", str(script), "browser"],
            check=False,
            capture_output=True,
            text=True,
            encoding="utf-8",
        )
        self.assertEqual(parameter_error.returncode, 2)

    def test_performance_cli_preserves_manifest_protocol(self) -> None:
        request = self.request(
            namespace="b" * 20,
            directory=str(self.root / "performance-cli"),
        )
        completed = subprocess.run(
            [
                sys.executable,
                "-X",
                "utf8",
                str(SCRIPTS / "user_import_fixture.py"),
                "performance",
            ],
            input=json.dumps(request),
            check=False,
            capture_output=True,
            text=True,
            encoding="utf-8",
        )
        self.assertEqual(completed.returncode, 0, completed.stderr)
        result = json.loads(completed.stdout)
        self.assertTrue(result["ok"])
        self.assertEqual(result["manifest"]["namespace"], "b" * 20)
        self.assertEqual(len(result["manifest"]["files"]), 4)
        self.assertTrue((self.root / "performance-cli" / "0-0.xlsx").is_file())


if __name__ == "__main__":
    unittest.main()
