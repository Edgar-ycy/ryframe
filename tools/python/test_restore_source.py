"""共享来源证明的恢复审计、诊断绑定与私有配置行为。"""
import json
import hashlib
from pathlib import Path
import sys
import unittest
import uuid

sys.path.insert(0, str(Path(__file__).resolve().parent))
from workspace_directory import WorkspaceDirectory
import restore_build
import restore_reference
import restore_source_runtime_producer as producer
from restore_reference_fixture import environment


def capture_dynamic_evidence(output: Path) -> None:
    """复现真实采集器每次改变 UUID 的两次存储与四库诊断文件。"""
    def write(name, value):
        path = output / name
        path.parent.mkdir(parents=True, exist_ok=True)
        restore_reference.write_json(path, value)
        return path

    commands = [(["/usr/bin/cat", "/proc/383/stat"], "383 (redis-server) " + " ".join(["S"] + ["0"] * 18 + ["123"])),
                (["/usr/bin/readlink", "-f", "/proc/383/exe"], "/usr/bin/redis-server"),
                (["/usr/bin/sha256sum", "/usr/bin/redis-server"], "a" * 64 + "  /usr/bin/redis-server"),
                (["/usr/bin/readlink", "-f", "/fixture/redis.conf"], "/fixture/redis.conf"),
                (["/usr/bin/sha256sum", "/fixture/redis.conf"], "b" * 64 + "  /fixture/redis.conf")]
    layout = {"process_receipt": {"fixed": True}, "launch_receipt": {"fixed": True},
              "actual_arguments_sha256": "a" * 64, "data_dir": "fixed-data",
              "environment_proof": "固定启动来源", "runtime_transition": {"same": True}}
    for _ in range(2):
        for args, stdout in commands:
            write("redis-kernel-" + uuid.uuid4().hex + ".command.json", {
                "command": ["wsl", "--distribution", "fixture", "--exec", *args],
                "returncode": 0, "error_type": None, "stdout": stdout, "stderr": ""})
        write("storage-layout-" + uuid.uuid4().hex + ".json", layout)
    for key in ("shared-control", "shared", "dedicated-a", "dedicated-b"):
        for check, count in (("identity", 4), ("ownership", 6)):
            for _ in range(count):
                name = "databases/mysql-" + check + "-" + uuid.uuid4().hex
                stdout_path = output / (name + ".stdout")
                stdout_path.parent.mkdir(exist_ok=True)
                stdout_path.write_bytes((check + "\t" + key + "\n").encode())
                stdout = {"path": str(stdout_path), **restore_build.file_digest(stdout_path)}
                write(name + ".json", {"format_version": 1, "kind": "mysql-verification-output",
                    "check": check, "target_key": key, "returncode": 0, "error_type": None,
                    "stderr_bytes": 0, "stderr_sha256": hashlib.sha256(b"").hexdigest(), "stdout_file": stdout})
                write("databases/command-" + uuid.uuid4().hex + ".json", {
                    "command": ["mysql", "--database=" + key, "--execute", check], "returncode": 0,
                    "error_type": None, "stdin": None, "stdout": "", "stderr": "", "stdout_file": stdout})
        for phase in ("before", "after"):
            write("databases/command-" + uuid.uuid4().hex + ".json", {
                "command": ["tenant-data", "target-inventory", "--target", key, "--output",
                            str(output / "databases" / f"{phase}-target-{key}.json")],
                "returncode": 0, "error_type": None, "stdin": None, "stdout": "只读清单", "stderr": ""})


class SourceEvidenceTests(unittest.TestCase):
    def setUp(self):
        self.backend, self.plan = environment(self)
        self.work = restore_reference.work_directory(self.plan)

    def write(self, name, value):
        path = self.work / name
        restore_reference.write_json(path, value)
        return path

    def test_recovery_audit_accepts_only_same_control_query_command_records(self):
        directory = self.work / "verification"
        original = directory / "audit" / ("mysql-shared-control-" + "a" * 32 + ".command.json")
        original.parent.mkdir(parents=True)
        value = {"command": [sys.executable, "mysql-fixture"], "returncode": 0,
                 "error_type": None, "stdout": "rows", "stderr": ""}
        restore_reference.write_json(original, value)
        inputs = [{"path": original.relative_to(directory).as_posix(),
                   **restore_build.file_digest(original)}]
        later = original.parent / ("mysql-shared-control-" + "c" * 32 + ".command.json")
        restore_reference.write_json(later, value)
        self.assertEqual(len(producer._verify_inputs(directory, inputs)), 2)
        restore_reference.write_json(directory / "source-verification-recovery-intent.json", {"fixed": True})
        recovery = directory / "recovery-audit" / ("mysql-shared-control-" + "b" * 32 + ".command.json")
        recovery.parent.mkdir()
        restore_reference.write_json(recovery, value)
        observed = producer._verify_inputs(directory, inputs)
        self.assertEqual(len(observed), 4)
        recovery.write_text(json.dumps({**value, "command": ["other-database"]}), encoding="utf-8")
        with self.assertRaisesRegex(ValueError, "改变了原查询命令"):
            producer._verify_inputs(directory, inputs)
        recovery.write_text(json.dumps(value), encoding="utf-8")
        unknown = recovery.parent / "unregistered.json"
        unknown.write_text("{}", encoding="utf-8")
        with self.assertRaisesRegex(ValueError, "未知文件"):
            producer._verify_inputs(directory, inputs)
        unknown.unlink()
        (directory / "source-verification-recovery-intent.json").unlink()
        with self.assertRaisesRegex(ValueError, "明确恢复 intent"):
            producer._verify_inputs(directory, inputs)

    def test_dynamic_image_records_change_only_uuid_and_output_directory(self):
        directory = self.work / "verification"
        before, after = directory / "before", directory / "after"
        before.mkdir(parents=True)
        after.mkdir()
        capture_dynamic_evidence(before)
        restore_reference.write_json(before / "image.json", {"complete": True})
        inputs = producer._evidence_files(directory)
        capture_dynamic_evidence(after)
        restore_reference.write_json(after / "image.json", {"complete": True})
        self.assertEqual(len(producer._verify_inputs(directory, inputs)), 282)
        commands = list((after / "databases").glob("command-*.json"))
        path = commands[0]
        raw = path.read_bytes()
        value = json.loads(raw)
        value["command"][0] = "different-executable"
        path.write_text(json.dumps(value), encoding="utf-8")
        with self.assertRaisesRegex(ValueError, "改变了原命令"):
            producer._verify_inputs(directory, inputs)
        path.write_bytes(raw)
        diagnostics = list((after / "databases").glob("mysql-identity-*.json"))
        path = diagnostics[0]
        raw = path.read_bytes()
        value = json.loads(raw)
        value["stdout_file"] = json.loads(diagnostics[1].read_text(encoding="utf-8"))["stdout_file"]
        path.write_text(json.dumps(value), encoding="utf-8")
        with self.assertRaisesRegex(ValueError, "类别、目标或错误状态"):
            producer._verify_inputs(directory, inputs)
        path.write_bytes(raw)
        grouped = {}
        for path in commands:
            value = json.loads(path.read_bytes())
            if "stdout_file" in value:
                grouped.setdefault(value["stdout_file"]["sha256"], []).append((path, value))
        first, second = next(values[:2] for values in grouped.values() if len(values) >= 2)
        path, value = second
        raw = path.read_bytes()
        value["stdout_file"] = first[1]["stdout_file"]
        path.write_text(json.dumps(value), encoding="utf-8")
        with self.assertRaisesRegex(ValueError, "未一一绑定"):
            producer._verify_inputs(directory, inputs)
        path.write_bytes(raw)
        missing = list(after.glob("storage-layout-*.json"))[0]
        missing.unlink()
        with self.assertRaisesRegex(ValueError, "改变了原类别"):
            producer._verify_inputs(directory, inputs)

class SourcePrivateConfigurationTests(unittest.TestCase):
    def setUp(self):
        self.backend = Path(__file__).resolve().parents[2]
        temporary = WorkspaceDirectory(dir=self.backend / '.local-tests/python-unit', prefix='source-private-')
        self.addCleanup(temporary.cleanup)
        self.directory = Path(temporary.name) / 'verification'
        self.directory.mkdir()

    def test_native_environment_contains_no_application_secret(self):
        value = producer._native_environment(
            {
                "APP_ENV": "test",
                "RYFRAME_PASSWORD": "secret",
            }
        )
        self.assertEqual(value["APP_ENV"], "test")
        self.assertNotIn("RYFRAME_PASSWORD", value)

    def test_configuration_binding_contains_only_path_and_digest(self):
        directory = self.directory.parent / "private-config"
        directory.mkdir()
        secrets = (
            "private-admin-password",
            "private-access-key",
            "private-secret-key",
            "private-session-token",
        )
        (directory / "app.toml").write_text("[application]\nname='unit'\n", encoding="utf-8")
        (directory / "app.test.toml").write_text(
            "[bootstrap]\nadmin_password='private-admin-password'\n"
            "[object_storage]\naccess_key='private-access-key'\n"
            "secret_key='private-secret-key'\nsession_token='private-session-token'\n",
            encoding="utf-8",
        )
        from restore_reference_plan import configuration_binding, validate_configuration_binding
        value = configuration_binding(self.backend, directory, "test")
        encoded = json.dumps(value, ensure_ascii=False, sort_keys=True)
        self.assertEqual(set(value), {"app_env", "directory", "files", "sha256"})
        self.assertEqual(set(value["files"]), {"app.toml", "app.test.toml"})
        for secret in secrets:
            self.assertNotIn(secret, encoded)
        invalid = {**value, "secret_key": "private-secret-key"}
        with self.assertRaisesRegex(ValueError, "参考证据参数字段缺失或包含未知字段") as failure:
            validate_configuration_binding(invalid, self.backend)
        self.assertNotIn("private-secret-key", str(failure.exception))


if __name__ == "__main__":
    unittest.main()
