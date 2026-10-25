"""仍存prepare消费端的严格换行与进程身份约束；初始化执行测试已由Rust承载。"""
import unittest
import json
from pathlib import Path
from workspace_directory import WorkspaceDirectory
from devex_clone_target_fixture import Fixture
from devex_clone_capture import read_json
from restore_build import file_digest
import devex_clone_target_binding as target_binding
import devex_clone_target_prepared_files as prepared_evidence

def replace_json(path, value):
    path.write_text(json.dumps(value, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")

class TargetResumeTests(unittest.TestCase):
    def setUp(self):
        temporary = WorkspaceDirectory(
            Path(__file__).resolve().parents[2] / ".local-tests/python-unit",
            prefix="resume-",
        )
        self.addCleanup(temporary.cleanup)
        self.f = Fixture(Path(temporary.name), self)


    def validate_current_prepare_tree(self, descriptor):
        prepared_evidence.validate_prepared_tree(
            self.f.root, self.f.output, target_binding.target_files(self.f.output),
            descriptor, read_json(self.f.path),
        )


    def test_prepared_tree_requires_one_mysql_output_line_with_platform_ending(self):
        f = self.f
        descriptor = {"path": str(f.path), **file_digest(f.path)}
        f.prepared()
        receipt = next(f.output.glob("mysql-shared-control-*.command.json"))
        value = read_json(receipt)
        self.assertEqual(value["stdout"], f.uuid + "\r\n")
        self.validate_current_prepare_tree(descriptor)
        value["stdout"] = f.uuid + "\n"
        replace_json(receipt, value)
        self.validate_current_prepare_tree(descriptor)

        for stdout in (f.uuid, f.uuid + "\r\nextra\r\n", " " + f.uuid + "\r\n",
                       f.uuid + " \r\n", f.uuid + "\r\n\r\n"):
            with self.subTest(stdout=repr(stdout)):
                value["stdout"] = stdout
                replace_json(receipt, value)
                with self.assertRaisesRegex(ValueError, "MySQL"):
                    self.validate_current_prepare_tree(descriptor)


    def test_prepared_tree_compares_stable_redis_proc_identity(self):
        f = self.f
        descriptor = {"path": str(f.path), **file_digest(f.path)}
        f.prepared()
        receipts = []
        for path in f.output.glob("redis-kernel-*.command.json"):
            value = read_json(path)
            if value["command"][-2:] == ["/usr/bin/cat", "/proc/200/stat"]:
                receipts.append(path)
        self.assertEqual(len(receipts), 2)
        changed = read_json(receipts[1])
        fields = changed["stdout"].removesuffix("\n").rpartition(") ")[2].split(" ")
        fields[0], fields[11], fields[12] = "R", "41", "73"
        changed["stdout"] = "200 (redis-server) " + " ".join(fields) + "\r\n"
        replace_json(receipts[1], changed)
        self.validate_current_prepare_tree(descriptor)

        stable = changed["stdout"]
        for stdout in (stable.replace("200 (", "201 (", 1),
                       stable.replace("(redis-server)", "(other)"),
                       stable.rsplit("67890", 1)[0] + "67891\r\n"):
            with self.subTest(stdout=stdout):
                changed["stdout"] = stdout
                replace_json(receipts[1], changed)
                with self.assertRaisesRegex(ValueError, "Redis"):
                    self.validate_current_prepare_tree(descriptor)
