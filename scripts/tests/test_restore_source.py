"""当前来源和比较业务派发；私有协议与公开参数由专门测试覆盖。"""
import json
import hashlib
from pathlib import Path
import subprocess
import sys
import unittest
import uuid
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import restore_build
import restore_reference
import restore_source as source
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


class SourceDispatchTests(unittest.TestCase):
    def setUp(self):
        self.backend, self.plan = environment(self)
        self.work = restore_reference.work_directory(self.plan)

    def write(self, name, value):
        path = self.work / name
        restore_reference.write_json(path, value)
        return path

    def test_source_request_delegates_only_generation_receipt(self):
        start = self.work / "results/start.json"
        output = self.work / "g0001/verification/source-runtime.json"
        request = source.SourceRequest("verify", {
            "backend_dir": self.backend,
            "source_generation": start,
            "output": output,
            "write": True,
        })
        result = {"output": str(output), "status": "source_runtime_verified"}
        with patch.object(
                source, "execute_source_verification", return_value=result) as verify, \
                patch("builtins.print") as printed:
            source.main(request)
        verify.assert_called_once_with(self.backend.resolve(), start, output)
        self.assertEqual(json.loads(printed.call_args.args[0]), result)

    def test_request_failure_preserves_no_lock_or_success_receipt(self):
        start = self.work / "results/start.json"
        output = self.work / "g0001/verification/source-runtime.json"
        request = source.SourceRequest("verify", {
            "backend_dir": self.backend,
            "source_generation": start,
            "output": output,
            "write": True,
        })
        failure = subprocess.CalledProcessError(1, ["node"], stderr=b"preserved protocol failure")
        with patch.object(
                source, "execute_source_verification", side_effect=failure
        ), self.assertRaises(subprocess.CalledProcessError):
            source.main(request)
        self.assertFalse(output.exists())
        self.assertFalse((self.work / ".reference-lock").exists())

    def test_recovery_dispatch_preserves_the_original_generation_and_output(self):
        start = self.work / "results/start.json"
        output = self.work / "g0001/verification/source-runtime.json"
        request = source.SourceRequest("verify-recover", {
            "backend_dir": self.backend, "source_generation": start,
            "output": output, "write": True,
        })
        result = {"output": str(output), "status": "source_runtime_verified"}
        with patch.object(source, "execute_source_verification_recovery", return_value=result) as recover, \
                patch.object(source, "execute_source_verification") as ordinary:
            self.assertEqual(source.execute(request), result)
        recover.assert_called_once_with(self.backend.resolve(), start, output)
        ordinary.assert_not_called()

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

    def test_comparison_capture_binds_the_exact_export_result(self):
        export = self.write("source-export-result.json", {"published": True})
        output = self.work / "comparison-sources.json"
        roots = {
            "b0-backend": self.backend / "b0-backend",
            "b0-adapter-backend": self.backend / "b0-adapter",
            "b0-frontend": self.backend / "b0-frontend",
            "b1-backend": self.backend / "b1-backend",
            "b1-frontend": self.backend / "b1-frontend",
        }
        builds = {
            "b0-backend-build": self.backend / "b0-backend.json",
            "b0-frontend-build": self.backend / "b0-frontend.json",
            "b1-backend-build": self.backend / "b1-backend.json",
            "b1-frontend-build": self.backend / "b1-frontend.json",
        }
        values = {
            name.replace("-", "_"): path
            for name, path in {**roots, **builds}.items()
        }
        values.update({
            "backend_dir": self.backend,
            "source_export_result": export,
            "output": output,
            "write": True,
        })
        request = source.SourceRequest("comparison-capture", values)
        receipt = {"format_version": 1, "kind": "restore-comparison-sources"}
        with patch.object(
            source, "validate_new_output", return_value=output
        ) as validate, patch.object(
            source, "capture_comparison_sources", return_value=receipt
        ) as capture, patch.object(
            source, "write_new"
        ) as write, patch("builtins.print"):
            source.main(request)
        validate.assert_called_once_with(output, self.backend)
        self.assertEqual(capture.call_args.kwargs["source_export_result"], {
            "path": str(export), **restore_build.file_digest(export),
        })
        write.assert_called_once_with(output, receipt, self.backend)

    def test_comparison_verify_is_read_only(self):
        receipt = self.write("comparison-sources.json", {
            "format_version": 1, "kind": "restore-comparison-sources",
        })
        result = {"source_export": {"identity_sha256": "a" * 64}}
        request = source.SourceRequest("comparison-verify", {
            "backend_dir": self.backend,
            "receipt": receipt,
            "write": False,
        })
        with patch.object(source, "verify_comparison_sources", return_value=result) as verify, \
                patch.object(source, "write_new") as write, patch("builtins.print"):
            source.main(request)
        verify.assert_called_once_with(
            self.backend, json.loads(receipt.read_text(encoding="utf-8"))
        )
        write.assert_not_called()



if __name__ == "__main__":
    unittest.main()
