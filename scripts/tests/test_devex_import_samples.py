"""导入性能样本保持全有效模型及每次测量独立命名空间。"""
from pathlib import Path
import shutil
import sys
import unittest
import uuid
import xml.etree.ElementTree as ET
import zipfile

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from devex_import_samples import MODEL, NS, canonical, cell_text, digest, prepare

ROOT = Path(__file__).resolve().parents[2]


def test_directory(label: str) -> Path:
    parent = ROOT / ".local-tests/python-unit"
    parent.mkdir(parents=True, exist_ok=True)
    path = parent / f"{label}-{uuid.uuid4()}"
    path.mkdir()
    return path


def template(path: Path, department: str = "测试总部/测试部门", headers=None) -> str:
    headers = headers or ["用户名", "昵称", "邮箱", "手机号", "部门完整路径"]
    def sheet(rows):
        root = ET.Element(f"{{{NS}}}worksheet")
        ET.SubElement(root, f"{{{NS}}}dimension", {"ref": "A1:E4"})
        data = ET.SubElement(root, f"{{{NS}}}sheetData")
        for number, values in enumerate(rows, 1):
            row = ET.SubElement(data, f"{{{NS}}}row", {"r": str(number)})
            for column, value in zip("ABCDE", values):
                cell = ET.SubElement(row, f"{{{NS}}}c", {"r": f"{column}{number}", "t": "inlineStr"})
                inline = ET.SubElement(cell, f"{{{NS}}}is")
                ET.SubElement(inline, f"{{{NS}}}t").text = value
        return ET.tostring(root)
    with zipfile.ZipFile(path, "x", compression=zipfile.ZIP_DEFLATED) as archive:
        archive.writestr("xl/worksheets/sheet1.xml", sheet([headers, ["旧示例", "示例", "", "", "错误部门"]]))
        archive.writestr("xl/worksheets/sheet2.xml", sheet([["部门完整路径"], [department]]))
        archive.writestr("unchanged.txt", "必须保留模板其他内容")
    return digest(path.read_bytes())


class ImportSampleTests(unittest.TestCase):
    def setUp(self):
        self.root = test_directory("import-samples")
        self.addCleanup(lambda: shutil.rmtree(self.root, ignore_errors=True))
        self.template = self.root / "template.xlsx"
        self.template_hash = template(self.template)

    def request(self, namespace="a" * 20, **values):
        return {"template": str(self.template), "template_sha256": self.template_hash,
                "model_sha256": digest(canonical(MODEL)), "directory": str(self.root / namespace),
                "namespace": namespace, "concurrency": 2, "cycles": 2, **values}

    def test_all_rows_valid_and_other_template_entries_unchanged(self):
        manifest = prepare(self.request())
        self.assertEqual(manifest["model"], MODEL)
        self.assertEqual(len(manifest["files"]), 4)
        with zipfile.ZipFile(self.template) as original:
            department = original.read("xl/worksheets/sheet2.xml")
        for entry in manifest["files"]:
            path = self.root / ("a" * 20) / entry["filename"]
            self.assertEqual(digest(path.read_bytes()), entry["sha256"])
            self.assertRegex(entry["username"], r"^dv[a-f0-9]{26}$")
            self.assertLessEqual(len(entry["username"]), 30)
            with zipfile.ZipFile(path) as archive:
                sheet = ET.fromstring(archive.read("xl/worksheets/sheet1.xml"))
                rows = sheet.find(f"{{{NS}}}sheetData")
                self.assertEqual(len(rows), 2)
                self.assertEqual([cell_text(cell, []) for cell in rows[1]],
                                 [entry["username"], "性能导入", entry["username"] + "@example.test", "", "测试总部/测试部门"])
                self.assertEqual(archive.read("xl/worksheets/sheet2.xml"), department)
                self.assertEqual(archive.read("unchanged.txt").decode(), "必须保留模板其他内容")
        self.assertEqual(digest(self.template.read_bytes()), self.template_hash)

    def test_abba_and_five_samples_per_arm_never_reuse_usernames(self):
        used = set()
        arms = ["A", "B", "B", "A", "A", "B", "B", "A", "A", "B"]
        self.assertEqual(arms.count("A"), 5)
        self.assertEqual(arms.count("B"), 5)
        for index, _arm in enumerate(arms):
            manifest = prepare(self.request(f"{index:020x}"))
            usernames = {row["username"] for row in manifest["files"]}
            self.assertFalse(used & usernames)
            used |= usernames
        self.assertEqual(len(used), 40)

    def test_fixed_namespace_model_is_reproducible_but_existing_directory_rejected(self):
        first = prepare(self.request())
        second = prepare(self.request(directory=str(self.root / "another-explicit-directory")))
        self.assertEqual(first, second)
        with self.assertRaisesRegex(ValueError, "目录已存在"):
            prepare(self.request())

    def test_wrong_template_model_header_and_department_fail_before_output(self):
        with self.assertRaisesRegex(ValueError, "模板SHA"):
            prepare(self.request(template_sha256="0" * 64))
        with self.assertRaisesRegex(ValueError, "模型SHA"):
            prepare(self.request(model_sha256="0" * 64))
        for suffix, headers, department in [("header", ["旧结构"], "部门"), ("department", None, "")]:
            file = self.root / (suffix + ".xlsx")
            checksum = template(file, department, headers)
            with self.assertRaises(ValueError):
                prepare(self.request(template=str(file), template_sha256=checksum))
        self.assertFalse((self.root / ("a" * 20)).exists())

    def test_invalid_dimensions_namespace_and_zip_paths_fail(self):
        for values in [{"concurrency": 0}, {"concurrency": True}, {"concurrency": 101}, {"cycles": 0},
                       {"concurrency": 100, "cycles": 101}, {"namespace": "../../escape"}]:
            with self.subTest(values=values), self.assertRaises(ValueError):
                prepare(self.request(**values))
        file = self.root / "invalid.xlsx"
        with zipfile.ZipFile(file, "x") as archive:
            archive.writestr("../outside", "data")
        with self.assertRaisesRegex(ValueError, "ZIP"):
            prepare(self.request(template=str(file), template_sha256=digest(file.read_bytes())))


if __name__ == "__main__":
    unittest.main()
