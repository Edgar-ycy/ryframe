"""开发复制维护工具绑定测试；用 Cargo 事件替身，不编译或连接外部服务。"""
import json
from pathlib import Path
import subprocess
import sys
import unittest
from workspace_directory import WorkspaceDirectory
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import devex_clone_tools as tools

REPO = Path(__file__).resolve().parents[2]


class CloneToolsTests(unittest.TestCase):
    def setUp(self):
        parent = REPO / ".local-tests/python-unit"
        parent.mkdir(parents=True, exist_ok=True)
        temporary = WorkspaceDirectory(dir=parent)
        self.addCleanup(temporary.cleanup)
        self.backend = Path(temporary.name).resolve()
        self.directory = self.backend / ".local-tests/build-tools"
        self.directory.parent.mkdir()
        self.source = {"head": "a" * 40, "clean": False, "files": {"Cargo.toml": "a" * 64}}
        self.snapshot = patch.object(tools, "source_snapshot", return_value=self.source).start()
        patch.object(tools, "capture_inventory", side_effect=lambda _root, source: {
            "source": {"snapshot": source, "worktree_fingerprint": "sha256:" + "b" * 64}, "files": []}).start()
        patch.object(tools, "reusable_artifact_source", return_value=None).start()
        self.fingerprint = patch.object(tools, "worktree_fingerprint", return_value="sha256:" + "b" * 64).start()
        self.addCleanup(patch.stopall)
        self.calls = []
        self.artifact_mode = "valid"

    def cargo(self, args, **kwargs):
        self.calls.append(args)
        if args[0] == "rustc":
            return subprocess.CompletedProcess(args, 0, stdout=b"rustc 1.98.0\nhost: test", stderr=b"")
        if args == ["cargo", "-V"]:
            return subprocess.CompletedProcess(args, 0, stdout=b"cargo 1.98.0", stderr=b"")
        name = args[args.index("--bin") + 1]
        executable = tools.target_directory(self.backend) / "debug" / (name + ".exe")
        executable.parent.mkdir(parents=True, exist_ok=True)
        executable.write_bytes(name.encode())
        event = {"reason": "compiler-artifact", "target": {"name": name, "kind": ["bin"]},
                 "manifest_path": str(self.backend / "crates/ryframe/Cargo.toml"), "executable": str(executable)}
        if self.artifact_mode == "foreign":
            event["manifest_path"] = str(self.backend / "foreign/Cargo.toml")
        kwargs["stdout"].write((json.dumps(event) + "\n").encode())
        if self.artifact_mode == "duplicate":
            kwargs["stdout"].write((json.dumps(event) + "\n").encode())
        kwargs["stderr"].write(b"fixture build log\n")
        if self.artifact_mode == "failure":
            raise subprocess.CalledProcessError(1, args)
        return subprocess.CompletedProcess(args, 0)

    def build(self):
        return tools.build(self.backend, self.directory, self.cargo)

    def verify(self):
        return tools.verify(self.backend, self.directory / "build.json", self.cargo)

    def replace_receipt(self, change):
        filename = self.directory / "build.json"
        value = json.loads(filename.read_text(encoding="utf-8"))
        change(value)
        filename.write_text(json.dumps(value), encoding="utf-8")

    def test_dirty_build_freezes_exact_tools_and_verify_never_builds_or_calls_service(self):
        receipt = self.build()
        self.assertFalse(receipt["source"]["snapshot"]["clean"])
        self.assertFalse(receipt["restore_qualified"])
        self.assertEqual(set(receipt["artifacts"]), {"reset", "migrate", "tenant-data"})
        for item in receipt["artifacts"].values():
            self.assertEqual(Path(item["executable"]).parent, self.directory)
            Path(item["cargo_executable"]).write_bytes(b"later unrelated build")
        self.calls.clear()
        before = {p.name: p.read_bytes() for p in self.directory.iterdir()}
        self.assertEqual(self.verify(), receipt)
        self.assertEqual(before, {p.name: p.read_bytes() for p in self.directory.iterdir()})
        self.assertEqual(self.calls, [["rustc", "-Vv"], ["cargo", "-V"]])

    def test_output_reuse_is_rejected_before_cargo(self):
        self.build()
        self.calls.clear()
        with self.assertRaises(ValueError):
            self.build()
        self.assertEqual(self.calls, [])

    def test_removed_cargo_cache_does_not_invalidate_frozen_verified_tools(self):
        receipt = self.build()
        for artifact in receipt["artifacts"].values():
            Path(artifact["cargo_executable"]).unlink()
        self.assertEqual(self.verify(), receipt)

    def test_toolchain_probe_failure_preserves_diagnostic_and_no_success(self):
        def failed(args, **kwargs):
            raise subprocess.CalledProcessError(1, args, output=b"probe stopped", stderr=b"secret-fixture")
        with patch.dict("os.environ", {"APP_DATABASE_PASSWORD": "secret-fixture"}):
            with self.assertRaises(subprocess.CalledProcessError):
                tools.build(self.backend, self.directory, failed)
        failure = json.loads((self.directory / "failed.json").read_text(encoding="utf-8"))
        self.assertEqual(failure["stage"], "toolchain")
        self.assertEqual(failure["stdout"], "probe stopped")
        self.assertNotIn("secret-fixture", failure["stderr"])
        self.assertIn("REDACTED", failure["stderr"])
        self.assertFalse((self.directory / "build.json").exists())

    def test_source_probe_failure_preserved_before_toolchain(self):
        self.snapshot.side_effect = ValueError("invalid repository")
        with self.assertRaises(ValueError):
            self.build()
        self.assertEqual(self.calls, [])
        failure = json.loads((self.directory / "failed.json").read_text(encoding="utf-8"))
        self.assertEqual(failure["stage"], "source_binding")
        self.assertIsNone(failure["source"])

    def test_readonly_cargo_event_binding_requires_unique_current_workspace_artifact(self):
        for path, manifest, duplicate in (("relative.exe", "current", False),
                                          (str(self.backend / "x.exe"), "foreign", False),
                                          (str(self.backend / "x.exe"), "current", True)):
            event = {"reason": "compiler-artifact", "target": {"name": "ryframe-migrate", "kind": ["bin"]},
                     "manifest_path": str(self.backend / ("crates/ryframe/Cargo.toml" if manifest == "current" else "other/Cargo.toml")),
                     "executable": path}
            content = (json.dumps(event) + "\n") * (2 if duplicate else 1)
            with self.subTest(path=path, manifest=manifest, duplicate=duplicate), self.assertRaises(ValueError):
                tools.recorded_cargo_path(self.backend, "ryframe-migrate", content)

    def test_foreign_or_duplicate_cargo_events_preserve_failure_without_receipt(self):
        for mode in ("foreign", "duplicate", "failure"):
            with self.subTest(mode=mode):
                self.directory = self.directory.parent / mode
                self.artifact_mode = mode
                with self.assertRaises((ValueError, subprocess.CalledProcessError)):
                    self.build()
                self.assertFalse((self.directory / "build.json").exists())
                self.assertTrue((self.directory / "failed.json").is_file())
                self.assertTrue((self.directory / "reset.cargo.log").is_file())

    def test_source_change_during_build_rejects_success(self):
        changed = {**self.source, "files": {"Cargo.toml": "c" * 64}}
        self.snapshot.side_effect = [self.source, changed]
        with self.assertRaisesRegex(ValueError, "构建期间"):
            self.build()
        self.assertFalse((self.directory / "build.json").exists())

    def test_same_head_dirty_change_rejects_old_tool_receipt(self):
        self.build()
        self.fingerprint.return_value = "sha256:" + "c" * 64
        with self.assertRaisesRegex(ValueError, "源码和工具链"):
            self.verify()

    def test_binary_or_cargo_event_tampering_rejected(self):
        self.build()
        for name in ("ryframe-migrate.exe", "migrate.cargo.jsonl", "migrate.cargo.log"):
            with self.subTest(name=name):
                path = self.directory / name
                old = path.read_bytes()
                path.write_bytes(old + b"tampered")
                with self.assertRaises(ValueError):
                    self.verify()
                path.write_bytes(old)

    def test_role_feature_path_and_failure_receipt_rejected(self):
        self.build()
        filename = self.directory / "build.json"
        original = filename.read_bytes()
        changes = [lambda v: v["artifacts"].pop("migrate"),
                   lambda v: v["artifacts"]["migrate"]["command"].append("--features=bin-api"),
                   lambda v: v["artifacts"]["migrate"]["cargo_log"].update(file="../other.log"),
                   lambda v: v.update(restore_qualified=True)]
        for change in changes:
            with self.subTest(change=change):
                self.replace_receipt(change)
                with self.assertRaises(ValueError):
                    self.verify()
                filename.write_bytes(original)
        (self.directory / "failed.json").write_text("{}")
        with self.assertRaises(ValueError):
            self.verify()

    def test_wrong_toolchain_and_source_change_during_verify_fail_closed(self):
        self.build()
        receipt = json.loads((self.directory / "build.json").read_text())
        self.replace_receipt(lambda v: v["toolchain"].update(rustc="other"))
        with self.assertRaises(ValueError):
            self.verify()
        (self.directory / "build.json").write_text(json.dumps(receipt))
        self.fingerprint.side_effect = ["sha256:" + "b" * 64, "sha256:" + "d" * 64]
        with self.assertRaisesRegex(ValueError, "校验维护工具期间"):
            self.verify()

    def test_linked_target_and_nonlocal_output_rejected_before_build(self):
        with patch.object(tools, "linked", return_value=True), self.assertRaises(ValueError):
            self.build()
        self.directory = self.backend / "outside"
        with self.assertRaises(ValueError):
            self.build()
        self.assertEqual(self.calls, [])


if __name__ == "__main__":
    unittest.main()
