"""维护工具读侧验证测试；使用真实 Git 来源与普通冻结文件夹具。"""
import json
from pathlib import Path
import subprocess
import sys
import unittest

from workspace_directory import WorkspaceDirectory

sys.path.insert(0, str(Path(__file__).resolve().parent))
import devex_clone_tools as tools
from source_inventory import capture_inventory, source_snapshot

REPO = Path(__file__).resolve().parents[2]


class CloneToolsTests(unittest.TestCase):
    def setUp(self):
        temporary = WorkspaceDirectory(dir=REPO / ".local-tests/python-unit")
        self.addCleanup(temporary.cleanup)
        self.backend = Path(temporary.name).resolve()
        files = {
            ".gitignore": ".local-tests/\ntarget/\n",
            "Cargo.toml": "[workspace]\n",
            "Cargo.lock": "version = 4\n",
            "crates/ryframe/Cargo.toml": "[package]\nname='ryframe'\n",
            "product/input.txt": "product\n",
        }
        for name, content in files.items():
            path = self.backend / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(content, encoding="utf-8")
        self.git("init", "--quiet")
        self.git("add", ".")
        self.git("commit", "--quiet", "-m", "test(recovery): 初始化维护验证夹具")
        self.head = self.git("rev-parse", "HEAD").decode().strip()
        self.product = self.backend / "product/input.txt"
        self.directory = self.backend / ".local-tests/build-tools"
        self.directory.mkdir(parents=True)
        self.filename = self.directory / "build.json"
        self.version_commands = []
        self.change_on_version = False
        self.receipt = self.frozen_receipt()
        self.assertEqual(self.receipt["source"]["snapshot"], source_snapshot(self.backend))
        before = self.raw_files()
        self.assertEqual(tools.verify_evidence(self.backend, self.filename), self.receipt)
        self.assertEqual(self.raw_files(), before)
        self.assertEqual(self.verify_read_only(), self.receipt)
        self.version_commands.clear()

    def git(self, *arguments):
        return subprocess.check_output([
            "git", "-C", str(self.backend), "-c", "core.autocrlf=false",
            "-c", "core.hooksPath=" + str(self.backend / ".local-tests/no-hooks"),
            "-c", "commit.gpgSign=false", "-c", "user.name=RyFrame fixture",
            "-c", "user.email=fixture@example.invalid", *arguments,
        ])

    def versions(self, args, **kwargs):
        self.assertEqual(kwargs["cwd"], self.backend)
        if args == ["rustc", "-Vv"]:
            output = b"rustc fixture\nhost: test"
        elif args == ["cargo", "-V"]:
            output = b"cargo fixture"
            if self.change_on_version:
                self.product.write_bytes(b"changed during verification\n")
                self.change_on_version = False
        else:
            self.fail(f"读侧验证不得运行构建或服务命令：{args}")
        self.version_commands.append(args)
        return subprocess.CompletedProcess(args, 0, stdout=output, stderr=b"")

    def frozen_receipt(self):
        source = tools.source_binding(self.backend)
        inventory = capture_inventory(self.backend, source["snapshot"])
        target = tools.target_directory(self.backend)
        artifacts = {}
        for role, (_, name) in tools.TOOLS.items():
            frozen = self.directory / (name + ".exe")
            frozen.write_bytes(("frozen " + role).encode())
            cache = target / "debug" / frozen.name
            cache.parent.mkdir(parents=True, exist_ok=True)
            cache.write_bytes(frozen.read_bytes())
            output, log = self.directory / (role + ".cargo.jsonl"), self.directory / (role + ".cargo.log")
            event = {"reason": "compiler-artifact", "target": {"name": name, "kind": ["bin"]},
                     "manifest_path": str(self.backend / "crates/ryframe/Cargo.toml"),
                     "executable": str(cache)}
            output.write_text(json.dumps(event) + "\n", encoding="utf-8")
            log.write_bytes(b"fixture build log\n")
            artifacts[role] = {
                "executable": str(frozen), "cargo_executable": str(cache),
                "command": tools.command(self.backend, role, target),
                "cargo_output": {"file": output.name, **tools.file_digest(output)},
                "cargo_log": {"file": log.name, **tools.file_digest(log)},
                **tools.file_digest(frozen),
            }
        receipt = {
            "format_version": 1, "kind": "devex-clone-tool-build", "backend_root": str(self.backend),
            "source": source, "source_inventory": inventory,
            "toolchain": tools.toolchain(self.backend, self.versions),
            "target_directory": str(target), "artifacts": artifacts,
            "restore_qualified": False, "resources_modified": False,
        }
        self.filename.write_text(json.dumps(receipt), encoding="utf-8")
        return receipt

    def raw_files(self):
        return {path.relative_to(self.backend).as_posix(): path.read_bytes()
                for path in self.backend.rglob("*")
                if ".git" not in path.relative_to(self.backend).parts and path.is_file()}

    def verify_read_only(self):
        before = self.raw_files()
        result = tools.verify(self.backend, self.filename, self.versions)
        self.assertEqual(self.raw_files(), before)
        return result

    def assert_rejected(self, pattern, expected_change=None):
        before = self.raw_files()
        if expected_change is not None:
            before["product/input.txt"] = expected_change
        with self.assertRaisesRegex(ValueError, pattern):
            tools.verify(self.backend, self.filename, self.versions)
        self.assertEqual(self.raw_files(), before)

    def replace_receipt(self, change):
        value = json.loads(self.filename.read_text(encoding="utf-8"))
        change(value)
        self.filename.write_text(json.dumps(value), encoding="utf-8")

    def test_verify_is_readonly_and_independent_of_replaced_or_removed_cargo_cache(self):
        for state in ("replaced", "removed"):
            with self.subTest(state=state):
                for item in self.receipt["artifacts"].values():
                    cache = Path(item["cargo_executable"])
                    if state == "replaced":
                        cache.write_bytes(b"later unrelated build")
                    else:
                        cache.unlink()
                self.version_commands.clear()
                self.assertEqual(self.verify_read_only(), self.receipt)
                self.assertEqual({tuple(command) for command in self.version_commands},
                                 {("rustc", "-Vv"), ("cargo", "-V")})

    def test_same_head_dirty_change_rejects_old_tool_receipt(self):
        self.product.write_bytes(b"changed tracked product\n")
        self.assertEqual(self.git("rev-parse", "HEAD").decode().strip(), self.head)
        self.assert_rejected("产品构建输入已经变化")

    def test_binary_or_cargo_event_tampering_rejected(self):
        for name in ("ryframe-migrate.exe", "migrate.cargo.jsonl", "migrate.cargo.log"):
            with self.subTest(name=name):
                path = self.directory / name
                original = path.read_bytes()
                path.write_bytes(original + b"tampered")
                self.assert_rejected("实际二进制|构建日志发生变化")
                path.write_bytes(original)

    def test_role_feature_path_and_failure_receipt_rejected(self):
        original = self.filename.read_bytes()
        changes = [
            ("role", lambda v: v["artifacts"].pop("migrate")),
            ("command", lambda v: v["artifacts"]["migrate"]["command"].append("--features=bin-api")),
            ("logpath", lambda v: v["artifacts"]["migrate"]["cargo_log"].update(file="../other.log")),
            ("purpose", lambda v: v.update(restore_qualified=True)),
        ]
        for name, change in changes:
            with self.subTest(name=name):
                self.replace_receipt(change)
                self.assert_rejected("格式或用途|定向构建命令|日志路径")
                self.filename.write_bytes(original)
        (self.directory / "failed.json").write_bytes(b"{}")
        self.assert_rejected("构建失败或收据名称不匹配")

    def test_wrong_toolchain_and_source_change_during_verify_fail_closed(self):
        original = self.filename.read_bytes()
        self.replace_receipt(lambda v: v["toolchain"].update(rustc="other"))
        self.assert_rejected("源码和工具链")
        self.filename.write_bytes(original)
        self.assertEqual(self.verify_read_only(), self.receipt)
        self.change_on_version = True
        self.assert_rejected("校验维护工具期间源码或收据变化", b"changed during verification\n")
        self.assertFalse(self.change_on_version)
        self.assertEqual(self.git("rev-parse", "HEAD").decode().strip(), self.head)


if __name__ == "__main__":
    unittest.main()
