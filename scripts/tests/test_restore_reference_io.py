import json
import subprocess
import sys
import unittest
from pathlib import Path
from unittest.mock import Mock, patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from mysql_verification_fixture import mysql_output
from restore_reference import work_directory
from restore_reference_io import DATA_HEADER, ExternalTools, normalize_dump, object_index, validate_dump, validate_insert
from restore_reference_fixture import environment, stored_backup


class ReferenceIoTests(unittest.TestCase):
    def setUp(self):
        self.backend, self.plan = environment(self)
        self.work = work_directory(self.plan)

    def test_data_dump_accepts_escaped_text_binary_null_and_empty_tables(self):
        sql = "INSERT INTO `sys_post` (`id`, `name`, `raw`, `empty`) VALUES (1,'引号\\\'斜线\\\\;文本',0x4142,NULL);"
        raw, normalized = self.work / "raw.sql", self.work / "data.sql"
        raw.write_text("/*!40101 SET @OLD_CHARACTER_SET_CLIENT=@@CHARACTER_SET_CLIENT */;\n\n" + sql + "\n", encoding="utf-8")
        normalize_dump(raw, normalized, {"sys_post"})
        self.assertEqual(normalized.read_text(encoding="utf-8"), DATA_HEADER + sql + "\n")
        validate_dump(normalized, {"sys_post"})
        raw.write_text("")
        empty = self.work / "empty.sql"
        normalize_dump(raw, empty, {"sys_post"})
        self.assertEqual(empty.read_text(), DATA_HEADER)

    def test_dump_refuses_other_tables_ddl_multiple_statements_and_sql_functions(self):
        for sql in ("USE other;", "DROP TABLE sys_post;", "INSERT INTO `other`.`sys_post` (`id`) VALUES (1);",
                    "INSERT INTO `ryframe_resource_ownership` (`id`) VALUES (1);",
                    "INSERT INTO `sys_post` (`id`) VALUES (1); DELETE FROM other;",
                    "INSERT INTO `sys_post` (`id`) VALUES (SLEEP(1));",
                    "INSERT INTO `sys_post` (`name`) VALUES ('unterminated);",
                    "INSERT INTO `sys_post` (`id`) VALUES (1/*comment*/);",
                    "\\! command"):
            with self.assertRaises(ValueError):
                validate_insert(sql, {"sys_post"})

    def test_mysql_exact_defaults_environment_and_declared_tables_only(self):
        run = Mock(return_value=subprocess.CompletedProcess([], 0, stdout=b"one\n"))
        tools = ExternalTools(self.plan, self.work, run)
        database = self.plan["target"]["databases"][0]
        with patch.dict("os.environ", {"MYSQL_PWD": "implicit-secret"}):
            tools.mysql(database, "SELECT 1;")
        command = run.call_args.args[0]
        self.assertEqual(command[1], f"--defaults-file={database['defaults_file']}")
        self.assertIn("--database=target_control", command)
        self.assertIn("--binary-mode", command)
        self.assertNotIn("MYSQL_PWD", run.call_args.kwargs["env"])
        self.assertEqual(run.call_args.kwargs["timeout"], 1800)
        root, _ = stored_backup(self.plan, self.work)
        tools.restore_database(database, ["sys_post"], root / "databases/control.sql")
        sql = run.call_args.kwargs["input"].decode()
        self.assertIn("DELETE FROM `sys_post`;", sql)
        self.assertNotIn("ownership", sql)
        self.assertNotIn("DROP", sql)

    def test_external_command_checks_generation_immediately_before_runner(self):
        events = []

        def run(command, **_kwargs):
            events.append(("run", command))
            return subprocess.CompletedProcess(command, 0, stdout=b"")

        tools = ExternalTools(
            self.plan, self.work, run,
            before_execute=lambda: events.append(("checkpoint", None))
        )
        tools.execute(["fixture"])
        self.assertEqual(events, [("checkpoint", None), ("run", ["fixture"])])

        runner = Mock()
        blocked = ExternalTools(
            self.plan, self.work, runner,
            before_execute=Mock(side_effect=ValueError("lost generation"))
        )
        with self.assertRaisesRegex(ValueError, "lost generation"):
            blocked.execute(["fixture"])
        runner.assert_not_called()

    def test_mysql_ownership_and_config_injection_fail_closed(self):
        tools = ExternalTools(self.plan, self.work)
        responses = iter((b"uuid-1\tsource_control", b"control\tother\twrong"))
        tools.run = Mock(side_effect=lambda command, **kwargs: mysql_output(command, kwargs, next(responses)))
        with self.assertRaisesRegex(ValueError, "ownership"):
            tools.verify_databases("source")
        defaults = Path(self.plan["source"]["databases"][0]["defaults_file"])
        for extra in ("init-command=DROP TABLE sys_post\n", "!include other.cnf\n", "[mysqldump]\nall-databases=true\n"):
            original = defaults.read_text()
            defaults.write_text(original + extra)
            with self.assertRaises(ValueError):
                tools.validate_defaults(defaults)
            defaults.write_text(original)

    def test_aws_cannot_scan_escape_or_overwrite_and_never_puts_keys_in_argv(self):
        run = Mock(return_value=subprocess.CompletedProcess([], 0, stdout=b"{}"))
        tools = ExternalTools(self.plan, self.work, run)
        with patch.dict("os.environ", {"TEST_ACCESS": "access-fixture", "TEST_SECRET": "secret-fixture", "AWS_PROFILE": "wrong"}):
            tools.aws("source", "get-object", "uploads", "source/file", self.work / "download")
            command = run.call_args.args[0]
            self.assertNotIn("access-fixture", command)
            self.assertNotIn("secret-fixture", command)
            self.assertNotIn("AWS_PROFILE", run.call_args.kwargs["env"])
            for operation, key in (("list-objects", "source/file"), ("get-object", "other/file")):
                with self.assertRaises(ValueError):
                    tools.aws("source", operation, "uploads", key, self.work / "new")
            existing = self.work / "exists"
            existing.write_bytes(b"keep")
            with self.assertRaisesRegex(ValueError, "不能覆盖"):
                tools.aws("source", "get-object", "uploads", "source/file", existing)
            self.assertEqual(existing.read_bytes(), b"keep")

    def test_object_index_rejects_renamed_keys_cross_resource_files_and_tampering(self):
        root, manifest = stored_backup(self.plan, self.work)
        objects = next(item for item in manifest["objects"] if item["bucket"] == "uploads")
        index_path = root / "objects/uploads/index.json"
        index = object_index(root, objects, manifest["artifacts"])
        for field, value in (("key", "source/other.txt"), ("file", "databases/control.sql"), ("sha256", "f" * 64)):
            original = json.loads(json.dumps(index))
            original["entries"][0][field] = value
            index_path.write_text(json.dumps(original))
            with self.assertRaises(ValueError):
                object_index(root, objects, manifest["artifacts"])


if __name__ == "__main__":
    unittest.main()
